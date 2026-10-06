//! Port of test/native/test_move_classifier.cpp.

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

fn to_end(pct: u8, travel: u32) -> MoveRequest {
    request(DIR_CLOSE, RUN_TO_END_STOP, pct, travel)
}

fn partial(counts: u16) -> MoveRequest {
    request(DIR_OPEN, counts, 0, 4000)
}

#[test]
fn classify_move_stop_reasons_map_to_their_wire_values() {
    assert_eq!(StopReason::None as i32, 0);
    assert_eq!(StopReason::Target as i32, 1);
    assert_eq!(StopReason::EndStop as i32, 2);
    assert_eq!(StopReason::EarlyEndStop as i32, 3);
    assert_eq!(StopReason::Timeout as i32, 4);
    assert_eq!(StopReason::Undercurrent as i32, 5);
    assert_eq!(StopReason::SafetyOvercurrent as i32, 6);
    assert_eq!(StopReason::Aborted as i32, 7);

    let r = partial(1200);
    assert_eq!(
        classify_move(&r, MotorStop::CountReached, 1201).reason,
        StopReason::Target
    );
    assert_eq!(
        classify_move(&r, MotorStop::EndStop, 300).reason,
        StopReason::EndStop
    );
    assert_eq!(
        classify_move(&r, MotorStop::SafetyOvercurrent, 30).reason,
        StopReason::SafetyOvercurrent
    );
    assert_eq!(
        classify_move(&r, MotorStop::Undercurrent, 0).reason,
        StopReason::Undercurrent
    );
    assert_eq!(
        classify_move(&r, MotorStop::Timeout, 9).reason,
        StopReason::Timeout
    );
    assert_eq!(
        classify_move(&r, MotorStop::Aborted, 0).reason,
        StopReason::Aborted
    );
    assert_eq!(
        classify_move(&r, MotorStop::None, 0).reason,
        StopReason::Aborted
    );
    // C++ also checks static_cast<MotorStop>(200) -> Aborted (its default branch): a Rust
    // enum cannot hold a value outside its variants
}

#[test]
fn classify_move_early_end_stop_only_for_moves_to_an_end_stop_s05() {
    // partial moves never count as early
    let c = classify_move(&partial(3000), MotorStop::EndStop, 10);
    assert!(!c.early);
    assert_eq!(c.reason, StopReason::EndStop);

    // full travel: less than half of the learned stroke
    assert!(classify_move(&to_end(100, 4000), MotorStop::EndStop, 1999).early);
    assert_eq!(
        classify_move(&to_end(100, 4000), MotorStop::EndStop, 1999).reason,
        StopReason::EarlyEndStop
    );
    assert!(!classify_move(&to_end(100, 4000), MotorStop::EndStop, 2000).early);
    assert_eq!(
        classify_move(&to_end(100, 4000), MotorStop::EndStop, 2000).reason,
        StopReason::EndStop
    );
    assert!(!classify_move(&to_end(100, 4001), MotorStop::EndStop, 2001).early);
    assert!(classify_move(&to_end(100, 4001), MotorStop::EndStop, 2000).early);
}

#[test]
fn classify_move_expected_travel_scales_with_the_distance_to_the_end() {
    // from 60 % to the closed end: 60 % of 4000 = 2400 expected, early below 1200
    assert!(classify_move(&to_end(60, 4000), MotorStop::EndStop, 1199).early);
    assert!(!classify_move(&to_end(60, 4000), MotorStop::EndStop, 1200).early);
    // the shortest checked move: 50 % of 4000 = 2000 expected, early below 1000
    assert!(
        classify_move(
            &to_end(EARLY_CHECK_MIN_TRAVEL_PCT, 4000),
            MotorStop::EndStop,
            999
        )
        .early
    );
    assert!(
        !classify_move(
            &to_end(EARLY_CHECK_MIN_TRAVEL_PCT, 4000),
            MotorStop::EndStop,
            1000
        )
        .early
    );
    // more than 100 % is treated as 100 %
    assert!(classify_move(&to_end(250, 4000), MotorStop::EndStop, 1999).early);
    assert!(!classify_move(&to_end(250, 4000), MotorStop::EndStop, 2000).early);
    // 0 % or an unknown stroke disables the check
    assert!(!classify_move(&to_end(0, 4000), MotorStop::EndStop, 0).early);
    assert!(!classify_move(&to_end(100, 0), MotorStop::EndStop, 0).early);
}

#[test]
fn classify_move_short_moves_to_an_end_stop_are_not_checked() {
    // review finding: believed 3 % of a 3600 pulse stroke, the valve really is at
    // about 1.4 % and reaches the end stop after 50 pulses; this is not early
    assert_eq!(EARLY_CHECK_MIN_TRAVEL_PCT, 50);
    let c = classify_move(&to_end(3, 3600), MotorStop::EndStop, 50);
    assert!(!c.early);
    assert_eq!(c.reason, StopReason::EndStop);
    assert!(!classify_move(&to_end(3, 3600), MotorStop::SafetyOvercurrent, 0).early);
    // just below the limit: not checked even for a stop right after the start
    assert!(
        !classify_move(
            &to_end(EARLY_CHECK_MIN_TRAVEL_PCT - 1, 4000),
            MotorStop::EndStop,
            0
        )
        .early
    );
    assert!(
        classify_move(
            &to_end(EARLY_CHECK_MIN_TRAVEL_PCT, 4000),
            MotorStop::EndStop,
            0
        )
        .early
    );
}

#[test]
fn classify_move_early_safety_stop_keeps_its_reason() {
    let c = classify_move(&to_end(100, 4000), MotorStop::SafetyOvercurrent, 100);
    assert!(c.early);
    assert_eq!(c.reason, StopReason::SafetyOvercurrent);
    assert!(!classify_move(&to_end(100, 4000), MotorStop::SafetyOvercurrent, 3000).early);
    // other causes are never early
    assert!(!classify_move(&to_end(100, 4000), MotorStop::Undercurrent, 0).early);
    assert!(!classify_move(&to_end(100, 4000), MotorStop::Timeout, 0).early);
    assert!(!classify_move(&to_end(100, 4000), MotorStop::CountReached, 0).early);
    assert!(!classify_move(&to_end(100, 4000), MotorStop::Aborted, 0).early);
}

#[test]
fn classify_move_large_values_do_not_overflow() {
    assert!(classify_move(&to_end(100, 0xFFFF_FFFF), MotorStop::EndStop, 0x7FFF_FFFF).early);
    assert!(!classify_move(&to_end(100, 0xFFFF_FFFF), MotorStop::EndStop, 0x8000_0000).early);
}

#[test]
fn make_move_result_fields_and_saturation() {
    let r = request(DIR_CLOSE, 1234, 0, 4000);
    let m: MoveResult = make_move_result(&r, StopReason::Target, 1235, 187, 5230);
    assert_eq!(m.dir, 1);
    assert_eq!(m.requested_counts, 1234);
    assert_eq!(m.counted_counts, 1235);
    assert_eq!(m.stop_reason, 1);
    assert_eq!(m.peak_current, 187);
    assert_eq!(m.duration_ms, 5230);

    let o = request(7, RUN_TO_END_STOP, 100, 0);
    let s: MoveResult = make_move_result(&o, StopReason::EndStop, 70000, 70000, 0xFFFF_FFFF);
    assert_eq!(s.dir, 0);
    assert_eq!(s.requested_counts, 65535);
    assert_eq!(s.counted_counts, 65535);
    assert_eq!(s.peak_current, 65535);
    assert_eq!(s.duration_ms, 0xFFFF_FFFF);

    assert_eq!(
        make_move_result(&o, StopReason::Aborted, 0, -5, 0).peak_current,
        0
    );
}

#[test]
fn classify_move_the_early_check_only_applies_to_a_run_to_the_end_stop() {
    // a counted move with a learned travel is never early, whatever it counted
    let r = request(DIR_CLOSE, 500, 100, 4000);
    let c = classify_move(&r, MotorStop::EndStop, 0);
    assert!(!c.early);
    assert_eq!(c.reason, StopReason::EndStop);
}

#[test]
fn classify_move_a_learned_travel_of_1_count_is_a_learned_travel() {
    // expected 1 * 100 / 100 = 1 count, 0 counted -> 0 < 1: early
    let c = classify_move(&to_end(100, 1), MotorStop::EndStop, 0);
    assert!(c.early);
    assert_eq!(c.reason, StopReason::EarlyEndStop);
    // no learned travel: never early
    assert!(!classify_move(&to_end(100, 0), MotorStop::EndStop, 0).early);
}

#[test]
fn classify_move_the_expected_travel_is_capped_at_exactly_100_pct() {
    // pct 200 counts as 100: expected 1000, early below 500 counted
    // pct 101 is above 100 as well: expected 1000, not 1010
    assert!(classify_move(&to_end(101, 1000), MotorStop::EndStop, 499).early);
    assert!(!classify_move(&to_end(101, 1000), MotorStop::EndStop, 500).early);
    assert!(classify_move(&to_end(200, 1000), MotorStop::EndStop, 499).early);
    assert!(!classify_move(&to_end(200, 1000), MotorStop::EndStop, 500).early);
    assert!(classify_move(&to_end(100, 1000), MotorStop::EndStop, 499).early);
    assert!(!classify_move(&to_end(100, 1000), MotorStop::EndStop, 500).early);
    assert!(classify_move(&to_end(99, 1000), MotorStop::EndStop, 494).early);
    assert!(!classify_move(&to_end(99, 1000), MotorStop::EndStop, 495).early);
}

#[test]
fn make_move_result_a_peak_of_1_is_kept_only_0_or_less_reads_0() {
    assert_eq!(
        make_move_result(&partial(10), StopReason::Target, 10, 1, 0).peak_current,
        1
    );
    assert_eq!(
        make_move_result(&partial(10), StopReason::Target, 10, 0, 0).peak_current,
        0
    );
    assert_eq!(
        make_move_result(&partial(10), StopReason::Target, 10, -1, 0).peak_current,
        0
    );
}
