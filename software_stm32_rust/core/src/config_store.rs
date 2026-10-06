//! The configuration as the firmware keeps it in RAM, the rules that build it
//! from the EEPROM blocks (at start-up and after a failed read), and the rules
//! that merge it with the changes made in RAM meanwhile. Hardware-free.

use crate::calibration::EscalationConfig;
use crate::config_blocks::{
    decode_calib, decode_safety, BlockState, CalibRecord, MotorShadow, SafetyBlock,
};
use crate::eeprom_layout::{
    decode_extension, ExtensionState, StoredExtension, CALIB_BLOCK_SIZE, EXTENSION_BLOCK_SIZE,
    SAFETY_BLOCK_SIZE,
};
use crate::failsafe::FAILSAFE_DEFAULT_PCT;
use crate::lease::LEASE_TIMEOUT_DEFAULT_MIN;
use crate::legacy_layout::{
    crc16_ccitt, decode_legacy_layout, sensor_slot_valid, LegacyLayout, SensorSlot,
    LEGACY_IMAGE_SIZE, VALVE_COUNT,
};

const VALVES: usize = VALVE_COUNT as usize;

/// The 1.x layout plus blocks A (escalation, learn time, lease timeout), B
/// (failsafe positions) and C (calibration records). (C++: a struct derived from
/// LegacyLayout; the Rust struct holds it in `layout`.)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConfigImage {
    pub layout: LegacyLayout,
    pub escalation: EscalationConfig,
    /// 0 = time trigger off
    pub learn_time_s: u32,
    /// 0 = off, 5..1440
    pub lease_timeout_min: u16,
    /// 0..100, FAILSAFE_HOLD
    pub failsafe_pct: [u8; VALVES],
    /// flags 0: no record
    pub calib: [CalibRecord; VALVES],
}

// cfgFlags (gstax field 18): what the last load found

/// the 1.x layout failed its CRC: motor fields and learn movements from block B
pub const CFG_LAYOUT_CRC: u8 = 0x01;
/// ... and block B was unusable: those fields are defaults
pub const CFG_SHADOW_MISSING: u8 = 0x02;
/// block A damaged: escalation, learn time and lease timeout are defaults
pub const CFG_SETTINGS_CORRUPT: u8 = 0x04;
/// block B damaged or lost: failsafe positions are 50
pub const CFG_SAFETY_CORRUPT: u8 = 0x08;
/// a sensor slot failed its ROM CRC and was cleared
pub const CFG_SENSOR_SLOT: u8 = 0x10;
/// a calibration record was damaged: that valve calibrates again
pub const CFG_CALIB: u8 = 0x20;
/// no layout CRC yet (first start after 1.x or 2.0.0): a migration write follows
pub const CFG_UNVERIFIED: u8 = 0x40;
/// the EEPROM could not be read
pub const CFG_READ_FAILED: u8 = 0x80;

// Fields changed in RAM (include/eeprom.h EEP_CHANGED_*): they select the
// blocks to write (blocks_for) and survive a re-read (merge_changes).

/// owsensors1/2, per slot in ChangeSet::slots
pub const CHANGED_SENSORS: u16 = 0x0001;
/// number_of_movements
pub const CHANGED_MOVEMENTS: u16 = 0x0002;
/// factors, start_on_power, no_of_min_counts, max_calib_retries
pub const CHANGED_MOTOR: u16 = 0x0004;
/// escalation
pub const CHANGED_ESCALATION: u16 = 0x0008;
/// learn_time_s
pub const CHANGED_LEARN_TIME: u16 = 0x0010;
/// lease_timeout_min
pub const CHANGED_LEASE: u16 = 0x0020;
/// failsafe_pct
pub const CHANGED_FAILSAFE: u16 = 0x0040;
/// calib, per valve in ChangeSet::calib
pub const CHANGED_CALIB: u16 = 0x0080;
pub const CHANGED_ALL: u16 = 0x00FF;

/// The bytes read from the EEPROM; a block that could not be read is 0xFF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawImages {
    pub layout: [u8; LEGACY_IMAGE_SIZE],
    pub settings: [u8; EXTENSION_BLOCK_SIZE],
    pub safety: [u8; SAFETY_BLOCK_SIZE],
    pub calib: [[u8; CALIB_BLOCK_SIZE]; VALVES],
    /// the read gave up (too many failed transfers)
    pub read_failed: bool,
}

// LoadResult::lease_source: where the lease timeout came from

/// block A
pub const LEASE_SOURCE_SETTINGS: u8 = 0;
/// the copy in block B
pub const LEASE_SOURCE_SAFETY: u8 = 1;
/// neither: LEASE_TIMEOUT_DEFAULT_MIN (after a warm reset the glue takes the copy kept in
/// RAM instead)
pub const LEASE_SOURCE_DEFAULT: u8 = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LoadResult {
    pub image: ConfigImage,
    pub cfg_flags: u8,
    /// CHANGED_* fields to write back (0 after a failed read)
    pub rewrite: u16,
    pub lease_source: u8,
}

/// the 1.x layout is written together with block B (shadow) and block A (layout CRC)
const REWRITE_LAYOUT: u16 = CHANGED_SENSORS | CHANGED_MOVEMENTS | CHANGED_MOTOR;

fn apply_shadow(img: &mut ConfigImage, s: &MotorShadow) {
    img.layout.currentbound_low_fac = s.low_fac;
    img.layout.currentbound_high_fac = s.high_fac;
    img.layout.number_of_movements = s.movements;
    img.layout.start_on_power = s.start_on_power;
    img.layout.no_of_min_counts = s.min_counts;
    img.layout.max_calib_retries = s.max_retries;
}

/// true if the slot failed its ROM CRC and was cleared
fn clear_if_invalid(s: &mut SensorSlot) -> bool {
    if sensor_slot_valid(s) {
        return false;
    }
    *s = SensorSlot::default();
    true
}

/// Builds the configuration from the blocks read at start-up or at a re-read:
/// - block A with a layout CRC: a 1.x layout that does not match it takes the
///   motor fields and learn movements from the shadow in block B (defaults if
///   B is unusable). Without a layout CRC (1.x or 2.0.0 image, or A damaged)
///   the layout is used as it is and gets one;
/// - sensor slots owsensors1/2 that fail their ROM CRC are cleared;
/// - failsafe positions from block B, else 50; lease timeout from block A,
///   else from the copy in B, else LEASE_TIMEOUT_DEFAULT_MIN; learn time and
///   escalation from A, else their defaults; calibration records from C;
/// - every repair or migration is written back (`rewrite`);
/// - after a failed read the layout and A are used as read, the lease timeout
///   and failsafe positions are the defaults (60, 50; the glue prefers the copy
///   kept in RAM after a warm reset), cfg_flags is CFG_READ_FAILED and nothing is
///   written back.
pub fn resolve_config(raw: &RawImages, out: &mut LoadResult) {
    let img = &mut out.image;
    decode_legacy_layout(&raw.layout, &mut img.layout);

    let mut a = StoredExtension::default();
    let a_state = decode_extension(&raw.settings, &mut a);
    img.escalation = a.escalation;
    img.learn_time_s = a.learn_time_s;

    let mut b = SafetyBlock::default();
    let b_state = decode_safety(&raw.safety, &mut b);
    img.failsafe_pct = b.failsafe_pct;

    if a.has_v3 {
        img.lease_timeout_min = a.lease_timeout_min;
        out.lease_source = LEASE_SOURCE_SETTINGS;
    } else if b.lease_valid {
        img.lease_timeout_min = b.lease_timeout_min;
        out.lease_source = LEASE_SOURCE_SAFETY;
    } else {
        img.lease_timeout_min = LEASE_TIMEOUT_DEFAULT_MIN;
        out.lease_source = LEASE_SOURCE_DEFAULT;
    }

    let mut calib_corrupt = false;
    for (v, (record, block)) in img.calib.iter_mut().zip(&raw.calib).enumerate() {
        if decode_calib(block, v as u8, record) == BlockState::Corrupt {
            calib_corrupt = true;
        }
    }

    out.rewrite = 0;
    if raw.read_failed {
        // what could be read is used as it is and nothing is written back; the
        // lease timeout and the failsafe positions are the defaults
        img.lease_timeout_min = LEASE_TIMEOUT_DEFAULT_MIN;
        out.lease_source = LEASE_SOURCE_DEFAULT;
        img.failsafe_pct = [FAILSAFE_DEFAULT_PCT; VALVES];
        out.cfg_flags = CFG_READ_FAILED;
        return;
    }

    let mut flags = 0u8;
    if a.has_v3 {
        if crc16_ccitt(&raw.layout) != a.layout_crc {
            // torn write or a 1.x downgrade: the motor fields as this firmware last wrote them
            // (decode_safety() leaves the defaults in an unusable block B)
            apply_shadow(img, &b.shadow);
            flags |= if b_state == BlockState::Valid {
                CFG_LAYOUT_CRC
            } else {
                CFG_LAYOUT_CRC | CFG_SHADOW_MISSING
            };
            out.rewrite |= REWRITE_LAYOUT;
        }
    } else {
        // 1.x or 2.0.0 image, or block A damaged: the layout as read, then stored with a CRC
        flags |= if a_state == ExtensionState::Corrupt {
            CFG_SETTINGS_CORRUPT
        } else {
            CFG_UNVERIFIED
        };
        out.rewrite |= REWRITE_LAYOUT;
    }

    let mut slot_cleared = false;
    for s in img
        .layout
        .owsensors1
        .iter_mut()
        .chain(img.layout.owsensors2.iter_mut())
    {
        if clear_if_invalid(s) {
            slot_cleared = true;
        }
    }
    if slot_cleared {
        flags |= CFG_SENSOR_SLOT;
        out.rewrite |= REWRITE_LAYOUT;
    }

    // a missing block B is expected on the first start after 1.x or 2.0.0 only
    if b_state == BlockState::Corrupt || (b_state == BlockState::Absent && a.has_v3) {
        flags |= CFG_SAFETY_CORRUPT;
    }
    if b_state != BlockState::Valid {
        out.rewrite |= CHANGED_FAILSAFE;
    }

    if calib_corrupt {
        flags |= CFG_CALIB;
    }
    out.cfg_flags = flags;
}

/// True for the flags of a load that repaired or defaulted a checked block
/// (gstax cfgEvents counts such loads).
pub fn repairs_config(cfg_flags: u8) -> bool {
    cfg_flags
        & (CFG_LAYOUT_CRC
            | CFG_SHADOW_MISSING
            | CFG_SETTINGS_CORRUPT
            | CFG_SAFETY_CORRUPT
            | CFG_SENSOR_SLOT
            | CFG_CALIB)
        != 0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    /// CHANGED_*
    pub fields: u16,
    /// bit s: sensor slot s changed (0..11 owsensors1, 12..23 owsensors2)
    pub slots: u32,
    /// bit v: calibration record of valve v changed
    pub calib: u16,
}

/// Copies the changes made in RAM into the configuration read again from the
/// EEPROM: sensor slots and calibration records one by one, the other fields
/// by group.
pub fn merge_changes(stored: &mut ConfigImage, ram: &ConfigImage, changes: &ChangeSet) {
    for v in 0..VALVES {
        if changes.slots & (1u32 << v) != 0 {
            stored.layout.owsensors1[v] = ram.layout.owsensors1[v];
        }
        if changes.slots & (1u32 << (v + VALVES)) != 0 {
            stored.layout.owsensors2[v] = ram.layout.owsensors2[v];
        }
        if changes.calib & (1u16 << v) != 0 {
            stored.calib[v] = ram.calib[v];
        }
    }
    if changes.fields & CHANGED_MOVEMENTS != 0 {
        stored.layout.number_of_movements = ram.layout.number_of_movements;
    }
    if changes.fields & CHANGED_MOTOR != 0 {
        stored.layout.currentbound_low_fac = ram.layout.currentbound_low_fac;
        stored.layout.currentbound_high_fac = ram.layout.currentbound_high_fac;
        stored.layout.start_on_power = ram.layout.start_on_power;
        stored.layout.no_of_min_counts = ram.layout.no_of_min_counts;
        stored.layout.max_calib_retries = ram.layout.max_calib_retries;
    }
    if changes.fields & CHANGED_ESCALATION != 0 {
        stored.escalation = ram.escalation;
    }
    if changes.fields & CHANGED_LEARN_TIME != 0 {
        stored.learn_time_s = ram.learn_time_s;
    }
    if changes.fields & CHANGED_LEASE != 0 {
        stored.lease_timeout_min = ram.lease_timeout_min;
    }
    if changes.fields & CHANGED_FAILSAFE != 0 {
        stored.failsafe_pct = ram.failsafe_pct;
    }
}

// StoreBlock
pub const BLOCK_LAYOUT: u8 = 0x01;
/// block A
pub const BLOCK_SETTINGS: u8 = 0x02;
/// block B
pub const BLOCK_SAFETY: u8 = 0x04;
/// the blocks C of the valves in ChangeSet::calib
pub const BLOCK_CALIB: u8 = 0x08;

/// The blocks that hold the fields: the 1.x fields are written with block B
/// (shadow) and block A (layout CRC); the lease timeout with A and B (copy).
pub fn blocks_for(fields: u16) -> u8 {
    let mut blocks = 0u8;
    if fields & (CHANGED_SENSORS | CHANGED_MOVEMENTS | CHANGED_MOTOR) != 0 {
        blocks |= BLOCK_LAYOUT | BLOCK_SAFETY | BLOCK_SETTINGS;
    }
    if fields & (CHANGED_ESCALATION | CHANGED_LEARN_TIME) != 0 {
        blocks |= BLOCK_SETTINGS;
    }
    if fields & CHANGED_LEASE != 0 {
        blocks |= BLOCK_SETTINGS | BLOCK_SAFETY;
    }
    if fields & CHANGED_FAILSAFE != 0 {
        blocks |= BLOCK_SAFETY;
    }
    if fields & CHANGED_CALIB != 0 {
        blocks |= BLOCK_CALIB;
    }
    blocks
}

#[cfg(test)]
mod tests;
