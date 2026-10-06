//! Port of test/native/test_mqtt_topics.cpp: MQTT topic tree, command parsing, payload
//! formatters and publish cadence. Golden strings follow the legacy byte format
//! (software_esp32/src/mqtt.cpp).
//!
//! C++ cases that pass a null pointer have no Rust form (a slice is never null); they are named
//! in comments, and where the C++ treats null like the empty input the empty slice is tested.
//! The C++ checks of the NUL a builder writes (`n == strlen(buf)`, `buf[0] == '\0'` after a
//! failure) are the returned lengths and the 0 results (docs/rust/PORTING.md); the bytes past a
//! capacity are still checked.

use super::*;
use crate::common::copy_string;
use crate::test_support::{assert_text, CRand};
use std::format;
use std::string::String;
use std::vec;
use std::vec::Vec;

fn ctx_of(station: &[u8], separate: bool, path_as_root: bool) -> TopicContext {
    let mut c = TopicContext::default();
    copy_string(&mut c.station, station);
    c.separate = separate;
    c.path_as_root = path_as_root;
    c
}

/// C++ `ctxOf(station)`: separate, no leading '/'.
fn ctx(station: &[u8]) -> TopicContext {
    ctx_of(station, true, false)
}

/// C++ `topic(c, t, seg)`; the C++ null segment is the empty one.
fn topic(c: &TopicContext, t: Topic, seg: &[u8]) -> Vec<u8> {
    let mut buf = [0u8; TOPIC_MAX + 1];
    let n = build_topic(c, t, seg, &mut buf);
    buf[..n].to_vec()
}

/// C++ `fillSegments(s, names)`: buildSegment() of every name (empty: the valve number).
fn fill_segments(names: &[&[u8]; 12]) -> Segments {
    core::array::from_fn(|i| {
        let mut buf = [0u8; SEGMENT_MAX + 1];
        let n = build_segment(names[i], i as u8, &mut buf);
        let mut s = Text::new();
        assert!(copy_string(&mut s, &buf[..n]));
        s
    })
}

fn seg_text(s: &[u8]) -> Text<SEGMENT_MAX> {
    let mut t = Text::new();
    assert!(copy_string(&mut t, s));
    t
}

fn fmt_temp(t: i32, valid: bool, german: bool) -> Vec<u8> {
    let mut buf = [0u8; 16];
    let n = format_temp(t, valid, german, &mut buf);
    buf[..n].to_vec()
}

fn fmt_volt(v: f64, valid: bool, german: bool) -> Vec<u8> {
    let mut buf = [0u8; 32];
    let n = format_volt(v, valid, german, &mut buf);
    buf[..n].to_vec()
}

#[test]
fn main_topic() {
    let mut buf = [0u8; TOPIC_MAX + 1];
    assert_eq!(build_main_topic(&ctx(b"VdMot"), &mut buf), 6);
    assert_text(&buf[..6], "VdMot/");
    assert_eq!(build_main_topic(&ctx_of(b"VdMot", true, true), &mut buf), 7);
    assert_text(&buf[..7], "/VdMot/");
    assert_eq!(build_main_topic(&ctx(b""), &mut buf), 9);
    assert_text(&buf[..9], "VdMotFBH/");
    assert_eq!(build_main_topic(&ctx_of(b"", true, true), &mut buf), 10);
    assert_text(&buf[..10], "/VdMotFBH/");
    assert_eq!(build_main_topic(&ctx(b"A"), &mut buf), 2);
    assert_text(&buf[..2], "A/");
    let n = build_main_topic(&ctx(b"My Station"), &mut buf);
    assert_text(&buf[..n], "My Station/"); // raw, like legacy
    let n = build_main_topic(&ctx(b"abcdefghijklmnopqrst"), &mut buf);
    assert_text(&buf[..n], "abcdefghijklmnopqrst/");
    let bad: [&[u8]; 7] = [b"a+b", b"a#", b"a/b", b"q\"", b"b\\", b"a\xc3", b"\x80"];
    for b in bad {
        assert_eq!(
            build_main_topic(&ctx(b), &mut buf),
            0,
            "{}",
            b.escape_ascii()
        );
    }
    // Raw like legacy: UTF-8 and spaces at either end.
    let n = build_main_topic(&ctx(b"Fu\xc3\x9fboden "), &mut buf);
    assert_text(&buf[..n], b"Fu\xc3\x9fboden /");
    // C++ a station array without a NUL (21 bytes 'x'): no Rust form, a Text<20> holds at most
    // 20 bytes. A NUL inside ends the station like in the C++ array.
    let mut cut = TopicContext::default();
    cut.station.extend_from_slice(b"ab\0cd").unwrap();
    let n = build_main_topic(&cut, &mut buf);
    assert_text(&buf[..n], "ab/");
    // Capacity: exactly fits / one short.
    let mut seven = [0u8; 7];
    assert_eq!(build_main_topic(&ctx(b"VdMot"), &mut seven), 6);
    let mut six = [0u8; 6];
    assert_eq!(build_main_topic(&ctx(b"VdMot"), &mut six), 0);
    // C++ a null output: no Rust form.
    assert_eq!(build_main_topic(&ctx(b"VdMot"), &mut buf[..0]), 0);
}

#[test]
fn segments() {
    let mut buf = [0u8; SEGMENT_MAX + 1];
    assert_eq!(build_segment(b"Bad 1", 0, &mut buf), 5);
    assert_text(&buf[..5], "Bad_1");
    assert_eq!(build_segment(b"a b c", 0, &mut buf), 5);
    assert_text(&buf[..5], "a_b_c");
    assert_eq!(build_segment(b"", 2, &mut buf), 1);
    assert_text(&buf[..1], "3");
    // C++ buildSegment(nullptr, 0, ..) gives "1" like the empty name.
    assert_eq!(build_segment(b"", 0, &mut buf), 1);
    assert_text(&buf[..1], "1");
    assert_eq!(build_segment(b"", 33, &mut buf), 2);
    assert_text(&buf[..2], "34");
    assert_eq!(build_segment(b"", 255, &mut buf), 3);
    assert_text(&buf[..3], "256");
    assert_eq!(build_segment(b"0123456789", 0, &mut buf), 10);
    assert_eq!(build_segment(b"0123456789a", 0, &mut buf), 0);
    let bad: [&[u8]; 8] = [
        b"a/b",
        b"a+",
        b"#",
        b"x\"",
        b"x\\",
        b"\x7f",
        b"\xc3",
        b"\xed\xa0\x80",
    ];
    for b in bad {
        assert_eq!(build_segment(b, 0, &mut buf), 0, "{}", b.escape_ascii());
    }
    // Legacy mapping: spaces (also at either end) become '_', UTF-8 stays.
    assert_eq!(build_segment(b"Bad ", 0, &mut buf), 4);
    assert_text(&buf[..4], "Bad_");
    assert_eq!(build_segment(b" K\xc3\xbcche", 0, &mut buf), 7);
    assert_text(&buf[..7], b"_K\xc3\xbcche");
    // A NUL ends the name like in a C string.
    assert_eq!(build_segment(b"ab\0/", 0, &mut buf), 2);
    assert_text(&buf[..2], "ab");
    let mut small = [0u8; 3];
    assert_eq!(build_segment(b"abc", 0, &mut small), 0);
    assert_eq!(build_segment(b"ab", 0, &mut small), 2);
    assert_eq!(build_segment(b"", 99, &mut small), 0);
    assert_eq!(build_segment(b"", 8, &mut small), 1);
    // C++ a null output: no Rust form.
    assert_eq!(build_segment(b"ab", 0, &mut small[..0]), 0);
}

#[test]
fn topic_segments_slash_only_between_two_non_empty_parts() {
    let ok: [&[u8]; 7] = [
        b"a",
        b"Bad/WC",
        b"a/b/c",
        b"0123456789",
        b"x y",
        b"K\xc3\xbcche",
        b"/",
    ];
    for s in ok {
        assert_eq!(topic_segment_valid(s), s != b"/", "{}", s.escape_ascii());
    }
    let bad: [&[u8]; 7] = [b"/a", b"a/", b"a//b", b"a+", b"#", b"01234567890", b"a/b/"];
    for s in bad {
        assert!(!topic_segment_valid(s), "{}", s.escape_ascii());
    }
    assert!(!topic_segment_valid(b""));
    // C++ topicSegmentValid(nullptr, 1): no Rust form.
    assert!(!topic_segment_valid(b"a\0b"));
    assert!(topic_segment_valid(&b"ab"[..1])); // len is authoritative
}

struct Row {
    t: Topic,
    seg: &'static [u8],
    separate: &'static str,
    /// None: no suffix rule (same as separate)
    plain: Option<&'static str>,
}

const fn row(t: Topic, seg: &'static [u8], separate: &'static str, plain: &'static str) -> Row {
    Row {
        t,
        seg,
        separate,
        plain: Some(plain),
    }
}

const fn new_row(t: Topic, seg: &'static [u8], separate: &'static str) -> Row {
    Row {
        t,
        seg,
        separate,
        plain: None,
    }
}

#[test]
fn compat_and_new_topics_byte_exact() {
    let s = ctx(b"VdMot");
    let p = ctx_of(b"VdMot", false, false);
    let rows: [Row; TOPIC_COUNT as usize] = [
        row(
            Topic::CommonIp,
            b"",
            "VdMot/common/ip/value",
            "VdMot/common/ip",
        ),
        row(
            Topic::CommonState,
            b"",
            "VdMot/common/state/value",
            "VdMot/common/state",
        ),
        row(
            Topic::CommonUptime,
            b"",
            "VdMot/common/uptime/value",
            "VdMot/common/uptime",
        ),
        row(
            Topic::CommonMessage,
            b"",
            "VdMot/common/message/value",
            "VdMot/common/message",
        ),
        row(
            Topic::ValveTarget,
            b"Bad_1",
            "VdMot/valves/Bad_1/target/value",
            "VdMot/valves/Bad_1/target",
        ),
        row(
            Topic::ValveState,
            b"3",
            "VdMot/valves/3/state/value",
            "VdMot/valves/3/state",
        ),
        row(
            Topic::ValveCalibDate,
            b"3",
            "VdMot/valves/3/calibration/date/value",
            "VdMot/valves/3/calibration/date",
        ),
        row(
            Topic::ValveCalibRepetitions,
            b"3",
            "VdMot/valves/3/calibration/repetitions/value",
            "VdMot/valves/3/calibration/repetitions",
        ),
        row(
            Topic::ValveMeanCurrent,
            b"3",
            "VdMot/valves/3/diag/meanCurrrent/value",
            "VdMot/valves/3/diag/meanCurrrent",
        ),
        row(
            Topic::ValveOpenCount,
            b"3",
            "VdMot/valves/3/diag/openCount/value",
            "VdMot/valves/3/diag/openCount",
        ),
        row(
            Topic::ValveCloseCount,
            b"3",
            "VdMot/valves/3/diag/closeCount/value",
            "VdMot/valves/3/diag/closeCount",
        ),
        row(
            Topic::ValveDeadZoneCount,
            b"3",
            "VdMot/valves/3/diag/deadZoneCount/value",
            "VdMot/valves/3/diag/deadZoneCount",
        ),
        row(
            Topic::ValveMoves,
            b"3",
            "VdMot/valves/3/diag/moves/value",
            "VdMot/valves/3/diag/moves",
        ),
        row(
            Topic::ValveTemp1,
            b"3",
            "VdMot/valves/3/temp1/value",
            "VdMot/valves/3/temp1",
        ),
        row(
            Topic::ValveTemp2,
            b"3",
            "VdMot/valves/3/temp2/value",
            "VdMot/valves/3/temp2",
        ),
        row(
            Topic::ValveActual,
            b"3",
            "VdMot/valves/3/actual/value",
            "VdMot/valves/3/actual",
        ),
        row(
            Topic::TempId,
            b"T_1",
            "VdMot/temps/T_1/id/value",
            "VdMot/temps/T_1/id",
        ),
        row(
            Topic::TempValue,
            b"T_1",
            "VdMot/temps/T_1/value/value",
            "VdMot/temps/T_1/value",
        ),
        row(
            Topic::VoltId,
            b"5",
            "VdMot/sensors/5/id/value",
            "VdMot/sensors/5/id",
        ),
        row(
            Topic::VoltValue,
            b"5",
            "VdMot/sensors/5/value/value",
            "VdMot/sensors/5/value",
        ),
        row(
            Topic::VoltUnit,
            b"5",
            "VdMot/sensors/5/unit/value",
            "VdMot/sensors/5/unit",
        ),
        new_row(
            Topic::DiagValveLastMove,
            b"3",
            "VdMot/diag/valves/3/lastMove",
        ),
        new_row(
            Topic::DiagValveEarlyStops,
            b"3",
            "VdMot/diag/valves/3/earlyStops",
        ),
        new_row(
            Topic::DiagValveCmdRejected,
            b"3",
            "VdMot/diag/valves/3/cmdRejected",
        ),
        new_row(
            Topic::DiagValveCalState,
            b"3",
            "VdMot/diag/valves/3/calState",
        ),
        new_row(Topic::DiagValveProfile, b"3", "VdMot/diag/valves/3/profile"),
        new_row(Topic::DiagStmProto, b"", "VdMot/diag/stm/proto"),
        new_row(Topic::DiagStmUptime, b"", "VdMot/diag/stm/uptime"),
        new_row(Topic::DiagStmResets, b"", "VdMot/diag/stm/resets"),
        new_row(Topic::DiagStmRxOverflow, b"", "VdMot/diag/stm/rxOverflow"),
        new_row(Topic::DiagStmParseErr, b"", "VdMot/diag/stm/parseErr"),
        new_row(Topic::DiagStmLink, b"", "VdMot/diag/stm/link"),
        new_row(
            Topic::DiagCalibrationActive,
            b"",
            "VdMot/diag/calibration/active",
        ),
        new_row(Topic::Events, b"", "VdMot/events"),
        new_row(Topic::Status, b"", "VdMot/status"),
        row(
            Topic::ValveRequested,
            b"3",
            "VdMot/valves/3/requested/value",
            "VdMot/valves/3/requested",
        ),
        row(
            Topic::ValveSync,
            b"3",
            "VdMot/valves/3/sync/value",
            "VdMot/valves/3/sync",
        ),
        row(
            Topic::ValveFailsafe,
            b"3",
            "VdMot/valves/3/failsafe/value",
            "VdMot/valves/3/failsafe",
        ),
        row(
            Topic::ValveProblem,
            b"3",
            "VdMot/valves/3/problem/value",
            "VdMot/valves/3/problem",
        ),
        new_row(Topic::StmStatus, b"", "VdMot/stm/status"),
        new_row(Topic::Failsafe, b"", "VdMot/failsafe"),
        new_row(Topic::DiagStmVersion, b"", "VdMot/diag/stm/version"),
        new_row(Topic::DiagStmStarted, b"", "VdMot/diag/stm/started"),
        new_row(Topic::DiagStmLease, b"", "VdMot/diag/stm/lease"),
        new_row(Topic::DiagStmSafeMode, b"", "VdMot/diag/stm/safeMode"),
        new_row(
            Topic::DiagMqttEventsSuppressed,
            b"",
            "VdMot/diag/mqtt/eventsSuppressed",
        ),
        new_row(
            Topic::DiagMqttCommandsRejected,
            b"",
            "VdMot/diag/mqtt/commandsRejected",
        ),
        new_row(
            Topic::DiagCalibrationNext,
            b"",
            "VdMot/diag/calibration/next",
        ),
        new_row(
            Topic::CmdValveCalibrate,
            b"Bad_1",
            "VdMot/cmd/valves/Bad_1/calibrate",
        ),
        new_row(Topic::CmdCalibrate, b"", "VdMot/cmd/calibrate"),
        new_row(Topic::CmdRestart, b"", "VdMot/cmd/restart"),
        new_row(Topic::CmdStmReset, b"", "VdMot/cmd/stmReset"),
        new_row(Topic::CmdDetect, b"", "VdMot/cmd/detect"),
        new_row(Topic::CmdStop, b"", "VdMot/cmd/stop"),
        new_row(Topic::CmdStmSafeExit, b"", "VdMot/cmd/stmSafeExit"),
    ];
    // every topic covered: the array type has TOPIC_COUNT entries
    for (i, r) in rows.iter().enumerate() {
        assert_eq!(r.t as usize, i, "{}", r.separate);
        assert_eq!(Topic::from_raw(i as u8), Some(r.t));
        assert_text(&topic(&s, r.t, r.seg), r.separate);
        assert_text(&topic(&p, r.t, r.seg), r.plain.unwrap_or(r.separate));
        assert_eq!(topic_is_compat(r.t), r.plain.is_some(), "{}", r.separate);
    }
    assert_text(
        &topic(&ctx_of(b"VdMot", true, true), Topic::ValveTarget, b"1"),
        "/VdMot/valves/1/target/value",
    );
    assert_text(&topic(&ctx(b""), Topic::Status, b""), "VdMotFBH/status");
    assert_text(
        &topic(&ctx(b"My St"), Topic::CommonIp, b""),
        "My St/common/ip/value",
    );
    // Non-item topics ignore the segment; topic overrides may span levels.
    assert_text(&topic(&s, Topic::Status, b"x"), "VdMot/status");
    assert_text(
        &topic(&s, Topic::ValveState, b"Bad/WC"),
        "VdMot/valves/Bad/WC/state/value",
    );
    assert_text(
        &topic(&ctx(b"VdMotFBH"), Topic::CmdValveCalibrate, b"Bad/WC"),
        "VdMotFBH/cmd/valves/Bad/WC/calibrate",
    );
}

#[test]
fn build_topic_rejects_bad_segments_topics_and_buffers() {
    let s = ctx(b"VdMot");
    let mut buf = [0u8; TOPIC_MAX + 1];
    // the empty segment is also the C++ null segment
    let bad: [&[u8]; 7] = [b"", b"/b", b"a/", b"a//b", b"a+", b"#", b"01234567890"];
    for b in bad {
        assert_eq!(
            build_topic(&s, Topic::ValveTarget, b, &mut buf),
            0,
            "{}",
            b.escape_ascii()
        );
    }
    assert!(build_topic(&s, Topic::ValveTarget, b"0123456789", &mut buf) > 0);
    // A NUL ends the segment like in a C string.
    let n = build_topic(&s, Topic::ValveTarget, b"7\0junk", &mut buf);
    assert_text(&buf[..n], "VdMot/valves/7/target/value");
    // C++ buildTopic(.., static_cast<Topic>(kTopicCount), ..) == 0: no Rust form, a Topic is
    // always in range.
    assert_eq!(Topic::from_raw(TOPIC_COUNT), None);
    assert_eq!(Topic::from_raw(255), None);
    assert_eq!(build_topic(&ctx(b"a+b"), Topic::Status, b"", &mut buf), 0);
    // "VdMot/status" is 12 chars.
    let mut thirteen = [0u8; 13];
    assert_eq!(build_topic(&s, Topic::Status, b"", &mut thirteen), 12);
    let mut twelve = [0u8; 12];
    assert_eq!(build_topic(&s, Topic::Status, b"", &mut twelve), 0);
    // The suffix must fit as well: "VdMot/common/ip/value" is 21 chars.
    let mut c21 = [0u8; 21];
    assert_eq!(build_topic(&s, Topic::CommonIp, b"", &mut c21), 0);
    let mut c22 = [0u8; 22];
    assert_eq!(build_topic(&s, Topic::CommonIp, b"", &mut c22), 21);
    // C++ a null output: no Rust form.
    assert_eq!(build_topic(&s, Topic::Status, b"", &mut buf[..0]), 0);
    // Longest possible topic fits in TOPIC_MAX.
    let longest = ctx_of(b"abcdefghijklmnopqrst", true, true);
    assert_text(
        &topic(&longest, Topic::ValveCalibRepetitions, b"0123456789"),
        "/abcdefghijklmnopqrst/valves/0123456789/calibration/repetitions/value",
    );
}

#[test]
fn retain_flags() {
    for i in 0..TOPIC_COUNT {
        let t = Topic::from_raw(i).unwrap();
        let always = matches!(
            t,
            Topic::Status | Topic::DiagCalibrationActive | Topic::StmStatus | Topic::Failsafe
        );
        let never = matches!(t, Topic::Events | Topic::DiagValveProfile)
            || (Topic::CmdValveCalibrate as u8..=Topic::CmdStmSafeExit as u8).contains(&i);
        if always {
            assert!(topic_retained(t, false), "{i}");
            assert!(topic_retained(t, true), "{i}");
        } else if never {
            assert!(!topic_retained(t, false), "{i}");
            assert!(!topic_retained(t, true), "{i}");
        } else {
            assert!(!topic_retained(t, false), "{i}");
            assert!(topic_retained(t, true), "{i}");
        }
    }
    // C++ topicRetained/topicIsCompat of static_cast<Topic>(kTopicCount) == false: no Rust form
    // (from_raw above).
}

#[test]
fn target_command_subscription_topic() {
    let mut buf = [0u8; TOPIC_MAX + 1];
    assert_eq!(
        build_target_command_topic(&ctx(b"VdMot"), b"Bad_1", &mut buf),
        29
    );
    assert_text(&buf[..29], "VdMot/valves/Bad_1/target/set");
    let n = build_target_command_topic(&ctx_of(b"VdMot", false, false), b"Bad_1", &mut buf);
    assert_text(&buf[..n], "VdMot/valves/Bad_1/target");
    let n = build_target_command_topic(&ctx_of(b"VdMot", true, true), b"2", &mut buf);
    assert_text(&buf[..n], "/VdMot/valves/2/target/set");
    let n = build_target_command_topic(&ctx(b"VdMot"), b"Bad/WC", &mut buf);
    assert_text(&buf[..n], "VdMot/valves/Bad/WC/target/set");
    assert_eq!(build_target_command_topic(&ctx(b"VdMot"), b"", &mut buf), 0);
    assert_eq!(
        build_target_command_topic(&ctx(b"VdMot"), b"a//b", &mut buf),
        0
    );
    let mut small = [0u8; 20];
    assert_eq!(
        build_target_command_topic(&ctx(b"VdMot"), b"1", &mut small),
        0
    );
    // C++ a null output: no Rust form.
}

#[test]
fn ha_status_topic() {
    let mut buf = [0u8; TOPIC_MAX + 1];
    assert_eq!(build_ha_status_topic(b"homeassistant", &mut buf), 20);
    assert_text(&buf[..20], "homeassistant/status");
    assert_eq!(build_ha_status_topic(b"ha/x", &mut buf), 11);
    assert_text(&buf[..11], "ha/x/status");
    // also the C++ null prefix
    assert_eq!(build_ha_status_topic(b"", &mut buf), 0);
    assert_eq!(build_ha_status_topic(b"\0ha", &mut buf), 0);
    // C++ a null output: no Rust form.
    let mut nine = [0u8; 9]; // "ha/status" needs 10
    assert_eq!(build_ha_status_topic(b"ha", &mut nine), 0);
    let mut ten = [0u8; 10];
    assert_eq!(build_ha_status_topic(b"ha", &mut ten), 9);
}

fn subs(
    c: &TopicContext,
    mode: MqttMode,
    prefix: &[u8],
    seg: Option<&Segments>,
    cap: usize,
) -> Vec<String> {
    let mut out: [Subscription; MAX_SUBSCRIPTIONS + 1] = Default::default();
    let n = build_subscriptions(c, mode, prefix, seg, &mut out[..cap]);
    out[..n]
        .iter()
        .map(|s| format!("{} q{}", s.filter.escape_ascii(), s.qos))
        .collect()
}

fn v(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| String::from(*s)).collect()
}

#[test]
fn subscriptions_per_mode_separate_and_prefix() {
    const ALL: usize = MAX_SUBSCRIPTIONS;
    let s = ctx(b"VdMot");
    assert_eq!(
        subs(&s, MqttMode::MqttHa, b"homeassistant", None, ALL),
        v(&[
            "VdMot/valves/+/target/set q1",
            "VdMot/valves/+/target/set/set q1",
            "VdMot/cmd/# q0",
            "homeassistant/status q1"
        ])
    );
    assert_eq!(
        subs(&s, MqttMode::MqttHa, b"ha", None, ALL),
        v(&[
            "VdMot/valves/+/target/set q1",
            "VdMot/valves/+/target/set/set q1",
            "VdMot/cmd/# q0",
            "homeassistant/status q1",
            "ha/status q1"
        ])
    );
    // C++ a null prefix: the empty prefix gives the same.
    assert_eq!(
        subs(&s, MqttMode::MqttHa, b"", None, ALL),
        v(&[
            "VdMot/valves/+/target/set q1",
            "VdMot/valves/+/target/set/set q1",
            "VdMot/cmd/# q0",
            "homeassistant/status q1"
        ])
    );
    assert_eq!(
        subs(
            &ctx_of(b"VdMot", false, false),
            MqttMode::Mqtt,
            b"ha",
            None,
            ALL
        ),
        v(&[
            "VdMot/valves/+/target q1",
            "VdMot/valves/+/target/set q1",
            "VdMot/cmd/# q0"
        ])
    );
    assert_eq!(
        subs(
            &ctx_of(b"VdMot", true, true),
            MqttMode::Mqtt,
            b"homeassistant",
            None,
            ALL
        ),
        v(&[
            "/VdMot/valves/+/target/set q1",
            "/VdMot/valves/+/target/set/set q1",
            "/VdMot/cmd/# q0"
        ])
    );
    assert!(subs(&s, MqttMode::Off, b"homeassistant", None, ALL).is_empty());
    assert_eq!(
        subs(&s, MqttMode::MqttHa, b"homeassistant", None, 2),
        v(&[
            "VdMot/valves/+/target/set q1",
            "VdMot/valves/+/target/set/set q1"
        ])
    );
    assert!(subs(&s, MqttMode::MqttHa, b"ha", None, 0).is_empty());
    // C++ buildSubscriptions(.., nullptr, 5) == 0: no Rust form (the empty output above).
    let mut one: [Subscription; 1] = Default::default();
    assert_eq!(
        build_subscriptions(&s, MqttMode::Mqtt, b"ha", None, &mut one),
        1
    );
    // An unusable root: only the HA status.
    assert_eq!(
        subs(&ctx(b"a+b"), MqttMode::MqttHa, b"homeassistant", None, ALL),
        v(&["homeassistant/status q1"])
    );
    // Segments with '/' are spelled out ('+' matches one level only).
    let names: [&[u8]; 12] = [
        b"Bad", b"", b"x", b"", b"", b"", b"", b"", b"", b"", b"", b"",
    ];
    let mut seg = fill_segments(&names);
    seg[2] = seg_text(b"Bad/WC");
    seg[5] = seg_text(b"a/b/c");
    seg[6] = seg_text(b"a//b"); // invalid: never subscribed
    assert_eq!(
        subs(&s, MqttMode::Mqtt, b"homeassistant", Some(&seg), ALL),
        v(&[
            "VdMot/valves/+/target/set q1",
            "VdMot/valves/+/target/set/set q1",
            "VdMot/cmd/# q0",
            "VdMot/valves/Bad/WC/target/set q1",
            "VdMot/valves/Bad/WC/target/set/set q1",
            "VdMot/valves/a/b/c/target/set q1",
            "VdMot/valves/a/b/c/target/set/set q1"
        ])
    );
    assert_eq!(
        subs(
            &ctx_of(b"VdMot", false, false),
            MqttMode::MqttHa,
            b"homeassistant",
            Some(&seg),
            ALL
        ),
        v(&[
            "VdMot/valves/+/target q1",
            "VdMot/valves/+/target/set q1",
            "VdMot/cmd/# q0",
            "VdMot/valves/Bad/WC/target q1",
            "VdMot/valves/Bad/WC/target/set q1",
            "VdMot/valves/a/b/c/target q1",
            "VdMot/valves/a/b/c/target/set q1",
            "homeassistant/status q1"
        ])
    );
    // Every valve spelled out still fits MAX_SUBSCRIPTIONS.
    let all: Segments = core::array::from_fn(|i| seg_text(format!("V/{}", i + 1).as_bytes()));
    assert_eq!(
        subs(&s, MqttMode::MqttHa, b"ha", Some(&all), ALL).len(),
        MAX_SUBSCRIPTIONS
    );
    // A root so long that "/set" no longer fits after the spelled-out filter.
    let long_root = ctx_of(b"abcdefghijklmnopqrst", true, true);
    assert_eq!(subs(&long_root, MqttMode::Mqtt, b"ha", None, ALL).len(), 3);
}

#[test]
fn build_subscription_entry_i_of_the_table_none_past_the_last_and_for_off() {
    let all: Segments = core::array::from_fn(|i| seg_text(format!("V/{}", i + 1).as_bytes()));
    let mut some: Segments = Default::default();
    some[3] = seg_text(b"Bad/WC");
    struct Case<'a> {
        ctx: TopicContext,
        mode: MqttMode,
        prefix: &'a [u8],
        seg: Option<&'a Segments>,
    }
    let cases = [
        // every entry: 29
        Case {
            ctx: ctx(b"VdMot"),
            mode: MqttMode::MqttHa,
            prefix: b"ha",
            seg: Some(&all),
        },
        Case {
            ctx: ctx_of(b"VdMot", false, false),
            mode: MqttMode::Mqtt,
            prefix: b"ha",
            seg: Some(&some),
        },
        // entries skipped
        Case {
            ctx: ctx_of(b"abcdefghijklmnopqrst", true, true),
            mode: MqttMode::Mqtt,
            prefix: b"ha",
            seg: Some(&all),
        },
        // the HA status only
        Case {
            ctx: ctx(b"a+b"),
            mode: MqttMode::MqttHa,
            prefix: b"homeassistant",
            seg: None,
        },
    ];
    for c in &cases {
        let mut table: [Subscription; MAX_SUBSCRIPTIONS + 1] = Default::default();
        let n = build_subscriptions(&c.ctx, c.mode, c.prefix, c.seg, &mut table);
        assert!(n > 0);
        for (i, want) in table[..n].iter().enumerate() {
            let one = build_subscription(&c.ctx, c.mode, c.prefix, c.seg, i).unwrap();
            assert_eq!(&one, want);
        }
        // C++ `past` keeps its 'y' bytes: None
        assert_eq!(build_subscription(&c.ctx, c.mode, c.prefix, c.seg, n), None);
    }
    assert_eq!(
        build_subscription(&ctx(b"VdMot"), MqttMode::Off, b"ha", Some(&all), 0),
        None
    );
}

fn inbound_with(c: &TopicContext, t: &[u8], seg: Option<&Segments>, prefix: &[u8]) -> String {
    let r = parse_inbound_topic(c, prefix, t, seg);
    // the Debug names are the C++ kKinds names
    let mut s = format!("{:?}", r.kind);
    if matches!(r.kind, InboundKind::Target | InboundKind::CalibrateValve) {
        s += &format!(" {}", r.valve.map_or(-1, i32::from));
    } else {
        assert_eq!(r.valve, None);
    }
    if r.state_form {
        s += " state";
    }
    s
}

fn inbound(c: &TopicContext, t: &[u8], seg: Option<&Segments>) -> String {
    inbound_with(c, t, seg, b"homeassistant")
}

#[test]
fn parse_inbound_topic_targets() {
    let names: [&[u8]; 12] = [
        b"Bad 1", b"", b"Kitchen", b"", b"", b"", b"", b"", b"", b"", b"", b"12",
    ];
    let seg = fill_segments(&names);
    let s = ctx(b"VdMot");
    assert_eq!(
        inbound(&s, b"VdMot/valves/Bad_1/target/set", Some(&seg)),
        "Target 0"
    );
    assert_eq!(
        inbound(&s, b"VdMot/valves/Bad_1/target/set/set", Some(&seg)),
        "Target 0"
    );
    assert_eq!(
        inbound(&s, b"/VdMot/valves/Bad_1/target/set", Some(&seg)),
        "Target 0"
    );
    assert_eq!(
        inbound(&s, b"VdMot/valves/2/target/set", Some(&seg)),
        "Target 1"
    );
    assert_eq!(
        inbound(&s, b"VdMot/valves/Kitchen/target/set", Some(&seg)),
        "Target 2"
    );
    // number of a named valve
    assert_eq!(
        inbound(&s, b"VdMot/valves/3/target/set", Some(&seg)),
        "Target 2"
    );
    assert_eq!(
        inbound(&s, b"VdMot/valves/1/target/set", Some(&seg)),
        "Target 0"
    );
    // valve 12 named "12"
    assert_eq!(
        inbound(&s, b"VdMot/valves/12/target/set", Some(&seg)),
        "Target 11"
    );
    assert_eq!(
        inbound(&s, b"VdMot/valves/11/target/set", Some(&seg)),
        "Target 10"
    );
    // Target topics naming no valve.
    let unknown: [&[u8]; 11] = [
        b"VdMot/valves/13/target/set",
        b"VdMot/valves/0/target/set",
        b"VdMot/valves/03/target/set",
        b"VdMot/valves/-1/target/set",
        b"VdMot/valves/1a/target/set",
        b"VdMot/valves/Old/target/set",
        b"VdMot/valves/Unknown/target/set",
        b"VdMot/valves/12345678901/target/set",
        b"VdMot/valves/Bad 1/target/set",
        b"VdMot/valves/4294967297/target/set",
        b"VdMot/valves/a/b/target/set",
    ];
    for t in unknown {
        assert_eq!(
            inbound(&s, t, Some(&seg)),
            "Target -1",
            "{}",
            t.escape_ascii()
        );
    }
    let none: [&[u8]; 24] = [
        b"VdMot/valves//target/set",
        b"VdMot/valves/1/target",
        b"VdMot/valves/1/target/value",
        b"VdMot/valves/1/target/",
        b"VdMot/valves/1/targetx/set",
        b"VdMot/valves/1/state/set",
        b"VdMot/valves/1",
        b"VdMot/valves/1/",
        b"VdMot/valves/",
        b"VdMot/valves",
        b"VdMot/common/state/set",
        b"Other/valves/1/target/set",
        b"VdMo/valves/1/target/set",
        b"VdMotX/valves/1/target/set",
        b"vdmot/valves/1/target/set",
        b"//VdMot/valves/1/target/set",
        b"VdMot/Valves/1/target/set",
        b"VdMot/valves/target/set",
        b"VdMot/valves/1/set/target",
        b"VdMot/status",
        b"homeassistant/statu",
        b"ha/status",
        b"VdMot/",
        b"",
    ];
    for t in none {
        assert_eq!(inbound(&s, t, Some(&seg)), "None", "{}", t.escape_ascii());
    }
    // Not separate: the state form and one /set.
    let p = ctx_of(b"VdMot", false, false);
    assert_eq!(
        inbound(&p, b"VdMot/valves/2/target", Some(&seg)),
        "Target 1 state"
    );
    assert_eq!(
        inbound(&p, b"VdMot/valves/2/target/set", Some(&seg)),
        "Target 1"
    );
    assert_eq!(
        inbound(&p, b"VdMot/valves/Old/target", Some(&seg)),
        "Target -1 state"
    );
    assert_eq!(
        inbound(&p, b"VdMot/valves/2/target/set/set", Some(&seg)),
        "None"
    );
    // pathAsRoot: the leading '/' is optional on both sides.
    let r = ctx_of(b"VdMot", true, true);
    assert_eq!(
        inbound(&r, b"/VdMot/valves/2/target/set", Some(&seg)),
        "Target 1"
    );
    assert_eq!(
        inbound(&r, b"VdMot/valves/2/target/set", Some(&seg)),
        "Target 1"
    );
    // Fallback main topic and an unusable root.
    assert_eq!(
        inbound(&ctx(b""), b"VdMotFBH/valves/2/target/set", Some(&seg)),
        "Target 1"
    );
    assert_eq!(
        inbound(&ctx(b"a+b"), b"a+b/valves/2/target/set", Some(&seg)),
        "None"
    );
    assert_eq!(
        inbound(&ctx(b"a+b"), b"valves/2/target/set", Some(&seg)),
        "None"
    );
    assert_eq!(
        inbound(&ctx(b"a+b"), b"homeassistant/status", Some(&seg)),
        "HaStatus"
    );
    // Without segment table: numbers only.
    assert_eq!(inbound(&s, b"VdMot/valves/5/target/set", None), "Target 4");
    assert_eq!(
        inbound(&s, b"VdMot/valves/Bad_1/target/set", None),
        "Target -1"
    );
    // First name match wins; names beat numbers.
    let dup: [&[u8]; 12] = [
        b"x", b"", b"", b"", b"x", b"", b"", b"", b"", b"", b"", b"3",
    ];
    let d = fill_segments(&dup);
    assert_eq!(
        inbound(&s, b"VdMot/valves/x/target/set", Some(&d)),
        "Target 0"
    );
    // valve 3's own segment "3"
    assert_eq!(
        inbound(&s, b"VdMot/valves/3/target/set", Some(&d)),
        "Target 2"
    );
    let named3: [&[u8]; 12] = [b"3", b"", b"Z", b"", b"", b"", b"", b"", b"", b"", b"", b""];
    let n3 = fill_segments(&named3);
    assert_eq!(
        inbound(&s, b"VdMot/valves/3/target/set", Some(&n3)),
        "Target 0"
    );
    // Multi-level segments (topic overrides) are matched before the number form.
    let mut ml = fill_segments(&names);
    ml[2] = seg_text(b"Bad/WC");
    ml[4] = seg_text(b"1/2");
    assert_eq!(
        inbound(&s, b"VdMot/valves/Bad/WC/target/set", Some(&ml)),
        "Target 2"
    );
    assert_eq!(
        inbound(&s, b"VdMot/valves/Bad/WC/target/set/set", Some(&ml)),
        "Target 2"
    );
    assert_eq!(
        inbound(&s, b"VdMot/valves/1/2/target/set", Some(&ml)),
        "Target 4"
    );
    assert_eq!(
        inbound(&p, b"VdMot/valves/Bad/WC/target", Some(&ml)),
        "Target 2 state"
    );
    assert_eq!(
        inbound(&p, b"VdMot/valves/Bad/WC/target/set", Some(&ml)),
        "Target 2"
    );
    // An empty entry in the table (invalid name) never matches.
    let mut holes = fill_segments(&names);
    holes[4].clear();
    assert_eq!(
        inbound(&s, b"VdMot/valves/5/target/set", Some(&holes)),
        "Target 4"
    );
    // Entries without a NUL are read bounded: the C++ memsets 11 'q' per entry, a Text<10>
    // holds 10; a NUL inside ends an entry like in the C++ array.
    let raw: Segments = core::array::from_fn(|_| seg_text(b"qqqqqqqqqq"));
    assert_eq!(
        inbound(&s, b"VdMot/valves/qqqqqqqqqq/target/set", Some(&raw)),
        "Target 0"
    );
    let mut nul: Segments = Default::default();
    nul[6].extend_from_slice(b"ab\0cd").unwrap();
    assert_eq!(
        inbound(&s, b"VdMot/valves/ab/target/set", Some(&nul)),
        "Target 6"
    );
    // len is authoritative; NUL bytes and oversize input are rejected.
    let with_tail = b"VdMot/valves/2/target/setGARBAGE";
    let r2 = parse_inbound_topic(&s, b"ha", &with_tail[..25], Some(&seg));
    assert_eq!(r2.valve, Some(1));
    let with_nul = b"VdMot/valves/2\0/target/set";
    assert_eq!(
        parse_inbound_topic(&s, b"ha", with_nul, Some(&seg)).kind,
        InboundKind::None
    );
    // C++ parseInboundTopic(.., nullptr, 10, ..): no Rust form.
    let t = b"VdMot/valves/2/target/set";
    assert_eq!(
        parse_inbound_topic(&s, b"ha", &t[..0], Some(&seg)).kind,
        InboundKind::None
    );
    let mut huge = vec![b'/'; TOPIC_MAX + 1 - t.len()];
    huge.extend_from_slice(t);
    assert_eq!(huge.len(), TOPIC_MAX + 1);
    assert_eq!(
        parse_inbound_topic(&s, b"ha", &huge, Some(&seg)).kind,
        InboundKind::None
    );
    let mut edge = b"/VdMot/valves/".to_vec();
    edge.resize(edge.len() + TOPIC_MAX - 25, b'1');
    edge.extend_from_slice(b"/target/set");
    assert_eq!(edge.len(), TOPIC_MAX);
    assert_eq!(
        parse_inbound_topic(&s, b"ha", &edge, Some(&seg)).kind,
        InboundKind::Target
    );
    // The empty topic is none of ours, also with an empty HA prefix (whose status topic does not
    // exist).
    assert_eq!(
        parse_inbound_topic(&s, b"", b"", Some(&seg)).kind,
        InboundKind::None
    );
    assert_eq!(
        parse_inbound_topic(&s, b"", b"/status", Some(&seg)).kind,
        InboundKind::None
    );
}

#[test]
fn parse_inbound_topic_ha_status_and_cmd_topics() {
    let names: [&[u8]; 12] = [
        b"Bad 1", b"", b"", b"", b"", b"", b"", b"", b"", b"", b"", b"",
    ];
    let seg = fill_segments(&names);
    let s = ctx(b"VdMot");
    let sg = Some(&seg);
    assert_eq!(inbound(&s, b"homeassistant/status", sg), "HaStatus");
    assert_eq!(
        inbound_with(&s, b"homeassistant/status", sg, b"ha"),
        "HaStatus"
    );
    assert_eq!(inbound_with(&s, b"ha/status", sg, b"ha"), "HaStatus");
    // also the C++ null prefix
    assert_eq!(inbound_with(&s, b"ha/status", sg, b""), "None");
    assert_eq!(inbound(&s, b"/homeassistant/status", sg), "None");
    assert_eq!(inbound(&s, b"homeassistant/status/x", sg), "None");
    assert_eq!(
        inbound(&s, b"VdMot/cmd/valves/Bad_1/calibrate", sg),
        "CalibrateValve 0"
    );
    assert_eq!(
        inbound(&s, b"VdMot/cmd/valves/2/calibrate", sg),
        "CalibrateValve 1"
    );
    assert_eq!(
        inbound(&s, b"VdMot/cmd/valves/13/calibrate", sg),
        "CalibrateValve -1"
    );
    assert_eq!(
        inbound(&s, b"/VdMot/cmd/valves/1/calibrate", sg),
        "CalibrateValve 0"
    );
    assert_eq!(
        inbound(&s, b"VdMot/cmd/valves//calibrate", sg),
        "UnknownCommand"
    );
    assert_eq!(
        inbound(&s, b"VdMot/cmd/valves/calibrate", sg),
        "UnknownCommand"
    );
    assert_eq!(inbound(&s, b"VdMot/cmd/calibrate", sg), "CalibrateAll");
    assert_eq!(inbound(&s, b"VdMot/cmd/restart", sg), "Restart");
    assert_eq!(inbound(&s, b"VdMot/cmd/stmReset", sg), "StmReset");
    assert_eq!(inbound(&s, b"VdMot/cmd/detect", sg), "Detect");
    assert_eq!(inbound(&s, b"VdMot/cmd/stop", sg), "StopAll");
    assert_eq!(inbound(&s, b"VdMot/cmd/stmSafeExit", sg), "StmSafeExit");
    let other: [&[u8]; 6] = [
        b"VdMot/cmd/foo",
        b"VdMot/cmd/",
        b"VdMot/cmd/restart/x",
        b"VdMot/cmd/Restart",
        b"VdMot/cmd/detec",
        b"VdMot/cmd/stops",
    ];
    for t in other {
        assert_eq!(inbound(&s, t, sg), "UnknownCommand", "{}", t.escape_ascii());
    }
    assert_eq!(inbound(&s, b"VdMot/cmd", sg), "None");
    assert_eq!(inbound(&s, b"VdMot/cmdx/restart", sg), "None");
    assert_eq!(inbound(&s, b"Other/cmd/restart", sg), "None");
}

#[test]
fn parse_inbound_topic_fuzz_fixed_seed() {
    let names: [&[u8]; 12] = [
        b"a", b"b b", b"", b"", b"", b"", b"", b"", b"", b"", b"", b"",
    ];
    let seg = fill_segments(&names);
    let s = ctx(b"VdMot");
    let mut rng = CRand::new(12345);
    const ALPHABET: &[u8] = b"VdMot/valves/target/set/cmd0123456789ab_ \x01\xff+#";
    let mut buf = [0u8; 160];
    for _ in 0..50000 {
        let len = (rng.rand() % 150) as usize;
        for b in &mut buf[..len] {
            *b = if rng.rand() % 4 == 0 {
                (rng.rand() % 256) as u8
            } else {
                ALPHABET[rng.rand() as usize % ALPHABET.len()]
            };
        }
        let r = parse_inbound_topic(&s, b"ha", &buf[..len], Some(&seg));
        assert!(r.valve.is_none_or(|v| v < VALVE_COUNT));
        // C++ also checks kind <= UnknownCommand: an InboundKind is always in range
    }
    // Mutations of a valid topic stay in range and mostly fail.
    let good: &[u8] = b"VdMot/valves/b_b/target/set";
    let mut accepted = 0;
    for _ in 0..20000 {
        let mut t = good.to_vec();
        let pos = rng.rand() as usize % t.len();
        t[pos] = (rng.rand() % 256) as u8;
        let r = parse_inbound_topic(&s, b"ha", &t, Some(&seg));
        assert!(r.valve.is_none_or(|v| v < VALVE_COUNT));
        if r.valve.is_some() {
            accepted += 1;
            assert!(r.valve == Some(1) || t == good);
        }
    }
    assert!(accepted < 20000);
}

#[test]
fn parse_target_payload_cases() {
    let oks: [(&[u8], u8); 29] = [
        (b"55", 55),
        (b"0", 0),
        (b"100", 100),
        (b" 55\r\n", 55),
        (b"\t7 ", 7),
        (b"55.0", 55),
        (b"55.00", 55),
        (b"100.000", 100),
        (b"007", 7),
        (b"OPEN", 100),
        (b"CLOSE", 0),
        (b" OPEN\n", 100),
        (b"0000000000000100", 100),
        (b"1", 1),
        (b"99", 99),
        (b"43.7", 44),
        (b"43,7", 44),
        (b"43.5", 44),
        (b"43.49999", 43),
        (b"0.4", 0),
        (b"0,5", 1),
        (b"99.5", 100),
        (b"100.0", 100),
        (b"00055.50", 56),
        (b" 42 ", 42),
        (b"55,0", 55),
        (b"55.01", 55),
        (b"100.000000000000", 100),
        (b"  55            ", 55),
    ];
    for (p, want) in oks {
        assert_eq!(parse_target_payload(p), Ok(want), "{}", p.escape_ascii());
    }
    use TargetPayload::{Empty, NotNumber, OutOfRange, Stop};
    // the C++ `out` stays 200 for each of these: the Err result
    let bads: [(&[u8], TargetPayload); 37] = [
        (b"", Empty),
        (b"   ", Empty),
        (b"\r\n", Empty),
        (b"101", OutOfRange),
        (b"1000", OutOfRange),
        (b"255", OutOfRange),
        (b"256", OutOfRange),
        (b"99999999999999", OutOfRange),
        (b"101.0", OutOfRange),
        (b"100.01", OutOfRange),
        (b"100.5", OutOfRange),
        (b"100.00000000001", OutOfRange),
        (b"55.", NotNumber),
        (b".5", NotNumber),
        (b",5", NotNumber),
        (b"1.2.3", NotNumber),
        (b"1,2,3", NotNumber),
        (b"1.2,3", NotNumber),
        (b"-5", NotNumber),
        (b"+5", NotNumber),
        (b"-0", NotNumber),
        (b"1e2", NotNumber),
        (b"0x10", NotNumber),
        (b"nan", NotNumber),
        (b"inf", NotNumber),
        (b"5 5", NotNumber),
        (b"5. 5", NotNumber),
        (b"open", NotNumber),
        (b"Close", NotNumber),
        (b"stop", NotNumber),
        (b"OPENX", NotNumber),
        (b"55a", NotNumber),
        (b"5.5a", NotNumber),
        (b"00000000000001000", NotNumber), // 17 chars
        (b"STOP", Stop),
        (b" STOP\n", Stop),
        (b"\x0c55", NotNumber), // a form feed is no blank (C isSpace)
    ];
    for (p, want) in bads {
        assert_eq!(parse_target_payload(p), Err(want), "{}", p.escape_ascii());
    }
    // C++ parseTargetPayload(nullptr, 3, out) == Empty: no Rust form (the empty payload).
    assert_eq!(parse_target_payload(&b"55"[..0]), Err(Empty));
    assert_eq!(parse_target_payload(b"55\0"), Err(NotNumber));
    assert_eq!(parse_target_payload(b"5\x005"), Err(NotNumber));
    assert_eq!(parse_target_payload(b"5.\0"), Err(NotNumber));
    assert_eq!(parse_target_payload(&b"559"[..2]), Ok(55)); // len bounded
    assert_eq!(parse_target_payload(&b"43.51"[..4]), Ok(44)); // "43.5"

    // Exact-size buffers (no terminator to lean on): every Rust slice is one.
    let exact: [(&[u8], bool); 6] = [
        (b"   ", false),
        (b" 5 ", true),
        (b"5", true),
        (b"OPEN", true),
        (b"4.5", true),
        (b"99.999999999999", true),
    ];
    for (p, ok) in exact {
        let r = parse_target_payload(p);
        assert_eq!(r.is_ok(), ok, "{}", p.escape_ascii());
        if !ok {
            assert_eq!(r, Err(Empty));
        }
    }
    let mut seventeen = [b' '; 17];
    seventeen[8] = b'5';
    assert_eq!(parse_target_payload(&seventeen), Err(NotNumber));
    let mut sixteen = [b' '; 16];
    sixteen[8] = b'5';
    assert_eq!(parse_target_payload(&sixteen), Ok(5));

    // Fuzz: never out of range, never a crash. The C++ alphabet is the string literal with its
    // NUL (rand() % 23).
    const ALPHABET: &[u8; 23] = b"0123456789., \r\nOPENCLS\0";
    let mut rng = CRand::new(777);
    let mut buf = [0u8; 24];
    for _ in 0..50000 {
        let len = (rng.rand() % 20) as usize;
        for b in &mut buf[..len] {
            *b = if rng.rand() % 3 == 0 {
                (rng.rand() % 256) as u8
            } else {
                ALPHABET[(rng.rand() % 23) as usize]
            };
        }
        if let Ok(v) = parse_target_payload(&buf[..len]) {
            assert!(v <= 100);
        }
    }
}

#[test]
fn target_payload_values() {
    // the numbers of the C++ enum (the detail of a payload reject); 0 is Ok
    assert_eq!(TargetPayload::Empty as u8, 1);
    assert_eq!(TargetPayload::NotNumber as u8, 2);
    assert_eq!(TargetPayload::OutOfRange as u8, 3);
    assert_eq!(TargetPayload::Stop as u8, 4);
    for v in 0..=255u8 {
        let want = (1..=4).contains(&v);
        assert_eq!(TargetPayload::from_raw(v).is_some(), want, "{v}");
        if let Some(r) = TargetPayload::from_raw(v) {
            assert_eq!(r as u8, v);
        }
    }
}

#[test]
fn parse_button_payload_cases() {
    assert!(parse_button_payload(b"PRESS"));
    assert!(parse_button_payload(&b"PRESSx"[..5])); // len is authoritative
    let bad: [&[u8]; 7] = [
        b"press", b"PRESS ", b" PRESS", b"PRES", b"PRESSED", b"", b"ON",
    ];
    for b in bad {
        assert!(!parse_button_payload(b), "{}", b.escape_ascii());
    }
    // C++ parseButtonPayload(nullptr, 5): no Rust form.
}

#[test]
fn every_builder_leaves_cap_0_untouched_and_clears_the_output_on_failure() {
    let s = ctx(b"VdMot");
    let mut buf = [b'X'; TOPIC_MAX + 1];
    // cap 0: nothing written.
    assert_eq!(build_main_topic(&s, &mut buf[..0]), 0);
    assert_eq!(buf[0], b'X');
    assert_eq!(build_topic(&s, Topic::Status, b"", &mut buf[..0]), 0);
    assert_eq!(buf[0], b'X');
    assert_eq!(build_target_command_topic(&s, b"1", &mut buf[..0]), 0);
    assert_eq!(buf[0], b'X');
    assert_eq!(build_segment(b"a", 0, &mut buf[..0]), 0);
    assert_eq!(build_segment(b"", 0, &mut buf[..0]), 0);
    assert_eq!(buf[0], b'X');
    assert_eq!(format_temp(1, true, false, &mut buf[..0]), 0);
    assert_eq!(format_volt(1.0, true, false, &mut buf[..0]), 0);
    assert_eq!(format_volt(1.0, true, true, &mut buf[..0]), 0);
    assert_eq!(format_uptime(1, &mut buf[..0]), 0);
    assert_eq!(format_valve_state(1, true, &mut buf[..0]), 0);
    assert_eq!(format_system_state(1, true, &mut buf[..0]), 0);
    assert_eq!(format_legacy_counter(1, &mut buf[..0]), 0);
    let t = LocalTime::default();
    assert_eq!(format_calib_date(&t, &mut buf[..0]), 0);
    assert_eq!(buf[0], b'X');

    // cap 1: only the terminator fits.
    let mut one = [b'X'; 1];
    assert_eq!(build_main_topic(&s, &mut one), 0);
    assert_eq!(build_topic(&s, Topic::Status, b"", &mut one), 0);
    assert_eq!(build_segment(b"a", 0, &mut one), 0);
    assert_eq!(build_segment(b"", 0, &mut one), 0);
    assert_eq!(format_temp(1, true, false, &mut one), 0);
    assert_eq!(format_volt(1.0, false, false, &mut one), 0);
    assert_eq!(format_valve_state(0, true, &mut one), 0); // "" fits

    // Failures after partial output give 0.
    let bad = ctx_of(b"a+b", true, true);
    assert_eq!(build_main_topic(&bad, &mut buf), 0);
    assert_eq!(build_topic(&ctx(b"a+b"), Topic::CommonIp, b"", &mut buf), 0);
    assert_eq!(build_topic(&s, Topic::ValveTarget, b"a//b", &mut buf), 0);
    assert_eq!(build_topic(&s, Topic::ValveTarget, b"", &mut buf), 0);
    // C++ buildTopic(s, static_cast<Topic>(200), ..): no Rust form.
    assert_eq!(build_target_command_topic(&s, b"", &mut buf), 0);
    assert_eq!(build_target_command_topic(&ctx(b"a#"), b"1", &mut buf), 0);
    assert_eq!(build_segment(b"a/b", 0, &mut buf), 0);
    let mut three = [b'X'; 3];
    assert_eq!(build_segment(b"abc", 0, &mut three), 0);
    assert_eq!(build_segment(b"", 199, &mut three), 0);
    assert_eq!(format_temp(215, true, false, &mut three), 0);
    assert_eq!(format_calib_date(&t, &mut three), 0);
    assert_eq!(format_legacy_counter(4294967295, &mut three), 2);
    assert_text(&three[..2], "-1");
    // Exact fit.
    let mut five = [0u8; 5];
    assert_eq!(format_temp(215, true, false, &mut five), 4);
    assert_text(&five[..4], "21.5");
    let mut seg11 = [0u8; SEGMENT_MAX + 1];
    assert_eq!(build_segment(b"0123456789", 0, &mut seg11), 10);
    assert_eq!(build_segment(b"", 0, &mut seg11[..2]), 1);
}

#[test]
fn temperature_and_volt_payloads() {
    assert_text(&fmt_temp(215, true, false), "21.5");
    assert_text(&fmt_temp(215, true, true), "21,5");
    assert_text(&fmt_temp(-5, true, false), "-0.5");
    assert_text(&fmt_temp(-5, true, true), "-0,5");
    assert_text(&fmt_temp(-15, true, false), "-1.5");
    assert_text(&fmt_temp(0, true, false), "0.0");
    assert_text(&fmt_temp(9, true, false), "0.9");
    assert_text(&fmt_temp(10, true, false), "1.0");
    assert_text(&fmt_temp(1250, true, false), "125.0");
    assert_text(&fmt_temp(-550, true, false), "-55.0");
    assert_text(&fmt_temp(i32::MIN, true, false), "-214748364.8");
    assert_text(&fmt_temp(i32::MAX, true, false), "214748364.7");
    assert_text(&fmt_temp(215, false, false), "failed");
    assert_text(&fmt_temp(215, false, true), "failed");
    let mut small = [0u8; 4];
    assert_eq!(format_temp(215, true, false, &mut small), 0);
    let mut five = [0u8; 5];
    assert_eq!(format_temp(215, true, false, &mut five), 4);
    assert_eq!(format_temp(215, false, false, &mut small), 0);
    // C++ formatTemp(.., nullptr, 8): no Rust form.

    assert_text(&fmt_volt(12.345, true, false), "12.345");
    assert_text(&fmt_volt(12.345, true, true), "12,345");
    assert_text(&fmt_volt(0.0, true, false), "0.000");
    assert_text(&fmt_volt(-1.5, true, false), "-1.500");
    assert_text(&fmt_volt(-1.5, true, true), "-1,500");
    assert_text(&fmt_volt(1234.5678, true, false), "1234.568");
    assert_text(&fmt_volt(12.0, false, false), "failed");
    assert_text(&fmt_volt(f64::NAN, true, false), "failed");
    assert_text(&fmt_volt(f64::INFINITY, true, false), "failed");
    assert_text(&fmt_volt(f64::NEG_INFINITY, true, true), "failed");
    assert_text(
        &fmt_volt(999999999999999.0, true, false),
        "999999999999999.000",
    );
    assert_text(&fmt_volt(-1e15, true, true), "-1000000000000000,000");
    let mut buf = [b'X'; 20];
    assert_eq!(format_volt(-1e15, true, false, &mut buf), 0);
    assert_eq!(format_volt(1e300, true, true, &mut buf), 0);
    let mut six = [0u8; 6];
    assert_eq!(format_volt(1.0, true, false, &mut six), 5);
    assert_eq!(format_volt(10.0, true, false, &mut six), 0);
}

#[test]
fn uptime_states_counters() {
    let mut buf = [0u8; 32];
    assert_eq!(format_uptime(0, &mut buf), 10);
    assert_text(&buf[..10], "0d 0:00:00");
    let n = format_uptime(273909, &mut buf);
    assert_text(&buf[..n], "3d 4:05:09");
    let n = format_uptime(86399, &mut buf);
    assert_text(&buf[..n], "0d 23:59:59");
    let n = format_uptime(86400, &mut buf);
    assert_text(&buf[..n], "1d 0:00:00");
    let n = format_uptime(u32::MAX, &mut buf);
    assert_text(&buf[..n], "49710d 6:28:15");
    let mut tiny = [0u8; 10];
    assert_eq!(format_uptime(0, &mut tiny), 0);

    let texts = [
        "",
        "idle",
        "opens",
        "closes",
        "failed",
        "unknown",
        "no valve",
        "full open",
        "connected",
        "blocked",
    ];
    for (st, text) in texts.iter().enumerate() {
        let n = format_valve_state(st as u8, true, &mut buf);
        assert_eq!(n, text.len());
        assert_text(&buf[..n], text);
        let n = format_valve_state(st as u8, false, &mut buf);
        assert_text(&buf[..n], format!("{st}"));
    }
    assert_eq!(format_valve_state(10, true, &mut buf), 0);
    assert_eq!(format_valve_state(255, false, &mut buf), 3);
    assert_text(&buf[..3], "255");
    let mut two = [0u8; 2];
    assert_eq!(format_valve_state(12, false, &mut two), 0);
    assert_eq!(format_valve_state(1, true, &mut two), 0);

    let sys = ["ok", "info", "error"];
    for (st, name) in sys.iter().enumerate() {
        let n = format_system_state(st as u8, true, &mut buf);
        assert_text(&buf[..n], name);
        let n = format_system_state(st as u8, false, &mut buf);
        assert_text(&buf[..n], format!("{st}"));
    }
    assert_eq!(format_system_state(3, true, &mut buf), 0);
    assert_eq!(format_system_state(3, false, &mut buf), 1);
    assert_text(&buf[..1], "3");
    assert_eq!(format_system_state(255, true, &mut buf), 0);

    assert_eq!(format_legacy_counter(0, &mut buf), 1);
    assert_text(&buf[..1], "0");
    let n = format_legacy_counter(3120, &mut buf);
    assert_text(&buf[..n], "3120");
    let n = format_legacy_counter(2147483647, &mut buf);
    assert_text(&buf[..n], "2147483647");
    let n = format_legacy_counter(2147483648, &mut buf);
    assert_text(&buf[..n], "-2147483648");
    let n = format_legacy_counter(4294967295, &mut buf);
    assert_text(&buf[..n], "-1");
    assert_eq!(format_legacy_counter(100, &mut two), 0);
    // C++ formatLegacyCounter(100, nullptr, 4): no Rust form.
}

#[test]
fn calibration_date_in_the_legacy_strftime_format() {
    let mut t = LocalTime {
        valid: true,
        year: 2026,
        month: 9,
        mday: 21,
        wday: 1,
        hour: 14,
        minute: 3,
        second: 5,
        ..LocalTime::default()
    };
    let mut buf = [0u8; 48];
    assert_eq!(format_calib_date(&t, &mut buf), 34);
    assert_text(&buf[..34], "Monday, September 21.2026 14:03:05");
    t.mday = 5;
    t.hour = 0;
    t.minute = 0;
    t.second = 0;
    let n = format_calib_date(&t, &mut buf);
    assert_text(&buf[..n], "Monday, September 05.2026 00:00:00");
    let days = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let months = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    for (d, day) in days.iter().enumerate() {
        t.wday = d as u8;
        let n = format_calib_date(&t, &mut buf);
        assert!(buf[..n].starts_with(format!("{day}, ").as_bytes()), "{d}");
    }
    t.wday = 3;
    for (m, month) in months.iter().enumerate() {
        t.month = m as u8 + 1;
        let n = format_calib_date(&t, &mut buf);
        assert_text(&buf[..n], format!("Wednesday, {month} 05.2026 00:00:00"));
    }
    t.month = 1;
    t.mday = 1;
    let n = format_calib_date(&t, &mut buf);
    assert_text(&buf[..n], "Wednesday, January 01.2026 00:00:00");
    t.month = 12;
    t.mday = 31;
    t.hour = 23;
    t.minute = 59;
    t.second = 60;
    let n = format_calib_date(&t, &mut buf);
    assert_text(&buf[..n], "Wednesday, December 31.2026 23:59:60");
    let failed = "Failed to obtain time";
    let mut bad = t;
    bad.valid = false;
    assert_eq!(format_calib_date(&bad, &mut buf), failed.len());
    assert_text(&buf[..failed.len()], failed);
    let muts: [fn(&mut LocalTime); 8] = [
        |x| x.wday = 7,
        |x| x.month = 0,
        |x| x.month = 13,
        |x| x.mday = 0,
        |x| x.mday = 32,
        |x| x.hour = 24,
        |x| x.minute = 60,
        |x| x.second = 61,
    ];
    for (i, m) in muts.iter().enumerate() {
        let mut x = t;
        m(&mut x);
        let n = format_calib_date(&x, &mut buf);
        assert_text(&buf[..n], failed);
        assert_eq!(n, failed.len(), "{i}");
    }
    let mut small = [0u8; 10];
    assert_eq!(format_calib_date(&t, &mut small), 0);
    assert_eq!(format_calib_date(&bad, &mut small), 0);
}

fn params(on_change: bool, publish_interval_ms: u32, min_delay_ms: u32) -> PublishSchedulerParams {
    PublishSchedulerParams {
        on_change,
        publish_interval_ms,
        min_delay_ms,
    }
}

#[test]
fn publish_scheduler_full_publishes() {
    let mut s = PublishScheduler::default();
    let p = params(false, 10000, 5000);
    s.configure(p);
    assert!(s.take_full_publish(123)); // first after start
    assert!(!s.take_full_publish(123));
    assert!(!s.take_full_publish(10122));
    assert!(s.take_full_publish(10123));
    assert!(!s.take_full_publish(10124));
    s.on_connected(15000);
    assert!(s.take_full_publish(15000));
    assert!(!s.take_full_publish(24999));
    assert!(s.take_full_publish(25000));
    // Periodic mode: items only with the full publish.
    s.mark_all_published(25000);
    assert!(!s.take_item(1, true, 99999));
    // Wrap-safe.
    let mut w = PublishScheduler::default();
    w.configure(p);
    assert!(w.take_full_publish(0xFFFF_F000));
    assert!(!w.take_full_publish(0xFFFF_F000u32.wrapping_add(9999)));
    assert!(w.take_full_publish(0xFFFF_F000u32.wrapping_add(10000)));
}

#[test]
fn publish_scheduler_on_change_items() {
    let mut s = PublishScheduler::default();
    let mut p = params(true, 10000, 5000);
    s.configure(p);
    // Not yet published since connect: goes out at once.
    assert!(s.take_item(3, false, 100));
    assert!(!s.take_item(3, true, 100));
    s.mark_all_published(1000);
    assert!(!s.take_item(1, true, 5999));
    assert!(s.take_item(1, true, 6000));
    assert!(!s.take_item(1, true, 6001));
    assert!(!s.take_item(2, false, 10999)); // heartbeat
    assert!(s.take_item(2, false, 11000));
    assert!(!s.take_item(2, false, 11001));
    assert!(s.take_item(63, false, 11000));
    assert!(!s.take_item(64, true, 99999));
    assert!(!s.take_item(255, true, 99999));
    // Reconnect forgets the per-item history.
    s.on_connected(12000);
    assert!(s.take_item(1, false, 12000));
    // minDelay 0: every change goes out.
    let mut z = PublishScheduler::default();
    p.min_delay_ms = 0;
    z.configure(p);
    z.mark_all_published(0);
    assert!(z.take_item(0, true, 0));
    assert!(z.take_item(0, true, 0));
    assert!(!z.take_item(0, false, 0));
}

#[test]
fn publish_scheduler_parameter_clamping() {
    let mut s = PublishScheduler::default();
    // below the 2 s minimum, above the interval
    let mut p = params(true, 500, 9000);
    s.configure(p);
    assert!(s.take_full_publish(0));
    assert!(!s.take_full_publish(1999));
    assert!(s.take_full_publish(2000));
    s.mark_all_published(0);
    assert!(!s.take_item(0, true, 1999));
    assert!(s.take_item(0, true, 2000)); // minDelay clamped to the interval
    p.publish_interval_ms = 2000;
    p.min_delay_ms = 2000;
    let mut e = PublishScheduler::default();
    e.configure(p);
    e.mark_all_published(0);
    assert!(!e.take_item(0, true, 1999));
    assert!(e.take_item(0, true, 2000));
    // Exactly the minimum is kept; one below is raised to it.
    let mut m2 = PublishScheduler::default();
    p.publish_interval_ms = 2000;
    p.min_delay_ms = 0;
    m2.configure(p);
    assert!(m2.take_full_publish(0));
    assert!(!m2.take_full_publish(1999));
    assert!(m2.take_full_publish(2000));
    let mut m1 = PublishScheduler::default();
    p.publish_interval_ms = 1999;
    m1.configure(p);
    assert!(m1.take_full_publish(0));
    assert!(!m1.take_full_publish(1999));
    assert!(m1.take_full_publish(2000));
    let mut d = PublishScheduler::default(); // default parameters before configure()
    assert!(d.take_full_publish(0));
    assert!(!d.take_full_publish(9999));
    assert!(d.take_full_publish(10000));
    assert_eq!(PublishSchedulerParams::default(), params(true, 10000, 5000));
    assert_eq!(PublishScheduler::SLOTS, 64);
}

#[test]
fn topics_a_topic_that_stops_fitting_part_way_gives_0() {
    let s = ctx(b"VdMot");
    let mut buf = [b'X'; 16];
    // "VdMot/valves/1/target" needs 22 bytes: main and head fit, the tail not.
    assert_eq!(build_topic(&s, Topic::ValveTarget, b"1", &mut buf), 0);
    assert_eq!(build_target_command_topic(&s, b"1", &mut buf), 0);
    let mut fit = [0u8; 32];
    assert!(build_topic(&s, Topic::ValveTarget, b"1", &mut fit) > 0);
}

#[test]
fn topics_an_empty_inbound_topic_is_rejected_before_looking_at_its_bytes() {
    let s = ctx(b"VdMot");
    let t = b"/VdMot/valves/1/target/set";
    assert_eq!(
        parse_inbound_topic(&s, b"ha", &t[..0], None).kind,
        InboundKind::None
    );
    assert_eq!(parse_inbound_topic(&s, b"ha", t, None).valve, Some(0));
    assert_eq!(
        parse_inbound_topic(&s, b"ha", &t[..1], None).kind,
        InboundKind::None
    );
}
