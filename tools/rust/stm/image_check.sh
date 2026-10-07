#!/usr/bin/env bash
# Image check C1-C6 and D9 (docs/rust/GLUE-DESIGN-STM.md §5.8) of the four images in
# software_stm32_rust/firmware/images/ (tools/rust/docker.sh fw); runs inside the Rust
# container (tools/rust/docker.sh image-check [<dir>]).
#
#   image_check.sh [<directory of the C++ 2.1.7 release images>]
#
# - C1-C3 and the ESP's erase set: the C++ validateImage/checkBoard of
#   software_esp32_revamped/lib/core (what an ESP 2.1.7 runs before it flashes), built with the
#   container's g++ (image-check/esp_validate.cpp), plus the ESP's acceptance of the gvers
#   reply the image gives after the flash
# - C4, C5, D9: vdm-stm-image-check on the ELF and the .bin
# - C6 (with a directory): the same ESP validation on the C++ release images in it, one
#   *STM32F401_C1.bin ... *STM32F411_C2.bin each, after their SHA-256 matched the values pinned
#   in tools/rust/stm/cpp217.sha256
set -euo pipefail
cpp_dir="${1:-}"
cpp_version=2.1.7-revamped
pins=/src/tools/rust/stm/cpp217.sha256
cd /src/software_stm32_rust
images=firmware/images
# (a checkout with CR LF line ends: the CR is not part of the version)
version=$(tr -d '\r' < Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
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

# C6: the reference, the C++ images through the same ESP code
if [ -n "$cpp_dir" ]; then
  for name in STM32F401_C1 STM32F401_C2 STM32F411_C1 STM32F411_C2; do
    chip=$(echo "${name:5:4}" | tr 'F' 'f')
    tag=${name: -2}
    echo
    files=("$cpp_dir"/*"$name".bin)
    if [ ${#files[@]} -ne 1 ] || [ ! -f "${files[0]}" ]; then
      echo "C6 $name: expected one *$name.bin in $cpp_dir"
      summary+=$(printf 'C6 %-14s %-46s FAIL (missing)' "$name" "-")$'\n'
      fail=1
      continue
    fi
    file=$(basename "${files[0]}")
    want=$(tr -d '\r' < "$pins" | awk -v n="$name" '$2 == n { print $1 }')
    got=$(sha256sum "${files[0]}" | cut -d' ' -f1)
    if [ "$got" != "$want" ]; then
      echo "C6 $file: SHA-256 $got, pinned $want"
      summary+=$(printf 'C6 %-14s %-46s FAIL (SHA-256)' "$name" "$file")$'\n'
      fail=1
      continue
    fi
    echo "C6 $file: SHA-256 $got as pinned"
    if "$esp" "${files[0]}" "$chip" "$tag" "$cpp_version" "gvers ${cpp_version}_${tag} 1 "; then c6=pass; else c6=FAIL; fail=1; fi
    summary+=$(printf 'C6 %-14s %-46s %s' "$name" "$file" "$c6")$'\n'
  done
else
  summary+="C6 not run: no C++ image directory (tools/rust/docker.sh image-check <dir>)"$'\n'
fi
echo
echo "$summary"
exit $fail
