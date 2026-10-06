#!/usr/bin/env bash
# Builds the four STM32 images and the boot probes of software_stm32_rust/firmware; runs inside
# the Rust container (tools/rust/docker.sh fw), repo at /src, target volume at /target.
#
# Output in software_stm32_rust/firmware/images/ (not in git):
#   STM32F401_C1.elf .bin .map ... STM32F411_C2.*   the images (release profile, clippy clean)
# The boot probe (B3, docs/rust/GLUE-DESIGN-STM.md §5.1) is built per chip with
# --no-default-features: it links the boot stage and the fault handlers with a panic handler
# that does not exist, so its link fails while any panic path is left in them.
set -euo pipefail
cd /src/software_stm32_rust/firmware
out=images
mkdir -p "$out"

for chip in f401 f411; do
  for board in c1 c2; do
    name="STM32${chip^^}_${board^^}"
    dir="/target/stm-fw/${chip}_${board}"
    echo "== $name"
    # clippy first: firmware/clippy.toml forbids flash and option byte writes (B4)
    CARGO_TARGET_DIR="$dir" cargo clippy -q --release --features "$chip,$board" -- -D warnings
    CARGO_TARGET_DIR="$dir" cargo build --release --features "$chip,$board"
    elf="$dir/thumbv7em-none-eabihf/release/vdm-stm-fw"
    cp "$elf" "$out/$name.elf"
    llvm-objcopy -O binary "$elf" "$out/$name.bin"
    cp "$(ls -t "$dir"/thumbv7em-none-eabihf/release/build/vdm-stm-fw-*/out/vdm-stm-fw.map | head -1)" "$out/$name.map"
  done
done

for chip in f401 f411; do
  echo "== boot probe $chip"
  CARGO_TARGET_DIR="/target/stm-fw/probe_$chip" cargo build --release --no-default-features \
    --features "$chip,c2,boot-probe"
done

echo
ls -l "$out"
