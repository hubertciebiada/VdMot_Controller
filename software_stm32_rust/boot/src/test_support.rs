//! Fake board of the boot stage tests (the role of `test/native/glue` fakes for
//! `test_otasupport.cpp` and `test_sysstat.cpp`): time in microseconds, SysTick from the
//! reload value and the system clock, the HSE with a start-up time, USART1 with a schedule of
//! received bytes (one data register: a byte that arrives while the previous one is unread is
//! lost, as with ORE), and an event log with time stamps.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::panic
)]

use std::collections::VecDeque;
use std::vec::Vec;

use crate::capture::NOINIT_LEN;
use crate::id_block::BootId;
use crate::io::{BootHw, BootIo, ClockIo};
use crate::stage::{HSE_HZ, HSI_HZ};

/// One character at 115200 8E1 (11 bits) in µs, rounded.
pub const BYTE_US: u64 = 95;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ev {
    ResetFlags,
    ClearResetFlags,
    Noinit,
    SetNoinit,
    OutputsSafe,
    TickStart(u32),
    TickStop,
    HseOn,
    HseOff,
    SysclkHse,
    LedBegin,
    Led(bool),
    UartBegin(u16),
    UartEnd,
    Tx(u8),
    Flush,
    BootId,
    WatchdogStart,
}

pub struct Fake {
    pub now_us: u64,
    /// CPU time of one pass of a polling loop (one `tick()` call)
    pub poll_us: u64,
    pub sysclk_hz: u32,
    tick_period_us: Option<u64>,
    next_tick_us: u64,
    pub log: Vec<(u64, Ev)>,
    /// received bytes: (arrival time, byte), in time order
    rx_schedule: VecDeque<(u64, u8)>,
    pub rx_reads: usize,
    pub rx_overruns: usize,
    pub uart_on: bool,
    pub led: bool,
    pub tx_bytes: Vec<u8>,
    pub csr: u32,
    pub cells: [u8; NOINIT_LEN],
    /// HSERDY this long after HSEON; `None`: the crystal is dead
    pub hse_start_us: Option<u64>,
    hse_on_at: Option<u64>,
    pub id: BootId,
}

impl Default for Fake {
    fn default() -> Self {
        Self::new()
    }
}

impl Fake {
    pub fn new() -> Self {
        Fake {
            now_us: 0,
            poll_us: 5,
            sysclk_hz: HSI_HZ,
            tick_period_us: None,
            next_tick_us: 0,
            log: Vec::new(),
            rx_schedule: VecDeque::new(),
            rx_reads: 0,
            rx_overruns: 0,
            uart_on: false,
            led: false,
            tx_bytes: Vec::new(),
            csr: 0,
            cells: [0xA5; NOINIT_LEN],
            hse_start_us: Some(500),
            hse_on_at: None,
            id: BootId::STANDARD,
        }
    }

    fn ev(&mut self, e: Ev) {
        self.log.push((self.now_us, e));
    }

    /// The events without time stamps.
    pub fn events(&self) -> Vec<Ev> {
        self.log.iter().map(|(_, e)| e.clone()).collect()
    }

    /// Time stamps of the events equal to `e`.
    pub fn times_of(&self, e: &Ev) -> Vec<u64> {
        self.log
            .iter()
            .filter(|(_, x)| x == e)
            .map(|(t, _)| *t)
            .collect()
    }

    pub fn count(&self, pred: impl Fn(&Ev) -> bool) -> usize {
        self.log.iter().filter(|(_, e)| pred(e)).count()
    }

    /// SysTick running with 1 ms on the current clock (the HAL tick of the C++ tests).
    pub fn start_ms_clock(&mut self) {
        let reload = self.sysclk_hz / 1000 - 1;
        self.tick_start(reload);
        self.log.pop();
    }

    /// Bytes arriving on USART1 from `at_us` on, back to back at 115200 8E1.
    pub fn send(&mut self, at_us: u64, bytes: &[u8]) {
        for (i, &b) in bytes.iter().enumerate() {
            let t = at_us + BYTE_US * (i as u64 + 1);
            let pos = self.rx_schedule.iter().position(|&(x, _)| x > t);
            match pos {
                Some(p) => self.rx_schedule.insert(p, (t, b)),
                None => self.rx_schedule.push_back((t, b)),
            }
        }
    }
}

impl ClockIo for Fake {
    fn tick_start(&mut self, reload: u32) {
        self.ev(Ev::TickStart(reload));
        let period = (u64::from(reload) + 1) * 1_000_000 / u64::from(self.sysclk_hz);
        self.tick_period_us = Some(period);
        self.next_tick_us = self.now_us + period;
    }

    fn tick(&mut self) -> bool {
        self.now_us += self.poll_us;
        match self.tick_period_us {
            Some(period) if self.now_us >= self.next_tick_us => {
                // COUNTFLAG is one bit: periods that passed unread count once
                while self.next_tick_us <= self.now_us {
                    self.next_tick_us += period;
                }
                true
            }
            _ => false,
        }
    }

    fn tick_stop(&mut self) {
        self.ev(Ev::TickStop);
        self.tick_period_us = None;
    }

    fn hse_on(&mut self) {
        self.ev(Ev::HseOn);
        self.hse_on_at = Some(self.now_us);
    }

    fn hse_ready(&mut self) -> bool {
        match (self.hse_on_at, self.hse_start_us) {
            (Some(on), Some(start)) => self.now_us >= on + start,
            _ => false,
        }
    }

    fn hse_off(&mut self) {
        self.ev(Ev::HseOff);
        self.hse_on_at = None;
    }

    fn sysclk_hse(&mut self) {
        self.ev(Ev::SysclkHse);
        assert!(
            self.hse_ready(),
            "SYSCLK switched to an HSE that is not ready"
        );
        self.sysclk_hz = HSE_HZ;
    }
}

impl BootIo for Fake {
    fn led_begin(&mut self) {
        self.ev(Ev::LedBegin);
    }

    fn set_led(&mut self, high: bool) {
        self.ev(Ev::Led(high));
        self.led = high;
    }

    fn led(&self) -> bool {
        self.led
    }

    fn uart_begin(&mut self, brr: u16) {
        self.ev(Ev::UartBegin(brr));
        self.uart_on = true;
        // nothing was received before the USART was on
        while matches!(self.rx_schedule.front(), Some(&(t, _)) if t <= self.now_us) {
            self.rx_schedule.pop_front();
        }
    }

    fn rx(&mut self) -> Option<u8> {
        if !self.uart_on {
            return None;
        }
        let (t, byte) = *self.rx_schedule.front()?;
        if t > self.now_us {
            return None;
        }
        self.rx_schedule.pop_front();
        // bytes that completed while this one waited in the data register are lost
        while matches!(self.rx_schedule.front(), Some(&(t2, _)) if t2 <= self.now_us) {
            self.rx_schedule.pop_front();
            self.rx_overruns += 1;
        }
        self.rx_reads += 1;
        Some(byte)
    }

    fn tx(&mut self, byte: u8) {
        self.ev(Ev::Tx(byte));
        self.tx_bytes.push(byte);
        self.now_us += BYTE_US;
    }

    fn flush(&mut self) {
        self.ev(Ev::Flush);
    }

    fn uart_end(&mut self) {
        self.ev(Ev::UartEnd);
        self.uart_on = false;
    }
}

impl BootHw for Fake {
    fn reset_flags(&mut self) -> u32 {
        self.ev(Ev::ResetFlags);
        self.csr
    }

    fn clear_reset_flags(&mut self) {
        self.ev(Ev::ClearResetFlags);
        self.csr = 0;
    }

    fn noinit(&mut self) -> [u8; NOINIT_LEN] {
        self.ev(Ev::Noinit);
        self.cells
    }

    fn set_noinit(&mut self, cells: &[u8; NOINIT_LEN]) {
        self.ev(Ev::SetNoinit);
        self.cells = *cells;
    }

    fn outputs_safe(&mut self) {
        self.ev(Ev::OutputsSafe);
    }

    fn boot_id(&mut self) -> BootId {
        self.ev(Ev::BootId);
        self.id
    }

    fn watchdog_start(&mut self) {
        self.ev(Ev::WatchdogStart);
    }
}

/// The ESP 2.1 flasher after its NRST release at t = 0: `DEADBEEF\n` at 20 ms and then every
/// 100 ms for 2.5 s (DESIGN.md §15); `first` is the time of the first send.
pub fn esp21_sends(fake: &mut Fake, first_us: u64) -> Vec<u64> {
    let mut at = first_us;
    let mut times = Vec::new();
    while at <= 2_500_000 {
        fake.send(at, b"DEADBEEF\n");
        times.push(at);
        at += 100_000;
    }
    times
}
