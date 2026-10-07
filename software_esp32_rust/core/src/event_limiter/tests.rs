//! Port of test/native/test_event_limiter.cpp: EventRateLimiter and EventAggregator.
//!
//! The C++ `offer(e, now, flushed)` and `poll(now, out)` return true and fill their
//! out-parameter; the Rust functions return the event, so `CHECK_FALSE(...)` is `is_none()`.

use super::*;
use crate::common::NO_VALVE;
use crate::event_log::{make_event, EventCode};

use Severity::{Critical, Debug, Error, Info, Warning};

type C = EventCode;

/// C++ `event(c, valve, sev)` (C++ defaults: valve 0, Warning).
fn event(c: EventCode, valve: u8, sev: Severity) -> Event {
    make_event(c, sev, valve, 0, 0, b"")
}

/// C++ `event(c, valve)`.
fn warn(c: EventCode, valve: u8) -> Event {
    event(c, valve, Warning)
}

/// C++ `at(c, valve, seq, sev)` (C++ default: Warning).
fn at(c: EventCode, valve: u8, seq: u32, sev: Severity) -> Event {
    let mut e = make_event(c, sev, valve, 60, 0, b"");
    e.seq = seq;
    e
}

#[test]
fn constants_and_defaults_are_the_cpp_ones() {
    assert_eq!(EventRateLimiter::KEYS, 64);
    assert_eq!(EventAggregator::WINDOW_MS, 2000);
    assert_eq!(EventAggregator::SLOTS, 4);
    assert_eq!(
        EventRateLimiter::default(),
        EventRateLimiter::new(600_000, 30, 30, 60_000)
    );
    assert_eq!(EventRateLimiter::default().suppressed(), 0);
    assert_eq!(EventAggregator::default().duplicates(), 0);
}

#[test]
fn event_rate_limiter_severity_gate_and_per_key_limit() {
    let mut rl = EventRateLimiter::default();
    assert!(!rl.allow(&event(C::TargetSet, 0, Info), 0));
    assert!(!rl.allow(&event(C::ValveStateChanged, 0, Debug), 0));
    assert_eq!(rl.suppressed(), 0);
    assert!(rl.allow(&event(C::CalibOk, 0, Info), 0));
    assert!(rl.allow(&warn(C::EarlyStop, 0), 0));
    assert!(!rl.allow(&warn(C::EarlyStop, 0), 1));
    assert_eq!(rl.suppressed(), 1);
    assert!(rl.allow(&warn(C::EarlyStop, 1), 1)); // other valve
    assert!(rl.allow(&warn(C::CmdRejected, 0), 1)); // other code
    assert!(rl.allow(&event(C::LinkDown, NO_VALVE, Error), 1));
    assert!(!rl.allow(&warn(C::EarlyStop, 0), 599_999));
    assert_eq!(rl.suppressed(), 2);
    assert!(rl.allow(&warn(C::EarlyStop, 0), 600_000));
    assert!(!rl.allow(&warn(C::EarlyStop, 0), 600_001));
    assert!(rl.allow(&event(C::StmFlashFailed, NO_VALVE, Critical), 600_001));
}

#[test]
fn event_rate_limiter_hourly_token_bucket() {
    let mut rl = EventRateLimiter::new(600_000, 30, 30, 60_000);
    for i in 0..30 {
        assert!(rl.allow(&warn(C::EarlyStop, i), 1000));
    }
    assert!(!rl.allow(&warn(C::EarlyStop, 100), 1000));
    assert_eq!(rl.suppressed(), 1);
    // One token per 120 s.
    assert!(!rl.allow(&warn(C::EarlyStop, 101), 1000 + 119_999));
    assert!(rl.allow(&warn(C::EarlyStop, 102), 1000 + 120_000));
    assert!(!rl.allow(&warn(C::EarlyStop, 103), 1000 + 120_000));
    // Frequent calls must not lose the fractional refill.
    let mut f = EventRateLimiter::new(600_000, 30, 30, 60_000);
    for i in 0..30 {
        assert!(f.allow(&warn(C::EarlyStop, i), 0));
    }
    let mut t = 0;
    while t < 119_980 {
        t += 20;
        assert!(!f.allow(&event(C::TargetSet, 0, Info), t));
    }
    assert!(!f.allow(&warn(C::EarlyStop, 200), 119_999));
    assert!(f.allow(&warn(C::EarlyStop, 201), 120_000));
    // The bucket never exceeds its capacity.
    let mut c = EventRateLimiter::new(1, 2, 30, 60_000);
    assert!(c.allow(&warn(C::EarlyStop, 0), 0));
    assert!(c.allow(&warn(C::EarlyStop, 1), 0));
    assert!(!c.allow(&warn(C::EarlyStop, 2), 0));
    assert!(c.allow(&warn(C::EarlyStop, 3), 36_000_000));
    assert!(c.allow(&warn(C::EarlyStop, 4), 36_000_000));
    assert!(!c.allow(&warn(C::EarlyStop, 5), 36_000_000));
    // Refill precision at one token per hour, and the remainder is dropped when the bucket is
    // full.
    let mut h = EventRateLimiter::new(1, 1, 30, 60_000);
    assert!(h.allow(&warn(C::EarlyStop, 0), 0));
    assert!(!h.allow(&warn(C::EarlyStop, 1), 3_599_999));
    assert!(h.allow(&warn(C::EarlyStop, 2), 3_600_000));
    let mut q = EventRateLimiter::new(1, 1, 30, 60_000);
    assert!(q.allow(&warn(C::EarlyStop, 0), 0));
    assert!(q.allow(&warn(C::EarlyStop, 1), 3_601_800));
    assert!(!q.allow(&warn(C::EarlyStop, 2), 7_200_000));
    assert!(q.allow(&warn(C::EarlyStop, 3), 7_201_800));
    // No budget at all.
    let mut z = EventRateLimiter::new(600_000, 0, 30, 60_000);
    assert!(!z.allow(&warn(C::EarlyStop, 0), 0));
    assert!(!z.allow(&warn(C::EarlyStop, 0), 3_600_000));
    assert_eq!(z.suppressed(), 2);
}

#[test]
fn event_rate_limiter_key_table_recycles_the_least_recently_used_entry() {
    let mut rl = EventRateLimiter::new(1_000_000_000, 1000, 30, 60_000);
    for i in 0..EventRateLimiter::KEYS as u8 {
        assert!(rl.allow(&warn(C::EarlyStop, i), 10 + u32::from(i)));
    }
    assert!(rl.allow(&warn(C::EarlyStop, 200), 100)); // evicts valve 0
    assert!(!rl.allow(&warn(C::EarlyStop, 5), 101));
    assert!(!rl.allow(&warn(C::EarlyStop, 200), 101));
    assert!(rl.allow(&warn(C::EarlyStop, 0), 102)); // evicts valve 1
    assert!(rl.allow(&warn(C::EarlyStop, 1), 103)); // evicts valve 2
    assert!(!rl.allow(&warn(C::EarlyStop, 3), 104));
    assert!(rl.allow(&warn(C::EarlyStop, 2), 105));
}

#[test]
fn event_rate_limiter_of_equally_old_entries_the_first_one_is_recycled() {
    let mut rl = EventRateLimiter::new(1_000_000_000, 1000, 30, 60_000);
    for i in 0..EventRateLimiter::KEYS as u8 {
        assert!(rl.allow(&warn(C::EarlyStop, i), 10));
    }
    assert!(rl.allow(&warn(C::EarlyStop, 200), 20)); // evicts valve 0
    let last = EventRateLimiter::KEYS as u8 - 1;
    assert!(!rl.allow(&warn(C::EarlyStop, last), 21));
    assert!(rl.allow(&warn(C::EarlyStop, 0), 22));
}

#[test]
fn event_rate_limiter_entries_expire_across_a_millis_wrap() {
    let mut rl = EventRateLimiter::new(600_000, 30, 30, 60_000);
    let t0: u32 = 0xFFFF_FF00;
    assert!(rl.allow(&warn(C::EarlyStop, 0), t0));
    assert!(!rl.allow(&warn(C::EarlyStop, 0), t0.wrapping_add(599_999)));
    assert!(rl.allow(&warn(C::EarlyStop, 0), t0.wrapping_add(600_000)));
}

#[test]
fn event_rate_limiter_error_events_have_their_own_bucket() {
    let mut rl = EventRateLimiter::default();
    for i in 0..30 {
        assert!(rl.allow(&warn(C::EarlyStop, i), 0));
    }
    assert!(!rl.allow(&warn(C::EarlyStop, 30), 0)); // Normal bucket empty
    assert!(rl.allow(&event(C::LinkDown, NO_VALVE, Error), 0));
    assert_eq!(rl.suppressed(), 1);
    let mut e = EventRateLimiter::default();
    for i in 0..30 {
        assert!(e.allow(&event(C::CalibFailed, i, Error), 0));
    }
    assert!(!e.allow(&event(C::CalibFailed, 30, Error), 0)); // Error bucket empty
    assert!(e.allow(&warn(C::EarlyStop, 0), 0)); // Warning still passes
    assert_eq!(e.suppressed(), 1);
    // The Error bucket refills on its own: one token per 3600000 / 30 ms.
    assert!(!e.allow(&event(C::CalibFailed, 31, Error), 119_999));
    assert!(e.allow(&event(C::CalibFailed, 32, Error), 120_000));
    // Its size is the third parameter.
    let mut s = EventRateLimiter::new(600_000, 30, 1, 60_000);
    assert!(s.allow(&event(C::LinkDown, 0, Error), 0));
    assert!(!s.allow(&event(C::LinkDown, 1, Error), 0));
    assert!(s.allow(&warn(C::EarlyStop, 1), 0));
    // An Error key is held per_key_ms (not the Critical minute).
    let mut k = EventRateLimiter::new(600_000, 30, 30, 1000);
    assert!(k.allow(&event(C::LinkDown, 0, Error), 0));
    assert!(!k.allow(&event(C::LinkDown, 0, Error), 599_999));
    assert!(k.allow(&event(C::LinkDown, 0, Error), 600_000));
}

#[test]
fn event_rate_limiter_critical_events_bypass_the_buckets_one_per_key_per_minute() {
    let mut rl = EventRateLimiter::new(600_000, 30, 30, 60_000);
    for i in 0..30 {
        assert!(rl.allow(&warn(C::EarlyStop, i), 0));
        assert!(rl.allow(&event(C::CalibFailed, i, Error), 0));
    }
    let crit = event(C::StmFlashFailed, NO_VALVE, Critical);
    assert!(rl.allow(&crit, 0));
    assert!(!rl.allow(&crit, 59_999));
    assert_eq!(rl.suppressed(), 1);
    assert!(rl.allow(&crit, 60_000));
    assert!(!rl.allow(&crit, 119_999));
    assert!(rl.allow(&crit, 120_000));
    // Critical takes no tokens: both buckets refilled exactly one each by now.
    assert!(rl.allow(&warn(C::EarlyStop, 100), 120_000));
    assert!(!rl.allow(&warn(C::EarlyStop, 101), 120_000));
    assert!(rl.allow(&event(C::CalibFailed, 100, Error), 120_000));
    assert!(!rl.allow(&event(C::CalibFailed, 101, Error), 120_000));
    // Another per-key time for Critical.
    let mut c = EventRateLimiter::new(600_000, 30, 30, 1000);
    assert!(c.allow(&crit, 0));
    assert!(!c.allow(&crit, 999));
    assert!(c.allow(&crit, 1000));
}

#[test]
fn event_rate_limiter_events_that_do_not_reach_mqtt_are_refused_without_counting() {
    let mut rl = EventRateLimiter::default();
    assert!(!rl.allow(&event(C::FilesRemoved, NO_VALVE, Warning), 0)); // column No
    assert!(!rl.allow(&event(C::LinkDown, NO_VALVE, Info), 0)); // below Warning
    assert_eq!(rl.suppressed(), 0);
    assert!(rl.allow(&event(C::CalibOk, 0, Info), 0)); // Always
}

#[test]
fn event_aggregator_one_event_for_the_same_code_on_several_valves() {
    let mut a = EventAggregator::default();
    for v in 0..VALVE_COUNT {
        let e = at(C::ValveStale, v, 10 + u32::from(v), Warning);
        assert!(a.offer(&e, u32::from(v)).is_none());
    }
    assert!(a.poll(1999, false).is_none());
    let out = a.poll(2000, false).unwrap();
    assert_eq!(out.valve_mask, 0x0FFF);
    assert_eq!(out.event.valve, ALL_VALVES);
    assert_eq!(out.event.seq, 10); // the first event's
    assert_eq!(out.event.arg1, 60);
    assert_eq!(out.event.code, C::ValveStale);
    assert!(a.poll(100_000, false).is_none());
    assert_eq!(a.duplicates(), 0);
}

#[test]
fn event_aggregator_a_single_valve_is_the_original_event() {
    let mut a = EventAggregator::default();
    assert!(a.offer(&at(C::EarlyStop, 3, 7, Info), 100).is_none());
    assert!(a.poll(2099, false).is_none());
    let out = a.poll(2100, false).unwrap();
    assert_eq!(out.valve_mask, 0x0008);
    assert_eq!(out.event.valve, 3);
    assert_eq!(out.event.seq, 7);
    assert_eq!(out.event.severity, Info);
    assert_eq!(out.event, at(C::EarlyStop, 3, 7, Info));
}

#[test]
fn event_aggregator_highest_severity_duplicates_non_valve_events_flush_all_reset() {
    let mut a = EventAggregator::default();
    assert!(a.offer(&at(C::EarlyStop, 0, 1, Warning), 0).is_none());
    assert!(a.offer(&at(C::EarlyStop, 1, 2, Critical), 1).is_none());
    assert!(a.offer(&at(C::EarlyStop, 2, 3, Info), 2).is_none());
    assert!(a.offer(&at(C::EarlyStop, 1, 4, Error), 3).is_none()); // duplicate valve
    assert_eq!(a.duplicates(), 1);
    assert!(a
        .offer(&at(C::EarlyStop, NO_VALVE, 5, Warning), 4)
        .is_none()); // not taken
    assert!(a
        .offer(&at(C::EarlyStop, ALL_VALVES, 6, Warning), 4)
        .is_none());
    assert!(a
        .offer(&at(C::EarlyStop, VALVE_COUNT, 6, Warning), 4)
        .is_none());
    assert!(a.poll(5, false).is_none());
    let out = a.poll(5, true).unwrap();
    assert_eq!(out.valve_mask, 0x0007);
    assert_eq!(out.event.severity, Critical);
    assert_eq!(out.event.seq, 1);
    assert!(a.poll(5, true).is_none());
    // Two valves at the first event's severity when nothing is higher.
    assert!(a.offer(&at(C::EarlyStop, 4, 8, Error), 10).is_none());
    assert!(a.offer(&at(C::EarlyStop, 5, 9, Warning), 10).is_none());
    let out = a.poll(5000, false).unwrap();
    assert_eq!(out.event.severity, Error);
    assert_eq!(out.valve_mask, 0x0030);
    assert!(a.offer(&at(C::EarlyStop, 0, 7, Warning), 10).is_none());
    a.reset();
    assert!(a.poll(100_000, true).is_none());
    assert_eq!(a.duplicates(), 1); // reset keeps the counter
}

#[test]
fn event_aggregator_a_fifth_code_flushes_the_oldest_slot() {
    let mut a = EventAggregator::default();
    let codes = [
        C::EarlyStop,
        C::ValveStale,
        C::CalibFailed,
        C::TargetNotConfirmed,
        C::CmdRejected,
    ];
    assert!(a.offer(&at(codes[0], 0, 1, Warning), 10).is_none());
    assert!(a.offer(&at(codes[1], 1, 2, Warning), 5).is_none()); // opened earlier: the oldest
    assert!(a.offer(&at(codes[2], 2, 3, Warning), 20).is_none());
    assert!(a.offer(&at(codes[3], 3, 4, Warning), 30).is_none());
    let f = a.offer(&at(codes[4], 4, 5, Warning), 40).unwrap();
    assert_eq!(f.event.code, C::ValveStale);
    assert_eq!(f.event.valve, 1);
    assert_eq!(f.valve_mask, 0x0002);
    // Oldest first when several windows passed.
    assert_eq!(a.poll(100_000, false).unwrap().event.code, C::EarlyStop);
    assert_eq!(a.poll(100_000, false).unwrap().event.code, C::CalibFailed);
    assert_eq!(
        a.poll(100_000, false).unwrap().event.code,
        C::TargetNotConfirmed
    );
    assert_eq!(a.poll(100_000, false).unwrap().event.code, C::CmdRejected);
    assert!(a.poll(100_000, false).is_none());
    // Windows across a millis() wrap.
    let mut w = EventAggregator::default();
    assert!(w.offer(&at(codes[0], 0, 1, Warning), 0xFFFF_FF00).is_none());
    assert!(w.poll(0xFFFF_FF00u32.wrapping_add(1999), false).is_none());
    assert!(w.poll(0xFFFF_FF00u32.wrapping_add(2000), false).is_some());
}

#[test]
fn event_aggregator_of_equally_old_slots_the_first_one_goes_first() {
    // The C++ takes a slot only when it is strictly older than the one found so far.
    let codes = [
        C::EarlyStop,
        C::ValveStale,
        C::CalibFailed,
        C::TargetNotConfirmed,
        C::CmdRejected,
    ];
    let mut a = EventAggregator::default();
    for (i, &code) in codes[..4].iter().enumerate() {
        assert!(a.offer(&at(code, i as u8, i as u32, Warning), 10).is_none());
    }
    let f = a.offer(&at(codes[4], 4, 4, Warning), 20).unwrap();
    assert_eq!((f.event.code, f.event.seq), (C::EarlyStop, 0));
    // poll: slots 1..3 and the new slot 0 (opened later) passed their windows
    assert_eq!(a.poll(5000, false).unwrap().event.code, C::ValveStale);
    let mut p = EventAggregator::default();
    assert!(p.offer(&at(codes[2], 0, 1, Warning), 100).is_none());
    assert!(p.offer(&at(codes[3], 1, 2, Warning), 100).is_none());
    assert_eq!(p.poll(2100, false).unwrap().event.code, C::CalibFailed);
    assert_eq!(
        p.poll(2100, false).unwrap().event.code,
        C::TargetNotConfirmed
    );
    assert!(p.poll(2100, true).is_none());
}
