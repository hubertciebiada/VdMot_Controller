# VdMot Revamped STM32 in Rust: safety review

Adversarial review of the safety-critical paths of `software_stm32_rust` on branch
`revamped-rust` (base 92718f5) against the acceptance bar of the operator: no state of the Rust
STM firmware may require physical access (BOOT0, SWD), and none may damage valves or motors. The
STM drives the valve motors in a closed cabinet and is re-flashed only by the ESP over USART1
(`DEADBEEF` in the STM's own boot window, then the ROM bootloader; D9 at the base: the ESP
writes sector 0 last; since F9 it writes sector 0 first for images with the record of F1).

Method: the code of boot stage, firmware, glue and the core functions they call, read against
the C++ 2.1.7 reference (`software_stm32/src`: otasupport, boot_jump, main, motor, app,
communication, eeprom), the built images (map, ELF, `vdm-stm-image-check` with
`VDM_LIST_BOOT`), the sources of embassy-stm32 0.6.0, embassy-hal-internal 0.5.0 and
cortex-m-rt 0.7.7 in the cargo registry, host tests and Renode. Each scenario below is a concrete
sequence: power loss at every step, malformed or partial UART input, stuck buses, starved
timers and interrupts.

Severity: High (the bar is broken in a plausible sequence), Medium (the bar can be broken in a
rare but real sequence), Low (degraded protection, no damage expected), Info (remark, accepted
risk or a difference to the C++ without effect).

## Summary

| # | area | severity | finding | status |
|---|---|---|---|---|
| F1 | boot, D9 | Medium | a flash cut before sector 0 lets the old boot stage start new or erased code without a watchdog | fixed: d0b7aca (IWDG from the end of the window), bf2c2e6 (record and CRC-32 of the application part) |
| F2 | motor, real time | Low | the sensor match prints 1-4 KB to USART6 inside the motor lock: TIM1 and TIM2 masked up to ~0.25 s, also while a valve moves | fixed: 686e4f9 |
| F3 | motor, real time | Low | other debug lines inside the motor lock wait for a full USART6 ring | open, accepted |
| F4 | faults | Low | a stack overflow ends in a lockup on stacking: no handler, outputs as they were until the IWDG | open, accepted |
| F5 | motor, faults | Info | embassy's blocking ADC read spins without a bound in the 1 ms interrupt | not a bug, accepted |
| F6 | boot | Info | no clock security system in the boot window: an HSE that dies there stops the CPU until the ESP's NRST | open, proposal |
| F7 | boot | Info | a failed switch of SYSCLK to a ready HSE leaves the window at the wrong baud rate for one boot | not a bug |
| F8 | terminal | Info | `learn`, `open`, `close` on the debug terminal move valves also in safe mode | not a bug (C++ parity) |
| F9 | flashing from C++ | Medium | the first flash from C++ 2.1.7 keeps the whole session as brick window: the C++ reset and fault handlers lie in sector 3 | decided 2026-10-07: the ESP writes sector 0 first for images with the record of F1; fixed on the ESP side: 4c04c36 |

No finding against: the boot window after any reset, the ROM jump (R2), the IWDG feed rule,
the fault records, the motor state machine and its stop paths, the hand-over of commands,
persistence across power loss, the no-init cells across C++ and Rust, the UART paths. Details
below each section.

## F1 (Medium): a flash cut before sector 0 starts the old boot stage into foreign code

**Scenario.** The Rust ESP flasher at the base (D9) erases and writes sectors 1..n first and
sector 0 last. A pass of sectors 1..n that fails (block retries and session retries used up),
is aborted, or loses the ESP's power ends with the ESP resetting the STM "into the application"
(`stm_flasher.rs` `abort()`, the Failed path). Sector 0 is still the old one: vector table, ID
block, boot stage. The old window works (the image can be flashed again), but on timeout the
old `main` (`firmware/src/main.rs:48-51` at 92718f5) calls `app::run` at its old address,
0x0800BB44 in the F401_C2 image of 92718f5, inside sector 2:
- erased flash: the UDF-like 0xFFFF faults, the handlers lie in sector 0 (`fault::stop`):
  outputs off, IWDG with its reset values (512 ms), next window. Safe.
- new code (the failed pass wrote past that address): execution starts in the middle of a
  function of another image, with the old vector table in front of it. The IWDG started only
  inside `app::run` (`firmware/src/app.rs:48-49` at 92718f5), so a hang there runs without any
  watchdog until the ESP's link policy resets the STM (at most once per 10 min,
  `link_policy.rs` `reset_min_interval_ms`); the valve outputs are whatever the stray code
  wrote. Unlikely to drive a motor, but nothing bounded it.

**Fix.**
1. d0b7aca: the boot stage starts the IWDG (8 s, the values of `IWatchdog.begin(8000000)`) as
   its last step on the timeout path, in sector 0 (`boot/src/stage.rs`; the register sequence
   is `boot/src/watchdog.rs` since 3434554); never on the handshake path (B6). The application
   stage repeats the set-up (same values, idempotent). From its reload to the first reload of
   `setup_system` pass about 0.6 s (HSE probe <= 100 ms, the 500 ms delay).
2. bf2c2e6: B8, the record of the application part. Sector 0 holds at 0x08000240 16 bytes
   (`VDAC`, 0x08004000, length, CRC-32 of the image from 0x08004000 on; format in design
   §5.11), written into the .elf and the .bin after the link (`vdm-stm-image-check patch`,
   `tools/rust/stm/build_images.sh`). The boot stage computes the CRC in 8-byte steps in the
   idle time of the window's polling loop (`boot/src/app_check.rs`, `window_with`): no delay of
   the application, at most ~30 µs between two looks at the receiver on the 16 MHz HSI, 112 KiB
   in about 0.5 s. Without a match it opens the next window instead of the application, for
   ever, without IWDG and with the outputs safe: a half-flashed STM stays re-flashable and runs
   no stray code. The record lies behind the ID block, so the ESP's scan still meets the version
   and the board marker first (image check C1-C3 with the ESP's C++ code, C7).

**Verification.** Boot crate tests (CRC check value, record, every changed or erased byte, an
unpatched or out-of-range record, the steps, the window repetition without IWDG, the check
inside the first window); image check C1-C7 and D9 on the four images, C6 with the C++ 2.1.7
images; Renode E8 (a hang at the entry of `app::run` ends in an IWDG reset 8.0 s after the boot
stage started it; a fault at the entry of the boot stage still ends after 512 ms) and E12 (one
word of sector 2 changed: three windows without application and without IWDG, `BEEFIT` in the
third, the application again after the word is restored). E5 shows the application starting as
before the check (first IWDG write 3,013,413 µs after reset). Renode:

| tree | boot.robot E1-E10, E12 | app.robot A1-A6 | E11 C++ 2.1.7 -> Rust -> C++ 2.1.7 |
|---|---|---|---|
| the images of bf2c2e6, local, with the flasher of the base (sector 0 last) | pass on all four images (14 cases each) | pass on all four | pass on all four |
| f0342a6 (bf2c2e6, the boot-stage move 3434554, the ESP flasher's order of F9 4c04c36), GitHub CI run 37744471912 | pass on all four | pass on all four | pass on all four, `e11.py` `check_flash_order` with sector 0 first for the Rust images |

**Remaining.** Only the sector-0 pass itself (~2 s) leaves no valid vector table when it is cut
(§5.10); F9 covers the flash from C++. Images that were not patched never start their
application: flash only the files of `tools/rust/docker.sh fw` (README); the Rust release
packaging, when it is written, must take those.

## F2 (Low): the sensor match printed inside the motor lock

**Scenario.** The main loop reaches the motor state under `MotorLock::lock`, which raises
BASEPRI to P14 and masks TIM1 (1 ms: current, end stops, the 60/100 mA limits) and TIM2 (10 ms:
valve state machine). `masns` from the ESP (`glue/src/system.rs:435-439` at 92718f5), the
1-Wire search of `stons` or the daily re-scan (`system.rs:1075`) and an EEPROM re-read after a
failed read (`system.rs:1060`) ran `app_match_sensors` inside one lock. With 12-34 sensors it
prints 1-4 KB (`{ 28, ... }` and `found as ...` lines); the USART6 transmit ring holds 1023
bytes, the rest waits for the line at 11.5 bytes/ms: 80-250 ms with TIM1 and TIM2 masked. `masns`
and the re-read come at any time, also while a valve moves: its end stop or overcurrent is seen
that much later. A motor against its end stop for another 0.25 s does no damage, but design
§2.3 promised sections below ~80 µs, and the C++ prints with interrupts on.

**Fix.** 686e4f9: `App::app_find_sensors` matches and prints without the motor state,
`App::app_set_sensors` applies the indices; the three call sites print outside the lock and take
it only for the indices. The output order is unchanged (all system goldens byte-equal). A system
test logs the USART6 bytes written under the lock and fails on the old call sites.

## F3 (Low, open): short debug lines inside the motor lock

**Scenario.** Still inside the lock: the decision line of `app_loop` (`App: valve x unknown,
try to find out...`, only while no valve moves), `learning_movements: n` of a re-read, and the
learn-trigger lines of `app_10s_loop` (`glue/src/app.rs`, up to 24 lines, ~860 bytes, every
10 s). They wait only when the USART6 ring is already full, i.e. while the terminal output runs
above its line rate; worst case ~75 ms of masked TIM1 and TIM2.

**Status.** Accepted: no damage (a later end-stop stop), only with a saturated debug port.
Removing it would need a deferred debug buffer for every app function under the lock; not worth
the change of the 1:1 port now. Documented in design §2.3.

## F4 (Low, open): stack overflow

**Scenario.** The MPU guard covers 32 bytes at the bottom of the stack (0x20005020-0x2000503F in
the F401_C2 image of the base; `firmware/src/fault.rs` `stack_guard`). A push into it raises MemManage,
escalated to HardFault; stacking that exception frame into the guard again is a fault on
exception entry: lockup. No handler runs, the outputs keep their state until the IWDG resets
the chip (5.4-15 s). A frame larger than 32 bytes can also skip the guard (no stack probes on
thumbv7em) and corrupt `.uninit` and `.bss` silently.

**Status.** Accepted: the stack has ~44 KB above 7.4 KB of `.data`, `.bss` and `.uninit`
(F401), no recursion; the largest frames of the F401_C2 image are `app::run` 6.9 KB (it holds
the controller), `Controller::run` 0.9 KB, `MainLoop::step` 0.8 KB, `MainLoop::run` 0.5 KB, all
others below 0.4 KB, so the deepest path stays far below 44 KB. A guard of 1 KB would catch
frames up to that size too; not needed for the current depth.

## F5 (Info): embassy's blocking ADC read in the 1 ms interrupt

`FwAdc::sample` (`firmware/src/board.rs`) calls embassy-stm32 0.6.0 `blocking_read`,
whose `convert()` spins on STRT and EOC without a bound. A dead ADC would hang TIM1, starve TIM2
(same priority) and the main loop: IWDG reset within 5.4-15 s, a running motor stays on until
then (EXTI still stops a counted move). The C++ HAL polls with a timeout. Hardware failure only;
accepted.

## F6 (Info, proposal): no clock security system in the boot window

With an HSE ready within 5 ms the window runs on HSE without PLL (D1). An HSE that stops during
the 3 s window stops the CPU; the IWDG is off there (B6), so the STM hangs until the ESP's NRST
(the flasher asserts it anyway, the link policy within 10 min). Setting CSSON for the window
would switch to HSI and raise an NMI (`fault::stop`, IWDG 512 ms). The C++ is worse (its PLL from
HSE runs from reset). Not done: a design change for a fault inside 3 s windows.

## F7 (Info): failed switch to a ready HSE

The boot stage waits at most `SPIN_LIMIT` passes for SWS = HSE after selecting it
(`boot/src/stage.rs`, `spin_until(|| hw.sysclk_is_hse())`; `firmware/src/boot_hw.rs` at the
base). Should the switch not happen although HSERDY was set, the window runs on HSI with the
HSE values (SysTick 1.56 ms, BRR for 25 MHz: 73.7 kBd): no handshake in that boot, the next
reset retries. Not seen on healthy silicon; no change.

## F8 (Info): terminal commands in safe mode

`learn`, `open` and `close` on the debug terminal (`glue/src/terminal.rs:209-230`) hand commands
to the valve state machine also in safe mode; `sena` refuses there. The C++ terminal does the
same; USART6 is the bench port. Not a bug.

## F9 (Medium): the first flash from C++ 2.1.7 keeps the whole session as brick window

**Scenario.** The pinned C++ 2.1.7 release images keep their reset handler (0x0800F7D5) and every
fault vector (0x0800F825) in sector 3, SysTick (0x0800440B) in sector 1, `BootLoop` (its literal
pool at 0x0800A390) in sector 2 and the `DEADBEEF` string at 0x08010CFD in sector 4 (F401_C2;
F411 alike). D9 writes sectors 1..n first: once they are erased, the C++ sector 0 points into
erased flash, and any reset of the STM in the rest of the session ends in a lockup. Every
cabinet unit goes through one such flash (C++ -> Rust), and the brick window is the whole
session (~30-55 s), as with the C++ flasher.

**Decision** (2026-10-07): the ESP flasher writes sector 0 first when the new image
carries a valid record of F1 (checked against the image before anything is erased), sector 0
last otherwise. A cut after the sector-0 pass leaves the new Rust boot stage, whose check loops
its window. Brick windows then: C++ -> Rust, Rust -> Rust and Rust -> C++ ~2 s (the sector-0
pass), C++ -> C++ the whole session as before. Implemented on the ESP side in 4c04c36
(`vdm_esp_core::stm_flasher`) against the format of design §5.11; E11 checks the order of every
ROM session (`e11.py` `check_flash_order`), green in CI run 37744471912.

## Reviewed without findings

### Boot window after any reset (B1-B3, B6)

- Order: cortex-m-rt start-up (`.bss` 7.4 KB), capture, outputs safe, HSE probe (<= 5 ms on
  SysTick), USART1 8E1, 10 ms drop, 3001 calls. Listening from 2.8 ms (HSE) / 7.8 ms (HSI) after
  reset (Renode E10); the ESP 2.1's first `DEADBEEF` comes at 20 ms.
- Every hardware wait is bounded: SysTick-counted waits, `SPIN_LIMIT` on SWS, HSIRDY, TXE and
  TC; no interrupts, no embassy. B3: the boot crate denies the panic lints, the boot probe links
  without a panic handler.
- Fault loop: every cycle runs the window (E7, E8). IWDG before the window: never; a system
  reset (pin, software, IWDG) stops the IWDG, the option bytes keep the software watchdog (B4).
- Garbage in the no-init cells: magic and CRC (counter, guard), cold path. Garbage on USART1:
  fixed 8-byte blocks, the FIFO drops beyond 1023 bytes, ORE cleared by the SR/DR read; the
  window still ends after 3001 calls.
- D9: the boot stage, the fault handlers and every function and flash datum they reach lie in
  sector 0 (image check D9; the only data read outside it is the application part for the CRC
  of B8); the only indirect branch is the ROM jump.

### ROM jump (R2)

At the jump the Rust state is: SYSCLK = HSI (CFGR 0, SWS awaited), HSE, CSS and PLLs off (not
awaited, as harmless as in HAL_RCC_DeInit), SysTick off and at its reset values, PRIMASK set, no
NVIC line enabled, USART1 at 8E1 without interrupts, SYSCFG clock on, MEMRMP = 01, VTOR = 0
(cortex-m-rt without `set-vtor`), MSP from the ROM's vector. The C++ jumps with VTOR = 0x08000000
and the USART1 RXNE interrupt enabled in the NVIC (masked by PRIMASK). The Rust state is closer to
a start with BOOT0; only the bench proves the ROM (§5.9).

### Watchdog and faults

- The main loop reloads only when `valve_loop_ticks` advanced and the stall detector (5 min in
  one busy state) is clear; `setup_system` reloads between the long steps; every blocking step
  stays below 1 s (EEPROM load <= 0.8 s, 1-Wire search ~1 s per call, `reset` 200 ms), far below
  the 5.4 s of the fastest LSI.
- Unbounded waits of `embassy_stm32::init` (HSERDY after the probe, PLL lock) run under the IWDG.
- Faults, unexpected interrupts and panics: ENA0..5 and the PSU off by BSRR, `FaultRecord`, IWDG
  start key, spin. Panics in the 1 ms and 10 ms handlers and a nested motor lock end there too.
  The reset reason is the IWDG, so a fault loop enters safe mode after 3 resets in 10 min.
- `IsrCell::lock` raises BASEPRI to 0xE0 (embassy `prio-bits-4`: P14 = 0xE0, P1 = 0x10): TIM1 and
  TIM2 masked, EXTI4, the USARTs and the time driver run. `FwUsart::masked` sees PRIMASK and
  BASEPRI <= 0x10 correctly. A lock cannot nest by construction: the contexts handed into a lock
  (`AppCtx`) have no motor access.

### Motor

- The motor runs only through the soft start of TIM1 (`turning && go && !fin`) or the presence
  test (`MState::TestStart`); every end of a move goes through `motor_halt` (EXTI detached,
  `turning` false, soft start cancelled, ENA0..5 off).
- Bounds: soft start 10 ticks, undercurrent 2 s, normal current 120 s, presence test <= 0.93 s
  (7 settle samples, then 81 without current or 6 with it, at most 93 in a mix), stall 5 min
  (IWDG), PSU off after 15 s idle. TIM1
  switches ENA off in the 1 ms interrupt at the end-stop bound, at 60 mA for > 10 samples and at
  100 mA; EXTI switches it off at the target count.
- Race kept from the C++: EXTI's stop between TIM1's soft-start test and `ena_on` turns the motor
  on for at most 10 ms (TIM2 then stops it). A pending edge from before a detach counts once at
  the next attach (STM32duino does the same).
- Malformed commands: `stgtp` 0..100, `svmov` counts 1..10000 and 5..60 mA, `smotc`/`scalx`
  ranges in the core, `sfspo` 0..100 or 255, valve indices < 12 or 255; a target beyond the
  stroke ends at the mechanical end stop (current trip) or the 120 s timeout. `sena` on the
  terminal: 2 s or 60 mA, idle valve machine, not in safe mode.
- A soft reset in a move stops the motor (GPIO reset, the boot stage drives the outputs safe in
  its first microseconds); the warm state marks the position of a moving valve invalid.
- Pins during reset: PA15 (ENA4) has the JTAG pull-up until the boot stage, the PSU is off
  (PB9 floating, external pull-up), as in C++.

### Persistence

- EEPROM: write order C, B, 1.x layout, A with a CRC per block and the layout CRC in A; a torn
  block falls back to its shadow or default (core `resolve_config`), loaded values are
  sanitised; I2C waits are bounded (100 ms per phase, ready polling within 5 ms); a load stops
  after 3 failed transfers, a write at the first error; a cut I2C write without STOP changes
  nothing in the 24LC64.
- No-init cells: the C++ 2.1.7 layout at its addresses (E9, A4); the warm state with magic, CRC
  and range checks per valve; a reset in the middle of a write invalidates the CRC (cold start of
  that state). The Rust-only `FaultRecord` in `.uninit` has its own magic and check word.

### UART

- Rings: one producer and one consumer each (no interrupt handler prints), TXEIE without lost
  wake-ups, error codes per interrupt, a full RX ring drops and counts.
- `communication_loop`: at most 4 lines and 512 bytes per call, lines <= 128 bytes, an
  unterminated line expires after 100 ms of silence; arguments range-checked by the core
  tokenizer (fuzz-tested). A flood of valid requests blocks the main loop on TX for at most one
  call (~0.15 s), the IWDG is fed every pass.
- The ESP link recovers from garbage and from an ESP restart in the middle of a line; bytes of
  the 8E1 window are dropped at the set-up.

## Changed files for the mutation gate

The files the fixes of this review changed (d0b7aca, 686e4f9, bf2c2e6):

- vdm-stm-boot: `src/io.rs`, `src/stage.rs`, `src/window.rs`, `src/app_check.rs` (new),
  `src/test_support.rs`, `src/lib.rs`.
- vdm-stm-glue: `src/app.rs`, `src/system.rs` (bench and system tests changed too).
- not gated: `firmware/src/{app.rs, boot_hw.rs, fault.rs}`, `firmware/memory/vdm.x`,
  `image-check/src/{main.rs, elf.rs, sector0.rs}`, `renode/{boot.robot, vdm.resource}`,
  `tools/rust/{renode.sh, stm/build_images.sh, README.md}`.
