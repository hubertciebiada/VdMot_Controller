#!/usr/bin/env bash
# Renode tests of the STM32 images (docs/rust/GLUE-DESIGN-STM.md §5.7) in the Docker image
# antmicro/renode:1.16.1 (D11), once per image: software_stm32_rust/renode/boot.robot (the boot
# stage, E1-E10) and app.robot (the application against the C++ goldens, A1-A6).
#
#   tools/rust/docker.sh fw                     first: the images in software_stm32_rust/firmware/images/
#   tools/rust/renode.sh [image...]             all four images, or e.g. STM32F401_C2
#   tools/rust/renode.sh STM32F401_C2 -- --include E1   extra arguments for renode-test after --
#                                               (tags: E1-E10, A1-A6)
#
# Results (robot log, report, Renode logs of failed tests) in
# software_stm32_rust/renode/results/<image>/. Exit code 0 when every image passed.
#
# Environment: VDM_RENODE_IMAGE (default antmicro/renode:1.16.1).
set -euo pipefail

ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
HOST_ROOT="$ROOT"
if command -v cygpath >/dev/null 2>&1; then HOST_ROOT="$(cygpath -m "$ROOT")"; fi
IMAGE="${VDM_RENODE_IMAGE:-antmicro/renode:1.16.1}"
export MSYS_NO_PATHCONV=1

images=()
extra=()
while [ $# -gt 0 ]; do
  if [ "$1" = "--" ]; then shift; extra=("$@"); break; fi
  images+=("$1"); shift
done
[ ${#images[@]} -gt 0 ] || images=(STM32F401_C1 STM32F401_C2 STM32F411_C1 STM32F411_C2)

# (a checkout with CR LF line ends: the CR is not part of the version)
version=$(tr -d '\r' < "$ROOT/software_stm32_rust/Cargo.toml" | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
fail=0
summary=""
for name in "${images[@]}"; do
  map="$ROOT/software_stm32_rust/firmware/images/$name.map"
  [ -f "$map" ] || { echo "$map not found: tools/rust/docker.sh fw" >&2; exit 2; }
  chip=$(echo "${name:5:4}" | tr 'F' 'f')
  tag=${name: -2}
  # the application entry, where E8 raises its fault
  app_run=$(awk '/3app3run\)$/ { print "0x" $1; exit }' "$map")
  [ -n "$app_run" ] || { echo "$name: app::run not in $map" >&2; exit 2; }
  echo "== $name ($chip, $tag, $version, app::run at $app_run)"
  set +e
  docker run --rm --init -v "$HOST_ROOT:/src" -w /src/software_stm32_rust/renode \
    --entrypoint renode-test "$IMAGE" \
    -r "/src/software_stm32_rust/renode/results/$name" \
    --variable "IMAGE:$name" --variable "CHIP:$chip" --variable "TAG:$tag" \
    --variable "VERSION:$version" --variable "APP_RUN:$app_run" \
    "${extra[@]}" boot.robot app.robot
  rc=$?
  set -e
  if [ $rc -eq 0 ]; then result=pass; else result=FAIL; fail=1; fi
  summary+="$name $result"$'\n'
done
echo
printf '%s' "$summary"
exit $fail
