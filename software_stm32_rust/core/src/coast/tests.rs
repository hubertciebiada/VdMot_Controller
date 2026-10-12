//! The coast samples, the running mean, the shortened pulse target and the move pairs of
//! normal operation.

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

#[test]
fn opposite_swaps_the_directions() {
    assert_eq!(opposite(DIR_OPEN), DIR_CLOSE);
    assert_eq!(opposite(DIR_CLOSE), DIR_OPEN);
}

#[test]
fn coast_sample_is_the_move_back_beyond_the_step_up_to_the_limit() {
    assert_eq!(coast_sample(200, 260), Some(60));
    assert_eq!(coast_sample(200, 200), Some(0));
    assert_eq!(coast_sample(200, 400), Some(COAST_MAX_COUNTS));
    assert_eq!(coast_sample(200, 401), None);
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
fn a_coast_estimate_takes_the_first_sample_then_a_rounded_running_mean() {
    let mut e = CoastEstimate::default();
    assert_eq!(e.counts(), 0);
    e.add(60);
    assert_eq!(e.counts(), 60);
    // (3 * 60 + 40 + 2) / 4
    e.add(40);
    assert_eq!(e.counts(), 55);
    // 55.25 rounds down, 55.5 rounds up
    e.add(56);
    assert_eq!(e.counts(), 55);
    e.add(57);
    assert_eq!(e.counts(), 56);
}

#[test]
fn a_first_sample_of_0_counts_as_learned() {
    let mut e = CoastEstimate::default();
    e.add(0);
    e.add(100);
    assert_eq!(e.counts(), 25);
}

#[test]
fn the_running_mean_of_the_largest_values_fits() {
    let mut e = CoastEstimate::default();
    e.add(u16::MAX);
    e.add(u16::MAX);
    assert_eq!(e.counts(), u16::MAX);
}

#[test]
fn a_valve_learns_each_direction_apart() {
    let mut c = ValveCoast::default();
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (0, 0));
    c.add(DIR_OPEN, 50);
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (50, 0));
    c.add(DIR_CLOSE, 70);
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (50, 70));
}

#[test]
fn a_step_up_from_the_closed_end_and_the_move_back_give_the_opening_coast() {
    let mut c = ValveCoast::default();
    c.at_end(DIR_CLOSE);
    c.on_move(&step(DIR_OPEN, 201));
    assert_eq!(
        c.probe,
        Probe::Away {
            end: DIR_CLOSE,
            counted: 201
        }
    );
    c.on_move(&back(DIR_CLOSE, 255));
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (54, 0));
    assert_eq!(c.probe, Probe::AtEnd(DIR_CLOSE));
}

#[test]
fn a_step_down_from_the_open_end_and_the_move_back_give_the_closing_coast() {
    let mut c = ValveCoast::default();
    c.on_move(&back(DIR_OPEN, 1500));
    assert_eq!(c.probe, Probe::AtEnd(DIR_OPEN));
    c.on_move(&step(DIR_CLOSE, 181));
    c.on_move(&back(DIR_OPEN, 243));
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (0, 62));
    // the next pair adds to the mean
    c.on_move(&step(DIR_CLOSE, 181));
    c.on_move(&back(DIR_OPEN, 239));
    assert_eq!(c.counts(DIR_CLOSE), 61);
}

#[test]
fn a_step_needs_at_least_the_sample_minimum() {
    let mut c = ValveCoast::default();
    c.at_end(DIR_CLOSE);
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
    let mut c = ValveCoast::default();
    c.at_end(DIR_CLOSE);
    c.on_move(&step(DIR_OPEN, 200));
    c.on_move(&step(DIR_OPEN, 200));
    assert_eq!(c.probe, Probe::None);
    c.on_move(&back(DIR_CLOSE, 460));
    assert_eq!(c.counts(DIR_OPEN), 0);
    assert_eq!(c.probe, Probe::AtEnd(DIR_CLOSE));
}

#[test]
fn a_move_to_the_other_end_stop_gives_no_sample() {
    let mut c = ValveCoast::default();
    c.at_end(DIR_CLOSE);
    c.on_move(&step(DIR_OPEN, 200));
    c.on_move(&back(DIR_OPEN, 4300));
    assert_eq!((c.counts(DIR_OPEN), c.counts(DIR_CLOSE)), (0, 0));
    assert_eq!(c.probe, Probe::AtEnd(DIR_OPEN));
}

#[test]
fn a_move_back_outside_the_limits_gives_no_sample_but_ends_at_the_end_stop() {
    let mut c = ValveCoast::default();
    c.at_end(DIR_CLOSE);
    c.on_move(&step(DIR_OPEN, 200));
    c.on_move(&back(DIR_CLOSE, 190));
    assert_eq!(c.counts(DIR_OPEN), 0);
    assert_eq!(c.probe, Probe::AtEnd(DIR_CLOSE));
    c.on_move(&step(DIR_OPEN, 200));
    c.on_move(&back(DIR_CLOSE, 401));
    assert_eq!(c.counts(DIR_OPEN), 0);
}

#[test]
fn forget_ends_the_pair() {
    let mut c = ValveCoast::default();
    c.at_end(DIR_CLOSE);
    c.on_move(&step(DIR_OPEN, 200));
    c.forget();
    assert_eq!(c.probe, Probe::None);
    c.on_move(&back(DIR_CLOSE, 260));
    assert_eq!(c.counts(DIR_OPEN), 0);
}
