# Xtensa (ESP32) toolchain of the Rust firmware (software_esp32_rust/firmware) and of its QEMU
# harness (tools/rust/esp/qemu). Used by tools/rust/esp/docker.sh.
#
# Every version is pinned and every direct download is checked against its SHA-256; a change
# here builds a new image (the tag follows this file). ESP-IDF itself, its GCC and its python
# environment are not in the image: esp-idf-sys (native builder) fetches them on the first build
# into the volume mounted at /opt/espressif (ESP_IDF_TOOLS_INSTALL_DIR below), at the version
# the firmware crate names (software_esp32_rust/firmware/Cargo.toml).
FROM rust:1.99.0-bookworm

# ESP-IDF prerequisites (get-started, Linux), the runtime libraries of Espressif's QEMU.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      git wget flex bison gperf python3 python3-pip python3-venv cmake ninja-build ccache \
      libffi-dev libssl-dev dfu-util libusb-1.0-0 xz-utils unzip \
      libgcrypt20 libglib2.0-0 libpixman-1-0 libsdl2-2.0-0 libslirp0 \
 && rm -rf /var/lib/apt/lists/*

# Xtensa Rust toolchain "esp" (rustc fork + rust-src + Xtensa libclang for bindgen). --std: the
# GCC comes with ESP-IDF (esp-idf-sys installs it), espup does not install one.
# 1.98.1.0, not 1.99.0.0: the std of Rust 1.99 does not build for xtensa-esp32-espidf
# (sys/fs/unix.rs set_perm_nofollow calls libc::fchmodat(libc::AT_FDCWD, ..), which libc 0.2.189
# does not define for espidf; error E0425 in -Zbuild-std).
ARG ESPUP_VERSION=0.18.0
ARG ESPUP_SHA256=32d0cc5a0d1a48be4d374e62de0a3c4f22307d43a5fcc0d06195854e064253b6
ARG ESP_RUST_VERSION=1.98.1.0
RUN curl -fsSL -o /usr/local/bin/espup \
      "https://github.com/esp-rs/espup/releases/download/v${ESPUP_VERSION}/espup-x86_64-unknown-linux-gnu" \
 && echo "${ESPUP_SHA256}  /usr/local/bin/espup" | sha256sum -c - \
 && chmod +x /usr/local/bin/espup \
 && espup install --std --targets esp32 --toolchain-version "${ESP_RUST_VERSION}" \
      --export-file /opt/export-esp.sh \
 && rustup toolchain list \
 && rm -rf /tmp/* /root/.cache

# Linker wrapper of esp-idf-sys (cargo hands the link to the ESP-IDF GCC through it).
ARG LDPROXY_VERSION=0.3.5
RUN cargo install --locked "ldproxy@${LDPROXY_VERSION}" \
 && rm -rf /usr/local/cargo/registry /usr/local/cargo/git

# esptool: ELF -> app image (elf2image), image checks, flash images for QEMU (merge-bin).
# littlefs-python: reads the LittleFS partition of a QEMU flash image (disk version, files).
ARG ESPTOOL_VERSION=5.4.0
ARG LITTLEFS_PYTHON_VERSION=0.19.0
RUN python3 -m venv /opt/pytools \
 && /opt/pytools/bin/pip install --no-cache-dir "esptool==${ESPTOOL_VERSION}" \
      "littlefs-python==${LITTLEFS_PYTHON_VERSION}" \
 && ln -s /opt/pytools/bin/esptool /usr/local/bin/esptool \
 && esptool version

# Espressif's QEMU (ESP32 machine, open_eth NIC).
ARG QEMU_RELEASE=esp-develop-9.2.2-20260417
ARG QEMU_ASSET=qemu-xtensa-softmmu-esp_develop_9.2.2_20260417-x86_64-linux-gnu.tar.xz
ARG QEMU_SHA256=0eecb2a34a5586c0e59110f77b9343b7b336e82fdb0e1a30e1dc1bab8a547e35
RUN curl -fsSL -o /tmp/qemu.tar.xz \
      "https://github.com/espressif/qemu/releases/download/${QEMU_RELEASE}/${QEMU_ASSET}" \
 && echo "${QEMU_SHA256}  /tmp/qemu.tar.xz" | sha256sum -c - \
 && tar -xJf /tmp/qemu.tar.xz -C /opt \
 && rm /tmp/qemu.tar.xz \
 && /opt/qemu/bin/qemu-system-xtensa --version

# mklittlefs of PlatformIO's espressif32@6.1.0 (the C++ firmware's platform: tool-mklittlefs
# ~1.203.0 next to Arduino-ESP32 2.0.7): writes and reads LittleFS images the way the C++
# toolchain does (QEMU harness, scenario littlefs).
ARG MKLITTLEFS_VERSION=1.203.210628
ARG MKLITTLEFS_SHA256=272f75679b1b5562ee712375ce32701f101e498b19c8b2ad1a3766885823dc8f
RUN curl -fsSL -o /tmp/mklittlefs.tar.gz \
      "https://dl.registry.platformio.org/download/platformio/tool/tool-mklittlefs/${MKLITTLEFS_VERSION}/tool-mklittlefs-linux_x86_64-${MKLITTLEFS_VERSION}.tar.gz" \
 && echo "${MKLITTLEFS_SHA256}  /tmp/mklittlefs.tar.gz" | sha256sum -c - \
 && mkdir -p /opt/mklittlefs && tar -xzf /tmp/mklittlefs.tar.gz -C /opt/mklittlefs \
 && rm /tmp/mklittlefs.tar.gz \
 && ln -s /opt/mklittlefs/mklittlefs /usr/local/bin/mklittlefs \
 && mklittlefs --version

ENV PATH=/opt/qemu/bin:$PATH \
    ESP_IDF_TOOLS_INSTALL_DIR=custom:/opt/espressif \
    CARGO_TERM_COLOR=never \
    PYTHONDONTWRITEBYTECODE=1
WORKDIR /src
