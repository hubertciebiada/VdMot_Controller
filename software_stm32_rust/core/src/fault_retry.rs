//! Automatic retry of a failed (4) or blocked (9) valve: a calibration (or a
//! presence test after a short) 1 h after the fault, then 6 h, then every
//! 24 h. Hardware-free; time comes in as elapsed seconds.

pub const FAULT_RETRY_FIRST_S: u32 = 3600;
pub const FAULT_RETRY_SECOND_S: u32 = 21600;
pub const FAULT_RETRY_REPEAT_S: u32 = 86400;

/// 0 -> FAULT_RETRY_FIRST_S, 1 -> FAULT_RETRY_SECOND_S, >= 2 -> FAULT_RETRY_REPEAT_S
pub fn fault_retry_interval(attempts: u8) -> u32 {
    match attempts {
        0 => FAULT_RETRY_FIRST_S,
        1 => FAULT_RETRY_SECOND_S,
        _ => FAULT_RETRY_REPEAT_S,
    }
}

/// C++ FaultRetry::Snapshot
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub attempts: u8,
    pub scheduled: bool,
    pub remaining_s: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct FaultRetry {
    attempts: u8,
    scheduled: bool,
    remaining_s: u32,
}

impl FaultRetry {
    /// faulted: status 4 or 9. busy: a calibration or presence test of the valve
    /// is requested or running. Returns true once when a retry is due.
    ///   busy                       -> nothing scheduled (the retry or another
    ///                                 calibration runs), attempts kept, false
    ///   !faulted                   -> reset (attempts 0), false
    ///   faulted, nothing scheduled -> schedule fault_retry_interval(attempts), false
    ///   scheduled, elapsed >= rest -> unschedule, attempts + 1 (saturating), true
    ///   scheduled                  -> rest - elapsed, false
    pub fn update(&mut self, faulted: bool, busy: bool, elapsed_s: u32) -> bool {
        if busy {
            self.scheduled = false;
            return false;
        }
        if !faulted {
            self.attempts = 0;
            self.scheduled = false;
            return false;
        }
        if !self.scheduled {
            self.scheduled = true;
            self.remaining_s = fault_retry_interval(self.attempts);
            return false;
        }
        if elapsed_s < self.remaining_s {
            self.remaining_s -= elapsed_s;
            return false;
        }
        self.scheduled = false;
        self.attempts = self.attempts.saturating_add(1);
        true
    }

    pub fn scheduled(&self) -> bool {
        self.scheduled
    }

    pub fn remaining_s(&self) -> u32 {
        if self.scheduled {
            self.remaining_s
        } else {
            0
        }
    }

    pub fn attempts(&self) -> u8 {
        self.attempts
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            attempts: self.attempts,
            scheduled: self.scheduled,
            remaining_s: self.remaining_s(),
        }
    }

    pub fn restore(&mut self, s: &Snapshot) {
        self.attempts = s.attempts;
        self.scheduled = s.scheduled;
        self.remaining_s = s.remaining_s;
    }
}
