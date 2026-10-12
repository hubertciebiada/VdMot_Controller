// New cases (no C++ counterpart): the coast of a valve motor (core coast). The coast probes of
// a calibration learn it, partial moves stop it earlier, a step away from an end stop and the
// move back refine it; moves of another kind or that end otherwise give no sample; every end of
// a probe move.
use core::sync::atomic::Ordering;

use vdm_stm_core::move_classifier::{StopReason, DIR_CLOSE, DIR_OPEN};
use vdm_stm_core::valve_codes::{ValveFault, ST_BLOCKED, ST_FAILED, ST_IDLE, ST_OPEN_CIRCUIT};

use crate::board::BoardRev;

use super::bench::{start_valves, states, Bench, REVS};
use super::{
    AState, CMD_A_CLOSE, CMD_A_CLOSE_END, CMD_A_LEARN, CMD_A_OPEN, CMD_A_OPEN_END, CMD_A_TEST,
    MOVE_KEEP_STATUS,
};

/// pulses the sim's motor turns after its enable went off
const COAST: i32 = 40;

fn coast(b: &Bench) -> (u16, u16) {
    let c = b.m.mots[0].coast;
    (c.counts(DIR_OPEN), c.counts(DIR_CLOSE))
}

/// Valve 0 calibrated with a motor that coasts COAST pulses; it stands at 50 %.
fn calibrated(rev: BoardRev) -> Bench {
    let mut b = start_valves(rev);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.rig.valve[0].coast_pulses = COAST;
    b.learn(0);
    assert_eq!(b.m.mots[0].status, ST_IDLE);
    assert_eq!(b.m.mots[0].scaler, 36);
    // the coast of the move to the target
    b.run_ms(100);
    b
}

/// A calibration of valve 0 with a motor that coasts COAST pulses, until the valve state
/// machine is in `state`.
fn learn_until(rev: BoardRev, state: AState) -> Bench {
    let mut b = start_valves(rev);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.rig.valve[0].coast_pulses = COAST;
    assert_eq!(b.appsetaction(CMD_A_LEARN, 0, 0), 0);
    b.run_ms(10);
    assert!(b.run_until(|b| b.state() == state, 30000));
    b
}

fn learn_done(b: &mut Bench) {
    assert!(b.run_until(
        |b| !b.m.mots[0].calib_active && b.state() == AState::Idle,
        2_000_000
    ));
}

/// Pulses the real position is ahead of the counted one (opening).
fn ahead(b: &Bench) -> i32 {
    b.rig.valve[0].position - i32::from(b.m.mots[0].actual_position) * 36
}

fn calibration_learns_the_coast(rev: BoardRev) {
    let b = calibrated(rev);
    assert_eq!(states(&b.rig), "5 6 18 19 7 18 19 8 9 10 11 1");
    assert_eq!(coast(&b), (40, 40));
    // the move to the target 50 % stopped its coast earlier: the pulse that stops the motor
    assert_eq!(b.m.mots[0].actual_position, 50);
    assert_eq!(ahead(&b), 1);
    assert_eq!(b.last(0).requested_counts, 1800);
    assert_eq!(b.last(0).counted_counts, 1761);
}

#[test]
fn coast_a_calibration_learns_the_coast_of_both_directions_from_its_probes() {
    for rev in REVS {
        calibration_learns_the_coast(rev);
    }
}

fn compensated_steps(rev: BoardRev) {
    let mut b = calibrated(rev);
    let mut steps = 0;
    while b.m.mots[0].actual_position < 98 {
        b.run(CMD_A_OPEN, 0, 4, 0);
        b.run_ms(100);
        steps += 1;
        assert_eq!(b.last(0).stop_reason, StopReason::Target as u8);
        assert_eq!(b.last(0).requested_counts, 144);
        assert_eq!(b.last(0).counted_counts, 105);
        // one pulse per move: the pulse that stops the motor
        assert_eq!(ahead(&b), 1 + steps);
    }
    assert_eq!(b.diag(0).early_stops, 0);
    assert!(!b.m.mots[0].early_learn_due);
}

#[test]
fn coast_opening_steps_with_a_learned_coast_stay_where_they_are_counted() {
    for rev in REVS {
        compensated_steps(rev);
    }
}

/// From the end stop of `end` a step of `change` % away from it, then the move back.
fn step_and_back(b: &mut Bench, end: u8, change: u8) {
    let (to_end, away) = if end == DIR_CLOSE {
        (CMD_A_CLOSE_END, CMD_A_OPEN)
    } else {
        (CMD_A_OPEN_END, CMD_A_CLOSE)
    };
    b.run(to_end, 0, 0, 0);
    b.run(away, 0, change, 0);
    assert_eq!(b.last(0).stop_reason, StopReason::Target as u8);
    b.run(to_end, 0, 0, 0);
    assert_eq!(b.last(0).stop_reason, StopReason::EndStop as u8);
}

fn online_pairs(rev: BoardRev) {
    let mut b = calibrated(rev);
    b.rig.valve[0].coast_pulses = 60;
    // 360 pulses requested, target 320, 321 counted and 60 turned on, 381 counted back: 60
    step_and_back(&mut b, DIR_CLOSE, 10);
    assert_eq!(coast(&b), ((3 * 40 + 60 + 2) / 4, 40));
    step_and_back(&mut b, DIR_OPEN, 10);
    assert_eq!(coast(&b), (45, 45));
    // the valve stands at the open end stop: a step down and back again
    b.run(CMD_A_CLOSE, 0, 10, 0);
    b.run(CMD_A_OPEN_END, 0, 0, 0);
    assert_eq!(coast(&b), (45, 49));
    // a step down, then a partial opening move from 90 % that meets the open end stop (open end)
    b.run(CMD_A_CLOSE, 0, 10, 0);
    b.run(CMD_A_OPEN, 0, 20, 0);
    assert_eq!(b.last(0).stop_reason, StopReason::EndStop as u8);
    assert_eq!(b.m.mots[0].actual_position, 100);
    assert_eq!(coast(&b), (45, 52));
}

#[test]
fn coast_a_step_away_from_an_end_stop_and_the_move_back_refine_the_coast() {
    for rev in REVS {
        online_pairs(rev);
    }
}

fn no_samples(rev: BoardRev) {
    let mut b = calibrated(rev);
    // agrees with the learned 40: a pair formed where none is would change it
    b.rig.valve[0].coast_pulses = 60;
    let learned = coast(&b);
    // a step shorter than the sample minimum (72 pulses requested)
    b.run(CMD_A_CLOSE_END, 0, 0, 0);
    b.run(CMD_A_OPEN, 0, 2, 0);
    b.run(CMD_A_CLOSE_END, 0, 0, 0);
    assert_eq!(coast(&b), learned);
    // a keep-status move (a failed or blocked valve) is not shortened and gives no sample
    b.run(CMD_A_OPEN, 0, 10, MOVE_KEEP_STATUS);
    assert_eq!(b.last(0).counted_counts, 361);
    b.run(CMD_A_CLOSE_END, 0, 0, MOVE_KEEP_STATUS);
    b.run(CMD_A_CLOSE_END, 0, 0, 0);
    assert_eq!(coast(&b), learned);
    // a service move between the step and the move back ends the pair
    b.run(CMD_A_OPEN, 0, 10, 0);
    b.service(0, DIR_OPEN, 50, 40);
    b.run(CMD_A_CLOSE_END, 0, 0, 0);
    assert_eq!(coast(&b), learned);
    // a step that meets an obstacle
    b.rig.valve[0].jam_from = 200;
    b.rig.valve[0].jam_to = 300;
    b.run(CMD_A_OPEN, 0, 10, 0);
    assert_ne!(b.last(0).stop_reason, StopReason::Target as u8);
    b.rig.valve[0].jam_from = -1;
    b.rig.valve[0].jam_to = -1;
    b.run(CMD_A_CLOSE_END, 0, 0, 0);
    assert_eq!(coast(&b), learned);
    // a second step, then the move back
    b.run(CMD_A_OPEN, 0, 10, 0);
    b.run(CMD_A_OPEN, 0, 10, 0);
    b.run(CMD_A_CLOSE_END, 0, 0, 0);
    assert_eq!(coast(&b), learned);
    // the presence test between the step and the move back
    b.run(CMD_A_OPEN, 0, 10, 0);
    b.run(CMD_A_TEST, 0, 0, 0);
    b.run(CMD_A_CLOSE_END, 0, 0, 0);
    assert_eq!(coast(&b), learned);
    // a partial move back that meets the end stop (not early): the valve is not taken as standing
    // at it, closing keeps the counted position
    b.run(CMD_A_OPEN, 0, 10, 0);
    b.run(CMD_A_CLOSE, 0, 13, 0);
    assert_eq!(b.last(0).stop_reason, StopReason::EndStop as u8);
    assert_eq!(coast(&b), learned);
}

#[test]
fn coast_moves_of_another_kind_short_steps_and_steps_that_do_not_end_on_their_count_give_no_sample()
{
    for rev in REVS {
        no_samples(rev);
    }
}

fn short_step(rev: BoardRev) {
    let mut b = calibrated(rev);
    // 36 pulses requested: shortened by half of them, not by the coast of 40
    b.run(CMD_A_OPEN, 0, 1, 0);
    assert_eq!(b.last(0).requested_counts, 36);
    assert_eq!(b.last(0).counted_counts, 19);
    assert_eq!(b.last(0).stop_reason, StopReason::Target as u8);
    assert_eq!(b.m.mots[0].actual_position, 51);
}

#[test]
fn coast_a_step_shorter_than_twice_the_coast_keeps_half_of_its_pulses() {
    for rev in REVS {
        short_step(rev);
    }
}

fn probe_at_an_obstacle(rev: BoardRev) {
    // the opening probe meets an obstacle 150 pulses above the closed end stop
    let mut b = learn_until(rev, AState::Probe1);
    b.rig.valve[0].jam_from = 150;
    b.rig.valve[0].jam_to = 151;
    assert!(b.run_until(|b| b.state() == AState::Probe2, 30000));
    b.rig.valve[0].jam_from = -1;
    b.rig.valve[0].jam_to = -1;
    learn_done(&mut b);
    // the calibration goes on: only the closing coast is learned
    assert_eq!(b.m.mots[0].status, ST_IDLE);
    assert!(b.m.mots[0].calibrated);
    assert_eq!(coast(&b), (0, 40));
}

#[test]
fn coast_a_probe_step_that_meets_an_obstacle_gives_no_sample_the_calibration_goes_on() {
    for rev in REVS {
        probe_at_an_obstacle(rev);
    }
}

fn failed_pass(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.rig.valve[0].coast_pulses = COAST;
    b.rig.valve[0].stroke = 2500;
    b.rig.valve[0].position = 1200;
    b.m.no_of_min_counts = 3000;
    b.learn(0);
    let mot = b.m.mots[0];
    assert_eq!(mot.status, ST_BLOCKED);
    assert_eq!(mot.fault_reason, ValveFault::StrokesTooShort as u8);
    // the probes of the pass ran, their samples are not kept
    assert_eq!(states(&b.rig), "5 6 18 19 7 18 19 8 1");
    assert_eq!(coast(&b), (0, 0));
}

#[test]
fn coast_a_calibration_that_is_not_accepted_keeps_no_sample() {
    for rev in REVS {
        failed_pass(rev);
    }
}

fn probe_back_on_the_safety_limit(rev: BoardRev) {
    // the move back of the opening probe stops on the 60 mA safety limit (a threshold of
    // 40 mA x 1.7 above it): no sample, the calibration goes on
    let mut b = learn_until(rev, AState::Probe1);
    let mean = b.m.mots[0].meancurrent;
    b.m.mots[0].meancurrent = 40;
    assert!(b.run_until(|b| b.state() == AState::Learn3, 30000));
    assert_eq!(b.last(0).stop_reason, StopReason::SafetyOvercurrent as u8);
    b.m.mots[0].meancurrent = mean;
    learn_done(&mut b);
    assert_eq!(b.m.mots[0].status, ST_IDLE);
    assert!(b.m.mots[0].calibrated);
    assert_eq!(coast(&b), (0, 40));
}

#[test]
fn coast_a_probe_move_back_that_stops_on_the_safety_limit_gives_no_sample() {
    for rev in REVS {
        probe_back_on_the_safety_limit(rev);
    }
}

fn sstop_in_a_probe(rev: BoardRev, state: AState) {
    let mut b = learn_until(rev, state);
    // the probe move runs
    b.run_ms(600);
    assert_eq!(b.state(), state);
    assert_eq!(b.m.appstop(0), 0);
    learn_done(&mut b);
    assert_eq!(b.last(0).stop_reason, StopReason::Aborted as u8);
    let mot = b.m.mots[0];
    assert_eq!(mot.status, ST_IDLE);
    assert!(mot.needs_reference);
    assert!(!mot.calibration);
    assert_eq!(mot.calib_seq, 0);
    assert_eq!(coast(&b), (0, 0));
}

#[test]
fn coast_sstop_in_a_probe_move_ends_the_calibration_without_a_result() {
    for rev in REVS {
        sstop_in_a_probe(rev, AState::Probe1);
        sstop_in_a_probe(rev, AState::Probe2);
    }
}

/// The calibration ended without a result: no coast.
fn no_result(b: &Bench) {
    assert_eq!(b.m.mots[0].calib_seq, 0);
    assert!(!b.m.mots[0].calib_active);
    assert_eq!(coast(b), (0, 0));
}

fn probe_failures(rev: BoardRev) {
    // the enforced inrush limit stops the step
    let mut b = learn_until(rev, AState::Probe1);
    b.flags.protect_enforce.store(true, Ordering::SeqCst);
    b.rig.valve[0].inrush_peak_dma = 2800;
    b.rig.valve[0].inrush_ms = 60;
    learn_done(&mut b);
    assert_eq!(b.m.mots[0].status, ST_FAILED);
    assert_eq!(b.m.mots[0].fault_reason, ValveFault::InrushTrip as u8);
    assert!(b.diag(0).last_cal_failed);
    no_result(&b);
    // no current in the move back
    let mut b = learn_until(rev, AState::Probe2);
    b.rig.valve[0].connected = false;
    learn_done(&mut b);
    assert_eq!(b.m.mots[0].status, ST_OPEN_CIRCUIT);
    assert!(b.m.mots[0].recal);
    no_result(&b);
    // the step stalls below its threshold until the move times out
    let mut b = learn_until(rev, AState::Probe1);
    b.rig.valve[0].stall_current_dma = 250;
    b.rig.valve[0].jam_from = 0;
    b.rig.valve[0].jam_to = 100_000;
    learn_done(&mut b);
    assert_eq!(b.m.mots[0].status, ST_FAILED);
    assert_eq!(b.m.mots[0].fault_reason, ValveFault::StrokeTimeout as u8);
    assert_eq!(b.last(0).stop_reason, StopReason::Timeout as u8);
    no_result(&b);
}

#[test]
fn coast_an_inrush_trip_no_current_or_a_timeout_of_a_probe_move_ends_the_calibration() {
    for rev in REVS {
        probe_failures(rev);
    }
}

fn temperature_gaps(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.flags.temp_refresh_request.store(true, Ordering::SeqCst);
    b.learn(0);
    // a due temperature cycle pauses the calibration at each end stop before its next move
    assert_eq!(states(&b.rig), "5 6 17 18 19 17 7 17 18 19 17 8 9 10 11 1");
    assert_eq!(b.m.mots[0].status, ST_IDLE);
}

#[test]
fn coast_a_due_temperature_cycle_pauses_the_calibration_before_and_after_each_probe() {
    for rev in REVS {
        temperature_gaps(rev);
    }
}

fn play_of_the_gears(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.rig.valve[0].coast_pulses = COAST;
    b.rig.valve[0].backlash_pulses = 15;
    b.learn(0);
    assert_eq!(b.m.mots[0].status, ST_IDLE);
    // both moves of a probe start with a reversal: the play counts in both
    assert_eq!(coast(&b), (40, 40));
}

#[test]
fn coast_the_play_of_the_gears_does_not_change_the_coast_a_probe_measures() {
    for rev in REVS {
        play_of_the_gears(rev);
    }
}

fn pairs_without_a_calibration(rev: BoardRev) {
    let mut b = start_valves(rev);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.rig.valve[0].coast_pulses = COAST;
    b.place(0, 50);
    // the first pair waits for a second one that agrees
    step_and_back(&mut b, DIR_CLOSE, 10);
    assert_eq!(coast(&b), (0, 0));
    step_and_back(&mut b, DIR_CLOSE, 10);
    assert_eq!(coast(&b), (40, 0));
}

#[test]
fn coast_without_a_calibration_two_agreeing_move_pairs_set_the_coast() {
    for rev in REVS {
        pairs_without_a_calibration(rev);
    }
}
