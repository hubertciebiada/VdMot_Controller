#!/usr/bin/env bash
# The Rust ESP32 firmware (software_esp32_rust/firmware) in a Linux container: Xtensa toolchain,
# ESP-IDF, esptool and Espressif's QEMU (tools/rust/esp/Dockerfile), the same on Windows, macOS
# and Linux.
#
#   tools/rust/esp/docker.sh image                  build the image (done on first use)
#   tools/rust/esp/docker.sh build [features]       release build of the firmware and its app image
#                                                   (features: comma list, "" = default = wifi;
#                                                   e.g. "qemu", "qemu,fail-boot", "nowifi")
#   tools/rust/esp/docker.sh size [features]        build, then the image size against the budget
#   tools/rust/esp/docker.sh qemu [scenarios...]    build the QEMU variants, then the end-to-end
#                                                   harness (tools/rust/esp/qemu/harness.py)
#   tools/rust/esp/docker.sh run <command...>       any command in the container, repo at /src
#
# Volumes: vdmot-esp-idf (ESP-IDF, its tools and python env, fetched by esp-idf-sys on the first
# build; shared by all checkouts, one build at a time), vdmot-esp-cargo (cargo registry),
# vdmot-esp-target-<checkout> (cargo target dir, images, QEMU runs; bind mounts on Windows are
# slow, so nothing is built on /src). Outputs: /target/images/<variant>/ (app image, ELF, linker
# map) and /target/qemu/ (flash files, serial logs) in the target volume.
#
# Environment: VDM_RUST_ESP_IMAGE (default vdmot-rust-esp:<hash of the Dockerfile>), DOCKER_ARGS
# (extra arguments of docker run).
set -euo pipefail

ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
HOST_ROOT="$ROOT"
if command -v cygpath >/dev/null 2>&1; then HOST_ROOT="$(cygpath -m "$ROOT")"; fi
IMAGE="${VDM_RUST_ESP_IMAGE:-vdmot-rust-esp:$(md5sum "$ROOT/tools/rust/esp/Dockerfile" | cut -c1-12)}"
TARGET_VOLUME="vdmot-esp-target-$(printf '%s' "$HOST_ROOT" | md5sum | cut -c1-12)"
export MSYS_NO_PATHCONV=1

quote_args() {
  [ $# -eq 0 ] || printf ' %q' "$@"
}

build_image() {
  docker build -q -t "$IMAGE" -f "$ROOT/tools/rust/esp/Dockerfile" "$ROOT/tools/rust/esp" >/dev/null
}

# $1: container script; extra docker args before the image come from DOCKER_ARGS
in_container() {
  docker image inspect "$IMAGE" >/dev/null 2>&1 || build_image
  # shellcheck disable=SC2086
  docker run --rm --init ${DOCKER_ARGS:-} -v "$HOST_ROOT:/src" -v "$TARGET_VOLUME:/target" \
    -v vdmot-esp-idf:/opt/espressif -v vdmot-esp-cargo:/cargo-cache -w /src \
    "$IMAGE" bash -c "
      set -e
      mkdir -p /cargo-cache/registry /cargo-cache/git
      ln -sfn /cargo-cache/registry /usr/local/cargo/registry
      ln -sfn /cargo-cache/git /usr/local/cargo/git
      . /opt/export-esp.sh
      $1"
}

# Container script: build one variant and write its app image. $1 = features ("" = default).
# The variant name is the feature list ("default" for none); "nowifi" = no default features.
build_script() {
  local features="$1" variant args
  case "$features" in
    ""|default) variant=default; args="" ;;
    nowifi) variant=nowifi; args="--no-default-features" ;;
    nowifi,*) variant="${features//,/-}"; args="--no-default-features --features ${features#nowifi,}" ;;
    *) variant="${features//,/-}"; args="--features $features" ;;
  esac
  cat <<EOS
exec 9>/opt/espressif/.vdm-build.lock
if ! flock -n 9; then echo 'waiting for another ESP-IDF build (vdmot-esp-idf)' >&2; flock 9; fi
cd /src/software_esp32_rust/firmware
export CARGO_TARGET_DIR=/target/cargo
out=/target/images/$variant
mkdir -p \$out
cargo build --release $args
elf=/target/cargo/xtensa-esp32-espidf/release/vdm-esp-fw
cp \$elf \$out/vdm-esp-fw.elf
# esp-idf-sys links with -Wl,--Map=<its out dir>/build/libespidf.map: the map of this link
cp \$(ls -t /target/cargo/xtensa-esp32-espidf/release/build/esp-idf-sys*/out/build/libespidf.map /target/cargo/xtensa-esp32-espidf/release/build/esp-idf-sys/*/out/build/libespidf.map 2>/dev/null | head -1) \$out/vdm-esp-fw.map
# the app image as ESP-IDF 5.5 writes it (esptool_py/project_include.cmake): DIO 80 MHz 4 MB,
# ELF SHA-256 in the app descriptor, chip revisions v0.0 .. v3.99, SHA-256 appended
esptool --chip esp32 elf2image --flash-mode dio --flash-freq 80m --flash-size 4MB \
  --elf-sha256-offset 0xb0 --min-rev-full 0 --max-rev-full 399 \
  -o \$out/vdm-esp-fw.bin \$elf >/dev/null
echo "variant $variant: \$out/vdm-esp-fw.bin \$(stat -c %s \$out/vdm-esp-fw.bin) B"
EOS
}

size_script() {
  cat <<'EOS'
python3 /src/tools/rust/esp/image_size.py "$out/vdm-esp-fw.bin" "$out/vdm-esp-fw.map"
EOS
}

# The C++ firmware 2.1.7 for the QEMU harness: the GitHub release asset (gh, checked against the
# release's SHA256SUMS), cached in tools/rust/esp/qemu/cache; without gh the 2.0.0 release of the
# repository (releases/revamped/2.0.0-revamped).
cpp_image() {
  local cache="$ROOT/tools/rust/esp/qemu/cache" name="VdMot-Revamped_2.1.7-revamped_ESP32-WT32-ETH01.bin"
  mkdir -p "$cache"
  if [ ! -f "$cache/$name" ] && command -v gh >/dev/null 2>&1; then
    gh release download v2.1.7-revamped --repo hubertciebiada/VdMot_Controller \
      --pattern '*ESP32-WT32-ETH01.bin' --pattern SHA256SUMS --dir "$cache" --clobber >&2 || true
  fi
  if [ -f "$cache/$name" ] && (cd "$cache" && grep " $name\$" SHA256SUMS | sha256sum -c - >&2); then
    echo "/src/tools/rust/esp/qemu/cache/$name"
  else
    echo "C++ 2.1.7 release asset not available, using releases/revamped/2.0.0-revamped" >&2
    echo "/src/releases/revamped/2.0.0-revamped/ESP32_revamped_firmware.bin"
  fi
}

case "${1:-}" in
  image)
    build_image
    ;;
  build)
    in_container "$(build_script "${2:-}")"
    ;;
  size)
    in_container "$(build_script "${2:-}")
$(size_script)"
    ;;
  qemu)
    shift
    cpp="$(cpp_image)"
    in_container "$(build_script qemu)
$(build_script qemu,fail-boot)
$(build_script qemu,short-deadline)
/opt/pytools/bin/python /src/tools/rust/esp/qemu/harness.py --cpp-image $cpp$(quote_args "$@")"
    ;;
  run)
    shift
    in_container "$*"
    ;;
  *)
    sed -n '2,22p' "$0" >&2
    exit 2
    ;;
esac
