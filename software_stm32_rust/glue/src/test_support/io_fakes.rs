//! Fakes of the HAL traits for the suites of this port (communication, eeprom, i2c_bus,
//! ow_devices, sysstat, terminal): the time, the pins and the I2C lines share one board with one
//! event log, as `fake_board.cpp` records them; a UART with its receive ring; the device id.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use vdm_stm_core::uart_errors::count_uart_errors as comm_rx_irq;
use crate::hal::{Clock, In, Out, Pins, Serial, System};
use crate::i2c_bus::{I2cLine, LineMode, RecoveryPins, Wire};
use vdm_stm_core::uart_errors::UartErrorCounters;

/// What the fakes recorded, in one sequence (fake::Ev of the C++ fakes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ev {
    /// `delay(ms)`
    Delay(u32),
    /// `delayMicroseconds(us)`
    DelayUs(u32),
    /// `digitalWrite` of an output
    Set(Out, bool),
    /// `pinMode` / `digitalWrite` of an I2C line
    LineMode(I2cLine, LineMode),
    LineWrite(I2cLine, bool),
    WireEnd,
    WireBegin,
}

/// Index of an output in the latch array.
fn out_index(pin: Out) -> usize {
    match pin {
        Out::Ena0 => 0,
        Out::Ena1 => 1,
        Out::Ena2 => 2,
        Out::Ena3 => 3,
        Out::Ena4 => 4,
        Out::Ena5 => 5,
        Out::Dir => 6,
        Out::Mux => 7,
        Out::PsuEna => 8,
        Out::Led => 9,
    }
}

/// An SDA level for the reads during a recovery; None: the line level (pulled up unless driven
/// low).
pub type SdaInput = Box<dyn Fn(&[Ev]) -> Option<bool>>;

pub struct Board {
    pub now_us: u64,
    /// added by every millis()/micros() call: busy loops see time pass
    pub auto_advance_us: u32,
    pub events: Vec<Ev>,
    /// output latches
    pub out: [bool; 10],
    pub button: bool,
    pub rev_in: bool,
    /// I2C lines: mode and latch (both start as inputs, latch low)
    pub line_mode: [LineMode; 2],
    pub line_out: [bool; 2],
    pub sda_input: Option<SdaInput>,
    pub wire_running: bool,
}

impl Default for Board {
    fn default() -> Self {
        Board {
            now_us: 0,
            auto_advance_us: 0,
            events: Vec::new(),
            out: [false; 10],
            button: true,
            rev_in: false,
            line_mode: [LineMode::Input; 2],
            line_out: [false; 2],
            sda_input: None,
            wire_running: false,
        }
    }
}

fn line_index(line: I2cLine) -> usize {
    match line {
        I2cLine::Scl => 0,
        I2cLine::Sda => 1,
    }
}

/// One board shared by the clock, the pins and the I2C lines of a test.
#[derive(Clone, Default)]
pub struct FakeBoard(pub Rc<RefCell<Board>>);

impl FakeBoard {
    pub fn new() -> Self {
        FakeBoard::default()
    }

    pub fn advance_ms(&self, ms: u32) {
        self.0.borrow_mut().now_us += u64::from(ms) * 1000;
    }

    pub fn advance_us(&self, us: u64) {
        self.0.borrow_mut().now_us += us;
    }

    pub fn now_us(&self) -> u64 {
        self.0.borrow().now_us
    }

    pub fn set_now_us(&self, us: u64) {
        self.0.borrow_mut().now_us = us;
    }

    pub fn events(&self) -> Vec<Ev> {
        self.0.borrow().events.clone()
    }

    pub fn clear_events(&self) {
        self.0.borrow_mut().events.clear();
    }

    /// The Set events of one output.
    pub fn writes(&self, pin: Out) -> Vec<bool> {
        self.0
            .borrow()
            .events
            .iter()
            .filter_map(|e| match e {
                Ev::Set(p, v) if *p == pin => Some(*v),
                _ => None,
            })
            .collect()
    }

    pub fn out(&self, pin: Out) -> bool {
        self.0.borrow().out[out_index(pin)]
    }

    fn tick(&self) {
        let mut b = self.0.borrow_mut();
        b.now_us += u64::from(b.auto_advance_us);
    }

    fn record(&self, e: Ev) {
        self.0.borrow_mut().events.push(e);
    }
}

impl Clock for FakeBoard {
    fn millis(&self) -> u32 {
        let ms = (self.0.borrow().now_us / 1000) as u32;
        self.tick();
        ms
    }

    fn micros(&self) -> u32 {
        let us = self.0.borrow().now_us as u32;
        self.tick();
        us
    }

    fn delay_ms(&self, ms: u32) {
        self.record(Ev::Delay(ms));
        self.advance_ms(ms);
    }

    fn delay_us(&self, us: u32) {
        self.record(Ev::DelayUs(us));
        self.advance_us(u64::from(us));
    }
}

impl Pins for FakeBoard {
    fn set(&self, pin: Out, high: bool) {
        self.0.borrow_mut().out[out_index(pin)] = high;
        self.record(Ev::Set(pin, high));
    }

    fn latch(&self, pin: Out) -> bool {
        self.out(pin)
    }

    fn read(&self, pin: In) -> bool {
        match pin {
            In::Button => self.0.borrow().button,
            In::RevIn => self.0.borrow().rev_in,
        }
    }
}

impl RecoveryPins for FakeBoard {
    fn mode(&mut self, line: I2cLine, mode: LineMode) {
        self.0.borrow_mut().line_mode[line_index(line)] = mode;
        self.record(Ev::LineMode(line, mode));
    }

    fn write(&mut self, line: I2cLine, high: bool) {
        self.0.borrow_mut().line_out[line_index(line)] = high;
        self.record(Ev::LineWrite(line, high));
    }

    /// open drain: driven low by a 0 in the latch, otherwise the level of the line
    fn read(&mut self, line: I2cLine) -> bool {
        let b = self.0.borrow();
        let i = line_index(line);
        if b.line_mode[i] == LineMode::OutputOpenDrain && !b.line_out[i] {
            return false;
        }
        if line == I2cLine::Sda {
            if let Some(input) = &b.sda_input {
                if let Some(level) = input(&b.events) {
                    return level;
                }
            }
        }
        true
    }
}

impl Wire for FakeBoard {
    fn end(&mut self) {
        self.0.borrow_mut().wire_running = false;
        self.record(Ev::WireEnd);
    }

    fn begin(&mut self) {
        self.0.borrow_mut().wire_running = true;
        self.record(Ev::WireBegin);
    }
}

/// Receive ring size of the firmware (SERIAL_RX_BUFFER_SIZE): one slot stays free.
pub const RX_RING_SIZE: usize = 1024;

/// A UART: the receive ring as the RX interrupt fills it, the transmitted bytes.
#[derive(Default)]
pub struct FakeSerial {
    rx: VecDeque<u8>,
    pub tx: Vec<u8>,
    pub read_calls: u32,
    pub flushes: u32,
    /// HAL error code of the next received byte
    pub next_error: u32,
    /// the counters of the USART1 RX interrupt (communication::comm_rx_irq)
    pub errors: UartErrorCounters,
}

impl FakeSerial {
    pub fn new() -> Self {
        FakeSerial::default()
    }

    /// Bytes arrive one by one through the RX interrupt: its error code is counted and a byte
    /// that finds the ring full is dropped.
    pub fn inject(&mut self, bytes: &[u8]) {
        for &b in bytes {
            let ring_full = self.rx.len() == RX_RING_SIZE - 1;
            comm_rx_irq(&mut self.errors, self.next_error, ring_full);
            self.next_error = 0;
            if !ring_full {
                self.rx.push_back(b);
            }
        }
    }

    pub fn inject_str(&mut self, s: &str) {
        self.inject(s.as_bytes());
    }

    /// The bytes written since the last take, as text.
    pub fn take_tx(&mut self) -> String {
        let out = String::from_utf8_lossy(&self.tx).into_owned();
        self.tx.clear();
        out
    }
}

impl Serial for FakeSerial {
    fn available(&self) -> usize {
        self.rx.len()
    }

    fn read(&mut self) -> Option<u8> {
        self.read_calls += 1;
        self.rx.pop_front()
    }

    fn write(&mut self, bytes: &[u8]) {
        self.tx.extend_from_slice(bytes);
    }

    fn flush(&mut self) {
        self.flushes += 1;
    }
}

/// The device id of the STM32F401CC; a reset ends the test with a panic.
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
        panic!("SystemReset");
    }

    fn dev_id(&self) -> u16 {
        self.dev_id
    }
}
