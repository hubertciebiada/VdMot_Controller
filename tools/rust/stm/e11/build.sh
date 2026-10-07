#!/usr/bin/env bash
# Builds the E11 harness vdm-e11 (the Rust ESP flasher on the host) as a static Linux binary
# into software_stm32_rust/renode/e11/ (not in the repository), where the Renode container
# finds it. Runs in the Rust container:
#
#   tools/rust/docker.sh run "bash tools/rust/stm/e11/build.sh"
set -euo pipefail
cd /src/tools/rust/stm/e11
export CARGO_TARGET_DIR=/target/e11
# static: the Renode image has an older C library than the Rust image
RUSTFLAGS="-C target-feature=+crt-static" cargo build -q --release --target x86_64-unknown-linux-gnu
mkdir -p /src/software_stm32_rust/renode/e11
cp "$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release/vdm-e11" /src/software_stm32_rust/renode/e11/
ls -l /src/software_stm32_rust/renode/e11/vdm-e11
