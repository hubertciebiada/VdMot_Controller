//! EEPROM layout of the configuration, and block A. Hardware-free.
//!
//! 24LC64 (8192 bytes), multi-byte fields little endian:
//!   0x0007-0x013B  1.x layout (legacy_layout), byte-identical to firmware 1.x
//!   0x013C-0x014B  block A "settings" (below)
//!   0x014C-0x015F  reserved
//!   0x0160-0x017F  block B "safety" (config_blocks)
//!   0x0180-0x023F  blocks C0..C11 "calibration", valve v at 0x0180 + 16 v (config_blocks)
//! Firmware 1.x writes only the 1.x layout and never touches the bytes behind it
//! (0xFF on a new chip).
//!
//! Block A:
//!   \[0\] layout version (LAYOUT_VERSION)   \[1\] payload length n
//!   \[2 .. 2+n-1\] payload                  \[2+n\] CRC-8 (Dallas) over \[0 .. 2+n-1\]
//! Payload of version 2 (firmware 2.0.0): escalation enable, stepPct, maxmA.
//! Version 3 appends learnTimeS (4 bytes), leaseTimeoutMin (2) and layoutCrc
//! (2, the CRC-16 of the 1.x layout as last written). A newer layout only
//! appends payload bytes, so an older reader uses the prefix it knows: 2.0.0
//! takes the escalation of a version 3 block. A block that is missing (1.x
//! image, erased chip) or damaged (torn write) loads with defaults.

use crate::calibration::{sanitize_escalation, EscalationConfig, ESCALATION_DEFAULT};
use crate::lease::{sanitize_lease_timeout, LEASE_TIMEOUT_DEFAULT_MIN};
use crate::legacy_layout::{LEGACY_IMAGE_SIZE, VALVE_COUNT};
use crate::onewire_check::crc8;
use crate::settings::LEARN_TIME_DEFAULT_S;

pub const LEGACY_LAYOUT_ADDRESS: u16 = 0x0007;
pub const LEGACY_LAYOUT_SIZE: usize = LEGACY_IMAGE_SIZE;
pub const EXTENSION_ADDRESS: u16 = 0x013C;
pub const EXTENSION_BLOCK_SIZE: usize = 16;
pub const SAFETY_BLOCK_ADDRESS: u16 = 0x0160;
pub const SAFETY_BLOCK_SIZE: usize = 32;
pub const CALIB_BLOCK_ADDRESS: u16 = 0x0180;
pub const CALIB_BLOCK_SIZE: usize = 16;
pub const CONFIG_END: u16 = 0x0240;
pub const EEPROM_SIZE: usize = 8192;

const _: () = assert!(
    LEGACY_LAYOUT_ADDRESS as usize + LEGACY_LAYOUT_SIZE == EXTENSION_ADDRESS as usize,
    "block A follows the 1.x layout"
);
const _: () = assert!(
    EXTENSION_ADDRESS as usize + EXTENSION_BLOCK_SIZE <= SAFETY_BLOCK_ADDRESS as usize,
    "block A ends before block B"
);
const _: () = assert!(
    SAFETY_BLOCK_ADDRESS as usize + SAFETY_BLOCK_SIZE == CALIB_BLOCK_ADDRESS as usize,
    "blocks C follow block B"
);
const _: () = assert!(
    CALIB_BLOCK_ADDRESS as usize + VALVE_COUNT as usize * CALIB_BLOCK_SIZE == CONFIG_END as usize,
    "one block C per valve"
);
const _: () = assert!(
    CONFIG_END as usize <= EEPROM_SIZE,
    "the configuration fits the 24LC64"
);

/// first version with block A (firmware 2.0.0)
pub const LAYOUT_VERSION_V2: u8 = 2;
/// written by this firmware
pub const LAYOUT_VERSION: u8 = 3;
pub const EXTENSION_MAX_PAYLOAD: usize = EXTENSION_BLOCK_SIZE - 3;

/// payload of layout version 2
pub const EXTENSION_PAYLOAD_V2: usize = 3;
/// payload of layout version 3
pub const EXTENSION_PAYLOAD_V3: usize = 11;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoredExtension {
    pub escalation: EscalationConfig,
    /// 0 = time trigger off
    pub learn_time_s: u32,
    /// 0 = off, 5..1440
    pub lease_timeout_min: u16,
    /// crc16_ccitt() of the 1.x layout
    pub layout_crc: u16,
    /// the three fields above were stored (version 3)
    pub has_v3: bool,
}

pub const STORED_EXTENSION_DEFAULT: StoredExtension = StoredExtension {
    escalation: ESCALATION_DEFAULT,
    learn_time_s: LEARN_TIME_DEFAULT_S,
    lease_timeout_min: LEASE_TIMEOUT_DEFAULT_MIN,
    layout_crc: 0,
    has_v3: false,
};

impl Default for StoredExtension {
    /// STORED_EXTENSION_DEFAULT (the C++ member initializers)
    fn default() -> Self {
        STORED_EXTENSION_DEFAULT
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ExtensionState {
    /// no block: image written by firmware 1.x or erased chip
    Legacy = 0,
    /// block read (out-of-range fields replaced by defaults); version 2 has no has_v3 fields
    Valid = 1,
    /// block present but damaged
    Corrupt = 2,
}

fn byte(p: &[u8], at: usize) -> u8 {
    p.get(at).copied().unwrap_or(0xFF)
}

fn get_u16(p: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([byte(p, at), byte(p, at + 1)])
}

fn get_u32(p: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([
        byte(p, at),
        byte(p, at + 1),
        byte(p, at + 2),
        byte(p, at + 3),
    ])
}

/// `out` is always written: defaults unless the state is Valid, the defaults of
/// the version 3 fields unless out.has_v3.
pub fn decode_extension(
    raw: &[u8; EXTENSION_BLOCK_SIZE],
    out: &mut StoredExtension,
) -> ExtensionState {
    *out = STORED_EXTENSION_DEFAULT;

    let version = raw[0];
    // 0xFF: never written (1.x image or new chip); 0x00..0x01 were never used as a version
    if version < LAYOUT_VERSION_V2 || version == 0xFF {
        return ExtensionState::Legacy;
    }

    let length = usize::from(raw[1]);
    if !(EXTENSION_PAYLOAD_V2..=EXTENSION_MAX_PAYLOAD).contains(&length) {
        return ExtensionState::Corrupt;
    }
    if crc8(&raw[..2 + length]) != raw[2 + length] {
        return ExtensionState::Corrupt;
    }

    let payload = &raw[2..];
    let esc = EscalationConfig {
        enable: payload[0],
        step_pct: payload[1],
        max_ma: payload[2],
    };
    out.escalation = sanitize_escalation(&esc);

    if version >= LAYOUT_VERSION && length >= EXTENSION_PAYLOAD_V3 {
        out.learn_time_s = get_u32(payload, 3);
        out.lease_timeout_min = sanitize_lease_timeout(get_u16(payload, 7));
        out.layout_crc = get_u16(payload, 9);
        out.has_v3 = true;
    }
    ExtensionState::Valid
}

/// Encodes the current layout version; unused bytes are 0xFF. Returns the
/// number of bytes to write.
pub fn encode_extension(ext: &StoredExtension, out: &mut [u8; EXTENSION_BLOCK_SIZE]) -> usize {
    out.fill(0xFF);
    out[0] = LAYOUT_VERSION;
    out[1] = EXTENSION_PAYLOAD_V3 as u8;
    out[2] = ext.escalation.enable;
    out[3] = ext.escalation.step_pct;
    out[4] = ext.escalation.max_ma;
    out[5..9].copy_from_slice(&ext.learn_time_s.to_le_bytes());
    out[9..11].copy_from_slice(&ext.lease_timeout_min.to_le_bytes());
    out[11..13].copy_from_slice(&ext.layout_crc.to_le_bytes());
    out[2 + EXTENSION_PAYLOAD_V3] = crc8(&out[..2 + EXTENSION_PAYLOAD_V3]);
    3 + EXTENSION_PAYLOAD_V3
}
