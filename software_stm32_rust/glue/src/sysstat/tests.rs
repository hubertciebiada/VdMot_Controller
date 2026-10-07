// The glue part of test/native/glue/test_sysstat.cpp (S9): the state before the capture, the
// uptime across the wrap of millis() and the safe mode across watchdog resets. The cases of the
// capture itself (boot reason from RCC_CSR, the reset counter in .noinit) belong to vdm-stm-boot
// `capture`; here each boot runs the core reset guard the way the capture does, and the guard
// cell survives the reboots in the store the firmware writes.

use super::*;
use crate::test_support::fake_board::FakeBoard;
use vdm_stm_core::reset_guard::reset_guard_on_boot;

/// The no-init cell across the boots of one case.
#[derive(Default)]
struct Store {
    cell: ResetGuardCell,
    writes: u32,
}

impl SysstatEnv for Store {
    fn store_guard_cell(&mut self, cell: &ResetGuardCell) {
        self.cell = *cell;
        self.writes += 1;
    }
}

/// The boot stage of a start for this reason: the reset guard on the kept cell (a power-on
/// finds random content), then the module of the main loop.
fn boot(store: &mut Store, reason: BootReason) -> Sysstat {
    if reason == BootReason::PowerOn {
        store.cell = ResetGuardCell {
            magic: 0xA5A5_A5A5,
            count: 0xA5,
            safe: 0xA5,
            pad: 0xA5A5,
            window_s: 0xA5A5_A5A5,
            last_uptime_s: 0xA5A5_A5A5,
            crc: 0xA5A5,
        };
    }
    let mut cell = store.cell;
    let safe = reset_guard_on_boot(&mut cell, reason);
    store.cell = cell;
    Sysstat::new(reason, 0, safe, cell)
}

#[test]
fn before_the_capture_the_reason_is_unknown_and_nothing_is_counted() {
    let s = Sysstat::default();
    assert_eq!(s.boot_reason(), BootReason::Unknown);
    assert_eq!(s.resets(), 0);
    assert_eq!(s.uptime_s(), 0);
    assert!(!s.safe_mode());
    assert_eq!(s.wdg_resets(), 0);
}

#[test]
fn the_uptime_counts_seconds_across_the_wrap_of_millis() {
    let mut s = Sysstat::default();
    let mut store = Store::default();
    let clock = FakeBoard::new();
    s.loop_(&clock, &mut store);
    clock.advance_ms(999);
    s.loop_(&clock, &mut store);
    assert_eq!(s.uptime_s(), 0);
    clock.advance_ms(1);
    s.loop_(&clock, &mut store);
    assert_eq!(s.uptime_s(), 1);
    clock.set_now_us((1u64 << 31) * 1000);
    s.loop_(&clock, &mut store);
    assert_eq!(s.uptime_s(), 2_147_483);
    clock.set_now_us(((1u64 << 32) - 500) * 1000);
    s.loop_(&clock, &mut store);
    clock.advance_ms(1500);
    s.loop_(&clock, &mut store);
    assert_eq!(clock.millis(), 1000);
    assert_eq!(s.uptime_s(), 4_294_968);
    // the guard cell got the uptime of each new second
    assert_eq!(store.cell.last_uptime_s, 4_294_968);
    assert_eq!(store.writes, 4);
}

#[test]
fn s9_three_watchdog_resets_within_10_min_enter_safe_mode_30_min_of_uptime_leave_it() {
    let mut store = Store::default();
    let reasons = [
        BootReason::PowerOn,
        BootReason::IndependentWatchdog,
        BootReason::IndependentWatchdog,
        BootReason::IndependentWatchdog,
    ];
    for (b, &reason) in reasons.iter().enumerate() {
        let mut s = boot(&mut store, reason);
        let clock = FakeBoard::new();
        assert_eq!(usize::from(s.wdg_resets()), b, "boot {b}");
        assert_eq!(s.safe_mode(), b == 3, "boot {b}");
        s.loop_(&clock, &mut store);
        clock.advance_ms(100_000);
        s.loop_(&clock, &mut store);
        if b < 3 {
            continue;
        }
        assert!(s.safe_mode());
        clock.advance_ms(1_699_000);
        s.loop_(&clock, &mut store);
        assert_eq!(s.uptime_s(), 1799);
        assert!(s.safe_mode());
        clock.advance_ms(1000);
        s.loop_(&clock, &mut store);
        assert!(!s.safe_mode());
        assert_eq!(s.wdg_resets(), 0);
    }
}

#[test]
fn s9_ssafe_0_leaves_safe_mode_and_clears_the_window_a_power_on_clears_it_too() {
    let mut store = Store::default();
    let mut reason = BootReason::PowerOn;
    for b in 0..6 {
        let mut s = boot(&mut store, reason);
        if b < 3 {
            reason = BootReason::IndependentWatchdog;
            continue;
        }
        if b == 3 {
            assert!(s.safe_mode());
            s.leave_safe_mode(&mut store);
            assert!(!s.safe_mode());
            assert_eq!(s.wdg_resets(), 0);
            reason = BootReason::IndependentWatchdog;
            continue;
        }
        if b == 4 {
            // a new window: one reset counted
            assert!(!s.safe_mode());
            assert_eq!(s.wdg_resets(), 1);
            reason = BootReason::PowerOn;
            continue;
        }
        assert!(!s.safe_mode());
        assert_eq!(s.wdg_resets(), 0);
        assert_eq!(s.boot_reason(), BootReason::PowerOn);
    }
}

#[test]
fn s9_watchdog_resets_400_s_apart_never_enter_safe_mode() {
    let mut store = Store::default();
    let counts = [0u8, 1, 2, 1, 2];
    let mut reason = BootReason::PowerOn;
    for (b, &count) in counts.iter().enumerate() {
        let mut s = boot(&mut store, reason);
        let clock = FakeBoard::new();
        assert_eq!(s.wdg_resets(), count, "boot {b}");
        assert!(!s.safe_mode());
        s.loop_(&clock, &mut store);
        clock.advance_ms(400_000);
        s.loop_(&clock, &mut store);
        reason = BootReason::IndependentWatchdog;
    }
}

#[test]
fn the_capture_values_are_reported_as_given() {
    let cell = ResetGuardCell {
        count: 2,
        ..ResetGuardCell::default()
    };
    let s = Sysstat::new(BootReason::Software, 17, true, cell);
    assert_eq!(s.boot_reason(), BootReason::Software);
    assert_eq!(s.resets(), 17);
    assert!(s.safe_mode());
    assert_eq!(s.wdg_resets(), 2);
    assert_eq!(s.uptime_s(), 0);
}
