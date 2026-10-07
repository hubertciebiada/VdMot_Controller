//! Port of test/native/test_log_sink.cpp: flush policy, severity filters, rotation steps, gap
//! lines, syslog packets.

use std::format;

use super::*;
use crate::common::NO_VALVE;
use crate::event_log::{make_event, EventCode};
use crate::test_support::assert_text;

#[test]
fn log_flush_params_defaults_are_the_cpp_ones() {
    let p = LogFlushParams::default();
    assert_eq!(
        (
            p.period_ms,
            p.urgent_gap_ms,
            p.backlog_high,
            p.failure_report_ms
        ),
        (300_000, 10_000, 256, 3_600_000)
    );
    assert_eq!(LogFlushPolicy::default(), LogFlushPolicy::new(p));
    assert_eq!(
        [
            LogFileStep::Append as u8,
            LogFileStep::Rotate as u8,
            LogFileStep::Defer as u8
        ],
        [0, 1, 2]
    );
}

#[test]
fn log_flush_policy_triggers_after_begin_at_1000() {
    let mut p = LogFlushPolicy::default();
    p.begin(1000);
    assert!(!p.attempted());
    assert_eq!(p.last_attempt_ms(), 1000);
    assert!(!p.due(0, true, true, 1000));
    assert!(!p.due(0, true, true, 4_000_000_000));
    assert!(p.due(1, false, true, 1000));
    assert!(p.due(256, false, false, 1000));
    assert!(!p.due(255, false, false, 1000));
    assert!(!p.due(1, true, false, 1000 + 9999));
    assert!(p.due(1, true, false, 1000 + 10_000));
    assert!(!p.due(255, false, false, 1000 + 299_999));
    assert!(p.due(1, false, false, 1000 + 300_000));
}

#[test]
fn log_flush_policy_attempts_failures_and_the_back_off_after_a_failure() {
    let mut p = LogFlushPolicy::default();
    p.begin(0);
    let t = 500_000;
    assert!(p.on_attempt(false, t)); // the first failure is reported
    assert!(p.attempted());
    assert_eq!(p.attempts(), 1);
    assert_eq!(p.failures(), 1);
    assert_eq!(p.last_attempt_ms(), t);
    assert!(!p.due(300, false, true, t + 9999));
    assert!(!p.due(300, true, true, t + 9999));
    assert!(p.due(300, false, true, t + 10_000));
    assert!(!p.on_attempt(false, t + 3_599_999));
    assert_eq!(p.failures(), 2);
    assert!(p.on_attempt(false, t + 3_600_000));
    assert!(!p.on_attempt(true, t + 3_600_001));
    assert_eq!(p.attempts(), 4);
    assert_eq!(p.failures(), 3);
    // After a success no back-off: a request is due at once.
    assert!(p.due(1, false, true, t + 3_600_001));
    // The report interval counts from the last report, not from the last failure.
    assert!(!p.on_attempt(false, t + 3_600_002));
    assert!(p.on_attempt(false, t + 7_200_000));
}

#[test]
fn log_flush_policy_an_ok_attempt_restarts_the_periodic_timer() {
    let mut p = LogFlushPolicy::default();
    p.begin(0);
    assert!(!p.on_attempt(true, 100_000));
    assert!(!p.due(1, false, false, 399_999));
    assert!(p.due(1, false, false, 400_000));
    assert!(!p.due(1, true, false, 109_999));
    assert!(p.due(1, true, false, 110_000));
}

#[test]
fn log_flush_policy_begin_near_the_32_bit_wrap_and_custom_parameters() {
    let mut p = LogFlushPolicy::default();
    let s: u32 = 0xFFFF_F000;
    p.begin(s);
    assert!(!p.due(1, true, false, s.wrapping_add(9999)));
    assert!(p.due(1, true, false, s.wrapping_add(10_000)));
    assert!(!p.due(1, false, false, s.wrapping_add(299_999)));
    assert!(p.due(1, false, false, s.wrapping_add(300_000)));
    let prm = LogFlushParams {
        period_ms: 50,
        urgent_gap_ms: 5,
        backlog_high: 3,
        failure_report_ms: 20,
    };
    let mut c = LogFlushPolicy::new(prm);
    c.begin(0);
    assert!(!c.due(2, false, false, 49));
    assert!(c.due(2, false, false, 50));
    assert!(c.due(3, false, false, 1));
    assert!(!c.due(2, true, false, 4));
    assert!(c.due(2, true, false, 5));
    assert!(c.on_attempt(false, 100));
    assert!(!c.due(3, false, true, 104));
    assert!(c.due(3, false, true, 105));
    assert!(!c.on_attempt(false, 119));
    assert!(c.on_attempt(false, 120));
}

#[test]
fn log_flush_policy_begin_ends_the_back_off_of_a_failure() {
    let mut p = LogFlushPolicy::default();
    p.begin(0);
    assert!(p.on_attempt(false, 1000));
    assert!(!p.due(1, false, true, 1001));
    p.begin(1001);
    assert!(p.due(1, false, true, 1001));
    assert_eq!(
        (p.attempts(), p.failures(), p.last_attempt_ms()),
        (1, 1, 1001)
    );
}

#[test]
fn log_flush_policy_reference_day_one_info_per_10_s_one_warning_per_hour_at_30() {
    // C++ test suite "slow".
    let mut p = LogFlushPolicy::default();
    p.begin(0);
    let mut backlog = 0;
    let mut urgent = false;
    let mut t = 0u32;
    while t < 24 * 3600 * 1000 {
        if t.is_multiple_of(10_000) {
            backlog += 1;
        }
        if t % 3_600_000 == 1_800_000 {
            backlog += 1;
            urgent = true;
        }
        if p.due(backlog, urgent, false, t) {
            p.on_attempt(true, t);
            backlog = 0;
            urgent = false;
        }
        t += 100;
    }
    assert!(p.attempts() >= 280, "{}", p.attempts());
    assert!(p.attempts() <= 312, "{}", p.attempts());
    assert_eq!(p.failures(), 0);
}

#[test]
fn file_wants_severity_and_syslog_wants() {
    assert!(!file_wants_severity(Severity::Debug));
    assert!(file_wants_severity(Severity::Info));
    assert!(file_wants_severity(Severity::Warning));
    assert!(file_wants_severity(Severity::Error));
    assert!(file_wants_severity(Severity::Critical));
    let all = [
        Severity::Debug,
        Severity::Info,
        Severity::Warning,
        Severity::Error,
        Severity::Critical,
    ];
    for s in all {
        let v = s as u8;
        assert!(!syslog_wants(0, s));
        assert_eq!(syslog_wants(1, s), v >= 2);
        assert_eq!(syslog_wants(2, s), v >= 1);
        assert!(syslog_wants(3, s));
        assert!(!syslog_wants(4, s));
        assert!(!syslog_wants(255, s));
    }
}

#[test]
fn log_file_step_with_max_65536_and_slack_8192() {
    assert_eq!(
        log_file_step(65_526, 10, false, 65_536, 8192),
        LogFileStep::Append
    );
    assert_eq!(
        log_file_step(65_526, 11, false, 65_536, 8192),
        LogFileStep::Rotate
    );
    assert_eq!(
        log_file_step(65_526, 11, true, 65_536, 8192),
        LogFileStep::Append
    );
    assert_eq!(
        log_file_step(73_718, 10, true, 65_536, 8192),
        LogFileStep::Append
    );
    assert_eq!(
        log_file_step(73_718, 11, true, 65_536, 8192),
        LogFileStep::Defer
    );
    assert_eq!(log_file_step(0, 0, true, 0, 0), LogFileStep::Append);
}

#[test]
fn detect_log_gap_cases() {
    assert_eq!(detect_log_gap(10, 11), None);
    assert_eq!(detect_log_gap(10, 5), None);
    assert_eq!(detect_log_gap(10, 0), None);
    assert_eq!(detect_log_gap(0, 1), None);
    // C++ "g unchanged by the failures": the None results
    assert_eq!(detect_log_gap(10, 12), Some(LogGap { from: 11, to: 11 }));
    assert_eq!(detect_log_gap(10, 15), Some(LogGap { from: 11, to: 14 }));
    assert_eq!(detect_log_gap(0, 600), Some(LogGap { from: 1, to: 599 }));
    // uint32 like the C++: cursor + 1 wraps to 0
    assert_eq!(detect_log_gap(u32::MAX, 5), Some(LogGap { from: 0, to: 4 }));
    assert_eq!(LogGap::default(), LogGap { from: 0, to: 0 });
}

#[test]
fn format_log_gap_line_cases() {
    let mut out = [0u8; 64];
    let g = LogGap { from: 11, to: 14 };
    assert_eq!(format_log_gap_line(&g, &mut out), 32);
    assert_text(&out[..32], "#11-14 gap: 4 events not written");
    let big = LogGap {
        from: 1,
        to: 4_294_967_294,
    };
    let n = format_log_gap_line(&big, &mut out);
    assert_text(
        &out[..n],
        "#1-4294967294 gap: 4294967294 events not written",
    );
    // uint32 like the C++: a gap that wraps counts modulo 2^32
    let wrapped = LogGap { from: 5, to: 4 };
    let n = format_log_gap_line(&wrapped, &mut out);
    assert_text(&out[..n], "#5-4 gap: 0 events not written");
    assert_eq!(format_log_gap_line(&g, &mut out[..1]), 0);
    assert_eq!(format_log_gap_line(&g, &mut out[..6]), 5);
    assert_text(&out[..5], "#11-1");
    out[0] = b'q';
    assert_eq!(format_log_gap_line(&g, &mut out[..0]), 0);
    assert_eq!(out[0], b'q');
    // C++ formatLogGapLine(g, nullptr, 10): no Rust form
}

#[test]
fn format_syslog_cases() {
    let mut e = make_event(EventCode::NetDown, Severity::Warning, NO_VALVE, 1, 0, b"");
    let mut out = [0u8; 200];
    let pre = format!(
        "<{}>1 - host vdmot - ",
        128 + u32::from(syslog_severity(Severity::Warning))
    );
    let n = format_syslog(&e, b"msg text", b"host", &mut out);
    let want = format!("{pre}{} - msg text", event_code_name(EventCode::NetDown));
    assert_text(&out[..n], &want);
    assert_eq!(n, want.len());
    assert_text(&out[..n], "<132>1 - host vdmot - net_down - msg text");
    e.epoch = 1_790_136_000; // 2026-09-23T04:00:00Z
    e.severity = Severity::Debug;
    let n = format_syslog(&e, b"m", b"", &mut out);
    assert_text(
        &out[..n],
        format!(
            "<{}>1 2026-09-23T04:00:00Z - vdmot - {} - m",
            128 + u32::from(syslog_severity(Severity::Debug)),
            event_code_name(EventCode::NetDown)
        ),
    );
    // C++ formatSyslog(e, nullptr, nullptr, ..): the empty C strings
    let n = format_syslog(&e, b"", b"", &mut out);
    assert_text(
        &out[..n],
        "<135>1 2026-09-23T04:00:00Z - vdmot - net_down - ",
    );
    // C strings end at their NUL
    let n = format_syslog(&e, b"m\0x", b"h\0y", &mut out);
    assert_text(
        &out[..n],
        "<135>1 2026-09-23T04:00:00Z h vdmot - net_down - m",
    );
    let n = format_syslog(&e, b"m", b"\0h", &mut out);
    assert_text(
        &out[..n],
        "<135>1 2026-09-23T04:00:00Z - vdmot - net_down - m",
    );
    assert_eq!(format_syslog(&e, b"m", b"h", &mut out[..5]), 4);
    assert_text(&out[..4], "<135");
    out[0] = b'q';
    assert_eq!(format_syslog(&e, b"m", b"h", &mut out[..0]), 0);
    assert_eq!(out[0], b'q');
    // C++ formatSyslog(e, "m", "h", nullptr, 10): no Rust form
    // every severity: PRI = local0 (16) * 8 + the RFC 5424 severity
    e.epoch = 0;
    for (sev, pri) in [
        (Severity::Info, 134),
        (Severity::Error, 131),
        (Severity::Critical, 130),
    ] {
        e.severity = sev;
        let n = format_syslog(&e, b"x", b"h", &mut out);
        assert_text(&out[..n], format!("<{pri}>1 - h vdmot - net_down - x"));
    }
}
