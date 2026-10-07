//! Fake board of the glue tests (port of the board part of `test/native/fakes`: fake time, pins,
//! EXTI, the I2C lines and the `Wire` driver, control timers, watchdog, no-init RAM, system
//! reset). One board for every suite: it implements the HAL traits with `&self`, and a clone is
//! a handle to the same board (the clock of an EEPROM, the pins of a recovery and the board of a
//! test share one time and one event log). The pin modes are firmware set-up in Rust, so the
//! outputs start at the levels the firmware's set-up leaves (PSU enable high = off, everything
//! else low).
//!
//! Time passes only through [`FakeBoard::advance_us`] and the `Clock` calls. Every millisecond
//! boundary on the way runs the interrupts of the harness that owns the shared state: the hook
//! passed to [`FakeBoard::advance_us_with`] (`motor::bench`) or the one installed with
//! [`FakeBoard::set_on_ms`] (the system bench, so that `delay()` of the code under test runs the
//! interrupts as the C++ fake does). Neither runs while [`BoardState::masked`] is set.

use std::cell::{Cell, RefCell};
use std::ops::Deref;
use std::rc::Rc;
use std::vec::Vec;

use crate::hal::{
    Clock, ControlTimer, In, NoinitStore, Out, Pins, RevIrq, System, Watchdog, NOINIT_SIZE,
};
use crate::i2c_bus::{I2cLine, LineMode, RecoveryPins, Wire};

/// Panic payload of `System::reset` (C++ `fake::SystemReset`).
#[derive(Debug)]
pub struct SystemReset;

/// Panic payload of an expired watchdog (C++ `fake::WatchdogReset`).
#[derive(Debug)]
pub struct WatchdogReset;

/// Recorded calls, in one sequence over all kinds (C++ `fake::Ev`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ev {
    /// `digitalWrite` of an output
    Write(Out, bool),
    Attach,
    Detach,
    /// `delay(ms)`
    Delay(u32),
    /// `delayMicroseconds(us)`
    DelayUs(u32),
    Reload,
    /// `pinMode` / `digitalWrite` of an I2C line during a bus recovery
    LineMode(I2cLine, LineMode),
    LineWrite(I2cLine, bool),
    /// `Wire.begin()` on I2C1 (SDA PB7, SCL PB6)
    WireBegin,
    WireEnd,
}

#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub seq: u32,
    pub kind: Ev,
}

/// Panic payload of a stub that ends an endless loop of the glue (C++ `stub::Stop`).
#[derive(Debug)]
pub struct Stop;

/// Panic payload of the firmware's fault handler (no C++ counterpart): a misuse of the state the
/// interrupts share (`irq::CellGuard`).
#[derive(Debug)]
pub struct Fault;

/// An SDA level for the reads during a recovery, from the events so far; None: the line level
/// (pulled up unless driven low).
pub type SdaInput = Box<dyn Fn(&[Ev]) -> Option<bool>>;

const OUTS: usize = 10;

fn out_index(pin: Out) -> usize {
    pin as usize
}

fn line_index(line: I2cLine) -> usize {
    match line {
        I2cLine::Scl => 0,
        I2cLine::Sda => 1,
    }
}

/// The state behind every handle of one board.
pub struct BoardState {
    now_us: Cell<u64>,
    /// added by every millis()/micros() call: busy loops see time pass
    pub auto_advance_us: Cell<u32>,
    latch: [Cell<bool>; OUTS],
    button: Cell<bool>,
    rev_in: Cell<bool>,
    exti: Cell<bool>,
    /// interrupts masked: EXTI and the per-ms interrupts do not run while it is set
    pub masked: Cell<bool>,
    events: RefCell<Vec<Event>>,
    seq: Cell<u32>,
    /// I2C lines (SCL, SDA): mode and latch, both inputs with latch low at the start
    line_mode: [Cell<LineMode>; 2],
    line_out: [Cell<bool>; 2],
    pub sda_input: RefCell<Option<SdaInput>>,
    pub wire_running: Cell<bool>,
    /// `HAL_GetDEVID()`: the STM32F401xB/C's
    pub dev_id: Cell<u16>,
    /// the interrupts of a millisecond boundary (system bench)
    on_ms: RefCell<Option<Box<dyn FnMut()>>>,
}

/// A handle to one fake board.
#[derive(Clone)]
pub struct FakeBoard(Rc<BoardState>);

impl Deref for FakeBoard {
    type Target = BoardState;

    fn deref(&self) -> &BoardState {
        &self.0
    }
}

impl Default for FakeBoard {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeBoard {
    pub fn new() -> Self {
        let board = FakeBoard(Rc::new(BoardState {
            now_us: Cell::new(0),
            auto_advance_us: Cell::new(0),
            latch: Default::default(),
            button: Cell::new(false),
            rev_in: Cell::new(false),
            exti: Cell::new(false),
            masked: Cell::new(false),
            events: RefCell::new(Vec::new()),
            seq: Cell::new(0),
            line_mode: [Cell::new(LineMode::Input), Cell::new(LineMode::Input)],
            line_out: [Cell::new(false), Cell::new(false)],
            sda_input: RefCell::new(None),
            wire_running: Cell::new(false),
            dev_id: Cell::new(0x423),
            on_ms: RefCell::new(None),
        }));
        board.latch[out_index(Out::PsuEna)].set(true);
        board
    }

    pub fn now_us(&self) -> u64 {
        self.now_us.get()
    }

    /// Sets the time without passing the time between (C++ `fake::board.nowUs = ...`).
    pub fn set_now_us(&self, us: u64) {
        self.now_us.set(us);
    }

    pub fn record(&self, kind: Ev) {
        let seq = self.seq.get() + 1;
        self.seq.set(seq);
        self.events.borrow_mut().push(Event { seq, kind });
    }

    /// The recorded calls, in order.
    pub fn events(&self) -> Vec<Ev> {
        self.events.borrow().iter().map(|e| e.kind).collect()
    }

    /// The recorded calls with their sequence numbers.
    pub fn event_log(&self) -> Vec<Event> {
        self.events.borrow().clone()
    }

    pub fn clear_events(&self) {
        self.events.borrow_mut().clear();
    }

    /// The kinds of the recorded events that `select` takes, in order.
    pub fn events_of(&self, select: impl Fn(&Ev) -> bool) -> Vec<Ev> {
        self.events
            .borrow()
            .iter()
            .filter(|e| select(&e.kind))
            .map(|e| e.kind)
            .collect()
    }

    /// The levels written to one output, in order.
    pub fn writes_of(&self, pin: Out) -> Vec<bool> {
        self.events
            .borrow()
            .iter()
            .filter_map(|e| match e.kind {
                Ev::Write(p, level) if p == pin => Some(level),
                _ => None,
            })
            .collect()
    }

    /// The output latch (C++ `fake::board.out[pin]`).
    pub fn out(&self, pin: Out) -> bool {
        self.latch[out_index(pin)].get()
    }

    /// Sets an output latch without recording a write (a test that sets the hardware up).
    pub fn set_latch(&self, pin: Out, high: bool) {
        self.latch[out_index(pin)].set(high);
    }

    pub fn set_input(&self, pin: In, high: bool) {
        match pin {
            In::Button => self.button.set(high),
            In::RevIn => self.rev_in.set(high),
        }
    }

    /// The modes of the I2C lines (SCL, SDA).
    pub fn line_modes(&self) -> [LineMode; 2] {
        [self.line_mode[0].get(), self.line_mode[1].get()]
    }

    /// A rising edge on REVIN: runs the handler if it is attached and interrupts are not masked
    /// (C++ `fake::fireExti`).
    pub fn fire_exti(&self, handler: impl FnOnce()) {
        if self.exti.get() && !self.masked.get() {
            handler();
        }
    }

    /// The interrupts every millisecond boundary runs from now on (C++ `board.onMs`, the
    /// timers and `board.afterMs` in one hook); None removes them.
    pub fn set_on_ms(&self, hook: Option<Box<dyn FnMut()>>) {
        *self.on_ms.borrow_mut() = hook;
    }

    /// Advances the time; every millisecond boundary on the way runs `on_ms` unless masked.
    pub fn advance_us_with(&self, us: u64, on_ms: &mut dyn FnMut()) {
        let target = self.now_us.get() + us;
        loop {
            let next = (self.now_us.get() / 1000 + 1) * 1000;
            if next > target {
                break;
            }
            self.now_us.set(next);
            if !self.masked.get() {
                on_ms();
            }
        }
        // a hook that waited may have passed the target already
        if self.now_us.get() < target {
            self.now_us.set(target);
        }
    }

    /// Advances the time with the installed hook ([`FakeBoard::set_on_ms`]). An interrupt that
    /// waits (a hook that advances the time itself) runs no nested interrupts, as the C++ fake.
    pub fn advance_us(&self, us: u64) {
        let hook = self.on_ms.borrow_mut().take();
        match hook {
            Some(mut hook) => {
                self.advance_us_with(us, &mut *hook);
                let mut slot = self.on_ms.borrow_mut();
                if slot.is_none() {
                    *slot = Some(hook);
                }
            }
            None => self.advance_us_with(us, &mut || {}),
        }
    }

    pub fn advance_ms(&self, ms: u32) {
        self.advance_us(u64::from(ms) * 1000);
    }
}

impl Pins for FakeBoard {
    fn set(&self, pin: Out, high: bool) {
        self.latch[out_index(pin)].set(high);
        self.record(Ev::Write(pin, high));
    }

    fn latch(&self, pin: Out) -> bool {
        self.out(pin)
    }

    fn read(&self, pin: In) -> bool {
        match pin {
            In::Button => self.button.get(),
            In::RevIn => self.rev_in.get(),
        }
    }
}

impl RevIrq for FakeBoard {
    fn attach(&self) {
        self.exti.set(true);
        self.record(Ev::Attach);
    }

    fn detach(&self) {
        self.exti.set(false);
        self.record(Ev::Detach);
    }
}

impl Clock for FakeBoard {
    fn millis(&self) -> u32 {
        let ms = (self.now_us.get() / 1000) as u32;
        let step = self.auto_advance_us.get();
        if step != 0 {
            self.advance_us(u64::from(step));
        }
        ms
    }

    fn micros(&self) -> u32 {
        let us = self.now_us.get() as u32;
        let step = self.auto_advance_us.get();
        if step != 0 {
            self.advance_us(u64::from(step));
        }
        us
    }

    fn delay_ms(&self, ms: u32) {
        self.record(Ev::Delay(ms));
        self.advance_us(u64::from(ms) * 1000);
    }

    fn delay_us(&self, us: u32) {
        self.record(Ev::DelayUs(us));
        self.advance_us(u64::from(us));
    }
}

/// `HAL_NVIC_SystemReset` ends the boot with a [`SystemReset`] panic.
impl System for FakeBoard {
    fn reset(&self) -> ! {
        std::panic::panic_any(SystemReset)
    }

    fn dev_id(&self) -> u16 {
        self.dev_id.get()
    }
}

impl RecoveryPins for FakeBoard {
    fn mode(&mut self, line: I2cLine, mode: LineMode) {
        self.line_mode[line_index(line)].set(mode);
        self.record(Ev::LineMode(line, mode));
    }

    fn write(&mut self, line: I2cLine, high: bool) {
        self.line_out[line_index(line)].set(high);
        self.record(Ev::LineWrite(line, high));
    }

    /// open drain: driven low by a 0 in the latch, otherwise the level of the line
    fn read(&mut self, line: I2cLine) -> bool {
        let i = line_index(line);
        if self.line_mode[i].get() == LineMode::OutputOpenDrain && !self.line_out[i].get() {
            return false;
        }
        if line == I2cLine::Sda {
            if let Some(input) = self.sda_input.borrow().as_ref() {
                if let Some(level) = input(&self.events()) {
                    return level;
                }
            }
        }
        true
    }
}

impl Wire for FakeBoard {
    fn end(&mut self) {
        self.wire_running.set(false);
        self.record(Ev::WireEnd);
    }

    fn begin(&mut self) {
        self.wire_running.set(true);
        self.record(Ev::WireBegin);
    }
}

/// A control timer (C++ `STM32TimerInterrupt` of the fakes): the interval and the attach calls;
/// the harness runs the callback when it is due.
#[derive(Debug, Default)]
pub struct FakeTimer {
    pub interval_us: u32,
    pub attaches: u32,
    pub fail_attach: bool,
    pub last_fire_us: u64,
    pub fires: u32,
    attached_at_us: Option<u64>,
}

impl FakeTimer {
    /// Attached with a non-zero interval.
    pub fn running(&self) -> bool {
        self.attached_at_us.is_some() && self.interval_us != 0
    }

    /// At a millisecond boundary: true when the interval passed since the last fire (and counts
    /// the fire).
    pub fn due(&mut self, now_us: u64) -> bool {
        if !self.running() || now_us - self.last_fire_us < u64::from(self.interval_us) {
            return false;
        }
        self.last_fire_us = now_us;
        self.fires += 1;
        true
    }

    /// `attachInterruptInterval` at the fake time `now_us`.
    pub fn attach_at(&mut self, now_us: u64, interval_us: u32) -> bool {
        self.attaches += 1;
        if self.fail_attach {
            return false;
        }
        self.interval_us = interval_us;
        self.last_fire_us = now_us;
        self.attached_at_us = Some(now_us);
        true
    }
}

/// A [`FakeTimer`] bound to the time of a board, for the code under test.
pub struct BoardTimer<'a> {
    pub board: &'a FakeBoard,
    pub timer: &'a mut FakeTimer,
}

impl ControlTimer for BoardTimer<'_> {
    fn attach_interval(&mut self, interval_us: u32) -> bool {
        self.timer.attach_at(self.board.now_us(), interval_us)
    }
}

/// The independent watchdog (C++ `IWatchdogClass` of the fakes): started by the firmware before
/// the glue runs; an expired timeout ends the boot with a [`WatchdogReset`] panic.
#[derive(Debug, Default)]
pub struct FakeWatchdog {
    pub enabled: bool,
    pub timeout_us: u32,
    pub reloads: u32,
    pub last_reload_us: u64,
}

impl FakeWatchdog {
    /// `IWatchdog.begin(timeout)`: values outside 125 us .. 32 s are ignored, as in STM32duino.
    pub fn begin(&mut self, now_us: u64, timeout_us: u32) {
        if !(125..=32_000_000).contains(&timeout_us) {
            return;
        }
        self.enabled = true;
        self.timeout_us = timeout_us;
        self.last_reload_us = now_us;
    }

    /// Reload at `now_us`; nothing while not started. Returns true when it counted.
    pub fn reload_at(&mut self, now_us: u64) -> bool {
        if !self.enabled {
            return false;
        }
        self.reloads += 1;
        self.last_reload_us = now_us;
        true
    }

    /// Panics with [`WatchdogReset`] when the timeout passed without a reload.
    pub fn check(&mut self, now_us: u64) {
        if self.enabled && now_us - self.last_reload_us >= u64::from(self.timeout_us) {
            // the reset stops it until the next begin()
            self.enabled = false;
            std::panic::panic_any(WatchdogReset);
        }
    }
}

/// A [`FakeWatchdog`] bound to a board (time and event log).
pub struct BoardWatchdog<'a> {
    pub board: &'a FakeBoard,
    pub dog: &'a mut FakeWatchdog,
}

impl Watchdog for BoardWatchdog<'_> {
    fn reload(&mut self) {
        if self.dog.reload_at(self.board.now_us()) {
            self.board.record(Ev::Reload);
        }
    }
}

/// The no-init RAM cells: 0xA5 after a power-on (the runner hooks of the C++ glue suites).
pub struct FakeNoinit {
    pub bytes: [u8; NOINIT_SIZE],
}

impl Default for FakeNoinit {
    fn default() -> Self {
        FakeNoinit {
            bytes: [0xA5; NOINIT_SIZE],
        }
    }
}

impl NoinitStore for FakeNoinit {
    fn read(&self) -> [u8; NOINIT_SIZE] {
        self.bytes
    }

    fn write(&mut self, image: &[u8; NOINIT_SIZE]) {
        self.bytes = *image;
    }
}

/// `HAL_NVIC_SystemReset` ends the boot with a [`SystemReset`] panic; the device id is the
/// STM32F401xB/C's unless a test sets another.
#[derive(Debug)]
pub struct FakeSystem {
    pub dev_id: u16,
}

impl Default for FakeSystem {
    fn default() -> Self {
        FakeSystem { dev_id: 0x423 }
    }
}

impl System for FakeSystem {
    fn reset(&self) -> ! {
        std::panic::panic_any(SystemReset)
    }

    fn dev_id(&self) -> u16 {
        self.dev_id
    }
}

/// The panics that end a boot ([`SystemReset`], [`WatchdogReset`], [`Stop`]) or stand for the
/// fault handler ([`Fault`]) print nothing; every other panic keeps the default message.
/// Installed once for the whole test binary.
pub fn quiet_expected_panics() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(std::boxed::Box::new(move |info| {
            let p = info.payload();
            if p.is::<SystemReset>() || p.is::<WatchdogReset>() || p.is::<Stop>() || p.is::<Fault>()
            {
                return;
            }
            default(info);
        }));
    });
}

/// Runs `f`; true when it ended with a panic whose payload is a `T` (C++ `CHECK_THROWS_AS`).
pub fn expect_panic<T: 'static>(f: impl FnOnce()) -> bool {
    quiet_expected_panics();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(()) => false,
        Err(payload) => payload.is::<T>(),
    }
}

#[cfg(test)]
mod tests;
