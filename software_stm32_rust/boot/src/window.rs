//! The boot window after reset in which the ESP may start an STM update: `DEADBEEF` ->
//! `BEEFIT` -> ROM bootloader (port of `src/otasupport.cpp`, PROTOCOL_V2.md "Firmware update
//! handshake"). The ESP drives NRST but not BOOT0, so this window is the only way to re-flash
//! the STM without opening the cabinet.
//!
//! `BootSetup`: LED on, USART1 115200 8E1, 10 ms, then the bytes received meanwhile are
//! dropped. `BootLoop`, one call per millisecond: the C++ reads fixed 8-byte blocks whenever 8
//! bytes wait and compares each with `DEADBEEF`, without skipping CR, LF or any other byte and
//! without resynchronising (D2: kept as in C++). A match answers `BEEFIT\r\n` on the next call
//! and ends in the ROM bootloader; without a match the window ends after 3001 calls.

use crate::fifo::Fifo;
use crate::id_block::BootId;
use crate::io::BootIo;

/// `timer > 3000` ends the window: calls 1..=3001 listen.
pub const WINDOW_CALLS: u32 = 3000;
/// `ledtimer > 100` toggles the LED: every 102nd call.
pub const LED_PERIOD: u32 = 100;
/// `delay(10)` of `BootSetup`; what arrives meanwhile is dropped.
pub const SETUP_DROP_MS: u32 = 10;
/// `delay(10)` before `BEEFIT`.
pub const BEFORE_REPLY_MS: u32 = 10;
/// `delay(200)` after `BEEFIT` ("wait for send"), then the jump.
pub const AFTER_REPLY_MS: u32 = 200;
/// The calls of a window at most: 3001 listening calls and the call that jumps or ends it.
pub const MAX_CALLS: u32 = WINDOW_CALLS + 2;

/// Waits `ms` SysTick periods (`delay(ms)`: the first period may be partial) and stores what
/// USART1 receives meanwhile in `fifo`, as the C++ RX interrupt does during `delay`.
/// `elapsed` counts the periods (the boot stage's millisecond clock).
pub fn wait_ms<I: BootIo>(io: &mut I, ms: u32, fifo: &mut Fifo, elapsed: &mut u32) {
    wait_ms_with(io, ms, fifo, elapsed, &mut |_: &mut I| {});
}

/// [`wait_ms`] that runs `work` once per pass of its polling loop, after the receiver: the
/// check of the application part uses the idle time of the window. A pass must stay well below
/// one character time (95 us) or a byte is lost.
pub fn wait_ms_with<I: BootIo, W: FnMut(&mut I)>(
    io: &mut I,
    ms: u32,
    fifo: &mut Fifo,
    elapsed: &mut u32,
    work: &mut W,
) {
    let mut left = ms;
    while left > 0 {
        if let Some(byte) = io.rx() {
            fifo.push(byte);
        }
        work(io);
        if io.tick() {
            left = left.saturating_sub(1);
            *elapsed = elapsed.wrapping_add(1);
        }
    }
}

/// `BootSetup` after the clock is set: LED on, USART1 8E1 at `brr`, 10 ms, drop what came.
pub fn setup<I: BootIo>(io: &mut I, brr: u16, fifo: &mut Fifo, elapsed: &mut u32) {
    io.led_begin();
    io.set_led(false);
    io.uart_begin(brr);
    wait_ms(io, SETUP_DROP_MS, fifo, elapsed);
    fifo.clear();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// state 0: listening
    Listening,
    /// state 1: DEADBEEF seen, the next call answers and jumps
    Matched,
    /// state 2: the window is over
    Over,
}

/// What one `BootLoop` call ended with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// the window goes on
    Continue,
    /// `BEEFIT` was sent: jump into the ROM bootloader now
    Jump,
    /// the window is over: start the application (`bootstate = 1`)
    Timeout,
}

/// How the window ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowEnd {
    /// `BEEFIT` was sent: the caller jumps into the ROM bootloader.
    Update,
    /// No handshake: the application starts.
    Timeout,
}

/// The state of `BootLoop` (the C++ function statics `timer`, `ledtimer`, `state`).
#[derive(Clone, Debug)]
pub struct Window {
    timer: u32,
    led_timer: u32,
    state: State,
}

impl Default for Window {
    fn default() -> Self {
        Self::new()
    }
}

impl Window {
    pub const fn new() -> Self {
        Window {
            timer: 0,
            led_timer: 0,
            state: State::Listening,
        }
    }

    /// One `BootLoop()` call.
    pub fn step<I: BootIo>(
        &mut self,
        io: &mut I,
        fifo: &mut Fifo,
        id: &BootId,
        elapsed: &mut u32,
    ) -> Step {
        self.step_with(io, fifo, id, elapsed, &mut |_: &mut I| {})
    }

    /// [`Window::step`] with `work` in the idle time of its listening millisecond
    /// ([`wait_ms_with`]).
    pub fn step_with<I: BootIo, W: FnMut(&mut I)>(
        &mut self,
        io: &mut I,
        fifo: &mut Fifo,
        id: &BootId,
        elapsed: &mut u32,
        work: &mut W,
    ) -> Step {
        match self.state {
            State::Listening => {
                self.timer = self.timer.saturating_add(1);
                if self.timer > WINDOW_CALLS {
                    self.state = State::Over;
                }
                // a match in the last call still starts the update (it overrides state 2)
                if let Some(block) = fifo.take_block() {
                    if block == id.pattern {
                        self.state = State::Matched;
                        io.set_led(false);
                    }
                }
                if self.led_timer > LED_PERIOD {
                    let led = io.led();
                    io.set_led(!led);
                    self.led_timer = 0;
                } else {
                    self.led_timer = self.led_timer.saturating_add(1);
                }
                wait_ms_with(io, 1, fifo, elapsed, work);
                Step::Continue
            }
            State::Matched => {
                io.set_led(true);
                wait_ms(io, BEFORE_REPLY_MS, fifo, elapsed);
                // Serial1.println("BEEFIT"); CR and LF as immediates: no .rodata outside the
                // boot stage's flash (D9)
                for &byte in id.reply.iter() {
                    io.tx(byte);
                }
                io.tx(b'\r');
                io.tx(b'\n');
                io.flush();
                wait_ms(io, AFTER_REPLY_MS, fifo, elapsed);
                Step::Jump
            }
            State::Over => Step::Timeout,
        }
    }
}

/// The whole window: `BootSetup`, then `BootLoop` until it jumps or ends (at most
/// [`MAX_CALLS`] calls, about 3.0 s; 3.2 s with the reply).
pub fn window<I: BootIo>(io: &mut I, id: &BootId, brr: u16, elapsed: &mut u32) -> WindowEnd {
    window_with(io, id, brr, elapsed, &mut |_: &mut I| {})
}

/// [`window`] with `work` in the idle time of its listening calls ([`wait_ms_with`]).
pub fn window_with<I: BootIo, W: FnMut(&mut I)>(
    io: &mut I,
    id: &BootId,
    brr: u16,
    elapsed: &mut u32,
    work: &mut W,
) -> WindowEnd {
    let mut fifo = Fifo::new();
    setup(io, brr, &mut fifo, elapsed);
    let mut w = Window::new();
    for _ in 0..MAX_CALLS {
        match w.step_with(io, &mut fifo, id, elapsed, work) {
            Step::Continue => {}
            Step::Jump => return WindowEnd::Update,
            Step::Timeout => return WindowEnd::Timeout,
        }
    }
    WindowEnd::Timeout
}

#[cfg(test)]
mod tests;
