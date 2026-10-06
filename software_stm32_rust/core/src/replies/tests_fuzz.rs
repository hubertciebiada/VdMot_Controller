//! Port of the formatValveData case of test/native/test_fuzz.cpp (same seed and iteration
//! count).

use super::*;
use crate::buf_writer::StaticBufWriter;
use crate::test_support::{text, Rng};
use std::format;

#[test]
fn format_valve_data_matches_the_v1_itoa_format() {
    let mut rng = Rng::new(0x7777);
    for round in 0..3000u32 {
        let mut gen = || {
            if round % 2 == 1 {
                rng.range_i32(i32::MIN, i32::MAX)
            } else {
                rng.range_i32(-2000, 70000)
            }
        };
        let r = ValveDataReply {
            index: round % 12,
            actual_position: gen(),
            mean_current: gen(),
            status: gen(),
            temperature1: gen(),
            temperature2: gen(),
            movements: gen(),
            opening_count: gen(),
            closing_count: gen(),
            deadzone_count: gen(),
            calib_retries: gen(),
        };
        let mut w = StaticBufWriter::<{ VALVE_DATA_REPLY_MAX_LEN + 1 }>::default();
        assert!(format_valve_data(&mut w, b"gvlvd", &r));
        let expected = format!(
            "gvlvd {} {} {} {} {} {} {} {} {} {} {} ",
            r.index,
            r.actual_position,
            r.mean_current,
            r.status,
            r.temperature1,
            r.temperature2,
            r.movements,
            r.opening_count,
            r.closing_count,
            r.deadzone_count,
            r.calib_retries
        );
        assert_eq!(text(w.as_bytes()), expected);
    }
}
