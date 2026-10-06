//! Port of test/native/test_target_store.cpp: desired-target persistence: record codec, boot
//! choice, capture/restore and the debounced NVS saver.

use super::*;
use crate::stm_codec::TargetReply;
use crate::valve_model::TargetSync;

/// valve 0: 42 from MQTT, valve 11: 100 from an assembly, the rest invalid.
const GOLDEN: [u8; PERSISTED_TARGETS_SIZE] = [
    0x56, 0x44, 0x54, 0x47, 0x01, 0x0C, 0x01, 0x2A, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x64, 0x05, 0x44, 0x80, 0xBC, 0x76,
];

fn golden() -> PersistedTargets {
    let mut t = PersistedTargets::default();
    t.valid[0] = true;
    t.pos[0] = 42;
    t.source[0] = TargetSource::Mqtt;
    t.valid[11] = true;
    t.pos[11] = 100;
    t.source[11] = TargetSource::Assembly;
    t
}

/// Rewrites the CRC after a deliberate change.
fn fix_crc(b: &mut [u8; PERSISTED_TARGETS_SIZE]) {
    let crc = crc32(&b[..42], 0);
    b[42..].copy_from_slice(&crc.to_le_bytes());
}

fn all_invalid(t: &PersistedTargets) -> bool {
    (0..12).all(|v| !t.valid[v] && t.pos[v] == 0 && t.source[v] == TargetSource::None)
}

/// A decoded record (C++ `decodeTargets(b, sizeof b, out)` with its result).
fn decode(b: &[u8]) -> (bool, PersistedTargets) {
    let mut out = golden(); // overwritten in every case
    let ok = decode_targets(b, &mut out);
    (ok, out)
}

#[test]
fn targets_golden_encoding_and_round_trip() {
    let mut out = [0xEE; PERSISTED_TARGETS_SIZE];
    assert_eq!(encode_targets(&golden(), &mut out), PERSISTED_TARGETS_SIZE);
    assert_eq!(out, GOLDEN);
    let (ok, back) = decode(&GOLDEN);
    assert!(ok);
    assert_eq!(back, golden());
    assert_eq!(back.pos[0], 42);
    assert_eq!(back.source[11], TargetSource::Assembly);
    assert!(!back.valid[5]);
    // Every valve, every source, the boundary positions.
    let mut full = PersistedTargets::default();
    for v in 0..12u8 {
        let i = usize::from(v);
        full.valid[i] = true;
        full.pos[i] = v * 9 + 1;
        full.source[i] = TargetSource::from_raw(v % 6).unwrap();
    }
    full.pos[0] = 0;
    full.pos[1] = 100;
    encode_targets(&full, &mut out);
    let (ok, back) = decode(&out);
    assert!(ok);
    assert_eq!(back, full);
    assert_eq!(PERSISTED_TARGETS_SIZE, 46);
}

#[test]
fn targets_invalid_entries_are_written_as_zeros_whatever_they_hold() {
    let mut t = golden();
    t.pos[5] = 77;
    t.source[5] = TargetSource::Web;
    let mut out = [0u8; PERSISTED_TARGETS_SIZE];
    encode_targets(&t, &mut out);
    assert_eq!(out, GOLDEN);
    assert!(t == golden()); // invalid entries do not compare
    let mut u = golden();
    u.pos[0] = 43;
    assert!(!(u == golden()));
    assert!(u != golden());
    u = golden();
    u.source[0] = TargetSource::Web;
    assert!(!(u == golden()));
    u = golden();
    u.valid[3] = true;
    assert!(!(u == golden()));
}

#[test]
fn targets_every_single_bit_flip_is_rejected_and_clears_the_output() {
    for byte in 0..PERSISTED_TARGETS_SIZE {
        for bit in 0..8 {
            let mut b = GOLDEN;
            b[byte] ^= 1 << bit;
            let (ok, out) = decode(&b);
            assert!(!ok, "byte {byte} bit {bit}");
            assert!(all_invalid(&out), "byte {byte} bit {bit}");
        }
    }
}

#[test]
fn targets_wrong_length_bad_fields_with_a_correct_crc() {
    assert!(!decode(&GOLDEN[..45]).0);
    let mut longer = [0u8; 47];
    longer[..46].copy_from_slice(&GOLDEN);
    assert!(!decode(&longer).0);
    // C++ decodeTargets(nullptr, 46, out): no Rust form.
    let mut b = GOLDEN;
    b[7] = 101; // pos of valve 0
    fix_crc(&mut b);
    assert!(!decode(&b).0);
    b = GOLDEN;
    b[7] = 100;
    fix_crc(&mut b);
    assert!(decode(&b).0);
    b = GOLDEN;
    b[8] = 6; // source above Assembly
    fix_crc(&mut b);
    assert!(!decode(&b).0);
    b = GOLDEN;
    b[6] = 3; // unknown flag bit
    fix_crc(&mut b);
    assert!(!decode(&b).0);
    b = GOLDEN;
    b[4] = 2; // version
    fix_crc(&mut b);
    assert!(!decode(&b).0);
    b = GOLDEN;
    b[5] = 11; // count
    fix_crc(&mut b);
    assert!(!decode(&b).0);
    b = GOLDEN;
    b[3] = b'H'; // magic
    fix_crc(&mut b);
    assert!(!decode(&b).0);
    b = GOLDEN;
    b[0] = b'X';
    fix_crc(&mut b);
    assert!(!decode(&b).0);
    // An invalid entry (flag byte 0) may carry any value bytes: they are ignored.
    b = GOLDEN;
    b[10] = 200;
    b[11] = 9;
    fix_crc(&mut b);
    let (ok, out) = decode(&b);
    assert!(ok);
    assert!(!out.valid[1]);
    assert_eq!(out.pos[1], 0);
}

#[test]
fn targets_the_rtc_copy_wins_else_nvs_else_none() {
    let mut other = [0u8; PERSISTED_TARGETS_SIZE];
    let mut t = PersistedTargets::default();
    t.valid[4] = true;
    t.pos[4] = 7;
    t.source[4] = TargetSource::Web;
    encode_targets(&t, &mut other);
    let garbage = [0xA5; PERSISTED_TARGETS_SIZE];
    let mut out = PersistedTargets::default();
    assert_eq!(
        choose_targets(&GOLDEN, &other, &mut out),
        RestoreSource::Rtc
    );
    assert_eq!(out, golden());
    assert_eq!(
        choose_targets(&garbage, &other, &mut out),
        RestoreSource::Nvs
    );
    assert_eq!(out, t);
    assert_eq!(
        choose_targets(&garbage, &other[..0], &mut out),
        RestoreSource::None
    );
    assert!(all_invalid(&out));
    // C++ chooseTargets(nullptr, 0, nullptr, 0, out): the empty slices.
    out = golden();
    assert_eq!(choose_targets(&[], &[], &mut out), RestoreSource::None);
    assert!(all_invalid(&out));
}

#[test]
fn targets_capture_and_restore_through_the_model() {
    let mut m = ValveModel::default();
    m.set_active_mask(0x00D); // valves 0, 2, 3
    m.apply_target(
        &TargetReply {
            valve: 0,
            target: 30,
        },
        0,
    ); // adopted: source Stm
    assert!(m.set_desired_target(2, 40, TargetSource::Mqtt, 0));
    m.set_assembly(3, 0);
    let mut t = PersistedTargets::default();
    t.valid[7] = true; // cleared by the capture
    capture_targets(&m, &mut t);
    assert!(t.valid[0]);
    assert_eq!(t.pos[0], 30);
    assert_eq!(t.source[0], TargetSource::Stm);
    assert!(t.valid[2]);
    assert_eq!(t.source[2], TargetSource::Mqtt);
    assert_eq!(t.pos[3], 100);
    assert_eq!(t.source[3], TargetSource::Assembly);
    assert!(!t.valid[1]);
    assert!(!t.valid[7]);

    t.valid[1] = true; // valve 1 is inactive in the new model
    t.pos[1] = 5;
    let mut n = ValveModel::default();
    n.set_active_mask(0x00D);
    assert_eq!(restore_targets(&mut n, &t), 3);
    assert_eq!(n.valve(0).source, TargetSource::Restored);
    assert_eq!(n.valve(2).source, TargetSource::Restored);
    assert_eq!(n.valve(3).source, TargetSource::Assembly);
    assert_eq!(n.valve(2).desired, 40);
    assert_eq!(n.valve(2).sync, TargetSync::Pending);
    assert!(!n.valve(1).desired_valid);
    assert_eq!(restore_targets(&mut n, &PersistedTargets::default()), 0);
}

#[test]
fn restore_source_values() {
    assert_eq!(RestoreSource::from_raw(0), Some(RestoreSource::None));
    assert_eq!(RestoreSource::from_raw(1), Some(RestoreSource::Rtc));
    assert_eq!(RestoreSource::from_raw(2), Some(RestoreSource::Nvs));
    assert_eq!(RestoreSource::from_raw(3), None);
    assert_eq!(RestoreSource::Nvs as u8, 2);
}

// ================================================================ saver

#[test]
fn saver_5_min_after_one_change() {
    let mut s = TargetSaver::default();
    s.prime_stored(&PersistedTargets::default());
    assert!(!s.dirty());
    s.update(&golden(), 1000);
    assert!(s.dirty());
    assert!(!s.due(1000 + 299999));
    assert!(s.due(1000 + 300000));
    assert_eq!(*s.bytes(), GOLDEN);
    s.saved(true, 301000);
    assert!(!s.dirty());
    assert!(!s.due(10000000));
    s.update(&golden(), 400000); // equal to the stored copy: never dirty
    assert!(!s.dirty());
    assert_eq!(TargetSaver::DEBOUNCE_MS, 300000);
    assert_eq!(TargetSaver::MAX_DELAY_MS, 1800000);
}

#[test]
fn saver_a_change_every_minute_is_written_at_most_30_min_after_the_first() {
    let mut s = TargetSaver::default();
    s.prime_stored(&PersistedTargets::default());
    let mut t = PersistedTargets::default();
    t.valid[0] = true;
    let t0 = 5000;
    for k in 0..30u32 {
        t.pos[0] = (k + 1) as u8;
        s.update(&t, t0 + k * 60000);
        assert!(!s.due(t0 + k * 60000 + 59999), "{k}");
    }
    assert!(!s.due(t0 + 1799999));
    assert!(s.due(t0 + 1800000));
}

#[test]
fn saver_a_change_back_to_the_stored_value_is_clean_again() {
    let mut s = TargetSaver::default();
    s.prime_stored(&golden());
    assert_eq!(*s.bytes(), GOLDEN);
    s.update(&golden(), 0);
    assert!(!s.dirty());
    s.update(&PersistedTargets::default(), 100);
    assert!(s.dirty());
    s.update(&golden(), 200);
    assert!(!s.dirty());
    assert!(!s.due(10000000));
    assert_eq!(*s.bytes(), GOLDEN);
}

#[test]
fn saver_a_failed_write_is_retried_one_debounce_later() {
    let mut s = TargetSaver::default();
    s.prime_stored(&PersistedTargets::default());
    s.update(&golden(), 0);
    assert!(s.due(300000));
    s.saved(false, 400000);
    assert!(s.dirty());
    assert!(!s.due(400000 + 299999));
    assert!(s.due(400000 + 300000));
    s.saved(true, 700000);
    assert!(!s.dirty());
    // The stored copy is the saved value now.
    s.update(&golden(), 800000);
    assert!(!s.dirty());
}

#[test]
fn saver_the_first_change_time_is_kept_while_dirty() {
    let mut s = TargetSaver::default();
    s.prime_stored(&PersistedTargets::default());
    let mut t = PersistedTargets::default();
    t.valid[0] = true;
    t.pos[0] = 1;
    s.update(&t, 1000);
    t.pos[0] = 2;
    s.update(&t, 1000 + 1600000);
    assert!(!s.due(1000 + 1799999));
    assert!(s.due(1000 + 1800000));
    s.saved(true, 1000 + 1800000);
    t.pos[0] = 3;
    s.update(&t, 2000000); // a new dirty period starts its own maximum
    assert!(!s.due(2000000 + 299999));
    assert!(s.due(2000000 + 300000));
}
