//! Port of test/native/test_move_classifier__mut.cpp: clamp edges of position_after_end_stop.

use super::*;

#[test]
fn position_after_end_stop_an_open_move_one_past_100_is_capped_to_100() {
    assert_eq!(
        position_after_end_stop(95, DIR_OPEN, 40 * 89, 6 * 89, 89),
        100
    );
    assert_eq!(
        position_after_end_stop(0, DIR_OPEN, 40 * 89, 101 * 89, 89),
        100
    );
}

#[test]
fn position_after_end_stop_the_counted_travel_is_capped_to_100_before_a_close() {
    assert_eq!(
        position_after_end_stop(150, DIR_CLOSE, 40 * 89, 101 * 89, 89),
        50
    );
    assert_eq!(
        position_after_end_stop(150, DIR_CLOSE, 40 * 89, 100 * 89, 89),
        50
    );
    assert_eq!(
        position_after_end_stop(150, DIR_CLOSE, 40 * 89, 99 * 89, 89),
        51
    );
}
