#!/usr/bin/env python3
"""End-to-end checks of the Rust firmware on Espressif's QEMU (ESP32 machine, OpenETH NIC),
with the bootloader and the partition table of the devices and the C++ firmware 2.1.7 in the
other OTA slot. Runs in the container of tools/rust/esp/docker.sh ("qemu" subcommand).

Flash image: software_esp32/bootloader_dio_40m.bin @0x1000 (the devices' bootloader, IDF 4.4
era), software_esp32/partitions.bin @0x8000, otadata @0xE000, app0 @0x10000, app1 @0x150000,
spiffs (LittleFS) @0x290000. otadata is written as the C++ firmware leaves it after an OTA into
app1: sector 0 = sequence 1 (boot_app0.bin of the serial flash), sector 1 = sequence 2, state
NEW (the ESP-IDF libraries of Arduino-ESP32 2.0.7 are built with BOOTLOADER_APP_ROLLBACK_ENABLE).

The flash file is 16 MB, the devices' layout in its first 4 MB: QEMU picks the flash model by
the file size, and with the 4 MB model (gd25q32) the C++ firmware stops at startup
(do_core_init: esp_flash_init_default_chip fails; its ESP-IDF 4.4.4 runs the flash in QIO and
cannot set the quad-enable bit of that model). With the 16 MB model (is25lp128) it starts.

What the C++ firmware does in this QEMU (2.1.7, Arduino-ESP32 2.0.7): it starts and its ESP-IDF
prints "esp_core_dump_flash: No core dump partition found!" (Arduino's core dump to flash, the
device table has no such partition; the Rust image never prints this), then its LittleFS mount
and format fail (its flash writes do not reach this flash model) and it prints none of its own
event lines; it does not reach the boot counter in NVS. So "the C++ firmware starts" is proven
by that ESP-IDF 4.4.4 line after the switch to app0, not by its event log.

After a software restart out of a Rust app that ran Ethernet and HTTP, the C++ firmware starts
unreliably in QEMU: in some runs its ESP-IDF line comes, in others nothing (one run traced with
-d in_asm sat in an interrupt loop in its code). QEMU's OpenETH model occupies the register block
and the interrupt source of the ESP32 EMAC that the C++ firmware drives and is not reset by a
software restart; the ESP32 resets its EMAC on a restart (esp_restart_noos). When the line does
not come within 30 s, the scenario starts the same flash in a new QEMU process (power-on), which
starts the C++ firmware every time, and says so in its evidence.

Scenarios (default: all):
  boot      (a) the devices' bootloader boots the Rust app, (b) GET /api/health answers through
            the forwarded port, (c) the boot guard confirms on health; then a power cycle keeps
            the confirmation, and POST /api/system/switch-back starts the C++ firmware
  rollback  (d) a Rust image that panics after the guard (feature fail-boot) gets 3 boot
            attempts, the 4th boot switches back to app0 and the C++ firmware starts; otadata
            shows whether the bootloader itself ever touched the trial (PENDING_VERIFY/ABORTED)
  deadline  a Rust image on trial whose health is never shown (no HTTP request; feature
            short-deadline: 60 s instead of 15 min) switches back to app0 at the deadline
  ota       (e) the C++ image uploaded through POST /api/ota of the Rust app into the empty other
            slot starts
  littlefs  a LittleFS image made by mklittlefs of the C++ toolchain (PlatformIO espressif32
            6.1.0) is mounted and listed by the Rust app, which appends a file; mklittlefs then
            unpacks the partition the Rust app wrote to, and littlefs-python shows the on-disk
            version before and after (2.0 must stay 2.0)

Every scenario prints its evidence lines (serial log, HTTP answers, otadata) and PASS or FAIL;
the full serial logs are in --workdir.
"""
from __future__ import annotations

import argparse
import binascii
import hashlib
import os
import random
import re
import shutil
import struct
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request

FLASH_SIZE = 16 * 1024 * 1024
BOOTLOADER_OFFSET = 0x1000
PARTITIONS_OFFSET = 0x8000
NVS_OFFSET = 0x9000
OTADATA_OFFSET = 0xE000
APP0_OFFSET = 0x10000
APP1_OFFSET = 0x150000
APP_SIZE = 0x140000
SPIFFS_OFFSET = 0x290000
SPIFFS_SIZE = 0x170000

OTA_STATES = {0: "NEW", 1: "PENDING_VERIFY", 2: "VALID", 3: "INVALID", 4: "ABORTED",
              0xFFFFFFFF: "UNDEFINED"}

# ESP-IDF 5.5 tools/idf_py_actions/qemu_ext.py: default eFuses of the esp32 target (chip rev 3)
ESP32_EFUSE = binascii.unhexlify(
    "00000000000000000000000000800000000000000000100000000000000000000000000000000000"
    "00000000000000000000000000000000000000000000000000000000000000000000000000000000"
    "00000000000000000000000000000000000000000000000000000000000000000000000000000000"
    "00000000")

# Printed by the ESP-IDF 4.4.4 of the C++ firmware at every start (see the module docstring)
CPP_STARTED = r"E \(\d+\) esp_core_dump_flash: No core dump partition found!"
RUST_BANNER = r"vdm-esp-fw \S+ boot"


def log(msg: str) -> None:
    print(msg, flush=True)


def ota_entry(seq: int, state: int) -> bytes:
    crc = binascii.crc32(struct.pack("<I", seq), 0xFFFFFFFF) & 0xFFFFFFFF
    entry = struct.pack("<I20sII", seq, b"\xff" * 20, state, crc)
    return entry + b"\xff" * (0x1000 - len(entry))


def otadata(entries: list[tuple[int, int] | None]) -> bytes:
    """Two sectors; None leaves a sector erased."""
    out = b""
    for e in entries:
        out += ota_entry(*e) if e else b"\xff" * 0x1000
    return out


def read_otadata(flash: bytes) -> list[str]:
    out = []
    for sector in range(2):
        e = flash[OTADATA_OFFSET + sector * 0x1000:OTADATA_OFFSET + sector * 0x1000 + 32]
        seq, = struct.unpack("<I", e[:4])
        state, crc = struct.unpack("<II", e[24:32])
        if seq == 0xFFFFFFFF:
            out.append(f"sector {sector}: erased")
            continue
        ok = crc == binascii.crc32(struct.pack("<I", seq), 0xFFFFFFFF) & 0xFFFFFFFF
        slot = f"app{(seq - 1) % 2}" if seq else "-"
        out.append(f"sector {sector}: seq {seq} -> {slot}, state {OTA_STATES.get(state, hex(state))}"
                   f"{'' if ok else ', CRC BAD'}")
    return out


def app_version(image: bytes) -> str:
    # esp_app_desc_t right after the image header (24 B) and the first segment header (8 B):
    # magic word, secure_version, reserved[2], version[32]
    magic, = struct.unpack("<I", image[32:36])
    if magic != 0xABCD5432:
        return "?"
    return image[48:80].split(b"\0")[0].decode("ascii", "replace")


def cpp_firmware_version(image: bytes) -> str:
    # Arduino-ESP32 puts the ESP-IDF version into esp_app_desc_t; the firmware's own version is
    # the VDM_VERSION literal ("2.1.7-revamped") in its constants
    m = re.search(rb"\x00(\d+\.\d+\.\d+-revamped)\x00", image)
    return m.group(1).decode() if m else "?"


class Flash:
    def __init__(self, path: str, bootloader: bytes, partitions: bytes):
        self.path = path
        self.img = bytearray(b"\xff" * FLASH_SIZE)
        self.put(BOOTLOADER_OFFSET, bootloader)
        self.put(PARTITIONS_OFFSET, partitions)

    def put(self, offset: int, data: bytes) -> "Flash":
        self.img[offset:offset + len(data)] = data
        return self

    def app(self, slot: int, image: bytes | None) -> "Flash":
        if image is not None:
            if len(image) > APP_SIZE:
                raise SystemExit(f"image of {len(image)} B does not fit a slot")
            self.put(APP0_OFFSET if slot == 0 else APP1_OFFSET, image)
        return self

    def write(self) -> None:
        with open(self.path, "wb") as f:
            f.write(self.img)

    def read(self) -> bytes:
        with open(self.path, "rb") as f:
            return f.read()


class Qemu:
    """One QEMU process; serial output collected line by line (also into a log file)."""

    def __init__(self, flash_path: str, efuse_path: str, log_path: str, port: int):
        self.lines: list[tuple[float, str]] = []
        self.cond = threading.Condition()
        self.log_path = log_path
        self.args = [
            "qemu-system-xtensa", "-M", "esp32", "-m", "4M",
            "-drive", f"file={flash_path},if=mtd,format=raw",
            "-drive", f"file={efuse_path},if=none,format=raw,id=efuse",
            "-global", "driver=nvram.esp32.efuse,property=drive,value=efuse",
            "-global", "driver=timer.esp32.timg,property=wdt_disable,value=true",
            "-nic", f"user,model=open_eth,hostfwd=tcp:127.0.0.1:{port}-:80",
            # -nographic: UART0 on stdout. With "-display none -serial stdio", or with a -monitor
            # of its own, this machine prints nothing; QEMU is stopped with SIGTERM (it shuts
            # down orderly and flushes the flash file).
            "-nographic",
        ]
        self.t0 = time.monotonic()
        self.proc = subprocess.Popen(self.args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                     stdin=subprocess.DEVNULL)
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()

    def _read(self) -> None:
        buf = b""
        with open(self.log_path, "a", encoding="utf-8") as logf:
            while True:
                chunk = self.proc.stdout.read1(4096) if self.proc.stdout else b""
                if not chunk:
                    break
                buf += chunk
                while b"\n" in buf:
                    raw, buf = buf.split(b"\n", 1)
                    line = raw.decode("latin-1").rstrip("\r")
                    t = time.monotonic() - self.t0
                    logf.write(f"[{t:7.2f}] {line}\n")
                    logf.flush()
                    with self.cond:
                        self.lines.append((t, line))
                        self.cond.notify_all()

    def wait_for(self, pattern: str, timeout: float, start: int = 0) -> tuple[int, re.Match]:
        rx = re.compile(pattern)
        deadline = time.monotonic() + timeout
        i = start
        with self.cond:
            while True:
                while i < len(self.lines):
                    m = rx.search(self.lines[i][1])
                    if m:
                        return i, m
                    i += 1
                left = deadline - time.monotonic()
                if left <= 0 or self.proc.poll() is not None:
                    raise TimeoutError(f"no serial line /{pattern}/ within {timeout:.0f} s")
                self.cond.wait(min(left, 0.5))

    def count(self, pattern: str, start: int = 0) -> int:
        rx = re.compile(pattern)
        with self.cond:
            return sum(1 for _, text in self.lines[start:] if rx.search(text))

    def line(self, i: int) -> str:
        t, text = self.lines[i]
        return f"[{t:7.2f}] {text}"

    def quit(self) -> None:
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(15)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.reader.join(5)


def http(port: int, method: str, path: str, body: bytes | None = None,
         timeout: float = 30) -> tuple[int, str]:
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=body, method=method)
    if body is not None:
        req.add_header("Content-Type", "application/octet-stream")
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode("utf-8", "replace")


def http_retry(port: int, path: str, timeout: float) -> tuple[int, str]:
    deadline = time.monotonic() + timeout
    last: Exception | None = None
    while time.monotonic() < deadline:
        try:
            return http(port, "GET", path, timeout=10)
        except (urllib.error.URLError, OSError, TimeoutError) as e:
            last = e
            time.sleep(1)
    raise TimeoutError(f"GET {path}: no answer within {timeout:.0f} s ({last})")


class Harness:
    def __init__(self, args: argparse.Namespace):
        self.args = args
        os.makedirs(args.workdir, exist_ok=True)
        self.bootloader = open(args.bootloader, "rb").read()
        self.partitions = open(args.partitions, "rb").read()
        self.cpp = open(args.cpp_image, "rb").read()
        self.cpp_version = cpp_firmware_version(self.cpp)
        self.efuse = os.path.join(args.workdir, "efuse.bin")
        with open(self.efuse, "wb") as f:
            f.write(ESP32_EFUSE)
        self.results: list[tuple[str, bool]] = []

    def image(self, path: str) -> bytes:
        data = open(path, "rb").read()
        log(f"  image {path}: {len(data)} B, version {app_version(data)}, "
            f"sha256 {hashlib.sha256(data).hexdigest()[:16]}")
        return data

    def flash(self, name: str) -> Flash:
        return Flash(os.path.join(self.args.workdir, f"{name}.flash.bin"), self.bootloader,
                     self.partitions)

    def qemu(self, flash: Flash, name: str) -> Qemu:
        logp = os.path.join(self.args.workdir, f"{name}.serial.log")
        with open(logp, "w", encoding="utf-8"):
            pass  # one log per run
        return Qemu(flash.path, self.efuse, logp, self.args.port)

    def evidence(self, q: Qemu, i: int, tag: str) -> None:
        log(f"  {tag}: {q.line(i)}")

    def cpp_started(self, q: Qemu, start: int, tag: str, timeout: float = 60) -> int:
        """The C++ firmware's start after `start`: the bootloader output, then its ESP-IDF line,
        and no Rust banner in between."""
        i, _ = q.wait_for(r"^entry 0x", 30, start)
        self.evidence(q, i, "bootloader starts the next app")
        j, _ = q.wait_for(CPP_STARTED, timeout, i)
        if q.count(RUST_BANNER, i) and q.wait_for(RUST_BANNER, 0, i)[0] < j:
            raise AssertionError("the Rust app started instead")
        self.evidence(q, j, f"{tag} (ESP-IDF 4.4.4 of the C++ firmware)")
        try:
            k, _ = q.wait_for(r"esp_littlefs", 20, j)
            self.evidence(q, k, "C++ firmware")
        except TimeoutError:
            pass
        return j

    def cpp_started_or_cold(self, q: Qemu, start: int, fl: Flash, name: str, tag: str) -> Qemu:
        """As cpp_started; when the C++ firmware prints nothing after the software restart (see
        the module docstring), the same flash is started again in a new QEMU process (power-on)
        and must start the C++ firmware. Returns the QEMU that is running."""
        try:
            self.cpp_started(q, start, tag + ", after the software restart", timeout=30)
            return q
        except TimeoutError as e:
            log(f"  no C++ line after the software restart ({e}); QEMU power-on from the same flash")
        q.quit()
        q2 = self.qemu(fl, name + "-poweron")
        try:
            self.cpp_started(q2, 0, tag + ", after a power-on")
        except Exception:
            q2.quit()
            raise
        return q2

    def run(self, name: str, fn) -> None:
        log(f"\n=== {name}")
        try:
            fn()
            ok = True
        except Exception as e:  # noqa: BLE001 - every failure ends as FAIL with its reason
            log(f"  FAIL: {type(e).__name__}: {e}")
            ok = False
        log(f"=== {name}: {'PASS' if ok else 'FAIL'}")
        self.results.append((name, ok))

    # (a) (b) (c), power cycle, switch-back
    def scenario_boot(self) -> None:
        rust = self.image(self.args.rust_image)
        fl = self.flash("boot").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, otadata([(1, 0xFFFFFFFF), (2, 0)]))
        fl.write()
        log("  otadata before: " + "; ".join(read_otadata(fl.read())))
        q = self.qemu(fl, "boot")
        try:
            i, _ = q.wait_for(r"^entry 0x", 30)
            self.evidence(q, i, "devices' bootloader")
            i, _ = q.wait_for(RUST_BANNER, 60, i)
            self.evidence(q, i, "(a) bootloader -> Rust app")
            i, m = q.wait_for(r"boot guard: app1 image (\w+) seq 2 on trial, boot attempt 1 of 3", 30, i)
            self.evidence(q, i, "guard")
            image_id = m.group(1)
            i, _ = q.wait_for(r"net: ethernet ip ", 60, i)
            self.evidence(q, i, "net")
            status, body = http_retry(self.args.port, "/api/health", 30)
            log(f"  (b) GET /api/health -> {status} {body.strip()}")
            if status != 200 or '"version"' not in body:
                raise AssertionError("health did not answer 200 with a document")
            i, _ = q.wait_for(rf"boot guard: image {image_id} seq 2 confirmed", 15, i)
            self.evidence(q, i, "(c) guard")
            status, body = http(self.args.port, "GET", "/api/health")
            log(f"  GET /api/health after confirmation -> {status} {body.strip()}")
        finally:
            q.quit()
        log("  otadata after: " + "; ".join(read_otadata(fl.read())))

        log("  power cycle (new QEMU process, RTC memory lost)")
        q = self.qemu(fl, "boot-powercycle")
        try:
            i, _ = q.wait_for(rf"boot guard: app1 image {image_id} seq 2 confirmed before", 60)
            self.evidence(q, i, "guard after power cycle")
            q.wait_for(r"net: ethernet ip ", 60, i)
            status, body = http_retry(self.args.port, "/api/health", 30)
            log(f"  GET /api/health -> {status}")
            status, body = http(self.args.port, "POST", "/api/system/switch-back", b"")
            log(f"  POST /api/system/switch-back -> {status} {body.strip()}")
            if status != 200:
                raise AssertionError("switch-back refused")
            i, _ = q.wait_for(r"boot guard: switching back to app0", 10, i)
            self.evidence(q, i, "guard")
            q = self.cpp_started_or_cold(q, i, fl, "boot-switchback", "C++ firmware after switch-back")
        finally:
            q.quit()
        log("  otadata after switch-back: " + "; ".join(read_otadata(fl.read())))

    # (d)
    def scenario_rollback(self) -> None:
        rust = self.image(self.args.rust_fail_image)
        fl = self.flash("rollback").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, otadata([(1, 0xFFFFFFFF), (2, 0)]))
        fl.write()
        log("  otadata before: " + "; ".join(read_otadata(fl.read())))
        q = self.qemu(fl, "rollback")
        try:
            i = 0
            for attempt in (1, 2, 3):
                i, _ = q.wait_for(rf"boot guard: app1 image \w+ seq 2 on trial, boot attempt {attempt} of 3", 90, i)
                self.evidence(q, i, f"attempt {attempt}")
                i, _ = q.wait_for(r"fail-boot: injected failure", 30, i)
                self.evidence(q, i, "panic")
                i, _ = q.wait_for(r"^rst:", 30, i)
                self.evidence(q, i, "reset")
            i, _ = q.wait_for(r"boot attempt 4 of 3", 90, i)
            self.evidence(q, i, "attempt 4")
            i, _ = q.wait_for(r"boot guard: switching back to app0", 30, i)
            self.evidence(q, i, "(d) guard")
            self.cpp_started(q, i, "(d) C++ firmware")
        finally:
            q.quit()
        ota = read_otadata(fl.read())
        log("  otadata after: " + "; ".join(ota))
        seq2 = [o for o in ota if "seq 2 " in o]
        if seq2 and "state NEW" in seq2[0]:
            log("  bootloader: sequence 2 still NEW after 4 boots of app1, so the devices' bootloader"
                " never set PENDING_VERIFY or ABORTED (it has no app rollback)")
        else:
            log(f"  bootloader: sequence 2 entry is {seq2}: the bootloader handled the trial state")

    def scenario_deadline(self) -> None:
        rust = self.image(self.args.rust_deadline_image)
        fl = self.flash("deadline").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, otadata([(1, 0xFFFFFFFF), (2, 0)]))
        fl.write()
        q = self.qemu(fl, "deadline")
        try:
            i, _ = q.wait_for(r"boot guard: app1 image \w+ seq 2 on trial, boot attempt 1 of 3", 60)
            self.evidence(q, i, "guard")
            i, _ = q.wait_for(r"net: ethernet ip ", 60, i)
            self.evidence(q, i, "net up, no HTTP request")
            i, _ = q.wait_for(r"boot guard: switching back to app0 .*\(not confirmed within 60 s\)", 90, i)
            self.evidence(q, i, "guard at the deadline")
            q = self.cpp_started_or_cold(q, i, fl, "deadline", "C++ firmware after the deadline")
        finally:
            q.quit()
        log("  otadata after: " + "; ".join(read_otadata(fl.read())))

    # (e)
    def scenario_ota(self) -> None:
        rust = self.image(self.args.rust_image)
        fl = self.flash("ota").app(1, rust)
        fl.put(OTADATA_OFFSET, otadata([(1, 0xFFFFFFFF), (2, 0)]))
        fl.write()
        log("  app0 erased; otadata before: " + "; ".join(read_otadata(fl.read())))
        q = self.qemu(fl, "ota")
        try:
            i, _ = q.wait_for(r"boot guard: app1 image \w+ seq 2: no valid image in the other slot", 60)
            self.evidence(q, i, "guard")
            i, _ = q.wait_for(r"net: ethernet ip ", 60, i)
            http_retry(self.args.port, "/api/health", 30)
            t0 = time.monotonic()
            status, body = http(self.args.port, "POST", "/api/ota", self.cpp, timeout=300)
            log(f"  POST /api/ota ({len(self.cpp)} B, C++ {self.cpp_version}) -> {status} "
                f"{body.strip()} in {time.monotonic() - t0:.1f} s")
            if status != 200:
                raise AssertionError("upload refused")
            i, _ = q.wait_for(r"ota: \d+ B written and selected", 30, i)
            self.evidence(q, i, "Rust app")
            q = self.cpp_started_or_cold(q, i, fl, "ota", "(e) C++ firmware from the uploaded image")
        finally:
            q.quit()
        log("  otadata after: " + "; ".join(read_otadata(fl.read())))
        written = fl.read()[APP0_OFFSET:APP0_OFFSET + len(self.cpp)]
        log(f"  app0 == uploaded image: {written == self.cpp}")
        if written != self.cpp:
            raise AssertionError("app0 differs from the uploaded image")

    def lfs_report(self, part: bytes, tag: str) -> dict:
        from littlefs import LittleFS, UserContext  # littlefs-python (Dockerfile)

        ctx = UserContext(len(part))
        ctx.buffer = bytearray(part)
        fs = LittleFS(context=ctx, block_size=4096, block_count=len(part) // 4096, mount=False,
                      read_size=128, prog_size=128, cache_size=512, lookahead_size=128)
        fs.mount()
        st = fs.fs_stat()
        files = {}
        for root, _dirs, names in fs.walk("/"):
            for n in names:
                p = root.rstrip("/") + "/" + n
                files[p] = fs.stat(p).size
        fs.unmount()
        ver = f"{st.disk_version >> 16}.{st.disk_version & 0xFFFF}"
        log(f"  littlefs-python, {tag}: disk version {ver}, name_max {st.name_max}, "
            f"blocks {st.block_count} x {st.block_size}, files {files}")
        return {"version": ver, "files": files}

    def scenario_littlefs(self) -> None:
        rust = self.image(self.args.rust_image)
        work = os.path.join(self.args.workdir, "littlefs")
        shutil.rmtree(work, ignore_errors=True)
        src = os.path.join(work, "src")
        # the C++ firmware's layout (DESIGN.md section 9)
        rnd = random.Random(217)
        content = {
            "log/events.log": b"".join(f"#{n} +{n}s INFO boot boot (reset poweron, count {n}, fw 2.1.7-revamped)\n".encode()
                                       for n in range(1, 400)),
            "stm/stm217.bin": bytes(rnd.getrandbits(8) for _ in range(70312)),
            "sys/cfg.bak": bytes(rnd.getrandbits(8) for _ in range(2048)),
            "HADiscovery.cfg": b"homeassistant/sensor/vdmot/valve1/config\n" * 12,
        }
        for rel, data in content.items():
            path = os.path.join(src, rel)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "wb") as f:
                f.write(data)
        image = os.path.join(work, "cpp-toolchain.littlefs.bin")
        out = subprocess.run(["mklittlefs", "-c", src, "-p", "256", "-b", "4096", "-s", str(SPIFFS_SIZE),
                              image], capture_output=True, text=True, check=True)
        log(f"  mklittlefs (PlatformIO tool-mklittlefs 1.203.210628) wrote {image}: {out.stdout.strip()!r}")
        part = open(image, "rb").read()
        before = self.lfs_report(part, "image of mklittlefs")

        fl = self.flash("littlefs").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, otadata([(1, 0xFFFFFFFF), (2, 0)]))
        fl.put(SPIFFS_OFFSET, part)
        fl.write()
        for run in (1, 2):
            q = self.qemu(fl, f"littlefs-{run}")
            try:
                i, m = q.wait_for(r"fs: mounted /littlefs, (\d+) entries", 60)
                self.evidence(q, i, f"Rust boot {run} mounts it")
                q.wait_for(r"^uart: ", 30, i)  # the listing is printed before this line
                listed = {}
                j = i + 1
                while j < len(q.lines) and q.lines[j][1].startswith("fs:   "):
                    name, size = q.lines[j][1][6:].rsplit(" ", 1)
                    listed[name] = int(size)
                    j += 1
                log(f"  Rust listing: {listed}")
                for rel, data in content.items():
                    if listed.get(f"/littlefs/{rel}") != len(data):
                        raise AssertionError(f"/littlefs/{rel}: listed {listed.get(f'/littlefs/{rel}')}, "
                                             f"written {len(data)}")
                q.wait_for(r"web: http server", 30, i)
            finally:
                q.quit()
        part = fl.read()[SPIFFS_OFFSET:SPIFFS_OFFSET + SPIFFS_SIZE]
        after = self.lfs_report(part, "after two boots of the Rust app")
        if after["version"] != before["version"]:
            raise AssertionError(f"disk version changed {before['version']} -> {after['version']}")
        if "/rust-spike.txt" not in after["files"]:
            raise AssertionError("the Rust app did not write its file")
        written = os.path.join(work, "after-rust.littlefs.bin")
        with open(written, "wb") as f:
            f.write(part)
        dest = os.path.join(work, "unpacked")
        out = subprocess.run(["mklittlefs", "-u", dest, "-p", "256", "-b", "4096", "-s",
                              str(SPIFFS_SIZE), written], capture_output=True, text=True)
        if out.returncode != 0:
            raise AssertionError(f"mklittlefs cannot unpack what the Rust app wrote: {out.stdout} {out.stderr}")
        for rel, data in content.items():
            if open(os.path.join(dest, rel), "rb").read() != data:
                raise AssertionError(f"{rel} changed")
        marker = open(os.path.join(dest, "rust-spike.txt"), "rb").read().decode()
        log(f"  mklittlefs unpacked the partition after the Rust app: all {len(content)} files "
            f"byte-identical, rust-spike.txt = {marker.strip().splitlines()}")


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("scenarios", nargs="*", default=["boot", "rollback", "deadline", "ota", "littlefs"])
    p.add_argument("--bootloader", default="/src/software_esp32/bootloader_dio_40m.bin")
    p.add_argument("--partitions", default="/src/software_esp32/partitions.bin")
    p.add_argument("--cpp-image", required=True)
    p.add_argument("--rust-image", default="/target/images/qemu/vdm-esp-fw.bin")
    p.add_argument("--rust-fail-image", default="/target/images/qemu-fail-boot/vdm-esp-fw.bin")
    p.add_argument("--rust-deadline-image", default="/target/images/qemu-short-deadline/vdm-esp-fw.bin")
    p.add_argument("--workdir", default="/target/qemu")
    p.add_argument("--port", type=int, default=18080)
    args = p.parse_args()
    h = Harness(args)
    log(f"bootloader {args.bootloader} sha256 {hashlib.sha256(h.bootloader).hexdigest()[:16]}, "
        f"partitions {args.partitions}, C++ image {args.cpp_image} ({h.cpp_version}, "
        f"sha256 {hashlib.sha256(h.cpp).hexdigest()[:16]})")
    for name in args.scenarios:
        fn = getattr(h, f"scenario_{name}", None)
        if fn is None:
            log(f"unknown scenario {name}")
            return 2
        h.run(name, fn)
    log("\nsummary: " + ", ".join(f"{n} {'PASS' if ok else 'FAIL'}" for n, ok in h.results))
    return 0 if all(ok for _, ok in h.results) else 1


if __name__ == "__main__":
    sys.exit(main())
