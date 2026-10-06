//! Port of test/native/test_temp_filter.cpp.

use super::*;

/// C++ constexpr raw(double degC): 1/128 degC, truncated like static_cast<int16_t>
fn raw(deg_c: f64) -> i16 {
    (deg_c * 128.0) as i16
}

/// C lround(): the nearest integer, halves away from zero
fn lround(x: f64) -> i64 {
    x.round() as i64
}

#[test]
fn raw_to_tenths_equals_lround_raw_128_0_10_for_every_12_bit_value_from_55_to_125_deg_c() {
    for r in (-55 * 128..=125 * 128).step_by(8) {
        assert_eq!(
            i64::from(raw_to_tenths(r as i16)),
            lround(f64::from(r) / 128.0 * 10.0),
            "r {r}"
        );
    }
}

#[test]
fn raw_to_tenths_equals_lround_raw_128_0_10_for_every_raw_value() {
    for r in i32::from(i16::MIN)..=i32::from(i16::MAX) {
        assert_eq!(
            i64::from(raw_to_tenths(r as i16)),
            lround(f64::from(r) / 128.0 * 10.0),
            "rawToTenths differs, r {r}"
        );
    }
}

#[test]
fn raw_to_tenths_halves_round_away_from_zero_the_rest_to_the_nearest_tenth() {
    assert_eq!(raw_to_tenths(32), 3); // 0.25 degC = 2.5 tenths
    assert_eq!(raw_to_tenths(-32), -3);
    assert_eq!(raw_to_tenths(6), 0); // 0.46875 tenths
    assert_eq!(raw_to_tenths(7), 1); // 0.546875 tenths
    assert_eq!(raw_to_tenths(-6), 0);
    assert_eq!(raw_to_tenths(-7), -1);
    assert_eq!(raw_to_tenths(0), 0);
    assert_eq!(raw_to_tenths(i16::MAX), 2560);
    assert_eq!(raw_to_tenths(i16::MIN), -2560);
}

#[test]
fn filter_temperature_a_good_read_is_reported_and_becomes_the_last_value() {
    let mut t = TempTrack::default();
    assert_eq!(filter_temperature(&mut t, raw(21.5)), 215);
    assert_eq!(t.last, 215);
    assert!(t.have_last);
    assert_eq!(t.missed, 0);
    assert_eq!(t.errors, 0);
    assert_eq!(filter_temperature(&mut t, raw(-10.25)), -103);
    assert_eq!(t.last, -103);
}

#[test]
fn filter_temperature_a_failed_read_without_history_is_1270() {
    let mut t = TempTrack::default();
    assert_eq!(filter_temperature(&mut t, TEMP_RAW_DISCONNECTED), -1270);
    assert_eq!(filter_temperature(&mut t, TEMP_RAW_DISCONNECTED - 1), -1270);
    assert_eq!(t.errors, 2);
    assert!(!t.have_last);
    assert_eq!(t.missed, 0);
    // the lowest real reading
    assert_eq!(filter_temperature(&mut t, TEMP_RAW_DISCONNECTED + 1), -550);
}

#[test]
fn filter_temperature_after_a_good_value_a_failure_is_held_twice_then_1270() {
    let mut t = TempTrack::default();
    filter_temperature(&mut t, raw(20.0));
    assert_eq!(filter_temperature(&mut t, TEMP_RAW_DISCONNECTED), 200);
    assert_eq!(t.missed, 1);
    assert_eq!(filter_temperature(&mut t, TEMP_RAW_DISCONNECTED), 200);
    assert_eq!(t.missed, 2);
    assert_eq!(filter_temperature(&mut t, TEMP_RAW_DISCONNECTED), -1270);
    assert_eq!(t.missed, 2);
    assert_eq!(filter_temperature(&mut t, TEMP_RAW_DISCONNECTED), -1270);
    assert_eq!(t.errors, 4);
}

#[test]
fn filter_temperature_a_good_value_ends_the_hold_and_resets_the_missed_reads() {
    let mut t = TempTrack::default();
    filter_temperature(&mut t, raw(20.0));
    filter_temperature(&mut t, TEMP_RAW_DISCONNECTED);
    filter_temperature(&mut t, TEMP_RAW_DISCONNECTED);
    assert_eq!(filter_temperature(&mut t, raw(22.0)), 220);
    assert_eq!(t.missed, 0);
    assert_eq!(filter_temperature(&mut t, TEMP_RAW_DISCONNECTED), 220);
    assert_eq!(filter_temperature(&mut t, TEMP_RAW_DISCONNECTED), 220);
    assert_eq!(t.errors, 4);
}

#[test]
fn filter_temperature_85_0_deg_c_only_after_a_reading_of_at_least_75_0_deg_c() {
    let mut none = TempTrack::default();
    assert_eq!(filter_temperature(&mut none, TEMP_RAW_85C), -1270);
    assert_eq!(none.errors, 1);

    let mut low = TempTrack::default();
    filter_temperature(&mut low, raw(74.9375));
    assert_eq!(filter_temperature(&mut low, TEMP_RAW_85C), 749); // held
    assert_eq!(low.errors, 1);

    let mut at = TempTrack::default();
    filter_temperature(&mut at, raw(75.0));
    assert_eq!(filter_temperature(&mut at, TEMP_RAW_85C), 850);

    let mut high = TempTrack::default();
    filter_temperature(&mut high, raw(76.0));
    assert_eq!(filter_temperature(&mut high, TEMP_RAW_85C), 850);
    assert_eq!(high.errors, 0);
    // 85.0078: an ordinary value
    assert_eq!(filter_temperature(&mut high, TEMP_RAW_85C + 1), 850);
}

#[test]
fn filter_temperature_values_next_to_85_0_deg_c_are_ordinary_readings() {
    let mut t = TempTrack::default();
    assert_eq!(filter_temperature(&mut t, TEMP_RAW_85C - 1), 850);
    let mut u = TempTrack::default();
    assert_eq!(filter_temperature(&mut u, TEMP_RAW_85C + 16), 851);
}

#[test]
fn filter_temperature_the_error_count_saturates() {
    let mut t = TempTrack {
        errors: u16::MAX - 1,
        ..TempTrack::default()
    };
    filter_temperature(&mut t, TEMP_RAW_DISCONNECTED);
    assert_eq!(t.errors, u16::MAX);
    filter_temperature(&mut t, TEMP_RAW_DISCONNECTED);
    assert_eq!(t.errors, u16::MAX);
}

#[test]
fn the_constants() {
    assert_eq!(TEMP_RAW_DISCONNECTED, -7040);
    assert_eq!(TEMP_RAW_85C, 85 * 128);
    assert_eq!(TEMP_85_MIN_PREVIOUS, 750);
    assert_eq!(TEMP_HOLD_CYCLES, 2);
    assert_eq!(TEMP_FAILED_TENTHS, -1270);
}
