#!/usr/bin/env bash
# Self-test of the ESP glue harness (ctest: glue_selftest): the passing cases of selftest.cpp,
# then every case of the suite "xfail" alone, which must fail with the expected verdict.
#   selftest.sh <path of the glue_selftest executable>
set -u
BIN="$1"
T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
unset VDM_FAIL_FAST VDM_GLUE_NOFORK VDM_CASE_TIMEOUT_S VDM_MAX_BOOTS

fail() {
  echo "glue selftest FAILED: $*"
  [ -f "$T/out" ] && sed 's/^/  | /' "$T/out"
  exit 1
}

# run <args...>: output in $T/out, exit code in $RC
run() {
  "$BIN" "$@" >"$T/out" 2>&1
  RC=$?
}

run -tse=xfail
[ "$RC" -eq 0 ] || fail "passing cases: exit $RC"
cat "$T/out"

# xfail <case name> <text the output must contain>
xfail() {
  run -ts=xfail -tc="$1"
  [ "$RC" -eq 1 ] || fail "\"$1\": exit $RC, expected 1"
  grep -qF "$2" "$T/out" || fail "\"$1\": no \"$2\" in the output"
  echo "xfail ok: $1"
}

xfail "xfail: an assertion" 'FAILED "xfail: an assertion"'
xfail "xfail: a second take of a held mutex" "second xSemaphoreTake of a held mutex"
xfail "xfail: xTaskGetHandle without a name" "xTaskGetHandle(nullptr)"
xfail "xfail: a critical section left entered" "a critical section is still entered"
xfail "xfail: an HTTP request without an answer" "GET /api/status answered 0 times"
xfail "xfail: an HTTP request answered twice" "GET / answered 2 times"
xfail "xfail: the topic of an MQTT callback is gone after a publish in it" "heap-use-after-free"

# --fail-fast stops at the first failing case.
run --fail-fast -ts=xfail
[ "$RC" -eq 1 ] || fail "--fail-fast: exit $RC, expected 1"
[ "$(grep -c '^\[testkit\] FAILED' "$T/out")" -eq 1 ] ||
  fail "--fail-fast: more than one failing case ran"
grep -q "fail-fast: stopped after the first failing case" "$T/out" || fail "--fail-fast: message"
echo "glue selftest passed"
