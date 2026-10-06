//! Port of test/native/test_health_monitor.cpp: HealthMonitor events. A C++ null `out` is the
//! empty slice, a `maxOut` below the array size a shorter slice.

use super::*;
use crate::event_log::{format_event_message, Severity};
use crate::stm_codec::{StopReason, CAL_FLAG_EARLY_STOP, CAL_STATE_RUNNING};
use crate::test_support::assert_text;
use crate::valve_model::{HEALTH_CALIB_RETRIES, HEALTH_EARLY_STOP, HEALTH_TEMP_FAILED};
use std::vec::Vec;

fn known(status: u8) -> ValveState {
    ValveState {
        known: true,
        status,
        ..ValveState::default()
    }
}

/// Up to [`MAX_EVENTS_PER_UPDATE`] events of one `on_valve` call (C++ `Events`).
fn on_valve_with(
    hm: &HealthMonitor,
    a: &ValveState,
    b: &ValveState,
    active: bool,
    valve: u8,
) -> Vec<Event> {
    let mut e: [Event; MAX_EVENTS_PER_UPDATE] = Default::default();
    let n = hm.on_valve(valve, a, b, active, &mut e);
    e[..n].to_vec()
}

/// C++ defaults: active true, valve 3.
fn on_valve(hm: &HealthMonitor, a: &ValveState, b: &ValveState) -> Vec<Event> {
    on_valve_with(hm, a, b, true, 3)
}

#[track_caller]
fn check_event(e: &Event, code: EventCode, valve: u8, a1: i32, a2: i32) {
    assert_eq!(e.code, code);
    assert_eq!(e.severity, event_default_severity(code));
    assert_eq!(e.valve, valve);
    assert_eq!(e.arg1, a1);
    assert_eq!(e.arg2, a2);
    assert!(e.text.is_empty());
}

#[test]
fn on_valve_argument_guards() {
    let hm = HealthMonitor::default();
    let (a, b) = (known(1), known(9));
    let mut out: [Event; MAX_EVENTS_PER_UPDATE] = Default::default();
    assert_eq!(hm.on_valve(12, &a, &b, true, &mut out), 0);
    assert_eq!(hm.on_valve(0, &a, &b, true, &mut []), 0); // C++ out nullptr
    assert_eq!(hm.on_valve(0, &a, &b, true, &mut out[..0]), 0);
    assert_eq!(hm.on_valve(11, &a, &b, true, &mut out[..1]), 1);
    assert_eq!(out[0].valve, 11);
}

#[test]
fn on_valve_first_snapshot_is_silent_except_bad_states() {
    let hm = HealthMonitor::default();
    let unknown = ValveState::default();
    assert!(on_valve(&hm, &unknown, &known(1)).is_empty());
    assert!(on_valve(&hm, &unknown, &known(5)).is_empty());
    let mut blocked = known(9);
    blocked.calib_retries = 2;
    let r = on_valve(&hm, &unknown, &blocked);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveBlocked, 3, 2, -1);
    let r = on_valve(&hm, &unknown, &known(4));
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveFailed, 3, 0, -1);
    let r = on_valve(&hm, &unknown, &known(6));
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveNoValve, 3, 0, 0);
    assert!(on_valve_with(&hm, &unknown, &known(6), false, 3).is_empty());
    // Calibrating, counters, flags: all silent on the first snapshot.
    let mut busy = known(2);
    busy.calibrating = true;
    busy.calib_retries = 1;
    busy.health = HEALTH_STALE | HEALTH_TARGET_UNCONFIRMED;
    assert!(on_valve(&hm, &unknown, &busy).is_empty());
    // A user target set before the first data is still reported.
    let mut t = known(1);
    t.desired_valid = true;
    t.desired = 20;
    t.source = TargetSource::Web;
    let r = on_valve(&hm, &unknown, &t);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::TargetSet, 3, 20, TargetSource::Web as i32);
    let u = ValveState {
        desired_valid: true,
        desired: 5,
        source: TargetSource::Mqtt,
        ..ValveState::default()
    };
    let r = on_valve(&hm, &ValveState::default(), &u); // valve still unknown
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::TargetSet, 3, 5, TargetSource::Mqtt as i32);
    // Nothing at all for an unknown valve without a target change.
    let w = ValveState {
        status: 9,
        ..ValveState::default()
    };
    assert!(on_valve(&hm, &ValveState::default(), &w).is_empty());
}

#[test]
fn on_valve_bad_status_transitions_and_recovery() {
    let hm = HealthMonitor::default();
    let mut b = known(9);
    b.calib_retries = 1;
    let r = on_valve(&hm, &known(1), &b);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveBlocked, 3, 1, -1);
    assert_eq!(r[0].severity, Severity::Error);
    let r = on_valve(&hm, &known(9), &known(1));
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveRecovered, 3, 9, 0);
    let r = on_valve(&hm, &known(4), &known(9));
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveBlocked, 3, 0, -1);
    let r = on_valve(&hm, &known(1), &known(4));
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveFailed, 3, 0, -1);
    let r = on_valve(&hm, &known(6), &known(8));
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveRecovered, 3, 6, 0);
    let r = on_valve(&hm, &known(1), &known(6));
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveNoValve, 3, 0, 0);
    // Inactive NoValve is just a state change (Debug).
    let r = on_valve_with(&hm, &known(1), &known(6), false, 3);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveStateChanged, 3, 1, 6);
    assert_eq!(r[0].severity, Severity::Debug);
    // Staying bad: nothing.
    assert!(on_valve(&hm, &known(9), &known(9)).is_empty());
    // Plain status change.
    let r = on_valve(&hm, &known(1), &known(2));
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveStateChanged, 3, 1, 2);
    assert!(on_valve(&hm, &known(2), &known(2)).is_empty());
}

#[test]
fn on_valve_calibration_outcomes() {
    let hm = HealthMonitor::default();
    let idle = known(1);
    let mut cal = known(1);
    cal.calibrating = true;
    let r = on_valve(&hm, &idle, &cal);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::CalibStarted, 3, 0, 0);

    let mut ok = known(1);
    ok.open_count = 3120;
    ok.close_count = 3350;
    let r = on_valve(&hm, &cal, &ok);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::CalibOk, 3, 3120, 3350);

    // Ending Blocked: CalibFailed replaces ValveBlocked.
    let mut failed = known(9);
    failed.calib_retries = 2;
    let mut cal_retry = cal;
    cal_retry.calib_retries = 1;
    let r = on_valve(&hm, &cal_retry, &failed);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::CalibFailed, 3, 2, -1);

    // Blocked valve recalibrated fine: CalibOk replaces ValveRecovered.
    let mut cal_from_blocked = known(9);
    cal_from_blocked.calibrating = true;
    let r = on_valve(&hm, &cal_from_blocked, &ok);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].code, EventCode::CalibOk);
    // Blocked valve failing again (status stays 9): CalibFailed only.
    let r = on_valve(&hm, &cal_from_blocked, &failed);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].code, EventCode::CalibFailed);

    // Ending in another state: no outcome; the status event describes it.
    let r = on_valve(&hm, &cal, &known(6));
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveNoValve, 3, 0, 0);
    let r = on_valve(&hm, &cal, &known(5));
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveStateChanged, 3, 1, 5);
    // Starting calibration from a bad state recovers it (status changed).
    let mut cal_idle = known(8);
    cal_idle.calibrating = true;
    let r = on_valve(&hm, &known(9), &cal_idle);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].code, EventCode::CalibStarted);
    check_event(&r[1], EventCode::ValveRecovered, 3, 9, 0);

    // Retries during calibration.
    let r = on_valve(&hm, &cal, &cal_retry);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::CalibRetry, 3, 1, 0);
    assert_eq!(r[0].severity, Severity::Warning);
    // ... also when the calibration ends ok in the same snapshot.
    let mut ok_retry = ok;
    ok_retry.calib_retries = 2;
    let r = on_valve(&hm, &cal_retry, &ok_retry);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].code, EventCode::CalibOk);
    check_event(&r[1], EventCode::CalibRetry, 3, 2, 0);
    // ... and when it starts with a retry count already raised.
    let mut start_retry = cal;
    start_retry.calib_retries = 1;
    let r = on_valve(&hm, &idle, &start_retry);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].code, EventCode::CalibStarted);
    assert_eq!(r[1].code, EventCode::CalibRetry);
    // Retry count changes outside calibration are not events.
    let mut idle_retry = idle;
    idle_retry.calib_retries = 1;
    assert!(on_valve(&hm, &idle, &idle_retry).is_empty());
    // A drop is not a retry.
    assert!(on_valve(&hm, &cal_retry, &cal).is_empty());
}

#[test]
fn on_valve_blocked_and_failed_messages_claim_no_failsafe_position_or_fault() {
    let hm = HealthMonitor::default();
    let message = |e: &Event| {
        let mut buf = [0u8; 160];
        let n = format_event_message(e, &mut buf);
        buf[..n].to_vec()
    };
    let mut blocked = known(9);
    blocked.calib_retries = 3;
    let r = on_valve_with(&hm, &known(1), &blocked, true, 2);
    assert_eq!(r.len(), 1);
    assert_text(&message(&r[0]), "valve 3: blocked (calibration retries 3)");
    let r = on_valve_with(&hm, &known(1), &known(4), true, 2);
    assert_eq!(r.len(), 1);
    assert_text(&message(&r[0]), "valve 3: failed");
    let mut cal = known(1);
    cal.calibrating = true;
    let r = on_valve_with(&hm, &cal, &blocked, true, 2);
    assert_eq!(r.len(), 1);
    assert_text(
        &message(&r[0]),
        "valve 3: calibration failed after 3 retries",
    );
}

#[test]
fn on_valve_v2_reports_a_failed_calibration_with_cal_state_bit_3() {
    let hm = HealthMonitor::default();
    let mut cal = known(1);
    cal.has_extended = true;
    cal.calibrating = true;
    cal.cal_state = CAL_STATE_RUNNING;
    let mut done = known(1);
    done.has_extended = true;
    done.cal_flags = CAL_FLAG_LAST_FAILED;
    let r = on_valve(&hm, &cal, &done);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].code, EventCode::CalibFailed);
    // The early-stop flag alone is not a failure.
    done.cal_flags = CAL_FLAG_EARLY_STOP;
    let r = on_valve(&hm, &cal, &done);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].code, EventCode::CalibOk);
    // Without extended data the flag is not looked at (1.x).
    let mut v1 = known(1);
    v1.cal_flags = CAL_FLAG_LAST_FAILED;
    let mut v1cal = cal;
    v1cal.has_extended = false;
    let r = on_valve(&hm, &v1cal, &v1);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].code, EventCode::CalibOk);
}

#[test]
fn on_valve_v2_counters_flags_and_target_events() {
    let hm = HealthMonitor::default();
    let mut a = known(1);
    a.has_extended = true;
    a.early_stops = 1;
    a.cmd_rejected = 1;
    let mut b = a;
    b.early_stops = 2;
    b.last_move.stop = StopReason::EndStop;
    let r = on_valve(&hm, &a, &b);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::EarlyStop, 3, 2, 2);
    let mut b = a;
    b.cmd_rejected = 4;
    let r = on_valve(&hm, &a, &b);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::CmdRejected, 3, 4, 0);
    // First gvlvx (before without extended data): baseline, silent.
    let no_ex = known(1);
    assert!(on_valve(&hm, &no_ex, &b).is_empty());
    // Decrease (STM reboot): silent.
    assert!(on_valve(&hm, &b, &a).is_empty());

    let mut f = known(1);
    f.desired_valid = true;
    f.desired = 40;
    f.source = TargetSource::Web;
    let mut g = f;
    g.health = HEALTH_TARGET_UNCONFIRMED;
    g.push_attempts = 5;
    let r = on_valve(&hm, &f, &g);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::TargetNotConfirmed, 3, 40, 5);
    assert!(on_valve(&hm, &g, &g).is_empty()); // stays set: nothing
    let r = on_valve(&hm, &g, &f);
    assert_eq!(r.len(), 1);
    check_event(
        &r[0],
        EventCode::ValveRecovered,
        3,
        0,
        i32::from(HEALTH_TARGET_UNCONFIRMED),
    );

    let mut s = known(1);
    s.health = HEALTH_STALE;
    let r = on_valve(&hm, &known(1), &s);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::ValveStale, 3, 60, 0);
    let r = on_valve(&hm, &s, &known(1));
    assert_eq!(r.len(), 1);
    check_event(
        &r[0],
        EventCode::ValveRecovered,
        3,
        0,
        i32::from(HEALTH_STALE),
    );
    let mut both = known(1);
    both.health = HEALTH_STALE | HEALTH_TARGET_UNCONFIRMED | HEALTH_TEMP_FAILED;
    let r = on_valve(&hm, &both, &known(1));
    assert_eq!(r.len(), 1);
    check_event(
        &r[0],
        EventCode::ValveRecovered,
        3,
        0,
        i32::from(HEALTH_STALE | HEALTH_TARGET_UNCONFIRMED),
    );
    // Other flags have no transition events of their own.
    let mut other = known(1);
    other.health = HEALTH_TEMP_FAILED | HEALTH_CALIB_RETRIES | HEALTH_EARLY_STOP;
    assert!(on_valve(&hm, &known(1), &other).is_empty());
    assert!(on_valve(&hm, &other, &known(1)).is_empty());

    // TargetSet: web/mqtt changes only.
    let t0 = known(1);
    let mut t1 = t0;
    t1.desired_valid = true;
    t1.desired = 30;
    t1.source = TargetSource::Mqtt;
    let r = on_valve(&hm, &t0, &t1);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::TargetSet, 3, 30, 3);
    let mut t2 = t1;
    t2.desired = 31;
    t2.source = TargetSource::Web;
    let r = on_valve(&hm, &t1, &t2);
    assert_eq!(r.len(), 1);
    check_event(&r[0], EventCode::TargetSet, 3, 31, 2);
    assert!(on_valve(&hm, &t2, &t2).is_empty());
    let mut t3 = t2;
    t3.source = TargetSource::Mqtt; // same value, other source
    assert!(on_valve(&hm, &t2, &t3).is_empty());
    let mut stm = t2;
    stm.desired = 50;
    stm.source = TargetSource::Stm;
    assert!(on_valve(&hm, &t2, &stm).is_empty());
    let mut none = t2;
    none.desired = 50;
    none.source = TargetSource::None;
    assert!(on_valve(&hm, &t2, &none).is_empty());
    let mut invalid = t2;
    invalid.desired_valid = false;
    invalid.desired = 60;
    assert!(on_valve(&hm, &t2, &invalid).is_empty());

    // TargetSet comes before the Debug state change.
    let mut moving = t2;
    moving.status = 2;
    moving.desired = 90;
    let r = on_valve(&hm, &t2, &moving);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].code, EventCode::TargetSet);
    assert_eq!(r[1].code, EventCode::ValveStateChanged);
}

#[test]
fn on_valve_many_events_are_ordered_and_bounded() {
    let hm = HealthMonitor::default();
    let mut a = known(1);
    a.has_extended = true;
    a.health = HEALTH_STALE;
    let mut b = a;
    b.status = 9;
    b.calibrating = true;
    b.calib_retries = 1;
    b.early_stops = 1;
    b.cmd_rejected = 1;
    b.health = HEALTH_TARGET_UNCONFIRMED;
    b.desired_valid = true;
    b.desired = 10;
    b.source = TargetSource::Web;
    let mut out: [Event; MAX_EVENTS_PER_UPDATE + 4] = Default::default();
    let n = hm.on_valve(0, &a, &b, true, &mut out);
    assert_eq!(n, 8);
    assert_eq!(out[0].code, EventCode::CalibStarted);
    assert_eq!(out[1].code, EventCode::CalibRetry);
    assert_eq!(out[2].code, EventCode::ValveBlocked);
    assert_eq!(out[3].code, EventCode::EarlyStop);
    assert_eq!(out[4].code, EventCode::CmdRejected);
    assert_eq!(out[5].code, EventCode::TargetNotConfirmed);
    assert_eq!(out[6].code, EventCode::ValveRecovered);
    assert_eq!(out[7].code, EventCode::TargetSet);
    let mut few: [Event; 3] = Default::default();
    assert_eq!(hm.on_valve(0, &a, &b, true, &mut few), 3);
    assert_eq!(few[2].code, EventCode::ValveBlocked);
}

#[test]
fn on_valve_nine_events_keep_the_first_eight() {
    // Rust addition: the cap of MAX_EVENTS_PER_UPDATE holds for a longer slice (the C++ case
    // above raises exactly 8). A rising stroke warning makes 9; TargetSet is the one dropped.
    let mut hm = HealthMonitor::default();
    hm.set_min_counts(3000);
    let mut a = known(1);
    a.has_extended = true;
    a.health = HEALTH_STALE;
    let mut b = a;
    b.status = 9;
    b.calibrating = true;
    b.calib_retries = 1;
    b.early_stops = 1;
    b.cmd_rejected = 1;
    b.health = HEALTH_TARGET_UNCONFIRMED | HEALTH_STROKE_SHORT;
    b.open_count = 3100;
    b.close_count = 3200;
    b.desired_valid = true;
    b.desired = 10;
    b.source = TargetSource::Web;
    let mut out: [Event; MAX_EVENTS_PER_UPDATE + 4] = Default::default();
    assert_eq!(
        hm.on_valve(0, &a, &b, true, &mut out),
        MAX_EVENTS_PER_UPDATE
    );
    assert_eq!(out[6].code, EventCode::CalibStrokeShort);
    assert_eq!(out[7].code, EventCode::ValveRecovered);
    assert_eq!(out[8], Event::default()); // untouched
    assert_eq!(MAX_EVENTS_PER_UPDATE, 8);
}

#[test]
fn on_link() {
    let hm = HealthMonitor::default();
    let mut out: [Event; 2] = Default::default();
    assert_eq!(hm.on_link(LinkState::Up, LinkState::Up, 0, &mut out), 0);
    assert_eq!(
        hm.on_link(LinkState::Unknown, LinkState::Up, 0, &mut out),
        1
    );
    check_event(&out[0], EventCode::LinkUp, NO_VALVE, 0, 0);
    assert_eq!(
        hm.on_link(LinkState::Up, LinkState::Degraded, 1, &mut out),
        1
    );
    check_event(&out[0], EventCode::LinkDegraded, NO_VALVE, 1, 0);
    assert_eq!(
        hm.on_link(LinkState::Degraded, LinkState::Down, 5, &mut out),
        1
    );
    check_event(&out[0], EventCode::LinkDown, NO_VALVE, 5, 0);
    assert_eq!(out[0].severity, Severity::Error);
    assert_eq!(
        hm.on_link(LinkState::Booting, LinkState::Up, 0, &mut out),
        1
    );
    assert_eq!(
        hm.on_link(LinkState::Up, LinkState::Booting, 0, &mut out),
        0
    );
    assert_eq!(
        hm.on_link(LinkState::Up, LinkState::Suspended, 0, &mut out),
        0
    );
    assert_eq!(
        hm.on_link(LinkState::Up, LinkState::Unknown, 0, &mut out),
        0
    );
    assert_eq!(hm.on_link(LinkState::Up, LinkState::Down, 5, &mut []), 0); // C++ out nullptr
    assert_eq!(
        hm.on_link(LinkState::Up, LinkState::Down, 5, &mut out[..0]),
        0
    );
}

#[test]
fn on_stm_counters_increases_rate_limit_baselines() {
    let mut hm = HealthMonitor::default();
    let mut out: [Event; 2] = Default::default();
    assert_eq!(hm.on_stm_counters(1, 1, 2, 0, &mut out), 0);
    // ESP side counts from 0.
    assert_eq!(hm.on_stm_counters(3, 0, 0, 1000, &mut out), 1);
    check_event(&out[0], EventCode::StmRxOverflow, NO_VALVE, 3, 0);
    assert_eq!(hm.on_stm_counters(3, 0, 0, 2000, &mut out), 0);
    assert_eq!(hm.on_stm_counters(4, 0, 0, 1000 + 599_999, &mut out), 0); // within 10 min
    assert_eq!(hm.on_stm_counters(5, 0, 0, 1000 + 600_000, &mut out), 1);
    check_event(&out[0], EventCode::StmRxOverflow, NO_VALVE, 5, 0);
    // Pending increase reported at the next allowed time with the latest total.
    assert_eq!(hm.on_stm_counters(6, 0, 0, 700_000, &mut out), 0);
    assert_eq!(hm.on_stm_counters(6, 0, 0, 1_201_000, &mut out), 1);
    assert_eq!(out[0].arg1, 6);
    // Parse errors are tracked separately.
    assert_eq!(hm.on_stm_counters(6, 2, 0, 1_201_500, &mut out), 1);
    check_event(&out[0], EventCode::StmParseErrors, NO_VALVE, 2, 0);

    // STM side: the first observation is only the baseline.
    assert_eq!(hm.on_stm_counters(10, 20, 1, 0, &mut out), 0);
    assert_eq!(hm.on_stm_counters(10, 20, 1, 1, &mut out), 0);
    assert_eq!(hm.on_stm_counters(11, 21, 1, 2, &mut out), 2);
    check_event(&out[0], EventCode::StmRxOverflow, NO_VALVE, 11, 1);
    check_event(&out[1], EventCode::StmParseErrors, NO_VALVE, 21, 1);
    assert_eq!(hm.on_stm_counters(12, 22, 1, 600_002, &mut out[..1]), 1); // bounded by maxOut
    assert_eq!(out[0].code, EventCode::StmRxOverflow);
    // Counters restart (STM reboot): silent re-baseline.
    assert_eq!(hm.on_stm_counters(0, 0, 1, 700_000, &mut out), 0);
    assert_eq!(hm.on_stm_counters(1, 0, 1, 1_300_000, &mut out), 1);
    // Wrap-safe timing.
    let mut w = HealthMonitor::default();
    assert_eq!(w.on_stm_counters(1, 0, 0, 0xFFFF_FF00, &mut out), 1);
    assert_eq!(
        w.on_stm_counters(2, 0, 0, 0xFFFF_FF00u32.wrapping_add(599_999), &mut out),
        0
    );
    assert_eq!(
        w.on_stm_counters(2, 0, 0, 0xFFFF_FF00u32.wrapping_add(600_000), &mut out),
        1
    );
}

#[test]
fn on_temp_sensor() {
    let hm = HealthMonitor::default();
    let mut out: [Event; 1] = Default::default();
    assert_eq!(hm.on_temp_sensor(0, true, true, false, -1270, &mut out), 0);
    assert_eq!(hm.on_temp_sensor(35, true, true, false, -1270, &mut out), 0);
    assert_eq!(hm.on_temp_sensor(4, false, true, false, -1270, &mut out), 0);
    assert_eq!(hm.on_temp_sensor(4, true, true, true, 200, &mut out), 0);
    assert_eq!(hm.on_temp_sensor(4, true, false, false, 850, &mut out), 0);
    assert_eq!(hm.on_temp_sensor(4, true, true, false, -1270, &mut out), 1);
    check_event(&out[0], EventCode::TempSensorFailed, NO_VALVE, 4, -1270);
    assert_eq!(hm.on_temp_sensor(34, true, false, true, 215, &mut out), 1);
    check_event(&out[0], EventCode::TempSensorRecovered, NO_VALVE, 34, 0);
    assert_eq!(hm.on_temp_sensor(1, true, false, true, 215, &mut out), 1);
    assert_eq!(hm.on_temp_sensor(1, true, false, true, 215, &mut []), 0); // C++ out nullptr
}
