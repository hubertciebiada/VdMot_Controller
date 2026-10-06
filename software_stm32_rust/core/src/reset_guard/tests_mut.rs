//! Port of test/native/test_reset_guard__mut.cpp.

use super::*;

/// a valid cell after a power-on and two watchdog resets (count 2)
fn two_watchdogs() -> ResetGuardCell {
    // C++ memset(&c, 0xA5, sizeof c): every byte of every field 0xA5
    let mut c = ResetGuardCell {
        magic: 0xA5A5_A5A5,
        count: 0xA5,
        safe: 0xA5,
        pad: 0xA5A5,
        window_s: 0xA5A5_A5A5,
        last_uptime_s: 0xA5A5_A5A5,
        crc: 0xA5A5,
    };
    reset_guard_on_boot(&mut c, BootReason::PowerOn);
    reset_guard_on_boot(&mut c, BootReason::IndependentWatchdog);
    reset_guard_on_boot(&mut c, BootReason::IndependentWatchdog);
    c
}

#[test]
fn brown_out_and_unknown_clear_a_valid_cell() {
    let cold = [
        BootReason::BrownOut,
        BootReason::Unknown,
        BootReason::PowerOn,
    ];
    for r in cold {
        let mut c = two_watchdogs();
        assert_eq!(c.count, 2, "{r:?}");
        reset_guard_alive(&mut c, 30);
        assert!(!reset_guard_on_boot(&mut c, r), "{r:?}");
        assert_eq!(c.count, 0, "{r:?}");
        assert_eq!(c.window_s, 0, "{r:?}");
    }
}

#[test]
fn a_boot_with_no_uptime_keeps_the_window_sum() {
    let mut c = two_watchdogs();
    reset_guard_alive(&mut c, 100);
    reset_guard_on_boot(&mut c, BootReason::Pin);
    assert_eq!(c.window_s, 100);
    reset_guard_on_boot(&mut c, BootReason::Pin); // last_uptime_s 0
    assert_eq!(c.window_s, 100);
    reset_guard_alive(&mut c, u32::MAX - 100);
    reset_guard_on_boot(&mut c, BootReason::Pin);
    assert_eq!(c.window_s, u32::MAX);
}

#[test]
fn the_watchdog_count_saturates_at_255() {
    let mut c = two_watchdogs();
    for _ in 3..=254 {
        reset_guard_on_boot(&mut c, BootReason::IndependentWatchdog);
    }
    assert_eq!(c.count, 254);
    assert!(reset_guard_on_boot(&mut c, BootReason::IndependentWatchdog));
    assert_eq!(c.count, 255);
    for _ in 0..3 {
        assert!(reset_guard_on_boot(&mut c, BootReason::WindowWatchdog));
    }
    assert_eq!(c.count, 255);
    assert_eq!(c.window_s, 0);
}
