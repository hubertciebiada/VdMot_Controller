#!/usr/bin/env python3
# Bounded, read-only load/soak test for a VdMot controller's web server.
# Drives concurrent GETs at the heavy JSON endpoints to measure the heap floor
# under web load and confirm there is no leak. No writes, no valve actions, no
# OTA. The server has 2 response slots (12 KB each): past 2 concurrent JSON
# responses it answers 503 (by design), so this measures a plateau. Bounded by
# --seconds; stops itself if the controller reboots (uptime drops).
#
#   loadtest.py <ip> [--seconds N] [--workers K] [--low-heap BYTES]
import collections
import json
import sys
import threading
import time
import urllib.error
import urllib.request

ip = sys.argv[1]
args = sys.argv[2:]


def flag(name, default):
    return int(args[args.index(name) + 1]) if name in args else default


seconds = flag("--seconds", 180)
workers = flag("--workers", 10)
low_heap = flag("--low-heap", 20000)
# GET only. No POST, no /api/valves/<n>/target, no upload, no /api/log stream.
READ_PATHS = ["/api/status", "/api/valves", "/api/health", "/"]

stop = threading.Event()
codes = collections.Counter()
lock = threading.Lock()


def get(path, timeout=5):
    req = urllib.request.Request("http://%s%s" % (ip, path))
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            r.read()
            return r.status
    except urllib.error.HTTPError as e:
        return e.code
    except Exception as e:
        return type(e).__name__


def worker():
    i = 0
    while not stop.is_set():
        c = get(READ_PATHS[i % len(READ_PATHS)])
        with lock:
            codes[c] += 1
        i += 1


def health():
    for _ in range(2):
        try:
            with urllib.request.urlopen("http://%s/api/health" % ip, timeout=5) as r:
                return json.loads(r.read())
        except Exception:
            time.sleep(0.5)
    return {}


h0 = health()
if not h0:
    print("no baseline health from %s" % ip)
    sys.exit(2)
print("baseline %s uptime %s heap %s" % (h0.get("version"), h0.get("uptime"), h0.get("heap")))

min_free = min_largest = 10 ** 9
prev_uptime = h0.get("uptime")
rebooted = False

threads = [threading.Thread(target=worker, daemon=True) for _ in range(workers)]
for t in threads:
    t.start()

t0 = time.time()
t_end = t0 + seconds
while time.time() < t_end:
    time.sleep(5)
    h = health()
    heap = h.get("heap") or {}
    fr, lg, up = heap.get("free"), heap.get("largest"), h.get("uptime")
    if isinstance(fr, int):
        min_free = min(min_free, fr)
    if isinstance(lg, int):
        min_largest = min(min_largest, lg)
    if isinstance(up, int) and isinstance(prev_uptime, int) and up < prev_uptime:
        rebooted = True
    if isinstance(up, int):
        prev_uptime = up
    with lock:
        snap = dict(codes)
    total = sum(snap.values())
    ok = snap.get(200, 0)
    busy = snap.get(503, 0)
    errs = total - ok - busy
    print("t+%03ds free %s largest %s uptime %s | reqs %d ok %d busy503 %d other %d"
          % (time.time() - t0, fr, lg, up, total, ok, busy, errs))
    if rebooted or (isinstance(fr, int) and fr < low_heap):
        print("STOP early: %s" % ("reboot detected" if rebooted else "free heap < %d" % low_heap))
        break

stop.set()
for t in threads:
    t.join(timeout=2)

with lock:
    final = dict(codes)
total = sum(final.values())
print("DONE reqs %d codes %s" % (total, dict(sorted(final.items(), key=lambda kv: str(kv[0])))))
print("min_free %d min_largest %d rebooted %s" % (min_free, min_largest, rebooted))
h1 = health()
print("after uptime %s heap %s" % (h1.get("uptime"), h1.get("heap")))
