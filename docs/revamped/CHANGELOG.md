# Changelog: VdMot Revamped

Unofficial fork of VdMot_Controller. Versions carry the suffix `-revamped`.
Base: `hc-version` (upstream `developer` 1.4.12 + fixes).
The STM entries of each release are in
[software_stm32/ChangeLog.md](../../software_stm32/ChangeLog.md); this file lists
the ESP, the tooling and a summary of the STM.

## [Unreleased]

## [2.1.6-revamped]

ESP release: about 40 KB more free heap with the same functions (measured on
a controller: 132 KB free after boot, at least 80 KB under parallel web load;
2.1.5: about 90 KB and 41 to 50 KB), and one HA discovery run after a boot
instead of up to six. The STM firmware is unchanged; its image differs from
2.1.5 only in the version string and needs no update.

### Changed
- ESP: about 9 KB more free heap: the valve profiles (12 x 260 B) are no longer
  part of the STM snapshot, of which the firmware holds four copies (STM task,
  app, web, MQTT). They are kept once in a profile store of the app module;
  the snapshot counts the profile replies per valve, and `GET
  /api/valves/{n}/profile` and the MQTT diag topic `diag/valves/<V>/profile`
  copy the one profile they need. Nothing visible changes.
- ESP: the web server's working set shrinks from about 33 KB to about 10 KB
  (software_esp32_revamped/DESIGN.md sections 9 and 12): a JSON body gets a
  heap buffer of its length for its request instead of a permanent 8 KB
  buffer; the views and lists of the handlers share one scratch buffer; the
  patched copy of a config save lives on the heap for that request only.
  Visible in the API (docs/revamped/API.md): a body or a config save the
  device has no memory for is answered `503 busy` (`out of memory`, nothing
  applied), and `GET /api/files` lists at most 32 entries (was 48;
  `truncated` when there are more, the file system holds about a dozen).
  `GET /api/events` still takes `limit` 1..50; its buffer is sized to the
  32-event ring, which no response could exceed anyway.
- ESP: HA discovery after a connect waits until the STM data has settled
  (link up, re-sync done, 30 s for the 1-Wire lists; at most 2 min), and so
  does the run after a change of the STM's sensor assignments. 2.1.5 sent the
  whole set (about 147 configs) up to six times within 90 s after a boot,
  once per step of the STM start-up; the set published in the end is the
  same. Manual runs, the run when HA comes back and the first-run cleanup of
  mode MQTT start at once as before (docs/revamped/MQTT.md).
- ESP: about 7 KB more free heap in the MQTT task: the discovery context and
  payload buffer (5.4 KB) are allocated for a discovery run only (no memory:
  the run waits for the next pass), the on-change check of a valve keeps a
  CRC-32 of the fields it compares instead of the whole published valve
  state (1.6 KB), and a valve profile is copied with its JSON for the diag
  check only.
- ESP: 5.5 KB more free heap: the config blob buffers (`cfg` 4 KB, `cfgx`
  1.5 KB) are allocated for a config load, save, backup write or the import
  report only. Without memory a save is answered like an NVS error (500
  `invalid`, detail `nvs`, nothing applied), a backup waits for the next
  pass, and the boot load takes the defaults (`config_defaults` reason 102).

## [2.1.5-revamped]

ESP release: WiFi STA is back, and the rare web server panic of 2.1.3 and
2.1.4 is fixed. The STM firmware is unchanged; its image differs from 2.1.4
only in the version string and needs no update.

### Added
- ESP: WiFi STA again, as in 2.0.0: `net.iface` (0 auto, 1 Ethernet, 2 WiFi),
  `net.ssid` and `net.wifiPassword` ("" for an open network, else 8..63
  bytes), in the dashboard under Settings → Station and network. With the
  interface auto WiFi is the fallback after 30 s without an Ethernet IP (at
  once when the Ethernet driver did not start) and goes off again once
  Ethernet has an IP; while WiFi has no IP it reconnects with a back-off of
  5 s to 60 s. `/api/status` shows the state `wifi` and `rssi`, the events
  `net_up` (200) and `net_interface_restart` (212) report WiFi as interface 2
  (212: 3 = both), the network watchdog also reconnects WiFi, the network
  trial covers the interface and the WiFi fields, the legacy import takes
  `netCfg/ethwifi`, `ssid` and `pwd`, and the config repair has its two WiFi
  rules again (repair bits 2 and 3). Unlike 2.0.0, DHCP over WiFi sends the
  station's host name (2.0.0 sent the default `esp32-xxxxxx`), and Arduino's
  automatic reconnect is off: it would start a stopped WiFi again on
  Arduino's event task. WiFi takes about 33 KB of heap while it runs
  (software_esp32_revamped/DESIGN.md sections 9 and 16).
- ESP: the stack monitor (`/api/health` `tasks`, event 114 `stack_low`) also
  watches the ESP-IDF event loop `sys_evt` (network events) and the lwIP task
  `tiT` (TCP/IP, DHCP, SNTP), as it already did `arduino_events`, the task of
  the network event handler (DESIGN.md section 3).

### Changed
- ESP: `POST /api/config` applies `net.iface`, `net.ssid` and
  `net.wifiPassword` again instead of ignoring them. The stored settings kept
  them at their 2.0.0 places, so nothing is migrated: settings saved by 2.1.0
  to 2.1.4 (also by their legacy import, which runs once) hold the interface
  auto without WiFi, and such a controller stays on Ethernet; settings last
  saved by 2.0.0 keep their WiFi.
- ESP: a downgrade to 2.1.0 through 2.1.4 ignores the WiFi settings (a save
  there clears them), so a controller on WiFi only has no network after it:
  connect it by Ethernet first. With an OTA rollback the downgraded image
  goes back after 15 min; without one only an Ethernet cable or USB-serial
  reaches the controller again (INSTALL.md section 5).

### Fixed
- ESP: the rare panic of the web server under parallel requests, the known
  limitation of 2.1.3 and 2.1.4. The web server library dropped the headers
  no handler asked for with a use after free, which crashed when the lwIP
  thread reused the freed block in between; `tools/patch_libs.py` patches it
  (web-server-stability.md).

## [2.1.4-revamped]

Dashboard release of the ESP: the valve card explains its states and shows
the loop temperatures. The STM firmware is unchanged; its image differs
from 2.1.3 only in the version string and needs no update.

### Added
- Dashboard: every chip of a valve card (state, calibration, target
  delivery, failsafe, health flags) explains itself in a tooltip, on hover,
  on keyboard focus and on a tap (touch screens have no hover).
- Dashboard: the valve card shows Supply (sensor 1), Return (sensor 2) and
  their ΔT in a row above the diagnostics instead of a line of text; a
  failed reading is "no reading" in the warning colour. Settings names the
  columns "Sensor 1 (supply)" and "Sensor 2 (return)".
- `tools/loadtest.py`: the read-only load test of the web server, and
  `docs/revamped/web-server-stability.md`, the analysis of the AsyncTCP
  panic behind 2.1.1 to 2.1.3.

### Changed
- API.md and MQTT.md: `temp1`/`temp2` of a valve are the supply and the
  return of its heating loop.

### Known limitation
- Unchanged from 2.1.3: the rarer AsyncTCP panic under sustained abnormal
  concurrent HTTP load remains.

## [2.1.3-revamped]

Hotfix of 2.1.2 for the ESP: AsyncTCP no longer panics under concurrent
connections. The STM firmware is unchanged; its image differs from 2.1.2
only in the version string and needs no update.

### Fixed
- ESP: under concurrent connections AsyncTCP 1.1.1 leaked its closed slots
  (one per connection, given back only on the peer's FIN) and, once all 16
  were taken, wrote `_closed_slots[-1]` out of bounds: a panic after 16
  closes from our side (timeouts, aborts), which is about 35 s of 10
  parallel GETs or hours of normal use (it fits the 2.1.0 crash a minute
  after a web request). Confirmed on hardware: `esp.resetReason` of the
  next boot is `panic`. Its event queue also blocked on an unbounded send,
  a deadlock of the lwIP thread and the async task found in review. The
  build-time patch (`tools/patch_libs.py`) caps connections at 4 through
  the listen backlog (a SYN beyond it waits in the peer's TCP instead of
  being reset), releases the slot on every close path, bounds every send
  into the event queue to 10 ms and drops the event instead (received
  data stays with lwIP and is delivered again), and guards the callbacks
  against a client that is already gone.

### Known limitation
- ESP: under sustained abnormal concurrent HTTP load (minutes of many
  parallel clients) a rarer panic in AsyncTCP 1.1.1 remains: on the load
  test the controller served about 2100 requests in about 3 minutes before
  it. Normal use (Home Assistant over MQTT, the dashboard with at most 4
  requests at a time) has not triggered it so far, and the controller
  restarts on its own in about 4 s. The full fix is a web server on
  `esp_http_server` instead of AsyncTCP (future work). Analysis and load
  tests: [web-server-stability.md](web-server-stability.md).

## [2.1.2-revamped]

Hotfix of 2.1.1 for the ESP: a failed allocation in AsyncTCP no longer
panics the controller. The STM firmware is unchanged; its image differs
from 2.1.1 only in the version string and needs no update.

### Fixed
- ESP: AsyncTCP dereferenced its `lwip_event_packet_t` allocations without
  a NULL check and panicked under concurrent connections (reproduced: 10
  parallel GETs rebooted the controller in about 35 s). A failed allocation
  now drops the event or defers the receive (lwIP delivers the data again)
  instead of crashing, and the accept path allocates its client with
  `std::nothrow` and closes the connection instead of calling `abort()`.
  The guards are patched into the library at build time
  (`tools/patch_libs.py`).

## [2.1.1-revamped]

Hotfix of 2.1.0 for the ESP: the web server keeps its buffers again. The STM
firmware is unchanged; its image differs from 2.1.0 only in the version
string and needs no update.

### Changed
- ESP: the web server keeps its buffers (the handlers' working set, ~34 KB,
  and the response slots, 12 KB each) from the first request until the next
  restart, as before 2.1.0. With the other savings of 2.1.0 a WT32-ETH01
  keeps about 80 KB free after a visit of the dashboard.

### Fixed
- ESP: a controller on 2.1.0 restarted with a panic 14.5 h after the update,
  about a minute after a web request. The prime suspect is the release of the
  web buffers 30 s after the last request, which 2.1.1 withdraws; the cause
  is not confirmed (no backtrace).

## [2.1.0-revamped]

Review release: failsafe, persistence across restarts, rollback-proof Home
Assistant entities, a hardened HTTP API and many fixes of both firmwares. The
ESP runs on Ethernet only, without a web login and mDNS, gives the web
server's buffers back 30 s after the last request and restarts in a
controlled way when a leak keeps the heap below 12 KB. The new ESP needs an
STM **1.4.0** or newer. Upgrade order, rollback and the new recovery steps:
see INSTALL.md.

### Changed (breaking)
- ESP, HTTP API: every POST/DELETE on `/api/*` needs the header `X-VdMot: 1`,
  and a POST with a body needs `Content-Type: application/json`. Scripts and
  HA `rest_command`s must add them. `POST /setvalve` is exempt from the
  header.
- ESP, HTTP API: the `Host` header must be the device's IP address, its host
  name (`<name>` or `<name>.local`) or a name listed in `web.allowedHosts`;
  other names get `403 host_not_allowed`. A cross-site `Origin` gets
  `403 origin_not_allowed`.
- ESP, MQTT: the client id is `<host>-<mac6>` (e.g. `VdMot-a1b2c3`) or
  `mqtt.clientId`; broker ACLs keyed on the old id `VdMot` must be updated.
- ESP, MQTT: with **separate**, `valves/<V>/target` carries the target the STM
  confirmed; the requested value is in `valves/<V>/requested`.
- ESP, HA: the kept (legacy) entities no longer have an availability topic, as
  in the legacy firmware; the STM link has its own entities.
- ESP: the failsafe is **on** after the update (60 min, 50 %). MQTT users whose
  broker is often down get failsafe moves; set `failsafe.timeoutMin` to 0 to
  switch it off.
- STM: `stdet x` with x != 255 answers `stdet err` (was an empty `stdet`).
- ESP: Ethernet only: WiFi is gone, a controller reached over WiFi only has no
  network after the update (see Removed).
- ESP: no web login: every endpoint answers without credentials (see Removed).
- ESP: no mDNS: `<name>.local` resolves only through a local DNS (see Removed).

### Added
- ESP: failsafe lease (`slhbt`, `slcfg`, `sfspo`, `glcfg`) with the settings
  `failsafe.timeoutMin` and per-valve `valves.N.failsafePct`; the regulator is
  the MQTT broker (mode 1) or Home Assistant's status (mode 2); ESP emulation
  for STMs without protocol 3; events 314-318; dashboard notice while the
  failsafe is active.
- ESP: desired targets survive restarts (RTC memory, NVS at most 5 min after
  the last change and 30 min after the first unsaved one); event
  `targets_restored`.
- ESP: STM protocol 3 (`gvlvy`, `gstax`, `gtlnt`, `sstop`, `ssafe`): valve
  flags and fault, drive target, automatic retries, safe mode, UART and EEPROM
  counters; stop buttons and "Leave safe mode" in the dashboard, `STOP` over
  MQTT.
- ESP: STM minimum version 1.4.0: older STMs only get targets and `gvers`;
  dashboard banner and `409 stm_unsupported` for the other actions.
- ESP: board revision check in the STM flasher (`VDM-HW:C1`/`C2` marker against
  the running STM, board choice in blank mode, `409 board_mismatch` /
  `board_required`), blank-mode completion without a false error, one more
  session at 57600 baud after a failed sync.
- ESP: `GET /api/health`, network trial with
  `POST /api/system/network/confirm|revert`, file list and delete (`/api/files`),
  legacy import report (`/api/import-report`), `POST /api/valves/{n}/stop`,
  `POST /api/valves/stop`, `POST /api/stm/safe-mode/leave`, `?dryRun=1` and
  `restartRequired`/`netTrial` for `POST /api/config`.
- ESP: legacy HTTP aliases `GET /valves`, `/temps`, `/volts`, `POST /setvalve`;
  the other legacy paths answer `410 gone` with the replacement.
- ESP: new settings `web.allowedHosts`, `mqtt.rootTopic`, `mqtt.clientId`,
  `mqtt.discoveryPrefix`, `valves/temps/volts.N.topic` (topic overrides).
- ESP, MQTT: `valves/<V>/requested`, `sync`, `failsafe`, `problem`,
  `stm/status`, `failsafe`, `diag/stm/{version,started,lease,safeMode}`,
  `diag/mqtt/*`, `diag/calibration/next`, buttons under `cmd/`.
- ESP, HA: entities for failsafe, problem, target delivery, STM online, lease,
  safe mode, next calibration, calibrate/detect/reset/restart/stop buttons and
  an event entity; configurable discovery prefix; HA-safe ids.
- ESP: event aggregation (valve events of one code within 2 s become one MQTT
  event with `valves`) and priority classes for the MQTT rate limit.
- ESP: config backup in `/sys`, repair of single fields instead of defaults,
  events `config_restored`, `config_repaired`, `config_newer_schema`; newer
  keys kept for a downgrade.
- ESP: OTA validation by network, HTTP self-check and STM link; network
  watchdog that restarts the interface first; stack, heap-fragmentation and log
  write alarms.
- ESP: heap guard: when the free heap stays below 12 KB for 60 s (armed after
  10 min of uptime, not during an ESP update or an STM flash), the ESP logs
  event 122 `heap_critical` and restarts through the restart sequence
  (restart reason 6 `heap guard`); the desired targets are kept and the valves
  do not move.
- ESP: log lines start with `#<seq>`; missing lines are written as a gap line.
- ESP: event 415 `calib_stroke_short` and the health flag `strokeShort` when a
  calibration stroke is close to minCounts.
- STM: protocol 3, failsafe lease, calibrations and settings in EEPROM blocks,
  warm-reset restore, automatic retries of blocked valves, safe mode, board
  revision marker; see the STM changelog.
- Tooling: glue test harnesses for both firmwares, `tools/native/docker.sh`,
  mutation suites `stm32-glue` and `esp32-glue`, target 95 % per suite and per
  file.

### Changed
- ESP: assembly keeps target 100 until the next command (also across STM and
  ESP restarts) and is excluded from the failsafe.
- ESP: after an STM reboot every desired target is pushed once, also when the
  read-back already equals it.
- ESP: a scheduled calibration is booked only after the STM confirmed `staln`;
  retries every 10 min inside the 120 min window, then
  `scheduled_calibration_missed`. With a schedule the STM's own time trigger is
  switched off (`stlnt 0`).
- ESP: every ESP restart and every user STM reset waits (at most 10 s) until the
  STM has stored its EEPROM (`eepst`).
- ESP: a link interruption counts as an STM reboot only when `gstat`/`gstax`
  says so; `gproto` is probed again every 5 min while a revamped STM runs in
  protocol 1.
- ESP: sensor replies are matched by id; one failed read is bridged by the
  previous value; STM temperatures older than 200 s count as stale.
- ESP: `masns` after a sensor scan on STMs below protocol 2.
- ESP, MQTT: QoS 1 subscriptions for targets and HA status, persistent session
  (clean only after a topic change), back-off reset only after 60 s
  connected, retained commands cleared after processing, commands buffered
  while the app queue is full.
- ESP, MQTT: targets with a fraction (`43.7`, `43,7`) are rounded half up; the
  number form `1..12` and names with `/` work through the topic overrides;
  rejected commands are logged (throttled).
- ESP, MQTT: unnamed `temps/<T>` and `sensors/<S>` use the STM bus index again
  (as legacy); `sensors/<S>` is published for every configured slot.
- ESP, MQTT: `common/state` is `error` also in STM safe mode and at least `info`
  during a failsafe.
- ESP, HA: temperatures use `expire_after` and render `failed` as unknown;
  readable entity names; `hw_version` = STM board revision.
- ESP, HA: `/HADiscovery.cfg` is kept as the list of published topics (legacy
  format), so the legacy "delete discovery" removes the new entities after a
  rollback; every run prunes stale topics.
- ESP, HTTP: body limit 8 KB. Targets with a fraction are rounded half up.
- ESP: the event log file is written every 5 min, Warning+ within 10 s.
- ESP: factory reset by GPIO2 needs **5 s** at boot and works once per fitting of
  the jumper.
- ESP: network settings run on a 2 min trial; changes that do not need a restart
  (static fields under DHCP) no longer restart the ESP.
- ESP: the imported legacy syslog level 1..3 becomes level 3 (debug).
- ESP: a static IP without DNS uses the gateway as DNS.
- ESP: about 28 KB more free heap on the WT32-ETH01, every function kept.
  Static DRAM 64.3 -> 54.5 KB: the buffers of rare work (MQTT subscriptions,
  the discovery key, the legacy temps blob, the last_good copy, the HA event
  types) are built one at a time, borrowed or taken from the heap for the
  moment. Boot allocations 60.4 -> 48.0 KB: two whole config copies stay
  instead of seven, the other readers keep their parts or copy the config
  for the moment of a reload. The gateway ping session (~2.7 KB) exists only
  while its probe runs. Task stacks after a static analysis of the firmware:
  stm 6656, app 7168, mqtt 7168, AsyncTCP 8960 B (were 6144, 8192, 8192,
  10240 B). An open LittleFS file holds 1.4 KB instead of 5 KB (512 B stdio
  buffer instead of 4 KB).

### Fixed
- ESP: 2.1.0-revamped-rc1 did not start on hardware: the working
  objects allocated by static constructors ran out of heap before
  `initArduino()` released the Bluetooth DRAM, `abort()` in
  `do_global_ctors`, a reboot every 0.5 s. The first `bootAlloc()` now
  releases that DRAM itself.
- ESP: on a WT32-ETH01 the heap ran out once the network was up (0.6-2.4 KB
  left, Ethernet dropped received frames, the emac_rx task overflowed its
  stack). The event log RAM ring holds 32 events in the firmware build (the
  file keeps the history), the web server allocates its working set and its
  response slots in the first request, and AsyncTCP's task stack is 8960 B
  instead of 16 KB.
- ESP: the web server kept these buffers (~58 KB) until the next reboot, so
  one visit of the dashboard left a WT32-ETH01 with 28.9 KB of heap. It gives
  them back 30 s after the last request and allocates them again for the
  next one.
- ESP: the heap figures of `/api/health` and the `low_heap` and
  `heap_fragmented` alarms counted ~45 KB of IRAM that buffers cannot use;
  they count the 8-bit capable heap now.
- ESP: `POST /api/config` could answer 503 after it had already applied the
  change.
- ESP: the dashboard computed the volt value with another formula than MQTT.
- ESP: the dashboard lost the temp1/temp2 position of a valve's sensors.
- ESP: an old `app.js` could stay in the browser cache after an OTA.
- ESP: the HA temperature entities froze at the last value when a sensor failed.
- ESP: HA entities stayed orphaned after renames; a rollback left every entity
  unavailable.
- ESP: `svmov -1 err 1` was not parsed.
- ESP: late sensor replies could complete another request.

### Removed
- ESP: mDNS (`MDNS.begin`, the `_http._tcp` service): the device is reached by
  its IP address or a name a local DNS serves; the request guard still
  accepts `<name>` and `<name>.local`.
- ESP: WiFi: `net.iface` (the choice auto/Ethernet/WiFi and the WiFi fallback
  after 30 s without Ethernet), `net.ssid`, `net.wifiPassword`, the `wifi`
  state and `rssi` of `/api/status`, and the WiFi part of the network
  watchdog. `POST /api/config` accepts the old keys and ignores them; the
  legacy import ignores `netCfg/ethwifi`, `ssid` and `pwd`. The stored
  settings and the network trial record keep their layout: a rollback reads
  Ethernet without WiFi.
- ESP: the web login: HTTP Basic auth with `web.user`, `web.password` and
  `web.protectRead`, the per-address lockout (`429`), the `auth` member of
  `/api/status`, and the export with passwords (`?secrets=1` is ignored: an
  export never carries a secret). Events `auth_failed` (206) and
  `auth_locked` (214) are no longer raised; their numbers stay reserved.
  `POST /api/config` accepts the old keys and ignores them, so an older export
  still imports; the legacy import ignores `netCfg/userName` and
  `netCfg/userPwd`. The stored settings keep their layout: a rollback reads
  the login as off.
- ESP: `/HADiscovery.cfg` is no longer renamed to `.done`.
- ESP, HA: the entity `diag_stm_uptime` (replaced by `diag_stm_started`).

## [2.0.0-revamped] - 2026-09-24

First release of VdMot Revamped: hardened STM32 firmware with protocol v2 and a
new ESP32 firmware. Both stay compatible with the other side's 1.4.x firmware.
Upgrade order and rollback: see INSTALL.md.

### Added
- STM: UART protocol v2 with new commands only: `gproto`, `gvlvx` (extended
  valve data), `gprof` (current profile), `svmov` (service move), `scalx`/`gcalx`
  (breakaway escalation), `gstat` (uptime, resets, boot reason, dropped lines,
  EEPROM state), `gmotx` (motor parameter ranges).
- STM: per-move diagnostics (direction, requested/counted pulses, stop reason,
  peak current, duration), 32-point current profile of the last move, early
  end stop and rejected-command counters.
- STM: optional breakaway escalation for calibration repetitions, stored in a
  versioned EEPROM extension block (1.x images load defaults).
- STM: release envs `STM32F411_release_C1` and `STM32F411_release_C2`.
- ESP: new firmware `software_esp32_revamped` for the WT32-ETH01.
- ESP: dashboard (valves, sensors, events, settings, maintenance) that works on
  phones, with current-profile chart, service move dialog and light/dark theme.
- ESP: HTTP JSON API (`/api/*`) with optional HTTP Basic auth, brute-force
  lockout and optional read protection.
- ESP: structured event log (RAM ring, rotating LittleFS file, optional syslog),
  downloadable from the dashboard.
- ESP: target delivery with read-back verification and retry; targets re-sent
  after an STM restart.
- ESP: STM link supervision (timeouts, retries, restart detection, protocol
  detection with v1 fallback).
- ESP: STM flashing from the dashboard with image validation, chip-ID check,
  read-back verification and blank-chip mode; up to 3 stored images plus
  `last_good.bin`.
- ESP: OTA update with MD5 check and automatic rollback of an image that does
  not become healthy.
- ESP: MQTT topics `status` (LWT), `diag/valves/<V>/...`, `diag/stm/...`,
  `diag/calibration/active`, `events` (rate-limited), and `valves/<V>/actual`.
- ESP: Home Assistant discovery for the new diagnostic entities, availability
  topic on every entity.
- ESP: one-time import of the legacy NVS configuration.
- ESP: calibration schedule with minute offset, DST-safe, once per day.
- ESP: factory reset via GPIO2 at boot or API; config export/import.
- Native unit tests (doctest, ASan/UBSan) and mutation testing for both
  cores (STM 96.8 %, ESP 95.4 %).
- CI: native tests, mutation runs, builds of all STM envs and both ESP
  firmwares, size budget check, packaged releases on `v*-revamped` tags.

### Changed
- STM: end-stop thresholds use the learned mean current with a 15 mA floor
  (calibration closing strokes: at least 20 mA × factor).
- STM: the mean current is learned only from a successful calibration pass.
- STM: `staln` starts the calibration as soon as the valve is idle.
- STM: `stgtp` is accepted while a calibration is requested or running; the
  calibration ends at the newest target.
- STM: `smotc` checks every value against one range table (factors 10..40,
  41..50 applied as 40) and answers `smotc err` for out-of-range values.
- STM: `stlnm` range 0 (off) or 50..65534, also after a restart.
- STM: `eepst 1` only when the configuration is actually stored.
- STM: the watchdog is fed only while the valve state machine makes progress.
- ESP: the firmware no longer resets the STM when the ESP boots (with jumper X20
  fitted the IO15 strap can still do it, see INSTALL.md). The STM is reset only on
  user request, for flashing, or after a sustained link failure.
- ESP: HA discovery entities get `availability` and JSON escaping; invalid
  `device_class`/`state_class` on text/valve entities removed. Legacy
  unique_ids unchanged.
- ESP: web configuration requires HTTP Basic auth when a user and password are set.

### Fixed
- STM: failed calibrations stored the counts of the failed pass and reported
  the valve idle; now the valve is blocked and nothing is overwritten.
- STM: failed or blocked valves faked `actual = target` without moving; a move
  timeout reported the target as reached.
- STM: the 60 mA safety limit counted samples over the whole move, so short
  spikes caused false end stops.
- STM: calibration requests lost while a move of the same valve was running.
- STM: races between the valve ISR and the main loop (command/position handoff),
  blocking delay in ISR paths, soft-start handshake left set after a stop.
- STM: unbounded UART/terminal line handling, missing index checks, wrong
  integer widths, buffer sizes of reply formatters.
- STM: 1-Wire device counts unbounded, DS2438 all-zero pages accepted,
  endless search on a bad bus; I2C bus lock-up at start-up; EEPROM write/read
  failures not retried.
- ESP (new firmware, compared with the legacy one): valve targets acknowledged
  but dropped by the STM during calibration are now detected and re-sent.

### Removed
- ESP: PI/room temperature control, heat/cool modes, park position, window
  logic and their MQTT topics and HA entities (`climate`, `control/*`,
  `window/*`, `tTarget`, `tValue`, `heatControl`, `parkPosition`). Their HA
  discovery configs are deleted once.
- ESP: alarms and notifications (Pushover, e-mail).
- ESP: °F display (°C only) and the legacy web UI pages.
