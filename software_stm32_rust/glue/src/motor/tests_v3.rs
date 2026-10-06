// Port of software_stm32/test/native/glue/test_motor_v3.cpp: motor.cpp with the valve sim
// (glue_motor, every case for both board revisions): partial end stops (W9), keep-status and
// reference moves (K2, W2), sstop (S8), calibration results (W2, S2, S14), presence test short
// (W10), inrush limit (C-4, report only and enforced), the temperature gap between strokes (S1).
use core::sync::atomic::Ordering;
use std::string::String;
use std::vec;

use vdm_stm_core::move_classifier::{StopReason, DIR_OPEN};
use vdm_stm_core::valve_codes::{
    ValveFault, ST_BLOCKED, ST_FAILED, ST_IDLE, ST_OPEN_CIRCUIT, ST_PRESENT,
};

use crate::board::BoardRev;

use super::bench::{start_valves, states, Bench, REVS};
use super::{
    AState, CalibState, CMD_A_CLOSE, CMD_A_CLOSE_END, CMD_A_LEARN, CMD_A_OPEN, CMD_A_OPEN_END,
    CMD_A_TEST, MOVE_KEEP_STATUS, MOVE_REFERENCE, SVMOV_COUNTS_MAX,
};

fn reason(r: StopReason) -> u8 {
    r as u8
}

fn calibration_done(b: &Bench, v: usize) -> bool {
    !b.m.mots[v].calib_active && b.state() == AState::Idle
}

fn partial_move_at_an_obstacle(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(0, 20);
    // 25 %
    b.rig.valve[0].jam_from = 900;
    b.rig.valve[0].jam_to = 3600;
    b.run(CMD_A_OPEN, 0, 40, 0);
    assert_eq!(b.m.mots[0].actual_position, 25);
    assert_eq!(b.m.mots[0].status, ST_IDLE);
    assert_eq!(b.last(0).stop_reason, reason(StopReason::EarlyEndStop));
    assert_eq!(b.last(0).requested_counts, 40 * 36);
    assert_eq!(b.diag(0).early_stops, 1);
    assert!(b.diag(0).early_warn);
    assert!(b.diag(0).last_early);
    assert_eq!(b.diag(0).early_run.count(), 1);
    assert!(!b.m.mots[0].early_learn_due);
    assert_eq!(b.m.mots[0].move_seq, 1);
    // the retry stops early again: a calibration is requested
    b.run(CMD_A_OPEN, 0, 35, 0);
    assert_eq!(b.m.mots[0].actual_position, 25);
    assert_eq!(b.diag(0).early_stops, 2);
    assert_eq!(b.diag(0).early_run.count(), 0);
    assert!(b.m.mots[0].early_learn_due);
    assert_eq!(b.m.mots[0].move_seq, 2);
}

#[test]
fn w9_a_partial_move_that_meets_an_obstacle_takes_its_position_from_the_counted_pulses() {
    for rev in REVS {
        partial_move_at_an_obstacle(rev);
    }
}

#[test]
fn w9_an_end_stop_after_80_of_the_requested_pulses_is_not_early_the_position_still_counted() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.place(0, 60);
        // the open end stop at 95 %
        b.rig.valve[0].stroke = 3420;
        b.run(CMD_A_OPEN, 0, 40, 0);
        assert_eq!(b.last(0).stop_reason, reason(StopReason::EndStop));
        assert!(!b.diag(0).last_early);
        assert_eq!(b.diag(0).early_stops, 0);
        assert_eq!(b.diag(0).early_run.count(), 0);
        assert_eq!(
            b.m.mots[0].actual_position,
            (60 + (3420 - 60 * 36) / 36) as u8
        );
    }
}

fn keep_status_move(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(1, 0);
    b.m.mots[1].status = ST_BLOCKED;
    // 28 %
    b.rig.valve[1].jam_from = 1008;
    b.rig.valve[1].jam_to = 3600;
    b.run(CMD_A_OPEN, 1, 50, MOVE_KEEP_STATUS);
    assert_eq!(b.m.mots[1].status, ST_BLOCKED);
    // 9 also while it moves
    for s in &b.rig.transitions {
        assert!(!(s.valve == 1 && s.status != ST_BLOCKED), "{s:?}");
    }
    assert_eq!(b.m.mots[1].actual_position, 28);
    assert_eq!(b.last(1).stop_reason, reason(StopReason::EndStop));
    assert_eq!(b.diag(1).early_stops, 0);
    assert_eq!(b.diag(1).early_run.count(), 0);
    assert_eq!(b.m.mots[1].move_seq, 1);
    // a free move to the target keeps it too
    b.rig.valve[1].jam_from = -1;
    b.run(CMD_A_OPEN, 1, 10, MOVE_KEEP_STATUS);
    assert_eq!(b.m.mots[1].status, ST_BLOCKED);
    assert_eq!(b.m.mots[1].actual_position, 38);
    assert_eq!(b.last(1).stop_reason, reason(StopReason::Target));
    b.run(CMD_A_CLOSE_END, 1, 0, MOVE_KEEP_STATUS);
    assert_eq!(b.m.mots[1].status, ST_BLOCKED);
    assert_eq!(b.m.mots[1].actual_position, 0);
    assert!(!b.diag(1).last_early);
}

#[test]
fn k2_a_keep_status_move_of_a_blocked_valve_keeps_status_9_and_never_feeds_the_early_run() {
    for rev in REVS {
        keep_status_move(rev);
    }
}

fn reference_move(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(2, 30);
    // the valve really stands near the open end
    b.rig.valve[2].position = 3000;
    b.m.mots[2].needs_reference = true;
    b.run(CMD_A_OPEN_END, 2, 0, MOVE_REFERENCE);
    assert_eq!(b.m.mots[2].actual_position, 100);
    assert!(!b.m.mots[2].needs_reference);
    assert_eq!(b.last(2).stop_reason, reason(StopReason::EndStop));
    assert_eq!(b.diag(2).early_stops, 0);
    // without the flag the same short stroke is an early end stop of an end-stop move
    b.place(3, 30);
    b.rig.valve[3].position = 3400;
    b.run(CMD_A_OPEN_END, 3, 0, 0);
    assert_eq!(b.last(3).stop_reason, reason(StopReason::EarlyEndStop));
    assert_eq!(b.m.mots[3].actual_position, 100);
}

#[test]
fn w2_a_reference_move_to_the_end_stop_is_never_early_and_references_the_valve() {
    for rev in REVS {
        reference_move(rev);
    }
}

#[test]
fn w2_a_move_to_an_end_stop_that_ends_there_clears_needs_reference_a_partial_one_not() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.place(4, 50);
        b.m.mots[4].needs_reference = true;
        b.run(CMD_A_OPEN, 4, 10, 0);
        assert!(b.m.mots[4].needs_reference);
        b.run(CMD_A_CLOSE_END, 4, 0, 0);
        assert!(!b.m.mots[4].needs_reference);
        assert_eq!(b.m.mots[4].actual_position, 0);
    }
}

fn sstop_service_move(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(2, 10);
    b.m.valves[2].svc_hold = 0;
    assert_eq!(b.appsetservice(2, DIR_OPEN, 10000, 40), 0);
    b.run_ms(2000);
    assert_eq!(b.m.appstop(5), -1);
    assert_eq!(b.m.appstop(2), 2);
    assert!(b.run_until(Bench::idle, 5000));
    let r = b.last(2);
    assert_eq!(r.stop_reason, reason(StopReason::Aborted));
    assert!(r.counted_counts > 100);
    assert!(r.counted_counts < 10000);
    assert_eq!(
        b.m.mots[2].actual_position,
        (10 + r.counted_counts / 36) as u8
    );
    assert_eq!(b.m.mots[2].status, ST_IDLE);
    assert_eq!(b.m.appstop(255), -1);
}

#[test]
fn s8_sstop_during_a_service_move_stops_it_where_the_pulses_took_it() {
    for rev in REVS {
        sstop_service_move(rev);
    }
}

fn sstop_normal_and_keep_status(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(0, 10);
    b.rig.valve[0].pulses_per_ms = 0.1;
    assert_eq!(b.appsetaction(CMD_A_OPEN, 0, 80), 0);
    b.run_ms(3000);
    assert_eq!(b.m.appstop(255), 0);
    assert!(b.run_until(Bench::idle, 5000));
    assert_eq!(b.last(0).stop_reason, reason(StopReason::Aborted));
    assert_eq!(
        b.m.mots[0].actual_position,
        (10 + b.last(0).counted_counts / 36) as u8
    );
    assert!(b.m.mots[0].actual_position < 90);
    assert_eq!(b.m.mots[0].status, ST_IDLE);
    assert_eq!(b.m.mots[0].move_seq, 1);
    b.m.mots[0].status = ST_BLOCKED;
    assert_eq!(
        b.appsetaction_with(CMD_A_CLOSE_END, 0, 0, false, MOVE_KEEP_STATUS),
        0
    );
    b.run_ms(3000);
    assert_eq!(b.m.appstop(0), 0);
    assert!(b.run_until(Bench::idle, 5000));
    assert_eq!(b.m.mots[0].status, ST_BLOCKED);
    assert_eq!(b.last(0).stop_reason, reason(StopReason::Aborted));
}

#[test]
fn s8_sstop_of_a_normal_move_and_of_a_keep_status_move() {
    for rev in REVS {
        sstop_normal_and_keep_status(rev);
    }
}

fn sstop_before_the_motor_started(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(3, 40);
    assert_eq!(b.appsetaction(CMD_A_OPEN, 3, 20), 0);
    assert_eq!(b.m.valve_busy_index(), 3);
    // not taken yet: dropped
    assert_eq!(b.m.appstop(3), 3);
    assert!(b.idle());
    assert_eq!(b.m.valve_busy_index(), -1);
    b.run_ms(100);
    assert_eq!(b.rig.valve[3].enables, 0);
    // taken, waiting for the PSU (Open1)
    assert_eq!(b.appsetaction(CMD_A_OPEN, 3, 20), 0);
    b.run_ms(20);
    assert_eq!(b.state(), AState::Open1);
    assert_eq!(b.m.valve_busy_index(), 3);
    assert_eq!(b.m.appstop(3), 3);
    b.run_ms(20);
    assert_eq!(b.state(), AState::Idle);
    assert_eq!(b.last(3).stop_reason, reason(StopReason::Aborted));
    assert_eq!(b.last(3).counted_counts, 0);
    assert_eq!(b.m.mots[3].actual_position, 40);
    assert_eq!(b.rig.valve[3].enables, 0);
    // a service move waiting for the PSU
    assert_eq!(b.appsetservice(3, 1, 100, 30), 0);
    b.run_ms(20);
    assert_eq!(b.state(), AState::Svc1);
    assert_eq!(b.m.appstop(255), 3);
    b.run_ms(20);
    assert_eq!(b.state(), AState::Idle);
    assert_eq!(b.rig.valve[3].enables, 0);
}

#[test]
fn s8_sstop_before_the_motor_started_the_command_is_dropped_or_the_start_refused() {
    for rev in REVS {
        sstop_before_the_motor_started(rev);
    }
}

fn sstop_in_a_calibration(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(0, 50);
    b.m.mots[0].calibrated = true;
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    b.m.mots[0].calibration = true;
    b.m.mots[0].calib_state = CalibState::InProgress;
    b.run_ms(1500);
    assert!(b.m.mots[0].calib_active);
    assert_eq!(b.m.appstop(0), 0);
    assert!(b.run_until(Bench::idle, 5000));
    let mot = b.m.mots[0];
    assert_eq!(mot.status, ST_IDLE);
    assert!(mot.needs_reference);
    assert!(!mot.calib_active);
    assert!(!mot.calibration);
    assert_eq!(mot.calib_state, CalibState::Idle);
    assert_eq!(mot.calib_seq, 0);
    assert!(!b.diag(0).last_cal_failed);
    assert_eq!(b.last(0).stop_reason, reason(StopReason::Aborted));
    // stopped right after the hand-over (Learn1)
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    b.run_ms(10);
    assert_eq!(b.state(), AState::Learn1);
    b.m.appstop(0);
    b.run_ms(10);
    assert_eq!(b.state(), AState::Idle);
    assert!(!b.m.mots[0].calib_active);
}

#[test]
fn s8_sstop_in_a_calibration_ends_it_without_a_result_the_position_unreferenced() {
    for rev in REVS {
        sstop_in_a_calibration(rev);
    }
}

fn accepted_calibration(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.m.learning_movements = 77;
    b.m.valves[0].movements = 5;
    b.m.valves[0].learn_movements = 3;
    b.m.mots[0].recal = true;
    b.m.mots[0].needs_reference = true;
    b.m.mots[0].fault_reason = 4;
    b.m.mots[0].status = ST_PRESENT;
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    b.run_ms(10);
    assert!(b.run_until(|b| calibration_done(b, 0), 30000));
    let mot = b.m.mots[0];
    assert!(mot.calibrated);
    assert!(!mot.recal);
    assert!(!mot.needs_reference);
    assert_eq!(mot.fault_reason, 0);
    assert!(!mot.calib_failed);
    assert_eq!(mot.calib_seq, 1);
    // S14-1: gvlvx moves 0 after every successful calibration
    assert_eq!(b.m.valves[0].movements, 0);
    assert_eq!(b.m.valves[0].learn_movements, 77);
    assert_eq!(mot.status, ST_IDLE);
}

#[test]
fn w2_s14_an_accepted_calibration_marks_the_valve_calibrated_and_restarts_the_movement_trigger() {
    for rev in REVS {
        accepted_calibration(rev);
    }
}

fn strokes_too_short(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.rig.valve[0].stroke = 2500;
    b.rig.valve[0].position = 1200;
    b.m.no_of_min_counts = 3000;
    b.m.mots[0].target_position = 40;
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    b.run_ms(10);
    assert!(b.run_until(|b| calibration_done(b, 0), 30000));
    let mot = b.m.mots[0];
    assert_eq!(mot.status, ST_BLOCKED);
    assert_eq!(mot.fault_reason, ValveFault::StrokesTooShort as u8);
    assert!(mot.calib_failed);
    assert_eq!(mot.calib_seq, 1);
    assert!(!mot.calibrated);
    assert_eq!(mot.actual_position, 0);
    assert_eq!(mot.scaler, 89);
    assert!(b.diag(0).last_cal_failed);
}

#[test]
fn k2_w2_strokes_too_short_block_the_valve_the_record_is_marked_failed() {
    for rev in REVS {
        strokes_too_short(rev);
    }
}

fn contact_lost_in_a_calibration(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[3].connected = false;
    b.m.mots[3].calibrated = true;
    b.m.mots[3].actual_position = 20;
    b.m.mots[3].target_position = 70;
    assert_eq!(b.appsetaction(CMD_A_LEARN, 3, 0), 0);
    b.run_ms(10);
    assert!(b.run_until(Bench::idle, 30000));
    let mot = b.m.mots[3];
    assert_eq!(mot.status, ST_OPEN_CIRCUIT);
    assert_eq!(mot.target_position, 70);
    assert_eq!(mot.actual_position, 70);
    assert!(mot.recal);
    assert!(!mot.connected);
    assert_eq!(mot.calib_seq, 0);
    // in a move and a service move too
    b.place(4, 30);
    b.rig.valve[4].connected = false;
    b.run(CMD_A_CLOSE, 4, 10, 0);
    assert!(b.m.mots[4].recal);
    assert_eq!(b.m.mots[4].actual_position, 30);
    b.place(5, 30);
    b.rig.valve[5].connected = false;
    assert_eq!(b.appsetservice(5, DIR_OPEN, 100, 30), 0);
    b.run_ms(10);
    assert!(b.run_until(Bench::idle, 30000));
    assert!(b.m.mots[5].recal);
    assert_eq!(b.m.mots[5].status, ST_OPEN_CIRCUIT);
}

#[test]
fn s2_contact_lost_in_a_calibration_keeps_the_target_the_valve_needs_a_full_calibration() {
    for rev in REVS {
        contact_lost_in_a_calibration(rev);
    }
}

fn presence_test_results(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.m.mots[2].calibrated = true;
    b.m.mots[2].fault_reason = 1;
    b.run(CMD_A_TEST, 2, 0, 0);
    assert_eq!(b.m.mots[2].status, ST_IDLE);
    assert!(b.m.mots[2].needs_reference);
    assert_eq!(b.m.mots[2].fault_reason, 0);
    b.m.mots[2].recal = true;
    b.run(CMD_A_TEST, 2, 0, 0);
    assert_eq!(b.m.mots[2].status, ST_PRESENT);
    b.rig.valve[3].connected = false;
    b.m.mots[3].actual_position = 0;
    b.m.mots[3].target_position = 35;
    b.m.mots[3].fault_reason = 3;
    b.run(CMD_A_TEST, 3, 0, 0);
    assert_eq!(b.m.mots[3].status, ST_OPEN_CIRCUIT);
    assert_eq!(b.m.mots[3].actual_position, 35);
    assert_eq!(b.m.mots[3].fault_reason, 0);
    assert_eq!(b.m.mots[3].trip_seq, 0);
}

#[test]
fn presence_test_a_calibrated_valve_is_idle_and_unreferenced_an_absent_one_takes_its_target() {
    for rev in REVS {
        presence_test_results(rev);
    }
}

#[test]
fn w10_a_short_in_the_presence_test_is_reported_only_by_default() {
    for rev in REVS {
        let mut b = start_valves(rev);
        assert!(!b.flags.protect_enforce.load(Ordering::SeqCst));
        b.rig.valve[4].shorted = true;
        b.rig.valve[4].short_current_dma = 2600;
        b.run(CMD_A_TEST, 4, 0, 0);
        assert_eq!(b.m.mots[4].status, ST_PRESENT);
        assert_eq!(b.m.mots[4].fault_reason, ValveFault::Short as u8);
        assert_eq!(b.m.mots[4].trip_seq, 1);
        // a healthy motor
        b.run(CMD_A_TEST, 5, 0, 0);
        assert_eq!(b.m.mots[5].fault_reason, 0);
        assert_eq!(b.m.mots[5].trip_seq, 0);
    }
}

#[test]
fn w10_an_enforced_short_fails_the_valve_the_protection_guard_switches_the_check_off() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.flags.protect_enforce.store(true, Ordering::SeqCst);
        b.rig.valve[4].shorted = true;
        b.rig.valve[4].short_current_dma = 2600;
        b.run(CMD_A_TEST, 4, 0, 0);
        assert_eq!(b.m.mots[4].status, ST_FAILED);
        assert_eq!(b.m.mots[4].fault_reason, ValveFault::Short as u8);
        assert_eq!(b.m.mots[4].trip_seq, 1);
        assert_eq!(b.rig.valve[4].pulses, 0);
        b.flags.protect_suspended.store(true, Ordering::SeqCst);
        b.run(CMD_A_TEST, 4, 0, 0);
        assert_eq!(b.m.mots[4].status, ST_PRESENT);
        assert_eq!(b.m.mots[4].fault_reason, 0);
        assert_eq!(b.m.mots[4].trip_seq, 1);
    }
}

fn inrush_reported(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(6, 20);
    b.rig.valve[6].inrush_peak_dma = 2800;
    b.rig.valve[6].inrush_ms = 60;
    b.run(CMD_A_OPEN, 6, 20, 0);
    assert_eq!(b.m.mots[6].status, ST_IDLE);
    assert_eq!(b.m.mots[6].actual_position, 40);
    assert_eq!(b.m.mots[6].fault_reason, ValveFault::InrushTrip as u8);
    assert_eq!(b.m.mots[6].trip_seq, 1);
    assert_eq!(b.last(6).stop_reason, reason(StopReason::Target));
    // a short inrush peak is normal
    b.place(7, 20);
    b.rig.valve[7].inrush_peak_dma = 2800;
    b.rig.valve[7].inrush_ms = 15;
    b.run(CMD_A_OPEN, 7, 20, 0);
    assert_eq!(b.m.mots[7].fault_reason, 0);
    assert_eq!(b.m.mots[7].trip_seq, 0);
}

#[test]
fn c4_an_inrush_above_the_limit_is_reported_only_by_default_the_move_goes_on() {
    for rev in REVS {
        inrush_reported(rev);
    }
}

fn inrush_enforced(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.flags.protect_enforce.store(true, Ordering::SeqCst);
    b.place(6, 20);
    b.rig.valve[6].inrush_peak_dma = 2800;
    b.rig.valve[6].inrush_ms = 60;
    b.run(CMD_A_OPEN, 6, 20, 0);
    assert_eq!(b.m.mots[6].status, ST_FAILED);
    assert_eq!(b.m.mots[6].fault_reason, ValveFault::InrushTrip as u8);
    assert_eq!(b.m.mots[6].actual_position, 20);
    assert_eq!(b.last(6).stop_reason, reason(StopReason::SafetyOvercurrent));
    assert!(b.last(6).peak_current >= 2500);
    assert_eq!(b.m.mots[6].trip_seq, 1);
    b.flags.protect_suspended.store(true, Ordering::SeqCst);
    b.m.mots[6].status = ST_IDLE;
    b.m.mots[6].fault_reason = 0;
    b.run(CMD_A_OPEN, 6, 20, 0);
    assert_eq!(b.m.mots[6].status, ST_IDLE);
    assert_eq!(b.m.mots[6].actual_position, 40);
    assert_eq!(b.m.mots[6].fault_reason, 0);
}

#[test]
fn c4_an_enforced_inrush_trip_fails_the_move_at_its_start_the_guard_switches_it_off() {
    for rev in REVS {
        inrush_enforced(rev);
    }
}

fn service_inrush_and_timeout(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.flags.protect_enforce.store(true, Ordering::SeqCst);
    b.place(5, 20);
    b.rig.valve[5].inrush_peak_dma = 2800;
    b.rig.valve[5].inrush_ms = 60;
    assert_eq!(b.appsetservice(5, DIR_OPEN, 1000, 40), 0);
    b.run_ms(10);
    assert!(b.run_until(Bench::idle, 10000));
    assert_eq!(b.m.mots[5].status, ST_FAILED);
    assert_eq!(b.m.mots[5].fault_reason, ValveFault::InrushTrip as u8);
    // no end stop within 120 s
    b.place(7, 20);
    b.rig.valve[7].stroke = 10_000_000;
    b.rig.valve[7].pulses_per_ms = 0.05;
    assert_eq!(b.appsetservice(7, DIR_OPEN, SVMOV_COUNTS_MAX, 40), 0);
    b.run_ms(10);
    assert!(b.run_until(Bench::idle, 130_000));
    assert_ne!(b.last(7).stop_reason, reason(StopReason::Aborted));
    assert_eq!(b.m.mots[7].status, ST_FAILED);
    assert_eq!(b.m.mots[7].fault_reason, ValveFault::MoveTimeout as u8);
}

#[test]
fn c4_an_enforced_inrush_trip_and_a_timeout_of_a_service_move_fail_the_valve_with_their_fault() {
    for rev in REVS {
        service_inrush_and_timeout(rev);
    }
}

fn calibration_inrush_enforced(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.flags.protect_enforce.store(true, Ordering::SeqCst);
    b.rig.valve[0].inrush_peak_dma = 2800;
    b.rig.valve[0].inrush_ms = 60;
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    b.run_ms(10);
    assert!(b.run_until(Bench::idle, 30000));
    let mot = b.m.mots[0];
    assert_eq!(mot.status, ST_FAILED);
    assert_eq!(mot.fault_reason, ValveFault::InrushTrip as u8);
    assert_eq!(mot.calib_seq, 0);
    assert!(!mot.calib_active);
    assert!(!mot.calibration);
    assert_eq!(mot.opening_count, 0);
    assert!(b.diag(0).last_cal_failed);
}

#[test]
fn c4_an_enforced_inrush_trip_in_a_calibration_stroke_fails_it_without_blocks_or_counts() {
    for rev in REVS {
        calibration_inrush_enforced(rev);
    }
}

fn temperature_gap(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.flags.temp_refresh_request.store(true, Ordering::SeqCst);
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    b.run_ms(10);
    assert!(b.run_until(|b| b.state() == AState::Gap, 30000));
    // C++: the last temp_command() call is temp_command(3) (UNLOCK)
    assert!(!b.flags.temp_lock.load(Ordering::SeqCst));
    let gap_start = b.rig.ms;
    assert!(b.run_until(|b| b.state() != AState::Gap, 10000));
    assert!(b.rig.ms - gap_start >= 2990);
    assert!(b.rig.ms - gap_start <= 3010);
    // C++: the last temp_command() call is temp_command(2) (LOCK)
    assert!(b.flags.temp_lock.load(Ordering::SeqCst));
    assert_eq!(b.state(), AState::Learn3);
    assert!(b.flags.temp_gap_timeout.load(Ordering::SeqCst));
    b.flags.temp_gap_timeout.store(false, Ordering::SeqCst);
    // the request ends: the next stroke starts at once
    assert!(b.run_until(|b| b.state() == AState::Gap, 30000));
    b.run_ms(100);
    b.flags.temp_refresh_request.store(false, Ordering::SeqCst);
    b.run_ms(20);
    assert_eq!(b.state(), AState::Learn4);
    assert!(!b.flags.temp_gap_timeout.load(Ordering::SeqCst));
    assert!(b.run_until(|b| calibration_done(b, 0), 30000));
    assert_eq!(states(&b.rig), "5 6 17 7 17 8 9 10 11 1");
    assert_eq!(b.m.mots[0].status, ST_IDLE);
    assert_eq!(b.m.mots[0].calib_seq, 1);
}

#[test]
fn s1_a_due_temperature_cycle_pauses_the_calibration_between_the_strokes_at_most_3_s() {
    for rev in REVS {
        temperature_gap(rev);
    }
}

fn sstop_in_the_gap(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.flags.temp_refresh_request.store(true, Ordering::SeqCst);
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    b.run_ms(10);
    assert!(b.run_until(|b| b.state() == AState::Gap, 30000));
    assert_eq!(b.m.appstop(0), 0);
    b.run_ms(20);
    assert_eq!(b.state(), AState::Idle);
    assert!(b.m.mots[0].needs_reference);
    assert!(!b.m.mots[0].calib_active);
    // C++: the last temp_command() call is temp_command(2) (LOCK)
    assert!(b.flags.temp_lock.load(Ordering::SeqCst));
}

#[test]
fn s1_sstop_in_the_temperature_gap_ends_the_calibration() {
    for rev in REVS {
        sstop_in_the_gap(rev);
    }
}

fn hand_over_invalidates_the_kept_position(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(3, 40);
    assert_eq!(
        b.appsetaction_with(CMD_A_OPEN, 3, 10, false, MOVE_KEEP_STATUS + MOVE_REFERENCE),
        0
    );
    assert_eq!(b.env.log.calls, vec![String::from("app_warm_moving(3)")]);
    assert_eq!(b.appsetaction(CMD_A_OPEN, 4, 10), -1);
    assert_eq!(b.env.log.calls, vec![String::from("app_warm_moving(3)")]);
    assert!(b.run_until(Bench::idle, 10000));
    b.env.log.clear();
    assert_eq!(b.appsetservice(5, DIR_OPEN, 10, 30), 0);
    assert_eq!(b.env.log.calls, vec![String::from("app_warm_moving(5)")]);
    assert_eq!(b.m.valve_busy_index(), 5);
}

#[test]
fn c8_appsetaction_and_appsetservice_invalidate_the_kept_position_of_their_valve() {
    for rev in REVS {
        hand_over_invalidates_the_kept_position(rev);
    }
}
