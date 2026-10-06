//! Port of test/native/test_eeprom_layout.cpp.

use super::*;
use crate::calibration::{sanitize_escalation, EscalationConfig, ESCALATION_DEFAULT};
use crate::onewire_check::crc8;

type Block = [u8; EXTENSION_BLOCK_SIZE];

fn is_default(e: &StoredExtension) -> bool {
    e.escalation.enable == ESCALATION_DEFAULT.enable
        && e.escalation.step_pct == ESCALATION_DEFAULT.step_pct
        && e.escalation.max_ma == ESCALATION_DEFAULT.max_ma
        && e.learn_time_s == 604_800
        && e.lease_timeout_min == 60
        && e.layout_crc == 0
        && !e.has_v3
}

fn sample() -> StoredExtension {
    StoredExtension {
        escalation: EscalationConfig {
            enable: 1,
            step_pct: 30,
            max_ma: 55,
        },
        learn_time_s: 0x1234_5678,
        lease_timeout_min: 1440, // 0x05A0
        layout_crc: 0xBEEF,
        has_v3: true,
    }
}

/// a block as firmware 2.0.0 wrote it: version 2, the escalation only
fn write_v2(raw: &mut Block, version: u8, length: u8) {
    raw.fill(0xFF);
    raw[0] = version;
    raw[1] = length;
    raw[2] = 1;
    raw[3] = 10;
    raw[4] = 20;
    let n = 2 + usize::from(length);
    for (i, b) in raw[..n].iter_mut().enumerate().skip(5) {
        *b = 0x30 + i as u8;
    }
    raw[n] = crc8(&raw[..n]);
}

/// The decode rule of firmware 2.0.0 (58632d6 lib/core/src/eeprom_layout.cpp), to
/// show that a downgrade keeps the escalation of a block written by this firmware.
fn decode200(raw: &Block, esc: &mut EscalationConfig) -> ExtensionState {
    *esc = ESCALATION_DEFAULT;
    let version = raw[0];
    if version < 2 || version == 0xFF {
        return ExtensionState::Legacy;
    }
    let length = usize::from(raw[1]);
    if !(3..=13).contains(&length) {
        return ExtensionState::Corrupt;
    }
    if crc8(&raw[..2 + length]) != raw[2 + length] {
        return ExtensionState::Corrupt;
    }
    let c = EscalationConfig {
        enable: raw[2],
        step_pct: raw[3],
        max_ma: raw[4],
    };
    *esc = sanitize_escalation(&c);
    ExtensionState::Valid
}

#[test]
fn eeprom_address_map_1_x_layout_blocks_a_b_and_c() {
    assert_eq!(LEGACY_LAYOUT_ADDRESS, 0x0007);
    assert_eq!(LEGACY_LAYOUT_SIZE, 309);
    // 1.x: last field maxCalibRetries at 0x013B
    assert_eq!(EXTENSION_ADDRESS, 0x013C);
    assert_eq!(EXTENSION_BLOCK_SIZE, 16);
    assert_eq!(SAFETY_BLOCK_ADDRESS, 0x0160);
    assert_eq!(SAFETY_BLOCK_SIZE, 32);
    assert_eq!(CALIB_BLOCK_ADDRESS, 0x0180);
    assert_eq!(CALIB_BLOCK_SIZE, 16);
    assert_eq!(CONFIG_END, 0x0240);
    // a comparison of constants: checked at compile time (clippy assertions_on_constants)
    const { assert!(CONFIG_END <= 8192) };
    assert_eq!(LAYOUT_VERSION, 3);
    assert_eq!(EXTENSION_PAYLOAD_V2, 3);
    assert_eq!(EXTENSION_PAYLOAD_V3, 11);
}

#[test]
fn decode_extension_1_x_image_erased_bytes_loads_defaults() {
    let raw: Block = [0xFF; EXTENSION_BLOCK_SIZE];
    let mut e = sample();
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Legacy);
    assert!(is_default(&e));
}

#[test]
fn decode_extension_all_zero_block_and_versions_below_2_are_legacy() {
    let mut raw: Block = [0x00; EXTENSION_BLOCK_SIZE];
    let mut e = sample();
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Legacy);
    assert!(is_default(&e));
    raw[0] = 1;
    e = sample();
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Legacy);
    assert!(is_default(&e));
}

#[test]
fn encode_extension_version_3_block() {
    let mut raw: Block = [0; EXTENSION_BLOCK_SIZE];
    let n = encode_extension(&sample(), &mut raw);
    assert_eq!(n, 14);
    assert_eq!(raw[0], 3);
    assert_eq!(raw[1], 11);
    assert_eq!(raw[2], 1);
    assert_eq!(raw[3], 30);
    assert_eq!(raw[4], 55);
    // learnTimeS, leaseTimeoutMin and layoutCrc, little endian
    assert_eq!(raw[5], 0x78);
    assert_eq!(raw[6], 0x56);
    assert_eq!(raw[7], 0x34);
    assert_eq!(raw[8], 0x12);
    assert_eq!(raw[9], 0xA0);
    assert_eq!(raw[10], 0x05);
    assert_eq!(raw[11], 0xEF);
    assert_eq!(raw[12], 0xBE);
    assert_eq!(raw[13], crc8(&raw[..13]));
    for (i, &b) in raw.iter().enumerate().skip(n) {
        assert_eq!(b, 0xFF, "byte {i}");
    }
}

#[test]
fn encode_decode_round_trip() {
    let mut raw: Block = [0; EXTENSION_BLOCK_SIZE];
    encode_extension(&sample(), &mut raw);
    let mut e = StoredExtension::default();
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Valid);
    assert_eq!(e.escalation.enable, 1);
    assert_eq!(e.escalation.step_pct, 30);
    assert_eq!(e.escalation.max_ma, 55);
    assert_eq!(e.learn_time_s, 0x1234_5678);
    assert_eq!(e.lease_timeout_min, 1440);
    assert_eq!(e.layout_crc, 0xBEEF);
    assert!(e.has_v3);

    // time trigger and lease off are stored values, not defaults
    let mut off = sample();
    off.learn_time_s = 0;
    off.lease_timeout_min = 0;
    off.layout_crc = 0;
    encode_extension(&off, &mut raw);
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Valid);
    assert_eq!(e.learn_time_s, 0);
    assert_eq!(e.lease_timeout_min, 0);
    assert_eq!(e.layout_crc, 0);
    assert!(e.has_v3);
}

#[test]
fn decode_extension_any_flipped_bit_is_detected() {
    let mut good: Block = [0; EXTENSION_BLOCK_SIZE];
    let n = encode_extension(&sample(), &mut good);
    for byte in 0..n {
        for bit in 0..8 {
            let mut raw = good;
            raw[byte] ^= 1 << bit;
            let mut e = sample();
            let s = decode_extension(&raw, &mut e);
            assert_ne!(s, ExtensionState::Valid, "byte {byte} bit {bit}");
            assert!(is_default(&e), "byte {byte} bit {bit}");
        }
    }
}

#[test]
fn decode_extension_bad_payload_length_is_corrupt() {
    let mut raw: Block = [0; EXTENSION_BLOCK_SIZE];
    encode_extension(&sample(), &mut raw);
    let mut e = StoredExtension::default();
    raw[1] = 2;
    raw[4] = crc8(&raw[..4]);
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Corrupt);
    raw[1] = (EXTENSION_MAX_PAYLOAD + 1) as u8;
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Corrupt);
    raw[1] = 0xFF;
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Corrupt);
    assert!(is_default(&e));
}

#[test]
fn decode_extension_a_2_0_0_block_keeps_the_escalation_the_version_3_fields_are_defaults() {
    let mut raw: Block = [0; EXTENSION_BLOCK_SIZE];
    write_v2(&mut raw, 2, 3);
    let mut e = sample();
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Valid);
    assert_eq!(e.escalation.enable, 1);
    assert_eq!(e.escalation.step_pct, 10);
    assert_eq!(e.escalation.max_ma, 20);
    assert!(!e.has_v3);
    assert_eq!(e.learn_time_s, 604_800);
    assert_eq!(e.lease_timeout_min, 60);
    assert_eq!(e.layout_crc, 0);
}

#[test]
fn decode_extension_the_version_not_the_length_selects_the_version_3_fields() {
    let mut raw: Block = [0; EXTENSION_BLOCK_SIZE];
    let mut e = StoredExtension::default();
    // version 3 with the payload of version 2: read like version 2
    write_v2(&mut raw, 3, 3);
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Valid);
    assert_eq!(e.escalation.step_pct, 10);
    assert!(!e.has_v3);
    assert_eq!(e.learn_time_s, 604_800);
    // version 3 one byte short of the version 3 fields
    write_v2(&mut raw, 3, 10);
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Valid);
    assert!(!e.has_v3);
    // version 2 with a payload as long as version 3: the extra bytes are not version 3 fields
    write_v2(&mut raw, 2, 11);
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Valid);
    assert_eq!(e.escalation.max_ma, 20);
    assert!(!e.has_v3);
    assert_eq!(e.learn_time_s, 604_800);
    assert_eq!(e.lease_timeout_min, 60);
}

#[test]
fn decode_extension_a_newer_layout_with_a_longer_payload_is_read_by_its_prefix() {
    let mut raw: Block = [0; EXTENSION_BLOCK_SIZE];
    encode_extension(&sample(), &mut raw);
    raw[0] = 4;
    raw[1] = EXTENSION_MAX_PAYLOAD as u8;
    raw[13] = 0x5A;
    raw[14] = 0x5A;
    raw[2 + EXTENSION_MAX_PAYLOAD] = crc8(&raw[..2 + EXTENSION_MAX_PAYLOAD]);
    let mut e = StoredExtension::default();
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Valid);
    assert_eq!(e.escalation.enable, 1);
    assert_eq!(e.escalation.step_pct, 30);
    assert_eq!(e.escalation.max_ma, 55);
    assert_eq!(e.learn_time_s, 0x1234_5678);
    assert_eq!(e.lease_timeout_min, 1440);
    assert_eq!(e.layout_crc, 0xBEEF);
    assert!(e.has_v3);
}

#[test]
fn decode_extension_out_of_range_values_in_a_valid_block_fall_back_to_defaults() {
    let mut bad = sample();
    bad.escalation = EscalationConfig {
        enable: 1,
        step_pct: 25,
        max_ma: 61,
    };
    bad.lease_timeout_min = 4;
    let mut raw: Block = [0; EXTENSION_BLOCK_SIZE];
    encode_extension(&bad, &mut raw);
    let mut e = StoredExtension::default();
    assert_eq!(decode_extension(&raw, &mut e), ExtensionState::Valid);
    assert_eq!(e.escalation.enable, ESCALATION_DEFAULT.enable);
    assert_eq!(e.escalation.step_pct, ESCALATION_DEFAULT.step_pct);
    assert_eq!(e.escalation.max_ma, ESCALATION_DEFAULT.max_ma);
    assert_eq!(e.lease_timeout_min, 60);
    // the other fields of the block are still taken
    assert_eq!(e.learn_time_s, 0x1234_5678);
    assert_eq!(e.layout_crc, 0xBEEF);
    assert!(e.has_v3);
}

#[test]
fn a_version_3_block_keeps_the_escalation_after_a_downgrade_to_2_0_0() {
    let mut raw: Block = [0; EXTENSION_BLOCK_SIZE];
    encode_extension(&sample(), &mut raw);
    let mut esc = EscalationConfig::default();
    assert_eq!(decode200(&raw, &mut esc), ExtensionState::Valid);
    assert_eq!(esc.enable, 1);
    assert_eq!(esc.step_pct, 30);
    assert_eq!(esc.max_ma, 55);
}
