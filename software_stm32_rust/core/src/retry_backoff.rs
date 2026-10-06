//! Retry schedule for an operation that failed (e.g. an EEPROM read or write):
//! the first retry `first` ticks after the failure, every further one after
//! twice the previous interval, at most `max` ticks. Hardware-free.

#[derive(Clone, Copy, Debug)]
pub struct RetryBackoff {
    first: u32,
    max: u32,
    /// 0: no retry scheduled
    interval: u32,
    remaining: u32,
}

impl RetryBackoff {
    /// first >= 1 and max >= first are enforced.
    pub fn new(first: u32, max: u32) -> Self {
        let first = first.max(1);
        RetryBackoff {
            first,
            max: max.max(first),
            interval: 0,
            remaining: 0,
        }
    }

    /// The operation failed (again): schedules the next retry.
    pub fn failed(&mut self) {
        self.interval = if self.interval == 0 {
            self.first
        } else if self.interval > self.max / 2 {
            self.max
        } else {
            self.interval * 2
        };
        self.remaining = self.interval;
    }

    /// The operation succeeded: no retry scheduled, the next failure starts
    /// with the first interval again.
    pub fn succeeded(&mut self) {
        self.interval = 0;
        self.remaining = 0;
    }

    /// One tick passed. Returns true while a retry is due; it stays due until
    /// failed() or succeeded() is called.
    pub fn tick(&mut self) -> bool {
        if self.interval == 0 {
            return false;
        }
        self.remaining = self.remaining.saturating_sub(1);
        self.remaining == 0
    }

    pub fn pending(&self) -> bool {
        self.interval != 0
    }

    pub fn interval(&self) -> u32 {
        self.interval
    }
}
