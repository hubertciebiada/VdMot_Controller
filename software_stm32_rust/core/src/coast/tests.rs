//! The coast samples, the estimate a calibration sets and move pairs refine, the shortened pulse
//! target and the move pairs of normal moves.

use super::*;

fn step(dir: u8, counted: u32) -> MoveEnd {
    MoveEnd {
        dir,
        count_reached: true,
        at_end: false,
        counted,
    }
}

fn back(dir: u8, counted: u32) -> MoveEnd {
    MoveEnd {
        dir,
        count_reached: false,
        at_end: true,
        counted,
    }
}

/// a valve with the coast of a calibration, at its closed end stop
fn calibrated(open: u16, close: u16) -> ValveCoast {
    let mut c = ValveCoast::default();
    c.set(DIR_OPEN, open);
    c.set(DIR_CLOSE, close);
    c.at_end(DIR_CLOSE);
    c
}

#[test]
fn opposite_swaps_the_directions() {
    assert_eq!(opposite(DIR_OPEN), DIR_CLOSE);
    assert_eq!(opposite(DIR_CLOSE), DIR_OPEN);
}

#[test]
fn coast_sample_is_the_move_back_beyond_the_step_up_to_the_limit() {
    assert_eq!(coast_sample(200, 260), Some(60));
    assert_eq!(coast_sample(200, 200), Some(0));
    assert_eq!(coast_sample(200, 300), Some(COAST_MAX_COUNTS));
    assert_eq!(coast_sample(200, 301), None);
    // the move back counted fewer pulses than the step
    assert_eq!(coast_sample(200, 199), None);
    // a difference beyond a u16
    assert_eq!(coast_sample(0, 70_000), None);
}

#[test]
fn shortened_target_takes_the_coast_off_but_keeps_half_of_the_pulses() {
    assert_eq!(shortened_target(200, 0), 200);
    assert_eq!(shortened_target(200, 60), 140);
    assert_eq!(shortened_target(200, 100), 100);
    assert_eq!(shortened_target(200, 101), 100);
    // a step shorter than twice the coast keeps half of its pulses (rounded up)
    assert_eq!(shortened_target(45, 60), 23);
    assert_eq!(shortened_target(1, 60), 1);
    assert_eq!(shortened_target(0, 60), 0);
}

#[test]
fn a_sample_agrees_within_half_of_the_reference_but_at_least_8() {
    assert!(agrees(40, 20));
    assert!(agrees(40, 60));
    assert!(!agrees(40, 19));
    assert!(!agrees(40, 61));
    assert!(agrees(0, COAST_AGREE_MIN));
    assert!(!agrees(0, COAST_AGREE_MIN + 1));
    assert!(agrees(10, 18));
    assert!(!agrees(10, 19));
    // 17 / 2 rounds down
    assert!(agrees(17, 25));
    assert!(!agrees(17, 26));
}

#[test]
fn a_calibration_sets_the_coast() {
    let mut e = CoastEstimate::default();
    assert_eq!(e.counts(), 0);
    e.set(60);
    assert_eq!(e.counts(), 60);
    // the next calibration replaces it
    e.set(40);
    assert_eq!(e.counts(), 40);
}

#[test]
fn a_move_pair_moves_a_learned_coast_by_a_rounded_quarter() {
    let mut e = CoastEstimate::default();
    e.set(60);
    // (3 * 60 + 40 + 2) / 4
    e.refine(40);
    assert_eq!(e.counts(), 55);
    // 55.25 rounds down, 55.5 rounds up
    e.refine(56);
    assert_eq!(e.counts(), 55);
    e.refine(57);
    assert_eq!(e.counts(), 56);
}

#[test]
fn a_move_pair_that_does_not_agree_with_the_learned_coast_is_dropped() {
    let mut e = CoastEstimate::default();
    e.set(40);
    e.refine(61);
    assert_eq!(e.counts(), 40);
    e.refine(19);
    assert_eq!(e.counts(), 40);
    e.refine(60);
    assert_eq!(e.counts(), 45);
    // a learned coast of 0
    e.set(0);
    e.refine(9);
    assert_eq!(e.counts(), 0);
    e.refine(8);
    assert_eq!(e.counts(), 2);
}

#[test]
fn without_a_calibration_two_agreeing_move_pairs_in_a_row_set_the_coast() {
    let mut e = CoastEstimate::default();
    e.refine(148);
    assert_eq!(e.counts(), 0);
    // does not agree with 148: it waits instead
    e.refine(40);
    assert_eq!(e.counts(), 0);
    // (40 + 51 + 1) / 2
    e.refine(51);
    assert_eq!(e.counts(), 46);
    // learned: the next sample refines it
    e.refine(50);
    assert_eq!(e.counts(), 47);
    // a calibration drops a waiting sample
    let mut e = CoastEstimate::default();
    e.refine(40);
    e.set(60);
    e.refine(50);
    assert_eq!(e.counts(), 58);
}

#[test]
fn a_valve_learns_each_direction_apart() {
    let mut c = ValveCoast::default();
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (0, 0));
    c.set(DIR_OPEN, 50);
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (50, 0));
    c.set(DIR_CLOSE, 70);
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (50, 70));
}

#[test]
fn a_step_up_from_the_closed_end_and_the_move_back_refine_the_opening_coast() {
    let mut c = calibrated(40, 40);
    c.on_move(&step(DIR_OPEN, 201));
    assert_eq!(
        c.probe,
        Probe::Away {
            end: DIR_CLOSE,
            counted: 201
        }
    );
    // 255 - 201
    c.on_move(&back(DIR_CLOSE, 255));
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (44, 40));
    assert_eq!(c.probe, Probe::AtEnd(DIR_CLOSE));
}

#[test]
fn a_step_down_from_the_open_end_and_the_move_back_refine_the_closing_coast() {
    let mut c = calibrated(40, 40);
    c.on_move(&back(DIR_OPEN, 1500));
    assert_eq!(c.probe, Probe::AtEnd(DIR_OPEN));
    c.on_move(&step(DIR_CLOSE, 181));
    c.on_move(&back(DIR_OPEN, 241));
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (40, 45));
    // the next pair refines it again
    c.on_move(&step(DIR_CLOSE, 181));
    c.on_move(&back(DIR_OPEN, 241));
    assert_eq!(c.counts(DIR_CLOSE), 49);
}

#[test]
fn a_step_needs_at_least_the_sample_minimum() {
    let mut c = calibrated(40, 40);
    c.on_move(&step(DIR_OPEN, COAST_SAMPLE_MIN_COUNTS - 1));
    assert_eq!(c.probe, Probe::None);
    c.at_end(DIR_CLOSE);
    c.on_move(&step(DIR_OPEN, COAST_SAMPLE_MIN_COUNTS));
    assert_eq!(
        c.probe,
        Probe::Away {
            end: DIR_CLOSE,
            counted: COAST_SAMPLE_MIN_COUNTS
        }
    );
}

#[test]
fn only_a_step_that_leaves_the_end_stop_starts_a_pair() {
    // not at an end stop
    let mut c = ValveCoast::default();
    c.on_move(&step(DIR_OPEN, 200));
    assert_eq!(c.probe, Probe::None);
    // a step towards the end stop it stands at
    c.at_end(DIR_CLOSE);
    c.on_move(&step(DIR_CLOSE, 200));
    assert_eq!(c.probe, Probe::None);
    // a step that did not end on its pulse count
    c.at_end(DIR_CLOSE);
    c.on_move(&MoveEnd {
        dir: DIR_OPEN,
        count_reached: false,
        at_end: false,
        counted: 200,
    });
    assert_eq!(c.probe, Probe::None);
}

#[test]
fn a_second_step_ends_the_pair() {
    let mut c = calibrated(40, 40);
    c.on_move(&step(DIR_OPEN, 200));
    c.on_move(&step(DIR_OPEN, 200));
    assert_eq!(c.probe, Probe::None);
    c.on_move(&back(DIR_CLOSE, 460));
    assert_eq!(c.counts(DIR_OPEN), 40);
    assert_eq!(c.probe, Probe::AtEnd(DIR_CLOSE));
}

#[test]
fn a_move_to_the_other_end_stop_gives_no_sample() {
    let mut c = calibrated(40, 40);
    c.on_move(&step(DIR_OPEN, 200));
    c.on_move(&back(DIR_OPEN, 4300));
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (40, 40));
    assert_eq!(c.probe, Probe::AtEnd(DIR_OPEN));
}

#[test]
fn a_move_back_outside_the_limits_gives_no_sample_but_ends_at_the_end_stop() {
    let mut c = calibrated(40, 40);
    c.on_move(&step(DIR_OPEN, 200));
    c.on_move(&back(DIR_CLOSE, 190));
    assert_eq!(c.counts(DIR_OPEN), 40);
    assert_eq!(c.probe, Probe::AtEnd(DIR_CLOSE));
    c.on_move(&step(DIR_OPEN, 200));
    c.on_move(&back(DIR_CLOSE, 301));
    assert_eq!(c.counts(DIR_OPEN), 40);
}

#[test]
fn forget_ends_the_pair() {
    let mut c = calibrated(40, 40);
    c.on_move(&step(DIR_OPEN, 200));
    c.forget();
    assert_eq!(c.probe, Probe::None);
    c.on_move(&back(DIR_CLOSE, 260));
    assert_eq!(c.counts(DIR_OPEN), 40);
}

#[test]
fn without_a_calibration_two_pairs_of_a_direction_set_its_coast() {
    let mut c = ValveCoast::default();
    c.on_move(&back(DIR_CLOSE, 900));
    c.on_move(&step(DIR_OPEN, 321));
    c.on_move(&back(DIR_CLOSE, 361));
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (0, 0));
    c.on_move(&step(DIR_OPEN, 321));
    c.on_move(&back(DIR_CLOSE, 365));
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (42, 0));
}
