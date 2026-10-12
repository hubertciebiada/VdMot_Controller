//! The coast samples, the estimate a calibration sets and move pairs refine, the shortened pulse
//! target and the move pairs of normal moves (closing steps from the open end stop).

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

/// a valve with the closing coast of a calibration that a normal move opened
fn calibrated(close: u16) -> ValveCoast {
    let mut c = ValveCoast::default();
    c.set(close);
    c.on_move(&back(DIR_OPEN, 900));
    c
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
    e.set(COAST_MAX_COUNTS + 1);
    assert_eq!(e.counts(), COAST_MAX_COUNTS);
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
fn a_move_pair_that_does_not_agree_with_the_learned_coast_changes_it_not_alone() {
    let mut e = CoastEstimate::default();
    e.set(40);
    e.refine(61);
    assert_eq!(e.counts(), 40);
    // does not agree with 61 either
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
fn two_move_pairs_in_a_row_that_agree_with_each_other_but_not_with_the_coast_replace_it() {
    let mut e = CoastEstimate::default();
    e.set(90);
    e.refine(40);
    assert_eq!(e.counts(), 90);
    // (40 + 42 + 1) / 2
    e.refine(42);
    assert_eq!(e.counts(), 41);
    // a sample that agrees with the coast ends the row
    e.set(40);
    e.refine(70);
    e.refine(44);
    assert_eq!(e.counts(), 41);
    e.refine(70);
    assert_eq!(e.counts(), 41);
    e.refine(71);
    assert_eq!(e.counts(), 71);
    // a calibration drops a waiting sample
    e.set(40);
    e.refine(90);
    e.set(40);
    e.refine(92);
    assert_eq!(e.counts(), 40);
    e.refine(93);
    assert_eq!(e.counts(), 93);
}

#[test]
fn without_a_calibration_two_agreeing_move_pairs_in_a_row_set_the_coast() {
    let mut e = CoastEstimate::default();
    e.refine(148);
    assert_eq!(e.counts(), 0);
    // does not agree with 100 (148 limited): it waits instead
    e.refine(40);
    assert_eq!(e.counts(), 0);
    // (40 + 51 + 1) / 2
    e.refine(51);
    assert_eq!(e.counts(), 46);
    // learned: the next sample refines it
    e.refine(50);
    assert_eq!(e.counts(), 47);
    // samples beyond the limit count as the limit
    let mut e = CoastEstimate::default();
    e.refine(150);
    e.refine(150);
    assert_eq!(e.counts(), COAST_MAX_COUNTS);
}

#[test]
fn two_samples_agree_on_half_of_the_smaller_one_in_either_order() {
    assert!(pair_agrees(40, 60));
    assert!(pair_agrees(60, 40));
    assert!(!pair_agrees(40, 61));
    assert!(!pair_agrees(61, 40));
    for (a, b) in [(40, 61), (61, 40)] {
        let mut e = CoastEstimate::default();
        e.refine(a);
        e.refine(b);
        assert_eq!(e.counts(), 0);
    }
}

#[test]
fn only_the_closing_coast_is_learned() {
    let mut c = ValveCoast::default();
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (0, 0));
    c.set(70);
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (0, 70));
}

#[test]
fn a_step_down_from_the_open_end_and_the_move_back_refine_the_closing_coast() {
    let mut c = calibrated(40);
    assert_eq!(c.probe, Probe::AtOpenEnd);
    c.on_move(&step(DIR_CLOSE, 181));
    assert_eq!(c.probe, Probe::Away { counted: 181 });
    // 241 - 181
    c.on_move(&back(DIR_OPEN, 241));
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (0, 45));
    assert_eq!(c.probe, Probe::AtOpenEnd);
    // the next pair refines it again
    c.on_move(&step(DIR_CLOSE, 181));
    c.on_move(&back(DIR_OPEN, 241));
    assert_eq!(c.counts(DIR_CLOSE), 49);
}

#[test]
fn a_step_up_from_the_closed_end_and_the_move_back_give_no_sample() {
    let mut c = calibrated(40);
    c.on_move(&back(DIR_CLOSE, 900));
    assert_eq!(c.probe, Probe::None);
    c.on_move(&step(DIR_OPEN, 201));
    assert_eq!(c.probe, Probe::None);
    c.on_move(&back(DIR_CLOSE, 255));
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (0, 40));
    assert_eq!(c.probe, Probe::None);
}

#[test]
fn a_step_needs_at_least_the_sample_minimum() {
    let mut c = calibrated(40);
    c.on_move(&step(DIR_CLOSE, COAST_SAMPLE_MIN_COUNTS - 1));
    assert_eq!(c.probe, Probe::None);
    c.on_move(&back(DIR_OPEN, 900));
    c.on_move(&step(DIR_CLOSE, COAST_SAMPLE_MIN_COUNTS));
    assert_eq!(
        c.probe,
        Probe::Away {
            counted: COAST_SAMPLE_MIN_COUNTS
        }
    );
}

#[test]
fn only_a_closing_step_that_leaves_the_open_end_stop_starts_a_pair() {
    // not at the open end stop
    let mut c = ValveCoast::default();
    c.on_move(&step(DIR_CLOSE, 200));
    assert_eq!(c.probe, Probe::None);
    // an opening step at the open end stop
    c.on_move(&back(DIR_OPEN, 900));
    c.on_move(&step(DIR_OPEN, 200));
    assert_eq!(c.probe, Probe::None);
    // a closing step that did not end on its pulse count
    c.on_move(&back(DIR_OPEN, 900));
    c.on_move(&MoveEnd {
        dir: DIR_CLOSE,
        count_reached: false,
        at_end: false,
        counted: 200,
    });
    assert_eq!(c.probe, Probe::None);
    // an opening move that ends without reaching the open end stop
    c.on_move(&back(DIR_OPEN, 900));
    c.on_move(&step(DIR_CLOSE, 200));
    assert_eq!(c.probe, Probe::Away { counted: 200 });
    c.on_move(&MoveEnd {
        dir: DIR_OPEN,
        count_reached: false,
        at_end: false,
        counted: 260,
    });
    assert_eq!(c.probe, Probe::None);
    assert_eq!(c.counts(DIR_CLOSE), 0);
}

#[test]
fn a_second_step_ends_the_pair() {
    let mut c = calibrated(40);
    c.on_move(&step(DIR_CLOSE, 200));
    c.on_move(&step(DIR_CLOSE, 200));
    assert_eq!(c.probe, Probe::None);
    c.on_move(&back(DIR_OPEN, 460));
    assert_eq!(c.counts(DIR_CLOSE), 40);
    assert_eq!(c.probe, Probe::AtOpenEnd);
}

#[test]
fn a_move_to_the_closed_end_stop_gives_no_sample() {
    let mut c = calibrated(40);
    c.on_move(&step(DIR_CLOSE, 200));
    c.on_move(&back(DIR_CLOSE, 4300));
    assert_eq!(c.counts(DIR_CLOSE), 40);
    assert_eq!(c.probe, Probe::None);
}

#[test]
fn a_move_back_outside_the_limits_gives_no_sample_but_ends_at_the_end_stop() {
    let mut c = calibrated(40);
    c.on_move(&step(DIR_CLOSE, 200));
    c.on_move(&back(DIR_OPEN, 190));
    assert_eq!(c.counts(DIR_CLOSE), 40);
    assert_eq!(c.probe, Probe::AtOpenEnd);
    c.on_move(&step(DIR_CLOSE, 200));
    c.on_move(&back(DIR_OPEN, 301));
    assert_eq!(c.counts(DIR_CLOSE), 40);
}

#[test]
fn forget_ends_the_pair() {
    let mut c = calibrated(40);
    c.on_move(&step(DIR_CLOSE, 200));
    c.forget();
    assert_eq!(c.probe, Probe::None);
    c.on_move(&back(DIR_OPEN, 260));
    assert_eq!(c.counts(DIR_CLOSE), 40);
}

#[test]
fn without_a_calibration_two_pairs_set_the_closing_coast() {
    let mut c = ValveCoast::default();
    c.on_move(&back(DIR_OPEN, 900));
    c.on_move(&step(DIR_CLOSE, 321));
    c.on_move(&back(DIR_OPEN, 361));
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (0, 0));
    c.on_move(&step(DIR_CLOSE, 321));
    c.on_move(&back(DIR_OPEN, 365));
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (0, 42));
}
