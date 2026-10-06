//! Port of test/native/test_valve_scheduler__mut.cpp.

use super::*;

type Views = [ValveView; VALVES];

/// every valve idle, calibrated and at its target 50
fn idle_all() -> Views {
    [ValveView {
        status: ST_IDLE,
        actual: 50,
        target: 50,
        drive: 50,
        calibrated: true,
        ..Default::default()
    }; VALVES]
}

const RUN: SchedulerInputs = SchedulerInputs {
    safe_mode: false,
    hold_for_temperature: false,
};

fn is(d: &Decision, k: ActionKind, valve: u8) -> bool {
    d.kind == k && d.valve == valve
}

#[test]
fn a_presence_test_clears_the_end_stop_latch_of_the_valve() {
    let mut v = idle_all();
    v[2].drive = 70;
    let mut s = ValveScheduler::default();
    assert!(is(&s.next(&v, &RUN), ActionKind::Open, 2)); // test index 0
    s.move_ended(2, StopReason::EndStop, false, ST_IDLE);
    assert!(s.latched(2));
    v[2].status = ST_UNKNOWN;
    assert!(is(&s.next(&v, &RUN), ActionKind::Test, 2)); // test index 2
    assert!(!s.latched(2));
}

#[test]
fn a_move_the_other_way_keeps_the_retry_of_an_early_end_stop() {
    let mut v = idle_all();
    v[3].drive = 60;
    v[3].actual = 20;
    let mut s = ValveScheduler::default();
    assert!(is(&s.next(&v, &RUN), ActionKind::Open, 3));
    s.move_ended(3, StopReason::EarlyEndStop, true, ST_IDLE);
    assert!(s.latched(3));
    // the valve is above its drive value: a close move, not reported as ended
    v[3].actual = 90;
    assert!(is(&s.next(&v, &RUN), ActionKind::Close, 3));
    // below again: the one retry in the open direction is still there
    v[3].actual = 20;
    assert!(is(&s.next(&v, &RUN), ActionKind::Open, 3));
}

#[test]
fn a_full_open_move_that_ends_at_the_end_stop_latches_early_with_one_retry() {
    let mut v = idle_all();
    v[4].status = ST_FULL_OPEN;
    v[4].drive = 100;
    v[4].actual = 40;
    let mut s = ValveScheduler::default();
    assert!(is(&s.next(&v, &RUN), ActionKind::OpenEnd, 4));
    s.move_ended(4, StopReason::EarlyEndStop, true, ST_IDLE);
    assert!(s.latched(4));
    v[4].status = ST_IDLE;
    v[4].actual = 90;
    // the retry of the early end stop
    assert!(is(&s.next(&v, &RUN), ActionKind::OpenEnd, 4));
    s.move_ended(4, StopReason::EndStop, false, ST_IDLE);
    assert!(s.latched(4));
    assert_eq!(s.next(&v, &RUN).kind, ActionKind::None);
}

#[test]
fn a_reference_move_clears_the_latch_when_it_is_decided() {
    let mut v = idle_all();
    v[5].drive = 70;
    let mut s = ValveScheduler::default();
    assert!(is(&s.next(&v, &RUN), ActionKind::Open, 5));
    s.move_ended(5, StopReason::EndStop, false, ST_IDLE);
    assert!(s.latched(5));
    v[5].needs_reference = true;
    let d = s.next(&v, &RUN);
    assert!(is(&d, ActionKind::OpenEnd, 5));
    assert!(d.reference);
    assert!(!s.latched(5));
}

#[test]
fn a_calibration_drops_the_pending_move_of_the_valve() {
    let mut v = idle_all();
    v[6].drive = 70;
    let mut s = ValveScheduler::default();
    assert!(is(&s.next(&v, &RUN), ActionKind::Open, 6));
    s.move_ended(6, StopReason::Target, false, ST_IDLE);
    assert!(!s.latched(6));
    v[6].actual = 70;
    v[6].status = ST_PRESENT;
    assert!(is(&s.next(&v, &RUN), ActionKind::Learn, 6));
    s.move_ended(6, StopReason::EndStop, false, ST_IDLE);
    assert!(!s.latched(6));
}

#[test]
fn the_same_move_at_an_end_stop_latches_again_after_the_latch_was_cleared() {
    let mut v = idle_all();
    v[7].drive = 70;
    let mut s = ValveScheduler::default();
    assert!(is(&s.next(&v, &RUN), ActionKind::Open, 7));
    s.move_ended(7, StopReason::EndStop, false, ST_IDLE);
    assert!(s.latched(7));
    s.clear_latch(7);
    assert!(is(&s.next(&v, &RUN), ActionKind::Open, 7));
    s.move_ended(7, StopReason::EndStop, false, ST_IDLE);
    assert!(s.latched(7));
}

#[test]
fn an_end_stop_the_other_way_at_the_same_drive_value_latches_that_way() {
    let mut v = idle_all();
    v[8].drive = 50;
    v[8].actual = 20;
    let mut s = ValveScheduler::default();
    assert!(is(&s.next(&v, &RUN), ActionKind::Open, 8));
    s.move_ended(8, StopReason::EndStop, false, ST_IDLE);
    assert!(s.latched(8));
    v[8].actual = 80;
    assert!(is(&s.next(&v, &RUN), ActionKind::Close, 8));
    s.move_ended(8, StopReason::EndStop, false, ST_IDLE);
    // latched closing now: no further close move, an open move is allowed
    assert_eq!(s.next(&v, &RUN).kind, ActionKind::None);
    v[8].actual = 20;
    assert!(is(&s.next(&v, &RUN), ActionKind::Open, 8));
}

#[test]
fn clear_latch_of_an_invalid_valve_touches_no_other_valve() {
    let mut v = idle_all();
    v[0].drive = 70;
    let mut s = ValveScheduler::default();
    assert!(is(&s.next(&v, &RUN), ActionKind::Open, 0));
    s.clear_latch(VALVE_COUNT);
    s.clear_latch(255);
    s.move_ended(0, StopReason::EndStop, false, ST_IDLE);
    assert!(s.latched(0));
}
