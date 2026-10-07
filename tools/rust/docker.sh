#!/usr/bin/env bash
# Rust host builds, tests and mutation runs in a Linux container, the same on Windows, macOS
# and Linux as in CI.
#
#   tools/rust/docker.sh image                          build the image (done on first use)
#   tools/rust/docker.sh test <workspace> [args...]     cargo test --workspace in <workspace>
#                                                       (software_esp32_rust, software_stm32_rust)
#   tools/rust/docker.sh lint <workspace>               rustfmt --check of the workspace and of
#                                                       its firmware crate, clippy -D warnings of
#                                                       all targets (default and all features)
#   tools/rust/docker.sh mutate <workspace> <package> [args...]
#                                                       cargo mutants on one package, then the
#                                                       per-file gate (tools/rust/mutation_gate.py)
#   tools/rust/docker.sh gate <workspace> <package>     the gate on the last mutate run only
#   tools/rust/docker.sh fw                             the four STM32 images and the boot probes
#                                                       -> software_stm32_rust/firmware/images/
#                                                       (tools/rust/stm/build_images.sh)
#   tools/rust/docker.sh image-check [<dir>]            C1-C5 and D9 on those images, with the
#                                                       ESP's C++ validation (tools/rust/stm/image_check.sh);
#                                                       with <dir>: C6 on the four C++ 2.1.7 release images
#                                                       in it (*STM32F401_C1.bin ..., SHA-256 pinned in
#                                                       tools/rust/stm/cpp217.sha256)
#   tools/rust/docker.sh interop <workspace>            the ignored tests that need the image's
#                                                       external programs (mosquitto, g++ -m32)
#   tools/rust/docker.sh run <command...>               any command in the container, repo at /src
#
# The Renode tests of the images run in their own container: tools/rust/renode.sh (README.md).
#
# Environment: VDM_MUTATION_JOBS (default 4), VDM_RUST_IMAGE (default
# vdmot-rust:<hash of the Dockerfile>), VDM_MUTATION_GATE=0 (mutate: cargo mutants without the
# gate, for the shards of a CI run (--shard k/n), whose outcomes are gated together).
#
# Each checkout (git worktree) has its own target volume, so parallel worktrees never share
# build directories; `run` builds into /target/run, never into the checkout. The cargo registry
# is one shared volume. Mutation runs of all checkouts
# (Rust and C++, tools/native/docker.sh) share one lock: a second run waits for the first.
set -euo pipefail

ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
HOST_ROOT="$ROOT"
if command -v cygpath >/dev/null 2>&1; then HOST_ROOT="$(cygpath -m "$ROOT")"; fi
IMAGE="${VDM_RUST_IMAGE:-vdmot-rust:$(md5sum "$ROOT/tools/rust/Dockerfile" | cut -c1-12)}"
TARGET_VOLUME="vdmot-rust-target-$(printf '%s' "$HOST_ROOT" | md5sum | cut -c1-12)"
CARGO_VOLUME="vdmot-rust-cargo"
LOCKS_VOLUME="vdmot-locks"
MUTATION_JOBS="${VDM_MUTATION_JOBS:-4}"
MUTATION_GATE="${VDM_MUTATION_GATE:-1}"
export MSYS_NO_PATHCONV=1

quote_args() {
  [ $# -eq 0 ] || printf ' %q' "$@"
}

build_image() {
  docker build -q -t "$IMAGE" -f "$ROOT/tools/rust/Dockerfile" "$ROOT/tools/rust" >/dev/null
}

# more `docker run` arguments of one command (an extra mount)
extra_args=()

in_container() {
  docker image inspect "$IMAGE" >/dev/null 2>&1 || build_image
  docker run --rm --init -v "$HOST_ROOT:/src" -v "$TARGET_VOLUME:/target" \
    ${extra_args[@]+"${extra_args[@]}"} \
    -v "$CARGO_VOLUME:/cargo-cache" -v "$LOCKS_VOLUME:/locks" -w /src \
    -e CARGO_HOME_CACHE=/cargo-cache -e PYTHONDONTWRITEBYTECODE=1 -e CARGO_TERM_COLOR=never \
    -e CARGO_TARGET_DIR=/target/run \
    "$IMAGE" bash -c "
      mkdir -p /cargo-cache/registry /cargo-cache/git
      ln -sfn /cargo-cache/registry /usr/local/cargo/registry
      ln -sfn /cargo-cache/git /usr/local/cargo/git
      $1"
}

check_workspace() {
  case "$1" in
    software_esp32_rust|software_stm32_rust) ;;
    *) echo "unknown workspace: $1 (software_esp32_rust, software_stm32_rust)" >&2; exit 2 ;;
  esac
  [ -f "$ROOT/$1/Cargo.toml" ] || { echo "$1/Cargo.toml not found" >&2; exit 2; }
}

# Container script: take the global mutation lock (fd 9 stays open until the container ends).
mutation_lock() {
  cat <<EOS
exec 9>/locks/mutate.lock
if ! flock -n 9; then
  echo "waiting for the mutation lock, held by: \$(cat /locks/mutate.owner 2>/dev/null || echo another run)" >&2
  flock 9
  echo "mutation lock acquired" >&2
fi
echo "$HOST_ROOT (since \$(date -u +%FT%TZ))" > /locks/mutate.owner
EOS
}

case "${1:-}" in
  image)
    build_image
    ;;
  test)
    [ $# -ge 2 ] || { echo "usage: $0 test <workspace> [cargo test args]" >&2; exit 2; }
    ws="$2"; shift 2
    check_workspace "$ws"
    in_container "set -e
      exec 8>/target/$ws.lock
      if ! flock -n 8; then echo 'waiting for another cargo run of $ws in this checkout' >&2; flock 8; fi
      cd $ws
      CARGO_TARGET_DIR=/target/$ws cargo test --workspace$(quote_args "$@")"
    ;;
  lint)
    [ $# -ge 2 ] || { echo "usage: $0 lint <workspace>" >&2; exit 2; }
    ws="$2"
    check_workspace "$ws"
    # every check runs; the exit code says whether one failed
    in_container "exec 8>/target/$ws.lock
      if ! flock -n 8; then echo 'waiting for another cargo run of $ws in this checkout' >&2; flock 8; fi
      cd $ws || exit 2
      failed=''
      cargo fmt --all --check || failed=\"\$failed rustfmt\"
      # the firmware crate is no workspace member (own target and toolchain); rustfmt follows
      # its modules from main.rs
      rustfmt --check --edition 2021 firmware/src/main.rs firmware/build.rs || failed=\"\$failed rustfmt(firmware)\"
      export CARGO_TARGET_DIR=/target/$ws
      cargo clippy --workspace --all-targets -- -D warnings || failed=\"\$failed clippy\"
      cargo clippy --workspace --all-targets --all-features -- -D warnings || failed=\"\$failed clippy(all-features)\"
      if [ -n \"\$failed\" ]; then echo \"lint $ws failed:\$failed\" >&2; exit 1; fi
      echo 'lint $ws: rustfmt and clippy clean'"
    ;;
  mutate)
    [ $# -ge 3 ] || { echo "usage: $0 mutate <workspace> <package> [cargo mutants args]" >&2; exit 2; }
    ws="$2"; pkg="$3"; shift 3
    check_workspace "$ws"
    in_container "$(mutation_lock)
set -e
cd $ws
out=/target/mutants/$ws/$pkg
mkdir -p \$out
set +e
env -u CARGO_TARGET_DIR cargo mutants --package $pkg --jobs $MUTATION_JOBS --output \$out --no-shuffle$(quote_args "$@")
rc=\$?
set -e
# 0 all caught, 2 missed mutants, 3 timeouts: the gate decides; anything else is an error
case \$rc in 0|2|3) ;; *) echo \"cargo mutants failed with exit code \$rc\" >&2; exit \$rc ;; esac
if [ $MUTATION_GATE = 0 ]; then
  echo \"no gate (VDM_MUTATION_GATE=0): \$out/mutants.out/outcomes.json\"
  exit 0
fi
python3 /src/tools/rust/mutation_gate.py --workspace $ws --package $pkg --outcomes \$out/mutants.out/outcomes.json"
    ;;
  gate)
    [ $# -ge 3 ] || { echo "usage: $0 gate <workspace> <package>" >&2; exit 2; }
    ws="$2"; pkg="$3"
    check_workspace "$ws"
    in_container "python3 /src/tools/rust/mutation_gate.py --workspace $ws --package $pkg --outcomes /target/mutants/$ws/$pkg/mutants.out/outcomes.json"
    ;;
  fw)
    in_container "set -e
      exec 8>/target/stm-fw.lock
      if ! flock -n 8; then echo 'waiting for another firmware build in this checkout' >&2; flock 8; fi
      bash tools/rust/stm/build_images.sh"
    ;;
  image-check)
    if [ $# -ge 2 ]; then
      # C6: the C++ release images, mounted read-only
      cpp="$(cd "$2" && pwd)"
      if command -v cygpath >/dev/null 2>&1; then cpp="$(cygpath -m "$cpp")"; fi
      extra_args=(-v "$cpp:/cpp-images:ro")
      in_container "bash tools/rust/stm/image_check.sh /cpp-images"
    else
      in_container "bash tools/rust/stm/image_check.sh"
    fi
    ;;
  interop)
    [ $# -ge 2 ] || { echo "usage: $0 interop <workspace>" >&2; exit 2; }
    ws="$2"
    check_workspace "$ws"
    in_container "set -e
      exec 8>/target/$ws.lock
      if ! flock -n 8; then echo 'waiting for another cargo run of $ws in this checkout' >&2; flock 8; fi
      cd $ws
      CARGO_TARGET_DIR=/target/$ws cargo test --workspace --lib --tests -- --ignored"
    ;;
  run)
    shift
    in_container "$*"
    ;;
  *)
    sed -n '2,36p' "$0" >&2
    exit 2
    ;;
esac
