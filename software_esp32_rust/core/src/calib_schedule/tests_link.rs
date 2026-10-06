//! Port of test/native/test_calib_schedule__link.cpp: scheduled calibration confirmed by the
//! STM (attempts, retries, no result, missed slots) and the STM learn-time sync.

use super::*;
use crate::stm_codec::{parse_reply, ParseStatus};
use std::string::String;

/// 2026-09-23 is a Wednesday (3), 2026-09-24 a Thursday. C++ `wed(h, mi, d)`.
fn wed_d(h: u8, mi: u8, d: u8) -> LocalTime {
    LocalTime {
        valid: true,
        year: 2026,
        month: 9,
        mday: d,
        wday: 3 + d - 23,
        hour: h,
        minute: mi,
        ..LocalTime::default()
    }
}

/// C++ `wed(h, mi)` (d = 23).
fn wed(h: u8, mi: u8) -> LocalTime {
    wed_d(h, mi, 23)
}

fn daily(hour: u8) -> CalibScheduleConfig {
    CalibScheduleConfig {
        day_mask: 0x7F,
        hour,
        minute: 0,
    }
}

fn reply(s: &str) -> Reply {
    let mut r = Reply::default();
    assert_eq!(parse_reply(s.as_bytes(), &mut r), ParseStatus::Ok, "{s}");
    r
}

/// The request text without the " \r\n" that ends it.
fn text(r: &RequestLine) -> String {
    let t = &r.text[..];
    String::from_utf8_lossy(t.get(..t.len().saturating_sub(3)).unwrap_or(t)).into_owned()
}

#[derive(Default)]
struct Sync {
    ls: LearnTimeSync,
    last: RequestLine,
}

impl Sync {
    /// The next request as text, "-" for none (C++ also checks the cleared line: None has none).
    fn next(&mut self, now: u32) -> String {
        match self.ls.next(now) {
            None => String::from("-"),
            Some(r) => {
                let t = text(&r);
                self.last = r;
                t
            }
        }
    }

    fn ok(&mut self, line: &str, now: u32) {
        let r = reply(line);
        self.ls
            .on_completion(&self.last, Outcome::Ok, Some(&r), now);
    }

    fn timeout(&mut self, now: u32) {
        self.ls
            .on_completion(&self.last, Outcome::Timeout, None, now);
    }
}

#[test]
fn e1_constants_and_failure_names() {
    assert_eq!(CalibScheduler::RETRY_MS, 600_000);
    assert_eq!(CalibScheduler::RESULT_TIMEOUT_MS, 60_000);
    assert_eq!(calib_failure_name(CalibFailure::None), "none");
    assert_eq!(calib_failure_name(CalibFailure::NoReply), "no_reply");
    assert_eq!(calib_failure_name(CalibFailure::NotSent), "not_sent");
    assert_eq!(calib_failure_name(CalibFailure::NoResult), "no_result");
    assert_eq!(
        calib_failure_name(CalibFailure::Unsupported),
        "stm_unsupported"
    );
    // C++ calibFailureName(5) == "unknown": a Rust CalibFailure cannot hold 5.
    assert_eq!(CalibFailure::from_raw(5), None);
    let all = [
        CalibFailure::None,
        CalibFailure::NoReply,
        CalibFailure::NotSent,
        CalibFailure::NoResult,
        CalibFailure::Unsupported,
    ];
    for (v, f) in all.into_iter().enumerate() {
        assert_eq!(f as usize, v);
        assert_eq!(CalibFailure::from_raw(v as u8), Some(f));
    }
    assert_eq!(CalibFailure::from_raw(255), None);
    assert_eq!(CalibFailure::default(), CalibFailure::None);
}

#[test]
fn e1_a_fire_books_nothing_the_confirmation_books_the_slot_once() {
    let mut s = CalibScheduler::default();
    assert!(!s.attempt_pending());
    assert_eq!(s.attempt_slot(), 0);
    assert_eq!(s.attempts(), 0);
    assert!(!s.on_result(true, 0)); // nothing pending
    assert_eq!(s.evaluate(&daily(0), &wed(0, 0), 1000), CalibDecision::Fire);
    assert_eq!(s.last_slot(), 0);
    assert!(s.attempt_pending());
    assert_eq!(s.attempt_slot(), 20260923);
    assert_eq!(s.attempts(), 1);
    assert_eq!(s.evaluate(&daily(0), &wed(0, 1), 2000), CalibDecision::None); // waiting
    assert!(s.on_result(true, 3000));
    assert_eq!(s.last_slot(), 20260923);
    assert!(!s.attempt_pending());
    assert!(!s.on_result(true, 4000));
    assert_eq!(
        s.evaluate(&daily(0), &wed(1, 0), 700_000),
        CalibDecision::None
    );
    assert_eq!(
        s.evaluate(&daily(0), &wed(3, 0), 800_000),
        CalibDecision::None
    ); // window closed: booked
}

#[test]
fn e1_a_failed_attempt_fires_again_10_min_later_while_the_window_is_open() {
    let mut s = CalibScheduler::default();
    let u: u32 = 100_000;
    assert_eq!(
        s.evaluate(&daily(3), &wed(3, 0), u - 5000),
        CalibDecision::Fire
    );
    assert!(!s.on_result(false, u));
    assert!(!s.attempt_pending());
    assert_eq!(s.last_slot(), 0);
    assert_eq!(
        s.evaluate(&daily(3), &wed(3, 9), u + 599_999),
        CalibDecision::None
    );
    assert_eq!(
        s.evaluate(&daily(3), &wed(3, 10), u + 600_000),
        CalibDecision::Fire
    );
    assert_eq!(s.attempts(), 2);
    assert_eq!(s.late_minutes(), 10);
    assert!(s.on_result(true, u + 601_000));
    assert_eq!(s.last_slot(), 20260923);
}

#[test]
fn e1_no_result_within_60_s_is_a_failed_attempt() {
    let mut s = CalibScheduler::default();
    let u: u32 = 50_000;
    assert_eq!(s.evaluate(&daily(3), &wed(3, 0), u), CalibDecision::Fire);
    assert_eq!(
        s.evaluate(&daily(3), &wed(3, 0), u + 59_999),
        CalibDecision::None
    );
    assert_eq!(
        s.evaluate(&daily(3), &wed(3, 1), u + 60_000),
        CalibDecision::NoResult
    );
    assert!(!s.attempt_pending());
    assert!(!s.on_result(true, u + 60_001)); // a late result is ignored
    assert_eq!(s.last_slot(), 0);
    assert_eq!(
        s.evaluate(&daily(3), &wed(3, 10), u + 60_000 + 599_999),
        CalibDecision::None
    );
    assert_eq!(
        s.evaluate(&daily(3), &wed(3, 11), u + 60_000 + 600_000),
        CalibDecision::Fire
    );
    assert_eq!(s.attempts(), 2);
}

#[test]
fn e1_the_window_closes_after_failed_attempts_missed_once_then_the_next_date() {
    let mut s = CalibScheduler::default();
    assert_eq!(s.evaluate(&daily(3), &wed(3, 0), 0), CalibDecision::Fire);
    s.on_result(false, 1000);
    assert_eq!(
        s.evaluate(&daily(3), &wed(3, 20), 700_000),
        CalibDecision::Fire
    );
    s.on_result(false, 701_000);
    assert_eq!(
        s.evaluate(&daily(3), &wed(4, 59), 1_300_999),
        CalibDecision::None
    ); // open, retry hold
    assert_eq!(
        s.evaluate(&daily(3), &wed(5, 0), 2_100_000),
        CalibDecision::Missed
    );
    assert_eq!(s.attempt_slot(), 20260923);
    assert_eq!(s.attempts(), 2);
    assert_eq!(
        s.evaluate(&daily(3), &wed(5, 1), 2_200_000),
        CalibDecision::None
    );
    assert_eq!(
        s.evaluate(&daily(3), &wed_d(3, 0, 24), 90_000_000),
        CalibDecision::Fire
    );
    assert_eq!(s.attempt_slot(), 20260924);
    assert_eq!(s.attempts(), 1);
}

#[test]
fn e1_an_attempt_pending_over_the_window_end_reports_no_result_first_then_missed() {
    let mut s = CalibScheduler::default();
    assert_eq!(s.evaluate(&daily(3), &wed(4, 59), 0), CalibDecision::Fire);
    assert_eq!(
        s.evaluate(&daily(3), &wed(5, 0), 30_000),
        CalibDecision::None
    ); // still waiting
    assert_eq!(
        s.evaluate(&daily(3), &wed(5, 1), 60_000),
        CalibDecision::NoResult
    );
    assert_eq!(
        s.evaluate(&daily(3), &wed(5, 1), 70_000),
        CalibDecision::Missed
    );
    assert_eq!(
        s.evaluate(&daily(3), &wed(5, 2), 80_000),
        CalibDecision::None
    );
}

#[test]
fn e1_another_date_closes_an_unconfirmed_slot_too() {
    let mut s = CalibScheduler::default();
    assert_eq!(s.evaluate(&daily(3), &wed(3, 0), 0), CalibDecision::Fire);
    s.on_result(false, 0);
    assert_eq!(
        s.evaluate(&daily(3), &wed_d(1, 0, 24), 80_000_000),
        CalibDecision::Missed
    );
    assert_eq!(
        s.evaluate(&daily(3), &wed_d(3, 0, 24), 90_000_000),
        CalibDecision::Fire
    );
}

#[test]
fn e1_a_reboot_inside_the_window_fires_again_the_slot_was_never_booked() {
    let mut a = CalibScheduler::default();
    a.restore_last_slot(20260922);
    assert_eq!(a.evaluate(&daily(3), &wed(3, 0), 0), CalibDecision::Fire);
    a.on_result(false, 0);
    let mut b = CalibScheduler::default();
    b.restore_last_slot(20260922);
    assert_eq!(b.evaluate(&daily(3), &wed(3, 5), 5000), CalibDecision::Fire);
}

#[test]
fn e1_a_confirmed_slot_is_never_reported_as_missed() {
    let mut s = CalibScheduler::default();
    assert_eq!(s.evaluate(&daily(3), &wed(3, 0), 0), CalibDecision::Fire);
    assert!(s.on_result(true, 0));
    assert_eq!(
        s.evaluate(&daily(3), &wed(6, 0), 100_000),
        CalibDecision::None
    );
    assert_eq!(
        s.evaluate(&daily(3), &wed_d(1, 0, 24), 200_000),
        CalibDecision::None
    );
}

// ================================================================ learn time

#[test]
fn learn_time_0_while_the_esp_schedule_is_on_else_one_week() {
    assert_eq!(STM_LEARN_TIME_DEFAULT_S, 604_800);
    let mut c = CalibScheduleConfig {
        day_mask: 0,
        ..CalibScheduleConfig::default()
    };
    assert_eq!(stm_learn_time(&c), 604_800);
    c.day_mask = 9;
    assert_eq!(stm_learn_time(&c), 0);
    c.day_mask = 0x80;
    assert_eq!(stm_learn_time(&c), 604_800);
    c.day_mask = 1;
    c.hour = 24;
    assert_eq!(stm_learn_time(&c), 604_800);
    c.hour = 23;
    c.minute = 60;
    assert_eq!(stm_learn_time(&c), 604_800);
    c.minute = 59;
    assert_eq!(stm_learn_time(&c), 0);
    assert_eq!(LearnTimeSync::RETRY_MS, 60_000);
    assert_eq!(LearnTimeSync::LOST_REQUEST_MS, 10_000);
}

#[test]
fn learn_time_protocol_3_reads_writes_a_different_value_and_verifies() {
    let mut s = Sync::default();
    s.ls.set_protocol(3);
    assert_eq!(s.next(0), "-"); // no desired value yet
    s.ls.set_desired(0);
    assert_eq!(s.next(0), "gtlnt");
    assert_eq!(s.next(1), "-");
    s.ok("gtlnt 604800", 10);
    assert!(s.ls.have_stm_value());
    assert_eq!(s.ls.stm_value(), 604_800);
    assert_eq!(s.next(10), "stlnt 0");
    s.ok("stlnt", 20);
    assert_eq!(s.next(20), "gtlnt");
    s.ok("gtlnt 0", 30);
    assert_eq!(s.ls.stm_value(), 0);
    assert_eq!(s.next(40), "-");
    assert_eq!(s.next(10_000_000), "-");
    s.ls.set_desired(0); // unchanged
    assert_eq!(s.next(10_000_000), "-");
    s.ls.set_desired(604_800);
    assert_eq!(s.next(10_000_000), "gtlnt");
    s.ok("gtlnt 0", 10_000_000);
    assert_eq!(s.next(10_000_000), "stlnt 604800");
    s.ls.on_stm_reboot();
    assert_eq!(s.next(10_000_001), "gtlnt");
    s.ok("gtlnt 604800", 10_000_002);
    assert_eq!(s.next(10_000_003), "-");
}

#[test]
fn learn_time_protocol_3_failures_retry_after_60_s() {
    let mut s = Sync::default();
    s.ls.set_protocol(3);
    s.ls.set_desired(0);
    assert_eq!(s.next(0), "gtlnt");
    s.timeout(100);
    assert_eq!(s.next(60_099), "-");
    assert_eq!(s.next(60_100), "gtlnt");
    s.ok("gtlnt 5", 60_100);
    assert_eq!(s.next(60_100), "stlnt 0");
    s.timeout(60_200);
    assert_eq!(s.next(120_199), "-");
    assert_eq!(s.next(120_200), "gtlnt");
    s.ok("gtlnt 5", 120_200);
    assert_eq!(s.next(120_200), "stlnt 0");
    s.ok("stlnt", 120_300);
    assert_eq!(s.next(120_300), "gtlnt");
    s.ok("gtlnt 5", 120_400); // the STM kept its value: a failure
    assert_eq!(s.next(180_399), "-");
    assert_eq!(s.next(180_400), "gtlnt");
    assert_eq!(s.next(180_400 + 9999), "-");
    assert_eq!(s.next(180_400 + 10_000), "-"); // lost: a failure, next try 60 s later
    assert_eq!(s.next(180_400 + 10_000 + 59_999), "-");
    assert_eq!(s.next(180_400 + 10_000 + 60_000), "gtlnt");
}

#[test]
fn learn_time_protocols_1_2_send_stlnt_0_once_per_session_the_default_only_after_it() {
    let mut s = Sync::default();
    s.ls.set_protocol(1);
    s.ls.set_desired(604_800);
    assert_eq!(s.next(0), "-"); // nothing was switched off in this session
    s.ls.set_desired(0);
    assert_eq!(s.next(0), "stlnt 0");
    s.ok("stlnt", 10);
    assert_eq!(s.next(20), "-");
    assert_eq!(s.next(10_000_000), "-");
    s.ls.set_desired(604_800);
    assert_eq!(s.next(10_000_000), "stlnt 604800");
    s.ok("stlnt", 10_000_010);
    assert_eq!(s.next(10_000_020), "-");
    assert!(!s.ls.have_stm_value());
    s.ls.set_desired(0);
    assert_eq!(s.next(10_000_030), "stlnt 0");
    s.timeout(10_000_040); // failed: again 60 s later
    assert_eq!(s.next(10_060_039), "-");
    assert_eq!(s.next(10_060_040), "stlnt 0");
    s.ok("stlnt", 10_060_050);
    s.ls.set_protocol(2); // new session: once more
    assert_eq!(s.next(10_060_060), "stlnt 0");
    s.ok("stlnt", 10_060_070);
    s.ls.on_stm_reboot();
    assert_eq!(s.next(10_060_080), "stlnt 0");
    s.ok("stlnt", 10_060_090);
    s.ls.set_protocol(0);
    s.ls.set_desired(604_800);
    assert_eq!(s.next(20_000_000), "-");
}

#[test]
fn learn_time_completions_of_other_requests_are_ignored() {
    let mut s = Sync::default();
    s.ls.set_protocol(3);
    s.ls.set_desired(0);
    assert_eq!(s.next(0), "gtlnt");
    // C++ REQUIRE(buildSetLearnTime(0, other)): the Rust builder cannot fail.
    let other = build_set_learn_time(0);
    let r = reply("gtlnt 0");
    s.ls.on_completion(&other, Outcome::Ok, Some(&r), 10);
    assert!(!s.ls.have_stm_value());
    assert_eq!(s.next(20), "-");
}

#[test]
fn e1_the_slot_epoch_takes_the_utc_offset_of_the_reference_time() {
    let midnight: i64 = 1_790_121_600; // 2026-09-23 00:00 UTC
    let mut utc = wed(3, 5);
    utc.second = 30;
    utc.epoch = midnight + 3 * 3600 + 5 * 60 + 30;
    assert_eq!(
        calib_slot_epoch(20260930, 3, 0, &utc),
        midnight + 7 * 86400 + 3 * 3600
    );
    assert_eq!(
        calib_slot_epoch(20260923, 23, 59, &utc),
        midnight + 23 * 3600 + 59 * 60
    );
    assert_eq!(
        calib_slot_epoch(20261001, 0, 1, &utc),
        midnight + 8 * 86400 + 60
    );
    let mut cest = wed(5, 5); // UTC+2: 05:05 local is 03:05 UTC
    cest.epoch = midnight + 3 * 3600 + 5 * 60;
    assert_eq!(
        calib_slot_epoch(20260930, 3, 0, &cest),
        midnight + 7 * 86400 + 3600
    );
    let mut west = wed_d(22, 5, 22); // UTC-5: Tuesday 22:05 local is Wednesday 03:05 UTC
    west.wday = 2;
    west.epoch = midnight + 3 * 3600 + 5 * 60;
    assert_eq!(calib_slot_epoch(20260923, 3, 0, &west), midnight + 8 * 3600);
}

#[test]
fn e1_no_slot_epoch_without_a_slot_or_a_valid_reference_time() {
    let mut r = wed(3, 5);
    r.epoch = 1_790_121_600 + 3 * 3600 + 5 * 60;
    assert_eq!(calib_slot_epoch(0, 3, 0, &r), 0);
    assert_eq!(calib_slot_epoch(20260923, 3, 5, &r), r.epoch);
    r.valid = false;
    assert_eq!(calib_slot_epoch(20260930, 3, 0, &r), 0);
}
