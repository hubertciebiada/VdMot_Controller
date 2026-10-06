//! When the configuration EEPROM is written and read again. The glue (eepromloop(), once per
//! second) runs the steps tick() returns and reports their results. Hardware-free.
//!
//! - A change is written DEBOUNCE_S ticks after the last change, at most MAX_DELAY_S ticks after
//!   the first unsaved one, so a client that repeats its configuration does not wear the EEPROM.
//! - A failed write is repeated at the next ticks, WRITE_ATTEMPTS per change; then the change is
//!   given up for now (write_failed) and retried on a RetryBackoff schedule. A change while
//!   write_failed gets one attempt (the bus is known to be bad).
//! - After a failed read nothing is written (RAM holds fallbacks for what could not be read): the
//!   read is repeated on the backoff schedule (Reread) and the changes wait.
//! - retrying(): the step follows a failure, the glue restarts the I2C bus before it.

use crate::replies_v2::{
    EEP_STATE_OK, EEP_STATE_PENDING, EEP_STATE_READ_FAILED, EEP_STATE_WRITE_FAILED,
};
use crate::retry_backoff::RetryBackoff;

/// C++ StoreScheduler::Step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Step {
    None = 0,
    Write = 1,
    Reread = 2,
}

#[derive(Clone, Copy, Debug)]
pub struct StoreScheduler {
    backoff: RetryBackoff,
    dirty: u16,
    since_change: u16,
    since_first: u16,
    /// failed attempts of the pending change
    attempts: u8,
    /// a change waits for its write (debounce or attempts left)
    pending: bool,
    read_failed: bool,
    /// the attempts failed, the backoff retries the write
    write_failed: bool,
}

impl Default for StoreScheduler {
    /// StoreScheduler::new(30, 3600) (the C++ default arguments)
    fn default() -> Self {
        StoreScheduler::new(30, 3600)
    }
}

impl StoreScheduler {
    /// write 3 s after the last change
    pub const DEBOUNCE_S: u16 = 3;
    /// at most 30 s after the first unsaved change
    pub const MAX_DELAY_S: u16 = 30;
    /// per change, one per tick
    pub const WRITE_ATTEMPTS: u8 = 3;

    pub fn new(retry_first_s: u32, retry_max_s: u32) -> Self {
        StoreScheduler {
            backoff: RetryBackoff::new(retry_first_s, retry_max_s),
            dirty: 0,
            since_change: 0,
            since_first: 0,
            attempts: 0,
            pending: false,
            read_failed: false,
            write_failed: false,
        }
    }

    /// result of the start-up read or of a Reread
    pub fn read_result(&mut self, ok: bool) {
        self.read_failed = !ok;
        if ok {
            self.backoff.succeeded();
        } else {
            self.backoff.failed();
        }
    }

    /// fields (CHANGED_*, config_store) changed in RAM
    pub fn changed(&mut self, fields: u16) {
        self.dirty |= fields;
        if !self.pending {
            self.since_first = 0;
        }
        self.pending = true;
        self.since_change = 0;
    }

    pub fn tick(&mut self) -> Step {
        if self.read_failed {
            return if self.backoff.tick() {
                Step::Reread
            } else {
                Step::None
            };
        }
        if self.pending {
            // after a failed attempt the debounce has passed already: the write is repeated at once
            self.since_change = self.since_change.wrapping_add(1);
            self.since_first = self.since_first.wrapping_add(1);
            return if self.since_change >= Self::DEBOUNCE_S || self.since_first >= Self::MAX_DELAY_S
            {
                Step::Write
            } else {
                Step::None
            };
        }
        if self.write_failed && self.backoff.tick() {
            Step::Write
        } else {
            Step::None
        }
    }

    /// the step tick() returned follows a failed read or write
    pub fn retrying(&self) -> bool {
        self.read_failed || self.write_failed || self.attempts > 0
    }

    /// the fields the Write step has to store
    pub fn dirty(&self) -> u16 {
        self.dirty
    }

    pub fn write_result(&mut self, ok: bool) {
        if ok {
            self.dirty = 0;
            self.pending = false;
            self.attempts = 0;
            self.write_failed = false;
            self.backoff.succeeded();
            return;
        }
        // attempts is not counted while write_failed, the next successful write clears it
        if self.write_failed {
            self.give_up();
            return;
        }
        self.attempts = self.attempts.wrapping_add(1);
        if self.attempts >= Self::WRITE_ATTEMPTS {
            self.give_up();
        }
    }

    fn give_up(&mut self) {
        self.pending = false;
        self.write_failed = true;
        self.backoff.failed();
    }

    pub fn read_failed(&self) -> bool {
        self.read_failed
    }

    pub fn write_failed(&self) -> bool {
        self.write_failed
    }

    /// gstat eepState: EEP_STATE_READ_FAILED, EEP_STATE_PENDING, EEP_STATE_WRITE_FAILED,
    /// EEP_STATE_OK
    pub fn eep_state(&self) -> u8 {
        if self.read_failed {
            EEP_STATE_READ_FAILED
        } else if self.pending {
            EEP_STATE_PENDING
        } else if self.write_failed {
            EEP_STATE_WRITE_FAILED
        } else {
            EEP_STATE_OK
        }
    }

    /// a reset may happen now: no write is waiting (or none can happen until a read succeeds)
    pub fn free(&self) -> bool {
        self.read_failed || !self.pending
    }
}

#[cfg(test)]
mod tests;
