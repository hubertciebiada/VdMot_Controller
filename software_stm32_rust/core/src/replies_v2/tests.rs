//! Port of test/native/test_replies_v2.cpp.

use super::*;
use crate::buf_writer::StaticBufWriter;
use crate::test_support::text;

fn typical() -> ValveExtReply {
    ValveExtReply {
        index: 3,
        status: 9,
        position: 0,
        target: 30,
        mean_current: 17,
        opening_count: 3567,
        closing_count: 3610,
        deadzone_count: 43,
        calib_retries: 2,
        movements: 12,
        cal_state: CAL_FLAG_LAST_FAILED,
        early_stops: 1,
        cmd_rejected: 4,
        last: MoveResult {
            dir: 1,
            requested_counts: 65535,
            counted_counts: 102,
            stop_reason: 3,
            peak_current: 356,
            duration_ms: 2140,
        },
    }
}

#[test]
fn gvlvx_field_order_of_the_protocol_table() {
    let mut w = StaticBufWriter::<{ VALVE_EXT_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_valve_ext(&mut w, &typical()));
    assert_eq!(
        text(w.as_bytes()),
        "gvlvx 3 9 0 30 17 3567 3610 43 2 12 8 1 4 1 65535 102 3 356 2140"
    );
}

#[test]
fn gvlvx_worst_case_fits_the_declared_maximum() {
    let r = ValveExtReply {
        index: 255,
        status: 255,
        position: 255,
        target: 255,
        mean_current: 65535,
        opening_count: 0xFFFF_FFFF,
        closing_count: 0xFFFF_FFFF,
        deadzone_count: i32::MIN,
        calib_retries: 255,
        movements: 0xFFFF_FFFF,
        cal_state: 255,
        early_stops: 65535,
        cmd_rejected: 65535,
        last: MoveResult {
            dir: 255,
            requested_counts: 65535,
            counted_counts: 65535,
            stop_reason: 255,
            peak_current: 65535,
            duration_ms: 0xFFFF_FFFF,
        },
    };
    let mut w = StaticBufWriter::<{ VALVE_EXT_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_valve_ext(&mut w, &r));
    assert!(w.length() <= VALVE_EXT_REPLY_MAX_LEN);
    assert!(text(w.as_bytes()).contains("-2147483648"));
}

#[test]
fn formatters_write_nothing_when_the_buffer_is_too_small() {
    let mut w = StaticBufWriter::<40>::default();
    w.append(b"x");
    assert!(!format_valve_ext(&mut w, &typical()));
    assert_eq!(text(w.as_bytes()), "x");

    let mut p = ProfileRecorder::default();
    p.reset();
    for c in 0..32u32 {
        p.add(c * 1000, 600);
    }
    assert!(!format_profile(&mut w, 1, &p));
    assert_eq!(text(w.as_bytes()), "x");

    let mut tiny = StaticBufWriter::<8>::default();
    assert!(!format_stat(
        &mut tiny,
        &StatReply {
            uptime_seconds: 1,
            resets: 2,
            boot_reason: 3,
            rx_overflow: 4,
            parse_errors: 5,
            eeprom_state: 6,
        }
    ));
    assert!(!format_escalation(
        &mut tiny,
        &EscalationConfig {
            enable: 1,
            step_pct: 25,
            max_ma: 50,
        }
    ));
    assert!(!format_motor_limits(&mut tiny));
    assert!(!format_indexed_result(&mut tiny, b"svmov", 11, 2));
    assert!(tiny.as_bytes().is_empty());
}

#[test]
fn gprof_pairs_of_count_and_current() {
    let mut p = ProfileRecorder::default();
    p.reset();
    p.add(0, 0);
    p.add(120, -187);
    p.finish(4012, 402);
    let mut w = StaticBufWriter::<{ PROFILE_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_profile(&mut w, 7, &p));
    assert_eq!(text(w.as_bytes()), "gprof 7 3 0:0 120:187 4012:402");

    let mut empty = ProfileRecorder::default();
    empty.reset();
    w.clear();
    assert!(format_profile(&mut w, 0, &empty));
    assert_eq!(text(w.as_bytes()), "gprof 0 0");
}

#[test]
fn gprof_32_samples_of_maximum_width_fit() {
    let mut p = ProfileRecorder::default();
    p.reset();
    for c in 0..32u32 {
        p.add(65504 + c, 65535);
    }
    assert_eq!(p.size(), 32);
    let mut w = StaticBufWriter::<{ PROFILE_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_profile(&mut w, 11, &p));
    assert!(w.length() <= PROFILE_REPLY_MAX_LEN);
}

#[test]
fn gstat_gcalx_gmotx_gproto() {
    let mut w = StaticBufWriter::<{ STAT_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_stat(
        &mut w,
        &StatReply {
            uptime_seconds: 86400,
            resets: 3,
            boot_reason: 4,
            rx_overflow: 17,
            parse_errors: 2,
            eeprom_state: 1,
        }
    ));
    assert_eq!(text(w.as_bytes()), "gstat 86400 3 4 17 2 1");

    let mut c = StaticBufWriter::<32>::default();
    assert!(format_escalation(
        &mut c,
        &EscalationConfig {
            enable: 1,
            step_pct: 25,
            max_ma: 50,
        }
    ));
    assert_eq!(text(c.as_bytes()), "gcalx 1 25 50");

    let mut m = StaticBufWriter::<{ MOTOR_LIMITS_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_motor_limits(&mut m));
    assert_eq!(text(m.as_bytes()), "gmotx 10 40 10 40 0 100 0 60000 0 2");

    let mut g = StaticBufWriter::<16>::default();
    assert!(format_protocol_version(&mut g));
    assert_eq!(text(g.as_bytes()), "gproto 3");
}

#[test]
fn ok_err_replies() {
    let mut w = StaticBufWriter::<32>::default();
    assert!(format_result(&mut w, b"scalx", true));
    assert_eq!(text(w.as_bytes()), "scalx ok");
    w.clear();
    assert!(format_result(&mut w, b"smotc", false));
    assert_eq!(text(w.as_bytes()), "smotc err");
    w.clear();
    assert!(format_indexed_result(&mut w, b"svmov", 4, 0));
    assert_eq!(text(w.as_bytes()), "svmov 4 ok");
    w.clear();
    assert!(format_indexed_result(&mut w, b"svmov", 4, 2));
    assert_eq!(text(w.as_bytes()), "svmov 4 err 2");
    w.clear();
    assert!(format_indexed_result(&mut w, b"svmov", -1, 1));
    assert_eq!(text(w.as_bytes()), "svmov -1 err 1");
    w.clear();
    // C++ formatResult(w, nullptr, true): a Rust slice is never null; the nearest case is a
    // command that does not fit, which fails the same way before anything is written
    assert!(!format_result(
        &mut w,
        b"0123456789abcdef0123456789abcdef",
        true
    ));
    assert!(w.as_bytes().is_empty());
}

#[test]
fn compose_cal_state_state_in_bits_0_1_flags_above() {
    assert_eq!(compose_cal_state(false, false, false, false), 0);
    assert_eq!(compose_cal_state(false, true, false, false), 1);
    assert_eq!(compose_cal_state(true, false, false, false), 2);
    assert_eq!(compose_cal_state(true, true, false, false), 2);
    assert_eq!(compose_cal_state(false, false, true, false), 4);
    assert_eq!(compose_cal_state(false, false, false, true), 8);
    assert_eq!(compose_cal_state(true, true, true, true), 14);
    assert_eq!(
        compose_cal_state(false, true, true, true) & CAL_STATE_MASK,
        CAL_STATE_REQUESTED
    );
}

#[test]
fn encode_valve_status_gvlvd_encoding_bit_7_while_calibrating() {
    assert_eq!(encode_valve_status(1, false), 1);
    assert_eq!(encode_valve_status(8, true), 0x88);
    assert_eq!(encode_valve_status(9, true), 0x89);
    assert_eq!(encode_valve_status(0, true), 0x80);
    // same result as the gvlvd handler, which ORs the bit into the raw status
    for s in 0..=0xFFu8 {
        assert_eq!(encode_valve_status(s, false), s, "s {s}");
        assert_eq!(encode_valve_status(s, true), s | 0x80, "s {s}");
    }
}

#[test]
fn eepst_saved_eepst_reports_1_only_for_a_stored_configuration() {
    assert_eq!(eepst_saved(EEP_STATE_OK), 1);
    assert_eq!(eepst_saved(EEP_STATE_PENDING), 0);
    assert_eq!(eepst_saved(EEP_STATE_WRITE_FAILED), 0);
    assert_eq!(eepst_saved(EEP_STATE_READ_FAILED), 0);
    assert_eq!(eepst_saved(255), 0);
}

#[test]
fn reply_maximum_lengths_are_the_contract_values() {
    // not in the C++ suite: the derived constants of the header, pinned so that a changed
    // operator in their definition does not pass unnoticed (mutation gate)
    assert_eq!(VALVE_EXT_REPLY_MAX_LEN, 245);
    assert_eq!(PROFILE_REPLY_MAX_LEN, 396);
    assert_eq!(STAT_REPLY_MAX_LEN, 77);
    assert_eq!(MOTOR_LIMITS_REPLY_MAX_LEN, 65);
}
