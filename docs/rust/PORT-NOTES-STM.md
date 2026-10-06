# VdMot Revamped STM32 in Rust: port notes

C++ behaviour of `software_stm32/lib/core` that the Rust port (`software_stm32_rust/core`) keeps
although it looks odd, and the places where the C++ has no defined behaviour to keep. Each entry:
module, what, why it matters. The Rust tests assert the C++ behaviour.

| module | what | why it matters |
|---|---|---|
| presence_test | `sample()` takes the magnitude with `v < 0 ? -v : v` on an `int32_t`: undefined for `INT32_MIN` (in practice it wraps to a negative value and counts as no current). The Rust computes it in `i64`, so `i32::MIN` counts as a large current. | None in practice: the glue feeds the filtered current of the end-stop detector, which never leaves ±100000. |
| move_classifier | `positionAfterEndStop()` is documented as "clamped to 0..100", but a close from a start above 100 returns `start - delta` unclamped (start 150, delta 10 -> 140); only an open is capped at 100. The Rust does the same; test_move_classifier__mut.cpp checks it. | None in practice: the start is the believed position, always 0..100. A caller that passes an unchecked start gets a position above 100. |

## Glue: communication (`src/communication.cpp`)

| what | why it matters |
|---|---|
| The debug lines of `gtlnm` and of a valid `gvlon` end without CR LF (`commdbg_print`), so the next debug line continues them. Kept. | Debug port only; the ESP replies are not affected. |
| `printSensorAddress` prints "00-00-00-00-00-00-00-00" when the address does not fit its buffer; the 24-byte buffer always holds the 23 characters, so the fallback is dead code. Kept 1:1. | None. |

## Glue: eeprom (`src/eeprom.cpp`)

| what | why it matters |
|---|---|
| `eeprom_changed_slot(slot)` shifts `1ul` by the slot: undefined in C++ from 32 on. The Rust marks no slot then. | None in practice: communication passes 0..23. |
| `eeprom_read_layout(lay)` marks the repairs through `eeprom_changed()`, which also sets the status of the global `eep_content`, then sets `lay->status`. The Rust reads into `eep_content` only (the one target of the firmware); the C++ test that reads into a local layout reads into `eep_content` in Rust. | None for the firmware. |

## Glue: eeprom24 (I2C_eeprom 1.9.4)

| what | why it matters |
|---|---|
| `_waitEEReady()` polls while `micros() - _lastWrite <= 5000`. `_lastWrite` starts at 0 because the firmware never calls `begin()`, and after every wrap of `micros()` (71.6 min) the difference passes that window again, so a transfer then probes the device first. Kept. | One extra address probe (~0.1 ms) now and then. |
| The `I2C_eeprom.cpp` of the 1.9.4 tag (and of the PlatformIO package) says VERSION 1.9.3 in its header; library.json says 1.9.4. | Upstream quirk; the port follows that file. |

## Glue: ow_devices (`src/owDevices.cpp`)

| what | why it matters |
|---|---|
| `T_INIT` sets the wait timer to `CONV_INTERVALL / 10`, which `T_REQUEST` overwrites before `T_WAIT` reads it: dead store, not ported. | None. |
| During the three search steps `noOfDS18Devices` and `noOfDS2438Devices` are 0, so a `gonec`/`gowvc` in that window (3 calls, ~30 ms) reports no sensor. Kept. | A client polling exactly then sees an empty list once. |

## Glue: ds2438 (`src/DS2438.cpp`)

| what | why it matters |
|---|---|
| `begin()` takes the first device of the search as the monitor's address, whatever its family (usually a DS18B20 on this bus), and reports success for any device with a valid ROM CRC. Kept. | None: the firmware ignores the result and sets each monitor's address before its read. |

## Glue: dallas (DallasTemperature as vendored in `lib/`)

| what | why it matters |
|---|---|
| The vendored library is 3.8.1 (its header), the design table names 3.9.0; the vendored code is ported. `checkForConversion` is always true and not ported as a field; the blocking wait of `requestTemperatures()` is ported but unused, owDevices sets `waitForConversion` false before the first request. | None. |

## Glue: terminal (`src/terminal.cpp`)

| what | why it matters |
|---|---|
| `settar` returns `CMD_CLOSE` (the code of `close`); `stdet x` with x != 255 prints " - error" and then "stdet " anyway; `stons` prints without CR LF. Kept; the C++ suite asserts the first two. | Debug port only; nobody reads the return code. |
