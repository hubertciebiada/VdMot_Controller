# /// script
# requires-python = ">=3.10"
# ///
"""flowbench: flow characteristic of each heating loop, measured with a float rotameter.

The tool drives the valves of one or more VdMot controllers through their HTTP API with
service moves (exact pulse counts), and a phone page shows a model of the rotameter: the user
taps where the float is. One loop at a time, every other valve closed, opening sweeps only.

  uv run tools/flowbench/flowbench.py --controller west=192.168.1.51 --controller east=192.168.1.52
  uv run tools/flowbench/flowbench.py --sim                 (two simulated controllers)
  uv run tools/flowbench/flowbench.py --sim --sim-auto      (simulated reader, no page needed)

Then open http://<this PC>:4680 on the phone. README.md next to this file explains the method.
"""
import argparse
import csv
import os
import io
import json
import math
import random
import socket
import sqlite3
import threading
import time
import traceback
import urllib.error
import urllib.parse
import urllib.request
from datetime import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

HERE = Path(__file__).resolve().parent

# STM constants the engine relies on (software_stm32_rust/core/src/calibration.rs, coast.rs,
# docs/revamped/PROTOCOL_V2.md "svmov")
CLOSE_MA = 34  # the closing bound of a calibration: floor 20 mA x factor 1.7
OPEN_MA = 30  # above the load inside the band (~15 mA), at the opening end-stop bound (~29 mA)
HOLD_S = 320  # the STM keeps a service position for about 5.5 min, then returns to its target
KEEP_S = 120  # while a reading is due, a valve stop this often restarts the hold (sstop)
STOPS = ["none", "target", "endstop", "early_endstop", "timeout", "undercurrent",
         "safety_overcurrent", "aborted"]
BAD_FLAGS = {"uncalibrated", "needsRef", "recal", "fsBlocked", "retry"}

# sweep rules
START_PULSES = 200  # first move from the closed end stop, inside the free travel
COARSE_STEP = 60  # step while the float stays at 0
DQ_TARGET = 0.15  # wanted flow change per step [l/min]
STEP_MIN, STEP_MAX = 15, 150
CAP_PULSES = 2600  # never further than this from the closed end stop
FLAT_N, FLAT_EPS = 3, 0.05  # saturation: this many steps in a row with less flow gain
SPREAD_OK = 0.12  # sweeps agree when the rms difference is below this [l/min]
SWEEPS_MIN, SWEEPS_MAX = 2, 3
CLOSE_STEP = 40  # requested pulses of a closing step; the valve also coasts 50..80 more


# ───────────────────────────── hydraulics of one loop ─────────────────────────────

WATER_NU = [(20, 1.004e-6), (25, 0.893e-6), (30, 0.801e-6), (35, 0.724e-6), (40, 0.658e-6),
            (45, 0.602e-6)]  # kinematic viscosity of water [m²/s]
KV_METER = 1.1  # KAN-therm InoxFlow flow meter, Kvs [m³/h at 1 bar]
KV_VALVE = 2.4  # M30x1.5 valve insert, Kvs
FITTINGS = 1.10  # bends of the loop: +10 % on the straight pipe


def water_nu(t):
    t = max(WATER_NU[0][0], min(WATER_NU[-1][0], t))
    for (t0, n0), (t1, n1) in zip(WATER_NU, WATER_NU[1:]):
        if t0 <= t <= t1:
            return n0 + (n1 - n0) * (t - t0) / (t1 - t0)
    return WATER_NU[-1][1]


def loop_dp(q_lpm, length_m, pipe_id_mm=12.0, water_c=35.0):
    """Pressure drop [m of water] of a loop at q: pipe (Darcy, laminar 64/Re, turbulent Blasius,
    linear between Re 2300 and 4000), flow meter and fully open valve (Kv)."""
    d = pipe_id_mm / 1000
    v = q_lpm / 60000 / (math.pi * d * d / 4)
    re = v * d / water_nu(water_c)
    lam, turb = 64 / 2300, 0.316 / 4000 ** 0.25
    if re <= 0:
        f = 0.0
    elif re < 2300:
        f = 64 / re
    elif re > 4000:
        f = 0.316 / re ** 0.25
    else:
        f = lam + (turb - lam) * (re - 2300) / 1700
    g = 9.81
    pipe = f * length_m / d * v * v / 2 / g * FITTINGS
    qh = q_lpm * 0.06
    meter = (qh / KV_METER) ** 2 * 10.2  # 1 bar = 10.2 m
    valve = (qh / KV_VALVE) ** 2 * 10.2
    return {"re": round(re), "pipe": pipe, "meter": meter, "valve": valve,
            "total": pipe + meter + valve}


def pl(x):
    """A number with one decimal and a decimal comma, for the page."""
    return f"{x:.1f}".replace(".", ",")


def design_flow(heat_w, dt_k):
    """Flow [l/min] that carries heat_w at a supply/return difference dt_k."""
    return heat_w / (4180 * dt_k) / 0.994 * 60


def load_loops(path):
    if not path or not Path(path).is_file():
        return None
    return json.loads(Path(path).read_text(encoding="utf-8"))


def design_table(loops_cfg, dt_k):
    if not loops_cfg:
        return {"available": False}
    pid = float(loops_cfg.get("pipe_id_mm", 12))
    wc = float(loops_cfg.get("water_c", 35))
    rows = []
    for lp in loops_cfg.get("loops", []):
        q = design_flow(float(lp["heat_w"]), dt_k)
        dp = loop_dp(q, float(lp["length_m"]), pid, wc)
        rows.append({"ctrl": lp["ctrl"], "idx": lp["idx"], "name": lp.get("name"),
                     "length_m": lp["length_m"], "heat_w": lp["heat_w"], "q": round(q, 2),
                     "re": dp["re"], "dp_pipe": round(dp["pipe"], 2),
                     "dp_meter": round(dp["meter"], 2), "dp_valve": round(dp["valve"], 2),
                     "dp_total": round(dp["total"], 2)})
    rows.sort(key=lambda r: -r["dp_total"])
    return {"available": True, "pipe_id_mm": pid, "water_c": wc, "dt": dt_k, "rows": rows,
            "total_q": round(sum(r["q"] for r in rows), 1),
            "need_m": rows[0]["dp_total"] if rows else None,
            "index": [[r["ctrl"], r["idx"]] for r in rows[:2]]}


class Stop(Exception):
    """The user pressed STOP."""


class Skip(Exception):
    """The user skipped the loop."""


class EngineError(Exception):
    pass


# ───────────────────────────── pump power ─────────────────────────────

class HaPower:
    """The state of one Home Assistant power sensor, read over the REST API."""

    def __init__(self, url, token, entity):
        self.url = url.rstrip("/") + "/api/states/" + entity
        self.token = token

    def read(self):
        r = urllib.request.Request(self.url, headers={"Authorization": "Bearer " + self.token})
        try:
            with urllib.request.urlopen(r, timeout=3) as f:
                return float(json.loads(f.read())["state"])
        except (OSError, ValueError, KeyError):
            return None  # a missing power value never stops a measurement


# ───────────────────────────── controller client ─────────────────────────────

class Esp:
    def __init__(self, name, base):
        self.name = name
        self.base = base.rstrip("/")
        self.ev_since = None

    def req(self, method, path, body=None, timeout=6):
        headers, data = {}, None
        if method == "POST":
            headers["X-VdMot"] = "1"
            if body is None:
                data = b""
            else:
                headers["Content-Type"] = "application/json"
                data = json.dumps(body).encode()
        r = urllib.request.Request(self.base + path, data=data, method=method, headers=headers)
        try:
            with urllib.request.urlopen(r, timeout=timeout) as f:
                raw = f.read()
        except urllib.error.HTTPError as e:
            detail = e.read().decode(errors="replace")[:200]
            raise EngineError(f"{self.name}: {method} {path} -> {e.code} {detail}") from None
        except OSError as e:
            raise EngineError(f"{self.name}: {method} {path} -> {e}") from None
        return json.loads(raw) if raw else {}

    def get(self, path):
        return self.req("GET", path)

    def post(self, path, body=None):
        return self.req("POST", path, body)

    def valves(self):
        return [v for v in self.get("/api/valves")["valves"] if v.get("active")]

    def valve(self, idx):
        for v in self.get("/api/valves")["valves"]:
            if v["idx"] == idx:
                return v
        raise EngineError(f"{self.name}: no valve {idx}")

    def events(self, valve=None):
        """New events since the last call (the first call only sets the start)."""
        q = {"limit": 50}
        if self.ev_since is not None:
            q["since"] = self.ev_since
        if valve:
            q["valve"] = valve
        d = self.get("/api/events?" + urllib.parse.urlencode(q))
        first = self.ev_since is None
        self.ev_since = d.get("next", self.ev_since)
        return [] if first else d.get("events", [])


# ───────────────────────────── measurement engine ─────────────────────────────

def interp(points, x):
    """Piecewise-linear value of sorted (x, y) points at x; None outside."""
    for (x0, y0), (x1, y1) in zip(points, points[1:]):
        if x0 <= x <= x1:
            return y0 if x1 == x0 else y0 + (y1 - y0) * (x - x0) / (x1 - x0)
    return None


def sweep_spread(a, b, top):
    """RMS flow difference of the points of sweep b against the line of sweep a, inside the band."""
    line = sorted((p["pos"], p["flow"]) for p in a if p["kind"] == "value")
    res = []
    for p in b:
        if p["kind"] != "value" or not 0 < p["flow"] < top:
            continue
        q = interp(line, p["pos"])
        if q is not None:
            res.append(p["flow"] - q)
    return math.sqrt(sum(r * r for r in res) / len(res)) if len(res) >= 2 else None


def onset_below(points):
    """The position of the last zero reading before the flow starts, or 0."""
    last = 0
    for p in points:
        if p["kind"] == "zero" or (p["kind"] == "value" and p["flow"] <= 0):
            last = p.get("pos_est", p["pos"])
        else:
            break
    return last


def next_step(points, top, dq=DQ_TARGET):
    """Pulses of the next opening step, or (None, reason) when the sweep is complete."""
    last = points[-1]
    if last["kind"] == "over" or (last["kind"] == "value" and last["flow"] >= top - 1e-6):
        return None, "top"
    if last["pos"] >= CAP_PULSES:
        return None, "cap"
    flowing = [p for p in points if p["kind"] == "value" and p["flow"] > 0]
    if not flowing:
        return COARSE_STEP, None
    # with FLAT_N points or fewer both slices are the same list and every gain would be 0
    gains = [b["flow"] - a["flow"] for a, b in zip(points[-FLAT_N - 1:], points[-FLAT_N:])]
    if len(points) > FLAT_N and last["flow"] > 0.3 and all(g < FLAT_EPS for g in gains):
        return None, "flat"
    if len(points) < 2:
        return COARSE_STEP, None
    # slope from the last two points (the last zero point counts as the onset side)
    a, b = points[-2], points[-1]
    if b["pos"] == a["pos"]:
        return COARSE_STEP, None
    slope = (b["flow"] - a["flow"]) / (b["pos"] - a["pos"])
    step = STEP_MAX if slope <= 0 else dq / slope
    return int(max(STEP_MIN, min(STEP_MAX, step))), None


# ───────────────────────────── session store ─────────────────────────────

# Each entry moves the schema one version up (PRAGMA user_version). A new version of the tool
# appends to this list and never edits an entry that a database file may already have.
MIGRATIONS = [
    """
    CREATE TABLE sessions (
        id INTEGER PRIMARY KEY,
        started TEXT NOT NULL,
        ended TEXT,
        status TEXT NOT NULL,          -- running, done, stopped, error
        mode TEXT NOT NULL,            -- loops, head
        pump_head_m REAL,              -- the pump head of a loop characteristic
        notes TEXT,
        meta TEXT                      -- JSON: scale, controllers with oc/cc, head test settings
    );
    CREATE TABLE points (
        id INTEGER PRIMARY KEY,
        session_id INTEGER NOT NULL REFERENCES sessions(id),
        ctrl TEXT NOT NULL, valve INTEGER NOT NULL, name TEXT,
        sweep INTEGER, point INTEGER,
        pos_counted INTEGER, pos_requested INTEGER, step INTEGER, step_counted INTEGER,
        stop TEXT,
        kind TEXT,                     -- value, zero, over, void
        flow_lpm REAL, tap_frac REAL, waited_s REAL, t_1 REAL, t_2 REAL,
        t TEXT NOT NULL
    );
    CREATE INDEX points_loop ON points (ctrl, valve, session_id);
    CREATE TABLE sweeps (
        id INTEGER PRIMARY KEY,
        session_id INTEGER NOT NULL REFERENCES sessions(id),
        ctrl TEXT NOT NULL, valve INTEGER NOT NULL, sweep INTEGER,
        reason TEXT, pos_counted INTEGER, pos_requested INTEGER,
        back_counted INTEGER, back_stop TEXT,
        t TEXT NOT NULL
    );
    CREATE TABLE head_points (
        id INTEGER PRIMARY KEY,
        session_id INTEGER NOT NULL REFERENCES sessions(id),
        ctrl TEXT NOT NULL, valve INTEGER NOT NULL, name TEXT,
        head_m REAL, power_w REAL, target_pct INTEGER,
        kind TEXT, flow_lpm REAL, design_lpm REAL, tap_frac REAL, t_1 REAL, t_2 REAL,
        t TEXT NOT NULL
    );
    CREATE TABLE head_steps (
        id INTEGER PRIMARY KEY,
        session_id INTEGER NOT NULL REFERENCES sessions(id),
        head_m REAL, power_w REAL, ok INTEGER, flows TEXT, recommend TEXT,
        t TEXT NOT NULL
    );
    CREATE TABLE events (
        id INTEGER PRIMARY KEY,
        session_id INTEGER NOT NULL REFERENCES sessions(id),
        kind TEXT NOT NULL,            -- log, void
        text TEXT,
        t TEXT NOT NULL
    );
    """,
    """
    ALTER TABLE points ADD COLUMN direction TEXT NOT NULL DEFAULT 'open';  -- open, close
    -- the position a single move from the closed end stop would count, set at the sweep end
    ALTER TABLE points ADD COLUMN pos_est INTEGER;
    """,
    """
    ALTER TABLE points ADD COLUMN power_w REAL;  -- pump power at the reading (--ha-power)
    """,
]


def iso(t=None):
    return datetime.fromtimestamp(time.time() if t is None else t).isoformat(timespec="seconds")


def num(x):
    try:
        return None if x is None or x == "" else float(x)
    except (TypeError, ValueError):
        return None


class Store:
    """flowbench.db: every session, one row per reading. The file stays when the tool changes;
    open() brings an older file to the current schema."""

    def __init__(self, path):
        self.path = path
        self.db = sqlite3.connect(path, check_same_thread=False)
        self.db.execute("PRAGMA foreign_keys = ON")
        self.lock = threading.Lock()
        have = self.db.execute("PRAGMA user_version").fetchone()[0]
        if have > len(MIGRATIONS):
            raise SystemExit(f"{path}: schema {have} is newer than this flowbench "
                             f"({len(MIGRATIONS)}); update the tool")
        for n in range(have, len(MIGRATIONS)):
            self.db.executescript("BEGIN;" + MIGRATIONS[n] + f"PRAGMA user_version = {n + 1};"
                                  "COMMIT;")

    def exec(self, sql, args=()):
        with self.lock, self.db:
            return self.db.execute(sql, args).lastrowid

    def begin(self, mode, pump_head, notes, meta):
        return self.exec("INSERT INTO sessions (started, status, mode, pump_head_m, notes, meta) "
                         "VALUES (?, 'running', ?, ?, ?, ?)",
                         (iso(), mode, num(pump_head), notes,
                          json.dumps(meta, ensure_ascii=False)))

    def end(self, sid, status):
        self.exec("UPDATE sessions SET ended = ?, status = ? WHERE id = ?", (iso(), status, sid))

    def record(self, sid, rec):
        t = iso(rec.get("t"))
        if "point" in rec:
            r = rec["point"]
            temps = [x.get("temp") for x in r.get("temps") or []] + [None, None]
            self.exec("INSERT INTO points (session_id, ctrl, valve, name, sweep, point, "
                      "pos_counted, pos_requested, step, step_counted, stop, kind, flow_lpm, "
                      "tap_frac, waited_s, t_1, t_2, t, direction, power_w) VALUES "
                      "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                      (sid, r["ctrl"], r["idx"], r["name"], r["sweep"], r["point"], r["pos"],
                       r["pos_req"], r["step"], r["cnt"], r["stop"],
                       r["kind"], r["flow"], num(r.get("tap")),
                       r["waited_s"], num(temps[0]), num(temps[1]), iso(r["t"]),
                       r.get("dir", "open"), num(r.get("power"))))
        elif "pos_est" in rec:
            c = rec["pos_est"]
            for point, pos in c["pos"].items():
                self.exec("UPDATE points SET pos_est = ? WHERE session_id = ? AND ctrl = ? "
                          "AND valve = ? AND sweep = ? AND point = ?",
                          (pos, sid, c["ctrl"], c["idx"], c["sweep"], int(point)))
        elif "void" in rec:
            v = rec["void"]
            # the STM returned the valve to its target: no point of this sweep has a reference
            self.exec("UPDATE points SET kind = 'void' WHERE session_id = ? AND ctrl = ? "
                      "AND valve = ? AND sweep = ?", (sid, v["ctrl"], v["idx"], v["sweep"]))
            self.exec("INSERT INTO events (session_id, kind, text, t) VALUES (?, 'void', ?, ?)",
                      (sid, json.dumps(v, ensure_ascii=False), t))
        elif "sweep_end" in rec:
            e = rec["sweep_end"]
            self.exec("INSERT INTO sweeps (session_id, ctrl, valve, sweep, reason, pos_counted, "
                      "pos_requested, back_counted, back_stop, t) VALUES "
                      "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                      (sid, e["ctrl"], e["idx"], e["sweep"], e["reason"], e["pos_cnt"],
                       e["pos_req"], e["back_cnt"], e["back_stop"], t))
        elif "head_point" in rec:
            r = rec["head_point"]
            temps = [x.get("temp") for x in r.get("temps") or []] + [None, None]
            self.exec("INSERT INTO head_points (session_id, ctrl, valve, name, head_m, power_w, "
                      "target_pct, kind, flow_lpm, design_lpm, tap_frac, t_1, t_2, t) VALUES "
                      "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                      (sid, r["ctrl"], r["idx"], r["name"], r["head"], num(r.get("power")),
                       r["target"], r["kind"], r["flow"], num(r.get("design")),
                       num(r.get("tap")), num(temps[0]), num(temps[1]), iso(r["t"])))
        elif "head_step" in rec:
            h = rec["head_step"]
            self.exec("INSERT INTO head_steps (session_id, head_m, power_w, ok, flows, recommend, "
                      "t) VALUES (?, ?, ?, ?, ?, ?, ?)",
                      (sid, h["head"], num(h.get("power")), int(bool(h["ok"])),
                       json.dumps(h["flows"], ensure_ascii=False), rec.get("recommend"), t))
        elif "log" in rec:
            self.exec("INSERT INTO events (session_id, kind, text, t) VALUES (?, 'log', ?, ?)",
                      (sid, rec["log"], t))


class Engine:
    def __init__(self, esps, outdir):
        self.esps = {e.name: e for e in esps}
        self.outdir = outdir
        self.lock = threading.Condition()
        self.thread = None
        self.inbox = None  # a reading or command from the page
        self.stop_flag = False
        self.paused = False
        self.snapshot = {}
        self.store = None  # opened on the first session
        self.sid = None  # the id of the running session in the store
        self.rows = []  # every recorded point of the session
        self.close_sweep = False  # one closing sweep per loop after the opening sweeps
        self.dq = DQ_TARGET  # wanted flow change per step of this session
        self.power = None  # reads the pump power (HaPower), or None
        self.decide_s = 20  # the page can answer "enough" / "one more" this long after a sweep
        self.expected = {}  # (ctrl, idx) -> the target the app set; absent = 0
        self.head_rows = []  # readings of a pump head test
        self.on_head = None  # the simulator follows the pump head the user enters
        self.loops_cfg = None  # loops.json: length and heat of every loop
        self.state = {"phase": "setup", "msg": "", "log": [], "loops": [], "current": None,
                      "points": [], "scale": {"max": 2.5, "major": 0.5, "minor": 0.25},
                      "settle_s": 30, "settle_until": 0, "hold_until": 0, "rule": "",
                      "session": None, "error": None, "restorable": False, "notice": "",
                      "mode": "loops", "head_results": [], "head_reads": [], "recommend": ""}

    # page side
    def view(self):
        with self.lock:
            s = json.loads(json.dumps(self.state))
        s["now"] = time.time()
        s["paused"] = self.paused
        return s

    def post(self, item):
        with self.lock:
            if item.get("cmd") == "stop":
                self.stop_flag = True
            elif item.get("cmd") == "pause":
                self.paused = True
            elif item.get("cmd") == "resume":
                self.paused = False
            else:
                self.inbox = item
            self.lock.notify_all()

    def start(self, cfg):
        with self.lock:
            if self.thread and self.thread.is_alive():
                raise EngineError("Pomiar już trwa.")
            self.stop_flag, self.paused, self.inbox = False, False, None
            self.state.update(phase="prepare", error=None, msg="Start", points=[], loops=[],
                              restorable=False, mode=cfg.get("mode", "loops"), head_results=[],
                              head_reads=[], recommend="", current=None, notice="")
            self.state["scale"] = cfg.get("scale") or self.state["scale"]
            self.state["settle_s"] = int(cfg.get("settle_s", 30))
        self.thread = threading.Thread(target=self.run, args=(cfg,), daemon=True)
        self.thread.start()

    # engine side
    def set(self, **kw):
        with self.lock:
            self.state.update(kw)

    def log(self, text):
        line = time.strftime("%H:%M:%S ") + text
        with self.lock:
            self.state["log"] = (self.state["log"] + [line])[-40:]
        self.write({"t": time.time(), "log": text})

    def write(self, rec):
        if self.sid is not None:
            self.store.record(self.sid, rec)

    def check(self):
        with self.lock:
            while self.paused and not self.stop_flag:
                self.state["msg"] = "Pauza"
                self.lock.wait(0.5)
            if self.stop_flag:
                raise Stop()

    def wait_input(self, timeout=None):
        """Blocks until the page sends a reading or a command."""
        t_end = None if timeout is None else time.time() + timeout
        with self.lock:
            while self.inbox is None:
                if self.stop_flag:
                    raise Stop()
                if t_end is not None and time.time() > t_end:
                    return None
                self.lock.wait(0.5)
            item, self.inbox = self.inbox, None
            return item

    def sleep(self, s):
        t_end = time.time() + s
        while time.time() < t_end:
            self.check()
            time.sleep(min(0.3, max(0.0, t_end - time.time())))

    def service_move(self, esp, idx, direction, counts, ma):
        """One service move; returns (counted pulses, stop name)."""
        self.check()
        esp.events(idx)  # drop old events
        v0 = esp.valve(idx)
        seq0 = v0.get("ext", {}).get("moveSeq")
        esp.post(f"/api/valves/{idx}/service-move",
                 {"dir": direction, "counts": int(counts), "maxmA": int(ma)})
        t0, started = time.time(), False
        while True:
            time.sleep(0.4)
            for e in esp.events(idx):
                if e.get("code") == 414:
                    a2 = e.get("a2", 0)
                    return int(e.get("a1", 0)), STOPS[a2] if 0 <= a2 < len(STOPS) else str(a2)
                if e.get("code") == 411:
                    raise EngineError(f"{esp.name} {idx}: STM odrzucił ruch serwisowy "
                                      f"(kalibracja w kolejce albo zawór zajęty): {e.get('msg')}")
            v = esp.valve(idx)
            ext = v.get("ext", {})
            if v.get("stateKey") in ("opening", "closing"):
                started = True
            if ext.get("moveSeq") != seq0 and v.get("stateKey") == "idle":
                lm = ext.get("lastMove") or {}
                return int(lm.get("cnt", 0)), lm.get("stop", "?")
            el = time.time() - t0
            if not started and el > 12:
                raise EngineError(f"{esp.name} {idx}: ruch serwisowy nie ruszył w 12 s")
            if el > 120:
                raise EngineError(f"{esp.name} {idx}: ruch serwisowy trwa ponad 120 s")

    def foreign_target(self):
        """A valve whose target the app did not set (HA or a schedule still drives the valves)."""
        for esp in self.esps.values():
            for v in esp.valves():
                want = self.expected.get((esp.name, v["idx"]), 0)
                if v.get("target") not in (want, None):
                    return f"{esp.name} {v['idx']} ({v.get('name')}) ma cel {v.get('target')} %"
        return None

    def set_targets(self, targets):
        """targets: {(ctrl, idx): pct}. Waits until every valve stands at its target."""
        self.expected = dict(targets)
        for (c, i), t in targets.items():
            self.esps[c].post(f"/api/valves/{i}/target", {"target": t})
        t0, resent = time.time(), False
        while True:
            self.sleep(2)
            off = []
            for esp in self.esps.values():
                for v in esp.valves():
                    t = targets.get((esp.name, v["idx"]))
                    if t is not None and (v.get("stateKey") != "idle"
                                          or abs((v.get("pos") or 0) - t) > 1):
                        off.append((esp, v, t))
            if not off:
                return
            self.set(msg="Czekam na zawory: " + ", ".join(
                f"{e.name} {v['idx']} {v.get('pos')}→{t} %" for e, v, t in off[:5]))
            el = time.time() - t0
            if el > 150 and not resent:
                # an opening from 0 % may stop early (sticky pin); one more request
                resent = True
                for e, v, t in off:
                    if v.get("stateKey") == "idle":
                        e.post(f"/api/valves/{v['idx']}/target", {"target": t})
                        self.log(f"{e.name} {v['idx']}: cel {t} % wysłany drugi raz")
            if el > 300:
                raise EngineError("Zawory nie doszły do celu w 5 min: " + ", ".join(
                    f"{e.name} {v['idx']} ({v.get('name')}) {v.get('pos')} % zamiast {t} %"
                    for e, v, t in off))

    def prepare_closed(self):
        """Every active valve on every controller to target 0, then wait until all are at 0."""
        self.expected = {}
        self.set(phase="prepare", msg="Zamykam wszystkie zawory")
        for esp in self.esps.values():
            for v in esp.valves():
                esp.post(f"/api/valves/{v['idx']}/target", {"target": 0})
        t0 = time.time()
        while True:
            self.sleep(2)
            open_ = [f"{esp.name} {v['idx']}" for esp in self.esps.values() for v in esp.valves()
                     if v.get("pos") != 0 or v.get("stateKey") != "idle"]
            if not open_:
                return
            self.set(msg="Czekam na zamknięcie: " + ", ".join(open_[:6]))
            if time.time() - t0 > 240:
                raise EngineError("Zawory nie zamknęły się w 4 min: " + ", ".join(open_))

    def precheck(self, loops):
        problems = []
        for esp in self.esps.values():
            st = esp.get("/api/status")
            # firmware 2.2.7: stm.lease is the object, stm.status.lease only its state name
            lease = st.get("lease") or (st.get("stm") or {}).get("lease") or {}
            if isinstance(lease, dict) and lease.get("regulator") not in (None, "alive"):
                problems.append(f"{esp.name}: regulator {lease.get('regulator')} "
                                f"- failsafe może ruszyć zawory")
            vs = {v["idx"]: v for v in esp.valves()}
            for lp in loops:
                if lp["ctrl"] != esp.name:
                    continue
                v = vs.get(lp["idx"])
                if not v:
                    problems.append(f"{esp.name} {lp['idx']}: zawór nieaktywny")
                    continue
                flags = set(v.get("ext", {}).get("v3", {}).get("flags", []))
                if v.get("stateKey") not in ("idle", "opening", "closing"):
                    problems.append(f"{esp.name} {lp['idx']}: stan {v.get('stateKey')}")
                if flags & BAD_FLAGS:
                    problems.append(f"{esp.name} {lp['idx']}: flagi {sorted(flags & BAD_FLAGS)}")
        return problems

    def power_w(self):
        return self.power.read() if self.power else None

    def temps(self, esp, idx):
        v = esp.valve(idx)
        return [{"name": s.get("name"), "temp": s.get("temp")} for s in v.get("sensors", [])]

    def run(self, cfg):
        if self.store is None:
            self.outdir.mkdir(parents=True, exist_ok=True)
            self.store = Store(self.outdir / "flowbench.db")
        self.rows, self.head_rows = [], []
        self.close_sweep = bool(cfg.get("close_sweep"))
        self.dq = float(cfg.get("dq") or DQ_TARGET)
        loops = cfg.get("loops", [])
        self.set(loops=[dict(lp, status="czeka", sweeps=[], spread=None) for lp in loops])
        try:
            meta = {"scale": self.state["scale"], "settle_s": self.state["settle_s"],
                    "controllers": {}}
            for esp in self.esps.values():
                st = esp.get("/api/status")
                meta["controllers"][esp.name] = {
                    "base": esp.base, "esp": st.get("esp", {}).get("version"),
                    "stm": st.get("stm", {}).get("version"),
                    "valves": {v["idx"]: {"name": v.get("name"), "oc": v.get("oc"),
                                          "cc": v.get("cc")} for v in esp.valves()}}
                self.snapshot[esp.name] = {v["idx"]: v.get("target") for v in esp.valves()}
            meta["cfg"] = {k: cfg.get(k) for k in ("read", "others_pct", "pump_s",
                                                   "margin_pct", "dt", "close_sweep", "dq")}
            self.sid = self.store.begin(cfg.get("mode", "loops"), cfg.get("pump_head"),
                                        cfg.get("notes", ""), meta)
            self.set(session=f"{self.store.path} #{self.sid}", restorable=True)
            head = cfg.get("mode") == "head"
            every = [{"ctrl": e.name, "idx": v["idx"]} for e in self.esps.values()
                     for v in e.valves()]
            problems = self.precheck(every if head else loops)
            if problems:
                raise EngineError("Nie zaczynam: " + "; ".join(problems))
            if head:
                self.head_test(cfg)
            else:
                for n, lp in enumerate(loops):
                    self.measure_loop(n, lp)
            self.set(phase="done", msg="Pomiar skończony", current=None, notice="")
            self.log("Pomiar skończony")
            self.restore()
        except Stop:
            for esp in self.esps.values():
                try:
                    esp.post("/api/valves/stop")
                except EngineError as e:
                    self.log(str(e))
            self.set(phase="stopped", notice="", msg="Zatrzymane. Zawory stoją. Przywróć cele, gdy chcesz.")
            self.log("STOP")
        except EngineError as e:
            self.set(phase="error", error=str(e), msg=str(e), notice="")
            self.log("Błąd: " + str(e))
        except Exception as e:  # a bug must reach the page, not end the thread in silence
            traceback.print_exc()
            msg = f"Błąd programu: {type(e).__name__}: {e}. Zawory stoją, jak stały."
            self.set(phase="error", error=msg, msg=msg, notice="")
            self.log(msg)
        finally:
            if self.sid is not None:
                status = {"done": "done", "stopped": "stopped"}.get(self.state["phase"], "error")
                self.store.end(self.sid, status)
                self.sid = None

    def restore(self):
        for name, targets in self.snapshot.items():
            esp = self.esps[name]
            for idx, t in targets.items():
                if t is not None:
                    esp.post(f"/api/valves/{idx}/target", {"target": t})
        self.set(restorable=False)
        self.log("Cele sprzed pomiaru przywrócone")

    def set_loop(self, n, **kw):
        with self.lock:
            self.state["loops"][n].update(kw)

    def measure_loop(self, n, lp):
        esp = self.esps[lp["ctrl"]]
        idx = lp["idx"]
        name = esp.valve(idx).get("name") or str(idx)
        top = float(self.state["scale"]["max"])
        self.set_loop(n, status="trwa")
        sweeps = []
        try:
            self.prepare_closed()
            k = 0
            while True:
                k += 1
                offset, base = 0, START_PULSES
                if sweeps:
                    steps = [b["pos"] - a["pos"] for a, b in zip(sweeps[0], sweeps[0][1:])]
                    steps = sorted(s for s in steps if s > 0)
                    offset = steps[len(steps) // 2] // 2 if steps else COARSE_STEP // 2
                    # the first sweep found the onset: start one coarse step below its last zero
                    base = max(START_PULSES, onset_below(sweeps[0]) - COARSE_STEP)
                pts = self.sweep(esp, idx, name, n, k, base + offset, top)
                sweeps.append(pts)
                spread = sweep_spread(sweeps[0], pts, top) if k > 1 else None
                self.set_loop(n, sweeps=[len(s) for s in sweeps], spread=spread)
                more = k < SWEEPS_MIN or (spread is not None and spread > SPREAD_OK
                                          and k < SWEEPS_MAX)
                rule = (f"przebieg {k}, punkty {len(pts)}"
                        + ("" if spread is None else f", rozrzut przebiegów {spread:.2f} l/min")
                        + (" - robię jeszcze jeden" if more else " - wystarczy"))
                self.set(rule=rule, phase="decide",
                         msg="Pętla gotowa? Wybierz albo poczekaj 20 s." if not more
                         else "Następny przebieg za 20 s.")
                self.log(f"{esp.name} {idx}: {rule}")
                item = self.wait_input(timeout=self.decide_s)
                cmd = (item or {}).get("cmd")
                if cmd == "more":
                    more = k < 6
                elif cmd == "enough":
                    more = False
                if not more:
                    break
            if self.close_sweep:
                self.close_run(esp, idx, name, k + 1, sweeps[-1], top)
                self.set_loop(n, sweeps=[len(s) for s in sweeps] + ["zam."])
            self.set_loop(n, status="gotowa")
        except Skip:
            self.set_loop(n, status="pominięta")
            self.log(f"{esp.name} {idx}: pominięta")
        finally:
            try:
                esp.post(f"/api/valves/{idx}/target", {"target": 0})
            except EngineError as e:
                self.log(str(e))

    def await_reading(self, esp, idx, keep=False):
        """Waits for the float reading; returns the item or raises Skip. Restarts nothing itself.
        With keep, a stop of the valve every KEEP_S seconds restarts its service hold (the STM
        sstop leaves a stopped valve where it is for another hold), so a reading can take as long
        as the reader needs; state["hold_until"] follows each restart."""
        while True:
            item = self.wait_input(timeout=KEEP_S if keep else None)
            if item is None:
                try:
                    esp.post(f"/api/valves/{idx}/stop")
                    self.set(hold_until=time.time() + HOLD_S)
                except EngineError as e:
                    self.log(f"{esp.name} {idx}: pozycji nie przedłużyłem ({e})")
                continue
            cmd = item.get("cmd")
            if cmd == "skip":
                raise Skip()
            if cmd in ("enough", "more"):
                with self.lock:  # keep it for the decision after the sweep
                    self.state["msg"] = "Najpierw odczytaj pływak."
                continue
            if item.get("kind") in ("value", "zero", "over"):
                return item

    def close_run(self, esp, idx, name, k, opening, top):
        """One closing sweep for the hysteresis: from the end stop open in one move to where the
        opening sweep reached the top, then close in CLOSE_STEP steps with a reading after each.
        A closing move coasts pulses that the STM does not count; the counted way back to the
        end stop gives their sum, and every step gets an equal part of it."""
        cur = {"ctrl": esp.name, "idx": idx, "name": name, "sweep": k, "point": 0, "pos": 0}
        self.set(current=cur, rule="przebieg zamykający (histereza)")
        self.set(phase="moving", msg="Zamykam do krańcówki (odniesienie)")
        v = esp.valve(idx)
        cnt, stop = self.service_move(esp, idx, "close",
                                      min(10000, int((v.get("pos", 0) + 5) * v.get("oc", 5000) / 100)
                                          + 400), CLOSE_MA)
        if stop not in ("endstop", "early_endstop"):
            raise EngineError(f"{esp.name} {idx}: zamykanie nie doszło do krańcówki ({stop})")
        over = [p["pos"] for p in opening if p["kind"] == "over"]
        top_pos = min(over) if over else max(p["pos"] for p in opening)
        self.set(phase="moving", msg=f"Otwieram do {top_pos} imp")
        up, stop = self.service_move(esp, idx, "open", top_pos, OPEN_MA)
        pts, pos, steps, step, cnt = [], up, [], 0, up
        while True:
            foreign = self.foreign_target()
            if foreign:
                raise EngineError(f"Ktoś inny ustawia cele: {foreign}. Wyłącz automatykę w HA "
                                  "i zacznij pomiar od nowa.")
            hold_until = time.time() + HOLD_S
            cur.update(point=len(pts) + 1, pos=pos, step=step, cnt=cnt, stop=stop)
            self.set(current=cur, phase="reading", hold_until=hold_until,
                     settle_until=time.time() + self.state["settle_s"],
                     msg="Zamykanie: poczekaj, aż pływak się uspokoi, i zaznacz go na skali.")
            item = self.await_reading(esp, idx, keep=True)
            late = time.time() > self.state["hold_until"]
            flow = 0.0 if item["kind"] == "zero" else (top if item["kind"] == "over"
                                                       else round(float(item["value"]), 3))
            row = {"t": time.time(), "ctrl": esp.name, "idx": idx, "name": name, "sweep": k,
                   "point": len(pts) + 1, "pos": pos, "pos_req": top_pos - CLOSE_STEP * len(steps), "step": -step,
                   "cnt": -cnt if pts else cnt, "stop": stop, "kind": item["kind"], "flow": flow,
                   "tap": item.get("frac"),
                   "waited_s": round(time.time() - hold_until + HOLD_S, 1), "late": late,
                   "temps": self.temps(esp, idx), "dir": "close", "power": self.power_w()}
            self.write({"point": row})
            self.rows.append(row)
            if late:
                self.log(f"{esp.name} {idx}: odczyt po końcu blokady pozycji - zamykanie od nowa")
                self.set(notice="Odczyt przyszedł po końcu trzymania pozycji: zawór wrócił do "
                                "zera. Zaczynam przebieg zamykający od nowa.")
                for p in pts + [row]:
                    p["kind"] = "void"
                self.write({"void": {"ctrl": esp.name, "idx": idx, "sweep": k}})
                return self.close_run(esp, idx, name, k, opening, top)
            pts.append(row)
            self.set(notice="", points=[p for p in self.rows
                                        if p["ctrl"] == esp.name and p["idx"] == idx])
            if item["kind"] == "zero" or stop in ("endstop", "early_endstop") or pos <= 0:
                break
            self.set(phase="moving", msg=f"Zamykam o {CLOSE_STEP} imp")
            step = CLOSE_STEP
            cnt, stop = self.service_move(esp, idx, "close", step, CLOSE_MA)
            steps.append(cnt)
            pos = max(0, pos - cnt)
        self.set(phase="moving", msg="Zamykam i liczę drogę powrotną")
        back, bstop = (0, stop) if stop in ("endstop", "early_endstop") else \
            self.service_move(esp, idx, "close", min(10000, pos + 600), CLOSE_MA)
        moved = [c for c in steps]
        coast = max(0, up - sum(moved) - back) / len(moved) if moved else 0
        fixed, done = {}, 0
        for j, p in enumerate(pts):
            if j:
                done += moved[j - 1] + coast
            p["pos_est"] = max(0, round(up - done))
            fixed[p["point"]] = p["pos_est"]
        self.write({"pos_est": {"ctrl": esp.name, "idx": idx, "sweep": k, "pos": fixed}})
        self.set(points=[p for p in self.rows if p["ctrl"] == esp.name and p["idx"] == idx])
        self.write({"sweep_end": {"ctrl": esp.name, "idx": idx, "sweep": k, "reason": "close",
                                  "pos_cnt": up, "pos_req": top_pos, "back_cnt": back,
                                  "back_stop": bstop}})
        self.log(f"{esp.name} {idx} przebieg {k} (zamykanie): w górę {up} imp, {len(moved)} "
                 f"kroków po {CLOSE_STEP} imp, dobieg {coast:.0f} imp na krok, reszta {back} imp")

    def sweep(self, esp, idx, name, n, k, start, top):
        cur = {"ctrl": esp.name, "idx": idx, "name": name, "sweep": k, "point": 0, "pos": 0}
        self.set(current=cur, points=[p for p in self.rows if p["ctrl"] == esp.name
                                      and p["idx"] == idx])
        self.set(phase="moving", msg="Zamykam do krańcówki (odniesienie)")
        v = esp.valve(idx)
        cnt, stop = self.service_move(esp, idx, "close",
                                      min(10000, int((v.get("pos", 0) + 5) * v.get("oc", 5000) / 100)
                                          + 400), CLOSE_MA)
        self.log(f"{esp.name} {idx} przebieg {k}: odniesienie, {cnt} imp, {stop}")
        if stop not in ("endstop", "early_endstop"):
            raise EngineError(f"{esp.name} {idx}: zamykanie nie doszło do krańcówki ({stop})")
        pts, pos_req, pos_cnt, step = [], 0, 0, start
        while True:
            foreign = self.foreign_target()
            if foreign:
                raise EngineError(f"Ktoś inny ustawia cele: {foreign}. Wyłącz automatykę w HA "
                                  "i zacznij pomiar od nowa.")
            self.set(phase="moving", msg=f"Otwieram o {step} imp")
            cnt, stop = self.service_move(esp, idx, "open", step, OPEN_MA)
            pos_req += step
            pos_cnt += cnt
            hold_until = time.time() + HOLD_S
            settle = self.state["settle_s"]
            cur.update(point=len(pts) + 1, pos=pos_cnt, step=step, cnt=cnt, stop=stop)
            self.set(current=cur, phase="reading", hold_until=hold_until,
                     settle_until=time.time() + settle,
                     msg="Poczekaj, aż pływak się uspokoi, i zaznacz go na skali.")
            if stop not in ("target",):
                self.log(f"{esp.name} {idx}: ruch zatrzymany ({stop}) przy {pos_cnt} imp")
            item = self.await_reading(esp, idx, keep=True)
            late = time.time() > self.state["hold_until"]
            flow = 0.0 if item["kind"] == "zero" else (top if item["kind"] == "over"
                                                       else round(float(item["value"]), 3))
            row = {"t": time.time(), "ctrl": esp.name, "idx": idx, "name": name, "sweep": k,
                   "point": len(pts) + 1, "pos": pos_cnt, "pos_req": pos_req, "step": step,
                   "cnt": cnt, "stop": stop, "kind": item["kind"], "flow": flow,
                   "tap": item.get("frac"), "waited_s": round(time.time() - hold_until + HOLD_S, 1),
                   "late": late, "temps": self.temps(esp, idx), "power": self.power_w()}
            self.write({"point": row})
            if late:
                # the STM has returned the valve to its target: this reading is not at pos_cnt,
                # and the points before it have no reference any more
                self.log(f"{esp.name} {idx}: odczyt po końcu blokady pozycji - przebieg od nowa")
                self.set(notice="Odczyt przyszedł po końcu trzymania pozycji: zawór wrócił do "
                                "zera. Zaczynam ten przebieg od nowa.")
                for p in pts + [row]:
                    p["kind"] = "void"
                self.rows.append(row)
                self.write({"void": {"ctrl": esp.name, "idx": idx, "sweep": k}})
                return self.sweep(esp, idx, name, n, k, start, top)
            self.rows.append(row)
            pts.append(row)
            self.set(notice="")
            self.set(points=[p for p in self.rows if p["ctrl"] == esp.name and p["idx"] == idx])
            nxt, reason = next_step(pts, top, self.dq)
            if stop not in ("target",) and nxt is not None:
                nxt, reason = None, f"stop {stop}"
            if nxt is None:
                break
            step = nxt
        self.set(phase="moving", msg="Zamykam i liczę drogę powrotną")
        back, bstop = self.service_move(esp, idx, "close", min(10000, pos_cnt + 600), CLOSE_MA)
        # Every opening move coasts a few pulses that the STM does not count, so after j moves
        # the spindle is about j coasts further than the count. The counted way back to the
        # end stop holds their sum; each move gets an equal part. pos_est is the count that one
        # move from the end stop would show for the same spindle position.
        coast = max(0.0, (back - pos_cnt) / len(pts)) if pts and bstop in (
            "endstop", "early_endstop") else 0.0
        fixed = {}
        for j, p in enumerate(pts):
            p["pos_est"] = round(p["pos"] + j * coast)
            fixed[p["point"]] = p["pos_est"]
        self.write({"pos_est": {"ctrl": esp.name, "idx": idx, "sweep": k, "pos": fixed}})
        self.set(points=[p for p in self.rows if p["ctrl"] == esp.name and p["idx"] == idx])
        self.write({"sweep_end": {"ctrl": esp.name, "idx": idx, "sweep": k, "reason": reason,
                                  "pos_cnt": pos_cnt, "pos_req": pos_req, "back_cnt": back,
                                  "back_stop": bstop}})
        self.log(f"{esp.name} {idx} przebieg {k}: koniec ({reason}), w górę {pos_cnt} imp, "
                 f"w dół {back} imp ({bstop}), dobieg {coast:.0f} imp na krok")
        return pts

    # pump head test

    def head_test(self, cfg):
        """The loops in cfg["read"] at 100 %, every other loop at others_pct; for each pump head
        the user sets on the pump, the user reads the rotameters of the read loops."""
        reads = cfg["read"]
        others = int(cfg.get("others_pct", 15))
        pump_s = int(cfg.get("pump_s", 60))
        top = float(self.state["scale"]["max"])
        rkeys = {(r["ctrl"], r["idx"]) for r in reads}
        targets = {(e.name, v["idx"]): 100 if (e.name, v["idx"]) in rkeys else others
                   for e in self.esps.values() for v in e.valves()}
        self.set(phase="prepare", msg=f"Ustawiam zawory: czytane 100 %, pozostałe {others} %")
        self.set_targets(targets)
        names = {(e.name, v["idx"]): v.get("name") or str(v["idx"])
                 for e in self.esps.values() for v in e.valves()}
        reads = [dict(r, name=names[(r["ctrl"], r["idx"])]) for r in reads]
        results = []
        self.set(head_reads=reads, head_results=results, recommend="")
        while True:
            self.set(phase="head", current=None, msg="Ustaw ciśnienie na pompie, wpisz je i "
                                                     "naciśnij „Gotowe”.")
            item = self.wait_input()
            if item.get("cmd") == "finish":
                break
            if item.get("cmd") != "head":
                continue
            try:
                head = round(float(item["head"]), 2)
            except (KeyError, TypeError, ValueError):
                self.set(notice="Wpisz ciśnienie pompy w metrach.")
                continue
            power = item.get("power")
            if power is None:
                power = self.power_w()
            if self.on_head:
                self.on_head(head)
            self.set(phase="pump", notice="", settle_until=time.time() + pump_s,
                     msg=f"Pompa na {head} m: czekam {pump_s} s, aż przepływ się ustali.")
            self.sleep(pump_s)
            foreign = self.foreign_target()
            if foreign:
                raise EngineError(f"Ktoś inny ustawia cele: {foreign}. Wyłącz automatykę w HA.")
            which = list(reads)
            if item.get("all"):
                which += [{"ctrl": c, "idx": i, "name": names[(c, i)]}
                          for (c, i) in targets if (c, i) not in rkeys]
            flows = {}
            for r in which:
                key = (r["ctrl"], r["idx"])
                esp = self.esps[r["ctrl"]]
                cur = {"ctrl": r["ctrl"], "idx": r["idx"], "name": r["name"], "sweep": 0,
                       "point": len(self.head_rows) + 1, "pos": targets[key], "head": head}
                self.set(current=cur, phase="reading", settle_until=0, hold_until=0,
                         msg=f"Odczytaj rotametr pętli {r['ctrl']} {r['idx']} {r['name']}.")
                try:
                    it = self.await_reading(esp, r["idx"])
                except Skip:
                    continue
                kind = it["kind"]
                flow = 0.0 if kind == "zero" else (top if kind == "over"
                                                   else round(float(it["value"]), 3))
                row = {"t": time.time(), "ctrl": r["ctrl"], "idx": r["idx"], "name": r["name"],
                       "head": head, "power": power, "target": targets[key], "kind": kind,
                       "flow": flow, "tap": it.get("frac"), "design": r.get("design"),
                       "temps": self.temps(esp, r["idx"])}
                self.write({"head_point": row})
                self.head_rows.append(row)
                flows[f"{r['ctrl']} {r['idx']}"] = {"flow": flow, "kind": kind}
            ok = all(self.reaches(r, flows.get(f"{r['ctrl']} {r['idx']}")) for r in reads)
            results.append({"head": head, "power": power, "flows": flows, "ok": ok})
            rec = self.recommend(results, reads, cfg)
            self.set(head_results=results, recommend=rec, current=None)
            self.log(f"ciśnienie {head} m: {'wystarcza' if ok else 'za mało'}")
            self.write({"head_step": results[-1], "recommend": rec})

    @staticmethod
    def reaches(r, f):
        if not f:
            return False
        return f["kind"] == "over" or (r.get("design") is not None and f["flow"] >= r["design"])

    def recommend(self, results, reads, cfg):
        margin = float(cfg.get("margin_pct", 10)) / 100
        good = sorted(x["head"] for x in results if x["ok"])
        bad = sorted(x["head"] for x in results if not x["ok"])
        if not good:
            return (f"Żadne sprawdzone ciśnienie nie daje przepływu projektowego "
                    f"(najwyższe: {pl(max(bad))} m). Sprawdź wyższe.")
        low = good[0]
        # turbulent flow: the head grows with the square of the flow
        with_margin = round(low * (1 + margin) ** 2, 1)
        txt = (f"Najniższe ciśnienie z przepływem projektowym w czytanych pętlach: {pl(low)} m. "
               f"Z zapasem {round(margin * 100)} % przepływu: około {pl(with_margin)} m.")
        if not any(b < low for b in bad):
            txt += " Niżej jeszcze nie sprawdzone: spróbuj mniejszego ciśnienia, żeby znaleźć granicę."
        res = self.residual(results, reads, low)
        if res:
            txt += " " + res
        return txt

    def residual(self, results, reads, head):
        """Head lost outside the loop (pipes buffer-manifold, manifold) at the given head."""
        if not self.loops_cfg:
            return ""
        lengths = {(lp["ctrl"], lp["idx"]): lp["length_m"] for lp in self.loops_cfg["loops"]}
        step = next(x for x in results if x["head"] == head)
        out = []
        for r in reads:
            f = step["flows"].get(f"{r['ctrl']} {r['idx']}")
            L = lengths.get((r["ctrl"], r["idx"]))
            if f and f["kind"] == "value" and L:
                dp = loop_dp(f["flow"], L, self.loops_cfg.get("pipe_id_mm", 12),
                             self.loops_cfg.get("water_c", 35))["total"]
                out.append(f"{r['name']}: pętla {pl(dp)} m z {pl(head)} m, "
                           f"reszta {pl(head - dp)} m")
        return ("Straty poza pętlą (rury od bufora, belki rozdzielacza), z obliczenia: "
                + "; ".join(out) + ".") if out else ""

    def csv_text(self):
        out = io.StringIO()
        w = csv.writer(out, delimiter=";")
        w.writerow(["ctrl", "valve", "name", "sweep", "point", "pos_counted", "pos_requested",
                    "step", "step_counted", "stop", "kind", "flow_lpm", "tap_frac", "waited_s",
                    "t_1", "t_2", "time", "direction", "pos_est", "power_w"])
        for r in self.rows:
            ts = r.get("temps") or []
            t = [x.get("temp") for x in ts] + [None, None]
            w.writerow([r["ctrl"], r["idx"], r["name"], r["sweep"], r["point"], r["pos"],
                        r["pos_req"], r["step"], r["cnt"], r["stop"], r["kind"],
                        str(r["flow"]).replace(".", ","), r.get("tap"), r["waited_s"],
                        t[0], t[1], datetime.fromtimestamp(r["t"]).isoformat(timespec="seconds"),
                        r.get("dir", "open"), r.get("pos_est"), r.get("power")])
        if self.head_rows:
            w.writerow([])
            w.writerow(["ctrl", "valve", "name", "head_m", "power_w", "target_pct", "kind",
                        "flow_lpm", "design_lpm", "tap_frac", "t_1", "t_2", "time"])
            for r in self.head_rows:
                t = [x.get("temp") for x in r.get("temps") or []] + [None, None]
                w.writerow([r["ctrl"], r["idx"], r["name"], str(r["head"]).replace(".", ","),
                            r.get("power"), r["target"], r["kind"],
                            str(r["flow"]).replace(".", ","),
                            "" if r.get("design") is None else str(r["design"]).replace(".", ","),
                            r.get("tap"), t[0], t[1],
                            datetime.fromtimestamp(r["t"]).isoformat(timespec="seconds")])
        return out.getvalue()


# ───────────────────────────── simulator ─────────────────────────────

SIM_HOLD_S = 330  # the simulated STM keeps a service position this long


class SimValve:
    def __init__(self, idx, name, rnd):
        self.idx, self.name = idx, name
        self.oc = rnd.randint(4000, 5100)
        self.cc = self.oc + rnd.randint(20, 280)
        self.onset = rnd.randint(280, 520)  # pulses where flow starts
        self.k = rnd.randint(450, 800)
        self.qmax = rnd.uniform(2.9, 3.6)
        self.true = 0.0  # true spindle position [pulses from the closed end stop]
        self.hyst = rnd.randint(20, 60)  # the pin lags this far behind a closing spindle
        self.closing = False
        self.at_stop = True  # pressed against the closed end stop
        self.pos, self.target = 0, 0
        self.state, self.seq = "idle", 0
        self.last = {"dir": "close", "req": 0, "cnt": 0, "stop": "endstop", "peak": 20.0, "ms": 0}
        self.hold_until = 0
        self.flags = []

    def flow(self, head=5.7):
        x = self.true - self.onset + (self.hyst if self.closing else 0)
        q = 0.0 if x <= 0 else self.qmax * (1 - math.exp(-x / self.k))
        return q * math.sqrt(head / 5.7)  # the simulated curves are made at 5.7 m


class SimDevice:
    """A fake ESP with the routes flowbench uses and a simple valve model."""

    def __init__(self, name, names, seed, speed):
        self.name, self.speed = name, speed
        self.head = 5.7  # pump head the simulated loops see [m]
        self.rnd = random.Random(seed)
        self.valves = [SimValve(i + 1, n, self.rnd) for i, n in enumerate(names)]
        self.events, self.seq = [], 0
        self.lock = threading.Lock()
        threading.Thread(target=self.tick, daemon=True).start()

    def event(self, code, valve, a1=0, a2=0, msg=""):
        self.seq += 1
        self.events.append({"seq": self.seq, "code": code, "valve": valve, "a1": a1, "a2": a2,
                            "msg": msg})
        self.events = self.events[-200:]

    def tick(self):
        while True:  # an expired service hold returns the valve to its target
            time.sleep(0.5)
            with self.lock:
                for v in self.valves:
                    if v.hold_until and time.time() > v.hold_until and v.state == "idle":
                        v.hold_until, v.flags = 0, []
                        self.move_target(v)

    def run_move(self, v, direction, counts, ma, service):
        rnd = self.rnd
        dur = counts * 0.004 / self.speed
        v.state = "opening" if direction == "open" else "closing"

        def done():
            time.sleep(dur)
            with self.lock:
                v.closing = direction == "close"
                if direction == "open":
                    windup = rnd.randint(44, 98) if v.at_stop else 0
                    coast = rnd.randint(2, 14)
                    v.true = min(v.true + counts - windup + coast, v.oc)
                    v.at_stop, cnt, stop = False, counts, "target"
                else:
                    coast = rnd.randint(50, 80) if v.true > v.onset - 100 else rnd.randint(5, 20)
                    if counts >= v.true:  # reaches the closed end stop
                        cnt = int(v.true) + rnd.randint(44, 98)
                        v.true, v.at_stop, stop = 0.0, True, "endstop"
                    else:
                        v.true, cnt, stop = max(0.0, v.true - counts - coast), counts, "target"
                v.seq += 1
                v.state = "idle"
                v.last = {"dir": direction, "req": counts, "cnt": cnt, "stop": stop,
                          "peak": 15.0, "ms": int(dur * 1000)}
                # the STM believes a target move reached its target; a service move follows its count
                v.pos = max(0, min(100, round(v.true * 100 / v.oc))) if service else v.target
                if service:
                    v.hold_until, v.flags = time.time() + SIM_HOLD_S / self.speed, ["svcHold"]
                    self.event(414, v.idx, cnt, STOPS.index(stop), "service move done")

        threading.Thread(target=done, daemon=True).start()

    def move_target(self, v):
        want = v.target * v.oc / 100
        if v.target == 0 and not v.at_stop:
            self.run_move(v, "close", int(v.true) + 300, 34, False)
        elif abs(want - v.true) > v.oc / 200:
            d = "open" if want > v.true else "close"
            self.run_move(v, d, int(abs(want - v.true)), 30, False)
        else:
            v.pos = v.target

    def handle(self, method, path, body):
        u = urllib.parse.urlparse(path)
        q = dict(urllib.parse.parse_qsl(u.query))
        p = u.path
        with self.lock:
            if method == "GET" and p == "/api/valves":
                return {"valves": [self.valve_doc(v) for v in self.valves]}
            if method == "GET" and p == "/api/status":
                return {"station": self.name, "esp": {"version": "sim"},
                        "stm": {"version": "sim", "status": {"lease": "running"},
                                "lease": {"state": "running", "regulator": "alive"}}}
            if method == "GET" and p == "/api/events":
                since = int(q.get("since", 0))
                ev = [e for e in self.events if e["seq"] > since
                      and ("valve" not in q or e["valve"] == int(q["valve"]))]
                return {"next": self.seq, "events": ev[: int(q.get("limit", 50))]}
            if method == "POST" and p == "/api/valves/stop":
                return {"result": "queued"}
            parts = p.split("/")
            if method == "POST" and len(parts) == 5 and parts[2] == "valves":
                v = self.valves[int(parts[3]) - 1]
                if parts[4] == "stop":
                    if v.state == "idle" and v.hold_until:
                        v.hold_until = time.time() + SIM_HOLD_S / self.speed
                    return {"result": "queued"}
                if parts[4] == "target":
                    v.target, v.hold_until, v.flags = int(body["target"]), 0, []
                    if v.state == "idle":
                        self.move_target(v)
                    return {"valve": v.idx, "target": v.target}
                if parts[4] == "service-move":
                    if v.state != "idle":
                        self.event(411, v.idx, 1, 0, "command rejected")
                        return {"result": "queued"}
                    self.run_move(v, body["dir"], int(body["counts"]), int(body["maxmA"]), True)
                    return {"result": "queued"}
        raise KeyError(p)

    def valve_doc(self, v):
        return {"idx": v.idx, "name": v.name, "active": True, "stateKey": v.state,
                "calibrating": False, "pos": v.pos, "target": v.target, "oc": v.oc, "cc": v.cc,
                "sensors": [{"name": f"Z{v.idx}", "temp": 32.0}, {"name": f"P{v.idx}",
                                                                  "temp": 27.5}],
                "ext": {"moveSeq": v.seq, "lastMove": v.last, "v3": {"flags": v.flags}}}


def serve_sim(dev, port):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def go(self, method):
            n = int(self.headers.get("Content-Length") or 0)
            body = json.loads(self.rfile.read(n) or b"{}") if n else None
            try:
                doc, code = dev.handle(method, self.path, body), 200
            except KeyError:
                doc, code = {"error": "not_found"}, 404
            raw = json.dumps(doc).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def do_GET(self):
            self.go("GET")

        def do_POST(self):
            self.go("POST")

    srv = ThreadingHTTPServer(("127.0.0.1", port), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{port}"


def sim_reader(engine, devices):
    """Reads the simulated float like a careful person: true flow, +-0.04 l/min, top at the max."""
    rnd = random.Random(7)
    seen = None
    while True:
        time.sleep(0.2)
        s = engine.view()
        if s["phase"] == "decide":
            continue
        if s["phase"] == "head":
            done = [x["head"] for x in s.get("head_results", [])]
            nxt = round((done[-1] if done else 6.0) - 0.5, 1)
            fails = [x for x in s.get("head_results", []) if not x["ok"]]
            engine.post({"cmd": "finish"} if fails or nxt < 1.5 else
                        {"cmd": "head", "head": nxt, "power": 40})
            time.sleep(0.5)
            continue
        cur = s.get("current")
        if s["phase"] != "reading" or not cur:
            continue
        key = (cur["ctrl"], cur["idx"], cur["sweep"], cur["point"])
        if key == seen:
            continue
        seen = key
        v = devices[cur["ctrl"]].valves[cur["idx"] - 1]
        q, top = v.flow(devices[cur["ctrl"]].head), s["scale"]["max"]
        if q > top:
            engine.post({"kind": "over"})
        elif q < 0.05:
            engine.post({"kind": "zero"})
        else:
            engine.post({"kind": "value", "value": round(max(0.0, q + rnd.uniform(-0.04, 0.04)), 2),
                         "frac": round(q / top, 3)})


# ───────────────────────────── web server for the phone ─────────────────────────────

def make_handler(engine, page):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def send(self, code, body, ctype="application/json; charset=utf-8", extra=None):
            raw = body if isinstance(body, bytes) else body.encode()
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(raw)))
            self.send_header("Cache-Control", "no-store")
            for k, v in (extra or {}).items():
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(raw)

        def js(self, doc, code=200):
            self.send(code, json.dumps(doc, ensure_ascii=False))

        def do_GET(self):
            p = urllib.parse.urlparse(self.path).path
            if p == "/":
                return self.send(200, page, "text/html; charset=utf-8")
            if p == "/api/state":
                return self.js(engine.view())
            if p == "/api/setup":
                ctrls = []
                for esp in engine.esps.values():
                    try:
                        vs = [{"idx": v["idx"], "name": v.get("name"), "pos": v.get("pos"),
                               "target": v.get("target"), "state": v.get("stateKey"),
                               "flags": v.get("ext", {}).get("v3", {}).get("flags", []),
                               "oc": v.get("oc")} for v in esp.valves()]
                        ctrls.append({"name": esp.name, "base": esp.base, "valves": vs})
                    except EngineError as e:
                        ctrls.append({"name": esp.name, "base": esp.base, "error": str(e)})
                return self.js({"controllers": ctrls})
            if p == "/api/design":
                q = dict(urllib.parse.parse_qsl(urllib.parse.urlparse(self.path).query))
                return self.js(design_table(engine.loops_cfg, float(q.get("dt", 5))))
            if p == "/export.csv":
                return self.send(200, engine.csv_text(), "text/csv; charset=utf-8",
                                 {"Content-Disposition": "attachment; filename=flowbench.csv"})
            self.send(404, "{}")

        def do_POST(self):
            p = urllib.parse.urlparse(self.path).path
            n = int(self.headers.get("Content-Length") or 0)
            try:
                body = json.loads(self.rfile.read(n) or b"{}")
            except json.JSONDecodeError:
                return self.js({"error": "bad json"}, 400)
            try:
                if p == "/api/start":
                    engine.start(body)
                elif p == "/api/input":
                    engine.post(body)
                elif p == "/api/restore":
                    engine.restore()
                else:
                    return self.js({"error": "not found"}, 404)
            except EngineError as e:
                return self.js({"error": str(e)}, 409)
            self.js({"ok": True})

    return H


def lan_ip():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("10.255.255.255", 1))
        return s.getsockname()[0]
    except OSError:
        return "127.0.0.1"
    finally:
        s.close()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--controller", action="append", default=[], metavar="NAME=HOST",
                    help="a VdMot controller, e.g. west=192.168.1.51 (repeat for each)")
    ap.add_argument("--port", type=int, default=4680)
    ap.add_argument("--bind", default="0.0.0.0", help="address of the page (default: all)")
    ap.add_argument("--out", type=Path, default=None,
                    help="folder of flowbench.db (default ~/flowbench, with --sim ~/flowbench/sim)")
    ap.add_argument("--sim", action="store_true", help="two simulated controllers, no hardware")
    ap.add_argument("--sim-auto", action="store_true", help="with --sim: a simulated reader")
    ap.add_argument("--sim-speed", type=float, default=1.0, help="with --sim: time factor")
    ap.add_argument("--loops", type=Path, default=None,
                    help="length and heat of every loop for the pump head calculator "
                         "(default ~/flowbench/loops.json, with --sim loops.example.json)")
    ap.add_argument("--ha-power", metavar="ENTITY",
                    help="a Home Assistant power sensor of the pump, stored with every reading; "
                         "needs HA_URL and HA_TOKEN in the environment")
    a = ap.parse_args()

    esps, devices = [], {}
    if a.sim:
        for i, (name, n) in enumerate((("sim-west", 6), ("sim-east", 7))):
            dev = SimDevice(name, [f"Pętla {j + 1}" for j in range(n)], seed=i + 1,
                            speed=a.sim_speed)
            devices[name] = dev
            esps.append(Esp(name, serve_sim(dev, 18081 + i)))
    for c in a.controller:
        name, _, host = c.partition("=")
        if not host:
            ap.error(f"--controller {c}: use NAME=HOST")
        esps.append(Esp(name, host if host.startswith("http") else f"http://{host}"))
    if not esps:
        ap.error("give --controller NAME=HOST (or --sim)")

    out = a.out or Path.home() / "flowbench" / ("sim" if a.sim else "")
    engine = Engine(esps, out)
    out.mkdir(parents=True, exist_ok=True)
    engine.store = Store(out / "flowbench.db")  # a schema error stops the tool here, not mid-session
    default_loops = HERE / "loops.example.json" if a.sim else Path.home() / "flowbench" / "loops.json"
    engine.loops_cfg = load_loops(a.loops or default_loops)
    if a.ha_power:
        url, token = os.environ.get("HA_URL"), os.environ.get("HA_TOKEN")
        if not (url and token):
            ap.error("--ha-power needs HA_URL and HA_TOKEN in the environment")
        engine.power = HaPower(url, token, a.ha_power)
        print(f"pump power: {a.ha_power} = {engine.power.read()} W")
    if a.sim:
        engine.on_head = lambda h: [setattr(d, "head", h) for d in devices.values()]
    if a.sim and a.sim_speed != 1:
        global HOLD_S, KEEP_S
        HOLD_S = HOLD_S / a.sim_speed
        KEEP_S = KEEP_S / a.sim_speed
    page = (HERE / "flowbench.html").read_text(encoding="utf-8")
    srv = ThreadingHTTPServer((a.bind, a.port), make_handler(engine, page))
    print(f"flowbench: http://{lan_ip()}:{a.port}  (on this PC: http://localhost:{a.port})")
    print(f"sessions: {out / 'flowbench.db'}")
    if a.sim_auto:
        engine.decide_s = 1
        threading.Thread(target=sim_reader, args=(engine, devices), daemon=True).start()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
