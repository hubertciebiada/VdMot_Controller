//! Port of test/native/test_poll_planner.cpp: re-sync sequence (v1/v2/v3), one-shots, periodic
//! cadence, priorities, retries and lost requests. `next()` returns an Option, so the C++ checks
//! that an idle call leaves an empty RequestLine behind are the None results.

use super::*;
use crate::stm_codec::{build_calibrate, build_get_proto, build_get_status, Cmd};
use crate::version::parse_version;
use std::format;
use std::string::{String, ToString};
use std::vec;
use std::vec::Vec;

const LOST: u32 = PollPlanner::LOST_REQUEST_MS as u32;

/// The line without its " \r\n" tail, for readable comparisons.
fn text(r: &RequestLine) -> String {
    let s = String::from_utf8(r.text.to_vec()).expect("ASCII");
    match s.strip_suffix(" \r\n") {
        Some(t) => t.to_string(),
        None => s,
    }
}

fn next(p: &mut PollPlanner, now: u32) -> String {
    match p.next(now) {
        None => "-".to_string(),
        Some(r) => {
            assert!(!r.text.is_empty());
            text(&r)
        }
    }
}

/// Cadence where every period is the same, for tie-order tests.
fn flat(ms: u16) -> PollCadence {
    PollCadence {
        valve_busy_ms: ms,
        valve_active_ms: ms,
        valve_inactive_ms: ms,
        temp_data_ms: ms,
        volt_data_ms: ms,
        sensor_count_ms: ms,
        status_ms: ms,
        version_ms: u32::from(ms),
        ..PollCadence::default()
    }
}

/// Takes everything that is due at `now` (the initial burst of valve reads).
fn drain(p: &mut PollPlanner, now: u32) -> Vec<String> {
    let mut out = Vec::new();
    for _ in 0..200 {
        let Some(r) = p.next(now) else {
            return out;
        };
        out.push(text(&r));
        if p.last_was_resync() {
            p.on_result(&r, true, now);
        }
    }
    panic!("planner never ran dry");
}

/// Runs the re-sync to completion answering every step with ok (the planner's own gproto
/// handling aside); returns only the step lines.
fn run_resync(p: &mut PollPlanner, now: &mut u32, proto_reply: u8) -> Vec<String> {
    let mut steps = Vec::new();
    for _ in 0..500 {
        if !p.resync_active() {
            break;
        }
        let Some(r) = p.next(*now) else {
            *now += 100;
            continue;
        };
        if !p.last_was_resync() {
            continue; // periodic item: ignored
        }
        steps.push(text(&r));
        if r.cmd == Cmd::Gproto {
            if proto_reply != 0 {
                p.set_protocol(proto_reply);
            }
            p.on_result(&r, proto_reply != 0, *now);
        } else {
            p.on_result(&r, true, *now);
        }
    }
    assert!(!p.resync_active());
    steps
}

fn strings(s: &[&str]) -> Vec<String> {
    s.iter().map(|x| x.to_string()).collect()
}

fn v1_steps() -> Vec<String> {
    let mut s = strings(&[
        "gproto",
        "gvers",
        "ghwin",
        "gmotc",
        "gtlnm",
        "gonec 255",
        "gowvc 255",
        "gvlon 255",
        "gvlst",
    ]);
    s.extend((0..12).map(|v| format!("gtgtp {v}")));
    s
}

fn ver(s: &str) -> Version {
    parse_version(s.as_bytes())
}

fn v3_steps() -> Vec<String> {
    let mut s = strings(&[
        "gproto",
        "gvers",
        "ghwin",
        "gmotc",
        "gtlnm",
        "gcalx",
        "gonec 255",
        "gowvc 255",
        "gvlon 255",
    ]);
    s.extend((0..12).map(|v| format!("gvlvy {v}")));
    s
}

fn v2_steps() -> Vec<String> {
    let mut s = strings(&[
        "gproto",
        "gvers",
        "ghwin",
        "gmotc",
        "gtlnm",
        "gcalx",
        "gonec 255",
        "gowvc 255",
        "gvlon 255",
    ]);
    s.extend((0..12).map(|v| format!("gvlvx {v}")));
    s
}

fn count(v: &[String], s: &str) -> usize {
    v.iter().filter(|x| *x == s).count()
}

#[test]
fn cadence_defaults_match_the_binding_table() {
    let c = PollCadence::default();
    assert_eq!(c.valve_busy_ms, 500);
    assert_eq!(c.valve_active_ms, 2000);
    assert_eq!(c.valve_inactive_ms, 30000);
    assert_eq!(c.temp_data_ms, 10000);
    assert_eq!(c.volt_data_ms, 10000);
    assert_eq!(c.sensor_count_ms, 30000);
    assert_eq!(c.status_ms, 10000);
    assert_eq!(c.version_ms, 300_000);
    let p = PollPlanner::default();
    assert_eq!(p.protocol(), 0);
    assert!(!p.resync_active());
    assert_eq!(p.resync_step(), ResyncStep::Done);
    assert!(!p.last_was_resync());
}

#[test]
fn resync_step_numbers_and_item_masks() {
    // Rust: from_raw over all of u8, and the one-shot item masks against their definition.
    for v in 0..=u8::MAX {
        assert_eq!(
            ResyncStep::from_raw(v).map(|s| s as u8),
            (v < 12).then_some(v)
        );
    }
    assert_eq!(ResyncStep::Targets.following(), ResyncStep::Done);
    assert_eq!(ResyncStep::Done.following(), ResyncStep::Done);
    let valve_bits = (1u64 << VALVE_COUNT) - 1;
    let covered = (valve_bits << ITEM_TARGET)
        | 1 << ITEM_TEMP_LIST
        | 1 << ITEM_VOLT_LIST
        | 1 << ITEM_VALVE_SENSORS
        | 1 << ITEM_MOTOR_CHARS
        | 1 << ITEM_LEARN_MOVEMENTS
        | 1 << ITEM_BREAKAWAY
        | 1 << ITEM_PROBE
        | 1 << ITEM_STATUS
        | 1 << ITEM_MATCH_SENSORS;
    assert_eq!(RESYNC_COVERED, covered);
    let v2 = (valve_bits << ITEM_PROFILE) | 1 << ITEM_BREAKAWAY | 1 << ITEM_STATUS;
    assert_eq!(V2_ITEMS, v2);
    assert_eq!(
        (
            ITEM_PROFILE,
            ITEM_TEMP_LIST,
            ITEM_PROBE,
            ITEM_STATUS,
            ITEM_MATCH_SENSORS
        ),
        (12, 24, 30, 31, 32)
    );
    assert_eq!(ITEM_COUNT, 33);
    assert_eq!(PollPlanner::SCAN_MATCH_DELAY_MS, 5000);
    assert_eq!(PollPlanner::LOST_REQUEST_MS, 10000);
}

#[test]
fn fresh_planner_reads_every_valve_once_then_waits() {
    let mut p = PollPlanner::default();
    let expect: Vec<String> = (0..12).map(|v| format!("gvlvd {v}")).collect();
    assert_eq!(drain(&mut p, 1000), expect);
    assert_eq!(next(&mut p, 1000 + 29999), "-");
    // Sensor counts are first due one period after start; on a tie they precede inactive
    // valves.
    assert_eq!(next(&mut p, 1000 + 30000), "gonec");
    assert_eq!(next(&mut p, 1000 + 30000), "gowvc");
    assert_eq!(next(&mut p, 1000 + 30000), "gvlvd 0");
    assert_eq!(next(&mut p, 1000 + 30000), "gvlvd 1");
}

#[test]
fn valve_classes_and_periods() {
    let mut p = PollPlanner::default();
    p.set_active_mask(0x0002); // valve 1 active
    p.set_valve_busy(2, true); // valve 2 busy (inactive)
    p.set_valve_busy(1, false);
    let t0 = 5000;
    let first = drain(&mut p, t0);
    assert_eq!(first.len(), 12);
    // Most overdue first: primed as due for 30 s, so the busy (500 ms) and the active (2 s)
    // valve lead.
    assert_eq!(first[0], "gvlvd 2");
    assert_eq!(first[1], "gvlvd 1");
    assert_eq!(first[2], "gvlvd 0");
    assert_eq!(next(&mut p, t0 + 499), "-");
    assert_eq!(next(&mut p, t0 + 500), "gvlvd 2");
    assert_eq!(next(&mut p, t0 + 999), "-");
    assert_eq!(next(&mut p, t0 + 1000), "gvlvd 2");
    assert_eq!(next(&mut p, t0 + 1500), "gvlvd 2");
    assert_eq!(next(&mut p, t0 + 1999), "-");
    // Both exactly due: busy before active.
    assert_eq!(next(&mut p, t0 + 2000), "gvlvd 2");
    assert_eq!(next(&mut p, t0 + 2000), "gvlvd 1");
    assert_eq!(next(&mut p, t0 + 2000), "-");
}

#[test]
fn busy_wins_a_tie_with_active_and_goes_back_to_its_class_when_idle() {
    let mut p = PollPlanner::default();
    p.set_active_mask(0x0003);
    p.set_valve_busy(1, true);
    drain(&mut p, 0);
    // At 2000 both valves are exactly due by class (valve 1 busy is 1500 overdue).
    assert_eq!(next(&mut p, 2000), "gvlvd 1");
    assert_eq!(next(&mut p, 2000), "gvlvd 0");
    p.set_valve_busy(1, false);
    assert_eq!(next(&mut p, 3999), "-");
    assert_eq!(next(&mut p, 4000), "gvlvd 0");
    assert_eq!(next(&mut p, 4000), "gvlvd 1");
    // Invalid valve indices are ignored.
    p.set_valve_busy(12, true);
    p.set_valve_busy(255, true);
    assert_eq!(next(&mut p, 4499), "-");
}

#[test]
fn active_mask_ignores_bits_above_valve_11() {
    let mut p = PollPlanner::default();
    p.set_active_mask(0xF000);
    drain(&mut p, 0);
    assert_eq!(next(&mut p, 2000), "-");
    assert_eq!(next(&mut p, 29999), "-");
}

#[test]
fn v2_uses_gvlvx_and_polls_gstat() {
    let mut p = PollPlanner::default();
    p.set_protocol(2);
    assert_eq!(p.protocol(), 2);
    let first = drain(&mut p, 0);
    assert_eq!(first.len(), 13);
    assert_eq!(first[0], "gstat"); // tie with inactive valves: gstat first
    assert_eq!(first[1], "gvlvx 0");
    assert_eq!(first[12], "gvlvx 11");
    assert_eq!(next(&mut p, 9999), "-");
    assert_eq!(next(&mut p, 10000), "gstat");
    p.set_protocol(7);
    assert_eq!(p.protocol(), 3);
    p.set_protocol(1);
    assert_eq!(p.protocol(), 1);
    assert_eq!(next(&mut p, 20000), "-"); // no gstat on v1
    assert_eq!(next(&mut p, 30000), "gonec");
    assert_eq!(next(&mut p, 30000), "gowvc");
    assert_eq!(next(&mut p, 30000), "gvlvd 0");
}

#[test]
fn tie_order_busy_active_gstat_goned_gowvd_counts_inactive_gvers() {
    let mut p = PollPlanner::new(flat(1000));
    p.set_protocol(2);
    p.set_sensor_counts(1, 1);
    p.set_active_mask(1 << 1);
    p.set_valve_busy(2, true);
    let t0 = 100;
    let mut expect0 = strings(&[
        "gvlvx 2", "gvlvx 1", "gstat", "goned 0", "gowvd 0", "gvlvx 0",
    ]);
    expect0.extend((3..12).map(|v| format!("gvlvx {v}")));
    assert_eq!(drain(&mut p, t0), expect0);
    let mut expect1 = strings(&[
        "gvlvx 2", "gvlvx 1", "gstat", "goned 0", "gowvd 0", "gonec", "gowvc", "gvlvx 0",
    ]);
    expect1.extend((3..12).map(|v| format!("gvlvx {v}")));
    expect1.push("gvers".to_string());
    assert_eq!(drain(&mut p, t0 + 1000), expect1);
}

#[test]
fn the_most_overdue_item_wins_over_the_tie_order() {
    let mut p = PollPlanner::new(flat(1000));
    drain(&mut p, 0); // 12 valves
    drain(&mut p, 1000); // counts, 12 valves, gvers: everything handed out at 1000
    p.set_active_mask(1 << 5);
    assert_eq!(next(&mut p, 2600), "gvlvd 5"); // all 600 overdue: active first
    assert_eq!(next(&mut p, 2700), "gonec"); // 700 overdue, counts before inactive
    assert_eq!(next(&mut p, 2700), "gowvc");
    // Valve 5 (active) is 100 ms overdue, valve 0 (inactive) 1700 ms.
    assert_eq!(next(&mut p, 3700), "gvlvd 0");
    assert_eq!(next(&mut p, 3700), "gvlvd 1");
}

#[test]
fn ds18_and_ds2438_round_robin_spread_over_the_period() {
    let mut p = PollPlanner::default();
    p.set_sensor_counts(3, 2);
    drain(&mut p, 0); // valves + goned 0 + gowvd 0
                      // goned every 10000/3 = 3333 ms, gowvd every 5000 ms.
    assert_eq!(next(&mut p, 3332), "-");
    assert_eq!(next(&mut p, 3333), "goned 1");
    assert_eq!(next(&mut p, 4999), "-");
    assert_eq!(next(&mut p, 5000), "gowvd 1");
    assert_eq!(next(&mut p, 6666), "goned 2");
    assert_eq!(next(&mut p, 9999), "goned 0");
    assert_eq!(next(&mut p, 10000), "gowvd 0");
    // Shrinking the count wraps the index.
    p.set_sensor_counts(1, 1);
    assert_eq!(next(&mut p, 19999), "goned 0");
    assert_eq!(next(&mut p, 20000), "gowvd 0");
    p.set_sensor_counts(0, 0);
    assert_eq!(next(&mut p, 29999), "-");
}

#[test]
fn sensor_counts_are_clamped() {
    let mut p = PollPlanner::default();
    p.set_sensor_counts(200, 200);
    let first = drain(&mut p, 0);
    // Primed one full period overdue: far more than their short spacing.
    assert_eq!(first[0], "goned 0");
    assert_eq!(first[1], "gowvd 0");
    assert_eq!(first.len(), 14);
    // 34 DS18 -> one goned every 294 ms, 8 DS2438 -> every 1250 ms.
    assert_eq!(next(&mut p, 293), "-");
    assert_eq!(next(&mut p, 294), "goned 1");
    let mut t = 294;
    for i in 2..34 {
        t += 294;
        let mut s = next(&mut p, t);
        if s.starts_with("gowvd") {
            s = next(&mut p, t);
        }
        assert_eq!(s, format!("goned {i}"));
    }
    t += 294;
    let mut s = next(&mut p, t);
    if s.starts_with("gowvd") {
        s = next(&mut p, t);
    }
    assert_eq!(s, "goned 0"); // wrapped after index 33
}

#[test]
fn gvers_every_5_minutes() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    for t in (1000..300_000).step_by(1000) {
        for s in drain(&mut p, t) {
            assert_ne!(s, "gvers", "{t}");
        }
    }
    for s in drain(&mut p, 299_999) {
        assert_ne!(s, "gvers");
    }
    assert!(drain(&mut p, 300_000).iter().any(|s| s == "gvers"));
    for s in drain(&mut p, 599_999) {
        assert_ne!(s, "gvers");
    }
    assert!(drain(&mut p, 600_000).iter().any(|s| s == "gvers"));
}

// ================================================================ resync

#[test]
fn v1_resync_sequence() {
    let mut p = PollPlanner::default();
    p.request_resync();
    assert!(p.resync_active());
    assert_eq!(p.resync_step(), ResyncStep::Proto);
    let mut now = 0;
    assert_eq!(run_resync(&mut p, &mut now, 0), v1_steps());
    assert_eq!(p.protocol(), 1);
    assert_eq!(p.resync_step(), ResyncStep::Done);
}

#[test]
fn v2_resync_sequence() {
    let mut p = PollPlanner::default();
    p.request_resync();
    let mut now = 0;
    assert_eq!(run_resync(&mut p, &mut now, 2), v2_steps());
    assert_eq!(p.protocol(), 2);
}

#[test]
fn gproto_answered_without_a_usable_protocol_keeps_v1_commands() {
    let mut p = PollPlanner::default();
    p.request_resync();
    let mut now = 0;
    let r = p.next(now).expect("gproto");
    assert_eq!(r.cmd, Cmd::Gproto);
    p.on_result(&r, true, now); // caller did not call set_protocol
    assert_eq!(p.protocol(), 0);
    let rest = run_resync(&mut p, &mut now, 0);
    let mut expect = v1_steps();
    expect.remove(0);
    assert_eq!(rest, expect);
}

#[test]
fn a_gproto_timeout_does_not_downgrade_a_known_protocol() {
    let mut p = PollPlanner::default();
    p.request_resync();
    let r = p.next(0).expect("gproto");
    p.set_protocol(2); // e.g. a stray gproto reply was applied
    p.on_result(&r, false, 0);
    assert_eq!(p.protocol(), 2);
    assert_eq!(p.resync_step(), ResyncStep::Version);
}

#[test]
fn resync_steps_alternate_with_due_items() {
    let mut p = PollPlanner::default();
    p.set_active_mask(0x0FFF);
    p.request_resync();
    let mut seq = Vec::new();
    let mut is_step = Vec::new();
    for _ in 0..8 {
        let r = p.next(0).expect("due");
        seq.push(text(&r));
        is_step.push(p.last_was_resync());
        if p.last_was_resync() {
            if r.cmd == Cmd::Gproto {
                p.set_protocol(2);
            }
            p.on_result(&r, true, 0);
        }
    }
    // gproto and gvers go out alone, then steps alternate with valve polls.
    assert_eq!(
        seq,
        strings(&["gproto", "gvers", "gvlvx 0", "ghwin", "gvlvx 1", "gmotc", "gvlvx 2", "gtlnm"])
    );
    assert_eq!(
        is_step,
        vec![true, true, false, true, false, true, false, true]
    );
}

#[test]
fn steps_go_back_to_back_when_nothing_else_is_due() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_resync();
    let r = p.next(1).expect("gproto");
    assert_eq!(text(&r), "gproto");
    p.on_result(&r, false, 1);
    let r = p.next(1).expect("gvers");
    assert_eq!(text(&r), "gvers");
    assert!(p.last_was_resync());
}

#[test]
fn a_handed_out_step_is_not_repeated_until_its_result() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_resync();
    let r = p.next(1).expect("gproto");
    assert_eq!(text(&r), "gproto");
    assert_eq!(next(&mut p, 2), "-");
    assert_eq!(next(&mut p, 1 + LOST - 1), "-");
    // Lost (dropped by an STM reset or evicted): handed out again.
    assert_eq!(next(&mut p, 1 + LOST), "gproto");
    assert!(p.last_was_resync());
}

#[test]
fn a_failed_step_is_retried_after_valve_active_ms() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_resync();
    let r = p.next(1).expect("gproto");
    p.on_result(&r, false, 1); // gproto timeout -> v1, advance
    let r = p.next(1).expect("gvers");
    assert_eq!(text(&r), "gvers");
    p.on_result(&r, false, 10);
    assert_eq!(p.resync_step(), ResyncStep::Version);
    assert_eq!(next(&mut p, 2009), "-");
    assert_eq!(next(&mut p, 2010), "gvers");
    p.on_result(&r, true, 2010);
    assert_eq!(p.resync_step(), ResyncStep::HwId);
}

#[test]
fn a_failed_targets_step_retries_the_same_valve() {
    let mut p = PollPlanner::default();
    let now = 0;
    drain(&mut p, now);
    p.request_resync();
    // Answer until the Targets step reaches valve 5.
    let mut r = RequestLine::default();
    for _ in 0..50 {
        r = p.next(now).expect("due");
        if r.cmd == Cmd::Gtgtp && r.valve == 5 {
            break;
        }
        p.on_result(&r, r.cmd != Cmd::Gproto, now);
    }
    assert_eq!(r.cmd, Cmd::Gtgtp);
    p.on_result(&r, false, now);
    assert_eq!(p.resync_step(), ResyncStep::Targets);
    assert_eq!(next(&mut p, now + 1999), "-");
    assert_eq!(next(&mut p, now + 2000), "gtgtp 5");
}

#[test]
fn results_for_other_requests_do_not_advance_the_resync() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_resync();
    let r = p.next(1).expect("gproto");
    p.on_result(&build_get_version(), true, 1);
    assert_eq!(p.resync_step(), ResyncStep::Proto);
    p.on_result(&build_calibrate(3).expect("valve"), true, 1);
    assert_eq!(p.resync_step(), ResyncStep::Proto);
    p.on_result(&r, true, 1);
    assert_eq!(p.resync_step(), ResyncStep::Version);
    // A duplicate result for the finished step changes nothing.
    p.on_result(&r, true, 1);
    assert_eq!(p.resync_step(), ResyncStep::Version);
}

#[test]
fn request_resync_restarts_the_sequence_and_resets_the_protocol() {
    let mut p = PollPlanner::default();
    p.request_resync();
    let mut now = 0;
    run_resync(&mut p, &mut now, 2);
    assert_eq!(p.protocol(), 2);
    p.request_profile(3);
    p.request_temp_list();
    p.request_target(4);
    p.request_resync();
    assert_eq!(p.protocol(), 0);
    assert_eq!(p.resync_step(), ResyncStep::Proto);
    // Pending covered one-shots and v2-only one-shots were dropped.
    let steps = run_resync(&mut p, &mut now, 0);
    assert_eq!(steps, v1_steps());
    for _ in 0..40 {
        let s = next(&mut p, now);
        assert_ne!(s, "gprof 3");
        assert_ne!(s, "gonec 255");
        assert_ne!(s, "gtgtp 4");
    }
}

#[test]
fn request_resync_while_a_step_is_in_flight() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_resync();
    let r = p.next(1).expect("gproto");
    p.on_result(&r, false, 1);
    let r = p.next(1).expect("gvers");
    assert_eq!(text(&r), "gvers");
    p.request_resync();
    let r2 = p.next(2).expect("gproto");
    assert_eq!(text(&r2), "gproto");
    p.on_result(&r, true, 3); // the old gvers result is not the current step
    assert_eq!(p.resync_step(), ResyncStep::Proto);
}

// ================================================================ one-shots

#[test]
fn one_shots_are_coalesced_and_wait_for_their_result() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_temp_list();
    p.request_temp_list();
    let r = p.next(1).expect("gonec 255");
    assert_eq!(text(&r), "gonec 255");
    assert!(!p.last_was_resync());
    p.request_temp_list(); // already in flight: nothing new
    assert_eq!(next(&mut p, 2), "-");
    p.on_result(&r, true, 3);
    assert_eq!(next(&mut p, 4), "-");
    // After success a new request is served again.
    p.request_temp_list();
    assert_eq!(next(&mut p, 5), "gonec 255");
}

#[test]
fn failed_one_shot_retried_after_valve_active_ms_lost_after_lost_request_ms() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_volt_list();
    let r = p.next(100).expect("gowvc 255");
    assert_eq!(text(&r), "gowvc 255");
    p.on_result(&r, false, 200);
    assert_eq!(next(&mut p, 2199), "-");
    assert_eq!(next(&mut p, 2200), "gowvc 255");
    assert!(!next(&mut p, 2200 + LOST - 1).starts_with("gowvc 255"));
    // Never answered: handed out again.
    let mut q = PollPlanner::default();
    drain(&mut q, 0);
    q.request_valve_sensors();
    assert_eq!(next(&mut q, 100), "gvlon 255");
    assert_eq!(next(&mut q, 100 + LOST - 1), "-");
    assert_eq!(next(&mut q, 100 + LOST), "gvlon 255");
    assert!(q.next(100 + LOST).is_none());
    let g = build_valve_sensors(ALL_VALVES).expect("all");
    q.on_result(&g, true, 15000);
    assert_eq!(next(&mut q, 15000 + LOST), "-");
}

#[test]
fn one_shot_priority_targets_profiles_lists_motor_params() {
    let mut p = PollPlanner::default();
    p.set_protocol(2);
    drain(&mut p, 0);
    p.request_motor_params();
    p.request_valve_sensors();
    p.request_volt_list();
    p.request_temp_list();
    p.request_profile(7);
    p.request_profile(2);
    p.request_target(9);
    p.request_target(1);
    let got: Vec<String> = (0..12).map(|_| next(&mut p, 1)).collect();
    assert_eq!(
        got,
        strings(&[
            "gvlvx 1",
            "gvlvx 9",
            "gprof 2",
            "gprof 7",
            "gonec 255",
            "gowvc 255",
            "gvlon 255",
            "gmotc",
            "gtlnm",
            "gcalx",
            "-",
            "-"
        ])
    );
}

#[test]
fn one_shots_precede_periodic_items() {
    let mut p = PollPlanner::default();
    p.set_active_mask(0x0FFF);
    p.request_target(3);
    assert_eq!(next(&mut p, 0), "gtgtp 3");
    assert_eq!(next(&mut p, 0), "gvlvd 0");
}

#[test]
fn v1_motor_params_and_target_read_backs() {
    let mut p = PollPlanner::default();
    p.set_protocol(1);
    drain(&mut p, 0);
    p.request_motor_params();
    p.request_target(11);
    p.request_target(12); // ignored
    p.request_target(255); // ignored
    assert_eq!(next(&mut p, 1), "gtgtp 11");
    assert_eq!(next(&mut p, 1), "gmotc");
    assert_eq!(next(&mut p, 1), "gtlnm");
    assert_eq!(next(&mut p, 1), "-");
}

#[test]
fn profiles_only_on_v2() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_profile(3); // proto unknown
    assert_eq!(next(&mut p, 1), "-");
    p.set_protocol(1);
    p.request_profile(3);
    assert_eq!(next(&mut p, 1), "-");
    p.set_protocol(2);
    p.request_profile(12); // invalid
    p.request_profile(3);
    p.request_profile(4);
    assert_eq!(next(&mut p, 1), "gprof 3");
    // Protocol downgrade drops pending and in-flight v2 one-shots.
    p.request_motor_params();
    p.set_protocol(1);
    assert_eq!(next(&mut p, 1), "gmotc");
    assert_eq!(next(&mut p, 1), "gtlnm");
    assert_eq!(next(&mut p, 1 + LOST), "gmotc");
    assert_eq!(next(&mut p, 1 + LOST), "gtlnm");
    assert_eq!(next(&mut p, 1 + LOST), "-");
}

#[test]
fn target_read_back_uses_gvlvx_on_v2_a_periodic_gvlvx_result_also_satisfies_it() {
    let mut p = PollPlanner::default();
    p.set_protocol(2);
    drain(&mut p, 0);
    p.request_target(4);
    let r = p.next(1).expect("gvlvx 4");
    assert_eq!(text(&r), "gvlvx 4");
    assert_eq!(r.cmd, Cmd::Gvlvx);
    let same = build_valve_ex(4).expect("valve");
    p.on_result(&same, true, 2);
    for s in drain(&mut p, 2 + LOST) {
        assert_ne!(s, "gvlvx 4");
    }
}

#[test]
fn an_unrelated_failed_result_does_not_disturb_one_shots() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_temp_list();
    let r = p.next(1).expect("gonec 255");
    let count = build_temp_count(); // same cmd, different arg
    p.on_result(&count, true, 2);
    assert_eq!(next(&mut p, 3), "-"); // still in flight ...
    assert_eq!(next(&mut p, 1 + LOST), "gonec 255"); // ... and still wanted
    p.on_result(&r, true, 3);
    assert_eq!(next(&mut p, 3 + 2 * LOST), "-");
}

#[test]
fn out_and_last_was_resync_on_an_idle_call() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_resync();
    assert!(p.next(1).is_some());
    assert!(p.last_was_resync());
    // C++ also checks that the idle call resets the caller's line (gstat before) to an empty
    // RequestLine: None carries no line.
    assert!(build_get_status().cmd == Cmd::Gstat);
    assert!(p.next(2).is_none());
}

#[test]
fn works_across_the_millis_wrap() {
    let mut p = PollPlanner::default();
    p.set_active_mask(1);
    let t0 = 0xFFFF_FC00u32;
    let first = drain(&mut p, t0);
    assert_eq!(first.len(), 12);
    assert_eq!(next(&mut p, t0.wrapping_add(1999)), "-");
    assert_eq!(next(&mut p, t0.wrapping_add(2000)), "gvlvd 0"); // t0 + 2000 wrapped past 0
}

// ================================================================ edge cases

#[test]
fn set_protocol_2_again_keeps_pending_v2_one_shots() {
    let mut p = PollPlanner::default();
    p.set_protocol(2);
    drain(&mut p, 0);
    p.request_profile(0);
    p.set_protocol(2);
    assert_eq!(next(&mut p, 1), "gprof 0");
}

#[test]
fn valve_0_as_busy_valve_and_as_target_profile_one_shot() {
    let mut p = PollPlanner::default();
    p.set_valve_busy(0, true);
    drain(&mut p, 0);
    assert_eq!(next(&mut p, 499), "-");
    assert_eq!(next(&mut p, 500), "gvlvd 0");
    p.set_protocol(1);
    p.request_target(0);
    let r = p.next(501).expect("gtgtp 0");
    assert_eq!(text(&r), "gtgtp 0");
    p.on_result(&r, true, 502);
    assert_ne!(next(&mut p, 502 + LOST), "gtgtp 0");
}

#[test]
fn a_result_for_a_step_that_was_never_handed_out_is_ignored() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_resync();
    p.on_result(&build_get_proto(), false, 1); // stale timeout from before the restart
    assert_eq!(p.protocol(), 0);
    assert_eq!(p.resync_step(), ResyncStep::Proto);
    // After a step completes, a result for the next step arrives early.
    let r = p.next(2).expect("gproto");
    p.on_result(&r, false, 3);
    assert_eq!(p.resync_step(), ResyncStep::Version);
    p.on_result(&build_get_version(), true, 4);
    assert_eq!(p.resync_step(), ResyncStep::Version);
}

#[test]
fn a_late_success_for_a_failed_step_waits_for_the_retry() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_resync();
    let r = p.next(1).expect("gproto");
    p.on_result(&r, false, 1); // -> Version
    let r = p.next(2).expect("gvers");
    assert_eq!(text(&r), "gvers");
    p.on_result(&r, false, 3); // failed, retry held
    p.on_result(&r, true, 4); // duplicate late reply: not in flight any more
    assert_eq!(p.resync_step(), ResyncStep::Version);
    assert_eq!(next(&mut p, 2003), "gvers");
}

#[test]
fn an_in_flight_one_shot_does_not_block_later_ones() {
    let mut p = PollPlanner::default();
    p.set_protocol(1);
    drain(&mut p, 0);
    p.request_target(0);
    assert_eq!(next(&mut p, 1), "gtgtp 0"); // now held while in flight
    p.request_target(2);
    assert_eq!(next(&mut p, 2), "gtgtp 2");
    p.request_target(1);
    assert_eq!(next(&mut p, 3), "gtgtp 1");
    assert_eq!(next(&mut p, 4), "-");
}

#[test]
fn a_shrinking_sensor_count_restarts_the_round_robin_at_index_0() {
    let mut p = PollPlanner::new(flat(60000));
    p.set_sensor_counts(5, 5);
    // flat(): every item one period overdue at the first call; goned 0 first.
    let first = drain(&mut p, 0);
    assert_eq!(count(&first, "goned 0"), 1);
    // Walk both round robins to index 3 with an ample period per index.
    let mut t = 0;
    for i in 1..=3 {
        t += 12000;
        let due = drain(&mut p, t);
        assert_eq!(count(&due, &format!("goned {i}")), 1, "{i}");
        assert_eq!(count(&due, &format!("gowvd {i}")), 1, "{i}");
    }
    // Shrink to exactly the next index: it is out of range now, so restart at 0.
    p.set_sensor_counts(4, 4);
    t += 15000;
    let due = drain(&mut p, t);
    assert_eq!(count(&due, "goned 0"), 1);
    assert_eq!(count(&due, "gowvd 0"), 1);
    assert_eq!(count(&due, "goned 4"), 0);
    assert_eq!(count(&due, "gowvd 4"), 0);
    // A count that still covers the index keeps it.
    p.set_sensor_counts(3, 3);
    t += 20000;
    let due = drain(&mut p, t);
    assert_eq!(count(&due, "goned 1"), 1);
    assert_eq!(count(&due, "gowvd 1"), 1);
}

#[test]
fn protocol_3_polls_gvlvy_and_gstax_reads_back_and_re_syncs_with_gvlvy() {
    let mut p = PollPlanner::default();
    p.set_protocol(4);
    assert_eq!(p.protocol(), 3);
    p.set_protocol(3);
    let first = drain(&mut p, 0);
    assert_eq!(first.len(), 13);
    assert_eq!(first[0], "gstax");
    assert_eq!(first[1], "gvlvy 0");
    assert_eq!(first[12], "gvlvy 11");
    assert_eq!(next(&mut p, 9999), "-");
    assert_eq!(next(&mut p, 10000), "gstax");
    p.request_target(5);
    assert_eq!(next(&mut p, 10000), "gvlvy 5");
    p.request_resync();
    let mut now = 20000;
    assert_eq!(run_resync(&mut p, &mut now, 3), v3_steps());
    assert_eq!(p.protocol(), 3);
}

#[test]
fn gproto_and_gvers_go_out_alone_valve_polls_wait_for_the_version() {
    let mut p = PollPlanner::default();
    p.set_active_mask(0x0FFF);
    p.request_resync();
    let r = p.next(0).expect("gproto");
    assert_eq!(text(&r), "gproto");
    assert!(p.last_was_resync());
    assert_eq!(next(&mut p, 0), "-"); // in flight: nothing else goes out
    assert_eq!(next(&mut p, 9999), "-");
    p.on_result(&r, false, 100); // probe timed out: protocol 1
    let r = p.next(100).expect("gvers");
    assert_eq!(text(&r), "gvers");
    assert_eq!(next(&mut p, 100), "-");
    p.on_result(&r, false, 200); // failed: retried after valve_active_ms, nothing in between
    assert_eq!(next(&mut p, 2199), "-");
    let r = p.next(2200).expect("gvers");
    assert_eq!(text(&r), "gvers");
    p.on_result(&r, true, 2300);
    assert_eq!(next(&mut p, 2300), "gvlvd 0");
    assert_eq!(p.resync_step(), ResyncStep::HwId);
}

#[test]
fn an_stm_below_1_4_0_ends_the_re_sync_and_gets_gvers_every_30_s() {
    let mut p = PollPlanner::default();
    p.set_active_mask(0x0FFF);
    assert_eq!(p.support(), StmSupport::Unknown);
    assert_eq!(PollCadence::default().unsupported_version_ms, 30000);
    p.request_resync();
    let r = p.next(0).expect("gproto");
    p.on_result(&r, false, 0); // gproto: silent
    let r = p.next(0).expect("gvers");
    assert_eq!(text(&r), "gvers");
    p.request_temp_list();
    p.request_motor_params();
    p.on_version(&ver("1.3.5_C2"));
    assert_eq!(p.support(), StmSupport::TooOld);
    assert!(!p.resync_active());
    p.on_result(&r, true, 10); // the gvers result of the finished step changes nothing
    assert!(!p.resync_active());
    assert_eq!(next(&mut p, 10), "-"); // one-shots dropped, no valve polls
    assert_eq!(next(&mut p, 29999), "-");
    assert_eq!(next(&mut p, 30000), "gvers");
    assert_eq!(next(&mut p, 30000), "-");
    assert_eq!(next(&mut p, 59999), "-");
    assert_eq!(next(&mut p, 60000), "gvers");
    // Only target read-backs are still taken.
    p.request_temp_list();
    p.request_volt_list();
    p.request_valve_sensors();
    p.request_motor_params();
    p.request_profile(1);
    p.request_status();
    p.request_match_sensors(60000);
    assert_eq!(next(&mut p, 70000), "-");
    p.request_target(0);
    assert_eq!(next(&mut p, 70000), "gtgtp 0");
    // The same too old version again changes nothing.
    p.on_version(&ver("1.3.5_C2"));
    assert!(!p.resync_active());
    assert_eq!(p.support(), StmSupport::TooOld);
    // A supported version (after an update) restarts the re-sync.
    p.on_version(&ver("1.4.9_C2"));
    assert!(p.resync_active());
    assert_eq!(p.resync_step(), ResyncStep::Proto);
    assert_eq!(p.support(), StmSupport::Unknown);
    assert_eq!(p.protocol(), 0);
}

#[test]
fn a_supported_version_marks_support_and_keeps_the_re_sync_going() {
    let mut p = PollPlanner::default();
    p.request_resync();
    let r = p.next(0).expect("gproto");
    p.set_protocol(2);
    p.on_result(&r, true, 0);
    let r = p.next(0).expect("gvers");
    assert_eq!(text(&r), "gvers");
    p.on_version(&ver("2.0.0-revamped_C2"));
    assert_eq!(p.support(), StmSupport::Supported);
    assert_eq!(p.resync_step(), ResyncStep::Version);
    p.on_result(&r, true, 0);
    assert_eq!(p.resync_step(), ResyncStep::HwId);
    p.on_version(&ver("garbage"));
    assert_eq!(p.support(), StmSupport::Unknown);
}

#[test]
fn a_different_version_outside_a_re_sync_restarts_it_the_same_one_does_not() {
    let mut p = PollPlanner::default();
    p.on_version(&ver("2.0.0-revamped_C2")); // first version ever: no re-sync
    assert!(!p.resync_active());
    p.on_version(&ver("2.0.0-revamped_C2"));
    assert!(!p.resync_active());
    p.on_version(&ver("garbage")); // an unparsable version keeps the last text
    assert!(!p.resync_active());
    p.on_version(&ver("2.0.0-revamped_C2"));
    assert!(!p.resync_active());
    p.on_version(&ver("2.1.0-revamped_C2"));
    assert!(p.resync_active());
    // Inside the re-sync another change does not restart it again.
    let r = p.next(0).expect("gproto");
    p.on_result(&r, false, 0);
    let r = p.next(0).expect("gvers");
    assert_eq!(text(&r), "gvers");
    p.on_version(&ver("2.1.1-revamped_C2"));
    assert_eq!(p.resync_step(), ResyncStep::Version);
    p.on_result(&r, true, 0);
    assert_eq!(p.resync_step(), ResyncStep::HwId);
}

#[test]
fn a_revamped_gvers_on_protocol_1_arms_a_gproto_probe_success_re_syncs() {
    let mut p = PollPlanner::default();
    p.request_resync();
    let mut now = 0;
    assert_eq!(run_resync(&mut p, &mut now, 0), v1_steps());
    assert_eq!(p.protocol(), 1);
    drain(&mut p, now);
    p.on_version(&ver("2.1.0-revamped_C2"));
    assert!(!p.resync_active());
    let r = p.next(now).expect("gproto");
    assert_eq!(text(&r), "gproto");
    assert!(r.probe);
    assert!(!p.last_was_resync());
    p.set_protocol(3);
    p.on_result(&r, true, now);
    assert!(p.resync_active());
    assert_eq!(p.resync_step(), ResyncStep::Proto);
    assert_eq!(p.protocol(), 0);
}

#[test]
fn a_timed_out_probe_leaves_protocol_1_and_the_next_gvers_arms_it_again() {
    let mut p = PollPlanner::default();
    p.request_resync();
    let mut now = 0;
    run_resync(&mut p, &mut now, 0);
    drain(&mut p, now);
    p.on_version(&ver("2.1.0-revamped_C2"));
    let r = p.next(now).expect("gproto");
    assert_eq!(text(&r), "gproto");
    p.on_result(&r, false, now);
    assert_eq!(p.protocol(), 1);
    assert!(!p.resync_active());
    // A timeout drops the probe until the next gvers.
    for s in drain(&mut p, now + 100_000) {
        assert_ne!(s, "gproto");
    }
    now += 100_000;
    p.on_version(&ver("2.1.0-revamped_C2"));
    let r = p.next(now).expect("gproto");
    assert_eq!(text(&r), "gproto");
    p.on_result(&r, true, now); // answered, but protocol 1 is still set: no re-sync
    assert!(!p.resync_active());
    assert_eq!(p.protocol(), 1);
    drain(&mut p, now);
    p.on_version(&ver("2.1.0-revamped_C2"));
    assert_eq!(next(&mut p, now), "gproto");
}

#[test]
fn no_probe_for_a_legacy_version_or_on_protocol_0_2_and_3() {
    for proto in [0u8, 2, 3] {
        let mut p = PollPlanner::default();
        p.set_protocol(proto);
        drain(&mut p, 0);
        p.on_version(&ver("2.1.0-revamped_C2"));
        for s in drain(&mut p, 100_000) {
            assert_ne!(s, "gproto", "{proto}");
        }
    }
    let mut p = PollPlanner::default();
    p.request_resync();
    let mut now = 0;
    run_resync(&mut p, &mut now, 0);
    drain(&mut p, now);
    p.on_version(&ver("1.4.9_C2"));
    for s in drain(&mut p, now + 100_000) {
        assert_ne!(s, "gproto");
    }
}

#[test]
fn request_status_asks_gstax_on_3_gstat_on_2_nothing_on_0_or_1() {
    let want = ["-", "-", "gstat", "gstax"];
    for proto in 0..=3u8 {
        let mut p = PollPlanner::default();
        p.set_protocol(proto);
        drain(&mut p, 0);
        p.request_status();
        assert_eq!(next(&mut p, 1), want[usize::from(proto)], "{proto}");
    }
}

#[test]
fn a_status_one_shot_is_dropped_when_the_protocol_falls_below_2() {
    let mut p = PollPlanner::default();
    p.set_protocol(3);
    drain(&mut p, 0);
    p.request_status();
    p.set_protocol(1);
    assert_eq!(next(&mut p, 1), "-");
}

#[test]
fn masns_waits_5_s_after_the_request_coalesces_and_retries_after_a_failure() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_match_sensors(1000);
    assert_eq!(next(&mut p, 5999), "-");
    p.request_match_sensors(3000); // coalesced: the first delay stays
    let r = p.next(6000).expect("masns");
    assert_eq!(text(&r), "masns");
    assert_eq!(next(&mut p, 6000), "-");
    p.on_result(&r, false, 6000);
    assert_eq!(next(&mut p, 7999), "-");
    let r = p.next(8000).expect("masns");
    assert_eq!(text(&r), "masns");
    p.on_result(&r, true, 8000);
    assert_ne!(next(&mut p, 8000 + LOST), "masns");
}

#[test]
fn request_resync_drops_a_pending_masns() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_match_sensors(0);
    p.request_resync();
    let mut now = 10000;
    run_resync(&mut p, &mut now, 0);
    for s in drain(&mut p, now + 100_000) {
        assert_ne!(s, "masns");
    }
}

// ================================================================ Rust additions

#[test]
fn the_most_overdue_item_is_measured_by_its_overdue_not_its_age() {
    // A busy valve 100 ms overdue loses to an active valve 300 ms overdue.
    let mut p = PollPlanner::default();
    p.set_active_mask(1 << 1);
    p.set_valve_busy(0, true);
    drain(&mut p, 0);
    assert_eq!(next(&mut p, 1700), "gvlvd 0");
    assert_eq!(next(&mut p, 2300), "gvlvd 1");
    assert_eq!(next(&mut p, 2300), "gvlvd 0");
}

#[test]
fn an_unparsable_version_between_two_versions_keeps_the_last_text() {
    let mut p = PollPlanner::default();
    p.on_version(&ver("2.0.0-revamped_C2"));
    p.on_version(&ver("garbage"));
    assert!(!p.resync_active());
    p.on_version(&ver("2.1.0-revamped_C2"));
    assert!(p.resync_active());
}

#[test]
fn a_result_or_a_protocol_change_touches_only_its_own_in_flight_bits() {
    // A gproto result that no probe asked for never re-syncs, whatever happened before.
    let mut p = PollPlanner::default();
    p.set_protocol(2);
    drain(&mut p, 0);
    p.request_temp_list();
    let r = p.next(1).expect("gonec 255");
    assert_eq!(text(&r), "gonec 255");
    p.on_result(&r, true, 1);
    p.on_result(&build_get_proto(), true, 2);
    assert!(!p.resync_active());
    p.set_protocol(1);
    p.set_protocol(2);
    p.on_result(&build_get_proto(), true, 3);
    assert!(!p.resync_active());
}

#[test]
fn one_shots_in_flight_keep_their_own_bits() {
    // A new one-shot and the result of another leave an in-flight one-shot alone: its own
    // result still completes it.
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_temp_list();
    let r = p.next(1).expect("gonec 255");
    p.request_volt_list();
    let v = p.next(2).expect("gowvc 255");
    assert_eq!(text(&v), "gowvc 255");
    p.on_result(&r, true, 3);
    p.on_result(&v, true, 4);
    for s in drain(&mut p, 2 + LOST) {
        assert_ne!(s, "gonec 255");
        assert_ne!(s, "gowvc 255");
    }
}

#[test]
fn an_unparsable_version_after_too_old_starts_no_re_sync() {
    let mut p = PollPlanner::default();
    p.on_version(&ver("1.3.5_C2"));
    assert_eq!(p.support(), StmSupport::TooOld);
    p.on_version(&ver("garbage"));
    assert_eq!(p.support(), StmSupport::Unknown);
    assert!(!p.resync_active());
}

#[test]
fn a_protocol_change_keeps_v1_one_shots_in_flight() {
    let mut p = PollPlanner::default();
    drain(&mut p, 0);
    p.request_temp_list();
    let r = p.next(1).expect("gonec 255");
    p.set_protocol(1);
    p.on_result(&r, true, 2);
    for s in drain(&mut p, 1 + LOST) {
        assert_ne!(s, "gonec 255");
    }
}

#[test]
fn request_resync_forgets_a_probe_in_flight() {
    let mut p = PollPlanner::default();
    p.request_resync();
    let mut now = 0;
    run_resync(&mut p, &mut now, 0);
    drain(&mut p, now);
    p.on_version(&ver("2.1.0-revamped_C2")); // arms the probe
    let probe = p.next(now).expect("gproto");
    assert_eq!(text(&probe), "gproto");
    p.request_resync(); // while the probe is in flight
    let step = p.next(now).expect("gproto step");
    assert!(p.last_was_resync());
    p.set_protocol(2);
    p.on_result(&step, true, now);
    assert_eq!(p.resync_step(), ResyncStep::Version);
}

#[test]
fn one_shot_priority_ends_with_probe_status_masns() {
    let mut p = PollPlanner::default();
    p.set_protocol(2);
    drain(&mut p, 0);
    p.request_match_sensors(0);
    p.request_status();
    p.request_motor_params();
    assert_eq!(next(&mut p, 5000), "gmotc");
    assert_eq!(next(&mut p, 5000), "gtlnm");
    assert_eq!(next(&mut p, 5000), "gcalx");
    assert_eq!(next(&mut p, 5000), "gstat");
    assert_eq!(next(&mut p, 5000), "masns");
}
