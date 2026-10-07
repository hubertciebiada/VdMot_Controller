//! Port of test/native/test_lease_client__mut.cpp: LeaseClient attempt counters, protocol
//! changes, exact durations in events and status, restored records and the RTC record checks
//! behind a valid CRC.

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

fn glcfg(c: &LeaseConfig) -> String {
    let mut s = format!("glcfg {}", c.timeout_min);
    for p in c.failsafe_pct {
        s += &format!(" {p}");
    }
    s
}

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

    fn ok(&mut self, line: &str, now: u32) {
        let r = reply(line);
        self.lc
            .on_completion(&self.last, Outcome::Ok, Some(&r), now);
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

#[track_caller]
fn start_v3(r: &mut Rig, now: u32) {
    r.lc.set_protocol(3, now);
    r.lc.set_regulator(RegulatorCause::Alive, 0, now);
    assert_eq!(r.next(now), "slhbt 1");
    r.ok("slhbt 1 3600", now);
}

/// A failed glcfg attempt at `now`: the heartbeat first when it is due.
#[track_caller]
fn fail_glcfg(r: &mut Rig, now: u32) {
    let mut n = r.next(now);
    if n == "slhbt 1" {
        r.ok("slhbt 1 3600", now);
        n = r.next(now);
    }
    assert_eq!(n, "glcfg");
    r.timeout(now);
}

fn has(ev: &[Event], code: EventCode) -> bool {
    ev.iter().any(|e| e.code == code)
}

fn find(ev: &[Event], code: EventCode) -> Option<&Event> {
    ev.iter().find(|e| e.code == code)
}

#[test]
fn a_glcfg_that_differs_only_in_valve_0_is_pushed() {
    let mut r = Rig::new();
    let mut want = all_pct(60, 50);
    want.failsafe_pct[0] = 30;
    r.lc.set_config(&want);
    start_v3(&mut r, 0);
    assert_eq!(r.next(0), "glcfg");
    r.ok(&glcfg(&all_pct(60, 50)), 10);
    assert!(!r.lc.status(10).config_synced);
    assert_eq!(r.next(10), "sfspo 0 30");
}

#[test]
fn a_config_change_starts_the_attempt_count_again() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 50));
    start_v3(&mut r, 0);
    fail_glcfg(&mut r, 10);
    r.lc.set_config(&all_pct(60, 40));
    fail_glcfg(&mut r, 20);
    fail_glcfg(&mut r, 20 + LeaseClient::CONFIG_RETRY_MS);
    assert!(!r.lc.status(70_000).config_failed);
    assert!(!has(&r.tick(70_000), EventCode::LeaseConfigFailed));
    fail_glcfg(&mut r, 20 + 2 * LeaseClient::CONFIG_RETRY_MS);
    assert!(r.lc.status(130_000).config_failed);
}

#[test]
fn a_success_starts_the_attempt_count_again() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 50));
    start_v3(&mut r, 0);
    fail_glcfg(&mut r, 10);
    fail_glcfg(&mut r, 10 + LeaseClient::CONFIG_RETRY_MS);
    let t = 10 + 2 * LeaseClient::CONFIG_RETRY_MS;
    assert_eq!(r.next(t), "slhbt 1");
    r.ok("slhbt 1 3600", t);
    assert_eq!(r.next(t), "glcfg");
    r.ok(&glcfg(&all_pct(60, 50)), t);
    assert!(r.lc.status(130_000).config_synced);
    // gstax: the timeout drifted, a re-read without a config change
    let s = StmStatus {
        v3: true,
        lease_timeout_min: 0,
        ..StmStatus::default()
    };
    r.lc.on_status(&s, 130_000);
    fail_glcfg(&mut r, 130_000);
    fail_glcfg(&mut r, 130_000 + LeaseClient::CONFIG_RETRY_MS);
    assert!(!r.lc.status(200_000).config_failed);
}

#[test]
fn a_protocol_above_3_counts_as_3() {
    let mut r = Rig::new();
    r.lc.set_protocol(4, 0);
    assert_eq!(r.lc.status(0).mode, LeaseMode::Stm);
    r.lc.set_regulator(RegulatorCause::Alive, 0, 0);
    assert_eq!(r.next(0), "slhbt 1");
}

#[test]
fn only_the_return_to_protocol_3_starts_a_new_session() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 50));
    start_v3(&mut r, 0);
    assert_eq!(r.next(0), "glcfg");
    r.ok(&glcfg(&all_pct(60, 50)), 0);
    assert!(r.lc.status(0).config_synced);
    r.lc.set_protocol(1, 1000);
    assert!(r.lc.status(1000).config_synced);
    assert_eq!(r.next(1000), "-");
    r.lc.set_protocol(3, 2000);
    assert_eq!(r.next(2000), "slhbt 1");
    r.ok("slhbt 1 3600", 2000);
    assert_eq!(r.next(2000), "glcfg");
}

#[test]
fn the_cfg_events_of_a_rebooted_stm_are_a_new_baseline() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 50));
    start_v3(&mut r, 0);
    assert_eq!(r.next(0), "glcfg");
    r.ok(&glcfg(&all_pct(60, 50)), 0);
    let mut s = StmStatus {
        v3: true,
        lease_timeout_min: 60,
        cfg_events: 5,
        ..StmStatus::default()
    };
    r.lc.on_status(&s, 1000);
    assert!(r.lc.status(1000).config_synced);
    r.lc.on_stm_reboot();
    assert_eq!(r.next(2000), "slhbt 1");
    r.ok("slhbt 1 3600", 2000);
    assert_eq!(r.next(2000), "glcfg");
    r.ok(&glcfg(&all_pct(60, 50)), 2000);
    s.cfg_events = 0; // counted again since the reboot
    r.lc.on_status(&s, 3000);
    assert!(r.lc.status(3000).config_synced);
    assert_eq!(r.next(3000), "-");
}

#[test]
fn exact_seconds_of_the_lost_regulator_in_status_regulator_back_and_the_snapshot() {
    let mut r = Rig::new();
    start_v3(&mut r, 0);
    r.lc.set_regulator(RegulatorCause::BrokerDown, 0, 0);
    assert_eq!(r.tick(60_000).len(), 1);
    assert_eq!(r.lc.status(999_000).regulator_lost_s, 999);
    assert_eq!(r.lc.status(1_998_000).regulator_lost_s, 1998);
    r.lc.set_regulator(RegulatorCause::Alive, 0, 999_000);
    assert_eq!(r.lc.status(999_000).regulator_lost_s, 0);
    assert!(!r.lc.snapshot(999_000).lost);
    let ev = r.tick(999_000);
    let back = find(&ev, EventCode::RegulatorBack).expect("RegulatorBack");
    assert_eq!(back.arg1, 999);
}

#[test]
fn a_timeout_of_1_min_emulates() {
    let mut lc = LeaseClient::default();
    lc.set_config(&all_pct(1, 50));
    lc.set_protocol(2, 0);
    assert_eq!(lc.status(0).mode, LeaseMode::Emulated);
    lc.set_regulator(RegulatorCause::BrokerDown, 0, 0);
    assert_eq!(lc.status(60_000).state, LeaseState::Expired);
}

#[test]
fn a_restored_active_record_without_a_lost_regulator_drives_its_own_mask() {
    let s = LeaseClientSnapshot {
        active: true,
        mask: 0x0005,
        ..LeaseClientSnapshot::default()
    };
    let mut lc = LeaseClient::default();
    lc.set_config(&all_pct(60, 50));
    lc.restore(&s, 100);
    assert_eq!(lc.status(100).state, LeaseState::Expired);
    assert_eq!(lc.emulated_mask(100), 0x0005);
    assert_eq!(lc.status(100).failsafe_mask, 0x0005);
    lc.set_regulator(RegulatorCause::Alive, 0, 100);
    assert_eq!(lc.emulated_mask(200), 0x0005);
}

#[test]
fn exactly_two_differing_valves_with_one_wanted_value_are_one_sfspo_255() {
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 40));
    start_v3(&mut r, 0);
    assert_eq!(r.next(0), "glcfg");
    let mut stm = all_pct(60, 40);
    stm.failsafe_pct[3] = 50;
    stm.failsafe_pct[9] = 20;
    r.ok(&glcfg(&stm), 10);
    assert_eq!(r.next(10), "sfspo 255 40");
}

#[test]
fn a_completion_of_another_request_of_the_same_length_is_ignored() {
    let mut r = Rig::new();
    r.lc.set_protocol(3, 0);
    r.lc.set_regulator(RegulatorCause::Alive, 0, 0);
    assert_eq!(r.next(0), "slhbt 1");
    let other = build_heartbeat(false);
    assert_eq!(other.text.len(), r.last.text.len());
    let rep = reply("slhbt 1 3600");
    r.lc.on_completion(&other, Outcome::Ok, Some(&rep), 10);
    assert_eq!(r.next(20), "-");
    r.ok("slhbt 1 3600", 30);
    assert_eq!(r.next(30), "glcfg");
}

#[test]
fn failsafe_ended_once_with_the_exact_seconds_stm_and_emulation() {
    let mut r = Rig::new();
    start_v3(&mut r, 0);
    let mut s = StmStatus {
        v3: true,
        lease: LeaseState::Expired,
        ..StmStatus::default()
    };
    r.lc.on_status(&s, 1000);
    assert_eq!(r.tick(1000).len(), 1);
    s.lease = LeaseState::Running;
    r.lc.on_status(&s, 1000 + 999_000);
    let ev = r.tick(1000 + 999_000);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].code, EventCode::FailsafeEnded);
    assert_eq!(ev[0].arg1, 999);
    assert!(r.tick(1000 + 999_500).is_empty());

    let mut e = Rig::new();
    e.lc.set_config(&all_pct(1, 50));
    e.lc.set_protocol(2, 0);
    e.lc.set_regulator(RegulatorCause::BrokerDown, 0, 0);
    let ev = e.tick(60_000);
    assert!(has(&ev, EventCode::FailsafeActive));
    e.lc.set_regulator(RegulatorCause::Alive, 1, 60_000 + 999_000);
    let ev = e.tick(60_000 + 999_000);
    let ended = find(&ev, EventCode::FailsafeEnded).expect("FailsafeEnded");
    assert_eq!(ended.arg1, 999);
    assert!(e.tick(60_000 + 999_500).is_empty());
}

#[test]
fn a_restored_loss_of_exactly_60_s_was_reported_already() {
    let mut s = LeaseClientSnapshot {
        lost: true,
        lost_elapsed_ms: LeaseClient::REGULATOR_EVENT_MS,
        ..LeaseClientSnapshot::default()
    };
    let mut lc = LeaseClient::default();
    lc.restore(&s, 100_000);
    let mut e: [Event; 8] = Default::default();
    assert_eq!(lc.tick(100_000, &mut e), 0);
    s.lost_elapsed_ms = LeaseClient::REGULATOR_EVENT_MS - 1;
    let mut lc2 = LeaseClient::default();
    lc2.restore(&s, 100_000);
    assert_eq!(lc2.tick(100_001, &mut e), 1);
}

#[test]
fn a_failed_sfspo_of_one_valve_re_reads_before_pushing_again() {
    // Rust addition: a per-valve push that fails keeps its mask bit, and the next attempt
    // still starts with glcfg (the C++ cases fail only slcfg and sfspo 255 pushes).
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(60, 40));
    start_v3(&mut r, 0);
    assert_eq!(r.next(0), "glcfg");
    let mut stm = all_pct(60, 40);
    stm.failsafe_pct[5] = 50;
    r.ok(&glcfg(&stm), 0);
    assert_eq!(r.next(0), "sfspo 5 40");
    r.timeout(1000);
    assert_eq!(r.next(60_000), "slhbt 1");
    r.ok("slhbt 1 3600", 60_000);
    assert_eq!(r.next(61_000), "glcfg");
    r.ok(&glcfg(&stm), 61_000);
    assert_eq!(r.next(61_000), "sfspo 5 40");
}

#[test]
fn a_config_value_the_codec_refuses_hands_out_the_empty_request() {
    // Rust addition, a kept C++ quirk (docs/rust/PORT-NOTES.md): next() hands out the empty
    // request a failed build leaves; the link refuses it, so it is lost after LOST_REQUEST_MS
    // and each round fails with reason 1.
    let mut r = Rig::new();
    r.lc.set_config(&all_pct(3, 50)); // slcfg takes 0 or 5..1440
    start_v3(&mut r, 0);
    for round in 0..3u32 {
        let t = round * 70_000;
        if round > 0 {
            assert_eq!(r.next(t), "slhbt 1");
            r.ok("slhbt 1 3600", t);
        }
        assert_eq!(r.next(t), "glcfg");
        r.ok(&glcfg(&all_pct(60, 50)), t);
        assert_eq!(r.lc.next(t), Some(RequestLine::default()));
        assert_eq!(r.lc.next(t + 9999), None); // in flight
        assert_eq!(r.lc.next(t + 10_000), None); // lost: a failed attempt
    }
    let ev = r.tick(150_000);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].code, EventCode::LeaseConfigFailed);
    assert_eq!((ev[0].arg1, ev[0].arg2), (1, 3));
    // A percent of 101..254: the same for sfspo.
    let mut r = Rig::new();
    let mut want = all_pct(60, 50);
    want.failsafe_pct[4] = 101;
    r.lc.set_config(&want);
    start_v3(&mut r, 0);
    assert_eq!(r.next(0), "glcfg");
    r.ok(&glcfg(&all_pct(60, 50)), 0);
    assert_eq!(r.lc.next(0), Some(RequestLine::default()));
}

#[test]
fn the_rtc_record_checks_every_magic_byte_and_each_flag_on_its_own() {
    let mut s = LeaseClientSnapshot {
        lost: true,
        mask: 0x0003,
        lost_elapsed_ms: 1234,
        ..LeaseClientSnapshot::default()
    };
    let mut b = [0u8; LEASE_RECORD_SIZE];
    encode_lease_record(&s, &mut b);
    for i in 0..4 {
        let mut c = b;
        c[i] = b'X';
        let crc = crc32(&c[..12], 0);
        c[12] = crc as u8;
        c[13] = (crc >> 8) as u8;
        assert_eq!(decode_lease_record(&c), None, "magic byte {i}");
    }
    let back = decode_lease_record(&b).expect("valid record");
    assert!(back.lost);
    assert!(!back.active);
    s.lost = false;
    s.active = true;
    encode_lease_record(&s, &mut b);
    let back = decode_lease_record(&b).expect("valid record");
    assert!(!back.lost);
    assert!(back.active);
}
