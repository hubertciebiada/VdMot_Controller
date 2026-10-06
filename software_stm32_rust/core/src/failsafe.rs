//! Failsafe positions: where a valve goes when the lease of its target expired
//! (lease) or when it is blocked. Set by sfspo and stored in the EEPROM.
//! Hardware-free.

use crate::valve_codes::{ST_BLOCKED, ST_FAILED, ST_OPEN_CIRCUIT};

/// the valve keeps its target
pub const FAILSAFE_HOLD: u8 = 255;
pub const FAILSAFE_DEFAULT_PCT: u8 = 50;

/// 0..100 % or FAILSAFE_HOLD
pub fn failsafe_pct_valid(v: u32) -> bool {
    v <= 100 || v == u32::from(FAILSAFE_HOLD)
}

/// Stored value at start-up: anything sfspo accepts is kept, anything else
/// loads FAILSAFE_DEFAULT_PCT.
pub fn sanitize_failsafe_pct(v: u8) -> u8 {
    if failsafe_pct_valid(u32::from(v)) {
        v
    } else {
        FAILSAFE_DEFAULT_PCT
    }
}

/// gvlvy field 23 and gstax failsafeMask: where a valve is driven to and why
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum DriveSource {
    Target = 0,
    LeaseFailsafe = 1,
    BlockedFailsafe = 2,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drive {
    pub position: u8,
    pub source: DriveSource,
}

/// Hold (255) -> target; blocked (9) -> failsafe; failed (4) or open circuit (6)
/// -> target (not driven); lease expired and no assembly hold -> failsafe;
/// otherwise the target.
pub fn drive_target(
    target: u8,
    failsafe_pct: u8,
    status: u8,
    lease_expired: bool,
    assembly_hold: bool,
) -> Drive {
    let (position, source) = if failsafe_pct == FAILSAFE_HOLD {
        (target, DriveSource::Target)
    } else if status == ST_BLOCKED {
        (failsafe_pct, DriveSource::BlockedFailsafe)
    } else if status == ST_FAILED || status == ST_OPEN_CIRCUIT {
        (target, DriveSource::Target)
    } else if lease_expired && !assembly_hold {
        (failsafe_pct, DriveSource::LeaseFailsafe)
    } else {
        (target, DriveSource::Target)
    };
    Drive { position, source }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_drive;
