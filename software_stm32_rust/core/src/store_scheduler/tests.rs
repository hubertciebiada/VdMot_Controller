//! Port of test/native/test_store_scheduler.cpp.

use super::*;
use crate::config_store::{
    CHANGED_CALIB, CHANGED_ESCALATION, CHANGED_FAILSAFE, CHANGED_LEARN_TIME, CHANGED_LEASE,
    CHANGED_MOTOR, CHANGED_MOVEMENTS, CHANGED_SENSORS,
};
use crate::replies_v2::{
    EEP_STATE_OK, EEP_STATE_PENDING, EEP_STATE_READ_FAILED, EEP_STATE_WRITE_FAILED,
};

/// ticks until tick() returns `step`, at most limit (0: never); every other result must be None
fn ticks_until(s: &mut StoreScheduler, step: Step, limit: u32) -> u32 {
    for n in 1..=limit {
        let got = s.tick();
        if got == step {
            return n;
        }
        assert_eq!(got, Step::None, "tick {n}");
    }
    0
}

/// a change written and the write failed WRITE_ATTEMPTS times: write_failed
fn give_up(s: &mut StoreScheduler) {
    s.changed(CHANGED_MOTOR);
    assert_eq!(ticks_until(s, Step::Write, 10), 3);
    s.write_result(false);
    assert_eq!(s.tick(), Step::Write);
    s.write_result(false);
    assert_eq!(s.tick(), Step::Write);
    s.write_result(false);
    assert!(s.write_failed());
}

#[test]
fn nothing_to_do_without_a_change() {
    let mut s = StoreScheduler::default();
    assert_eq!(ticks_until(&mut s, Step::Write, 5000), 0);
    assert_eq!(s.dirty(), 0);
    assert_eq!(s.eep_state(), EEP_STATE_OK);
    assert!(s.free());
    assert!(!s.retrying());
    assert!(!s.read_failed());
    assert!(!s.write_failed());
}

#[test]
fn one_change_is_written_3_ticks_after_it_not_as_a_retry() {
    let mut s = StoreScheduler::default();
    s.changed(CHANGED_MOVEMENTS);
    assert_eq!(s.eep_state(), EEP_STATE_PENDING);
    assert!(!s.free());
    assert_eq!(s.tick(), Step::None);
    assert_eq!(s.tick(), Step::None);
    assert_eq!(s.tick(), Step::Write);
    assert!(!s.retrying());
    assert_eq!(s.dirty(), CHANGED_MOVEMENTS);
}

#[test]
fn changes_at_ticks_0_2_and_4_are_written_at_tick_7() {
    let mut s = StoreScheduler::default();
    s.changed(CHANGED_MOTOR);
    assert_eq!(s.tick(), Step::None);
    assert_eq!(s.tick(), Step::None);
    s.changed(CHANGED_LEASE);
    assert_eq!(s.tick(), Step::None);
    assert_eq!(s.tick(), Step::None);
    s.changed(CHANGED_FAILSAFE);
    assert_eq!(ticks_until(&mut s, Step::Write, 10), 3);
    assert_eq!(s.dirty(), CHANGED_MOTOR | CHANGED_LEASE | CHANGED_FAILSAFE);
}

#[test]
fn a_change_every_tick_is_written_30_ticks_after_the_first_one() {
    let mut s = StoreScheduler::default();
    // an earlier write, so the counters do not start from their initial values
    s.changed(CHANGED_MOTOR);
    assert_eq!(ticks_until(&mut s, Step::Write, 10), 3);
    s.write_result(true);
    s.changed(CHANGED_SENSORS);
    let mut n: u32 = 1;
    while n <= 40 {
        if s.tick() == Step::Write {
            break;
        }
        s.changed(CHANGED_SENSORS);
        n += 1;
    }
    assert_eq!(n, 30);
}

#[test]
fn a_successful_write_ends_the_change() {
    let mut s = StoreScheduler::default();
    s.changed(CHANGED_ESCALATION);
    assert_eq!(ticks_until(&mut s, Step::Write, 10), 3);
    s.write_result(true);
    assert_eq!(s.dirty(), 0);
    assert_eq!(s.eep_state(), EEP_STATE_OK);
    assert!(s.free());
    assert!(!s.retrying());
    assert_eq!(ticks_until(&mut s, Step::Write, 5000), 0);
}

#[test]
fn a_failed_write_is_repeated_at_the_next_ticks_the_third_failure_gives_up() {
    let mut s = StoreScheduler::default();
    s.changed(CHANGED_LEARN_TIME);
    assert_eq!(ticks_until(&mut s, Step::Write, 10), 3);
    s.write_result(false);
    assert_eq!(s.eep_state(), EEP_STATE_PENDING);
    assert!(!s.free());
    assert!(!s.write_failed());
    assert!(s.retrying());
    assert_eq!(s.tick(), Step::Write);
    s.write_result(false);
    assert_eq!(s.tick(), Step::Write);
    assert!(s.retrying());
    s.write_result(false);
    assert!(s.write_failed());
    assert_eq!(s.eep_state(), EEP_STATE_WRITE_FAILED);
    assert!(s.free());
    assert_eq!(s.dirty(), CHANGED_LEARN_TIME);
}

#[test]
fn a_change_after_a_failed_attempt_restarts_the_debounce_of_the_repetition() {
    let mut s = StoreScheduler::default();
    s.changed(CHANGED_LEARN_TIME);
    assert_eq!(ticks_until(&mut s, Step::Write, 10), 3);
    s.write_result(false);
    s.changed(CHANGED_LEASE);
    assert_eq!(ticks_until(&mut s, Step::Write, 10), 3);
    assert!(s.retrying());
    s.write_result(false);
    assert_eq!(s.tick(), Step::Write);
    s.write_result(false);
    assert!(s.write_failed());
}

#[test]
fn after_the_attempts_the_write_is_retried_after_30_60_3600_ticks() {
    let mut s = StoreScheduler::default();
    give_up(&mut s);
    let expected: [u32; 9] = [30, 60, 120, 240, 480, 960, 1920, 3600, 3600];
    for interval in expected {
        assert_eq!(
            ticks_until(&mut s, Step::Write, 5000),
            interval,
            "interval {interval}"
        );
        assert!(s.retrying(), "interval {interval}");
        s.write_result(false); // a failed retry gives up at once
        assert_eq!(s.eep_state(), EEP_STATE_WRITE_FAILED, "interval {interval}");
    }
}

#[test]
fn a_retry_that_succeeds_clears_the_failure_the_next_change_has_3_attempts() {
    let mut s = StoreScheduler::default();
    give_up(&mut s);
    assert_eq!(ticks_until(&mut s, Step::Write, 100), 30);
    s.write_result(true);
    assert!(!s.write_failed());
    assert_eq!(s.eep_state(), EEP_STATE_OK);
    assert_eq!(s.dirty(), 0);
    assert!(!s.retrying());
    assert_eq!(ticks_until(&mut s, Step::Write, 5000), 0);
    // three attempts again
    give_up(&mut s);
    // the backoff starts again with the first interval
    assert_eq!(ticks_until(&mut s, Step::Write, 100), 30);
}

#[test]
fn a_change_while_the_write_failed_gets_one_attempt_after_the_debounce() {
    let mut s = StoreScheduler::default();
    give_up(&mut s);
    s.changed(CHANGED_CALIB);
    assert_eq!(s.eep_state(), EEP_STATE_PENDING);
    assert!(!s.free());
    assert_eq!(ticks_until(&mut s, Step::Write, 10), 3);
    assert!(s.retrying());
    assert_eq!(s.dirty(), CHANGED_MOTOR | CHANGED_CALIB);
    s.write_result(false);
    assert_eq!(s.eep_state(), EEP_STATE_WRITE_FAILED);
    assert_eq!(ticks_until(&mut s, Step::Write, 5000), 60);
}

#[test]
fn after_a_failed_read_nothing_is_written_the_read_is_repeated() {
    let mut s = StoreScheduler::default();
    s.read_result(false);
    assert!(s.read_failed());
    assert_eq!(s.eep_state(), EEP_STATE_READ_FAILED);
    assert!(s.free());
    s.changed(CHANGED_SENSORS);
    assert_eq!(s.eep_state(), EEP_STATE_READ_FAILED);
    assert!(s.free());
    assert_eq!(ticks_until(&mut s, Step::Reread, 100), 30);
    assert!(s.retrying());
    s.read_result(false);
    assert_eq!(ticks_until(&mut s, Step::Reread, 100), 60);
    s.read_result(true);
    assert!(!s.read_failed());
    assert_eq!(s.eep_state(), EEP_STATE_PENDING);
    assert!(!s.free());
    // the change made meanwhile is written after the debounce, as a first attempt
    assert_eq!(ticks_until(&mut s, Step::Write, 10), 3);
    assert!(!s.retrying());
    assert_eq!(s.dirty(), CHANGED_SENSORS);
}

#[test]
fn a_successful_read_schedules_nothing() {
    let mut s = StoreScheduler::default();
    s.read_result(true);
    assert!(!s.read_failed());
    assert_eq!(s.eep_state(), EEP_STATE_OK);
    assert_eq!(ticks_until(&mut s, Step::Reread, 5000), 0);
    assert_eq!(ticks_until(&mut s, Step::Write, 5000), 0);
}

#[test]
fn the_retry_intervals_of_the_constructor() {
    let mut s = StoreScheduler::new(5, 12);
    s.read_result(false);
    assert_eq!(ticks_until(&mut s, Step::Reread, 100), 5);
    s.read_result(false);
    assert_eq!(ticks_until(&mut s, Step::Reread, 100), 10);
    s.read_result(false);
    assert_eq!(ticks_until(&mut s, Step::Reread, 100), 12);
}
