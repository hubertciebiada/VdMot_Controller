# ESP web server stability under concurrent connections

Scope: the ESP32 (WT32-ETH01) web server, built on `AsyncTCP` 1.1.1 +
`AsyncWebServer_WT32_ETH01` 1.6.2. This note records the concurrent-connection
reboot found in 2.1.x, the analysis behind the 2.1.3 fix, the use after free
behind the panic that remained after it, and the recommended full fix. What
each release changed is in [CHANGELOG.md](CHANGELOG.md) (2.1.1, 2.1.2, 2.1.3,
Unreleased).

## Symptom

Under sustained concurrent HTTP connections the controller reboots with a
panic. A read-only load test (many parallel `GET`s to `/api/status`,
`/api/valves`, `/api/health`) reproduces it; a single client and normal use do
not. On 2.1.0/2.1.1/2.1.2 the reboot came within ~35 s of 10 parallel clients.

The reboot is a `panic` (not the heap guard, not a stack overflow, not the task
watchdog): confirmed by `esp.resetReason` in `GET /api/status` and by the `Boot`
event (code 100, `esp_reset_reason()` in arg1). Free heap at the time is
40–90 KB, far above the 12 KB heap-guard threshold.

## Root cause

`AsyncTCP` 1.1.1 leaks its per-pcb "closed slot": the slot is released only in
`_lwip_fin`, so `close()`, `abort()` and error paths keep their slot. Once all
16 slots are taken, the client constructor and `_lwip_fin` index
`_closed_slots[-1]` — an out-of-bounds write into static memory → panic. Under
connection churn 16 app-side closes accumulate quickly (~tens of seconds under
load), which matches the load-test reboot; accumulating slowly over days matches
the "crash after hours" seen on 2.0.x/2.1.0.

A second, secondary weakness: the library allocates every lwIP event packet with
`malloc()` and dereferences it without a NULL check, and it sends events onto its
queue with an unbounded `xQueueSend(..., portMAX_DELAY)`.

## How the fixes are applied

All ESP fixes are build-time patches of the vendored libraries by
`software_esp32_revamped/tools/patch_libs.py` (a PlatformIO post script that
also runs in CI). Each is guarded by a marker and a build-time check that fails
the build if an expected site is missing. None of this is in `src/`/`lib/`, so
it is outside the mutation suites.

The tunables: `web::kMaxConnections` in `src/web_server.h` (4; the script
reads it and it becomes `VDM_MAX_CONN`, the lwIP listen backlog that caps the
connections of a server; the web server sizes its table of refused requests by
it) and `ASYNC_TCP_QUEUE_WAIT_MS` at the top of the script (10; the longest
wait of a send into the event queue before the event is dropped). A library
already patched with other values fails the build; delete `.pio/libdeps` to
patch it again.

## Result on 2.1.3

The deterministic fast crash is gone; the panic is not. The load-test runs on
2.1.3 (2026-09-28, `tools/loadtest.py`, below) were:

| Workers | Outcome |
|---|---|
| 6 | panic after 13 s — sooner than the ~35 s of 10 workers before the fix |
| 6 (re-run) | no panic in the 60 s of the run, 847 requests |
| 10 | panic after 176 s, about 2100 requests |

These three runs were all the data of 2.1.3; the residual is probabilistic.
Its cause was found later: see "Root cause of the residual panic" below.

In normal use no reboot has been observed so far: one controller on 2.1.3 ran
10.5 h without a reboot (from 2026-09-28 21:23 UTC, read on 2026-09-29 07:51
UTC). That is one unit and one night, not proof.

Normal use opens few connections at a time, but the dashboard is not
sequential. It polls with independent timers — status every 5 s; valves and
events every 3 s and sensors every 10 s on their views; the STM flasher every
1 s and the file lists every 60 s on Maintenance — and each poller waits only
for its own previous request. Switching a view starts the pollers of the new
view at once, returning to the tab starts all of them at once. That is up to 2
requests at a time on Valves and up to 4 on Maintenance, the whole connection
cap; loading the page fetches `app.css` and `app.js` in parallel after
`index.html`, and every further open dashboard adds its own. Home Assistant
uses MQTT, not HTTP. After a reboot the controller is back in ~4 s and restores
the valve targets from RTC/NVS.

The residual was accepted as a known limitation in 2.1.3 and 2.1.4
(CHANGELOG 2.1.3).

## Root cause of the residual panic

A test build that keeps the cause of a panic in RTC memory (a wrapper of
`esp_panic_handler`, never part of a release) caught the residual on a
WT32-ETH01 under 3 parallel clients: `LoadProhibited` in the `async_tcp`
task, `EXCVADDR` = the controller's own IP address, backtrace `strlen` ←
`String::String` ← `AsyncWebServerRequest::_removeNotInterestingHeaders()` ←
`_parseLine()` ← `_onData()` ← `AsyncClient::_recv()`.

`_removeNotInterestingHeaders()` runs for every request once its headers are
parsed and drops the headers no handler registered. The firmware registers
`Origin`, `X-VdMot`, `If-None-Match` and the MD5 headers, so a browser request
drops most of its headers. The library calls `_headers.remove(header)` inside a
range-for over `_headers`: `remove()` deletes the node the iterator stands on,
and the next step of the loop reads `next` from the freed node. Nothing happens
while the block keeps its old bytes. When the lwIP thread (priority 18, the
same core as `async_tcp`) preempts the loop in between and takes the block for
a packet, the loop follows a pointer made of packet bytes, here the IP address
of a packet header. That is why the panic needs parallel traffic and comes at
random; more heap traffic on that core (WiFi scanning) makes it likelier.

`patch_libs.py` (marker `VDM-PATCH-WEB-HEADERS`) replaces the loop with
`remove_first()` calls, one header per pass from the start of the list, and
fails the build if the library still removes inside the loop. With the fix,
the same test build (WiFi scanning next to Ethernet) ran 300 s with 3 clients
(3833 requests) and 600 s with 10 clients (7898 requests) without a panic;
without the fix it had panicked after 143 s with 3 clients.

## Suggested improvements

Ordered by impact.

1. **Replace `AsyncTCP`/`AsyncWebServer` with ESP-IDF `esp_http_server`.** The
   root problem is the library's architecture: allocations and callbacks run in
   the lwIP TCP/IP thread, with an event queue between it and an async task.
   `esp_http_server` runs one server task over BSD sockets: the handlers run in
   that task one after another, the open sockets are bounded by
   `max_open_sockets` (optionally with an LRU purge), and no application code
   or allocation runs in the lwIP thread, which removes this whole class of
   fault. Requests are served one at a time unless a handler hands its request
   to a worker task of its own (`httpd_req_async_handler_begin`, ESP-IDF 5.x;
   the Arduino core used here, 2.0.7, is on ESP-IDF 4.4.4 and has
   `esp_http_server` without it). This is the recommended full fix and the
   natural next step (it also keeps a future move to `esp-idf-svc`/Rust open).
2. **Close the remaining `AsyncTCP` defects** if the library is kept:
   - a stale poll event queued before an RST can still call `_close()` on a freed
     pcb — needs the closed slot marked in the lwIP-thread error callback (an
     `AsyncTCP.h` header patch); found in review, not reproduced;
   - unverified: whether a peer that disappears after the response can hold a
     connection-cap slot for good. The library turns the RX timeout off in
     `send()` (`WebRequest.cpp`); data in flight is closed by the 5 s ACK
     timeout, and a closed pcb by lwIP's FIN retransmissions and its FIN_WAIT_2
     timeout. If a path without any timer exists, four such peers make the web
     server unreachable until a restart (the cap is 4), and this item ranks
     first; the fix would be to re-arm the RX timeout after `send()`.
3. **Diagnosability:** the only post-mortem signal of a release is
   `esp_reset_reason()`. A core-dump partition or an RTC breadcrumb of the
   faulting PC would let a future crash be pinpointed without serial access;
   the breadcrumb of a test build found the residual panic above.
4. **Defense in depth:** bound simultaneous connections at the application layer
   as well, and review the lwIP socket/pcb limits (16 active TCP pcbs, 16
   sockets). With `framework = arduino` those limits are precompiled into the
   core: changing them means building the core with the lib-builder or using
   Arduino as an ESP-IDF component.

## Reproducing and observing

- Reproduce: `software_esp32_revamped/tools/loadtest.py <ip> --workers N
  --seconds S` sends read-only `GET`s to `/api/status`, `/api/valves`,
  `/api/health` and `/` from N threads, a new connection per request, and
  prints the heap and the request counts every 5 s. It stops by itself when
  the uptime drops (a reboot) or the free heap falls below `--low-heap`. The
  crash needs sustained parallelism, so vary the worker count.
- Observe the cause after any reboot: `GET /api/status` → `esp.resetReason`
  (`panic` / `task_wdt` / `sw` / `poweron` / `brownout` / …) and `esp.boots`; or
  the `Boot` event in `GET /api/events`.
