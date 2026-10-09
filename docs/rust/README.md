# VdMot Revamped in Rust

The Rust tree is a port of VdMot Revamped 2.1.7 to Rust, for both MCUs: the ESP32 (WT32-ETH01:
network, dashboard, MQTT, STM link) and the STM32 (BlackPill F401/F411: valve motors, 1-Wire,
EEPROM). It is a port, not a redesign: the external contracts of 2.1.7 stay byte for byte (the
HTTP API, MQTT and Home Assistant, the UART protocol, the settings in NVS and LittleFS, the STM
EEPROM and warm state), so the C++ and the Rust firmware replace each other in both directions.
The rules are in [PORTING.md](PORTING.md); every difference from 2.1.7 is listed in
[CHANGES.md](CHANGES.md). The first Rust release is `2.2.0-revamped` on both MCUs (decision D8 of
[GLUE-DESIGN-STM.md](GLUE-DESIGN-STM.md#8-open-decisions-and-risks)), so `gvers` and the dashboard
tell it from C++ 2.1.7. The C++ sources (`software_esp32_revamped`, `software_stm32`) left the
branch after the Rust firmware had run on the controllers: they are on the branch `revamped` and
in the tag `v2.1.7-revamped`, and the paths of C++ files in the comments and documents of the
port name those sources. The cross checks against the C++ 2.1.7 take its code from the tag
(`tools/rust/cpp217.sh`).

Installing it on a controller: [INSTALL.md](INSTALL.md).

## Documents

| Document | Content |
|---|---|
| [PORTING.md](PORTING.md) | porting rules, layout, idioms, the mutation gate |
| [PORT-NOTES.md](PORT-NOTES.md), [PORT-NOTES-STM.md](PORT-NOTES-STM.md) | C++ behaviour kept although it looks odd, C++ bugs fixed on purpose, C++ tests without a Rust form |
| [GLUE-DESIGN-ESP.md](GLUE-DESIGN-ESP.md) | ESP32 glue and firmware: ports, threads, memory, HTTP, tests, the boot guard, decisions |
| [GLUE-DESIGN-STM.md](GLUE-DESIGN-STM.md) | STM32 glue and firmware: real-time model, persistence, boot and flashing safety, Renode, decisions and risks |
| [CHANGES.md](CHANGES.md) | what differs from C++ 2.1.7, for users |
| [PARITY.md](PARITY.md), [PARITY-TESTS.md](PARITY-TESTS.md) | the audit of the port against C++ 2.1.7: every C++ test case and every external feature with its Rust code and tests, the open gaps |
| [INSTALL.md](INSTALL.md) | first flash and rollout on a controller |
| [REVIEW-ESP-SAFETY.md](REVIEW-ESP-SAFETY.md), [REVIEW-STM-SAFETY.md](REVIEW-STM-SAFETY.md) | the safety reviews against the closed-cabinet bar: every path of boot, update and STM flashing that could need physical access, the findings with their fixes or reasons to stay open |
| [mutation-esp32-core.md](mutation-esp32-core.md), [mutation-esp32-glue.md](mutation-esp32-glue.md), [mutation-stm32.md](mutation-stm32.md) | the mutation reports: the score of every file, the runs and their trees, the equivalent mutants with reasons, the survivors and the cases that killed them, the timeouts run again alone |
| [tools/rust/README.md](../../tools/rust/README.md) | the container scripts, their images and options |

## Layout

| Path | Content |
|---|---|
| `software_esp32_rust/core` | `vdm-esp-core`: port of `software_esp32_revamped/lib/core`, `no_std`, no allocation |
| `software_esp32_rust/glue` | `vdm-esp-glue`: the logic of `software_esp32_revamped/src` over port traits, tested on the host against fakes |
| `software_esp32_rust/firmware` | `vdm-esp-fw`: ESP-IDF adapters of the ports and `main` (target `xtensa-esp32-espidf`, own toolchain; outside the host workspace) |
| `software_stm32_rust/core` | `vdm-stm-core`: port of `software_stm32/lib/core` |
| `software_stm32_rust/boot` | `vdm-stm-boot`: boot window, reset capture, ID block, fault record |
| `software_stm32_rust/glue` | `vdm-stm-glue`: the logic of `software_stm32/src` over HAL traits |
| `software_stm32_rust/image-check` | `vdm-stm-image-check`: layout, size and sector-0 checks of the images |
| `software_stm32_rust/firmware` | `vdm-stm-fw`: embassy adapters, boot stage registers, linker scripts of the four images (target `thumbv7em-none-eabihf`; outside the host workspace) |
| `software_stm32_rust/renode` | the Renode machine, board models and suites of the images |
| `tools/rust` | the container scripts (below), the mutation gate and its lists of equivalent mutants, the QEMU harness, the STM32 build, image check and E11 programs |
| `tools/release/package.py` | the release assets of the C++ and of the Rust firmware |

## Build, test, gate, emulate

Docker is the only requirement; every command runs in a container from the repository root
(Windows, macOS, Linux; on Windows in Git Bash). The options and the images are in
[tools/rust/README.md](../../tools/rust/README.md).

| Step | Command |
|---|---|
| host tests of a workspace | `bash tools/rust/docker.sh test software_esp32_rust` (or `software_stm32_rust`) |
| rustfmt and clippy `-D warnings` | `bash tools/rust/docker.sh lint software_esp32_rust` |
| tests that need Mosquitto and `g++ -m32` | `bash tools/rust/docker.sh interop software_esp32_rust` |
| mutation gate of a package | `bash tools/rust/docker.sh mutate software_esp32_rust vdm-esp-glue` |
| STM32 images | `bash tools/rust/docker.sh fw` |
| image check C1-C5, D9 (C6 with the C++ 2.1.7 images in `<dir>`) | `bash tools/rust/docker.sh image-check [<dir>]` |
| Renode E1-E10, A1-A6 | `bash tools/rust/renode.sh [STM32F401_C2 ...]` |
| Renode E11 (C++ -> Rust -> C++ with `--cpp <dir>`) | `bash tools/rust/renode.sh --e11 [--cpp <dir>]` |
| ESP32 image, size against the budget | `bash tools/rust/esp/docker.sh size` |
| ESP32 release images into `software_esp32_rust/firmware/images/` | `bash tools/rust/esp/docker.sh export` |
| clippy of the ESP32 firmware | `bash tools/rust/esp/docker.sh lint` |
| QEMU harness | `bash tools/rust/esp/docker.sh qemu [scenario ...]` |
| release assets from local builds | `python3 tools/release/package.py --tag v2.2.0-revamped --from-builds . --out dist` |

The packaging takes the STM32 images of `docker.sh fw` and the ESP32 images of
`esp/docker.sh export`. It refuses a tag that is not the version of the Rust firmware
(`software_stm32_rust/Cargo.toml`, `VDM_VERSION` in `software_esp32_rust/firmware/.cargo/config.toml`
and the version of `software_esp32_rust/firmware/Cargo.toml`) and images that do not carry that
version, and writes the asset set of the C++ releases under the same names.

## CI

`.github/workflows/build.yml` runs the commands above (jobs `rust-*`) next to the C++ jobs:

| Job | Runs | When |
|---|---|---|
| `rust-host` | per workspace: `test`, `lint`, `interop` | pushes, pull requests, tags |
| `rust-stm` | `fw`, `image-check` with the C++ 2.1.7 STM32 images of the release v2.1.7-revamped (C6; their SHA-256 are pinned in `tools/rust/stm/cpp217.sha256`) | pushes, pull requests, tags |
| `rust-renode` | per image: `renode.sh`, then `renode.sh --e11 --cpp` | pushes, pull requests, tags |
| `rust-esp` | `esp/docker.sh export`, `esp/docker.sh lint` | pushes, pull requests, tags |
| `rust-esp-qemu` | `esp/docker.sh qemu` with the pinned C++ 2.1.7 ESP32 image | weekly, manual (input `qemu`) |
| `rust-mutation`, `rust-mutation-gate` | `docker.sh mutate` per package, the large packages in shards; one 95 % per-file gate per package over all its shards | weekly, manual (input `rust_mutation`) |
| `release-firmware`, `release-rust` | `package.py`, GitHub release | a `v*-revamped` tag, or the manual input `release_tag`, with the version of the Rust firmware |

The pushes are those of the branches `hc-version`, `revamped` and `revamped-rust`, the pull
requests those into them.

A newer push to a branch cancels the run it supersedes; tags, the weekly and the manual runs
always finish. GitHub starts the weekly (`schedule`) and the manual (`workflow_dispatch`) runs only
when the workflow file is on the repository's default branch; pushes, pull requests and tags work
on every branch.

## What is proven where

| Where | What it proves | Details |
|---|---|---|
| host tests | every test case of the ported C++ modules with all its assertions, new cases for the Rust mechanisms (ports, boot guard, HTTP and MQTT protocol code), differential checks that run the C++ and the Rust code on the same random inputs | [PORTING.md](PORTING.md#modules-and-tests), [PORT-NOTES.md](PORT-NOTES.md) |
| STM32 goldens | the STM32 glue reproduces the UART bytes, EEPROM rows and no-init bytes of every C++ glue_system case, byte for byte | [GLUE-DESIGN-STM.md §7.4](GLUE-DESIGN-STM.md#74-cross-implementation-goldens) |
| interop tests | `json_body` gives what ArduinoJson 6.21.6 gives on the device's 32-bit layout; `mqtt_conn` works against Mosquitto | [GLUE-DESIGN-ESP.md §4.4](GLUE-DESIGN-ESP.md#44-json-bodies), [§7](GLUE-DESIGN-ESP.md#7-open-risks-and-decisions) item 11 |
| mutation gate | the tests kill at least 95 % of the mutants of every file; on the final tree every file of the gated crates (ESP core and glue, STM core, boot and glue) is at 100 % | [PORTING.md](PORTING.md#mutation-gate), [mutation-esp32-core.md](mutation-esp32-core.md), [mutation-esp32-glue.md](mutation-esp32-glue.md), [mutation-stm32.md](mutation-stm32.md) |
| image check | the ESP 2.1.7 accepts every STM32 image (chip, board, version, handshake, erase set), the layout of each image, the boot stage inside sector 0 (D9), the reference values on the C++ 2.1.7 images (C6) | [GLUE-DESIGN-STM.md §5.8](GLUE-DESIGN-STM.md#58-byte-level-image-check) |
| Renode | per STM32 image: the boot window (both ESP handshakes, stray bytes, HSE dead, faults, no-init cells), the application against the C++ goldens, warm start from C++ 2.1.7 state, the watchdog, a flash cycle C++ -> Rust -> C++ with the Rust ESP flasher (E11) | [GLUE-DESIGN-STM.md §5.7](GLUE-DESIGN-STM.md#57-proof-in-the-emulator-renode) |
| QEMU | the ESP32 image with the devices' bootloader and partition table: trial, confirmation, boot limit, boot deadline, switch back, OTA to the C++ image, the C++ NVS and LittleFS read and written, HTTP, MQTT, load | [GLUE-DESIGN-ESP.md §5.5](GLUE-DESIGN-ESP.md#55-end-to-end-in-qemu-vdm-esp-fw) |

Only hardware proves:

- ESP32: a boot that loops before `main` (the boot guard cannot catch it, and QEMU cannot run a
  restart after the first minute of uptime), the LAN8720 and WiFi, the C++ 2.1.7 firmware
  uploading the Rust image and reading the settings and files the Rust image wrote, the stack and
  heap figures of the device
  ([GLUE-DESIGN-ESP.md §5.5](GLUE-DESIGN-ESP.md#55-end-to-end-in-qemu-vdm-esp-fw), "Not provable
  in QEMU"; [§2.1](GLUE-DESIGN-ESP.md#21-threads-d3), [§2.3](GLUE-DESIGN-ESP.md#23-ram-budget-no-psram-263-kb-of-8-bit-heap)).
- STM32: the ROM bootloader, the flash interface and its timing, the NRST pulse, the HSE start-up,
  UART noise, the motor currents, 1-Wire and I2C timing on the wire (R5), the reset flags and the
  fault escalation of the chip, the warm state across a real flash (R3)
  ([GLUE-DESIGN-STM.md §5.7](GLUE-DESIGN-STM.md#57-proof-in-the-emulator-renode), "What Renode
  does not prove"; [§5.9](GLUE-DESIGN-STM.md#59-proof-on-hardware)).

[INSTALL.md](INSTALL.md) turns these into the checks of the first flash: the first-flash checklist
of the ESP32 and the bench proof of the STM32 (D10).
