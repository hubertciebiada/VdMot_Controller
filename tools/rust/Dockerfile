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
WORKDIR /src
