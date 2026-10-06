//! Calibration decisions: end-stop thresholds, breakaway escalation, the
//! verdict on the counts of a calibration pass and mean current learning.
//! Hardware-free. Currents: mean currents in mA, bounds in 0.1 mA.

use crate::move_classifier::DIR_CLOSE;

/// Floor for the mean current an end-stop threshold is derived from.
pub const MEAN_CURRENT_FLOOR_MA: u16 = 15;
/// Mean current assumed for a valve that was never calibrated.
pub const MEAN_CURRENT_DEFAULT_MA: u16 = 20;
/// Floor for the closing strokes of a calibration: 1.x closed with a fixed
/// 20 mA x factor, and a valve that closes against the water pressure needs at
/// least that, also when its learned mean current is lower.
pub const CALIBRATION_CLOSE_FLOOR_MA: u16 = MEAN_CURRENT_DEFAULT_MA;
/// A stroke must have at least this many mean current samples (one every
/// ~0.5 s) before its mean is learned.
pub const MIN_MEAN_SAMPLES: u16 = 4;
/// A stroke shorter than this is never accepted: scaler = counts / 100 must not be 0.
pub const MIN_TRAVEL_COUNTS: u32 = 100;
/// Absolute end-stop limit of the motor current detection (the safety limit).
pub const SAFETY_LIMIT_MA: u8 = 60;

/// End-stop threshold in 0.1 mA for a mean current and a factor in tenths:
/// max(mean, floor) * factor. (C++ default for floor_ma: MEAN_CURRENT_FLOOR_MA.)
pub fn end_stop_bound(mean_current_ma: u16, factor: u8, floor_ma: u16) -> i32 {
    i32::from(mean_current_ma.max(floor_ma)) * i32::from(factor)
}

/// Mean current floor of a calibration stroke: closing strokes use
/// CALIBRATION_CLOSE_FLOOR_MA, opening strokes MEAN_CURRENT_FLOOR_MA.
pub fn calibration_floor(dir: u8) -> u16 {
    if dir == DIR_CLOSE {
        CALIBRATION_CLOSE_FLOOR_MA
    } else {
        MEAN_CURRENT_FLOOR_MA
    }
}

/// Breakaway escalation: on calibration repetition n (n >= 1) the threshold
/// grows by n * step_pct percent, but never beyond max_ma (which itself is capped
/// by the 60 mA safety limit). A threshold that already is above the cap is
/// left as it is.
/// `Default` is the zero value of the C++ `EscalationConfig{}`, not ESCALATION_DEFAULT.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EscalationConfig {
    /// 0 / 1
    pub enable: u8,
    /// 0..100
    pub step_pct: u8,
    /// 20..60
    pub max_ma: u8,
}

pub const ESCALATION_STEP_MAX: u8 = 100;
pub const ESCALATION_MAX_MA_MIN: u8 = 20;
pub const ESCALATION_MAX_MA_MAX: u8 = SAFETY_LIMIT_MA;
pub const ESCALATION_DEFAULT: EscalationConfig = EscalationConfig {
    enable: 0,
    step_pct: 25,
    max_ma: 50,
};

pub fn escalation_valid(c: &EscalationConfig) -> bool {
    c.enable <= 1
        && c.step_pct <= ESCALATION_STEP_MAX
        && (ESCALATION_MAX_MA_MIN..=ESCALATION_MAX_MA_MAX).contains(&c.max_ma)
}

/// Out-of-range config (e.g. from an EEPROM image) -> default.
pub fn sanitize_escalation(c: &EscalationConfig) -> EscalationConfig {
    if escalation_valid(c) {
        *c
    } else {
        ESCALATION_DEFAULT
    }
}

pub fn escalated_bound(bound: i32, repetition: u8, c: &EscalationConfig) -> i32 {
    if c.enable == 0 || repetition == 0 || c.step_pct == 0 || bound <= 0 {
        return bound;
    }

    let cap = i32::from(c.max_ma.min(SAFETY_LIMIT_MA)) * 10;
    if bound >= cap {
        return bound;
    }

    let grown = i64::from(bound) * (100 + i64::from(c.step_pct) * i64::from(repetition)) / 100;
    // below the cap it fits an i32
    grown.min(i64::from(cap)) as i32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum CalibrationVerdict {
    /// store counts, scaler and mean current
    Accept = 0,
    /// repeat the calibration, keep the previous values
    Retry = 1,
    /// repetitions exhausted: BLOCKS, keep the previous values
    Blocked = 2,
}

/// failed_before: failed passes of this calibration before this one.
/// A pass fails if either stroke has fewer than max(min_counts, MIN_TRAVEL_COUNTS)
/// pulses; after max_retries repetitions the valve is blocked.
pub fn evaluate_calibration(
    opening_count: u32,
    closing_count: u32,
    min_counts: u16,
    failed_before: u8,
    max_retries: u8,
) -> CalibrationVerdict {
    let required = u32::from(min_counts).max(MIN_TRAVEL_COUNTS);
    if opening_count >= required && closing_count >= required {
        return CalibrationVerdict::Accept;
    }
    // failed_before + 1 failed passes now; blocked once they exceed the repetitions
    if failed_before >= max_retries {
        CalibrationVerdict::Blocked
    } else {
        CalibrationVerdict::Retry
    }
}

/// Mean current of one stroke: average of `samples` values in 0.1 mA whose sum
/// is `sum`, in mA. 0 without samples.
pub fn stroke_mean_current(sum: i32, samples: u16) -> u16 {
    if samples == 0 {
        return 0;
    }
    let mean = i64::from(sum).abs() / i64::from(samples) / 10;
    u16::try_from(mean).unwrap_or(u16::MAX)
}

/// Mean current after a successful pass: strokes with fewer than
/// MIN_MEAN_SAMPLES samples or a mean of 0 are ignored; the remaining are
/// averaged; without any the previous value stays.
pub fn learn_mean_current(
    previous_ma: u16,
    open_mean_ma: u16,
    open_samples: u16,
    close_mean_ma: u16,
    close_samples: u16,
) -> u16 {
    let open_valid = open_samples >= MIN_MEAN_SAMPLES && open_mean_ma > 0;
    let close_valid = close_samples >= MIN_MEAN_SAMPLES && close_mean_ma > 0;

    match (open_valid, close_valid) {
        // the mean of two u16 fits a u16
        (true, true) => ((u32::from(open_mean_ma) + u32::from(close_mean_ma)) / 2) as u16,
        (true, false) => open_mean_ma,
        (false, true) => close_mean_ma,
        (false, false) => previous_ma,
    }
}
