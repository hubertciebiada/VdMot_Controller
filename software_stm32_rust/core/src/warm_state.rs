//! Valve positions, lease and retry schedule kept across a warm reset (pin,
//! software, watchdog) in RAM the start-up code does not clear (.noinit).
//! Layout of contracts.md section 4.1. Hardware-free.

use crate::legacy_layout::{crc16_ccitt, VALVE_COUNT};
use crate::system_stats::BootReason;
use crate::valve_codes::{ST_BLOCKED, ST_CLOSING, ST_IDLE, ST_OPENING, ST_UNKNOWN};

const VALVES: usize = VALVE_COUNT as usize;

/// "VDWS"
pub const WARM_STATE_MAGIC: u32 = 0x5644_5753;
pub const WARM_STATE_VERSION: u8 = 1;

// WarmValve.flags

/// actual is where the valve stands (no move handed over)
pub const WARM_POS_VALID: u8 = 0x01;
pub const WARM_ASSEMBLY_HOLD: u8 = 0x02;
pub const WARM_NEEDS_REFERENCE: u8 = 0x04;
pub const WARM_RECAL: u8 = 0x08;
pub const WARM_FLAGS_UNUSED: u8 = 0xF0;
pub const WARM_RETRY_MAX_S: u32 = 86400;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct WarmValve {
    pub actual: u8,
    /// stored target, not the drive target
    pub target: u8,
    pub status: u8,
    pub flags: u8,
    pub retry_attempts: u8,
    /// 0/1
    pub retry_scheduled: u8,
    pub pad: [u8; 2],
    pub retry_remaining_s: u32,
}

/// The C++ layout (`#[repr(C)]`: valves at 8, lease at 152, failsafe positions at 164, crc at
/// 176); the CRC covers the 176 bytes before `crc` in the little-endian byte order of the
/// STM32, so a C++ and a Rust firmware read each other's state after a warm reset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct WarmState {
    pub magic: u32,
    pub version: u8,
    pub count: u8,
    pub reserved: u16,
    pub valves: [WarmValve; VALVES],
    pub lease_since_renewal_s: u32,
    pub lease_since_client_s: u32,
    pub lease_client: u8,
    pub pad: u8,
    pub lease_timeout_min: u16,
    pub failsafe_pct: [u8; VALVES],
    /// CRC-16/CCITT-FALSE over all bytes before it
    pub crc: u16,
}

const _: () = assert!(core::mem::size_of::<WarmValve>() == 12, "WarmValve layout");
const _: () = assert!(
    core::mem::offset_of!(WarmState, valves) == 8,
    "WarmState layout"
);
const _: () = assert!(
    core::mem::offset_of!(WarmState, lease_since_renewal_s) == 152,
    "WarmState layout"
);
const _: () = assert!(
    core::mem::offset_of!(WarmState, failsafe_pct) == 164,
    "WarmState layout"
);
const _: () = assert!(
    core::mem::offset_of!(WarmState, crc) == 176,
    "WarmState layout"
);

/// offsetof(WarmState, crc)
const CRC_COVERED: usize = 176;

/// The bytes the CRC covers, as they lie in the RAM of the STM32.
fn warm_bytes(s: &WarmState) -> [u8; CRC_COVERED] {
    let mut b = [0u8; CRC_COVERED];
    b[0..4].copy_from_slice(&s.magic.to_le_bytes());
    b[4] = s.version;
    b[5] = s.count;
    b[6..8].copy_from_slice(&s.reserved.to_le_bytes());
    for (chunk, w) in b[8..152].as_chunks_mut::<12>().0.iter_mut().zip(&s.valves) {
        chunk[0] = w.actual;
        chunk[1] = w.target;
        chunk[2] = w.status;
        chunk[3] = w.flags;
        chunk[4] = w.retry_attempts;
        chunk[5] = w.retry_scheduled;
        chunk[6..8].copy_from_slice(&w.pad);
        chunk[8..12].copy_from_slice(&w.retry_remaining_s.to_le_bytes());
    }
    b[152..156].copy_from_slice(&s.lease_since_renewal_s.to_le_bytes());
    b[156..160].copy_from_slice(&s.lease_since_client_s.to_le_bytes());
    b[160] = s.lease_client;
    b[161] = s.pad;
    b[162..164].copy_from_slice(&s.lease_timeout_min.to_le_bytes());
    b[164..176].copy_from_slice(&s.failsafe_pct);
    b
}

fn warm_crc(s: &WarmState) -> u16 {
    crc16_ccitt(&warm_bytes(s))
}

fn warm_valve_valid(w: &WarmValve) -> bool {
    w.actual <= 100
        && w.target <= 100
        && (ST_IDLE..=ST_BLOCKED).contains(&w.status)
        && w.flags & WARM_FLAGS_UNUSED == 0
        && w.retry_scheduled <= 1
        && w.retry_remaining_s <= WARM_RETRY_MAX_S
}

/// magic, version, count, reserved and the CRC (last)
pub fn warm_state_seal(s: &mut WarmState) {
    s.magic = WARM_STATE_MAGIC;
    s.version = WARM_STATE_VERSION;
    s.count = VALVE_COUNT;
    s.reserved = 0;
    s.crc = warm_crc(s);
}

/// magic, version, count and CRC
pub fn warm_state_valid(s: &WarmState) -> bool {
    s.magic == WARM_STATE_MAGIC
        && s.version == WARM_STATE_VERSION
        && s.count == VALVE_COUNT
        && s.crc == warm_crc(s)
}

/// Pin, Software, IndependentWatchdog, WindowWatchdog, LowPower
pub fn is_warm_boot(r: BootReason) -> bool {
    matches!(
        r,
        BootReason::Pin
            | BootReason::Software
            | BootReason::IndependentWatchdog
            | BootReason::WindowWatchdog
            | BootReason::LowPower
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RestoredValve {
    /// false: this valve takes the cold path (presence test)
    pub valid: bool,
    pub status: u8,
    pub actual: u8,
    pub target: u8,
    pub needs_reference: bool,
    pub recal: bool,
    pub assembly_hold: bool,
}

/// Field checks: actual and target <= 100, status 1..9, flags bits 4..7 clear,
/// retry_scheduled <= 1, retry_remaining_s <= WARM_RETRY_MAX_S; any failure ->
/// !valid. Status: a valve that was moving (2, 3) or whose position is not
/// valid -> 1 with needs_reference when calibrated, else 5 (tested again);
/// 1, 4..9 kept. needs_reference, recal, assembly_hold from the flags.
pub fn restore_valve(w: &WarmValve, calibrated: bool) -> RestoredValve {
    if !warm_valve_valid(w) {
        return RestoredValve::default();
    }
    let mut r = RestoredValve {
        valid: true,
        status: w.status,
        actual: w.actual,
        target: w.target,
        needs_reference: w.flags & WARM_NEEDS_REFERENCE != 0,
        recal: w.flags & WARM_RECAL != 0,
        assembly_hold: w.flags & WARM_ASSEMBLY_HOLD != 0,
    };
    let moving = w.status == ST_OPENING || w.status == ST_CLOSING;
    if moving || w.flags & WARM_POS_VALID == 0 {
        r.status = if calibrated { ST_IDLE } else { ST_UNKNOWN };
        r.needs_reference = r.needs_reference || calibrated;
    }
    r
}

#[cfg(test)]
mod tests;
