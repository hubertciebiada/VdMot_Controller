//! Port of test/native/test_sys_health.cpp: stack threshold, ResourceMonitor, HeapGuard,
//! http_status_ok, reset reasons.

use super::*;
use crate::event_log::{Severity, EVENT_TEXT_MAX};
use crate::test_support::assert_text;

/// One sample per second from `from_ms` to `to_ms` (both included) with the same figures; the
/// uptime of the first sample that asks for the restart, 0 if none.
fn first_restart(
    g: &mut HeapGuard,
    free_heap: u32,
    blocked: bool,
    from_ms: u32,
    to_ms: u32,
) -> u32 {
    (from_ms..=to_ms)
        .step_by(1000)
        .find(|&t| g.on_sample(free_heap, blocked, t))
        .unwrap_or(0)
}

fn events<const N: usize>() -> [Event; N] {
    core::array::from_fn(|_| Event::default())
}

#[test]
fn constants_are_the_cpp_ones() {
    assert_eq!(ResourceMonitor::LOW_HEAP_BYTES, 30_720);
    assert_eq!(ResourceMonitor::LOW_LARGEST_BLOCK_BYTES, 8192);
    assert_eq!(ResourceMonitor::REPEAT_MS, 3_600_000);
    assert_eq!(ResourceMonitor::MAX_TASKS, 8);
    assert_eq!(HeapGuard::CRITICAL_BYTES, 12_288);
    assert_eq!(HeapGuard::HOLD_MS, 60_000);
    assert_eq!(HeapGuard::ARM_MS, 600_000);
}

#[test]
fn stack_low_threshold_cases() {
    assert_eq!(stack_low_threshold(0), 512);
    assert_eq!(stack_low_threshold(4096), 512);
    assert_eq!(stack_low_threshold(4103), 512);
    assert_eq!(stack_low_threshold(4104), 513);
    assert_eq!(stack_low_threshold(6144), 768);
    assert_eq!(stack_low_threshold(16_384), 2048);
}

#[test]
fn resource_monitor_on_heap_low_heap_below_30_kib_again_after_an_hour() {
    let mut m = ResourceMonitor::default();
    assert_eq!(m.min_largest_block(), 0);
    let mut ev = events::<2>();
    assert_eq!(m.on_heap(30_720, 20_000, 90_000, 0, &mut ev), 0);
    assert_eq!(m.min_largest_block(), 90_000);
    assert_eq!(m.on_heap(30_719, 20_000, 90_000, 10_000, &mut ev), 1);
    assert_eq!(ev[0].code, EventCode::LowHeap);
    assert_eq!(ev[0].severity, event_default_severity(EventCode::LowHeap));
    assert_eq!(ev[0].severity, Severity::Warning);
    assert_eq!(ev[0].valve, NO_VALVE);
    assert_eq!(ev[0].arg1, 30_719);
    assert_eq!(ev[0].arg2, 20_000);
    assert!(ev[0].text.is_empty());
    assert_eq!(m.on_heap(100, 50, 90_000, 10_000 + 3_599_999, &mut ev), 0);
    assert_eq!(m.on_heap(100, 50, 90_000, 10_000 + 3_600_000, &mut ev), 1);
    assert_eq!(ev[0].arg1, 100);
    assert_eq!(ev[0].arg2, 50);
    assert_eq!(m.on_heap(100, 50, 90_000, 10_000 + 3_600_001, &mut ev), 0);
}

#[test]
fn resource_monitor_on_heap_heap_fragmented_below_8_kib_largest_block() {
    let mut m = ResourceMonitor::default();
    let mut ev = events::<2>();
    assert_eq!(m.on_heap(100_000, 90_000, 8192, 0, &mut ev), 0);
    assert_eq!(m.on_heap(100_000, 90_000, 8191, 1000, &mut ev), 1);
    assert_eq!(ev[0].code, EventCode::HeapFragmented);
    assert_eq!(
        ev[0].severity,
        event_default_severity(EventCode::HeapFragmented)
    );
    assert_eq!(ev[0].valve, NO_VALVE);
    assert_eq!(ev[0].arg1, 8191);
    assert_eq!(ev[0].arg2, 100_000);
    assert_eq!(
        m.on_heap(100_000, 90_000, 100, 1000 + 3_599_999, &mut ev),
        0
    );
    assert_eq!(
        m.on_heap(100_000, 90_000, 100, 1000 + 3_600_000, &mut ev),
        1
    );
    assert_eq!(m.min_largest_block(), 100);
    assert_eq!(m.on_heap(100_000, 90_000, 5000, 5_000_000, &mut ev), 0);
    assert_eq!(m.min_largest_block(), 100);
}

#[test]
fn resource_monitor_on_heap_both_at_once_max_out_and_null_out() {
    let mut m = ResourceMonitor::default();
    let mut ev = events::<2>();
    assert_eq!(m.on_heap(29_000, 28_000, 4000, 0, &mut ev), 2);
    assert_eq!(ev[0].code, EventCode::LowHeap);
    assert_eq!(ev[1].code, EventCode::HeapFragmented);
    assert_eq!(ev[1].arg1, 4000);
    assert_eq!(ev[1].arg2, 29_000);
    let mut one = ResourceMonitor::default();
    assert_eq!(one.on_heap(29_000, 28_000, 4000, 0, &mut ev[..1]), 1);
    assert_eq!(ev[0].code, EventCode::LowHeap);
    // The fragmentation alarm was not written, so it is still due.
    assert_eq!(one.on_heap(29_000, 28_000, 4000, 10_000, &mut ev[..1]), 1);
    assert_eq!(ev[0].code, EventCode::HeapFragmented);
    // C++ onHeap(.., nullptr, 2) and onHeap(.., ev, 0): the empty output
    let mut none = ResourceMonitor::default();
    assert_eq!(none.on_heap(29_000, 28_000, 4000, 0, &mut []), 0);
    assert_eq!(none.min_largest_block(), 4000);
    assert_eq!(none.on_heap(29_000, 28_000, 4000, 0, &mut ev[..0]), 0);
    assert_eq!(none.on_heap(29_000, 28_000, 4000, 0, &mut ev), 2);
}

#[test]
fn resource_monitor_on_heap_byte_counts_become_int32_like_the_cpp_casts() {
    let mut m = ResourceMonitor::default();
    let mut ev = events::<2>();
    // a free heap figure above i32::MAX cannot be low; the largest block is reported as int32
    assert_eq!(m.on_heap(0x8000_0005, 7, 100, 0, &mut ev), 1);
    assert_eq!(ev[0].code, EventCode::HeapFragmented);
    assert_eq!((ev[0].arg1, ev[0].arg2), (100, i32::MIN + 5));
}

#[test]
fn resource_monitor_on_stack() {
    let mut m = ResourceMonitor::default();
    let mut ev = events::<1>();
    assert_eq!(m.on_stack(0, b"stm", 6144, 768, &mut ev), 0);
    assert_eq!(m.on_stack(0, b"stm", 6144, 767, &mut ev), 1);
    assert_eq!(ev[0].code, EventCode::StackLow);
    assert_eq!(ev[0].severity, event_default_severity(EventCode::StackLow));
    assert_eq!(ev[0].valve, NO_VALVE);
    assert_eq!(ev[0].arg1, 767);
    assert_eq!(ev[0].arg2, 6144);
    assert_text(&ev[0].text, "stm");
    assert_eq!(m.on_stack(0, b"stm", 6144, 10, &mut ev), 0); // once per task per boot
    assert_eq!(m.on_stack(7, b"app", 8192, 100, &mut ev), 1);
    assert_text(&ev[0].text, "app");
    assert_eq!(m.on_stack(7, b"app", 8192, 100, &mut ev), 0);
    assert_eq!(m.on_stack(1, b"mqtt", 8192, 1023, &mut ev), 1);
    assert_eq!(m.on_stack(8, b"x", 8192, 0, &mut ev), 0);
    assert_eq!(m.on_stack(255, b"x", 8192, 0, &mut ev), 0);
    // C++ onStack(2, .., nullptr, 1) and onStack(2, .., ev, 0): the empty output
    assert_eq!(m.on_stack(2, b"x", 8192, 0, &mut []), 0);
    assert_eq!(m.on_stack(2, b"x", 8192, 0, &mut ev[..0]), 0);
    // Not reported by the calls above: still due.
    assert_eq!(
        m.on_stack(2, b"abcdefghijklmnopqrstuvwxyz0123", 8192, 0, &mut ev),
        1
    );
    assert_text(&ev[0].text, "abcdefghijklmnopqrstuvw");
    assert_eq!(ev[0].text.len(), EVENT_TEXT_MAX);
    // every task index has its own flag
    for task in 3..ResourceMonitor::MAX_TASKS {
        if task != 7 {
            assert_eq!(m.on_stack(task, b"t", 8192, 0, &mut ev), 1, "{task}");
        }
    }
}

#[test]
fn heap_guard_below_12_kib_on_every_sample_for_60_s_asks_once() {
    let mut g = HeapGuard::default();
    assert_eq!(first_restart(&mut g, 12_287, false, 600_000, 659_000), 0);
    assert!(g.on_sample(12_287, false, 660_000));
    // Still low: the same low period never asks again.
    assert_eq!(first_restart(&mut g, 0, false, 661_000, 900_000), 0);
}

#[test]
fn heap_guard_12_kib_free_is_not_low() {
    let mut g = HeapGuard::default();
    assert_eq!(first_restart(&mut g, 12_288, false, 600_000, 700_000), 0);
    assert_eq!(
        first_restart(&mut g, 12_287, false, 701_000, 800_000),
        761_000
    );
}

#[test]
fn heap_guard_exactly_60_s_after_the_first_low_sample_whatever_the_sample_spacing() {
    let mut g = HeapGuard::default();
    assert!(!g.on_sample(100, false, 700_000));
    assert!(!g.on_sample(100, false, 759_999));
    assert!(g.on_sample(100, false, 760_000));
}

#[test]
fn heap_guard_one_sample_at_or_above_the_threshold_starts_the_60_s_again() {
    let mut g = HeapGuard::default();
    assert_eq!(first_restart(&mut g, 5000, false, 600_000, 659_000), 0);
    assert!(!g.on_sample(12_288, false, 659_500));
    assert_eq!(first_restart(&mut g, 5000, false, 660_000, 719_000), 0);
    assert!(g.on_sample(5000, false, 720_000));
}

#[test]
fn heap_guard_not_armed_during_the_first_10_minutes() {
    let mut g = HeapGuard::default();
    // Low from boot: nothing before 10 min, and the 60 s start there.
    assert_eq!(first_restart(&mut g, 1000, false, 0, 599_000), 0);
    assert!(!g.on_sample(1000, false, 599_999));
    assert_eq!(first_restart(&mut g, 1000, false, 600_000, 659_000), 0);
    assert!(g.on_sample(1000, false, 660_000));
}

#[test]
fn heap_guard_stays_armed_when_millis_wraps() {
    let mut g = HeapGuard::default();
    assert!(!g.on_sample(50_000, false, 600_000));
    assert!(!g.on_sample(1000, false, 0xFFFF_FFFF - 29_999)); // low from 2^32 - 30 s
    assert!(!g.on_sample(1000, false, 29_999));
    assert!(g.on_sample(1000, false, 30_000));
}

#[test]
fn heap_guard_blocked_by_an_esp_upload_or_an_stm_flash_the_60_s_start_again_after_it() {
    let mut g = HeapGuard::default();
    assert_eq!(first_restart(&mut g, 1000, false, 600_000, 630_000), 0);
    assert_eq!(first_restart(&mut g, 1000, true, 631_000, 700_000), 0);
    assert_eq!(first_restart(&mut g, 1000, false, 701_000, 760_000), 0);
    assert!(g.on_sample(1000, false, 761_000));
    // Blocked exactly when it would ask.
    let mut h = HeapGuard::default();
    assert_eq!(first_restart(&mut h, 1000, false, 600_000, 659_000), 0);
    assert!(!h.on_sample(1000, true, 660_000));
    assert_eq!(
        first_restart(&mut h, 1000, false, 661_000, 800_000),
        721_000
    );
}

#[test]
fn heap_guard_a_new_low_period_after_a_recovery_or_a_block_asks_again() {
    let mut g = HeapGuard::default();
    assert_eq!(
        first_restart(&mut g, 1000, false, 600_000, 700_000),
        660_000
    );
    assert!(!g.on_sample(20_000, false, 701_000));
    assert_eq!(
        first_restart(&mut g, 1000, false, 702_000, 800_000),
        762_000
    );
    // The restart it asked for is pending (blocked), then it did not happen.
    assert!(!g.on_sample(1000, true, 801_000));
    assert_eq!(
        first_restart(&mut g, 1000, false, 802_000, 900_000),
        862_000
    );
}

#[test]
fn http_status_ok_cases() {
    let ok = |s: &str| http_status_ok(s.as_bytes());
    assert!(ok("HTTP/1.1 200 OK\r\n"));
    assert!(http_status_ok(&b"HTTP/1.0 200"[..12]));
    assert!(ok("HTTP/1.1 200\r"));
    assert!(ok("HTTP/1.0 200 "));
    assert!(!ok("HTTP/1.1 2000"));
    assert!(!ok("HTTP/1.1 503 Service"));
    assert!(!ok("HTTP/1.1 20"));
    assert!(!ok("http/1.1 200"));
    assert!(!ok("HTTP/1.2 200"));
    assert!(!ok("HTTP/2.1 200"));
    assert!(!ok("HTTP/1.1 201"));
    assert!(!ok("HTTP/1.1_200"));
    assert!(!ok("HTTP/1.1 200\n"));
    assert!(!http_status_ok(&b"HTTP/1.1 200 OK"[..11]));
    // C++ httpStatusOk(nullptr, 12): no Rust form; the empty line
    assert!(!http_status_ok(b""));
    // the C++ compares bytes with memcmp: a NUL is a byte like any other
    assert!(!http_status_ok(b"HTTP/1.1 200\0"));
}

#[test]
fn is_abnormal_reset_cases() {
    for r in -1..12 {
        assert_eq!(
            is_abnormal_reset(r),
            r == 4 || r == 5 || r == 6 || r == 7 || r == 9,
            "{r}"
        );
    }
    assert!(!is_abnormal_reset(i32::MIN));
    assert!(!is_abnormal_reset(i32::MAX));
}
