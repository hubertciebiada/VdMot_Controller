//! Port of test/native/test_lease_client.cpp: LeaseClient heartbeat, config sync with the STM,
//! ESP emulation, status, events and the RTC record. A C++ `next()` that returns false also
//! leaves the empty line in `out`; the Rust None carries no line.

use super::*;
use crate::stm_codec::{parse_reply, ParseStatus};
use std::format;
use std::string::{String, ToString};
use std::vec::Vec;

fn reply(s: &str) -> Reply {
    let mut r = Reply::default();
    assert_eq!(parse_reply(s.as_bytes(), &mut r), ParseStatus::Ok, "{s}");
    r
}

/// The request text without the " \r\n" every request ends with.
fn text(r: &RequestLine) -> String {
    let mut s = String::from_utf8(r.text.to_vec()).expect("ASCII");
    if s.len() >= 3 {
        s.truncate(s.len() - 3);
    }
    s
}

fn all_pct(timeout: u16, pct: u8) -> LeaseConfig {
    LeaseConfig {
        timeout_min: timeout,
        failsafe_pct: [pct; VALVE_COUNT as usize],
    }
}

/// glcfg reply text for a config.
fn glcfg(c: &LeaseConfig) -> String {
    let mut s = format!("glcfg {}", c.timeout_min);
    for p in c.failsafe_pct {
        s += &format!(" {p}");
    }
    s
}

/// Drives a client like the STM task: next() and scripted completions.
struct Rig {
    lc: LeaseClient,
    last: RequestLine,
}

impl Rig {
    fn new() -> Self {
        Self {
            lc: LeaseClient::default(),
            last: RequestLine::default(),
        }
    }

    fn next(&mut self, now: u32) -> String {
        match self.lc.next(now) {
            None => "-".to_string(),
            Some(r) => {
                let t = text(&r);
                self.last = r;
                t
            }
        }
    }

    /// The periodic heartbeat, answered.
    #[track_caller]
    fn heartbeat(&mut self, now: u32) {
        assert_eq!(self.next(now), "slhbt 1");
        self.ok("slhbt 1 3600", now);
    }

    fn ok(&mut self, line: &str, now: u32) {
        let r = reply(line);
        self.lc
            .on_completion(&self.last, Outcome::Ok, Some(&r), now);
    }

    fn rejected(&mut self, line: &str, now: u32) {
        let r = reply(line);
        self.lc
            .on_completion(&self.last, Outcome::Rejected, Some(&r), now);
    }

    fn timeout(&mut self, now: u32) {
        self.lc
            .on_completion(&self.last, Outcome::Timeout, None, now);
    }

    fn tick(&mut self, now: u32) -> Vec<Event> {
        let mut e: [Event; 8] = Default::default();
        let n = self.lc.tick(now, &mut e);
        e[..n].to_vec()
    }
}

/// Protocol 3 session with a first heartbeat answered at `now` (C++ default cause: Alive).
#[track_caller]
fn start_v3(r: &mut Rig, now: u32, c: RegulatorCause) {
    r.lc.set_protocol(3, now);
    r.lc.set_regulator(c, 0, now);
    let want = if c == RegulatorCause::Alive {
        "slhbt 1"
    } else {
        "slhbt 0"
    };
    assert_eq!(r.next(now), want);
    r.ok("slhbt 1 3600", now);
}

#[track_caller]
fn check_event(e: &Event, code: EventCode, a1: i32, a2: i32) {
    assert_eq!(e.code, code);
    assert_eq!(e.arg1, a1);
    assert_eq!(e.arg2, a2);
    assert_eq!(e.valve, NO_VALVE);
    assert_eq!(e.severity, event_default_severity(code));
}

#[test]
fn constants() {
    assert_eq!(LeaseClient::HEARTBEAT_MS, 60_000);
    assert_eq!(LeaseClient::CONFIG_CHECK_MS, 600_000);
    assert_eq!(LeaseClient::CONFIG_RETRY_MS, 60_000);
    assert_eq!(LeaseClient::CONFIG_MAX_ATTEMPTS, 3);
    assert_eq!(LeaseClient::REGULATOR_EVENT_MS, 60_000);
    assert_eq!(LeaseClient::LOST_REQUEST_MS, 10_000);
    assert_eq!(LeaseClient::RENEW_HOLD_MS, 120_000);
    let c = LeaseConfig::default();
    assert_eq!(c.timeout_min, 60);
    for p in c.failsafe_pct {
        assert_eq!(p, 50);
    }
}

#[test]
fn effective_lease_config_holds_inactive_valves_operator_eq_per_field() {
    let mut cfg = Config::default();
    let lc = effective_lease_config(&cfg);
    assert_eq!(lc.timeout_min, 60);
    for p in lc.failsafe_pct {
        assert_eq!(p, FAILSAFE_HOLD);
    }
    cfg.valves[3].active = true;
    cfg.valves[3].failsafe_pct = 40;
    cfg.valves[4].failsafe_pct = 10; // inactive
    cfg.failsafe.timeout_min = 5;
    let lc = effective_lease_config(&cfg);
    assert_eq!(lc.timeout_min, 5);
    assert_eq!(lc.failsafe_pct[3], 40);
    assert_eq!(lc.failsafe_pct[4], FAILSAFE_HOLD);
    assert_eq!(lc.failsafe_pct[2], FAILSAFE_HOLD);
    // C++ operator== and operator!= per field: the derived PartialEq.
    let a = all_pct(60, 50);
    let mut b = a;
    assert_eq!(a, b);
    b.timeout_min = 61;
    assert_ne!(a, b);
    for v in 0..usize::from(VALVE_COUNT) {
        let mut b = a;
        b.failsafe_pct[v] = 51;
        assert_ne!(a, b, "valve {v}");
    }
}

// ================================================================ heartbeat

#[test]
fn heartbeat_every_60_s_after_the_last_one_completed_at_once_on_an_alive_change() {
    let mut r = Rig::new();
    r.lc.set_protocol(3, 0);
    r.lc.set_regulator(RegulatorCause::BrokerDown, 0, 0);
    assert_eq!(r.next(0), "slhbt 0");
    assert_eq!(r.next(50), "-"); // in flight
    r.ok("slhbt 1 3600", 100);
    assert_eq!(r.next(101), "glcfg"); // config after the first answered heartbeat
    r.ok(&glcfg(&LeaseConfig::default()), 150);
    assert_eq!(r.next(60_099), "-");
    assert_eq!(r.next(60_100), "slhbt 0");
    r.ok("slhbt 1 3500", 60_200);
    r.lc.set_regulator(RegulatorCause::Alive, 0, 70_000);
    assert_eq!(r.next(70_000), "slhbt 1");
    r.timeout(70_400); // no answer: the next one a period later
    assert_eq!(r.next(130_399), "-");
    assert_eq!(r.next(130_400), "slhbt 1");
    r.rejected("slhbt err", 130_500); // error form: also a period later
    assert_eq!(r.next(190_499), "-");
    assert_eq!(r.next(190_500), "slhbt 1");
}

#[test]
fn nothing_on_protocols_0_1_and_2() {
    for p in [0u8, 1, 2] {
        let mut r = Rig::new();
        r.lc.set_protocol(p, 0);
        r.lc.set_regulator(RegulatorCause::Alive, 0, 0);
        assert_eq!(r.next(0), "-");
        assert_eq!(r.next(1_000_000), "-");
    }
    let mut r = Rig::new();
    r.lc.set_protocol(7, 0); // counts as 3
    assert_eq!(r.next(0), "slhbt 0");
}

#[test]
fn a_lost_request_counts_as_a_timeout_after_10_s() {
    let mut r = Rig::new();
    r.lc.set_protocol(3, 0);
    assert_eq!(r.next(0), "slhbt 0");
    assert_eq!(r.next(9999), "-");
    assert_eq!(r.next(10_000), "-"); // timed out now: the next heartbeat 60 s later
    assert_eq!(r.next(69_999), "-");
    assert_eq!(r.next(70_000), "slhbt 0");
}

#[test]
fn completions_of_other_requests_are_ignored() {
    let mut r = Rig::new();
    r.lc.set_protocol(3, 0);
    assert_eq!(r.next(0), "slhbt 0");
    let other = build_heartbeat(true);
    let rep = reply("slhbt 2 0");
    r.lc.on_completion(&other, Outcome::Ok, Some(&rep), 10);
    assert_eq!(r.next(20), "-"); // still in flight
    r.lc.on_completion(&r.last, Outcome::Ok, Some(&rep), 30);
    assert_eq!(r.lc.status(30).state, LeaseState::Expired);
}

#[test]
fn the_heartbeat_reply_updates_the_stm_lease_state() {
    let mut r = Rig::new();
    r.lc.set_protocol(3, 0);
    r.lc.set_regulator(RegulatorCause::Alive, 0, 0);
    assert_eq!(r.next(0), "slhbt 1");
    r.ok("slhbt 1 3540", 1000);
    let s = r.lc.status(1000);
    assert_eq!(s.mode, LeaseMode::Stm);
    assert_eq!(s.state, LeaseState::Running);
    assert_eq!(s.remain_s, 3540);
    let s = r.lc.status(1000 + 9999);
    assert_eq!(s.remain_s, 3531);
    let s = r.lc.status(1000 + 4_000_000);
    assert_eq!(s.remain_s, 0);
}

// ================================================================ config sync

#[test]
fn an_equal_glcfg_is_synced_the_next_compare_10_min_later() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 50));
    start_v3(&mut r, 0, RegulatorCause::Alive);
    assert!(!r.lc.status(0).config_synced);
    assert_eq!(r.next(0), "glcfg");
    r.ok(&glcfg(&all_pct(60, 50)), 100);
    assert!(r.lc.status(100).config_synced);
    assert_eq!(r.next(200), "-");
    // heartbeats keep going
    assert_eq!(r.next(60_000), "slhbt 1");
    r.ok("slhbt 1 3600", 60_000);
    assert_eq!(r.next(600_099), "slhbt 1");
    r.ok("slhbt 1 3600", 600_099);
    assert_eq!(r.next(600_099), "-");
    assert_eq!(r.next(600_100), "glcfg");
}

#[test]
fn push_order_slcfg_sfspo_per_valve_then_verify() {
    let mut r = Rig::new();
    let mut want = all_pct(60, 50);
    want.failsafe_pct[7] = 30;
    r.lc.set_config(&want);
    start_v3(&mut r, 0, RegulatorCause::Alive);
    assert_eq!(r.next(0), "glcfg");
    let mut stm = all_pct(0, 50);
    stm.failsafe_pct[2] = 20;
    r.ok(&glcfg(&stm), 10);
    assert_eq!(r.next(10), "slcfg 60");
    r.ok("slcfg ok", 20);
    assert_eq!(r.next(20), "sfspo 2 50");
    r.ok("sfspo 2 ok", 30);
    assert_eq!(r.next(30), "sfspo 7 30");
    r.ok("sfspo 7 ok", 40);
    assert_eq!(r.next(40), "glcfg");
    assert!(!r.lc.status(40).config_synced);
    r.ok(&glcfg(&want), 50);
    assert!(r.lc.status(50).config_synced);
}

#[test]
fn one_sfspo_255_when_all_valves_want_one_value_and_several_differ() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 40));
    start_v3(&mut r, 0, RegulatorCause::Alive);
    assert_eq!(r.next(0), "glcfg");
    r.ok(&glcfg(&all_pct(60, 50)), 10);
    assert_eq!(r.next(10), "sfspo 255 40");
    r.ok("sfspo 255 ok", 20);
    assert_eq!(r.next(20), "glcfg");
}

#[test]
fn one_differing_valve_is_a_single_sfspo_even_when_all_are_equal() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 40));
    start_v3(&mut r, 0, RegulatorCause::Alive);
    assert_eq!(r.next(0), "glcfg");
    let mut stm = all_pct(60, 40);
    stm.failsafe_pct[5] = 50;
    r.ok(&glcfg(&stm), 10);
    assert_eq!(r.next(10), "sfspo 5 40");
    r.ok("sfspo 5 ok", 20);
    assert_eq!(r.next(20), "glcfg");
}

#[test]
fn two_differing_valves_with_unequal_targets_are_pushed_one_by_one() {
    let mut r = Rig::new();
    let mut want = all_pct(60, 40);
    want.failsafe_pct[11] = FAILSAFE_HOLD;
    r.lc.set_config(&want);
    start_v3(&mut r, 0, RegulatorCause::Alive);
    assert_eq!(r.next(0), "glcfg");
    let mut stm = all_pct(60, 50);
    stm.failsafe_pct[11] = FAILSAFE_HOLD;
    stm.failsafe_pct[1] = 40;
    r.ok(&glcfg(&stm), 10);
    let mut sent: Vec<String> = Vec::new();
    for i in 0..12 {
        let s = r.next(10 + i);
        if s == "glcfg" {
            break;
        }
        let valve = s[6..].split(' ').next().expect("valve").to_string();
        sent.push(s);
        r.ok(&format!("sfspo {valve} ok"), 10 + i);
    }
    assert_eq!(sent.len(), 10); // valves 0, 2..10
    assert_eq!(sent.first().map(String::as_str), Some("sfspo 0 40"));
    assert_eq!(sent.last().map(String::as_str), Some("sfspo 10 40"));
}

#[test]
fn a_failed_attempt_waits_60_s_the_third_one_reports_once_then_every_10_min() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 50));
    start_v3(&mut r, 0, RegulatorCause::Alive);
    assert_eq!(r.next(0), "glcfg");
    r.ok(&glcfg(&all_pct(0, 50)), 0);
    assert_eq!(r.next(0), "slcfg 60");
    r.timeout(1000); // attempt 1: no reply
    assert!(r.tick(1000).is_empty());
    r.heartbeat(60_000);
    assert_eq!(r.next(60_999), "-");
    assert_eq!(r.next(61_000), "glcfg");
    r.ok(&glcfg(&all_pct(0, 50)), 61_000);
    assert_eq!(r.next(61_000), "slcfg 60");
    r.rejected("slcfg err", 62_000); // attempt 2: rejected
    r.heartbeat(120_000);
    assert_eq!(r.next(121_999), "-");
    assert_eq!(r.next(122_000), "glcfg");
    r.ok(&glcfg(&all_pct(0, 50)), 122_000);
    assert_eq!(r.next(122_000), "slcfg 60");
    r.ok("slcfg ok", 122_000);
    assert_eq!(r.next(122_000), "glcfg");
    r.ok(&glcfg(&all_pct(0, 50)), 123_000); // attempt 3: the read-back still differs
    assert!(r.lc.status(123_000).config_failed);
    let ev = r.tick(123_000);
    assert_eq!(ev.len(), 1);
    check_event(&ev[0], EventCode::LeaseConfigFailed, 3, 3);
    assert!(r.tick(124_000).is_empty());
    // Later attempts every 10 min, no further event.
    let t = 123_000;
    for hb in (180_000..723_000).step_by(60_000) {
        r.heartbeat(hb);
    }
    assert_eq!(r.next(722_999), "-");
    assert_eq!(r.next(723_000), "glcfg");
    r.ok(&glcfg(&all_pct(0, 50)), t + 600_000);
    assert_eq!(r.next(t + 600_000), "slcfg 60");
    r.timeout(t + 600_000);
    assert!(r.tick(t + 600_000).is_empty());
    // A successful verify clears config_failed.
    r.lc.set_config(&all_pct(0, 50));
    assert_eq!(r.next(t + 600_001), "glcfg");
    r.ok(&glcfg(&all_pct(0, 50)), t + 600_001);
    assert!(r.lc.status(t + 600_001).config_synced);
    assert!(!r.lc.status(t + 600_001).config_failed);
}

#[test]
fn a_rejected_sfspo_ends_the_attempt_with_reason_2() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 40));
    start_v3(&mut r, 0, RegulatorCause::Alive);
    for attempt in 0..3u32 {
        let t = attempt * 60_000;
        if attempt > 0 {
            assert_eq!(r.next(t), "slhbt 1");
            r.ok("slhbt 1 3600", t);
        }
        assert_eq!(r.next(t), "glcfg");
        r.ok(&glcfg(&all_pct(60, 50)), t);
        assert_eq!(r.next(t), "sfspo 255 40");
        r.rejected("sfspo -1 err 1", t);
    }
    let ev = r.tick(200_000);
    assert_eq!(ev.len(), 1);
    check_event(&ev[0], EventCode::LeaseConfigFailed, 2, 3);
}

#[test]
fn a_glcfg_timeout_is_a_failed_attempt_with_reason_1() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 40));
    start_v3(&mut r, 0, RegulatorCause::Alive);
    for attempt in 0..3u32 {
        let t = attempt * 60_000;
        if attempt > 0 {
            assert_eq!(r.next(t), "slhbt 1");
            r.ok("slhbt 1 3600", t);
        }
        assert_eq!(r.next(t), "glcfg");
        r.timeout(t);
    }
    let ev = r.tick(200_000);
    assert_eq!(ev.len(), 1);
    check_event(&ev[0], EventCode::LeaseConfigFailed, 1, 3);
}

#[test]
fn re_read_triggers() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 50));
    start_v3(&mut r, 0, RegulatorCause::Alive);
    assert_eq!(r.next(0), "glcfg");
    r.ok(&glcfg(&all_pct(60, 50)), 0);
    assert!(r.lc.status(0).config_synced);
    assert_eq!(r.next(1000), "-");
    r.lc.set_config(&all_pct(60, 50)); // the same value: nothing
    assert_eq!(r.next(1000), "-");
    r.lc.set_config(&all_pct(60, 40));
    assert!(!r.lc.status(1000).config_synced);
    assert_eq!(r.next(1000), "glcfg");
    r.ok(&glcfg(&all_pct(60, 40)), 1000);
    // gstax: the timeout drifted.
    let mut s = StmStatus {
        v3: true,
        lease_timeout_min: 60,
        ..StmStatus::default()
    };
    r.lc.on_status(&s, 2000);
    assert_eq!(r.next(2000), "-");
    s.lease_timeout_min = 0;
    r.lc.on_status(&s, 3000);
    assert_eq!(r.next(3000), "glcfg");
    r.ok(&glcfg(&all_pct(60, 40)), 3000);
    // gstax: cfg_events changed (0 was the baseline above).
    s.lease_timeout_min = 60;
    r.lc.on_status(&s, 4000);
    assert_eq!(r.next(4000), "-");
    s.cfg_events = 1;
    r.lc.on_status(&s, 5000);
    assert_eq!(r.next(5000), "glcfg");
    r.ok(&glcfg(&all_pct(60, 40)), 5000);
    // A status without protocol 3 data is ignored.
    let old = StmStatus {
        lease_timeout_min: 0,
        ..StmStatus::default()
    };
    r.lc.on_status(&old, 6000);
    assert_eq!(r.next(6000), "-");
    // STM reboot: heartbeat first, then glcfg.
    r.lc.on_stm_reboot();
    assert_eq!(r.next(7000), "slhbt 1");
    assert_eq!(r.next(7000), "-");
    r.ok("slhbt 1 3600", 7000);
    assert_eq!(r.next(7000), "glcfg");
}

#[test]
fn a_drifting_timeout_is_not_re_read_while_unsynced_no_retry_storm() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 50));
    start_v3(&mut r, 0, RegulatorCause::Alive);
    assert_eq!(r.next(0), "glcfg");
    r.ok(&glcfg(&all_pct(0, 50)), 0);
    assert_eq!(r.next(0), "slcfg 60");
    r.rejected("slcfg err", 0);
    let s = StmStatus {
        v3: true,
        lease_timeout_min: 0,
        ..StmStatus::default()
    };
    r.lc.on_status(&s, 10_000);
    assert_eq!(r.next(10_000), "-");
}

#[test]
fn a_config_change_drops_the_result_of_the_glcfg_in_flight() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 50));
    start_v3(&mut r, 0, RegulatorCause::Alive);
    assert_eq!(r.next(0), "glcfg");
    let old = r.last.clone();
    r.lc.set_config(&all_pct(60, 40));
    let rep = reply(&glcfg(&all_pct(60, 50)));
    r.lc.on_completion(&old, Outcome::Ok, Some(&rep), 10);
    assert!(!r.lc.status(10).config_synced);
    assert_eq!(r.next(10), "glcfg");
}

#[test]
fn an_untrusted_config_is_compared_but_never_pushed() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 50));
    r.lc.set_config_trusted(false);
    assert!(!r.lc.status(0).config_trusted);
    start_v3(&mut r, 0, RegulatorCause::Alive);
    assert_eq!(r.next(0), "glcfg");
    r.ok(&glcfg(&all_pct(0, 50)), 0);
    assert_eq!(r.next(0), "-");
    assert!(!r.lc.status(0).config_synced);
    r.lc.set_config_trusted(false); // unchanged
    assert_eq!(r.next(0), "-");
    r.lc.set_config_trusted(true);
    assert!(r.lc.status(0).config_trusted);
    assert_eq!(r.next(0), "glcfg");
    r.ok(&glcfg(&all_pct(0, 50)), 0);
    assert_eq!(r.next(0), "slcfg 60");
}

#[test]
fn an_untrusted_config_compares_again_10_min_later() {
    let mut r = Rig::new();
    r.lc.set_config_trusted(false);
    start_v3(&mut r, 0, RegulatorCause::Alive);
    assert_eq!(r.next(0), "glcfg");
    r.ok(&glcfg(&all_pct(0, 50)), 0);
    for hb in (60_000..=600_000).step_by(60_000) {
        assert_eq!(r.next(hb), "slhbt 1");
        r.ok("slhbt 1 3600", hb);
        if hb < 600_000 {
            assert_eq!(r.next(hb), "-");
        }
    }
    assert_eq!(r.next(600_000), "glcfg");
}

// ================================================================ renewal hold

#[test]
fn during_an_expired_stm_lease_a_regulator_blip_does_not_renew() {
    let mut r = Rig::new();
    start_v3(&mut r, 0, RegulatorCause::BrokerDown);
    let s = StmStatus {
        v3: true,
        lease: LeaseState::Expired,
        failsafe_mask: 0x00F,
        lease_timeout_min: 60,
        ..StmStatus::default()
    };
    r.lc.on_status(&s, 1000);
    assert_eq!(r.next(1000), "glcfg");
    r.ok(&glcfg(&LeaseConfig::default()), 1000);
    r.lc.set_regulator(RegulatorCause::Alive, 0, 10_000);
    assert_eq!(r.next(10_000), "-");
    r.lc.set_regulator(RegulatorCause::BrokerDown, 0, 40_000); // 30 s blip
    assert_eq!(r.next(40_000), "-");
    r.lc.set_regulator(RegulatorCause::Alive, 0, 50_000);
    r.lc.set_regulator(RegulatorCause::Alive, 0, 50_000 + 119_999);
    assert_eq!(r.next(50_000 + 119_999), "slhbt 0"); // periodic, still dead
    r.ok("slhbt 2 0", 50_000 + 119_999);
    r.lc.set_regulator(RegulatorCause::Alive, 0, 50_000 + 120_000);
    assert_eq!(r.next(50_000 + 120_000), "slhbt 1");
    assert_eq!(
        r.lc.status(50_000 + 120_000).regulator,
        RegulatorCause::Alive
    );
}

#[test]
fn an_mqtt_command_renews_at_once_during_the_failsafe() {
    let mut r = Rig::new();
    start_v3(&mut r, 0, RegulatorCause::BrokerDown);
    let s = StmStatus {
        v3: true,
        lease: LeaseState::Expired,
        ..StmStatus::default()
    };
    r.lc.on_status(&s, 1000);
    assert_eq!(r.next(1000), "glcfg");
    r.ok(&glcfg(&LeaseConfig::default()), 1000);
    r.lc.set_regulator(RegulatorCause::Alive, 0, 2000);
    assert_eq!(r.next(2000), "-");
    r.lc.set_regulator(RegulatorCause::Alive, 1, 3000);
    assert_eq!(r.next(3000), "slhbt 1");
}

#[test]
fn without_a_failsafe_the_renewal_is_immediate() {
    let mut r = Rig::new();
    start_v3(&mut r, 0, RegulatorCause::HaOffline);
    assert_eq!(r.next(0), "glcfg");
    r.ok(&glcfg(&LeaseConfig::default()), 0);
    r.lc.set_regulator(RegulatorCause::Alive, 0, 5000);
    assert_eq!(r.next(5000), "slhbt 1");
}

// ================================================================ emulation

#[test]
fn emulation_after_timeout_min_without_the_regulator_valves_with_a_position() {
    let mut lc = LeaseClient::default();
    let mut c = all_pct(60, FAILSAFE_HOLD);
    c.failsafe_pct[0] = 50;
    c.failsafe_pct[3] = 20;
    lc.set_config(&c);
    lc.set_protocol(1, 0);
    assert_eq!(lc.status(0).mode, LeaseMode::Emulated);
    lc.set_regulator(RegulatorCause::BrokerDown, 0, 1000);
    assert_eq!(lc.emulated_mask(3_601_000 - 1), 0);
    assert_eq!(lc.emulated_mask(3_601_000), 0x009);
    lc.set_regulator(RegulatorCause::BrokerDown, 0, 3_602_000); // still dead: the timer keeps running
    assert_eq!(lc.emulated_mask(3_602_000), 0x009);
    lc.set_protocol(0, 3_603_000); // re-sync: the mode stays
    assert_eq!(lc.status(3_603_000).mode, LeaseMode::Emulated);
    assert_eq!(lc.emulated_mask(3_603_000), 0x009);
    lc.set_protocol(2, 3_603_000);
    lc.set_regulator(RegulatorCause::Alive, 1, 3_604_000); // a command renews at once
    assert_eq!(lc.emulated_mask(3_604_000), 0);
    lc.set_protocol(3, 3_604_000);
    lc.set_regulator(RegulatorCause::BrokerDown, 1, 3_605_000);
    assert_eq!(lc.emulated_mask(3_605_000 + 3_600_000), 0);
    assert_eq!(lc.status(3_605_000).mode, LeaseMode::Stm);
}

#[test]
fn timeout_0_never_emulates_no_protocol_ever_known_is_mode_none() {
    let mut lc = LeaseClient::default();
    lc.set_config(&all_pct(0, 50));
    lc.set_protocol(1, 0);
    lc.set_regulator(RegulatorCause::BrokerDown, 0, 0);
    assert_eq!(lc.emulated_mask(100_000_000), 0);
    assert_eq!(lc.status(0).mode, LeaseMode::None);
    assert_eq!(lc.status(0).state, LeaseState::Off);
    let mut fresh = LeaseClient::default();
    fresh.set_regulator(RegulatorCause::BrokerDown, 0, 0);
    assert_eq!(fresh.status(0).mode, LeaseMode::None);
    assert_eq!(fresh.emulated_mask(100_000_000), 0);
}

#[test]
fn the_lost_timer_starts_at_the_first_dead_call_alive_first_starts_none() {
    let mut lc = LeaseClient::default();
    lc.set_config(&all_pct(5, 50));
    lc.set_protocol(2, 0);
    lc.set_regulator(RegulatorCause::Alive, 0, 0);
    assert_eq!(lc.emulated_mask(10_000_000), 0);
    lc.set_regulator(RegulatorCause::HaOffline, 0, 1000);
    lc.set_regulator(RegulatorCause::BrokerDown, 0, 2000); // another dead cause keeps the timer
    assert_eq!(lc.emulated_mask(1000 + 299_999), 0);
    assert_eq!(lc.emulated_mask(1000 + 300_000), 0x0FFF);
    assert_eq!(
        lc.status(1000 + 300_000).regulator,
        RegulatorCause::BrokerDown
    );
}

#[test]
fn emulation_renews_only_after_120_s_of_life_or_a_command() {
    let mut lc = LeaseClient::default();
    lc.set_config(&all_pct(5, 50));
    lc.set_protocol(1, 0);
    lc.set_regulator(RegulatorCause::BrokerDown, 0, 0);
    assert_ne!(lc.emulated_mask(300_000), 0);
    lc.set_regulator(RegulatorCause::Alive, 0, 300_000);
    assert_ne!(lc.emulated_mask(300_000), 0);
    lc.set_regulator(RegulatorCause::Alive, 0, 300_000 + 119_999);
    assert_ne!(lc.emulated_mask(300_000 + 119_999), 0);
    lc.set_regulator(RegulatorCause::Alive, 0, 300_000 + 120_000);
    assert_eq!(lc.emulated_mask(300_000 + 120_000), 0);
}

#[test]
fn status_of_the_emulation() {
    let mut lc = LeaseClient::default();
    lc.set_config(&all_pct(5, 50));
    lc.set_protocol(1, 0);
    lc.set_regulator(RegulatorCause::Alive, 0, 0);
    let s = lc.status(0);
    assert_eq!(s.state, LeaseState::Running);
    assert_eq!(s.remain_s, 300);
    assert_eq!(s.regulator_lost_s, 0);
    assert_eq!(s.timeout_min, 5);
    assert_eq!(s.regulator, RegulatorCause::Alive);
    lc.set_regulator(RegulatorCause::HaOffline, 0, 1000);
    let s = lc.status(1000 + 120_500);
    assert_eq!(s.regulator_lost_s, 120);
    assert_eq!(s.remain_s, 180);
    assert_eq!(s.regulator, RegulatorCause::HaOffline);
    assert_eq!(s.failsafe_mask, 0);
    let s = lc.status(1000 + 300_000);
    assert_eq!(s.state, LeaseState::Expired);
    assert_eq!(s.remain_s, 0);
    assert_eq!(s.failsafe_mask, 0x0FFF);
}

#[test]
fn status_of_the_stm_lease_from_gstax() {
    let mut r = Rig::new();
    start_v3(&mut r, 0, RegulatorCause::Alive);
    let mut s = StmStatus {
        v3: true,
        lease: LeaseState::Running,
        lease_remain_s: 100,
        failsafe_mask: 0x003,
        ..StmStatus::default()
    };
    r.lc.on_status(&s, 1000);
    let st = r.lc.status(1000 + 30_999);
    assert_eq!(st.state, LeaseState::Running);
    assert_eq!(st.remain_s, 70);
    assert_eq!(st.failsafe_mask, 0x003);
    let st = r.lc.status(1000 + 100_000);
    assert_eq!(st.remain_s, 0);
    s.lease = LeaseState::Expired;
    r.lc.on_status(&s, 2000);
    let st = r.lc.status(2000);
    assert_eq!(st.state, LeaseState::Expired);
    assert_eq!(st.remain_s, 0);
}

// ================================================================ events

#[test]
fn regulator_lost_at_60_s_once_regulator_back_after_it() {
    let mut r = Rig::new();
    r.lc.set_regulator(RegulatorCause::Alive, 0, 0);
    r.lc.set_regulator(RegulatorCause::HaOffline, 0, 1000);
    assert!(r.tick(1000 + 59_999).is_empty());
    let ev = r.tick(1000 + 60_000);
    assert_eq!(ev.len(), 1);
    check_event(&ev[0], EventCode::RegulatorLost, 2, 0);
    assert!(r.tick(1000 + 70_000).is_empty());
    r.lc.set_regulator(RegulatorCause::Alive, 0, 1000 + 90_500);
    let ev = r.tick(1000 + 91_000);
    assert_eq!(ev.len(), 1);
    check_event(&ev[0], EventCode::RegulatorBack, 90, 0);
    assert!(r.tick(1000 + 92_000).is_empty());
    // A short loss is silent both ways.
    r.lc.set_regulator(RegulatorCause::BrokerDown, 0, 200_000);
    assert!(r.tick(250_000).is_empty());
    r.lc.set_regulator(RegulatorCause::Alive, 0, 255_000);
    assert!(r.tick(400_000).is_empty());
}

#[test]
fn failsafe_active_ended_of_the_emulation_also_with_an_all_hold_config() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(5, FAILSAFE_HOLD));
    r.lc.set_protocol(1, 0);
    r.lc.set_regulator(RegulatorCause::BrokerDown, 0, 0);
    assert_eq!(r.tick(299_999).len(), 1); // RegulatorLost only
    let ev = r.tick(300_000);
    assert_eq!(ev.len(), 1);
    check_event(&ev[0], EventCode::FailsafeActive, 0, 2);
    assert!(r.tick(301_000).is_empty());
    r.lc.set_regulator(RegulatorCause::Alive, 1, 360_000);
    let ev = r.tick(360_500);
    assert_eq!(ev.len(), 2);
    check_event(&ev[0], EventCode::RegulatorBack, 360, 0);
    check_event(&ev[1], EventCode::FailsafeEnded, 60, 2);
}

#[test]
fn failsafe_active_ended_of_the_stm_lease() {
    let mut r = Rig::new();
    start_v3(&mut r, 0, RegulatorCause::Alive);
    let mut s = StmStatus {
        v3: true,
        lease: LeaseState::Expired,
        failsafe_mask: 0x00F,
        ..StmStatus::default()
    };
    r.lc.on_status(&s, 1000);
    let ev = r.tick(1000);
    assert_eq!(ev.len(), 1);
    check_event(&ev[0], EventCode::FailsafeActive, 0x00F, 1);
    assert!(r.tick(2000).is_empty());
    s.lease = LeaseState::Running;
    r.lc.on_status(&s, 61_000);
    let ev = r.tick(61_500);
    assert_eq!(ev.len(), 1);
    check_event(&ev[0], EventCode::FailsafeEnded, 60, 1);
}

#[test]
fn tick_respects_the_output_bounds() {
    let mut lc = LeaseClient::default();
    lc.set_regulator(RegulatorCause::BrokerDown, 0, 0);
    assert_eq!(lc.tick(60_000, &mut []), 0); // C++ out nullptr
    let mut e: [Event; 1] = Default::default();
    let mut lc2 = LeaseClient::default();
    lc2.set_regulator(RegulatorCause::BrokerDown, 0, 0);
    assert_eq!(lc2.tick(60_000, &mut e[..0]), 0);
}

// ================================================================ restarts

#[test]
fn snapshot_and_restore_continue_the_lost_timer() {
    let mut before = LeaseClient::default();
    before.set_config(&all_pct(60, 50));
    before.set_protocol(1, 0);
    before.set_regulator(RegulatorCause::BrokerDown, 0, 0);
    let s = before.snapshot(50 * 60_000);
    assert!(s.lost);
    assert!(!s.active);
    assert_eq!(s.mask, 0);
    assert_eq!(s.lost_elapsed_ms, 50 * 60_000);
    let mut after = LeaseClient::default();
    after.set_config(&all_pct(60, 50));
    after.restore(&s, 1000);
    after.set_regulator(RegulatorCause::BrokerDown, 0, 1000);
    after.set_protocol(1, 2000);
    assert_eq!(after.emulated_mask(1000 + 10 * 60_000 - 1), 0);
    assert_eq!(after.emulated_mask(1000 + 10 * 60_000), 0x0FFF);
    let mut e: [Event; 8] = Default::default();
    // FailsafeActive; the loss was reported before
    assert_eq!(after.tick(1000 + 10 * 60_000, &mut e), 1);
    assert_eq!(e[0].code, EventCode::FailsafeActive);
}

#[test]
fn an_active_record_drives_its_mask_at_once_without_a_second_event() {
    let mut before = LeaseClient::default();
    before.set_config(&all_pct(5, 50));
    before.set_protocol(2, 0);
    before.set_regulator(RegulatorCause::BrokerDown, 0, 0);
    let s = before.snapshot(400_000);
    assert!(s.active);
    assert_eq!(s.mask, 0x0FFF);
    let mut after = LeaseClient::default();
    after.set_config(&all_pct(5, 50));
    after.restore(&s, 100);
    assert_eq!(after.emulated_mask(100), 0x0FFF);
    assert_eq!(after.status(100).mode, LeaseMode::Emulated);
    after.set_regulator(RegulatorCause::BrokerDown, 0, 100);
    let mut e: [Event; 8] = Default::default();
    assert_eq!(after.tick(200, &mut e), 0);
    after.set_protocol(2, 300);
    assert_eq!(after.emulated_mask(300), 0x0FFF);
    assert_eq!(after.tick(400, &mut e), 0);
    let mut alive = LeaseClient::default();
    alive.set_regulator(RegulatorCause::Alive, 0, 0);
    let a = alive.snapshot(1000);
    assert!(!a.lost);
    assert_eq!(a.lost_elapsed_ms, 0);
}

#[test]
fn the_rtc_record_round_trip_and_its_rejections() {
    let mut s = LeaseClientSnapshot {
        lost: true,
        active: true,
        mask: 0x0A05,
        lost_elapsed_ms: 0x0102_0304,
    };
    let mut b = [0u8; LEASE_RECORD_SIZE];
    assert_eq!(encode_lease_record(&s, &mut b), LEASE_RECORD_SIZE);
    assert_eq!(&b[..12], b"VDLE\x01\x01\x05\x0A\x04\x03\x02\x01");
    let crc = crc32(&b[..12], 0);
    assert_eq!(b[12], crc as u8);
    assert_eq!(b[13], (crc >> 8) as u8);
    let back = decode_lease_record(&b).expect("valid record");
    assert!(back.lost);
    assert!(back.active);
    assert_eq!(back.mask, 0x0A05);
    assert_eq!(back.lost_elapsed_ms, 0x0102_0304);
    for i in 0..LEASE_RECORD_SIZE {
        for bit in 0..8 {
            let mut c = b;
            c[i] ^= 1 << bit;
            // C++ also checks that the failed decode reset `out` (lost false, mask 0): None.
            assert_eq!(decode_lease_record(&c), None, "byte {i} bit {bit}");
        }
    }
    assert_eq!(decode_lease_record(&b[..13]), None);
    // C++ decodeLeaseRecord(nullptr, 14, ..): no Rust form
    let mut d = b;
    d[4] = 2; // flag byte above 1 with a matching CRC
    let c2 = crc32(&d[..12], 0);
    d[12] = c2 as u8;
    d[13] = (c2 >> 8) as u8;
    assert_eq!(decode_lease_record(&d), None);
    d[4] = 1;
    d[5] = 2;
    let c3 = crc32(&d[..12], 0);
    d[12] = c3 as u8;
    d[13] = (c3 >> 8) as u8;
    assert_eq!(decode_lease_record(&d), None);
    s = LeaseClientSnapshot::default();
    encode_lease_record(&s, &mut b);
    let back = decode_lease_record(&b).expect("valid record");
    assert!(!back.lost);
    assert!(!back.active);
}
