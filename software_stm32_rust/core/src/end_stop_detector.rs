//! End-stop / overcurrent detection of a running valve motor. Hardware-free.
//! Fed once per millisecond (TIM1 interrupt) with the raw motor current; all
//! currents are in 0.1 mA and signed (the sign depends on the direction).

/// far outside what the ADC can deliver; keeps the filter arithmetic in range
const RAW_LIMIT: i32 = 100_000;

/// What the inrush limit does (protection_guard::PROTECT_ENFORCE, protection guard):
/// Off, Report (only inrush_seen()), Enforce (trips Hard, inrush_trip()).
/// C++ EndStopDetector::InrushMode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum InrushMode {
    #[default]
    Off = 0,
    Report = 1,
    Enforce = 2,
}

/// C++ EndStopDetector::Trip.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Trip {
    #[default]
    None = 0,
    /// filtered current left [low, high] after the inrush time (end stop)
    Bound = 1,
    /// consecutive samples above the safety limit
    Safety = 2,
    /// hard limit exceeded
    Hard = 3,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EndStopDetector {
    low: i32,
    high: i32,
    current: i32,
    peak: i32,
    trip_current: i32,
    debounce: u16,
    over_count: u8,
    inrush_over: u8,
    inrush: InrushMode,
    inrush_seen: bool,
    inrush_trip: bool,
    trip: Trip,
}

impl EndStopDetector {
    /// Samples ignored after the motor start (inrush); the filter is held at 0.
    /// All limits below compare the filtered current, so none of them can trip
    /// during the inrush time, and after it the filter (alpha 0.02) needs some
    /// tens of samples to reach them (firmware 1.x behaves the same).
    pub const INRUSH_SAMPLES: u16 = 250;
    /// Safety limit: more than SAFETY_CONSECUTIVE consecutive filtered samples
    /// above it trip.
    pub const SAFETY_LIMIT: i32 = 600;
    pub const SAFETY_CONSECUTIVE: u8 = 10;
    /// Hard limit: the first filtered sample above it trips.
    pub const HARD_LIMIT: i32 = 1000;
    /// Inrush limit (hardware tuning values): during the inrush time more than
    /// INRUSH_CONSECUTIVE consecutive raw samples above INRUSH_LIMIT are a short
    /// or a hard stall at the start (|raw| saturates near 2824).
    pub const INRUSH_LIMIT: i32 = 2500;
    pub const INRUSH_CONSECUTIVE: u8 = 20;

    /// Sets the end-stop bounds and the inrush mode for the next move and clears
    /// the per-move statistics (peak, trip, inrush). Call before the motor is
    /// switched on. (C++ default for inrush: InrushMode::Off.)
    pub fn arm(&mut self, low: i32, high: i32, inrush: InrushMode) {
        self.low = low;
        self.high = high;
        self.peak = 0;
        self.trip_current = 0;
        self.trip = Trip::None;
        self.inrush = inrush;
        self.inrush_seen = false;
        self.inrush_trip = false;
    }

    /// One sample while the motor runs. Returns the trip condition of this
    /// sample (Hard before Safety before Bound); the first trip of a move is
    /// latched in trip() / trip_current().
    pub fn sample(&mut self, raw: i32) -> Trip {
        let raw = raw.clamp(-RAW_LIMIT, RAW_LIMIT);
        // counts up to 255 (C++: if (debounce_ < 255) debounce_++)
        self.debounce = (self.debounce + 1).min(255);

        // first order IIR, alpha = 0.02, same integer arithmetic as firmware 1.x
        let settled = self.debounce > Self::INRUSH_SAMPLES;
        if settled {
            self.current = (self.current * 9800 + raw * 200) / 10000;
        }

        let magnitude = self.current.abs();
        if magnitude > self.peak {
            self.peak = magnitude;
        }

        // consecutive samples: a short spike in a long move no longer adds up
        if magnitude > Self::SAFETY_LIMIT {
            self.over_count = self.over_count.saturating_add(1);
        } else {
            self.over_count = 0;
        }

        // inrush limit on the raw samples (the filter is still held at 0)
        let mut inrush_over = false;
        if !settled && self.inrush != InrushMode::Off {
            if raw.abs() > Self::INRUSH_LIMIT {
                self.inrush_over = self.inrush_over.saturating_add(1);
            } else {
                self.inrush_over = 0;
            }
            inrush_over = self.inrush_over > Self::INRUSH_CONSECUTIVE;
            if inrush_over {
                self.inrush_seen = true;
            }
        }

        if inrush_over && self.inrush == InrushMode::Enforce {
            if raw.abs() > self.peak {
                self.peak = raw.abs();
            }
            if self.trip == Trip::None {
                self.trip = Trip::Hard;
                self.trip_current = raw;
                self.inrush_trip = true;
            }
            return Trip::Hard;
        }
        let t = if magnitude > Self::HARD_LIMIT {
            Trip::Hard
        } else if self.over_count > Self::SAFETY_CONSECUTIVE {
            Trip::Safety
        } else if settled && (self.current > self.high || self.current < self.low) {
            Trip::Bound
        } else {
            Trip::None
        };

        if t != Trip::None && self.trip == Trip::None {
            self.trip = t;
            self.trip_current = self.current;
        }
        t
    }

    /// Motor not running: resets filter, inrush and safety counters (the
    /// per-move statistics stay readable until the next arm()).
    pub fn idle(&mut self) {
        self.current = 0;
        self.debounce = 0;
        self.over_count = 0;
        self.inrush_over = 0;
    }

    /// Filtered current (0 during the inrush time).
    pub fn current(&self) -> i32 {
        self.current
    }

    /// Largest |filtered current| since arm().
    pub fn peak(&self) -> i32 {
        self.peak
    }

    pub fn trip(&self) -> Trip {
        self.trip
    }

    /// Filtered current at the first trip since arm().
    pub fn trip_current(&self) -> i32 {
        self.trip_current
    }

    pub fn over_count(&self) -> u8 {
        self.over_count
    }

    /// the inrush limit was exceeded since arm() (Report and Enforce)
    pub fn inrush_seen(&self) -> bool {
        self.inrush_seen
    }

    /// the first trip since arm() was the inrush limit (Enforce)
    pub fn inrush_trip(&self) -> bool {
        self.inrush_trip
    }
}
