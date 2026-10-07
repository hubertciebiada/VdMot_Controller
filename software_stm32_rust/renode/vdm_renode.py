"""Helpers of boot.robot: the C++ 2.1.7 no-init cells (vdm::ResetCounterCell, vdm::ResetGuardCell,
the warm state area) as the C++ writes them, so the test can load a C++ image into RAM and read
back what the Rust boot stage made of it (docs/rust/GLUE-DESIGN-STM.md §3.2, D3)."""

import struct

NOINIT = 0x20003234
GUARD = NOINIT + 0xB4
COUNTER = NOINIT + 0xC8
GUARD_MAGIC = 0x56445247  # "VDRG"
COUNTER_MAGIC = 0x5644524D  # "VDRM"


def crc16_ccitt(data):
    """CRC-16/CCITT-FALSE, vdm::crc16Ccitt."""
    crc = 0xFFFF
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) if crc & 0x8000 else (crc << 1)
            crc &= 0xFFFF
    return crc


def cpp_noinit_words(resets, guard_count, guard_window_s, guard_last_uptime_s):
    """The 53 words of the region as C++ 2.1.7 leaves them: a warm state area filled with the
    byte pattern i ^ 0x5A, a counter cell with `resets`, a guard cell with a valid CRC and
    tail padding 0xBEEF. Returns [(address, word)] as strings for `sysbus WriteDoubleWord`."""
    region = bytearray((i ^ 0x5A) & 0xFF for i in range(0xD4))
    guard = struct.pack("<IBBHII", GUARD_MAGIC, int(guard_count), 0, 0, int(guard_window_s),
                        int(guard_last_uptime_s))
    guard += struct.pack("<H", crc16_ccitt(guard)) + b"\xEF\xBE"
    region[0xB4:0xC8] = guard
    resets = int(resets)
    region[0xC8:0xD4] = struct.pack("<III", COUNTER_MAGIC, resets, (~resets) & 0xFFFFFFFF)
    return [("0x%08X" % (NOINIT + 4 * i), "0x%08X" % struct.unpack_from("<I", region, 4 * i)[0])
            for i in range(0xD4 // 4)]


def noinit_bytes(words):
    """Bytes of the region from the read-back words (hex strings in address order)."""
    return b"".join(struct.pack("<I", int(w.strip(), 16)) for w in words)


def check_noinit_after_warm_boot(before_words, after_words, expected_resets, expected_window_s):
    """What the Rust capture must leave after a warm (pin) reset: warm state and guard padding
    byte-equal, counter = expected_resets with its check word, guard sealed by the C++ rule
    (window_s summed, last_uptime_s 0, count kept, CRC valid). Raises AssertionError."""
    before = noinit_bytes([w for _, w in before_words])
    after = noinit_bytes(after_words)
    assert after[:0xB4] == before[:0xB4], "warm state bytes changed"
    assert after[0xC6:0xC8] == before[0xC6:0xC8], "guard tail padding changed"
    magic, count, check = struct.unpack_from("<III", after, 0xC8)
    assert (magic, count, check) == (COUNTER_MAGIC, int(expected_resets),
                                     (~int(expected_resets)) & 0xFFFFFFFF), \
        "counter cell %08x %d %08x" % (magic, count, check)
    g_magic, g_count, g_safe, g_pad, g_window, g_last = struct.unpack_from("<IBBHII", after, 0xB4)
    (g_crc,) = struct.unpack_from("<H", after, 0xC4)
    b_count = struct.unpack_from("<B", before, 0xB8)[0]
    assert g_magic == GUARD_MAGIC and g_pad == 0, "guard magic/pad"
    assert g_count == b_count and g_safe == 0, "guard count %d safe %d" % (g_count, g_safe)
    assert g_window == int(expected_window_s) and g_last == 0, \
        "guard window %d last %d" % (g_window, g_last)
    assert g_crc == crc16_ccitt(after[0xB4:0xC4]), "guard CRC"
    return "counter %d, guard count %d window %d s" % (count, g_count, g_window)


def fault_record(words):
    """The FaultRecord (firmware/src/fault.rs): magic, kind, pc, lr, xpsr, cfsr, hfsr, bfar,
    count, check. Returns (valid, kind, pc, cfsr)."""
    w = [int(x.strip(), 16) for x in words]
    check = 0
    for x in w[:9]:
        check ^= x
    valid = w[0] == 0x56444652 and w[9] == (~check) & 0xFFFFFFFF
    return valid, w[1], w[2], w[5]
