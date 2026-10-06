//! Port of test/native/test_replies_v3.cpp.

use super::*;
use crate::buf_writer::StaticBufWriter;
use crate::lease::LeaseState;
use crate::move_classifier::MoveResult;
use crate::replies_v2::{
    format_indexed_result, format_result, format_stat, format_valve_ext, CAL_FLAG_LAST_FAILED,
    PROFILE_REPLY_MAX_LEN, STAT_REPLY_MAX_LEN, VALVE_EXT_REPLY_MAX_LEN,
};
use crate::test_support::text;
use crate::valve_codes::{
    ValveFault, ST_BLOCKED, ST_CLOSING, ST_FAILED, ST_FULL_OPEN, ST_IDLE, ST_OPENING,
    ST_OPEN_CIRCUIT, ST_PRESENT, ST_UNKNOWN, SYS_FLAG_PROTECT_SUSPENDED, VLV_FLAG_ASSEMBLY,
    VLV_FLAG_CAL_RESTORED, VLV_FLAG_EARLY_PENDING, VLV_FLAG_FS_BLOCKED, VLV_FLAG_FS_LEASE,
    VLV_FLAG_NEEDS_REF, VLV_FLAG_RECAL, VLV_FLAG_RETRY, VLV_FLAG_SVC_HOLD, VLV_FLAG_UNCALIBRATED,
};
use std::string::String;

// the replies fit the reply buffer of the dispatcher (the gprof reply is the longest)
const _: () = assert!(
    VALVE_EXT_V3_REPLY_MAX_LEN <= PROFILE_REPLY_MAX_LEN,
    "gvlvy fits the reply buffer"
);
const _: () = assert!(
    STAT_V3_REPLY_MAX_LEN <= PROFILE_REPLY_MAX_LEN,
    "gstax fits the reply buffer"
);

// codes of the protocol table
const _: () = assert!(
    ST_IDLE == 1
        && ST_OPENING == 2
        && ST_CLOSING == 3
        && ST_FAILED == 4
        && ST_UNKNOWN == 5
        && ST_OPEN_CIRCUIT == 6
        && ST_FULL_OPEN == 7
        && ST_PRESENT == 8
        && ST_BLOCKED == 9,
    "valve status codes"
);
const _: () = assert!(
    VLV_FLAG_FS_LEASE == 1
        && VLV_FLAG_FS_BLOCKED == 2
        && VLV_FLAG_UNCALIBRATED == 4
        && VLV_FLAG_NEEDS_REF == 8
        && VLV_FLAG_RECAL == 16
        && VLV_FLAG_CAL_RESTORED == 32
        && VLV_FLAG_RETRY == 64
        && VLV_FLAG_EARLY_PENDING == 128
        && VLV_FLAG_ASSEMBLY == 256
        && VLV_FLAG_SVC_HOLD == 512,
    "gvlvy flag bits"
);
const _: () = assert!(
    ValveFault::None as u8 == 0
        && ValveFault::MoveTimeout as u8 == 1
        && ValveFault::StrokeTimeout as u8 == 2
        && ValveFault::Short as u8 == 3
        && ValveFault::StrokesTooShort as u8 == 4
        && ValveFault::InrushTrip as u8 == 5,
    "gvlvy fault codes"
);
const _: () = assert!(SYS_FLAG_PROTECT_SUSPENDED == 1, "gstax sysFlags");

/// valve 3 blocked at its failsafe position 50 % (the example of the protocol table)
fn blocked_valve() -> ValveExtV3Reply {
    ValveExtV3Reply {
        base: ValveExtReply {
            index: 3,
            status: ST_BLOCKED,
            position: 50,
            target: 30,
            mean_current: 17,
            opening_count: 3567,
            closing_count: 3610,
            deadzone_count: 43,
            calib_retries: 2,
            movements: 0,
            cal_state: CAL_FLAG_LAST_FAILED,
            early_stops: 1,
            cmd_rejected: 4,
            last: MoveResult {
                dir: 0,
                requested_counts: 1750,
                counted_counts: 1750,
                stop_reason: 1,
                peak_current: 262,
                duration_ms: 6120,
            },
        },
        flags: VLV_FLAG_FS_BLOCKED | VLV_FLAG_RETRY,
        fault: ValveFault::StrokesTooShort as u8,
        failsafe_pct: 50,
        drive: 50,
        retry_s: 3540,
        retries: 0,
    }
}

fn lease_running() -> StatV3Reply {
    StatV3Reply {
        base: StatReply {
            uptime_seconds: 86400,
            resets: 3,
            boot_reason: 2,
            rx_overflow: 0,
            parse_errors: 2,
            eeprom_state: 0,
        },
        lease: LeaseState::Running as u8,
        lease_remain_s: 3540,
        lease_client: 1,
        lease_timeout_min: 60,
        eep_writes: 12,
        temp_age_s: 2,
        ow_scan_age_s: 3600,
        ..StatV3Reply::default()
    }
}

/// every value different, so any change of the field order shows
fn distinct_stat() -> StatV3Reply {
    StatV3Reply {
        base: StatReply {
            uptime_seconds: 101,
            resets: 102,
            boot_reason: 3,
            rx_overflow: 104,
            parse_errors: 105,
            eeprom_state: 1,
        },
        lease: 2,
        lease_remain_s: 108,
        lease_client: 1,
        lease_timeout_min: 1440,
        failsafe_mask: 4095,
        safe_mode: 1,
        wdg_resets: 13,
        uart_ore: 114,
        uart_fe: 115,
        uart_ne: 116,
        rx_dropped: 117,
        cfg_flags: 118,
        cfg_events: 119,
        eep_writes: 120,
        temp_age_s: 121,
        ow_scan_age_s: 122,
        sys_flags: 123,
    }
}

fn after_command(line: &[u8]) -> String {
    text(&line[5..])
}

#[test]
fn gvlvy_the_blocked_valve_of_the_protocol_table() {
    let mut w = StaticBufWriter::<{ VALVE_EXT_V3_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_valve_ext_v3(&mut w, &blocked_valve()));
    assert_eq!(
        text(w.as_bytes()),
        "gvlvy 3 9 50 30 17 3567 3610 43 2 0 8 1 4 0 1750 1750 1 262 6120 66 4 50 50 3540 0"
    );
}

#[test]
fn gvlvy_values_1_19_are_those_of_gvlvx() {
    let mut r = blocked_valve();
    r.flags = 0x0203;
    r.fault = 5;
    r.failsafe_pct = 255;
    r.drive = 30;
    r.retry_s = 86400;
    r.retries = 255;
    let mut v3 = StaticBufWriter::<{ VALVE_EXT_V3_REPLY_MAX_LEN + 1 }>::default();
    let mut v2 = StaticBufWriter::<{ VALVE_EXT_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_valve_ext_v3(&mut v3, &r));
    assert!(format_valve_ext(&mut v2, &r.base));
    assert_eq!(
        after_command(v3.as_bytes()),
        after_command(v2.as_bytes()) + " 515 5 255 30 86400 255"
    );
}

#[test]
fn gvlvy_worst_case_fits_the_declared_maximum() {
    let r = ValveExtV3Reply {
        base: ValveExtReply {
            index: 255,
            status: 255,
            position: 255,
            target: 255,
            mean_current: 65535,
            opening_count: u32::MAX,
            closing_count: u32::MAX,
            deadzone_count: i32::MIN,
            calib_retries: 255,
            movements: u32::MAX,
            cal_state: 255,
            early_stops: 65535,
            cmd_rejected: 65535,
            last: MoveResult {
                dir: 255,
                requested_counts: 65535,
                counted_counts: 65535,
                stop_reason: 255,
                peak_current: 65535,
                duration_ms: u32::MAX,
            },
        },
        flags: 65535,
        fault: 255,
        failsafe_pct: 255,
        drive: 255,
        retry_s: u32::MAX,
        retries: 255,
    };
    let mut w = StaticBufWriter::<{ VALVE_EXT_V3_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_valve_ext_v3(&mut w, &r));
    assert!(w.length() <= VALVE_EXT_V3_REPLY_MAX_LEN);
}

#[test]
fn gstax_running_lease_the_example_of_the_protocol_table() {
    let mut w = StaticBufWriter::<{ STAT_V3_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_stat_v3(&mut w, &lease_running()));
    assert_eq!(
        text(w.as_bytes()),
        "gstax 86400 3 2 0 2 0 1 3540 1 60 0 0 0 0 0 0 0 0 0 12 2 3600 0"
    );
}

#[test]
fn gstax_23_values_in_the_order_of_the_protocol_table() {
    let mut w = StaticBufWriter::<{ STAT_V3_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_stat_v3(&mut w, &distinct_stat()));
    assert_eq!(
        text(w.as_bytes()),
        "gstax 101 102 3 104 105 1 2 108 1 1440 4095 1 13 114 115 116 117 118 119 120 121 122 123"
    );
}

#[test]
fn gstax_values_1_6_are_those_of_gstat() {
    let r = distinct_stat();
    let mut v3 = StaticBufWriter::<{ STAT_V3_REPLY_MAX_LEN + 1 }>::default();
    let mut v2 = StaticBufWriter::<{ STAT_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_stat_v3(&mut v3, &r));
    assert!(format_stat(&mut v2, &r.base));
    assert!(after_command(v3.as_bytes()).starts_with(&(after_command(v2.as_bytes()) + " ")));
}

#[test]
fn gstax_worst_case_fits_the_declared_maximum() {
    let r = StatV3Reply {
        base: StatReply {
            uptime_seconds: u32::MAX,
            resets: u32::MAX,
            boot_reason: 255,
            rx_overflow: u32::MAX,
            parse_errors: u32::MAX,
            eeprom_state: 255,
        },
        lease: 255,
        lease_remain_s: u32::MAX,
        lease_client: 255,
        lease_timeout_min: 65535,
        failsafe_mask: 65535,
        safe_mode: 255,
        wdg_resets: 255,
        uart_ore: u32::MAX,
        uart_fe: u32::MAX,
        uart_ne: u32::MAX,
        rx_dropped: u32::MAX,
        cfg_flags: 255,
        cfg_events: u32::MAX,
        eep_writes: u32::MAX,
        temp_age_s: u32::MAX,
        ow_scan_age_s: u32::MAX,
        sys_flags: 255,
    };
    let mut w = StaticBufWriter::<{ STAT_V3_REPLY_MAX_LEN + 1 }>::default();
    assert!(format_stat_v3(&mut w, &r));
    assert!(w.length() <= STAT_V3_REPLY_MAX_LEN);
}

#[test]
fn slhbt_lease_state_and_remaining_seconds() {
    let mut w = StaticBufWriter::<32>::default();
    assert!(format_heartbeat(&mut w, 1, 3540));
    assert_eq!(text(w.as_bytes()), "slhbt 1 3540");
    w.clear();
    assert!(format_heartbeat(&mut w, 0, 0));
    assert_eq!(text(w.as_bytes()), "slhbt 0 0");
    w.clear();
    assert!(format_heartbeat(&mut w, 2, 0));
    assert_eq!(text(w.as_bytes()), "slhbt 2 0");
}

#[test]
fn glcfg_timeout_and_the_12_failsafe_positions() {
    let mut w = StaticBufWriter::<80>::default();
    let fs: [u8; 12] = [50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 255];
    assert!(format_lease_config(&mut w, 60, &fs));
    assert_eq!(
        text(w.as_bytes()),
        "glcfg 60 50 50 50 50 50 50 50 50 50 50 50 255"
    );
    w.clear();
    let distinct: [u8; 12] = [0, 1, 2, 3, 40, 50, 60, 70, 80, 99, 100, 255];
    assert!(format_lease_config(&mut w, 0, &distinct));
    assert_eq!(
        text(w.as_bytes()),
        "glcfg 0 0 1 2 3 40 50 60 70 80 99 100 255"
    );
    w.clear();
    assert!(format_lease_config(&mut w, 1440, &fs));
    assert!(text(w.as_bytes()).starts_with("glcfg 1440 50 "));
}

#[test]
fn gtlnt_stored_learn_time() {
    let mut w = StaticBufWriter::<32>::default();
    assert!(format_learn_time(&mut w, 604800));
    assert_eq!(text(w.as_bytes()), "gtlnt 604800");
    w.clear();
    assert!(format_learn_time(&mut w, 0));
    assert_eq!(text(w.as_bytes()), "gtlnt 0");
    w.clear();
    assert!(format_learn_time(&mut w, u32::MAX));
    assert_eq!(text(w.as_bytes()), "gtlnt 4294967295");
}

/// The reply a formatter left in `w` ("<none>" when it failed); clears `w`. (C++: a lambda over
/// the result of a formatter call that writes into the captured writer.)
fn reply<B: Storage>(
    w: &mut BufWriter<B>,
    format: impl FnOnce(&mut BufWriter<B>) -> bool,
) -> String {
    let formatted = format(w);
    let s = if formatted {
        text(w.as_bytes())
    } else {
        String::from("<none>")
    };
    w.clear();
    s
}

#[test]
fn protocol_3_ok_and_err_replies() {
    let mut w = StaticBufWriter::<32>::default();
    let w = &mut w;
    assert_eq!(reply(w, |w| format_result(w, b"slhbt", false)), "slhbt err");
    assert_eq!(reply(w, |w| format_result(w, b"slcfg", true)), "slcfg ok");
    assert_eq!(reply(w, |w| format_result(w, b"slcfg", false)), "slcfg err");
    assert_eq!(
        reply(w, |w| format_indexed_result(w, b"sfspo", 3, 0)),
        "sfspo 3 ok"
    );
    assert_eq!(
        reply(w, |w| format_indexed_result(w, b"sfspo", 255, 0)),
        "sfspo 255 ok"
    );
    assert_eq!(
        reply(w, |w| format_indexed_result(w, b"sfspo", 3, 1)),
        "sfspo 3 err 1"
    );
    assert_eq!(
        reply(w, |w| format_indexed_result(w, b"sfspo", -1, 1)),
        "sfspo -1 err 1"
    );
    assert_eq!(
        reply(w, |w| format_indexed_result(w, b"sstop", 2, 0)),
        "sstop 2 ok"
    );
    assert_eq!(
        reply(w, |w| format_indexed_result(w, b"sstop", 255, 0)),
        "sstop 255 ok"
    );
    assert_eq!(
        reply(w, |w| format_indexed_result(w, b"sstop", -1, 1)),
        "sstop -1 err 1"
    );
    assert_eq!(reply(w, |w| format_result(w, b"ssafe", true)), "ssafe ok");
    assert_eq!(reply(w, |w| format_result(w, b"ssafe", false)), "ssafe err");
}

#[test]
fn protocol_3_formatters_write_nothing_when_the_buffer_is_too_small() {
    let fs: [u8; 12] = [50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 255];

    // room for the values of gvlvx, not for the rest of gvlvy
    let mut w = StaticBufWriter::<70>::default();
    w.append(b"x");
    let mut probe = StaticBufWriter::<70>::default();
    probe.append(b"x");
    assert!(format_valve_ext(&mut probe, &blocked_valve().base));
    assert!(!format_valve_ext_v3(&mut w, &blocked_valve()));
    assert_eq!(text(w.as_bytes()), "x");
    // room for the values of gstat, not for the rest of gstax
    probe.clear();
    probe.append(b"x");
    assert!(format_stat(&mut probe, &distinct_stat().base));
    assert!(!format_stat_v3(&mut w, &distinct_stat()));
    assert_eq!(text(w.as_bytes()), "x");

    let mut tiny = StaticBufWriter::<12>::default();
    tiny.append(b"x");
    assert!(!format_heartbeat(&mut tiny, 1, 3540000));
    assert!(!format_lease_config(&mut tiny, 60, &fs));
    assert!(!format_learn_time(&mut tiny, 604800));
    assert_eq!(text(tiny.as_bytes()), "x");

    // one character short of the whole reply (C++ sizeof("...") - 1 is the length of the text)
    let mut almost =
        StaticBufWriter::<{ b"glcfg 60 50 50 50 50 50 50 50 50 50 50 50 255".len() }>::default();
    assert!(!format_lease_config(&mut almost, 60, &fs));
    assert!(almost.as_bytes().is_empty());
    let mut almost_time = StaticBufWriter::<{ b"gtlnt 604800".len() }>::default();
    assert!(!format_learn_time(&mut almost_time, 604800));
    assert!(almost_time.as_bytes().is_empty());
    let mut almost_beat = StaticBufWriter::<{ b"slhbt 1 3540".len() }>::default();
    assert!(!format_heartbeat(&mut almost_beat, 1, 3540));
    assert!(almost_beat.as_bytes().is_empty());
}

#[test]
fn reply_maximum_lengths_are_the_contract_values() {
    // not in the C++ suite: the derived constants of the header, pinned so that a changed
    // operator in their definition does not pass unnoticed (mutation gate)
    assert_eq!(VALVE_EXT_V3_REPLY_MAX_LEN, 305);
    assert_eq!(STAT_V3_REPLY_MAX_LEN, 281);
}
