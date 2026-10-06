//! Blocks B "safety" and C "calibration" of the configuration EEPROM (address
//! map in eeprom_layout). Hardware-free. Multi-byte fields little endian,
//! unused bytes 0xFF, CRC-8 (Dallas) over the bytes before it.
//!
//! Block B (0x0160), version 1, payload length 22:
//!   \[0\] version  \[1\] payload length n  \[2..13\] failsafePct\[0..11\]
//!   \[14..21\] shadow of the 1.x motor fields: lowFac, highFac, movements (2),
//!            startOnPower, minCounts (2), maxRetries
//!   \[22..23\] leaseTimeoutMin (copy of block A)   \[2+n\] CRC-8 over \[0 .. 1+n\]
//! The shadow replaces the 1.x fields when the 1.x layout fails its CRC.
//!
//! Block C of valve v (0x0180 + 16 v), version 1, payload length 9:
//!   \[0\] version  \[1\] payload length n  \[2..3\] openingCount  \[4..5\] closingCount
//!   \[6..7\] meanCurrent (mA)  \[8\] flags  \[9\] v  \[10\] 0  \[2+n\] CRC-8 over \[0 .. 1+n\]
//!
//! A later version only appends payload bytes; the known prefix is read.

use crate::calibration::{MEAN_CURRENT_DEFAULT_MA, MIN_TRAVEL_COUNTS};
use crate::eeprom_layout::{CALIB_BLOCK_SIZE, SAFETY_BLOCK_SIZE};
use crate::failsafe::{sanitize_failsafe_pct, FAILSAFE_DEFAULT_PCT};
use crate::lease::{lease_timeout_valid, LEASE_TIMEOUT_DEFAULT_MIN};
use crate::legacy_layout::VALVE_COUNT;
use crate::motor_params::MOTOR_PARAMS_DEFAULT;
use crate::onewire_check::crc8;
use crate::settings::LEARN_MOVEMENTS_DEFAULT;

const VALVES: usize = VALVE_COUNT as usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum BlockState {
    /// never written (0xFF)
    Absent = 0,
    Valid = 1,
    /// damaged: wrong version, length, CRC or content
    Corrupt = 2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MotorShadow {
    pub low_fac: u8,
    pub high_fac: u8,
    pub movements: u16,
    pub start_on_power: u8,
    pub min_counts: u16,
    pub max_retries: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SafetyBlock {
    /// 0..100, FAILSAFE_HOLD
    pub failsafe_pct: [u8; VALVES],
    pub shadow: MotorShadow,
    /// copy of the timeout of block A
    pub lease_timeout_min: u16,
    /// decode: the copy is a valid timeout (encode ignores it)
    pub lease_valid: bool,
}

pub const SAFETY_VERSION: u8 = 1;
pub const SAFETY_PAYLOAD: usize = 22;

pub const CALIB_VALID: u8 = 0x01; // counts of a successful calibration
pub const CALIB_FAILED: u8 = 0x02; // the last calibration of the valve ended blocked

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CalibRecord {
    pub opening_count: u16,
    pub closing_count: u16,
    /// mA
    pub mean_current: u16,
    /// CALIB_VALID, CALIB_FAILED; 0 = no record
    pub flags: u8,
}

pub const CALIB_VERSION: u8 = 1;
pub const CALIB_PAYLOAD: usize = 9;

fn get_u16(raw: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([
        raw.get(at).copied().unwrap_or(0xFF),
        raw.get(at + 1).copied().unwrap_or(0xFF),
    ])
}

fn put_u16(out: &mut [u8], at: usize, v: u16) {
    if let Some(dst) = out.get_mut(at..at + 2) {
        dst.copy_from_slice(&v.to_le_bytes());
    }
}

/// Valid if the block holds at least `payload` bytes (any version from 1 on)
/// and its CRC matches
fn check_block(raw: &[u8], payload: usize) -> BlockState {
    let (Some(&version), Some(&length)) = (raw.first(), raw.get(1)) else {
        return BlockState::Corrupt;
    };
    if version == 0xFF {
        return BlockState::Absent;
    }
    let length = usize::from(length);
    if version == 0 || length < payload {
        return BlockState::Corrupt;
    }
    // a payload whose CRC lies behind the block (C++: length > size - 3) is Corrupt
    let (Some(covered), Some(&crc)) = (raw.get(..2 + length), raw.get(2 + length)) else {
        return BlockState::Corrupt;
    };
    if crc8(covered) != crc {
        return BlockState::Corrupt;
    }
    BlockState::Valid
}

fn safety_defaults(out: &mut SafetyBlock) {
    out.failsafe_pct = [FAILSAFE_DEFAULT_PCT; VALVES];
    out.shadow = MotorShadow {
        low_fac: MOTOR_PARAMS_DEFAULT.low_fac,
        high_fac: MOTOR_PARAMS_DEFAULT.high_fac,
        movements: LEARN_MOVEMENTS_DEFAULT,
        start_on_power: MOTOR_PARAMS_DEFAULT.start_on_power,
        min_counts: MOTOR_PARAMS_DEFAULT.min_counts,
        max_retries: MOTOR_PARAMS_DEFAULT.max_retries,
    };
    out.lease_timeout_min = LEASE_TIMEOUT_DEFAULT_MIN;
    out.lease_valid = false;
}

/// `out` is always written: failsafe positions 50, the default motor fields and
/// no lease copy unless the state is Valid. An invalid failsafe position loads
/// 50, an invalid lease copy counts as missing.
pub fn decode_safety(raw: &[u8; SAFETY_BLOCK_SIZE], out: &mut SafetyBlock) -> BlockState {
    safety_defaults(out);
    let state = check_block(raw, SAFETY_PAYLOAD);
    if state != BlockState::Valid {
        return state;
    }

    // the 12 failsafe positions from byte 2 on (the zip ends with them)
    for (pct, &stored) in out.failsafe_pct.iter_mut().zip(&raw[2..]) {
        *pct = sanitize_failsafe_pct(stored);
    }
    out.shadow.low_fac = raw[14];
    out.shadow.high_fac = raw[15];
    out.shadow.movements = get_u16(raw, 16);
    out.shadow.start_on_power = raw[18];
    out.shadow.min_counts = get_u16(raw, 19);
    out.shadow.max_retries = raw[21];
    let lease = get_u16(raw, 22);
    if lease_timeout_valid(u32::from(lease)) {
        out.lease_timeout_min = lease;
        out.lease_valid = true;
    }
    BlockState::Valid
}

/// Returns the number of bytes to write (25).
pub fn encode_safety(input: &SafetyBlock, out: &mut [u8; SAFETY_BLOCK_SIZE]) -> usize {
    out.fill(0xFF);
    out[0] = SAFETY_VERSION;
    out[1] = SAFETY_PAYLOAD as u8;
    out[2..2 + VALVES].copy_from_slice(&input.failsafe_pct);
    out[14] = input.shadow.low_fac;
    out[15] = input.shadow.high_fac;
    put_u16(out, 16, input.shadow.movements);
    out[18] = input.shadow.start_on_power;
    put_u16(out, 19, input.shadow.min_counts);
    out[21] = input.shadow.max_retries;
    put_u16(out, 22, input.lease_timeout_min);
    out[2 + SAFETY_PAYLOAD] = crc8(&out[..2 + SAFETY_PAYLOAD]);
    3 + SAFETY_PAYLOAD
}

/// `out` is always written: all zero (no record) unless the state is Valid. A
/// record whose counts are marked valid but shorter than MIN_TRAVEL_COUNTS is
/// Corrupt; a mean current of 0 or above 1000 mA loads MEAN_CURRENT_DEFAULT_MA.
pub fn decode_calib(raw: &[u8; CALIB_BLOCK_SIZE], valve: u8, out: &mut CalibRecord) -> BlockState {
    *out = CalibRecord::default();
    let state = check_block(raw, CALIB_PAYLOAD);
    if state != BlockState::Valid {
        return state;
    }
    if raw[9] != valve {
        return BlockState::Corrupt;
    }

    let mut r = CalibRecord {
        opening_count: get_u16(raw, 2),
        closing_count: get_u16(raw, 4),
        mean_current: get_u16(raw, 6),
        flags: raw[8],
    };
    let min = MIN_TRAVEL_COUNTS as u16;
    if r.flags & CALIB_VALID != 0 && (r.opening_count < min || r.closing_count < min) {
        return BlockState::Corrupt;
    }
    if r.mean_current == 0 || r.mean_current > 1000 {
        r.mean_current = MEAN_CURRENT_DEFAULT_MA;
    }
    *out = r;
    BlockState::Valid
}

/// Returns the number of bytes to write (12).
pub fn encode_calib(input: &CalibRecord, valve: u8, out: &mut [u8; CALIB_BLOCK_SIZE]) -> usize {
    out.fill(0xFF);
    out[0] = CALIB_VERSION;
    out[1] = CALIB_PAYLOAD as u8;
    put_u16(out, 2, input.opening_count);
    put_u16(out, 4, input.closing_count);
    put_u16(out, 6, input.mean_current);
    out[8] = input.flags;
    out[9] = valve;
    out[10] = 0;
    out[2 + CALIB_PAYLOAD] = crc8(&out[..2 + CALIB_PAYLOAD]);
    3 + CALIB_PAYLOAD
}

#[cfg(test)]
mod tests;
