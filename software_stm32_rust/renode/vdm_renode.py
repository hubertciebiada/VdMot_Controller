"""Helpers of boot.robot and app.robot: the C++ 2.1.7 no-init cells (vdm::ResetCounterCell,
vdm::ResetGuardCell, the warm state area) as the C++ writes them, so the test can load a C++
image into RAM and read back what the Rust boot stage made of it (docs/rust/GLUE-DESIGN-STM.md
§3.2, D3), and the C++ goldens of the glue_system suites (software_stm32_rust/glue/tests/golden,
format of glue/src/system/golden.rs) that app.robot replays against the running image."""

import os
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


# ---- the C++ goldens (glue/tests/golden/<slug>.txt)

GOLDEN_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "glue", "tests",
                          "golden")
EEPROM_SIZE = 8192
CPP_IDENTITY = "2.1.7-revamped_C2"


def _unescape(s):
    out = bytearray()
    i = 0
    while i < len(s):
        c = s[i]
        if c != "\\":
            out += c.encode("latin-1")
            i += 1
            continue
        n = s[i + 1]
        if n == "x":
            out.append(int(s[i + 2:i + 4], 16))
            i += 4
            continue
        out.append({"r": 13, "n": 10, "t": 9}.get(n, ord(n)))
        i += 2
    return bytes(out)


def _golden(slug):
    """The boots of a golden: [{reset, events: [(ms, dir, bytes)], end, eeprom, noinit}]."""
    boots = []
    with open(os.path.join(GOLDEN_DIR, slug + ".txt"), encoding="latin-1") as f:
        for raw in f:
            line = raw.rstrip("\r\n")
            key, _, rest = line.partition(" ")
            if key == "boot":
                boots.append({"reset": rest.split(" ")[1], "events": [], "end": "",
                              "eeprom": bytearray(b"\xff" * EEPROM_SIZE), "noinit": b""})
            elif key == "t":
                ms, direction, quoted = rest.split(" ", 2)
                boots[-1]["events"].append((int(ms), direction, _unescape(quoted[1:-1])))
            elif key == "end":
                boots[-1]["end"] = rest
            elif key == "e":
                address, hexrow = rest.split(" ")
                row = bytes.fromhex(hexrow)
                a = int(address, 16)
                boots[-1]["eeprom"][a:a + len(row)] = row
            elif key == "noinit":
                boots[-1]["noinit"] = bytes.fromhex(rest)
    return boots


def _lines(chunks, identity):
    """(ms, line) of a byte stream that came in chunks [(ms, bytes)]: a line has the time of the
    chunk it starts in; CR is dropped, the C++ identity becomes the image's."""
    out = []
    current = None
    for ms, data in chunks:
        for c in data.decode("latin-1"):
            if current is None:
                current = [ms, ""]
            if c == "\n":
                out.append((current[0], current[1]))
                current = None
            elif c != "\r":
                current[1] += c
    if current is not None and current[1]:
        out.append((current[0], current[1]))
    return [(ms, text.replace(CPP_IDENTITY, identity)) for ms, text in out]


def golden_boot_events(slug, index, identity):
    """The ESP side of one boot as [kind, ms, payload]: "rx" with the request bytes as hex pairs
    (Send Bytes), "tx" with one reply line (no CR LF), the C++ identity "2.1.7-revamped_C2"
    replaced by `identity` (e.g. 2.2.0-revamped_C1)."""
    boot = _golden(slug)[int(index)]
    events = []
    for ms, direction, data in boot["events"]:
        if direction == "rx":
            events.append(["rx", ms, " ".join("%02X" % b for b in data)])
        elif direction == "tx":
            for _, text in _lines([(ms, data)], identity):
                events.append(["tx", ms, text])
    return events


def golden_terminal_lines(slug, index, identity):
    """The debug terminal lines of one boot as [ms, line]."""
    boot = _golden(slug)[int(index)]
    chunks = [(ms, data) for ms, direction, data in boot["events"] if direction == "dbg"]
    return [[ms, text] for ms, text in _lines(chunks, identity)]


def golden_terminal_head(slug, index, identity):
    """The terminal lines of one boot before its EEPROM load ("Read eeprom layout..."): what a
    start prints before the stored configuration can make a difference."""
    head = []
    for _, text in golden_terminal_lines(slug, index, identity):
        if text.startswith("Read eeprom layout"):
            break
        head.append(text)
    return head


def golden_boot_end(slug, index):
    """How the boot ended: "reboot software", "reboot power-on", "reboot watchdog", "case"."""
    return _golden(slug)[int(index)]["end"]


def _rows(image):
    return ["e %04x %s" % (a, image[a:a + 32].hex()) for a in range(0, len(image), 32)
            if any(b != 0xFF for b in image[a:a + 32])]


def golden_eeprom_rows(slug, index):
    """The EEPROM at the end of one boot as the golden writes it: the rows not erased."""
    return _rows(bytes(_golden(slug)[int(index)]["eeprom"]))


def eeprom_rows(hexdump):
    """The same rows of the EEPROM model (sysbus.i2c1.eeprom Hex 0 8192)."""
    text = "".join(c for c in hexdump if c in "0123456789abcdefABCDEF")
    return _rows(bytes.fromhex(text))


def golden_eeprom_load(slug, index):
    """[address, hex] of the rows of one boot's end, for sysbus.i2c1.eeprom LoadHex."""
    return [[int(r.split(" ")[1], 16), r.split(" ")[2]] for r in golden_eeprom_rows(slug, index)]


def golden_noinit_words(slug, index):
    """[(address, word)] of the no-init region at the end of one boot (C++ 2.1.7 bytes)."""
    data = _golden(slug)[int(index)]["noinit"]
    return [("0x%08X" % (NOINIT + 4 * i), "0x%08X" % struct.unpack_from("<I", data, 4 * i)[0])
            for i in range(len(data) // 4)]


def compare_terminal(golden, lines, t0_ms):
    """The terminal lines of a boot against the golden's: the same text in the same order.
    `lines` are the tester results (Line, Timestamp in ms of virtual time), `t0_ms` the reset.
    Returns the largest time difference of the lines that start a golden chunk, in ms."""
    got = [(float(x["Timestamp"]) - float(t0_ms), x["Line"]) for x in lines]
    texts = [text for _, text in got]
    want = [text for _, text in golden]
    if texts != want:
        for i, (w, g) in enumerate(zip(want, texts)):
            if w != g:
                raise AssertionError("terminal line %d: golden %r, image %r" % (i, w, g))
        rest = texts[len(want):] or want[len(texts):]
        raise AssertionError("terminal: golden %d lines, image %d lines, then %r"
                             % (len(want), len(texts), rest[:3]))
    worst = 0.0
    previous = None
    for (ms, _), (t, _) in zip(golden, got):
        if ms != previous:
            worst = max(worst, abs(t - ms))
        previous = ms
    return round(worst, 1)


def reply_field(reply, n):
    """Field n (1 = the first value after the prefix) of a reply such as gstax, as int."""
    return int(reply.split()[int(n)])


def fields_except(reply, skip):
    """The words of a reply without the fields in `skip` ("1,8": 1 = the first value after the
    prefix), for replies with fields that count time."""
    drop = {int(x) for x in str(skip).split(",") if x.strip()}
    return [w for i, w in enumerate(reply.split()) if i not in drop]


def valve_statuses(reply):
    """The status codes of a gvlst reply ("gvlst 12 8,8,...,6 ") as ints."""
    return [int(x) for x in reply.split()[2].split(",")]
