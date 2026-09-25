#!/usr/bin/env bash
# Native (host) test suites and the mutation tool in a Linux container, so they run the
# same on Windows, macOS and Linux as in CI (ASan/UBSan and mutate.py need Linux).
#
#   tools/native/docker.sh image                      build the image (done on first use)
#   tools/native/docker.sh test [stm32|esp32|all]     configure, build and run the native tests
#   tools/native/docker.sh mutate <config> [args...]  tools/mutation/mutate.py --config tools/mutation/<config>.json
#   tools/native/docker.sh mutate-all [args...]       stm32, stm32-glue, esp32, esp32-glue and one summary table
#   tools/native/docker.sh run <command...>           any command in the container, repo at /src
#
# Environment: VDM_JOBS (build parallelism, default 2), VDM_MUTATION_JOBS (default 6),
# VDM_NATIVE_IMAGE (default vdmot-native:<hash of the Dockerfile>).
#
# Each checkout (git worktree) has its own build volume, so parallel worktrees never share build
# directories; test runs of one project in one checkout wait for each other. Mutation runs of
# all checkouts share one lock (volume vdmot-locks): a second run waits until the first ends.
set -euo pipefail

ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
HOST_ROOT="$ROOT"
if command -v cygpath >/dev/null 2>&1; then HOST_ROOT="$(cygpath -m "$ROOT")"; fi
# The tag follows the Dockerfile, so a toolchain change builds a new image and older
# checkouts keep theirs.
IMAGE="${VDM_NATIVE_IMAGE:-vdmot-native:$(md5sum "$ROOT/tools/native/Dockerfile" | cut -c1-12)}"
VOLUME="vdmot-build-$(printf '%s' "$HOST_ROOT" | md5sum | cut -c1-12)"
LOCKS_VOLUME="vdmot-locks"
JOBS="${VDM_JOBS:-2}"
MUTATION_JOBS="${VDM_MUTATION_JOBS:-6}"
export MSYS_NO_PATHCONV=1

# Shell-quoted arguments for a container command line (nothing for no arguments).
quote_args() {
  [ $# -eq 0 ] || printf ' %q' "$@"
}

build_image() {
  docker build -q -t "$IMAGE" -f "$ROOT/tools/native/Dockerfile" "$ROOT/tools/native" >/dev/null
}

in_container() {
  docker image inspect "$IMAGE" >/dev/null 2>&1 || build_image
  # --init: Ctrl-C and docker stop reach the tools (bash as PID 1 would ignore them).
  docker run --rm --init -v "$HOST_ROOT:/src" -v "$VOLUME:/build" -v "$LOCKS_VOLUME:/locks" -w /src \
    -e PYTHONDONTWRITEBYTECODE=1 -e CCACHE_DIR=/build/ccache -e CCACHE_BASEDIR=/tmp \
    -e CCACHE_NOHASHDIR=1 -e CCACHE_MAXSIZE=2G "$IMAGE" bash -c "$1"
}

# Container script: take the global mutation lock (fd 9 stays open until the container ends).
mutation_lock() {
  cat <<EOS
exec 9>/locks/mutate.lock
if ! flock -n 9; then
  echo "waiting for the mutation lock, held by: \$(cat /locks/mutate.owner 2>/dev/null || echo another run)" >&2
  flock 9
  echo "mutation lock acquired" >&2
fi
echo "$HOST_ROOT (since \$(date -u +%FT%TZ))" > /locks/mutate.owner
EOS
}

# Container script of mutate-all; expects MUTATION_JOBS and the mutate.py arguments in "$@".
mutate_all_script() {
  cat <<'EOS'
rc_all=0
rows=""
for s in stm32 stm32-glue esp32 esp32-glue; do
  start=$SECONDS
  rm -f "tools/mutation/$s.report.json"
  if [ -f "tools/mutation/$s.json" ]; then
    set +e
    python3 tools/mutation/mutate.py --config "tools/mutation/$s.json" --jobs "$MUTATION_JOBS" "$@"
    rc=$?
    set -e
  else
    echo "tools/mutation/$s.json not found" >&2
    rc=2
  fi
  [ "$rc" -eq 0 ] || rc_all=1
  rows="$rows$s $rc $((SECONDS - start))
"
done
echo
echo "## Mutation gate"
echo
echo "| suite | result | score | worst file | minutes |"
echo "|---|---|---|---|---|"
printf '%s' "$rows" | while read -r s rc secs; do
  python3 - "$s" "$rc" "$secs" <<'PY'
import json, sys
suite, rc, secs = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
result = {0: "passed", 1: "below threshold"}.get(rc, "error (exit %d)" % rc)
score = worst = "-"
try:
    with open("tools/mutation/%s.report.json" % suite) as f:
        r = json.load(f)
    score = "%.1f %%" % r["score"]
    files = [(v["score"], k) for k, v in r["files"].items() if v["score"] is not None]
    if files:
        s, name = min(files)
        worst = "%s %.1f %%" % (name, s)
except (OSError, ValueError, KeyError):
    pass
print("| %s | %s | %s | %s | %.1f |" % (suite, result, score, worst, secs / 60.0))
PY
done
exit "$rc_all"
EOS
}

native_test() {
  local project="$1" pre=""
  if [ "$project" = software_stm32 ] && [ -f "$ROOT/tools/native/check_release_flags.py" ]; then
    pre="python3 tools/native/check_release_flags.py"
  fi
  in_container "set -e
    exec 8>/build/$project.lock
    if ! flock -n 8; then echo 'waiting for another test run of $project in this checkout' >&2; flock 8; fi
    $pre
    b=/build/$project
    # build directories of older versions of this script used Makefiles
    if [ -f \$b/CMakeCache.txt ] && ! grep -q '^CMAKE_GENERATOR:INTERNAL=Ninja\$' \$b/CMakeCache.txt; then rm -rf \$b; fi
    cmake -S $project/test/native -B \$b -G Ninja -DCMAKE_BUILD_TYPE=Debug >/dev/null
    cmake --build \$b -j $JOBS
    ctest --test-dir \$b --output-on-failure -j $JOBS"
}

case "${1:-}" in
  image)
    build_image
    ;;
  test)
    case "${2:-all}" in
      stm32) native_test software_stm32 ;;
      esp32) native_test software_esp32_revamped ;;
      all) native_test software_stm32 && native_test software_esp32_revamped ;;
      *) echo "unknown suite: $2 (stm32, esp32, all)" >&2; exit 2 ;;
    esac
    ;;
  mutate)
    [ $# -ge 2 ] || { echo "usage: $0 mutate <config> [mutate.py args]" >&2; exit 2; }
    config="$2"; shift 2
    in_container "$(mutation_lock)
python3 tools/mutation/mutate.py --config tools/mutation/$config.json --jobs $MUTATION_JOBS$(quote_args "$@")"
    ;;
  mutate-all)
    shift
    in_container "$(mutation_lock)
MUTATION_JOBS=$MUTATION_JOBS
set --$(quote_args "$@")
$(mutate_all_script)"
    ;;
  run)
    shift
    in_container "$*"
    ;;
  *)
    sed -n '2,16p' "$0" >&2
    exit 2
    ;;
esac
