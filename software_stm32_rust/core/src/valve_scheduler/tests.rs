//! Port of test/native/test_valve_scheduler.cpp.

use super::*;
use std::format;
use std::string::String;

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

fn show(d: &Decision) -> String {
    const NAMES: [&str; 8] = [
        "None",
        "Test",
        "OpenEnd",
        "CloseEnd",
        "Open",
        "Close",
        "Learn",
        "MarkPresent",
    ];
    if d.kind == ActionKind::None {
        return String::from("None");
    }
    let mut s = format!("{} {}", NAMES[d.kind as usize], d.valve);
    if d.kind == ActionKind::Open || d.kind == ActionKind::Close {
        s += &format!(" {}", d.delta);
    }
    if d.keep_status {
        s += " keep";
    }
    if d.reference {
        s += " ref";
    }
    s
}

fn next(s: &mut ValveScheduler, v: &Views) -> String {
    show(&s.next(v, &RUN))
}

/// the valve state machine did what the decision asked, without an end stop
fn apply(v: &mut Views, d: &Decision) {
    let x = &mut v[usize::from(d.valve)];
    match d.kind {
        ActionKind::OpenEnd => x.actual = 100,
        ActionKind::CloseEnd => x.actual = 0,
        ActionKind::Open => x.actual = x.actual.wrapping_add(d.delta),
        ActionKind::Close => x.actual = x.actual.wrapping_sub(d.delta),
        _ => {}
    }
}

#[test]
fn nothing_in_safe_mode_or_while_a_temperature_cycle_holds_the_motors() {
    let mut v = idle_all();
    v[0].status = ST_UNKNOWN;
    v[3].drive = 70;
    let mut s = ValveScheduler::default();
    let safe = SchedulerInputs {
        safe_mode: true,
        hold_for_temperature: false,
    };
    let hold = SchedulerInputs {
        safe_mode: false,
        hold_for_temperature: true,
    };
    let both = SchedulerInputs {
        safe_mode: true,
        hold_for_temperature: true,
    };
    assert_eq!(show(&s.next(&v, &safe)), "None");
    assert_eq!(show(&s.next(&v, &hold)), "None");
    assert_eq!(show(&s.next(&v, &both)), "None");
    // the test index did not move
    assert_eq!(next(&mut s, &v), "Test 0");
}

#[test]
fn presence_tests_in_the_order_0_2_4_10_1_3_11_0() {
    let mut v = idle_all();
    for x in v.iter_mut() {
        x.status = ST_UNKNOWN;
    }
    let mut s = ValveScheduler::default();
    let mut order = String::new();
    for _ in 0..13 {
        order += &next(&mut s, &v);
        order += ",";
    }
    assert_eq!(
        order,
        "Test 0,Test 2,Test 4,Test 6,Test 8,Test 10,Test 1,Test 3,Test 5,Test 7,Test 9,Test 11,Test 0,"
    );
}

#[test]
fn the_test_index_advances_also_when_its_valve_is_known() {
    let mut v = idle_all();
    v[4].status = ST_UNKNOWN;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "None"); // index 0
    assert_eq!(next(&mut s, &v), "None"); // index 2
    assert_eq!(next(&mut s, &v), "Test 4");
}

#[test]
fn plain_moves_by_the_difference_to_the_end_stops_for_0_and_100() {
    let mut v = idle_all();
    v[0].drive = 70;
    v[1].drive = 100;
    v[2].drive = 20;
    v[3].drive = 0;
    let mut s = ValveScheduler::default();
    assert!(!s.first_change());
    let mut d = s.next(&v, &RUN);
    assert_eq!(show(&d), "Open 0 20");
    assert!(s.first_change());
    apply(&mut v, &d);
    d = s.next(&v, &RUN);
    assert_eq!(show(&d), "OpenEnd 1");
    apply(&mut v, &d);
    d = s.next(&v, &RUN);
    assert_eq!(show(&d), "Close 2 30");
    apply(&mut v, &d);
    d = s.next(&v, &RUN);
    assert_eq!(show(&d), "CloseEnd 3");
    apply(&mut v, &d);
    assert_eq!(next(&mut s, &v), "None");
}

#[test]
fn a_service_hold_a_request_or_a_pending_calibration_keeps_a_valve_from_step_2() {
    let mut v = idle_all();
    v[0].drive = 70;
    v[0].svc_hold = true;
    v[1].drive = 70;
    v[1].timed_learn = true;
    v[2].drive = 70;
    v[2].recal = true;
    v[2].svc_hold = true;
    v[3].drive = 70;
    v[3].calibrated = false;
    v[3].svc_hold = true;
    let mut s = ValveScheduler::default();
    // valve 1: marked present (step 3c); valves 2, 3 pending but held
    assert_eq!(next(&mut s, &v), "MarkPresent 1");
    v[1].status = ST_PRESENT;
    assert_eq!(next(&mut s, &v), "Learn 1");
    v[1] = v[4];
    assert_eq!(next(&mut s, &v), "None");
    v[3].svc_hold = false;
    assert_eq!(next(&mut s, &v), "Learn 3");
}

#[test]
fn full_open_open_circuit_retest_known_failed_valves() {
    let mut v = idle_all();
    v[2].status = ST_FULL_OPEN;
    v[2].actual = 100;
    v[2].target = 100;
    v[2].drive = 100;
    v[5].status = ST_OPEN_CIRCUIT;
    v[5].drive = 40;
    v[5].actual = 40;
    v[6].status = ST_FAILED;
    v[6].drive = 80;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "OpenEnd 2");
    v[2].status = ST_IDLE;
    assert_eq!(next(&mut s, &v), "None");
    v[5].drive = 60;
    v[5].target = 60;
    assert_eq!(next(&mut s, &v), "Test 5");
}

#[test]
fn an_open_circuit_valve_is_tested_once_per_target_change_c_3() {
    let mut v = idle_all();
    v[0].status = ST_OPEN_CIRCUIT;
    v[0].actual = 20;
    v[0].target = 20;
    v[0].drive = 20;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "None");
    v[0].target = 30;
    v[0].drive = 30;
    assert_eq!(next(&mut s, &v), "Test 0");
    v[0].actual = 30; // A_TEST "absent" takes the drive target
    for i in 0..20 {
        assert_eq!(next(&mut s, &v), "None", "call {i}");
    }
}

#[test]
fn blocked_valve_to_its_failsafe_position_with_the_status_kept_k2_2() {
    let mut v = idle_all();
    v[3].status = ST_BLOCKED;
    v[3].actual = 0;
    v[3].target = 20;
    v[3].drive = 50;
    v[3].blocked_failsafe = true;
    let mut s = ValveScheduler::default();
    let d = s.next(&v, &RUN);
    assert_eq!(show(&d), "Open 3 50 keep");
    assert!(!s.first_change());
    apply(&mut v, &d);
    assert_eq!(next(&mut s, &v), "None");
    // hold (255): the drive is the target, the source not BlockedFailsafe
    v[3].actual = 0;
    v[3].drive = 20;
    v[3].blocked_failsafe = false;
    assert_eq!(next(&mut s, &v), "None");
    v[3].drive = 50;
    v[3].blocked_failsafe = true;
    v[3].svc_hold = true;
    assert_eq!(next(&mut s, &v), "None");
    v[3].svc_hold = false;
    v[3].drive = 100;
    assert_eq!(next(&mut s, &v), "OpenEnd 3 keep");
    v[3].drive = 0;
    v[3].actual = 30;
    assert_eq!(next(&mut s, &v), "CloseEnd 3 keep");
    v[3].drive = 10;
    assert_eq!(next(&mut s, &v), "Close 3 20 keep");
}

#[test]
fn a_failed_or_blocked_valve_is_calibrated_only_on_staln_or_its_retry_c_1() {
    let mut v = idle_all();
    v[1].status = ST_BLOCKED;
    v[1].blocked_failsafe = true;
    v[1].early_learn = true;
    v[1].timed_learn = true;
    v[1].calib_flag = true;
    v[2].status = ST_FAILED;
    v[2].early_learn = true;
    v[2].timed_learn = true;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "None");
    v[1].retry_learn = true;
    assert_eq!(next(&mut s, &v), "MarkPresent 1");
    v[2].forced_learn = true;
    assert_eq!(next(&mut s, &v), "MarkPresent 2");
    // a request keeps the blocked valve from its failsafe move
    v[1].status = ST_BLOCKED;
    v[1].drive = 60;
    assert_eq!(next(&mut s, &v), "MarkPresent 1");
}

#[test]
fn needs_reference_goes_to_the_nearer_end_stop_first_w2_4() {
    let mut v = idle_all();
    v[0].needs_reference = true;
    v[0].actual = 30;
    v[0].drive = 70;
    let mut s = ValveScheduler::default();
    let d = s.next(&v, &RUN);
    assert_eq!(show(&d), "OpenEnd 0 ref");
    assert!(s.first_change());
    apply(&mut v, &d);
    s.move_ended(0, StopReason::EndStop, false, ST_IDLE);
    v[0].needs_reference = false;
    assert_eq!(next(&mut s, &v), "Close 0 30");

    let mut w = idle_all();
    w[4].needs_reference = true;
    w[4].actual = 30;
    w[4].drive = 20;
    let mut t = ValveScheduler::default();
    assert_eq!(next(&mut t, &w), "CloseEnd 4 ref");
    w[4].drive = 50;
    assert_eq!(next(&mut t, &w), "OpenEnd 4 ref");
    w[4].drive = 49;
    assert_eq!(next(&mut t, &w), "CloseEnd 4 ref");
}

#[test]
fn a_reference_move_also_when_touched_or_lease_forced_c_2() {
    let mut v = idle_all();
    v[0].needs_reference = true;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "None");
    v[0].lease_forced = true;
    assert_eq!(next(&mut s, &v), "OpenEnd 0 ref");
    v[0].lease_forced = false;
    v[0].touched = true;
    assert_eq!(next(&mut s, &v), "OpenEnd 0 ref");
    // after the reference: needs_reference and touched clear, nothing more
    v[0].touched = false;
    v[0].needs_reference = false;
    assert_eq!(next(&mut s, &v), "None");
    // touched alone does nothing for a referenced valve
    v[0].touched = true;
    assert_eq!(next(&mut s, &v), "None");
    v[0].svc_hold = true;
    v[0].needs_reference = true;
    assert_eq!(next(&mut s, &v), "None");
}

#[test]
fn a_present_valve_calibrates_after_a_change_a_request_or_when_due_c_2() {
    let mut v = idle_all();
    v[5].status = ST_PRESENT;
    v[5].calibrated = false;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "None");
    v[5].lease_forced = true;
    assert_eq!(next(&mut s, &v), "Learn 5");
    assert!(!s.first_change());
    v[5].lease_forced = false;
    v[5].touched = true;
    assert_eq!(next(&mut s, &v), "Learn 5");
    v[5].touched = false;
    v[5].forced_learn = true;
    assert_eq!(next(&mut s, &v), "Learn 5");
    v[5].forced_learn = false;
    v[5].calib_flag = true;
    assert_eq!(next(&mut s, &v), "Learn 5");
    v[5].calib_flag = false;
    v[5].retry_learn = true;
    assert_eq!(next(&mut s, &v), "Learn 5");
    v[5].retry_learn = false;
    v[5].early_learn = true;
    assert_eq!(next(&mut s, &v), "Learn 5");
    v[5].early_learn = false;
    v[5].timed_learn = true;
    assert_eq!(next(&mut s, &v), "None");
    v[5].drive = 60;
    assert_eq!(next(&mut s, &v), "Learn 5");
    assert!(s.first_change());
    v[5].drive = 50;
    assert_eq!(next(&mut s, &v), "Learn 5"); // after the first change every present valve calibrates
}

#[test]
fn a_counted_rejection_is_a_change_for_the_present_valves() {
    let mut v = idle_all();
    v[5].status = ST_PRESENT;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "None");
    s.note_change();
    assert!(s.first_change());
    assert_eq!(next(&mut s, &v), "Learn 5");
}

#[test]
fn calibration_pending_at_the_valves_own_target_change_w12_1() {
    let mut v = idle_all();
    v[0].calibrated = false;
    v[0].actual = 100;
    v[0].target = 100;
    v[0].drive = 100;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "None");
    assert!(!s.first_change());
    v[0].drive = 40;
    assert_eq!(next(&mut s, &v), "Learn 0");
    assert!(s.first_change());
    let mut w = idle_all();
    w[7].recal = true;
    w[7].touched = true;
    let mut t = ValveScheduler::default();
    assert_eq!(next(&mut t, &w), "Learn 7");
    w[7].touched = false;
    w[7].lease_forced = true;
    assert_eq!(next(&mut t, &w), "Learn 7");
}

#[test]
fn a_leased_out_valve_status_8_after_power_on_calibrates_with_drive_equal_to_actual_c_2() {
    let mut v = idle_all();
    v[2].status = ST_PRESENT;
    v[2].calibrated = false;
    v[2].lease_forced = true;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "Learn 2");
}

#[test]
fn requests_of_known_valves_mark_them_present_not_unknown_or_present_ones() {
    let mut v = idle_all();
    v[0].status = ST_UNKNOWN;
    v[0].forced_learn = true;
    v[1].status = ST_OPEN_CIRCUIT;
    v[1].forced_learn = true;
    v[2].calib_flag = true;
    v[3].early_learn = true;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "Test 0");
    assert_eq!(next(&mut s, &v), "MarkPresent 1");
    assert_eq!(next(&mut s, &v), "MarkPresent 2");
    assert_eq!(next(&mut s, &v), "MarkPresent 3");
    v[1].status = ST_PRESENT;
    v[2].status = ST_PRESENT;
    v[3].status = ST_PRESENT;
    v[1].forced_learn = false;
    v[2].calib_flag = false;
    v[3].early_learn = false;
    assert_eq!(next(&mut s, &v), "None");
}

#[test]
fn moves_of_every_valve_before_the_next_calibration_s8_1() {
    let mut v = idle_all();
    v[3].status = ST_PRESENT;
    v[3].forced_learn = true;
    v[5].drive = 80;
    let mut s = ValveScheduler::default();
    let d = s.next(&v, &RUN);
    assert_eq!(show(&d), "Open 5 30");
    apply(&mut v, &d);
    assert_eq!(next(&mut s, &v), "Learn 3");

    let mut w = idle_all();
    w[0].status = ST_PRESENT;
    w[0].forced_learn = true;
    w[1].status = ST_PRESENT;
    w[1].forced_learn = true;
    w[11].drive = 10;
    let mut t = ValveScheduler::default();
    assert_eq!(next(&mut t, &w), "Close 11 40");
}

#[test]
fn round_robin_continues_after_the_valve_of_the_last_decision() {
    let mut v = idle_all();
    v[2].drive = 60;
    v[8].drive = 60;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "Open 2 10");
    assert_eq!(next(&mut s, &v), "Open 8 10");
    assert_eq!(next(&mut s, &v), "Open 2 10");
    v[8].status = ST_PRESENT;
    v[1].status = ST_PRESENT;
    v[2].drive = 50;
    // step 3 scans from valve 3 as well
    assert_eq!(next(&mut s, &v), "Learn 8");
    assert_eq!(next(&mut s, &v), "Learn 1");
}

#[test]
fn after_12_step_2_decisions_in_a_row_a_calibration_goes_first_c_3() {
    let mut v = idle_all();
    v[0].drive = 70; // never reaches it: a permanent step-2 item
    v[6].status = ST_PRESENT;
    v[6].forced_learn = true;
    let mut s = ValveScheduler::default();
    let mut learn_at = 0;
    for i in 1..=100 {
        if next(&mut s, &v) == "Learn 6" {
            learn_at = i;
            break;
        }
    }
    assert_eq!(learn_at, 13);
    // the run starts again
    v[6].status = ST_IDLE;
    v[6].forced_learn = false;
    v[9].status = ST_PRESENT;
    v[9].forced_learn = true;
    learn_at = 0;
    for i in 1..=100 {
        if next(&mut s, &v) == "Learn 9" {
            learn_at = i;
            break;
        }
    }
    assert_eq!(learn_at, 13);
}

#[test]
fn fairness_looks_at_step_3_first_only_while_it_has_something() {
    let mut v = idle_all();
    v[0].drive = 70;
    let mut s = ValveScheduler::default();
    for i in 0..20 {
        assert_eq!(next(&mut s, &v), "Open 0 20", "call {i}");
    }
    v[4].status = ST_PRESENT;
    v[4].forced_learn = true;
    assert_eq!(next(&mut s, &v), "Learn 4");
    for i in 0..12 {
        assert_eq!(next(&mut s, &v), "Open 0 20", "call {i}");
    }
    assert_eq!(next(&mut s, &v), "Learn 4");
}

#[test]
fn a_decision_without_step_2_work_ends_the_run_of_step_2_decisions_c_3() {
    let mut v = idle_all();
    v[0].drive = 70;
    let mut s = ValveScheduler::default();
    for i in 0..11 {
        assert_eq!(next(&mut s, &v), "Open 0 20", "call {i}");
    }
    v[0].drive = 50;
    assert_eq!(next(&mut s, &v), "None");
    v[0].drive = 70;
    v[4].status = ST_PRESENT;
    v[4].forced_learn = true;
    for i in 0..12 {
        assert_eq!(next(&mut s, &v), "Open 0 20", "call {i}");
    }
    assert_eq!(next(&mut s, &v), "Learn 4");
}

#[test]
fn blocked_valve_at_28_pct_after_its_failsafe_move_met_the_end_stop_c_1() {
    let mut v = idle_all();
    v[3].status = ST_BLOCKED;
    v[3].actual = 0;
    v[3].drive = 50;
    v[3].blocked_failsafe = true;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "Open 3 50 keep");
    v[3].actual = 28;
    s.move_ended(3, StopReason::EarlyEndStop, true, ST_BLOCKED);
    assert!(s.latched(3));
    for i in 0..1000 {
        assert_eq!(next(&mut s, &v), "None", "moved again at call {i}");
    }
    // a new failsafe position clears the latch
    v[3].drive = 60;
    assert_eq!(next(&mut s, &v), "Open 3 32 keep");
    assert!(!s.latched(3));
}

#[test]
fn a_non_early_end_stop_is_not_repeated_drive_95_actual_94() {
    let mut v = idle_all();
    v[0].actual = 60;
    v[0].drive = 95;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "Open 0 35");
    v[0].actual = 94;
    s.move_ended(0, StopReason::EndStop, false, ST_IDLE);
    for i in 0..50 {
        assert_eq!(next(&mut s, &v), "None", "call {i}");
    }
    // the other direction is allowed and clears the latch at its end
    v[0].drive = 90;
    assert_eq!(next(&mut s, &v), "Close 0 4");
    v[0].actual = 90;
    s.move_ended(0, StopReason::Target, false, ST_IDLE);
    assert!(!s.latched(0));
    v[0].drive = 95;
    assert_eq!(next(&mut s, &v), "Open 0 5");
}

#[test]
fn an_early_end_stop_gets_exactly_one_retry() {
    let mut v = idle_all();
    v[2].actual = 20;
    v[2].drive = 60;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "Open 2 40");
    v[2].actual = 25;
    s.move_ended(2, StopReason::EarlyEndStop, true, ST_IDLE);
    assert_eq!(next(&mut s, &v), "Open 2 35");
    s.move_ended(2, StopReason::EarlyEndStop, true, ST_IDLE);
    assert!(s.latched(2));
    for i in 0..20 {
        assert_eq!(next(&mut s, &v), "None", "call {i}");
    }
    // an early safety stop counts like an early end stop
    let mut w = idle_all();
    w[1].drive = 80;
    let mut t = ValveScheduler::default();
    assert_eq!(next(&mut t, &w), "Open 1 30");
    w[1].actual = 55;
    t.move_ended(1, StopReason::SafetyOvercurrent, true, ST_IDLE);
    assert_eq!(next(&mut t, &w), "Open 1 25");
    t.move_ended(1, StopReason::SafetyOvercurrent, false, ST_IDLE);
    assert_eq!(next(&mut t, &w), "None");
}

#[test]
fn a_failsafe_move_gets_no_retry_after_an_early_end_stop() {
    let mut v = idle_all();
    v[3].status = ST_BLOCKED;
    v[3].actual = 0;
    v[3].drive = 50;
    v[3].blocked_failsafe = true;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "Open 3 50 keep");
    v[3].actual = 10;
    s.move_ended(3, StopReason::SafetyOvercurrent, true, ST_BLOCKED);
    assert_eq!(next(&mut s, &v), "None");
}

#[test]
fn the_latch_clears_with_the_status_a_calibration_a_test_or_clear_latch() {
    let mut v = idle_all();
    v[3].status = ST_BLOCKED;
    v[3].actual = 0;
    v[3].drive = 50;
    v[3].blocked_failsafe = true;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "Open 3 50 keep");
    v[3].actual = 28;
    s.move_ended(3, StopReason::EndStop, false, ST_BLOCKED);
    assert_eq!(next(&mut s, &v), "None");
    // the retry calibration: marked present, calibrated, blocked again at 0
    v[3].retry_learn = true;
    assert_eq!(next(&mut s, &v), "MarkPresent 3");
    assert!(!s.latched(3));
    v[3].retry_learn = false;
    v[3].actual = 0;
    assert_eq!(next(&mut s, &v), "Open 3 50 keep");
    v[3].actual = 28;
    s.move_ended(3, StopReason::EndStop, false, ST_BLOCKED);
    s.clear_latch(3);
    assert!(!s.latched(3));
    s.clear_latch(12);
    assert!(!s.latched(12));
    // a status change clears it too
    v[3].actual = 0;
    assert_eq!(next(&mut s, &v), "Open 3 50 keep");
    v[3].actual = 28;
    s.move_ended(3, StopReason::EndStop, false, ST_BLOCKED);
    v[3].status = ST_IDLE;
    v[3].blocked_failsafe = false;
    v[3].drive = 50;
    assert_eq!(next(&mut s, &v), "Open 3 22");
}

#[test]
fn a_reference_move_and_other_stop_reasons_set_no_latch() {
    let mut v = idle_all();
    v[0].needs_reference = true;
    v[0].drive = 60;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "OpenEnd 0 ref");
    s.move_ended(0, StopReason::EndStop, false, ST_IDLE);
    assert!(!s.latched(0));
    v[0].needs_reference = false;
    v[0].actual = 40;
    assert_eq!(next(&mut s, &v), "Open 0 20");
    s.move_ended(0, StopReason::Timeout, false, ST_FAILED);
    assert!(!s.latched(0));
    s.move_ended(0, StopReason::EndStop, false, ST_IDLE); // no move pending
    assert!(!s.latched(0));
    s.move_ended(12, StopReason::EndStop, false, ST_IDLE);
    let others = [
        StopReason::Target,
        StopReason::Undercurrent,
        StopReason::Aborted,
        StopReason::None,
    ];
    for r in others {
        assert_eq!(next(&mut s, &v), "Open 0 20", "{r:?}");
        s.move_ended(0, r, true, ST_IDLE);
        assert!(!s.latched(0), "{r:?}");
    }
    assert_eq!(next(&mut s, &v), "Open 0 20");
    s.move_ended(0, StopReason::EndStop, false, ST_IDLE);
    assert!(s.latched(0));
    // the next test of the valve clears it
    v[0].status = ST_OPEN_CIRCUIT;
    v[0].drive = 30;
    assert_eq!(next(&mut s, &v), "Test 0");
    assert!(!s.latched(0));
}

#[test]
fn a_full_open_move_ends_at_the_end_stop_without_blocking_later_moves() {
    let mut v = idle_all();
    v[0].status = ST_FULL_OPEN;
    v[0].drive = 100;
    let mut s = ValveScheduler::default();
    assert_eq!(next(&mut s, &v), "OpenEnd 0");
    v[0].status = ST_IDLE;
    v[0].actual = 100;
    s.move_ended(0, StopReason::EndStop, false, ST_IDLE);
    v[0].drive = 30;
    assert_eq!(next(&mut s, &v), "Close 0 70");
}
