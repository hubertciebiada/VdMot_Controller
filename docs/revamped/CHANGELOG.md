# Changelog: VdMot Revamped

Unofficial fork of VdMot_Controller. Versions carry the suffix `-revamped`.
Base: `hc-version` (upstream `developer` 1.4.12 + fixes).
The STM entries of each release are in
[software_stm32/ChangeLog.md](../../software_stm32/ChangeLog.md); this file lists
the ESP, the tooling and a summary of the STM.

## [2.1.0-revamped]

Review release: failsafe, persistence across restarts, rollback-proof Home
Assistant entities, a hardened HTTP API and many fixes of both firmwares. The
new ESP needs an STM **1.4.0** or newer. Upgrade order, rollback and the new
recovery steps: see INSTALL.md.

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
- ESP: `GET /api/health` (public), network trial with
  `POST /api/system/network/confirm|revert`, config export with passwords
  (`?secrets=1`, only with web login), file list and delete (`/api/files`),
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
- ESP, HTTP: failed logins lock per client address (1, 5, 15 min); 429 with
  `Retry-After`. Body limit 8 KB. Targets with a fraction are rounded half up.
- ESP: the event log file is written every 5 min, Warning+ within 10 s.
- ESP: factory reset by GPIO2 needs **5 s** at boot and works once per fitting of
  the jumper.
- ESP: network settings run on a 2 min trial; changes that do not need a restart
  (static fields under DHCP, WiFi fields on Ethernet) no longer restart the ESP.
- ESP: the imported legacy syslog level 1..3 becomes level 3 (debug).
- ESP: a static IP without DNS uses the gateway as DNS.

### Fixed
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
