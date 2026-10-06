//! Port of test/native/test_manual_enable.cpp.

use super::*;

#[test]
fn manual_enable_expired_the_time_limit_of_2000_ms() {
    assert!(!manual_enable_expired(1000, 1000, 0));
    assert!(!manual_enable_expired(1000, 2999, 0));
    assert!(manual_enable_expired(1000, 3000, 0));
    assert!(manual_enable_expired(1000, 3001, 0));
}

#[test]
fn manual_enable_expired_more_than_60_ma_either_way_switches_off_at_once() {
    assert!(!manual_enable_expired(0, 0, 600));
    assert!(manual_enable_expired(0, 0, 601));
    assert!(!manual_enable_expired(0, 0, -600));
    assert!(manual_enable_expired(0, 0, -601));
    assert!(manual_enable_expired(0, 1999, 601));
}

#[test]
fn manual_enable_expired_across_the_millis_wrap() {
    assert!(!manual_enable_expired(0xFFFF_FC18, 0x0000_03E7, 0)); // 1999 ms
    assert!(manual_enable_expired(0xFFFF_FC18, 0x0000_03E8, 0)); // 2000 ms
}

#[test]
fn the_limits() {
    assert_eq!(MANUAL_ENABLE_MAX_MS, 2000);
    assert_eq!(MANUAL_ENABLE_LIMIT, 600);
}
