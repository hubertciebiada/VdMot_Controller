// New cases (no C++ counterpart): the brake at the end of a service move (sbrak, 2.2.9). On the
// stopping pulse the direction flips with the enable on; the brake ends when no pulse came for
// the gap, or when its time is up (then a motor that already stands turns back); the end-stop
// detector is blind meanwhile; sstop ends it; a normal move, a calibration stroke and the coast
// probe are never braked.
use core::sync::atomic::Ordering;

use vdm_stm_core::move_classifier::{StopReason, DIR_CLOSE, DIR_OPEN};
use vdm_stm_core::valve_codes::ST_IDLE;

use crate::board::BoardRev;
use crate::hal::{Out, Pins, RevIrq};

use super::bench::{start_valves, Bench, REVS};
use super::{CMD_A_OPEN, SBRAK_MS_MAX};

/// pulses the sim's motor turns after its enable went off
const COAST: i32 = 40;
/// forward pulses of the sim's motor after the direction flips with the enable on
const MOMENTUM: i32 = 10;

/// Valve 0 at 20 % (sim position 720), one pulse per ms, coasting COAST pulses, braking over
/// MOMENTUM pulses.
fn placed(rev: BoardRev) -> Bench {
    let mut b = start_valves(rev);
    b.place(0, 20);
    b.rig.valve[0].pulses_per_ms = 1.0;
    b.rig.valve[0].coast_pulses = COAST;
    b.rig.valve[0].brake_pulses = MOMENTUM;
    b
}

/// Pulses the valve really turned in `dir` since the sim position 720.
fn travel(b: &Bench, dir: u8) -> i32 {
    let moved = b.rig.valve[0].position - 720;
    if dir == DIR_OPEN {
        moved
    } else {
        -moved
    }
}

fn braking(b: &Bench) -> bool {
    b.pulse.braking.load(Ordering::SeqCst)
}

/// A service move of valve 0, and the coast of the sim's motor after it.
fn service(b: &mut Bench, dir: u8, counts: u16) {
    b.service(0, dir, counts, 40);
    b.run_ms(100);
}

fn no_brake_by_default(rev: BoardRev) {
    let mut b = placed(rev);
    service(&mut b, DIR_OPEN, 500);
    assert_eq!(b.last(0).counted_counts, 500);
    assert_eq!(b.last(0).stop_reason, StopReason::Target as u8);
    // the full coast after the stopping pulse
    assert_eq!(travel(&b, DIR_OPEN), 500 + COAST);
    // the direction output never flipped
    assert!(!b.board.latch(Out::Dir));
    assert!(!braking(&b));
}

#[test]
fn a_service_move_without_a_brake_coasts_as_before() {
    for rev in REVS {
        no_brake_by_default(rev);
    }
}

fn brake_ends_on_the_pulse_gap(rev: BoardRev, dir: u8) {
    let mut b = placed(rev);
    assert_eq!(b.m.appsetbrake(0, 50, 3), 0);
    service(&mut b, dir, 500);
    let r = b.last(0);
    assert_eq!(r.stop_reason, StopReason::Target as u8);
    assert_eq!(r.requested_counts, 500);
    // the pulses of the slowing motor are counted; the gap of 3 ms ends the brake 17 ms after
    // the flip, after the 8th of the 10 momentum pulses, the rest of the momentum coasts 8
    // pulses (40 without the brake)
    assert_eq!(r.counted_counts, 508);
    assert_eq!(travel(&b, dir), 516);
    assert_eq!(b.m.mots[0].status, ST_IDLE);
    assert_eq!(b.rig.valve[0].enables, 1);
    // the direction output stays flipped until the next move sets it
    assert_eq!(b.board.latch(Out::Dir), dir == DIR_OPEN);
    assert!(!braking(&b));
}

#[test]
fn the_brake_flips_the_direction_on_the_stopping_pulse_and_ends_when_the_pulses_stop() {
    for rev in REVS {
        brake_ends_on_the_pulse_gap(rev, DIR_OPEN);
        brake_ends_on_the_pulse_gap(rev, DIR_CLOSE);
    }
}

fn brake_on_the_time_only(rev: BoardRev) {
    let mut b = placed(rev);
    assert_eq!(b.m.appsetbrake(0, 50, 0), 0);
    service(&mut b, DIR_OPEN, 500);
    let r = b.last(0);
    assert_eq!(r.stop_reason, StopReason::Target as u8);
    // the motor stood 28 ms after the flip, after its 10 momentum pulses, and turned back for
    // the rest of the 50 ms: its 4 pulses count as forward travel, and it coasted back further,
    // so the valve ended short of where the count says
    assert_eq!(r.counted_counts, 514);
    assert_eq!(travel(&b, DIR_OPEN), 486);
    assert!(!braking(&b));
}

#[test]
fn a_brake_on_the_time_only_that_outlasts_the_motion_turns_the_motor_back() {
    for rev in REVS {
        brake_on_the_time_only(rev);
    }
}

fn detector_blind_while_braking(rev: BoardRev) {
    let mut b = placed(rev);
    // far above the hard limit: sampled, the filter would trip within the brake
    b.rig.valve[0].brake_current_dma = 2900;
    assert_eq!(b.m.appsetbrake(0, 50, 0), 0);
    service(&mut b, DIR_OPEN, 500);
    let r = b.last(0);
    assert_eq!(r.stop_reason, StopReason::Target as u8);
    assert_eq!(r.counted_counts, 514);
    // the peak is the running current
    assert!(r.peak_current < 400, "{}", r.peak_current);
    assert_eq!(b.m.mots[0].status, ST_IDLE);
}

#[test]
fn the_end_stop_limits_do_not_judge_the_brake_current() {
    for rev in REVS {
        detector_blind_while_braking(rev);
    }
}

fn sstop_ends_the_brake(rev: BoardRev) {
    let mut b = placed(rev);
    b.rig.valve[0].brake_pulses = 60;
    assert_eq!(b.m.appsetbrake(0, 100, 0), 0);
    assert_eq!(b.appsetservice(0, DIR_OPEN, 500, 40), 0);
    b.run_ms(10);
    assert!(b.run_until(braking, 5000));
    assert_eq!(b.m.appstop(0), 0);
    assert!(b.run_until(|b| !braking(b), 20));
    // the motor state machine halted the motor before the brake time was up
    assert!(b.run_until(Bench::idle, 5000));
    let r = b.last(0);
    assert_eq!(r.stop_reason, StopReason::Aborted as u8);
    assert!(
        r.counted_counts >= 500 && r.counted_counts < 520,
        "{}",
        r.counted_counts
    );
    assert_eq!(b.rig.valve[0].enables, 1);
    assert_eq!(b.m.mots[0].status, ST_IDLE);
}

#[test]
fn s8_sstop_during_the_brake_ends_it_with_the_move() {
    for rev in REVS {
        sstop_ends_the_brake(rev);
    }
}

fn normal_moves_and_calibrations_not_braked(rev: BoardRev) {
    let mut b = placed(rev);
    assert_eq!(b.m.appsetbrake(255, 50, 3), 0);
    b.run(CMD_A_OPEN, 0, 10, 0);
    b.run_ms(100);
    // the pulse that stops the motor and the full coast, as without a brake
    assert_eq!(b.last(0).counted_counts, 361);
    assert_eq!(travel(&b, DIR_OPEN), 361 + COAST);
    assert!(!b.board.latch(Out::Dir));
    // the coast probe of a calibration measures the coast of an unbraked step
    b.learn(0);
    assert_eq!(b.m.mots[0].status, ST_IDLE);
    assert_eq!(b.m.mots[0].coast.counts(DIR_CLOSE), COAST as u16);
}

#[test]
fn a_normal_move_and_a_calibration_are_not_braked() {
    for rev in REVS {
        normal_moves_and_calibrations_not_braked(rev);
    }
}

#[test]
fn appsetbrake_sets_one_valve_or_all_clamps_to_the_maximum_and_refuses_an_invalid_valve() {
    let mut b = start_valves(BoardRev::C2);
    assert_eq!(b.m.appsetbrake(255, 30, 5), 0);
    for mot in &b.m.mots {
        assert_eq!((mot.brake_max_ms, mot.brake_gap_ms), (30, 5));
    }
    assert_eq!(b.m.appsetbrake(3, 200, 150), 0);
    assert_eq!(
        (b.m.mots[3].brake_max_ms, b.m.mots[3].brake_gap_ms),
        (SBRAK_MS_MAX, SBRAK_MS_MAX)
    );
    assert_eq!(
        (b.m.mots[2].brake_max_ms, b.m.mots[2].brake_gap_ms),
        (30, 5)
    );
    assert_eq!(b.m.appsetbrake(12, 1, 1), -1);
    assert_eq!(b.m.appsetbrake(254, 1, 1), -1);
    assert_eq!(
        (b.m.mots[2].brake_max_ms, b.m.mots[2].brake_gap_ms),
        (30, 5)
    );
}

#[test]
fn the_stopping_pulse_of_a_motor_that_a_trip_already_stopped_is_not_braked() {
    // the EXTI handler alone: target reached, the brake armed, but the motor off (turning false)
    let b = start_valves(BoardRev::C2);
    b.board.attach();
    b.pulse.arm_brake(50, 3);
    b.pulse.target.store(0, Ordering::SeqCst);
    b.pulse.turning.store(false, Ordering::SeqCst);
    b.pulse.on_edge(&b.board);
    assert!(!braking(&b));
    assert!(b.pulse.stop_request.load(Ordering::SeqCst));
    assert!(!b.board.latch(Out::Dir));
    // armed again for the next move: a brake marked in progress is over
    b.pulse.braking.store(true, Ordering::SeqCst);
    b.pulse.arm_brake(0, 0);
    assert!(!braking(&b));
    assert_eq!(b.pulse.brake_max_ms.load(Ordering::SeqCst), 0);
}
