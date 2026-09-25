#!/usr/bin/env python3
"""Mock of the VdMot Revamped ESP32 HTTP API for previewing the dashboard.

Serves web/ (uncompressed) and a stateful, simulated /api/* that follows
DESIGN.md "HTTP API": valves move towards their targets, calibrations run and
fail for the blocked valves, events accumulate, uploads and STM flashing show
progress, and restarts make the device briefly unreachable.

    python3 tools/mock_api.py                     # station "east" on :8080
    python3 tools/mock_api.py --station west --port 8081
    python3 tools/mock_api.py --proto 1           # legacy STM 1.4.x (no gvlvx/gprof/svmov)
    python3 tools/mock_api.py --auth admin:secret # HTTP Basic auth like web.user/web.password
    python3 tools/mock_api.py --proto 3           # STM protocol 3: failsafe lease, stop, safe mode
    python3 tools/mock_api.py --scenario health,busy,queue,failsafe,safemode,tooold,haoffline
    python3 tools/mock_api.py --import-report     # a legacy import report (notice N2)
    python3 tools/mock_api.py --station-name "Dom Północ"

The mock follows the firmware's request guard: Host must name the device
(127.0.0.1, localhost, the station's host name), API writes need the header
"X-VdMot: 1" and JSON bodies "Content-Type: application/json".

Standard library only. Development tool; never part of the firmware image.
"""
from __future__ import annotations

import argparse
import base64
import copy
import datetime as dt
import hashlib
import json
import math
import os
import random
import re
import threading
import time
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlsplit

WEB_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "web")
ESP_VERSION = "2.1.0-revamped-dev"
STM_VERSION_V3 = "2.1.0-revamped_C2"
STM_VERSION_V2 = "2.0.0-revamped_C2"
STM_VERSION_V1 = "1.4.9_Dev_C2"
STM_VERSION_OLD = "1.3.9_C2"
MAX_BODY = 8192
TRIAL_S = 120
STM_IMAGE_MAX = 512 * 1024
ESP_APP_MAX = 0x140000

# name, blocked calibration (oc, cc, cr) or None
STATIONS = {
    "east": [("Korytarz1", None), ("Pokoj1", None), ("Pokoj2", None), ("Korytarz2", None),
             ("Lazienka", None), ("Garderoba", (67, 82, 3)), ("Sypialnia", None)],
    "west": [("Toaleta", (72, 100, 3)), ("Kuchnia", None), ("Salon", None), ("Wiatrolap", None),
             ("Gabinet", None), ("Pokoj3", None)],
}

SEVERITIES = ["debug", "info", "warning", "error", "critical"]
EVENT_NAMES = {
    100: "boot", 102: "config_saved", 106: "esp_ota_done", 109: "reboot_requested", 111: "time_synced",
    200: "net_up", 202: "mqtt_connected", 205: "ha_discovery_sent", 300: "link_up", 304: "stm_reset_by_user",
    306: "stm_version", 311: "stm_flash_started", 312: "stm_flash_done", 400: "target_set",
    401: "valve_state_changed", 402: "valve_blocked", 406: "calib_started", 407: "calib_ok",
    408: "calib_retry", 409: "calib_failed", 410: "early_stop", 411: "cmd_rejected",
    414: "service_move_done", 502: "sensor_count_changed", 600: "scheduled_calibration",
}

INT_RANGES = {
    "net.iface": (0, 2), "net.reconnectTimeoutMin": (0, 240), "syslog.level": (0, 3), "syslog.port": (1, 65535),
    "mqtt.mode": (0, 2), "mqtt.port": (1, 65535), "mqtt.keepAliveS": (5, 300), "mqtt.publishIntervalS": (2, 3600),
    "mqtt.minDelayS": (0, 3600), "calib.dayMask": (0, 127), "calib.hour": (0, 23), "calib.minute": (0, 59),
}
STR_RANGES = {
    "station": (1, 20), "net.ssid": (0, 32), "time.ntpServer": (0, 64), "time.tzName": (0, 49),
    "time.tzPosix": (1, 49), "web.user": (0, 64), "mqtt.host": (0, 64), "mqtt.user": (0, 64),
}
SECRETS = {"net.wifiPassword": 63, "web.password": 64, "mqtt.password": 64}
IP_KEYS = {"net.ip", "net.mask", "net.gateway", "net.dns", "syslog.server"}
ONEWIRE_RE = re.compile(r"^[0-9a-fA-F]{2}(-[0-9a-fA-F]{2}){7}$")
SAFE_BAD = re.compile(r'[+#/"\\]')
SEG_RE = re.compile(r"^[A-Za-z0-9_-]*$")
PATH_RE = re.compile(r"^([A-Za-z0-9_-]+(/[A-Za-z0-9_-]+)*)?$")
# The legacy paths (spec-web section 4): 410 with the replacement.
LEGACY_GONE = dict(
    [(x, "/api/status") for x in ("/netinfo", "/sysinfo", "/sysdyninfo", "/update/identity")] +
    [(x, "/api/config") for x in ("/netconfig", "/protconfig", "/valvesconfig", "/tempsconfig", "/voltsconfig",
                                  "/sysconfig", "/sysLogCfg")] +
    [("/motorconfig", "/api/stm/motor"), ("/tempsensorsid", "/api/sensors"), ("/voltsensorsid", "/api/sensors"),
     ("/fsdir", "/api/files"), ("/fupload", "/api/stm/images"), ("/stmupdate", "/#maintenance"),
     ("/stmupdstatus", "/api/stm/flash"), ("/stmdoupdate", "/api/stm/flash"), ("/update", "/api/ota/esp"),
     ("/cmd", "/api/system/reboot, /api/valves/calibrate, /api/valves/assembly, /api/valves/detect, "
              "/api/sensors/scan, /api/mqtt/reconnect, /api/mqtt/discovery"),
     ("/valvesctrlconfig", "removed: PI control"), ("/msgconfig", "removed: messenger"),
     ("/testPO", "removed: messenger"), ("/testEmail", "removed: messenger"), ("/ssidinfo", "removed: WiFi scan"),
     ("/auth", "removed: HTTP Basic auth is used")])
LEGACY_ALIASES = {"/valves": "GET", "/temps": "GET", "/volts": "GET", "/setvalve": "POST"}
DEMO_IMPORT_REPORT = {"imported": 57, "rejected": 2, "ignored": 14, "firstRejected": "valvesCfg/valves.5.name",
                      "piValves": 3, "windowValves": 1, "dropped": ["pi", "window", "messenger", "ds18Timeout"],
                      "legacyFailsafe": {"enabled": True, "timeoutMin": 120, "pct": 10}, "rootTopic": "VdMotFBH",
                      "renamed": [{"kind": "valve", "n": 3, "name": "Bad_WC", "topic": "Bad/WC"}],
                      "syslogDebug": True, "voltsBlob448": False}


def build_hostname(station: str, cap: int = 21) -> str:
    """vdm::buildHostname: [A-Za-z0-9_-] kept, runs of other bytes -> one '-', no '-' at the ends."""
    out = []
    for b in station.encode("utf-8"):
        c = chr(b)
        if c.isascii() and (c.isalnum() or c in "-_"):
            out.append(c)
        elif not out or out[-1] != "-":
            out.append("-")
    name = "".join(out).strip("-")[:cap - 1].strip("-")
    return name or "VdMot"


def round_target(v):
    """vdm::roundTargetPercent: a JSON number 0..100 rounded half up, else None."""
    if isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v) or not 0 <= v <= 100:
        return None
    return int(math.floor(v + 0.5))


def effective_dns(n):
    return n["gateway"] if not n["dhcp"] and n["dns"] == "0.0.0.0" else n["dns"]


def net_trial_required(b, a):
    """vdm::netTrialRequired (config.h)."""
    if b["iface"] != a["iface"] or b["dhcp"] != a["dhcp"]:
        return True
    if not a["dhcp"] and any(b[k] != a[k] for k in ("ip", "mask", "gateway")) or \
            (not a["dhcp"] and effective_dns(b) != effective_dns(a)):
        return True
    if not (b["iface"] == 1 and a["iface"] == 1) and (b["ssid"] != a["ssid"] or b["wifiPassword"] != a["wifiPassword"]):
        return True
    return False


def restart_reasons(b, a):
    """vdm::configRestartReasons: 1 network (trial), 2 host name."""
    r = 1 if net_trial_required(b["net"], a["net"]) else 0
    if build_hostname(b["station"]) != build_hostname(a["station"]):
        r |= 2
    return r


def file_kind(path: str) -> str:
    """vdm::classifyFsPath."""
    if re.fullmatch(r"/stm/[^/]+\.bin", path):
        return "stm_image"
    if re.fullmatch(r"/stm/[^/]+\.part", path):
        return "upload_part"
    if path.startswith("/log/"):
        return "log"
    if path.startswith("/sys/"):
        return "internal"
    if path == "/HADiscovery.cfg":
        return "legacy_ha_list"
    if re.fullmatch(r"/[^/]+\.[bB][iI][nN]", path) and len(path) > 5:
        return "legacy_image"
    return "other"


PROTECTED = {"stm_image": "delete it under STM firmware", "log": "the event log", "internal": "used by the firmware",
             "legacy_ha_list": "kept for a rollback to the legacy firmware"}


def fs_path_valid(p) -> bool:
    return (isinstance(p, str) and p.startswith("/") and 2 <= len(p.encode()) <= 95 and "//" not in p
            and not p.endswith("/") and all(seg not in (".", "..") for seg in p.split("/")[1:])
            and all(ord(c) >= 0x20 and ord(c) != 0x7F for c in p))
LAST_GOOD = "last_good"
MAX_IMAGES = 3  # uploaded images besides last_good (storage::kMaxUploadedImages)


def image_name(raw: str):
    """storage::normalizeImageName: strip ".bin", 1..31 of [A-Za-z0-9._-], no leading '.'."""
    base = raw[:-4] if raw.endswith(".bin") else raw
    return base if re.fullmatch(r"[A-Za-z0-9_-][A-Za-z0-9._-]{0,30}", base) else None


def one_wire_id(rng: random.Random, family: int) -> str:
    b = [family] + [rng.randrange(256) for _ in range(6)]
    crc = 0
    for byte in b:
        for _ in range(8):
            mix = (crc ^ byte) & 1
            crc >>= 1
            if mix:
                crc ^= 0x8C
            byte >>= 1
    return "-".join(f"{x:02x}" for x in b + [crc])


def safe_name(s: str, lo: int, hi: int) -> bool:
    """vdm::isSafeName: printable UTF-8 (spaces allowed), no + # / " \\, lo..hi bytes."""
    n = len(s.encode("utf-8"))
    return lo <= n <= hi and all(ord(c) >= 0x20 and not 0x7F <= ord(c) <= 0x9F for c in s) and not SAFE_BAD.search(s)


def printable(s: str) -> bool:
    return all(ord(c) >= 0x20 and not 0x7F <= ord(c) <= 0x9F for c in s)


def ipv4(s) -> bool:
    if not isinstance(s, str):
        return False
    parts = s.split(".")
    return len(parts) == 4 and all(p.isdigit() and 1 <= len(p) <= 3 and int(p) <= 255 for p in parts)


class Device:
    def __init__(self, station: str, proto: int, auth: tuple[str, str] | None, scenarios=(), import_report=False,
                 station_name=None):
        self.lock = threading.RLock()
        self.rng = random.Random(42 if station == "east" else 7)
        self.station = station
        self.proto = proto
        self.boot_wall = time.time() - 3 * 86400 - 4 * 3600 - 17 * 60
        self.boots = 14
        self.down_until = 0.0
        self.restart_at = 0.0
        self.events: list[dict] = []
        self.next_seq = 1
        self.dropped = 0
        self.scenarios = set(scenarios)
        self.esp_version = ESP_VERSION
        self.esp_build = int(time.time()) - 86400 * 2
        self.stm_version = (STM_VERSION_OLD if "tooold" in self.scenarios else
                            STM_VERSION_V3 if proto >= 3 else STM_VERSION_V2 if proto >= 2 else STM_VERSION_V1)
        self.import_report = copy.deepcopy(DEMO_IMPORT_REPORT) if import_report else None
        self.trial = None           # {"until": wall time, "previous": net config} while a network trial runs
        self.pending_trial = None   # previous net config of a save that restarts into a trial
        self.limiter = {}           # client address -> {"fails", "win", "until", "level"}
        self.requests = 0           # /api/* requests (scenario busy)
        self.actions = 0            # valve actions (scenario queue)
        self.safe_mode = "safemode" in self.scenarios
        self.config_source = "stored"
        self.files = {"/log/events.log": 18234, "/log/events.1.log": 65530, "/sys/cfg.bak": 1391,
                      "/sys/cfgx.bak": 212, "/HADiscovery.cfg": 4410, "/VdMot_1.4.9.bin": 61440}
        self.stm_boot_wall = self.boot_wall - 86400 * 9
        self.stm_resets = 3
        self.link = "up"
        self.images: dict[str, dict] = {}
        self.flash: dict = {"phase": "idle"}
        self.motor = {"lowC": 17, "highC": 17, "startOnPower": 30, "noOfMinCount": 100, "maxCalReps": 2}
        self.learn = 0
        self.cfg_rev = 1
        self.breakaway = {"enable": False, "stepPct": 25, "maxmA": 45} if proto >= 2 else None
        self.config = self._default_config(auth)
        self.valves = []
        self.temps = []
        self.volts = []
        if station_name:
            self.config["station"] = station_name
        self._build_station()
        self._history()

    # ------------------------------------------------------------ setup

    def _default_config(self, auth):
        c = {
            "schema": 2, "station": f"VdMot-{self.station}",
            "net": {"iface": 1, "dhcp": True, "ip": "0.0.0.0", "mask": "0.0.0.0", "gateway": "0.0.0.0",
                    "dns": "0.0.0.0", "ssid": "", "wifiPassword": "", "reconnectTimeoutMin": 5},
            "time": {"ntpServer": "pool.ntp.org", "tzName": "Europe/Warsaw", "tzPosix": "CET-1CEST,M3.5.0,M10.5.0/3"},
            "syslog": {"level": 0, "server": "0.0.0.0", "port": 514},
            "web": {"user": auth[0] if auth else "", "password": auth[1] if auth else "", "protectRead": False,
                    "allowedHosts": "localhost"},
            "mqtt": {"mode": 2, "host": "homeassistant.local", "port": 1883, "user": "vdmot", "password": "mqtt-pass",
                     "keepAliveS": 60, "publishIntervalS": 10, "minDelayS": 5, "separate": True, "allTemps": True,
                     "pathAsRoot": False, "upTime": True, "onChange": True, "retained": True, "plainText": True,
                     "diag": True, "germanDecimal": False, "newDiag": True, "events": True,
                     "haDiscoveryOnConnect": True, "rootTopic": "", "clientId": "",
                     "discoveryPrefix": "homeassistant"},
            "valves": [{"name": "", "active": False, "failsafePct": 50, "topic": ""} for _ in range(12)],
            "temps": [{"name": "", "active": False, "offset": 0.0, "id": "", "topic": ""} for _ in range(34)],
            "volts": [{"name": "", "active": False, "offset": 0.0, "factor": 1.0, "unit": "", "id": "", "topic": ""}
                      for _ in range(8)],
            "calib": {"dayMask": 9, "hour": 3, "minute": 30},
            "failsafe": {"timeoutMin": 60},
            "persistLog": True,
        }
        return c

    def _build_station(self):
        rng = self.rng
        for i in range(12):
            self.valves.append({"active": False, "known": True, "state": 6, "pos": 0, "target": None,
                                "src": "none", "sync": "unknown", "meanCur": 20, "moves": 0, "oc": 0, "cc": 0,
                                "cr": 0, "earlyStops": 0, "cmdRejected": 0, "lastMove": None, "moveSeq": 0,
                                "calUntil": 0.0, "calState": 0, "profile": None, "slots": [0, 0],
                                "moveFrom": 0.0, "moveStart": 0.0, "blocked": None, "baseEarly": 0, "baseRej": 0})
        for i, (name, blocked) in enumerate(STATIONS[self.station]):
            v = self.valves[i]
            self.config["valves"][i].update(name=name, active=True)
            oc = rng.randrange(380, 520)
            v.update(active=True, state=1, meanCur=rng.choice([16, 16, 17, 17, 18]), moves=rng.randrange(900, 4200),
                     oc=oc, cc=oc + rng.randrange(20, 60), cr=0, src="mqtt", sync="synced")
            v["pos"] = v["target"] = rng.choice([0, 15, 30, 45, 60, 100])
            slot = i + 1
            tid = one_wire_id(rng, 0x28)
            self.config["temps"][slot - 1].update(name=name[:10], active=True, offset=rng.choice([0.0, -0.3, 0.5]),
                                                  id=tid)
            self.temps.append({"slot": slot, "id": tid, "base": rng.uniform(20.5, 23.5)})
            v["slots"] = [slot, 0] if i != 2 else [0, slot]  # valve 3: only sensor 2
            if blocked:
                oc_b, cc_b, cr_b = blocked
                v.update(state=9, oc=oc_b, cc=cc_b, cr=cr_b, blocked=blocked, earlyStops=3 if self.proto >= 2 else 0,
                         cmdRejected=2 if self.proto >= 2 else 0, pos=30, target=0, sync="synced")
            self._record_move(v, "close" if blocked else "open", abs(v["oc"]) if not blocked else 400,
                              blocked is not None)
        # flow/return sensors that are configured but not mapped, and one unconfigured bus sensor
        n = len(STATIONS[self.station])
        for k, label in enumerate(["Supply", "Return"]):
            slot = n + 1 + k
            tid = one_wire_id(rng, 0x28)
            self.config["temps"][slot - 1].update(name=label, active=True, offset=0.0, id=tid)
            self.temps.append({"slot": slot, "id": tid, "base": 34.0 if label == "Supply" else 29.5})
        self.temps.append({"slot": None, "id": one_wire_id(rng, 0x28), "base": 24.0})
        vid = one_wire_id(rng, 0x26)
        self.config["volts"][0].update(name="Supply", active=True, offset=0.0, factor=1.0, unit="V", id=vid)
        self.volts.append({"slot": 1, "id": vid, "base": 1208})

    def _history(self):
        t0 = self.boot_wall
        self.event(100, info=1, a1=1, a2=self.boots, text=ESP_VERSION, at=t0)
        self.event(200, a1=1, text="192.168.1.51" if self.station == "east" else "192.168.1.52", at=t0 + 4)
        self.event(111, at=t0 + 6)
        self.event(300, at=t0 + 7)
        self.event(306, a1=self.proto, a2=0x431, text=self.stm_version, at=t0 + 7)
        self.event(202, at=t0 + 9)
        self.event(205, a1=58, a2=0, at=t0 + 10)
        for i, v in enumerate(self.valves):
            if v["blocked"]:
                at = t0 + 3600 * 20
                self.event(406, valve=i, a1=1, at=at)
                for r in range(1, 3):
                    self.event(408, valve=i, a1=r, at=at + 90 * r)
                self.event(409, valve=i, a1=3, at=at + 300)
                self.event(402, valve=i, a1=3, at=at + 300)
                if self.proto >= 2:
                    self.event(410, valve=i, a1=3, a2=3, at=at + 300)
                    self.event(411, valve=i, a1=2, at=at + 7200)
            elif v["active"]:
                self.event(407, valve=i, a1=v["oc"], a2=v["cc"], at=t0 + 3600 * 20 + 60 * i)
        self.events.sort(key=lambda e: e["t"])
        for n, e in enumerate(self.events, start=1):
            e["seq"] = n

    # ------------------------------------------------------------ helpers

    def now_up(self) -> int:
        return max(0, int(time.time() - self.boot_wall))

    def name(self, i: int) -> str:
        return self.config["valves"][i]["name"] or str(i + 1)

    def event(self, code, valve=None, a1=0, a2=0, text="", sev=None, at=None, info=None):
        sev = sev or self._default_sev(code)
        at = at or time.time()
        msg = self._message(code, valve, a1, a2, text)
        with self.lock:
            e = {"seq": self.next_seq, "t": int(at), "up": max(0, int(at - self.boot_wall)), "sev": sev, "code": code,
                 "name": EVENT_NAMES.get(code, str(code)), "valve": None if valve is None else valve + 1,
                 "a1": a1, "a2": a2, "text": text, "msg": msg}
            self.next_seq += 1
            self.events.append(e)
            if len(self.events) > 512:
                self.events.pop(0)
                self.dropped += 1

    @staticmethod
    def _default_sev(code):
        if code in (402, 403, 409, 302, 303, 307):
            return "error"
        if code in (313,):
            return "critical"
        if code in (408, 410, 411, 412, 413, 201, 203, 204, 206, 305):
            return "warning"
        if code == 401:
            return "debug"
        return "info"

    def _message(self, code, valve, a1, a2, text):
        v = f"valve {valve + 1}: " if valve is not None else ""
        stops = ["none", "target", "endstop", "early_endstop", "timeout", "undercurrent", "safety_overcurrent", "aborted"]
        m = {
            100: f"boot (reset reason {a1}, boot {a2}) {text}", 102: f"configuration saved (rev {a1}, {text})",
            106: f"ESP firmware updated ({a1} bytes)", 109: f"restart requested (reason {a1})",
            111: "time synchronised", 200: f"network up ({'ethernet' if a1 == 1 else 'wifi'}, {text})",
            202: "MQTT connected", 205: f"HA discovery sent ({a1} configs, {a2} deletes)", 300: "STM link up",
            304: "STM reset by user", 306: f"STM firmware {text} (protocol {a1})",
            311: f"STM flashing started ({a1} bytes, {text})", 312: f"STM flashed in {a1} ms, now {text}",
            400: f"{v}target {a1} %", 401: f"{v}state {a1} -> {a2}", 402: f"{v}blocked after {a1} calibration retries",
            406: f"{v}calibration started" + (" (scheduled)" if a1 == 1 else ""),
            407: f"{v}calibration ok (oc {a1}, cc {a2})", 408: f"{v}calibration retry {a1}",
            409: f"{v}calibration failed after {a1} retries",
            410: f"{v}early stop (total {a1}, {stops[a2] if 0 <= a2 < len(stops) else a2})",
            411: f"{v}command rejected (total {a1})",
            414: f"{v}service move done ({a1} counts, {stops[a2] if 0 <= a2 < len(stops) else a2})",
            600: f"scheduled calibration ({a1})",
        }.get(code, text)
        return m

    def _record_move(self, v, direction, req, early):
        cnt = int(req * (0.18 if early else self.rng.uniform(0.96, 1.0)))
        peak = round(v["meanCur"] * (2.05 if early else self.rng.uniform(1.75, 1.95)), 1)
        stop = "early_endstop" if early else ("endstop" if req >= v["oc"] * 0.9 else "target")
        v["lastMove"] = {"dir": direction, "req": int(req), "cnt": cnt, "stop": stop, "peak": peak,
                         "ms": int(cnt * 38 + self.rng.randrange(200, 900))}
        v["moveSeq"] += 1
        if self.proto < 2:
            return
        n = 32
        samples = []
        for k in range(n):
            c = int(cnt * (k + 1) / n)
            base = v["meanCur"] * (1 + 0.08 * math.sin(k * 0.9)) + self.rng.uniform(-0.6, 0.6)
            if k < 2:
                base *= 1.5 - 0.2 * k  # inrush
            if k >= n - 3 and stop != "target":
                base = v["meanCur"] + (peak - v["meanCur"]) * (k - (n - 4)) / 3
            samples.append([c, max(0, int(round(base * 10)))])
        v["profile"] = samples

    # ------------------------------------------------------------ simulation

    def tick(self):
        now = time.time()
        with self.lock:
            if self.restart_at and now >= self.restart_at:
                self.restart_at = 0.0
                self.down_until = now + 6
                self.boot_wall = now + 6
                self.boots += 1
            if self.trial and now >= self.trial["until"]:
                self.config["net"] = self.trial["previous"]
                self.trial = None
                self.event(209, a1=1, text="previous address")
                self.request_restart()
            if self.down_until and now >= self.down_until:
                self.down_until = 0.0
                if self.pending_trial is not None:
                    self.trial = {"until": now + TRIAL_S, "previous": self.pending_trial}
                    self.pending_trial = None
                self.event(100, a1=3, a2=self.boots, text=self.esp_version)
                self.event(200, a1=1, text="192.168.1.51")
                self.event(300)
            self._tick_flash(now)
            if self.link == "suspended":
                return
            for i, v in enumerate(self.valves):
                if not v["active"]:
                    continue
                if v["calUntil"]:
                    self._tick_cal(i, v, now)
                    continue
                if v["state"] in (2, 3, 7) and v["target"] is not None:
                    goal = 100 if v["state"] == 7 else v["target"]
                    step = (now - v["moveStart"]) * 4.0
                    if v["pos"] < goal:
                        v["pos"] = min(goal, int(v["moveFrom"] + step))
                    else:
                        v["pos"] = max(goal, int(v["moveFrom"] - step))
                    if v["pos"] == goal:
                        req = abs(goal - v["moveFrom"]) * v["oc"] / 100
                        self._record_move(v, "open" if goal > v["moveFrom"] else "close", max(req, 1), False)
                        v["moves"] += 1
                        v["state"] = 7 if v["state"] == 7 else 1
                        v["sync"] = "synced"

    def _tick_cal(self, i, v, now):
        left = v["calUntil"] - now
        v["calState"] = 2
        v["state"] = 3 if left > 10 else 2
        v["pos"] = max(0, min(100, int(100 - left * 10))) if left <= 10 else max(0, v["pos"] - 5)
        if left > 0:
            return
        v["calUntil"] = 0.0
        v["calState"] = 0
        if v["blocked"]:
            oc_b, cc_b, cr_b = v["blocked"]
            v.update(state=9, oc=oc_b, cc=cc_b, cr=cr_b)
            self.event(409, valve=i, a1=cr_b)
            self.event(402, valve=i, a1=cr_b)
            if self.proto >= 2:
                v["earlyStops"] += 1
                self._record_move(v, "close", 400, True)
                self.event(410, valve=i, a1=v["earlyStops"], a2=3)
        else:
            v["oc"] = max(300, v["oc"] + self.rng.randrange(-8, 9))
            v["cc"] = v["oc"] + self.rng.randrange(20, 60)
            v["state"] = 1
            v["moves"] += 2
            self._record_move(v, "open", v["oc"], False)
            self.event(407, valve=i, a1=v["oc"], a2=v["cc"])
            if v["target"] is not None:
                v["moveFrom"], v["moveStart"] = v["pos"], now
                v["state"] = 2 if v["target"] > v["pos"] else 3 if v["target"] < v["pos"] else 1

    def _tick_flash(self, now):
        f = self.flash
        if f.get("phase") in (None, "idle", "done", "failed"):
            return
        el = now - f["t0"]
        size = f["image"]["size"]
        plan = [("validating", 0.5, 0, 2), ("resetting", 0.3, 2, 3), ("handshake", 0.6, 3, 4), ("sync", 0.4, 4, 4),
                ("getid", 0.2, 4, 5), ("erasing", 3.0, 5, 15), ("writing", 8.0, 15, 75), ("verifying", 3.0, 75, 95),
                ("starting", 0.5, 95, 96), ("waiting_app", 4.0, 96, 99)]
        t = 0.0
        for phase, dur, p0, p1 in plan:
            if el < t + dur:
                frac = (el - t) / dur
                f["phase"] = phase
                f["percent"] = int(p0 + (p1 - p0) * frac)
                f["bytesDone"] = int(size * frac) if phase in ("writing", "verifying") else (size if p0 >= 75 else 0)
                if phase not in ("validating", "resetting", "handshake", "sync"):
                    f["chipId"], f["chipName"], f["bootloaderVersion"] = "0x431", "STM32F411xx", "3.1"
                return
            t += dur
        f.update(phase="done", percent=100, bytesDone=size, finishedMs=int(el * 1000),
                 appVersion=f["image"]["version"] or STM_VERSION_V2, manualReset=f.get("blank", False))
        self.stm_version = f["appVersion"]
        src = self.images.get(f["image"]["name"])
        if src is not None and src["name"] != LAST_GOOD:
            self.add_image(LAST_GOOD, src["data"], scanned_at=now)
        self.stm_boot_wall = now
        self.link = "up"
        self.event(312, a1=int(el * 1000), text=self.stm_version)
        for i, v in enumerate(self.valves):
            if v["active"]:
                v["calUntil"] = now + 20 + 25 * i
                v["calState"] = 1

    # ------------------------------------------------------------ images

    def add_image(self, name, data, scanned_at=None):
        m = re.search(rb"(\d+\.\d+\.\d+[A-Za-z0-9_.+-]*revamped[A-Za-z0-9_.+-]*)", data)
        hw = re.search(rb"VDM-HW:(C\d{1,2})", data)
        img = {"name": name, "size": len(data), "crc32": "0x%08x" % (zlib.crc32(data) & 0xFFFFFFFF),
               "hw": hw.group(1).decode() if hw else None,
               "version": m.group(1).decode() if m else "", "data": data,
               # the device validates a fresh upload in the background
               "scanned_at": scanned_at if scanned_at is not None else time.time() + 1.5}
        with self.lock:
            self.images[name] = img
        return img

    @staticmethod
    def image_check(data: bytes) -> str:
        if not data:
            return "image_empty"
        if len(data) > STM_IMAGE_MAX:
            return "image_too_large"
        if len(data) < 8:
            return "image_bad_vectors"
        sp, rv = int.from_bytes(data[0:4], "little"), int.from_bytes(data[4:8], "little")
        if not (0x20000000 <= sp <= 0x20020000 and sp % 4 == 0) or not (rv & 1 and 0x08000000 <= rv < 0x08000000 + len(data)):
            return "image_bad_vectors"
        if b"DEADBEEF" not in data or b"BEEFIT" not in data:
            return "image_no_handshake"
        return "none"

    def image_info(self, img):
        scanned = time.time() >= img["scanned_at"]
        return {"name": img["name"], "size": img["size"], "crc32": img["crc32"] if scanned else None,
                "version": (img["version"] or None) if scanned else None,
                "check": self.image_check(img["data"]) if scanned else None, "hw": img["hw"]}

    # ------------------------------------------------------------ documents

    def status(self):
        now = time.time()
        lt = dt.datetime.now()
        net_ip = "192.168.1.51" if self.station == "east" else "192.168.1.52"
        mqtt_on = self.config["mqtt"]["mode"] > 0
        c = self.config
        trial = {"remainS": max(0, int(self.trial["until"] - now))} if self.trial else None
        lease = None
        if self.proto >= 3 or c["failsafe"]["timeoutMin"] and self.proto >= 1:
            expired = "failsafe" in self.scenarios
            mask = sum(1 << i for i, v in enumerate(self.valves) if v["active"]) if expired else 0
            lease = {"mode": "stm" if self.proto >= 3 else "esp",
                     "state": "off" if not c["failsafe"]["timeoutMin"] else "expired" if expired else "running",
                     "remainS": 0 if expired else c["failsafe"]["timeoutMin"] * 60 - 37,
                     "timeoutMin": c["failsafe"]["timeoutMin"], "failsafeMask": mask,
                     "regulator": "ha_offline" if expired else "alive", "regulatorLostS": 4000 if expired else 0,
                     "configSynced": True, "configFailed": False, "configTrusted": True}
        status = None
        if self.proto >= 2:
            status = {"uptime": int(now - self.stm_boot_wall), "resets": self.stm_resets, "bootReason": 1,
                      "rxOverflow": 0, "parseErr": 2, "eepState": 1}
            if self.proto >= 3:
                status.update(lease=lease["state"] if lease else "off", leaseRemainS=lease["remainS"] if lease else 0,
                              leaseClient=True, leaseTimeoutMin=c["failsafe"]["timeoutMin"],
                              failsafeMask=lease["failsafeMask"] if lease else 0, safeMode=self.safe_mode,
                              wdgResets=3 if self.safe_mode else 0, uartOre=0, uartFe=0, uartNe=0, rxDropped=0,
                              cfgFlags=[], cfgEvents=0, eepWrites=412, tempAgeS=4, owScanAgeS=3600,
                              protectSuspended=False)
        nxt = (lt + dt.timedelta(days=1)).replace(hour=c["calib"]["hour"], minute=c["calib"]["minute"], second=0)
        return {
            "station": c["station"],
            "esp": {"version": self.esp_version, "build": self.esp_build, "uptime": self.now_up(),
                    "resetReason": "sw" if self.boots > 14 else "poweron", "boots": self.boots,
                    "heap": {"free": 142336 - self.rng.randrange(0, 2048), "min": 118420, "largest": 90100},
                    "flash": {"used": 1012432, "size": ESP_APP_MAX}},
            "time": {"valid": True, "epoch": int(now), "local": lt.strftime("%Y-%m-%dT%H:%M:%S"),
                     "lastSync": int(now) - 1800},
            "net": {"state": "ethernet", "ip": net_ip, "mask": "255.255.255.0", "gw": "192.168.1.1",
                    "dns": "192.168.1.1", "mac": "A8:03:2A:6C:1E:%02X" % (0x51 if self.station == "east" else 0x52),
                    "rssi": None, "hostname": build_hostname(c["station"]), "trial": trial},
            "mqtt": {"state": "connected" if mqtt_on else "disabled", "rc": 0, "reconnects": 2, "publishFailures": 0,
                     "clientId": c["mqtt"]["clientId"] or build_hostname(c["station"]) + "-6c1e51",
                     "haStatus": "offline" if "haoffline" in self.scenarios else "online" if mqtt_on else "unknown"},
            "stm": {"link": self.link, "proto": self.proto, "version": self.stm_version, "build": None,
                    "hwId": "0x431", "chip": "STM32F411xx", "compatible": "tooold" not in self.scenarios,
                    "minVersion": "1.4.0",
                    "stats": {"sent": 482113, "answered": 482051, "timeouts": 62, "failedRequests": 3,
                              "strayLines": 0, "parseErrors": 1, "queueFull": 0, "evictions": 4,
                              "policyResets": 0, "userResets": self.stm_resets - 3,
                              "consecutiveTimeouts": 0, "lastReplyMs": int(self.now_up() * 1000)},
                    "status": status,
                    "espRx": {"overflow": 0, "malformed": 1},
                    "support": "too_old" if "tooold" in self.scenarios else "ok", "lease": lease,
                    "learnTime": 7200 if self.proto >= 3 else None},
            "calibration": {"active": any(v["calUntil"] for v in self.valves), "lastScheduled": int(now) - 86400 * 2,
                            "nextSlot": int(nxt.strftime("%Y%m%d")), "next": nxt.strftime("%Y-%m-%dT%H:%M:%S")},
            "auth": bool(c["web"]["user"] and c["web"]["password"]),
            "lastEventSeq": self.next_seq - 1,
            "config": {"source": self.config_source, "repairs": 0, "newerSchema": False},
            "importReport": self.import_report is not None,
        }

    def temp_value(self, t):
        base = t["base"] + 0.4 * math.sin(time.time() / 600 + t["base"])
        off = self.config["temps"][t["slot"] - 1]["offset"] if t["slot"] else 0.0
        return round(base + off, 1), int(round(base * 10))

    def valves_doc(self):
        out = []
        for i, v in enumerate(self.valves):
            cfg = self.config["valves"][i]
            sensors = []
            for k, slot in enumerate(v["slots"]):
                if slot:
                    t = next((x for x in self.temps if x["slot"] == slot), None)
                    temp = self.temp_value(t)[0] if t else None
                    if "health" in self.scenarios and i == 4:
                        temp = None
                    sensors.append({"sensor": k + 1, "slot": slot, "name": self.config["temps"][slot - 1]["name"],
                                    "temp": temp})
            state = v["state"]
            status_key = ["nodata", "idle", "opening", "closing", "failed", "unknown", "novalve", "fullopen",
                          "connected", "blocked"][state]
            health = []
            if cfg["active"]:
                if state == 9:
                    health.append("blocked")
                if v["cr"] > 0:
                    health.append("calibRetries")
                if self.proto >= 2 and v["earlyStops"] > v["baseEarly"]:
                    health.append("earlyStop")
                if self.proto >= 2 and v["cmdRejected"] > v["baseRej"]:
                    health.append("cmdRejected")
            sync = v["sync"] if cfg["active"] else "unknown"
            age = self.rng.randrange(0, 4)
            if "health" in self.scenarios and cfg["active"]:
                if i == 2:
                    age = 400
                    health.append("stale")
                elif i == 3:
                    sync = "failed"
                    health.append("targetUnconfirmed")
                elif i == 4:
                    health.append("tempFailed")
            lease_on = "failsafe" in self.scenarios and cfg["active"]
            if lease_on:
                health.append("failsafe")
            ext = None
            if self.proto >= 2:
                ext = {"calState": v["calState"], "calEarlyStop": v["blocked"] is not None,
                       "calLastFailed": v["blocked"] is not None, "earlyStops": v["earlyStops"],
                       "cmdRejected": v["cmdRejected"], "lastMove": v["lastMove"], "moveSeq": v["moveSeq"],
                       "v3": None}
                if self.proto >= 3:
                    ext["v3"] = {"flags": (["fs_lease"] if lease_on else []) + (["needs_ref"] if v["blocked"] else []),
                                 "fault": "none", "drive": v["target"] or 0, "retryS": 0, "retries": 0}
            pct = cfg["failsafePct"]
            failsafe = {"state": "blocked" if v["state"] == 9 and self.proto >= 3 else "lease" if lease_on else "off",
                        "pct": None if pct == 255 else pct}
            cal_end = dt.datetime.fromtimestamp(self.boot_wall + 3600 * 20 + 60 * i).strftime("%Y-%m-%dT%H:%M:%S")
            out.append({"idx": i + 1, "name": cfg["name"], "active": cfg["active"], "known": True, "state": state,
                        "stateKey": status_key, "calibrating": bool(v["calUntil"]) and v["calState"] == 2,
                        "pos": v["pos"], "target": v["target"] if cfg["active"] else None,
                        "targetSource": v["src"], "sync": sync,
                        "stmTarget": v["target"], "meanCur": v["meanCur"], "moves": v["moves"], "oc": v["oc"],
                        "cc": v["cc"], "dc": v["cc"] - v["oc"], "cr": v["cr"], "health": health,
                        "age": age, "sensors": sensors, "ext": ext, "failsafe": failsafe,
                        "calibrationEnd": cal_end if cfg["active"] else None})
        return {"valves": out}

    def sensors_doc(self):
        temps = []
        for t in self.temps:
            slot = t["slot"]
            c = self.config["temps"][slot - 1] if slot else {"name": "", "active": False}
            val, raw = self.temp_value(t)
            valve = next((i + 1 for i, v in enumerate(self.valves) if slot and slot in v["slots"]), None)
            temps.append({"slot": slot, "name": c["name"], "id": t["id"], "active": c["active"], "onBus": True,
                          "temp": val, "raw": raw, "age": self.rng.randrange(1, 9), "valve": valve})
        volts = []
        for x in self.volts:
            c = self.config["volts"][x["slot"] - 1]
            raw = x["base"] + self.rng.randrange(-3, 4)
            volts.append({"slot": x["slot"], "name": c["name"], "id": x["id"], "active": c["active"], "onBus": True,
                          "value": round((raw / 100 + c["offset"]) * c["factor"], 3), "unit": c["unit"], "raw": raw,
                          "age": self.rng.randrange(1, 9)})
        return {"temps": temps, "volts": volts}

    def config_doc(self, secrets=False):
        c = copy.deepcopy(self.config)
        for key in SECRETS:
            grp, field = key.split(".")
            c[grp][field + "Set"] = bool(c[grp][field])
            if not secrets:
                c[grp].pop(field)
        return c

    def legacy_valves(self):
        out = []
        for i, v in enumerate(self.valves):
            if v["state"] in (0, 6):
                continue
            e = {"idx": i + 1, "name": self.config["valves"][i]["name"], "state": v["state"], "pos": v["pos"],
                 "meanCur": v["meanCur"], "targetPos": v["target"] if v["target"] is not None else v["pos"], "link": 0,
                 "moves": v["moves"], "oc": v["oc"], "cc": v["cc"], "dc": v["cc"] - v["oc"], "cr": v["cr"]}
            for k, slot in enumerate(v["slots"]):
                if slot:
                    t = next((x for x in self.temps if x["slot"] == slot), None)
                    e[f"tIdxName{k + 1}"] = self.config["temps"][slot - 1]["name"]
                    e[f"temp{k + 1}"] = self.temp_value(t)[0] if t else "failed"
            e["controlActive"] = 0
            if v["calUntil"]:
                e["calibration"] = 1
            out.append(e)
        return {"valves": out}

    def legacy_temps(self):
        used = {s for v in self.valves for s in v["slots"] if s}
        out = []
        for t in self.temps:
            slot = t["slot"]
            if not slot:
                continue
            c = self.config["temps"][slot - 1]
            if c["active"] and c["id"] and (slot not in used or self.config["mqtt"]["allTemps"]):
                out.append({"id": c["id"], "name": c["name"], "temp": self.temp_value(t)[0]})
        return out

    def legacy_volts(self):
        return [{"id": x["id"], "name": c["name"], "unit": c["unit"],
                 "value": round((x["base"] / 100 + c["offset"]) * c["factor"], 3)}
                for x in self.volts for c in [self.config["volts"][x["slot"] - 1]] if c["active"] and c["id"]]

    def files_doc(self):
        files = dict(self.files)
        for img in self.images.values():
            files["/stm/" + img["name"] + ".bin"] = img["size"]
        if self.import_report is not None:
            files["/sys/import.json"] = len(json.dumps(self.import_report))
        used = sum(files.values()) + 8192
        return {"total": 0x170000, "used": used, "truncated": False,
                "files": [{"path": k, "size": v, "kind": file_kind(k), "deletable": file_kind(k) not in PROTECTED}
                          for k, v in sorted(files.items())]}

    def health_doc(self):
        return {"ok": True, "version": self.esp_version, "uptime": self.now_up(),
                "heap": {"free": 142336, "min": 118420, "largest": 90100, "minLargest": 65536},
                "tasks": [{"name": "stm", "stack": 6144, "minFree": 2100}, {"name": "app", "stack": 8192, "minFree": 3200},
                          {"name": "mqtt", "stack": 6144, "minFree": 2400}],
                "net": {"ip": True, "reachable": True, "proven": True, "pingArmed": True, "evidence": "ping",
                        "evidenceAgeS": 12, "ifaceRestarts": 0,
                        "trial": {"remainS": max(0, int(self.trial["until"] - time.time()))} if self.trial else None},
                "ota": None,
                "log": {"persist": True, "backlog": 3, "flushes": 12, "lastFlushAgeS": 40, "lost": 0, "failures": 0}}

    def host_ok(self, host: str, local_ip: str) -> bool:
        """vdm::hostAllowed."""
        if not host:
            return True
        if ":" in host:
            host, _, port = host.partition(":")
            if not re.fullmatch(r"\d{1,5}", port):
                return False
        if host.endswith("."):
            host = host[:-1]
        if not host:
            return False
        allowed = [x.strip() for x in self.config["web"]["allowedHosts"].split(",") if x.strip()]
        if re.fullmatch(r"[\d.]+", host):
            if not ipv4(host) or host == "0.0.0.0":
                return False
            own = "192.168.1.51" if self.station == "east" else "192.168.1.52"
            return host in (local_ip, own) or host in allowed
        name = build_hostname(self.config["station"]).lower()
        return host.lower() in (name, name + ".local") or host.lower() in (x.lower() for x in allowed)

    def events_doc(self, q):
        try:
            since = int(q.get("since", ["0"])[0])
            limit = max(1, min(50, int(q.get("limit", ["50"])[0])))
            valve = int(q["valve"][0]) if "valve" in q else None
        except ValueError:
            return None
        sev = q.get("minSeverity", ["debug"])[0]
        if sev not in SEVERITIES or (valve is not None and not 1 <= valve <= 12):
            return None
        floor = SEVERITIES.index(sev)
        with self.lock:
            sel = [e for e in self.events if e["seq"] > since and SEVERITIES.index(e["sev"]) >= floor
                   and (valve is None or e["valve"] == valve)][:limit]
            nxt = sel[-1]["seq"] if len(sel) == limit else max(since, self.next_seq - 1)
            first = self.events[0]["seq"] if self.events else 0
            return {"first": first, "last": self.next_seq - 1, "next": nxt,
                    "dropped": self.dropped, "events": sel}

    # ------------------------------------------------------------ config patch

    def apply_patch(self, doc, dry_run=False):
        """Returns (status, body). Mirrors handleConfigPatch: every failure is 400 invalid <path>."""
        if not isinstance(doc, dict):
            return 400, {"error": "invalid", "detail": "@0"}
        flat = {}

        def walk(prefix, obj):
            if isinstance(obj, dict):
                for k, val in obj.items():
                    walk(f"{prefix}.{k}" if prefix else k, val)
            elif isinstance(obj, list):
                for n, val in enumerate(obj):
                    walk(f"{prefix}.{n + 1}", val)
            else:
                flat[prefix] = obj

        walk("", doc)
        clear = flat.pop("clearSecrets", False) is True
        new = copy.deepcopy(self.config)
        for key, val in flat.items():
            if key == "schema" or key.endswith("PasswordSet") or key.endswith("passwordSet"):
                continue
            err = self._set(new, key, val, clear)
            if err:
                return 400, {"error": "invalid", "detail": key}
        bad = self._validate(new)
        if bad:
            return 400, {"error": "invalid", "detail": bad}
        reasons = restart_reasons(self.config, new)
        info = {"restartRequired": reasons != 0, "netTrial": net_trial_required(self.config["net"], new["net"])}
        if dry_run:
            return 200, info
        if info["netTrial"]:
            self.pending_trial = copy.deepcopy(self.config["net"])
        elif self.trial and new["net"] != self.config["net"]:
            self.trial = None
        self.config = new
        self.cfg_rev += 1
        self.event(102, a1=self.cfg_rev, text="web")
        if reasons:
            self.request_restart()
        doc = self.config_doc()
        doc.update(info)
        return 200, doc

    def _set(self, c, key, val, clear):
        parts = key.split(".")
        try:
            if parts[0] in ("valves", "temps", "volts"):
                idx = int(parts[1])
                count = {"valves": 12, "temps": 34, "volts": 8}[parts[0]]
                if not 1 <= idx <= count or len(parts) != 3 or parts[1] != str(idx):
                    return "unknown_key"
                obj, field = c[parts[0]][idx - 1], parts[2]
                generic = f"{parts[0]}.N.{field}"
            elif len(parts) == 1:
                obj, field, generic = c, parts[0], key
            elif len(parts) == 2 and isinstance(c.get(parts[0]), dict):
                obj, field, generic = c[parts[0]], parts[1], key
            else:
                return "unknown_key"
        except (ValueError, KeyError):
            return "unknown_key"
        if field not in obj:
            return "unknown_key"
        cur = obj[field]
        if key in SECRETS:
            if not isinstance(val, str):
                return "wrong_type"
            if len(val.encode()) > SECRETS[key] or not printable(val):
                return "out_of_range"
            if val or clear:
                obj[field] = val
            return None
        if isinstance(cur, bool):
            if isinstance(val, bool) or val in (0, 1):
                obj[field] = bool(val)
                return None
            return "wrong_type"
        if isinstance(cur, int):
            if isinstance(val, str) and val.lstrip("-").isdigit():
                val = int(val)
            if isinstance(val, bool) or not isinstance(val, int):
                return "wrong_type"
            lo, hi = INT_RANGES.get(key, (0, 65535))
            if generic == "valves.N.failsafePct":
                lo, hi = 0, 255
                if 100 < val < 255:
                    return "out_of_range"
            if key == "failsafe.timeoutMin" and 0 < val < 5:
                return "out_of_range"
            if key == "failsafe.timeoutMin":
                hi = 1440
            if not lo <= val <= hi:
                return "out_of_range"
            obj[field] = val
            return None
        if isinstance(cur, float):
            if isinstance(val, bool) or not isinstance(val, (int, float)) or not math.isfinite(val):
                return "wrong_type"
            lo, hi = (-10, 10) if generic == "temps.N.offset" else (-1000, 1000)
            if not lo <= val <= hi or (generic == "volts.N.factor" and val == 0):
                return "out_of_range"
            obj[field] = round(val, 1) if generic == "temps.N.offset" else float(val)
            return None
        if not isinstance(val, str):
            return "wrong_type"
        if key in IP_KEYS:
            if not ipv4(val):
                return "out_of_range"
        elif field == "id":
            if val and not ONEWIRE_RE.match(val):
                return "out_of_range"
            val = val.lower()
        elif field in ("name", "unit") or key in ("station", "mqtt.rootTopic"):
            lo, hi = (1, 20) if key == "station" else (0, 20) if key == "mqtt.rootTopic" else (0, 8 if field == "unit" else 10)
            if not safe_name(val, lo, hi):
                return "out_of_range"
        elif field == "topic":
            if len(val) > 10 or not SEG_RE.match(val):
                return "out_of_range"
        elif key == "mqtt.clientId":
            if len(val) > 23 or not SEG_RE.match(val):
                return "out_of_range"
        elif key == "mqtt.discoveryPrefix":
            if len(val) > 32 or not PATH_RE.match(val):
                return "out_of_range"
        elif key == "web.allowedHosts":
            hosts = [x.strip() for x in val.split(",")] if val else []
            if len(val) > 80 or len(hosts) > 4 or any(not re.fullmatch(r"[A-Za-z0-9](?:[A-Za-z0-9.-]*[A-Za-z0-9])?", x)
                                                      for x in hosts):
                return "out_of_range"
        else:
            lo, hi = STR_RANGES.get(key, (0, 64))
            if not lo <= len(val) <= hi or any(not 0x20 <= ord(ch) <= 0x7E for ch in val):
                return "out_of_range"
        obj[field] = val
        return None

    @staticmethod
    def _validate(c):
        n = c["net"]
        if not n["dhcp"]:
            for k in ("ip", "mask", "gateway"):
                if n[k] == "0.0.0.0":
                    return f"net.{k}"
        if n["wifiPassword"] and len(n["wifiPassword"].encode()) < 8:  # empty = open network
            return "net.wifiPassword"
        if n["iface"] == 2 and not n["ssid"]:
            return "net.ssid"
        if c["syslog"]["level"] > 0 and c["syslog"]["server"] == "0.0.0.0":
            return "syslog.server"
        w = c["web"]
        if bool(w["user"]) != bool(w["password"]):
            return "web.password" if w["user"] else "web.user"
        m = c["mqtt"]
        if m["mode"] and not m["host"]:
            return "mqtt.host"
        if m["minDelayS"] > m["publishIntervalS"]:
            return "mqtt.minDelayS"
        if m["mode"] == 2 and not m["separate"]:
            return "mqtt.mode"
        names = [v["name"].replace(" ", "_") for v in c["valves"]]
        for i, nm in enumerate(names):
            if nm and nm in names[:i]:
                return f"valves.{i + 1}.name"
        for grp in ("temps", "volts"):
            seen = set()
            for i, t in enumerate(c[grp]):
                if not t["id"]:
                    if t["active"]:
                        return f"{grp}.{i + 1}.active"
                    continue
                if t["id"] in seen:
                    return f"{grp}.{i + 1}.id"
                seen.add(t["id"])
        return None

    # ------------------------------------------------------------ actions

    def request_restart(self):
        self.event(109, a1=0)
        self.restart_at = time.time() + 1.0

    def queue_full(self):
        """Scenario queue: every 2nd valve action answers 503 queue_full."""
        if "queue" not in self.scenarios:
            return False
        self.actions += 1
        return self.actions % 2 == 0

    def set_target(self, i, t):
        v = self.valves[i]
        now = time.time()
        v["target"], v["src"] = t, "web"
        self.event(400, valve=i, a1=t, a2=2)
        if v["state"] == 9:
            if self.proto >= 2:
                v["cmdRejected"] += 1
                self.event(411, valve=i, a1=v["cmdRejected"])
            return
        if v["calUntil"]:
            v["sync"] = "pending"
            return
        v["sync"] = "await_verify"
        v["moveFrom"], v["moveStart"] = v["pos"], now
        v["state"] = 2 if t > v["pos"] else 3 if t < v["pos"] else 1
        if v["state"] == 1:
            v["sync"] = "synced"

    def calibrate(self, idxs, scheduled=False):
        now = time.time()
        for n, i in enumerate(idxs):
            v = self.valves[i]
            v["calUntil"] = now + 25 + 30 * n
            v["calState"] = 1
            self.event(406, valve=i, a1=1 if scheduled else 0)


# ------------------------------------------------------------------ HTTP


def bump_version(v: str) -> str:
    """"2.1.0-revamped-dev" -> "2.1.1-revamped-dev" (the mock's ESP OTA)."""
    m = re.match(r"(\d+)\.(\d+)\.(\d+)(.*)", v)
    return f"{m.group(1)}.{m.group(2)}.{int(m.group(3)) + 1}{m.group(4)}" if m else v + "+1"


def parse_multipart(body: bytes, ctype: str):
    m = re.search(r'boundary="?([^";]+)"?', ctype or "")
    if not m:
        return None
    sep = b"--" + m.group(1).encode()
    for part in body.split(sep)[1:]:
        if part.startswith(b"--"):
            break
        head, _, data = part.partition(b"\r\n\r\n")
        fn = re.search(rb'filename="([^"]*)"', head)
        if fn is None:
            continue
        if data.endswith(b"\r\n"):
            data = data[:-2]
        return fn.group(1).decode("utf-8", "replace"), data
    return None


class Handler(BaseHTTPRequestHandler):
    server_version = "VdMotMock/1"
    dev: Device = None  # set in main()

    def log_message(self, fmt, *args):  # quieter than the default
        if os.environ.get("MOCK_VERBOSE"):
            super().log_message(fmt, *args)

    # -------------------------------------------------------- plumbing

    def send_json(self, code, obj=None, extra=None):
        body = b"" if obj is None else json.dumps(obj, separators=(",", ":")).encode()
        self.send_response(code)
        if obj is not None:
            self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        for k, v in (extra or {}).items():
            self.send_header(k, v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    def error(self, code, err, detail="", extra=None):
        self.send_json(code, {"error": err, "detail": detail}, extra)

    def accepted(self):
        self.send_json(202, {"result": "queued"})

    def length(self):
        return int(self.headers.get("Content-Length") or 0)

    def drain(self):
        n = self.length()
        if n:
            self.rfile.read(n)

    def body(self, limit=MAX_BODY):
        n = self.length()
        if n > limit:
            self.rfile.read(n)
            return None, 413
        return self.rfile.read(n), 0

    def json_body(self):
        raw, err = self.body()
        if err:
            self.error(413, "too_large", "body")
            return None
        if not raw:
            return {}
        try:
            doc = json.loads(raw)
        except ValueError:
            self.error(400, "bad_request", "json")
            return None
        if not isinstance(doc, dict):
            self.error(400, "bad_request", "object expected")
            return None
        return doc

    def guard(self, method, scope, upload=False):
        """vdm::checkRequest: Host, Origin, X-VdMot (API writes), JSON Content-Type. True = refused."""
        d = self.dev
        local = self.connection.getsockname()[0]
        host = self.headers.get("Host", "")
        verdict = None
        if not d.host_ok(host, local):
            verdict = (1, 403, "host_not_allowed", f"{host[:48]}: use {local} or add the name to web.allowedHosts")
        else:
            origin = self.headers.get("Origin")
            if origin is not None:
                m = re.fullmatch(r"(?i)https?://([^/]+)", origin)
                if not m or not d.host_ok(m.group(1), local):
                    verdict = (2, 403, "origin_not_allowed", origin[:48])
        if verdict is None and scope == "api" and method in ("POST", "DELETE") and self.headers.get("X-VdMot") != "1":
            verdict = (3, 403, "header_required", "X-VdMot: 1")
        ctype = (self.headers.get("Content-Type") or "").split(";")[0].strip().lower()
        if verdict is None and method == "POST" and self.length() > 0 and not upload and ctype != "application/json":
            verdict = (4, 415, "unsupported_media_type", "application/json required")
        if verdict is None:
            return False
        self.drain()
        d.event(213, a1=verdict[0], text=self.client_address[0])
        self.error(verdict[1], verdict[2], verdict[3])
        return True

    def authorized(self, read_only):
        """Basic auth with the per-address limiter (vdm::AuthLimiter)."""
        c = self.dev.config["web"]
        if not (c["user"] and c["password"]) or (read_only and not c["protectRead"]):
            return True
        ip = self.client_address[0]
        now = time.time()
        lim = self.dev.limiter.setdefault(ip, {"fails": 0, "win": 0.0, "until": 0.0, "level": 0})
        if now < lim["until"]:
            left = max(1, math.ceil(lim["until"] - now))
            self.error(429, "locked", f"too many failed logins from this address, retry in {left} s",
                       {"Retry-After": str(left)})
            return False
        hdr = self.headers.get("Authorization", "")
        if hdr:
            user = pw = None
            if hdr.startswith("Basic "):
                try:
                    user, _, pw = base64.b64decode(hdr[6:]).decode().partition(":")
                except ValueError:
                    pass
            if user == c["user"] and pw == c["password"]:
                self.dev.limiter.pop(ip, None)
                return True
            if lim["fails"] == 0 or now - lim["win"] >= 60:
                lim["fails"], lim["win"] = 0, now
            lim["fails"] += 1
            self.dev.event(206, a1=lim["fails"], text=ip)
            if lim["fails"] >= 10:
                secs = (60, 300, 900)[min(lim["level"], 2)]
                lim.update(until=now + secs, fails=0, level=lim["level"] + 1)
                self.dev.event(214, a1=secs, a2=lim["level"], text=ip)
        self.send_json(401, {"error": "unauthorized", "detail": ""},
                       {"WWW-Authenticate": 'Basic realm="VdMot"'})
        return False

    def static(self, path):
        if path == "/":
            path = "/index.html"
        rel = os.path.normpath(unquote(path).lstrip("/"))
        full = os.path.join(WEB_DIR, rel)
        if rel.startswith("..") or not os.path.isfile(full):
            return self.error(404, "not_found", path)
        types = {".html": "text/html; charset=utf-8", ".js": "application/javascript; charset=utf-8",
                 ".css": "text/css; charset=utf-8"}
        with open(full, "rb") as f:
            data = f.read()
        self.send_response(200)
        self.send_header("Content-Type", types.get(os.path.splitext(full)[1], "application/octet-stream"))
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        self.route("GET")

    def do_POST(self):
        self.route("POST")

    def do_DELETE(self):
        self.route("DELETE")

    def do_PUT(self):
        self.route("PUT")

    # -------------------------------------------------------- routes

    def route(self, method):
        d = self.dev
        d.tick()
        u = urlsplit(self.path)
        path, q = u.path, parse_qs(u.query)
        if d.down_until and time.time() < d.down_until:
            self.close_connection = True  # rebooting: drop the request like a dead host
            return
        if not path.startswith("/api/"):
            return self.non_api(method, path)
        upload = method == "POST" and path in ("/api/stm/images", "/api/ota/esp")
        if self.guard(method, "api", upload):
            return
        if path == "/api/health":
            if method != "GET":
                return self.error(405, "method_not_allowed", path)
            return self.send_json(200, d.health_doc())
        read_only = method == "GET" and (path in ("/api/status", "/api/valves", "/api/sensors", "/api/events",
                                                  "/api/stm/motor", "/api/stm/flash", "/api/import-report")
                                         or re.fullmatch(r"/api/valves/(\d+)/profile", path) is not None)
        if not self.authorized(read_only):
            return self.drain()
        if "busy" in d.scenarios:
            d.requests += 1
            if d.requests % 4 == 0:
                self.drain()
                return self.error(503, "busy", "response buffers in use")
        with d.lock:
            self.dispatch(method, path, q)

    def non_api(self, method, path):
        d = self.dev
        if path in LEGACY_GONE:
            self.drain()
            return self.error(410, "gone", LEGACY_GONE[path])
        want = LEGACY_ALIASES.get(path)
        if want:
            if method != want:
                self.drain()
                return self.error(405, "method_not_allowed", path)
            if self.guard(method, "legacy"):
                return
            if not self.authorized(path != "/setvalve"):
                return self.drain()
            with d.lock:
                if path == "/valves":
                    return self.send_json(200, d.legacy_valves())
                if path == "/temps":
                    return self.send_json(200, d.legacy_temps())
                if path == "/volts":
                    return self.send_json(200, d.legacy_volts())
                doc = self.json_body()
                if doc is None:
                    return
                valve, t = doc.get("valve"), round_target(doc.get("value"))
                if isinstance(valve, bool) or not isinstance(valve, int) or not 1 <= valve <= 12 or t is None:
                    return self.error(400, "out_of_range", "valve 1..12, value 0..100")
                if not d.config["valves"][valve - 1]["active"]:
                    return self.error(409, "inactive", "valve not active")
                if d.queue_full():
                    return self.error(503, "queue_full", "STM command queue full")
                d.set_target(valve - 1, t)
                return self.send_json(200, {"res": "ok"})
        if self.length():
            self.drain()
            return self.error(404, "not_found", path) if method == "GET" else self.error(405, "method_not_allowed", path)
        if method != "GET":
            return self.error(405, "method_not_allowed", path)
        return self.static(path)

    def stm_refused(self, v3=False):
        """409 stm_unsupported for STM actions of a too old STM, and for protocol 3 commands."""
        d = self.dev
        if "tooold" in d.scenarios:
            self.error(409, "stm_unsupported", f"STM firmware {d.stm_version} is older than 1.4.0: update the STM")
            return True
        if v3 and d.proto < 3:
            self.error(409, "stm_unsupported", "STM protocol 3 required")
            return True
        return False

    KNOWN = {"/api/status": "GET", "/api/valves": "GET", "/api/sensors": "GET", "/api/events": "GET",
             "/api/config": ("GET", "POST"), "/api/config/export": "GET", "/api/stm/motor": ("GET", "POST"),
             "/api/stm/images": ("GET", "POST"), "/api/stm/flash": ("GET", "POST"), "/api/log": "GET",
             "/api/files": ("GET", "DELETE"), "/api/import-report": ("GET", "DELETE"), "/api/stm/reset": "POST",
             "/api/stm/flash/abort": "POST", "/api/ota/esp": "POST", "/api/system/reboot": "POST",
             "/api/system/factory-reset": "POST", "/api/mqtt/reconnect": "POST", "/api/mqtt/discovery": "POST",
             "/api/valves/calibrate": "POST", "/api/valves/assembly": "POST", "/api/valves/detect": "POST",
             "/api/sensors/scan": "POST", "/api/valves/stop": "POST", "/api/stm/safe-mode/leave": "POST",
             "/api/system/network/confirm": "POST", "/api/system/network/revert": "POST"}

    def dispatch(self, method, path, q):
        d = self.dev
        m = re.fullmatch(r"/api/valves/([1-9]|1[0-2])/([a-z-]+)", path)
        if m:
            return self.valve_route(method, int(m.group(1)) - 1, m.group(2))
        known = self.KNOWN.get(path)
        if known is None and not re.fullmatch(r"/api/stm/images/[^/]+", path):
            self.drain()
            return self.error(404, "not_found", path)
        if known is not None and method not in ((known,) if isinstance(known, str) else known):
            self.drain()
            return self.error(405, "method_not_allowed", path)
        if method == "GET":
            if path == "/api/status":
                return self.send_json(200, d.status())
            if path == "/api/valves":
                return self.send_json(200, d.valves_doc())
            if path == "/api/sensors":
                return self.send_json(200, d.sensors_doc())
            if path == "/api/events":
                doc = d.events_doc(q)
                return self.send_json(200, doc) if doc else self.error(400, "bad_request", "since/limit/valve")
            if path == "/api/config":
                return self.send_json(200, d.config_doc())
            if path == "/api/config/export":
                secrets = q.get("secrets")
                if secrets is None:
                    return self.send_json(200, d.config_doc(),
                                          {"Content-Disposition": 'attachment; filename="vdmot-config.json"'})
                if secrets != ["1"]:
                    return self.error(400, "bad_request", "secrets=1")
                w = d.config["web"]
                if not (w["user"] and w["password"]):
                    return self.error(403, "auth_required", "enable web login to export passwords")
                return self.send_json(200, d.config_doc(secrets=True),
                                      {"Content-Disposition": 'attachment; filename="vdmot-config-secrets.json"'})
            if path == "/api/stm/motor":
                return self.send_json(200, {"motor": d.motor, "learnMovements": d.learn, "breakaway": d.breakaway,
                                            "known": True})
            if path == "/api/stm/images":
                return self.send_json(200, [d.image_info(img) for img in d.images.values()])
            if path == "/api/stm/flash":
                return self.send_json(200, self.flash_doc())
            if path == "/api/log":
                return self.log_text()
            if path == "/api/files":
                return self.send_json(200, d.files_doc())
            if path == "/api/import-report":
                if d.import_report is None:
                    return self.error(404, "not_found", "no import report")
                return self.send_json(200, d.import_report)
        if method == "DELETE":
            if path == "/api/import-report":
                if d.import_report is None:
                    return self.error(404, "not_found", "no import report")
                d.import_report = None
                return self.send_json(204)
            if path == "/api/files":
                p = q.get("path", [None])[0]
                if not fs_path_valid(p):
                    return self.error(400, "bad_path", p if p is not None else "path")
                kind = file_kind(p)
                known_files = d.files_doc()["files"]
                if not any(f["path"] == p for f in known_files):
                    return self.error(404, "not_found", p)
                if kind in PROTECTED:
                    return self.error(403, "protected", PROTECTED[kind])
                size = d.files.pop(p, 0)
                d.event(121, a1=1, a2=(size + 1023) // 1024, text=p)
                return self.send_json(204)
            m = re.fullmatch(r"/api/stm/images/([A-Za-z0-9_-][A-Za-z0-9._-]{0,30})", path)
            if not m:
                return self.error(404, "not_found", path)
            if d.flash.get("phase") not in ("idle", "done", "failed"):
                return self.error(409, "flashing", "STM flash in progress")
            name = image_name(m.group(1))
            if name is None:
                return self.error(400, "bad_name", m.group(1))
            if d.images.pop(name, None) is None:
                return self.error(404, "not_found", name)
            return self.send_json(204)
        if path == "/api/stm/images":
            return self.stm_upload()
        if path == "/api/ota/esp":
            return self.esp_upload(q)
        doc = self.json_body()
        if doc is None:
            return
        active = [i for i in range(12) if d.config["valves"][i]["active"]]
        if path == "/api/config":
            dry = q.get("dryRun")
            if dry is not None and dry != ["1"]:
                return self.error(400, "bad_request", "dryRun=1")
            if not doc:
                return self.error(400, "bad_request", "JSON body required")
            code, body = d.apply_patch(doc, dry_run=dry is not None)
            return self.send_json(code, body)
        if path in ("/api/valves/calibrate", "/api/valves/assembly", "/api/valves/detect", "/api/sensors/scan",
                    "/api/stm/motor") and self.stm_refused():
            return
        if path in ("/api/valves/calibrate", "/api/valves/assembly", "/api/valves/detect", "/api/sensors/scan") \
                and d.queue_full():
            return self.error(503, "queue_full", "STM command queue full")
        if path == "/api/valves/calibrate":
            d.calibrate(active)
            return self.accepted()
        if path == "/api/valves/assembly":
            for i in active:
                v = d.valves[i]
                if v["state"] != 9:
                    v.update(state=7, moveFrom=v["pos"], moveStart=time.time())
            return self.accepted()
        if path == "/api/valves/stop":
            if self.stm_refused(v3=True):
                return
            return self.accepted()
        if path == "/api/stm/safe-mode/leave":
            if self.stm_refused(v3=True):
                return
            d.safe_mode = False
            d.event(320)
            return self.accepted()
        if path in ("/api/system/network/confirm", "/api/system/network/revert"):
            if not d.trial:
                return self.error(409, "no_trial", "no network trial running")
            if path.endswith("revert"):
                d.config["net"] = d.trial["previous"]
                d.event(209, a1=4, text="previous address")
                d.request_restart()
            else:
                d.event(208, a1=int(TRIAL_S - (d.trial["until"] - time.time())))
            d.trial = None
            return self.accepted()
        if path in ("/api/valves/detect", "/api/sensors/scan", "/api/mqtt/reconnect"):
            if path == "/api/sensors/scan":
                d.event(502, a1=len(d.temps), a2=0)
            return self.accepted()
        if path == "/api/mqtt/discovery":
            action = doc.get("action")
            mq = d.config["mqtt"]
            if mq["mode"] == 0:
                return self.error(409, "disabled", "MQTT is off")
            if action not in ("publish", "delete", "republish"):
                return self.error(400, "bad_request", "action publish|delete|republish")
            if action != "delete" and not mq["separate"]:
                return self.error(409, "separate_required", "enable mqtt.separate first")
            d.event(205, a1=0 if action == "delete" else 58, a2=58 if action == "delete" else 0)
            return self.accepted()
        if path == "/api/stm/motor":
            return self.motor_set(doc)
        if path == "/api/stm/reset":
            if doc.get("confirm") is not True:
                return self.error(400, "confirm_required", '{"confirm":true}')
            d.stm_resets += 1
            d.stm_boot_wall = time.time()
            d.event(304)
            d.calibrate(active)
            return self.accepted()
        if path == "/api/stm/flash":
            return self.flash_start(doc)
        if path == "/api/stm/flash/abort":
            if d.flash.get("phase") in ("idle", "done", "failed"):
                return self.error(409, "idle", "no flash running")
            d.flash.update(phase="failed", error={"code": "aborted", "phase": d.flash["phase"], "addr": None})
            d.link = "up"
            return self.accepted()
        if path == "/api/system/reboot":
            d.request_restart()
            return self.send_json(202, {"result": "restarting"})
        if path == "/api/system/factory-reset":
            if doc.get("confirm") != "factory-reset":
                return self.error(400, "confirm_required", '{"confirm":"factory-reset"}')
            d.config = d._default_config(None)
            d.request_restart()
            return self.send_json(202, {"result": "restarting"})
        return self.error(404, "not_found", path)

    def valve_route(self, method, i, what):
        d = self.dev
        v = d.valves[i]
        routes = {"target": "POST", "calibrate": "POST", "assembly": "POST", "service-move": "POST",
                  "sensors": "POST", "profile": ("GET", "POST"), "stop": "POST"}
        if what not in routes:
            self.drain()
            return self.error(404, "not_found", self.path)
        if method not in ((routes[what],) if isinstance(routes[what], str) else routes[what]):
            self.drain()
            return self.error(405, "method_not_allowed", self.path)
        if what == "profile" and method == "GET":
            if d.proto < 2 or not v["profile"]:
                return self.error(404, "not_found", "no profile")
            return self.send_json(200, {"valve": i + 1, "count": len(v["profile"]), "samples": v["profile"]})
        doc = self.json_body()
        if doc is None:
            return
        active = d.config["valves"][i]["active"]
        if what == "target":
            t = round_target(doc.get("target"))
            if t is None or set(doc) != {"target"}:
                return self.error(400, "out_of_range", "target 0..100")
            if not active:
                return self.error(409, "inactive", "valve not active")
            if d.queue_full():
                return self.error(503, "queue_full", "STM command queue full")
            d.set_target(i, t)
            return self.send_json(202, {"valve": i + 1, "target": t})
        if self.stm_refused(v3=what == "stop"):
            return
        if d.queue_full():
            return self.error(503, "queue_full", "STM command queue full")
        # calibrate, assembly and service-move are accepted for inactive valves (firmware behaviour)
        if what == "stop":
            return self.accepted()
        if what == "calibrate":
            d.calibrate([i])
            return self.accepted()
        if what == "assembly":
            v.update(state=7, moveFrom=v["pos"], moveStart=time.time())
            return self.accepted()
        if what == "service-move":
            if d.proto < 2:
                return self.error(409, "unsupported", "STM protocol v2 required")
            direction, counts, ma = doc.get("dir"), doc.get("counts"), doc.get("maxmA")
            if direction not in ("open", "close") or not isinstance(counts, int) or isinstance(counts, bool) \
                    or not 1 <= counts <= 10000 or not isinstance(ma, int) or isinstance(ma, bool) or not 5 <= ma <= 60:
                return self.error(400, "out_of_range", "dir open|close, counts 1..10000, maxmA 5..60")
            d._record_move(v, direction, counts, v["blocked"] is not None and direction == "close")
            v["moves"] += 1
            stop = 3 if v["lastMove"]["stop"] == "early_endstop" else 1
            d.event(414, valve=i, a1=v["lastMove"]["cnt"], a2=stop)
            return self.accepted()
        if what == "sensors":
            s1, s2 = doc.get("slot1"), doc.get("slot2")
            for s in (s1, s2):
                if not isinstance(s, int) or isinstance(s, bool) or not 0 <= s <= 34:
                    return self.error(400, "out_of_range", "slot1/slot2 0..34, distinct")
                if s and not d.config["temps"][s - 1]["id"]:
                    return self.error(400, "invalid", "slot has no valid sensor id")
            v["slots"] = [s1, s2]
            return self.accepted()
        if d.proto < 2:
            return self.error(409, "unsupported", "STM protocol v2 required")
        return self.accepted()

    def motor_set(self, doc):
        d = self.dev
        rng = {"lowC": (10, 40), "highC": (10, 40), "startOnPower": (0, 100), "noOfMinCount": (0, 60000),
               "maxCalReps": (0, 2)}
        m = doc.get("motor")
        if m is not None:
            if not isinstance(m, dict) or not set(m) <= set(rng):
                return self.error(400, "invalid", "motor")
            for k, (lo, hi) in rng.items():
                if k in m and (not isinstance(m[k], int) or not lo <= m[k] <= hi):
                    return self.error(400, "out_of_range", f"motor.{k}")
        n = doc.get("learnMovements")
        if n is not None and (not isinstance(n, int) or not (n == 0 or 50 <= n <= 65534)):
            return self.error(400, "out_of_range", "learnMovements")
        b = doc.get("breakaway")
        if b is not None:
            if d.breakaway is None:
                return self.error(409, "unsupported", "breakaway needs STM protocol 2")
            if not isinstance(b, dict) or not isinstance(b.get("enable"), bool) \
                    or not isinstance(b.get("stepPct"), int) or not 0 <= b["stepPct"] <= 100 \
                    or not isinstance(b.get("maxmA"), int) or not 20 <= b["maxmA"] <= 60:
                return self.error(400, "out_of_range", "breakaway")
        if m is not None:
            d.motor.update(m)
        if n is not None:
            d.learn = n
        if b is not None:
            d.breakaway = {"enable": b["enable"], "stepPct": b["stepPct"], "maxmA": b["maxmA"]}
        if m is None and n is None and b is None:
            return self.error(400, "bad_request", "nothing to set")
        return self.accepted()

    def stm_upload(self):
        d = self.dev
        if d.flash.get("phase") not in ("idle", "done", "failed"):
            self.drain()
            return self.error(409, "busy", "upload or flash running")
        raw, err = self.body(STM_IMAGE_MAX + 8192)
        if err:
            return self.error(413, "too_large")
        part = parse_multipart(raw, self.headers.get("Content-Type"))
        if not part:
            return self.error(400, "invalid", "multipart")
        raw_name, data = part
        name = image_name(raw_name)
        if name is None or name == LAST_GOOD:
            return self.error(400, "upload_failed", "bad_name")
        if name not in d.images and sum(1 for n in d.images if n != LAST_GOOD) >= MAX_IMAGES:
            return self.error(507, "upload_failed", "too_many_images")
        if not data:
            return self.error(400, "upload_failed", "empty")
        if len(data) > STM_IMAGE_MAX:
            return self.error(413, "upload_failed", "too_large")
        img = d.add_image(name, data)
        # like the device: CRC known at once, version and check after the background scan
        return self.send_json(201, {"name": img["name"], "size": img["size"], "crc32": img["crc32"],
                                    "version": None, "check": None, "hw": img["hw"]})

    def esp_upload(self, q):
        d = self.dev
        raw, err = self.body(ESP_APP_MAX + 8192)
        if err:
            return self.error(413, "too_large")
        part = parse_multipart(raw, self.headers.get("Content-Type"))
        if not part:
            return self.error(400, "invalid", "multipart")
        _, data = part
        if not data or data[0] != 0xE9:
            return self.error(400, "ota_failed", "Magic byte is wrong, not 0xE9")
        want = (q.get("md5", [""])[0] or self.headers.get("X-Update-MD5", "")).lower()
        if want and hashlib.md5(data).hexdigest() != want:
            return self.error(400, "ota_failed", "MD5 Check Failed")
        d.esp_version = bump_version(d.esp_version)
        d.event(106, a1=len(data), text=d.esp_version)
        d.request_restart()
        return self.send_json(200, {"result": "ok", "restart": True})

    def flash_start(self, doc):
        d = self.dev
        if d.flash.get("phase") not in ("idle", "done", "failed"):
            return self.error(409, "busy", "upload or flash running")
        name = image_name(doc.get("image")) if isinstance(doc.get("image"), str) else None
        board = doc.get("board", "")
        if (name is None or set(doc) - {"image", "mode", "force", "board"}
                or doc.get("mode", "normal") not in ("normal", "blank") or board not in ("", "C1", "C2")
                or not isinstance(doc.get("force", False), bool)):
            return self.error(400, "bad_request", "image, mode normal|blank, force, board C1|C2")
        img = d.images.get(name)
        if img is None:
            return self.error(404, "not_found", name)
        check = d.image_info(img)["check"]
        if check is None:
            return self.error(409, "validating", "image check pending, retry")
        if check != "none" and not (doc.get("force") and check == "image_no_handshake"):
            return self.error(400, "invalid_image", check)
        running = re.search(r"_(C\d+)$", d.stm_version or "")
        board_hw = running.group(1) if running and doc.get("mode", "normal") == "normal" else board
        if not doc.get("force") and img["hw"]:
            if not board_hw:
                return self.error(409, "board_required", "choose the board: C1 or C2")
            if board_hw != img["hw"]:
                return self.error(409, "board_mismatch", f"image {img['hw']}, board {board_hw}")
        d.flash = {"phase": "validating", "status": 1, "percent": 0, "bytesDone": 0, "bytesTotal": img["size"],
                   "chipId": None, "chipName": None, "bootloaderVersion": None, "attempt": 1, "error": None,
                   "startedMs": d.now_up() * 1000, "finishedMs": None, "image": dict(img), "appVersion": None,
                   "t0": time.time(), "board": "ok" if img["hw"] else "untagged", "boardHw": board_hw or None,
                   "manualReset": False, "baud": 115200, "pending": False, "blank": doc.get("mode") == "blank"}
        d.link = "suspended"
        d.event(311, a1=img["size"], text=img["name"])
        return self.send_json(202)

    def flash_doc(self):
        f = self.dev.flash
        if f.get("phase", "idle") == "idle":
            return {"phase": "idle", "status": 0, "percent": 0, "bytesDone": 0, "bytesTotal": 0, "chipId": None,
                    "chipName": None, "bootloaderVersion": None, "attempt": 0, "error": None, "startedMs": None,
                    "finishedMs": None, "image": None, "appVersion": None, "board": "ok", "boardHw": None,
                    "manualReset": False, "baud": 0, "pending": False}
        out = {k: v for k, v in f.items() if k not in ("t0", "blank")}
        out["status"] = {"validating": 1, "resetting": 1, "handshake": 1, "sync": 1, "getid": 1, "erasing": 3,
                         "writing": 4, "verifying": 5, "starting": 5, "waiting_app": 5, "done": 6,
                         "failed": 8}[f["phase"]]
        if out.get("image"):
            out["image"] = {k: out["image"][k] for k in ("name", "size", "crc32", "version", "hw")}
            out["image"]["version"] = out["image"]["version"] or None
        return out

    def log_text(self):
        lines = []
        for e in self.dev.events:
            ts = dt.datetime.fromtimestamp(e["t"], dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
            v = f" v{e['valve']}" if e["valve"] else ""
            lines.append(f"#{e['seq']} {ts} {e['sev'].upper()} {e['name']}{v} {e['msg']}")
        body = ("\n".join(lines) + "\n").encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Content-Disposition", 'attachment; filename="vdmot-events.log"')
        self.end_headers()
        self.wfile.write(body)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--station", choices=sorted(STATIONS), default="east")
    ap.add_argument("--port", type=int, default=8080)
    ap.add_argument("--bind", default="127.0.0.1")
    ap.add_argument("--proto", type=int, choices=(1, 2, 3), default=2,
                    help="STM protocol: 1 = legacy 1.4.x, 2 = revamped 2.0, 3 = revamped 2.1")
    ap.add_argument("--auth", metavar="USER:PASS", help="enable HTTP Basic auth for changes")
    ap.add_argument("--scenario", default="", help="comma-separated: health, busy, queue, failsafe, safemode, tooold, "
                                                   "haoffline")
    ap.add_argument("--import-report", action="store_true", help="show a legacy import report")
    ap.add_argument("--station-name", help="station name (UTF-8 allowed)")
    args = ap.parse_args()
    auth = None
    if args.auth:
        user, sep, pw = args.auth.partition(":")
        if not sep or not user or not pw:
            ap.error("--auth needs USER:PASS")
        auth = (user, pw)
    scenarios = [x for x in args.scenario.split(",") if x]
    unknown = set(scenarios) - {"health", "busy", "queue", "failsafe", "safemode", "tooold", "haoffline"}
    if unknown:
        ap.error("unknown scenario: " + ", ".join(sorted(unknown)))
    Handler.dev = Device(args.station, args.proto, auth, scenarios, args.import_report, args.station_name)
    srv = ThreadingHTTPServer((args.bind, args.port), Handler)
    srv.daemon_threads = True
    print(f"VdMot mock ({args.station}, STM proto {args.proto}) on http://{args.bind}:{args.port}/", flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
