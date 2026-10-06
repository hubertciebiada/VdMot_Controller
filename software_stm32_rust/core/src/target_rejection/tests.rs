//! Port of test/native/test_target_rejection.cpp.

use super::*;

/// position the failed or blocked valve is stuck at in these tests
const STUCK: u8 = 70;

#[test]
fn reject_target_without_a_known_target_the_first_one_is_recorded_not_counted() {
    let mut rejected = NO_REJECTED_TARGET;
    let mut count: u16 = 0;
    assert!(!reject_target(&mut rejected, &mut count, 30, STUCK));
    assert_eq!(rejected, 30);
    assert_eq!(count, 0);
    // app_loop visits the valve again and again: nothing changes
    for i in 0..100 {
        assert!(
            !reject_target(&mut rejected, &mut count, 30, STUCK),
            "visit {i}"
        );
    }
    assert_eq!(count, 0);
}

#[test]
fn reject_target_the_target_recorded_at_the_handover_is_not_counted() {
    // app_loop recorded 50 when it handed the move that then timed out
    let mut rejected: u8 = 50;
    let mut count: u16 = 0;
    assert!(!reject_target(&mut rejected, &mut count, 50, STUCK));
    assert_eq!(count, 0);
}

#[test]
fn reject_target_a_new_target_before_the_first_visit_after_the_fault_is_counted() {
    // handed with target 50, the move timed out, the ESP sent 60 before app_loop saw the fault
    let mut rejected: u8 = 50;
    let mut count: u16 = 0;
    assert!(reject_target(&mut rejected, &mut count, 60, STUCK));
    assert_eq!(count, 1);
    assert_eq!(rejected, 60);
}

#[test]
fn reject_target_every_later_target_change_is_counted_once() {
    let mut rejected = NO_REJECTED_TARGET;
    let mut count: u16 = 0;
    reject_target(&mut rejected, &mut count, 30, STUCK);
    assert!(reject_target(&mut rejected, &mut count, 100, STUCK)); // e.g. staop on a blocked valve
    assert_eq!(count, 1);
    assert!(!reject_target(&mut rejected, &mut count, 100, STUCK));
    assert!(reject_target(&mut rejected, &mut count, 0, STUCK));
    assert!(reject_target(&mut rejected, &mut count, 30, STUCK)); // back to the first value is a change too
    assert_eq!(count, 3);
    assert_eq!(rejected, 30);
}

#[test]
fn reject_target_a_target_equal_to_the_position_is_not_rejected_the_next_one_counts() {
    // review finding: stuck at 30 with the left-behind target 50; the ESP sets 30, then 50 again
    let mut rejected: u8 = 50;
    let mut count: u16 = 0;
    assert!(!reject_target(&mut rejected, &mut count, 50, 30));
    assert!(!reject_target(&mut rejected, &mut count, 30, 30));
    assert_eq!(count, 0);
    assert_eq!(rejected, 30);
    assert!(reject_target(&mut rejected, &mut count, 50, 30));
    assert_eq!(count, 1);
    // also without a known target
    rejected = NO_REJECTED_TARGET;
    assert!(!reject_target(&mut rejected, &mut count, 30, 30));
    assert_eq!(rejected, 30);
    assert!(reject_target(&mut rejected, &mut count, 31, 30));
    assert_eq!(count, 2);
}

#[test]
fn reject_target_the_counter_saturates() {
    let mut rejected: u8 = 10;
    let mut count: u16 = 0xFFFE;
    assert!(reject_target(&mut rejected, &mut count, 11, STUCK));
    assert_eq!(count, 0xFFFF);
    assert!(reject_target(&mut rejected, &mut count, 12, STUCK));
    assert_eq!(count, 0xFFFF);
}

#[test]
fn reject_target_0_and_100_are_ordinary_targets() {
    let mut rejected = NO_REJECTED_TARGET;
    let mut count: u16 = 0;
    assert!(!reject_target(&mut rejected, &mut count, 0, STUCK));
    assert_eq!(rejected, 0);
    assert!(reject_target(&mut rejected, &mut count, 100, STUCK));
    assert_eq!(count, 1);
    assert!(!reject_target(&mut rejected, &mut count, 100, 100));
    assert_eq!(count, 1);
}
