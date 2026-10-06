//! Port of test/native/test_config_blocks.cpp.

use super::*;
use crate::eeprom_layout::{CALIB_BLOCK_SIZE, SAFETY_BLOCK_SIZE};
use crate::onewire_check::crc8;

type SafetyRaw = [u8; SAFETY_BLOCK_SIZE];
type CalibRaw = [u8; CALIB_BLOCK_SIZE];

fn sample_safety() -> SafetyBlock {
    // memset(&b, 0, sizeof(b)), then the fields
    SafetyBlock {
        failsafe_pct: [0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 255],
        shadow: MotorShadow {
            low_fac: 12,
            high_fac: 34,
            movements: 0x0102,
            start_on_power: 56,
            min_counts: 0x0304,
            max_retries: 1,
        },
        lease_timeout_min: 1440, // 0x05A0
        lease_valid: false,
    }
}

/// memset(&b, fill, sizeof(b)): every byte of every field is `fill`; the bool, which holds
/// only 0 or 1 in Rust, is true like the non-zero C++ byte
fn safety_filled(fill: u8) -> SafetyBlock {
    let w = u16::from_le_bytes([fill, fill]);
    SafetyBlock {
        failsafe_pct: [fill; VALVE_COUNT as usize],
        shadow: MotorShadow {
            low_fac: fill,
            high_fac: fill,
            movements: w,
            start_on_power: fill,
            min_counts: w,
            max_retries: fill,
        },
        lease_timeout_min: w,
        lease_valid: fill != 0,
    }
}

fn is_safety_default(b: &SafetyBlock) -> bool {
    if b.failsafe_pct.iter().any(|&pct| pct != 50) {
        return false;
    }
    b.shadow.low_fac == 17
        && b.shadow.high_fac == 17
        && b.shadow.movements == 2000
        && b.shadow.start_on_power == 30
        && b.shadow.min_counts == 3000
        && b.shadow.max_retries == 2
        && b.lease_timeout_min == 60
        && !b.lease_valid
}

/// rewrites version and payload length of an encoded block and seals it again
fn reseal(raw: &mut [u8], version: u8, length: u8) {
    raw[0] = version;
    raw[1] = length;
    let n = 2 + usize::from(length);
    raw[n] = crc8(&raw[..n]);
}

fn sample_calib() -> CalibRecord {
    CalibRecord {
        opening_count: 3567, // 0x0DEF
        closing_count: 3610, // 0x0E1A
        mean_current: 17,
        flags: CALIB_VALID,
    }
}

/// memset(&out, fill, sizeof(out))
fn calib_filled(fill: u8) -> CalibRecord {
    let w = u16::from_le_bytes([fill, fill]);
    CalibRecord {
        opening_count: w,
        closing_count: w,
        mean_current: w,
        flags: fill,
    }
}

fn is_no_record(r: &CalibRecord) -> bool {
    r.opening_count == 0 && r.closing_count == 0 && r.mean_current == 0 && r.flags == 0
}

fn decode_calib_with(
    opening: u16,
    closing: u16,
    mean: u16,
    flags: u8,
    out: &mut CalibRecord,
) -> BlockState {
    let input = CalibRecord {
        opening_count: opening,
        closing_count: closing,
        mean_current: mean,
        flags,
    };
    let mut raw: CalibRaw = [0; CALIB_BLOCK_SIZE];
    encode_calib(&input, 5, &mut raw);
    decode_calib(&raw, 5, out)
}

#[test]
fn block_b_version_1_payload_length_22() {
    assert_eq!(SAFETY_VERSION, 1);
    assert_eq!(SAFETY_PAYLOAD, 22);
}

#[test]
fn encode_safety_byte_layout() {
    let mut raw: SafetyRaw = [0; SAFETY_BLOCK_SIZE];
    let n = encode_safety(&sample_safety(), &mut raw);
    assert_eq!(n, 25);
    assert_eq!(raw[0], 1);
    assert_eq!(raw[1], 22);
    let fs: [u8; 12] = [0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 255];
    assert_eq!(&raw[2..14], &fs);
    assert_eq!(raw[14], 12);
    assert_eq!(raw[15], 34);
    assert_eq!(raw[16], 0x02);
    assert_eq!(raw[17], 0x01);
    assert_eq!(raw[18], 56);
    assert_eq!(raw[19], 0x04);
    assert_eq!(raw[20], 0x03);
    assert_eq!(raw[21], 1);
    assert_eq!(raw[22], 0xA0);
    assert_eq!(raw[23], 0x05);
    assert_eq!(raw[24], crc8(&raw[..24]));
    for (i, &b) in raw.iter().enumerate().skip(n) {
        assert_eq!(b, 0xFF, "byte {i}");
    }
}

#[test]
fn decode_safety_encode_safety_x_eq_x() {
    let mut raw: SafetyRaw = [0; SAFETY_BLOCK_SIZE];
    encode_safety(&sample_safety(), &mut raw);
    let mut b = safety_filled(0xAA);
    assert_eq!(decode_safety(&raw, &mut b), BlockState::Valid);
    let input = sample_safety();
    assert_eq!(b.failsafe_pct, input.failsafe_pct);
    assert_eq!(b.shadow.low_fac, 12);
    assert_eq!(b.shadow.high_fac, 34);
    assert_eq!(b.shadow.movements, 0x0102);
    assert_eq!(b.shadow.start_on_power, 56);
    assert_eq!(b.shadow.min_counts, 0x0304);
    assert_eq!(b.shadow.max_retries, 1);
    assert_eq!(b.lease_timeout_min, 1440);
    assert!(b.lease_valid);
}

#[test]
fn decode_safety_never_written_is_absent() {
    let mut raw: SafetyRaw = [0xFF; SAFETY_BLOCK_SIZE];
    let mut b = sample_safety();
    assert_eq!(decode_safety(&raw, &mut b), BlockState::Absent);
    assert!(is_safety_default(&b));
    // version byte erased, the rest written: absent as well
    encode_safety(&sample_safety(), &mut raw);
    raw[0] = 0xFF;
    b = sample_safety();
    assert_eq!(decode_safety(&raw, &mut b), BlockState::Absent);
    assert!(is_safety_default(&b));
}

#[test]
fn decode_safety_every_single_bit_flip_of_bytes_0_24_is_corrupt() {
    let mut good: SafetyRaw = [0; SAFETY_BLOCK_SIZE];
    encode_safety(&sample_safety(), &mut good);
    for byte in 0..25 {
        for bit in 0..8 {
            let mut raw = good;
            raw[byte] ^= 1 << bit;
            let mut b = sample_safety();
            assert_eq!(
                decode_safety(&raw, &mut b),
                BlockState::Corrupt,
                "byte {byte} bit {bit}"
            );
            assert!(is_safety_default(&b), "byte {byte} bit {bit}");
        }
    }
}

#[test]
fn decode_safety_version_0_and_payload_lengths_outside_22_29_are_corrupt() {
    let mut raw: SafetyRaw = [0; SAFETY_BLOCK_SIZE];
    let mut b = SafetyBlock::default();
    encode_safety(&sample_safety(), &mut raw);
    reseal(&mut raw, 0, 22);
    assert_eq!(decode_safety(&raw, &mut b), BlockState::Corrupt);
    encode_safety(&sample_safety(), &mut raw);
    reseal(&mut raw, 1, 21);
    assert_eq!(decode_safety(&raw, &mut b), BlockState::Corrupt);
    encode_safety(&sample_safety(), &mut raw);
    raw[1] = 30;
    assert_eq!(decode_safety(&raw, &mut b), BlockState::Corrupt);
    raw[1] = 0xFF;
    assert_eq!(decode_safety(&raw, &mut b), BlockState::Corrupt);
    assert!(is_safety_default(&b));
}

#[test]
fn decode_safety_a_later_version_with_a_longer_payload_is_read_by_its_prefix() {
    let mut raw: SafetyRaw = [0; SAFETY_BLOCK_SIZE];
    encode_safety(&sample_safety(), &mut raw);
    raw[24..31].fill(0x5A);
    reseal(&mut raw, 2, 29);
    let mut b = SafetyBlock::default();
    assert_eq!(decode_safety(&raw, &mut b), BlockState::Valid);
    assert_eq!(b.failsafe_pct[11], 255);
    assert_eq!(b.shadow.max_retries, 1);
    assert_eq!(b.lease_timeout_min, 1440);
    assert!(b.lease_valid);
}

#[test]
fn decode_safety_invalid_failsafe_positions_load_50_0_100_and_255_are_kept() {
    let mut input = sample_safety();
    input.failsafe_pct[0] = 101;
    input.failsafe_pct[1] = 254;
    input.failsafe_pct[2] = 0;
    input.failsafe_pct[3] = 100;
    input.failsafe_pct[4] = 255;
    let mut raw: SafetyRaw = [0; SAFETY_BLOCK_SIZE];
    encode_safety(&input, &mut raw);
    let mut b = SafetyBlock::default();
    assert_eq!(decode_safety(&raw, &mut b), BlockState::Valid);
    assert_eq!(b.failsafe_pct[0], 50);
    assert_eq!(b.failsafe_pct[1], 50);
    assert_eq!(b.failsafe_pct[2], 0);
    assert_eq!(b.failsafe_pct[3], 100);
    assert_eq!(b.failsafe_pct[4], 255);
    assert_eq!(b.failsafe_pct[5], 50);
    assert_eq!(b.failsafe_pct[10], 100);
}

#[test]
fn decode_safety_an_invalid_lease_copy_counts_as_missing() {
    let mut input = sample_safety();
    let mut raw: SafetyRaw = [0; SAFETY_BLOCK_SIZE];
    let mut b = SafetyBlock::default();
    input.lease_timeout_min = 4;
    encode_safety(&input, &mut raw);
    assert_eq!(decode_safety(&raw, &mut b), BlockState::Valid);
    assert!(!b.lease_valid);
    assert_eq!(b.lease_timeout_min, 60);
    assert_eq!(b.failsafe_pct[1], 10); // the block itself is used
    input.lease_timeout_min = 0xFFFF;
    encode_safety(&input, &mut raw);
    assert_eq!(decode_safety(&raw, &mut b), BlockState::Valid);
    assert!(!b.lease_valid);
    // 0 is the stored "off"
    input.lease_timeout_min = 0;
    encode_safety(&input, &mut raw);
    assert_eq!(decode_safety(&raw, &mut b), BlockState::Valid);
    assert!(b.lease_valid);
    assert_eq!(b.lease_timeout_min, 0);
}

#[test]
fn block_c_version_1_payload_length_9_flags() {
    assert_eq!(CALIB_VERSION, 1);
    assert_eq!(CALIB_PAYLOAD, 9);
    assert_eq!(CALIB_VALID, 0x01);
    assert_eq!(CALIB_FAILED, 0x02);
}

#[test]
fn encode_calib_byte_layout() {
    let mut raw: CalibRaw = [0; CALIB_BLOCK_SIZE];
    let n = encode_calib(&sample_calib(), 11, &mut raw);
    assert_eq!(n, 12);
    assert_eq!(raw[0], 1);
    assert_eq!(raw[1], 9);
    assert_eq!(raw[2], 0xEF);
    assert_eq!(raw[3], 0x0D);
    assert_eq!(raw[4], 0x1A);
    assert_eq!(raw[5], 0x0E);
    assert_eq!(raw[6], 17);
    assert_eq!(raw[7], 0);
    assert_eq!(raw[8], 0x01);
    assert_eq!(raw[9], 11);
    assert_eq!(raw[10], 0);
    assert_eq!(raw[11], crc8(&raw[..11]));
    for (i, &b) in raw.iter().enumerate().skip(n) {
        assert_eq!(b, 0xFF, "byte {i}");
    }
}

#[test]
fn decode_calib_encode_calib_x_eq_x_for_valves_0_and_11() {
    let valves: [u8; 2] = [0, 11];
    for v in valves {
        let mut input = sample_calib();
        input.flags = CALIB_VALID | CALIB_FAILED;
        input.mean_current = 300 + u16::from(v);
        let mut raw: CalibRaw = [0; CALIB_BLOCK_SIZE];
        encode_calib(&input, v, &mut raw);
        let mut out = calib_filled(0xAA);
        assert_eq!(
            decode_calib(&raw, v, &mut out),
            BlockState::Valid,
            "valve {v}"
        );
        assert_eq!(out.opening_count, 3567, "valve {v}");
        assert_eq!(out.closing_count, 3610, "valve {v}");
        assert_eq!(out.mean_current, 300 + u16::from(v), "valve {v}");
        assert_eq!(out.flags, 0x03, "valve {v}");
    }
}

#[test]
fn decode_calib_never_written_is_absent_no_record() {
    let raw: CalibRaw = [0xFF; CALIB_BLOCK_SIZE];
    let mut out = sample_calib();
    assert_eq!(decode_calib(&raw, 0, &mut out), BlockState::Absent);
    assert!(is_no_record(&out));
}

#[test]
fn decode_calib_a_record_of_another_valve_is_corrupt() {
    let mut raw: CalibRaw = [0; CALIB_BLOCK_SIZE];
    encode_calib(&sample_calib(), 3, &mut raw);
    let mut out = sample_calib();
    assert_eq!(decode_calib(&raw, 4, &mut out), BlockState::Corrupt);
    assert!(is_no_record(&out));
    encode_calib(&sample_calib(), 0, &mut raw);
    assert_eq!(decode_calib(&raw, 11, &mut out), BlockState::Corrupt);
    encode_calib(&sample_calib(), 11, &mut raw);
    assert_eq!(decode_calib(&raw, 0, &mut out), BlockState::Corrupt);
}

#[test]
fn decode_calib_every_single_bit_flip_of_bytes_0_11_is_corrupt() {
    let mut good: CalibRaw = [0; CALIB_BLOCK_SIZE];
    encode_calib(&sample_calib(), 7, &mut good);
    for byte in 0..12 {
        for bit in 0..8 {
            let mut raw = good;
            raw[byte] ^= 1 << bit;
            let mut out = sample_calib();
            assert_eq!(
                decode_calib(&raw, 7, &mut out),
                BlockState::Corrupt,
                "byte {byte} bit {bit}"
            );
            assert!(is_no_record(&out), "byte {byte} bit {bit}");
        }
    }
}

#[test]
fn decode_calib_version_0_and_payload_lengths_outside_9_13_are_corrupt() {
    let mut raw: CalibRaw = [0; CALIB_BLOCK_SIZE];
    let mut out = CalibRecord::default();
    encode_calib(&sample_calib(), 2, &mut raw);
    reseal(&mut raw, 0, 9);
    assert_eq!(decode_calib(&raw, 2, &mut out), BlockState::Corrupt);
    encode_calib(&sample_calib(), 2, &mut raw);
    reseal(&mut raw, 1, 8);
    assert_eq!(decode_calib(&raw, 2, &mut out), BlockState::Corrupt);
    encode_calib(&sample_calib(), 2, &mut raw);
    raw[1] = 14;
    assert_eq!(decode_calib(&raw, 2, &mut out), BlockState::Corrupt);
    // a later version with a longer payload: the known prefix is read
    encode_calib(&sample_calib(), 2, &mut raw);
    raw[11] = 0x5A;
    raw[12] = 0x5A;
    reseal(&mut raw, 2, 13);
    assert_eq!(decode_calib(&raw, 2, &mut out), BlockState::Valid);
    assert_eq!(out.opening_count, 3567);
    assert_eq!(out.flags, CALIB_VALID);
}

#[test]
fn decode_calib_counts_marked_valid_must_be_a_real_stroke() {
    let mut out = CalibRecord::default();
    assert_eq!(
        decode_calib_with(99, 3000, 17, CALIB_VALID, &mut out),
        BlockState::Corrupt
    );
    assert!(is_no_record(&out));
    assert_eq!(
        decode_calib_with(3000, 99, 17, CALIB_VALID, &mut out),
        BlockState::Corrupt
    );
    assert_eq!(
        decode_calib_with(100, 100, 17, CALIB_VALID, &mut out),
        BlockState::Valid
    );
    assert_eq!(out.opening_count, 100);
    assert_eq!(out.closing_count, 100);
    // a valve that never calibrated successfully has no counts, only the failure
    assert_eq!(
        decode_calib_with(0, 0, 20, CALIB_FAILED, &mut out),
        BlockState::Valid
    );
    assert_eq!(out.flags, CALIB_FAILED);
    assert_eq!(out.opening_count, 0);
}

#[test]
fn decode_calib_a_mean_current_of_0_or_above_1000_ma_loads_20() {
    let mut out = CalibRecord::default();
    assert_eq!(
        decode_calib_with(3000, 3000, 0, CALIB_VALID, &mut out),
        BlockState::Valid
    );
    assert_eq!(out.mean_current, 20);
    assert_eq!(
        decode_calib_with(3000, 3000, 1, CALIB_VALID, &mut out),
        BlockState::Valid
    );
    assert_eq!(out.mean_current, 1);
    assert_eq!(
        decode_calib_with(3000, 3000, 1000, CALIB_VALID, &mut out),
        BlockState::Valid
    );
    assert_eq!(out.mean_current, 1000);
    assert_eq!(
        decode_calib_with(3000, 3000, 1001, CALIB_VALID, &mut out),
        BlockState::Valid
    );
    assert_eq!(out.mean_current, 20);
    assert_eq!(
        decode_calib_with(3000, 3000, 0xFFFF, CALIB_VALID, &mut out),
        BlockState::Valid
    );
    assert_eq!(out.mean_current, 20);
}
