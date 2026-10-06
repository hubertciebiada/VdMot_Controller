//! Presence test of a valve motor (stdet, start-up): the motor output is
//! switched on for a short time and the filtered current tells an open
//! circuit, a present motor and a short circuit apart. Hardware-free; one
//! sample per 10 ms tick of the valve state machine.

use crate::valve_codes::{ValveFault, ST_FAILED, ST_IDLE, ST_OPEN_CIRCUIT, ST_PRESENT, ST_UNKNOWN};

/// C++ PresenceTest::Result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PresenceResult {
    Pending = 0,
    Present = 1,
    Absent = 2,
    Short = 3,
}

#[derive(Clone, Copy, Debug)]
pub struct PresenceTest {
    settle: u8,
    low: u8,
    normal: u8,
    over: u8,
    short_check: bool,
    enforce: bool,
    short_seen: bool,
}

impl Default for PresenceTest {
    /// the C++ member initializers: short check and enforcement on
    fn default() -> Self {
        PresenceTest {
            settle: 0,
            low: 0,
            normal: 0,
            over: 0,
            short_check: true,
            enforce: true,
            short_seen: false,
        }
    }
}

impl PresenceTest {
    /// samples before the current is evaluated
    pub const SETTLE_TICKS: u8 = 7;
    /// 0.1 mA: below is no current
    pub const NO_CURRENT: i32 = 20;
    /// absent after more than 80 samples without current
    pub const ABSENT_TICKS: u8 = 80;
    /// present after more than 5 samples with current
    pub const PRESENT_TICKS: u8 = 5;
    /// 0.1 mA filtered (hardware tuning value)
    pub const SHORT_LIMIT: i32 = 2000;
    /// consecutive samples above SHORT_LIMIT
    pub const SHORT_TICKS: u8 = 3;

    /// short_check false: the short limit is off (protection guard suspended).
    /// enforce false (report only, protection_guard::PROTECT_ENFORCE): a short is only
    /// recorded in short_seen() and the test goes on with the 1.x logic.
    pub fn start(&mut self, short_check: bool, enforce: bool) {
        self.settle = 0;
        self.low = 0;
        self.normal = 0;
        self.over = 0;
        self.short_check = short_check;
        self.enforce = enforce;
        self.short_seen = false;
    }

    /// The short check first, then the 1.x logic unchanged (currents of either sign).
    pub fn sample(&mut self, filtered_current: i32) -> PresenceResult {
        if self.settle <= Self::SETTLE_TICKS {
            self.settle += 1;
        }
        if self.settle <= Self::SETTLE_TICKS {
            return PresenceResult::Pending;
        }

        // i64: |i32::MIN| (undefined in the C++) is a large current here
        let magnitude = i64::from(filtered_current).abs();
        if self.short_check && magnitude > i64::from(Self::SHORT_LIMIT) {
            // the 8-bit counters wrap like the C++ ones (only after 256 samples)
            self.over = self.over.wrapping_add(1);
            if self.over >= Self::SHORT_TICKS {
                self.short_seen = true;
                if self.enforce {
                    return PresenceResult::Short;
                }
            }
        } else {
            self.over = 0;
        }

        if magnitude < i64::from(Self::NO_CURRENT) {
            self.low = self.low.wrapping_add(1);
            if self.low > Self::ABSENT_TICKS {
                return PresenceResult::Absent;
            }
        } else {
            self.normal = self.normal.wrapping_add(1);
            if self.normal > Self::PRESENT_TICKS {
                return PresenceResult::Present;
            }
        }
        PresenceResult::Pending
    }

    /// SHORT_TICKS consecutive samples above SHORT_LIMIT since start()
    pub fn short_seen(&self) -> bool {
        self.short_seen
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresenceOutcome {
    pub status: u8,
    pub needs_reference: bool,
    /// ValveFault
    pub fault: u8,
}

/// Absent -> 6; Short -> 4, fault Short; Present -> 1 with needs_reference for a
/// calibrated valve without recal (its counts are kept, the position is not
/// known), else 8 (calibration pending). Pending -> 5.
pub fn presence_outcome(r: PresenceResult, calibrated: bool, recal: bool) -> PresenceOutcome {
    let none = ValveFault::None as u8;
    match r {
        PresenceResult::Absent => PresenceOutcome {
            status: ST_OPEN_CIRCUIT,
            needs_reference: false,
            fault: none,
        },
        PresenceResult::Short => PresenceOutcome {
            status: ST_FAILED,
            needs_reference: false,
            fault: ValveFault::Short as u8,
        },
        PresenceResult::Present if calibrated && !recal => PresenceOutcome {
            status: ST_IDLE,
            needs_reference: true,
            fault: none,
        },
        PresenceResult::Present => PresenceOutcome {
            status: ST_PRESENT,
            needs_reference: false,
            fault: none,
        },
        PresenceResult::Pending => PresenceOutcome {
            status: ST_UNKNOWN,
            needs_reference: false,
            fault: none,
        },
    }
}
