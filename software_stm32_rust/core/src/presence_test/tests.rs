//! Port of test/native/test_presence_test.cpp.

use super::*;

/// sample index (1-based) of the first result != Pending, 0 if none within n samples
fn first_result(t: &mut PresenceTest, current: i32, n: i32, r: &mut PresenceResult) -> i32 {
    for k in 1..=n {
        *r = t.sample(current);
        if *r != PresenceResult::Pending {
            return k;
        }
    }
    0
}

/// the C++ overload firstResult(shortCheck, enforce, current, r): a new test, 200 samples
fn first_result_new(short_check: bool, enforce: bool, current: i32, r: &mut PresenceResult) -> i32 {
    let mut t = PresenceTest::default();
    t.start(short_check, enforce);
    first_result(&mut t, current, 200, r)
}

#[test]
fn constants() {
    assert_eq!(PresenceTest::SETTLE_TICKS, 7);
    assert_eq!(PresenceTest::NO_CURRENT, 20);
    assert_eq!(PresenceTest::ABSENT_TICKS, 80);
    assert_eq!(PresenceTest::PRESENT_TICKS, 5);
    assert_eq!(PresenceTest::SHORT_LIMIT, 2000);
    assert_eq!(PresenceTest::SHORT_TICKS, 3);
}

#[test]
fn no_current_is_absent_at_sample_88_pending_before() {
    let mut r = PresenceResult::Pending;
    assert_eq!(first_result_new(true, true, 0, &mut r), 88);
    assert_eq!(r, PresenceResult::Absent);
    assert_eq!(first_result_new(true, true, 19, &mut r), 88);
    assert_eq!(first_result_new(true, true, -19, &mut r), 88);
    assert_eq!(r, PresenceResult::Absent);
}

#[test]
fn a_motor_current_is_present_at_sample_13() {
    let mut r = PresenceResult::Pending;
    assert_eq!(first_result_new(true, true, 300, &mut r), 13);
    assert_eq!(r, PresenceResult::Present);
    assert_eq!(first_result_new(true, true, 20, &mut r), 13);
    assert_eq!(r, PresenceResult::Present);
    assert_eq!(first_result_new(true, true, -20, &mut r), 13);
    // the short limit itself is not a short
    assert_eq!(first_result_new(true, true, 2000, &mut r), 13);
    assert_eq!(r, PresenceResult::Present);
}

#[test]
fn a_short_is_found_at_sample_10_when_enforced() {
    let mut r = PresenceResult::Pending;
    assert_eq!(first_result_new(true, true, 2500, &mut r), 10);
    assert_eq!(r, PresenceResult::Short);
    assert_eq!(first_result_new(true, true, -2001, &mut r), 10);
    assert_eq!(r, PresenceResult::Short);
    let mut t = PresenceTest::default();
    t.start(true, true);
    first_result(&mut t, 2500, 200, &mut r);
    assert!(t.short_seen());
}

#[test]
fn only_three_consecutive_samples_above_the_limit_are_a_short() {
    let mut t = PresenceTest::default();
    t.start(true, true);
    for k in 1..=7 {
        assert_eq!(t.sample(2500), PresenceResult::Pending, "sample {k}");
    }
    assert_eq!(t.sample(2500), PresenceResult::Pending);
    assert_eq!(t.sample(2500), PresenceResult::Pending);
    assert_eq!(t.sample(1500), PresenceResult::Pending);
    assert_eq!(t.sample(2500), PresenceResult::Pending);
    assert_eq!(t.sample(2500), PresenceResult::Pending);
    // sample 13: the 6th normal sample would be present
    assert_eq!(t.sample(2500), PresenceResult::Short);
}

#[test]
fn report_only_records_the_short_and_goes_on_like_1_x() {
    let mut t = PresenceTest::default();
    t.start(true, false);
    let mut r = PresenceResult::Pending;
    assert_eq!(first_result(&mut t, 2500, 200, &mut r), 13);
    assert_eq!(r, PresenceResult::Present);
    assert!(t.short_seen());
    t.start(true, false);
    assert!(!t.short_seen());
    assert_eq!(first_result(&mut t, 300, 200, &mut r), 13);
    assert!(!t.short_seen());
}

#[test]
fn with_the_short_check_off_a_short_is_never_seen() {
    let mut t = PresenceTest::default();
    t.start(false, true);
    let mut r = PresenceResult::Pending;
    assert_eq!(first_result(&mut t, 2800, 200, &mut r), 13);
    assert_eq!(r, PresenceResult::Present);
    assert!(!t.short_seen());
}

#[test]
fn start_begins_a_new_test() {
    let mut t = PresenceTest::default();
    t.start(true, true);
    for _ in 0..50 {
        t.sample(0);
    }
    t.start(true, true);
    let mut r = PresenceResult::Pending;
    assert_eq!(first_result(&mut t, 0, 200, &mut r), 88);
    t.start(true, true);
    for _ in 0..9 {
        t.sample(2500);
    }
    t.start(true, true);
    assert_eq!(first_result(&mut t, 300, 200, &mut r), 13);
    assert_eq!(r, PresenceResult::Present);
}

#[test]
fn presence_outcome_status_reference_and_fault_per_result() {
    let mut o = presence_outcome(PresenceResult::Present, true, false);
    assert_eq!(o.status, 1);
    assert!(o.needs_reference);
    assert_eq!(o.fault, 0);
    o = presence_outcome(PresenceResult::Present, true, true);
    assert_eq!(o.status, 8);
    assert!(!o.needs_reference);
    o = presence_outcome(PresenceResult::Present, false, false);
    assert_eq!(o.status, 8);
    assert!(!o.needs_reference);
    o = presence_outcome(PresenceResult::Present, false, true);
    assert_eq!(o.status, 8);
    o = presence_outcome(PresenceResult::Absent, true, false);
    assert_eq!(o.status, 6);
    assert!(!o.needs_reference);
    assert_eq!(o.fault, 0);
    o = presence_outcome(PresenceResult::Short, true, false);
    assert_eq!(o.status, 4);
    assert_eq!(o.fault, 3);
    assert!(!o.needs_reference);
    o = presence_outcome(PresenceResult::Pending, true, false);
    assert_eq!(o.status, 5);
    assert!(!o.needs_reference);
    assert_eq!(o.fault, 0);
}
