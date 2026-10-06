//! Classification of a finished motor move (stop reason, early end stop) and
//! the per-move record reported by gvlvx. Hardware-free.

/// Wire values of gvlvx lastStop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum StopReason {
    /// no move since start-up
    None = 0,
    /// requested pulse count reached
    Target = 1,
    /// current above the end-stop threshold
    EndStop = 2,
    /// end stop after less than half of the expected travel
    EarlyEndStop = 3,
    /// motor ran too long without an end stop
    Timeout = 4,
    /// no motor current (open circuit)
    Undercurrent = 5,
    /// safety or hard current limit
    SafetyOvercurrent = 6,
    /// move could not be started / was cancelled
    Aborted = 7,
}

/// What the motor layer saw when the move ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum MotorStop {
    None = 0,
    CountReached = 1,
    EndStop = 2,
    SafetyOvercurrent = 3,
    Undercurrent = 4,
    Timeout = 5,
    Aborted = 6,
}

// MoveDirection
pub const DIR_OPEN: u8 = 0;
pub const DIR_CLOSE: u8 = 1;

/// Pulse count meaning "run until an end stop".
pub const RUN_TO_END_STOP: u16 = 0xFFFF;

/// A move to an end stop is checked for an early end stop only when it was
/// expected to cover at least this much of the full stroke. On a shorter move
/// half of the expected travel is within the error of the believed position
/// (scaler rounding, drift over many partial moves) and of the 250 ms inrush
/// time in which no end stop is detected, so a correct stop would count as early.
pub const EARLY_CHECK_MIN_TRAVEL_PCT: u8 = 50;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MoveRequest {
    /// DIR_OPEN / DIR_CLOSE
    pub dir: u8,
    /// RUN_TO_END_STOP for a move to an end stop
    pub requested_counts: u16,
    /// For a move to an end stop: expected travel in % of the full stroke
    /// (100 = full travel); below EARLY_CHECK_MIN_TRAVEL_PCT (0 included) the
    /// early end stop check is off.
    pub expected_travel_pct: u8,
    /// Full stroke learned by the last successful calibration, 0 = unknown.
    pub learned_travel: u32,
    /// A partial move (requested_counts != RUN_TO_END_STOP) is checked for an
    /// early end stop: true only for normal partial moves (not for service moves,
    /// calibration strokes and the moves of a failed or blocked valve). C++ default false.
    pub partial_early_check: bool,
}

/// A partial move that ends at an end stop before this share of the requested
/// pulses stopped early.
pub const PARTIAL_EARLY_PCT: u8 = 80;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MoveResult {
    pub dir: u8,
    pub requested_counts: u16,
    pub counted_counts: u16,
    /// StopReason
    pub stop_reason: u8,
    /// 0.1 mA, largest filtered |current|
    pub peak_current: u16,
    pub duration_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveClassification {
    pub reason: StopReason,
    /// end stop (threshold or safety) before half of the expected travel
    pub early: bool,
}

fn ended_early(req: &MoveRequest, counted: u32) -> bool {
    if req.requested_counts != RUN_TO_END_STOP {
        return req.partial_early_check
            && u64::from(counted) * 100
                < u64::from(req.requested_counts) * u64::from(PARTIAL_EARLY_PCT);
    }
    if req.learned_travel == 0 {
        return false;
    }
    if req.expected_travel_pct < EARLY_CHECK_MIN_TRAVEL_PCT {
        return false;
    }
    let pct = req.expected_travel_pct.min(100);
    let expected = u64::from(req.learned_travel) * u64::from(pct) / 100;
    u64::from(counted) * 2 < expected
}

fn saturate16(v: u64) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}

/// Early: a move to an end stop with expected_travel_pct >= EARLY_CHECK_MIN_TRAVEL_PCT
/// that ended at an end stop (threshold or safety limit) after less than 50 % of
/// learned_travel * expected_travel_pct / 100; a partial move with
/// partial_early_check that ended at an end stop before PARTIAL_EARLY_PCT % of the
/// requested pulses. A safety stop keeps reason SafetyOvercurrent but may still
/// be early.
pub fn classify_move(req: &MoveRequest, stop: MotorStop, counted: u32) -> MoveClassification {
    match stop {
        MotorStop::CountReached => MoveClassification {
            reason: StopReason::Target,
            early: false,
        },
        MotorStop::EndStop => {
            let early = ended_early(req, counted);
            MoveClassification {
                reason: if early {
                    StopReason::EarlyEndStop
                } else {
                    StopReason::EndStop
                },
                early,
            }
        }
        MotorStop::SafetyOvercurrent => MoveClassification {
            reason: StopReason::SafetyOvercurrent,
            early: ended_early(req, counted),
        },
        MotorStop::Undercurrent => MoveClassification {
            reason: StopReason::Undercurrent,
            early: false,
        },
        MotorStop::Timeout => MoveClassification {
            reason: StopReason::Timeout,
            early: false,
        },
        MotorStop::Aborted | MotorStop::None => MoveClassification {
            reason: StopReason::Aborted,
            early: false,
        },
    }
}

pub fn make_move_result(
    req: &MoveRequest,
    reason: StopReason,
    counted: u32,
    peak: i32,
    duration_ms: u32,
) -> MoveResult {
    MoveResult {
        dir: if req.dir == DIR_CLOSE {
            DIR_CLOSE
        } else {
            DIR_OPEN
        },
        requested_counts: req.requested_counts,
        counted_counts: saturate16(u64::from(counted)),
        stop_reason: reason as u8,
        peak_current: if peak <= 0 {
            0
        } else {
            saturate16(u64::from(peak.unsigned_abs()))
        },
        duration_ms,
    }
}

/// Position after a move that ended at an end stop: 100 / 0 for a move to the
/// end stop (RUN_TO_END_STOP); for a partial move the start moved by the counted
/// pulses (counted / scaler %, the start when the scaler is 0), clamped to 0..100.
pub fn position_after_end_stop(
    start: u8,
    dir: u8,
    requested_counts: u16,
    counted: u32,
    scaler: u32,
) -> u8 {
    if requested_counts == RUN_TO_END_STOP {
        return if dir == DIR_CLOSE { 0 } else { 100 };
    }
    let pct = counted.checked_div(scaler).unwrap_or(0);
    let delta = pct.min(100);
    let start = u32::from(start);
    // both results are at most 100 (start is a u8)
    if dir == DIR_CLOSE {
        return start.saturating_sub(delta) as u8;
    }
    (start + delta).min(100) as u8
}

/// Early partial stops in a row of one valve: the second one requests a calibration.
#[derive(Clone, Copy, Debug, Default)]
pub struct EarlyStopRun {
    run: u8,
}

impl EarlyStopRun {
    /// partial_early: this move stopped early; true when it is the second in a row
    /// (the run starts again); any other move ends the run
    pub fn on_move(&mut self, partial_early: bool) -> bool {
        if !partial_early {
            self.run = 0;
            return false;
        }
        self.run += 1;
        if self.run < 2 {
            return false;
        }
        self.run = 0;
        true
    }

    pub fn reset(&mut self) {
        self.run = 0;
    }

    pub fn count(&self) -> u8 {
        self.run
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
#[cfg(test)]
mod tests_partial;
