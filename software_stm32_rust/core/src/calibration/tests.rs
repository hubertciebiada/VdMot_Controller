//! Port of test/native/test_calibration.cpp.

use super::*;
use crate::move_classifier::{DIR_CLOSE, DIR_OPEN};

/// the C++ aggregate initializer EscalationConfig{enable, stepPct, maxmA}
fn esc(enable: u8, step_pct: u8, max_ma: u8) -> EscalationConfig {
    EscalationConfig {
        enable,
        step_pct,
        max_ma,
    }
}

/// the C++ default argument of endStopBound
const FLOOR: u16 = MEAN_CURRENT_FLOOR_MA;

#[test]
fn end_stop_bound_learned_mean_times_factor_15_ma_floor_s01() {
    assert_eq!(end_stop_bound(20, 17, FLOOR), 340); // 1.x default: 34 mA
    assert_eq!(end_stop_bound(30, 17, FLOOR), 510); // a high learned mean raises the threshold
    assert_eq!(end_stop_bound(15, 17, FLOOR), 255);
    assert_eq!(end_stop_bound(14, 17, FLOOR), 255); // floor
    assert_eq!(end_stop_bound(0, 17, FLOOR), 255);
    assert_eq!(end_stop_bound(16, 10, FLOOR), 160);
    assert_eq!(end_stop_bound(65535, 50, FLOOR), 65535 * 50);
    assert_eq!(end_stop_bound(20, 0, FLOOR), 0);
}

#[test]
fn end_stop_bound_an_explicit_floor() {
    assert_eq!(end_stop_bound(16, 17, 20), 340);
    assert_eq!(end_stop_bound(20, 17, 20), 340);
    assert_eq!(end_stop_bound(25, 17, 20), 425);
    assert_eq!(end_stop_bound(0, 17, 0), 0);
}

#[test]
fn calibration_floor_closing_strokes_never_below_the_1_x_closing_threshold() {
    assert_eq!(CALIBRATION_CLOSE_FLOOR_MA, 20);
    assert_eq!(calibration_floor(DIR_CLOSE), 20);
    assert_eq!(calibration_floor(DIR_OPEN), MEAN_CURRENT_FLOOR_MA);

    // owner's valves: learned mean 16..17 mA, factor 1.7; 1.x closed at 34 mA
    for mean in [16u16, 17] {
        let closing = end_stop_bound(mean, 17, calibration_floor(DIR_CLOSE));
        assert_eq!(closing, 340, "mean {mean}");
        assert_eq!(
            end_stop_bound(mean, 17, calibration_floor(DIR_OPEN)),
            i32::from(mean) * 17,
            "mean {mean}"
        );
    }
    // a valve that needs more current than 20 mA keeps its learned mean
    assert_eq!(end_stop_bound(24, 17, calibration_floor(DIR_CLOSE)), 408);
}

#[test]
fn escalation_config_validation() {
    assert!(escalation_valid(&ESCALATION_DEFAULT));
    assert!(escalation_valid(&esc(0, 0, 20)));
    assert!(escalation_valid(&esc(1, 100, 60)));
    assert!(!escalation_valid(&esc(2, 25, 50)));
    assert!(!escalation_valid(&esc(1, 101, 50)));
    assert!(!escalation_valid(&esc(1, 25, 19)));
    assert!(!escalation_valid(&esc(1, 25, 61)));

    let bad = esc(1, 25, 61);
    let s = sanitize_escalation(&bad);
    assert_eq!(s.enable, ESCALATION_DEFAULT.enable);
    assert_eq!(s.step_pct, ESCALATION_DEFAULT.step_pct);
    assert_eq!(s.max_ma, ESCALATION_DEFAULT.max_ma);
    let good = esc(1, 40, 45);
    assert_eq!(sanitize_escalation(&good).step_pct, 40);
    assert_eq!(ESCALATION_DEFAULT.enable, 0);
}

#[test]
fn escalated_bound_grows_by_step_pct_per_repetition_f01() {
    let c = esc(1, 25, 60);
    assert_eq!(escalated_bound(340, 0, &c), 340);
    assert_eq!(escalated_bound(340, 1, &c), 425);
    assert_eq!(escalated_bound(340, 2, &c), 510);
    assert_eq!(escalated_bound(340, 3, &c), 595);
    assert_eq!(escalated_bound(340, 4, &c), 600); // capped by maxmA
    assert_eq!(escalated_bound(340, 255, &c), 600);
}

#[test]
fn escalated_bound_disabled_or_zero_step_leaves_the_bound() {
    assert_eq!(escalated_bound(340, 2, &esc(0, 25, 60)), 340);
    assert_eq!(escalated_bound(340, 2, &esc(1, 0, 60)), 340);
    assert_eq!(escalated_bound(0, 2, &esc(1, 25, 60)), 0);
    assert_eq!(escalated_bound(-5, 2, &esc(1, 25, 60)), -5);
}

#[test]
fn escalated_bound_cap_by_max_ma_and_by_the_60_ma_safety_limit() {
    assert_eq!(escalated_bound(340, 1, &esc(1, 100, 40)), 400);
    assert_eq!(escalated_bound(340, 1, &esc(1, 100, 20)), 340); // already above the cap
    assert_eq!(escalated_bound(700, 2, &esc(1, 100, 60)), 700); // never lowered
    assert_eq!(escalated_bound(599, 1, &esc(1, 1, 60)), 600); // 604.99 -> cap
    assert_eq!(escalated_bound(340, 1, &esc(1, 100, 200)), 600); // bad cap: safety limit
    assert_eq!(escalated_bound(600, 1, &esc(1, 50, 60)), 600);
}

#[test]
fn evaluate_calibration_accept_needs_both_strokes_at_the_minimum() {
    use CalibrationVerdict::{Accept, Blocked, Retry};
    assert_eq!(evaluate_calibration(3000, 3000, 3000, 0, 2), Accept);
    assert_eq!(evaluate_calibration(2999, 3000, 3000, 0, 2), Retry);
    assert_eq!(evaluate_calibration(3000, 2999, 3000, 0, 2), Retry);
    assert_eq!(evaluate_calibration(0, 0, 0, 0, 0), Blocked);
}

#[test]
fn evaluate_calibration_never_accepts_a_stroke_that_gives_scaler_0() {
    use CalibrationVerdict::{Accept, Retry};
    assert_eq!(evaluate_calibration(99, 5000, 0, 0, 2), Retry);
    assert_eq!(evaluate_calibration(5000, 99, 50, 0, 2), Retry);
    assert_eq!(evaluate_calibration(100, 100, 0, 0, 2), Accept);
}

#[test]
fn evaluate_calibration_retries_then_blocks_like_1_x() {
    use CalibrationVerdict::{Accept, Blocked, Retry};
    // maxRetries 2: passes 1, 2 retry, pass 3 blocks
    assert_eq!(evaluate_calibration(80, 100, 3000, 0, 2), Retry);
    assert_eq!(evaluate_calibration(80, 100, 3000, 1, 2), Retry);
    assert_eq!(evaluate_calibration(80, 100, 3000, 2, 2), Blocked);
    assert_eq!(evaluate_calibration(80, 100, 3000, 0, 0), Blocked);
    assert_eq!(evaluate_calibration(80, 100, 3000, 255, 255), Blocked);
    // a good pass after failed ones is accepted
    assert_eq!(evaluate_calibration(4000, 4100, 3000, 2, 2), Accept);
}

#[test]
fn stroke_mean_current_sum_samples_in_ma() {
    assert_eq!(stroke_mean_current(0, 0), 0);
    assert_eq!(stroke_mean_current(12345, 0), 0);
    assert_eq!(stroke_mean_current(1700, 10), 17);
    assert_eq!(stroke_mean_current(-1700, 10), 17);
    assert_eq!(stroke_mean_current(1799, 10), 17);
    assert_eq!(stroke_mean_current(i32::MIN, 1), 0xFFFF);
    assert_eq!(stroke_mean_current(i32::MAX, 65535), 3276);
}

#[test]
fn learn_mean_current_only_strokes_with_enough_samples_s02() {
    let n = MIN_MEAN_SAMPLES;
    assert_eq!(learn_mean_current(20, 16, n, 18, n), 17);
    assert_eq!(learn_mean_current(20, 16, n, 18, n - 1), 16);
    assert_eq!(learn_mean_current(20, 16, n - 1, 18, n), 18);
    assert_eq!(learn_mean_current(20, 16, n - 1, 18, n - 1), 20);
    assert_eq!(learn_mean_current(20, 0, n, 0, n), 20);
    assert_eq!(learn_mean_current(20, 0, n, 30, n), 30);
    assert_eq!(learn_mean_current(20, 65535, 65535, 65535, 65535), 65535);
    assert_eq!(learn_mean_current(20, 17, 0, 19, 0), 20);
}

#[test]
fn escalated_bound_the_smallest_positive_bound_still_grows() {
    // bound 1 is > 0: it escalates (1 * 200 / 100 = 2), only bound <= 0 is left alone
    assert_eq!(escalated_bound(1, 1, &esc(1, 100, 60)), 2);
    assert_eq!(escalated_bound(1, 3, &esc(1, 100, 60)), 4);
}

#[test]
fn learn_mean_current_1_ma_is_a_valid_stroke_mean_0_ma_is_not() {
    let n = MIN_MEAN_SAMPLES;
    assert_eq!(learn_mean_current(20, 1, n, 0, n), 1);
    assert_eq!(learn_mean_current(20, 0, n, 1, n), 1);
    assert_eq!(learn_mean_current(20, 1, n, 3, n), 2);
}
