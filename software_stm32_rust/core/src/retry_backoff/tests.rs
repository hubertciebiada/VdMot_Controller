//! Port of test/native/test_retry_backoff.cpp.

use super::*;

/// ticks until the retry is due (tick() returns true), at most limit
fn ticks_until_due(b: &mut RetryBackoff, limit: u32) -> u32 {
    for n in 1..=limit {
        if b.tick() {
            return n;
        }
    }
    0
}

#[test]
fn nothing_is_due_without_a_failure() {
    let mut b = RetryBackoff::new(30, 3600);
    assert!(!b.pending());
    assert_eq!(b.interval(), 0);
    assert_eq!(ticks_until_due(&mut b, 10000), 0);
}

#[test]
fn intervals_double_up_to_the_maximum() {
    let mut b = RetryBackoff::new(30, 200);
    b.failed();
    assert!(b.pending());
    assert_eq!(ticks_until_due(&mut b, 1000), 30);
    b.failed();
    assert_eq!(ticks_until_due(&mut b, 1000), 60);
    b.failed();
    assert_eq!(ticks_until_due(&mut b, 1000), 120);
    b.failed();
    assert_eq!(ticks_until_due(&mut b, 1000), 200);
    b.failed();
    assert_eq!(ticks_until_due(&mut b, 1000), 200);
}

#[test]
fn a_due_retry_stays_due_until_the_caller_reports_the_outcome() {
    let mut b = RetryBackoff::new(2, 10);
    b.failed();
    assert!(!b.tick());
    assert!(b.tick());
    assert!(b.tick());
    assert!(b.tick());
    b.succeeded();
    assert!(!b.pending());
    assert!(!b.tick());
}

#[test]
fn success_resets_the_schedule_to_the_first_interval() {
    let mut b = RetryBackoff::new(5, 100);
    b.failed();
    b.failed();
    b.failed();
    assert_eq!(b.interval(), 20);
    b.succeeded();
    b.failed();
    assert_eq!(b.interval(), 5);
    assert_eq!(ticks_until_due(&mut b, 100), 5);
}

#[test]
fn degenerate_configurations_stay_usable() {
    let mut zero = RetryBackoff::new(0, 0);
    zero.failed();
    assert_eq!(zero.interval(), 1);
    assert!(zero.tick());

    let mut inverted = RetryBackoff::new(50, 10); // max below first: first wins
    inverted.failed();
    assert_eq!(inverted.interval(), 50);
    inverted.failed();
    assert_eq!(inverted.interval(), 50);

    let mut huge = RetryBackoff::new(0x8000_0001, 0xFFFF_FFFF); // doubling must not overflow
    huge.failed();
    huge.failed();
    assert_eq!(huge.interval(), 0xFFFF_FFFF);
    huge.failed();
    assert_eq!(huge.interval(), 0xFFFF_FFFF);
}

#[test]
fn doubling_stops_exactly_at_half_the_maximum() {
    // 3 == 7 / 2 is not above half: it doubles to 6, then 6 > 3 jumps to the maximum
    let mut b = RetryBackoff::new(3, 7);
    b.failed();
    assert_eq!(b.interval(), 3);
    b.failed();
    assert_eq!(b.interval(), 6);
    b.failed();
    assert_eq!(b.interval(), 7);

    // an interval between a third and half of the maximum still doubles
    let mut c = RetryBackoff::new(40, 200);
    c.failed();
    c.failed();
    assert_eq!(c.interval(), 80);
    c.failed();
    assert_eq!(c.interval(), 160);
    c.failed();
    assert_eq!(c.interval(), 200);
}
