#!/usr/bin/env bash
# Self-test of the fork-per-case runner (ctest: testkit_selftest). Runs the cases of
# selftest.cpp with suite filters and checks the runner's verdicts.
#   selftest.sh <path of the testkit_selftest executable>
set -u
BIN="$1"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
unset VDM_FAIL_FAST VDM_GLUE_NOFORK VDM_CASE_TIMEOUT_S VDM_MAX_BOOTS

fail() {
  echo "testkit selftest FAILED: $*"
  [ -f "$T/out" ] && sed 's/^/  | /' "$T/out"
  exit 1
}

# run <marker dir> <args...>: output in $T/out, exit code in $RC, process id in $PID
run() {
  local dir="$T/$1"
  shift
  mkdir -p "$dir"
  TESTKIT_MARKERS="$dir" "$BIN" "$@" >"$T/out" 2>&1 &
  PID=$!
  wait "$PID"
  RC=$?
}

lines() { [ -f "$1" ] && wc -l <"$1" || echo 0; }

# Isolation (static state starts fresh), multi-boot hand-over (pin reset: boot 1 sees the stores
# and the warm RAM), power-on pattern 0xA5 at boot 0 and after a power-on reboot.
run pass -ts=pass
[ "$RC" -eq 0 ] || fail "suite pass: exit $RC"
grep -q "6 cases, 6 run: 6 passed, 0 failed" "$T/out" || fail "suite pass: summary"

# Fork mode: every case in its own child process.
run fork -ts=pid
[ "$RC" -eq 0 ] || fail "suite pid: exit $RC"
[ "$(sort -u "$T/fork/pid" | wc -l)" -eq 2 ] || fail "fork mode: cases did not run in two processes"
grep -qx "$PID" "$T/fork/pid" && fail "fork mode: a case ran in the runner process"

# --no-fork and VDM_GLUE_NOFORK=1 run every case in the runner process.
run nofork --no-fork -ts=pid
[ "$RC" -eq 0 ] || fail "--no-fork: exit $RC"
[ "$(sort -u "$T/nofork/pid")" = "$PID" ] || fail "--no-fork: cases did not run in the runner process"
VDM_GLUE_NOFORK=1 run nofork_env -ts=pid
[ "$(sort -u "$T/nofork_env/pid")" = "$PID" ] || fail "VDM_GLUE_NOFORK=1: cases did not run in the runner process"
run nofork_static --no-fork -ts=pass -tc="static state*"
[ "$RC" -ne 0 ] || fail "--no-fork: the second case did not see the first case's static state"

# Without fail-fast both failing cases run; with --fail-fast or VDM_FAIL_FAST=1 only the first.
run fail_all -ts=fail
[ "$RC" -ne 0 ] || fail "suite fail: exit 0"
[ "$(lines "$T/fail_all/fail2")" -eq 1 ] || fail "suite fail: the second case did not run"
grep -q "2 cases, 2 run: 0 passed, 2 failed" "$T/out" || fail "suite fail: summary"
run fail_fast --fail-fast -ts=fail
[ "$RC" -ne 0 ] || fail "--fail-fast: exit 0"
[ "$(lines "$T/fail_fast/fail1")" -eq 1 ] || fail "--fail-fast: the first case did not run"
[ -f "$T/fail_fast/fail2" ] && fail "--fail-fast: the second failing case ran"
grep -q "2 cases, 1 run: 0 passed, 1 failed" "$T/out" || fail "--fail-fast: summary"
VDM_FAIL_FAST=1 run fail_fast_env -ts=fail
[ -f "$T/fail_fast_env/fail2" ] && fail "VDM_FAIL_FAST=1: the second failing case ran"

# A case that never ends is stopped by SIGALRM within VDM_CASE_TIMEOUT_S + 1 s.
start=$(date +%s%N)
VDM_CASE_TIMEOUT_S=1 run hang -ts=hang
elapsed_ms=$((($(date +%s%N) - start) / 1000000))
[ "$RC" -ne 0 ] || fail "hanging case: exit 0"
grep -q 'FAILED "a case that never ends" (.*): timeout (signal 14) after 1 s at boot 0' "$T/out" ||
  fail "hanging case: no timeout verdict"
[ "$elapsed_ms" -lt 2000 ] || fail "hanging case: took ${elapsed_ms} ms with VDM_CASE_TIMEOUT_S=1"

# VDM_MAX_BOOTS bounds the boots of a case.
VDM_MAX_BOOTS=3 run boots -ts=boots
[ "$RC" -ne 0 ] || fail "endless reboots: exit 0"
grep -q "too many boots (VDM_MAX_BOOTS 3)" "$T/out" || fail "endless reboots: verdict"
[ "$(lines "$T/boots/boots")" -eq 3 ] || fail "endless reboots: $(lines "$T/boots/boots") boots instead of 3"

# A broken invariant fails a case that has no failed assertion.
run invariant -ts=invariant
[ "$RC" -ne 0 ] || fail "invariant: exit 0"
grep -q 'invariant violated in "a broken invariant fails a case without a failed assertion" at boot 0: 1 RTOS violation' "$T/out" ||
  fail "invariant: message"
grep -q ": invariant violated at boot 0" "$T/out" || fail "invariant: verdict"

# A failed assertion stops the reboot: the case ends at boot 0.
run failreboot -ts=failreboot
[ "$RC" -ne 0 ] || fail "failed assertion before reboot: exit 0"
[ "$(lines "$T/failreboot/failreboot")" -eq 1 ] || fail "failed assertion before reboot: the case rebooted"
grep -q "reboot requested after a failed assertion at boot 0" "$T/out" || fail "failed assertion before reboot: verdict"

echo "testkit selftest OK"
