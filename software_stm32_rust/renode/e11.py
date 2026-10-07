"""Helpers of e11.robot: the C++ release image of a name, checked against its pinned SHA-256
(tools/rust/stm/cpp217.sha256), and the order check of D9 on the log of the ROM bootloader
stand-in (VdmRom.cs Operations())."""

import glob
import hashlib
import os

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


def check_d9_order(ops):
    """Every ROM session erases and writes the sectors above 0 first and verifies them, then
    erases sector 0, writes it from its second block on and the vector table (block 0) last.
    Returns the number of sessions; raises AssertionError with the session otherwise."""
    sessions = []
    for op in [o.strip() for o in ops.strip().split(";") if o.strip()]:
        if op.startswith("session"):
            sessions.append([])
        elif sessions:
            sessions[-1].append(op)
    for i, s in enumerate(sessions, 1):
        erases = [k for k, op in enumerate(s) if op.startswith("E ")]
        writes = [k for k, op in enumerate(s) if op.startswith("W ")]
        if len(erases) != 2 or s[erases[1]] != "E 0" or s[erases[0]].split(" ")[1] != "1":
            raise AssertionError("session %d: erases %r" % (i, [s[k] for k in erases]))
        upper = [k for k in writes if k < erases[1]]
        lower = [k for k in writes if k > erases[1]]
        if not upper or any(_range(s[k])[0] < SECTOR1 for k in upper):
            raise AssertionError("session %d: writes before the erase of sector 0: %r" % (i, s))
        reads_between = [k for k, op in enumerate(s) if op.startswith("R ") and upper[-1] < k < erases[1]]
        if not reads_between:
            raise AssertionError("session %d: no verify before the erase of sector 0: %r" % (i, s))
        if not lower or _range(s[lower[-1]]) != (VECTORS, VECTORS):
            raise AssertionError("session %d: the vector table is not the last write: %r" % (i, s))
        if any(_range(s[k])[1] >= SECTOR1 for k in lower):
            raise AssertionError("session %d: a write above sector 0 after its erase: %r" % (i, s))
    return len(sessions)
