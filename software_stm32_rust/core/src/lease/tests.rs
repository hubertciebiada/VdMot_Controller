//! Port of test/native/test_lease.cpp.

use super::*;

#[test]
fn timeout_bounds_default_and_client_idle_time() {
    assert_eq!(LEASE_TIMEOUT_OFF, 0);
    assert_eq!(LEASE_TIMEOUT_MIN_MIN, 5);
    assert_eq!(LEASE_TIMEOUT_MAX_MIN, 1440);
    assert_eq!(LEASE_TIMEOUT_DEFAULT_MIN, 60);
    assert_eq!(LEASE_CLIENT_IDLE_S, 300);
    // gstax field 7
    assert_eq!(LeaseState::Off as u8, 0);
    assert_eq!(LeaseState::Running as u8, 1);
    assert_eq!(LeaseState::Expired as u8, 2);
}

#[test]
fn lease_timeout_valid_0_off_and_5_to_1440_minutes() {
    assert!(lease_timeout_valid(0));
    assert!(!lease_timeout_valid(1));
    assert!(!lease_timeout_valid(4));
    assert!(lease_timeout_valid(5));
    assert!(lease_timeout_valid(6));
    assert!(lease_timeout_valid(60));
    assert!(lease_timeout_valid(1439));
    assert!(lease_timeout_valid(1440));
    assert!(!lease_timeout_valid(1441));
    assert!(!lease_timeout_valid(0xFFFF));
    // the request value is 32 bit: no truncation to 16 bit
    assert!(!lease_timeout_valid(0x10000));
    assert!(!lease_timeout_valid(0x10000 + 60));
    assert!(!lease_timeout_valid(u32::MAX));
}

#[test]
fn sanitize_lease_timeout_what_slcfg_accepts_is_kept_anything_else_loads_60() {
    assert_eq!(sanitize_lease_timeout(0), 0);
    assert_eq!(sanitize_lease_timeout(1), 60);
    assert_eq!(sanitize_lease_timeout(3), 60);
    assert_eq!(sanitize_lease_timeout(4), 60);
    assert_eq!(sanitize_lease_timeout(5), 5);
    assert_eq!(sanitize_lease_timeout(30), 30);
    assert_eq!(sanitize_lease_timeout(1440), 1440);
    assert_eq!(sanitize_lease_timeout(1441), 60);
    assert_eq!(sanitize_lease_timeout(0xFFFF), 60); // erased EEPROM
}
