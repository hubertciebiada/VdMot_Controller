//! Port of test/native/test_config_store.cpp.

use super::*;
use crate::calibration::{EscalationConfig, ESCALATION_DEFAULT};
use crate::config_blocks::{
    encode_calib, encode_safety, CalibRecord, MotorShadow, SafetyBlock, CALIB_FAILED, CALIB_VALID,
};
use crate::eeprom_layout::{
    encode_extension, StoredExtension, CALIB_BLOCK_SIZE, EXTENSION_BLOCK_SIZE, SAFETY_BLOCK_SIZE,
};
use crate::legacy_layout::{
    crc16_ccitt, encode_legacy_layout, LegacyLayout, SensorSlot, EXTRA_SENSOR_SLOTS,
    LEGACY_IMAGE_SIZE, VALVE_COUNT,
};
use crate::onewire_check::crc8;

// offsets in the 1.x image
const LOW_FAC_OFFSET: usize = 29;
/// owsensors1[0]
const SLOTS_OFFSET: usize = 33;

const EXTRA_SLOTS: usize = EXTRA_SENSOR_SLOTS as usize;

/// The 8 bytes of a slot in the order of the C++ struct (address order); the C++ test fills
/// the struct with memcpy and memset.
fn slot_from_bytes(b: [u8; 8]) -> SensorSlot {
    SensorSlot {
        familycode: b[0],
        romcode: [b[1], b[2], b[3], b[4], b[5], b[6]],
        crc: b[7],
    }
}

/// a DS18B20-like address with a valid CRC, different for every n
fn valid_slot(n: u8) -> SensorSlot {
    let mut address: [u8; 8] = [
        0x28,
        0x10u8.wrapping_add(n),
        0x4C,
        n.wrapping_mul(7),
        0x61,
        0x16,
        0x04,
        0,
    ];
    address[7] = crc8(&address[..7]);
    slot_from_bytes(address)
}

fn erased_slot() -> SensorSlot {
    slot_from_bytes([0xFF; 8])
}

/// what this firmware writes: a consistent set of blocks
#[derive(Clone, Copy)]
struct Stored {
    layout: LegacyLayout,
    a: StoredExtension,
    b: SafetyBlock,
    /// flags 0: block never written
    calib: [CalibRecord; VALVES],
}

fn sample_stored() -> Stored {
    // Stored s{}: zeros, block A with the member initializers of StoredExtension
    let mut s = Stored {
        layout: LegacyLayout::default(),
        a: StoredExtension::default(),
        b: SafetyBlock::default(),
        calib: [CalibRecord::default(); VALVES],
    };
    let l = &mut s.layout;
    l.b_slave = 0;
    l.descr[..5].copy_from_slice(b"VdMot"); // strcpy: descr[5] is the NUL already
    l.one_wire_cfg[0] = 20;
    l.currentbound_low_fac = 21;
    l.currentbound_high_fac = 27;
    l.number_of_movements = 1500;
    for v in 0..VALVE_COUNT {
        let i = usize::from(v);
        l.owsensors1[i] = if v < 8 { valid_slot(v) } else { erased_slot() };
        l.owsensors2[i] = if v < 4 {
            valid_slot(20 + v)
        } else {
            erased_slot()
        };
    }
    l.owsensors2[4] = slot_from_bytes([0; 8]); // a cleared assignment
    for x in &mut l.owsensors {
        *x = erased_slot();
    }
    l.start_on_power = 40;
    l.no_of_min_counts = 2500;
    l.max_calib_retries = 1;

    s.a.escalation = EscalationConfig {
        enable: 1,
        step_pct: 30,
        max_ma: 55,
    };
    s.a.learn_time_s = 86400;
    s.a.lease_timeout_min = 30;
    s.a.has_v3 = true;

    s.b.failsafe_pct = [0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 255];
    s.b.shadow = MotorShadow {
        low_fac: 21,
        high_fac: 27,
        movements: 1500,
        start_on_power: 40,
        min_counts: 2500,
        max_retries: 1,
    };
    s.b.lease_timeout_min = 30;

    s.calib[0] = CalibRecord {
        opening_count: 3567,
        closing_count: 3610,
        mean_current: 17,
        flags: CALIB_VALID,
    };
    s.calib[3] = CalibRecord {
        opening_count: 2800,
        closing_count: 2900,
        mean_current: 25,
        flags: CALIB_VALID | CALIB_FAILED,
    };
    s.calib[11] = CalibRecord {
        opening_count: 0,
        closing_count: 0,
        mean_current: 20,
        flags: CALIB_FAILED,
    };
    s
}

/// memset(&raw, 0xFF, sizeof(raw)): every block 0xFF; read_failed is true like the C++ bool
/// that holds 0xFF
fn erased() -> RawImages {
    RawImages {
        layout: [0xFF; LEGACY_IMAGE_SIZE],
        settings: [0xFF; EXTENSION_BLOCK_SIZE],
        safety: [0xFF; SAFETY_BLOCK_SIZE],
        calib: [[0xFF; CALIB_BLOCK_SIZE]; VALVES],
        read_failed: true,
    }
}

/// the blocks as read from the EEPROM; `layout_crc` of block A is the CRC of the layout
/// as `s` has it, the layout bytes may be changed afterwards
fn write_raw(s: &Stored, raw: &mut RawImages) {
    *raw = erased();
    raw.read_failed = false;
    encode_legacy_layout(&s.layout, &mut raw.layout);
    let mut a = s.a;
    a.layout_crc = crc16_ccitt(&raw.layout);
    encode_extension(&a, &mut raw.settings);
    encode_safety(&s.b, &mut raw.safety);
    for (v, (record, block)) in s.calib.iter().zip(raw.calib.iter_mut()).enumerate() {
        if record.flags != 0 {
            encode_calib(record, v as u8, block);
        }
    }
}

/// block A as firmware 2.0.0 wrote it (version 2: escalation only)
fn write_settings_v2(raw: &mut RawImages, enable: u8, step_pct: u8, max_ma: u8) {
    raw.settings.fill(0xFF);
    raw.settings[0] = 2;
    raw.settings[1] = 3;
    raw.settings[2] = enable;
    raw.settings[3] = step_pct;
    raw.settings[4] = max_ma;
    raw.settings[5] = crc8(&raw.settings[..5]);
}

/// memcmp of the 8 bytes: every field
fn same_slot(x: &SensorSlot, y: &SensorSlot) -> bool {
    x == y
}

fn is_zero_slot(x: &SensorSlot) -> bool {
    same_slot(x, &slot_from_bytes([0; 8]))
}

fn same_calib(x: &CalibRecord, y: &CalibRecord) -> bool {
    x.opening_count == y.opening_count
        && x.closing_count == y.closing_count
        && x.mean_current == y.mean_current
        && x.flags == y.flags
}

fn no_record(x: &CalibRecord) -> bool {
    same_calib(
        x,
        &CalibRecord {
            opening_count: 0,
            closing_count: 0,
            mean_current: 0,
            flags: 0,
        },
    )
}

fn same_motor_fields(
    img: &ConfigImage,
    low: u8,
    high: u8,
    movements: u16,
    start_on_power: u8,
    min_counts: u16,
    max_retries: u8,
) -> bool {
    img.layout.currentbound_low_fac == low
        && img.layout.currentbound_high_fac == high
        && img.layout.number_of_movements == movements
        && img.layout.start_on_power == start_on_power
        && img.layout.no_of_min_counts == min_counts
        && img.layout.max_calib_retries == max_retries
}

fn all_failsafe(img: &ConfigImage, pct: u8) -> bool {
    img.failsafe_pct.iter().all(|&p| p == pct)
}

/// The whole image as `s` describes it (the fields a load keeps unchanged).
fn matches_stored(img: &ConfigImage, s: &Stored) -> bool {
    let l = &s.layout;
    if img.layout.b_slave != l.b_slave
        || img.layout.descr != l.descr
        || img.layout.one_wire_cfg != l.one_wire_cfg
        || !same_motor_fields(
            img,
            l.currentbound_low_fac,
            l.currentbound_high_fac,
            l.number_of_movements,
            l.start_on_power,
            l.no_of_min_counts,
            l.max_calib_retries,
        )
    {
        return false;
    }
    for v in 0..VALVES {
        if !same_slot(&img.layout.owsensors1[v], &l.owsensors1[v])
            || !same_slot(&img.layout.owsensors2[v], &l.owsensors2[v])
        {
            return false;
        }
        if !same_calib(&img.calib[v], &s.calib[v]) {
            return false;
        }
    }
    for i in 0..EXTRA_SLOTS {
        if !same_slot(&img.layout.owsensors[i], &l.owsensors[i]) {
            return false;
        }
    }
    img.escalation.enable == s.a.escalation.enable
        && img.escalation.step_pct == s.a.escalation.step_pct
        && img.escalation.max_ma == s.a.escalation.max_ma
        && img.learn_time_s == s.a.learn_time_s
        && img.lease_timeout_min == s.a.lease_timeout_min
        && img.failsafe_pct == s.b.failsafe_pct
}

const ALL_BLOCKS: u8 = BLOCK_LAYOUT | BLOCK_SETTINGS | BLOCK_SAFETY;

/// memset(&r, fill, sizeof(r)): every byte of every field is `fill`
fn load_result_filled(fill: u8) -> LoadResult {
    let w = u16::from_le_bytes([fill, fill]);
    let slot = slot_from_bytes([fill; 8]);
    LoadResult {
        image: ConfigImage {
            layout: LegacyLayout {
                b_slave: fill,
                descr: [fill; 25],
                one_wire_cfg: [fill; 3],
                currentbound_low_fac: fill,
                currentbound_high_fac: fill,
                number_of_movements: w,
                owsensors1: [slot; VALVES],
                owsensors2: [slot; VALVES],
                owsensors: [slot; EXTRA_SLOTS],
                start_on_power: fill,
                no_of_min_counts: w,
                max_calib_retries: fill,
            },
            escalation: EscalationConfig {
                enable: fill,
                step_pct: fill,
                max_ma: fill,
            },
            learn_time_s: u32::from_le_bytes([fill; 4]),
            lease_timeout_min: w,
            failsafe_pct: [fill; VALVES],
            calib: [CalibRecord {
                opening_count: w,
                closing_count: w,
                mean_current: w,
                flags: fill,
            }; VALVES],
        },
        cfg_flags: fill,
        rewrite: w,
        lease_source: fill,
    }
}

fn resolve(raw: &RawImages) -> LoadResult {
    let mut r = load_result_filled(0xAA);
    resolve_config(raw, &mut r);
    r
}

#[test]
fn flag_mask_and_source_values() {
    assert_eq!(CFG_LAYOUT_CRC, 0x01);
    assert_eq!(CFG_SHADOW_MISSING, 0x02);
    assert_eq!(CFG_SETTINGS_CORRUPT, 0x04);
    assert_eq!(CFG_SAFETY_CORRUPT, 0x08);
    assert_eq!(CFG_SENSOR_SLOT, 0x10);
    assert_eq!(CFG_CALIB, 0x20);
    assert_eq!(CFG_UNVERIFIED, 0x40);
    assert_eq!(CFG_READ_FAILED, 0x80);
    assert_eq!(CHANGED_SENSORS, 0x0001);
    assert_eq!(CHANGED_MOVEMENTS, 0x0002);
    assert_eq!(CHANGED_MOTOR, 0x0004);
    assert_eq!(CHANGED_ESCALATION, 0x0008);
    assert_eq!(CHANGED_LEARN_TIME, 0x0010);
    assert_eq!(CHANGED_LEASE, 0x0020);
    assert_eq!(CHANGED_FAILSAFE, 0x0040);
    assert_eq!(CHANGED_CALIB, 0x0080);
    assert_eq!(CHANGED_ALL, 0x00FF);
    assert_eq!(LEASE_SOURCE_SETTINGS, 0);
    assert_eq!(LEASE_SOURCE_SAFETY, 1);
    assert_eq!(LEASE_SOURCE_DEFAULT, 2);
}

#[test]
fn resolve_config_a_consistent_blocks_load_unchanged_nothing_to_write() {
    let s = sample_stored();
    let mut raw = erased();
    write_raw(&s, &mut raw);
    let r = resolve(&raw);
    assert_eq!(r.cfg_flags, 0);
    assert_eq!(r.rewrite, 0);
    assert_eq!(r.lease_source, LEASE_SOURCE_SETTINGS);
    assert!(matches_stored(&r.image, &s));
    assert_eq!(r.image.calib[3].flags, CALIB_VALID | CALIB_FAILED);
    assert!(no_record(&r.image.calib[5]));
}

#[test]
fn resolve_config_b_a_changed_motor_field_fails_the_layout_crc_block_b_has_the_values() {
    let s = sample_stored();
    let mut raw = erased();
    write_raw(&s, &mut raw);
    raw.layout[LOW_FAC_OFFSET] = 35; // currentbound_low_fac, torn write
    let r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_LAYOUT_CRC);
    assert_eq!(blocks_for(r.rewrite), ALL_BLOCKS);
    assert_eq!(r.lease_source, LEASE_SOURCE_SETTINGS);
    assert!(matches_stored(&r.image, &s)); // low factor 21 from the shadow, the rest as stored

    // every motor field and the learn movements come from the shadow
    let mut other = s;
    other.b.shadow = MotorShadow {
        low_fac: 11,
        high_fac: 12,
        movements: 60000,
        start_on_power: 13,
        min_counts: 14,
        max_retries: 2,
    };
    write_raw(&other, &mut raw);
    raw.layout[LOW_FAC_OFFSET] = 35;
    let r2 = resolve(&raw);
    assert_eq!(r2.cfg_flags, CFG_LAYOUT_CRC);
    assert!(same_motor_fields(&r2.image, 11, 12, 60000, 13, 14, 2));
    assert!(same_slot(
        &r2.image.layout.owsensors1[2],
        &s.layout.owsensors1[2]
    ));
    assert_eq!(r2.image.layout.descr, s.layout.descr);
}

#[test]
fn resolve_config_c_layout_crc_fails_and_block_b_is_damaged_motor_defaults() {
    let s = sample_stored();
    let mut raw = erased();
    write_raw(&s, &mut raw);
    raw.layout[LOW_FAC_OFFSET] = 35;
    raw.safety[5] ^= 0x10;
    let r = resolve(&raw);
    assert_eq!(
        r.cfg_flags,
        CFG_LAYOUT_CRC | CFG_SHADOW_MISSING | CFG_SAFETY_CORRUPT
    );
    assert!(same_motor_fields(&r.image, 17, 17, 2000, 30, 3000, 2));
    assert!(all_failsafe(&r.image, 50));
    assert_eq!(blocks_for(r.rewrite), ALL_BLOCKS);
    assert_eq!(r.lease_source, LEASE_SOURCE_SETTINGS);
    assert_eq!(r.image.lease_timeout_min, 30);

    // block B never written (A has a layout CRC, so B was lost): the same
    write_raw(&s, &mut raw);
    raw.layout[LOW_FAC_OFFSET] = 35;
    raw.safety.fill(0xFF);
    let r2 = resolve(&raw);
    assert_eq!(
        r2.cfg_flags,
        CFG_LAYOUT_CRC | CFG_SHADOW_MISSING | CFG_SAFETY_CORRUPT
    );
    assert!(same_motor_fields(&r2.image, 17, 17, 2000, 30, 3000, 2));
}

#[test]
fn resolve_config_d_a_sensor_slot_with_a_flipped_bit_is_cleared_the_others_stay() {
    let s = sample_stored();
    let mut damaged = s;
    damaged.layout.owsensors1[5].romcode[2] ^= 0x04; // stored like that: the layout CRC matches
    let mut raw = erased();
    write_raw(&damaged, &mut raw);
    let r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_SENSOR_SLOT);
    assert_eq!(blocks_for(r.rewrite), ALL_BLOCKS);
    assert!(is_zero_slot(&r.image.layout.owsensors1[5]));
    for v in 0..VALVES {
        if v != 5 {
            assert!(
                same_slot(&r.image.layout.owsensors1[v], &s.layout.owsensors1[v]),
                "valve {v}"
            );
        }
        assert!(
            same_slot(&r.image.layout.owsensors2[v], &s.layout.owsensors2[v]),
            "valve {v}"
        );
    }

    // the same in a second slot of a valve
    damaged = s;
    damaged.layout.owsensors2[1].crc ^= 0x80;
    write_raw(&damaged, &mut raw);
    let r2 = resolve(&raw);
    assert_eq!(r2.cfg_flags, CFG_SENSOR_SLOT);
    assert!(is_zero_slot(&r2.image.layout.owsensors2[1]));
    assert!(same_slot(
        &r2.image.layout.owsensors1[1],
        &s.layout.owsensors1[1]
    ));

    // a bit flipped after the write fails the layout CRC as well
    write_raw(&s, &mut raw);
    raw.layout[SLOTS_OFFSET + 8 * 3 + 2] ^= 0x01; // owsensors1[3]
    let r3 = resolve(&raw);
    assert_eq!(r3.cfg_flags, CFG_LAYOUT_CRC | CFG_SENSOR_SLOT);
    assert!(is_zero_slot(&r3.image.layout.owsensors1[3]));

    // the additional slots are not used and not checked
    damaged = s;
    damaged.layout.owsensors[0] = valid_slot(9);
    damaged.layout.owsensors[0].crc ^= 0x01;
    write_raw(&damaged, &mut raw);
    let r4 = resolve(&raw);
    assert_eq!(r4.cfg_flags, 0);
    assert!(same_slot(
        &r4.image.layout.owsensors[0],
        &damaged.layout.owsensors[0]
    ));
}

#[test]
fn resolve_config_e_first_start_after_2_0_0_keeps_the_escalation_migrates_the_layout() {
    let s = sample_stored();
    let mut raw = erased();
    write_raw(&s, &mut raw);
    write_settings_v2(&mut raw, 1, 10, 20);
    raw.safety.fill(0xFF);
    for c in &mut raw.calib {
        c.fill(0xFF);
    }
    raw.layout[LOW_FAC_OFFSET] = 35; // no layout CRC yet: used as read
    let r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_UNVERIFIED);
    assert_eq!(blocks_for(r.rewrite), ALL_BLOCKS);
    assert_eq!(r.image.escalation.enable, 1);
    assert_eq!(r.image.escalation.step_pct, 10);
    assert_eq!(r.image.escalation.max_ma, 20);
    assert_eq!(r.image.learn_time_s, 604_800);
    assert_eq!(r.image.lease_timeout_min, 60);
    assert_eq!(r.lease_source, LEASE_SOURCE_DEFAULT);
    assert!(all_failsafe(&r.image, 50));
    assert_eq!(r.image.layout.currentbound_low_fac, 35);
    assert_eq!(r.image.layout.number_of_movements, 1500);
    for (v, c) in r.image.calib.iter().enumerate() {
        assert!(no_record(c), "valve {v}");
    }
}

#[test]
fn resolve_config_f_a_new_chip_all_0xff_loads_defaults_and_gets_written_once() {
    let mut raw = erased();
    raw.read_failed = false;
    let r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_UNVERIFIED);
    assert_eq!(blocks_for(r.rewrite), ALL_BLOCKS);
    assert_eq!(r.image.escalation.enable, ESCALATION_DEFAULT.enable);
    assert_eq!(r.image.escalation.step_pct, ESCALATION_DEFAULT.step_pct);
    assert_eq!(r.image.escalation.max_ma, ESCALATION_DEFAULT.max_ma);
    assert_eq!(r.image.learn_time_s, 604_800);
    assert_eq!(r.image.lease_timeout_min, 60);
    assert_eq!(r.lease_source, LEASE_SOURCE_DEFAULT);
    assert!(all_failsafe(&r.image, 50));
    for (v, c) in r.image.calib.iter().enumerate() {
        assert!(no_record(c), "valve {v}");
    }
    // the 1.x fields as read: app_load_config() replaces what is out of range
    assert_eq!(r.image.layout.currentbound_low_fac, 0xFF);
    assert_eq!(r.image.layout.no_of_min_counts, 0xFFFF);
    assert!(same_slot(&r.image.layout.owsensors1[0], &erased_slot()));
}

#[test]
fn resolve_config_g_after_a_failed_read_nothing_is_repaired_or_written() {
    let s = sample_stored();
    let mut raw = erased();
    write_raw(&s, &mut raw);
    raw.read_failed = true;
    let mut r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_READ_FAILED);
    assert_eq!(r.rewrite, 0);
    // lease timeout and failsafe positions: defaults (the glue prefers the copies in RAM after
    // a warm reset)
    assert_eq!(r.lease_source, LEASE_SOURCE_DEFAULT);
    assert_eq!(r.image.lease_timeout_min, 60);
    assert!(all_failsafe(&r.image, 50));
    // what was read is used
    assert_eq!(r.image.escalation.step_pct, 30);
    assert_eq!(r.image.learn_time_s, 86400);
    assert_eq!(r.image.layout.currentbound_low_fac, 21);
    assert!(same_calib(&r.image.calib[0], &s.calib[0]));

    // unread blocks are 0xFF: no layout check, no slot check, no flags but the read failure
    raw.settings.fill(0xFF);
    raw.safety.fill(0xFF);
    raw.layout[LOW_FAC_OFFSET] = 35;
    raw.layout[SLOTS_OFFSET + 1] ^= 0x01;
    raw.calib[2][4] ^= 0x01;
    r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_READ_FAILED);
    assert_eq!(r.rewrite, 0);
    assert_eq!(r.image.layout.currentbound_low_fac, 35);
    assert!(!is_zero_slot(&r.image.layout.owsensors1[0]));
    assert_eq!(r.image.escalation.step_pct, ESCALATION_DEFAULT.step_pct);
    assert_eq!(r.image.learn_time_s, 604_800);
}

#[test]
fn resolve_config_h_block_b_missing_next_to_a_block_a_of_this_firmware_is_flagged() {
    let s = sample_stored();
    let mut raw = erased();
    write_raw(&s, &mut raw);
    raw.safety.fill(0xFF);
    let r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_SAFETY_CORRUPT);
    assert_eq!(r.rewrite, CHANGED_FAILSAFE);
    assert_eq!(blocks_for(r.rewrite), BLOCK_SAFETY);
    assert!(all_failsafe(&r.image, 50));
    assert_eq!(r.image.lease_timeout_min, 30); // from block A
    assert_eq!(r.image.layout.currentbound_low_fac, 21);
}

#[test]
fn resolve_config_i_a_damaged_calibration_record_is_dropped_and_flagged() {
    let s = sample_stored();
    let mut raw = erased();
    write_raw(&s, &mut raw);
    raw.calib[3][6] ^= 0x20;
    let r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_CALIB);
    assert_eq!(r.rewrite, 0); // written again after the next calibration of the valve
    assert!(no_record(&r.image.calib[3]));
    assert!(same_calib(&r.image.calib[0], &s.calib[0]));
    assert!(same_calib(&r.image.calib[11], &s.calib[11]));

    // the record of valve 11 in the block of valve 10 (index check)
    write_raw(&s, &mut raw);
    raw.calib[10] = raw.calib[11];
    let r2 = resolve(&raw);
    assert_eq!(r2.cfg_flags, CFG_CALIB);
    assert!(no_record(&r2.image.calib[10]));
    assert!(same_calib(&r2.image.calib[11], &s.calib[11]));
}

#[test]
fn resolve_config_j_after_a_1_x_downgrade_new_sensor_slots_stay_and_motor_fields_come_from_b() {
    let s = sample_stored();
    let mut raw = erased();
    write_raw(&s, &mut raw);
    // 1.x rewrote the layout: a sensor for valve 9 and another low factor; A and B untouched
    let mut by1x = s;
    by1x.layout.owsensors1[9] = valid_slot(42);
    by1x.layout.currentbound_low_fac = 33;
    encode_legacy_layout(&by1x.layout, &mut raw.layout);
    let r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_LAYOUT_CRC);
    assert_eq!(blocks_for(r.rewrite), ALL_BLOCKS);
    assert!(same_slot(&r.image.layout.owsensors1[9], &valid_slot(42)));
    assert_eq!(r.image.layout.currentbound_low_fac, 21);
    assert!(!matches_stored(&r.image, &by1x));
    let mut expected = s;
    expected.layout.owsensors1[9] = valid_slot(42);
    assert!(matches_stored(&r.image, &expected));
}

#[test]
fn resolve_config_lease_timeout_from_block_a_else_the_copy_in_b_else_60() {
    let s = sample_stored();
    let mut raw = erased();

    // A damaged, B valid: the copy in B
    write_raw(&s, &mut raw);
    raw.settings[3] ^= 0x01;
    let mut r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_SETTINGS_CORRUPT);
    assert_eq!(r.lease_source, LEASE_SOURCE_SAFETY);
    assert_eq!(r.image.lease_timeout_min, 30);
    assert_eq!(r.image.learn_time_s, 604_800);
    assert_eq!(r.image.escalation.step_pct, ESCALATION_DEFAULT.step_pct);
    assert_eq!(blocks_for(r.rewrite), ALL_BLOCKS);
    assert_eq!(r.image.failsafe_pct[1], 10);

    // 2.1 -> 2.0.0 -> 2.1: A is version 2 again, B and C survived
    write_raw(&s, &mut raw);
    write_settings_v2(&mut raw, 1, 30, 55);
    r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_UNVERIFIED);
    assert_eq!(r.lease_source, LEASE_SOURCE_SAFETY);
    assert_eq!(r.image.lease_timeout_min, 30);
    assert_eq!(r.image.failsafe_pct[10], 100);
    assert!(same_calib(&r.image.calib[3], &s.calib[3]));

    // A of a 1.x image (erased) and no B: default
    write_raw(&s, &mut raw);
    raw.settings.fill(0xFF);
    raw.safety.fill(0xFF);
    r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_UNVERIFIED);
    assert_eq!(r.lease_source, LEASE_SOURCE_DEFAULT);
    assert_eq!(r.image.lease_timeout_min, 60);

    // B valid with an invalid copy: default
    let mut bad_copy = s;
    bad_copy.b.lease_timeout_min = 2000;
    write_raw(&bad_copy, &mut raw);
    raw.settings.fill(0xFF);
    r = resolve(&raw);
    assert_eq!(r.lease_source, LEASE_SOURCE_DEFAULT);
    assert_eq!(r.image.lease_timeout_min, 60);
    assert_eq!(r.image.failsafe_pct[2], 20);

    // A of this firmware wins over the copy, also "off"
    let mut off = s;
    off.a.lease_timeout_min = 0;
    write_raw(&off, &mut raw);
    r = resolve(&raw);
    assert_eq!(r.lease_source, LEASE_SOURCE_SETTINGS);
    assert_eq!(r.image.lease_timeout_min, 0);

    // both damaged
    write_raw(&s, &mut raw);
    raw.settings[3] ^= 0x01;
    raw.safety[3] ^= 0x01;
    r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_SETTINGS_CORRUPT | CFG_SAFETY_CORRUPT);
    assert_eq!(r.lease_source, LEASE_SOURCE_DEFAULT);
    assert_eq!(r.image.lease_timeout_min, 60);
    assert!(all_failsafe(&r.image, 50));
}

#[test]
fn repairs_config_repaired_or_defaulted_blocks_count_a_migration_and_a_failed_read_do_not() {
    assert!(repairs_config(CFG_LAYOUT_CRC));
    assert!(repairs_config(CFG_SHADOW_MISSING));
    assert!(repairs_config(CFG_SETTINGS_CORRUPT));
    assert!(repairs_config(CFG_SAFETY_CORRUPT));
    assert!(repairs_config(CFG_SENSOR_SLOT));
    assert!(repairs_config(CFG_CALIB));
    assert!(!repairs_config(0));
    assert!(!repairs_config(CFG_UNVERIFIED));
    assert!(!repairs_config(CFG_READ_FAILED));
    assert!(!repairs_config(CFG_UNVERIFIED | CFG_READ_FAILED));
    assert!(repairs_config(CFG_UNVERIFIED | CFG_CALIB));
    assert!(repairs_config(0xFF));
}

fn image_a() -> ConfigImage {
    let s = sample_stored();
    let mut raw = erased();
    write_raw(&s, &mut raw);
    let mut r = LoadResult::default();
    resolve_config(&raw, &mut r);
    r.image
}

/// every field different from image_a()
fn image_b() -> ConfigImage {
    let mut b = image_a();
    for v in 0..VALVE_COUNT {
        let i = usize::from(v);
        b.layout.owsensors1[i] = valid_slot(100 + v);
        b.layout.owsensors2[i] = valid_slot(120 + v);
        b.calib[i] = CalibRecord {
            opening_count: 1000 + u16::from(v),
            closing_count: 2000 + u16::from(v),
            mean_current: 40,
            flags: CALIB_VALID,
        };
        b.failsafe_pct[i] = v + 1;
    }
    for i in 0..EXTRA_SENSOR_SLOTS {
        b.layout.owsensors[usize::from(i)] = valid_slot(140 + i);
    }
    b.layout.b_slave = 7;
    b.layout.descr[0] = b'x';
    b.layout.one_wire_cfg[1] = 9;
    b.layout.currentbound_low_fac = 31;
    b.layout.currentbound_high_fac = 32;
    b.layout.number_of_movements = 333;
    b.layout.start_on_power = 77;
    b.layout.no_of_min_counts = 1234;
    b.layout.max_calib_retries = 0;
    b.escalation = EscalationConfig {
        enable: 0,
        step_pct: 5,
        max_ma: 44,
    };
    b.learn_time_s = 12345;
    b.lease_timeout_min = 720;
    b
}

/// fields of `img` that differ from image_a(), as CHANGED_* groups; slot and record
/// differences as bits in `slots` / `calib`
fn changed_groups(img: &ConfigImage, slots: &mut u32, calib: &mut u16) -> u16 {
    let a = image_a();
    let mut groups: u16 = 0;
    *slots = 0;
    *calib = 0;
    for v in 0..VALVES {
        if !same_slot(&img.layout.owsensors1[v], &a.layout.owsensors1[v]) {
            *slots |= 1 << v;
        }
        if !same_slot(&img.layout.owsensors2[v], &a.layout.owsensors2[v]) {
            *slots |= 1 << (v + 12);
        }
        if !same_calib(&img.calib[v], &a.calib[v]) {
            *calib |= 1 << v;
        }
    }
    if img.layout.number_of_movements != a.layout.number_of_movements {
        groups |= CHANGED_MOVEMENTS;
    }
    if img.layout.currentbound_low_fac != a.layout.currentbound_low_fac
        || img.layout.currentbound_high_fac != a.layout.currentbound_high_fac
        || img.layout.start_on_power != a.layout.start_on_power
        || img.layout.no_of_min_counts != a.layout.no_of_min_counts
        || img.layout.max_calib_retries != a.layout.max_calib_retries
    {
        groups |= CHANGED_MOTOR;
    }
    // all five motor fields change together
    if groups & CHANGED_MOTOR != 0 {
        assert_ne!(
            img.layout.currentbound_low_fac,
            a.layout.currentbound_low_fac
        );
        assert_ne!(
            img.layout.currentbound_high_fac,
            a.layout.currentbound_high_fac
        );
        assert_ne!(img.layout.start_on_power, a.layout.start_on_power);
        assert_ne!(img.layout.no_of_min_counts, a.layout.no_of_min_counts);
        assert_ne!(img.layout.max_calib_retries, a.layout.max_calib_retries);
    }
    if img.escalation.enable != a.escalation.enable
        || img.escalation.step_pct != a.escalation.step_pct
        || img.escalation.max_ma != a.escalation.max_ma
    {
        groups |= CHANGED_ESCALATION;
    }
    if img.learn_time_s != a.learn_time_s {
        groups |= CHANGED_LEARN_TIME;
    }
    if img.lease_timeout_min != a.lease_timeout_min {
        groups |= CHANGED_LEASE;
    }
    if img.failsafe_pct != a.failsafe_pct {
        groups |= CHANGED_FAILSAFE;
    }
    // never merged
    assert_eq!(img.layout.b_slave, a.layout.b_slave);
    assert_eq!(img.layout.descr, a.layout.descr);
    assert_eq!(img.layout.one_wire_cfg, a.layout.one_wire_cfg);
    for i in 0..EXTRA_SLOTS {
        assert!(
            same_slot(&img.layout.owsensors[i], &a.layout.owsensors[i]),
            "slot {i}"
        );
    }
    groups
}

fn merge_and_diff(changes: &ChangeSet, slots: &mut u32, calib: &mut u16) -> u16 {
    let mut stored = image_a();
    let ram = image_b();
    merge_changes(&mut stored, &ram, changes);
    changed_groups(&stored, slots, calib)
}

/// vdm::ChangeSet{fields, slots, calib}
fn change_set(fields: u16, slots: u32, calib: u16) -> ChangeSet {
    ChangeSet {
        fields,
        slots,
        calib,
    }
}

#[test]
fn merge_changes_sensor_slots_one_by_one() {
    let mut slots: u32 = 0;
    let mut calib: u16 = 0;
    let only_5 = change_set(CHANGED_SENSORS, 1 << 5, 0);
    assert_eq!(merge_and_diff(&only_5, &mut slots, &mut calib), 0);
    assert_eq!(slots, 1 << 5); // owsensors1[5] only
    assert_eq!(calib, 0);
    let only_17 = change_set(CHANGED_SENSORS, 1 << 17, 0);
    assert_eq!(merge_and_diff(&only_17, &mut slots, &mut calib), 0);
    assert_eq!(slots, 1 << 17); // owsensors2[5] only
    let four = (1 << 0) | (1 << 11) | (1 << 12) | (1 << 23);
    assert_eq!(
        merge_and_diff(
            &change_set(CHANGED_SENSORS, four, 0),
            &mut slots,
            &mut calib
        ),
        0
    );
    assert_eq!(slots, four);
    // the group bit alone copies no slot
    let none = change_set(CHANGED_SENSORS, 0, 0);
    assert_eq!(merge_and_diff(&none, &mut slots, &mut calib), 0);
    assert_eq!(slots, 0);
}

#[test]
fn merge_changes_calibration_records_one_by_one() {
    let mut slots: u32 = 0;
    let mut calib: u16 = 0;
    let only_3 = change_set(CHANGED_CALIB, 0, 1 << 3);
    assert_eq!(merge_and_diff(&only_3, &mut slots, &mut calib), 0);
    assert_eq!(calib, 1 << 3);
    assert_eq!(slots, 0);
    let two = change_set(CHANGED_CALIB, 0, (1 << 0) | (1 << 11));
    assert_eq!(merge_and_diff(&two, &mut slots, &mut calib), 0);
    assert_eq!(calib, (1 << 0) | (1 << 11));
    let none = change_set(CHANGED_CALIB, 0, 0);
    assert_eq!(merge_and_diff(&none, &mut slots, &mut calib), 0);
    assert_eq!(calib, 0);
}

#[test]
fn merge_changes_the_other_fields_by_group() {
    let mut slots: u32 = 0;
    let mut calib: u16 = 0;
    let groups: [u16; 6] = [
        CHANGED_MOVEMENTS,
        CHANGED_MOTOR,
        CHANGED_ESCALATION,
        CHANGED_LEARN_TIME,
        CHANGED_LEASE,
        CHANGED_FAILSAFE,
    ];
    for g in groups {
        assert_eq!(
            merge_and_diff(&change_set(g, 0, 0), &mut slots, &mut calib),
            g,
            "group {g}"
        );
        assert_eq!(slots, 0, "group {g}");
        assert_eq!(calib, 0, "group {g}");
    }
    assert_eq!(
        merge_and_diff(&change_set(CHANGED_ALL, 0, 0), &mut slots, &mut calib),
        CHANGED_MOVEMENTS
            | CHANGED_MOTOR
            | CHANGED_ESCALATION
            | CHANGED_LEARN_TIME
            | CHANGED_LEASE
            | CHANGED_FAILSAFE
    );
}

#[test]
fn merge_changes_an_empty_set_and_bits_beyond_the_valves_copy_nothing() {
    let mut slots: u32 = 0;
    let mut calib: u16 = 0;
    assert_eq!(
        merge_and_diff(&change_set(0, 0, 0), &mut slots, &mut calib),
        0
    );
    assert_eq!(slots, 0);
    assert_eq!(calib, 0);
    let beyond = change_set(0, 0xFF00_0000, 0xF000);
    assert_eq!(merge_and_diff(&beyond, &mut slots, &mut calib), 0);
    assert_eq!(slots, 0);
    assert_eq!(calib, 0);
}

#[test]
fn blocks_for_the_blocks_that_hold_the_fields() {
    const LAYOUT_BA: u8 = BLOCK_LAYOUT | BLOCK_SAFETY | BLOCK_SETTINGS;
    assert_eq!(blocks_for(0), 0);
    assert_eq!(blocks_for(CHANGED_SENSORS), LAYOUT_BA);
    assert_eq!(blocks_for(CHANGED_MOVEMENTS), LAYOUT_BA);
    assert_eq!(blocks_for(CHANGED_MOTOR), LAYOUT_BA);
    assert_eq!(blocks_for(CHANGED_ESCALATION), BLOCK_SETTINGS);
    assert_eq!(blocks_for(CHANGED_LEARN_TIME), BLOCK_SETTINGS);
    assert_eq!(blocks_for(CHANGED_LEASE), BLOCK_SETTINGS | BLOCK_SAFETY);
    assert_eq!(blocks_for(CHANGED_FAILSAFE), BLOCK_SAFETY);
    assert_eq!(blocks_for(CHANGED_CALIB), BLOCK_CALIB);
    assert_eq!(
        blocks_for(CHANGED_ESCALATION | CHANGED_CALIB),
        BLOCK_SETTINGS | BLOCK_CALIB
    );
    assert_eq!(blocks_for(CHANGED_ALL), LAYOUT_BA | BLOCK_CALIB);
    assert_eq!(blocks_for(0xFF00), 0);
}

// Not in the C++ suite, which checks the rewrite of a layout repair only through blocks_for():
// pins the derived constant REWRITE_LAYOUT (the mutation gate mutates const items). The
// contract: LoadResult::rewrite of a layout repair holds the three groups of the 1.x fields.
#[test]
fn a_layout_repair_writes_back_the_1_x_fields() {
    let s = sample_stored();
    let mut raw = erased();
    write_raw(&s, &mut raw);
    raw.layout[LOW_FAC_OFFSET] = 35;
    let r = resolve(&raw);
    assert_eq!(r.cfg_flags, CFG_LAYOUT_CRC);
    assert_eq!(
        r.rewrite,
        CHANGED_SENSORS | CHANGED_MOVEMENTS | CHANGED_MOTOR
    );
}
