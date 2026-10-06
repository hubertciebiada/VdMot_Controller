// Port of software_stm32/test/native/glue/test_motor__mut.cpp: motor.cpp edges with the valve sim
// (glue_motor, every case for both board revisions): clamped positions, early checks of moves to
// an end stop and of calibration strokes, service move bounds and counts, argument checks, PSU and
// temperature lock timing, calibration retries and results, the motor state machine and
// timer_handler0.
use core::sync::atomic::Ordering;

use vdm_stm_core::calibration::EscalationConfig;
use vdm_stm_core::move_classifier::{StopReason, DIR_CLOSE, DIR_OPEN};
use vdm_stm_core::profile_recorder::ProfileRecorder;
use vdm_stm_core::valve_codes::{
    ValveFault, ST_BLOCKED, ST_CLOSING, ST_FAILED, ST_IDLE, ST_OPENING, ST_OPEN_CIRCUIT, ST_PRESENT,
};

use crate::board::BoardRev;
use crate::hal::{Out, Pins};
use crate::test_support::valve_sim::FixedAdc;

use super::bench::{start_valves, state_step, Bench, REVS};
use super::{
    timer_handler0, AState, ValveSnapshot, CMD_A_CLOSE, CMD_A_LEARN, CMD_A_OPEN, CMD_A_OPEN_END,
    CMD_A_TEST, SVMOV_COUNTS_MAX, SVMOV_COUNTS_MIN, SVMOV_MAXMA_MAX, SVMOV_MAXMA_MIN,
};

const RES_IDLE: u8 = 2;
const RES_OPENS: u8 = 3;
const RES_ERROR: u8 = 11;

fn reason(r: StopReason) -> u8 {
    r as u8
}

fn calibration_done(b: &Bench, v: usize) -> bool {
    !b.m.mots[v].calib_active && b.state() == AState::Idle
}

// ------------------------------------------------------------------ positions of normal moves

fn clamped_at_the_target_count(rev: BoardRev) {
    let mut b = start_valves(rev);
    for v in 0..4 {
        b.rig.valve[v].stroke = 100_000;
    }
    b.place(0, 95);
    b.run(CMD_A_OPEN, 0, 10, 0);
    assert_eq!(b.last(0).stop_reason, reason(StopReason::Target));
    assert_eq!(b.m.mots[0].actual_position, 100);
    b.place(1, 91);
    b.run(CMD_A_OPEN, 1, 10, 0);
    assert_eq!(b.m.mots[1].actual_position, 100);
    b.place(2, 60);
    b.run(CMD_A_CLOSE, 2, 10, 0);
    assert_eq!(b.last(2).stop_reason, reason(StopReason::Target));
    assert_eq!(b.m.mots[2].actual_position, 50);
    b.place(3, 20);
    b.rig.valve[3].position = 3000;
    b.run(CMD_A_CLOSE, 3, 30, 0);
    assert_eq!(b.last(3).stop_reason, reason(StopReason::Target));
    assert_eq!(b.m.mots[3].actual_position, 0);
}

#[test]
fn normal_move_the_position_is_clamped_to_0_100_at_the_target_count() {
    for rev in REVS {
        clamped_at_the_target_count(rev);
    }
}

#[test]
fn normal_move_the_learning_counter_counts_down_to_0_and_stays_there() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.place(0, 20);
        b.m.valves[0].learn_movements = 3;
        b.run(CMD_A_OPEN, 0, 5, 0);
        assert_eq!(b.m.valves[0].learn_movements, 2);
        b.m.valves[0].learn_movements = 0;
        b.run(CMD_A_OPEN, 0, 5, 0);
        assert_eq!(b.m.valves[0].learn_movements, 0);
        assert_eq!(b.m.valves[0].movements, 2);
    }
}

// ------------------------------------------------------------------ early checks of moves to an end stop

fn early_below_half_of_the_learned_travel(rev: BoardRev) {
    let mut b = start_valves(rev);
    // from 20 %: expected 80 % of 3600 counts, early below 1440 counts
    b.place(0, 20);
    b.rig.valve[0].jam_from = 720 + 1450;
    b.rig.valve[0].jam_to = 100_000;
    b.run(CMD_A_OPEN_END, 0, 0, 0);
    assert_eq!(b.last(0).stop_reason, reason(StopReason::EndStop));
    assert!(!b.diag(0).last_early);
    b.place(1, 20);
    // only the travel of the move direction counts
    b.m.mots[1].closing_count = 200;
    b.rig.valve[1].jam_from = 720 + 1430;
    b.rig.valve[1].jam_to = 100_000;
    b.run(CMD_A_OPEN_END, 1, 0, 0);
    assert_eq!(b.last(1).stop_reason, reason(StopReason::EarlyEndStop));
    assert!(b.diag(1).last_early);
    // a move to the end stop never feeds the run of early partial stops
    assert_eq!(b.diag(1).early_run.count(), 0);
    assert_eq!(b.diag(1).early_stops, 1);
    assert_eq!(b.m.mots[1].actual_position, 100);
}

#[test]
fn move_to_the_end_stop_early_below_half_of_the_learned_travel_from_the_start_position() {
    for rev in REVS {
        early_below_half_of_the_learned_travel(rev);
    }
}

fn no_early_check_without_a_learned_travel(rev: BoardRev) {
    let mut b = start_valves(rev);
    // scaler 0: nothing learned
    b.place(0, 20);
    b.m.mots[0].scaler = 0;
    b.rig.valve[0].jam_from = 720 + 100;
    b.rig.valve[0].jam_to = 100_000;
    b.run(CMD_A_OPEN_END, 0, 0, 0);
    assert_eq!(b.last(0).stop_reason, reason(StopReason::EndStop));
    assert!(!b.diag(0).last_early);
    // scaler 1 is valid
    b.place(1, 20);
    b.m.mots[1].scaler = 1;
    b.rig.valve[1].jam_from = 720 + 100;
    b.rig.valve[1].jam_to = 100_000;
    b.run(CMD_A_OPEN_END, 1, 0, 0);
    assert!(b.diag(1).last_early);
    // a travel of exactly MIN_TRAVEL_COUNTS is valid
    b.place(2, 20);
    b.m.mots[2].opening_count = vdm_stm_core::calibration::MIN_TRAVEL_COUNTS;
    b.rig.valve[2].jam_from = 720 + 10;
    b.rig.valve[2].jam_to = 100_000;
    b.run(CMD_A_OPEN_END, 2, 0, 0);
    assert!(b.diag(2).last_early);
    // one count less is not
    b.place(3, 20);
    b.m.mots[3].opening_count = vdm_stm_core::calibration::MIN_TRAVEL_COUNTS - 1;
    b.rig.valve[3].jam_from = 720 + 10;
    b.rig.valve[3].jam_to = 100_000;
    b.run(CMD_A_OPEN_END, 3, 0, 0);
    assert!(!b.diag(3).last_early);
    // no travel, a full stroke expected, not a single count turned
    b.place(4, 0);
    b.m.mots[4].scaler = 0;
    b.rig.valve[4].jam_from = 0;
    b.rig.valve[4].jam_to = 100_000;
    b.run(CMD_A_OPEN_END, 4, 0, 0);
    assert_eq!(b.last(4).counted_counts, 0);
    assert!(!b.diag(4).last_early);
}

#[test]
fn move_to_the_end_stop_no_early_check_without_a_valid_learned_travel() {
    for rev in REVS {
        no_early_check_without_a_learned_travel(rev);
    }
}

#[test]
fn move_to_the_end_stop_a_start_position_above_100_expects_no_travel() {
    for rev in REVS {
        let mut b = start_valves(rev);
        for v in 0..2 {
            b.place(v, 20);
            b.rig.valve[v].jam_from = 720 + 100;
            b.rig.valve[v].jam_to = 100_000;
        }
        b.m.mots[0].actual_position = 150;
        b.run(CMD_A_OPEN_END, 0, 0, 0);
        assert_eq!(b.last(0).stop_reason, reason(StopReason::EndStop));
        assert!(!b.diag(0).last_early);
        b.m.mots[1].actual_position = 101;
        b.run(CMD_A_OPEN_END, 1, 0, 0);
        assert!(!b.diag(1).last_early);
    }
}

// ------------------------------------------------------------------ requested counts, refused moves

fn requested_counts_below_run_to_end_stop(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(0, 10);
    b.run(CMD_A_OPEN, 0, 5, 0);
    assert!(b.last(0).counted_counts > 0);
    assert!(b.last(0).duration_ms > 0);
    let cases: [(u32, u8, u16); 3] = [(771, 85, 65534), (1000, 100, 65534), (655, 100, 65500)];
    for (i, &(scaler, change, expected)) in cases.iter().enumerate() {
        b.m.mots[0].scaler = scaler;
        assert_eq!(b.appsetaction(CMD_A_OPEN, 0, change), 0, "case {i}");
        b.run_ms(20);
        assert_eq!(b.state(), AState::Open1, "case {i}");
        assert_eq!(b.m.appstop(0), 0, "case {i}");
        assert!(b.run_until(Bench::idle, 1000), "case {i}");
        let r = b.last(0);
        assert_eq!(r.requested_counts, expected, "case {i}");
        assert_eq!(r.stop_reason, reason(StopReason::Aborted), "case {i}");
        assert_eq!(r.counted_counts, 0, "case {i}");
        assert_eq!(r.peak_current, 0, "case {i}");
        assert_eq!(r.duration_ms, 0, "case {i}");
    }
}

#[test]
fn normal_move_the_requested_counts_stay_below_run_to_end_stop_a_refused_move_records_nothing() {
    for rev in REVS {
        requested_counts_below_run_to_end_stop(rev);
    }
}

// ------------------------------------------------------------------ calibration strokes

#[test]
fn calibration_a_full_stroke_below_half_of_the_learned_travel_is_early() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.place(0, 0);
        b.m.no_of_min_counts = 100;
        b.rig.valve[0].stroke = 1790;
        b.rig.valve[0].position = 1790;
        b.rig.valve[0].pulses_per_ms = 1.0;
        b.m.mots[0].target_position = 0;
        b.learn(0);
        assert_eq!(b.m.mots[0].calib_seq, 1);
        assert_eq!(b.m.mots[0].status, ST_IDLE);
        // the last stroke (closing, 1790 counts against 3600 learned) is early
        assert_eq!(b.last(0).stop_reason, reason(StopReason::EarlyEndStop));
        assert!(b.diag(0).last_early);
        assert!(!b.diag(0).last_cal_failed);
        assert!(!b.diag(0).early_warn);
    }
}

fn strokes_close_open_close(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[0].stroke = 9920;
    b.rig.valve[0].pulses_per_ms = 2.0;
    b.m.mots[0].target_position = 0;
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    assert!(b.run_until(|b| b.state() == AState::Learn3, 60000));
    b.run_ms(1000);
    assert_eq!(b.m.mots[0].status, ST_OPENING);
    assert!(b.run_until(|b| b.state() == AState::Learn4, 60000));
    b.run_ms(1000);
    assert_eq!(b.m.mots[0].status, ST_CLOSING);
    assert!(b.run_until(|b| calibration_done(b, 0), 60000));
    assert!(b.m.mots[0].opening_count >= 9900);
    assert_eq!(b.m.mots[0].scaler, 99);
    assert_eq!(b.m.mots[0].actual_position, 0);
    assert!(!b.diag(0).last_cal_failed);
}

#[test]
fn calibration_the_strokes_close_open_and_close_the_scaler_is_the_opening_count_over_100() {
    for rev in REVS {
        strokes_close_open_close(rev);
    }
}

fn enforced_inrush_of_the_first_stroke(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.flags.protect_enforce.store(true, Ordering::SeqCst);
    b.place(0, 50);
    b.rig.valve[0].inrush_peak_dma = 2800;
    b.rig.valve[0].inrush_ms = 60;
    b.learn(0);
    assert_eq!(b.m.mots[0].status, ST_FAILED);
    assert_eq!(b.last(0).stop_reason, reason(StopReason::SafetyOvercurrent));
    assert!(!b.diag(0).last_early);
    b.rig.valve[0].inrush_peak_dma = 0;
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.m.mots[0].target_position = 0;
    b.learn(0);
    assert_eq!(b.m.mots[0].status, ST_IDLE);
    assert_eq!(b.m.mots[0].calib_seq, 1);
}

#[test]
fn calibration_an_enforced_inrush_trip_of_the_first_stroke_is_not_early_the_next_runs() {
    for rev in REVS {
        enforced_inrush_of_the_first_stroke(rev);
    }
}

fn early_warning_and_failed_flag(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(0, 20);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.rig.valve[0].jam_from = 900;
    b.rig.valve[0].jam_to = 3600;
    b.run(CMD_A_OPEN, 0, 40, 0);
    assert!(b.diag(0).early_warn);
    // strokes too short: BLOCKS, the warning stays
    b.rig.valve[0].jam_from = -1;
    b.rig.valve[0].stroke = 50;
    b.rig.valve[0].position = 20;
    b.learn(0);
    assert_eq!(b.m.mots[0].status, ST_BLOCKED);
    assert!(b.diag(0).last_cal_failed);
    assert!(b.diag(0).early_warn);
    // success: both cleared as soon as the pass is accepted, before the move to the target
    b.rig.valve[0].stroke = 3600;
    b.m.mots[0].target_position = 50;
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    assert!(b.run_until(|b| b.state() == AState::Set2, 60000));
    assert!(!b.diag(0).last_cal_failed);
    assert!(!b.diag(0).early_warn);
    assert!(b.run_until(|b| calibration_done(b, 0), 60000));
    assert!(!b.diag(0).last_cal_failed);
    assert!(!b.diag(0).early_warn);
    assert_eq!(b.m.mots[0].actual_position, 50);
}

#[test]
fn calibration_early_warning_and_failed_flag_over_a_failed_and_a_successful_calibration() {
    for rev in REVS {
        early_warning_and_failed_flag(rev);
    }
}

fn retries_restart_at_0(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[0].pulses_per_ms = 2.0;
    b.rig.valve[0].stroke = 50;
    b.rig.valve[0].position = 20;
    b.m.calib_retries = 5;
    b.m.mots[0].calib_retries = 5;
    b.m.max_calib_retries = 1;
    b.learn(0);
    assert_eq!(b.m.mots[0].status, ST_BLOCKED);
    assert_eq!(b.m.mots[0].calib_retries, 2);
    assert_eq!(b.m.calib_retries, 2);
    // a successful calibration after failed ones
    b.m.calib_retries = 5;
    b.m.mots[0].calib_retries = 5;
    b.rig.valve[0].stroke = 3600;
    b.m.mots[0].target_position = 0;
    b.learn(0);
    assert_eq!(b.m.mots[0].status, ST_IDLE);
    assert_eq!(b.m.mots[0].calib_retries, 0);
}

#[test]
fn calibration_the_retries_restart_at_0_with_every_calibration_the_valve_count_saturates() {
    for rev in REVS {
        retries_restart_at_0(rev);
    }
}

#[test]
fn calibration_the_retry_count_of_the_valve_saturates_at_255() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.rig.valve[0].pulses_per_ms = 2.0;
        b.rig.valve[0].stroke = 30;
        b.rig.valve[0].position = 10;
        b.m.max_calib_retries = 255;
        b.learn(0);
        assert_eq!(b.m.mots[0].status, ST_BLOCKED);
        assert_eq!(b.m.mots[0].calib_retries, 255);
        // 256 failed passes
        assert_eq!(b.m.calib_retries, 0);
    }
}

#[test]
fn calibration_contact_lost_marks_the_calibration_failed() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.rig.valve[3].connected = false;
        b.learn(3);
        assert_eq!(b.m.mots[3].status, ST_OPEN_CIRCUIT);
        assert!(b.diag(3).last_cal_failed);
    }
}

#[test]
fn calibration_a_stroke_without_an_end_stop_times_out_and_fails_the_valve() {
    for rev in REVS {
        let mut b = start_valves(rev);
        // below the end-stop bound: no trip
        b.rig.valve[0].stall_current_dma = 300;
        b.rig.valve[0].jam_from = 0;
        b.rig.valve[0].jam_to = 100_000;
        b.rig.valve[0].position = 1000;
        b.learn(0);
        assert_eq!(b.m.mots[0].status, ST_FAILED);
        assert_eq!(b.m.mots[0].fault_reason, ValveFault::StrokeTimeout as u8);
        assert_eq!(b.last(0).stop_reason, reason(StopReason::Timeout));
        assert!(b.diag(0).last_cal_failed);
    }
}

fn target_1_after_the_calibration(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[3].pulses_per_ms = 1.0;
    b.m.mots[3].target_position = 1;
    b.learn(3);
    assert_eq!(b.m.mots[3].actual_position, 1);
    assert_eq!(b.last(3).stop_reason, reason(StopReason::Target));
    assert!(!b.diag(3).last_cal_failed);
    // target 0: valve 0 is not touched by the end of the calibration of valve 3
    b.m.mots[0].status = ST_IDLE;
    b.m.mots[0].connected = false;
    b.m.mots[3].target_position = 0;
    b.learn(3);
    assert_eq!(b.m.mots[3].actual_position, 0);
    assert!(!b.diag(3).last_cal_failed);
    assert!(!b.m.mots[0].connected);
}

#[test]
fn calibration_target_1_is_set_after_the_calibration_target_0_needs_no_move() {
    for rev in REVS {
        target_1_after_the_calibration(rev);
    }
}

#[test]
fn calibration_blocked_valve_3_leaves_valve_0_alone() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.rig.valve[3].pulses_per_ms = 1.0;
        b.rig.valve[3].stroke = 50;
        b.rig.valve[3].position = 20;
        b.m.mots[0].status = ST_IDLE;
        b.m.mots[0].connected = false;
        b.learn(3);
        assert_eq!(b.m.mots[3].status, ST_BLOCKED);
        assert!(!b.m.mots[0].connected);
    }
}

fn sstop_before_the_move_to_the_target(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.m.mots[0].target_position = 50;
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    assert!(b.run_until(|b| b.state() == AState::Set1, 60000));
    let set1 = b.rig.ms;
    b.run_ms(500);
    // waits for the PSU (1 s)
    assert_eq!(b.state(), AState::Set1);
    assert_eq!(b.m.appstop(0), 0);
    assert!(b.run_until(Bench::idle, 1000));
    assert!(b.rig.ms - set1 < 1000);
    assert!(!b.m.mots[0].calib_active);
    assert!(!b.diag(0).last_cal_failed);
    assert_eq!(b.last(0).stop_reason, reason(StopReason::Aborted));
}

#[test]
fn calibration_sstop_while_the_move_to_the_target_waits_for_its_start() {
    for rev in REVS {
        sstop_before_the_move_to_the_target(rev);
    }
}

fn sstop_while_a_stroke_waits(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(0, 10);
    b.rig.valve[0].pulses_per_ms = 0.1;
    // a move ended by sstop: its stop cause is Aborted
    assert_eq!(b.appsetaction(CMD_A_OPEN, 0, 80), 0);
    b.run_ms(3000);
    assert_eq!(b.m.appstop(0), 0);
    assert!(b.run_until(Bench::idle, 5000));
    assert_eq!(b.last(0).stop_reason, reason(StopReason::Aborted));
    let enables = b.rig.valve[0].enables;
    // the calibration waits in Learn2 for the first stroke
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    assert!(b.run_until(|b| b.state() == AState::Learn2, 1000));
    b.run_ms(100);
    assert_eq!(b.m.appstop(0), 0);
    assert!(b.run_until(Bench::idle, 5000));
    // the stroke started and was stopped
    assert_eq!(b.rig.valve[0].enables, enables + 1);
    assert!(b.m.mots[0].needs_reference);
    // the next move runs
    b.place(0, 10);
    b.run(CMD_A_OPEN, 0, 5, 0);
    assert_eq!(b.last(0).stop_reason, reason(StopReason::Target));
    assert_eq!(b.m.mots[0].actual_position, 15);
}

#[test]
fn calibration_sstop_while_a_stroke_waits_for_its_start_leaves_the_motor_ready_for_the_next_move() {
    for rev in REVS {
        sstop_while_a_stroke_waits(rev);
    }
}

#[test]
fn calibration_the_temperature_gap_ends_after_exactly_3_s() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.rig.valve[0].pulses_per_ms = 1.0;
        b.flags.temp_refresh_request.store(true, Ordering::SeqCst);
        assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
        assert!(b.run_until(|b| b.state() == AState::Learn3, 60000));
        let gap = state_step(&b.rig, AState::Gap).expect("gap");
        let next = state_step(&b.rig, AState::Learn3).expect("learn3");
        assert_eq!(next.ms - gap.ms, 3000);
    }
}

// ------------------------------------------------------------------ service moves

fn service_exact_counts(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(0, 10);
    let start = b.rig.valve[0].position;
    assert_eq!(b.appsetservice(0, DIR_OPEN, 500, 40), 0);
    b.run_ms(1500);
    assert_eq!(b.m.mots[0].status, ST_OPENING);
    assert!(b.run_until(Bench::idle, 20000));
    let r = b.last(0);
    assert_eq!(r.stop_reason, reason(StopReason::Target));
    assert_eq!(r.counted_counts, 500);
    assert_eq!(b.rig.valve[0].position - start, 500);
    assert!(r.duration_ms >= 2500);
    assert!(r.duration_ms < 3000);
    assert_eq!(u32::from(b.m.mots[0].actual_position), 10 + 500 / 36);
    assert_eq!(b.m.mots[0].status, ST_IDLE);
}

#[test]
fn service_move_exactly_the_requested_counts_the_position_from_the_scaler() {
    for rev in REVS {
        service_exact_counts(rev);
    }
}

#[test]
fn service_move_an_end_stop_is_never_early_and_leaves_the_valve_idle() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.place(0, 10);
        b.service(0, DIR_CLOSE, 1000, 40);
        assert_eq!(b.last(0).stop_reason, reason(StopReason::EndStop));
        assert!(!b.diag(0).last_early);
        assert_eq!(b.m.mots[0].status, ST_IDLE);
        assert_eq!(b.m.mots[0].actual_position, 0);
    }
}

fn service_bound_is_maxma_x_10(rev: BoardRev) {
    let mut b = start_valves(rev);
    // 48 mA filtered at the obstacle, bound 45 mA: stops at the obstacle
    b.place(0, 10);
    b.rig.valve[0].stall_current_dma = 530;
    b.rig.valve[0].jam_from = 400;
    b.rig.valve[0].jam_to = 100_000;
    b.service(0, DIR_OPEN, 1000, 45);
    assert_eq!(b.last(0).stop_reason, reason(StopReason::EndStop));
    // 53 mA filtered at the obstacle, bound 55 mA: runs into the timeout
    b.place(1, 10);
    b.rig.valve[1].stall_current_dma = 580;
    b.rig.valve[1].jam_from = 400;
    b.rig.valve[1].jam_to = 100_000;
    b.service(1, DIR_OPEN, 1000, 55);
    assert_eq!(b.last(1).stop_reason, reason(StopReason::Timeout));
    assert_eq!(b.last(1).duration_ms, 120_270);
    assert_eq!(b.m.mots[1].status, ST_FAILED);
}

#[test]
fn service_move_the_end_stop_bound_is_maxma_x_10() {
    for rev in REVS {
        service_bound_is_maxma_x_10(rev);
    }
}

fn service_position_with_scaler_1(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(0, 10);
    b.m.mots[0].scaler = 1;
    b.service(0, DIR_OPEN, 50, 40);
    assert_eq!(b.m.mots[0].actual_position, 60);
    let cases: [(u16, u8); 3] = [(101, 50), (150, 50), (99, 51)];
    for (i, &(counts, pos)) in cases.iter().enumerate() {
        let v = 1 + i;
        b.place(v, 10);
        b.rig.valve[v].position = 3000;
        b.m.mots[v].scaler = 1;
        b.m.mots[v].actual_position = 150;
        b.service(v as u32, DIR_CLOSE, counts, 40);
        assert_eq!(b.last(v as u32).counted_counts, counts, "case {i}");
        assert_eq!(b.m.mots[v].actual_position, pos, "case {i}");
    }
}

#[test]
fn service_move_the_position_with_scaler_1_is_clamped_to_100_of_travel() {
    for rev in REVS {
        service_position_with_scaler_1(rev);
    }
}

fn appsetservice_limits(rev: BoardRev) {
    let mut b = start_valves(rev);
    let take = |b: &mut Bench, v: u32, dir: u8, counts: u16, maxma: u8| {
        let r = b.appsetservice(v, dir, counts, maxma);
        if r == 0 {
            b.m.appstop(255);
        }
        r
    };
    assert_eq!(take(&mut b, 12, DIR_OPEN, 100, 30), -1);
    assert_eq!(take(&mut b, 11, DIR_OPEN, 100, 30), 0);
    assert_eq!(take(&mut b, 0, 2, 100, 30), -1);
    assert_eq!(take(&mut b, 0, DIR_CLOSE, 100, 30), 0);
    assert_eq!(take(&mut b, 0, DIR_OPEN, 0, 30), -1);
    assert_eq!(take(&mut b, 0, DIR_OPEN, SVMOV_COUNTS_MIN, 30), 0);
    assert_eq!(take(&mut b, 0, DIR_OPEN, SVMOV_COUNTS_MAX, 30), 0);
    assert_eq!(take(&mut b, 0, DIR_OPEN, SVMOV_COUNTS_MAX + 1, 30), -1);
    assert_eq!(take(&mut b, 0, DIR_OPEN, 100, SVMOV_MAXMA_MIN - 1), -1);
    assert_eq!(take(&mut b, 0, DIR_OPEN, 100, SVMOV_MAXMA_MIN), 0);
    assert_eq!(take(&mut b, 0, DIR_OPEN, 100, SVMOV_MAXMA_MAX), 0);
    assert_eq!(take(&mut b, 0, DIR_OPEN, 100, SVMOV_MAXMA_MAX + 1), -1);
    assert!(b.idle());
    // a command is pending: busy
    assert_eq!(b.appsetservice(0, DIR_OPEN, 100, 30), 0);
    assert_eq!(b.appsetservice(1, DIR_OPEN, 100, 30), -2);
    // taken, the machine is busy
    b.run_ms(20);
    assert_eq!(b.state(), AState::Svc1);
    assert_eq!(b.appsetservice(1, DIR_OPEN, 100, 30), -2);
}

#[test]
fn appsetservice_argument_limits_busy() {
    for rev in REVS {
        appsetservice_limits(rev);
    }
}

// ------------------------------------------------------------------ snapshots and profiles

#[test]
fn valve_get_snapshot_and_valve_get_profile_valve_12_and_above_give_empty_results() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.place(0, 20);
        b.run(CMD_A_OPEN, 0, 10, 0);
        let mut p = ProfileRecorder::default();
        b.m.valve_get_profile(0, &mut p);
        assert!(p.size() > 0);
        b.m.valve_get_profile(12, &mut p);
        assert_eq!(p.size(), 0);
        let mut s = ValveSnapshot::default();
        b.m.valve_get_snapshot(0, &mut s);
        assert!(s.diag.last.counted_counts > 0);
        b.m.valve_get_snapshot(12, &mut s);
        assert_eq!(s.diag.last.counted_counts, 0);
        assert_eq!(s.actual_position, 0);
    }
}

#[test]
fn profile_a_move_that_ends_at_an_end_stop_ends_with_the_trip_current() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.place(0, 60);
        b.run(CMD_A_OPEN_END, 0, 0, 0);
        assert_eq!(b.last(0).stop_reason, reason(StopReason::EndStop));
        let mut p = ProfileRecorder::default();
        b.m.valve_get_profile(0, &mut p);
        assert!(p.size() > 1);
        assert!(p.at(p.size() - 1).current > 300);
    }
}

// ------------------------------------------------------------------ PSU and temperature lock timing

fn unlock_after_50_psu_off_after_1500(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.place(0, 20);
    b.run(CMD_A_OPEN, 0, 5, 0);
    // C++: the first temp_command() call after the move, temp_command(3) = UNLOCK
    assert!(b.flags.temp_lock.load(Ordering::SeqCst));
    let start = b.rig.ms;
    assert!(b.run_until(|b| !b.flags.temp_lock.load(Ordering::SeqCst), 2000));
    assert_eq!(b.rig.ms - start, 510);
    assert_eq!(b.ms_to_psu_off(), 15010 - 510);
    // the next 15 s
    let writes = b.board.writes_of(Out::PsuEna).len();
    b.run_ms(15000);
    assert_eq!(b.board.writes_of(Out::PsuEna).len(), writes);
    b.run_ms(20);
    assert!(b.board.writes_of(Out::PsuEna).len() > writes);
}

#[test]
fn idle_temperature_measurement_unlocked_after_50_idle_cycles_psu_off_after_1500() {
    for rev in REVS {
        unlock_after_50_psu_off_after_1500(rev);
    }
}

fn psu_off_15_s_after_everything(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.run(CMD_A_TEST, 2, 0, 0);
    assert_eq!(b.ms_to_psu_off(), 15010);
    b.place(1, 10);
    b.service(1, DIR_OPEN, 100, 40);
    assert_eq!(b.ms_to_psu_off(), 15010);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.m.mots[0].target_position = 0;
    b.learn(0);
    assert_eq!(b.ms_to_psu_off(), 15010);
    b.m.mots[0].target_position = 30;
    b.learn(0);
    assert_eq!(b.ms_to_psu_off(), 15010);
    // a retried calibration
    b.rig.valve[0].stroke = 50;
    b.rig.valve[0].position = 20;
    b.m.max_calib_retries = 1;
    b.learn(0);
    assert_eq!(b.m.mots[0].status, ST_BLOCKED);
    assert_eq!(b.ms_to_psu_off(), 15010);
}

#[test]
fn idle_the_psu_goes_off_15_s_after_a_presence_test_a_service_move_and_a_calibration() {
    for rev in REVS {
        psu_off_15_s_after_everything(rev);
    }
}

// ------------------------------------------------------------------ valve_loop without the sim

#[test]
fn valve_loop_init_until_the_motor_machine_is_idle_no_valve_touched() {
    for rev in REVS {
        let mut b = Bench::new(rev);
        b.valve_setup();
        for v in 0..2 {
            b.m.mots[v].status = ST_IDLE;
            b.m.mots[v].connected = false;
        }
        b.valve_loop();
        assert_eq!(b.state(), AState::Init);
        assert!(!b.m.mots[1].connected);
        b.m.mots[0].connected = false;
        b.valve_loop();
        assert_eq!(b.state(), AState::Idle);
        assert!(!b.m.mots[0].connected);
        assert!(!b.m.mots[1].connected);
        b.valve_loop();
        assert!(b.m.mots[0].connected);
    }
}

#[test]
fn valve_setup_no_valve_starts_failed_calibrated_or_with_a_pending_reference() {
    let mut b = Bench::new(BoardRev::C2);
    for mot in &mut b.m.mots {
        mot.calib_failed = true;
        mot.calibrated = true;
        mot.needs_reference = true;
        mot.early_learn_due = true;
    }
    b.valve_setup();
    for (v, mot) in b.m.mots.iter().enumerate() {
        assert!(!mot.calib_failed, "valve {v}");
        assert!(!mot.calibrated, "valve {v}");
        assert!(!mot.needs_reference, "valve {v}");
        assert!(!mot.early_learn_due, "valve {v}");
    }
}

#[test]
fn normal_move_the_end_stop_bound_is_never_escalated() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.m.motor_set_escalation(
            &mut b.env,
            &EscalationConfig {
                enable: 1,
                step_pct: 25,
                max_ma: 50,
            },
        );
        // 39 mA filtered at the end stop: above the 34 mA bound, below its first escalation
        // (42.5 mA)
        b.place(0, 60);
        b.rig.valve[0].stall_current_dma = 440;
        b.run(CMD_A_OPEN_END, 0, 0, 0);
        assert_eq!(b.last(0).stop_reason, reason(StopReason::EndStop));
    }
}

#[test]
fn valve_loop_counts_its_runs_idle_is_never_a_stall() {
    let mut b = Bench::new(BoardRev::C2);
    b.valve_setup();
    for _ in 0..30100 {
        b.valve_loop();
    }
    assert_eq!(b.flags.valve_loop_ticks.load(Ordering::SeqCst), 30100);
    assert_eq!(b.state(), AState::Idle);
    assert!(!b.flags.valve_loop_stalled.load(Ordering::SeqCst));
}

#[test]
fn open_circuit_of_valve_11_not_connected() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.rig.valve[11].connected = false;
        b.place(11, 30);
        b.run(CMD_A_OPEN, 11, 10, 0);
        assert_eq!(b.m.mots[11].status, ST_OPEN_CIRCUIT);
        assert!(!b.m.mots[11].connected);
    }
}

#[test]
fn undercurrent_exactly_the_threshold_is_a_turning_motor_in_both_directions() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.place(0, 20);
        b.rig.valve[0].run_current_dma = 20;
        b.run(CMD_A_OPEN, 0, 10, 0);
        assert_eq!(b.m.mots[0].status, ST_IDLE);
        assert_eq!(b.m.mots[0].actual_position, 30);
        b.run(CMD_A_CLOSE, 0, 10, 0);
        assert_eq!(b.m.mots[0].status, ST_IDLE);
        assert_eq!(b.m.mots[0].actual_position, 20);
    }
}

#[test]
fn presence_test_2_s_for_the_psu_then_the_test() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.run(CMD_A_TEST, 2, 0, 0);
        let test = state_step(&b.rig, AState::Test).expect("test");
        let back = state_step(&b.rig, AState::Idle).expect("idle");
        assert_eq!(back.ms - test.ms, 2250);
        assert_eq!(b.m.mots[2].status, ST_PRESENT);
    }
}

// ------------------------------------------------------------------ motor state machine

#[test]
fn motorcycle_valve_numbers_outside_0_11_are_refused() {
    for rev in REVS {
        let mut b = Bench::new(rev);
        // M_INIT
        assert_eq!(b.motorcycle(0, b'n'), 1);
        assert_eq!(b.motorcycle(12, b'o'), RES_IDLE);
        assert_eq!(b.motorcycle(-1, b'o'), RES_IDLE);
        assert_eq!(b.motorcycle(11, b'o'), RES_OPENS);
    }
}

#[test]
fn motorcycle_a_stop_request_of_the_exti_handler_in_idle_is_dropped() {
    let mut b = Bench::new(BoardRev::C2);
    b.motorcycle(0, b'n');
    b.pulse.stop_request.store(true, Ordering::SeqCst);
    assert_eq!(b.motorcycle(0, b'n'), RES_IDLE);
    assert!(!b.pulse.stop_request.load(Ordering::SeqCst));
    assert_eq!(b.motorcycle(0, b'n'), RES_IDLE);
}

#[test]
fn motorcycle_the_motor_enable_times_out_after_timeout_turnon_cycles_without_timer_handler0() {
    for rev in REVS {
        let mut b = Bench::new(rev);
        b.motorcycle(0, b'n');
        assert_eq!(b.motorcycle(0, b'o'), RES_OPENS);
        let mut calls = 0;
        let mut r;
        loop {
            r = b.motorcycle(0, b'n');
            calls += 1;
            if r == RES_ERROR || calls >= 100 {
                break;
            }
        }
        assert_eq!(r, RES_ERROR);
        // M_OPEN, settle, M_START, 11 M_TURNON
        assert_eq!(calls, 1 + 10 + 1 + 11);
        assert_eq!(b.motorcycle(0, b'n'), RES_IDLE);
    }
}

#[test]
fn motorcycle_halt_and_the_exti_stop_cancel_the_soft_start() {
    for rev in REVS {
        let mut b = start_valves(rev);
        b.place(0, 20);
        b.run(CMD_A_OPEN, 0, 5, 0);
        assert!(!b.pulse.turning.load(Ordering::SeqCst));
        assert!(!b.m.isr_timer_go);
        assert!(!b.m.isr_timer_fin);
        b.pulse.turning.store(true, Ordering::SeqCst);
        b.pulse.callback_motorstop(&b.board);
        assert!(!b.pulse.turning.load(Ordering::SeqCst));
        assert!(b.pulse.stop_request.load(Ordering::SeqCst));
    }
}

// ------------------------------------------------------------------ timer_handler0

#[test]
fn timer_handler0_the_test_current_filter_idle_current_0() {
    let mut b = Bench::new(BoardRev::C2);
    // C++ analogReadResolution(12): the ADC delivers 12 bit
    let cases: [(u16, i32); 4] = [
        (2048 + 2047, 10000),
        (2048 + 1000, 7777),
        (2048 - 1500, 12345),
        (2048 + 7, 3),
    ];
    for (adc, old) in cases {
        b.m.analog_current_old = old;
        b.m.current_ma = 5;
        let mut fixed = FixedAdc {
            current: adc,
            reference: 2048,
        };
        timer_handler0(&mut b.m, &b.pulse, &mut fixed, &b.board);
        let value = (i32::from(adc) - 2048) * 138 / 100;
        let expected = (old * 9800 + value * 200) / 10000;
        assert_eq!(b.m.analog_current, expected, "adc {adc}");
        assert_eq!(b.m.analog_current_old, expected, "adc {adc}");
        assert_eq!(b.m.current_ma, 0, "adc {adc}");
    }
}

#[test]
fn timer_handler0_the_motor_is_enabled_only_while_turning_requested_and_not_yet_enabled() {
    let mut b = Bench::new(BoardRev::C2);
    b.m.isr_valvenr = 4;
    // (turning, go, fin, on)
    let cases = [
        (true, true, false, true),
        (false, true, false, false),
        (true, false, false, false),
        (true, true, true, false),
    ];
    for (turning, go, fin, on) in cases {
        b.board.set_latch(Out::Ena2, false);
        b.pulse.turning.store(turning, Ordering::SeqCst);
        b.m.isr_timer_go = go;
        b.m.isr_timer_fin = fin;
        let mut fixed = FixedAdc {
            current: 2048,
            reference: 2048,
        };
        timer_handler0(&mut b.m, &b.pulse, &mut fixed, &b.board);
        let case = (turning, go, fin);
        assert_eq!(b.board.latch(Out::Ena2), on, "{case:?}");
        assert_eq!(b.m.isr_timer_fin, on || fin, "{case:?}");
    }
}

// ------------------------------------------------------------------ mean current of the strokes

// One sample of the turning current every 51 cycles from the 51st on, MIN_MEAN_SAMPLES (4) of them
// make a stroke mean valid. The opening stroke (15 mA) is just long enough for the 4th sample, the
// closing stroke (25 mA) ends just before its 4th: only the opening mean is learned. One sample
// more or less on either stroke changes the result (20 mA: both or none valid).
fn stroke_means_need_4_samples(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.m.no_of_min_counts = 100;
    b.rig.valve[0].stroke = 2023;
    b.rig.valve[0].position = 0;
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.rig.valve[0].run_current_dma = 150;
    b.m.mots[0].target_position = 0;
    assert_eq!(b.m.mots[0].meancurrent, 20);
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    assert!(b.run_until(|b| b.state() == AState::Learn4, 200_000));
    b.rig.valve[0].pulses_per_ms = 0.9932;
    b.rig.valve[0].run_current_dma = 300;
    assert!(b.run_until(|b| calibration_done(b, 0), 200_000));
    assert_eq!(b.m.mots[0].calib_seq, 1);
    assert_eq!(b.m.mots[0].meancurrent, 15);
}

#[test]
fn calibration_stroke_means_need_4_samples_one_every_51_cycles() {
    for rev in REVS {
        stroke_means_need_4_samples(rev);
    }
}
