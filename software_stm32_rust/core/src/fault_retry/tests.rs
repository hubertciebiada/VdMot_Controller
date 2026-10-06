//! Port of test/native/test_fault_retry.cpp.

use super::*;

#[test]
fn fault_retry_interval_1_h_6_h_then_every_24_h() {
    assert_eq!(fault_retry_interval(0), 3600);
    assert_eq!(fault_retry_interval(1), 21600);
    assert_eq!(fault_retry_interval(2), 86400);
    assert_eq!(fault_retry_interval(3), 86400);
    assert_eq!(fault_retry_interval(255), 86400);
}

#[test]
fn schedules_on_the_first_faulted_call_and_fires_after_exactly_3600_s() {
    let mut r = FaultRetry::default();
    assert!(!r.scheduled());
    assert_eq!(r.remaining_s(), 0);
    assert!(!r.update(true, false, 50)); // the elapsed time of the scheduling call does not count
    assert!(r.scheduled());
    assert_eq!(r.remaining_s(), 3600);
    assert_eq!(r.attempts(), 0);
    assert!(!r.update(true, false, 3590));
    assert_eq!(r.remaining_s(), 10);
    assert!(r.update(true, false, 10));
    assert!(!r.scheduled());
    assert_eq!(r.remaining_s(), 0);
    assert_eq!(r.attempts(), 1);
}

#[test]
fn a_late_call_fires_once_the_retry_runs_busy_a_new_fault_waits_6_h() {
    let mut r = FaultRetry::default();
    r.update(true, false, 0);
    assert!(r.update(true, false, 4000));
    assert!(!r.update(true, false, 1)); // not busy yet: schedules the next interval
    assert_eq!(r.remaining_s(), 21600);
    assert!(!r.update(false, true, 1)); // the retry calibration runs
    assert!(!r.scheduled());
    assert_eq!(r.attempts(), 1);
    assert!(!r.update(true, true, 1)); // still requested
    assert!(!r.scheduled());
    assert!(!r.update(true, false, 1)); // failed again
    assert_eq!(r.remaining_s(), 21600);
    assert!(r.update(true, false, 21600));
    assert_eq!(r.attempts(), 2);
    r.update(true, false, 0);
    assert_eq!(r.remaining_s(), 86400);
}

#[test]
fn a_valve_that_is_fine_again_resets_the_attempts() {
    let mut r = FaultRetry::default();
    r.update(true, false, 0);
    assert!(r.update(true, false, 3600));
    assert_eq!(r.attempts(), 1);
    assert!(!r.update(false, false, 1));
    assert_eq!(r.attempts(), 0);
    assert!(!r.scheduled());
    r.update(true, false, 0);
    assert_eq!(r.remaining_s(), 3600);
}

#[test]
fn busy_while_scheduled_drops_the_schedule_and_keeps_the_attempts() {
    let mut r = FaultRetry::default();
    r.update(true, false, 0);
    assert!(r.update(true, false, 3600));
    r.update(true, false, 0);
    r.update(true, false, 100);
    assert_eq!(r.remaining_s(), 21500);
    assert!(!r.update(true, true, 5));
    assert!(!r.scheduled());
    assert_eq!(r.attempts(), 1);
    r.update(true, false, 7);
    assert_eq!(r.remaining_s(), 21600);
}

#[test]
fn the_attempts_saturate_at_255() {
    let mut r = FaultRetry::default();
    for i in 0..300 {
        r.update(true, false, 0);
        assert!(r.update(true, false, 86400), "retry {i}");
    }
    assert_eq!(r.attempts(), 255);
}

#[test]
fn snapshot_and_restore() {
    let mut a = FaultRetry::default();
    a.update(true, false, 0);
    a.update(true, false, 600);
    let s: Snapshot = a.snapshot();
    assert_eq!(s.attempts, 0);
    assert!(s.scheduled);
    assert_eq!(s.remaining_s, 3000);
    let mut b = FaultRetry::default();
    b.restore(&s);
    assert!(b.scheduled());
    assert_eq!(b.remaining_s(), 3000);
    assert!(b.update(true, false, 3000));
    assert_eq!(b.attempts(), 1);
    b.restore(&Snapshot {
        attempts: 7,
        scheduled: false,
        remaining_s: 123,
    });
    assert_eq!(b.attempts(), 7);
    assert!(!b.scheduled());
    assert_eq!(b.remaining_s(), 0);
    assert_eq!(b.snapshot().remaining_s, 0);
    b.restore(&Snapshot {
        attempts: 2,
        scheduled: true,
        remaining_s: 5,
    });
    assert!(!b.update(true, false, 4));
    assert_eq!(b.remaining_s(), 1);
}
