#!/usr/bin/env python3
"""End stops and counted travel of the valves, without a reader (pulse counts only).

Per valve, one after the other on each controller (the controllers run in parallel):

  closed  close at 34 mA, then 5 x (open 300, close at 34 mA): the pulses of each close
  steps   2 x (one opening move of N, close) and 2 x (N in 5 moves, close): what the counter
          misses per move = (way back after 5 moves - way back after 1 move) / 4
  open    open to the open end stop, then 5 x (close 300, open to the end stop)
  zero    2 x (0 %, the target back, 0 % = the normal close of the STM, close at 34 mA): how far the
          normal 0 % stands above the 34 mA stop
  restore target 0 %, then the target the valve had

Two more phases (--phases ruler,top), for the stop the flow layer measures from:

  ruler   5 x (close at 34 mA, open to the open end stop): the spread of the counted stroke says
          whether the 34 mA stop repeats (the first after --cold-s idle: the cold motor)
  top     open to the end stop, then 5 x (close K, open to the end stop), K = stroke - the
          position of 1.0 l/min (--pos): the same point reached from above

One more (--phases brake), for the brake of the service moves (STM 2.2.9 sbrak):

  brake   for every setting of --brakes: set the brake, one warm-up move (open 300, close), then
          the steps phase, 5 x (one opening move of N, close at 34 mA) and, with --pos, the top
          phase; the missed pulses per opening move with and without the brake, the spread of
          the single move, the brake pulses (counted - requested) and the move time; at the end
          the brake is off

Every move goes to a JSON line in --out. The loops keep flowing on their targets; only the valve
under test moves.

  python tools/flowbench/endstop_test.py --controller west=192.168.1.51 --out endstops.jsonl
  python tools/flowbench/endstop_test.py --sim --out sim.jsonl
"""
import argparse
import json
import statistics
import sys
import threading
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from flowbench import CLOSE_MA, OPEN_MA, EngineError, Esp, SimDevice, serve_sim  # noqa: E402

REPEATS = 5
STEP_PULSES = 300
STEPS_K = 5
N_DEFAULT = 900  # about 1.5 l/min on the measured loops
REFUSED_WAIT_S = 60  # a calibration in the queue refuses service moves; wait and try again
PHASES_DEFAULT = ("closed", "steps", "open", "zero")
# sbrak settings (maxMs, gapMs, mult) of the brake phase; (0, 0, 0) = off
BRAKES_DEFAULT = ((0, 0, 0), (40, 12, 0), (40, 8, 0), (40, 0, 3), (60, 20, 0), (30, 0, 2))


class Run:
    def __init__(self, esp, out, lock, n_by_valve, phases=PHASES_DEFAULT, pos_by_valve=None,
                 brakes=BRAKES_DEFAULT):
        self.esp, self.out, self.lock, self.n_by_valve = esp, out, lock, n_by_valve
        self.phases, self.pos_by_valve, self.brakes = phases, pos_by_valve or {}, brakes
        # the counted open stroke from the 34 mA stop (ruler phase) per valve, for K of the top phase
        self.stroke_by_valve = {}

    def write(self, **rec):
        rec = {"t": round(time.time(), 1), "ctrl": self.esp.name, **rec}
        with self.lock:
            with self.out.open("a", encoding="utf-8") as f:
                f.write(json.dumps(rec, ensure_ascii=False) + "\n")
        print(json.dumps(rec, ensure_ascii=False), flush=True)

    def move(self, idx, direction, counts, ma, phase, refused=0):
        """One service move; (counted pulses, stop). Same rules as flowbench.service_move."""
        esp = self.esp
        esp.events(idx)
        ext0 = esp.valve(idx).get("ext", {})
        seq0, last0 = ext0.get("moveSeq"), ext0.get("lastMove") or {}
        counts = int(counts)
        if last0.get("dir") == direction and last0.get("req") == counts:
            counts += 1 if counts < 10000 else -1
        esp.post(f"/api/valves/{idx}/service-move", {"dir": direction, "counts": counts,
                                                     "maxmA": int(ma)})
        t0, started = time.time(), False
        while True:
            time.sleep(0.4)
            for e in esp.events(idx):
                if e.get("code") == 411 or (e.get("code") == 414 and e.get("a1", 0) < 0):
                    if refused >= 3:
                        raise EngineError(f"{esp.name} {idx}: service move refused 4 times")
                    self.write(idx=idx, phase=phase, refused=e.get("msg"))
                    time.sleep(REFUSED_WAIT_S)
                    return self.move(idx, direction, counts, ma, phase, refused + 1)
            v = esp.valve(idx)
            ext = v.get("ext", {})
            lm = ext.get("lastMove") or {}
            if v.get("stateKey") in ("opening", "closing"):
                started = True
            if v.get("stateKey") == "idle" and ext.get("moveSeq") != seq0 \
                    and lm.get("dir") == direction and lm.get("req") == counts:
                rec = {"idx": idx, "phase": phase, "dir": direction, "req": counts,
                       "cnt": int(lm.get("cnt", 0)), "stop": lm.get("stop"),
                       "peak": lm.get("peak"), "ms": lm.get("ms")}
                self.write(**rec)
                return rec["cnt"], rec["stop"]
            el = time.time() - t0
            if not started and el > 12:
                raise EngineError(f"{esp.name} {idx}: no confirmation of the move in 12 s")
            if el > 120:
                raise EngineError(f"{esp.name} {idx}: the move takes more than 120 s")

    def close_ref(self, idx, span, phase):
        """Close at 34 mA to the end stop from about span pulses above it; the pulses counted."""
        total, stop = self.move(idx, "close", min(10000, span + 500), CLOSE_MA, phase)
        for _ in range(2):
            if stop in ("endstop", "early_endstop"):
                return total
            more, stop = self.move(idx, "close", 800, CLOSE_MA, phase)
            total += more
        if stop not in ("endstop", "early_endstop"):
            raise EngineError(f"{self.esp.name} {idx}: the close did not reach the end stop")
        return total

    def open_end(self, idx, phase):
        total, stop = self.move(idx, "open", 10000, OPEN_MA, phase)
        if stop not in ("endstop", "early_endstop"):
            raise EngineError(f"{self.esp.name} {idx}: the open did not reach the end stop ({stop})")
        return total

    def top(self, idx, stroke, phase):
        k = int(round(stroke - self.pos_by_valve[idx]))
        self.write(idx=idx, phase=phase, k=k, stroke=round(stroke), pos10=self.pos_by_valve[idx])
        self.open_end(idx, phase)
        for _ in range(REPEATS):
            self.move(idx, "close", k, CLOSE_MA, phase)
            self.open_end(idx, phase)

    def set_brake(self, idx, setting):
        max_ms, gap_ms, mult = setting
        body = {"maxMs": max_ms, "gapMs": gap_ms}
        if mult:
            body["mult"] = mult
        self.esp.post(f"/api/valves/{idx}/brake", body)
        self.write(idx=idx, phase="brake", brake=list(setting))

    def steps(self, idx, n, tag=""):
        for k in (1, 1, STEPS_K, STEPS_K):
            for j in range(k):
                part = n // k + (1 if j < n % k else 0)
                self.move(idx, "open", part, OPEN_MA, f"steps{k}{tag}")
            self.close_ref(idx, n, f"steps{k}{tag}")

    def target(self, idx, pct, phase):
        self.esp.post(f"/api/valves/{idx}/target", {"target": pct})
        t0 = time.time()
        while time.time() - t0 < 180:
            time.sleep(2)
            v = self.esp.valve(idx)
            if v.get("stateKey") == "idle" and abs((v.get("pos") or 0) - pct) <= 1 \
                    and v.get("target") == pct:
                self.write(idx=idx, phase=phase, target=pct, pos=v.get("pos"))
                return
        raise EngineError(f"{self.esp.name} {idx}: target {pct} % not reached in 180 s")

    def valve(self, v):
        idx, oc = v["idx"], v.get("oc") or 4500
        back_to = v.get("target") if v.get("target") is not None else v.get("pos", 0)
        n = self.n_by_valve.get(idx, N_DEFAULT)
        self.write(idx=idx, phase="start", pos=v.get("pos"), target=back_to, oc=oc, n=n)
        span = int((v.get("pos", 0) + 5) * oc / 100)
        ph = self.phases
        stroke = self.stroke_by_valve.get(idx, oc)
        if "closed" in ph:
            self.close_ref(idx, span, "closed")
            for _ in range(REPEATS):
                self.move(idx, "open", STEP_PULSES, OPEN_MA, "closed")
                self.close_ref(idx, STEP_PULSES, "closed")
        if "steps" in ph:
            self.steps(idx, n)
        if "brake" in ph:
            self.close_ref(idx, span, "brake")
            try:
                for setting in self.brakes:
                    self.set_brake(idx, setting)
                    tag = "@" + ",".join(map(str, setting))
                    self.move(idx, "open", STEP_PULSES, OPEN_MA, "warmup" + tag)
                    self.close_ref(idx, STEP_PULSES, "warmup" + tag)
                    self.steps(idx, n, tag)
                    for _ in range(REPEATS):
                        self.move(idx, "open", n, OPEN_MA, "p1" + tag)
                        self.close_ref(idx, n, "p1" + tag)
                    if idx in self.pos_by_valve:
                        self.top(idx, stroke, "top" + tag)
                        self.close_ref(idx, oc, "top" + tag)
            finally:
                self.set_brake(idx, (0, 0, 0))
        if "open" in ph:
            self.open_end(idx, "open")
            for _ in range(REPEATS):
                self.move(idx, "close", STEP_PULSES, CLOSE_MA, "open")
                self.open_end(idx, "open")
            self.close_ref(idx, oc, "open")
        if "ruler" in ph:
            self.close_ref(idx, span, "ruler")
            strokes = []
            for _ in range(REPEATS):
                strokes.append(self.open_end(idx, "ruler"))
                self.close_ref(idx, oc, "ruler")
            stroke = statistics.mean(strokes)
        if "top" in ph and idx in self.pos_by_valve:
            self.top(idx, stroke, "top")
        if "zero" in ph:
            self.zero(idx, back_to)
        self.target(idx, 0, "restore")
        self.target(idx, back_to, "restore")

    def zero(self, idx, back_to):
        """2 x the normal close of the STM, then the close at 34 mA. The ESP sends only a changed
        target and a service hold ignores the target, so 0 % first (ends the hold), then the
        target back (the normal opening), then 0 % (the normal close)."""
        for _ in range(2):
            self.target(idx, 0, "zero")
            self.target(idx, max(5, back_to), "zero")
            self.target(idx, 0, "zero")
            self.close_ref(idx, max(5, back_to) * 50, "zero")

    def run(self, only, zero_only=False):
        for v in self.esp.valves():
            if only and v["idx"] not in only:
                continue
            try:
                if zero_only:
                    back = v.get("target") if v.get("target") is not None else v.get("pos", 0)
                    self.zero(v["idx"], back)
                    self.target(v["idx"], 0, "restore")
                    self.target(v["idx"], back, "restore")
                    continue
                self.valve(v)
            except Exception as e:  # noqa: BLE001 - any failure leaves the valve on a target
                self.write(idx=v["idx"], phase="error", error=f"{type(e).__name__}: {e}")
                # leave the valve on a normal target again
                try:
                    back = v.get("target") if v.get("target") is not None else v.get("pos", 0)
                    self.target(v["idx"], 0, "restore")
                    self.target(v["idx"], back, "restore")
                except EngineError as e2:
                    self.write(idx=v["idx"], phase="error", error=f"restore: {e2}")


def summary(path):
    """Per valve: spread of the 34 mA closes, missed pulses per opening move, open end, zero."""
    recs = [json.loads(line) for line in Path(path).read_text(encoding="utf-8").splitlines()]
    out = {}
    for r in recs:
        out.setdefault((r["ctrl"], r["idx"]), []).append(r)
    rows = []
    for (c, i), rs in sorted(out.items()):
        # a run that stopped early is repeated: the phases come from the last full start, the
        # zero phase (also run on its own) from every run
        starts = [k for k, r in enumerate(rs) if r["phase"] == "start"]
        last = rs[starts[-1]:] if starts else rs
        moves = [r for r in last if "cnt" in r and r["phase"] != "zero"]             + [r for r in rs if "cnt" in r and r["phase"] == "zero"]
        def closes(phase):
            # a close group = the closes after an opening, summed up to the end stop
            groups, cur = [], None
            for r in moves:
                if r["phase"] != phase:
                    continue
                if r["dir"] == "close":
                    cur = (cur or 0) + r["cnt"]
                    if r["stop"] in ("endstop", "early_endstop"):
                        groups.append(cur)
                        cur = None
            return groups
        cl = closes("closed")[1:]
        s1, s5 = closes("steps1"), closes(f"steps{STEPS_K}")
        up1 = [r["cnt"] for r in moves if r["phase"] == "steps1" and r["dir"] == "open"]
        up5 = [r["cnt"] for r in moves if r["phase"] == f"steps{STEPS_K}" and r["dir"] == "open"]
        op = [r["cnt"] for r in moves if r["phase"] == "open" and r["dir"] == "open"][1:]
        zero = closes("zero")
        brake = {}
        for tag in dict.fromkeys(r["phase"].partition("@")[2] for r in moves if "@" in r["phase"]
                                 and r["phase"].startswith("steps")):
            b1, b5 = closes(f"steps1@{tag}"), closes(f"steps{STEPS_K}@{tag}")
            o1 = [r for r in moves if r["phase"] == f"steps1@{tag}" and r["dir"] == "open"]
            o5 = [r for r in moves if r["phase"] == f"steps{STEPS_K}@{tag}" and r["dir"] == "open"]

            def per_move(key):
                # (way back after 5 moves - way up) - (the same after 1 move), per extra move
                if not (b1 and b5):
                    return None
                return round(((sum(b5) - sum(r[key] for r in o5)) / len(b5)
                              - (sum(b1) - sum(r[key] for r in o1)) / len(b1)) / (STEPS_K - 1), 1)
            brake[tag] = {
                # against the requested pulses: what one more move adds to the travel of the
                # request; against the counted ones: what the counter still misses
                "missed_per_move": per_move("req"), "missed_counted": per_move("cnt"),
                "p1": closes(f"p1@{tag}"),
                "p1_sd": round(statistics.pstdev(closes(f"p1@{tag}")), 1)
                if len(closes(f"p1@{tag}")) > 1 else None,
                "top": [r["cnt"] for r in moves if r["phase"] == f"top@{tag}"
                        and r["dir"] == "open"][1:],
                "top_close": [r["cnt"] for r in moves if r["phase"] == f"top@{tag}"
                              and r["dir"] == "close" and r["stop"] == "target"],
                "brake_pulses": [r["cnt"] - r["req"] for r in o1 + o5],
                "ms5": [r["ms"] for r in o5], "back1": b1, "back5": b5}
        ru = [r["cnt"] for r in moves if r["phase"] == "ruler" and r["dir"] == "open"]
        top_k = [r for r in last if r["phase"] == "top" and "k" in r]
        # the first open of the top phase starts at the 34 mA stop, the others after a close of K
        tp = [r["cnt"] for r in moves if r["phase"] == "top" and r["dir"] == "open"][1:]
        tk = [r["cnt"] for r in moves if r["phase"] == "top" and r["dir"] == "close"]
        row = {"ctrl": c, "idx": i,
               "close34": cl, "close34_sd": round(statistics.pstdev(cl), 1) if len(cl) > 1 else None,
               "open_end": op, "open_end_sd": round(statistics.pstdev(op), 1) if len(op) > 1 else None,
               "back1": s1, "back5": s5,
               "missed_per_move": round(((sum(s5) - sum(up5)) / max(1, len(s5))
                                         - (sum(s1) - sum(up1)) / max(1, len(s1))) / (STEPS_K - 1), 1)
               if s1 and s5 else None,
               "zero_offset": zero,
               "brake": brake,
               "ruler": ru, "ruler_sd": round(statistics.pstdev(ru[1:]), 1) if len(ru) > 2 else None,
               "ruler_close": closes("ruler"),
               "top_k": top_k[0]["k"] if top_k else None, "top_close": tk, "top": tp,
               "top_sd": round(statistics.pstdev(tp), 1) if len(tp) > 1 else None,
               "errors": [r["error"] for r in last if r["phase"] == "error"]}
        rows.append(row)
    return rows


def main():
    # the controller names can be Polish words; a Windows console has another code page
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--controller", action="append", default=[], metavar="NAME=HOST")
    ap.add_argument("--sim", action="store_true")
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--valves", default="", help="NAME:1,2,... per controller, e.g. west:1,3")
    ap.add_argument("--n", type=Path, help='JSON {"NAME IDX": pulses} for the steps phase')
    ap.add_argument("--summary", action="store_true", help="only print the summary of --out")
    ap.add_argument("--zero-only", action="store_true", help="only the zero phase")
    ap.add_argument("--phases", default=",".join(PHASES_DEFAULT),
                    help="closed,steps,open,ruler,top,zero (default: the first test)")
    ap.add_argument("--pos", type=Path, help='JSON {"NAME IDX": pulses of 1.0 l/min} for the top phase')
    ap.add_argument("--stroke", type=Path,
                    help='JSON {"NAME IDX": counted open stroke from the 34 mA stop} for K of top')
    ap.add_argument("--cold-s", type=int, default=0, help="wait before the first valve (cold motors)")
    ap.add_argument("--brakes", default=";".join(",".join(map(str, b)) for b in BRAKES_DEFAULT),
                    help="sbrak settings maxMs,gapMs,mult separated by ; for the brake phase")
    a = ap.parse_args()
    if a.summary:
        for row in summary(a.out):
            print(json.dumps(row, ensure_ascii=False))
        return
    esps = []
    if a.sim:
        for i, (name, n) in enumerate((("sim-west", 2), ("sim-east", 2))):
            dev = SimDevice(name, [f"Loop {j + 1}" for j in range(n)], seed=i + 1, speed=20)
            esps.append(Esp(name, serve_sim(dev, 18091 + i)))
    for c in a.controller:
        name, _, host = c.partition("=")
        esps.append(Esp(name, host if host.startswith("http") else f"http://{host}"))
    only = {}
    for part in filter(None, a.valves.split(";")):
        name, _, idxs = part.partition(":")
        only[name] = {int(x) for x in idxs.split(",") if x}
    n_cfg = json.loads(a.n.read_text(encoding="utf-8")) if a.n else {}
    pos_cfg = json.loads(a.pos.read_text(encoding="utf-8")) if a.pos else {}
    stroke_cfg = json.loads(a.stroke.read_text(encoding="utf-8")) if a.stroke else {}
    phases = tuple(x for x in a.phases.split(",") if x)
    if a.cold_s:
        print(f"waiting {a.cold_s} s for cold motors", flush=True)
        time.sleep(a.cold_s)
    lock = threading.Lock()
    threads = []
    for esp in esps:
        def per_valve(cfg):
            return {int(k.split()[-1]): int(v) for k, v in cfg.items()
                    if k.rsplit(" ", 1)[0] == esp.name}
        brakes = tuple(tuple(int(x) for x in b.split(",")) for b in a.brakes.split(";") if b)
        run = Run(esp, a.out, lock, per_valve(n_cfg), phases, per_valve(pos_cfg), brakes)
        run.stroke_by_valve = per_valve(stroke_cfg)
        t = threading.Thread(target=run.run, args=(only.get(esp.name, set()), a.zero_only))
        t.start()
        threads.append(t)
    for t in threads:
        t.join()
    for row in summary(a.out):
        print(json.dumps(row, ensure_ascii=False))


if __name__ == "__main__":
    main()
