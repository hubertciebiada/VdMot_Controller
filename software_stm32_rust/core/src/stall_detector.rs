//! Detects a state machine that stays in one busy state for too long. Hardware-free.

#[derive(Clone, Copy, Debug)]
pub struct StallDetector {
    limit: u32,
    age: u32,
    state: u8,
    busy: bool,
}

impl StallDetector {
    pub const fn new(limit_ticks: u32) -> Self {
        StallDetector {
            limit: limit_ticks,
            age: 0,
            state: 0,
            busy: false,
        }
    }

    /// Call once per state machine tick with the current state. Being idle or
    /// entering a different state counts as progress and restarts the count.
    pub fn tick(&mut self, state: u8, idle: bool) {
        if idle || !self.busy || state != self.state {
            self.busy = !idle;
            self.state = state;
            self.age = 0;
        } else {
            // the age stops at the limit (C++: ++age_ while age_ < limit_)
            self.age = self.age.saturating_add(1).min(self.limit);
        }
    }

    /// True once the same busy state was seen on `limit_ticks` further ticks.
    pub fn stalled(&self) -> bool {
        self.busy && self.age >= self.limit
    }
}
