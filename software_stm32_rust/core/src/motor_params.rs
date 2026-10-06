//! Motor parameters (smotc/gmotc), their one validated range table and the
//! loader used for the values stored in the EEPROM. Hardware-free.

use crate::calibration::MEAN_CURRENT_FLOOR_MA;
use crate::end_stop_detector::EndStopDetector;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamRange {
    pub min: u16,
    pub max: u16,
    /// used when a stored value is out of range
    pub def: u16,
}

impl ParamRange {
    pub const fn contains(&self, v: u32) -> bool {
        v >= self.min as u32 && v <= self.max as u32
    }
}

// One table for smotc, the EEPROM load at start-up and gmotx.
//  - end-stop factors are tenths (17 = 1.7 x mean current); 10..40 is what 1.x
//    kept across a restart. A factor below 10 puts the threshold under the
//    running current, so every move would stop at once; 1.x smotc took 5..50
//    but loaded only 10..40 at start-up, and an EEPROM written by it may hold
//    such a value: it loads the default like any other out-of-range value.
//  - start_on_power is the position in % assumed and targeted after a start
//  - min_counts is the minimum number of pulses of a calibration stroke
//  - max_retries is the number of calibration repetitions before BLOCKS
pub const LOW_FAC_RANGE: ParamRange = ParamRange {
    min: 10,
    max: 40,
    def: 17,
};
pub const HIGH_FAC_RANGE: ParamRange = ParamRange {
    min: 10,
    max: 40,
    def: 17,
};
pub const START_ON_POWER_RANGE: ParamRange = ParamRange {
    min: 0,
    max: 100,
    def: 30,
};
pub const MIN_COUNTS_RANGE: ParamRange = ParamRange {
    min: 0,
    max: 60000,
    def: 3000,
};
pub const MAX_RETRIES_RANGE: ParamRange = ParamRange {
    min: 0,
    max: 2,
    def: 2,
};

/// smotc (not the EEPROM load) also takes an end-stop factor above the table
/// maximum up to this value and applies it as the maximum: 1.x smotc and the
/// legacy web page accept up to 50 (5.0), and the legacy ESP ignores
/// `smotc err`, so refusing it would leave the valve at the old factor while the
/// web page shows the new one. Nothing is lost: with the 15 mA floor a factor of
/// 40 already puts every threshold at or above the 60 mA safety limit, which
/// stops the motor at the same current (asserted below).
pub const FAC_REQUEST_MAX: u16 = 50;

// a factor above the table maximum cannot change where a move stops (FAC_REQUEST_MAX)
const _: () = assert!(
    MEAN_CURRENT_FLOOR_MA as i32 * LOW_FAC_RANGE.max as i32 >= EndStopDetector::SAFETY_LIMIT
        && MEAN_CURRENT_FLOOR_MA as i32 * HIGH_FAC_RANGE.max as i32
            >= EndStopDetector::SAFETY_LIMIT,
    "the largest end-stop factor must reach the safety limit"
);
const _: () = assert!(
    FAC_REQUEST_MAX >= LOW_FAC_RANGE.max
        && FAC_REQUEST_MAX >= HIGH_FAC_RANGE.max
        && FAC_REQUEST_MAX <= 0xFF,
    "factor request limit"
);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MotorParams {
    pub low_fac: u8,
    pub high_fac: u8,
    pub start_on_power: u8,
    pub min_counts: u16,
    pub max_retries: u8,
}

pub const MOTOR_PARAMS_DEFAULT: MotorParams = MotorParams {
    low_fac: LOW_FAC_RANGE.def as u8,
    high_fac: HIGH_FAC_RANGE.def as u8,
    start_on_power: START_ON_POWER_RANGE.def as u8,
    min_counts: MIN_COUNTS_RANGE.def,
    max_retries: MAX_RETRIES_RANGE.def as u8,
};

// a table value fits its field: ranges of u8 fields end below 256
fn pick_u8(r: &ParamRange, v: u8) -> u8 {
    if r.contains(u32::from(v)) {
        v
    } else {
        r.def as u8
    }
}

fn pick_u16(r: &ParamRange, v: u16) -> u16 {
    if r.contains(u32::from(v)) {
        v
    } else {
        r.def
    }
}

/// stores v in field if it is inside r
fn take_u8(r: &ParamRange, v: u32, field: &mut u8) -> bool {
    if !r.contains(v) {
        return false;
    }
    *field = v as u8;
    true
}

fn take_u16(r: &ParamRange, v: u32, field: &mut u16) -> bool {
    if !r.contains(v) {
        return false;
    }
    *field = v as u16;
    true
}

/// an end-stop factor: like take_u8(), a value in (max, FAC_REQUEST_MAX] is stored as max
fn take_factor(r: &ParamRange, v: u32, field: &mut u8) -> bool {
    let max = u32::from(r.max);
    let v = if v > max && v <= u32::from(FAC_REQUEST_MAX) {
        max
    } else {
        v
    };
    take_u8(r, v, field)
}

/// True if every field is inside its range.
pub fn motor_params_valid(p: &MotorParams) -> bool {
    LOW_FAC_RANGE.contains(u32::from(p.low_fac))
        && HIGH_FAC_RANGE.contains(u32::from(p.high_fac))
        && START_ON_POWER_RANGE.contains(u32::from(p.start_on_power))
        && MIN_COUNTS_RANGE.contains(u32::from(p.min_counts))
        && MAX_RETRIES_RANGE.contains(u32::from(p.max_retries))
}

pub fn same_motor_params(a: &MotorParams, b: &MotorParams) -> bool {
    a.low_fac == b.low_fac
        && a.high_fac == b.high_fac
        && a.start_on_power == b.start_on_power
        && a.min_counts == b.min_counts
        && a.max_retries == b.max_retries
}

/// Replaces every out-of-range field by its default (EEPROM load).
pub fn sanitize_motor_params(p: &MotorParams) -> MotorParams {
    MotorParams {
        low_fac: pick_u8(&LOW_FAC_RANGE, p.low_fac),
        high_fac: pick_u8(&HIGH_FAC_RANGE, p.high_fac),
        start_on_power: pick_u8(&START_ON_POWER_RANGE, p.start_on_power),
        min_counts: pick_u16(&MIN_COUNTS_RANGE, p.min_counts),
        max_retries: pick_u8(&MAX_RETRIES_RANGE, p.max_retries),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ParamsRequest {
    /// every supplied value was in range and is applied
    Applied = 0,
    /// at least one value was out of range: its field is unchanged, the others are applied
    Partial = 1,
    /// argc outside 3..5: nothing is applied
    Rejected = 2,
}

/// A parsed smotc request: the first three values are mandatory, min_counts and
/// max_retries are optional (argc 3..5) and keep their current value if absent.
/// Each supplied value is checked against its own range; an end-stop factor in
/// (max, FAC_REQUEST_MAX] is applied as max and counts as in range. A value out of range
/// never reaches `in_out`, but it does not drop the valid values sent with it:
/// the legacy ESP always sends all five values and ignores `smotc err`.
pub fn apply_motor_params_request(
    in_out: &mut MotorParams,
    argc: u8,
    values: &[u32; 5],
) -> ParamsRequest {
    if !(3..=5).contains(&argc) {
        return ParamsRequest::Rejected;
    }

    let mut all = take_factor(&LOW_FAC_RANGE, values[0], &mut in_out.low_fac);
    all = take_factor(&HIGH_FAC_RANGE, values[1], &mut in_out.high_fac) && all;
    all = take_u8(&START_ON_POWER_RANGE, values[2], &mut in_out.start_on_power) && all;
    if argc >= 4 {
        all = take_u16(&MIN_COUNTS_RANGE, values[3], &mut in_out.min_counts) && all;
    }
    if argc == 5 {
        all = take_u8(&MAX_RETRIES_RANGE, values[4], &mut in_out.max_retries) && all;
    }
    if all {
        ParamsRequest::Applied
    } else {
        ParamsRequest::Partial
    }
}

#[cfg(test)]
mod tests;
