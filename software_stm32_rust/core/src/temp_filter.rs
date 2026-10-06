//! Temperatures of the DS18B20 sensors as the firmware reports them (goned, gvlvd). Hardware-free:
//! the glue reads the raw register value (1/128 degC, DallasTemperature::getTemp(), retried once
//! when the read failed) and reports what filter_temperature() makes of it:
//! - a good read in 0.1 degC, rounded like round(getTempC() * 10) of firmware 2.0.0;
//! - a failed read keeps the last good value for TEMP_HOLD_CYCLES cycles, then
//!   TEMP_FAILED_TENTHS;
//! - 85.0 degC is the power-on value of the DS18B20 register (a sensor that lost its supply during
//!   the conversion): it counts as a read only after a reading of at least 75.0 degC.

/// DallasTemperature DEVICE_DISCONNECTED_RAW
pub const TEMP_RAW_DISCONNECTED: i16 = -7040;
/// 85.0 degC in 1/128 degC
pub const TEMP_RAW_85C: i16 = 10880;
/// 0.1 degC
pub const TEMP_85_MIN_PREVIOUS: i32 = 750;
pub const TEMP_HOLD_CYCLES: u8 = 2;
/// a failed read (round(-127.0 * 10))
pub const TEMP_FAILED_TENTHS: i32 = -1270;

/// 1/128 degC -> 0.1 degC, rounded half away from zero
pub fn raw_to_tenths(raw128: i16) -> i32 {
    // raw128 * 10 / 128 = raw128 * 5 / 64
    let x = i32::from(raw128) * 5;
    if x >= 0 {
        (x + 32) / 64
    } else {
        -((32 - x) / 64)
    }
}

/// one sensor; a new sensor at the index starts with a new track
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TempTrack {
    /// 0.1 degC, the last good read
    pub last: i32,
    pub have_last: bool,
    /// failed reads since the last good one
    pub missed: u8,
    /// failed reads, saturating
    pub errors: u16,
}

/// the value to report for raw128 (after the retry of a failed read)
pub fn filter_temperature(t: &mut TempTrack, raw128: i16) -> i32 {
    let good = raw128 > TEMP_RAW_DISCONNECTED
        && (raw128 != TEMP_RAW_85C || t.last >= TEMP_85_MIN_PREVIOUS);
    if good {
        t.last = raw_to_tenths(raw128);
        t.have_last = true;
        t.missed = 0;
        return t.last;
    }
    t.errors = t.errors.saturating_add(1);
    if t.have_last && t.missed < TEMP_HOLD_CYCLES {
        t.missed += 1;
        return t.last;
    }
    TEMP_FAILED_TENTHS
}
