# Changelog
All notable changes to this project will be documented in this file.

## [Unreleased] - Development

### Breaking Changed

### Changed

### Fixed

### Removed

## [2.1.0-revamped]
Unofficial VdMot Revamped release (see docs/revamped). Protocol 3; details in
PROTOCOL_V2.md. The v1 request and reply bytes are unchanged except `stdet x`
(x != 255), which now answers `stdet err`.

### Added
- Failsafe lease: `slhbt`, `slcfg`, `sfspo`, `glcfg`. Without a renewal for the
  lease timeout (default 60 min, stored; 0 = off) every valve goes to its
  failsafe position (default 50 %, per valve 0..100 or 255 = hold). The legacy
  ESP renews it with its `gvlvd` polls.
- Protocol 3 read commands: `gvlvy` (valve flags, fault, failsafe position,
  drive target, automatic retries), `gstax` (lease, safe mode, UART error
  counters, EEPROM repairs and writes, temperature age, protection state),
  `gtlnt` (stored learn time); `gproto` answers 3. Replies may get more fields
  in later versions.
- `sstop <idx|255>`: stops a running move, calibration or service move and
  cancels requested calibrations; the valve stays where it stopped.
- Safe mode: 3 watchdog resets within 10 min stop every valve movement until
  `ssafe 0`, a power-on or 30 min of uptime.
- Blocked valves are driven to their failsafe position and calibrated again
  automatically after 1 h, 6 h, then every 24 h.
- Short-circuit check in the presence test and an inrush limit at the start of
  every stroke (fault 3 / 5). Report-only by default (`kProtectEnforce` in
  lib/core `protection_guard.h`); trips on 3 valves within 10 min suspend both
  checks until the next start.
- Calibration records (counts, mean current) per valve in their own EEPROM
  blocks; a copy of the 1.x motor fields and the failsafe settings in a CRC
  block; a CRC over the 1.x layout. A torn or corrupt block is repaired from
  the others (`gstax` cfgFlags / cfgEvents).
- Warm reset (reset pin, software, watchdog) keeps position, target and status
  of every valve: no presence test, no movement. After a power-on the first
  move of a calibrated valve is a reference move.
- Board revision marker `VDM-HW:C1` / `VDM-HW:C2` in every image (checked by the
  ESP flasher).
- UART error counters (overrun, framing, noise, dropped bytes) in `gstax`.

### Changed
- The learn time (`stlnt`) is stored in the EEPROM and counts real seconds; with
  0 stored and no lease client for 24 h the time trigger runs every 7 days
  anyway (a rolled-back ESP never leaves the valves without calibration).
- A partial move that ends at an end stop takes its position from the counted
  pulses; an end stop before 80 % of the requested pulses is an early stop,
  with one immediate retry per target.
- Valves without contact (open circuit) keep their position, are not driven to
  the failsafe and are tested again when their target changes.
- Every accepted `stgtp` of an uncalibrated or unreferenced valve starts its
  calibration or reference move, also when the target did not change.
- The calibration requirement no longer depends on the valve status (a valve
  found at start-up calibrates at its first target).
- Targets have priority over waiting calibrations.
- Movement counters (`moves`) and the movement trigger restart after every
  successful calibration.
- The EEPROM is written only when a stored value changed, 3 s after the last
  change and at most 30 s after the first unsaved one; the I2C bus is recovered
  before every retry; the 1.x sensor slots are merged one by one.
- Temperatures: one retry on a failed read, the last good value is held for 2
  cycles, 85.0 °C only after a reading of at least 75.0 °C, the bus is
  enumerated again every 24 h; no temperature conversions during a series of
  moves.
- Debug terminal: `sena` only while no valve moves, switched off after 2 s or
  above 60 mA; `seteep` refused while the EEPROM is unreadable and written
  through the normal path; `testmode` and `stm` removed; `stdet` answers only
  on the debug port.
- One OneWire implementation; `rs485.cpp` is no longer built.

### Known limitations
- Coast pulses after the motor enable is switched off are not counted (TODO:
  to be measured on the device, see PROTOCOL_V2.md).
- The short and inrush limits are tuning values that still need a measurement
  on the hardware; until then they only report.

## [2.0.0-revamped]
Unofficial VdMot Revamped release (see docs/revamped). Protocol 2 (new commands
`gproto`, `gvlvx`, `gprof`, `svmov`, `scalx`, `gcalx`, `gstat`, `gmotx`), bounded
UART and terminal line handling, calibration fixes (end-stop thresholds with a
floor, no counts from a failed pass, blocked instead of idle), move diagnostics,
optional breakaway escalation, F411 release envs. Details in PROTOCOL_V2.md.

## [1.4.9]
### Fixed
- System :     valves detection went sometimes wrong

## [1.4.8]
### Changed
- System :      set the valves to previous % state after calibration

## [1.4.0]
### Changed
- System :      add DS2438 for voltage sensors

## [1.3.7]
### Fixed
- Config :      fix bug valve 12 set tIdx1 and tIdx2 was not saved

## [1.3.6]
### Changed
- rework motor end detection (filtering of motor current, no soft PWM for motor turn on anymore)

## [1.3.4]
### Changed
- faster motor end detection
### Added
- min. counts in eeprom
- calibration retries if counts less than min. counts in config

## [1.3.3]
### Added
- get chip id

## [1.3.2]
### Changed
- led heartbeat : change frequency 3s off / 100ms on

## [1.3.1]
### fixed
- next calibration : movements set to min. 50

## [1.3.0]
### Fixed
- calibration move numbers was not set from eeprom
- calibration function was not correct
### Added
- send calibration state to WTH32

## [1.2.2]
### Add
- send valve movements, open counts, close counts, dead counts with cmd APP_PRE_GETVLVDATA

## [1.1.0]
### Changed
- new command : APP_PRE_EEPSTATE : gets the state of eeprom (1 = ready, 0 = write pending)
- disable motDebug also in DEV to prevent handling UART in interrupt routines
- set default motor low / high to 17 (1.7) 

## [1.0.9]
### Fixed
- UART : max read length for UART buffer is limited to 990 now, not unlimited anymore
- valves : valve_loop() is now called via timer interrupt, this hopefully ensures correct motor drive and stopping during high system loads

## [1.0.8]
### Changed
- Softreset timing changed

## [1.0.7] 
### Added
- new STM32 commands

### Fixed
- fixed problems with configuration of 1-wire sensors (matching to valves) 
