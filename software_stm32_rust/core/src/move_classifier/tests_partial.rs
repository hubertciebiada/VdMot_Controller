//! Port of test/native/test_move_classifier__partial.cpp: partial moves that end at an end
//! stop (W9): early check, position, run of early stops.

use super::*;

/// C++ aggregate `MoveRequest{dir, requestedCounts, expectedTravelPct, learnedTravel}`:
/// partial_early_check keeps its default (false)
fn request(
    dir: u8,
    requested_counts: u16,
    expected_travel_pct: u8,
    learned_travel: u32,
) -> MoveRequest {
    MoveRequest {
        dir,
        requested_counts,
        expected_travel_pct,
        learned_travel,
        ..Default::default()
    }
}

fn partial_checked(counts: u16) -> MoveRequest {
    let mut r = request(DIR_OPEN, counts, 0, 4000);
    r.partial_early_check = true;
    r
}

#[test]
fn classify_move_a_checked_partial_move_is_early_below_80_pct_of_the_requested_pulses() {
    assert_eq!(PARTIAL_EARLY_PCT, 80);
    let r = partial_checked(1000);
    let mut c: MoveClassification = classify_move(&r, MotorStop::EndStop, 799);
    assert_eq!(c.reason, StopReason::EarlyEndStop);
    assert!(c.early);
    c = classify_move(&r, MotorStop::EndStop, 800);
    assert_eq!(c.reason, StopReason::EndStop);
    assert!(!c.early);
    c = classify_move(&r, MotorStop::SafetyOvercurrent, 500);
    assert_eq!(c.reason, StopReason::SafetyOvercurrent);
    assert!(c.early);
    c = classify_move(&r, MotorStop::SafetyOvercurrent, 800);
    assert!(!c.early);
    c = classify_move(&r, MotorStop::CountReached, 10);
    assert_eq!(c.reason, StopReason::Target);
    assert!(!c.early);
    c = classify_move(&r, MotorStop::Undercurrent, 10);
    assert_eq!(c.reason, StopReason::Undercurrent);
    assert!(!c.early);
    // 79 % of 40 x 89
    let s = partial_checked(3560);
    assert!(classify_move(&s, MotorStop::EndStop, 2812).early);
    assert!(!classify_move(&s, MotorStop::EndStop, 2848).early);
}

#[test]
fn classify_move_no_partial_check_for_service_moves_and_calibration_strokes() {
    let r = request(DIR_OPEN, 1000, 0, 4000);
    let c = classify_move(&r, MotorStop::EndStop, 10);
    assert_eq!(c.reason, StopReason::EndStop);
    assert!(!c.early);
    assert!(!r.partial_early_check);
}

#[test]
fn classify_move_the_check_does_not_overflow_for_the_largest_request() {
    let r = partial_checked(0xFFFE);
    // 80 % of 65534 = 52427.2
    assert!(classify_move(&r, MotorStop::EndStop, 52427).early);
    assert!(!classify_move(&r, MotorStop::EndStop, 52428).early);
    assert!(!classify_move(&r, MotorStop::EndStop, 0xFFFF_FFFF).early);
}

#[test]
fn classify_move_a_move_to_the_end_stop_keeps_its_rule_with_partial_early_check() {
    let mut r = request(DIR_CLOSE, RUN_TO_END_STOP, 100, 4000);
    r.partial_early_check = true;
    assert!(classify_move(&r, MotorStop::EndStop, 1999).early);
    assert!(!classify_move(&r, MotorStop::EndStop, 2000).early);
}

#[test]
fn position_after_end_stop_from_the_counted_pulses_for_a_partial_move() {
    assert_eq!(
        position_after_end_stop(20, DIR_OPEN, 40 * 89, 5 * 89, 89),
        25
    );
    assert_eq!(
        position_after_end_stop(20, DIR_OPEN, 40 * 89, 5 * 89 + 88, 89),
        25
    );
    assert_eq!(
        position_after_end_stop(10, DIR_CLOSE, 40 * 89, 1001 * 89, 89),
        0
    );
    assert_eq!(
        position_after_end_stop(10, DIR_CLOSE, 40 * 89, 10 * 89, 89),
        0
    );
    assert_eq!(
        position_after_end_stop(10, DIR_CLOSE, 40 * 89, 9 * 89, 89),
        1
    );
    assert_eq!(
        position_after_end_stop(95, DIR_OPEN, 40 * 89, 20 * 89, 89),
        100
    );
    assert_eq!(
        position_after_end_stop(95, DIR_OPEN, 40 * 89, 5 * 89, 89),
        100
    );
    assert_eq!(
        position_after_end_stop(95, DIR_OPEN, 40 * 89, 4 * 89, 89),
        99
    );
    assert_eq!(
        position_after_end_stop(0, DIR_OPEN, 100, 0xFFFF_FFFF, 1),
        100
    );
    assert_eq!(
        position_after_end_stop(33, DIR_OPEN, 40 * 89, 5 * 89, 0),
        33
    );
    assert_eq!(
        position_after_end_stop(33, DIR_CLOSE, 40 * 89, 5 * 89, 0),
        33
    );
    assert_eq!(
        position_after_end_stop(33, DIR_OPEN, RUN_TO_END_STOP, 5, 89),
        100
    );
    assert_eq!(
        position_after_end_stop(33, DIR_CLOSE, RUN_TO_END_STOP, 5, 89),
        0
    );
}

#[test]
fn early_stop_run_the_second_early_partial_stop_in_a_row_requests_a_calibration() {
    let mut run = EarlyStopRun::default();
    assert_eq!(run.count(), 0);
    assert!(!run.on_move(true));
    assert_eq!(run.count(), 1);
    assert!(run.on_move(true));
    assert_eq!(run.count(), 0);
    assert!(!run.on_move(true));
    assert!(!run.on_move(false));
    assert_eq!(run.count(), 0);
    assert!(!run.on_move(true));
    assert_eq!(run.count(), 1);
    run.reset();
    assert_eq!(run.count(), 0);
    assert!(!run.on_move(true));
    assert!(run.on_move(true));
}
