#!/usr/bin/env bash
# Native (host) test suites and the mutation tool in a Linux container, so they run the
# same on Windows, macOS and Linux as in CI (ASan/UBSan and mutate.py need Linux).
#
#   tools/native/docker.sh image                      build the image (done on first use)
#   tools/native/docker.sh test [stm32|esp32|all]     configure, build and run the native tests
#   tools/native/docker.sh mutate <config> [args...]  tools/mutation/mutate.py --config tools/mutation/<config>.json
#   tools/native/docker.sh run <command...>           any command in the container, repo at /src
#
# Environment: VDM_JOBS (build parallelism, default 2), VDM_MUTATION_JOBS (default 6),
# VDM_NATIVE_IMAGE (default vdmot-native:local).
set -euo pipefail

IMAGE="${VDM_NATIVE_IMAGE:-vdmot-native:local}"
ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
HOST_ROOT="$ROOT"
if command -v cygpath >/dev/null 2>&1; then HOST_ROOT="$(cygpath -m "$ROOT")"; fi
# One build volume per checkout: incremental builds survive between runs, worktrees don't collide.
VOLUME="vdmot-build-$(printf '%s' "$HOST_ROOT" | md5sum | cut -c1-12)"
JOBS="${VDM_JOBS:-2}"
export MSYS_NO_PATHCONV=1

build_image() {
  docker build -q -t "$IMAGE" -f "$ROOT/tools/native/Dockerfile" "$ROOT/tools/native" >/dev/null
}

in_container() {
  docker image inspect "$IMAGE" >/dev/null 2>&1 || build_image
  docker run --rm -v "$HOST_ROOT:/src" -v "$VOLUME:/build" -w /src "$IMAGE" bash -c "$1"
}

native_test() {
  local project="$1"
  in_container "set -e
    cmake -S $project/test/native -B /build/$project -DCMAKE_BUILD_TYPE=Debug >/dev/null
    cmake --build /build/$project -j $JOBS
    ctest --test-dir /build/$project --output-on-failure"
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
    in_container "python3 tools/mutation/mutate.py --config tools/mutation/$config.json --jobs ${VDM_MUTATION_JOBS:-6} $*"
    ;;
  run)
    shift
    in_container "$*"
    ;;
  *)
    sed -n '2,11p' "$0" >&2
    exit 2
    ;;
esac
