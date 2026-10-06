# Host toolchain of the Rust ports (software_esp32_rust, software_stm32_rust): stable Rust,
# clippy, rustfmt, cargo-mutants and the STM32 target. Used by tools/rust/docker.sh.
# Every version is pinned; a change here builds a new image (the tag follows this file).
FROM rust:1.99.0-bookworm
RUN rustup component add clippy rustfmt \
 && rustup target add thumbv7em-none-eabihf \
 && cargo install --locked cargo-mutants@27.1.0 \
 && rm -rf /usr/local/cargo/registry /usr/local/cargo/git \
 && apt-get update \
 && apt-get install -y --no-install-recommends python3 \
 && rm -rf /var/lib/apt/lists/*
# STM32 images: llvm-objcopy (ELF -> .bin), llvm-nm/llvm-objdump for the image check; the
# toolchain's own LLVM tools, so they follow RUST_VERSION
RUN rustup component add llvm-tools \
 && ln -s "$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-objcopy" /usr/local/bin/llvm-objcopy \
 && ln -s "$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-objdump" /usr/local/bin/llvm-objdump \
 && ln -s "$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-nm" /usr/local/bin/llvm-nm \
 && ln -s "$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/llvm-size" /usr/local/bin/llvm-size
WORKDIR /src
