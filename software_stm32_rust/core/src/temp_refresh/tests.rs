//! Port of test/native/test_temp_refresh.cpp.

use super::*;

#[test]
fn constants() {
    assert_eq!(TEMP_REFRESH_MS, 60000);
    assert_eq!(TEMP_HOLD_MAX_MS, 3000);
}

#[test]
fn due_60_s_after_start_up_the_hold_ends_with_the_cycle() {
    let mut t = TempRefresh::default();
    assert!(!t.due(59999));
    assert!(!t.hold_commands(59999));
    assert!(t.due(60000));
    assert!(t.hold_commands(60000));
    assert!(t.hold_commands(61000));
    t.cycle_done(61500);
    assert!(!t.due(61500));
    assert!(!t.hold_commands(61500));
    assert!(!t.due(121499));
    assert!(t.due(121500));
}

#[test]
fn a_hold_ends_after_3000_ms_and_comes_again_60_s_later() {
    let mut t = TempRefresh::default();
    assert!(t.hold_commands(60000));
    assert!(t.hold_commands(62999));
    assert!(!t.hold_commands(63000));
    assert!(!t.due(63000));
    assert!(!t.hold_commands(64000));
    assert!(!t.hold_commands(122999));
    assert!(t.hold_commands(123000));
    assert!(t.hold_commands(125999));
    assert!(!t.hold_commands(126000));
}

#[test]
fn a_hold_that_timed_out_elsewhere_ends_the_period() {
    let mut t = TempRefresh::default();
    assert!(t.due(61000));
    t.hold_timed_out(64000);
    assert!(!t.due(64000));
    assert!(!t.due(123999));
    assert!(t.due(124000));
    assert!(t.hold_commands(124000));
    t.hold_timed_out(125000);
    assert!(!t.hold_commands(125000));
    assert!(t.hold_commands(185000));
    assert!(t.hold_commands(187999));
}

#[test]
fn the_hold_time_starts_with_the_first_hold_not_with_the_due_time() {
    let mut t = TempRefresh::default();
    assert!(t.due(70000));
    assert!(t.hold_commands(70000));
    assert!(t.hold_commands(72999));
    assert!(!t.hold_commands(73000));
}

#[test]
fn age_since_start_up_and_since_the_last_cycle() {
    let mut t = TempRefresh::default();
    assert_eq!(t.age_s(0), 0);
    assert_eq!(t.age_s(999), 0);
    assert_eq!(t.age_s(1000), 1);
    assert_eq!(t.age_s(65000), 65);
    t.cycle_done(65000);
    assert_eq!(t.age_s(65999), 0);
    assert_eq!(t.age_s(67000), 2);
    // a timed-out hold does not reset the age
    t.hold_commands(125000);
    t.hold_commands(128000);
    assert_eq!(t.age_s(128000), 63);
}

#[test]
fn millis_wrap() {
    let mut t = TempRefresh::default();
    t.cycle_done(0xFFFF_F000);
    assert!(!t.due(0xFFFF_FFFF));
    assert!(t.due(0x0001_0000));
    assert_eq!(t.age_s(0x0001_0000), 69);
    assert!(t.hold_commands(0x0001_0000));
    assert!(!t.hold_commands(0x0001_0000 + 3000));
}
