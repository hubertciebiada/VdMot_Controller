//! Port of test/native/test_stm_codec.cpp: command table, request builders (golden lines),
//! reply parser (every v1, v2 and v3 reply, malformed input, fuzz), reply matching, helpers.
//!
//! C++ builders write into a `RequestLine&` that the tests fill with garbage first, to see that
//! a build overwrites (or, rejected, resets) every member; a Rust builder returns a fresh line
//! or None, so there is nothing stale to check. C++ checks of the NUL after the text and of
//! out-of-range enum values (`static_cast<Cmd>(41)`, `static_cast<MoveDir>(2)`, ...) have no
//! Rust form: the text holds no NUL and an enum cannot hold such a value (`from_raw` refuses
//! the numbers instead).

use super::*;
use crate::common::{ALL_VALVES, NO_VALVE, TEMP_SLOT_COUNT, VALVE_COUNT, VOLT_SLOT_COUNT};
use crate::failsafe::{failsafe_pct_valid, LeaseState, FAILSAFE_HOLD};
use crate::line_assembler::STM_MAX_LINE_LEN;
use crate::test_support::assert_text;
use crate::test_support::stm_golden::STM_GOLDEN;
use crate::version::{compare_version, is_revamped, parse_version};
use std::format;
use std::string::{String, ToString};
use std::vec;
use std::vec::Vec;

/// valid CRC
const ID1: &str = "28-84-37-94-97-ff-03-23";
/// valid CRC
const ID2: &str = "28-aa-bb-cc-dd-ee-01-67";
/// valid CRC (DS2438 family)
const ID3: &str = "26-11-22-33-44-55-66-29";

fn id(s: &str) -> OneWireId {
    parse_one_wire_id(s.as_bytes()).expect(s)
}

fn parse(s: &str, r: &mut Reply) -> ParseStatus {
    parse_reply(s.as_bytes(), r)
}

fn text(r: &RequestLine) -> &str {
    core::str::from_utf8(&r.text).expect("ASCII")
}

/// Fills a Reply with non-default garbage so resets are observable.
fn dirty(r: &mut Reply) {
    r.cmd = Cmd::Gstat;
    r.gvlon_error = true;
    r.valve_data.valve = 7;
    r.valve_data.moves = 99;
    r.status.uptime_s = 1234;
    r.proto = 9;
    r.version.valid = true;
    r.build = 77;
    r.profile.count = 3;
}

/// The members `dirty` sets are back at their defaults (the C++ check); Rust also compares
/// the whole reply with the default one.
fn is_empty_reply(r: &Reply) -> bool {
    let cpp = r.cmd == Cmd::None
        && !r.gvlon_error
        && r.valve_data.valve == 0
        && r.valve_data.moves == 0
        && r.status.uptime_s == 0
        && r.proto == 0
        && !r.version.valid
        && r.build == 0
        && r.profile.count == 0;
    cpp && *r == Reply::default()
}

fn id_list(one: &str, n: usize, trailing_comma: bool) -> String {
    let mut s = vec![one; n].join(",");
    if trailing_comma {
        s.push(',');
    }
    s
}

// ================================================================ commands

#[test]
fn command_names_round_trip_and_are_exact() {
    assert_eq!(CMD_COUNT, 41);
    assert_eq!(cmd_name(Cmd::None), "");
    // C++ cmdName(static_cast<Cmd>(kCmdCount)) and (200) == "": no Rust form.
    assert_eq!(Cmd::from_raw(CMD_COUNT), None);
    assert_eq!(Cmd::from_raw(200), None);
    let expected = [
        "", "stgtp", "gtgtp", "gvlvd", "gvlst", "gonec", "goned", "gvlon", "gowvc", "gowvd",
        "stons", "stvls", "masns", "staop", "staln", "stdet", "stlnm", "gtlnm", "smotc", "gmotc",
        "gvers", "ghwin", "eepst", "reset", "gproto", "gvlvx", "gprof", "svmov", "scalx", "gcalx",
        "gstat", "gvlvy", "gstax", "slhbt", "slcfg", "sfspo", "glcfg", "sstop", "gtlnt", "ssafe",
        "stlnt",
    ];
    assert_eq!(expected.len(), usize::from(CMD_COUNT));
    for (i, name) in (0u8..).zip(expected) {
        let c = Cmd::from_raw(i).expect("in range");
        assert_eq!(c as u8, i);
        assert_eq!(cmd_name(c), name);
        if i > 0 {
            assert_eq!(cmd_from_name(name.as_bytes()), c);
        }
    }
    assert_eq!(cmd_from_name(b""), Cmd::None);
    // C++ cmdFromName(nullptr, 5) == None: no Rust form.
    assert_eq!(cmd_from_name(b"gvlv"), Cmd::None);
    assert_eq!(cmd_from_name(b"gvlvdx"), Cmd::None);
    assert_eq!(cmd_from_name(b"GVLVD"), Cmd::None);
    assert_eq!(cmd_from_name(b"gprot"), Cmd::None);
    assert_eq!(cmd_from_name(&b"gvlvd 1"[..5]), Cmd::Gvlvd); // exactly len bytes
    assert_eq!(cmd_from_name(b"stsnx"), Cmd::None);
}

#[test]
fn v2_and_idempotent_classification() {
    for i in 0..CMD_COUNT {
        let c = Cmd::from_raw(i).expect("in range");
        let mut proto = 1;
        if c == Cmd::None {
            proto = 0;
        }
        if i >= Cmd::Gproto as u8 {
            proto = 2;
        }
        if i >= Cmd::Gvlvy as u8 {
            proto = 3;
        }
        if c == Cmd::Stlnt {
            proto = 1;
        }
        assert_eq!(cmd_min_protocol(c), proto, "{}", cmd_name(c));
        assert_eq!(cmd_is_v2(c), proto >= 2, "{}", cmd_name(c));
    }
    assert_eq!(cmd_min_protocol(Cmd::Stgtp), 1);
    assert_eq!(cmd_min_protocol(Cmd::Reset), 1);
    assert_eq!(cmd_min_protocol(Cmd::Gproto), 2);
    assert_eq!(cmd_min_protocol(Cmd::Gstat), 2);
    assert_eq!(cmd_min_protocol(Cmd::Gvlvy), 3);
    assert_eq!(cmd_min_protocol(Cmd::Ssafe), 3);
    assert_eq!(cmd_min_protocol(Cmd::Stlnt), 1);
    assert!(!cmd_is_v2(Cmd::Stlnt));
    // C++ cmdMinProtocol / cmdIsV2 of kCmdCount and 255 (0, false): no Rust form.

    let not_idempotent = [
        Cmd::None,
        Cmd::Stons,
        Cmd::Masns,
        Cmd::Staop,
        Cmd::Staln,
        Cmd::Stdet,
        Cmd::Reset,
        Cmd::Svmov,
    ];
    for i in 0..CMD_COUNT {
        let c = Cmd::from_raw(i).expect("in range");
        let expect = !not_idempotent.contains(&c);
        assert_eq!(cmd_is_idempotent(c), expect, "{}", cmd_name(c));
    }
    // C++ cmdIsIdempotent(static_cast<Cmd>(kCmdCount)) == false: no Rust form.
}

#[test]
fn enum_numbers_round_trip() {
    // Rust: from_raw of every enum with external numbers, over all of u8.
    for v in 0..=u8::MAX {
        assert_eq!(
            Cmd::from_raw(v).map(|c| c as u8),
            (v < CMD_COUNT).then_some(v)
        );
        assert_eq!(MoveDir::from_raw(v).map(|d| d as u8), (v < 2).then_some(v));
        assert_eq!(
            ParseStatus::from_raw(v).map(|s| s as u8),
            (v < 9).then_some(v)
        );
        assert_eq!(
            ValveStatus::from_raw(v).map(|s| s as u8),
            (v < 10).then_some(v)
        );
        assert_eq!(
            StopReason::from_raw(v).map(|s| s as u8),
            (v < 8).then_some(v)
        );
        assert_eq!(
            ValveFault::from_raw(v).map(|f| f as u8),
            (v < 6).then_some(v)
        );
    }
    assert_eq!(ValveStatus::Blocked as u8, 9);
    assert_eq!(ValveStatus::Connected as u8, 8);
    assert_eq!(ValveStatus::Unknown as u8, 5);
    assert_eq!(MoveDir::Close as u8, 1);
}

// ================================================================ validators

#[test]
fn motor_chars_valid_boundaries() {
    let mut m = MotorChars::default();
    assert!(motor_chars_valid(&m));
    m.low_factor = 9;
    assert!(!motor_chars_valid(&m));
    m.low_factor = 10;
    assert!(motor_chars_valid(&m));
    m.low_factor = 40;
    assert!(motor_chars_valid(&m));
    m.low_factor = 41;
    assert!(!motor_chars_valid(&m));
    m = MotorChars::default();
    m.high_factor = 9;
    assert!(!motor_chars_valid(&m));
    m.high_factor = 10;
    assert!(motor_chars_valid(&m));
    m.high_factor = 40;
    assert!(motor_chars_valid(&m));
    m.high_factor = 41;
    assert!(!motor_chars_valid(&m));
    m = MotorChars::default();
    m.start_on_power = 0;
    assert!(motor_chars_valid(&m));
    m.start_on_power = 100;
    assert!(motor_chars_valid(&m));
    m.start_on_power = 101;
    assert!(!motor_chars_valid(&m));
    m = MotorChars::default();
    m.min_counts = 0;
    assert!(motor_chars_valid(&m));
    m.min_counts = 60000;
    assert!(motor_chars_valid(&m));
    m.min_counts = 60001;
    assert!(!motor_chars_valid(&m));
    m = MotorChars::default();
    m.max_calib_retries = 0;
    assert!(motor_chars_valid(&m));
    m.max_calib_retries = 2;
    assert!(motor_chars_valid(&m));
    m.max_calib_retries = 3;
    assert!(!motor_chars_valid(&m));
}

#[test]
fn breakaway_valid_and_learn_movements_valid_boundaries() {
    let mut b = Breakaway::default();
    assert!(breakaway_valid(&b));
    b.enable = true;
    b.step_pct = 100;
    assert!(breakaway_valid(&b));
    b.step_pct = 101;
    assert!(!breakaway_valid(&b));
    b.step_pct = 0;
    b.max_ma = 19;
    assert!(!breakaway_valid(&b));
    b.max_ma = 20;
    assert!(breakaway_valid(&b));
    b.max_ma = 60;
    assert!(breakaway_valid(&b));
    b.max_ma = 61;
    assert!(!breakaway_valid(&b));

    assert!(learn_movements_valid(0));
    assert!(!learn_movements_valid(1));
    assert!(!learn_movements_valid(49));
    assert!(learn_movements_valid(50));
    assert!(learn_movements_valid(2000));
    assert!(learn_movements_valid(65534));
    assert!(!learn_movements_valid(65535));
    assert!(!learn_movements_valid(65536));
    assert!(!learn_movements_valid(0xFFFF_FFFF));
}

#[test]
fn defaults_of_the_reply_types() {
    // Rust: the C++ member initialisers.
    let m = MotorChars::default();
    assert_eq!(
        (
            m.low_factor,
            m.high_factor,
            m.start_on_power,
            m.min_counts,
            m.max_calib_retries,
            m.field_count
        ),
        (17, 17, 30, 3000, 2, 5)
    );
    let b = Breakaway::default();
    assert_eq!((b.enable, b.step_pct, b.max_ma), (false, 0, 60));
    let r = Reply::default();
    assert_eq!(r.valve_data.temp1, TEMP_UNASSIGNED);
    assert_eq!(r.valve_data.temp2, TEMP_UNASSIGNED);
    assert_eq!(r.temp_data.value, TEMP_UNASSIGNED);
    assert_eq!(r.volt_data.vad, VAD_FAILED);
    assert_eq!(r.valve_ex.fs_pct, FAILSAFE_HOLD);
    assert_eq!(r.service_move.index, -1);
    assert_eq!(r.failsafe.index, -1);
    assert_eq!(r.stop.index, -1);
    assert_eq!(r.ack.valve, NO_VALVE);
    assert_eq!(r.motor_chars, m);
    assert_eq!(r.breakaway, b);
    assert_eq!(r.status.lease, LeaseState::Off);
    assert!(!r.one_wire_list.has_list);
    assert!(r.one_wire_list.ids.iter().all(is_zero));
}

// ================================================================ builders

/// The C++ checkLine: built, the exact text, its length and the metadata.
#[track_caller]
fn check_line(
    r: impl Into<Option<RequestLine>>,
    expected: &str,
    cmd: Cmd,
    valve: u8,
    arg: u16,
) -> RequestLine {
    let r = r.into().unwrap_or_else(|| panic!("{expected:?} rejected"));
    assert_eq!(text(&r), expected);
    assert_eq!(r.text.len(), expected.len());
    assert_eq!(r.cmd, cmd, "{expected:?}");
    assert_eq!(r.valve, valve, "{expected:?}");
    assert_eq!(r.arg, arg, "{expected:?}");
    r
}

/// The C++ checkRejected: false, and the output is the empty request (below).
#[track_caller]
fn check_rejected(r: Option<RequestLine>) {
    assert_eq!(r, None);
}

#[test]
fn request_line_default_is_the_empty_request() {
    // What C++ checkRejected expects of a rejected build: RequestLine{}.
    let r = RequestLine::default();
    assert!(r.text.is_empty());
    assert_eq!(r.cmd, Cmd::None);
    assert_eq!(r.valve, NO_VALVE);
    assert_eq!(r.arg, 0);
    assert!(!r.probe);
    assert!(is_zero(&r.expect));
    assert_eq!(REQUEST_MAX_LEN, 63);
}

#[test]
fn builders_without_arguments_golden_lines() {
    check_line(build_valve_states(), "gvlst \r\n", Cmd::Gvlst, NO_VALVE, 0);
    check_line(build_temp_count(), "gonec \r\n", Cmd::Gonec, NO_VALVE, 0);
    check_line(
        build_temp_list(),
        "gonec 255 \r\n",
        Cmd::Gonec,
        NO_VALVE,
        255,
    );
    check_line(build_volt_count(), "gowvc \r\n", Cmd::Gowvc, NO_VALVE, 0);
    check_line(
        build_volt_list(),
        "gowvc 255 \r\n",
        Cmd::Gowvc,
        NO_VALVE,
        255,
    );
    check_line(build_scan_one_wire(), "stons \r\n", Cmd::Stons, NO_VALVE, 0);
    check_line(build_match_sensors(), "masns \r\n", Cmd::Masns, NO_VALVE, 0);
    check_line(build_detect(), "stdet 255 \r\n", Cmd::Stdet, ALL_VALVES, 0);
    check_line(
        build_get_learn_movements(),
        "gtlnm \r\n",
        Cmd::Gtlnm,
        NO_VALVE,
        0,
    );
    check_line(
        build_get_motor_chars(),
        "gmotc \r\n",
        Cmd::Gmotc,
        NO_VALVE,
        0,
    );
    check_line(build_get_version(), "gvers \r\n", Cmd::Gvers, NO_VALVE, 0);
    check_line(build_get_hw_id(), "ghwin \r\n", Cmd::Ghwin, NO_VALVE, 0);
    check_line(build_eeprom_state(), "eepst \r\n", Cmd::Eepst, NO_VALVE, 0);
    check_line(build_soft_reset(), "reset \r\n", Cmd::Reset, NO_VALVE, 0);
    check_line(build_get_proto(), "gproto \r\n", Cmd::Gproto, NO_VALVE, 0);
    check_line(build_get_breakaway(), "gcalx \r\n", Cmd::Gcalx, NO_VALVE, 0);
    check_line(build_get_status(), "gstat \r\n", Cmd::Gstat, NO_VALVE, 0);
    check_line(build_get_status_v3(), "gstax \r\n", Cmd::Gstax, NO_VALVE, 0);
    check_line(
        build_get_lease_config(),
        "glcfg \r\n",
        Cmd::Glcfg,
        NO_VALVE,
        0,
    );
    check_line(
        build_get_learn_time(),
        "gtlnt \r\n",
        Cmd::Gtlnt,
        NO_VALVE,
        0,
    );
    check_line(
        build_leave_safe_mode(),
        "ssafe 0 \r\n",
        Cmd::Ssafe,
        NO_VALVE,
        0,
    );
}

#[test]
fn build_heartbeat_and_build_set_lease_timeout() {
    check_line(
        build_heartbeat(true),
        "slhbt 1 \r\n",
        Cmd::Slhbt,
        NO_VALVE,
        1,
    );
    check_line(
        build_heartbeat(false),
        "slhbt 0 \r\n",
        Cmd::Slhbt,
        NO_VALVE,
        0,
    );
    check_line(
        build_set_lease_timeout(60),
        "slcfg 60 \r\n",
        Cmd::Slcfg,
        NO_VALVE,
        60,
    );
    check_line(
        build_set_lease_timeout(0),
        "slcfg 0 \r\n",
        Cmd::Slcfg,
        NO_VALVE,
        0,
    );
    check_line(
        build_set_lease_timeout(5),
        "slcfg 5 \r\n",
        Cmd::Slcfg,
        NO_VALVE,
        5,
    );
    check_line(
        build_set_lease_timeout(1440),
        "slcfg 1440 \r\n",
        Cmd::Slcfg,
        NO_VALVE,
        1440,
    );
    for bad in [1u32, 4, 1441, 65541, 0xFFFF_FFFF] {
        check_rejected(build_set_lease_timeout(bad));
    }
}

#[test]
fn build_set_failsafe_cases() {
    check_line(
        build_set_failsafe(2, 50),
        "sfspo 2 50 \r\n",
        Cmd::Sfspo,
        2,
        50,
    );
    check_line(build_set_failsafe(0, 0), "sfspo 0 0 \r\n", Cmd::Sfspo, 0, 0);
    check_line(
        build_set_failsafe(11, 100),
        "sfspo 11 100 \r\n",
        Cmd::Sfspo,
        11,
        100,
    );
    check_line(
        build_set_failsafe(7, 255),
        "sfspo 7 255 \r\n",
        Cmd::Sfspo,
        7,
        255,
    );
    check_line(
        build_set_failsafe(ALL_VALVES, 40),
        "sfspo 255 40 \r\n",
        Cmd::Sfspo,
        ALL_VALVES,
        40,
    );
    for v in [12, 254, NO_VALVE] {
        check_rejected(build_set_failsafe(v, 50));
    }
    for p in [101, 200, 254] {
        check_rejected(build_set_failsafe(3, p));
    }
}

#[test]
fn build_set_learn_time_cases() {
    check_line(
        build_set_learn_time(0),
        "stlnt 0 \r\n",
        Cmd::Stlnt,
        NO_VALVE,
        0,
    );
    check_line(
        build_set_learn_time(604_800),
        "stlnt 604800 \r\n",
        Cmd::Stlnt,
        NO_VALVE,
        0,
    );
    check_line(
        build_set_learn_time(0xFFFF_FFFF),
        "stlnt 4294967295 \r\n",
        Cmd::Stlnt,
        NO_VALVE,
        0,
    );
}

#[test]
fn build_set_target_cases() {
    check_line(
        build_set_target(3, 50),
        "stgtp 3 50 \r\n",
        Cmd::Stgtp,
        3,
        50,
    );
    check_line(build_set_target(0, 0), "stgtp 0 0 \r\n", Cmd::Stgtp, 0, 0);
    check_line(
        build_set_target(11, 100),
        "stgtp 11 100 \r\n",
        Cmd::Stgtp,
        11,
        100,
    );
    check_rejected(build_set_target(12, 50));
    check_rejected(build_set_target(0, 101));
    check_rejected(build_set_target(255, 0));
}

type ValveBuilder = fn(u8) -> Option<RequestLine>;

#[test]
fn single_valve_builders() {
    let cases: [(ValveBuilder, &str, Cmd); 5] = [
        (build_get_target, "gtgtp", Cmd::Gtgtp),
        (build_valve_data, "gvlvd", Cmd::Gvlvd),
        (build_valve_ex, "gvlvx", Cmd::Gvlvx),
        (build_profile, "gprof", Cmd::Gprof),
        (build_valve_ex_v3, "gvlvy", Cmd::Gvlvy),
    ];
    for (f, name, cmd) in cases {
        for v in 0..VALVE_COUNT {
            let expect = format!("{name} {v} \r\n");
            check_line(f(v), &expect, cmd, v, 0);
        }
        check_rejected(f(12));
        check_rejected(f(ALL_VALVES));
        check_rejected(f(NO_VALVE));
    }
}

#[test]
fn valve_or_all_builders() {
    let cases: [(ValveBuilder, &str, Cmd); 4] = [
        (build_valve_sensors, "gvlon", Cmd::Gvlon),
        (build_assembly, "staop", Cmd::Staop),
        (build_calibrate, "staln", Cmd::Staln),
        (build_stop, "sstop", Cmd::Sstop),
    ];
    for (f, name, cmd) in cases {
        check_line(f(0), &format!("{name} 0 \r\n"), cmd, 0, 0);
        check_line(f(11), &format!("{name} 11 \r\n"), cmd, 11, 0);
        check_line(
            f(ALL_VALVES),
            &format!("{name} 255 \r\n"),
            cmd,
            ALL_VALVES,
            0,
        );
        check_rejected(f(12));
        check_rejected(f(254));
    }
}

#[test]
fn bus_index_builders() {
    check_line(build_temp_data(0), "goned 0 \r\n", Cmd::Goned, NO_VALVE, 0);
    check_line(
        build_temp_data(33),
        "goned 33 \r\n",
        Cmd::Goned,
        NO_VALVE,
        33,
    );
    check_rejected(build_temp_data(34));
    check_line(build_volt_data(0), "gowvd 0 \r\n", Cmd::Gowvd, NO_VALVE, 0);
    check_line(build_volt_data(7), "gowvd 7 \r\n", Cmd::Gowvd, NO_VALVE, 7);
    check_rejected(build_volt_data(8));
}

#[test]
fn build_set_valve_sensors_cases() {
    let zero = OneWireId::default();
    let r = check_line(
        build_set_valve_sensors(11, &id(ID1), &id(ID2)),
        "stvls 11 28-84-37-94-97-ff-03-23 28-aa-bb-cc-dd-ee-01-67 \r\n",
        Cmd::Stvls,
        11,
        0,
    );
    assert_eq!(r.text.len(), 59); // longest real request (spec 01 §2.4)
    check_line(
        build_set_valve_sensors(0, &zero, &zero),
        "stvls 0 00-00-00-00-00-00-00-00 00-00-00-00-00-00-00-00 \r\n",
        Cmd::Stvls,
        0,
        0,
    );
    check_line(
        build_set_valve_sensors(5, &zero, &id(ID3)),
        "stvls 5 00-00-00-00-00-00-00-00 26-11-22-33-44-55-66-29 \r\n",
        Cmd::Stvls,
        5,
        0,
    );
    // Upper-case input is emitted lower case.
    check_line(
        build_set_valve_sensors(1, &id("28-AA-BB-CC-DD-EE-01-67"), &zero),
        "stvls 1 28-aa-bb-cc-dd-ee-01-67 00-00-00-00-00-00-00-00 \r\n",
        Cmd::Stvls,
        1,
        0,
    );

    let mut bad = id(ID1);
    bad.b[7] ^= 1;
    check_rejected(build_set_valve_sensors(0, &bad, &zero));
    check_rejected(build_set_valve_sensors(0, &zero, &bad));
    check_rejected(build_set_valve_sensors(12, &zero, &zero));
    check_rejected(build_set_valve_sensors(ALL_VALVES, &id(ID1), &id(ID2)));
}

#[test]
fn build_set_learn_movements_cases() {
    check_line(
        build_set_learn_movements(0),
        "stlnm 0 \r\n",
        Cmd::Stlnm,
        NO_VALVE,
        0,
    );
    check_line(
        build_set_learn_movements(50),
        "stlnm 50 \r\n",
        Cmd::Stlnm,
        NO_VALVE,
        50,
    );
    check_line(
        build_set_learn_movements(65534),
        "stlnm 65534 \r\n",
        Cmd::Stlnm,
        NO_VALVE,
        65534,
    );
    check_rejected(build_set_learn_movements(49));
    check_rejected(build_set_learn_movements(65535));
    check_rejected(build_set_learn_movements(4_000_000_000));
}

#[test]
fn build_set_motor_chars_cases() {
    let mut m = MotorChars::default();
    check_line(
        build_set_motor_chars(&m),
        "smotc 17 17 30 3000 2 \r\n",
        Cmd::Smotc,
        NO_VALVE,
        17,
    );
    m.low_factor = 10;
    m.high_factor = 40;
    m.start_on_power = 100;
    m.min_counts = 60000;
    m.max_calib_retries = 0;
    m.field_count = 3; // parse-only field: always 5 args sent
    check_line(
        build_set_motor_chars(&m),
        "smotc 10 40 100 60000 0 \r\n",
        Cmd::Smotc,
        NO_VALVE,
        10,
    );
    m.low_factor = 5; // the v1 STM would take it, but loses it on reboot
    check_rejected(build_set_motor_chars(&m));
}

#[test]
fn build_service_move_cases() {
    check_line(
        build_service_move(3, MoveDir::Close, 500, 30),
        "svmov 3 1 500 30 \r\n",
        Cmd::Svmov,
        3,
        1,
    );
    check_line(
        build_service_move(0, MoveDir::Open, 1, 5),
        "svmov 0 0 1 5 \r\n",
        Cmd::Svmov,
        0,
        0,
    );
    check_line(
        build_service_move(11, MoveDir::Open, 10000, 60),
        "svmov 11 0 10000 60 \r\n",
        Cmd::Svmov,
        11,
        0,
    );
    check_rejected(build_service_move(12, MoveDir::Open, 1, 5));
    check_rejected(build_service_move(0, MoveDir::Open, 0, 5));
    check_rejected(build_service_move(0, MoveDir::Open, 10001, 5));
    check_rejected(build_service_move(0, MoveDir::Open, 1, 4));
    check_rejected(build_service_move(0, MoveDir::Open, 1, 61));
    // C++ buildServiceMove(0, static_cast<MoveDir>(2), 1, 5) rejected: no Rust form.
}

#[test]
fn build_set_breakaway_cases() {
    let mut b = Breakaway {
        enable: true,
        step_pct: 25,
        max_ma: 45,
    };
    check_line(
        build_set_breakaway(&b),
        "scalx 1 25 45 \r\n",
        Cmd::Scalx,
        NO_VALVE,
        1,
    );
    b.enable = false;
    b.step_pct = 0;
    b.max_ma = 20;
    check_line(
        build_set_breakaway(&b),
        "scalx 0 0 20 \r\n",
        Cmd::Scalx,
        NO_VALVE,
        0,
    );
    b.max_ma = 61;
    check_rejected(build_set_breakaway(&b));
}

fn some(r: Option<RequestLine>) -> RequestLine {
    r.expect("built")
}

#[test]
fn every_built_request_honours_the_wire_rules() {
    // Build one of each and check: ends with " \r\n", single spaces, <= 63.
    let m = MotorChars::default();
    let b = Breakaway::default();
    let lines = [
        some(build_set_target(11, 100)),
        some(build_get_target(11)),
        some(build_valve_data(11)),
        build_valve_states(),
        build_temp_count(),
        build_temp_list(),
        some(build_temp_data(33)),
        some(build_valve_sensors(255)),
        build_volt_count(),
        build_volt_list(),
        some(build_volt_data(7)),
        build_scan_one_wire(),
        some(build_set_valve_sensors(11, &id(ID1), &id(ID2))),
        build_match_sensors(),
        some(build_assembly(255)),
        some(build_calibrate(255)),
        build_detect(),
        some(build_set_learn_movements(65534)),
        build_get_learn_movements(),
        some(build_set_motor_chars(&m)),
        build_get_motor_chars(),
        build_get_version(),
        build_get_hw_id(),
        build_eeprom_state(),
        build_soft_reset(),
        build_get_proto(),
        some(build_valve_ex(11)),
        some(build_profile(11)),
        some(build_service_move(11, MoveDir::Close, 10000, 60)),
        some(build_set_breakaway(&b)),
        build_get_breakaway(),
        build_get_status(),
        some(build_valve_ex_v3(11)),
        build_get_status_v3(),
        build_heartbeat(true),
        some(build_set_lease_timeout(1440)),
        some(build_set_failsafe(255, 255)),
        build_get_lease_config(),
        some(build_stop(255)),
        build_get_learn_time(),
        build_leave_safe_mode(),
        build_set_learn_time(0xFFFF_FFFF),
    ];
    assert_eq!(lines.len(), 42);
    for l in &lines {
        let t = text(l);
        // Only gproto may stay unanswered (a v1 STM does not know it); no builder sets an
        // expected sensor id.
        assert_eq!(l.probe, l.cmd == Cmd::Gproto, "{t:?}");
        assert!(is_zero(&l.expect), "{t:?}");
        assert!(t.len() <= REQUEST_MAX_LEN, "{t:?}");
        assert!(t.len() >= 8, "{t:?}");
        assert!(t.ends_with(" \r\n"), "{t:?}");
        assert!(!t.contains("  "), "{t:?}");
        assert!(t.contains(' '), "{t:?}"); // never a line without a space
        assert_eq!(!t.contains('-'), l.cmd != Cmd::Stvls, "{t:?}"); // no negatives
        let name = cmd_name(l.cmd);
        assert!(t.starts_with(name), "{t:?}");
        assert_eq!(t.as_bytes()[name.len()], b' ', "{t:?}");
    }
}

// ================================================================ replies: basics

#[test]
fn parse_status_names() {
    assert_eq!(parse_status_name(ParseStatus::Ok), "ok");
    assert_eq!(parse_status_name(ParseStatus::Empty), "empty");
    assert_eq!(
        parse_status_name(ParseStatus::UnknownCommand),
        "unknown_command"
    );
    assert_eq!(parse_status_name(ParseStatus::BadArgCount), "bad_arg_count");
    assert_eq!(parse_status_name(ParseStatus::BadNumber), "bad_number");
    assert_eq!(parse_status_name(ParseStatus::OutOfRange), "out_of_range");
    assert_eq!(
        parse_status_name(ParseStatus::BadOneWireId),
        "bad_onewire_id"
    );
    assert_eq!(parse_status_name(ParseStatus::BadFormat), "bad_format");
    assert_eq!(parse_status_name(ParseStatus::TooLong), "too_long");
    // C++ parseStatusName(static_cast<ParseStatus>(99)) == "unknown": no Rust form.
    assert_eq!(ParseStatus::from_raw(99), None);
}

#[test]
fn stop_reason_names() {
    assert_eq!(stop_reason_name(StopReason::None), "none");
    assert_eq!(stop_reason_name(StopReason::Target), "target");
    assert_eq!(stop_reason_name(StopReason::EndStop), "endstop");
    assert_eq!(stop_reason_name(StopReason::EarlyEndStop), "early_endstop");
    assert_eq!(stop_reason_name(StopReason::Timeout), "timeout");
    assert_eq!(stop_reason_name(StopReason::UnderCurrent), "undercurrent");
    assert_eq!(
        stop_reason_name(StopReason::SafetyOverCurrent),
        "safety_overcurrent"
    );
    assert_eq!(stop_reason_name(StopReason::Aborted), "aborted");
    // C++ stopReasonName(static_cast<StopReason>(8)) == "unknown": no Rust form.
    assert_eq!(StopReason::from_raw(8), None);
}

#[test]
fn empty_blank_null_and_too_long_lines() {
    let mut r = Reply::default();
    dirty(&mut r);
    // C++ parseReply(nullptr, 5, r) == Empty (r reset): no Rust form; the empty line is.
    assert_eq!(parse("", &mut r), ParseStatus::Empty);
    assert!(is_empty_reply(&r));
    dirty(&mut r);
    assert_eq!(parse("", &mut r), ParseStatus::Empty);
    assert!(is_empty_reply(&r));
    assert_eq!(parse("     ", &mut r), ParseStatus::Empty);
    // Only ' ' separates tokens; a TAB is part of a token.
    assert_eq!(parse("\t", &mut r), ParseStatus::UnknownCommand);

    let at_limit = format!("stons{}", " ".repeat(STM_MAX_LINE_LEN - 5));
    assert_eq!(at_limit.len(), STM_MAX_LINE_LEN);
    assert_eq!(parse(&at_limit, &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Stons);
    dirty(&mut r);
    assert_eq!(parse(&(at_limit + " "), &mut r), ParseStatus::TooLong);
    assert!(is_empty_reply(&r));

    // 40 tokens are fine for the tokenizer (then the payload check decides).
    let mut t40 = String::from("gprof");
    for _ in 0..39 {
        t40 += " 1";
    }
    assert_eq!(parse(&t40, &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse(&(t40 + " 1"), &mut r), ParseStatus::TooLong);
}

#[test]
fn unknown_commands() {
    let mut r = Reply::default();
    let unknown = [
        "stsnx", "gvlvz", "gactp", "ESPalive", "gvlvdd 1", "gvl", "123456", "GVLVD 1", "gprot 2",
        ",", "gvlvd,1",
    ];
    for s in unknown {
        dirty(&mut r);
        assert_eq!(parse(s, &mut r), ParseStatus::UnknownCommand, "{s}");
        assert!(is_empty_reply(&r), "{s}");
    }
}

#[test]
fn whitespace_tolerance() {
    let mut r = Reply::default();
    assert_eq!(parse("   gtgtp    3     50     ", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gtgtp);
    assert_eq!(r.target.valve, 3);
    assert_eq!(r.target.target, 50);
    assert_eq!(parse("gtgtp 3\t50", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("gtgtp 3 50\t", &mut r), ParseStatus::BadNumber);
    // parse_reply reads exactly the bytes it is given.
    let buf = b"gtgtp 3 50 garbage";
    assert_eq!(parse_reply(&buf[..10], &mut r), ParseStatus::Ok);
    assert_eq!(r.target.target, 50);
}

// ================================================================ replies: acks

#[test]
fn ack_replies_golden() {
    let cases = [
        ("stgtp", Cmd::Stgtp),
        ("stons", Cmd::Stons),
        ("staln", Cmd::Staln),
        ("stlnm", Cmd::Stlnm),
        ("smotc", Cmd::Smotc),
        ("staop ", Cmd::Staop),
        ("stdet ", Cmd::Stdet),
        ("masns ", Cmd::Masns),
        ("reset ", Cmd::Reset),
    ];
    for (line, cmd) in cases {
        let mut r = Reply::default();
        dirty(&mut r);
        assert_eq!(parse(line, &mut r), ParseStatus::Ok, "{line}");
        assert_eq!(r.cmd, cmd);
        assert!(!r.ack.error);
        assert_eq!(r.ack.valve, NO_VALVE);
        assert!(!r.gvlon_error);
        assert_eq!(r.valve_data.moves, 0); // rest reset
                                           // An ack takes no argument (smotc: only "err").
        dirty(&mut r);
        let want = if cmd == Cmd::Smotc {
            ParseStatus::BadFormat
        } else {
            ParseStatus::BadArgCount
        };
        assert_eq!(parse(&format!("{line} 1"), &mut r), want, "{line}");
        assert!(is_empty_reply(&r));
    }
}

#[test]
fn smotc_and_scalx_ok_err_forms() {
    let mut r = Reply::default();
    assert_eq!(parse("smotc err", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Smotc);
    assert!(r.ack.error);
    assert_eq!(parse("smotc ok", &mut r), ParseStatus::BadFormat);
    assert_eq!(parse("smotc err 1", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("smotc ERR", &mut r), ParseStatus::BadFormat);

    assert_eq!(parse("scalx ok", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Scalx);
    assert!(!r.ack.error);
    assert_eq!(parse("scalx err ", &mut r), ParseStatus::Ok);
    assert!(r.ack.error);
    assert_eq!(parse("scalx", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("scalx ok ok", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("scalx fine", &mut r), ParseStatus::BadFormat);
    assert_eq!(parse("scalx o", &mut r), ParseStatus::BadFormat);
    assert_eq!(parse("scalx oks", &mut r), ParseStatus::BadFormat);
}

#[test]
fn stvls_ack_carries_the_valve() {
    let mut r = Reply::default();
    assert_eq!(parse("stvls 3", &mut r), ParseStatus::Ok); // v1: no trailing space
    assert_eq!(r.cmd, Cmd::Stvls);
    assert_eq!(r.ack.valve, 3);
    assert!(!r.ack.error);
    assert_eq!(parse("stvls 0", &mut r), ParseStatus::Ok);
    assert_eq!(r.ack.valve, 0);
    assert_eq!(parse("stvls 11 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.ack.valve, 11);
    assert_eq!(parse("stvls 12", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("stvls", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("stvls 1 2", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("stvls x", &mut r), ParseStatus::BadNumber);
}

// ================================================================ replies: v1 data

#[test]
fn gtgtp() {
    let mut r = Reply::default();
    assert_eq!(parse("gtgtp 3 50 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gtgtp);
    assert_eq!(r.target.valve, 3);
    assert_eq!(r.target.target, 50);
    assert_eq!(parse("gtgtp 11 100", &mut r), ParseStatus::Ok);
    assert_eq!(r.target.valve, 11);
    assert_eq!(r.target.target, 100);
    assert_eq!(parse("gtgtp 0 0", &mut r), ParseStatus::Ok);
    assert_eq!(r.target.target, 0);
    assert_eq!(parse("gtgtp 12 50", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("gtgtp 3 101", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("gtgtp 3", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("gtgtp 3 50 1", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("gtgtp -1 50", &mut r), ParseStatus::BadNumber);
}

#[test]
fn gvlvd_golden_and_calibrating_bit() {
    let mut r = Reply::default();
    dirty(&mut r);
    assert_eq!(
        parse("gvlvd 3 42 18 1 215 -500 57 3120 3350 230 0 ", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.cmd, Cmd::Gvlvd);
    let d = r.valve_data;
    assert_eq!(d.valve, 3);
    assert_eq!(d.position, 42);
    assert_eq!(d.mean_current, 18);
    assert_eq!(d.status, 1);
    assert!(!d.calibrating);
    assert_eq!(d.temp1, 215);
    assert_eq!(d.temp2, -500);
    assert_eq!(d.moves, 57);
    assert_eq!(d.open_count, 3120);
    assert_eq!(d.close_count, 3350);
    assert_eq!(d.dead_zone, 230);
    assert_eq!(d.calib_retries, 0);
    assert!(!r.gvlon_error); // other members reset
    assert_eq!(r.status.uptime_s, 0);

    assert_eq!(
        parse("gvlvd 0 0 20 131 -1270 850 0 0 0 0 2", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.valve_data.status, 3);
    assert!(r.valve_data.calibrating);
    assert_eq!(r.valve_data.temp1, -1270);
    assert_eq!(r.valve_data.temp2, 850);
    assert_eq!(r.valve_data.calib_retries, 2);

    assert_eq!(
        parse(
            "gvlvd 11 100 65535 255 32767 -32768 4294967295 4294967295 4294967295 -2147483648 255",
            &mut r
        ),
        ParseStatus::Ok
    );
    assert_eq!(r.valve_data.valve, 11);
    assert_eq!(r.valve_data.position, 100);
    assert_eq!(r.valve_data.mean_current, 65535);
    assert_eq!(r.valve_data.status, 127);
    assert!(r.valve_data.calibrating);
    assert_eq!(r.valve_data.temp1, 32767);
    assert_eq!(r.valve_data.temp2, -32768);
    assert_eq!(r.valve_data.moves, 4_294_967_295);
    assert_eq!(r.valve_data.open_count, 4_294_967_295);
    assert_eq!(r.valve_data.close_count, 4_294_967_295);
    assert_eq!(r.valve_data.dead_zone, i32::MIN);
    assert_eq!(r.valve_data.calib_retries, 255);

    assert_eq!(
        parse("gvlvd 1 0 0 128 0 0 0 0 0 2147483647 0", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.valve_data.status, 0);
    assert!(r.valve_data.calibrating);
    assert_eq!(r.valve_data.dead_zone, 2_147_483_647);
    assert_eq!(
        parse("gvlvd 1 0 0 127 0 0 0 0 0 -230 0", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.valve_data.status, 127);
    assert!(!r.valve_data.calibrating);
    assert_eq!(r.valve_data.dead_zone, -230);
}

#[test]
fn gvlvd_rejects_every_bad_field() {
    let base = [
        "3", "42", "18", "1", "215", "-500", "57", "3120", "3350", "230", "0",
    ];
    // Field index -> out-of-range value, bad-number value.
    let oor = [
        "12",
        "101",
        "65536",
        "256",
        "32768",
        "-32769",
        "4294967296",
        "4294967296",
        "4294967296",
        "2147483648",
        "256",
    ];
    let bad = [
        "-1", "-1", "-1", "-1", "2a", "+5", "-1", "-1", "-1", "--1", "-1",
    ];
    for f in 0..11 {
        for k in 0..2 {
            let mut line = String::from("gvlvd");
            for i in 0..11 {
                line.push(' ');
                line += if i == f {
                    if k == 0 {
                        oor[i]
                    } else {
                        bad[i]
                    }
                } else {
                    base[i]
                };
            }
            let mut r = Reply::default();
            dirty(&mut r);
            let st = parse(&line, &mut r);
            let want = if k == 0 {
                ParseStatus::OutOfRange
            } else {
                ParseStatus::BadNumber
            };
            assert_eq!(st, want, "{line}");
            assert!(is_empty_reply(&r), "{line}");
        }
    }
    let mut r = Reply::default();
    assert_eq!(
        parse("gvlvd 3 42 18 1 215 -500 57 3120 3350 230", &mut r),
        ParseStatus::BadArgCount
    );
    assert_eq!(
        parse("gvlvd 3 42 18 1 215 -500 57 3120 3350 230 0 0", &mut r),
        ParseStatus::BadArgCount
    );
    assert_eq!(parse("gvlvd", &mut r), ParseStatus::BadArgCount);
    assert_eq!(
        parse(
            "gvlvd 3 42 18 1 215 -500 57 3120 3350 -2147483649 0",
            &mut r
        ),
        ParseStatus::OutOfRange
    );
    assert_eq!(
        parse("gvlvd 3 42 18 1 215 -500 57 3120 3350 - 0", &mut r),
        ParseStatus::BadNumber
    );
    assert_eq!(
        parse("gvlvd 3 42 18 1 215 -500 57 12345678901 3350 0 0", &mut r),
        ParseStatus::BadNumber
    );
}

#[test]
fn gvlst_with_and_without_the_v1_trailing_comma() {
    let mut r = Reply::default();
    dirty(&mut r);
    assert_eq!(
        parse("gvlst 12 8,6,6,8,6,6,6,6,6,6,6,6, ", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.cmd, Cmd::Gvlst);
    assert_eq!(r.valve_states.status, [8, 6, 6, 8, 6, 6, 6, 6, 6, 6, 6, 6]);
    assert_eq!(r.valve_data.moves, 0);

    assert_eq!(
        parse("gvlst 12 0,1,2,3,4,5,6,7,8,9,255,137 ", &mut r),
        ParseStatus::Ok
    );
    for i in 0..10u8 {
        assert_eq!(r.valve_states.status[usize::from(i)], i);
    }
    assert_eq!(r.valve_states.status[10], 255);
    assert_eq!(r.valve_states.status[11], 137);

    let cases = [
        ("gvlst 12 1,1,1,1,1,1,1,1,1,1,1", ParseStatus::BadFormat), // 11
        ("gvlst 12 1,1,1,1,1,1,1,1,1,1,1,1,1", ParseStatus::BadFormat), // 13
        ("gvlst 12 1,1,1,1,1,1,1,1,1,1,1,1,,", ParseStatus::BadFormat),
        ("gvlst 12 1,,1,1,1,1,1,1,1,1,1,1,1", ParseStatus::BadFormat),
        ("gvlst 12 ,1,1,1,1,1,1,1,1,1,1,1,1", ParseStatus::BadFormat),
        ("gvlst 12 ,", ParseStatus::BadFormat),
        (
            "gvlst 12 1,1,1,1,1,1,1,1,1,1,1,256",
            ParseStatus::OutOfRange,
        ),
        ("gvlst 12 1,1,1,1,1,1,1,1,1,1,1,x", ParseStatus::BadNumber),
        ("gvlst 11 1,1,1,1,1,1,1,1,1,1,1", ParseStatus::OutOfRange),
        (
            "gvlst 13 1,1,1,1,1,1,1,1,1,1,1,1,1",
            ParseStatus::OutOfRange,
        ),
        ("gvlst 12", ParseStatus::BadArgCount),
        ("gvlst 12 1,1,1,1,1,1 1,1,1,1,1,1", ParseStatus::BadArgCount),
        ("gvlst x 1,1,1,1,1,1,1,1,1,1,1,1", ParseStatus::BadNumber),
    ];
    for (line, want) in cases {
        assert_eq!(parse(line, &mut r), want, "{line}");
    }
}

#[test]
fn gonec_gowvc_count_and_list_forms() {
    let mut r = Reply::default();
    assert_eq!(parse("gonec 0 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gonec);
    assert_eq!(r.one_wire_list.count, 0);
    assert!(!r.one_wire_list.has_list);
    assert_eq!(parse("gonec 34 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.one_wire_list.count, 34);
    assert!(!r.one_wire_list.has_list);
    assert_eq!(parse("gonec 35", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("gowvc 8", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gowvc);
    assert_eq!(r.one_wire_list.count, 8);
    assert_eq!(parse("gowvc 9", &mut r), ParseStatus::OutOfRange);

    let three = format!("{ID1},{ID2},{ID3}");
    assert_eq!(parse(&format!("gonec 3 {three} "), &mut r), ParseStatus::Ok);
    assert_eq!(r.one_wire_list.count, 3);
    assert!(r.one_wire_list.has_list);
    assert_eq!(r.one_wire_list.ids[0], id(ID1));
    assert_eq!(r.one_wire_list.ids[1], id(ID2));
    assert_eq!(r.one_wire_list.ids[2], id(ID3));
    assert!(is_zero(&r.one_wire_list.ids[3]));
    assert_eq!(parse(&format!("gowvc 3 {three},"), &mut r), ParseStatus::Ok); // trailing comma
    assert_eq!(r.one_wire_list.ids[2], id(ID3));

    // Full lists at the limits; the 34-sensor line is the longest reply.
    let line34 = format!("gonec 34 {} ", id_list(ID1, 34, false));
    assert_eq!(line34.len(), 9 + 34 * 23 + 33 + 1);
    assert_eq!(parse(&line34, &mut r), ParseStatus::Ok);
    assert_eq!(r.one_wire_list.count, 34);
    assert_eq!(r.one_wire_list.ids[33], id(ID1));
    assert_eq!(
        parse(&format!("gowvc 8 {}", id_list(ID3, 8, true)), &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.one_wire_list.count, 8);

    let cases = [
        (
            format!("gonec 35 {}", id_list(ID1, 35, false)),
            ParseStatus::OutOfRange,
        ),
        (format!("gonec 3 {ID1},{ID2}"), ParseStatus::BadFormat), // short
        (format!("gonec 1 {ID1},{ID2}"), ParseStatus::BadFormat), // long
        (format!("gonec 0 {ID1}"), ParseStatus::BadFormat),
        (format!("gonec 2 {ID1},,{ID2}"), ParseStatus::BadFormat),
        (
            "gonec 1 28-84-37-94-97-ff-03-2".to_string(),
            ParseStatus::BadOneWireId,
        ),
        (
            "gonec 1 28-84-37-94-97-ff-03-2g".to_string(),
            ParseStatus::BadOneWireId,
        ),
        (
            "gonec 1 28:84-37-94-97-ff-03-23".to_string(),
            ParseStatus::BadOneWireId,
        ),
        (format!("gonec 2 {ID1} {ID2}"), ParseStatus::BadArgCount),
        ("gonec".to_string(), ParseStatus::BadArgCount),
        ("gonec -1".to_string(), ParseStatus::BadNumber),
    ];
    for (line, want) in cases {
        assert_eq!(parse(&line, &mut r), want, "{line}");
    }
}

#[test]
fn goned_gowvd_data_and_error_forms() {
    let mut r = Reply::default();
    dirty(&mut r);
    assert_eq!(
        parse("goned 28-84-37-94-97-ff-03-23 215 ", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.cmd, Cmd::Goned);
    assert!(r.temp_data.valid);
    assert_eq!(r.temp_data.id, id(ID1));
    assert_eq!(r.temp_data.value, 215);
    assert!(!r.gvlon_error);
    assert_eq!(
        parse("goned 00-00-00-00-00-00-00-00 -500", &mut r),
        ParseStatus::Ok
    ); // stale index
    assert!(r.temp_data.valid);
    assert!(is_zero(&r.temp_data.id));
    assert_eq!(r.temp_data.value, -500);
    assert_eq!(
        parse("goned 28-84-37-94-97-FF-03-23 -32768", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.temp_data.value, -32768);
    assert_eq!(
        parse("goned 28-84-37-94-97-ff-03-23 32768", &mut r),
        ParseStatus::OutOfRange
    );
    assert_eq!(
        parse("goned 28-84-37-94-97-ff-03-23 -32769", &mut r),
        ParseStatus::OutOfRange
    );
    assert_eq!(
        parse("goned 28-84-37-94-97-ff-03-23 21.5", &mut r),
        ParseStatus::BadNumber
    );
    assert_eq!(
        parse("goned 28-84-37-94-97-ff-03 215", &mut r),
        ParseStatus::BadOneWireId
    );

    assert_eq!(parse("goned 0 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Goned);
    assert!(!r.temp_data.valid);
    assert_eq!(r.temp_data.value, TEMP_UNASSIGNED);
    assert_eq!(parse("goned 00", &mut r), ParseStatus::BadFormat);
    assert_eq!(parse("goned 1", &mut r), ParseStatus::BadFormat);
    assert_eq!(parse("goned", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("goned 1 2 3", &mut r), ParseStatus::BadArgCount);

    assert_eq!(
        parse("gowvd 26-11-22-33-44-55-66-29 1234 ", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.cmd, Cmd::Gowvd);
    assert!(r.volt_data.valid);
    assert_eq!(r.volt_data.id, id(ID3));
    assert_eq!(r.volt_data.vad, 1234);
    assert_eq!(
        parse("gowvd 26-11-22-33-44-55-66-29 -1000", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.volt_data.vad, VAD_FAILED);
    assert_eq!(
        parse("gowvd 26-11-22-33-44-55-66-29 -2147483648", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.volt_data.vad, i32::MIN);
    assert_eq!(
        parse("gowvd 26-11-22-33-44-55-66-29 2147483647", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.volt_data.vad, i32::MAX);
    assert_eq!(
        parse("gowvd 26-11-22-33-44-55-66-29 2147483648", &mut r),
        ParseStatus::OutOfRange
    );
    assert_eq!(parse("gowvd 0", &mut r), ParseStatus::Ok);
    assert!(!r.volt_data.valid);
    assert_eq!(r.volt_data.vad, VAD_FAILED);
    // "error" is only the gvlon quirk on the goned prefix.
    assert_eq!(parse("gowvd error", &mut r), ParseStatus::BadFormat);
}

#[test]
fn v1_gvlon_error_arrives_with_the_goned_prefix() {
    let mut r = Reply::default();
    dirty(&mut r);
    assert_eq!(parse("goned error ", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gvlon);
    assert!(r.gvlon_error);
    assert!(!r.temp_data.valid);
    assert_eq!(r.valve_data.moves, 0);
    assert_eq!(parse("goned error 1", &mut r), ParseStatus::BadOneWireId);
    assert_eq!(parse("goned errors", &mut r), ParseStatus::BadFormat);
    assert_eq!(parse("gvlon error", &mut r), ParseStatus::BadArgCount);
}

#[test]
fn gvlon_single_and_list() {
    let mut r = Reply::default();
    assert_eq!(
        parse(&format!("gvlon 3 {ID1} {ID2} "), &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.cmd, Cmd::Gvlon);
    assert!(!r.valve_sensors.is_list);
    assert!(!r.gvlon_error);
    assert_eq!(r.valve_sensors.valve, 3);
    assert_eq!(r.valve_sensors.ids[3][0], id(ID1));
    assert_eq!(r.valve_sensors.ids[3][1], id(ID2));
    assert!(is_zero(&r.valve_sensors.ids[0][0]));
    assert_eq!(
        parse(&format!("gvlon 11 00-00-00-00-00-00-00-00 {ID2}"), &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.valve_sensors.valve, 11);
    assert!(is_zero(&r.valve_sensors.ids[11][0]));
    assert_eq!(r.valve_sensors.ids[11][1], id(ID2));
    assert_eq!(
        parse(&format!("gvlon 12 {ID1} {ID2}"), &mut r),
        ParseStatus::OutOfRange
    );
    assert_eq!(
        parse(&format!("gvlon 3 x {ID2}"), &mut r),
        ParseStatus::BadOneWireId
    );
    assert_eq!(
        parse(&format!("gvlon 3 {ID2} x"), &mut r),
        ParseStatus::BadOneWireId
    );
    assert_eq!(parse("gvlon 3", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("gvlon 3 a b c", &mut r), ParseStatus::BadArgCount);

    // List: 24 ids in valve pairs; garbage ids (v1 OOB read) are passed verbatim.
    let mut list = String::new();
    for v in 0..12 {
        list += if v % 2 == 1 { ID1 } else { ID2 };
        list.push(',');
        list += if v == 5 {
            "de-ad-be-ef-01-02-03-04"
        } else {
            "00-00-00-00-00-00-00-00"
        };
        if v < 11 {
            list.push(',');
        }
    }
    assert_eq!(parse(&format!("gvlon 12 {list} "), &mut r), ParseStatus::Ok);
    assert!(r.valve_sensors.is_list);
    for v in 0..12 {
        let want = id(if v % 2 == 1 { ID1 } else { ID2 });
        assert_eq!(r.valve_sensors.ids[v][0], want, "{v}");
    }
    assert_eq!(r.valve_sensors.ids[5][1], id("de-ad-be-ef-01-02-03-04"));
    assert!(is_zero(&r.valve_sensors.ids[4][1]));
    assert_eq!(parse(&format!("gvlon 12 {list},"), &mut r), ParseStatus::Ok);
    assert_eq!(
        parse(&format!("gvlon 12 {list},{ID1}"), &mut r),
        ParseStatus::BadFormat
    ); // 25 ids
    assert_eq!(
        parse(&format!("gvlon 12 {}", id_list(ID1, 23, false)), &mut r),
        ParseStatus::BadFormat
    );
    assert_eq!(
        parse(&format!("gvlon 11 {}", id_list(ID1, 22, false)), &mut r),
        ParseStatus::OutOfRange
    );
    assert_eq!(
        parse(&format!("gvlon 12 {},bad", id_list(ID1, 23, false)), &mut r),
        ParseStatus::BadOneWireId
    );
}

#[test]
fn gtlnm_ghwin_eepst() {
    let mut r = Reply::default();
    assert_eq!(parse("gtlnm 2000 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gtlnm);
    assert_eq!(r.learn_movements, 2000);
    assert_eq!(parse("gtlnm 0", &mut r), ParseStatus::Ok);
    assert_eq!(r.learn_movements, 0);
    assert_eq!(parse("gtlnm 65535", &mut r), ParseStatus::Ok);
    assert_eq!(r.learn_movements, 65535);
    assert_eq!(parse("gtlnm 65536", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("gtlnm", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("gtlnm 1 2", &mut r), ParseStatus::BadArgCount);

    assert_eq!(parse("ghwin 1073 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Ghwin);
    assert_eq!(r.hw_id, 0x431);
    assert_eq!(stm_chip_name(r.hw_id), "STM32F411xx");
    assert_eq!(parse("ghwin 1059", &mut r), ParseStatus::Ok);
    assert_eq!(r.hw_id, 0x423);
    assert_eq!(parse("ghwin 4095", &mut r), ParseStatus::Ok);
    assert_eq!(r.hw_id, 4095);
    assert_eq!(parse("ghwin 0", &mut r), ParseStatus::Ok);
    assert_eq!(r.hw_id, 0);
    assert_eq!(parse("ghwin 4096", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("ghwin 0x431", &mut r), ParseStatus::BadNumber);

    assert_eq!(parse("eepst 1 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Eepst);
    assert!(r.eeprom_idle);
    assert_eq!(parse("eepst 0 ", &mut r), ParseStatus::Ok);
    assert!(!r.eeprom_idle);
    assert_eq!(parse("eepst 2", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("eepst", &mut r), ParseStatus::BadArgCount);
}

#[test]
fn gmotc_with_3_to_5_fields() {
    let mut r = Reply::default();
    assert_eq!(parse("gmotc 17 17 50 3000 0 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gmotc);
    assert_eq!(r.motor_chars.low_factor, 17);
    assert_eq!(r.motor_chars.high_factor, 17);
    assert_eq!(r.motor_chars.start_on_power, 50);
    assert_eq!(r.motor_chars.min_counts, 3000);
    assert_eq!(r.motor_chars.max_calib_retries, 0);
    assert_eq!(r.motor_chars.field_count, 5);
    assert_eq!(parse("gmotc 5 50 255 65535 255", &mut r), ParseStatus::Ok); // v1 accepts, verbatim
    assert_eq!(r.motor_chars.low_factor, 5);
    assert_eq!(r.motor_chars.high_factor, 50);
    assert_eq!(r.motor_chars.start_on_power, 255);
    assert_eq!(r.motor_chars.min_counts, 65535);
    assert_eq!(r.motor_chars.max_calib_retries, 255);
    assert_eq!(parse("gmotc 12 13 40", &mut r), ParseStatus::Ok);
    assert_eq!(r.motor_chars.field_count, 3);
    assert_eq!(r.motor_chars.low_factor, 12);
    assert_eq!(r.motor_chars.high_factor, 13);
    assert_eq!(r.motor_chars.start_on_power, 40);
    assert_eq!(r.motor_chars.min_counts, 3000); // defaults for missing fields
    assert_eq!(r.motor_chars.max_calib_retries, 2);
    assert_eq!(parse("gmotc 12 13 40 100", &mut r), ParseStatus::Ok);
    assert_eq!(r.motor_chars.field_count, 4);
    assert_eq!(r.motor_chars.min_counts, 100);
    assert_eq!(r.motor_chars.max_calib_retries, 2);
    assert_eq!(parse("gmotc 12 13", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("gmotc 1 2 3 4 5 6", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("gmotc 256 13 40", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("gmotc 12 256 40", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("gmotc 12 13 256", &mut r), ParseStatus::OutOfRange);
    assert_eq!(
        parse("gmotc 12 13 40 65536", &mut r),
        ParseStatus::OutOfRange
    );
    assert_eq!(
        parse("gmotc 12 13 40 1 256", &mut r),
        ParseStatus::OutOfRange
    );
}

#[test]
fn gvers_with_suffix_hw_and_build() {
    let mut r = Reply::default();
    dirty(&mut r);
    assert_eq!(parse("gvers 1.4.9_C1 1 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gvers);
    assert!(r.version.valid);
    assert_eq!(r.version.major, 1);
    assert_eq!(r.version.minor, 4);
    assert_eq!(r.version.patch, 9);
    assert_text(&r.version.suffix, "");
    assert_text(&r.version.hw, "C1");
    assert_eq!(r.build, 1);

    assert_eq!(
        parse("gvers 1.4.9_Dev_C2 1712345678 ", &mut r),
        ParseStatus::Ok
    );
    assert_text(&r.version.suffix, "_Dev");
    assert_text(&r.version.hw, "C2");
    assert_eq!(r.build, 1_712_345_678);
    assert!(!is_revamped(&r.version));

    assert_eq!(parse("gvers 2.0.0-revamped_C2 1", &mut r), ParseStatus::Ok);
    assert_eq!(r.version.major, 2);
    assert_text(&r.version.suffix, "-revamped");
    assert_text(&r.version.hw, "C2");
    assert!(is_revamped(&r.version));

    assert_eq!(
        parse("gvers 2.0.0-revamped-dev_C2 4294967295", &mut r),
        ParseStatus::Ok
    );
    assert_text(&r.version.suffix, "-revamped-dev");
    assert_eq!(r.build, 4_294_967_295);

    dirty(&mut r);
    assert_eq!(parse("gvers 1.4.9_Dev", &mut r), ParseStatus::Ok); // no build, no hw
    assert_eq!(r.build, 0);
    assert_text(&r.version.suffix, "_Dev");
    assert_text(&r.version.hw, "");

    let min = parse_version(b"1.4.0");
    assert!(min.valid);
    assert_eq!(parse("gvers 1.4.12+hc2 1", &mut r), ParseStatus::Ok);
    assert!(compare_version(&r.version, &min) > 0);
    assert_eq!(parse("gvers 1.3.9_C2 1", &mut r), ParseStatus::Ok);
    assert!(compare_version(&r.version, &min) < 0);

    dirty(&mut r);
    assert_eq!(parse("gvers 1.4 1", &mut r), ParseStatus::BadFormat);
    assert!(is_empty_reply(&r));
    assert_eq!(parse("gvers v1.4.9 1", &mut r), ParseStatus::BadFormat);
    assert_eq!(parse("gvers 1.4.9 x", &mut r), ParseStatus::BadNumber);
    assert_eq!(
        parse("gvers 1.4.9 4294967296", &mut r),
        ParseStatus::OutOfRange
    );
    assert_eq!(parse("gvers", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("gvers 1.4.9 1 2", &mut r), ParseStatus::BadArgCount);
}

// ================================================================ replies: v2

#[test]
fn gproto() {
    let mut r = Reply::default();
    assert_eq!(parse("gproto 2", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gproto);
    assert_eq!(r.proto, 2);
    assert_eq!(parse("gproto 1 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.proto, 1);
    assert_eq!(parse("gproto 255", &mut r), ParseStatus::Ok);
    assert_eq!(r.proto, 255);
    assert_eq!(parse("gproto 0", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("gproto 256", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("gproto", &mut r), ParseStatus::BadArgCount);
}

const GVLVX_FIELDS: [&str; 19] = [
    "4", "130", "42", "60", "21", "3120", "3350", "-230", "1", "57", "2", "7", "3", "1", "3000",
    "1450", "3", "412", "8123",
];

fn gvlvx_line() -> String {
    format!("gvlvx {}", GVLVX_FIELDS.join(" "))
}

/// The gvlvx golden with field `replace` (0-based, after the command) set to `value`.
fn gvlvx_line_with(replace: usize, value: &str) -> String {
    let mut fields = GVLVX_FIELDS;
    fields[replace] = value;
    format!("gvlvx {}", fields.join(" "))
}

#[test]
fn gvlvx_golden() {
    let mut r = Reply::default();
    dirty(&mut r);
    assert_eq!(parse(&(gvlvx_line() + " "), &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gvlvx);
    let x = r.valve_ex;
    assert_eq!(x.valve, 4);
    assert_eq!(x.status, 2);
    assert!(x.calibrating);
    assert_eq!(x.position, 42);
    assert_eq!(x.target, 60);
    assert_eq!(x.mean_current, 21);
    assert_eq!(x.open_count, 3120);
    assert_eq!(x.close_count, 3350);
    assert_eq!(x.dead_zone, -230);
    assert_eq!(x.calib_retries, 1);
    assert_eq!(x.moves, 57);
    assert_eq!(x.cal_state, 2);
    assert_eq!(x.cal_flags, 0);
    assert_eq!(x.early_stops, 7);
    assert_eq!(x.cmd_rejected, 3);
    assert_eq!(x.last_move.dir, MoveDir::Close);
    assert_eq!(x.last_move.requested_counts, 3000);
    assert_eq!(x.last_move.counted_counts, 1450);
    assert_eq!(x.last_move.stop, StopReason::EarlyEndStop);
    assert_eq!(x.last_move.peak_current, 412);
    assert_eq!(x.last_move.duration_ms, 8123);
    assert_eq!(r.valve_data.moves, 0);

    assert_eq!(parse(&gvlvx_line_with(1, "9"), &mut r), ParseStatus::Ok);
    assert_eq!(r.valve_ex.status, 9);
    assert!(r.valve_ex.calibrating); // calState 2: running, bit 7 does not matter
    assert_eq!(parse(&gvlvx_line_with(13, "0"), &mut r), ParseStatus::Ok);
    assert_eq!(r.valve_ex.last_move.dir, MoveDir::Open);
    assert_eq!(parse(&gvlvx_line_with(16, "7"), &mut r), ParseStatus::Ok);
    assert_eq!(r.valve_ex.last_move.stop, StopReason::Aborted);
    assert_eq!(parse(&gvlvx_line_with(16, "0"), &mut r), ParseStatus::Ok);
    assert_eq!(r.valve_ex.last_move.stop, StopReason::None);
    assert_eq!(parse(&gvlvx_line_with(10, "0"), &mut r), ParseStatus::Ok);
    assert_eq!(r.valve_ex.cal_state, 0);
}

#[test]
fn gvlvx_cal_state_is_a_bit_field_and_calibrating_follows_it() {
    // Examples of software_stm32/PROTOCOL_V2.md (revamped STM 2.x).
    let mut r = Reply::default();
    assert_eq!(
        parse(
            "gvlvx 3 9 0 30 17 3567 3610 43 2 12 8 1 4 1 65535 102 3 356 2140",
            &mut r
        ),
        ParseStatus::Ok
    );
    assert_eq!(r.valve_ex.valve, 3);
    assert_eq!(r.valve_ex.status, 9);
    assert_eq!(r.valve_ex.cal_state, CAL_STATE_IDLE);
    assert_eq!(r.valve_ex.cal_flags, CAL_FLAG_LAST_FAILED);
    assert!(!r.valve_ex.calibrating);
    assert_eq!(r.valve_ex.early_stops, 1);

    // (status, cal, state, flags, calibrating)
    let cases: [(&str, &str, u8, u8, bool); 11] = [
        ("1", "0", 0, 0, false),
        // requested by the time trigger / a valve found at start-up: bit 7 clear
        ("8", "1", 1, 0, false),
        // requested by staln or the movement trigger: bit 7 set
        ("129", "1", 1, 0, true),
        // running without bit 7 (time trigger, first target change)
        ("1", "2", 2, 0, true),
        ("1", "6", 2, CAL_FLAG_EARLY_STOP, true),
        ("1", "10", 2, CAL_FLAG_LAST_FAILED, true),
        (
            "129",
            "14",
            2,
            CAL_FLAG_EARLY_STOP | CAL_FLAG_LAST_FAILED,
            true,
        ),
        ("1", "4", 0, CAL_FLAG_EARLY_STOP, false),
        (
            "9",
            "12",
            0,
            CAL_FLAG_EARLY_STOP | CAL_FLAG_LAST_FAILED,
            false,
        ),
        (
            "1",
            "13",
            1,
            CAL_FLAG_EARLY_STOP | CAL_FLAG_LAST_FAILED,
            false,
        ),
        (
            "1",
            "15",
            3,
            CAL_FLAG_EARLY_STOP | CAL_FLAG_LAST_FAILED,
            false,
        ),
    ];
    for (status, cal, state, flags, calibrating) in cases {
        let line = gvlvx_line_with(10, cal).replacen(" 130 ", &format!(" {status} "), 1);
        assert_eq!(parse(&line, &mut r), ParseStatus::Ok, "{line}");
        let raw: u8 = status.parse().expect("number");
        assert_eq!(r.valve_ex.status, raw & 0x7F, "{line}");
        assert_eq!(r.valve_ex.cal_state, state, "{line}");
        assert_eq!(r.valve_ex.cal_flags, flags, "{line}");
        assert_eq!(r.valve_ex.calibrating, calibrating, "{line}");
    }
    assert_eq!(
        parse(&gvlvx_line_with(10, "16"), &mut r),
        ParseStatus::OutOfRange
    );
    assert_eq!(CAL_STATE_REQUESTED, 1);
    assert_eq!(CAL_STATE_RUNNING, 2);
    assert_eq!(CAL_STATE_MASK, 0x03);
    assert_eq!(CAL_FLAG_MASK, CAL_FLAG_EARLY_STOP | CAL_FLAG_LAST_FAILED);
}

#[test]
fn gvlvx_field_ranges() {
    // Max accepted and first rejected value per field.
    let max_ok = [
        "11",
        "255",
        "100",
        "100",
        "65535",
        "4294967295",
        "4294967295",
        "2147483647",
        "255",
        "4294967295",
        "15",
        "4294967295",
        "4294967295",
        "1",
        "4294967295",
        "4294967295",
        "7",
        "65535",
        "4294967295",
    ];
    let too_big = [
        "12",
        "256",
        "101",
        "101",
        "65536",
        "4294967296",
        "4294967296",
        "2147483648",
        "256",
        "4294967296",
        "16",
        "4294967296",
        "4294967296",
        "2",
        "4294967296",
        "4294967296",
        "8",
        "65536",
        "4294967296",
    ];
    for f in 0..19 {
        let mut r = Reply::default();
        assert_eq!(
            parse(&gvlvx_line_with(f, max_ok[f]), &mut r),
            ParseStatus::Ok,
            "{f}"
        );
        dirty(&mut r);
        assert_eq!(
            parse(&gvlvx_line_with(f, too_big[f]), &mut r),
            ParseStatus::OutOfRange,
            "{f}"
        );
        assert!(is_empty_reply(&r), "{f}");
        let bad = if f == 7 { "1-" } else { "-1" };
        assert_eq!(
            parse(&gvlvx_line_with(f, bad), &mut r),
            ParseStatus::BadNumber,
            "{f}"
        );
    }
    let mut r = Reply::default();
    assert_eq!(
        parse(&gvlvx_line_with(7, "-2147483648"), &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.valve_ex.dead_zone, i32::MIN);
    assert_eq!(
        parse(&gvlvx_line_with(7, "-2147483649"), &mut r),
        ParseStatus::OutOfRange
    );
    assert_eq!(
        parse(&(gvlvx_line() + " 0"), &mut r),
        ParseStatus::BadArgCount
    );
    let line = gvlvx_line();
    let eighteen = &line[..line.rfind(' ').expect("space")];
    assert_eq!(parse(eighteen, &mut r), ParseStatus::BadArgCount);
}

#[test]
fn gprof() {
    let mut r = Reply::default();
    dirty(&mut r);
    assert_eq!(
        parse("gprof 3 3 0:150 1500:212 3000:98 ", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.cmd, Cmd::Gprof);
    assert_eq!(r.profile.valve, 3);
    assert_eq!(r.profile.count, 3);
    assert_eq!(r.profile.samples[0].count, 0);
    assert_eq!(r.profile.samples[0].current, 150);
    assert_eq!(r.profile.samples[1].count, 1500);
    assert_eq!(r.profile.samples[1].current, 212);
    assert_eq!(r.profile.samples[2].count, 3000);
    assert_eq!(r.profile.samples[2].current, 98);
    assert_eq!(r.profile.samples[3].count, 0);

    assert_eq!(parse("gprof 11 0", &mut r), ParseStatus::Ok);
    assert_eq!(r.profile.valve, 11);
    assert_eq!(r.profile.count, 0);

    let mut full = String::from("gprof 0 32");
    for i in 0..32 {
        full += &format!(" {}:{}", i * 100, i);
    }
    assert_eq!(parse(&full, &mut r), ParseStatus::Ok);
    assert_eq!(r.profile.count, 32);
    assert_eq!(r.profile.samples[31].count, 3100);
    assert_eq!(r.profile.samples[31].current, 31);
    assert_eq!(parse("gprof 0 1 4294967295:65535", &mut r), ParseStatus::Ok);
    assert_eq!(r.profile.samples[0].count, 4_294_967_295);
    assert_eq!(r.profile.samples[0].current, 65535);

    dirty(&mut r);
    assert_eq!(
        parse(&format!("gprof 0 33{}", " ".repeat(33 * 4)), &mut r),
        ParseStatus::OutOfRange
    );
    assert!(is_empty_reply(&r));
    let cases = [
        ("gprof 12 0", ParseStatus::OutOfRange),
        ("gprof 0 2 1:1", ParseStatus::BadArgCount),
        ("gprof 0 1 1:1 2:2", ParseStatus::BadArgCount),
        ("gprof 0", ParseStatus::BadArgCount),
        ("gprof 0 1 11", ParseStatus::BadFormat),
        ("gprof 0 1 :1", ParseStatus::BadNumber),
        ("gprof 0 1 1:", ParseStatus::BadNumber),
        ("gprof 0 1 1:2:3", ParseStatus::BadNumber),
        ("gprof 0 1 1:65536", ParseStatus::OutOfRange),
        ("gprof 0 1 4294967296:1", ParseStatus::OutOfRange),
        ("gprof 0 1 -1:1", ParseStatus::BadNumber),
        ("gprof 0 x", ParseStatus::BadNumber),
    ];
    for (line, want) in cases {
        assert_eq!(parse(line, &mut r), want, "{line}");
    }
}

#[test]
fn svmov() {
    let mut r = Reply::default();
    assert_eq!(parse("svmov 3 ok", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Svmov);
    assert_eq!(r.service_move.index, 3);
    assert!(r.service_move.ok);
    assert_eq!(r.service_move.error_code, 0);
    assert_eq!(parse("svmov 11 err 2 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.service_move.index, 11);
    assert!(!r.service_move.ok);
    assert_eq!(r.service_move.error_code, 2);
    assert_eq!(parse("svmov 0 err 65535", &mut r), ParseStatus::Ok);
    assert_eq!(r.service_move.error_code, 65535);
    let cases = [
        ("svmov 0 err 65536", ParseStatus::OutOfRange),
        ("svmov 0 err", ParseStatus::BadFormat),
        ("svmov 0 ok 1", ParseStatus::BadFormat),
        ("svmov 0 fine", ParseStatus::BadFormat),
        ("svmov 12 ok", ParseStatus::OutOfRange),
        ("svmov 0", ParseStatus::BadArgCount),
        ("svmov 0 err 1 2", ParseStatus::BadArgCount),
        ("svmov x ok", ParseStatus::BadNumber),
    ];
    for (line, want) in cases {
        assert_eq!(parse(line, &mut r), want, "{line}");
    }
}

#[test]
fn gcalx() {
    let mut r = Reply::default();
    assert_eq!(parse("gcalx 1 10 40", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gcalx);
    assert!(r.breakaway.enable);
    assert_eq!(r.breakaway.step_pct, 10);
    assert_eq!(r.breakaway.max_ma, 40);
    assert_eq!(parse("gcalx 0 0 20 ", &mut r), ParseStatus::Ok);
    assert!(!r.breakaway.enable);
    assert_eq!(r.breakaway.step_pct, 0);
    assert_eq!(r.breakaway.max_ma, 20);
    assert_eq!(parse("gcalx 0 100 60", &mut r), ParseStatus::Ok);
    assert_eq!(r.breakaway.step_pct, 100);
    assert_eq!(r.breakaway.max_ma, 60);
    let cases = [
        ("gcalx 2 0 20", ParseStatus::OutOfRange),
        ("gcalx 0 101 20", ParseStatus::OutOfRange),
        ("gcalx 0 0 19", ParseStatus::OutOfRange),
        ("gcalx 0 0 61", ParseStatus::OutOfRange),
        ("gcalx 0 0", ParseStatus::BadArgCount),
        ("gcalx 0 0 20 1", ParseStatus::BadArgCount),
    ];
    for (line, want) in cases {
        assert_eq!(parse(line, &mut r), want, "{line}");
    }
}

#[test]
fn gstat() {
    let mut r = Reply::default();
    assert_eq!(parse("gstat 3600 2 4 17 5 1 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gstat);
    assert_eq!(r.status.uptime_s, 3600);
    assert_eq!(r.status.resets, 2);
    assert_eq!(r.status.boot_reason, 4);
    assert_eq!(r.status.rx_overflow, 17);
    assert_eq!(r.status.parse_errors, 5);
    assert_eq!(r.status.eep_state, 1);
    assert_eq!(
        parse(
            "gstat 4294967295 4294967295 4294967295 4294967295 4294967295 255",
            &mut r
        ),
        ParseStatus::Ok
    );
    assert_eq!(r.status.uptime_s, 4_294_967_295);
    assert_eq!(r.status.eep_state, 255);
    assert_eq!(
        parse("gstat 1 2 3 4 5 256", &mut r),
        ParseStatus::OutOfRange
    );
    assert_eq!(
        parse("gstat 4294967296 2 3 4 5 6", &mut r),
        ParseStatus::OutOfRange
    );
    assert_eq!(parse("gstat 1 2 3 4 5", &mut r), ParseStatus::BadArgCount);
    assert_eq!(
        parse("gstat 1 2 3 4 5 6 7", &mut r),
        ParseStatus::BadArgCount
    );
    assert_eq!(parse("gstat 3600 2 4 17 5 1", &mut r), ParseStatus::Ok);
    assert!(!r.status.v3);
    assert_eq!(r.status.lease, LeaseState::Off);
    assert_eq!(r.status.sys_flags, 0);
}

// ================================================================ replies: protocol 3

const GVLVY_GOLDEN: &str =
    "gvlvy 3 9 50 30 17 3567 3610 43 2 0 8 1 4 0 1750 1750 1 262 6120 66 4 50 50 3540 0";
const GSTAX_GOLDEN: &str = "gstax 86400 3 2 0 2 0 1 3540 1 60 0 0 0 0 0 0 0 0 0 12 2 3600 0";

/// The gvlvy golden with field `field` (1-based, after the command) replaced.
fn gvlvy_with(field: usize, value: &str) -> String {
    let mut tokens: Vec<&str> = GVLVY_GOLDEN.split(' ').collect();
    tokens[field] = value;
    tokens.join(" ")
}

/// A valid "gstax" line: field i = i, except lease (7), leaseClient (9) and safeMode (12) = 0,
/// then the given replacements.
fn gstax_line(repl: &[(usize, &str)], count: usize) -> String {
    let mut line = String::from("gstax");
    for i in 1..=count {
        let mut v = if i == 7 || i == 9 || i == 12 {
            "0".to_string()
        } else {
            i.to_string()
        };
        for (field, value) in repl {
            if *field == i {
                v = value.to_string();
            }
        }
        line += " ";
        line += &v;
    }
    line
}

#[test]
fn gvlvy_golden() {
    let mut r = Reply::default();
    dirty(&mut r);
    assert_eq!(parse(GVLVY_GOLDEN, &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gvlvy);
    let x = r.valve_ex;
    assert!(x.v3);
    assert_eq!(x.valve, 3);
    assert_eq!(x.status, 9);
    assert!(!x.calibrating);
    assert_eq!(x.position, 50);
    assert_eq!(x.target, 30);
    assert_eq!(x.mean_current, 17);
    assert_eq!(x.open_count, 3567);
    assert_eq!(x.close_count, 3610);
    assert_eq!(x.dead_zone, 43);
    assert_eq!(x.calib_retries, 2);
    assert_eq!(x.moves, 0);
    assert_eq!(x.cal_state, CAL_STATE_IDLE);
    assert_eq!(x.cal_flags, CAL_FLAG_LAST_FAILED);
    assert_eq!(x.early_stops, 1);
    assert_eq!(x.cmd_rejected, 4);
    assert_eq!(x.last_move.dir, MoveDir::Open);
    assert_eq!(x.last_move.requested_counts, 1750);
    assert_eq!(x.last_move.counted_counts, 1750);
    assert_eq!(x.last_move.stop, StopReason::Target);
    assert_eq!(x.last_move.peak_current, 262);
    assert_eq!(x.last_move.duration_ms, 6120);
    assert_eq!(x.flags, 66);
    assert_eq!(x.flags, STM_FLAG_FS_BLOCKED | STM_FLAG_RETRY);
    assert_eq!(x.fault, ValveFault::StrokesTooShort as u8);
    assert_eq!(x.fs_pct, 50);
    assert_eq!(x.drive, 50);
    assert_eq!(x.retry_s, 3540);
    assert_eq!(x.retries, 0);

    // Every v3 field at its upper limit, distinct values.
    assert_eq!(
        parse(
            "gvlvy 11 2 1 2 3 4 5 -6 7 8 2 9 10 1 11 12 7 13 14 65535 255 255 100 4294967295 255 ",
            &mut r
        ),
        ParseStatus::Ok
    );
    assert_eq!(r.valve_ex.flags, 65535);
    assert_eq!(r.valve_ex.fault, 255);
    assert_eq!(r.valve_ex.fs_pct, FAILSAFE_HOLD);
    assert_eq!(r.valve_ex.drive, 100);
    assert_eq!(r.valve_ex.retry_s, 4_294_967_295);
    assert_eq!(r.valve_ex.retries, 255);
    assert!(r.valve_ex.calibrating); // calState running
    assert_eq!(parse(&gvlvy_with(22, "0"), &mut r), ParseStatus::Ok);
    assert_eq!(r.valve_ex.fs_pct, 0);
    assert_eq!(parse(&gvlvy_with(22, "100"), &mut r), ParseStatus::Ok);
    assert_eq!(r.valve_ex.fs_pct, 100);
    assert_eq!(parse(&gvlvy_with(23, "0"), &mut r), ParseStatus::Ok);
    assert_eq!(r.valve_ex.drive, 0);

    // Field count: at least 25; numeric extras are ignored.
    let short = &GVLVY_GOLDEN[..GVLVY_GOLDEN.rfind(' ').expect("space")];
    assert_eq!(parse(short, &mut r), ParseStatus::BadArgCount);
    assert!(is_empty_reply(&r));
    assert_eq!(parse(&format!("{GVLVY_GOLDEN} 7"), &mut r), ParseStatus::Ok);
    assert_eq!(r.valve_ex.retry_s, 3540);
    assert_eq!(r.valve_ex.retries, 0);
    assert_eq!(
        parse(&format!("{GVLVY_GOLDEN} 0 4294967295"), &mut r),
        ParseStatus::Ok
    );
    assert_eq!(
        parse(&format!("{GVLVY_GOLDEN} x"), &mut r),
        ParseStatus::BadNumber
    );
    assert_eq!(
        parse(&format!("{GVLVY_GOLDEN} 7 -1"), &mut r),
        ParseStatus::BadNumber
    );
    assert_eq!(
        parse(&format!("{GVLVY_GOLDEN} 4294967296"), &mut r),
        ParseStatus::OutOfRange
    );

    // Ranges of the v3 fields.
    let cases = [
        (20, "65536", ParseStatus::OutOfRange),
        (21, "256", ParseStatus::OutOfRange),
        (22, "101", ParseStatus::OutOfRange),
        (22, "254", ParseStatus::OutOfRange),
        (22, "256", ParseStatus::OutOfRange),
    ];
    for (field, value, want) in cases {
        assert_eq!(
            parse(&gvlvy_with(field, value), &mut r),
            want,
            "{field} {value}"
        );
    }
    assert_eq!(parse(&gvlvy_with(22, "255"), &mut r), ParseStatus::Ok);
    assert_eq!(r.valve_ex.fs_pct, 255);
    let cases = [
        (23, "101", ParseStatus::OutOfRange),
        (24, "4294967296", ParseStatus::OutOfRange),
        (25, "256", ParseStatus::OutOfRange),
        (20, "x", ParseStatus::BadNumber),
        (22, "-1", ParseStatus::BadNumber),
        // The 19 gvlvx fields keep their rules.
        (1, "12", ParseStatus::OutOfRange),
        (3, "101", ParseStatus::OutOfRange),
        (11, "16", ParseStatus::OutOfRange),
        (19, "x", ParseStatus::BadNumber),
    ];
    for (field, value, want) in cases {
        assert_eq!(
            parse(&gvlvy_with(field, value), &mut r),
            want,
            "{field} {value}"
        );
    }
}

#[test]
fn failsafe_position_check_order_of_gvlvy_and_glcfg() {
    // Rust: the C++ order of the checks (PORT-NOTES.md). gvlvy reads its six v3 fields before
    // it checks the failsafe position; glcfg checks each position as soon as it is read.
    let mut r = Reply::default();
    let mut tokens: Vec<&str> = GVLVY_GOLDEN.split(' ').collect();
    tokens[22] = "101";
    tokens[25] = "x";
    assert_eq!(parse(&tokens.join(" "), &mut r), ParseStatus::BadNumber);
    tokens[25] = "256";
    assert_eq!(parse(&tokens.join(" "), &mut r), ParseStatus::OutOfRange);
    assert_eq!(
        parse("glcfg 60 101 50 50 50 50 50 50 50 50 50 50 x", &mut r),
        ParseStatus::OutOfRange
    );
    assert_eq!(
        parse("glcfg 60 50 50 50 50 50 50 50 50 50 50 x 101", &mut r),
        ParseStatus::BadNumber
    );
}

#[test]
fn gvlvx_stays_at_exactly_19_fields_without_v3_values() {
    let mut r = Reply::default();
    let gvlvx = "gvlvx 4 130 42 60 21 3120 3350 -230 1 57 2 7 3 1 3000 1450 3 412 8123";
    assert_eq!(parse(gvlvx, &mut r), ParseStatus::Ok);
    assert!(!r.valve_ex.v3);
    assert_eq!(r.valve_ex.flags, 0);
    assert_eq!(r.valve_ex.fault, 0);
    assert_eq!(r.valve_ex.fs_pct, FAILSAFE_HOLD);
    assert_eq!(r.valve_ex.drive, 0);
    assert_eq!(r.valve_ex.retry_s, 0);
    assert_eq!(r.valve_ex.retries, 0);
    assert_eq!(
        parse(&format!("{gvlvx} 1"), &mut r),
        ParseStatus::BadArgCount
    );
    assert_eq!(
        parse(&GVLVY_GOLDEN.replacen("gvlvy", "gvlvx", 1), &mut r),
        ParseStatus::BadArgCount
    );
}

#[test]
fn gstax_golden_with_23_values() {
    let mut r = Reply::default();
    dirty(&mut r);
    assert_eq!(parse(GSTAX_GOLDEN, &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gstax);
    let s = r.status;
    assert!(s.v3);
    assert_eq!(s.uptime_s, 86400);
    assert_eq!(s.resets, 3);
    assert_eq!(s.boot_reason, 2);
    assert_eq!(s.rx_overflow, 0);
    assert_eq!(s.parse_errors, 2);
    assert_eq!(s.eep_state, 0);
    assert_eq!(s.lease, LeaseState::Running);
    assert_eq!(s.lease_remain_s, 3540);
    assert!(s.lease_client);
    assert_eq!(s.lease_timeout_min, 60);
    assert_eq!(s.failsafe_mask, 0);
    assert!(!s.safe_mode);
    assert_eq!(s.wdg_resets, 0);
    assert_eq!(s.uart_ore, 0);
    assert_eq!(s.uart_fe, 0);
    assert_eq!(s.uart_ne, 0);
    assert_eq!(s.rx_dropped, 0);
    assert_eq!(s.cfg_flags, 0);
    assert_eq!(s.cfg_events, 0);
    assert_eq!(s.eep_writes, 12);
    assert_eq!(s.temp_age_s, 2);
    assert_eq!(s.ow_scan_age_s, 3600);
    assert_eq!(s.sys_flags, 0);

    // Distinct values in every field (catches swapped fields).
    assert_eq!(
        parse(
            &gstax_line(
                &[
                    (6, "6"),
                    (7, "2"),
                    (9, "1"),
                    (10, "1440"),
                    (11, "4095"),
                    (12, "1"),
                    (18, "255"),
                    (23, "255")
                ],
                23
            ),
            &mut r
        ),
        ParseStatus::Ok
    );
    let s = r.status;
    assert_eq!(s.uptime_s, 1);
    assert_eq!(s.resets, 2);
    assert_eq!(s.boot_reason, 3);
    assert_eq!(s.rx_overflow, 4);
    assert_eq!(s.parse_errors, 5);
    assert_eq!(s.eep_state, 6);
    assert_eq!(s.lease, LeaseState::Expired);
    assert_eq!(s.lease_remain_s, 8);
    assert!(s.lease_client);
    assert_eq!(s.lease_timeout_min, 1440);
    assert_eq!(s.failsafe_mask, 4095);
    assert!(s.safe_mode);
    assert_eq!(s.wdg_resets, 13);
    assert_eq!(s.uart_ore, 14);
    assert_eq!(s.uart_fe, 15);
    assert_eq!(s.uart_ne, 16);
    assert_eq!(s.rx_dropped, 17);
    assert_eq!(s.cfg_flags, 255);
    assert_eq!(s.cfg_events, 19);
    assert_eq!(s.eep_writes, 20);
    assert_eq!(s.temp_age_s, 21);
    assert_eq!(s.ow_scan_age_s, 22);
    assert_eq!(s.sys_flags, 255);
    assert_eq!(parse(&gstax_line(&[], 23), &mut r), ParseStatus::Ok);
    assert_eq!(r.status.lease, LeaseState::Off);
    assert!(!r.status.lease_client);
    assert!(!r.status.safe_mode);
    assert_eq!(r.status.wdg_resets, 13);
    assert_eq!(
        parse(
            &gstax_line(&[(13, "255"), (10, "5"), (11, "0")], 23),
            &mut r
        ),
        ParseStatus::Ok
    );
    assert_eq!(r.status.wdg_resets, 255);
    assert_eq!(r.status.lease_timeout_min, 5);
    assert_eq!(r.status.failsafe_mask, 0);
    assert_eq!(parse(&gstax_line(&[(7, "1")], 23), &mut r), ParseStatus::Ok);
    assert_eq!(r.status.lease, LeaseState::Running);

    // Field ranges (every other field valid).
    let cases = [
        (7, "3", ParseStatus::OutOfRange),
        (9, "2", ParseStatus::OutOfRange),
        (10, "1441", ParseStatus::OutOfRange),
        (11, "4096", ParseStatus::OutOfRange),
        (12, "2", ParseStatus::OutOfRange),
        (13, "256", ParseStatus::OutOfRange),
        (18, "256", ParseStatus::OutOfRange),
        (23, "256", ParseStatus::OutOfRange),
        (8, "4294967296", ParseStatus::OutOfRange),
        (14, "4294967296", ParseStatus::OutOfRange),
        (17, "4294967296", ParseStatus::OutOfRange),
        (19, "4294967296", ParseStatus::OutOfRange),
        (22, "4294967296", ParseStatus::OutOfRange),
        (22, "x", ParseStatus::BadNumber),
        (6, "256", ParseStatus::OutOfRange), // gstat field 6
    ];
    for (field, value, want) in cases {
        assert_eq!(
            parse(&gstax_line(&[(field, value)], 23), &mut r),
            want,
            "{field} {value}"
        );
    }
    assert!(is_empty_reply(&r));

    // Field count: at least 23; numeric extras are ignored.
    let short = &GSTAX_GOLDEN[..GSTAX_GOLDEN.rfind(' ').expect("space")];
    assert_eq!(parse(short, &mut r), ParseStatus::BadArgCount); // 22
    assert_eq!(
        parse(
            "gstax 86400 3 2 0 2 0 1 3540 1 60 0 0 0 0 0 0 0 0 0 12 2",
            &mut r
        ),
        ParseStatus::BadArgCount
    ); // 21
    assert_eq!(
        parse(&format!("{GSTAX_GOLDEN} 99"), &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.status.sys_flags, 0);
    assert_eq!(
        parse(&format!("{GSTAX_GOLDEN} x"), &mut r),
        ParseStatus::BadNumber
    );
    assert_eq!(
        parse(&format!("{GSTAX_GOLDEN} 4294967296"), &mut r),
        ParseStatus::OutOfRange
    );
    // gstat stays at exactly 6.
    assert_eq!(
        parse("gstat 86400 3 2 0 2 0 1", &mut r),
        ParseStatus::BadArgCount
    );
}

#[test]
fn glcfg() {
    let mut r = Reply::default();
    dirty(&mut r);
    assert_eq!(
        parse("glcfg 1440 0 1 2 3 4 5 6 7 8 9 100 255", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.cmd, Cmd::Glcfg);
    assert_eq!(r.lease_config.timeout_min, 1440);
    for i in 0..10u8 {
        assert_eq!(r.lease_config.failsafe_pct[usize::from(i)], i);
    }
    assert_eq!(r.lease_config.failsafe_pct[10], 100);
    assert_eq!(r.lease_config.failsafe_pct[11], FAILSAFE_HOLD);
    assert_eq!(
        parse("glcfg 0 50 50 50 50 50 50 50 50 50 50 50 50 ", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.lease_config.timeout_min, 0);
    assert_eq!(r.lease_config.failsafe_pct[0], 50);
    assert_eq!(
        parse("glcfg 60 50 50 50 50 50 50 50 50 50 50 50 50 1 2", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.lease_config.timeout_min, 60);
    let cases = [
        (
            "glcfg 60 50 50 50 50 50 50 50 50 50 50 50",
            ParseStatus::BadArgCount,
        ),
        ("glcfg", ParseStatus::BadArgCount),
        (
            "glcfg 1441 50 50 50 50 50 50 50 50 50 50 50 50",
            ParseStatus::OutOfRange,
        ),
        (
            "glcfg 60 101 50 50 50 50 50 50 50 50 50 50 50",
            ParseStatus::OutOfRange,
        ),
        (
            "glcfg 60 50 50 50 50 50 50 50 50 50 50 50 254",
            ParseStatus::OutOfRange,
        ),
        (
            "glcfg 60 50 50 50 50 50 50 50 50 50 50 50 256",
            ParseStatus::OutOfRange,
        ),
        (
            "glcfg 60 50 50 50 50 50 50 50 50 50 50 50 x",
            ParseStatus::BadNumber,
        ),
        (
            "glcfg x 50 50 50 50 50 50 50 50 50 50 50 50",
            ParseStatus::BadNumber,
        ),
        (
            "glcfg 60 50 50 50 50 50 50 50 50 50 50 50 50 x",
            ParseStatus::BadNumber,
        ),
    ];
    for (line, want) in cases {
        assert_eq!(parse(line, &mut r), want, "{line}");
    }
    assert!(is_empty_reply(&r));
}

#[test]
fn slhbt_ok_and_error_forms() {
    let mut r = Reply::default();
    dirty(&mut r);
    assert_eq!(parse("slhbt 1 3540", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Slhbt);
    assert_eq!(r.heartbeat.lease, LeaseState::Running);
    assert_eq!(r.heartbeat.remain_s, 3540);
    assert!(!r.ack.error);
    assert_eq!(parse("slhbt 2 0 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.heartbeat.lease, LeaseState::Expired);
    assert_eq!(parse("slhbt 0 4294967295", &mut r), ParseStatus::Ok);
    assert_eq!(r.heartbeat.lease, LeaseState::Off);
    assert_eq!(r.heartbeat.remain_s, 4_294_967_295);
    assert_eq!(parse("slhbt 1 5 7", &mut r), ParseStatus::Ok); // extra field
    assert_eq!(r.heartbeat.remain_s, 5);
    assert_eq!(parse("slhbt err", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Slhbt);
    assert!(r.ack.error);
    assert_eq!(r.heartbeat.lease, LeaseState::Off);
    let cases = [
        ("slhbt ok", ParseStatus::BadFormat),
        ("slhbt 1", ParseStatus::BadFormat),
        ("slhbt", ParseStatus::BadArgCount),
        ("slhbt 3 0", ParseStatus::OutOfRange),
        ("slhbt 1 4294967296", ParseStatus::OutOfRange),
        ("slhbt 1 5 x", ParseStatus::BadNumber),
        ("slhbt err 1", ParseStatus::BadNumber),
    ];
    for (line, want) in cases {
        assert_eq!(parse(line, &mut r), want, "{line}");
    }
}

#[test]
fn slcfg_and_ssafe_ok_err_forms() {
    for c in ["slcfg", "ssafe"] {
        let mut r = Reply::default();
        assert_eq!(parse(&format!("{c} ok"), &mut r), ParseStatus::Ok, "{c}");
        assert_eq!(r.cmd, cmd_from_name(c.as_bytes()));
        assert!(!r.ack.error);
        assert_eq!(parse(&format!("{c} err "), &mut r), ParseStatus::Ok, "{c}");
        assert!(r.ack.error);
        assert_eq!(parse(c, &mut r), ParseStatus::BadArgCount, "{c}");
        assert_eq!(
            parse(&format!("{c} ok 1"), &mut r),
            ParseStatus::BadArgCount,
            "{c}"
        );
        assert_eq!(
            parse(&format!("{c} fine"), &mut r),
            ParseStatus::BadFormat,
            "{c}"
        );
    }
}

#[test]
fn sfspo_and_sstop_indexed_results() {
    for c in ["sfspo", "sstop"] {
        let is_stop = c == "sstop";
        let mut r = Reply::default();
        let res = |r: &Reply| if is_stop { r.stop } else { r.failsafe };
        assert_eq!(parse(&format!("{c} 3 ok"), &mut r), ParseStatus::Ok, "{c}");
        assert_eq!(r.cmd, cmd_from_name(c.as_bytes()));
        assert_eq!(res(&r).index, 3);
        assert!(res(&r).ok);
        assert_eq!(res(&r).error_code, 0);
        assert_eq!(parse(&format!("{c} 0 ok"), &mut r), ParseStatus::Ok, "{c}");
        assert_eq!(res(&r).index, 0);
        assert_eq!(parse(&format!("{c} 11 ok"), &mut r), ParseStatus::Ok, "{c}");
        assert_eq!(res(&r).index, 11);
        assert_eq!(
            parse(&format!("{c} 255 ok "), &mut r),
            ParseStatus::Ok,
            "{c}"
        );
        assert_eq!(res(&r).index, 255);
        assert!(res(&r).ok);
        assert_eq!(
            parse(&format!("{c} -1 err 1"), &mut r),
            ParseStatus::Ok,
            "{c}"
        );
        assert_eq!(res(&r).index, -1);
        assert!(!res(&r).ok);
        assert_eq!(res(&r).error_code, 1);
        assert_eq!(
            parse(&format!("{c} 7 err 65535"), &mut r),
            ParseStatus::Ok,
            "{c}"
        );
        assert_eq!(res(&r).index, 7);
        assert_eq!(res(&r).error_code, 65535);
        let cases = [
            ("-1 ok", ParseStatus::BadFormat),
            ("12 ok", ParseStatus::OutOfRange),
            ("254 ok", ParseStatus::OutOfRange),
            ("256 ok", ParseStatus::OutOfRange),
            ("-2 err 1", ParseStatus::OutOfRange),
            ("3 err 65536", ParseStatus::OutOfRange),
            ("3 err", ParseStatus::BadFormat),
            ("3 ok 1", ParseStatus::BadFormat),
            ("3", ParseStatus::BadArgCount),
            ("3 err 1 2", ParseStatus::BadArgCount),
            ("x ok", ParseStatus::BadNumber),
        ];
        for (rest, want) in cases {
            assert_eq!(parse(&format!("{c} {rest}"), &mut r), want, "{c} {rest}");
        }
        assert!(is_empty_reply(&r));
    }
}

#[test]
fn svmov_signed_index() {
    let mut r = Reply::default();
    assert_eq!(parse("svmov -1 err 1", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Svmov);
    assert_eq!(r.service_move.index, -1);
    assert!(!r.service_move.ok);
    assert_eq!(r.service_move.error_code, 1);
    assert_eq!(parse("svmov 3 ok", &mut r), ParseStatus::Ok);
    assert_eq!(r.service_move.index, 3);
    assert_eq!(parse("svmov 0 ok", &mut r), ParseStatus::Ok);
    assert_eq!(r.service_move.index, 0);
    assert!(r.service_move.ok);
    assert_eq!(parse("svmov -2 err 1", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("svmov 12 ok", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("svmov 255 ok", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("svmov -1 ok", &mut r), ParseStatus::BadFormat);
    assert_eq!(parse("sfspo 255 ok", &mut r), ParseStatus::Ok);
    assert_eq!(r.failsafe.index, 255);
    assert_eq!(parse("sfspo 12 ok", &mut r), ParseStatus::OutOfRange);
}

#[test]
fn gtlnt_and_stlnt() {
    let mut r = Reply::default();
    dirty(&mut r);
    assert_eq!(parse("gtlnt 604800", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Gtlnt);
    assert_eq!(r.learn_time, 604_800);
    assert_eq!(parse("gtlnt 0 ", &mut r), ParseStatus::Ok);
    assert_eq!(r.learn_time, 0);
    assert_eq!(parse("gtlnt 4294967295", &mut r), ParseStatus::Ok);
    assert_eq!(r.learn_time, 4_294_967_295);
    assert_eq!(parse("gtlnt 4294967296", &mut r), ParseStatus::OutOfRange);
    assert_eq!(parse("gtlnt", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("gtlnt 1 2", &mut r), ParseStatus::BadArgCount);
    assert_eq!(parse("stlnt", &mut r), ParseStatus::Ok);
    assert_eq!(r.cmd, Cmd::Stlnt);
    assert!(!r.ack.error);
    assert_eq!(parse("stlnt ", &mut r), ParseStatus::Ok);
    assert_eq!(parse("stlnt 5", &mut r), ParseStatus::BadArgCount);
}

#[test]
fn v3_flag_fault_and_configuration_flag_names() {
    let flags = [
        STM_FLAG_FS_LEASE,
        STM_FLAG_FS_BLOCKED,
        STM_FLAG_UNCALIBRATED,
        STM_FLAG_NEEDS_REF,
        STM_FLAG_RECAL,
        STM_FLAG_CAL_RESTORED,
        STM_FLAG_RETRY,
        STM_FLAG_EARLY_PENDING,
        STM_FLAG_ASSEMBLY,
        STM_FLAG_SVC_HOLD,
    ];
    let flag_names = [
        "fsLease",
        "fsBlocked",
        "uncalibrated",
        "needsRef",
        "recal",
        "calRestored",
        "retry",
        "earlyPending",
        "assembly",
        "svcHold",
    ];
    for b in 0..10u8 {
        assert_eq!(flags[usize::from(b)], 1u16 << b, "{b}");
        assert_eq!(stm_flag_name(b), flag_names[usize::from(b)], "{b}");
    }
    for b in [10, 11, 15, 16, 255] {
        assert_eq!(stm_flag_name(b), "", "{b}");
    }

    let faults = [
        "none",
        "move_timeout",
        "stroke_timeout",
        "short",
        "strokes_too_short",
        "inrush_trip",
    ];
    for f in 0..6u8 {
        assert_eq!(valve_fault_name(f), faults[usize::from(f)]);
    }
    assert_eq!(ValveFault::InrushTrip as u8, 5);
    assert_eq!(ValveFault::StrokesTooShort as u8, 4);
    for f in [6, 7, 255] {
        assert_eq!(valve_fault_name(f), "unknown");
    }

    let cfg = [
        STM_CFG_LAYOUT_CRC,
        STM_CFG_SHADOW_MISSING,
        STM_CFG_SETTINGS_CORRUPT,
        STM_CFG_SAFETY_CORRUPT,
        STM_CFG_SENSOR_SLOT,
        STM_CFG_CALIB,
        STM_CFG_UNVERIFIED,
        STM_CFG_READ_FAILED,
    ];
    let cfg_names = [
        "layoutCrc",
        "shadowMissing",
        "settingsCorrupt",
        "safetyCorrupt",
        "sensorSlot",
        "calib",
        "unverified",
        "readFailed",
    ];
    for b in 0..8u8 {
        assert_eq!(u32::from(cfg[usize::from(b)]), 1u32 << b, "{b}");
        assert_eq!(stm_cfg_flag_name(b), cfg_names[usize::from(b)], "{b}");
    }
    for b in [8, 9, 255] {
        assert_eq!(stm_cfg_flag_name(b), "");
    }
    assert_eq!(STM_SYS_PROTECT_SUSPENDED, 0x01);
}

// ================================================================ matching

fn parsed(s: &str) -> Reply {
    let mut r = Reply::default();
    assert_eq!(parse(s, &mut r), ParseStatus::Ok, "{s}");
    r
}

#[test]
fn reply_matches_by_command_and_valve() {
    let q = some(build_valve_data(3));
    assert!(reply_matches(
        &q,
        &parsed("gvlvd 3 42 18 1 215 -500 57 3120 3350 230 0")
    ));
    assert!(!reply_matches(
        &q,
        &parsed("gvlvd 4 42 18 1 215 -500 57 3120 3350 230 0")
    ));
    assert!(!reply_matches(&q, &parsed("gtgtp 3 50")));

    let q = some(build_valve_ex(4));
    assert!(reply_matches(&q, &parsed(&gvlvx_line())));
    let q = some(build_valve_ex(5));
    assert!(!reply_matches(&q, &parsed(&gvlvx_line())));

    let q = some(build_get_target(3));
    assert!(reply_matches(&q, &parsed("gtgtp 3 50")));
    assert!(!reply_matches(&q, &parsed("gtgtp 2 50")));

    let q = some(build_profile(3));
    assert!(reply_matches(&q, &parsed("gprof 3 0")));
    assert!(!reply_matches(&q, &parsed("gprof 2 0")));

    let q = some(build_service_move(3, MoveDir::Open, 10, 10));
    assert!(reply_matches(&q, &parsed("svmov 3 ok")));
    assert!(reply_matches(&q, &parsed("svmov 3 err 1")));
    assert!(!reply_matches(&q, &parsed("svmov 2 ok")));

    let zero = OneWireId::default();
    let q = some(build_set_valve_sensors(7, &zero, &zero));
    assert!(reply_matches(&q, &parsed("stvls 7")));
    assert!(!reply_matches(&q, &parsed("stvls 6")));

    let q = some(build_set_target(3, 50));
    assert!(reply_matches(&q, &parsed("stgtp")));
    assert!(!reply_matches(&q, &parsed("staln")));

    let q = some(build_calibrate(ALL_VALVES));
    assert!(reply_matches(&q, &parsed("staln")));
    let q = some(build_set_motor_chars(&MotorChars::default()));
    assert!(reply_matches(&q, &parsed("smotc")));
    assert!(reply_matches(&q, &parsed("smotc err")));
    let q = build_get_proto();
    assert!(reply_matches(&q, &parsed("gproto 2")));
    assert!(!reply_matches(&q, &parsed("gstat 1 2 3 4 5 6")));
}

#[test]
fn reply_matches_for_gvlon_forms_and_error() {
    let one = some(build_valve_sensors(3));
    let all = some(build_valve_sensors(ALL_VALVES));
    let single = parsed(&format!("gvlon 3 {ID1} {ID2}"));
    let other = parsed(&format!("gvlon 4 {ID1} {ID2}"));
    let list = parsed(&format!("gvlon 12 {}", id_list(ID1, 24, true)));
    let err = parsed("goned error");
    assert!(reply_matches(&one, &single));
    assert!(!reply_matches(&one, &other));
    assert!(!reply_matches(&one, &list));
    assert!(reply_matches(&one, &err));
    assert!(reply_matches(&all, &list));
    assert!(!reply_matches(&all, &single));
    assert!(reply_matches(&all, &err));
    let goned = some(build_temp_data(0));
    assert!(!reply_matches(&goned, &err)); // the error form belongs to gvlon
}

#[test]
fn reply_matches_for_sensor_count_list_data() {
    let count = build_temp_count();
    let list = build_temp_list();
    let vcount = build_volt_count();
    let vlist = build_volt_list();
    let data = some(build_temp_data(5));
    let vdata = some(build_volt_data(5));
    let c0 = parsed("gonec 0");
    let c3 = parsed("gonec 3");
    let l1 = parsed(&format!("gonec 1 {ID1}"));
    assert!(reply_matches(&count, &c0));
    assert!(reply_matches(&count, &c3));
    assert!(!reply_matches(&count, &l1));
    assert!(reply_matches(&list, &c0)); // empty list looks like the count reply
    assert!(!reply_matches(&list, &c3));
    assert!(reply_matches(&list, &l1));
    assert!(!reply_matches(&vlist, &l1));
    assert!(reply_matches(&vlist, &parsed(&format!("gowvc 1 {ID3}"))));
    assert!(reply_matches(&vlist, &parsed("gowvc 0")));
    assert!(!reply_matches(&vlist, &parsed("gowvc 2")));
    assert!(reply_matches(&vcount, &parsed("gowvc 2")));
    assert!(reply_matches(&data, &parsed(&format!("goned {ID1} 215"))));
    assert!(reply_matches(&data, &parsed("goned 0")));
    assert!(!reply_matches(&data, &parsed("gowvd 0")));
    assert!(reply_matches(&vdata, &parsed("gowvd 0")));
    assert!(reply_matches(&vdata, &parsed(&format!("gowvd {ID3} 12"))));
}

#[test]
fn reply_matches_with_an_expected_sensor_id() {
    let mut data = some(build_temp_data(4));
    let mut vdata = some(build_volt_data(4));
    let x = parsed(&format!("goned {ID1} 215"));
    let y = parsed(&format!("goned {ID2} 215"));
    let vx = parsed(&format!("gowvd {ID3} 12"));
    let vy = parsed(&format!("gowvd {ID1} 12"));
    // Expect zero (unknown id at that bus index): any reading matches.
    assert!(reply_matches(&data, &x));
    assert!(reply_matches(&data, &y));
    assert!(reply_matches(&vdata, &vx));
    assert!(reply_matches(&vdata, &vy));
    data.expect = id(ID1);
    vdata.expect = id(ID3);
    assert!(reply_matches(&data, &x));
    assert!(!reply_matches(&data, &y)); // a late reply for another bus index
    assert!(reply_matches(&data, &parsed("goned 0"))); // the invalid form: completes as rejected
    assert!(reply_matches(&vdata, &vx));
    assert!(!reply_matches(&vdata, &vy));
    assert!(reply_matches(&vdata, &parsed("gowvd 0")));
    assert!(!reply_matches(&data, &vx)); // other command
}

#[test]
fn reply_matches_for_v3_and_indexed_replies() {
    let q = some(build_valve_ex_v3(3));
    assert!(reply_matches(&q, &parsed(GVLVY_GOLDEN)));
    let q = some(build_valve_ex_v3(4));
    assert!(!reply_matches(&q, &parsed(GVLVY_GOLDEN)));
    let q = some(build_valve_ex(3));
    assert!(!reply_matches(&q, &parsed(GVLVY_GOLDEN))); // gvlvx request, gvlvy reply

    let q = some(build_service_move(3, MoveDir::Open, 10, 10));
    assert!(reply_matches(&q, &parsed("svmov -1 err 1"))); // the STM could not read the index
    assert!(!reply_matches(&q, &parsed("svmov 4 ok")));
    assert!(!reply_matches(&q, &parsed("svmov 0 ok")));

    let q = some(build_set_failsafe(2, 50));
    assert!(reply_matches(&q, &parsed("sfspo 2 ok")));
    assert!(reply_matches(&q, &parsed("sfspo 2 err 1")));
    assert!(reply_matches(&q, &parsed("sfspo -1 err 1")));
    assert!(!reply_matches(&q, &parsed("sfspo 3 ok")));
    assert!(!reply_matches(&q, &parsed("sfspo 0 ok")));
    assert!(!reply_matches(&q, &parsed("sfspo 255 ok")));
    assert!(!reply_matches(&q, &parsed("sstop 2 ok")));
    let q = some(build_set_failsafe(ALL_VALVES, 50));
    assert!(reply_matches(&q, &parsed("sfspo 255 ok")));
    assert!(!reply_matches(&q, &parsed("sfspo 2 ok")));

    let q = some(build_stop(5));
    assert!(reply_matches(&q, &parsed("sstop 5 ok")));
    assert!(reply_matches(&q, &parsed("sstop -1 err 1")));
    assert!(!reply_matches(&q, &parsed("sstop 6 ok")));
    assert!(!reply_matches(&q, &parsed("sstop 0 ok")));
    let q = some(build_stop(ALL_VALVES));
    assert!(reply_matches(&q, &parsed("sstop 255 ok")));

    let q = build_get_status_v3();
    assert!(reply_matches(&q, &parsed(GSTAX_GOLDEN)));
    assert!(!reply_matches(&q, &parsed("gstat 1 2 3 4 5 6")));
    let q = build_heartbeat(true);
    assert!(reply_matches(&q, &parsed("slhbt 1 3540")));
    assert!(reply_matches(&q, &parsed("slhbt err")));
    let q = build_get_lease_config();
    assert!(reply_matches(
        &q,
        &parsed("glcfg 60 50 50 50 50 50 50 50 50 50 50 50 50")
    ));
    let q = some(build_set_lease_timeout(60));
    assert!(reply_matches(&q, &parsed("slcfg ok")));
    let q = build_get_learn_time();
    assert!(reply_matches(&q, &parsed("gtlnt 0")));
    let q = build_leave_safe_mode();
    assert!(reply_matches(&q, &parsed("ssafe err")));
    let q = build_set_learn_time(0);
    assert!(reply_matches(&q, &parsed("stlnt")));
}

#[test]
fn reply_matches_rejects_empty_requests_and_replies() {
    let none = RequestLine::default();
    let empty = Reply::default();
    assert!(!reply_matches(&none, &empty));
    let q = build_get_status();
    assert!(!reply_matches(&q, &empty));
    let mut no_len = q.clone();
    no_len.text.clear();
    assert!(!reply_matches(&no_len, &parsed("gstat 1 2 3 4 5 6")));
    assert!(reply_matches(&q, &parsed("gstat 1 2 3 4 5 6")));
}

#[test]
fn every_request_built_has_a_matching_golden_reply() {
    let b = Breakaway::default();
    let pairs = [
        (some(build_set_target(1, 2)), "stgtp"),
        (build_scan_one_wire(), "stons"),
        (build_match_sensors(), "masns "),
        (some(build_assembly(255)), "staop "),
        (build_detect(), "stdet "),
        (some(build_set_learn_movements(2000)), "stlnm"),
        (build_get_learn_movements(), "gtlnm 2000 "),
        (build_get_motor_chars(), "gmotc 17 17 50 3000 0 "),
        (build_get_version(), "gvers 1.4.9_C2 1 "),
        (build_get_hw_id(), "ghwin 1073 "),
        (build_eeprom_state(), "eepst 1 "),
        (build_soft_reset(), "reset "),
        (build_valve_states(), "gvlst 12 8,6,6,8,6,6,6,6,6,6,6,6, "),
        (build_get_breakaway(), "gcalx 1 10 40"),
        (some(build_set_breakaway(&b)), "scalx ok"),
        (some(build_valve_ex_v3(3)), GVLVY_GOLDEN),
        (build_get_status_v3(), GSTAX_GOLDEN),
        (build_heartbeat(false), "slhbt 2 0"),
        (some(build_set_lease_timeout(60)), "slcfg ok"),
        (some(build_set_failsafe(3, 50)), "sfspo 3 ok"),
        (
            build_get_lease_config(),
            "glcfg 60 50 50 50 50 50 50 50 50 50 50 50 50",
        ),
        (some(build_stop(255)), "sstop 255 ok"),
        (build_get_learn_time(), "gtlnt 604800"),
        (build_leave_safe_mode(), "ssafe ok"),
        (build_set_learn_time(0), "stlnt"),
    ];
    for (q, rep) in &pairs {
        assert!(reply_matches(q, &parsed(rep)), "{rep}");
    }
}

// ================================================================ helpers

#[test]
fn resolve_temp_slot_cases() {
    let mut slots = [OneWireId::default(); TEMP_SLOT_COUNT as usize];
    slots[4] = id(ID1);
    slots[33] = id(ID2);
    assert_eq!(resolve_temp_slot(&id(ID1), &slots), 5);
    assert_eq!(resolve_temp_slot(&id(ID2), &slots), 34);
    assert_eq!(resolve_temp_slot(&id(ID2), &slots[..33]), 0); // beyond slotCount
    assert_eq!(resolve_temp_slot(&id(ID3), &slots), 0);
    assert_eq!(resolve_temp_slot(&OneWireId::default(), &slots), 0); // zero never matches empty
    let mut bad_crc = id(ID1);
    bad_crc.b[7] ^= 0x80;
    slots[0] = bad_crc;
    assert_eq!(resolve_temp_slot(&bad_crc, &slots), 0);
    // C++ resolveTempSlot(id, nullptr, kTempSlotCount) == 0: no Rust form.
    assert_eq!(resolve_temp_slot(&id(ID1), &slots[..0]), 0);
    slots[1] = id(ID1); // duplicate: first slot wins
    assert_eq!(resolve_temp_slot(&id(ID1), &slots), 2);
}

#[test]
fn stm_chip_name_cases() {
    assert_eq!(stm_chip_name(0x413), "STM32F40xx/41xx");
    assert_eq!(stm_chip_name(0x423), "STM32F401xB/C");
    assert_eq!(stm_chip_name(0x431), "STM32F411xx");
    assert_eq!(stm_chip_name(0x433), "STM32F401xD/E");
    assert_eq!(stm_chip_name(0), "Unknown Chip");
    assert_eq!(stm_chip_name(0x432), "Unknown Chip");
    assert_eq!(stm_chip_name(0xFFFF), "Unknown Chip");
}

// ================================================================ fuzz

/// The xorshift32 generator of the C++ fuzz tests (local to test_stm_codec.cpp).
struct XorShift(u32);

impl XorShift {
    fn next(&mut self) -> u32 {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.0 = s;
        s
    }

    /// `rng.next() % n` as an index.
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u32) as usize
    }
}

fn check_invariants(line: &[u8], st: ParseStatus, r: &Reply) {
    let l = line.escape_ascii();
    assert!((st as u8) <= 8, "{l}");
    if st != ParseStatus::Ok {
        assert!(is_empty_reply(r), "{l}");
        return;
    }
    assert!(r.cmd != Cmd::None, "{l}");
    assert!(r.valve_data.valve < VALVE_COUNT, "{l}");
    assert!(r.valve_data.position <= 100, "{l}");
    assert!(r.valve_data.status <= 0x7F, "{l}");
    assert!(r.valve_ex.valve < VALVE_COUNT, "{l}");
    assert!(r.valve_ex.position <= 100, "{l}");
    assert!(r.valve_ex.target <= 100, "{l}");
    assert!(r.valve_ex.cal_state <= CAL_STATE_MASK, "{l}");
    assert_eq!(r.valve_ex.cal_flags & !CAL_FLAG_MASK, 0, "{l}");
    assert!(r.valve_ex.last_move.stop as u8 <= 7, "{l}");
    assert!(r.valve_ex.last_move.dir as u8 <= 1, "{l}");
    assert!(r.target.valve < VALVE_COUNT, "{l}");
    assert!(r.target.target <= 100, "{l}");
    assert!(r.one_wire_list.count <= TEMP_SLOT_COUNT, "{l}");
    assert!(r.profile.count <= PROFILE_MAX_SAMPLES, "{l}");
    assert!(r.profile.valve < VALVE_COUNT, "{l}");
    assert!(r.service_move.index >= -1, "{l}");
    assert!(r.service_move.index < i16::from(VALVE_COUNT), "{l}");
    assert!(!(r.service_move.ok && r.service_move.index < 0), "{l}");
    for x in [&r.failsafe, &r.stop] {
        assert!(
            x.index >= -1 && (x.index < i16::from(VALVE_COUNT) || x.index == i16::from(ALL_VALVES)),
            "{l}"
        );
        assert!(!(x.ok && x.index < 0), "{l}");
    }
    assert!(failsafe_pct_valid(u32::from(r.valve_ex.fs_pct)), "{l}");
    assert!(r.valve_ex.drive <= 100, "{l}");
    assert!(r.status.lease as u8 <= 2, "{l}");
    assert!(r.status.lease_timeout_min <= 1440, "{l}");
    assert!(r.status.failsafe_mask <= 0x0FFF, "{l}");
    assert!(r.heartbeat.lease as u8 <= 2, "{l}");
    assert!(r.lease_config.timeout_min <= 1440, "{l}");
    for pct in r.lease_config.failsafe_pct {
        assert!(failsafe_pct_valid(u32::from(pct)), "{l}");
    }
    if r.valve_ex.v3 {
        assert_eq!(r.cmd, Cmd::Gvlvy, "{l}");
    }
    if r.status.v3 {
        assert_eq!(r.cmd, Cmd::Gstax, "{l}");
    }
    assert!(r.valve_sensors.valve < VALVE_COUNT, "{l}");
    assert!(r.hw_id <= 0xFFF, "{l}");
    assert!(r.breakaway.step_pct <= 100, "{l}");
    assert!(r.breakaway.max_ma >= 20, "{l}");
    assert!(r.breakaway.max_ma <= 60, "{l}");
    assert!(r.ack.valve < VALVE_COUNT || r.ack.valve == NO_VALVE, "{l}");
    if r.cmd == Cmd::Gowvc {
        assert!(r.one_wire_list.count <= VOLT_SLOT_COUNT, "{l}");
    }
    if r.gvlon_error {
        assert_eq!(r.cmd, Cmd::Gvlon, "{l}");
    }
}

#[test]
fn fuzz_with_random_bytes() {
    let mut rng = XorShift(0x00C0_FFEE);
    let mut r = Reply::default();
    // Bias towards the protocol alphabet so deeper paths are reached.
    const ALPHA: &[u8] = b"0123456789 ,:-abcdefglnoprstvwx";
    for iter in 0..20000 {
        let len = rng.next() % 96;
        let mut line = Vec::new();
        for _ in 0..len {
            let k = rng.next() % 10;
            line.push(if k < 7 {
                ALPHA[rng.below(ALPHA.len())]
            } else {
                (rng.next() & 0xFF) as u8
            });
        }
        if iter % 3 == 0 {
            let golden = STM_GOLDEN[rng.below(STM_GOLDEN.len())].as_bytes();
            let mut prefixed = golden[..5].to_vec();
            prefixed.extend_from_slice(&line);
            line = prefixed;
        }
        dirty(&mut r);
        let st = parse_reply(&line, &mut r);
        check_invariants(&line, st, &r);
    }
}

#[test]
fn fuzz_by_mutating_golden_replies() {
    let mut rng = XorShift(12345);
    let mut r = Reply::default();
    let mut ok = 0;
    let mut rejected = 0;
    for g in STM_GOLDEN {
        assert_eq!(parse(g, &mut r), ParseStatus::Ok, "{g}");
    }
    const ALPHA: &[u8] = b"0123456789 ,:-aefx";
    for _ in 0..40000 {
        let mut line = STM_GOLDEN[rng.below(STM_GOLDEN.len())].as_bytes().to_vec();
        let edits = 1 + rng.next() % 3;
        let mut e = 0;
        while e < edits && !line.is_empty() {
            let pos = rng.below(line.len());
            let c = ALPHA[rng.below(ALPHA.len())];
            match rng.next() % 4 {
                0 => line[pos] = c,
                1 => line.insert(pos, c),
                2 => {
                    line.remove(pos);
                }
                _ => {
                    let n = rng.below(8).min(line.len() - pos);
                    let copy = line[pos..pos + n].to_vec();
                    line.splice(pos..pos, copy);
                }
            }
            e += 1;
        }
        dirty(&mut r);
        let st = parse_reply(&line, &mut r);
        check_invariants(&line, st, &r);
        if st == ParseStatus::Ok {
            ok += 1;
        } else {
            rejected += 1;
        }
    }
    assert!(ok > 1000, "{ok}");
    assert!(rejected > 1000, "{rejected}");
}

// ================================================================ lower bounds

#[test]
fn every_numeric_field_accepts_its_minimum() {
    let mut r = Reply::default();
    assert_eq!(
        parse("gvlvd 0 0 0 0 0 0 0 0 0 0 0", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.valve_data.status, 0);
    assert!(!r.valve_data.calibrating);
    assert_eq!(r.valve_data.temp1, 0);
    assert_eq!(
        parse("gvlvx 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.valve_ex.last_move.stop, StopReason::None);
    assert_eq!(
        parse(
            "gvlvy 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0",
            &mut r
        ),
        ParseStatus::Ok
    );
    assert_eq!(r.valve_ex.flags, 0);
    assert_eq!(r.valve_ex.fault, 0);
    assert_eq!(r.valve_ex.retry_s, 0);
    assert_eq!(parse("gstat 0 0 0 0 0 0", &mut r), ParseStatus::Ok);
    assert_eq!(r.status.uptime_s, 0);
    assert_eq!(
        parse(
            "gstax 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0",
            &mut r
        ),
        ParseStatus::Ok
    );
    assert_eq!(r.status.lease_remain_s, 0);
    assert_eq!(r.status.lease_timeout_min, 0);
    assert_eq!(r.status.eep_writes, 0);
    assert_eq!(r.status.temp_age_s, 0);
    assert_eq!(r.status.ow_scan_age_s, 0);
    assert_eq!(parse("gmotc 0 0 0 0 0", &mut r), ParseStatus::Ok);
    assert_eq!(r.motor_chars.low_factor, 0);
    assert_eq!(r.motor_chars.min_counts, 0);
    assert_eq!(parse("gcalx 0 0 20", &mut r), ParseStatus::Ok);
    assert_eq!(parse("gvers 1.4.9 0", &mut r), ParseStatus::Ok);
    assert_eq!(r.build, 0);
    assert_eq!(parse("svmov 0 err 0", &mut r), ParseStatus::Ok);
    assert_eq!(r.service_move.error_code, 0);
    assert!(!r.service_move.ok);
    assert_eq!(parse("gprof 0 1 0:0", &mut r), ParseStatus::Ok);
    assert_eq!(r.profile.count, 1);
    assert_eq!(
        parse(&format!("gvlon 0 {ID1} {ID2}"), &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.valve_sensors.valve, 0);
    assert_eq!(r.valve_sensors.ids[0][0], id(ID1));
    assert_eq!(r.valve_sensors.ids[0][1], id(ID2));
    assert_eq!(
        parse("gvlst 12 0,0,0,0,0,0,0,0,0,0,0,0", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(parse("stvls 0", &mut r), ParseStatus::Ok);
    assert_eq!(parse("gtlnm 0", &mut r), ParseStatus::Ok);
}

#[test]
fn signed_fields() {
    let mut r = Reply::default();
    assert_eq!(
        parse("goned 28-84-37-94-97-ff-03-23 -5", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.temp_data.value, -5);
    assert_eq!(
        parse("goned 28-84-37-94-97-ff-03-23 -0", &mut r),
        ParseStatus::Ok
    );
    assert_eq!(r.temp_data.value, 0);
    let cases = [
        ("goned 28-84-37-94-97-ff-03-23 -", ParseStatus::BadNumber),
        ("goned 28-84-37-94-97-ff-03-23 -5x", ParseStatus::BadNumber),
        ("goned 28-84-37-94-97-ff-03-23 -12x", ParseStatus::BadNumber),
        ("goned 28-84-37-94-97-ff-03-23 5-", ParseStatus::BadNumber),
        (
            "gowvd 26-11-22-33-44-55-66-29 -12345678901",
            ParseStatus::BadNumber,
        ),
        (
            "gowvd 26-11-22-33-44-55-66-29 -9999999999",
            ParseStatus::OutOfRange,
        ),
    ];
    for (line, want) in cases {
        assert_eq!(parse(line, &mut r), want, "{line}");
    }
}

#[test]
fn list_elements_beyond_the_count_are_rejected() {
    let mut r = Reply::default();
    assert_eq!(
        parse(&format!("gonec 1 {ID1},{ID2},"), &mut r),
        ParseStatus::BadFormat
    );
    assert_eq!(
        parse("gvlst 12 1,1,1,1,1,1,1,1,1,1,1,1,1,", &mut r),
        ParseStatus::BadFormat
    );
    assert_eq!(parse("gonec 1 ,", &mut r), ParseStatus::BadFormat);
    assert_eq!(parse("gonec 1 ,,", &mut r), ParseStatus::BadFormat);
}

#[test]
fn resolve_temp_slot_matches_the_first_slot() {
    let mut slots = [OneWireId::default(); 3];
    slots[0] = id(ID1);
    assert_eq!(resolve_temp_slot(&id(ID1), &slots), 1);
    assert_eq!(resolve_temp_slot(&id(ID1), &slots[..1]), 1);
}
