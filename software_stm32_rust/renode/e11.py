"""Helpers of e11.robot: the C++ release image of a name, checked against its pinned SHA-256
(tools/rust/stm/cpp217.sha256), and the order check of D9 and STM F9 on the log of the ROM
bootloader stand-in (VdmRom.cs Operations())."""

import glob
import hashlib
import os
import struct
import zlib

SECTOR1 = 0x08004000
VECTORS = 0x08000000
PINS = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "tools", "rust", "stm",
                    "cpp217.sha256")


def _pinned(name):
    with open(PINS, encoding="ascii") as f:
        for line in f:
            parts = line.split()
            if len(parts) == 2 and not line.startswith("#") and parts[1] == name:
                return parts[0]
    raise AssertionError("no SHA-256 pinned for %s in %s" % (name, PINS))


def find_one_image(directory, name):
    """The one *<name>.bin in the directory (release asset names end with the image name), after
    its SHA-256 matched the pinned one."""
    found = sorted(glob.glob(os.path.join(directory, "*" + name + ".bin")))
    if len(found) != 1:
        raise AssertionError("expected one *%s.bin in %s, found %r" % (name, directory, found))
    with open(found[0], "rb") as f:
        digest = hashlib.sha256(f.read()).hexdigest()
    if digest != _pinned(name):
        raise AssertionError("%s: SHA-256 %s, pinned %s" % (found[0], digest, _pinned(name)))
    return found[0]


def _range(op):
    """(first, last) address of a "W 08004000..08010100 x195" entry."""
    first, last = op.split(" ")[1].split("..")
    return int(first, 16), int(last, 16)


def record_valid(path):
    """STM F9: the image carries a valid record of its application part (GLUE-DESIGN-STM 5.11:
    at 0x240 "VDAC", 0x08004000, the length above sector 0 and its zlib CRC-32), so the ESP
    flasher writes its sector 0 first."""
    with open(path, "rb") as f:
        d = f.read()
    if len(d) <= SECTOR1 - VECTORS:
        return False
    magic, start, length, crc = struct.unpack_from("<4I", d, 0x240)
    return (magic == 0x43414456 and start == SECTOR1 and length == len(d) - (SECTOR1 - VECTORS)
            and crc == zlib.crc32(d[SECTOR1 - VECTORS:]) & 0xFFFFFFFF)


def _check_pass(i, s, ops, sector0):
    """One pass of a session: its writes in sector 0 (from the second block on, the vector table
    last) or above it, then a read back."""
    writes = [k for k, op in enumerate(ops) if op.startswith("W ")]
    if not writes:
        raise AssertionError("session %d: a pass without writes: %r" % (i, s))
    if sector0:
        if any(_range(ops[k])[1] >= SECTOR1 for k in writes):
            raise AssertionError("session %d: a write above sector 0 in its pass: %r" % (i, s))
        if _range(ops[writes[-1]]) != (VECTORS, VECTORS):
            raise AssertionError("session %d: the vector table is not the last write: %r" % (i, s))
    elif any(_range(ops[k])[0] < SECTOR1 for k in writes):
        raise AssertionError("session %d: a write in sector 0 in the pass above it: %r" % (i, s))
    if not any(op.startswith("R ") for op in ops[writes[-1] + 1:]):
        raise AssertionError("session %d: no verify after the writes of a pass: %r" % (i, s))


def check_flash_order(ops, jobs):
    """Every ROM session (one per job of `jobs`, "<image>[@<version>] ...") erases, writes and
    verifies in two passes: the sectors above 0 first and sector 0 last (D9), or sector 0 first
    when the job's image carries a valid record of its application part (STM F9). Sector 0 is
    written from its second block on, the vector table (block 0) last. Returns the number of
    sessions; raises AssertionError with the session otherwise."""
    files = [j.split("@")[0] for j in jobs.split()]
    sessions = []
    for op in [o.strip() for o in ops.strip().split(";") if o.strip()]:
        if op.startswith("session"):
            sessions.append([])
        elif sessions:
            sessions[-1].append(op)
    if len(sessions) != len(files):
        raise AssertionError("%d sessions for the jobs %r" % (len(sessions), files))
    for i, (s, path) in enumerate(zip(sessions, files), 1):
        first = record_valid(path)
        erases = [k for k, op in enumerate(s) if op.startswith("E ")]
        if len(erases) != 2:
            raise AssertionError("session %d: erases %r" % (i, [s[k] for k in erases]))
        low, high = (erases[0], erases[1]) if first else (erases[1], erases[0])
        if s[low] != "E 0" or s[high].split(" ")[1] != "1":
            raise AssertionError("session %d (sector 0 %s): erases %r"
                                 % (i, "first" if first else "last", [s[k] for k in erases]))
        _check_pass(i, s, s[erases[0] + 1:erases[1]], first)
        _check_pass(i, s, s[erases[1] + 1:], not first)
    return len(sessions)
