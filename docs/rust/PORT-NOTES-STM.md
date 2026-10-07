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
| gstax reads the four receive error counters of USART1 (overrun, framing, noise, dropped; fields 14..17) one after the other from `serial::Port`, where the C++ copied `commUartErrors` with interrupts off. An interrupt between two reads can count one flag of a byte in this reply and its other flag only in the next. A consistent copy needs the interrupt masked around the four reads (the firmware's critical section) or a sequence count in `serial::Port`. | Diagnostics only: each counter is exact, the next gstax shows both counts, the ESP compares totals (event 322). |

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
| `help` prints "Help:" and a line of stars, no command list; `gmotc` prints the high factor on a line of its own (`println(" high: ")`, then `println(high)`). Kept; seen on USART6 in Renode. | Debug port only. |

## Glue: motor

The same rule for `software_stm32/src/motor.cpp` and its port `software_stm32_rust/glue/src/motor.rs`.

| what | why it matters |
|---|---|
| A calibration does not wait for the valve PSU like a move does. `valve_loop` switches the PSU on for a `CMD_A_LEARN` and sets the 1 s wait (`WAIT_TIMER50`), but `A_LEARN1` does not check `waittimer`: it prepares the first stroke on the next tick and sets the 0.4 s stroke wait (`WAIT_TIMER20`), so the first stroke starts about 0.5 s after the PSU, a move about 1.1 s after it. A failed pass (verdict Retry) sets the 1 s wait again, which `A_LEARN1` overwrites the same way. The Rust keeps both writes. | None known: the PSU is up long before 0.4 s. The 1 s wait of the calibration paths is dead; a later change of `WAIT_TIMER50` does not reach calibrations. |
| `appsetaction(..., force = true)` hands a command over while the valve state machine works on another valve: it overwrites `valvenr`, and `A_SET` reads `valvenr` again after an accepted calibration, so the move to the target would run on the forced valve with the target of the calibrated one. No firmware caller passes `force` (only test_motor.cpp does); the Rust keeps the parameter and the behaviour. | None today. A new caller of `force` must not use it while a calibration runs. |

## Glue: main_loop

| what | why it matters |
|---|---|
| The button line of `loop_system()` prints "Button pressed" on every other 100 ms tick while the button is held (`buttontest` is set by one tick and cleared by the next), not once per press; test_main.cpp asserts this. The Rust does the same. | Debug output on USART6 only. |

## C++ tests without a Rust form

Cases and assertions of the glue suites (`software_stm32/test/native/glue`) that the Rust glue
cannot express; the pairing of every case is in [PARITY-TESTS.md](PARITY-TESTS.md). The rest of
each case is ported.

| C++ | Rust | why |
|---|---|---|
| `test_fakes.cpp` "UART: readBytes() of missing bytes waits its timeout in fake time", "UART: end() drops the received bytes and stops the reception" | `test_support/tests.rs::uart_read_bytes_and_end_have_no_rust_form` checks what the trait has | The glue's `Serial` trait has neither `readBytes` nor `end`. |
| `test_fakes.cpp` "PRIMASK: __get/__set restore the previous state and count the disables"; the `irqDisables` and PRIMASK checks of `test_motor.cpp` (appsetaction) | comments in `test_support/fake_board/tests.rs` and `motor/tests.rs` | The glue never touches the interrupt mask: the firmware's `IsrCell::lock` hands out `&mut MotorShared` (GLUE-DESIGN-STM.md 2.3). |
| `test_fakes.cpp` "runner hooks: warm RAM is 0xA5 after power-on and survives pin, software and watchdog resets", "runner hooks: noinitSnapshot() copies the whole .noinit section", "glue::run turns a system reset into a software reboot and a watchdog reset into a watchdog reboot" | `system/bench.rs` (a reboot is a new controller over the EEPROM bytes, the `NOINIT` image and the CSR flags; `FakeNoinit` starts at 0xA5); the 27 goldens compare the `NOINIT` bytes of every boot | No fork-per-case runner (GLUE-DESIGN-STM.md 7.2). |
| The set-up assertions of the firmware's part: USART1 on PA10/PA9 at 115200 8N1 (`communication_setup`), USART6 on PA12/PA11 (`Terminal_Init`), USART1 8E1 of the boot window (`BootSetup`), the `Wire` pins PB6/PB7, `IWatchdog.begin(8000000)` before `Wire.begin()`, the `ITimer0`/`ITimer1` callbacks, the pin modes of `valve_setup` and `setup_system` | the rest of each case is ported; the firmware sets these up (`firmware/src/app.rs`, `board.rs`, `isr.rs`), Renode checks the USART1 framing (`boot.robot` E1, E5, E6) and the terminal output (`app.robot`) | The firmware crate holds the register set-up without decisions (GLUE-DESIGN-STM.md 1.1). |
| `glue_motor_c1`, the C1 build of `test_motor.cpp` | `BoardRev` values in one binary: the motor cases run for both revisions, `board/tests.rs` | One test binary for both boards (GLUE-DESIGN-STM.md 7.1). |
