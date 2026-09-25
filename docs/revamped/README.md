# VdMot Revamped

> **Unofficial.** VdMot Revamped is an independently maintained fork of the
> [VdMot_Controller](https://github.com/Lenti84/VdMot_Controller) firmware
> (Lenti84 / SurfGargano). It is not an official release and is not supported by
> the upstream project. Every artifact carries the suffix `-revamped`, so it
> cannot be confused with the official firmware.

Version: **2.1.0-revamped** (ESP32 and STM32).
Base: branch `hc-version` = upstream `developer` 1.4.12 plus the owner's fixes,
a pinned toolchain and CI.

## What it is

The VdMot controller has two MCUs: an STM32 (BlackPill F401/F411) that drives
the 12 valve motors and reads the 1-Wire sensors, and an ESP32 (WT32-ETH01)
that provides network, MQTT and the web UI and talks to the STM over UART.

VdMot Revamped changes both:

| Part | Upstream 1.4.x | VdMot Revamped |
|---|---|---|
| STM32 | valve control, calibration | same job, hardened; calibration fixes; move diagnostics; failsafe lease; settings and calibrations kept in the EEPROM; protocol 3 (new commands only) |
| ESP32 | `software_esp32` (PI controller, window logic, alarms, web UI) | new firmware `software_esp32_revamped`: precise valve control, failsafe, dashboard, diagnostics, event log, HA discovery. No PI controller, no alarms |

Compatibility between the two halves:
- the revamped STM works with the old ESP: the v1 request and reply bytes are
  unchanged (one exception: `stdet x` with x != 255 answers `stdet err`), new
  data comes only through new commands;
- the new ESP works with every STM from **1.4.0** on (it detects protocol 1, 2
  or 3 and uses only what the STM supports). An STM below 1.4.0 only gets
  targets; the dashboard asks to update it.

## What changed in 2.1

The full list is in [CHANGELOG.md](CHANGELOG.md) (ESP) and
[software_stm32/ChangeLog.md](../../software_stm32/ChangeLog.md) (STM). The main points:

### Safety
- **Failsafe when the regulator goes silent.** The ESP renews a lease on the STM
  while its regulator (the MQTT broker, and Home Assistant in HA mode) is alive.
  Without renewal for `failsafe.timeoutMin` (default **60 min**) every active
  valve goes to its failsafe position (default **50 %**, per valve 0..100 or
  "hold"). STM 2.1 does this on its own, also when the ESP is dead; with an
  older STM the ESP emulates it. MQTT off counts as alive. Event
  `failsafe_active`, MQTT `failsafe`, HA "Failsafe active".
- **Blocked valves** go to their failsafe position instead of staying where the
  failed calibration left them, and are calibrated again automatically after
  1 h, 6 h, then every 24 h. A blocked or jammed valve is never moved or
  calibrated in a loop.
- **Short and inrush detection** (STM): a presence-test short and an inrush
  above the limit are detected and reported (`gvlvy` fault 3 / 5). By default
  they are **report-only**: the valve keeps its status. Enforcement is one build
  switch (`kProtectEnforce` in `software_stm32/lib/core/include/vdm/protection_guard.h`),
  to be enabled after a hardware measurement of the limits.
- **STM safe mode:** 3 watchdog resets within 10 min stop the motors until
  `ssafe 0`, a power-on, or 30 min of uptime.
- **HTTP guard:** API writes need the header `X-VdMot: 1` and JSON content; the
  `Host`/`Origin` must name the device; failed logins lock per client address.

### Robustness
- Targets survive restarts: the STM keeps position, target and status over a
  warm reset (reset pin, software, watchdog) and stores calibrations in its
  EEPROM; the ESP keeps the desired targets in RTC memory and NVS.
- Every ESP restart first waits (at most 10 s) until the STM has written its
  EEPROM.
- A new ESP image is kept only after it proved network, HTTP and (when it was
  there at the upload) the STM link; otherwise the bootloader rolls back.
- Network changes run on trial: without "Keep" within 2 min the old settings
  come back.
- The network watchdog first restarts the interface (5 min) and only then the
  ESP (10 min).
- The STM flasher checks the board revision (C1/C2 marker in the image against
  the running STM), completes blank-mode flashes and falls back to 57600 baud.
- The config is saved with a backup and repaired field by field instead of
  falling back to defaults.

### MQTT and Home Assistant
- Legacy topic tree, legacy unique_ids and the legacy root `VdMotFBH` for an
  empty station; per-item topic overrides for legacy names with `/`.
- New topics: `requested`, `sync`, `failsafe`, `problem` per valve, `stm/status`,
  `failsafe`, buttons under `cmd/`; `STOP` payload (STM 2.1).
- Kept entities have no availability, so a rollback to the legacy firmware
  leaves them working. New entities: failsafe, problem, target delivery, STM
  link, calibrate buttons, an event entity.
- QoS 1 for target commands and HA status, persistent session, client id
  `<host>-<mac6>`.

See [MQTT.md](MQTT.md) and [API.md](API.md) for the details.

## Dashboard preview

`python3 software_esp32_revamped/tools/mock_api.py` serves the dashboard with simulated data on a
local port, without a controller. `--proto 3 --scenario health --import-report` adds a blocked valve
at its failsafe position, an unconfirmed target, stale data and the legacy import report.

## Testing

- **Native unit tests** (host, doctest, `-Wall -Wextra -Werror`, AddressSanitizer +
  UBSan) for all decision logic, which lives in hardware-free cores:
  `software_stm32/lib/core` and `software_esp32_revamped/lib/core`. Parsers get
  fixed-seed random-input tests.
- **Glue suites:** the Arduino glue of both firmwares (`src/*.cpp`) is built on
  the host against fakes of the hardware and libraries (`tools/native/testkit`)
  and tested there, including multi-boot scenarios.
- **Mutation testing** (`tools/mutation/mutate.py`): target **95 %** overall and
  for every file, for the four suites `stm32`, `stm32-glue`, `esp32`,
  `esp32-glue` (mutants that are stillborn are not counted). Reports:
  [mutation-stm32.md](mutation-stm32.md), [mutation-esp32.md](mutation-esp32.md).
- All suites run the same everywhere in a container:
  `bash tools/native/docker.sh test stm32|esp32`,
  `bash tools/native/docker.sh mutate <suite>`.
- **Build checks** of every STM env and both ESP firmwares; ESP image size budget
  1.2 MB (partition 1.25 MB).
- On-device behaviour: manual checklist in [INSTALL.md](INSTALL.md#4-after-the-upgrade-checklist).
  Items marked **[HW]** there still need a measurement on a device.

## CI/CD

`.github/workflows/build.yml` (pinned PlatformIO 6.1.19, esptool 4.11.0, platforms and libraries):

| Job | When | What |
|---|---|---|
| native | push / PR | native tests of both cores, mutation tool self-test |
| mutation | push / PR (changed files), weekly and manual (all) | informational, does not block a release |
| stm32 | push / PR | `STM32_release_C1/C2`, `STM32F411_release_C1/C2` |
| esp32 | push / PR | revamped + legacy ESP, digest-less image, size check |
| release | tag `v*-revamped` | `tools/release/package.py`, GitHub Release (pre-release for `-rc`) |
| release-legacy | other `v*` tags | legacy binaries as before |

## Branches

| Branch | Content |
|---|---|
| `revamped` | VdMot Revamped: STM firmware, new ESP firmware `software_esp32_revamped`, native tests and harness, mutation configs, CI, release packaging, these docs. Built on `hc-version`; the only branch of this project |
| `hc-version` | base: upstream `developer` 1.4.12 plus the owner's fixes |

Fixes for the legacy `software_esp32` (no features) are kept apart on
`legacy/esp-fixes`.

## Documentation

- [INSTALL.md](INSTALL.md): building, release assets, upgrade, rollback, recovery
- [MQTT.md](MQTT.md): topics and Home Assistant entities
- [API.md](API.md): HTTP JSON API
- [CHANGELOG.md](CHANGELOG.md)
- [software_stm32/PROTOCOL_V2.md](../../software_stm32/PROTOCOL_V2.md): UART protocol 2 and 3
- [software_esp32_revamped/DESIGN.md](../../software_esp32_revamped/DESIGN.md): ESP design (internals)
- [mutation-stm32.md](mutation-stm32.md), [mutation-esp32.md](mutation-esp32.md)
