# VdMot Revamped STM32 in Rust: glue and firmware design

Design of the Rust port of `software_stm32/src` (the glue) and of the firmware around
`vdm-stm-core`. The implementation in `software_stm32_rust/` follows it; where it departs from
the first design, the text says so (*Implementation:*). Binding documents: `docs/rust/PORTING.md`,
`software_stm32/PROTOCOL_V2.md`, `software_esp32_revamped/DESIGN.md` §15. Values marked
*measured* come from the four C++ 2.1.7 release envs built with PlatformIO `ststm32@19.7.0`;
embassy, cortex-m-rt and Renode facts come from the sources of the `embassy-stm32-v0.6.0` tag,
of `cortex-m-rt` and of `renode-infrastructure`.

**Requirement (operator):** flashing never forces anyone to open the cabinet. The ESP drives
NRST but not BOOT0, so an STM image can be re-flashed remotely only through its own boot
window (`DEADBEEF` -> `BEEFIT` -> ROM bootloader). Every Rust image keeps that path, and code
added later cannot break it (§5).

## 1. Glue structure

### 1.1 Crates

| crate | path | builds for | std | unsafe | content | verified by |
|---|---|---|---|---|---|---|
| `vdm-stm-core` | `core/` | host, device | `no_std`, no alloc | forbidden | port of `lib/core` (separate agent) | host tests, mutation 95 % |
| `vdm-stm-boot` | `boot/` | host, device | `no_std`, no alloc | forbidden | boot window logic (`otasupport.cpp`), reset capture (`sysstat_capture_reset`), ID block layout | host tests, mutation 95 %, panic-free link probe (§5.1) |
| `vdm-stm-glue` | `glue/` | host, device | `#![cfg_attr(not(test), no_std)]`, no alloc | forbidden | logic of `src/*.cpp` and of the library code it uses, over HAL traits | glue suites on the host, mutation 95 % |
| `vdm-stm-fw` | `firmware/` | `thumbv7em-none-eabihf` | `no_std` | 5 files | embassy adapters, ISRs, boot stage registers, linker scripts, ID block | Renode suite, image check |
| `vdm-stm-image-check` | `image-check/` | host | std | forbidden | ESP image validation and ELF layout checks of the four images | CI |

Decision on the glue (PORTING.md left it open): the glue runs on the device, so it is
`no_std` without an allocator, unlike the ESP glue; its tests get `std` through `cfg(test)`.
Workspace members: `core`, `boot`, `glue`, `image-check`; `firmware` is excluded (own target
and lock file).

Rules:
- No `static` in boot or glue. All state lives in structs; the firmware owns the instances,
  the ISR-shared ones in statics (§2.3).
- Calls between glue modules go through one `<Module>Env` trait per module: the link seams of
  the C++ suites. `Controller` implements them by delegation; tests implement them with stubs
  that log calls (port of `test/native/glue/stubs`).
- Every decision lives in boot or glue (mutation-gated). The firmware maps traits to registers
  and has no branching beyond that.
- `unsafe` only in `firmware/src/{main.rs, boot_hw.rs, isr.rs, noinit.rs, fault.rs}`, each
  block with a `SAFETY:` comment.

### 1.2 Module map

| C++ (lines) | Rust module | crate | note |
|---|---|---|---|
| `main.cpp` (267) | `main_loop.rs`; `main.rs` | glue; fw | setup order, `loop_system` branches, watchdog feed; entry |
| `app.cpp` / `app.h` (970 / 141) | `app.rs` | glue | |
| `motor.cpp` / `motor.h` (1646 / 181) | `motor.rs`, `motor/pulse.rs` | glue | ISR entry points (§2) |
| `communication.cpp` / `.h` (1017 / 110) | `communication.rs` | glue | §4.1 |
| `eeprom.cpp` / `.h` (335 / 65) | `eeprom.rs` | glue | §3.1 |
| I2C_eeprom 1.9.4 (library) | `eeprom24.rs` | glue | used subset (§3.1) |
| `i2c_bus.cpp` (52) | `i2c_bus.rs` | glue | bus recovery over a pin trait |
| STM32 core `Wire` (twi.c) | `i2c_master.rs` | glue | the I2C v1 register sequence over `I2cRegs` (§1.3) |
| `owDevices.cpp` / `.h` (372 / 72) | `ow_devices.rs` | glue | |
| OneWire 2.3.7, patched (library) | `onewire.rs` | glue | search, CRC-8, select, byte I/O; bit timing in fw |
| DallasTemperature 3.9.0 (library) | `dallas.rs` | glue | `begin`, `requestTemperatures`, `getTemp`, `millisToWaitForConversion`, `getResolution`, `validAddress`, `validFamily`, `getDeviceCount` |
| `DS2438.cpp` (467) | `ds2438.rs` | glue | `setAddress`, `readVAD` |
| `terminal.cpp` / `.h` (517 / 58) | `terminal.rs` | glue | §4.2 |
| `sysstat.cpp` (78) | `capture.rs`; `sysstat.rs` | boot; glue | capture before the window; uptime and safe mode in the loop |
| `otasupport.cpp` (97) | `window.rs`; `boot_hw.rs` | boot; fw | logic; registers |
| `boot_jump.cpp` (118) | `boot_hw.rs` | fw | |
| `hardware.h`, `compile_time.h` | `board.rs`; `build.rs` | glue; fw | pin map, `BoardRev` (C1/C2 MUX level); ID block, version |
| Arduino `Print`, `HardwareSerial` | `print.rs`, `serial.rs` | glue | number formatting; rings and HAL UART error codes |
| STM32_TimerInterrupt 1.3.0 / `HardwareTimer` | `hw_timer.rs` | glue | PSC/ARR formula (§2.4) |
| `rs485.cpp` (96) | — | — | not built today, not ported |
| ArduinoMenu 4.21.5, ArduinoJson 6.21.6 | — | — | dropped: no source includes them; the 2.1.7 link loads the ArduinoMenu archive but takes no member (*measured*, map file) |

What stays in `firmware/` (embassy and PAC, no decisions):

| file | content |
|---|---|
| `main.rs` | `#[entry]`: `boot::run()`, then `app::run(token)`; nothing else |
| `boot_hw.rs` | `BootIo` on registers: RCC, GPIOA/C, USART1 polled, SysTick, ID block reads, the jump |
| `app.rs` | the application stage: IWDG start, HSE probe (`vdm_stm_boot::stage::probe_app_hse`), MPU guard, `embassy_stm32::init`, pin modes, the shared state, the serial ports, then `Controller::run` of the glue |
| `clocks.rs` | `embassy_stm32::Config` from the probe result (§5.3) |
| `board.rs` | the HAL trait implementations of §1.3 and the glue's `Platform` (`Fw`) |
| `i2c.rs`, `one_wire.rs` | the I2C1 registers under the glue's master, the 1-Wire line (§1.3) |
| `isr.rs` | `TIM1_UP_TIM10`, `TIM2`, `EXTI4`, `USART1`, `USART6` handlers, `IsrCell` (§2.3) |
| `noinit.rs`, `fault.rs` | `NOINIT` access (§3.2); fault handlers, panic handler, MPU guard (§5.5) |
| `memory/*.x`, `build.rs` | linker scripts per chip (§5.6), ID block and version (§6.2) |

### 1.3 HAL traits and their implementation

Trait signatures (glue `hal.rs`):

```rust
pub trait Pins {                                   // callable from any context
    fn set(&self, pin: Out, high: bool);           // Ena0..Ena5, Dir, Mux, PsuEna, Led
    fn latch(&self, pin: Out) -> bool;             // output latch (LED toggle)
    fn read(&self, pin: In) -> bool;               // Button, RevIn
}
pub trait CurrentAdc { fn sample(&mut self) -> (u16, u16); }   // (PA0 current, PA1 ref/2)
pub trait RevIrq { fn attach(&self); fn detach(&self); }       // EXTI4, rising edge
pub trait Clock {
    fn millis(&self) -> u32; fn micros(&self) -> u32;
    fn delay_ms(&self, ms: u32); fn delay_us(&self, us: u32);
}
pub trait Serial {
    fn available(&self) -> usize; fn read(&mut self) -> Option<u8>;
    fn write(&mut self, bytes: &[u8]);             // blocks while the TX ring is full
    fn flush(&mut self);                           // TX ring empty and TC set
}
pub trait I2cMaster {
    fn write(&mut self, addr: u8, bytes: &[u8]) -> WireStatus;  // 0 ok, 2 nack, 4 error, 5 timeout
    fn read(&mut self, addr: u8, buf: &mut [u8]) -> usize;
    fn restart(&mut self);                         // end, recover the bus, begin
}
pub trait OneWireLine { fn reset(&mut self) -> bool; fn write_bit(&mut self, bit: bool); fn read_bit(&mut self) -> bool; }
pub trait Watchdog { fn reload(&mut self); }
pub trait NoinitStore { fn read(&self) -> [u8; 212]; fn write(&mut self, image: &[u8; 212]); }
pub trait System { fn reset(&self) -> !; fn dev_id(&self) -> u16; }
```

| trait | C++ origin | firmware implementation (embassy-stm32 0.6.0; PAC through feature `unstable-pac`) |
|---|---|---|
| `Pins` | `pinMode`/`digitalWrite`/`digitalRead`: ENA0..5 (PA5, PA6, PA7, PB0, PA15, PB3), DIR PA8, MUX PB1, PSU PB9 (open drain, low = on), LED PC13, BUTTON PB2 (pull-up), REVIN PA4 | set-up with `gpio::Output::new(pin, Level::Low, Speed::Low)`, `gpio::OutputOpenDrain::new(p.PB9, Level::High, ..)` (latch high before open drain, as `valve_pins_safe`), `gpio::Input::new(p.PB2, Pull::Up)`; writes through `pac::GPIOx.bsrr()`: atomic, so TIM1, TIM2, EXTI and the main loop may all switch ENA |
| `CurrentAdc` | two `analogRead` in `TimerHandler0`: PA0 then PA1, 12 bit, 15 cycles, PCLK2/4 | `adc::Adc::new(p.ADC1)`, `blocking_read(&mut pa0, SampleTime::CYCLES15)` then PA1; the prescaler from PCLK2 is /4 (21 MHz F401, 24 MHz F411) like STM32duino; owned by the TIM1 context |
| `RevIrq` | `attachInterrupt(REVINPIN, isr_count, RISING)`, `detachInterrupt` | `exti::ExtiInput::<Blocking>::new_blocking(p.PA4, p.EXTI4, Pull::None, TriggerEdge::Rising)` (feature `exti`; no `bind_interrupts!`, own handler); attach = `enable_interrupt()` + NVIC EXTI4 enable; detach = `disable_interrupt()` + NVIC disable; neither clears a pending edge (STM32duino does not either); IMR changes in a critical section. *Implementation:* `ExtiInput` only configures PA4 and the rising edge once; attach and detach (`isr::rev_irq`) set EXTI IMR line 4 through the PAC in a critical section and switch the NVIC line, because the board handle is a `Copy` value used from the handlers and cannot own the `ExtiInput` |
| control timers (fw) | `STM32Timer` on TIM1 (1 ms) and TIM2 (10 ms) | `timer::low_level::Timer::new(p.TIM1 / p.TIM2)`; PSC and ARR from `hw_timer::overflow()` through `regs_core()`; `enable_update_interrupt(true)`, `start()`; the ISR calls `clear_update_interrupt()`. `set_frequency` / `set_period_us` are not used: their `calculate_psc_arr` counts the 32-bit TIM2 without prescaler (10.000 ms) where the C++ runs 9.99994 ms |
| `Serial` | `HardwareSerial` Serial1 (USART1 PA9/PA10) and Serial6 (USART6 PA11/PA12), rings 1024/1024 | `usart::Uart::new_blocking(peri, rx, tx, Config { baudrate: 115_200, .. })` sets pins, clock, BRR and frame; RXNE/TXE/error interrupts by own `#[interrupt] fn USART1/USART6` on `pac::USARTx` with the `serial.rs` logic. `BufferedUart` is not used: it has no error counters and another overflow rule. *Implementation:* glue `serial::Port` (the two rings and the counters, lock-free between one interrupt and thread mode) and `PortSerial` over the firmware's `UsartTx` (`FwUsart`: TXEIE on, SR, DR). A write that finds the USART interrupt masked (PRIMASK, or BASEPRI at P1 or above) sends from the ring itself by polling TXE, so output from a critical section cannot wait for an interrupt that cannot come |
| `I2cMaster` | `Wire`: I2C1 PB6/PB7, 100 kHz, 100 ms per phase (`I2C_TIMEOUT_TICK`) | `i2c::I2c::new_blocking(p.I2C1, p.PB6, p.PB7, Config { frequency: 100 kHz, timeout: 100 ms, .. })`; `blocking_write` and `blocking_read` as separate transactions with a STOP between (as `endTransmission` + `requestFrom`); `Error::Nack` -> 2, `Timeout` -> 5, others -> 4; `restart()` drops the driver, runs `i2c_bus::recover` on `gpio::Flex` pins and creates the driver again. *Implementation:* an own blocking master, not the embassy v1 driver: its `blocking_write` of zero bytes (the address probe of the EEPROM's ready polling) waits for BTF, which never comes without a data byte, and a NACK leaves the bus without a STOP. The sequence is glue (`i2c_master.rs`, `I2cV1` over the trait `I2cRegs`, host-tested against a model of the I2C v1 peripheral); the firmware only maps `I2cRegs` to `pac::I2C1` (`firmware/src/i2c.rs`). Each transfer has its own START and STOP; every wait is bounded by 100 ms; NACK -> 2 with STOP, timeout -> 5, ARLO/BERR -> 4, a bus busy for 100 ms -> 4 without START; a read clears ACK and sets STOP before its last byte (one byte: before ADDR is cleared, RM0368 §18.3.3); `restart()` and the set-up recovery run `i2c_bus::recover` with the peripheral off and the lines as GPIO |
| `OneWireLine` | patched OneWire 2.3.7: open drain on PB10, `noInterrupts()` around each timed window | `gpio::Flex::new(p.PB10)` + `set_as_input_output(Speed::Medium)` (open drain with input); each timed window inside `critical_section::with`; µs waits on the DWT cycle counter like STM32duino `delayMicroseconds`; slot values per risk R5. *Implementation:* `firmware/src/one_wire.rs`, windows in `cortex_m::interrupt::free` (the single-core critical section), the line through BSRR and IDR |
| `Watchdog` | `IWatchdog.begin(8000000)`, `reload()` | `pac::IWDG` key sequence: it starts before `embassy_stm32::init` (§5.4). Same registers as `wdg::IndependentWatchdog::new(p.IWDG, 8_000_000)` + `unleash()` / `pet()` (PR /64, RLR 3999), which needs the peripherals only `init` hands out |
| `Clock` | `millis`, `micros`, `delay`, `delayMicroseconds` | `embassy_time::Instant::now()` (time driver `time-driver-tim5`, 1 MHz tick) plus the boot-window offset (§5.2); `embassy_time::block_for` |
| `NoinitStore` | `__attribute__((noinit))` cells | volatile word copies of the `NOINIT` linker region (§3.2) |
| `System` | `HAL_NVIC_SystemReset`, `HAL_GetDEVID` | `cortex_m::peripheral::SCB::sys_reset()`; `pac::DBGMCU.idcode().read().dev_id()` |

Interrupt set-up: `#[interrupt]` handlers with `embassy_stm32::interrupt::{InterruptExt, Priority}`
(`interrupt::TIM2.set_priority(Priority::P14)`, `unsafe { interrupt::TIM2.enable() }`), the
pattern of the embassy `stm32f4` multiprio example. No PWM: the C++ switches ENA0..5 as plain
GPIO (the soft start is the TIM1 hand-shake of §2.1), so the TIM2/TIM3 functions of those pins
stay unused.

### 1.4 Boot crate interface

```rust
pub trait BootIo {                                 // PAC-only implementation in firmware/src/boot_hw.rs
    fn rx(&mut self) -> Option<u8>;                // polled USART1 (8E1)
    fn tx_all(&mut self, bytes: &[u8]);            // polled, then waits for TC
    fn set_led(&mut self, high: bool); fn led(&self) -> bool;
    fn wait_ms(&mut self, ms: u32, fifo: &mut Fifo); // SysTick ticks, RX drained into fifo meanwhile
}
pub fn capture_reset(csr: u32, cells: &mut [u8; 212]) -> ResetInfo;   // core classify/count/guard
pub fn window(io: &mut impl BootIo, id: &ImageId) -> WindowEnd;        // Update | Timeout
```

## 2. Real-time model

### 2.1 The C++ (2.1.7)

| context | trigger, rate | NVIC priority | work |
|---|---|---|---|
| SysTick | 1 kHz | 0 | HAL tick (`millis`) |
| USART1, USART6 | per byte | 1 | ring buffers; USART1 counts HAL error codes (`comm_rx_irq`) |
| EXTI4 (PA4 rising) | per motor revolution pulse | 6 | `isr_count`: `isr_counter++` while turning; `isr_target--`, at 0 `callback_motorstop` (detach, ENA off, `isr_stop_request`) |
| TIM1 update (`TIM1_UP_TIM10`) | 1 ms | 14 | `TimerHandler0`: (PA0 - PA1) x 138 / 100 in 0.1 mA; test filter (old x 9800 + v x 200) / 10000; while turning `EndStopDetector::sample`, a trip detaches EXTI, switches ENA off and sets `isr_overcurrentevent`; otherwise `idle()` and current 0; soft start: ENA on when turning && go && !fin |
| TIM2 update | 10 ms | 14 | `valve_loop`: valve state machine (A_*), `motorcycle` (M_*), heartbeat `valve_loop_ticks`, stall detector |
| main loop | busy loop, thread mode | — | `loop_system`, three branches below; handovers under `__disable_irq` |

| main loop branch | runs when | work |
|---|---|---|
| 1000 ms | more than 1000 ms passed (`>`): every 1001 ms | `app_1s_tick(real s)`, every 10th pass `app_10s_loop(real s)`, `eepromloop` |
| 100 ms | > 100 ms | LED off at the 30th, on at the 31st pass; `Terminal_Serve`; button line |
| 10 ms | > 10 ms | IWDG reload only if `valve_loop_ticks` advanced and not stalled; `app_loop`, `app_warm_save`, `communication_loop`, `temperature_loop`, `terminal_supervise` |

- STM32duino sets `NVIC_PRIORITYGROUP_4`: 16 preemption levels, no sub-priority.
- TIM1 and TIM2 share priority 14: they never preempt each other, and on a common update the
  NVIC serves TIM1 (IRQ 25) before TIM2 (IRQ 28). EXTI preempts both.
- The motor starts only through TIM1: M_TURNON sets `isr_timer_go`, the next 1 ms tick
  switches ENA on and sets `isr_timer_fin`, the next 10 ms tick moves to M_TURNING.
- 1-Wire bit slots mask interrupts for up to ~80 µs (reset sample 70 µs, write-0 65 µs). New
  temperature conversions are locked while a valve moves (`TEMP_CMD_LOCK`); the reads of a
  running cycle go on.
- `delay()` (500 ms in setup, 200 ms in `reset`, 10 ms per DS2438 read) blocks the main loop
  only; the ISRs keep running.

| constant (`motor.cpp`) | value | meaning |
|---|---|---|
| `MUX_SETTLE_TICKS` | 10 ticks = 100 ms | relay settles after `set_motor` |
| `WAIT_TIMER20` / `50` / `100` | 40 / 100 / 200 ticks = 0.4 / 1 / 2 s | waits after PSU on, between strokes, before the presence test |
| `TIMEOUT_TURNON` | 10 ticks | TIM1 must enable the motor |
| `TIMEOUT_UNDERCURRENT` | more than 200 ticks (in total, not in a row) below ±2.0 mA, checked from the 4th tick on | open circuit |
| `TIMEOUT_NORMALCURRENT` | more than 12,000 normal-current ticks = 120 s | move timeout |
| `TIMEOUT_VALVESTATE` | 30,000 ticks = 5 min | stuck state machine, the watchdog starves |
| `TIMEOUT_TEMPGAP` | 300 ticks = 3 s | temperature pause between calibration strokes |
| PSU off, temperature unlock | more than 1,500 / 50 idle ticks | |
| mean current | one sample every 51st tick after the first 50 | |
| end-stop limits | core `EndStopDetector` | filter alpha 0.02, 250 ms inrush hold, 60 mA safety after > 10 samples, 100 mA hard (PROTOCOL_V2) |

### 2.2 Mapping to Rust and embassy

| context | Rust | priority | glue entry |
|---|---|---|---|
| time base | embassy-time driver on TIM5 (16-bit periods of 2^15 ticks at 1 MHz: IRQ every 32.8 ms) | P0 (embassy default) | `Clock` |
| USART1, USART6 | `#[interrupt] fn USART1 / USART6` | P1 | `serial::Port::on_irq(sr, dr)` |
| EXTI4 | `#[interrupt] fn EXTI4` | P6 | `motor::Pulse::on_edge` |
| TIM1 | `#[interrupt] fn TIM1_UP_TIM10` | P14 | `motor::timer_handler0` |
| TIM2 | `#[interrupt] fn TIM2` | P14 | `motor::valve_loop` |
| main loop | `app::run`: busy superloop, no executor | thread | `MainLoop::step` (= `loop_system`) |

`AIRCR.PRIGROUP` keeps its reset value 0: with 4 priority bits every bit is a preemption bit,
the same as `NVIC_PRIORITYGROUP_4`.

- Hardware-timer ISRs, not an `InterruptExecutor`: an executor at P14 wakes tasks through
  time-driver alarms and polls them, so the 1 ms and 10 ms rates would follow the time driver,
  and the order TIM1 before TIM2 and their mutual non-preemption would no longer hold.
- Blocking ADC in the 1 ms ISR, not DMA: the C++ converts PA0 and PA1 back to back in that
  ISR; a TIM1-triggered DMA scan adds a trigger chain and a second ISR for the same samples.
  The Rust TIM1 ISR is shorter than the C++ one, which re-initialises the ADC per
  `analogRead` (~20-40 µs per read), and PA1, the steady reference, follows PA0 closely.
  *Implementation:* embassy 0.6.0 `blocking_read` also powers the ADC off and on per read
  (`stop`, `enable` with a 3 µs wait on a cycle loop, channel and sequence set-up), so a read
  takes about 5 µs and PA1 follows PA0 by about 5 µs.
- No executor in the main loop: the branch order and the `> period` conditions are behaviour;
  every `delay()` stays blocking (`block_for`). `embassy-executor` 0.10.0 is not linked (D6).

### 2.3 Shared state

| C++ data | used by | Rust |
|---|---|---|
| `isr_counter`, `isr_target`, `isr_turning`, `isr_stop_request` | EXTI <-> TIM1, TIM2 | `motor::Pulse`: `AtomicU32` / `AtomicBool` fields; EXTI preempts, so no lock |
| `myvalvemots[]`, `myvalves[]`, `command`, `valvenr`, `poschangecmd`, `moveflagscmd`, `svc_*`, `stop_request`, `calib_escalation`, end-stop detector, move state, `valve_records`, `isr_valvenr/go/fin`, `isr_overcurrentevent`, `current_mA`, `analog_current` | TIM1 + TIM2 (same priority) <-> main | `motor::MotorShared` in a firmware `IsrCell<MotorShared>`: `IsrCell::isr` only from the two P14 handlers (debug assertion on the active vector), `IsrCell::lock` from the main loop = `critical_section::with` (PRIMASK), as `__disable_irq`. *Implementation:* `IsrCell::with_isr` in the P14 handlers, the glue's `MotorLock::lock` in thread mode raises BASEPRI to P14 (`basepri_max`), so only TIM1 and TIM2 wait while EXTI4 and the USARTs go on; a nested lock or a use before `init` ends in the fault handler (§5.5) |
| `valve_loop_ticks`, `valve_loop_stalled`, temperature `lock`, `temp_refresh_request`, `temp_gap_timeout`, `protect_suspended`, `protect_enforce` | TIM2 <-> main | atomics in `motor::IsrFlags` |
| `warm_state` (also written by `app_warm_moving` inside the `appsetaction` hand-over) | main only | `NoinitStore`, main context |
| ADC | TIM1 only | owned by the TIM1 context |

- The C++ reads and writes some motor fields from the main loop without masking (status and
  positions while `valve_idle()`); safe Rust takes `lock()` there. The `app_loop` pass runs in
  one lock while no valve moves; a request handler holds the lock only for its field copies.
  Budget: every critical section stays below the 1-Wire masking of the C++ (~80 µs); EXTI
  latches an edge in its pending bit, so a shorter section cannot lose a pulse.
- Debug output inside the lock waits for USART6 when the 1023-byte transmit ring is full, with
  TIM1 and TIM2 masked meanwhile (the C++ prints with interrupts on). The sensor match (`masns`,
  the 1-Wire search, an EEPROM re-read: up to 34 sensors, 1-4 KB of lines) therefore prints
  outside the lock (`App::app_find_sensors`) and applies the indices under it
  (`App::app_set_sensors`); the set-up still matches inside, before TIM1 and TIM2 run. The
  short lines printed inside (a decision of `app_loop`, `learning_movements` of a re-read, the
  learn triggers of `app_10s_loop`, at most 24 lines) wait only when the ring is full already.
- `MotorShared::new()` is `const`: the static needs no lazy initialisation. The TIM1 context
  (ADC, channels) is set once before its interrupt is enabled.

### 2.4 Timer periods

`hw_timer::overflow(timer_clk_hz, period_us)` is the `HardwareTimer::setOverflow`
(MICROSEC_FORMAT) formula that STM32_TimerInterrupt uses: `cyc = us x clk/1e6`,
`factor = cyc/65536 + 1`, `PSC = factor - 1`, `ARR = cyc/factor - 1`.

| chip, timer clock | TIM1, 1 ms | TIM2, 10 ms |
|---|---|---|
| F401, 84 MHz | PSC 1, ARR 41,999 (1.000 ms) | PSC 12, ARR 64,614 (9.99994 ms) |
| F411, 96 MHz | PSC 1, ARR 47,999 (1.000 ms) | PSC 14, ARR 63,999 (10.000 ms) |

## 3. Persistence

### 3.1 I2C EEPROM

24LC64 (8 KiB) at 0x50 on I2C1 (PB6 SCL, PB7 SDA), 100 kHz. The layout belongs to
`vdm-stm-core` (`eeprom_layout`, `config_store`); `PROTOCOL_V2.md` documents it:

| range | content |
|---|---|
| 0x0007-0x013B | 1.x layout (309 B), byte-identical to 1.x |
| 0x013C-0x014B | block A "settings" v3: escalation, learn time, lease timeout, CRC-16 over the 1.x layout, CRC-8 |
| 0x0160-0x017F | block B "safety" v1: failsafe positions, copy of the motor fields and the lease timeout, CRC-8 |
| 0x0180-0x023F | blocks C0..C11 "calibration", 16 B each |

| layer | behaviour kept |
|---|---|
| `eeprom24.rs` (I2C_eeprom 1.9.4 subset) | a write is split at 30 bytes (`I2C_BUFFERSIZE` for STM) and at 32-byte page boundaries; each chunk first polls the device (address-only write) until it acknowledges, at most 5 ms after the previous write; status 0 = ok, the first error ends the block. A read goes in 30-byte chunks (address write, STOP, read); a failed chunk adds 0 bytes and the read continues; the caller compares the sum with the length |
| `eeprom.rs` (`eeprom.cpp`) | write order C(v), B, 1.x layout, A; stop at the first I2C error; a load allows 3 failed transfers in total and fills unread blocks with 0xFF; `StoreScheduler` (write 3 s after the last change, at most 30 s after the first, retries 30 s .. 1 h); bus restart before every retry; re-read after a failed read merges the changes made meanwhile (`mergeChanges`); counters cfgFlags, cfgEvents, eepWrites |
| worst case on a bad bus | 100 ms per I2C phase (embassy `Config::timeout`, as `I2C_TIMEOUT_TICK`): a load stays below ~0.8 s, a write step below ~0.3 s, inside the IWDG budget of `main.cpp` |

Parity proof: the core layout tests, plus golden 8 KiB images exported from the C++
`glue_system` scenarios that the Rust system tests reproduce byte for byte (§7.4).

### 3.2 Warm state and reset cells in no-init RAM

C++ 2.1.7, the same in all four release envs (*measured*, map files): the STM32duino linker
script puts `.noinit` right after `.bss`:

| cell | address | size | C++ source |
|---|---|---|---|
| `warm_state` (`vdm::WarmState`, 178 B + 2 B padding) | 0x20003234 | 180 B | `app.cpp` |
| `guard_cell` (`vdm::ResetGuardCell`) | 0x200032E8 | 20 B | `sysstat.cpp` |
| `reset_cell` (`vdm::ResetCounterCell`) | 0x200032FC | 12 B | `sysstat.cpp` |

The startup code never clears it, so it survives pin, software and watchdog resets. It also
survives a flash: the ROM bootloader uses 0x20000000-0x20002FFF (12 KiB; the stm32flash device
table starts user RAM at 0x20003000 for PIDs 0x423, 0x431, 0x433). So the NRST after a C++ to
C++ flash is a warm boot (`Pin`) that restores every valve without a presence test (derived
from the code and that RAM range; the bench check of §5.9 confirms it, R3).

The Rust image keeps the three cells at the same addresses with the same bytes (D3):

```
/* firmware/memory/f401.x; f411.x: FLASH 512K, RAM LENGTH 0x1CCF8 */
MEMORY
{
  FLASH  : ORIGIN = 0x08000000, LENGTH = 256K
  RAM_LO : ORIGIN = 0x20000000, LENGTH = 0x3234   /* reserved, unused (headroom) */
  NOINIT : ORIGIN = 0x20003234, LENGTH = 0xD4     /* C++ 2.1.7 .noinit: warm, guard, counter */
  RAM    : ORIGIN = 0x20003308, LENGTH = 0xCCF8   /* .data .bss .uninit; stack from the top */
}
PROVIDE(__vdm_noinit = ORIGIN(NOINIT));
```

- No Rust object lives in `NOINIT`. `firmware/src/noinit.rs` copies the 53 words with volatile
  accesses, like a peripheral, so no uninitialised Rust memory is ever read; core and glue see
  byte images (`from_bytes` / `to_bytes` with the C++ offsets and CRCs).
- cortex-m-rt initialises only `.data` and `.bss` inside `RAM`; `NOINIT` lies outside it.
- The stack starts at the top of RAM as in C++ (SP0 0x20010000 F401, 0x20020000 F411), so
  the ESP check "SP <= RAM top of the PID" keeps rejecting an F411 image for an F401 chip.
- A Rust-only `FaultRecord` (§5.5) lives in cortex-m-rt's `.uninit`, outside the C++ range,
  with its own magic and CRC.

| event | C++ 2.1.7 | Rust |
|---|---|---|
| power-on | random content, invalid -> cold start | the same |
| pin, software, watchdog reset | kept | kept |
| flash C++ -> C++, Rust -> Rust | kept | kept |
| flash C++ 2.1.7 <-> Rust | — | kept (same addresses and layout) |
| flash with a C++ build whose `.bss` size differs from 2.1.7 | moves | cold start once (magic/CRC reject) |

## 4. UART protocol and debug terminal

### 4.1 ESP link (USART1, PA9 TX / PA10 RX)

| item | behaviour (port of `communication.cpp`) |
|---|---|
| framing | 8E1 only inside the boot window (§5.2); 115200 8N1 afterwards (PROTOCOL_V2 "Framing") |
| rings | 1024 slots each, 1023 bytes usable (STM32duino head/tail rule); TX blocks while full |
| RX interrupt | per interrupt one error code like the F4 HAL: PE (parity enabled), NE, FE, ORE from SR; the byte of an interrupt with an error is still stored; `vdm::countUartErrors` (core) counts the code and a byte that finds the ring full (dropped) -> `gstax` uartOre, uartFe, uartNe, rxDropped |
| set-up order | the RX interrupt runs before the slow set-up steps (500 ms delay, EEPROM, 1-Wire), so requests sent meanwhile wait in the ring as in C++; bytes of the 8E1 window are lost (USART1 reset after the boot stage) |
| `communication_loop` | at most 4 lines and 512 bytes per call; line assembler 128 (core); tokenizer, at most 5 arguments (core); an unterminated line expires after 100 ms, only when no byte waits |
| dispatch | the `communication.cpp` chain; v1 replies through `print.rs` (Arduino `Print`: decimal, `println` = CR LF); v2/v3 replies formatted by core into one reply buffer of `kProfileReplyMaxLen + 1`; unknown commands get no reply |
| `gvers` | `gvers <version>_<tag> <build> ` + CR LF, version and tag read from the ID block (§6.2), build `1` in release images |
| `ghwin` | `DBGMCU_IDCODE & 0xFFF` in decimal (1059 F401, 1073 F411) |
| `reset` | reply, 200 ms blocking, soft reset when no EEPROM write waits |

### 4.2 Debug terminal (USART6, PA11 TX / PA12 RX, 115200 8N1)

Proposal: keep the existing terminal, minimal and 1:1 (D7).

- It is not ArduinoMenu: `terminal.cpp` is a tokenizer terminal (128-byte lines, 3 arguments,
  at most 256 bytes per call from the 100 ms branch) with `help`, `learn`, `open`, `close`,
  `settar`, `smux`, `sdir`, `sena` (2 s or 60 mA limit, `terminal_supervise`), `getone`,
  `seteep`, `saveep`, `stsnx`, `stsny`, `stvls`, `gvlon`, `gvers`, `stons`, `stlnt`, `staop`,
  `staln`, `smotc`, `gmotc`, `stdet`, and the start banner.
- ArduinoMenu is dropped: listed in `lib_deps` only, included by no source, no code in the image.
- The `commDebug` / `appDebug` lines (on in every C++ release env) are kept 1:1 through a
  `DebugOut` writer on USART6. The glue suites assert some of them.
- Everything sits behind the cargo feature `terminal` (default on), so it can go later
  without touching the ESP contract. Estimated cost 3-5 KiB of flash.

## 5. Boot and flashing safety

### 5.1 Invariants

| # | invariant | how it is enforced |
|---|---|---|
| B1 | every reset runs the boot stage before any other code except cortex-m-rt's start-up (FPU on, RAM init) | `#[entry]` holds only `let t = boot::run(); app::run(t)`; `app::run` needs a `BootToken` that only `boot::run` creates; Renode E1, E10 |
| B2 | the boot stage needs nothing that can fail or wait without a bound | reset clock or an HSE that is ready within 5 ms (§5.3); polled USART1 and SysTick; no interrupts, allocator, executor or `embassy_stm32::init`; no flash writes or option-byte access (it reads its own flash: ID block, B8); stack only. Only the windows repeat without end, and only under B8 |
| B3 | the boot stage cannot panic or fault | `vdm-stm-boot` denies `clippy::{indexing_slicing, unwrap_used, expect_used, panic, arithmetic_side_effects}`; a probe binary links `boot::run` and the core functions it calls with a panic handler that does not exist (`panic-never` pattern): any panic path fails the link; registers only at fixed PAC addresses |
| B4 | the application never changes state that outlives a reset | no option bytes, no internal-flash writes (`embassy_stm32::flash` not used; clippy `disallowed-methods` on the FLASH key and option registers), no backup domain |
| B5 | every failure ends in a reset that reaches the boot stage | faults and panics -> IWDG reset (§5.5); hangs after the boot stage -> IWDG; the ESP's NRST (flasher, reset policy of DESIGN.md §5) covers a hung boot stage, which B2/B3 exclude |
| B6 | the IWDG is never started before the window ends | a running IWDG would reset the ROM bootloader session (§5.4) |
| B7 | the image carries what the ESP validation needs | `DEADBEEF`, `BEEFIT`, version, exactly one `VDM-HW:C<n>`, SP and reset vector; image check C1-C6 with the ESP's own code (§5.8) |
| B8 | the boot stage starts only the application its sector 0 was linked with | the record of the application part at 0x08000240 (§5.11: magic `VDAC`, 0x08004000, length, CRC-32 of sectors 1..n), written after the link; the boot stage computes the CRC in the idle time of the window and opens the next window instead of the application when it differs (§5.10, D9); image check C7, Renode E12 |

### 5.2 Boot stage

| step | Rust boot stage (PAC only, `firmware/src/boot_hw.rs` + `vdm-stm-boot`) | C++ 2.1.7 | time |
|---|---|---|---|
| 0 | cortex-m-rt `Reset`: FPU, `.data`/`.bss` (~16-24 KiB) | `Reset_Handler`, `SystemInit`, `SystemClock_Config`: HSE + PLL, `Error_Handler` loops for ever when the HSE does not start | ~1 ms |
| 1 | capture: read RCC_CSR, set RMVF; classify, count and guard into the `NOINIT` cells (core) | `sysstat_capture_reset` | µs |
| 2 | valve outputs safe: PB9 latch high, then open drain (PSU off); PA5, PA6, PA7, PB0, PA15, PB3 outputs low | `valve_pins_safe` | µs |
| 3 | boot clock: HSEON, wait for HSERDY at most 5 ms (SysTick on HSI); ready -> SYSCLK = HSE 25 MHz without PLL (0 wait states); else HSE off, stay on HSI 16 MHz (D1) | (PLL from step 0) | ≤ 5 ms |
| 4 | USART1 115200 8E1 polled (BRR 0xD9 at 25 MHz, 0x8B at 16 MHz), PA9/PA10 AF7; LED PC13 low (on); 10 ms, then drop what arrived | `BootSetup` | 10 ms |
| 5 | window: 3001 ticks of 1 ms; RX drained continuously into a 1024-byte FIFO; per tick at most one 8-byte block when 8 bytes wait, compared with `DEADBEEF` read from the ID block; LED toggles every 102 ticks. Meanwhile, one 8-byte step per pass of the polling loop, the CRC-32 of the application part (B8): ~30 µs per step on HSI, 112 KiB in ~0.5 s | `BootLoop` state 0 | 3.001 s |
| 6a | match: LED low; next tick: LED high, 10 ms, `BEEFIT\r\n` from the ID block, wait for TC, 200 ms; clocks back to reset (HSI, HSE off, CFGR 0), SysTick off, `cpsid i`, SYSCFG clock on, MEMRMP = 01, `cortex_m::asm::bootload(0x1FFF0000)` | state 1, `JumpToBootloader` (`HAL_RCC_DeInit`, SysTick off, `__disable_irq`, MEMRMP, MSP, jump) | 0.21 s |
| 6b | timeout, application part matches its record (B8): USART1 back to its reset state (RCC reset pulse), SysTick off, IWDG started (8 s, §5.4); return `BootToken { reset, boot_ms, hse }`. No match: the next window (step 4), for ever, without IWDG | state 2, `bootstate = 1`; `IWatchdog.begin` at the head of `setup_system` | — |

- The 8-byte blocks, the 10 ms drop, the 3001 ticks, the LED period, `BEEFIT\r\n` and the
  10 ms / 200 ms delays are the C++ behaviour and the cases of `test_otasupport.cpp`
  (PROTOCOL_V2 "Firmware update handshake"; D2 keeps the fixed blocks).
- ESP 2.1 timing: NRST 100 ms, first `DEADBEEF\n` 20 ms after the release, then every 100 ms
  for 2.5 s. The Rust stage listens about 2-7 ms after reset and drops until ~12-17 ms, so the
  first send is usually aligned (`BEEFIT` complete ~33 ms after the release); a stray byte
  realigns within 8 sends.
- The legacy ESP 1.x sends `DEADBEEF\r\n` 1.5 s after reset: inside the window, aligned.
- `boot_ms` offsets the application's `millis()`, so the uptime in `gstat` counts the window
  as in C++ (millis starts at `HAL_Init`).
- Application stage, in this order: IWDG set-up again (§5.4); HSE probe (100 ms, as
  `HSE_STARTUP_TIMEOUT`); `embassy_stm32::init`; then the `setup_system` order of `main.cpp`
  without the watchdog line (I2C recovery, `Wire.begin`, pins, terminal, UART, 500 ms,
  EEPROM, 1-Wire, `app_setup`, valve set-up with TIM1, `app_restore`, TIM2).

### 5.3 Clocks

| phase | source | SYSCLK | USART1 115200 | note |
|---|---|---|---|---|
| boot, HSE starts within 5 ms | HSE 25 MHz, no PLL | 25 MHz | 115,207 Bd (+0.006 %) | crystal accuracy, like the C++ window |
| boot, HSE dead | HSI 16 MHz | 16 MHz | 115,108 Bd (-0.08 %) | plus the HSI error: ±1 % at 25 °C, a datasheet worst case of about ±4 % over -10..85 °C, against a receiver tolerance of about 3.4 % at 8E1 (D1, R11) |
| application, HSE ok | PLL from HSE: F401 M25 N336 P4 Q7, F411 M25 N192 P2 Q4 | F401 84 MHz (APB1 42, APB2 84), F411 96 MHz (APB1 48, APB2 96) | BRR 0x2D9 / 0x341 | the frequencies of the STM32duino BlackPill variants |
| application, HSE dead | PLL from HSI: M16, same N, P, Q | the same | the same | the C++ never gets here (D5) |

embassy-stm32 0.6.0 `rcc::init` waits for HSERDY without a bound (`rcc/f247.rs`), so the
firmware probes the HSE itself before `init` and passes `hse: None, pll_src: PllSource::HSI`
when it does not start. No clock security system, as in C++: a runtime HSE loss stops the
PLL clock and the IWDG (LSI) resets the chip; the next boot falls back to HSI. `rcc::Config`
fields used: `hse`, `pll_src`, `pll` (`PllPreDiv`, `PllMul`, `PllPDiv`, `PllQDiv`), `sys`
(`Sysclk::PLL1_P`), `ahb_pre`, `apb1_pre`, `apb2_pre`. embassy sets VOS scale 1 with a PLL
(the C++ F401 uses scale 2): no functional difference.

### 5.4 Watchdog

| phase | IWDG | a hang here is ended by |
|---|---|---|
| boot stage, window (≤ 3.02 s) | off | nothing needed: B2, B3; else the ESP's NRST |
| ROM bootloader (after the jump) | off, never started before the jump | the ESP flasher's NRST |
| end of the boot stage (window without handshake) | started (`boot_hw::watchdog_start`, sector 0): `pac::IWDG` KR 0xCCCC, KR 0x5555, PR 4 (/64), RLR 3999, KR 0xAAAA = 8 s nominal, 5.4-15 s with the LSI spread (as `IWatchdog.begin(8000000)`) | IWDG |
| application start | the same set-up again before `embassy_stm32::init`: same values, the start key leaves the running IWDG running, the reload restarts the 8 s | IWDG |
| set-up | reloaded between the long steps (EEPROM read, 1-Wire enumeration) as in `setup_system` | IWDG |
| main loop | reloaded in the 10 ms branch only if `valve_loop_ticks` advanced and the stall detector is clear | IWDG |
| fault, panic | started if it is not running, never reloaded | IWDG (§5.5) |

The IWDG is started by the PAC, because `wdg::IndependentWatchdog` needs the peripherals that
`embassy_stm32::init` creates. The boot stage starts it as its last step before it returns the
token, in sector 0, not the application stage: after an interrupted or failed flash of sectors
1..n (D9) the old boot stage calls `app::run` at its old address, now new or erased code. Erased
code faults (the handlers lie in sector 0), new code entered in the middle may hang; started
by the boot stage, the IWDG ends that hang too, and the next window keeps the STM re-flashable
(Renode E8). The C++ starts it at the head of `setup_system`, also after its window. From the
reload of the application stage to the first reload of `setup_system` (before the EEPROM read)
pass the HSE probe (≤ 100 ms), `embassy_stm32::init`, the I2C recovery, the terminal banner and
the 500 ms delay: about 0.6 s, far below the 5.4 s of the fastest LSI. The option bytes keep the
software watchdog (factory default); the firmware never writes them.

### 5.5 Faults

| event | C++ 2.1.7 | Rust |
|---|---|---|
| HardFault, NMI, unexpected interrupt before the IWDG runs | `Default_Handler` loops for ever: hang until NRST | ENA0..5 off and PSU off (BSRR), `FaultRecord` (kind, PC, LR, xPSR, CFSR, HFSR, BFAR, count), IWDG start key (a stopped IWDG starts with its reset values, 512 ms nominal), spin -> watchdog reset |
| the same after the IWDG start | loop -> IWDG reset after 5.4-15 s | the same handler: the start key leaves a running IWDG at 8 s, no reload -> IWDG reset; the reset reason is `IndependentWatchdog` as in C++, so the safe-mode count (3 within 10 min) is unchanged (D4) |
| panic | — | the same handler, no formatting (`core::fmt` is not linked for it) |
| stack overflow | silent corruption of `.noinit`/`.bss` | MPU region of 32 B, no access, at `_stack_end` (end of `.uninit`): MemManage -> HardFault; a lockup on stacking still ends through the IWDG |
| main loop hang, stuck valve state | IWDG not fed | the same |
| HSE lost at runtime | PLL stops, IWDG reset | the same; then HSI fallback (D5) |

The terminal prints a valid `FaultRecord` at the next start. No protocol reply carries it.

*Implementation:* the record type is `vdm_stm_boot::fault_record` (sector 0 with the handlers,
D9; no panic path, B3): ten words, magic `VDFR`, kind, pc, lr, xpsr, cfsr, hfsr, bfar, a count
of consecutive faults, and the inverted XOR of the nine words as check word. The terminal line
follows the banner: `last fault: <kind> pc 0x.. lr 0x.. xpsr 0x.. cfsr 0x.. hfsr 0x.. bfar 0x..
count <n>`. Only the HardFault handler gets the exception frame from cortex-m-rt; the
UsageFault, BusFault and MemManage handlers record pc, lr and xpsr as 0. On the chip they never
run, because SHCSR leaves those faults disabled and they escalate to HardFault; Renode takes
their vectors anyway (app.robot A6).

### 5.6 Memory layout per chip

| item | F401CC (256 KiB / 64 KiB) | F411CE (512 KiB / 128 KiB) |
|---|---|---|
| vector table | 0x08000000, 0x194 B (16 + 85 vectors) | 0x08000000, 0x198 B (16 + 86 vectors) |
| ID block `.vdm_id` | 0x08000200, ≤ 64 B | the same |
| record of the application part `.vdm_app_check` (B8, §5.11) | 0x08000240, 16 B | the same |
| boot stage `.vdm_boot` | 0x08000250, inside sector 0 | the same |
| `.text` start (`_stext`) | end of `.vdm_boot`, 8-byte aligned (0x080015A0 today) | the same |
| image budget | 128 KiB (D12) | 128 KiB |
| `RAM_LO` (reserved) | 0x20000000-0x20003233 | the same |
| `NOINIT` | 0x20003234-0x20003307 | the same |
| `RAM` (.data, .bss, .uninit, stack) | 0x20003308-0x2000FFFF, 52,472 B | 0x20003308-0x2001FFFF, 118,008 B |
| SP0 | 0x20010000 | 0x20020000 |

```
/* firmware/memory/vdm.x, included by f401.x and f411.x */
_stext = ORIGIN(FLASH) + 0x240;
SECTIONS
{
  .vdm_id ORIGIN(FLASH) + 0x200 : { KEEP(*(.vdm_id)) } > FLASH
} INSERT AFTER .vector_table;
ASSERT(SIZEOF(.vdm_id) <= 0x40, "vdm: ID block above 64 bytes");
```

### 5.7 Proof in the emulator (Renode)

Choice: Renode, not QEMU. QEMU's STM32F4 machines (`netduinoplus2`, `olimex-stm32-h405`) have
no RCC, GPIO or IWDG models. Renode (checked in `renode-infrastructure`) offers for the F4:
`STM32_UART` (bytes are instant, `ParityBit` and `CharacterLength` readable, ORE never set),
`STM32F4_EXTI`, `STM32_GPIOPort`, `STM32_Timer` (fixed frequency, not from RCC),
`STM32_IndependentWatchdog` (32 kHz, `machine.RequestReset()` on expiry, "Watchdog reset
triggered!" in the log), `STM32F4_RCC` (HSERDY mirrors HSEON; CSR reset flags not modelled),
`MappedMemory` that keeps RAM across resets; hooks `sysbus SetHookAfterPeripheralRead`,
`cpu AddHook`; Robot keywords `Write Line To Uart`, `Wait For Line On Uart`, `Wait For Log Entry`.

Set-up (`software_stm32_rust/renode/`, runner `tools/rust/renode.sh` in Docker
`antmicro/renode:1.16.1`, D11; the suites `boot.robot` and `app.robot` share `vdm.resource`):
- `stm32f401.repl` / `stm32f411.repl`: `using "platforms/cpus/stm32f4.repl"` and
  `./vdm_board.repl`, RAM and flash sizes, `nvic systickFrequency` 25 MHz for the boot clock
  (the HSI test sets 16 MHz), TIM1, TIM2 and TIM5 (the embassy time driver) at the timer clock of
  the application (84/96 MHz), `Miscellaneous.DWT` at SYSCLK (the 1-Wire waits), SYSCFG as plain
  memory (MEMRMP read back), DBGMCU_IDCODE = 0x10006423 / 0x10006431 as a tag.
- `vdm_board.repl`, the board around the chip: USART6 (not in Renode's STM32F4 platform) on
  NVIC line 71; `VdmValves.cs` at ADC1: the twelve valve motors of the C++ valve sim (same
  defaults: position 1800 of 3600, 0.2 pulses/ms, 25 mA running, 70 mA stalled; no inrush,
  coast, short or obstacle), driven by the ENA, DIR, MUX and PSU outputs, revolution pulses on
  PA4 (EXTI4), the motor current on PA0 as the inverse of `TimerHandler0`'s conversion, the
  reference 2048 on PA1; `VdmEeprom.cs`: the 24LC64 at 0x50 on I2C1, kept over a machine
  reset. Renode compiles both before the platform is loaded.
- Fake ROM at 0x1FFF0000: vector [SP 0x20002FF0, PC 0x1FFF0101] and `b .`; a `cpu AddHook` on
  0x1FFF0100 logs "ROM entered".
- `cpu VectorTableOffset 0x08000000` (no flash alias at 0), also in the `reset` macro; the
  `.bin` the ESP flashes is loaded at 0x08000000. Hooks write time stamps into the reserved
  `RAM_LO`: the first USART1 CR1 write with RE, the first IWDG_KR write, the USART1 bytes sent
  and the last start of the reset handler.
- RCC_CSR keeps its reset value in Renode: a read hook gives the flags of the reset kind a test
  needs (pin, software, watchdog, power-on).

Facts of the Renode models the suites depend on (found while building them):
- `STM32F4_I2C` hands the bytes of a write transfer to the slave in one `Write` call at its end
  and never calls `FinishTransmission`; a read transfer calls `Read()` once, with its default
  count of 1, at the address phase and gives the master just those bytes. `VdmEeprom` takes a
  `Write` call as one transfer and answers a read with a burst of 32 bytes.
- embassy reads ADC_DR by halfword (`ldrh`): `VdmValves` serves halfword reads.
- `STM32_UART` keeps no CR1.M bit (E1 takes the written value from the hook); bytes arrive at
  once and never with an error.
- After a wait that timed out, the `TerminalTester` of Renode 1.16.1 drops the first line that
  completes next and can leave the emulation running (the next `RunFor` is refused): the suites
  read terminal lines by count, the handshake keyword watches the USART1 byte counter, no wait
  is for a line that may not come.
- Renode takes the UsageFault vector for a UDF although SHCSR disables it (the chip escalates
  to HardFault).

| # | scenario | stimulus | expected |
|---|---|---|---|
| E1 | ESP 2.1 handshake | reset; after 20 ms `DEADBEEF\n` every 100 ms | `BEEFIT` within 15 ms after the first aligned block; during the window `usart1 ParityBit` = Even, `CharacterLength` = 8; "ROM entered"; SP = 0x20002FF0; MEMRMP = 1; a watchpoint on IWDG_KR sees no write before the jump |
| E2 | stray byte | one byte at 15 ms, then E1's pattern | `BEEFIT` within 8 sends |
| E3 | legacy ESP 1.x | `DEADBEEF\r\n` at 1500 ms | `BEEFIT` |
| E4 | last tick | `DEADBEEF` into tick 3001 | `BEEFIT` |
| E5 | no handshake | nothing for 3.1 s | no TX on USART1 in the window; afterwards 8N1, BRR 0x2D9 / 0x341; `gvers` -> `gvers 2.2.0-revamped_C2 1 ` (C1 images: `_C1`), `gproto` -> `gproto 3`, `ghwin` -> 1059 / 1073; no IWDG reset in 10 s |
| E6 | HSE dead | read hook clears HSERDY (RCC_CR bit 17) | E1 passes on HSI; E5 passes with PLLCFGR source HSI |
| E7 | fault in the application | PC set to a `UDF` in RAM | outputs off, "Watchdog reset triggered!" within 15 s virtual time; E1 passes afterwards |
| E8 | code after the window; a fault before the IWDG runs | a hang (`b .`) at the entry of `app::run`; a fault at the entry of `boot_hw::run` (`cpu AddHook`) | the IWDG runs from the end of the window: reset 8 s after its start; the fault: reset within 1 s (reset values, 512 ms); E1 passes after both |
| E9 | no-init cells across resets | `NOINIT` loaded with C++ 2.1.7 cells (counter 41, guard); a pin reset (CSR read hook = PINRSTF), then SYSRESETREQ | the Rust capture continues the C++ cells (counter 42, 43, guard window summed and sealed); warm state area and padding byte-equal. The warm restore itself: A4 |
| E10 | regression fence | reset | USART1 RE set at most 10 ms after reset (+5 ms HSE probe): fails when someone puts code before the window |
| E12 | half-flashed image (B8) | one word of sector 2 changed under the image's sector 0 | for 6.5 s (three windows) no `app::run`, no IWDG write, no byte sent, outputs off, USART1 at 8E1; the ESP 2.1 handshake in the third window: `BEEFIT`, ROM entered; the word restored and a reset: the application answers |
| E11 | end to end (D11) | host build of the Rust ESP flasher <-> USART1 socket <-> AN3155 responder (Python peripheral) after the jump | C++ -> Rust -> C++ image cycle in emulated flash, `gvers` after each. *Implementation:* `e11.robot`, `tools/rust/renode.sh --e11 [--cpp <dir>]`. The flasher (`vdm_esp_core::stm_flasher`) runs in a host program (`tools/rust/stm/e11`, `vdm-e11`, static) that `VdmEsp.cs` drives in lock-step, once per millisecond of virtual time: `VdmEsp` is the ESP's end of USART1 (an IUART on a UART hub with `usart1`) and of NRST (its release resets the machine). `VdmRom.cs` does the work of the ROM bootloader while the CPU is in the stand-in: USART1 to 8E1, then sync, GET, GET ID, Extended Erase with a sector list, Write and Read Memory on the registers of USART1 and the emulated flash, every command logged. With the C++ 2.1.7 release images in `<dir>` (SHA-256 pinned in `tools/rust/stm/cpp217.sha256`): C++ -> Rust -> C++; without: Rust -> Rust with another version string in the ID block -> Rust. Each flash ends when the new image answers `gvers` with the expected version and tag; the ROM log shows D9: sectors 1..n erased, written and verified, then sector 0 erased, written from its second block on, the vector table last. SysTick and the CPU run at the core clock (84 / 96 MHz, 84 / 96 MIPS): the C++ counts its milliseconds there, and its 1 ms interrupt (two `analogRead` with the whole HAL ADC set-up) takes more than 1 ms at 4 MIPS, so TIM2 at the same priority never runs and the IWDG resets the chip; the Rust boot window, counted for the 25 MHz boot clock, is shorter then (E1-E10 check its timing) |

`app.robot` runs the whole application against the C++ goldens of the glue_system suites
(§7.4): the requests of a golden boot go to USART1 at their times after the reset, every reply
line must equal the golden's (the C++ identity `2.1.7-revamped_C2` replaced by the image's),
the terminal on USART6 must print the golden's lines in the same order, and the EEPROM model
must hold the golden's bytes at the end of a boot.

| # | scenario (golden) | stimulus | expected |
|---|---|---|---|
| A1 | a new controller answers (`boot__a_new_controller...`) | erased EEPROM, every valve connected; gvers, gproto, gtgtp, gvlvd during the presence test, gstat, stgtp, gtgtp at the golden's times; then `gvers` on USART6 | the 7 replies and all terminal lines as the golden; the terminal answers `Version: 2.2.0-revamped` |
| A2 | the presence test (`boot__the_presence_test...`) | valve 5 open; gvlst after 43.5 s | `gvlst 12 8,8,8,8,8,6,8,8,8,8,8,8 `; the twelve presence tests in the golden's order and times (terminal); every valve enabled once, pulses from every connected one; never two L293 enables at once |
| A3 | configuration across a reset and a power cycle (K1-6/S3-2) | slcfg, sfspo, stlnt; the `reset` command (a real SYSRESETREQ through the boot stage); glcfg, gtlnt, new values; a power cycle (no-init region 0xA5, power-on flags); glcfg, gtlnt, gstax | the replies and terminal lines of all three boots as the golden; the EEPROM bytes after every boot as the golden's |
| A4 | warm start from C++ 2.1.7 state (K1-8/W2) | the no-init region and the EEPROM as C++ left them at its reset command; a software reset into the Rust image | lease state 2 and timeout 5 (gstax), `gvlst 12 8,8,8,8,8,8,8,8,6,6,6,6 `, no valve enabled in 10 s (no presence test), EEPROM unchanged |
| A5 | stalled valve loop, three times (S9-2) | 2 s into the application TIM2 stops (CR1.CEN = 0); watchdog flags by the CSR hook | each time an IWDG reset 8 s after the last reload; every start prints the golden's lines up to its EEPROM load ("reset by watchdog", then "safe mode"); the fourth answers gstax as the golden but uptime, eepState, lease remaining and cfgFlags (the C++ case resets before the first-start EEPROM write, the real IWDG after it); no valve moves; `ssafe 1` -> err, `ssafe 0` -> ok, then valves 0-3 present |
| A6 | fault record at the next start | UDF in the application, IWDG reset | the next start prints `last fault: ... count 1` right after the banner, then "reset by watchdog" |

Every scenario runs for each of the four images (`tools/rust/renode.sh`: about 12 minutes per
image).

What Renode does not prove, so the bench (§5.9) does:
- electrical and timing behaviour: the I2C bus is a transaction model (no waveform, clock
  stretching or stuck line, so the 9-clock recovery only runs its GPIO steps), PB10 has no
  1-Wire device (every reset finds no presence pulse: no temperature path beyond the search),
  the ADC returns the model's value at once, the LSI of the IWDG is exactly 32 kHz (the chip:
  5.4-15 s), interrupt latencies and the length of critical sections are not cycle-accurate;
- the HSE crystal start-up (E6 forces HSERDY off; a slow crystal between 5 and 100 ms is not
  modelled), USART framing, parity and noise errors (host tests of `serial` and `uart_errors`);
- the ROM bootloader itself (`VdmRom.cs` does its work; R2), the flash interface of the chip
  and its timing (erase and program are instant in E11), the NRST pulse (the STM runs on while
  it is held, its release is one reset);
- the RCC_CSR flags of a real reset (read hook), and the fault escalation to HardFault.

### 5.8 Byte-level image check

`vdm-stm-image-check` runs after the firmware build on the four `.bin` and `.elf` files:

| # | check | expected |
|---|---|---|
| C1 | ESP `validate_image(img, pid, require_handshake = true)` | `None` for F401 images with PID 0x423, 0x433, 0x431 and F411 images with 0x431; `ImageChipMismatch` for an F411 image with 0x423 (as C++ 2.1.7) |
| C2 | `ImageInfo` | version = workspace version (`2.2.0-revamped`, D8), `hwTag` `C1`/`C2`, `hwConflict` false, `hasHandshake` true |
| C3 | ESP `check_board(tag, board)` | `Ok` for the image's own tag, `Mismatch` for the other |
| C4 | ELF layout | SP0 and reset vector as §5.6; `.vdm_id` at 0x08000200 holds the first version-like run of the image; exactly one `VDM-HW:` marker; `NOINIT` symbols at 0x20003234 / 0x200032E8 / 0x200032FC; no section in `NOINIT` |
| C5 | size | ≤ 128 KiB, so `sectorsForImage` gives 5 sectors like the C++ images |
| C6 | reference | the same tool on the C++ 2.1.7 images gives version `2.1.7-revamped`, tags `C1`/`C2`, handshake present, SP 0x20010000 / 0x20020000 (*measured* with a port of `scanBytes`). *Implementation:* `tools/rust/docker.sh image-check <dir>` with the four release images in `<dir>` (`*STM32F401_C1.bin` ...), each checked against its SHA-256 pinned in `tools/rust/stm/cpp217.sha256` first |
| C7 | record of the application part (B8) | `__vdm_app_check` at 0x08000240 (§5.11; behind the ID block, so the ESP's scan still finds the version and the marker there first), its 16 bytes equal `VDAC`, 0x08004000, the length and the CRC-32 of the .bin from 0x4000 on, computed with the boot stage's own code (`vdm_stm_boot::app_check`) |

The ESP side is `vdm_esp_core::stm_flasher::{validate_image, check_board}`, the Rust port of
`stm_flasher.cpp` (`software_esp32_rust/core/src/stm_flasher.rs`). C1-C3 and C6 run through a
small native harness that links the C++ `stm_flasher.cpp`, the code of the ESPs in the field
(ESP 2.1.7); E11 flashes with the Rust port.

### 5.9 Proof on hardware

The emulator proves the control flow and the register values; the ROM bootloader and the
electrical timing need hardware. Before the first cabinet unit (the operator flashes, D10):
1. bench BlackPill (F401CC or F411CE) with an ESP or a USB-UART: C++ 2.1.7 -> Rust -> C++ 2.1.7
   -> Rust through the ESP flasher; after each flash `gvers` and a warm restore check;
2. a power cycle and a cold start;
3. 1-Wire slots and I2C on a logic analyzer next to the C++ (R5);
4. then one cabinet unit, with C++ 2.1.7 as the known-good fallback image on the ESP.

### 5.10 What the image cannot cover, and the half-flashed image

With the C++ flasher an interrupted flash (ESP crash or power loss between the erase and the
last block) leaves no valid vector table: the STM needs BOOT0. The Rust ESP flasher writes
sector 0 in a pass of its own (D9), so only that pass (~2 s) can leave no valid vector table.

A cut in the pass of sectors 1..n leaves a whole sector 0 (vector table, ID block, boot stage)
in front of erased, partly written or foreign sectors, and the ESP then resets the STM. The
window works, but `main` would call `app::run` at the address of the image sector 0 was linked
with (sector 2 today): erased code faults into the handlers of sector 0, foreign code entered
in the middle may do anything with the valve outputs, also hang. Two measures of the boot stage
keep that state safe:
- B8: sector 0 holds the record of the application part it was linked with (§5.11), written
  into the .elf and the .bin after the link (`vdm-stm-image-check patch`, `tools/rust/stm/
  build_images.sh`; image check C7). The boot stage computes the CRC in the idle time of the
  window (no delay of the application, at most ~30 µs between two looks at the receiver) and
  starts the application only when it matches; otherwise it opens the next window, for ever,
  without the IWDG, with the outputs safe: the ESP can flash at any time (Renode E12). An image
  that was not patched never starts its application either, so only the images of
  `tools/rust/docker.sh fw` may be flashed.
- the IWDG starts at the end of the boot stage (§5.4): whatever runs after the window is
  watched, also if a record ever matched wrong code.

The order of the passes (F9 of REVIEW-STM-SAFETY.md, decided on 2026-10-07): the C++ 2.1.7
images keep their reset handler (0x0800F7D5) and every fault vector (0x0800F825) in sector 3,
so sector 0 of a C++ image is worth nothing once sectors 1..n are erased. The ESP flasher
therefore writes sector 0 first when the new image carries a valid record (magic, start,
length and CRC checked against the image before anything is erased): from the end of that
pass on, a cut leaves the new boot stage, whose check loops its window. An image without a
valid record (C++) is written as D9 says, sector 0 last: the old Rust boot stage in front of it
loops its window on a cut. Brick windows: C++ -> Rust, Rust -> Rust, Rust -> C++ ~2 s each
(the sector-0 pass); C++ -> C++ the whole session, as before. Implemented in
`vdm_esp_core::stm_flasher`.

A half-flashed image therefore stays in its window and can be flashed again remotely; only the
sector-0 pass itself needs BOOT0 when it is cut. Images of at most 16 KiB (none today) have no
application part: their record holds length 0.

### 5.11 The record of the application part (B8)

The format the boot stage, the patch step, the image check and the ESP flasher share
(`vdm_stm_boot::app_check`):

| item | value |
|---|---|
| place | image offset 0x240 (address 0x08000240), 16 bytes, in sector 0 behind the ID block (0x200, at most 64 bytes) and in front of the boot stage (0x250); its own bytes lie outside the range it covers |
| layout | four 32-bit words, little endian, in this order |
| word 0, offset 0x240 | magic 0x43414456 (the bytes `56 44 41 43`, "VDAC") |
| word 1, offset 0x244 | start of the covered range: 0x08004000 (image offset 0x4000, sector 1) |
| word 2, offset 0x248 | length L of the covered range in bytes: image size - 0x4000 (0 for an image of at most 16 KiB); the boot stage accepts L <= 0x1C000 (112 KiB, D12) |
| word 3, offset 0x24C | CRC-32/ISO-HDLC (zlib `crc32`) of the image bytes at offsets 0x4000 .. 0x4000 + L - 1: polynomial 0x04C11DB7, reflected input and output (bitwise with 0xEDB88320), init 0xFFFFFFFF, final XOR 0xFFFFFFFF; check value of "123456789": 0xCBF43926; of no bytes: 0 |
| before the patch | 0xFFFFFFFF in all four words: no valid magic, the boot stage starts no application |
| valid | magic, start and L as above, L equal to the image size - 0x4000, the CRC matching; the boot stage needs the first three and the CRC over the flash |

The .bin the ESP gets is the patched one: the record lies in the image itself, the ESP can check
it before it erases anything (F9).

## 6. Build

### 6.1 Four images as cargo features

| feature | effect |
|---|---|
| `f401` / `f411` | `embassy-stm32/stm32f401cc` / `stm32f411ce`, `memory/f401.x` / `f411.x`, PLL 84 / 96 MHz |
| `c1` / `c2` | `BoardRev::C1` / `C2` (MUX on = high / low, even valve on MUX on), marker `VDM-HW:C1` / `C2` |
| `dev` | version suffix `-dev`, build field = build time (the C++ dev envs: `__TIME_UNIX__`) |
| `terminal` (default) | debug terminal and debug lines (§4.2) |

`compile_error!` unless exactly one chip and one board are set. Release build:
`cargo build --release --features f401,c2` in `firmware/`, then `llvm-objcopy -O binary`
(rustup component `llvm-tools`, added to `tools/rust/Dockerfile`). Release asset names stay
those of `tools/release/package.py` (`STM32F401_C1` ... `STM32F411_C2`). Profile `release`:
`opt-level = "s"`, `lto = "fat"`, `codegen-units = 1`, `panic = "abort"`, `debug = 2` (ELF only).

Dependencies, pinned with `=`: `embassy-stm32 0.6.0` (features `rt`, `exti`, `unstable-pac`,
`time-driver-tim5`, one chip; never `gpio-init-analog`, which would put every pin into analog
mode during `init`), `embassy-time 0.5.1`, `cortex-m` (`critical-section-single-core`),
`cortex-m-rt`; no `embassy-executor` (D6), no defmt in release images.

### 6.2 Markers and version

`firmware/build.rs` writes the ID block from the workspace version (`2.2.0-revamped`, D8):

```rust
#[used]
#[link_section = ".vdm_id"]
static VDM_ID: [u8; 42] = *b"\x002.2.0-revamped\x00VDM-HW:C2\x00DEADBEEF\x00BEEFIT\x00";
```

- Rust string literals carry no NUL and the linker packs them together, so the ESP's scan
  ("first NUL-terminated printable run that parses as a version") could miss the version or
  find another one. The NUL-delimited block at 0x08000200 comes first in every image (C4).
- The boot stage reads its pattern and its reply from this block with volatile reads: the
  bytes the ESP checks are the bytes the code uses. `gvers` and the banner read version and
  tag from it too.
- The version comes from the workspace (`2.2.0-revamped`, D8). `tools/release/package.py`
  names the assets from the git tag only (`VdMot-Revamped_<version>_STM32F401_C2.bin`), so
  image check C2 compares the embedded version with the workspace version.

### 6.3 Size

| | flash | RAM |
|---|---|---|
| C++ 2.1.7 (*measured*) | 70,216 B (F401), 70,312 B (F411): 5 erase sectors | .data 460 + .bss 12,392 + .noinit 212 B |
| Rust (estimate) | 70-105 KiB: core 25-35, glue 30-45, embassy + time driver + cortex-m-rt 12-20, compiler-builtins (u64 division, soft f64 for DS2438) 2-4 | 16-24 KiB of 52 KiB (F401) |
| budget (CI fails above) | 128 KiB: 50 % of the F401 flash, 25 % of the F411; erase set sectors 0-4 like C++ | |

A C++ STM flash takes 26-30 s for 70 KB through the ESP; a 128 KiB image takes about 40-55 s
(256-byte blocks are written and read back, the erase set stays the same).

## 7. Test strategy

### 7.1 Port of the glue suites

`test_<stem>.cpp` becomes `glue/src/<module>/tests.rs`, `test_<stem>__<part>.cpp` becomes
`tests_<part>.rs` (PORTING.md); every case and assertion is ported.

| C++ (`software_stm32/test/native/glue`) | cases | covers | Rust |
|---|---|---|---|
| `test_app.cpp`, `__mut`, `_v3` | 9 + 22 + 34 | start values, config load, valve walk of `app_loop`, 10 s countdowns, setters, soft reset; lease, failsafe, assembly hold, retries, warm restore, calibration records, stop, learn time rule, protection guard, safe mode, temperature hold | `app/` |
| `test_communication.cpp`, `__mut`, `_v1`, `_v3` | 8 + 13 + 7 + 21 | v1/v2 replies byte-exact against 2.0.0, argument bounds, list separators, protocol 3 commands and goldens, EEPROM marks only on change, lease poll, USART1 error counters, board marker | `communication/` |
| `test_eeprom.cpp`, `__mut` | 17 + 3 | block write order, load through `resolveConfig`, schedule, bus restart before retries, re-read merge, calibration records, counters, read failure budget | `eeprom/` |
| `test_fakes.cpp` | 29 | fake board, runner hooks, valve sim | `test_support/` |
| `test_i2c_bus.cpp` | 3 | recovery: at most 9 clocks, STOP; restart | `i2c_bus/` |
| `test_main.cpp`, `__mut` | 10 + 2 | start-up order, `setup_system` order, branch periods, LED and button ticks, watchdog feed | `main_loop/` |
| `test_motor.cpp` (+ C1 wiring), `__mut`, `_v3` | 7 + 42 + 22 | safe pins, full calibration, partial move, open circuit, presence test; clamping, early checks, service move, PSU and lock timing, retries, motor machine, `TimerHandler0`; partial end stops, keep-status and reference moves, `sstop`, short, inrush, temperature gap | `motor/` (C1 and C2 as `BoardRev` values in one binary) |
| `test_otasupport.cpp` | 6 | 8E1 set-up and 10 ms drop, window length 3001, LED period, `BEEFIT` and jump, block rule, last tick | `vdm-stm-boot` `window/` |
| `test_owDevices.cpp`, `__mut`, `_s7` | 5 + 8 + 6 | enumeration, cycle, lock and search, JSON; retries, held values, 85.0 °C rule, 24 h re-scan | `ow_devices/` |
| `test_sysstat.cpp` | 11 | boot reason from CSR, reset counter in `.noinit`, uptime across the wrap, safe mode | `vdm-stm-boot` `capture/`, glue `sysstat/` |
| `test_system_boot`, `_calib`, `_config`, `_io`, `_motion`, `_proto3` | 3 + 3 + 5 + 2 + 8 + 6 | the whole glue with the valve sim over the UART across resets and power cycles | `glue/src/system/tests_*.rs` |
| `test_terminal.cpp`, `__mut` | 12 + 12 | commands, outputs, `sena` limits, read budget, argument ranges | `terminal/` |

336 cases (+7 for C1); the C++ suites stay runnable as the reference (`tools/native/docker.sh`).

### 7.2 Fakes

`glue/src/test_support/` (only under `cfg(test)`): a fake board that implements every HAL
trait (time in µs; at every millisecond boundary the valve sim, then TIM1, then TIM2, as the
C++ fake), fake UARTs (inject, take TX, inject HAL error codes), a 24LC64 with failure
injection, a 1-Wire device list, the valve sim and the stubs with their call log. The glue has
no globals, so a reboot is a new `Controller` built from the persistent stores (EEPROM bytes,
`NOINIT` image, CSR flags) instead of the C++ fork-per-case runner. `System::reset`, the
bootloader jump and an expired IWDG end a boot with a typed panic payload that the runner
catches (`catch_unwind`), as the C++ fakes throw `SystemReset`, `BootloaderJump` and
`WatchdogReset`.

### 7.3 New tests (no C++ counterpart)

| module | content |
|---|---|
| `onewire`, `dallas`, `ds2438` | against a bit-level bus simulator: ROM search order, CRC-8, scratchpad CRC and all-zero checks, `DEVICE_DISCONNECTED_RAW`, DS18S20 extended math (C integer promotion, the `COUNT_PER_C` = 0 guard), DS2438 `readVAD` float steps (R6) |
| `eeprom24` | 30-byte and 32-byte page splits, ready polling, error propagation, short reads |
| `serial` | HAL error-code rules per interrupt, 1023-byte rings, drop counting, blocking TX |
| `hw_timer` | the PSC/ARR values of §2.4 |
| `vdm-stm-boot` | ESP 2.1 pattern (9-byte period, stray bytes), legacy pattern, flood of garbage |
| ID block | NUL layout, at most 64 bytes, scanner finds version and tag (also C4) |

### 7.4 Cross-implementation goldens

A test-only exporter in the C++ `glue_system` suite writes, per scenario, the UART transcript,
the final 8 KiB EEPROM image and the `NOINIT` bytes to `glue/tests/golden/*.json`; the Rust
system tests replay the inputs and compare byte for byte. This proves C++ <-> Rust
interchangeability of the EEPROM and the warm state on top of the ported assertions.

*Implementation:* the recorder (`tools/rust/stm/golden/`: `recorder.cpp` linked into the C++
glue_system suites with GNU ld `--wrap`, a doctest listener, the fork-per-case testkit) writes
one text file per case, `glue/tests/golden/<slug>.txt`: per boot the reset kind, the time-stamped
`rx`/`tx`/`dbg` bytes of both UARTs, how the boot ended, the EEPROM rows that are not erased and
the `NOINIT` bytes (format in `glue/src/system/golden.rs`). Text, not JSON: the diffs are
readable and the glue tests need no JSON parser. The Rust bench (`glue/src/system/bench.rs`)
reproduces all 27 cases byte for byte; `app.robot` replays some of them against the images
(§5.7).

### 7.5 Mutation gate

- Packages `vdm-stm-boot` and `vdm-stm-glue` (core by its own agent):
  `bash tools/rust/docker.sh mutate software_stm32_rust vdm-stm-glue [--file src/motor.rs]`,
  then `tools/rust/mutation_gate.py`: at least 95 % overall and per file, unviable mutants not
  counted, timeouts killed. C++ baseline: 97.92 % (2,639 of 2,695), lowest file
  `src/sysstat.cpp` 95.65 % (`docs/revamped/mutation-stm32-glue.md`).
- Equivalent mutants in `tools/rust/mutation/equivalents/vdm-stm-glue.json` with a reason;
  code that cannot run on the host gets `#[cfg_attr(test, mutants::skip)]` with a comment.
  Hardware code is in the firmware crate, so the glue should need none.
- The firmware crate is not mutated: it holds no decisions (§1.1) and Renode and the image
  check cover it.
- Run time: the C++ suite has ~2,700 mutants; cargo-mutants rebuilds per mutant, so CI shards
  per file (`--file`, `--shard k/n`) (R8).

### 7.6 CI order

host tests (core, boot, glue) -> mutation gate -> firmware build (4 images, size budget, boot
probe link, the patch of the record of B8) -> image check C1-C7 -> Renode E1-E10, E12, A1-A6
and E11 -> release packaging. A failure anywhere stops the release of every STM image.

## 8. Open decisions and risks

Decided on 2026-10-07 (the operator may revert): D1 recommendation (HSE if ready within 5 ms,
else HSI); D2 fixed 8-byte blocks as C++ (the proven path with the real ROM bootloader; the
sliding window stays a documented alternative); D3 the C++ 2.1.7 addresses; D4 IWDG reset;
D5 PLL from HSI; D6 no executor; D7 port 1:1 behind the feature `terminal`; D8
`2.2.0-revamped` for the first Rust release (ESP and STM); D9 the boot stage stays inside
sector 0 and the Rust ESP flasher erases and writes sector 0 last (a documented deviation of
the flasher port); D10 the bench proof is a required step of the install procedure, done by
the operator; D11 recommendation; D12 128 KiB.

### Decisions for the operator

| # | decision | recommendation | alternatives |
|---|---|---|---|
| D1 | clock of the boot window | HSE 25 MHz if ready within 5 ms, else HSI | HSI always (the operator's example: simplest, but HSI ±4 % worst case against ~3.4 % 8E1 tolerance); HSE only (C++ behaviour, blocked without HSE) |
| D2 | handshake matcher | fixed 8-byte blocks as C++ (PROTOCOL_V2 documents it; ESP 2.1 realigns) | sliding window: accepts misaligned streams too, a documented deviation |
| D3 | `NOINIT` cells | at the C++ 2.1.7 addresses: warm state survives C++ <-> Rust flashes and the ROM bootloader | own addresses: cold start with presence tests and reference moves after every implementation switch |
| D4 | fault policy | outputs off, `FaultRecord`, IWDG reset (reset reason as a C++ fault, also before the IWDG would run) | SYSRESETREQ: faster, but counted as software reset, so a fault loop never enters safe mode |
| D5 | application clock with a dead HSE | PLL from HSI with the C++ frequencies (C++ hangs in `Error_Handler` before its window) | stop after the window and wait for a re-flash |
| D6 | executor | none: blocking superloop and raw ISRs | `embassy-executor` 0.10 task around the superloop: more code, no gain |
| D7 | debug terminal | port 1:1 with the debug lines, feature `terminal`; ArduinoMenu dropped | drop the terminal (saves 3-5 KiB; its 24 test cases and the field tool go) |
| D8 | version of the first Rust release | `2.2.0-revamped`: `gvers` and the dashboard tell Rust from C++ | `2.1.7-revamped`: byte-identical `gvers`, implementations indistinguishable remotely |
| D9 | interrupted flash | keep the boot stage and its callees inside sector 0 (image check verifies) and let the ESP flasher erase and write sector 0 last: the brick window shrinks from the whole session (30-55 s) to ~2 s; decide with the ESP flasher port. *Implementation:* a flash cut in the pass of sectors 1..n leaves a boot stage in front of foreign sectors; it starts no application there (B8: record and CRC-32 of the application part, §5.11) and the IWDG runs from the end of every window that does start one (§5.4, §5.10). F9 (2026-10-07): the ESP writes sector 0 first for an image with a valid record, last otherwise, so a flash from C++ is protected too (§5.10) | accept as today; or a permanent first stage in sector 0 with the application at 0x08004000 (new image format, flasher change) |
| D10 | bench proof | mandatory before the first cabinet unit: spare BlackPill + ESP or USB-UART (§5.9) | flash a cabinet unit directly, C++ fallback on the ESP |
| D11 | emulator | `antmicro/renode:1.16.1` (newest published image; Renode 1.17.0 has a GitHub release but no image); E11 once the Rust ESP flasher exists | own image from the 1.17.0 release; skip E11 |
| D12 | size budget | 128 KiB per image (same erase set as C++) | 192 KiB (one more 128 KiB sector, ~1-2 s longer erase) |

### Risks

| # | risk | mitigation |
|---|---|---|
| R1 | embassy-stm32 0.6.0 waits for HSERDY without a bound; a later version may move or change that code | own HSE probe before `init`; E6 runs on every dependency bump |
| R2 | the ROM bootloader starts from another state than after the C++ jump (VTOR 0 instead of 0x08000000, peripherals) | replicate the C++ sequence; only hardware proves it (§5.9) |
| R3 | warm state across a flash relies on the ROM bootloader using only 0x20000000-0x20002FFF | the stm32flash device table starts user RAM at 0x20003000 for these PIDs (AN2606 not re-read here); bench check (§5.9) |
| R4 | Renode fidelity: no ADC, SYSCFG, DBGMCU models, instant bytes, timers independent of RCC, no CSR flags | board models (valves behind ADC1, the EEPROM), tags and hooks (§5.7); Renode proves control flow, register values and the protocol against the C++ goldens, not analog or electrical timing |
| R5 | 1-Wire slot timing: the C++ switches the pin through HAL calls (µs) inside the slots, Rust through BSRR; the sample point moves from ~16 µs to ~13 µs after the falling edge, with less rise time on long cables | measure the C++ slots on the bench and use the effective values |
| R6 | library semantics: C integer promotion (DallasTemperature `<< 11` on int16, wrap on narrowing), DS2438 float (f64 x 0.01 -> f32, x 100 -> truncation), a C++ division by a value that can be 0 yields 0 on the Cortex-M4 (no trap) | explicit tests (§7.3); every such division mirrors the C++ result |
| R7 | safe Rust needs critical sections where the C++ shares without masking | sections below the C++ 1-Wire masking (~80 µs); EXTI latches edges |
| R8 | cargo-mutants rebuilds per mutant: ~3,000 glue mutants take hours | sharding per file, cached target volume |
| R9 | the ESP validation port (`stm_flasher.rs`) does not exist yet | native harness on the C++ original until then (§5.8) |
| R10 | image growth beyond 128 KiB adds sector 5 and flash time | CI budget (D12) |
| R11 | with D1 = HSI always: handshake failures at temperature extremes | D1 recommendation |
| R12 | today's C++ image cannot be re-flashed remotely when its HSE fails (`Error_Handler` before the window); the first C++ -> Rust flash depends on a working HSE | none on the C++ side; the Rust boot stage removes it for later flashes |
