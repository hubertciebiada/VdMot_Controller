//! Port of test/native/test_link_policy.cpp: LinkPolicy (queue, priorities, timeouts, retries,
//! R6 reset policy, states) and RebootDetector. C++ cases that hand the policy a request the
//! Rust types cannot hold (a command number >= kCmdCount, len > kRequestMaxLen, priority 3) or
//! name an out-of-range LinkState have no Rust form; they are named in comments.

use super::*;
use crate::common::OneWireId;
use crate::stm_codec::{
    build_assembly, build_calibrate, build_detect, build_get_proto, build_get_status_v3,
    build_get_version, build_heartbeat, build_leave_safe_mode, build_match_sensors, build_profile,
    build_scan_one_wire, build_service_move, build_set_breakaway, build_set_failsafe,
    build_set_lease_timeout, build_set_motor_chars, build_set_target, build_set_valve_sensors,
    build_soft_reset, build_stop, build_temp_count, build_temp_data, build_temp_list,
    build_valve_data, build_valve_sensors, build_volt_count, build_volt_data, build_volt_list,
    parse_reply, Breakaway, MotorChars, MoveDir, ParseStatus, REQUEST_MAX_LEN,
};
use std::format;
use std::string::{String, ToString};

fn valve_data(v: u8) -> RequestLine {
    build_valve_data(v).expect("valve")
}

fn set_target(v: u8, pos: u8) -> RequestLine {
    build_set_target(v, pos).expect("target")
}

fn calibrate(v: u8) -> RequestLine {
    build_calibrate(v).expect("valve")
}

fn proto() -> RequestLine {
    build_get_proto()
}

fn reply(s: &str) -> Reply {
    let mut r = Reply::default();
    assert_eq!(parse_reply(s.as_bytes(), &mut r), ParseStatus::Ok, "{s}");
    r
}

fn gvlvd(v: u8) -> Reply {
    reply(&format!("gvlvd {v} 42 18 1 215 -500 57 3120 3350 230 0"))
}

fn text(r: Option<&RequestLine>) -> String {
    r.map_or_else(
        || "<null>".to_string(),
        |r| String::from_utf8(r.text.to_vec()).expect("ASCII"),
    )
}

/// Sends the next line at `now`, returns its text.
fn send(lp: &mut LinkPolicy, now: u32) -> String {
    let t = text(lp.next_to_send(now));
    if t != "<null>" {
        lp.on_sent(now);
    }
    t
}

/// Lets the outstanding request time out completely; returns the completion.
fn expire(lp: &mut LinkPolicy, now: &mut u32) -> Completion {
    for _ in 0..100 {
        *now += 5000;
        if let Some(c) = lp.poll(*now) {
            return c;
        }
        *now += 10;
        assert!(lp.next_to_send(*now).is_some());
        lp.on_sent(*now);
    }
    panic!("request never completed");
}

#[test]
fn state_names() {
    assert_eq!(link_state_name(LinkState::Unknown), "unknown");
    assert_eq!(link_state_name(LinkState::Up), "up");
    assert_eq!(link_state_name(LinkState::Degraded), "degraded");
    assert_eq!(link_state_name(LinkState::Down), "down");
    assert_eq!(link_state_name(LinkState::Booting), "booting");
    assert_eq!(link_state_name(LinkState::Suspended), "suspended");
    // C++ linkStateName(static_cast<LinkState>(42)) == "unknown": no Rust form.
    assert_eq!(LinkState::from_raw(42), None);
}

#[test]
fn enum_numbers_round_trip() {
    // Rust: from_raw over all of u8.
    for v in 0..=u8::MAX {
        assert_eq!(
            LinkState::from_raw(v).map(|s| s as u8),
            (v < 6).then_some(v)
        );
        assert_eq!(Priority::from_raw(v).map(|p| p as u8), (v < 3).then_some(v));
    }
    assert!(Priority::User < Priority::Config && Priority::Config < Priority::Poll);
    assert_eq!(Outcome::Timeout as u8, 2);
    assert_eq!(EnqueueResult::Invalid as u8, 3);
    assert_eq!(RebootDetectorRecovery::CheckStatus as u8, 2);
    let c = Completion::default();
    assert_eq!(
        (c.priority, c.outcome, c.tag, c.attempts),
        (Priority::Poll, Outcome::Timeout, 0, 0)
    );
}

#[test]
fn defaults_match_the_binding_table() {
    let p = LinkParams::default();
    assert_eq!(p.timeout_ms, 400);
    assert_eq!(p.long_timeout_ms, 1500);
    assert_eq!(p.slow_timeout_ms, 3000);
    assert_eq!(p.retries, 2);
    assert_eq!(p.inter_request_gap_ms, 5);
    assert_eq!(p.boot_holdoff_ms, 5000);
    assert_eq!(p.down_after, 5);
    assert_eq!(p.reset_min_timeouts, 5);
    assert_eq!(p.reset_min_span_ms, 60000);
    assert_eq!(p.reset_min_interval_ms, 600_000);
    assert_eq!(LinkPolicy::QUEUE_CAPACITY, 24);
    let mut lp = LinkPolicy::default();
    assert_eq!(lp.state(0), LinkState::Unknown);
    assert_eq!(lp.queued(), 0);
    assert!(!lp.busy());
    assert!(lp.next_to_send(0).is_none());
    assert!(lp.poll(0).is_none());
    assert!(!lp.should_reset_stm(0));
}

#[test]
fn enqueue_rejects_invalid_requests() {
    let mut lp = LinkPolicy::default();
    let empty = RequestLine::default();
    assert_eq!(
        lp.enqueue(&empty, Priority::User, 0),
        EnqueueResult::Invalid
    );
    let mut no_cmd = valve_data(1);
    no_cmd.cmd = Cmd::None;
    assert_eq!(
        lp.enqueue(&no_cmd, Priority::User, 0),
        EnqueueResult::Invalid
    );
    // C++ cmd = static_cast<Cmd>(kCmdCount), len = kRequestMaxLen + 1 and priority 3 are
    // Invalid: no Rust form.
    assert_eq!(lp.queued(), 0);
    let mut max_len = valve_data(1);
    max_len.text.resize(REQUEST_MAX_LEN, 0).expect("fits");
    assert_eq!(
        lp.enqueue(&max_len, Priority::User, 0),
        EnqueueResult::Queued
    );
}

#[test]
fn priority_order_fifo_within_a_priority() {
    let mut lp = LinkPolicy::default();
    assert_eq!(
        lp.enqueue(&valve_data(0), Priority::Poll, 0),
        EnqueueResult::Queued
    );
    assert_eq!(
        lp.enqueue(&valve_data(1), Priority::Config, 0),
        EnqueueResult::Queued
    );
    assert_eq!(
        lp.enqueue(&valve_data(2), Priority::User, 0),
        EnqueueResult::Queued
    );
    assert_eq!(
        lp.enqueue(&valve_data(3), Priority::Poll, 0),
        EnqueueResult::Queued
    );
    assert_eq!(
        lp.enqueue(&valve_data(4), Priority::User, 0),
        EnqueueResult::Queued
    );
    assert_eq!(
        lp.enqueue(&valve_data(5), Priority::Config, 0),
        EnqueueResult::Queued
    );
    assert_eq!(lp.queued(), 6);
    assert_eq!(lp.queued_with(Priority::User), 2);
    assert_eq!(lp.queued_with(Priority::Config), 2);
    assert_eq!(lp.queued_with(Priority::Poll), 2);
    let mut now = 1000;
    for v in [2, 4, 1, 5, 0, 3] {
        assert_eq!(send(&mut lp, now), format!("gvlvd {v} \r\n"));
        assert!(lp.busy());
        let c = lp.on_reply(&gvlvd(v), now + 10).expect("completes");
        assert_eq!(c.outcome, Outcome::Ok);
        now += 100;
    }
    assert_eq!(lp.queued(), 0);
}

#[test]
fn identical_lines_coalesce_priority_raised_tag_rules() {
    let mut lp = LinkPolicy::default();
    assert_eq!(
        lp.enqueue(&valve_data(0), Priority::Poll, 1),
        EnqueueResult::Queued
    );
    assert_eq!(
        lp.enqueue(&valve_data(1), Priority::Poll, 2),
        EnqueueResult::Queued
    );
    assert_eq!(
        lp.enqueue(&valve_data(2), Priority::Config, 3),
        EnqueueResult::Queued
    );
    // Same priority: position and tag kept when the new tag is 0.
    assert_eq!(
        lp.enqueue(&valve_data(0), Priority::Poll, 0),
        EnqueueResult::Coalesced
    );
    assert_eq!(lp.queued(), 3);
    // Lower priority request does not demote.
    assert_eq!(
        lp.enqueue(&valve_data(2), Priority::Poll, 0),
        EnqueueResult::Coalesced
    );
    assert_eq!(lp.queued_with(Priority::Config), 1);
    // Raising to User moves it to the tail of the User group, new tag taken.
    assert_eq!(
        lp.enqueue(&valve_data(1), Priority::User, 9),
        EnqueueResult::Coalesced
    );
    assert_eq!(lp.queued_with(Priority::User), 1);
    assert_eq!(lp.queued_with(Priority::Poll), 1);
    assert_eq!(lp.queued(), 3);

    let mut now = 0;
    assert_eq!(send(&mut lp, now), "gvlvd 1 \r\n");
    let c = lp.on_reply(&gvlvd(1), now).expect("completes");
    assert_eq!(c.tag, 9);
    assert_eq!(c.priority, Priority::User);
    now += 10;
    assert_eq!(send(&mut lp, now), "gvlvd 2 \r\n");
    let c = lp.on_reply(&gvlvd(2), now).expect("completes");
    assert_eq!(c.tag, 3);
    assert_eq!(c.priority, Priority::Config);
    now += 10;
    assert_eq!(send(&mut lp, now), "gvlvd 0 \r\n");
    let c = lp.on_reply(&gvlvd(0), now).expect("completes");
    assert_eq!(c.tag, 1);
    assert_eq!(c.priority, Priority::Poll);
}

#[test]
fn raised_entry_goes_behind_existing_entries_of_its_new_priority() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(0), Priority::Config, 0);
    lp.enqueue(&valve_data(1), Priority::Poll, 0);
    lp.enqueue(&valve_data(2), Priority::Config, 0);
    assert_eq!(
        lp.enqueue(&valve_data(1), Priority::Config, 0),
        EnqueueResult::Coalesced
    );
    let mut now = 0;
    for v in [0, 2, 1] {
        assert_eq!(send(&mut lp, now), format!("gvlvd {v} \r\n"));
        assert!(lp.on_reply(&gvlvd(v), now).is_some());
        now += 10;
    }
}

#[test]
fn stgtp_for_the_same_valve_is_replaced_in_place() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&set_target(3, 20), Priority::Config, 1);
    lp.enqueue(&set_target(4, 20), Priority::Config, 2);
    assert_eq!(
        lp.enqueue(&set_target(3, 70), Priority::Config, 5),
        EnqueueResult::Coalesced
    );
    assert_eq!(lp.queued(), 2);
    // A different stgtp payload for another valve is not merged.
    assert_eq!(
        lp.enqueue(&set_target(5, 70), Priority::Config, 0),
        EnqueueResult::Queued
    );
    // Replacing with the tag 0 still takes the new tag (latest request owns it).
    assert_eq!(
        lp.enqueue(&set_target(4, 21), Priority::Config, 0),
        EnqueueResult::Coalesced
    );
    let mut now = 0;
    assert_eq!(send(&mut lp, now), "stgtp 3 70 \r\n");
    let c = lp.on_reply(&reply("stgtp"), now).expect("completes");
    assert_eq!(c.tag, 5);
    assert_eq!(c.request.arg, 70);
    now += 10;
    assert_eq!(send(&mut lp, now), "stgtp 4 21 \r\n");
    let c = lp.on_reply(&reply("stgtp"), now).expect("completes");
    assert_eq!(c.tag, 0);
    now += 10;
    assert_eq!(send(&mut lp, now), "stgtp 5 70 \r\n");
}

#[test]
fn stgtp_replacement_can_raise_priority() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(0), Priority::User, 0);
    lp.enqueue(&set_target(3, 20), Priority::Poll, 0);
    lp.enqueue(&valve_data(1), Priority::Config, 0);
    assert_eq!(
        lp.enqueue(&set_target(3, 30), Priority::User, 0),
        EnqueueResult::Coalesced
    );
    let mut now = 0;
    assert_eq!(send(&mut lp, now), "gvlvd 0 \r\n");
    assert!(lp.on_reply(&gvlvd(0), now).is_some());
    now += 10;
    assert_eq!(send(&mut lp, now), "stgtp 3 30 \r\n");
}

#[test]
fn an_outstanding_line_is_not_coalesced_with_a_new_one() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    assert_eq!(send(&mut lp, 0), "gvlvd 0 \r\n");
    assert_eq!(
        lp.enqueue(&valve_data(0), Priority::Poll, 0),
        EnqueueResult::Queued
    );
    assert_eq!(lp.queued(), 1);
}

#[test]
fn full_queue_eviction_of_the_newest_poll_entry() {
    let mut lp = LinkPolicy::default();
    for i in 0..24 {
        let r = build_temp_data(i).expect("index");
        assert_eq!(lp.enqueue(&r, Priority::Poll, 0), EnqueueResult::Queued);
    }
    assert_eq!(lp.queued(), 24);
    let extra = build_temp_data(30).expect("index");
    assert_eq!(lp.enqueue(&extra, Priority::Poll, 0), EnqueueResult::Full);
    assert_eq!(lp.stats().queue_full, 1);
    assert_eq!(lp.stats().evictions, 0);
    // Coalescing still works on a full queue.
    let dup = build_temp_data(5).expect("index");
    assert_eq!(
        lp.enqueue(&dup, Priority::Poll, 0),
        EnqueueResult::Coalesced
    );
    assert_eq!(lp.stats().queue_full, 1);

    assert_eq!(
        lp.enqueue(&calibrate(1), Priority::User, 0),
        EnqueueResult::Queued
    );
    assert_eq!(lp.stats().evictions, 1);
    assert_eq!(lp.queued(), 24);
    assert_eq!(lp.queued_with(Priority::Poll), 23);
    assert_eq!(
        lp.enqueue(&set_target(1, 1), Priority::Config, 0),
        EnqueueResult::Queued
    );
    assert_eq!(lp.stats().evictions, 2);

    // The evicted ones were the newest: goned 23 and 22.
    let mut now = 0;
    assert_eq!(send(&mut lp, now), "staln 1 \r\n");
    assert!(lp.on_reply(&reply("staln"), now).is_some());
    now += 10;
    assert_eq!(send(&mut lp, now), "stgtp 1 1 \r\n");
    assert!(lp.on_reply(&reply("stgtp"), now).is_some());
    let mut last = String::new();
    for _ in 0..22 {
        now += 10;
        last = send(&mut lp, now);
        assert!(lp.on_reply(&reply("goned 0"), now).is_some());
    }
    assert_eq!(last, "goned 21 \r\n");
    assert_eq!(lp.queued(), 0);
}

#[test]
fn queue_of_only_user_config_entries_is_full_for_everyone() {
    let mut lp = LinkPolicy::default();
    for i in 0..12 {
        assert_eq!(
            lp.enqueue(&calibrate(i), Priority::User, 0),
            EnqueueResult::Queued
        );
        assert_eq!(
            lp.enqueue(&set_target(i, 1), Priority::Config, 0),
            EnqueueResult::Queued
        );
    }
    assert_eq!(
        lp.enqueue(&calibrate(ALL_VALVES), Priority::User, 0),
        EnqueueResult::Full
    );
    assert_eq!(
        lp.enqueue(&valve_data(0), Priority::Config, 0),
        EnqueueResult::Full
    );
    assert_eq!(
        lp.enqueue(&valve_data(0), Priority::Poll, 0),
        EnqueueResult::Full
    );
    assert_eq!(lp.stats().queue_full, 3);
    assert_eq!(lp.stats().evictions, 0);
}

#[test]
fn send_path_on_sent_accounting_and_inter_request_gap() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    lp.enqueue(&valve_data(1), Priority::Poll, 0);
    assert_eq!(text(lp.next_to_send(100)), "gvlvd 0 \r\n");
    assert!(lp.next_to_send(100).is_none()); // one outstanding
    assert_eq!(lp.stats().sent, 0);
    lp.on_sent(100);
    assert_eq!(lp.stats().sent, 1);
    let c = lp.on_reply(&gvlvd(0), 150).expect("completes");
    assert_eq!(c.attempts, 1);
    assert!(lp.next_to_send(154).is_none()); // 4 ms < 5 ms gap
    assert!(lp.next_to_send(155).is_some());
    lp.on_sent(155);
    assert_eq!(lp.stats().sent, 2);
    lp.on_sent(156); // spurious extra call while outstanding counts; never crashes
    assert!(lp.on_reply(&gvlvd(1), 160).is_some());
    lp.on_sent(170); // nothing outstanding: ignored
    assert_eq!(lp.stats().sent, 3);
}

#[test]
fn the_first_request_after_construction_is_not_delayed() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    assert!(lp.next_to_send(0).is_some());
}

#[test]
fn zero_gap_sends_immediately_after_a_reply() {
    let mut lp = LinkPolicy::new(LinkParams {
        inter_request_gap_ms: 0,
        ..LinkParams::default()
    });
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    lp.enqueue(&valve_data(1), Priority::Poll, 0);
    assert_eq!(send(&mut lp, 7), "gvlvd 0 \r\n");
    assert!(lp.on_reply(&gvlvd(0), 9).is_some());
    assert_eq!(send(&mut lp, 9), "gvlvd 1 \r\n");
}

#[test]
fn matching_reply_completes_ok_and_brings_the_link_up() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(3), Priority::Poll, 77);
    assert_eq!(lp.state(0), LinkState::Unknown);
    send(&mut lp, 10);
    assert!(lp.on_reply(&gvlvd(4), 20).is_none()); // other valve: stray
    assert_eq!(lp.stats().stray_lines, 1);
    assert!(lp.busy());
    let c = lp.on_reply(&gvlvd(3), 30).expect("completes");
    assert_eq!(c.outcome, Outcome::Ok);
    assert_eq!(c.tag, 77);
    assert_eq!(c.priority, Priority::Poll);
    assert_eq!(c.attempts, 1);
    assert_eq!(&c.request.text[..], b"gvlvd 3 \r\n");
    assert_eq!(lp.stats().answered, 1);
    assert_eq!(lp.stats().last_reply_ms, 30);
    assert_eq!(lp.state(30), LinkState::Up);
    assert!(!lp.busy());
    // A reply with nothing outstanding is stray.
    assert!(lp.on_reply(&gvlvd(3), 40).is_none());
    assert_eq!(lp.stats().stray_lines, 2);
    assert_eq!(lp.stats().answered, 1);
    assert_eq!(lp.stats().last_reply_ms, 30);
}

#[test]
fn stray_replies_do_not_prove_the_link() {
    let mut lp = LinkPolicy::default();
    assert!(lp.on_reply(&reply("stgtp"), 1).is_none());
    assert_eq!(lp.state(1), LinkState::Unknown);
    assert_eq!(lp.stats().last_reply_ms, 0);
}

#[test]
fn error_forms_complete_as_rejected() {
    let smotc = build_set_motor_chars(&MotorChars::default()).expect("valid");
    let scalx = build_set_breakaway(&Breakaway::default()).expect("valid");
    let svmov = build_service_move(2, MoveDir::Open, 100, 30).expect("valid");
    let goned = build_temp_data(3).expect("index");
    let gowvd = build_volt_data(3).expect("index");
    let gvlon = build_valve_sensors(2).expect("valve");
    let gvlon_all = build_valve_sensors(ALL_VALVES).expect("all");
    let cases = [
        (&smotc, "smotc err", Outcome::Rejected),
        (&smotc, "smotc", Outcome::Ok),
        (&scalx, "scalx err", Outcome::Rejected),
        (&scalx, "scalx ok", Outcome::Ok),
        (&svmov, "svmov 2 err 3", Outcome::Rejected),
        (&svmov, "svmov 2 ok", Outcome::Ok),
        (&goned, "goned 0", Outcome::Rejected),
        (&goned, "goned 28-84-37-94-97-ff-03-23 215", Outcome::Ok),
        (&gowvd, "gowvd 0", Outcome::Rejected),
        (&gowvd, "gowvd 26-11-22-33-44-55-66-29 12", Outcome::Ok),
        (&gvlon, "goned error", Outcome::Rejected),
        (&gvlon_all, "goned error", Outcome::Rejected),
        (
            &gvlon,
            "gvlon 2 00-00-00-00-00-00-00-00 00-00-00-00-00-00-00-00",
            Outcome::Ok,
        ),
    ];
    for (req, rep, expect) in cases {
        let mut lp = LinkPolicy::default();
        assert_eq!(lp.enqueue(req, Priority::User, 0), EnqueueResult::Queued);
        send(&mut lp, 0);
        let c = lp.on_reply(&reply(rep), 5).expect(rep);
        assert_eq!(c.outcome, expect, "{rep}");
        assert_eq!(lp.state(5), LinkState::Up, "{rep}");
        assert_eq!(lp.stats().answered, 1, "{rep}");
    }
}

#[test]
fn parse_errors_are_counted_only() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    send(&mut lp, 0);
    lp.on_parse_error(1);
    lp.on_parse_error(2);
    assert_eq!(lp.stats().parse_errors, 2);
    assert!(lp.busy());
    assert_eq!(lp.state(2), LinkState::Unknown);
}

#[track_caller]
fn check_timeout(req: &RequestLine, expect_ms: u16) {
    let mut lp = LinkPolicy::new(LinkParams {
        retries: 0,
        ..LinkParams::default()
    });
    assert_eq!(lp.enqueue(req, Priority::User, 0), EnqueueResult::Queued);
    let t0 = 0xFFFF_FF00u32; // across the millis() wrap
    assert!(lp.next_to_send(t0).is_some());
    lp.on_sent(t0);
    let name = text(Some(req));
    assert!(
        lp.poll(t0.wrapping_add(u32::from(expect_ms) - 1)).is_none(),
        "{name:?}"
    );
    let c = lp
        .poll(t0.wrapping_add(u32::from(expect_ms)))
        .expect("timeout");
    assert_eq!(c.outcome, Outcome::Timeout, "{name:?}");
    assert_eq!(c.attempts, 1, "{name:?}");
}

#[test]
fn per_command_timeouts() {
    let zero = OneWireId::default();
    check_timeout(&valve_data(1), 400);
    check_timeout(&build_temp_count(), 400);
    check_timeout(&build_volt_count(), 400);
    check_timeout(&build_valve_sensors(3).expect("valve"), 400);
    check_timeout(&build_get_proto(), 400);
    check_timeout(&build_temp_list(), 1500);
    check_timeout(&build_volt_list(), 1500);
    check_timeout(&build_valve_sensors(ALL_VALVES).expect("all"), 1500);
    check_timeout(&build_profile(0).expect("valve"), 1500);
    check_timeout(&build_scan_one_wire(), 3000);
    check_timeout(&build_match_sensors(), 3000);
    check_timeout(&build_detect(), 3000);
    check_timeout(&build_soft_reset(), 3000);
    check_timeout(
        &build_set_motor_chars(&MotorChars::default()).expect("valid"),
        3000,
    );
    check_timeout(
        &build_set_valve_sensors(1, &zero, &zero).expect("valid"),
        3000,
    );
    check_timeout(&calibrate(1), 400);
}

#[test]
fn timeout_starts_at_on_sent_not_at_next_to_send() {
    let mut lp = LinkPolicy::new(LinkParams {
        retries: 0,
        ..LinkParams::default()
    });
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    assert!(lp.next_to_send(0).is_some());
    lp.on_sent(100);
    assert!(lp.poll(499).is_none());
    assert!(lp.poll(500).is_some());
}

#[test]
fn idempotent_requests_are_retried_at_the_head_of_their_priority() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(0), Priority::Poll, 5);
    lp.enqueue(&valve_data(1), Priority::Poll, 0);
    lp.enqueue(&valve_data(2), Priority::Config, 0);
    let mut now = 0;
    assert_eq!(send(&mut lp, now), "gvlvd 2 \r\n");
    assert!(lp.on_reply(&gvlvd(2), now).is_some());
    now += 10;
    assert_eq!(send(&mut lp, now), "gvlvd 0 \r\n");
    now += 400;
    assert!(lp.poll(now).is_none()); // retry queued, no completion
    assert_eq!(lp.stats().timeouts, 1);
    assert_eq!(lp.stats().consecutive_timeouts, 1);
    assert_eq!(lp.queued(), 2);
    // A User request queued meanwhile still goes first.
    lp.enqueue(&calibrate(4), Priority::User, 0);
    assert!(lp.next_to_send(now + 4).is_none()); // gap after a timeout too
    now += 5;
    assert_eq!(send(&mut lp, now), "staln 4 \r\n");
    assert!(lp.on_reply(&reply("staln"), now).is_some());
    assert_eq!(lp.stats().consecutive_timeouts, 0);
    now += 10;
    assert_eq!(send(&mut lp, now), "gvlvd 0 \r\n"); // retry before gvlvd 1
    now += 400;
    assert!(lp.poll(now).is_none());
    now += 10;
    assert_eq!(send(&mut lp, now), "gvlvd 0 \r\n");
    now += 399;
    assert!(lp.poll(now).is_none());
    now += 1;
    let c = lp.poll(now).expect("completes");
    assert_eq!(c.outcome, Outcome::Timeout);
    assert_eq!(c.attempts, 3);
    assert_eq!(c.tag, 5);
    assert_eq!(c.priority, Priority::Poll);
    assert_eq!(lp.stats().timeouts, 3);
    assert_eq!(lp.stats().failed_requests, 1);
    assert_eq!(lp.stats().consecutive_timeouts, 2);
    now += 10;
    assert_eq!(send(&mut lp, now), "gvlvd 1 \r\n");
}

#[test]
fn a_retry_answered_completes_with_its_attempt_count() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    send(&mut lp, 0);
    assert!(lp.poll(400).is_none());
    send(&mut lp, 405);
    let c = lp.on_reply(&gvlvd(0), 450).expect("completes");
    assert_eq!(c.attempts, 2);
    assert_eq!(c.outcome, Outcome::Ok);
}

#[test]
fn retries_parameter_bounds_the_attempts() {
    for retries in [0u8, 1, 3] {
        let mut lp = LinkPolicy::new(LinkParams {
            retries,
            ..LinkParams::default()
        });
        lp.enqueue(&valve_data(0), Priority::Poll, 0);
        let mut now = 0;
        send(&mut lp, now);
        let c = expire(&mut lp, &mut now);
        assert_eq!(c.attempts, retries + 1, "{retries}");
        assert_eq!(lp.stats().timeouts, u32::from(retries) + 1, "{retries}");
        assert_eq!(lp.stats().sent, u32::from(retries) + 1, "{retries}");
    }
}

#[test]
fn actions_are_never_retried() {
    let actions = [
        calibrate(1),
        build_assembly(1).expect("valve"),
        build_detect(),
        build_scan_one_wire(),
        build_match_sensors(),
        build_soft_reset(),
        build_service_move(1, MoveDir::Open, 10, 10).expect("valid"),
    ];
    for r in &actions {
        let mut lp = LinkPolicy::default();
        lp.enqueue(r, Priority::User, 0);
        send(&mut lp, 0);
        let c = lp.poll(3000).expect("completes");
        assert_eq!(c.attempts, 1, "{}", text(Some(r)));
        assert_eq!(c.outcome, Outcome::Timeout);
        assert_eq!(lp.queued(), 0);
    }
}

#[test]
fn retry_into_a_full_queue_evicts_the_newest_poll_entry() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&set_target(0, 1), Priority::Config, 0);
    send(&mut lp, 0);
    for i in 0..24 {
        lp.enqueue(&build_temp_data(i).expect("index"), Priority::Poll, 0);
    }
    assert!(lp.poll(400).is_none());
    assert_eq!(lp.stats().evictions, 1);
    assert_eq!(lp.queued(), 24);
    assert_eq!(send(&mut lp, 405), "stgtp 0 1 \r\n");
}

#[test]
fn retry_with_no_room_completes_as_timeout() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&set_target(0, 1), Priority::Config, 0);
    send(&mut lp, 0);
    for i in 0..12 {
        lp.enqueue(&calibrate(i), Priority::User, 0);
        lp.enqueue(&set_target(i, 2), Priority::Config, 0);
    }
    assert_eq!(lp.queued(), 24);
    let c = lp.poll(400).expect("completes");
    assert_eq!(c.outcome, Outcome::Timeout);
    assert_eq!(c.attempts, 1);
    assert_eq!(lp.stats().failed_requests, 1);
    assert_eq!(lp.stats().evictions, 0);
}

fn no_retry() -> LinkParams {
    LinkParams {
        retries: 0,
        ..LinkParams::default()
    }
}

#[test]
fn degraded_after_one_timeout_down_after_five_up_after_a_reply() {
    let mut lp = LinkPolicy::new(no_retry());
    let mut now = 0;
    for i in 1..=6u8 {
        lp.enqueue(&valve_data(0), Priority::Poll, 0);
        now += 10;
        send(&mut lp, now);
        now += 400;
        assert!(lp.poll(now).is_some());
        assert_eq!(lp.stats().consecutive_timeouts, i);
        let want = if i >= 5 {
            LinkState::Down
        } else {
            LinkState::Degraded
        };
        assert_eq!(lp.state(now), want, "{i}");
    }
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    now += 10;
    send(&mut lp, now);
    assert!(lp.on_reply(&gvlvd(0), now).is_some());
    assert_eq!(lp.state(now), LinkState::Up);
    assert_eq!(lp.stats().consecutive_timeouts, 0);
}

#[test]
fn a_rejected_reply_also_proves_the_link() {
    let mut lp = LinkPolicy::new(no_retry());
    let g = build_temp_data(0).expect("index");
    lp.enqueue(&g, Priority::Poll, 0);
    send(&mut lp, 0);
    assert!(lp.poll(400).is_some());
    lp.enqueue(&g, Priority::Poll, 0);
    send(&mut lp, 500);
    let c = lp.on_reply(&reply("goned 0"), 510).expect("completes");
    assert_eq!(c.outcome, Outcome::Rejected);
    assert_eq!(lp.state(510), LinkState::Up);
}

#[test]
fn down_after_parameter() {
    let mut lp = LinkPolicy::new(LinkParams {
        down_after: 2,
        ..no_retry()
    });
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    send(&mut lp, 0);
    assert!(lp.poll(400).is_some());
    assert_eq!(lp.state(400), LinkState::Degraded);
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    send(&mut lp, 500);
    assert!(lp.poll(900).is_some());
    assert_eq!(lp.state(900), LinkState::Down);
}

#[test]
fn gproto_probe_timeouts_never_count_toward_the_failure_counter() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&proto(), Priority::Config, 0);
    let mut now = 0;
    send(&mut lp, now);
    let c = expire(&mut lp, &mut now);
    assert_eq!(c.outcome, Outcome::Timeout);
    assert_eq!(c.attempts, 3); // still retried: a v2 STM may have lost it
    assert_eq!(lp.stats().timeouts, 3);
    assert_eq!(lp.stats().consecutive_timeouts, 0);
    assert_eq!(lp.state(now), LinkState::Unknown);
    assert!(!lp.should_reset_stm(now + 100_000));
}

#[test]
fn consecutive_timeouts_saturate_at_255() {
    let mut lp = LinkPolicy::new(no_retry());
    let mut now = 0;
    for _ in 0..300 {
        lp.enqueue(&valve_data(0), Priority::Poll, 0);
        now += 10;
        send(&mut lp, now);
        now += 400;
        assert!(lp.poll(now).is_some());
    }
    assert_eq!(lp.stats().consecutive_timeouts, 255);
    assert_eq!(lp.stats().timeouts, 300);
    assert_eq!(lp.state(now), LinkState::Down);
}

/// Drives `n` single-attempt timeouts spaced `step_ms` apart starting at now.
fn fail_requests(lp: &mut LinkPolicy, now: &mut u32, n: usize, step_ms: u32) {
    for _ in 0..n {
        lp.enqueue(&valve_data(0), Priority::Poll, 0);
        assert!(lp.next_to_send(*now).is_some());
        lp.on_sent(*now);
        *now += 400;
        assert!(lp.poll(*now).is_some());
        *now += step_ms;
    }
}

#[test]
fn r6_reset_needs_at_least_5_consecutive_timeouts() {
    let mut lp = LinkPolicy::new(no_retry());
    let mut now = 1000;
    let first = now + 400;
    fail_requests(&mut lp, &mut now, 4, 30000);
    assert_eq!(lp.stats().consecutive_timeouts, 4);
    assert!(!lp.should_reset_stm(first + 200_000));
    fail_requests(&mut lp, &mut now, 1, 0);
    assert!(lp.should_reset_stm(first + 200_000));
}

#[test]
fn r6_reset_needs_the_first_timeout_at_least_60_s_ago() {
    let mut lp = LinkPolicy::new(no_retry());
    let mut now = 1000;
    let first = now + 400; // poll() time of the first timeout
    fail_requests(&mut lp, &mut now, 5, 10);
    assert!(!lp.should_reset_stm(first + 59999));
    assert!(lp.should_reset_stm(first + 60000));
    // A reply clears it.
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    assert!(lp.next_to_send(first + 60000).is_some());
    assert!(lp.on_reply(&gvlvd(0), first + 60001).is_some());
    assert!(!lp.should_reset_stm(first + 60002));
}

#[test]
fn r6_reset_at_most_once_per_10_minutes() {
    let mut lp = LinkPolicy::new(no_retry());
    let mut now = 0;
    fail_requests(&mut lp, &mut now, 5, 20000);
    assert!(lp.should_reset_stm(now));
    let reset_at = now;
    lp.on_stm_reset(reset_at, true);
    assert_eq!(lp.stats().policy_resets, 1);
    assert_eq!(lp.stats().user_resets, 0);
    assert_eq!(lp.stats().consecutive_timeouts, 0);
    assert!(!lp.should_reset_stm(reset_at));
    // STM stays dead after the hold-off.
    now = reset_at + 5000;
    fail_requests(&mut lp, &mut now, 10, 20000);
    assert_eq!(lp.stats().consecutive_timeouts, 10);
    assert!(!lp.should_reset_stm(reset_at + 599_999));
    assert!(lp.should_reset_stm(reset_at + 600_000));
}

#[test]
fn a_user_reset_does_not_rate_limit_a_policy_reset() {
    let mut lp = LinkPolicy::new(no_retry());
    lp.on_stm_reset(0, false);
    assert_eq!(lp.stats().user_resets, 1);
    assert_eq!(lp.stats().policy_resets, 0);
    let mut now = 5000;
    fail_requests(&mut lp, &mut now, 5, 20000);
    assert!(lp.should_reset_stm(now));
}

#[test]
fn the_reset_rate_limit_survives_the_millis_wrap_via_poll() {
    let mut lp = LinkPolicy::new(no_retry());
    lp.on_stm_reset(1000, true);
    let _ = lp.poll(1000 + 600_000); // retires the limit
                                     // Exactly 2^32 ms after the reset: elapsed_ms() alone would say ~100 s.
    let mut now = 1000;
    fail_requests(&mut lp, &mut now, 5, 20000);
    assert!(lp.should_reset_stm(now));
}

#[test]
fn no_r6_reset_while_booting_or_suspended() {
    let mut lp = LinkPolicy::new(no_retry());
    let mut now = 0;
    fail_requests(&mut lp, &mut now, 5, 20000);
    assert!(lp.should_reset_stm(now));
    lp.suspend();
    assert!(!lp.should_reset_stm(now));
    assert_eq!(lp.state(now), LinkState::Suspended);
}

#[test]
fn stm_reset_drops_the_outstanding_request_and_poll_entries() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    lp.enqueue(&valve_data(1), Priority::Poll, 0);
    lp.enqueue(&set_target(1, 50), Priority::Config, 0);
    lp.enqueue(&calibrate(2), Priority::User, 0);
    lp.enqueue(&valve_data(3), Priority::Poll, 0);
    assert_eq!(send(&mut lp, 0), "staln 2 \r\n");
    lp.on_stm_reset(100, false);
    assert!(!lp.busy());
    assert_eq!(lp.queued(), 1);
    assert_eq!(lp.queued_with(Priority::Config), 1);
    assert_eq!(lp.state(100), LinkState::Booting);
    assert_eq!(lp.state(5099), LinkState::Booting);
    assert!(lp.next_to_send(5099).is_none());
    assert_eq!(lp.state(5100), LinkState::Unknown);
    assert!(lp.on_reply(&reply("staln"), 200).is_none()); // late reply is stray
    assert!(lp.poll(4000).is_none()); // no completion for the dropped one
    assert_eq!(send(&mut lp, 5100), "stgtp 1 50 \r\n");
}

#[test]
fn booting_forgets_up_the_next_reply_brings_it_back() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    send(&mut lp, 0);
    assert!(lp.on_reply(&gvlvd(0), 1).is_some());
    assert_eq!(lp.state(1), LinkState::Up);
    lp.on_stm_reset(10, true);
    assert_eq!(lp.state(5010), LinkState::Unknown);
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    send(&mut lp, 5010);
    assert!(lp.on_reply(&gvlvd(0), 5011).is_some());
    assert_eq!(lp.state(5011), LinkState::Up);
}

#[test]
fn booting_hold_is_released_by_poll_and_survives_time_wrap() {
    let mut lp = LinkPolicy::default();
    let t0 = 0xFFFF_F000u32;
    lp.on_stm_reset(t0, false);
    lp.enqueue(&valve_data(0), Priority::Config, 0);
    assert!(lp.next_to_send(t0.wrapping_add(4999)).is_none());
    let _ = lp.poll(t0.wrapping_add(5000));
    assert_eq!(lp.state(t0.wrapping_add(5000)), LinkState::Unknown);
    // Far in the future (elapsed wraps): not Booting again.
    assert_eq!(
        lp.state(t0.wrapping_add(5000).wrapping_add(0xFFFF_F000)),
        LinkState::Unknown
    );
    assert!(lp.next_to_send(t0.wrapping_add(0xFFFF_F000)).is_some());
}

#[test]
fn suspend_drops_everything_resume_enters_booting() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    lp.enqueue(&set_target(1, 50), Priority::Config, 0);
    lp.enqueue(&calibrate(2), Priority::User, 0);
    send(&mut lp, 0);
    assert_eq!(lp.suspend(), 3);
    assert_eq!(lp.queued(), 0);
    assert!(!lp.busy());
    assert_eq!(lp.state(0), LinkState::Suspended);
    // Requests may still be queued, but wait.
    assert_eq!(
        lp.enqueue(&valve_data(0), Priority::Poll, 0),
        EnqueueResult::Queued
    );
    assert!(lp.next_to_send(100_000).is_none());
    assert_eq!(lp.state(100_000), LinkState::Suspended);
    assert_eq!(lp.suspend(), 1);
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    lp.resume(200_000);
    assert_eq!(lp.state(200_000), LinkState::Booting);
    assert!(lp.next_to_send(204_999).is_none());
    assert_eq!(lp.state(205_000), LinkState::Unknown);
    assert_eq!(send(&mut lp, 205_000), "gvlvd 0 \r\n");
    assert_eq!(lp.suspend(), 1); // only the outstanding one
}

#[test]
fn resume_clears_the_failure_counter() {
    let mut lp = LinkPolicy::new(no_retry());
    let mut now = 0;
    fail_requests(&mut lp, &mut now, 5, 20000);
    lp.suspend();
    lp.resume(now);
    assert_eq!(lp.stats().consecutive_timeouts, 0);
    assert!(!lp.should_reset_stm(now + 1_000_000));
}

// ================================================================ reboot detector

#[test]
fn reboot_gstat_uptime_decrease_or_reset_counter_change() {
    let mut d = RebootDetector::default();
    let mut s = StmStatus {
        uptime_s: 100,
        resets: 3,
        ..StmStatus::default()
    };
    assert_eq!(d.on_status(&s, 0), 0); // primes
    s.uptime_s = 110;
    assert_eq!(d.on_status(&s, 10000), 0);
    assert_eq!(d.on_status(&s, 10000), 0); // equal uptime at the same ESP time is not a reboot
    s.uptime_s = 109;
    assert_eq!(d.on_status(&s, 10000), 1);
    s.uptime_s = 200;
    assert_eq!(d.on_status(&s, 10000), 0);
    s.resets = 4;
    assert_eq!(d.on_status(&s, 10000), 2);
    s.resets = 2;
    s.uptime_s = 300;
    assert_eq!(d.on_status(&s, 10000), 2);
    assert_eq!(d.on_status(&s, 10000), 0);
    s.resets = 3;
    s.uptime_s = 1;
    assert_eq!(d.on_status(&s, 10000), 2); // the reset counter wins over the uptime
    d.reset();
    s.uptime_s = 1;
    assert_eq!(d.on_status(&s, 10000), 0); // primes again
}

#[test]
fn reboot_the_uptime_must_keep_pace_with_the_esp_clock() {
    // 5 s slack plus 1 s per 1000 s
    let mut d = RebootDetector::default();
    let mut s = StmStatus {
        uptime_s: 1000,
        ..StmStatus::default()
    };
    assert_eq!(d.on_status(&s, 50000), 0);
    s.uptime_s = 1005; // +5 after 10 s: slack 5 + 0
    assert_eq!(d.on_status(&s, 60000), 0);
    s.uptime_s = 1009; // +4 after 10.999 s
    assert_eq!(d.on_status(&s, 70999), 1);
    d.reset();
    s.uptime_s = 1000;
    assert_eq!(d.on_status(&s, 0), 0);
    s.uptime_s = 1010;
    assert_eq!(d.on_status(&s, 10000), 0); // +10 after 10 s
    s.uptime_s = 30;
    assert_eq!(d.on_status(&s, 70000), 1); // power-on: 30 s after 60 s
    d.reset();
    s.uptime_s = 5000;
    assert_eq!(d.on_status(&s, 1000), 0);
    s.uptime_s = 5000 + 9985; // e = 10000 s: slack 5 + 10
    assert_eq!(d.on_status(&s, 10_001_000), 0);
    s.uptime_s = 5000 + 9985 + 9984;
    assert_eq!(d.on_status(&s, 20_001_000), 1);
    d.reset();
    s.uptime_s = 5000;
    assert_eq!(d.on_status(&s, 1000), 0);
    s.uptime_s = 5000 + 9985;
    assert_eq!(d.on_status(&s, 10_001_999), 0); // 10000.999 s count as 10000 whole seconds
    d.reset();
    s.uptime_s = 5000;
    assert_eq!(d.on_status(&s, 1000), 0);
    s.uptime_s = 5000 + 9985;
    assert_eq!(d.on_status(&s, 10_000_999), 0); // 9999 s: slack 5 + 9
    s.uptime_s = 5000 + 9985 + 9984;
    assert_eq!(d.on_status(&s, 20_000_999), 1); // 10000 s: slack 15, one second short
}

#[test]
fn reboot_a_large_uptime_near_the_counter_limit_does_not_overflow_the_check() {
    let mut d = RebootDetector::default();
    let mut s = StmStatus {
        uptime_s: 0xFFFF_FFF0,
        ..StmStatus::default()
    };
    assert_eq!(d.on_status(&s, 0), 0);
    s.uptime_s = 0xFFFF_FFFA;
    assert_eq!(d.on_status(&s, 10000), 0);
}

fn vd(valve: u8, status: u8, oc: u32, cc: u32, moves: u32) -> ValveData {
    ValveData {
        valve,
        status,
        open_count: oc,
        close_count: cc,
        moves,
        ..ValveData::default()
    }
}

#[test]
fn reboot_v1_heuristic_on_gvlvd() {
    let mut d = RebootDetector::default();
    // Never calibrated valve: zeros are normal.
    assert!(!d.on_valve_data(&vd(0, 5, 0, 0, 0)));
    assert!(!d.on_valve_data(&vd(0, 8, 0, 0, 0)));
    // Calibrated, then zeros in each boot status.
    for st in [5, 6, 8] {
        assert!(!d.on_valve_data(&vd(1, 1, 3000, 0, 5)), "{st}");
        assert!(d.on_valve_data(&vd(1, st, 0, 0, 0)), "{st}");
        assert!(!d.on_valve_data(&vd(1, st, 0, 0, 0)), "{st}"); // reported once
    }
    assert!(!d.on_valve_data(&vd(1, 1, 0, 3000, 5))); // cc alone marks calibrated
    for st in [0, 1, 2, 3, 4, 7, 9, 10] {
        assert!(!d.on_valve_data(&vd(1, st, 0, 0, 0)), "{st}");
    }
    assert!(!d.on_valve_data(&vd(1, 5, 0, 0, 1))); // moves != 0
    assert!(!d.on_valve_data(&vd(1, 5, 1, 0, 0))); // still has counts
    assert!(!d.on_valve_data(&vd(1, 5, 0, 1, 0)));
    assert!(d.on_valve_data(&vd(1, 5, 0, 0, 0)));
    assert!(!d.on_valve_data(&vd(12, 5, 0, 0, 0))); // out of range ignored
    assert!(!d.on_valve_data(&vd(12, 1, 10, 10, 0)));
}

#[test]
fn reboot_one_detection_forgets_every_valve() {
    let mut d = RebootDetector::default();
    for v in 0..VALVE_COUNT {
        assert!(!d.on_valve_data(&vd(v, 1, 100, 100, 0)));
    }
    assert!(d.on_valve_data(&vd(3, 8, 0, 0, 0)));
    for v in 0..VALVE_COUNT {
        assert!(!d.on_valve_data(&vd(v, 8, 0, 0, 0)), "{v}");
    }
}

#[test]
fn reboot_reset_forgets_calibration_history() {
    let mut d = RebootDetector::default();
    assert!(!d.on_valve_data(&vd(11, 1, 100, 100, 0)));
    d.reset();
    assert!(!d.on_valve_data(&vd(11, 5, 0, 0, 0)));
}

#[test]
fn reboot_link_down_to_up_recovers_by_reboot_or_status_check() {
    // by reboot on protocol 0/1, by a status check on 2/3
    let all = [
        LinkState::Unknown,
        LinkState::Up,
        LinkState::Degraded,
        LinkState::Down,
        LinkState::Booting,
        LinkState::Suspended,
    ];
    for proto in 0..=3u8 {
        for a in all {
            for b in all {
                let mut d = RebootDetector::default();
                let recovery = a == LinkState::Down && b == LinkState::Up;
                let want = if !recovery {
                    RebootDetectorRecovery::None
                } else if proto <= 1 {
                    RebootDetectorRecovery::Reboot
                } else {
                    RebootDetectorRecovery::CheckStatus
                };
                let ctx = format!("{proto} {} {}", link_state_name(a), link_state_name(b));
                assert_eq!(d.on_link_state(a, b, proto), want, "{ctx}");
                assert_eq!(
                    d.on_status_failed(),
                    want == RebootDetectorRecovery::CheckStatus,
                    "{ctx}"
                );
            }
        }
    }
}

#[test]
fn reboot_a_failed_status_check_counts_once_a_status_or_reset_disarms_it() {
    let mut d = RebootDetector::default();
    assert!(!d.on_status_failed());
    assert_eq!(
        d.on_link_state(LinkState::Down, LinkState::Up, 2),
        RebootDetectorRecovery::CheckStatus
    );
    assert!(d.on_status_failed());
    assert!(!d.on_status_failed());
    assert_eq!(
        d.on_link_state(LinkState::Down, LinkState::Up, 3),
        RebootDetectorRecovery::CheckStatus
    );
    let s = StmStatus::default();
    d.on_status(&s, 0);
    assert!(!d.on_status_failed());
    assert_eq!(
        d.on_link_state(LinkState::Down, LinkState::Up, 3),
        RebootDetectorRecovery::CheckStatus
    );
    d.reset();
    assert!(!d.on_status_failed());
    assert_eq!(
        d.on_link_state(LinkState::Down, LinkState::Up, 1),
        RebootDetectorRecovery::Reboot
    );
    assert!(!d.on_status_failed());
}

#[test]
fn busy_with_names_the_priority_of_the_outstanding_request_only() {
    let mut lp = LinkPolicy::default();
    assert!(!lp.busy_with(Priority::User));
    assert!(!lp.busy_with(Priority::Config));
    assert!(!lp.busy_with(Priority::Poll));
    lp.enqueue(&valve_data(0), Priority::Config, 0);
    assert!(!lp.busy_with(Priority::Config)); // queued, not sent
    send(&mut lp, 0);
    assert!(!lp.busy_with(Priority::User));
    assert!(lp.busy_with(Priority::Config));
    assert!(!lp.busy_with(Priority::Poll));
    assert!(lp.on_reply(&gvlvd(0), 10).is_some());
    assert!(!lp.busy_with(Priority::Config));
    lp.enqueue(&valve_data(1), Priority::User, 0);
    send(&mut lp, 100);
    assert!(lp.busy_with(Priority::User));
    assert!(!lp.busy_with(Priority::Config));
}

#[test]
fn a_probe_request_never_counts_toward_the_failure_counter_the_same_line_without_it_does() {
    let mut r = valve_data(0);
    r.probe = true;
    let mut lp = LinkPolicy::default();
    lp.enqueue(&r, Priority::Poll, 0);
    let mut now = 0;
    send(&mut lp, now);
    let c = expire(&mut lp, &mut now);
    assert_eq!(c.attempts, 3);
    assert_eq!(lp.stats().consecutive_timeouts, 0);
    assert_eq!(lp.state(now), LinkState::Unknown);
    r.probe = false;
    lp.enqueue(&r, Priority::Poll, 0);
    now += 10;
    send(&mut lp, now);
    expire(&mut lp, &mut now);
    assert_eq!(lp.stats().consecutive_timeouts, 3);
    assert_eq!(lp.state(now), LinkState::Degraded);
}

#[test]
fn only_the_gproto_builder_marks_its_request_as_a_probe() {
    assert!(build_get_proto().probe);
    assert!(!valve_data(0).probe);
    assert!(!set_target(0, 1).probe);
    assert!(!calibrate(0).probe);
    assert!(!build_get_version().probe);
    assert!(!build_get_status_v3().probe);
}

fn check_outcome(req: &RequestLine, line: &str, want: Outcome) {
    let mut lp = LinkPolicy::default();
    lp.enqueue(req, Priority::Config, 0);
    send(&mut lp, 0);
    let c = lp.on_reply(&reply(line), 10).expect(line);
    assert_eq!(c.outcome, want, "{line}");
}

#[test]
fn error_forms_of_the_protocol_3_commands_complete_as_rejected() {
    let r = build_heartbeat(true);
    check_outcome(&r, "slhbt err", Outcome::Rejected);
    check_outcome(&r, "slhbt 1 3600", Outcome::Ok);
    let r = build_set_lease_timeout(60).expect("valid");
    check_outcome(&r, "slcfg err", Outcome::Rejected);
    check_outcome(&r, "slcfg ok", Outcome::Ok);
    let r = build_leave_safe_mode();
    check_outcome(&r, "ssafe err", Outcome::Rejected);
    check_outcome(&r, "ssafe ok", Outcome::Ok);
    let r = build_set_failsafe(2, 50).expect("valid");
    check_outcome(&r, "sfspo 2 err 1", Outcome::Rejected);
    check_outcome(&r, "sfspo -1 err 1", Outcome::Rejected);
    check_outcome(&r, "sfspo 2 ok", Outcome::Ok);
    let r = build_stop(2).expect("valve");
    check_outcome(&r, "sstop 2 err 1", Outcome::Rejected);
    check_outcome(&r, "sstop -1 err 1", Outcome::Rejected);
    check_outcome(&r, "sstop 2 ok", Outcome::Ok);
}

#[test]
fn svmov_minus_1_err_rejects_the_outstanding_svmov_another_index_is_stray() {
    let r = build_service_move(3, MoveDir::Open, 100, 50).expect("valid");
    let mut lp = LinkPolicy::default();
    lp.enqueue(&r, Priority::User, 0);
    send(&mut lp, 0);
    assert!(lp.on_reply(&reply("svmov 4 ok"), 5).is_none());
    assert_eq!(lp.stats().stray_lines, 1);
    let c = lp
        .on_reply(&reply("svmov -1 err 1"), 10)
        .expect("completes");
    assert_eq!(c.outcome, Outcome::Rejected);
}

// ================================================================ edge cases

#[test]
fn a_request_with_a_command_but_no_text_is_invalid() {
    let mut lp = LinkPolicy::default();
    let full = valve_data(1);
    let mut r = full.clone();
    r.text.clear();
    assert_eq!(lp.enqueue(&r, Priority::User, 0), EnqueueResult::Invalid);
    r.text = full.text;
    r.text.truncate(1);
    assert_eq!(lp.enqueue(&r, Priority::User, 0), EnqueueResult::Queued);
}

#[test]
fn eviction_looks_at_the_newest_entry_only() {
    let mut lp = LinkPolicy::default();
    for i in 0..12 {
        lp.enqueue(&calibrate(i), Priority::User, 0);
    }
    for i in 0..11 {
        lp.enqueue(&set_target(i, 1), Priority::Config, 0);
    }
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    assert_eq!(lp.queued(), 24);
    assert_eq!(
        lp.enqueue(&set_target(11, 1), Priority::Config, 0),
        EnqueueResult::Queued
    );
    assert_eq!(lp.stats().evictions, 1);
    assert_eq!(lp.queued_with(Priority::Poll), 0);
}

#[test]
fn poll_during_the_hold_off_keeps_booting() {
    let mut lp = LinkPolicy::default();
    lp.on_stm_reset(100, true);
    lp.enqueue(&valve_data(0), Priority::Config, 0);
    assert!(lp.poll(200).is_none());
    assert_eq!(lp.state(200), LinkState::Booting);
    assert!(lp.next_to_send(300).is_none());
    assert!(lp.next_to_send(5100).is_some());
}

#[test]
fn reset_min_timeouts_0_still_needs_a_timeout() {
    let mut lp = LinkPolicy::new(LinkParams {
        reset_min_timeouts: 0,
        reset_min_span_ms: 0,
        ..no_retry()
    });
    assert!(!lp.should_reset_stm(1_000_000));
    let mut now = 0;
    fail_requests(&mut lp, &mut now, 1, 0);
    assert!(lp.should_reset_stm(now));
}

#[test]
fn reset_min_timeouts_1_resets_after_the_first_timeout_and_span() {
    let mut lp = LinkPolicy::new(LinkParams {
        reset_min_timeouts: 1,
        ..no_retry()
    });
    let mut now = 0;
    fail_requests(&mut lp, &mut now, 1, 0);
    assert!(!lp.should_reset_stm(400 + 59999));
    assert!(lp.should_reset_stm(400 + 60000));
}

#[test]
fn stm_reset_clears_the_inter_request_gap() {
    let mut lp = LinkPolicy::new(LinkParams {
        boot_holdoff_ms: 0,
        inter_request_gap_ms: 100,
        ..LinkParams::default()
    });
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    send(&mut lp, 0);
    assert!(lp.on_reply(&gvlvd(0), 10).is_some());
    lp.on_stm_reset(11, false);
    lp.enqueue(&valve_data(1), Priority::Config, 0);
    assert!(lp.next_to_send(12).is_some());
}

#[test]
fn stm_reset_keeps_every_user_config_entry_and_nothing_stale() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&calibrate(0), Priority::User, 0);
    lp.enqueue(&calibrate(1), Priority::User, 0);
    lp.enqueue(&set_target(2, 5), Priority::Config, 0);
    lp.enqueue(&valve_data(3), Priority::Poll, 0);
    assert_eq!(send(&mut lp, 0), "staln 0 \r\n"); // C++: leaves a stale copy past the end
    lp.on_stm_reset(1, false);
    assert_eq!(lp.queued(), 2);
    assert_eq!(lp.queued_with(Priority::User), 1);
    assert_eq!(lp.queued_with(Priority::Config), 1);
    assert_eq!(lp.queued_with(Priority::Poll), 0);
    assert_eq!(send(&mut lp, 5001), "staln 1 \r\n");
    assert!(lp.on_reply(&reply("staln"), 5002).is_some());
    assert_eq!(send(&mut lp, 5010), "stgtp 2 5 \r\n");
    assert!(lp.on_reply(&reply("stgtp"), 5011).is_some());
    assert!(lp.next_to_send(5020).is_none());

    // A stale non-Poll slot just past the end is never resurrected.
    let mut lq = LinkPolicy::default();
    lq.enqueue(&calibrate(0), Priority::User, 0);
    lq.enqueue(&calibrate(1), Priority::User, 0);
    send(&mut lq, 0);
    lq.on_stm_reset(1, false);
    assert_eq!(lq.queued(), 1);
}

#[test]
fn down_after_1_goes_down_on_the_first_timeout() {
    let mut lp = LinkPolicy::new(LinkParams {
        down_after: 1,
        ..no_retry()
    });
    let mut now = 0;
    fail_requests(&mut lp, &mut now, 1, 0);
    assert_eq!(lp.state(now), LinkState::Down);
}

#[test]
fn down_after_0_still_needs_a_timeout() {
    // Rust: Down needs at least one timed-out attempt, also with down_after 0.
    let mut lp = LinkPolicy::new(LinkParams {
        down_after: 0,
        ..no_retry()
    });
    assert_eq!(lp.state(0), LinkState::Unknown);
    let mut now = 0;
    fail_requests(&mut lp, &mut now, 1, 0);
    assert_eq!(lp.state(now), LinkState::Down);
}

#[test]
fn a_replaced_stgtp_retry_gets_a_fresh_retry_budget() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&set_target(3, 20), Priority::Config, 1);
    let mut now = 0;
    assert_eq!(send(&mut lp, now), "stgtp 3 20 \r\n");
    now += 400;
    assert!(lp.poll(now).is_none()); // re-queued as retry (1 attempt made)
    assert_eq!(lp.queued(), 1);
    assert_eq!(
        lp.enqueue(&set_target(3, 60), Priority::Config, 2),
        EnqueueResult::Coalesced
    );
    assert_eq!(lp.queued(), 1);
    // The new target gets 1 + retries attempts of its own.
    for _ in 1..=2 {
        now += 10;
        assert_eq!(send(&mut lp, now), "stgtp 3 60 \r\n");
        now += 400;
        assert!(lp.poll(now).is_none());
    }
    now += 10;
    assert_eq!(send(&mut lp, now), "stgtp 3 60 \r\n");
    now += 400;
    let c = lp.poll(now).expect("completes");
    assert_eq!(c.outcome, Outcome::Timeout);
    assert_eq!(c.attempts, 3);
    assert_eq!(c.tag, 2);
    assert_eq!(lp.stats().timeouts, 4);
}

#[test]
fn an_identical_non_stgtp_line_keeps_its_retry_count() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(1), Priority::Poll, 0);
    let mut now = 0;
    assert_eq!(send(&mut lp, now), "gvlvd 1 \r\n");
    now += 400;
    assert!(lp.poll(now).is_none());
    assert_eq!(
        lp.enqueue(&valve_data(1), Priority::Poll, 0),
        EnqueueResult::Coalesced
    );
    now += 10;
    assert_eq!(send(&mut lp, now), "gvlvd 1 \r\n");
    now += 400;
    assert!(lp.poll(now).is_none());
    now += 10;
    assert_eq!(send(&mut lp, now), "gvlvd 1 \r\n");
    now += 400;
    let c = lp.poll(now).expect("completes");
    assert_eq!(c.attempts, 3);
}

#[test]
fn hold_after_esp_boot_keeps_the_queue_and_counts_no_reset() {
    let mut lp = LinkPolicy::default();
    lp.enqueue(&valve_data(0), Priority::Poll, 0);
    lp.enqueue(&set_target(1, 50), Priority::Config, 0);
    lp.hold_after_esp_boot(1000);
    assert_eq!(lp.state(1000), LinkState::Booting);
    assert_eq!(lp.queued(), 2);
    assert_eq!(lp.stats().user_resets, 0);
    assert_eq!(lp.stats().policy_resets, 0);
    assert!(lp.next_to_send(5999).is_none());
    assert_eq!(lp.state(5999), LinkState::Booting);
    assert_eq!(lp.state(6000), LinkState::Unknown);
    assert_eq!(send(&mut lp, 6000), "stgtp 1 50 \r\n");
}

#[test]
fn hold_after_esp_boot_clears_the_failure_counter_and_blocks_r6() {
    let mut lp = LinkPolicy::new(no_retry());
    let mut now = 0;
    fail_requests(&mut lp, &mut now, 5, 20000);
    assert!(lp.should_reset_stm(now));
    lp.hold_after_esp_boot(now);
    assert_eq!(lp.stats().consecutive_timeouts, 0);
    assert!(!lp.should_reset_stm(now));
    assert_eq!(lp.state(now), LinkState::Booting);
}
