//! Port of test/native/test_failsafe.cpp.

use super::*;

#[test]
fn hold_marker_and_default_position() {
    assert_eq!(FAILSAFE_HOLD, 255);
    assert_eq!(FAILSAFE_DEFAULT_PCT, 50);
}

#[test]
fn failsafe_pct_valid_0_to_100_pct_and_255_hold() {
    assert!(failsafe_pct_valid(0));
    assert!(failsafe_pct_valid(1));
    assert!(failsafe_pct_valid(50));
    assert!(failsafe_pct_valid(99));
    assert!(failsafe_pct_valid(100));
    assert!(!failsafe_pct_valid(101));
    assert!(!failsafe_pct_valid(200));
    assert!(!failsafe_pct_valid(254));
    assert!(failsafe_pct_valid(255));
    // the request value is 32 bit: no truncation to 8 bit
    assert!(!failsafe_pct_valid(256));
    assert!(!failsafe_pct_valid(256 + 50));
    assert!(!failsafe_pct_valid(256 + 255));
    assert!(!failsafe_pct_valid(u32::MAX));
}

#[test]
fn sanitize_failsafe_pct_what_sfspo_accepts_is_kept_anything_else_loads_50() {
    assert_eq!(sanitize_failsafe_pct(0), 0);
    assert_eq!(sanitize_failsafe_pct(30), 30);
    assert_eq!(sanitize_failsafe_pct(100), 100);
    assert_eq!(sanitize_failsafe_pct(101), 50);
    assert_eq!(sanitize_failsafe_pct(254), 50);
    assert_eq!(sanitize_failsafe_pct(255), 255);
}
