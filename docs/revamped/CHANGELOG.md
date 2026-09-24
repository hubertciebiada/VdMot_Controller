# Changelog: VdMot Revamped

Unofficial fork of VdMot_Controller. Versions carry the suffix `-revamped`.
Base: `hc-version` (upstream `developer` 1.4.12 + fixes).

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
- ESP: the ESP no longer resets the STM when it boots. The STM is reset only on
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
