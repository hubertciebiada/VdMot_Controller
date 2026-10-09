# VdMot Revamped

> **Unofficial.** VdMot Revamped is an independently maintained fork of the
> [VdMot_Controller](https://github.com/Lenti84/VdMot_Controller) firmware
> (Lenti84 / SurfGargano). It is not an official release and is not supported by
> the upstream project. Every artifact carries the suffix `-revamped`, so it
> cannot be confused with the official firmware.

The firmware of both MCUs is written in Rust since **2.2.0-revamped**
([docs/rust/README.md](../rust/README.md); installation
[docs/rust/INSTALL.md](../rust/INSTALL.md); every difference from the C++ 2.1.7 in
[docs/rust/CHANGES.md](../rust/CHANGES.md)). The C++ firmware ended with 2.1.7-revamped
(branch `revamped`, release `v2.1.7-revamped`), the way back from the Rust one. This page
describes what VdMot Revamped does; the user-facing behaviour is the same in both.

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
| ESP32 | `software_esp32` (PI controller, window logic, alarms, web UI) | new firmware (`software_esp32_rust`, in C++ up to 2.1.7): precise valve control, failsafe, dashboard, diagnostics, event log, HA discovery. No PI controller, no alarms |

Compatibility between the two halves:
- the revamped STM works with the old ESP: the v1 request and reply bytes are
  unchanged (one exception: `stdet x` with x != 255 answers `stdet err`), new
  data comes only through new commands;
- the new ESP works with every STM from **1.4.0** on (it detects protocol 1, 2
  or 3 and uses only what the STM supports). An STM below 1.4.0 only gets
  targets; the dashboard asks to update it.

## What changed in 2.1

The full list is in [CHANGELOG.md](CHANGELOG.md) (ESP) and
[CHANGELOG-STM.md](CHANGELOG-STM.md) (STM). The main points:

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
  `Host`/`Origin` must name the device. There is no login: the controller
  belongs in a private network.

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

`python3 software_esp32_rust/tools/mock_api.py` serves the dashboard with simulated data on a
local port, without a controller. `--proto 3 --scenario health --import-report` adds a blocked valve
at its failsafe position, an unconfirmed target, stale data and the legacy import report.

## Testing

Host tests of every module, the mutation gate (95 % for every file; every file of the gated
crates is at 100 %), Renode runs of the four STM32 images, the QEMU harness of the ESP32
image, the cross checks against the C++ 2.1.7 and the safety reviews: [docs/rust/README.md](../rust/README.md).
The steps on a controller: [docs/rust/INSTALL.md](../rust/INSTALL.md).

## CI/CD

`.github/workflows/build.yml`, the scripts of `tools/rust` (the same commands as on a
workstation; [tools/rust/README.md](../../tools/rust/README.md)):

| Job | When | What |
|---|---|---|
| rust-host | push / PR | host tests, rustfmt, clippy, interop tests of both workspaces |
| rust-stm | push / PR | the four STM32 images, image check C1-C6 and D9 |
| rust-renode | push / PR | Renode E1-E11 and A1-A6 per STM32 image |
| rust-esp | push / PR | the ESP32 image, size budget, digest-less copy, clippy of the firmware |
| rust-esp-qemu | weekly and manual | the QEMU harness of the ESP32 image |
| rust-mutation | weekly and manual | cargo-mutants and the 95 % gate per file |
| release-rust | tag `v*-revamped` | `tools/release/package.py`, GitHub Release (pre-release for `-rc`) |
| esp32-legacy | push / PR | the legacy ESP firmware of the upstream project, as before |

## Branches

| Branch | Content |
|---|---|
| `revamped-rust` | VdMot Revamped from 2.2.0 on: the Rust firmware of both MCUs, the dashboard, tools, CI, release packaging, these docs |
| `revamped` | VdMot Revamped in C++ up to 2.1.7-revamped (ended): STM firmware, ESP firmware `software_esp32_revamped`, native tests, mutation configs. Built on `hc-version` |
| `hc-version` | base: upstream `developer` 1.4.12 plus the owner's fixes |

Fixes for the legacy `software_esp32` (no features) are kept apart on
`legacy/esp-fixes`.

## Documentation

- [INSTALL.md](INSTALL.md): the C++ 2.1.x guide: building, release assets, upgrade, rollback,
  recovery (the Rust one: [docs/rust/INSTALL.md](../rust/INSTALL.md))
- [MQTT.md](MQTT.md): topics and Home Assistant entities
- [API.md](API.md): HTTP JSON API
- [CHANGELOG.md](CHANGELOG.md), [CHANGELOG-STM.md](CHANGELOG-STM.md): the releases up to 2.1.7
- [PROTOCOL_V2.md](PROTOCOL_V2.md): UART protocol 2 and 3
- [DESIGN.md](DESIGN.md): ESP design (internals)
- [web-server-stability.md](web-server-stability.md): the C++ ESP web server under concurrent connections: the AsyncTCP panic, the 2.1.3 fix, the residual, the next step
