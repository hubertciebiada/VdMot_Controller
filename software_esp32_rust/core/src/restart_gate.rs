//! Restart gate: before an ESP restart (which also resets the STM when jumper X20 is fitted) the
//! STM gets the chance to finish a pending EEPROM write. The stm task answers through
//! app::setStmSaveState() (port of `vdm/restart_gate.h`). Hardware-free.

use crate::common::elapsed_ms;
use crate::stm_types::StmSaveState;

/// [`RestartGate::update`] (C++ `RestartGate::Step`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartGateStep {
    RequestStmSave,
    Wait,
    Proceed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RestartGate {
    started: bool,
    done: bool,
    guard_expired: bool,
    start_ms: u32,
}

impl RestartGate {
    /// The stm task gives up after 10 s (TimedOut); this guard covers an stm task that does not
    /// answer at all.
    pub const GUARD_MS: u32 = 12_000;

    /// Every pass once a restart is due. First call -> RequestStmSave (the timer starts); then
    /// Saved, Unavailable or TimedOut -> Proceed; >= GUARD_MS -> Proceed with
    /// [`guard_expired`](Self::guard_expired); else Wait. Proceed is final until
    /// [`reset`](Self::reset).
    pub fn update(&mut self, s: StmSaveState, now_ms: u32) -> RestartGateStep {
        if self.done {
            return RestartGateStep::Proceed;
        }
        if !self.started {
            self.started = true;
            self.start_ms = now_ms;
            return RestartGateStep::RequestStmSave;
        }
        if matches!(
            s,
            StmSaveState::Saved | StmSaveState::Unavailable | StmSaveState::TimedOut
        ) {
            self.done = true;
            return RestartGateStep::Proceed;
        }
        if elapsed_ms(now_ms, self.start_ms) < Self::GUARD_MS {
            return RestartGateStep::Wait;
        }
        self.done = true;
        self.guard_expired = true;
        RestartGateStep::Proceed
    }

    pub fn guard_expired(&self) -> bool {
        self.guard_expired
    }

    /// Since RequestStmSave, 0 before.
    pub fn waited_ms(&self, now_ms: u32) -> u32 {
        if self.started {
            elapsed_ms(now_ms, self.start_ms)
        } else {
            0
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
