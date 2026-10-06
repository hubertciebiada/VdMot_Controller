//! Port of test/native/test_replies.cpp.

use super::*;
use crate::buf_writer::StaticBufWriter;
use crate::test_support::text;
use std::format;
use std::string::String;

/// The v1 implementation built the reply with itoa(int) + strcat.
fn legacy_valve_data(r: &ValveDataReply) -> String {
    let mut s = String::from("gvlvd ");
    let fields: [i64; 11] = [
        i64::from(r.index),
        i64::from(r.actual_position),
        i64::from(r.mean_current),
        i64::from(r.status),
        i64::from(r.temperature1),
        i64::from(r.temperature2),
        i64::from(r.movements),
        i64::from(r.opening_count),
        i64::from(r.closing_count),
        i64::from(r.deadzone_count),
        i64::from(r.calib_retries),
    ];
    for f in fields {
        s += &format!("{f} ");
    }
    s
}

fn typical() -> ValveDataReply {
    ValveDataReply {
        index: 3,
        actual_position: 42,
        mean_current: 17,
        status: 0x01 | 0x80,
        temperature1: 215,
        temperature2: -500,
        movements: 12,
        opening_count: 3567,
        closing_count: 3610,
        deadzone_count: 43,
        calib_retries: 0,
    }
}

#[test]
fn format_valve_data_v1_byte_format() {
    let mut w = StaticBufWriter::<{ VALVE_DATA_REPLY_MAX_LEN + 1 }>::default();
    let r = typical();
    assert!(format_valve_data(&mut w, b"gvlvd", &r));
    assert_eq!(
        text(w.as_bytes()),
        "gvlvd 3 42 17 129 215 -500 12 3567 3610 43 0 "
    );
    assert_eq!(text(w.as_bytes()), legacy_valve_data(&r));
}

#[test]
fn format_valve_data_negative_deadzone_and_sentinels_print_signed() {
    let mut w = StaticBufWriter::<{ VALVE_DATA_REPLY_MAX_LEN + 1 }>::default();
    let mut r = typical();
    r.deadzone_count = -150;
    r.temperature1 = -1270;
    r.temperature2 = 850;
    assert!(format_valve_data(&mut w, b"gvlvd", &r));
    assert_eq!(text(w.as_bytes()), legacy_valve_data(&r));
    assert!(text(w.as_bytes()).contains(" -150 "));
}

#[test]
fn format_valve_data_worst_case_fits_valve_data_reply_max_len() {
    let r = ValveDataReply {
        index: u32::MAX,
        actual_position: i32::MIN,
        mean_current: i32::MIN,
        status: i32::MIN,
        temperature1: i32::MIN,
        temperature2: i32::MIN,
        movements: i32::MIN,
        opening_count: i32::MIN,
        closing_count: i32::MIN,
        deadzone_count: i32::MIN,
        calib_retries: i32::MIN,
    };
    let mut w = StaticBufWriter::<{ VALVE_DATA_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_valve_data(&mut w, b"gvlvd", &r));
    assert!(w.length() <= VALVE_DATA_REPLY_MAX_LEN);
    assert_eq!(text(w.as_bytes()), legacy_valve_data(&r));

    // The old 60-byte buffer could not hold this reply (the v1 overflow).
    assert!(w.length() + 1 > 60);
}

#[test]
fn valve_data_reply_max_len_is_the_contract_value() {
    // 5-char prefix, 11 numbers of up to 11 characters, 12 spaces: the glue sizes its buffer
    // with it (the worst case above is 137 characters, the index has at most 10 digits)
    assert_eq!(VALVE_DATA_REPLY_MAX_LEN, 138);
}

#[test]
fn format_valve_data_too_small_buffer_writes_nothing() {
    let r = typical();
    let needed = legacy_valve_data(&r).len();
    for cap in 0..=needed {
        let mut buf = [b'q'; 128];
        let mut w = BufWriter::new(&mut buf[..cap]);
        assert!(!format_valve_data(&mut w, b"gvlvd", &r), "cap {cap}");
        assert_eq!(w.length(), 0, "cap {cap}");
        assert!(!w.ok(), "cap {cap}");
    }
    let mut buf = [0u8; 128];
    let mut exact = BufWriter::new(&mut buf[..needed + 1]);
    assert!(format_valve_data(&mut exact, b"gvlvd", &r));
    assert!(exact.ok());
}

#[test]
fn format_valve_data_appends_after_existing_content_and_rolls_back_on_failure() {
    let mut w = StaticBufWriter::<20>::default();
    assert!(w.append(b"keep"));
    assert!(!format_valve_data(&mut w, b"gvlvd", &typical()));
    assert_eq!(text(w.as_bytes()), "keep");
    // C++ formatValveData(w, nullptr, ...): a Rust slice is never null; a prefix that does not
    // fit is the nearest case
    assert!(!format_valve_data(
        &mut w,
        b"0123456789abcdefgh",
        &typical()
    ));
    assert_eq!(text(w.as_bytes()), "keep");
}

#[test]
fn format_status_list_v1_byte_format_without_trailing_comma() {
    let status: [u8; 12] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 1, 255, 0];
    let mut w = StaticBufWriter::<80>::default();
    assert!(format_status_list(&mut w, b"gvlst", &status));
    assert_eq!(text(w.as_bytes()), "gvlst 12 1,2,3,4,5,6,7,8,9,1,255,0 ");
}

#[test]
fn format_status_list_small_and_empty_lists() {
    let one: [u8; 1] = [6];
    let mut w = StaticBufWriter::<32>::default();
    assert!(format_status_list(&mut w, b"gvlst", &one));
    assert_eq!(text(w.as_bytes()), "gvlst 1 6 ");
    w.clear();
    assert!(format_status_list(&mut w, b"gvlst", &[]));
    assert_eq!(text(w.as_bytes()), "gvlst 0  ");
    // C++ formatStatusList(w, "gvlst", nullptr, 3) fails: the Rust list is a slice, its length
    // is n, so a null list with n > 0 cannot be expressed
}

#[test]
fn format_status_list_too_small_buffer_writes_nothing() {
    let status = [1u8; 12];
    let expected = "gvlst 12 1,1,1,1,1,1,1,1,1,1,1,1 ";
    for cap in 0..=expected.len() {
        let mut buf = [0u8; 64];
        let mut w = BufWriter::new(&mut buf[..cap]);
        assert!(!format_status_list(&mut w, b"gvlst", &status), "cap {cap}");
        assert_eq!(w.length(), 0, "cap {cap}");
    }
    let mut buf = [0u8; 64];
    let mut w = BufWriter::new(&mut buf[..expected.len() + 1]);
    assert!(format_status_list(&mut w, b"gvlst", &status));
    assert_eq!(text(w.as_bytes()), expected);
}
