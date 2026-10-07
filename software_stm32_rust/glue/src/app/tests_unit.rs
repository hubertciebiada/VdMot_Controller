// No C++ counterpart in glue_app: staop of a failed or blocked valve, the end-stop latch after a
// calibration record, the movement trigger of a valve whose count is not down or whose
// calibration is requested already, and the retry attempts of a valve waiting for its test or
// calibration. The C++ reaches some of them only through glue_system; these cases pin the
// behaviour of app.cpp at the module.
use std::string::String;
use std::vec::Vec;

use vdm_stm_core::move_classifier::StopReason;
use vdm_stm_core::valve_codes::{ST_BLOCKED, ST_FAILED, ST_IDLE, ST_PRESENT, ST_UNKNOWN};

use crate::motor::CalibState;

use super::stub_env::AppBench;

fn calls(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| String::from(*s)).collect()
}

fn none() -> Vec<String> {
    Vec::new()
}

/// app_setup, then every valve idle, calibrated, connected and at its target 50
fn begin() -> AppBench {
    let mut b = AppBench::new();
    b.app_setup();
    for mot in &mut b.m.mots {
        mot.status = ST_IDLE;
        mot.calibrated = true;
        mot.connected = true;
        mot.scaler = 36;
    }
    b.env.log.clear();
    b.env.take_tx();
    b
}

#[test]
fn staop_of_a_failed_or_blocked_valve_only_sets_the_target_counted_as_rejected() {
    let mut b = begin();
    // hold: the blocked valve has no failsafe move of its own
    b.app.app_set_failsafe(255, 255);
    b.m.mots[3].status = ST_FAILED;
    b.m.mots[4].status = ST_BLOCKED;
    // both were driven to 50 last
    b.m.valves[3].rejected_target = 50;
    b.m.valves[4].rejected_target = 50;
    assert_eq!(b.app_set_valveopen(3), 0);
    assert_eq!(b.app_set_valveopen(4), 0);
    assert_eq!(b.loop_actions(1), none());
    assert_eq!(b.m.mots[3].status, ST_FAILED);
    assert_eq!(b.m.mots[4].status, ST_BLOCKED);
    assert!(!b.m.valves[3].open_request);
    assert!(!b.m.valves[4].open_request);
    assert_eq!(b.m.valves[3].cmd_rejected, 1);
    assert_eq!(b.m.valves[4].cmd_rejected, 1);
}

#[test]
fn a_successful_calibration_record_clears_the_end_stop_latch_a_blocked_one_does_not() {
    for (failed, again) in [(false, true), (true, false)] {
        let mut b = begin();
        b.m.mots[0].target_position = 70;
        assert_eq!(b.loop_actions(1), calls(&["appsetaction(o, 0, 20, 0)"]));
        // the move ends at an end stop at 55 %: no second move that way
        b.m.mots[0].actual_position = 55;
        b.m.mots[0].move_seq = 1;
        b.env.snapshot[0].diag.last.stop_reason = StopReason::EndStop as u8;
        b.env.snapshot[0].status = ST_IDLE;
        assert_eq!(b.loop_actions(1), none());
        // the record of a calibration of the valve
        b.m.mots[0].calib_seq = 1;
        b.m.mots[0].calib_failed = failed;
        let want = if again {
            calls(&["appsetaction(o, 0, 15, 0)"])
        } else {
            none()
        };
        assert_eq!(b.loop_actions(1), want, "calib_failed {failed}");
    }
}

#[test]
fn the_movement_trigger_needs_its_count_at_0_and_no_calibration_in_hand() {
    let mut b = begin();
    b.app_set_learnmovements(50);
    // valve 1: the count is not down; valve 2: down, a calibration is requested already
    b.m.valves[2].learn_movements = 0;
    b.m.mots[2].calib_state = CalibState::Started;
    b.m.mots[2].calib_time = 7;
    b.app_10s_loop(10);
    assert!(!b.m.mots[1].calibration);
    assert_eq!(b.m.valves[1].learn_movements, 50);
    assert!(!b.m.mots[2].calibration);
    assert_eq!(b.m.mots[2].calib_time, 7);
    assert!(b.env.take_tx().is_empty());
}

#[test]
fn retry_attempts_stay_while_the_valve_waits_for_its_test_or_calibration() {
    for waiting in [ST_PRESENT, ST_UNKNOWN] {
        let mut b = begin();
        b.app.app_lease_configure(0);
        b.m.mots[3].status = ST_BLOCKED;
        // the first retry: scheduled, due after 1 h
        b.app_1s_tick(1);
        b.app_1s_tick(3600);
        assert!(b.m.valves[3].retry_learn);
        assert_eq!(b.v3(3).retries, 1);
        // app_loop took the request: the valve waits for its calibration (or test)
        b.m.valves[3].retry_learn = false;
        b.m.mots[3].status = waiting;
        b.app_1s_tick(1);
        assert_eq!(b.v3(3).retries, 1, "status {waiting}");
        // blocked again: the second retry comes after 6 h
        b.m.mots[3].status = ST_BLOCKED;
        b.app_1s_tick(1);
        assert_eq!(b.v3(3).retry_s, 21600, "status {waiting}");
    }
}
