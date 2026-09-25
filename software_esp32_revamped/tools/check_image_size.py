#!/usr/bin/env python3
"""Size gate of the application image (firmware.bin).

The image must fit the app partition with headroom for later releases: the build fails above
LIMIT_BYTES and warns above WARN_BYTES. The delta to the 2.0.0-revamped image (REFERENCE_BYTES,
wt32-eth01_revamped at 58632d6) shows how much each change costs; the release plan gives every
work package a budget of growth.

Runs as a PlatformIO post script (extra_scripts = post:tools/check_image_size.py) after
firmware.bin is written, and standalone:

    python3 tools/check_image_size.py .pio/build/wt32-eth01_revamped/firmware.bin
"""
from __future__ import annotations

import os
import sys

LIMIT_BYTES = 1228800       # app partition 1,310,720 B minus 80 KB headroom
WARN_BYTES = 1200000
REFERENCE_BYTES = 1091984   # firmware.bin of wt32-eth01_revamped at 58632d6


def check(path: str) -> tuple[bool, str]:
    """(passed, message) for the image at path."""
    size = os.path.getsize(path)
    delta = size - REFERENCE_BYTES
    text = (f"check_image_size.py: {os.path.basename(path)} = {size:,} B, delta {delta:+,} B to "
            f"{REFERENCE_BYTES:,} B (2.0.0-revamped), limit {LIMIT_BYTES:,} B, "
            f"headroom {LIMIT_BYTES - size:,} B")
    if size > LIMIT_BYTES:
        return False, f"{text}\ncheck_image_size.py: ERROR: the image exceeds the limit by {size - LIMIT_BYTES:,} B"
    if size > WARN_BYTES:
        return True, f"{text}\ncheck_image_size.py: WARNING: the image is above {WARN_BYTES:,} B"
    return True, text


def _run_pio(env) -> None:
    def after_bin(target, source, env):  # noqa: ARG001 - SCons action signature
        passed, message = check(str(target[0]))
        print(message)
        if not passed:
            env.Exit(1)

    env.AddPostAction("$BUILD_DIR/${PROGNAME}.bin", after_bin)


def _in_scons() -> bool:
    try:
        Import  # type: ignore[name-defined]  # noqa: B018,F821 - provided by SCons
    except NameError:
        return False
    return True


if _in_scons():
    Import("env")  # type: ignore[name-defined]  # noqa: F821
    _run_pio(env)  # type: ignore[name-defined]  # noqa: F821
elif __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    ok, msg = check(sys.argv[1])
    print(msg)
    sys.exit(0 if ok else 1)
