#!/usr/bin/env python3
# /// script
# requires-python = ">=3.10"
# ///
"""flowcheck: checks the flow curves of the loops on the VdMot flow layer (firmware 2.2.8).

The controller positions each valve itself (POST /api/valves/{n}/flow/counts: a move to the
reference end stop, one move away from it, the hold kept). The reader answers per valve on the
phone page. The other loops stand at 15 %, as they did while the curves were measured. At the end
of a session, also after STOP or an error, every valve goes back to the target it had.

Sessions:
  repeat  the same position 5 times per loop: how repeatable one positioning is
  house   every loop on the same flow at once (default 1.0 l/min): how the loops pull on each other
  check   3-4 loops of one manifold, a list of stops; per valve OK / too much / too little / a typed
          value; corrections in pulses until every card is OK, then the group is confirmed once

After a check session the tool fits per loop: counts = offset + gain x (counts of the curve), and
writes it into the controller on request (POST /api/valves/{n}/flow).

  python tools/flowbench/flowcheck.py --controller west=192.168.1.51 --controller east=192.168.1.52
  python tools/flowbench/flowcheck.py --sim [--sim-auto]
  python tools/flowbench/flowcheck.py --controller ... upload curves.json   (points, layer off)
"""
import argparse
import json
import os
import random
import sqlite3
import sys
import threading
import time
import traceback
import urllib.parse
from collections import Counter
from http.server import BaseHTTPRequestHandler
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from flowbench import EngineError, Esp, HaPower, Server, lan_ip  # noqa: E402

HERE = Path(__file__).resolve().parent
OK_LPM = 0.1  # the reader answers OK within this
STEP_LPM = 0.1  # a "too much" / "too little" asks for this change
STEP_MIN, STEP_MAX = 6, 120  # pulses of one correction
STEP_GROW = 1.5  # the same answer twice in a row: a larger step
POSITION_S = 60  # one positioning of one valve; a controller moves one valve at a time
TARGET_S = 30  # one target move of one valve
SETTLE_S = 15  # the float after a move; the page counts it down, the answers stay open
HOLD_CHECK_S = 5  # how often a card that waits for a reading checks that its valve still holds
OTHERS_PCT = 15  # the other loops while the curves were measured
PUMP_MIN_W = 5  # below this the circulation pump stands
SD_OK = 0.1  # repeat: the standard deviation that passes [l/min]
# stops of a check session [l/min]; mixed: every stop twice and never twice in a row
STOPS = {
    "short": [1.0, 2.0, 0.5],
    "mixed": [2.0, 0.25, 1.5, 0.5, 1.0, 2.0, 0.5, 1.5, 0.25, 1.0],
    "up": [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.7],
    "down": [2.7, 2.5, 2.25, 2.0, 1.75, 1.5, 1.25, 1.0, 0.75, 0.5, 0.25],
    "zigzag": [2.7, 0.25, 2.5, 0.5, 2.25, 0.75, 2.0, 1.0, 1.75, 1.25, 1.5],
}
STOP_LABELS = {"short": "krótka: 1 · 2 · 0,5 l/min", "mixed": "mieszana: 10 przystanków",
               "up": "w górę: 11 przystanków", "down": "w dół: 11 przystanków",
               "zigzag": "zygzak: 11 przystanków"}


# ───────────────────────────── curves ─────────────────────────────

def curve_counts(points, q):
    """Counts where the piecewise-linear curve first reaches q; the last counts above its top."""
    if q <= points[0][1]:
        return float(points[0][0])
    for (c0, q0), (c1, q1) in zip(points, points[1:]):
        if q0 < q <= q1:
            return c0 + (q - q0) * (c1 - c0) / (q1 - q0)
    return float(points[-1][0])


def curve_flow(points, c):
    if c <= points[0][0]:
        return points[0][1]
    for (c0, q0), (c1, q1) in zip(points, points[1:]):
        if c0 <= c <= c1:
            return q0 + (c - c0) * (q1 - q0) / (c1 - c0)
    return points[-1][1]


def slope_at(points, q):
    """l/min per pulse around q (at least over 40 pulses)."""
    c = curve_counts(points, q)
    a, b = curve_flow(points, c - 20), curve_flow(points, c + 20)
    return max((b - a) / 40, 1e-4)


def fit(pairs):
    """y = offset + gain x by least squares; offset only below 3 points or a narrow x range."""
    if not pairs:
        return None
    xs, ys = [p[0] for p in pairs], [p[1] for p in pairs]
    n = len(pairs)
    mx, my = sum(xs) / n, sum(ys) / n
    sxx = sum((x - mx) ** 2 for x in xs)
    if n < 3 or sxx < 100 ** 2:
        return {"offset": round(my - mx), "gain": 1.0, "n": n}
    gain = sum((x - mx) * (y - my) for x, y in pairs) / sxx
    gain = min(1.5, max(0.67, gain))
    return {"offset": round(my - gain * mx), "gain": round(gain, 3), "n": n}


def spread(values):
    """Mean and sample standard deviation."""
    n = len(values)
    mean = sum(values) / n
    return mean, (sum((v - mean) ** 2 for v in values) / (n - 1)) ** 0.5 if n > 1 else 0.0


class Loop:
    def __init__(self, esp, idx, name, cfg):
        self.esp, self.idx, self.name = esp, idx, name
        self.key = f"{esp.name} {idx}"
        self.cfg = cfg

    @property
    def points(self):
        return self.cfg["points"]

    def counts_for(self, q):
        if q <= 0:
            return 0
        c = curve_counts(self.points, q)
        return max(1, min(10000, round(self.cfg.get("offset", 0) + self.cfg.get("gain", 1.0) * c)))

    def curve_x(self, counts):
        """The counts of the curve without the correction."""
        return (counts - self.cfg.get("offset", 0)) / (self.cfg.get("gain", 1.0) or 1.0)


def flow_cfg(esp, idx):
    d = esp.get(f"/api/valves/{idx}/flow")
    return d.get("config", d)


# ───────────────────────────── engine ─────────────────────────────

class Stop(Exception):
    pass


class Engine:
    def __init__(self, esps, out, power=None, pick=()):
        self.esps = {e.name: e for e in esps}
        self.out = out
        self.power = power
        self.pick = list(pick)
        self.lock = threading.Lock()
        self.cv = threading.Condition(self.lock)
        self.db = sqlite3.connect(out / "flowcheck.db", check_same_thread=False)
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS sessions (id INTEGER PRIMARY KEY, started TEXT, kind TEXT,
                cfg TEXT, status TEXT);
            CREATE TABLE IF NOT EXISTS readings (id INTEGER PRIMARY KEY, session_id INTEGER,
                t TEXT, ctrl TEXT, valve INTEGER, stop_no INTEGER, round INTEGER, q_target REAL,
                counts INTEGER, answer TEXT, value REAL, others TEXT, power_w REAL);
            CREATE TABLE IF NOT EXISTS fits (id INTEGER PRIMARY KEY, session_id INTEGER, t TEXT,
                ctrl TEXT, valve INTEGER, offset INTEGER, gain REAL, n INTEGER, applied INTEGER);
        """)
        self.state = {"phase": "idle", "msg": "", "cards": [], "log": [], "confirm": False,
                      "stop": None, "fits": [], "summary": [], "kind": None, "error": None,
                      "settle_until": 0, "pump": None}
        self.answers = {}
        self.confirmed = False
        self.stop_req = False
        self.thread = None
        self.sid = None
        self.restore = []  # (loop, cfg) of the loops whose layer the session switched on
        self.before = {}  # (controller, valve): target of those loops before the session
        self.raised = {}  # the same of the other loops the session opened
        self.refs = {}  # loop key: refDelta of its last positioning
        if power:
            threading.Thread(target=self.watch_pump, daemon=True).start()

    def watch_pump(self):
        while True:
            self.set(pump=self.power.read())
            time.sleep(20)

    # state for the page
    def set(self, **kw):
        with self.lock:
            self.state.update(kw)
            self.cv.notify_all()

    def log(self, text):
        line = time.strftime("%H:%M:%S ") + text
        print(line, flush=True)
        with self.lock:
            self.state["log"] = (self.state["log"] + [line])[-60:]

    def view(self):
        with self.lock:
            s = json.loads(json.dumps({**self.state, "answered": self.answers}))
        s["settle_s"] = max(0, round(s.pop("settle_until") - time.time()))
        return s

    def presets(self):
        """The loops of every controller and the session buttons of the page."""
        groups = []
        for esp in self.esps.values():
            idxs = [v["idx"] for v in esp.valves()]
            half = (len(idxs) + 1) // 2 if len(idxs) > 4 else len(idxs)
            for part in (idxs[:half], idxs[half:]):
                if part:
                    groups.append({"label": f"{esp.name} {part[0]}–{part[-1]}",
                                   "loops": [f"{esp.name} {i}" for i in part]})
        return {"groups": groups, "pick": self.pick, "others": OTHERS_PCT,
                "stops": [{"id": k, "label": STOP_LABELS[k]} for k in STOPS]}

    def check(self):
        if self.stop_req:
            raise Stop()

    # controller calls
    def loops(self, keys):
        out = []
        for key in keys:
            name, _, idx = key.rpartition(" ")
            esp = self.esps[name]
            v = esp.valve(int(idx))
            cfg = flow_cfg(esp, int(idx))
            if not cfg.get("points"):
                raise EngineError(f"{key}: no flow curve in the controller (run upload)")
            out.append(Loop(esp, int(idx), v.get("name") or key, cfg))
        return out

    def docs(self, esps):
        """{(controller, valve): valve document}, one read per controller. A failed read only
        leaves its valves out: the wait around it has its own limit."""
        out = {}
        for esp in esps:
            try:
                out.update({(esp.name, v["idx"]): v for v in esp.valves()})
            except EngineError as e:
                self.log(f"odczyt sterownika: {e}")
        return out

    @staticmethod
    def limit(keys, each_s):
        """Seconds for a batch: the moves of one controller run one after the other."""
        return 30 + each_s * max(Counter(name for name, _ in keys).values())

    def targets(self, wanted, stoppable=False, passing=(), say=None):
        """wanted: {(controller, valve): pct}. Sets the targets, waits until the valves stand
        there and returns the ones that did not get there. A valve in `passing` goes on to
        another target, so the STM taking this one is enough: near the closed end a valve can
        stop short of 5 % and would hold the whole batch up to its limit. `say` is the message
        for the page."""
        left, t0, posted = dict(wanted), time.time(), 0.0
        esps = {self.esps[name] for name, _ in wanted}
        limit = self.limit(wanted, TARGET_S) if wanted else 0
        while left and time.time() - t0 < limit:
            if stoppable:
                self.check()
            if time.time() - posted > 30:
                posted = time.time()
                for (name, idx), pct in left.items():
                    try:
                        self.esps[name].post(f"/api/valves/{idx}/target", {"target": pct})
                    except EngineError as e:
                        self.log(f"cel zaworu: {e}")
            time.sleep(1)
            docs = self.docs(esps)
            for k, pct in list(left.items()):
                v = docs.get(k) or {}
                if k in passing and v.get("stmTarget") == pct:
                    del left[k]
                elif v.get("stateKey") == "idle" and v.get("target") == pct \
                        and abs((v.get("pos") or 0) - pct) <= 2:
                    del left[k]
            if say:
                self.set(msg=f"{say}: zostało {len(left)}")
        return left

    def keep_targets(self, loops):
        docs = self.docs({lp.esp for lp in loops})
        for lp in loops:
            k = (lp.esp.name, lp.idx)
            if k not in docs:
                raise EngineError(f"{lp.key}: nie odczytałem celu zaworu")
            if not lp.cfg.get("enabled"):
                self.before[k] = docs[k].get("target") or 0

    def open_others(self, loops, pct):
        """The pump pressure of the curves holds only with the other loops open as they were
        while the curves were measured. The old targets go down first, so that a session that
        stops here still brings them back."""
        mine = {(lp.esp.name, lp.idx) for lp in loops}
        for esp in self.esps.values():
            for v in esp.valves():
                k = (esp.name, v["idx"])
                if k not in mine and (v.get("target") or 0) < pct \
                        and not (v.get("flow") or {}).get("enabled"):
                    self.raised[k] = v.get("target") or 0
        if self.raised:
            self.log(f"pozostałe pętle na {pct} %: {len(self.raised)}")
            self.set(msg=f"Pozostałe pętle na {pct} %")
            left = self.targets({k: pct for k in self.raised}, stoppable=True)
            if left:
                raise EngineError(f"pozostałe pętle nie stoją na {pct} %: "
                                  + ", ".join(f"{n} {i}" for n, i in left))

    def enable(self, loops):
        """The layer on for the loops that have it off. Its first positioning serves the target
        of the valve, and /flow/counts during that one would queue behind it, so the wait ends
        at the hold."""
        left = {}
        for lp in loops:
            if not lp.cfg.get("enabled"):
                self.restore.append((lp, dict(lp.cfg)))
                lp.esp.post(f"/api/valves/{lp.idx}/flow", {**lp.cfg, "enabled": True})
                lp.cfg = flow_cfg(lp.esp, lp.idx)
                left[(lp.esp.name, lp.idx)] = lp
        t0, limit = time.time(), self.limit(left, POSITION_S) if left else 0
        esps = {lp.esp for lp in left.values()}
        while left:
            self.check()
            time.sleep(1)
            docs = self.docs(esps)
            for k, lp in list(left.items()):
                v = docs.get(k) or {}
                f = v.get("flow") or {}
                if f.get("state") in ("failed", "unsupported"):
                    raise EngineError(f"{lp.key}: warstwa przepływu: {f['state']}")
                if f.get("state") == "held" and v.get("stateKey") == "idle":
                    del left[k]
                    self.log(f"{lp.key}: warstwa przepływu włączona na sesję")
            self.set(msg="Włączam warstwę przepływu: "
                     + ", ".join(lp.key for lp in left.values()))
            if time.time() - t0 > limit:
                raise EngineError("warstwa przepływu nie ruszyła: "
                                  + ", ".join(lp.key for lp in left.values()))

    def disable_restored(self):
        waits = []
        for lp, cfg in self.restore:
            try:
                lp.esp.post(f"/api/valves/{lp.idx}/flow", {**flow_cfg(lp.esp, lp.idx),
                                                           "enabled": cfg.get("enabled", False)})
                waits.append((lp.esp.name, lp.idx))
                self.log(f"{lp.key}: warstwa przepływu jak przed sesją")
            except EngineError as e:
                self.log(f"{lp.key}: nie przywróciłem warstwy: {e}")
        self.restore = []
        # the ESP lets go of the keep on its next round, one valve at a time
        esps, t0 = {self.esps[name] for name, _ in waits}, time.time()
        while waits and time.time() - t0 < 30:
            time.sleep(1)
            docs = self.docs(esps)
            waits = [k for k in waits if k not in docs
                     or (docs[k].get("flow") or {}).get("state") != "off"
                     or "svcKeep" in (((docs[k].get("ext") or {}).get("v3") or {}).get("flags") or [])]

    def put_back(self):
        """The layers as before the session, then every valve on the target it had. A layer that
        lets go leaves the valve at its fallback target while the ESP still has the old target as
        desired, and the ESP sends only a changed target: a session loop goes over another target
        first. Returns the valves that did not get back."""
        self.disable_restored()
        over = {k: 0 if pct else 5 for k, pct in self.before.items()}
        say = "Przywracam zawory"
        left = {k: pct for k, pct in self.targets({**self.raised, **over}, passing=over, say=say).items()
                if k in self.raised}
        left.update(self.targets(self.before, say=say))
        self.raised, self.before = {}, {}
        return left

    def position(self, targets):
        """targets: {loop: counts}. Every valve positioned once (also an unchanged one). The hold
        before a request looks like the hold after it, so a valve is done only after a move."""
        esps = {lp.esp for lp in targets}
        now = self.docs(esps)
        seq = {lp: ((now.get((lp.esp.name, lp.idx)) or {}).get("ext") or {}).get("moveSeq")
               for lp in targets}
        with self.lock:
            self.answers = {}
        self.set(phase="moving", msg="Ustawiam zawory", confirm=False, settle_until=0)
        for lp, n in targets.items():
            self.check()
            lp.esp.post(f"/api/valves/{lp.idx}/flow/counts", {"counts": int(n)})
        t0, limit = time.time(), self.limit([(lp.esp.name, lp.idx) for lp in targets], POSITION_S)
        left = dict(targets)
        while left:
            self.check()
            time.sleep(1)
            docs = self.docs(esps)
            for lp, n in list(left.items()):
                v = docs.get((lp.esp.name, lp.idx))
                if not v:
                    continue
                f = v.get("flow") or {}
                if f.get("state") in ("failed", "unsupported", "off"):
                    raise EngineError(f"{lp.key}: pozycjonowanie: {f.get('state')}")
                if f.get("state") == "held" and f.get("targetCounts") == n \
                        and v.get("stateKey") == "idle" \
                        and (v.get("ext") or {}).get("moveSeq") != seq[lp]:
                    del left[lp]
            self.set(msg="Ustawiam: " + ", ".join(f"{lp.key} → {n} imp" for lp, n in left.items()))
            if time.time() - t0 > limit:
                raise EngineError(f"pozycjonowanie trwa ponad {limit} s: "
                                  + ", ".join(lp.key for lp in left))
        self.note_refs(targets)
        self.set(settle_until=time.time() + SETTLE_S)

    def note_refs(self, targets):
        """What the reference move counted above the way to the end stop: the only sign of a
        reading taken from a wrong position, and the event ring of the ESP keeps too few events
        to read it after a session."""
        for lp in targets:
            try:
                self.refs[lp.key] = lp.esp.get(f"/api/valves/{lp.idx}/flow").get("refDelta")
            except EngineError as e:
                self.log(f"refDelta: {e}")
        self.log("ref: " + ", ".join(f"{lp.key} {self.refs.get(lp.key)}" for lp in targets))

    def ask(self, cards, confirm=False):
        """Shows the cards and waits until each asked card has an answer (and the group confirm
        when `confirm`). A valve that loses its hold meanwhile goes to 0 % on its own: a reading
        of it would be wrong, so that ends the session."""
        with self.lock:
            self.answers = {}
            self.confirmed = False
            self.state.update(cards=cards, confirm=confirm, phase="reading",
                              msg="Odczytaj zaznaczone karty")
            self.cv.notify_all()
        checked = time.time()
        while True:
            with self.lock:
                if self.stop_req:
                    raise Stop()
                need = [c["key"] for c in cards if c["ask"]]
                if confirm and self.confirmed:
                    return dict(self.answers)
                if not confirm and all(k in self.answers for k in need):
                    return dict(self.answers)
                if confirm and self.answers:
                    # a card changed while confirming: back to the corrections
                    return dict(self.answers)
                self.cv.wait(1)
            if time.time() - checked > HOLD_CHECK_S:
                checked = time.time()
                self.check_holds(cards)

    def check_holds(self, cards):
        keys = {}
        for c in cards:
            name, _, idx = c["key"].rpartition(" ")
            keys[(name, int(idx))] = c["key"]
        docs = self.docs({self.esps[name] for name, _ in keys})
        for k, key in keys.items():
            state = ((docs.get(k) or {}).get("flow") or {}).get("state")
            if k in docs and state != "held":
                raise EngineError(f"{key}: zawór stracił pozycję ({state}), odczyt byłby zły")

    def answer(self, body):
        with self.lock:
            if body.get("confirm"):
                self.confirmed = True
            else:
                a = body.get("answer")
                if a not in ("ok", "more", "less", "value"):
                    raise EngineError("answer: ok|more|less|value")
                v = body.get("value")
                if a == "value":
                    v = float(v)
                    if v < 0:
                        raise EngineError("odczyt poniżej zera")
                self.answers[body["key"]] = {"answer": a, "value": v}
            self.cv.notify_all()

    def record(self, lp, stop_no, rnd, q, counts, ans, others):
        self.db.execute(
            "INSERT INTO readings (session_id, t, ctrl, valve, stop_no, round, q_target, counts, "
            "answer, value, others, power_w) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
            (self.sid, time.strftime("%Y-%m-%dT%H:%M:%S"), lp.esp.name, lp.idx, stop_no, rnd, q,
             counts, ans["answer"], ans.get("value"),
             json.dumps({**others, "ref": self.refs.get(lp.key)}),
             self.power.read() if self.power else None))
        self.db.commit()

    # sessions
    def start(self, cfg):
        with self.lock:
            if self.thread and self.thread.is_alive():
                raise EngineError("sesja już trwa")
            self.stop_req = False
        if self.power:
            w = self.power.read()
            if w is None:
                self.log("moc pompy nieznana, sesja rusza bez sprawdzenia")
            elif w < PUMP_MIN_W:
                raise EngineError(f"pompa obiegowa stoi ({w:.0f} W): włącz ją i zacznij jeszcze raz")
        self.thread = threading.Thread(target=self.run, args=(cfg,), daemon=True)
        self.thread.start()

    def stop(self):
        with self.lock:
            self.stop_req = True
            self.cv.notify_all()

    def run(self, cfg):
        kind = cfg.get("kind")
        cur = self.db.execute("INSERT INTO sessions (started, kind, cfg, status) VALUES (?,?,?,?)",
                              (time.strftime("%Y-%m-%dT%H:%M:%S"), kind, json.dumps(cfg), "running"))
        self.db.commit()
        self.sid = cur.lastrowid
        self.set(phase="moving", kind=kind, error=None, fits=[], summary=[], cards=[],
                 confirm=False, msg="Start", stop=None)
        status, msg, error = "done", "Sesja skończona", None
        try:
            loops = self.loops(cfg["loops"])
            self.keep_targets(loops)
            self.open_others(loops, int(cfg.get("others", OTHERS_PCT)))
            self.enable(loops)
            {"repeat": self.run_repeat, "house": self.run_house,
             "check": self.run_check}[kind](loops, cfg)
        except Stop:
            status, msg = "stopped", "Zatrzymane"
        except Exception as e:  # noqa: BLE001 - the page shows every failure
            status, msg, error = "error", f"Błąd: {e}", str(e)
            traceback.print_exc()
        self.stop_req = False
        self.set(phase="restoring", msg="Przywracam zawory", cards=[], confirm=False, stop=None)
        try:
            left = [f"{name} {idx}" for name, idx in self.put_back()]
        except Exception as e:  # noqa: BLE001 - the page shows every failure
            traceback.print_exc()
            left = [str(e)]
        if left:
            status = "error"
            error = (f"{error}; " if error else "") + "Zawory NIE wróciły: " + ", ".join(left)
            msg = error
        else:
            self.log("zawory wróciły na swoje cele")
        self.db.execute("UPDATE sessions SET status = ? WHERE id = ?", (status, self.sid))
        self.db.commit()
        self.set(phase="error" if error else status, msg=msg, error=error)

    def card(self, lp, q, counts, ask=True, expect=True):
        return {"key": lp.key, "name": lp.name, "expect": q if expect else None, "counts": counts,
                "ask": ask, "last": None}

    def run_repeat(self, loops, cfg):
        """The same counts `repeats` times; the reader types what the float shows. A valve with
        the closed reference closes first; with the open one every positioning starts at the
        open end anyway."""
        q = float(cfg.get("q", 1.0))
        repeats = int(cfg.get("repeats", 5))
        targets = {lp: int(cfg.get("counts", {}).get(lp.key) or lp.counts_for(q)) for lp in loops}
        closed = [lp for lp in loops if lp.cfg.get("refSide") != "open"]
        seen = {lp: [] for lp in loops}
        for r in range(1, repeats + 1):
            self.set(stop={"no": r, "of": repeats, "q": None})
            if closed:
                self.position({lp: 0 for lp in closed})
            self.position(targets)
            cards = [self.card(lp, q, n, expect=False) for lp, n in targets.items()]
            ans = self.ask(cards)
            for lp, n in targets.items():
                self.record(lp, r, 1, q, n, ans[lp.key], {})
                seen[lp].append(ans[lp.key]["value"])
            self.log(f"powtórka {r}: " + ", ".join(
                f"{lp.key} {ans[lp.key]['value']}" for lp in loops))
            summary = []
            for lp in loops:
                mean, sd = spread(seen[lp])
                summary.append({"key": lp.key, "name": lp.name, "counts": targets[lp],
                                "values": seen[lp], "mean": round(mean, 3), "sd": round(sd, 3),
                                "ok": sd <= SD_OK})
            self.set(summary=summary)
        self.make_fits(loops)

    def run_house(self, loops, cfg):
        q = float(cfg.get("q", 1.0))
        targets = {lp: lp.counts_for(q) for lp in loops}
        self.set(stop={"no": 1, "of": 1, "q": q})
        self.position(targets)
        cards = [self.card(lp, q, n) for lp, n in targets.items()]
        ans = self.ask(cards)
        for lp, n in targets.items():
            self.record(lp, 1, 1, q, n, ans[lp.key], {o.key: m for o, m in targets.items()})
        self.log("cały dom: " + ", ".join(
            f"{lp.key} {ans[lp.key]['value'] if ans[lp.key]['answer'] == 'value' else ans[lp.key]['answer']}"
            for lp in loops))
        self.make_fits(loops)

    def correction(self, lp, q, counts, ans, last):
        """New counts after an answer that is not OK; (counts, step factor)."""
        gain = lp.cfg.get("gain", 1.0) or 1.0
        if ans["answer"] == "value":
            v = ans["value"]
            top = lp.points[-1][1]
            if v <= 0:
                # below the onset: at least up to the first flow of the curve
                first = next((c for c, f in lp.points if f > 0), lp.points[-1][0])
                delta = max(STEP_MIN * 3, (curve_counts(lp.points, q) - first) * gain + 20)
                return counts + round(delta), 1.0
            v = min(v, top)
            delta = (curve_counts(lp.points, q) - curve_counts(lp.points, v)) * gain
            return counts + round(delta), 1.0
        mult = (last or {}).get("mult", 1.0)
        if last and last.get("answer") == ans["answer"]:
            mult *= STEP_GROW
        else:
            mult = 1.0
        step = STEP_LPM / slope_at(lp.points, q) * gain * mult
        step = round(min(STEP_MAX, max(STEP_MIN, step)))
        return counts + (step if ans["answer"] == "less" else -step), mult

    def run_check(self, loops, cfg):
        stops = cfg.get("stops") or STOPS.get(cfg.get("mode", "short"))
        for no, q in enumerate(stops, 1):
            self.set(stop={"no": no, "of": len(stops), "q": q})
            counts = {lp: lp.counts_for(q) for lp in loops}
            last = {lp.key: None for lp in loops}
            self.position(counts)
            ask = {lp.key for lp in loops}
            rnd, corrected = 1, False
            while True:
                cards = [self.card(lp, q, counts[lp], ask=lp.key in ask) for lp in loops]
                for c in cards:
                    c["last"] = last[c["key"]]
                confirm = corrected and not ask
                ans = self.ask(cards, confirm=confirm)
                if confirm and self.confirmed:
                    break
                fixes = {}
                for lp in loops:
                    a = ans.get(lp.key)
                    if not a:
                        continue
                    others = {o.key: counts[o] for o in loops if o is not lp}
                    self.record(lp, no, rnd, q, counts[lp], a, others)
                    if a["answer"] == "ok" or (a["answer"] == "value"
                                               and abs(a["value"] - q) <= OK_LPM):
                        last[lp.key] = {**a, "mult": 1.0}
                        continue
                    n, mult = self.correction(lp, q, counts[lp], a, last[lp.key])
                    last[lp.key] = {**a, "mult": mult}
                    fixes[lp] = max(0, min(10000, n))
                if not fixes and not corrected:
                    break  # every card OK at once: the stop is accepted
                if fixes:
                    corrected = True
                    self.log(f"przystanek {no} ({q:g} l/min) runda {rnd}: korekta " + ", ".join(
                        f"{lp.key} {counts[lp]}→{n}" for lp, n in fixes.items()))
                    counts.update(fixes)
                    self.position(fixes)
                ask = {lp.key for lp in fixes}
                rnd += 1
        self.make_fits(loops)

    def make_fits(self, loops):
        """offset + gain per loop from the OK and typed readings of this session."""
        fits = []
        for lp in loops:
            rows = self.db.execute(
                "SELECT q_target, counts, answer, value FROM readings WHERE session_id = ? AND "
                "ctrl = ? AND valve = ?", (self.sid, lp.esp.name, lp.idx)).fetchall()
            top = lp.points[-1][1]
            pairs = []
            for q, counts, answer, value in rows:
                meas = q if answer == "ok" else value if answer == "value" else None
                if meas is None or meas <= 0 or meas >= top:
                    continue
                # the counts the uncorrected curve gives for the flow seen, and the counts used
                pairs.append((curve_counts(lp.points, meas), counts))
            f = fit(pairs)
            if f:
                fits.append({"key": lp.key, "name": lp.name, **f,
                             "was": {"offset": lp.cfg.get("offset", 0),
                                     "gain": lp.cfg.get("gain", 1.0)}})
                self.db.execute("INSERT INTO fits (session_id, t, ctrl, valve, offset, gain, n, "
                                "applied) VALUES (?,?,?,?,?,?,?,0)",
                                (self.sid, time.strftime("%Y-%m-%dT%H:%M:%S"), lp.esp.name,
                                 lp.idx, f["offset"], f["gain"], f["n"]))
        self.db.commit()
        self.set(fits=fits)
        for f in fits:
            self.log(f"{f['key']}: offset {f['offset']}, gain {f['gain']} (n {f['n']})")

    def apply_fit(self, key):
        f = next((x for x in self.view()["fits"] if x["key"] == key), None)
        if not f:
            raise EngineError(f"{key}: brak dopasowania")
        name, _, idx = key.rpartition(" ")
        esp = self.esps[name]
        cfg = flow_cfg(esp, int(idx))
        esp.post(f"/api/valves/{idx}/flow", {**cfg, "offset": f["offset"], "gain": f["gain"]})
        self.db.execute("UPDATE fits SET applied = 1 WHERE session_id = ? AND ctrl = ? AND "
                        "valve = ?", (self.sid, name, int(idx)))
        self.db.commit()
        self.log(f"{key}: zapisane w sterowniku offset {f['offset']}, gain {f['gain']}")
        self.set(fits=[{**x, "applied": True} if x["key"] == key else x
                       for x in self.view()["fits"]])


# ───────────────────────────── simulator ─────────────────────────────

class SimEsp:
    """A controller with the flow layer: each valve has a true offset and gain against the curve
    of curves.example.json, plus a scatter of the positioning. As on the real one, the layer
    positions a valve as soon as it is switched on, a valve it lets go drives to the fallback
    target, and only a changed target reaches the STM."""

    def __init__(self, name, n, seed):
        self.name = name
        self.base = f"sim://{name}"
        self.rnd = random.Random(seed)
        pts = [[0, 0.0], [600, 0.0], [700, 0.25], [760, 0.6], [830, 1.0], [900, 1.5], [980, 2.0],
               [1070, 2.5], [1110, 2.7]]
        self.lock = threading.Lock()
        self.v = {}
        for i in range(1, n + 1):
            shift = self.rnd.randint(-60, 60)
            self.v[i] = {"name": f"Loop {i}", "cfg": {
                "enabled": False, "qmax": 2.7, "refmA": 34, "openmA": 30, "fallbackPct": 15,
                "minIntervalS": 300, "deadbandLpm": 0.05, "offset": 0, "gain": 1.0,
                "refSide": "open", "strokeCounts": 4700,
                "points": [[c + shift if c else 0, q] for c, q in pts]},
                "true_off": self.rnd.randint(-50, 50), "true_gain": self.rnd.uniform(0.9, 1.1),
                "counts": 0, "target": 0, "busy_until": 0.0, "noise": 0, "seq": 0,
                "desired": 0, "pct": 0}

    def flow_true(self, i):
        v = self.v[i]
        x = (v["counts"] + v["noise"] - v["true_off"]) / v["true_gain"]
        return curve_flow(v["cfg"]["points"], x)

    def get(self, path):
        with self.lock:
            if path == "/api/valves":
                return {"valves": [self.doc(i) for i in self.v]}
            if path.endswith("/flow"):
                return {"config": dict(self.v[int(path.split("/")[3])]["cfg"])}
        raise EngineError(f"sim: GET {path}")

    def doc(self, i):
        v = self.v[i]
        busy = time.time() < v["busy_until"]
        on = v["cfg"]["enabled"]
        return {"idx": i, "name": v["name"], "active": True, "stateKey": "opening" if busy else "idle",
                "pos": v["pct"], "target": v["desired"], "stmTarget": v["pct"], "ext": {"moveSeq": v["seq"]},
                "flow": {"enabled": on, "state": ("positioning" if busy else "held") if on else "off",
                         "counts": v["counts"], "targetCounts": v["target"],
                         "lpm": round(curve_flow(v["cfg"]["points"], v["counts"]), 3)}}

    def post(self, path, body=None):
        with self.lock:
            i = int(path.split("/")[3])
            v = self.v[i]
            if path.endswith("/flow/counts"):
                if not v["cfg"]["enabled"]:
                    raise EngineError("sim: 409 flow_off")
                v["target"] = v["counts"] = int(body["counts"])
                v["noise"] = self.rnd.gauss(0, 12)
                v["seq"] += 2
                v["busy_until"] = time.time() + 0.3
                return {"ok": True}
            if path.endswith("/flow"):
                was = v["cfg"]["enabled"]
                v["cfg"] = {**v["cfg"], **body}
                if v["cfg"]["enabled"] and not was:
                    v["target"] = v["counts"] = 0
                    v["seq"] += 1
                    v["busy_until"] = time.time() + 0.3
                elif was and not v["cfg"]["enabled"]:
                    v["pct"] = v["cfg"]["fallbackPct"]
                return {"ok": True}
            if path.endswith("/target"):
                if int(body["target"]) != v["desired"]:
                    v["desired"] = v["pct"] = int(body["target"])
                    v["busy_until"] = time.time() + 0.1
                return {"ok": True}
        raise EngineError(f"sim: POST {path}")

    def valves(self):
        return [v for v in self.get("/api/valves")["valves"] if v.get("active")]

    def valve(self, idx):
        with self.lock:
            return self.doc(idx)


def sim_reader(engine, sims):
    """Answers the cards like a reader: OK within 0.1, a value when far off, else more/less."""
    while True:
        time.sleep(0.3)
        s = engine.view()
        if s["phase"] != "reading":
            continue
        if s["confirm"]:
            engine.answer({"confirm": True})
            continue
        for c in s["cards"]:
            if not c["ask"] or c["key"] in s["answered"]:
                continue
            name, _, idx = c["key"].rpartition(" ")
            q = round(sims[name].flow_true(int(idx)) / 0.05) * 0.05
            e = c["expect"]
            if e is None:
                engine.answer({"key": c["key"], "answer": "value", "value": q})
            elif abs(q - e) <= OK_LPM:
                engine.answer({"key": c["key"], "answer": "ok"})
            elif abs(q - e) > 0.4:
                engine.answer({"key": c["key"], "answer": "value", "value": q})
            else:
                engine.answer({"key": c["key"], "answer": "more" if q > e else "less"})


# ───────────────────────────── page ─────────────────────────────

def make_handler(engine, page):
    class H(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def send(self, code, body, ctype="application/json; charset=utf-8"):
            raw = body if isinstance(body, bytes) else body.encode()
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(raw)))
            self.send_header("Cache-Control", "no-store")
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
            if p == "/api/presets":
                try:
                    return self.js(engine.presets())
                except EngineError as e:
                    return self.js({"error": str(e)}, 502)
            self.send(404, "{}")

        def do_POST(self):
            p = urllib.parse.urlparse(self.path).path
            n = int(self.headers.get("Content-Length") or 0)
            try:
                body = json.loads(self.rfile.read(n) or b"{}")
                if p == "/api/start":
                    engine.start(body)
                elif p == "/api/answer":
                    engine.answer(body)
                elif p == "/api/stop":
                    engine.stop()
                elif p == "/api/apply-fit":
                    engine.apply_fit(body["key"])
                else:
                    return self.send(404, "{}")
                self.js({"ok": True})
            except (EngineError, KeyError, ValueError) as e:
                self.js({"error": str(e)}, 400)

    return H


def upload(esps, path, enable=False):
    """Writes the points of a curves file ({"NAME IDX": {"curve": [[counts, lpm], ...]}}) into the
    controllers, with "refSide", "strokeCounts", "offset" and "gain" when an entry has them; the
    rest of the flow config stays."""
    curves = json.loads(Path(path).read_text(encoding="utf-8"))
    by = {e.name: e for e in esps}
    for key, c in curves.items():
        name, _, idx = key.rpartition(" ")
        esp = by.get(name)
        if not esp:
            continue
        pts = [[max(0, int(p)), round(float(q), 3)] for p, q in c["curve"]]
        # 2..16 points, counts strictly increasing, flow non-decreasing
        clean = []
        for p, q in pts:
            if clean and p <= clean[-1][0]:
                continue
            clean.append([p, max(q, clean[-1][1] if clean else 0.0)])
        while len(clean) > 16:
            # drop the point that changes the curve least
            k = min(range(1, len(clean) - 1), key=lambda j: abs(
                clean[j][1] - curve_flow([clean[j - 1], clean[j + 1]], clean[j][0])))
            clean.pop(k)
        cfg = flow_cfg(esp, int(idx))
        extra = {k: c[k] for k in ("refSide", "strokeCounts", "offset", "gain") if k in c}
        cfg = {**cfg, **extra, "points": clean, "enabled": bool(enable or cfg.get("enabled"))}
        esp.post(f"/api/valves/{idx}/flow", cfg)
        print(f"{key}: {len(clean)} points, top {clean[-1]}")


def main():
    # the log has Polish words and arrows; a Windows console has another code page
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--controller", action="append", default=[], metavar="NAME=HOST")
    ap.add_argument("--port", type=int, default=4681)
    ap.add_argument("--bind", default="0.0.0.0")
    ap.add_argument("--out", type=Path, default=None, help="folder of flowcheck.db (default ~/flowbench)")
    ap.add_argument("--sim", action="store_true")
    ap.add_argument("--sim-auto", action="store_true")
    ap.add_argument("--ha-power", metavar="ENTITY")
    ap.add_argument("--pick", default="", metavar="KEYS",
                    help="loops marked for the repeat session, comma-separated: 'west 3,east 7'")
    ap.add_argument("cmd", nargs="*", help="upload CURVES.json [--enable]")
    ap.add_argument("--enable", action="store_true")
    a = ap.parse_args()
    esps, sims = [], {}
    if a.sim:
        for i, (name, n) in enumerate((("sim-west", 6), ("sim-east", 7))):
            sims[name] = SimEsp(name, n, seed=i + 1)
            esps.append(sims[name])
    for c in a.controller:
        name, _, host = c.partition("=")
        esps.append(Esp(name, host if host.startswith("http") else f"http://{host}"))
    if not esps:
        ap.error("give --controller NAME=HOST (or --sim)")
    if a.cmd[:1] == ["upload"]:
        return upload(esps, a.cmd[1], a.enable)
    out = a.out or Path.home() / "flowbench" / ("sim" if a.sim else "")
    out.mkdir(parents=True, exist_ok=True)
    power = None
    if a.ha_power:
        url, token = os.environ.get("HA_URL"), os.environ.get("HA_TOKEN")
        if not (url and token):
            ap.error("--ha-power needs HA_URL and HA_TOKEN in the environment")
        power = HaPower(url, token, a.ha_power)
    engine = Engine(esps, out, power, [k.strip() for k in a.pick.split(",") if k.strip()])
    page = (HERE / "flowcheck.html").read_text(encoding="utf-8")
    srv = Server((a.bind, a.port), make_handler(engine, page))
    print(f"flowcheck: http://{lan_ip()}:{a.port}  (on this PC: http://localhost:{a.port})")
    if a.sim_auto:
        threading.Thread(target=sim_reader, args=(engine, sims), daemon=True).start()
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        # the valves of a running session go back first
        engine.stop()
        if engine.thread:
            engine.thread.join(300)


if __name__ == "__main__":
    main()
