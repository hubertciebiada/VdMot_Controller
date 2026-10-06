//! Port of test/native/test_settings.cpp.

use super::*;

#[test]
fn learn_movements_from_request_values_in_range_are_kept_0_disables() {
    assert_eq!(learn_movements_from_request(0), 0);
    assert_eq!(learn_movements_from_request(50), 50);
    assert_eq!(learn_movements_from_request(2000), 2000);
    assert_eq!(learn_movements_from_request(65533), 65533);
    assert_eq!(learn_movements_from_request(65534), 65534);
}

#[test]
fn learn_movements_from_request_1_to_49_are_raised_to_50() {
    assert_eq!(MIN_LEARN_MOVEMENTS, 50);
    assert_eq!(learn_movements_from_request(1), 50);
    assert_eq!(learn_movements_from_request(49), 50);
}

#[test]
fn learn_movements_from_request_larger_values_are_capped_never_wrapped() {
    assert_eq!(MAX_LEARN_MOVEMENTS, 65534);
    assert_eq!(learn_movements_from_request(65535), 65534);
    assert_eq!(learn_movements_from_request(65536), 65534); // would wrap to 0
    assert_eq!(learn_movements_from_request(70000), 65534); // would wrap to 4464
    assert_eq!(learn_movements_from_request(u32::MAX), 65534);
}

#[test]
fn sanitize_learn_movements_0_and_50_to_65534_load_anything_else_is_the_default() {
    assert_eq!(LEARN_MOVEMENTS_DEFAULT, 2000);
    assert_eq!(sanitize_learn_movements(0), 0);
    assert_eq!(sanitize_learn_movements(1), 2000);
    assert_eq!(sanitize_learn_movements(49), 2000);
    assert_eq!(sanitize_learn_movements(50), 50);
    assert_eq!(sanitize_learn_movements(65534), 65534);
    assert_eq!(sanitize_learn_movements(65535), 2000); // erased EEPROM
}

#[test]
fn every_value_stlnm_can_store_survives_a_restart_unchanged() {
    // the "works until restart" class of bug: the runtime and the start-up range must match
    let requests: [u32; 14] = [
        0,
        1,
        2,
        49,
        50,
        51,
        1000,
        2000,
        65533,
        65534,
        65535,
        65536,
        100_000,
        u32::MAX,
    ];
    for r in requests {
        let stored = learn_movements_from_request(r);
        assert_eq!(sanitize_learn_movements(stored), stored, "request {r}");
    }
    for v in 0..=0xFFFFu32 {
        let stored = learn_movements_from_request(v);
        assert_eq!(sanitize_learn_movements(stored), stored, "request {v}");
    }
}
