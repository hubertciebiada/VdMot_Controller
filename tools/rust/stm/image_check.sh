#!/usr/bin/env bash
# Image check C1-C5 and D9 (docs/rust/GLUE-DESIGN-STM.md §5.8) of the four images in
# software_stm32_rust/firmware/images/ (tools/rust/docker.sh fw); runs inside the Rust
# container (tools/rust/docker.sh image-check).
#
# - C1-C3 and the ESP's erase set: the C++ validateImage/checkBoard of
#   software_esp32_revamped/lib/core (what an ESP 2.1.7 runs before it flashes), built with the
#   container's g++ (image-check/esp_validate.cpp), plus the ESP's acceptance of the gvers
#   reply the image gives after the flash
# - C4, C5, D9: vdm-stm-image-check on the ELF and the .bin
set -euo pipefail
cd /src/software_stm32_rust
images=firmware/images
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
echo "workspace version: $version"

CARGO_TARGET_DIR=/target/software_stm32_rust cargo build -q --release -p vdm-stm-image-check
check=/target/software_stm32_rust/release/vdm-stm-image-check

esp=/target/esp_validate
core=/src/software_esp32_revamped/lib/core
g++ -std=c++17 -O1 -Wall -Wextra -Werror -fno-exceptions -fno-rtti -I "$core/include" \
  "$core"/src/*.cpp image-check/esp_validate.cpp -o "$esp"

fail=0
summary=""
for name in STM32F401_C1 STM32F401_C2 STM32F411_C1 STM32F411_C2; do
  chip=$(echo "${name:5:4}" | tr 'F' 'f')
  tag=${name: -2}
  echo
  if "$check" "$images/$name.elf" "$images/$name.bin" "$chip" "$tag" "$version"; then layout=pass; else layout=FAIL; fail=1; fi
  if "$esp" "$images/$name.bin" "$chip" "$tag" "$version" "gvers ${version}_${tag} 1 "; then espv=pass; else espv=FAIL; fail=1; fi
  size=$(stat -c %s "$images/$name.bin")
  pct=$(awk -v s="$size" 'BEGIN { printf "%.1f", s * 100 / 131072 }')
  summary+=$(printf '%-14s %7d B %5s %% of 128 KiB   layout+D9 %-4s  ESP %s' "$name" "$size" "$pct" \
    "$layout" "$espv")$'\n'
done
echo
echo "$summary"
exit $fail
