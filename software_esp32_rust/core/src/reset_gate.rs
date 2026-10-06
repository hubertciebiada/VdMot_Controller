//! Wait for pending STM EEPROM writes before the ESP pulses NRST (user reset, flash start) or
//! restarts itself (jumper X20 resets the STM with it): the STM gets up to MAX_WAIT_MS; the ESP
//! lets its queued User/Config requests go out first, then polls `eepst` every POLL_MS until
//! "eepst 1" (port of `vdm/reset_gate.h`). Hardware-free.

use crate::common::elapsed_ms;

/// [`ResetGate::state`] (C++ `ResetGate::State`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResetGateState {
    #[default]
    Idle,
    Waiting,
    Ready,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResetGate {
    state: ResetGateState,
    began_ms: u32,
    in_flight: bool,
    polled_once: bool,
    /// completion of the last poll
    last_poll_ms: u32,
}

impl ResetGate {
    pub const POLL_MS: u32 = 500;
    pub const MAX_WAIT_MS: u32 = 10_000;

    /// `stm_answers` false (link not Up/Degraded): Ready at once.
    pub fn begin(&mut self, now_ms: u32, stm_answers: bool) {
        self.state = if stm_answers {
            ResetGateState::Waiting
        } else {
            ResetGateState::Ready
        };
        self.began_ms = now_ms;
        self.in_flight = false;
        self.polled_once = false;
        self.last_poll_ms = now_ms;
    }

    /// Waiting, no eepst in flight, no queued User/Config requests (`requests_pending` false)
    /// and POLL_MS since the last poll completed; the first poll goes out at once.
    pub fn poll_due(&self, now_ms: u32, requests_pending: bool) -> bool {
        if self.state != ResetGateState::Waiting || self.in_flight || requests_pending {
            return false;
        }
        !self.polled_once || elapsed_ms(now_ms, self.last_poll_ms) >= Self::POLL_MS
    }

    pub fn on_poll_sent(&mut self, _now_ms: u32) {
        self.in_flight = true;
        self.polled_once = true;
    }

    /// Result of the eepst request: answered && idle -> Ready; otherwise the next poll POLL_MS
    /// later.
    pub fn on_eepst(&mut self, answered: bool, idle: bool, now_ms: u32) {
        self.in_flight = false;
        self.last_poll_ms = now_ms;
        if self.state == ResetGateState::Waiting && answered && idle {
            self.state = ResetGateState::Ready;
        }
    }

    /// Waiting -> TimedOut once MAX_WAIT_MS passed since begin(); returns the state.
    pub fn update(&mut self, now_ms: u32) -> ResetGateState {
        if self.state == ResetGateState::Waiting
            && elapsed_ms(now_ms, self.began_ms) >= Self::MAX_WAIT_MS
        {
            self.state = ResetGateState::TimedOut;
        }
        self.state
    }

    pub fn state(&self) -> ResetGateState {
        self.state
    }

    /// Since begin(), 0 while Idle.
    pub fn waited_ms(&self, now_ms: u32) -> u32 {
        if self.state == ResetGateState::Idle {
            0
        } else {
            elapsed_ms(now_ms, self.began_ms)
        }
    }

    /// -> Idle. poll_due() is false whatever the poll flags say; begin() sets them.
    pub fn reset(&mut self) {
        self.state = ResetGateState::Idle;
    }
}

#[cfg(test)]
mod tests;
