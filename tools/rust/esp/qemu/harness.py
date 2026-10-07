#!/usr/bin/env python3
"""End-to-end checks of the Rust firmware (software_esp32_rust/firmware, feature qemu) on
Espressif's QEMU (ESP32 machine, OpenETH NIC), with the bootloader and the partition table of
the devices and the C++ firmware 2.1.7 in the other OTA slot. Runs in the container of
tools/rust/esp/docker.sh ("qemu" subcommand).

Flash image: software_esp32/bootloader_dio_40m.bin @0x1000 (the devices' bootloader, IDF 4.4
era), software_esp32/partitions.bin @0x8000, otadata @0xE000, app0 @0x10000, app1 @0x150000,
spiffs (LittleFS) @0x290000. otadata is written as the C++ firmware leaves it after an OTA into
app1: sector 0 = sequence 1 (boot_app0.bin of the serial flash), sector 1 = sequence 2, state
NEW (the ESP-IDF libraries of Arduino-ESP32 2.0.7 are built with BOOTLOADER_APP_ROLLBACK_ENABLE).
That is the C++ -> Rust OTA: the C++ firmware cannot run its own upload in this QEMU (below).

The flash file is 16 MB, the devices' layout in its first 4 MB: QEMU picks the flash model by
the file size; with the 4 MB model (gd25q32) the C++ firmware stops at startup (its ESP-IDF
4.4.4 runs the flash in QIO and cannot set the quad-enable bit of that model).

What the C++ firmware 2.1.7 does in this QEMU: it starts (the bootloader's "entry" line, its
ESP-IDF line "esp_core_dump_flash: No core dump partition found!"); in most runs its setup then
mounts LittleFS (the mount fails: see below), prints its first event lines (boot, fs_formatted,
config_saved) and panics in its Ethernet init (LoadStorePIFAddrError on the ESP32 EMAC
registers: QEMU emulates OpenETH, not the EMAC), restarting in a loop; in other runs, mostly
after a power-on, its setup stalls for minutes before or after the mount. Its own flash I/O
does not work here: its QIO reads give garbage (a valid NVS is erased as unreadable, a valid
mklittlefs image does not mount), and GPIO2 reads LOW (the factory pin is held: a factory reset
at every start). So "the C++ firmware starts" is proven by the bootloader starting the C++
image and its ESP-IDF 4.4.4 startup line, with no Rust banner after it; its LittleFS mount and
its boot event ("fw 2.1.7-revamped") are logged when they come. What the C++ firmware reads of
NVS and LittleFS is proven with its own code outside QEMU: the C++ config codec of
software_esp32_revamped/lib/core compiled natively (cpp_config.cpp), ESP-IDF's NVS tools
(nvs_partition_gen, nvs_tool) and mklittlefs of the C++ toolchain.

The Rust image under test is the qemu variant: the same firmware with OpenETH instead of the
LAN8720, and a shutdown handler that stops the Ethernet driver before esp_restart (QEMU does not
reset the OpenETH model on a software restart; the ESP32 resets its EMAC there). GPIO2 reads LOW
here too: a scenario that keeps the device's settings writes NVS frLatch=1 first (the latch of
an earlier factory reset), so the boot skips the factory reset.

A software restart after the first minute of uptime does not get through ESP-IDF's startup in
this QEMU: the next boot takes an interrupt without a handler in esp_timer_init and restarts in
a loop, before main (a user restart at 30 s of uptime boots, at 70 s it loops); a restart into
the C++ image after Ethernet ran also hung silently in its startup once. The chip resets its
timer groups and its EMAC in esp_restart (DPORT_PERIP_RST_EN, DPORT_CORE_RST_EN), the QEMU model
does not. So every scenario whose restart leads into the C++ image or comes late (boot, ota,
netwatch, health, and rollback/deadline at their switch) ends the QEMU process at that restart,
checks the flash, and powers on a new process: the bootloader reads the same otadata then. The
early restarts of rollback and deadline (boots 1 to 4 of the Rust image) run through.

Scenarios (default: all but health):
  boot       the devices' bootloader boots the Rust app on trial (boot 1 of 3); GET /api/health;
             an ESP upload during the trial is refused (409 upload_failed "image on trial"); the
             trial is confirmed after 120 s of health (event app_marked_valid; NVS otaOk, no
             otaTrial); a power cycle boots it confirmed; POST /api/system/ota/switch-back
             restarts with otadata on app0, and the C++ firmware starts from it
  rollback   a Rust image that panics right after the boot guard (feature fail-boot): 3 counted
             boots, the 4th switches back to app0 without a verdict line; QEMU stops at that
             restart, the flash shows otadata selecting app0 (the devices' bootloader never set
             PENDING_VERIFY or ABORTED) and NVS otaTrial "switched back" after 4 boots, otaOk =
             the C++ image; then the C++ firmware starts from that flash
  deadline   a Rust image that never reaches the app thread (feature hang-setup): the boot
             deadline (60 s) restarts it, 3 counted boots, then the switch as in rollback
  badfs      a LittleFS partition of random bytes: the boot guard decides before the first
             LittleFS line and the first event (nothing that can fail runs before it, so a crash
             there is a counted boot, as rollback shows); the glue formats the partition (disk
             version 2.0) and the firmware comes up
  ota        the C++ image uploaded through POST /api/ota/esp (multipart, as the dashboard)
             into the empty other slot: app0 byte-identical and selected in otadata at the
             restart; the C++ firmware starts from it (power-on)
  netwatch   net.reconnectTimeoutMin 1 and a NIC without IPv4: the network watchdog restarts
             the interface after 60 s, then the ESP after 2 min; the next boot (power-on) is
             trial boot 2: a watchdog restart counts
  littlefs   a LittleFS image made by mklittlefs of the C++ toolchain with the C++ layout (log,
             STM image, config backup made by the C++ codec, discovery list): the Rust app
             restores its config from the C++ backup, lists the files, appends its events to the
             C++ log; mklittlefs unpacks the result, littlefs-python shows disk version 2.0
  nvs        an NVS made by ESP-IDF's generator with the C++ codec's cfg/cfgx blobs and the C++
             counters: the Rust app serves the same config document as the C++ codec; a config
             saved by the Rust app is decoded by the C++ codec from the NVS it wrote
  dashboard  the dashboard files: gzip bytes, ETag, Cache-Control, 304 for a matching
             If-None-Match (no Content-Type), the gunzipped files equal software_esp32_revamped/web
  api        every GET route against the document structure of software_esp32_revamped/tools/
             mock_api.py (the dashboard's contract), the 404/405/410 refusals, a config dry run
             and save, an STM image upload and delete, the log download (chunked); the heap and
             the httpd stack of /api/health after each kind of request
  mqtt       Mosquitto in the container (QEMU's user network: 10.0.2.2:1883): the app connects
             after a config save, publishes status online (retained); the idle heap 60 s after
             boot; values and the HA discovery (the broker holds every config the device
             reports); a broker restart is followed by a reconnect
  soak       web load: software_esp32_revamped/tools/loadtest.py (GET /api/status, /api/valves,
             /api/health and /; its request timeout raised from 5 to 30 s for QEMU) with 3,
             then 10 workers for 180 s each and a config POST every 15 s; free heap, its minimum
             and the largest block before, during and after, the 503 counts; no restart, the
             free heap back after the load. QEMU figures (OpenETH, no STM, WiFi off) are
             indicative only
  health     (not in the default set, ~17 min) a trial that needs the STM (NVS otaStm 1, as
             the uploading firmware writes it when its link was up; QEMU has no STM) with the
             network up: 15 min without 120 s of health, restart reason 4 (rollback, missing stm)
             into the C++ firmware without another counted boot; otadata and NVS as in
             rollback (one boot). Without a network the watchdog restarts first (netwatch)

Every scenario prints its evidence lines (serial log, HTTP answers, otadata, NVS) and PASS or
FAIL; the full serial logs are in --workdir.
"""
from __future__ import annotations

import argparse
import base64
import binascii
import gzip
import hashlib
import http.client
import json
import os
import random
import re
import shutil
import socket
import struct
import subprocess
import sys
import threading
import time

FLASH_SIZE = 16 * 1024 * 1024
BOOTLOADER_OFFSET = 0x1000
PARTITIONS_OFFSET = 0x8000
NVS_OFFSET = 0x9000
NVS_SIZE = 0x5000
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

# The device as QEMU's user network shows it: the guest address is also the Host header the
# request guard accepts (the interface address), whatever the station is called.
GUEST_IP = "10.0.2.15"
# Printed by the ESP-IDF 4.4.4 of the C++ firmware at every start
CPP_IDF_LINE = r"E \(\d+\) esp_core_dump_flash: No core dump partition found!"
# The LittleFS component of the C++ build (Arduino-ESP32 2.0.7) logs its mount errors under this
# path; the Rust build's component is ./managed_components/joltwallet__littlefs/
CPP_LFS_LINE = r"^\./components/esp_littlefs/src/littlefs/lfs\.c"
# The boot event of either firmware: "#1 +10s INFO boot boot (reset poweron, count 1, fw X)"
BOOT_EVENT = r"boot boot \(reset (\w+), count (\d+), fw (\S+)\)"
RUST_BANNER = r"vdm-esp-fw \S+ boot"
NET_UP = r"net_up network up \(eth, ([\d.]+)\)"
MARKED_VALID = r"app_marked_valid firmware marked valid after (\d+) s"
NVS_GEN = "/opt/espressif/esp-idf/v5.5.5/components/nvs_flash/nvs_partition_generator/nvs_partition_gen.py"
NVS_TOOL = "/opt/espressif/esp-idf/v5.5.5/components/nvs_flash/nvs_partition_tool/nvs_tool.py"
IDF_PY = "/opt/espressif/python_env/idf5.5_py3.11_env/bin/python"
CORE = "/src/software_esp32_revamped/lib/core"
WEB = "/src/software_esp32_revamped/web"
MOCK = "/src/software_esp32_revamped/tools/mock_api.py"
LOADTEST = "/src/software_esp32_revamped/tools/loadtest.py"
# the config POST of the soak scenario, every this many seconds during each load phase
SOAK_POST_S = 15


def log(msg: str) -> None:
    print(msg, flush=True)


def check(cond: bool, what: str) -> None:
    if not cond:
        raise AssertionError(what)


# ---------------------------------------------------------------- flash images


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


# otadata as the C++ firmware leaves it after uploading the Rust image into app1
AFTER_CPP_OTA = otadata([(1, 0xFFFFFFFF), (2, 0)])


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


def boot_slot(flash: bytes) -> str:
    """The slot the newest valid otadata entry selects."""
    best = None
    for sector in range(2):
        e = flash[OTADATA_OFFSET + sector * 0x1000:OTADATA_OFFSET + sector * 0x1000 + 32]
        seq, = struct.unpack("<I", e[:4])
        crc, = struct.unpack("<I", e[28:32])
        if seq != 0xFFFFFFFF and crc == binascii.crc32(struct.pack("<I", seq), 0xFFFFFFFF) & 0xFFFFFFFF:
            best = seq if best is None else max(best, seq)
    return "app0" if best is None else f"app{(best - 1) % 2}"


def app_version(image: bytes) -> str:
    # esp_app_desc_t right after the image header (24 B) and the first segment header (8 B):
    # magic word, secure_version, reserved[2], version[32]
    magic, = struct.unpack("<I", image[32:36])
    if magic != 0xABCD5432:
        return "?"
    return image[48:80].split(b"\0")[0].decode("ascii", "replace")


def app_id(image: bytes) -> bytes:
    """The boot guard's AppId: the first 8 bytes of esp_app_desc_t.app_elf_sha256."""
    return image[0xB0:0xB8]


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


# ---------------------------------------------------------------- QEMU


class Qemu:
    """One QEMU process; serial output collected line by line (also into a log file)."""

    def __init__(self, flash_path: str, efuse_path: str, log_path: str, port: int, dhcp: bool = True):
        self.lines: list[tuple[float, str]] = []
        self.cond = threading.Condition()
        self.log_path = log_path
        # dhcp=False: the NIC without IPv4 (no DHCP, so no address): the network never comes up
        # (without any NIC the qemu build faults on the OpenETH registers)
        opts = f"hostfwd=tcp:127.0.0.1:{port}-:80" if dhcp else "ipv4=off,ipv6=on"
        net = ["-nic", f"user,model=open_eth,{opts}"]
        self.args = [
            "qemu-system-xtensa", "-M", "esp32", "-m", "4M",
            "-drive", f"file={flash_path},if=mtd,format=raw",
            "-drive", f"file={efuse_path},if=none,format=raw,id=efuse",
            "-global", "driver=nvram.esp32.efuse,property=drive,value=efuse",
            "-global", "driver=timer.esp32.timg,property=wdt_disable,value=true",
            *net,
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

    def find(self, pattern: str, start: int = 0) -> list[re.Match]:
        rx = re.compile(pattern)
        with self.cond:
            return [m for _, text in self.lines[start:] for m in [rx.search(text)] if m]

    def line(self, i: int) -> str:
        t, text = self.lines[i]
        return f"[{t:7.2f}] {text}"

    def mark(self) -> int:
        with self.cond:
            return len(self.lines)

    def quit(self) -> None:
        if self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(15)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
        self.reader.join(5)


# ---------------------------------------------------------------- HTTP


class Answer:
    def __init__(self, status: int, headers: dict[str, str], body: bytes):
        self.status = status
        self.headers = headers
        self.body = body

    def text(self) -> str:
        return self.body.decode("utf-8", "replace")

    def json(self):
        return json.loads(self.body)

    def __repr__(self) -> str:
        t = self.text().strip()
        return f"{self.status} {t[:300]}{'...' if len(t) > 300 else ''}"


def request(port: int, method: str, path: str, body: bytes | None = None,
            headers: dict[str, str] | None = None, timeout: float = 60,
            host: str = GUEST_IP) -> Answer:
    """One request as the dashboard sends it: Host of the device, X-VdMot on writes. The
    timeout is generous: a LittleFS listing took over 30 s once in QEMU on a busy host."""
    h = {"Host": host}
    if method != "GET":
        h["X-VdMot"] = "1"
    if body is not None and not any(k.lower() == "content-type" for k in (headers or {})):
        h["Content-Type"] = "application/json"
    h.update(headers or {})
    c = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    try:
        c.request(method, path, body=body, headers=h)
        r = c.getresponse()
        data = r.read()
        return Answer(r.status, {k.lower(): v for k, v in r.getheaders()}, data)
    finally:
        c.close()


def get_retry(port: int, path: str, timeout: float) -> Answer:
    deadline = time.monotonic() + timeout
    last: Exception | None = None
    while time.monotonic() < deadline:
        try:
            return request(port, "GET", path, timeout=10)
        except (OSError, http.client.HTTPException) as e:
            last = e
            time.sleep(1)
    raise TimeoutError(f"GET {path}: no answer within {timeout:.0f} s ({last})")


def multipart(name: str, filename: str, data: bytes, fields=()) -> tuple[bytes, str]:
    b = "----vdmotQemuHarness7MA4YWxk"
    out = b""
    for k, v in fields:
        out += f'--{b}\r\nContent-Disposition: form-data; name="{k}"\r\n\r\n{v}\r\n'.encode()
    out += (f'--{b}\r\nContent-Disposition: form-data; name="{name}"; filename="{filename}"\r\n'
            "Content-Type: application/octet-stream\r\n\r\n").encode()
    out += data + f"\r\n--{b}--\r\n".encode()
    return out, f"multipart/form-data; boundary={b}"


# ---------------------------------------------------------------- NVS, C++ codec, MQTT


def nvs_image(work: str, entries: list[tuple[str, str, str, str]]) -> bytes:
    """An NVS partition made by ESP-IDF's generator: namespace vdmrev with `entries`
    (key, type, encoding, value; type "file" takes a file path)."""
    csv = os.path.join(work, "nvs.csv")
    with open(csv, "w") as f:
        f.write("key,type,encoding,value\nvdmrev,namespace,,\n")
        for e in entries:
            f.write(",".join(e) + "\n")
    out = os.path.join(work, "nvs.bin")
    subprocess.run([IDF_PY, NVS_GEN, "generate", csv, out, hex(NVS_SIZE)], check=True,
                   capture_output=True, text=True)
    return open(out, "rb").read()


def nvs_read(flash: bytes, work: str) -> dict[str, object]:
    """The vdmrev keys of the NVS in `flash` (ESP-IDF's nvs_tool): ints as ints, blobs as
    bytes (chunks joined in order)."""
    part = os.path.join(work, "nvs.read.bin")
    with open(part, "wb") as f:
        f.write(flash[NVS_OFFSET:NVS_OFFSET + NVS_SIZE])
    r = subprocess.run([IDF_PY, NVS_TOOL, "-f", "json", "-d", "minimal", part], check=True,
                       capture_output=True, text=True)
    out: dict[str, object] = {}
    for e in json.loads(r.stdout or "[]"):
        if e.get("namespace") != "vdmrev":
            continue
        key, data = e["key"], e["data"]
        if e["encoding"] in ("blob_data", "blob", "string"):
            raw = base64.b64decode(data) if isinstance(data, str) else bytes(data)
            out[key] = (out.get(key, b"") or b"") + raw
        else:
            out[key] = data
    return out


def nvs_page_versions(flash: bytes) -> set[int]:
    """The version byte of every initialised NVS page header (0xfe: ESP-IDF 4.x and 5.x)."""
    out = set()
    for p in range(NVS_SIZE // 0x1000):
        page = flash[NVS_OFFSET + p * 0x1000:NVS_OFFSET + p * 0x1000 + 32]
        if page[:4] != b"\xff\xff\xff\xff":
            out.add(page[8])
    return out


class CppCodec:
    """The config codec of the C++ firmware (lib/core, compiled natively)."""

    def __init__(self, work: str):
        self.exe = os.path.join(work, "cpp_config")
        src = sorted(os.path.join(CORE, "src", n) for n in os.listdir(os.path.join(CORE, "src"))
                     if n.endswith(".cpp"))
        r = subprocess.run(["g++", "-std=gnu++17", "-O1", "-w", "-I", os.path.join(CORE, "include"),
                            "-o", self.exe, "/src/tools/rust/esp/qemu/cpp_config.cpp", *src],
                           capture_output=True, text=True)
        if r.returncode != 0:
            raise RuntimeError("g++ of cpp_config.cpp failed:\n" + r.stderr[-2000:])

    def encode(self, out_dir: str, patch: dict) -> tuple[bytes, bytes]:
        os.makedirs(out_dir, exist_ok=True)
        subprocess.run([self.exe, "encode", out_dir, json.dumps(patch)], check=True,
                       capture_output=True, text=True)
        return (open(os.path.join(out_dir, "cfg.bin"), "rb").read(),
                open(os.path.join(out_dir, "cfgx.bin"), "rb").read())

    def decode(self, work: str, cfg: bytes, cfgx: bytes) -> dict:
        a, b = os.path.join(work, "dec.cfg.bin"), os.path.join(work, "dec.cfgx.bin")
        open(a, "wb").write(cfg)
        open(b, "wb").write(cfgx)
        r = subprocess.run([self.exe, "decode", a, b], check=True, capture_output=True, text=True)
        return json.loads(r.stdout)


class MqttSub:
    """A minimal MQTT 3.1.1 client: CONNECT, SUBSCRIBE '#', records every PUBLISH."""

    def __init__(self, port: int = 1883):
        self.msgs: list[tuple[str, bytes, bool]] = []
        self.lock = threading.Lock()
        self.s = socket.create_connection(("127.0.0.1", port), timeout=10)
        cid = b"qemu-harness"
        # keepalive 0: the broker never drops this listener for its silence (it sends nothing)
        var = b"\x00\x04MQTT\x04\x02\x00\x00"
        payload = struct.pack(">H", len(cid)) + cid
        self._send(0x10, var + payload)
        self._read_packet()  # CONNACK
        topic = b"#"
        self._send(0x82, struct.pack(">H", 1) + struct.pack(">H", len(topic)) + topic + b"\x00")
        self.s.settimeout(None)
        threading.Thread(target=self._loop, daemon=True).start()

    def _send(self, header: int, body: bytes) -> None:
        n, rl = len(body), b""
        while True:
            d, n = n % 128, n // 128
            rl += bytes([d | (0x80 if n else 0)])
            if not n:
                break
        self.s.sendall(bytes([header]) + rl + body)

    def _read_exact(self, n: int) -> bytes:
        out = b""
        while len(out) < n:
            c = self.s.recv(n - len(out))
            if not c:
                raise EOFError
            out += c
        return out

    def _read_packet(self) -> tuple[int, bytes]:
        h = self._read_exact(1)[0]
        mult, n = 1, 0
        while True:
            d = self._read_exact(1)[0]
            n += (d & 0x7F) * mult
            mult *= 128
            if not d & 0x80:
                break
        return h, self._read_exact(n)

    def _loop(self) -> None:
        try:
            while True:
                h, body = self._read_packet()
                if h >> 4 == 3:
                    tl, = struct.unpack(">H", body[:2])
                    topic = body[2:2 + tl].decode("utf-8", "replace")
                    rest = body[2 + tl:]
                    if (h >> 1) & 3:
                        rest = rest[2:]
                    with self.lock:
                        self.msgs.append((topic, rest, bool(h & 1)))
        except (EOFError, OSError):
            pass

    def topics(self) -> dict[str, bytes]:
        with self.lock:
            return {t: p for t, p, _ in self.msgs}

    def wait(self, pred, timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if pred(self.topics()):
                return True
            time.sleep(0.5)
        return False

    def close(self) -> None:
        try:
            self.s.close()
        except OSError:
            pass


def shape_diff(want, got, path: str = "") -> list[str]:
    """The keys and types of the mock's document `want` that `got` lacks or has otherwise; null
    on either side and the elements beyond the first of a list are not compared."""
    if want is None or got is None:
        return []
    if isinstance(want, dict):
        if not isinstance(got, dict):
            return [f"{path or '/'}: object expected, got {type(got).__name__}"]
        out = []
        for k, v in want.items():
            if k not in got:
                out.append(f"{path}/{k}: missing")
            else:
                out += shape_diff(v, got[k], f"{path}/{k}")
        return out
    if isinstance(want, list):
        if not isinstance(got, list):
            return [f"{path}: array expected, got {type(got).__name__}"]
        return shape_diff(want[0], got[0], f"{path}[0]") if want and got else []
    kind = lambda v: "bool" if isinstance(v, bool) else "number" if isinstance(v, (int, float)) \
        else type(v).__name__
    return [] if kind(want) == kind(got) else [f"{path}: {kind(want)} expected, got {kind(got)}"]


# ---------------------------------------------------------------- the harness


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
        self.stacks: dict[str, int] = {}
        self._codec: CppCodec | None = None

    @property
    def port(self) -> int:
        return self.args.port

    def codec(self) -> CppCodec:
        if self._codec is None:
            self._codec = CppCodec(self.args.workdir)
        return self._codec

    def image(self, path: str) -> bytes:
        data = open(path, "rb").read()
        log(f"  image {path}: {len(data)} B, version {app_version(data)}, "
            f"sha256 {hashlib.sha256(data).hexdigest()[:16]}, app id {app_id(data).hex()}")
        return data

    def flash(self, name: str) -> Flash:
        return Flash(os.path.join(self.args.workdir, f"{name}.flash.bin"), self.bootloader,
                     self.partitions)

    def qemu(self, flash: Flash, name: str, dhcp: bool = True) -> Qemu:
        logp = os.path.join(self.args.workdir, f"{name}.serial.log")
        with open(logp, "w", encoding="utf-8"):
            pass  # one log per run
        return Qemu(flash.path, self.efuse, logp, self.port, dhcp)

    def evidence(self, q: Qemu, i: int, tag: str) -> None:
        log(f"  {tag}: {q.line(i)}")

    def rust_up(self, q: Qemu, start: int = 0, timeout: float = 60) -> int:
        """The Rust app booted and its network is up; returns the index of the net_up line."""
        i, _ = q.wait_for(RUST_BANNER, timeout, start)
        i, _ = q.wait_for(NET_UP, timeout, i)
        get_retry(self.port, "/api/health", 30)
        return i

    def switch_boot(self, q: Qemu, start: int) -> int:
        """Boot 4 of a trial after `start`: the Rust app starts, its boot guard switches to the
        fallback and restarts before any verdict line; returns the index of that reset line."""
        i, _ = q.wait_for(RUST_BANNER, 30, start)
        self.evidence(q, i, "boot 4")
        j, _ = q.wait_for(r"^rst:", 30, i)
        check(q.count(r"boot guard: ", i) == 0 or q.wait_for(r"boot guard: ", 0, i)[0] > j,
              "boot 4 printed a verdict (a trial boot or a confirmation) instead of switching")
        self.evidence(q, j, "boot 4: no verdict, the guard switched and restarted")
        return j

    def cpp_started(self, q: Qemu, start: int, tag: str, timeout: float = 180) -> int:
        """The C++ firmware's start after `start`: the bootloader output and the ESP-IDF 4.4.4
        line of the C++ image, with no Rust banner after them. What its setup does next in this
        QEMU varies from run to run (GPIO2 reads LOW: a factory reset; its flash reads are
        garbage): its LittleFS mount (told from the Rust one by the component path) and its boot
        event ("fw <version>" without "-rust") came within seconds in most runs, after minutes or
        not at all in others, so they are logged when they come within 60 s."""
        i, _ = q.wait_for(r"^entry 0x", 30, start)
        self.evidence(q, i, "bootloader starts the next app")
        k, _ = q.wait_for(CPP_IDF_LINE, timeout, i)
        self.evidence(q, k, f"{tag}: ESP-IDF 4.4.4 of the C++ firmware")
        for pattern, what in ((CPP_LFS_LINE, "its setup mounts LittleFS"),
                              (BOOT_EVENT + r"$", "its boot event")):
            try:
                n, m = q.wait_for(pattern, 60, k)
            except TimeoutError:
                log(f"  {tag}: {what}: not within 60 s (this QEMU, see above)")
                break
            if what == "its boot event":
                check(not m.group(3).endswith("-rust"), f"the Rust app started instead ({m.group(3)})")
            self.evidence(q, n, f"{tag}: {what}")
            k = n
        check(q.count(RUST_BANNER, i) == 0 or q.wait_for(RUST_BANNER, 0, i)[0] > k,
              "the Rust app started instead")
        return k

    def health_stacks(self, a: Answer) -> None:
        """Keeps the lowest free stack of every task seen in a /api/health document."""
        try:
            doc = a.json()
        except ValueError:
            return
        for t in doc.get("tasks") or []:
            n, free = t.get("name"), t.get("minFree")
            if isinstance(free, int):
                self.stacks[n] = min(self.stacks.get(n, free), free)

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

    # ------------------------------------------------------------ boot chain, trial, switch back

    def scenario_boot(self) -> None:
        rust = self.image(self.args.rust_image)
        fl = self.flash("boot").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, AFTER_CPP_OTA)
        fl.write()
        log("  otadata before (as the C++ firmware leaves it after uploading the Rust image): "
            + "; ".join(read_otadata(fl.read())))
        q = self.qemu(fl, "boot")
        try:
            i, _ = q.wait_for(r"^entry 0x", 30)
            self.evidence(q, i, "devices' bootloader")
            i, _ = q.wait_for(RUST_BANNER, 60, i)
            self.evidence(q, i, "bootloader -> Rust app")
            i, _ = q.wait_for(r"boot guard: trial, boot 1 of 3", 30, i)
            self.evidence(q, i, "boot guard")
            i, m = q.wait_for(BOOT_EVENT, 60, i)
            self.evidence(q, i, "boot event")
            i, _ = q.wait_for(NET_UP, 60, i)
            self.evidence(q, i, "network")
            a = get_retry(self.port, "/api/health", 30)
            self.health_stacks(a)
            ota = a.json().get("ota")
            log(f"  GET /api/health -> {a.status}, ota {json.dumps(ota)}")
            check(a.status == 200 and ota and ota.get("checks") is not None, "health without the trial")
            body, ctype = multipart("file", "fw.bin", self.cpp)
            a = request(self.port, "POST", "/api/ota/esp", body, {"Content-Type": ctype}, timeout=120)
            log(f"  POST /api/ota/esp during the trial -> {a}")
            check(a.status == 409 and a.json() == {"error": "upload_failed", "detail": "image on trial"},
                  "an upload during the trial was not refused")
            i, m = q.wait_for(MARKED_VALID, 240, i)
            self.evidence(q, i, "trial confirmed")
            a = request(self.port, "GET", "/api/health")
            self.health_stacks(a)
            log(f"  GET /api/health after the confirmation -> ota {json.dumps(a.json().get('ota'))}")
        finally:
            q.quit()
        nvs = nvs_read(fl.read(), self.args.workdir)
        ok = nvs.get("otaOk")
        log(f"  NVS after: otaOk {ok.hex() if isinstance(ok, bytes) else ok}, otaTrial "
            f"{'present' if 'otaTrial' in nvs else 'absent'}")
        check(isinstance(ok, bytes) and ok[:4] == b"VDOK" and ok[4:12] == app_id(rust),
              "otaOk does not name the Rust image")
        check("otaTrial" not in nvs, "otaTrial left after the confirmation")
        log("  otadata after: " + "; ".join(read_otadata(fl.read())))

        log("  power cycle (new QEMU process, RTC memory lost)")
        q = self.qemu(fl, "boot-powercycle")
        try:
            i, _ = q.wait_for(r"boot guard: confirmed", 60)
            self.evidence(q, i, "boot guard after the power cycle")
            self.rust_up(q, 0)
            a = request(self.port, "POST", "/api/system/ota/switch-back", b"{}")
            log(f"  POST /api/system/ota/switch-back without confirm -> {a}")
            check(a.status == 400, "switch-back without confirm accepted")
            mark = q.mark()
            a = request(self.port, "POST", "/api/system/ota/switch-back", b'{"confirm":"switch-back"}')
            log(f"  POST /api/system/ota/switch-back -> {a}")
            check(a.status == 202 and a.json() == {"result": "restarting"}, "switch-back refused")
            i, _ = q.wait_for(r"reboot_requested restart requested \(switch back\)", 10, mark)
            self.evidence(q, i, "restart path")
            i, _ = q.wait_for(r"^rst:", 60, i)
            self.evidence(q, i, "software restart")
        finally:
            q.quit()
        after = fl.read()
        log("  otadata after the switch back: " + "; ".join(read_otadata(after)))
        check(boot_slot(after) == "app0", "otadata does not select app0")
        # a software restart into the C++ image does not always get through its startup in
        # QEMU (see the docstring): the bootloader reads the same otadata at a power-on
        log("  power on (new QEMU process) with that flash")
        q = self.qemu(fl, "boot-cpp")
        try:
            self.cpp_started(q, 0, "C++ firmware after the switch back")
        finally:
            q.quit()

    # ------------------------------------------------------------ crash loop

    def after_switch(self, fl: Flash, name: str, boots: int = 4) -> None:
        """The flash as the switch after `boots` boots left it (read before the C++ firmware
        runs: in this QEMU it erases the NVS it cannot read), then the C++ firmware's start."""
        after = fl.read()
        ota = read_otadata(after)
        log("  otadata after the switch: " + "; ".join(ota))
        check(boot_slot(after) == "app0", "otadata does not select app0")
        seq2 = [o for o in ota if "seq 2 " in o]
        check(bool(seq2) and "state NEW" in seq2[0],
              "the bootloader changed the trial state (it should have no app rollback)")
        log(f"  bootloader: sequence 2 still NEW after {boots} boot(s) of app1: the devices' "
            "bootloader never set PENDING_VERIFY or ABORTED")
        nvs = nvs_read(after, self.args.workdir)
        trial = nvs.get("otaTrial")
        ok = nvs.get("otaOk")
        log(f"  NVS: otaTrial {trial.hex() if isinstance(trial, bytes) else trial}, otaOk "
            f"{ok.hex() if isinstance(ok, bytes) else ok}")
        check(isinstance(trial, bytes) and trial[:4] == b"VDOT" and trial[5] == 2
              and trial[6] == boots, f"otaTrial is not 'switched back' after boot {boots}")
        check(isinstance(ok, bytes) and ok[4:12] == app_id(self.cpp),
              "otaOk does not name the C++ image switched to")
        log("  power on (new QEMU process) with that flash")
        q = self.qemu(fl, f"{name}-cpp")
        try:
            self.cpp_started(q, 0, "C++ firmware")
        finally:
            q.quit()

    def scenario_rollback(self) -> None:
        rust = self.image(self.args.rust_fail_image)
        fl = self.flash("rollback").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, AFTER_CPP_OTA)
        fl.write()
        q = self.qemu(fl, "rollback")
        try:
            i = 0
            for boot in (1, 2, 3):
                i, _ = q.wait_for(rf"boot guard: trial, boot {boot} of 3", 90, i)
                self.evidence(q, i, f"boot {boot}")
                i, _ = q.wait_for(r"fail-boot: injected failure", 30, i)
                self.evidence(q, i, "panic")
                i, _ = q.wait_for(r"^rst:", 30, i)
                self.evidence(q, i, "reset")
            self.switch_boot(q, i)
        finally:
            q.quit()
        self.after_switch(fl, "rollback")

    # ------------------------------------------------------------ boot deadline

    def scenario_deadline(self) -> None:
        rust = self.image(self.args.rust_deadline_image)
        fl = self.flash("deadline").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, AFTER_CPP_OTA)
        fl.write()
        q = self.qemu(fl, "deadline")
        try:
            i = 0
            for boot in (1, 2, 3):
                i, _ = q.wait_for(rf"boot guard: trial, boot {boot} of 3", 90, i)
                t0 = q.lines[i][0]
                self.evidence(q, i, f"boot {boot}")
                i, _ = q.wait_for(r"^rst:0xc \(SW_CPU_RESET\)", 90, i)
                log(f"  boot deadline restart after {q.lines[i][0] - t0:.0f} s: {q.line(i)}")
                check(55 <= q.lines[i][0] - t0 <= 75, "the restart did not come at the 60 s deadline")
            self.switch_boot(q, i)
        finally:
            q.quit()
        self.after_switch(fl, "deadline")

    # ------------------------------------------------------------ broken LittleFS

    def scenario_badfs(self) -> None:
        rust = self.image(self.args.rust_image)
        fl = self.flash("badfs").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, AFTER_CPP_OTA)
        fl.put(SPIFFS_OFFSET, random.Random(31).randbytes(SPIFFS_SIZE))
        fl.write()
        log("  LittleFS partition filled with random bytes")
        q = self.qemu(fl, "badfs")
        try:
            g, _ = q.wait_for(r"boot guard: trial, boot 1 of 3", 60)
            self.evidence(q, g, "boot guard")
            f, _ = q.wait_for(r"esp_littlefs", 60)
            self.evidence(q, f, "first LittleFS line")
            e, _ = q.wait_for(r"#\d+ ", 60)
            self.evidence(q, e, "first event line (logger)")
            check(g < f and g < e, "LittleFS or the logger ran before the boot guard decided")
            i, _ = q.wait_for(r"fs_formatted", 60, g)
            self.evidence(q, i, "the glue formats the partition it cannot mount")
            self.rust_up(q)
        finally:
            q.quit()
        r = self.lfs_report(fl.read()[SPIFFS_OFFSET:SPIFFS_OFFSET + SPIFFS_SIZE], "after the format")
        check(r["version"] == "2.0", "the format did not write disk version 2.0")

    # ------------------------------------------------------------ OTA Rust -> C++

    def scenario_ota(self) -> None:
        rust = self.image(self.args.rust_image)
        fl = self.flash("ota").app(1, rust)
        fl.put(OTADATA_OFFSET, AFTER_CPP_OTA)
        fl.write()
        log("  app0 erased; otadata before: " + "; ".join(read_otadata(fl.read())))
        q = self.qemu(fl, "ota")
        try:
            i, _ = q.wait_for(r"boot guard: confirmed", 60)
            self.evidence(q, i, "boot guard (no valid image in the other slot: confirmed)")
            i, _ = q.wait_for(r"esp_ota_failed .*-3", 60, i)
            self.evidence(q, i, "event 107 -3")
            i = self.rust_up(q, 0)
            body, ctype = multipart("file", "VdMot-Revamped_2.1.7.bin", self.cpp)
            t0 = time.monotonic()
            a = request(self.port, "POST", "/api/ota/esp", body, {"Content-Type": ctype}, timeout=300)
            log(f"  POST /api/ota/esp ({len(self.cpp)} B, C++ {self.cpp_version}) -> {a} in "
                f"{time.monotonic() - t0:.1f} s")
            check(a.status == 200 and a.json() == {"result": "ok", "restart": True}, "upload refused")
            try:
                # the stack peaks with the upload's flash writes, if the restart leaves time
                self.health_stacks(request(self.port, "GET", "/api/health", timeout=3))
            except (OSError, http.client.HTTPException):
                pass
            i, _ = q.wait_for(r"esp_ota_done", 10, i)
            self.evidence(q, i, "Rust app")
            i, _ = q.wait_for(r"reboot_requested restart requested \(ota\)", 10, i)
            self.evidence(q, i, "restart path")
            i, _ = q.wait_for(r"^rst:", 60, i)
            self.evidence(q, i, "software restart")
        finally:
            q.quit()
        after = fl.read()
        log("  otadata after: " + "; ".join(read_otadata(after)))
        written = after[APP0_OFFSET:APP0_OFFSET + len(self.cpp)]
        log(f"  app0 == uploaded image: {written == self.cpp}")
        check(written == self.cpp, "app0 differs from the uploaded image")
        check(boot_slot(after) == "app0", "otadata does not select app0")
        # a late software restart does not get through ESP-IDF's startup in QEMU (see the docstring)
        log("  power on (new QEMU process) with that flash")
        q = self.qemu(fl, "ota-cpp")
        try:
            self.cpp_started(q, 0, "C++ firmware from the uploaded image")
        finally:
            q.quit()

    # ------------------------------------------------------------ LittleFS both ways

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
        cfg, _cfgx = self.codec().encode(os.path.join(work, "codec"),
                                         {"station": "LfsCpp", "calib": {"hour": 4}})
        rnd = random.Random(217)
        # the C++ firmware's layout (DESIGN.md section 9); the config backup made by its codec
        cpp_log = b"".join(f"#{n} +{n}s INFO boot boot (reset poweron, count {n}, fw 2.1.7-revamped)\n"
                           .encode() for n in range(1, 400))
        content = {
            "log/events.log": cpp_log,
            "stm/stm217.bin": bytes(rnd.getrandbits(8) for _ in range(70312)),
            "sys/cfg.bak": cfg,
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
        fl.put(OTADATA_OFFSET, AFTER_CPP_OTA)
        fl.put(SPIFFS_OFFSET, part)
        # the latch of an earlier factory reset: GPIO2 reads LOW in QEMU
        fl.put(NVS_OFFSET, nvs_image(work, [("frLatch", "data", "u8", "1")]))
        fl.write()
        q = self.qemu(fl, "littlefs")
        try:
            i, _ = q.wait_for(r"config_restored", 60)
            self.evidence(q, i, "Rust app restores the config from the C++ backup file")
            self.rust_up(q, 0)
            a = request(self.port, "GET", "/api/config")
            log(f"  GET /api/config -> {a.status}, station {a.json().get('station')!r}, "
                f"calib {a.json().get('calib')}")
            check(a.json().get("station") == "LfsCpp" and a.json()["calib"]["hour"] == 4,
                  "the config of the C++ backup is not the one served")
            a = request(self.port, "GET", "/api/files")
            files = {f["path"]: f["size"] for f in a.json()["files"]}
            log(f"  GET /api/files -> {a.status} {files}")
            for rel, data in content.items():
                if rel != "log/events.log" and rel != "sys/cfg.bak":
                    check(files.get("/" + rel) == len(data), f"/{rel}: listed {files.get('/' + rel)}")
            # a download asks the logger for a flush and streams the files as they are (as the
            # C++ did): the lines of this boot are in the file once that flush ran
            for n in range(1, 7):
                a = request(self.port, "GET", "/api/log", timeout=60)
                text = a.text()
                if "fw 2.1.7-revamped-rust" in text[len(cpp_log):] or a.status != 200:
                    break
                time.sleep(5)
            log(f"  GET /api/log (download {n}) -> {a.status}, {len(a.body)} B, transfer-encoding "
                f"{a.headers.get('transfer-encoding')}; last line {text.strip().splitlines()[-1]!r}")
            check(a.status == 200 and a.headers.get("transfer-encoding") == "chunked", "log download")
            check(text.startswith(cpp_log.decode()), "the C++ lines are not the start of the log")
            check("fw 2.1.7-revamped-rust" in text[len(cpp_log):], "no Rust boot event in the log")
        finally:
            q.quit()
        part = fl.read()[SPIFFS_OFFSET:SPIFFS_OFFSET + SPIFFS_SIZE]
        after = self.lfs_report(part, "after the Rust app")
        check(after["version"] == before["version"] == "2.0",
              f"disk version changed {before['version']} -> {after['version']}")
        written = os.path.join(work, "after-rust.littlefs.bin")
        with open(written, "wb") as f:
            f.write(part)
        dest = os.path.join(work, "unpacked")
        out = subprocess.run(["mklittlefs", "-u", dest, "-p", "256", "-b", "4096", "-s",
                              str(SPIFFS_SIZE), written], capture_output=True, text=True)
        check(out.returncode == 0, f"mklittlefs cannot unpack what the Rust app wrote: {out.stdout} {out.stderr}")
        for rel in ("stm/stm217.bin", "HADiscovery.cfg"):
            check(open(os.path.join(dest, rel), "rb").read() == content[rel], f"{rel} changed")
        lines = open(os.path.join(dest, "log", "events.log"), "rb").read()
        check(lines.startswith(cpp_log), "the C++ log lines changed")
        decoded = self.codec().decode(work, open(os.path.join(dest, "sys", "cfg.bak"), "rb").read(), b"")
        log(f"  mklittlefs unpacked the partition: the C++ files byte-identical, events.log = the C++"
            f" lines + {len(lines) - len(cpp_log)} B of Rust lines, /sys/cfg.bak decoded by the C++"
            f" codec: station {decoded['station']!r}")
        check(decoded["station"] == "LfsCpp", "the backup the Rust app keeps is not readable by C++")

    # ------------------------------------------------------------ NVS both ways

    def scenario_nvs(self) -> None:
        rust = self.image(self.args.rust_image)
        work = os.path.join(self.args.workdir, "nvs")
        shutil.rmtree(work, ignore_errors=True)
        os.makedirs(work)
        codec = self.codec()
        patch = {"station": "NvsCpp",
                 "valves": [{"name": "Room1", "active": True, "failsafePct": 40}, {"name": "Room2"}],
                 "calib": {"dayMask": 5, "hour": 2, "minute": 15},
                 "mqtt": {"rootTopic": "heating", "keepAliveS": 30},
                 "failsafe": {"timeoutMin": 90}}
        cfg, cfgx = codec.encode(os.path.join(work, "cpp"), patch)
        want = codec.decode(work, cfg, cfgx)
        log(f"  C++ codec: cfg {len(cfg)} B, cfgx {len(cfgx)} B for {json.dumps(patch)}")
        nvs = nvs_image(work, [("cfg", "file", "binary", os.path.join(work, "cpp", "cfg.bin")),
                               ("cfgx", "file", "binary", os.path.join(work, "cpp", "cfgx.bin")),
                               ("boots", "data", "u32", "41"), ("imported", "data", "u8", "1"),
                               ("frLatch", "data", "u8", "1")])
        fl = self.flash("nvs").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, AFTER_CPP_OTA)
        fl.put(NVS_OFFSET, nvs)
        fl.write()
        q = self.qemu(fl, "nvs")
        try:
            i, m = q.wait_for(BOOT_EVENT, 60)
            self.evidence(q, i, "boot event (boot count of the C++ NVS + 1)")
            check(m.group(2) == "42", f"boot count {m.group(2)}, 42 expected")
            self.rust_up(q, 0)
            a = request(self.port, "GET", "/api/config")
            got = a.json()
            diff = [k for k in set(want) | set(got) if want.get(k) != got.get(k)]
            log(f"  GET /api/config -> {a.status}; equal to the C++ codec's document: {not diff}"
                f"{'' if not diff else ' (differs in ' + ', '.join(sorted(diff)) + ')'}")
            check(a.status == 200 and not diff, "the config document differs from the C++ one")
            save = {"valves": [{"name": "Bath", "active": True}], "calib": {"hour": 5},
                    "mqtt": {"rootTopic": "rust"}}
            a = request(self.port, "POST", "/api/config", json.dumps(save).encode())
            log(f"  POST /api/config {json.dumps(save)} -> {a.status}, restartRequired "
                f"{a.json().get('restartRequired')}")
            check(a.status == 200 and a.json().get("restartRequired") is False, "save refused")
            time.sleep(2)
        finally:
            q.quit()
        flash = fl.read()
        keys = nvs_read(flash, work)
        log(f"  NVS after (nvs_tool of ESP-IDF): keys {sorted(keys)}, page versions "
            f"{sorted(hex(v) for v in nvs_page_versions(flash))}")
        check(nvs_page_versions(flash) == {0xFE}, "an NVS page with another version than 0xfe")
        back = codec.decode(work, keys["cfg"], keys.get("cfgx", b""))
        log(f"  C++ codec reads the Rust save: valve 1 {back['valves'][0]}, calib {back['calib']}, "
            f"mqtt.rootTopic {back['mqtt']['rootTopic']!r}, station {back['station']!r}")
        check(back["valves"][0]["name"] == "Bath" and back["calib"]["hour"] == 5
              and back["mqtt"]["rootTopic"] == "rust" and back["station"] == "NvsCpp",
              "the C++ codec does not read what the Rust app saved")
        check(keys.get("boots") == 42, f"boots {keys.get('boots')}")

    # ------------------------------------------------------------ dashboard

    def scenario_dashboard(self) -> None:
        rust = self.image(self.args.rust_image)
        fl = self.flash("dashboard").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, AFTER_CPP_OTA)
        fl.write()
        q = self.qemu(fl, "dashboard")
        try:
            self.rust_up(q)
            for name in sorted(os.listdir(WEB)):
                url = "/" + name
                a = request(self.port, "GET", url)
                plain = gzip.decompress(a.body)
                same = plain == open(os.path.join(WEB, name), "rb").read()
                log(f"  GET {url} -> {a.status}, {len(a.body)} B gzip, type {a.headers.get('content-type')},"
                    f" etag {a.headers.get('etag')}, cache {a.headers.get('cache-control')}, "
                    f"gunzipped == web/{name}: {same}")
                check(a.status == 200 and a.headers.get("content-encoding") == "gzip" and same,
                      f"{url} not served as gzip of the source")
                check(a.headers.get("cache-control") == "no-cache", f"{url}: Cache-Control")
                etag = a.headers.get("etag")
                check(etag == '"%08x"' % (binascii.crc32(a.body) & 0xFFFFFFFF), f"{url}: ETag")
                b = request(self.port, "GET", url, headers={"If-None-Match": etag})
                log(f"  GET {url} If-None-Match {etag} -> {b.status}, {len(b.body)} B, content-type "
                    f"{b.headers.get('content-type')}")
                check(b.status == 304 and not b.body and "content-type" not in b.headers,
                      f"{url}: 304 expected without body and Content-Type")
            a = request(self.port, "GET", "/")
            log(f"  GET / -> {a.status}, {len(a.body)} B")
            check(a.status == 200 and gzip.decompress(a.body) == open(os.path.join(WEB, "index.html"), "rb").read(),
                  "/ is not index.html")
        finally:
            q.quit()

    # ------------------------------------------------------------ API

    def scenario_api(self) -> None:
        rust = self.image(self.args.rust_image)
        fl = self.flash("api").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, AFTER_CPP_OTA)
        fl.write()
        mock_port = self.port + 100
        mock = subprocess.Popen([sys.executable, MOCK, "--port", str(mock_port), "--bind", "127.0.0.1"],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        q = self.qemu(fl, "api")
        try:
            self.rust_up(q)
            get_retry(mock_port, "/api/health", 20)
            # the httpd stack after each kind of request (its peak is monotonic)
            web = ("httpd",)
            self.heap("after boot (only /api/health so far)", web)
            failures = []
            for path in ("/api/status", "/api/valves", "/api/sensors", "/api/events", "/api/config",
                         "/api/config/export", "/api/stm/motor", "/api/stm/images", "/api/stm/flash",
                         "/api/files", "/api/health"):
                a = request(self.port, "GET", path)
                m = request(mock_port, "GET", path, host=f"127.0.0.1:{mock_port}")
                if path == "/api/health":
                    self.health_stacks(a)
                try:
                    doc = a.json()
                except ValueError:
                    doc = None
                diff = shape_diff(m.json(), doc) if doc is not None and m.status == 200 else []
                log(f"  GET {path} -> {a.status} ({len(a.body)} B); structure of mock_api.py: "
                    f"{'same' if not diff else '; '.join(diff)}")
                if a.status not in (200, 409) or doc is None or diff:
                    failures.append(path)
            self.heap("after every GET route", web)
            a = request(self.port, "GET", "/api/config/export")
            log(f"  /api/config/export Content-Disposition: {a.headers.get('content-disposition')}")
            for method, path, want in (("GET", "/api/nothing", 404), ("DELETE", "/api/status", 405),
                                       ("GET", "/netinfo", 410), ("GET", "/valves", 200),
                                       ("GET", "/api/import-report", 404)):
                a = request(self.port, method, path, b"" if method != "GET" else None)
                log(f"  {method} {path} -> {a}")
                if a.status != want:
                    failures.append(f"{method} {path}")
            dry = {"station": "Kotlownia", "calib": {"hour": 1}}
            a = request(self.port, "POST", "/api/config?dryRun=1", json.dumps(dry).encode())
            log(f"  POST /api/config?dryRun=1 {json.dumps(dry)} -> {a}")
            if a.status != 200 or a.json() != {"restartRequired": True, "netTrial": False}:
                failures.append("dry run")
            a = request(self.port, "POST", "/api/config", b'{"calib":{"hour":7}}')
            log(f"  POST /api/config {{\"calib\":{{\"hour\":7}}}} -> {a.status}, calib "
                f"{a.json().get('calib')}, restartRequired {a.json().get('restartRequired')}")
            if a.status != 200 or a.json()["calib"]["hour"] != 7:
                failures.append("config save")
            a = request(self.port, "POST", "/api/config", b'{"calib":{"hour":25}}')
            log(f"  POST /api/config {{\"calib\":{{\"hour\":25}}}} -> {a}")
            if a.status != 400:
                failures.append("config refusal")
            self.heap("after the config POSTs", web)
            image = bytearray(b"\x80" * 8192)
            image[0:8] = struct.pack("<II", 0x20020000, 0x08000101)
            image[3000:3040] = b"\x01DEADBEEF\x00\x01BEEFIT\x00\x01" + b"2.1.0-revamped_C2\x00"
            body, ctype = multipart("file", "qemu.bin", bytes(image))
            a = request(self.port, "POST", "/api/stm/images", body, {"Content-Type": ctype}, timeout=60)
            log(f"  POST /api/stm/images (8 KiB image) -> {a}")
            if a.status != 201:
                failures.append("STM image upload")
            # the app thread validates a new image (reads the whole file); a DELETE in that
            # window fails with 500 io, as in C++ (LittleFS keeps a file with an open
            # descriptor): wait for the scan's result (crc32 or check) first
            deadline = time.monotonic() + 20
            while True:
                a = request(self.port, "GET", "/api/stm/images")
                scanned = a.status == 200 and any(e.get("crc32") or e.get("check") for e in a.json())
                if scanned or time.monotonic() > deadline:
                    break
                time.sleep(0.5)
            log(f"  GET /api/stm/images (scanned) -> {a}")
            a = request(self.port, "DELETE", "/api/stm/images/qemu")
            log(f"  DELETE /api/stm/images/qemu -> {a.status}")
            if a.status != 204:
                failures.append("STM image delete")
            self.heap("after the STM image upload and delete", web)
            a = request(self.port, "GET", "/api/log", timeout=60)
            log(f"  GET /api/log -> {a.status}, {len(a.body)} B, {a.headers.get('transfer-encoding')}")
            if a.status != 200 or a.headers.get("transfer-encoding") != "chunked":
                failures.append("log download")
            self.heap("after the log download", web)
            a = request(self.port, "GET", "/api/events?limit=5")
            log(f"  GET /api/events?limit=5 -> {a.status}, {len(a.json().get('events', []))} events")
            a = request(self.port, "GET", "/api/health")
            self.health_stacks(a)
            h = a.json()
            log(f"  GET /api/health heap {h.get('heap')}, tasks {h.get('tasks')}")
            check(not failures, "failed: " + ", ".join(failures))
        finally:
            q.quit()
            mock.terminate()

    # ------------------------------------------------------------ MQTT

    def scenario_mqtt(self) -> None:
        if shutil.which("mosquitto") is None:
            raise AssertionError("mosquitto is not installed in the container")
        rust = self.image(self.args.rust_image)
        fl = self.flash("mqtt").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, AFTER_CPP_OTA)
        fl.write()
        conf = os.path.join(self.args.workdir, "mosquitto.conf")
        with open(conf, "w") as f:
            f.write("listener 1883 127.0.0.1\nallow_anonymous true\npersistence false\n")
        broker = subprocess.Popen(["mosquitto", "-c", conf], stdout=subprocess.DEVNULL,
                                  stderr=subprocess.DEVNULL)
        q = self.qemu(fl, "mqtt")
        sub = None
        try:
            time.sleep(1)
            sub = MqttSub()
            self.rust_up(q)
            cfg = {"mqtt": {"mode": 2, "host": "10.0.2.2", "port": 1883, "separate": True,
                            "haDiscoveryOnConnect": True, "events": True, "newDiag": True}}
            a = request(self.port, "POST", "/api/config", json.dumps(cfg).encode())
            log(f"  POST /api/config {json.dumps(cfg)} -> {a.status}")
            check(a.status == 200, "MQTT settings refused")
            ok = sub.wait(lambda t: t.get("VdMot/status") == b"online", 60)
            log(f"  broker: VdMot/status = {sub.topics().get('VdMot/status')!r}")
            check(ok, "no 'online' status")
            # the idle figures of GLUE-DESIGN-ESP.md 2.3: 60 s after the boot, network and MQTT up
            while (request(self.port, "GET", "/api/health").json().get("uptime") or 0) < 60:
                time.sleep(2)
            self.heap("60 s after boot, Ethernet up, MQTT connected (idle)", ("mqtt", "httpd"))
            # the run after a connect waits for the STM inputs, at most 120 s (core
            # DiscoveryGate::MAX_WAIT_MS); QEMU has no STM, so it starts when that wait ends
            ok = sub.wait(lambda t: any(k.startswith("homeassistant/") and k.endswith("/config") and v
                                        for k, v in t.items()), 240)
            check(ok, "no HA discovery")
            # the run publishes over several passes: count once the device says it is done
            i, m = q.wait_for(r"ha_discovery_sent HA discovery sent \((\d+) configs, (\d+) deletes", 60)
            self.evidence(q, i, "device")
            time.sleep(2)
            disc = {k: v for k, v in sub.topics().items() if k.startswith("homeassistant/")}
            configs = sorted(k for k, v in disc.items() if v)
            log(f"  HA discovery at the broker: {len(configs)} config topics, "
                f"{len(disc) - len(configs)} deleted (empty retained), e.g. {configs[:3]}")
            check(len(configs) == int(m.group(1)), "the broker holds another number of configs")
            self.heap("after the HA discovery run", ("mqtt",))
            sub.wait(lambda t: len(t) > len(disc) + 20, 30)
            values = sorted(k for k in sub.topics() if not k.startswith("homeassistant/"))
            log(f"  device topics: {len(values)}, e.g. {values[:6]}")
            a = request(self.port, "GET", "/api/status")
            log(f"  GET /api/status mqtt {a.json().get('mqtt')}")
            check(a.json()["mqtt"]["state"] == "connected", "status does not say connected")
            mark = q.mark()
            broker.terminate()
            broker.wait(10)
            i, _ = q.wait_for(r"mqtt_disconnected|mqtt_connect_failed", 60, mark)
            self.evidence(q, i, "broker gone")
            broker = subprocess.Popen(["mosquitto", "-c", conf], stdout=subprocess.DEVNULL,
                                      stderr=subprocess.DEVNULL)
            time.sleep(1)
            sub.close()
            sub = MqttSub()
            i, _ = q.wait_for(r"mqtt_connected", 120, mark)
            self.evidence(q, i, "reconnected")
            check(sub.wait(lambda t: t.get("VdMot/status") == b"online", 60), "no 'online' after the reconnect")
            a = request(self.port, "GET", "/api/health")
            self.health_stacks(a)
            log(f"  GET /api/health tasks {a.json().get('tasks')}")
        finally:
            q.quit()
            if sub:
                sub.close()
            broker.terminate()

    # ------------------------------------------------------------ heap under web load

    def heap(self, tag: str, tasks: tuple[str, ...] = ()) -> dict:
        """GET /api/health: logs the heap figures and the stack used so far by `tasks`."""
        a = request(self.port, "GET", "/api/health")
        self.health_stacks(a)
        doc = a.json()
        h = doc.get("heap") or {}
        used = [f"{t['name']} {t['stack'] - t['minFree']} B of {t['stack']}"
                for t in doc.get("tasks") or [] if t.get("name") in tasks]
        log(f"  heap {tag}: free {h.get('free')}, min {h.get('min')}, largest {h.get('largest')}, "
            f"minLargest {h.get('minLargest')}, uptime {doc.get('uptime')} s"
            + (f"; stack used: {', '.join(used)}" if used else ""))
        return h

    def scenario_soak(self) -> None:
        rust = self.image(self.args.rust_image)
        fl = self.flash("soak").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, AFTER_CPP_OTA)
        fl.write()
        q = self.qemu(fl, "soak")
        try:
            self.rust_up(q)
            # loadtest.py names the device 127.0.0.1:<port> (QEMU's forwarded port); the
            # request guard admits that Host once it is in web.allowedHosts
            a = request(self.port, "POST", "/api/config", b'{"web":{"allowedHosts":"127.0.0.1"}}')
            log(f"  POST /api/config web.allowedHosts 127.0.0.1 -> {a.status}")
            check(a.status == 200, "allowedHosts refused")
            time.sleep(10)
            before = self.heap("before (idle, after boot)")
            for workers in (3, 10):
                posts: dict = {}
                stop = threading.Event()

                def poster() -> None:
                    hour = 6
                    while not stop.wait(SOAK_POST_S):
                        hour = 13 - hour  # 6, 7, 6, ...
                        try:
                            code = request(self.port, "POST", "/api/config",
                                           json.dumps({"calib": {"hour": hour}}).encode(),
                                           timeout=120).status
                        except (OSError, http.client.HTTPException) as e:
                            code = type(e).__name__
                        posts[code] = posts.get(code, 0) + 1

                t = threading.Thread(target=poster, daemon=True)
                t.start()
                # loadtest.py as it is, its 5 s request timeout raised to 30 s: QEMU (TCG on a
                # shared host) answers a request in 0.01 to 4 s, the device in milliseconds
                wrapper = ("import runpy, sys, urllib.request as u; o = u.urlopen; "
                           "u.urlopen = lambda *a, **k: o(*a, **{**k, 'timeout': 30}); "
                           "sys.argv = sys.argv[1:]; runpy.run_path(sys.argv[0], run_name='__main__')")
                r = subprocess.run([sys.executable, "-c", wrapper, LOADTEST, f"127.0.0.1:{self.port}",
                                    "--workers", str(workers), "--seconds", "180"],
                                   capture_output=True, text=True, timeout=900)
                stop.set()
                t.join(150)
                lines = r.stdout.strip().splitlines()
                log(f"  loadtest.py --workers {workers} --seconds 180 (GET /api/status, /api/valves, "
                    f"/api/health, /; request timeout 30 s), a config POST every {SOAK_POST_S} s:")
                samples = [m for m in (re.match(r"t\+\d+s free (\d+) largest (\d+)", ln) for ln in lines) if m]
                if samples:
                    free = [int(m.group(1)) for m in samples]
                    big = [int(m.group(2)) for m in samples]
                    log(f"    during, {len(samples)} samples of /api/health: free {min(free)}..{max(free)}, "
                        f"largest block {min(big)}..{max(big)}")
                for ln in lines:
                    if not ln.startswith("t+"):
                        log(f"    {ln}")
                    elif ln == [x for x in lines if x.startswith("t+")][-1]:
                        log(f"    last sample: {ln}")
                log(f"    config POSTs: {posts}")
                check(r.returncode == 0, f"loadtest.py failed ({r.returncode}): {r.stderr[-300:]}")
                check(any(ln.startswith("min_free") and "rebooted False" in ln for ln in lines),
                      "the device restarted under load")
                check(not any(ln.startswith("STOP early") for ln in lines), "loadtest stopped early")
                # HTTP answers: 200 or 503 only, at least one save; a connection reset or a
                # timeout is the connection cap of 4 (a fifth client purges the least recently
                # used session, GLUE-DESIGN-ESP.md 4.7) and is only counted
                answers = {c for c in posts if isinstance(c, int)}
                check(answers <= {200, 503} and 200 in answers, f"config POST answers {posts}")
                # the connections of the load end with it; lwIP keeps closed ones in TIME_WAIT
                # for 2 MSL (120 s), the web server's lists and buffers are freed per request
                t0, series = time.monotonic(), []
                while True:
                    time.sleep(10)
                    h = request(self.port, "GET", "/api/health").json().get("heap") or {}
                    series.append(f"+{time.monotonic() - t0:.0f}s {h.get('free')}")
                    back = h.get("free", 0) >= before.get("free", 0) - 4096
                    if back or time.monotonic() - t0 > 180:
                        break
                log(f"    free heap after the load: {', '.join(series)}")
                self.heap(f"after {workers} workers")
                check(back, "the free heap did not come back within 3 min of the load (leak)")
            log("  2.1.7 for comparison (GLUE-DESIGN-ESP.md 2.3, device, Ethernet): ~123 KB free and "
                "~108 KB minimum idle; under parallel web load (CHANGELOG 2.1.6) at least 80 KB free. "
                "QEMU (OpenETH, no STM, WiFi off) is indicative only.")
        finally:
            q.quit()

    # ------------------------------------------------------------ network watchdog

    def nvs_with(self, name: str, patch: dict, extra: list[tuple[str, str, str, str]]) -> bytes:
        """An NVS with the C++ codec's config for `patch` (settings kept: frLatch, imported)."""
        work = os.path.join(self.args.workdir, name)
        shutil.rmtree(work, ignore_errors=True)
        os.makedirs(work)
        self.codec().encode(os.path.join(work, "cpp"), patch)
        return nvs_image(work, [("cfg", "file", "binary", os.path.join(work, "cpp", "cfg.bin")),
                                ("cfgx", "file", "binary", os.path.join(work, "cpp", "cfgx.bin")),
                                ("imported", "data", "u8", "1"), ("frLatch", "data", "u8", "1"),
                                *extra])

    def scenario_netwatch(self) -> None:
        rust = self.image(self.args.rust_image)
        fl = self.flash("netwatch").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, AFTER_CPP_OTA)
        fl.put(NVS_OFFSET, self.nvs_with("netwatch", {"net": {"reconnectTimeoutMin": 1}}, []))
        fl.write()
        log("  config net.reconnectTimeoutMin 1; a NIC without IPv4 (no address)")
        q = self.qemu(fl, "netwatch", dhcp=False)
        try:
            i, _ = q.wait_for(r"boot guard: trial, boot 1 of 3", 60)
            self.evidence(q, i, "boot 1")
            i, _ = q.wait_for(r"net_interface_restart", 150, i)
            self.evidence(q, i, "the watchdog restarts the interface")
            i, _ = q.wait_for(r"reboot_requested restart requested \(net watchdog", 150, i)
            self.evidence(q, i, "then the ESP")
            i, _ = q.wait_for(r"^rst:", 60, i)
            self.evidence(q, i, "software restart")
        finally:
            q.quit()
        # a late software restart does not get through ESP-IDF's startup in QEMU (see the docstring):
        # the next boot is a power-on, which loses the RTC mirror; NVS still counts
        log("  power on (new QEMU process) with that flash")
        q = self.qemu(fl, "netwatch-2", dhcp=False)
        try:
            i, _ = q.wait_for(r"boot guard: trial, boot 2 of 3", 60)
            self.evidence(q, i, "the watchdog restart was a counted boot")
        finally:
            q.quit()

    # ------------------------------------------------------------ 15 min without health

    def scenario_health(self) -> None:
        rust = self.image(self.args.rust_image)
        fl = self.flash("health").app(0, self.cpp).app(1, rust)
        fl.put(OTADATA_OFFSET, AFTER_CPP_OTA)
        # otaStm 1: the uploading firmware saw the STM link up, so the trial needs it; QEMU has
        # no STM, so the image never has 120 s of health while its network is fine
        fl.put(NVS_OFFSET, self.nvs_with("health", {}, [("otaStm", "data", "u8", "1")]))
        fl.write()
        q = self.qemu(fl, "health")
        try:
            i, _ = q.wait_for(r"boot guard: trial, boot 1 of 3, stm required true", 60)
            self.evidence(q, i, "boot guard")
            self.rust_up(q)
            a = request(self.port, "GET", "/api/health")
            log(f"  GET /api/health -> {a.status}, ota {json.dumps(a.json().get('ota'))}")
            j, _ = q.wait_for(r"reboot_requested restart requested \(rollback", 16 * 60, i)
            self.evidence(q, j, "15 min without 120 s of health")
            check(q.count(r"boot guard: trial, boot 2", i) == 0, "the image restarted on its own")
            k, _ = q.wait_for(r"^rst:", 60, j)
            self.evidence(q, k, "restart into the fallback")
        finally:
            q.quit()
        self.after_switch(fl, "health", boots=1)


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("scenarios", nargs="*",
                   default=["boot", "rollback", "deadline", "badfs", "ota", "netwatch", "littlefs", "nvs",
                            "dashboard", "api", "mqtt", "soak"])
    p.add_argument("--bootloader", default="/src/software_esp32/bootloader_dio_40m.bin")
    p.add_argument("--partitions", default="/src/software_esp32/partitions.bin")
    p.add_argument("--cpp-image", required=True)
    p.add_argument("--rust-image", default="/target/images/qemu/vdm-esp-fw.bin")
    p.add_argument("--rust-fail-image", default="/target/images/qemu-fail-boot/vdm-esp-fw.bin")
    p.add_argument("--rust-deadline-image", default="/target/images/qemu-hang-setup/vdm-esp-fw.bin")
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
    if h.stacks:
        log("\nlowest free stack per task seen in /api/health: "
            + ", ".join(f"{k} {v} B" for k, v in sorted(h.stacks.items())))
    log("\nsummary: " + ", ".join(f"{n} {'PASS' if ok else 'FAIL'}" for n, ok in h.results))
    return 0 if all(ok for _, ok in h.results) else 1


if __name__ == "__main__":
    sys.exit(main())
