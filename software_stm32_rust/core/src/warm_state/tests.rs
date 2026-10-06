//! Port of test/native/test_warm_state.cpp. The C++ reads and changes the raw bytes of the
//! record (memset, reinterpret_cast); here the little-endian bytes of the C++ layout come from
//! `state_bytes` and `state_from_bytes`, placed by `offset_of!` where the fields lie in memory.

use super::*;
use core::mem::{offset_of, size_of};

const STATE_SIZE: usize = size_of::<WarmState>();
const VALVE_SIZE: usize = size_of::<WarmValve>();

fn put(b: &mut [u8], at: usize, v: &[u8]) {
    b[at..at + v.len()].copy_from_slice(v);
}

fn get<const N: usize>(b: &[u8], at: usize) -> [u8; N] {
    let mut v = [0u8; N];
    v.copy_from_slice(&b[at..at + N]);
    v
}

/// offset of valves[v] in the record
fn valve_at(v: usize) -> usize {
    offset_of!(WarmState, valves) + v * VALVE_SIZE
}

/// The record as the STM32 holds it in RAM (padding bytes after the CRC 0).
fn state_bytes(s: &WarmState) -> [u8; STATE_SIZE] {
    let mut b = [0u8; STATE_SIZE];
    put(&mut b, offset_of!(WarmState, magic), &s.magic.to_le_bytes());
    put(&mut b, offset_of!(WarmState, version), &[s.version]);
    put(&mut b, offset_of!(WarmState, count), &[s.count]);
    put(
        &mut b,
        offset_of!(WarmState, reserved),
        &s.reserved.to_le_bytes(),
    );
    for (v, w) in s.valves.iter().enumerate() {
        let at = valve_at(v);
        put(&mut b, at + offset_of!(WarmValve, actual), &[w.actual]);
        put(&mut b, at + offset_of!(WarmValve, target), &[w.target]);
        put(&mut b, at + offset_of!(WarmValve, status), &[w.status]);
        put(&mut b, at + offset_of!(WarmValve, flags), &[w.flags]);
        put(
            &mut b,
            at + offset_of!(WarmValve, retry_attempts),
            &[w.retry_attempts],
        );
        put(
            &mut b,
            at + offset_of!(WarmValve, retry_scheduled),
            &[w.retry_scheduled],
        );
        put(&mut b, at + offset_of!(WarmValve, pad), &w.pad);
        put(
            &mut b,
            at + offset_of!(WarmValve, retry_remaining_s),
            &w.retry_remaining_s.to_le_bytes(),
        );
    }
    put(
        &mut b,
        offset_of!(WarmState, lease_since_renewal_s),
        &s.lease_since_renewal_s.to_le_bytes(),
    );
    put(
        &mut b,
        offset_of!(WarmState, lease_since_client_s),
        &s.lease_since_client_s.to_le_bytes(),
    );
    put(
        &mut b,
        offset_of!(WarmState, lease_client),
        &[s.lease_client],
    );
    put(&mut b, offset_of!(WarmState, pad), &[s.pad]);
    put(
        &mut b,
        offset_of!(WarmState, lease_timeout_min),
        &s.lease_timeout_min.to_le_bytes(),
    );
    put(&mut b, offset_of!(WarmState, failsafe_pct), &s.failsafe_pct);
    put(&mut b, offset_of!(WarmState, crc), &s.crc.to_le_bytes());
    b
}

/// The record found in RAM holding these bytes.
fn state_from_bytes(b: &[u8; STATE_SIZE]) -> WarmState {
    let mut s = WarmState {
        magic: u32::from_le_bytes(get(b, offset_of!(WarmState, magic))),
        version: b[offset_of!(WarmState, version)],
        count: b[offset_of!(WarmState, count)],
        reserved: u16::from_le_bytes(get(b, offset_of!(WarmState, reserved))),
        valves: [WarmValve::default(); VALVE_COUNT as usize],
        lease_since_renewal_s: u32::from_le_bytes(get(
            b,
            offset_of!(WarmState, lease_since_renewal_s),
        )),
        lease_since_client_s: u32::from_le_bytes(get(
            b,
            offset_of!(WarmState, lease_since_client_s),
        )),
        lease_client: b[offset_of!(WarmState, lease_client)],
        pad: b[offset_of!(WarmState, pad)],
        lease_timeout_min: u16::from_le_bytes(get(b, offset_of!(WarmState, lease_timeout_min))),
        failsafe_pct: get(b, offset_of!(WarmState, failsafe_pct)),
        crc: u16::from_le_bytes(get(b, offset_of!(WarmState, crc))),
    };
    for (v, w) in s.valves.iter_mut().enumerate() {
        let at = valve_at(v);
        *w = WarmValve {
            actual: b[at + offset_of!(WarmValve, actual)],
            target: b[at + offset_of!(WarmValve, target)],
            status: b[at + offset_of!(WarmValve, status)],
            flags: b[at + offset_of!(WarmValve, flags)],
            retry_attempts: b[at + offset_of!(WarmValve, retry_attempts)],
            retry_scheduled: b[at + offset_of!(WarmValve, retry_scheduled)],
            pad: get(b, at + offset_of!(WarmValve, pad)),
            retry_remaining_s: u32::from_le_bytes(get(
                b,
                at + offset_of!(WarmValve, retry_remaining_s),
            )),
        };
    }
    s
}

/// C++ crc16Ccitt(reinterpret_cast<const uint8_t*>(&s), 176)
fn raw_crc_176(s: &WarmState) -> u16 {
    crc16_ccitt(&state_bytes(s)[..176])
}

fn sample() -> WarmState {
    // C++ memset(&s, 0, sizeof s): every field 0
    let mut s = WarmState::default();
    for v in 0..VALVE_COUNT {
        let i = usize::from(v);
        s.valves[i].actual = v * 8;
        s.valves[i].target = 100 - v;
        s.valves[i].status = 1;
        s.valves[i].flags = WARM_POS_VALID;
        s.failsafe_pct[i] = 50;
    }
    s.lease_since_renewal_s = 1234;
    s.lease_since_client_s = 56;
    s.lease_client = 1;
    s.lease_timeout_min = 60;
    warm_state_seal(&mut s);
    s
}

fn valve(actual: u8, target: u8, status: u8, flags: u8) -> WarmValve {
    // C++ memset(&w, 0, sizeof w), then the four fields
    WarmValve {
        actual,
        target,
        status,
        flags,
        ..WarmValve::default()
    }
}

#[test]
fn constants_of_the_layout() {
    assert_eq!(WARM_STATE_MAGIC, 0x5644_5753);
    assert_eq!(WARM_STATE_VERSION, 1);
    assert_eq!(WARM_POS_VALID, 1);
    assert_eq!(WARM_ASSEMBLY_HOLD, 2);
    assert_eq!(WARM_NEEDS_REFERENCE, 4);
    assert_eq!(WARM_RECAL, 8);
    assert_eq!(size_of::<WarmState>(), 180);
}

#[test]
fn warm_state_seal_magic_version_count_reserved_and_a_crc_that_warm_state_valid_accepts() {
    let mut s = sample();
    assert_eq!(s.magic, WARM_STATE_MAGIC);
    assert_eq!(s.version, 1);
    assert_eq!(s.count, 12);
    assert_eq!(s.reserved, 0);
    assert_eq!(s.crc, raw_crc_176(&s));
    assert!(warm_state_valid(&s));
    s.reserved = 7;
    warm_state_seal(&mut s);
    assert_eq!(s.reserved, 0);
    assert!(warm_state_valid(&s));
}

#[test]
fn warm_state_valid_any_changed_byte_before_the_crc_and_the_crc_itself_invalidate_the_record() {
    let good = sample();
    for i in 0..178 {
        let mut b = state_bytes(&good);
        b[i] ^= 0x01;
        let s = state_from_bytes(&b);
        assert!(!warm_state_valid(&s), "i {i}");
    }
}

#[test]
fn warm_state_valid_wrong_magic_version_or_count_with_a_matching_crc_are_invalid() {
    let mut s = sample();
    s.magic = 0x5644_5752;
    s.crc = raw_crc_176(&s);
    assert!(!warm_state_valid(&s));
    s = sample();
    s.version = 2;
    s.crc = raw_crc_176(&s);
    assert!(!warm_state_valid(&s));
    s = sample();
    s.count = 11;
    s.crc = raw_crc_176(&s);
    assert!(!warm_state_valid(&s));
    // C++ memset(&erased, 0xA5, sizeof erased)
    let erased = state_from_bytes(&[0xA5; STATE_SIZE]);
    assert!(!warm_state_valid(&erased));
}

#[test]
fn is_warm_boot_pin_software_watchdogs_and_low_power_are_warm() {
    assert!(!is_warm_boot(BootReason::Unknown));
    assert!(!is_warm_boot(BootReason::PowerOn));
    assert!(is_warm_boot(BootReason::Pin));
    assert!(is_warm_boot(BootReason::Software));
    assert!(is_warm_boot(BootReason::IndependentWatchdog));
    assert!(is_warm_boot(BootReason::WindowWatchdog));
    assert!(is_warm_boot(BootReason::LowPower));
    assert!(!is_warm_boot(BootReason::BrownOut));
}

#[test]
fn restore_valve_statuses_1_4_9_are_kept_with_a_valid_position() {
    let kept: [u8; 7] = [1, 4, 5, 6, 7, 8, 9];
    for st in kept {
        let r = restore_valve(&valve(30, 70, st, WARM_POS_VALID), true);
        assert!(r.valid, "st {st}");
        assert_eq!(r.status, st, "st {st}");
        assert_eq!(r.actual, 30, "st {st}");
        assert_eq!(r.target, 70, "st {st}");
        assert!(!r.needs_reference, "st {st}");
        assert!(!r.recal, "st {st}");
        assert!(!r.assembly_hold, "st {st}");
    }
}

#[test]
fn restore_valve_a_moving_valve_or_an_invalid_position_is_referenced_again_or_tested() {
    let moving: [u8; 2] = [2, 3];
    for st in moving {
        let mut r = restore_valve(&valve(30, 70, st, WARM_POS_VALID), true);
        assert!(r.valid, "st {st}");
        assert_eq!(r.status, 1, "st {st}");
        assert!(r.needs_reference, "st {st}");
        r = restore_valve(&valve(30, 70, st, WARM_POS_VALID), false);
        assert_eq!(r.status, 5, "st {st}");
        assert!(!r.needs_reference, "st {st}");
    }
    let mut r = restore_valve(&valve(30, 70, 1, 0), true);
    assert_eq!(r.status, 1);
    assert!(r.needs_reference);
    assert_eq!(r.actual, 30);
    r = restore_valve(&valve(30, 70, 9, 0), false);
    assert_eq!(r.status, 5);
    assert!(!r.needs_reference);
    r = restore_valve(&valve(30, 70, 9, WARM_NEEDS_REFERENCE), false);
    assert_eq!(r.status, 5);
    assert!(r.needs_reference);
}

#[test]
fn restore_valve_flags() {
    let r = restore_valve(
        &valve(
            0,
            100,
            1,
            WARM_POS_VALID | WARM_ASSEMBLY_HOLD | WARM_NEEDS_REFERENCE | WARM_RECAL,
        ),
        true,
    );
    assert!(r.valid);
    assert!(r.needs_reference);
    assert!(r.recal);
    assert!(r.assembly_hold);
    assert_eq!(r.actual, 0);
    assert_eq!(r.target, 100);
    let a = restore_valve(&valve(0, 100, 1, WARM_POS_VALID | WARM_ASSEMBLY_HOLD), true);
    assert!(a.assembly_hold);
    assert!(!a.recal);
    assert!(!a.needs_reference);
    let b = restore_valve(&valve(0, 100, 8, WARM_POS_VALID | WARM_RECAL), false);
    assert!(b.recal);
    assert!(!b.assembly_hold);
}

#[test]
fn restore_valve_every_field_out_of_range_sends_the_valve_down_the_cold_path() {
    assert!(!restore_valve(&valve(101, 50, 1, 1), true).valid);
    assert!(!restore_valve(&valve(50, 101, 1, 1), true).valid);
    assert!(!restore_valve(&valve(50, 50, 0, 1), true).valid);
    assert!(!restore_valve(&valve(50, 50, 10, 1), true).valid);
    assert!(!restore_valve(&valve(50, 50, 1, 0x10), true).valid);
    assert!(!restore_valve(&valve(50, 50, 1, 0x80), true).valid);
    assert!(restore_valve(&valve(100, 100, 9, 0x0F), true).valid);
    let mut w = valve(50, 50, 1, 1);
    w.retry_scheduled = 2;
    assert!(!restore_valve(&w, true).valid);
    w.retry_scheduled = 1;
    w.retry_remaining_s = 86401;
    assert!(!restore_valve(&w, true).valid);
    w.retry_remaining_s = 86400;
    assert!(restore_valve(&w, true).valid);
    let bad = restore_valve(&valve(101, 50, 1, 1), true);
    assert!(!bad.needs_reference);
    assert_eq!(bad.status, 0);
}
