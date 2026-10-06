//! Port of test/native/test_end_stop_detector__inrush.cpp.
//! EndStopDetector inrush limit (W10, C-4): off, report only, enforced.

use super::*;

/// 1-based index of the first trip within n samples of raw, 0 if none
fn first_trip(d: &mut EndStopDetector, raw: i32, n: i32) -> i32 {
    for i in 1..=n {
        if d.sample(raw) != Trip::None {
            return i;
        }
    }
    0
}

#[test]
fn inrush_constants() {
    assert_eq!(EndStopDetector::INRUSH_LIMIT, 2500);
    assert_eq!(EndStopDetector::INRUSH_CONSECUTIVE, 20);
}

#[test]
fn inrush_enforced_2600_constant_trips_hard_at_sample_21_with_the_raw_current() {
    let mut d = EndStopDetector::default();
    d.arm(-500, 500, InrushMode::Enforce);
    assert_eq!(first_trip(&mut d, 2600, 300), 21);
    assert_eq!(d.trip(), Trip::Hard);
    assert_eq!(d.trip_current(), 2600);
    assert_eq!(d.peak(), 2600);
    assert!(d.inrush_trip());
    assert!(d.inrush_seen());
    assert_eq!(d.current(), 0);
}

#[test]
fn inrush_enforced_negative_currents_trip_like_positive_ones() {
    let mut d = EndStopDetector::default();
    d.arm(-500, 500, InrushMode::Enforce);
    assert_eq!(first_trip(&mut d, -2600, 300), 21);
    assert_eq!(d.trip_current(), -2600);
    assert_eq!(d.peak(), 2600);
}

#[test]
fn inrush_2500_the_limit_itself_and_2400_never_trip_in_the_inrush_time() {
    let mut d = EndStopDetector::default();
    d.arm(-5000, 5000, InrushMode::Enforce);
    assert_eq!(first_trip(&mut d, 2500, 250), 0);
    assert!(!d.inrush_seen());
    d.idle();
    d.arm(-5000, 5000, InrushMode::Enforce);
    assert_eq!(first_trip(&mut d, 2400, 250), 0);
    assert_eq!(d.trip(), Trip::None);
}

#[test]
fn inrush_a_sample_below_the_limit_restarts_the_run() {
    let mut d = EndStopDetector::default();
    d.arm(-5000, 5000, InrushMode::Enforce);
    assert_eq!(first_trip(&mut d, 2600, 20), 0);
    assert_eq!(first_trip(&mut d, 0, 1), 0);
    assert_eq!(first_trip(&mut d, 2600, 20), 0);
    assert!(!d.inrush_seen());
    assert_eq!(first_trip(&mut d, 2600, 1), 1);
}

#[test]
fn inrush_idle_restarts_the_run_the_limit_ends_with_the_inrush_time() {
    let mut d = EndStopDetector::default();
    d.arm(-5000, 5000, InrushMode::Enforce);
    assert_eq!(first_trip(&mut d, 2600, 20), 0);
    d.idle();
    d.arm(-5000, 5000, InrushMode::Enforce);
    assert_eq!(first_trip(&mut d, 2600, 20), 0);
    assert!(!d.inrush_seen());
    d.idle();
    d.arm(-100_000, 100_000, InrushMode::Enforce);
    assert_eq!(first_trip(&mut d, 0, 240), 0);
    // samples 241..250 are inside the inrush time, 251.. are filtered
    assert_eq!(first_trip(&mut d, 2600, 20), 0);
    assert!(!d.inrush_seen());
}

#[test]
fn inrush_reported_seen_but_no_trip_the_move_goes_on() {
    let mut d = EndStopDetector::default();
    d.arm(-100_000, 100_000, InrushMode::Report);
    assert_eq!(first_trip(&mut d, 2600, 21), 0);
    assert!(d.inrush_seen());
    assert!(!d.inrush_trip());
    assert_eq!(d.trip(), Trip::None);
    assert_eq!(d.peak(), 0);
    d.arm(-100_000, 100_000, InrushMode::Report);
    assert!(!d.inrush_seen());
}

#[test]
fn inrush_off_nothing_is_seen() {
    let mut d = EndStopDetector::default();
    // C++ arm(low, high) with the default argument: InrushMode::default() is Off
    d.arm(-100_000, 100_000, InrushMode::default());
    assert_eq!(first_trip(&mut d, 2800, 100), 0);
    assert!(!d.inrush_seen());
    d.idle();
    d.arm(-100_000, 100_000, InrushMode::Off);
    assert_eq!(first_trip(&mut d, 2800, 100), 0);
    assert!(!d.inrush_seen());
}

#[test]
fn inrush_enforced_a_later_trip_after_the_inrush_trip_keeps_the_first() {
    let mut d = EndStopDetector::default();
    d.arm(-500, 500, InrushMode::Enforce);
    assert_eq!(first_trip(&mut d, 2600, 25), 21);
    assert_eq!(d.sample(2600), Trip::Hard);
    assert_eq!(d.trip_current(), 2600);
    assert!(d.inrush_trip());
    d.arm(-500, 500, InrushMode::Enforce);
    assert!(!d.inrush_trip());
    assert_eq!(d.trip(), Trip::None);
}
