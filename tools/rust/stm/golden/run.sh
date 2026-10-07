#!/usr/bin/env bash
# Writes the goldens of the C++ glue_system scenarios into software_stm32_rust/glue/tests/golden/
# (docs/rust/GLUE-DESIGN-STM.md §7.4). Runs in the native container, repo at /src, build volume
# at /build:
#
#   tools/native/docker.sh run "bash tools/rust/stm/golden/run.sh"
#
# Every case of glue_system must pass: a failed C++ assertion stops the reboots of its case
# (testkit), so its golden would be incomplete.
set -euo pipefail
cd /src
build=/build/stm-golden
out=software_stm32_rust/glue/tests/golden
cmake -S tools/rust/stm/golden -B "$build" -G Ninja -DCMAKE_BUILD_TYPE=Release >/dev/null
cmake --build "$build" -j "${VDM_JOBS:-2}"
rm -rf "$out"
mkdir -p "$out"
VDM_GOLDEN_DIR="$(pwd)/$out" VDM_CASE_TIMEOUT_S=120 "$build/glue_system_golden"
cases=$(ls "$out"/*.txt | wc -l)
expected=$(cat software_stm32/test/native/glue/test_system_*.cpp | grep -c '^TEST_CASE')
echo "goldens: $cases files in $out ($expected glue_system cases)"
[ "$cases" -eq "$expected" ] || { echo "a golden per glue_system case expected" >&2; exit 1; }
