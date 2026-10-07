#!/usr/bin/env python3
"""The boot guard's AppId of ESP32 app images: it must name the build.

    app_id.py <app.bin>...

The Rust firmware's boot guard (docs/rust/GLUE-DESIGN-ESP.md section 6) tells images apart by
the first 8 bytes of esp_app_desc_t.app_elf_sha256, which `esptool elf2image
--elf-sha256-offset 0xb0` writes into the image. An image built without it carries zeros there:
the guard treats such a running image as unknown and gives it no trial, and two such builds
would look the same. Fails (exit 1) for an image without the app descriptor at 0x20 or with an
all-zero ELF SHA-256; prints the AppId otherwise.
"""
from __future__ import annotations

import sys

# image header (24 B) + first segment header (8 B): the app descriptor starts the first segment
DESC_OFFSET = 0x20
DESC_MAGIC = 0xABCD5432
# esp_app_desc_t.app_elf_sha256 at 0x90 of the descriptor
SHA_OFFSET = DESC_OFFSET + 0x90
SHA_LEN = 32
IMAGE_MAGIC = 0xE9


def check(path: str) -> str | None:
    """The error of the image at `path`, None when its AppId names the build."""
    with open(path, "rb") as f:
        head = f.read(SHA_OFFSET + SHA_LEN)
    if len(head) < SHA_OFFSET + SHA_LEN or head[0] != IMAGE_MAGIC:
        return "not an ESP32 app image"
    if int.from_bytes(head[DESC_OFFSET:DESC_OFFSET + 4], "little") != DESC_MAGIC:
        return f"no app descriptor at {DESC_OFFSET:#x}"
    if not any(head[SHA_OFFSET:SHA_OFFSET + SHA_LEN]):
        return ("app_elf_sha256 is all zero (elf2image without --elf-sha256-offset 0xb0): "
                "the boot guard could not tell this build from another")
    return None


def main(paths: list[str]) -> int:
    if not paths:
        print(__doc__, file=sys.stderr)
        return 2
    bad = 0
    for path in paths:
        err = check(path)
        if err is None:
            with open(path, "rb") as f:
                app = f.read(SHA_OFFSET + 8)[SHA_OFFSET:]
            print(f"{path}: AppId {app.hex()}")
        else:
            print(f"{path}: {err}", file=sys.stderr)
            bad += 1
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
