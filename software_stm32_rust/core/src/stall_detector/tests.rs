//! Port of test/native/test_stall_detector.cpp.

use super::*;

#[test]
fn a_busy_state_is_stalled_after_exactly_limit_further_ticks() {
    let mut d = StallDetector::new(3);
    d.tick(5, false); // enters state 5
    assert!(!d.stalled());
    d.tick(5, false);
    d.tick(5, false);
    assert!(!d.stalled());
    d.tick(5, false);
    assert!(d.stalled());
    d.tick(5, false); // stays stalled, the count saturates
    assert!(d.stalled());
}

#[test]
fn a_state_change_restarts_the_count() {
    let mut d = StallDetector::new(2);
    d.tick(5, false);
    d.tick(5, false);
    d.tick(6, false);
    d.tick(6, false);
    assert!(!d.stalled());
    d.tick(6, false);
    assert!(d.stalled());
    d.tick(5, false); // progress after a stall clears it
    assert!(!d.stalled());
}

#[test]
fn idle_is_never_stalled() {
    let mut d = StallDetector::new(2);
    for _ in 0..10 {
        d.tick(1, true);
    }
    assert!(!d.stalled());
    d.tick(1, false); // same value, but now busy: counting starts after idle
    d.tick(1, false);
    assert!(!d.stalled());
    d.tick(1, false);
    assert!(d.stalled());
    d.tick(1, true);
    assert!(!d.stalled());
}

#[test]
fn the_first_busy_tick_enters_the_state_whatever_its_value() {
    let mut d = StallDetector::new(1);
    assert!(!d.stalled());
    d.tick(0, false);
    assert!(!d.stalled());
    d.tick(0, false);
    assert!(d.stalled());
}

#[test]
fn limit_0_means_stalled_while_busy() {
    let mut d = StallDetector::new(0);
    d.tick(7, false);
    assert!(d.stalled());
    d.tick(7, true);
    assert!(!d.stalled());
}

#[test]
fn large_limits_do_not_overflow() {
    let mut d = StallDetector::new(u32::MAX);
    d.tick(3, false);
    for _ in 0..1000 {
        d.tick(3, false);
    }
    assert!(!d.stalled());
}
