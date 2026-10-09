#!/usr/bin/env bash
# The C++ 2.1.7 references of the cross checks, taken from the release tag: the ESP core
# (software_esp32_revamped/lib/core: the config codec of the QEMU nvs scenario, the STM image
# validation of the image check) and the third-party headers of its native tests (ArduinoJson
# for the json_body reference program). The C++ sources are no longer in the tree; the tag is
# pinned to its commit, so a moved tag fails instead of changing the references.
#
#   tools/rust/cpp217.sh    extract once per checkout into .cache/cpp-2.1.7 (git-ignored),
#                           fetching the tag first when the clone lacks it; prints the directory
#
# Layout: .cache/cpp-2.1.7/lib/core, .cache/cpp-2.1.7/test/native/third_party (in the
# containers /src/.cache/cpp-2.1.7/...).
set -euo pipefail

ROOT="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
TAG="v2.1.7-revamped"
COMMIT="a513488701a0e38aa372155a353fe1a0dd2cfaea"
DIR="$ROOT/.cache/cpp-2.1.7"

if [ ! -f "$DIR/.complete" ]; then
  if ! git -C "$ROOT" rev-parse -q --verify "refs/tags/$TAG^{commit}" >/dev/null; then
    git -C "$ROOT" fetch -q --depth=1 --no-tags origin "+refs/tags/$TAG:refs/tags/$TAG"
  fi
  got="$(git -C "$ROOT" rev-parse "refs/tags/$TAG^{commit}")"
  if [ "$got" != "$COMMIT" ]; then
    echo "cpp217: $TAG is $got, expected $COMMIT" >&2
    exit 1
  fi
  rm -rf "$DIR"
  mkdir -p "$DIR"
  git -C "$ROOT" archive "$COMMIT" software_esp32_revamped/lib/core \
    software_esp32_revamped/test/native/third_party | tar -x -C "$DIR" --strip-components=1
  touch "$DIR/.complete"
fi
echo "$DIR"
