# Mutation testing: stm32 (Rust)

Scope: the three host-tested crates of `software_stm32_rust`: `vdm-stm-core` (`core/`, the port
of `software_stm32/lib/core`, 36 modules), `vdm-stm-boot` (`boot/`, the boot stage of sector 0)
and `vdm-stm-glue` (`glue/`, the logic of `software_stm32/src/*.cpp` and of the libraries it
uses, over port traits). The firmware crate is outside the gate (docs/rust/PORTING.md).
Target: >= 95 % overall and >= 95 % for every file, as for the C++ suites
(`docs/revamped/mutation-stm32.md` and `mutation-stm32-glue.md` in the tag `v2.1.7-revamped`).
Result: **100.00 % in every crate and every file, gate passed: core 1333 / 1333, boot
196 / 196, glue 1869 / 1869 killed.** 3574 mutants, 50 equivalent, 126 unviable.
Measured on 2026-10-07 and 08 with cargo-mutants 27.1.0, 4 jobs, in the runs listed under Runs;
the sources of every file are those of `fdf24ed`.

Tool: `tools/rust/docker.sh mutate` runs cargo-mutants on the package and then the per-file gate
`tools/rust/mutation_gate.py`. Every module is mutated against its own tests: `--file
<crate>/src/<m>.rs` with the libtest filter `<m>::`, like the C++ per-file test binaries; a
submodule file runs with the filter of its parent (`glue/src/onewire/slots.rs`: `onewire::`).
The runs made for this report (m1, m2, dallas2) mutate one module per cargo-mutants run; the
earlier runs that the other files are quoted from grouped modules, and their mutants ran the
tests of every module of the run. A mutant is built in the test profile (overflow checks on); a
failed test, a panic or a timeout (cargo-mutants' automatic limit: 20 s; 41 s in boot10, whose
baseline took 8 s) kills it.
Not counted: unviable mutants (they do not compile, the C++ "stillborn") and equivalent
mutants, which are listed with a reason in `tools/rust/mutation/equivalents/vdm-stm-core.json`
and `tools/rust/mutation/equivalents/vdm-stm-glue/`. Files without mutants:
`core/src/valve_codes.rs` (constants), `boot/src/jump.rs` (port calls that end in `-> !`),
`boot/src/io.rs` and `glue/src/hal.rs` (port traits) and the three `lib.rs`.

`software_stm32_rust/.cargo/mutants.toml` (`612a30f`) keeps the system suites of the glue out of
cargo-mutants: `glue/src/system/bench.rs`, `golden.rs` and `tests_*.rs` are test code declared
`#[cfg(all(test, feature = "terminal"))]`, which cargo-mutants' `cfg(test)` check does not
recognise, so without the exclusion a package-wide run (the CI job `rust-mutation`) mutates 278
mutants of test code. The module runs never reach them. The exclusion changes no other mutant:
1927 in the glue sources, 1434 in the core and 213 in the boot crate, before and after.

A mutant that the tests of its module kill also fails the whole suite. Run per module, the core
gives file by file the numbers of its package-wide run on `380ae86` (2026-10-07, before the glue
existed; the core has not changed since: 1434 mutants, 77 unviable, 24 equivalent, 1333 of 1333
killed, 21 of them by a timeout), so no core mutant needs the tests of another module. In the
glue one mutant did (`dallas.rs`, see Survivors).

## Reproduce

```sh
bash tools/rust/docker.sh mutate software_stm32_rust vdm-stm-core --file core/src/<m>.rs -- --lib -- <m>::
bash tools/rust/docker.sh mutate software_stm32_rust vdm-stm-boot --file boot/src/<m>.rs -- --lib -- <m>::
bash tools/rust/docker.sh mutate software_stm32_rust vdm-stm-glue --file glue/src/<m>.rs -- --lib -- <m>::
bash tools/rust/docker.sh mutate software_stm32_rust vdm-stm-glue --file glue/src/onewire/slots.rs -- --lib -- onewire::
```

`--lib` leaves out the doc-test pass.

## vdm-stm-core

| file | score | killed / counted | timeout | equivalent | unviable | run |
|---|---|---|---|---|---|---|
| `core/src/arg_parser.rs` | 100.00 % | 78 / 78 | 0 | 0 | 2 | m1 |
| `core/src/buf_writer.rs` | 100.00 % | 64 / 64 | 0 | 0 | 3 | m1 |
| `core/src/calibration.rs` | 100.00 % | 60 / 60 | 0 | 3 | 1 | m1 |
| `core/src/config_blocks.rs` | 100.00 % | 47 / 47 | 0 | 0 | 12 | m1 |
| `core/src/config_store.rs` | 100.00 % | 96 / 96 | 0 | 14 | 0 | m1 |
| `core/src/eeprom_layout.rs` | 100.00 % | 33 / 33 | 0 | 0 | 6 | m1 |
| `core/src/end_stop_detector.rs` | 100.00 % | 65 / 65 | 14 | 0 | 0 | m1 |
| `core/src/failsafe.rs` | 100.00 % | 14 / 14 | 0 | 0 | 1 | m2 |
| `core/src/fault_retry.rs` | 100.00 % | 21 / 21 | 0 | 0 | 0 | m2 |
| `core/src/lease.rs` | 100.00 % | 38 / 38 | 0 | 0 | 1 | m2 |
| `core/src/legacy_layout.rs` | 100.00 % | 35 / 35 | 0 | 0 | 0 | m2 |
| `core/src/line_assembler.rs` | 100.00 % | 57 / 57 | 7 | 0 | 6 | m2 |
| `core/src/manual_enable.rs` | 100.00 % | 6 / 6 | 0 | 0 | 0 | m2 |
| `core/src/motor_params.rs` | 100.00 % | 43 / 43 | 0 | 0 | 1 | m2 |
| `core/src/move_classifier.rs` | 100.00 % | 47 / 47 | 0 | 0 | 1 | m2 |
| `core/src/onewire_check.rs` | 100.00 % | 17 / 17 | 0 | 0 | 0 | m2 |
| `core/src/presence_test.rs` | 100.00 % | 25 / 25 | 0 | 0 | 2 | m2 |
| `core/src/profile_recorder.rs` | 100.00 % | 54 / 54 | 0 | 1 | 0 | m2 |
| `core/src/protection_guard.rs` | 100.00 % | 17 / 17 | 0 | 0 | 0 | m2 |
| `core/src/replies.rs` | 100.00 % | 24 / 24 | 0 | 0 | 1 | m2 |
| `core/src/replies_v2.rs` | 100.00 % | 56 / 56 | 0 | 0 | 17 | m2 |
| `core/src/replies_v3.rs` | 100.00 % | 14 / 14 | 0 | 0 | 4 | m2 |
| `core/src/reset_guard.rs` | 100.00 % | 30 / 30 | 0 | 0 | 0 | m2 |
| `core/src/retry_backoff.rs` | 100.00 % | 19 / 19 | 0 | 0 | 0 | m2 |
| `core/src/settings.rs` | 100.00 % | 23 / 23 | 0 | 0 | 0 | m2 |
| `core/src/stall_detector.rs` | 100.00 % | 10 / 10 | 0 | 0 | 0 | m2 |
| `core/src/store_scheduler.rs` | 100.00 % | 31 / 31 | 0 | 0 | 1 | m2 |
| `core/src/system_stats.rs` | 100.00 % | 22 / 22 | 0 | 0 | 0 | m2 |
| `core/src/target_rejection.rs` | 100.00 % | 5 / 5 | 0 | 0 | 0 | m2 |
| `core/src/temp_filter.rs` | 100.00 % | 33 / 33 | 0 | 0 | 0 | m2 |
| `core/src/temp_refresh.rs` | 100.00 % | 16 / 16 | 0 | 0 | 0 | m2 |
| `core/src/tokenizer.rs` | 100.00 % | 35 / 35 | 0 | 0 | 18 | m2 |
| `core/src/uart_errors.rs` | 100.00 % | 11 / 11 | 0 | 1 | 0 | m2 |
| `core/src/valve_scheduler.rs` | 100.00 % | 138 / 138 | 0 | 5 | 0 | m2 |
| `core/src/warm_state.rs` | 100.00 % | 49 / 49 | 0 | 0 | 0 | m2 |
| **total** | **100.00 %** | 1333 / 1333 | 21 | 24 | 77 | |

1434 mutants generated, 24 equivalent, 77 unviable.

## vdm-stm-boot

| file | score | killed / counted | timeout | equivalent | unviable | run |
|---|---|---|---|---|---|---|
| `boot/src/app_check.rs` | 100.00 % | 23 / 23 | 12 | 0 | 0 | boot10 |
| `boot/src/capture.rs` | 100.00 % | 42 / 42 | 0 | 0 | 1 | m2 |
| `boot/src/fault_record.rs` | 100.00 % | 34 / 34 | 0 | 0 | 4 | boot10 |
| `boot/src/fifo.rs` | 100.00 % | 28 / 28 | 0 | 0 | 0 | m2 |
| `boot/src/gpio.rs` | 100.00 % | 12 / 12 | 0 | 0 | 0 | boot10 |
| `boot/src/id_block.rs` | 100.00 % | 20 / 20 | 0 | 0 | 1 | boot10 |
| `boot/src/poll.rs` | 100.00 % | 1 / 1 | 0 | 0 | 0 | boot10 |
| `boot/src/stage.rs` | 100.00 % | 13 / 13 | 0 | 0 | 7 | boot10 |
| `boot/src/uart.rs` | 100.00 % | 5 / 5 | 3 | 0 | 0 | boot10 |
| `boot/src/watchdog.rs` | 100.00 % | 2 / 2 | 0 | 0 | 0 | boot10 |
| `boot/src/window.rs` | 100.00 % | 16 / 16 | 7 | 0 | 4 | boot10 |
| **total** | **100.00 %** | 196 / 196 | 22 | 0 | 17 | |

213 mutants generated, 0 equivalent, 17 unviable.

## vdm-stm-glue

| file | score | killed / counted | timeout | equivalent | unviable | run |
|---|---|---|---|---|---|---|
| `glue/src/app.rs` | 100.00 % | 310 / 310 | 0 | 0 | 6 | app7 |
| `glue/src/board.rs` | 100.00 % | 2 / 2 | 0 | 0 | 3 | motor6 |
| `glue/src/communication.rs` | 100.00 % | 195 / 195 | 2 | 0 | 0 | io10 |
| `glue/src/dallas.rs` | 100.00 % | 84 / 84 | 0 | 0 | 0 | dallas2 |
| `glue/src/ds2438.rs` | 100.00 % | 37 / 37 | 0 | 0 | 1 | m2 |
| `glue/src/eeprom.rs` | 100.00 % | 97 / 97 | 1 | 0 | 0 | m2 |
| `glue/src/eeprom24.rs` | 100.00 % | 27 / 27 | 1 | 0 | 1 | m2 |
| `glue/src/hw_timer.rs` | 100.00 % | 10 / 10 | 0 | 0 | 1 | io10 |
| `glue/src/i2c_bus.rs` | 100.00 % | 2 / 2 | 0 | 0 | 0 | m2 |
| `glue/src/i2c_master.rs` | 100.00 % | 33 / 33 | 0 | 11 | 0 | io11 |
| `glue/src/irq.rs` | 100.00 % | 10 / 10 | 4 | 0 | 4 | io10 |
| `glue/src/main_loop.rs` | 100.00 % | 24 / 24 | 3 | 0 | 0 | m2 |
| `glue/src/motor.rs` | 100.00 % | 349 / 349 | 21 | 0 | 3 | motor6 |
| `glue/src/motor/pulse.rs` | 100.00 % | 7 / 7 | 0 | 0 | 0 | motor6 |
| `glue/src/onewire.rs` | 100.00 % | 46 / 46 | 0 | 0 | 1 | io11 |
| `glue/src/onewire/slots.rs` | 100.00 % | 13 / 13 | 1 | 0 | 0 | io10 |
| `glue/src/ow_devices.rs` | 100.00 % | 100 / 100 | 0 | 0 | 0 | m2 |
| `glue/src/print.rs` | 100.00 % | 76 / 76 | 0 | 0 | 0 | m2 |
| `glue/src/serial.rs` | 100.00 % | 91 / 91 | 31 | 11 | 1 | io11 |
| `glue/src/sysstat.rs` | 100.00 % | 12 / 12 | 0 | 0 | 0 | m2 |
| `glue/src/system.rs` | 100.00 % | 247 / 247 | 3 | 4 | 10 | system11 |
| `glue/src/terminal.rs` | 100.00 % | 97 / 97 | 0 | 0 | 1 | io10 |
| **total** | **100.00 %** | 1869 / 1869 | 67 | 26 | 32 | |

1927 mutants generated, 26 equivalent, 32 unviable.

## Runs

| run | crate | files (test filters) | tree | date | mutants | time |
|---|---|---|---|---|---|---|
| m1 | core | arg_parser, buf_writer, calibration, config_blocks, config_store, eeprom_layout, end_stop_detector; one run per module (`<m>::`) | `612a30f` | 2026-10-08 | 484 | 9 min |
| m2 | core | the other 28 modules, one run per module | `612a30f` | 2026-10-08 | 950 | 15 min |
| m2 | glue | dallas, ds2438, eeprom, eeprom24, i2c_bus, main_loop, ow_devices, print, sysstat; one run per module | `612a30f` | 2026-10-08 | 461 | 13 min |
| m2 | boot | capture, fifo; one run per module | `612a30f` | 2026-10-08 | 71 | 24 s |
| dallas2 | glue | dallas (`dallas::`) | `fdf24ed` | 2026-10-08 | 84 | 1 min |
| boot10 | boot | app_check, fault_record, gpio, id_block, jump, poll, stage, uart, watchdog, window in one run (`app_check:: fault_record:: gpio:: id_block:: jump:: poll:: stage:: uart:: watchdog:: window::`) | `3434554` | 2026-10-08 | 142 | 10 min |
| io10 | glue | communication, hw_timer, i2c_master, irq, onewire, onewire/slots, serial, terminal in one run (`communication:: hw_timer:: i2c_master:: irq:: onewire:: serial:: terminal::`) | `3434554` | 2026-10-08 | 525 | 9 min |
| io11 | glue | i2c_master, onewire, serial (`i2c_master:: onewire:: serial::`) | `86f2b42` | 2026-10-08 | 194 | 6 min |
| system11 | glue | system (`system::`) | `86f2b42` | 2026-10-08 | 261 | 12 min |
| app7 | glue | app (`app::`) | `686e4f9` | 2026-10-08 | 316 | 21 min |
| motor6 | glue | motor, motor/pulse, board (`motor:: board::`; `board::` also selects the cases of the fake board) | `491641f` | 2026-10-07 | 364 | 26 min |

Each file's numbers come from its last run. m1 and m2 hold the mutation lock once each and make
one cargo-mutants run per module with the arguments of the commands above; dallas2 is that
command. The other runs are those of the porting work (the names of its logs); no source line of
their files changed since: the boot crate and communication, hw_timer, irq, onewire/slots and
terminal are the same in `3434554` and `fdf24ed`, i2c_master, onewire, serial and system the same
in `86f2b42`, app and its cases since `686e4f9`, motor, pulse, board and their cases since
`491641f`. Later commits only added cases (`86f2b42`: onewire, serial, system; `fdf24ed`:
dallas), and the shared test support of the glue changed after app7 and motor6 only in
`db05a8a`, which adds the panic payload `Fault` of the interrupt cell checks.

## Equivalent mutants

| file | function | mutation | matched | reason |
|---|---|---|---|---|
| `core/src/calibration.rs` | `escalated_bound` | `replace \|\| with && in escalated_bound` | 3 (2 killed) | only the \|\| between repetition == 0 and step_pct == 0 survives: with either of them 0 the growth is 0 %, so the bound comes back unchanged without the early return (the other two \|\| -> && mutants of the function are killed and fall under this name) |
| `core/src/config_store.rs` | `(const)` | `replace \| with ^` | 2 | const REWRITE_LAYOUT = CHANGED_SENSORS \| CHANGED_MOVEMENTS \| CHANGED_MOTOR: disjoint bits, so \| and ^ give the same value |
| `core/src/config_store.rs` | `resolve_config` | `replace \| with ^ in resolve_config` | 1 | CFG_LAYOUT_CRC \| CFG_SHADOW_MISSING: disjoint bits, so \| and ^ give the same value |
| `core/src/config_store.rs` | `repairs_config` | `replace \| with ^ in repairs_config` | 5 | the mask ORs six disjoint CFG_* bits, so \| and ^ give the same mask |
| `core/src/config_store.rs` | `blocks_for` | `replace \| with ^ in blocks_for` | 6 | every \| in blocks_for joins disjoint CHANGED_* or BLOCK_* constants, so \| and ^ give the same value |
| `core/src/uart_errors.rs` | `count_uart_errors` | `replace \| with ^ in count_uart_errors` | 1 | UART_ERROR_NOISE \| UART_ERROR_PARITY: disjoint bits, so \| and ^ give the same mask |
| `core/src/profile_recorder.rs` | `ProfileRecorder::compact` | `replace < with <= in ProfileRecorder::compact` | 1 | spacing < MAX_SPACING never decides the loop: at spacing 0x8000 the 16-bit counts fall into at most two cells, so size is below PROFILE_SAMPLES before the bound is reached (C++ the same defensive bound) |
| `core/src/valve_scheduler.rs` | `move_dir` | `replace > with >= in move_dir` | 1 | move_dir is evaluated only for drive != actual (the callers check it first), where > and >= agree |
| `core/src/valve_scheduler.rs` | `ValveScheduler::step2` | `replace % with + in ValveScheduler::step2` | 2 (1 killed) | rr = (i + 1) % VALVES: rr is only used as (rr + n) % VALVES, so storing i + 1 + VALVES picks the same valves (the % of the index itself is killed and falls under this name) |
| `core/src/valve_scheduler.rs` | `ValveScheduler::step3` | `replace % with + in ValveScheduler::step3` | 2 (1 killed) | rr = (i + 1) % VALVES: rr is only used as (rr + n) % VALVES, so storing i + 1 + VALVES picks the same valves (the % of the index itself is killed and falls under this name) |
| `glue/src/i2c_master.rs` | `(const)` | `replace << with >>` | 9 (8 killed) | only the shift of SR1_SB = 1 << 0 survives: a shift by 0, so 1 >> 0 is the same bit; the << -> >> mutants of the other SR1_* and SR2_BUSY constants (shifts by 1 to 10) are killed by the transfer tests and fall under this name |
| `glue/src/i2c_master.rs` | `I2cV1<R, C>::wait` | `replace \| with ^ in I2cV1<R, C>::wait` | 2 | SR1_ARLO \| SR1_BERR (bits 9 and 8) in the test and in the clear: disjoint bits, so \| and ^ give the same mask |
| `glue/src/serial.rs` | `(const)` | `replace << with >>` | 7 (6 killed) | only the shift of SR_PE = 1 << 0 survives: a shift by 0, so 1 >> 0 is the same bit; the << -> >> mutants of the other SR_* constants (shifts by 1 to 7) are killed by hal_error_bits_are_the_cores_and_one_per_flag and fall under this name |
| `glue/src/serial.rs` | `(const)` | `replace \| with ^` | 4 | const SR_READ_DR = SR_RXNE \| SR_ORE \| SR_NE \| SR_FE \| SR_PE: five disjoint bits, so each of the four \| gives the same mask as ^ |
| `glue/src/system.rs` | `<impl MainLoopEnv for Modules<'_, P>>::terminal_serve` | `delete - in <impl MainLoopEnv for Modules<'_, P>>::terminal_serve` | 1 | the -1 of the #[cfg(not(feature = "terminal"))] branch: the gated build has the default feature terminal and compiles the other branch only, and loop_system() ignores the result of terminal_serve() in every build (as the C++ ignores Terminal_Serve()'s) |
| `glue/src/system.rs` | `<impl MainLoopEnv for Modules<'_, P>>::eepromsetup` | `replace <impl MainLoopEnv for Modules<'_, P>>::eepromsetup -> i16 with 0` | 1 | Eeprom::setup() only sets the mirror status to Init, which the controller's fresh Eeprom::default() holds already when setup_system() calls it, once, before the first read; setup_system() ignores the result (as the C++ ignores eepromsetup()'s) |
| `glue/src/system.rs` | `<impl MainLoopEnv for Modules<'_, P>>::eepromsetup` | `replace <impl MainLoopEnv for Modules<'_, P>>::eepromsetup -> i16 with 1` | 1 | as the mutant with 0: the skipped Eeprom::setup() sets the status the fresh Eeprom::default() holds already, and setup_system() ignores the result |
| `glue/src/system.rs` | `<impl MainLoopEnv for Modules<'_, P>>::eepromsetup` | `replace <impl MainLoopEnv for Modules<'_, P>>::eepromsetup -> i16 with -1` | 1 | as the mutant with 0: the skipped Eeprom::setup() sets the status the fresh Eeprom::default() holds already, and setup_system() ignores the result |

### Name matching

The gate matches an entry by file, function and mutation name, without line and column, so an
entry survives edits that move the code (`mutation_gate.py`). It therefore covers every mutant of
its function with that name, the killed ones too, and an entry of a constant (function `""`)
every mutant of that name in the file's constant expressions. Of the 50 matched mutants 32
survive without their entry; the other 18 are killed by the tests and counted as equivalent all
the same (column matched): the two other `||` -> `&&` of `escalated_bound`, the `%` of the
index itself in `step2` and `step3`, and the `<<` -> `>>` of every `SR_*`, `SR1_*` and
`SR2_BUSY` constant with a shift above 0 (serial.rs: six, two of them timeouts; i2c_master.rs:
eight), as the reasons of `tools/rust/mutation/equivalents/vdm-stm-glue/serial.json` and
`i2c_master.json` say. They leave both numbers of the score, so they cannot raise it. The limit
is the other way: a new mutant of the same name in the same function, or for an entry of a
constant anywhere in the file's constants, counts as equivalent without a review, for example a
new constant of serial.rs whose `<<` -> `>>` survives or a `|` of overlapping bits added to
`blocks_for`. A change of a file with entries needs its entries checked by hand.

## Survivors found by the runs

| file | mutant | found by | killed by |
|---|---|---|---|
| `glue/src/dallas.rs` | `delete -` of `DEVICE_DISCONNECTED_RAW = -7040` (every dallas case compared with the constant, so in the io gate of `f1428a5` only the cases of other modules killed it) | m2 | `get_temp_of_a_device_that_does_not_answer_is_minus_7040` (`fdf24ed`) |
| `core/src/replies.rs` | two mutants of `5 + 11 * 11 + 12`, the expression of `VALVE_DATA_REPLY_MAX_LEN` (the glue sizes its reply buffer with it) | first run of the text protocol modules | `valve_data_reply_max_len_is_the_contract_value` (`6a4902b`) |
| `glue/src/motor.rs` | `replace + with * in MotorShared::prepare_normal_move` | first glue run (motor, pulse, app, main_loop, board) | `a_keep_status_move_to_the_end_stop_is_never_early` (`55d3c27`) |
| `glue/src/motor.rs` | `replace && with \|\| in MotorShared::state_idle`, `replace == with != in MotorShared::state_idle` | first glue run | `idle_ticks_count_no_movement` (`55d3c27`) |
| `glue/src/motor.rs` | `replace \|\| with && in MotorShared::state_learn_stroke` | first glue run | `a_calibration_stroke_whose_count_runs_out_fails_with_a_stroke_timeout` (`55d3c27`) |
| `glue/src/app.rs` | `replace && with \|\| in App::app_apply_requests` | first glue run | `staop_of_a_failed_or_blocked_valve_only_sets_the_target_counted_as_rejected` (`55d3c27`) |
| `glue/src/app.rs` | `delete ! in App::app_track_valves` | first glue run | `a_successful_calibration_record_clears_the_end_stop_latch_a_blocked_one_does_not` (`55d3c27`) |
| `glue/src/app.rs` | `replace && with \|\| in App::app_10s_loop` | first glue run | `the_movement_trigger_needs_its_count_at_0_and_no_calibration_in_hand` (`55d3c27`) |
| `glue/src/app.rs` | `replace \|\| with && in App::app_1s_tick` | first glue run | `retry_attempts_stay_while_the_valve_waits_for_its_test_or_calibration` (`55d3c27`) |
| `glue/src/app.rs` | `delete field reserved` and `delete field pad from struct WarmState expression in warm_from_bytes` | first glue run | `every_byte_of_the_record_is_read_back_reserved_and_pad_included` (`55d3c27`) |
| `glue/src/motor.rs` | `replace > with < in MotorShared::motor_turning` (`cyclecnt > 50`: the stroke mean sampled every cycle instead of every 51st) | motor run 3 | `strokes_shorter_than_four_sample_periods_learn_no_mean_current` (`491641f`) |
| `glue/src/system.rs` | `replace <impl AppEnv for AppCtx<'_, P>>::motor_set_escalation with ()` | system run 6 (92.8 %) | `the_stored_escalation_reaches_the_motor_at_the_next_start` (`d86fa58`) |
| `glue/src/system.rs` | `replace <impl CommunicationEnv for Ctx<'_, P>>::app_protect_suspended -> bool with false` | system run 6 | `shorts_on_three_valves_suspend_the_limits_and_gstax_reports_it` (`d86fa58`) |
| `glue/src/system.rs` | `replace <impl CommunicationEnv for Ctx<'_, P>>::valve_get_profile with ()`, `replace <impl TerminalEnv for Ctx<'_, P>>::valve_idle -> bool with true` | system run 6 | `gprof_reports_the_last_move_and_the_terminal_waits_for_the_valve_machine` (`d86fa58`) |
| `glue/src/system.rs` | `replace <impl TerminalEnv for Ctx<'_, P>>::sysstat_safe_mode -> bool with false` | system run 6 | `sena_is_refused_in_safe_mode` (`d86fa58`) |
| `glue/src/system.rs` | `replace <impl TerminalEnv for Ctx<'_, P>>::temp_command with ()`, `comm_set_valve_sensor_index -> i16` with 1 and -1, `comm_set_valve_sensors -> i16 with 0` | system run 6 | `the_terminal_searches_the_1_wire_bus_and_assigns_its_sensors` (`d86fa58`) |
| `glue/src/system.rs` | `replace <impl TerminalEnv for Ctx<'_, P>>::eeprom_state -> u8` with 0 and 1 | system run 6 | `seteep_is_blocked_while_the_eeprom_cannot_be_read` (`d86fa58`) |
| `glue/src/system.rs` | `replace <impl TerminalEnv for Ctx<'_, P>>::eeprom_changed with ()` | system run 6 | `saveep_schedules_an_eeprom_write` (`d86fa58`) |
| `glue/src/system.rs` | `replace <impl MainLoopEnv for Modules<'_, P>>::app_10s_loop -> u8` with 0 and 1 | system run 6 | `the_10_s_branch_runs_the_learn_time_trigger` (`d86fa58`) |
| `glue/src/system.rs` | `replace <impl AppEnv for AppCtx<'_, P>>::sysstat_uptime_s -> u32` with 0 and 1 | system run 9 | `inrush_trips_on_three_valves_count_for_the_guard_only_within_600_s_of_uptime` (`86f2b42`) |
| `glue/src/onewire.rs` | `replace OneWire<L>::reset_search with ()` | io10 | `reset_search_starts_the_next_search_at_the_first_device_again` (`86f2b42`) |
| `glue/src/serial.rs` | `replace & with \|` and `replace & with ^ in <impl Serial for PortSerial<'_, U>>::flush` | io10 | `flush_with_the_ring_empty_waits_until_tc_is_set` (`86f2b42`) |

Survivors that a rewrite with the same behaviour removed:

- core, the package-wide run of `380ae86`: the peak of the end-stop detector and the step-2 run of
  the scheduler use max/min, the scheduler wraps the test index with > before ==, take_factor
  caps a request with min, check_block lets the bounds check of the CRC position decide a too
  long payload, decode_safety zips the 12 positions; before it, `BufWriter::truncate` takes the
  minimum (`6a4902b`).
- glue, first run (`55d3c27`): `delete -` of `VALVE_INIT_TEMPERATURE = -2000` (app.h defines
  it, nothing uses it: removed) and `replace > with >= in MotorShared::motor_turning` of
  `debouncecnt > 50`, which holds whenever `cyclecnt > 50` does (the Rust tests cyclecnt alone and
  says why).
- glue io modules, first run (`f1428a5`, 12 survivors): gvlvd formats into the reply buffer, the
  stsnx/stsny index is parsed as any 16-bit number because comm_set_valve_sensor_index refuses
  the same values first, the write of block B no longer sets the lease flag only the decoder
  fills, the frame of an EEPROM chunk is the 32-byte Wire buffer, distinct bits are added; the
  rest got cases: `the_reply_buffer_holds_the_longest_gprof`,
  `stsnx_and_stsny_refuse_every_index_past_the_table_without_a_call`,
  `a_write_step_without_the_calibration_field_writes_no_record`,
  `a_change_of_other_fields_takes_no_sensor_slot_from_ram_at_the_re_read`,
  `a_slot_or_record_changed_twice_stays_marked_for_the_re_read`,
  `the_ready_wait_ends_exactly_5_ms_after_the_last_write`, the 9, 11 and 12-bit resolutions of
  one sensor in the dallas cases and print of a float 0.

## Timeouts

110 counted mutants were killed by a timeout (core 21, boot 22, glue 67): mutants that stop the
progress a test waits for in a loop (the filter of the end-stop detector, the line assembler,
the check of the application part, the USART ring and its flush, the motor's tick counters, the
dispatch of the main loop). 28 of them ran again alone, one at a time, against the tests of
their module (the irq constant also with the serial cases), with 65 s (on `612a30f`, after m2)
or 75 s (the last five, by the porting work on `86f2b42`). Each one failed a test; none passes
when given the time:

| mutant | mutation | run | alone |
|---|---|---|---|
| `boot/src/app_check.rs:58` | `replace crc32 -> u32 with 0` | boot10 | 4 failed in 0 s |
| `boot/src/app_check.rs:106` | `replace AppCheck::step with ()` | boot10 | 1 failed, 4 still running at 65 s |
| `boot/src/app_check.rs:127` | `replace AppCheck::finish -> bool with false` | boot10 | 4 failed in 0 s |
| `boot/src/uart.rs:13` | `replace rx -> Option<u8> with None` | boot10 | 2 failed in 0 s |
| `boot/src/window.rs:47` | `replace wait_ms_with with ()` | boot10 | 7 failed in 0 s |
| `boot/src/window.rs:62` | `replace setup with ()` | boot10 | 2 failed in 0 s |
| `boot/src/window.rs:151` | `replace == with != in Window::step_with` | boot10 | 8 failed in 0 s |
| `core/src/end_stop_detector.rs:86` | `replace EndStopDetector::sample -> Trip with Default::default()` | m1 | 23 failed, 1 still running at 65 s |
| `core/src/end_stop_detector.rs:93` | `replace / with % in EndStopDetector::sample` | m1 | 18 failed, 1 still running at 65 s |
| `core/src/end_stop_detector.rs:157` | `replace EndStopDetector::current -> i32 with 0` | m1 | 15 failed, 1 still running at 65 s |
| `core/src/line_assembler.rs:74` | `replace LineAssembler<B>::push -> bool with false` | m2 | 7 failed, 6 still running at 65 s |
| `core/src/line_assembler.rs:106` | `replace < with > in LineAssembler<B>::feed` | m2 | 7 failed, 6 still running at 65 s |
| `glue/src/eeprom.rs:234` | `replace += with *= in eeprom_read_block` | m2 | 2 failed in 16 s |
| `glue/src/eeprom24.rs:67` | `replace - with / in I2cEeprom<I, C>::page_block` | m2 | 6 failed in 49 s |
| `glue/src/irq.rs:35` | `replace blocked -> bool with false` | io10 | 2 failed in 0 s |
| `glue/src/main_loop.rs:139` | `replace MainLoop::step with ()` | m2 | 6 failed, 1 still running at 65 s |
| `glue/src/main_loop.rs:184` | `replace > with == in MainLoop::step` | m2 | 2 failed in 61 s, 1 over 60 s |
| `glue/src/motor.rs:1154` | `replace -= with += in MotorShared::tick` | motor6 | 62 failed in 5 s |
| `glue/src/motor.rs:1379` | `replace && with \|\| in MotorShared::state_learn_stroke` | motor6 | 20 failed in 6 s |
| `glue/src/motor.rs:1794` | `replace MotorShared::motor_turning -> u8 with 1` | motor6 | 45 failed in 6 s |
| `glue/src/serial.rs:130` | `replace Ring::len -> usize with 1` | io11 | 13 failed, 3 still running at 65 s |
| `glue/src/serial.rs:240` | `replace Port::read -> Option<u8> with Some(0)` | io11 | 4 failed in 28 s |
| `glue/src/system.rs:1041` | `replace <impl MainLoopEnv for Modules<'_, P>>::communication_loop -> i16 with 0` | system11 | 41 failed, 1 still running at 65 s |
| `glue/src/communication.rs:445` | `replace > with >= in Communication::setup` | io10 | 6 failed, stopped at 75 s |
| `glue/src/irq.rs:15` | `replace << with >>` (`PRIO_USART = 1 << 4`) | io10 | 3 failed, 1 still running at 75 s |
| `glue/src/onewire/slots.rs:110` | `replace < with > in spin_us` | io10 | 3 failed in 59 s |
| `glue/src/serial.rs:310` | `replace != with == in <impl Serial for PortSerial<'_, U>>::flush` | io11 | 1 failed, 2 still running at 75 s |
| `glue/src/serial.rs:310` | `replace == with != in <impl Serial for PortSerial<'_, U>>::flush` | io11 | 1 failed, 2 still running at 75 s |

The other 82 were not run again; they are mutants of the same functions or of the loops these
functions drive.

## Surviving mutants

None.
