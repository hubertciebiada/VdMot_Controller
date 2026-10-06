//! Port of test/native/test_system_stats.cpp.

use super::*;

#[test]
fn uptime_counter_whole_seconds_remainder_carried() {
    let mut u = UptimeCounter::default();
    assert_eq!(u.seconds(), 0);
    u.update(999);
    assert_eq!(u.seconds(), 0);
    u.update(1000);
    assert_eq!(u.seconds(), 1);
    u.update(2500);
    assert_eq!(u.seconds(), 2);
    u.update(2999);
    assert_eq!(u.seconds(), 2);
    u.update(3000);
    assert_eq!(u.seconds(), 3);
    u.update(3000);
    assert_eq!(u.seconds(), 3);
}

#[test]
fn uptime_counter_continues_across_the_49_7_day_wrap_of_millis() {
    let mut u = UptimeCounter::default();
    let mut now: u32 = 0;
    // walk to just before the wrap in steps below the wrap period
    for _ in 0..4 {
        now += 0x3FFF_FFFF;
        u.update(now);
    }
    let before = u.seconds();
    assert_eq!(before, 4_294_967); // 4 * 0x3FFFFFFF ms = 4294967.292 s
    now = now.wrapping_add(10000); // wraps
    u.update(now);
    assert_eq!(u.seconds(), before + 10);
}

#[test]
fn uptime_counter_many_small_steps_do_not_lose_milliseconds() {
    let mut u = UptimeCounter::default();
    let mut now: u32 = 0;
    for _ in 0..100_000 {
        now += 11;
        u.update(now);
    }
    assert_eq!(u.seconds(), 1100);
}

#[test]
fn classify_reset_most_specific_flag_wins() {
    let mut f = ResetFlags::default();
    assert_eq!(classify_reset(&f), BootReason::Unknown);

    f = ResetFlags::default();
    f.pin = true;
    assert_eq!(classify_reset(&f), BootReason::Pin);

    f.power_on = true;
    f.brown_out = true; // set together with power-on
    assert_eq!(classify_reset(&f), BootReason::PowerOn);

    f = ResetFlags::default();
    f.brown_out = true;
    f.pin = true;
    assert_eq!(classify_reset(&f), BootReason::BrownOut);

    f = ResetFlags::default();
    f.software = true;
    f.pin = true;
    assert_eq!(classify_reset(&f), BootReason::Software);

    f.independent_watchdog = true;
    assert_eq!(classify_reset(&f), BootReason::IndependentWatchdog);

    f = ResetFlags::default();
    f.window_watchdog = true;
    f.pin = true;
    assert_eq!(classify_reset(&f), BootReason::WindowWatchdog);

    f = ResetFlags::default();
    f.low_power = true;
    f.software = true;
    assert_eq!(classify_reset(&f), BootReason::LowPower);

    assert_eq!(BootReason::PowerOn as i32, 1);
    assert_eq!(BootReason::BrownOut as i32, 7);
}

#[test]
fn count_reset_counts_warm_resets_restarts_after_power_on() {
    // random RAM after power-up
    let mut cell = ResetCounterCell {
        magic: 0xDEAD_BEEF,
        count: 1234,
        check: 0,
    };
    assert_eq!(count_reset(&mut cell, BootReason::Pin), 0);
    assert_eq!(count_reset(&mut cell, BootReason::Software), 1);
    assert_eq!(count_reset(&mut cell, BootReason::IndependentWatchdog), 2);
    assert_eq!(count_reset(&mut cell, BootReason::Pin), 3);
    assert_eq!(count_reset(&mut cell, BootReason::PowerOn), 0);
    assert_eq!(count_reset(&mut cell, BootReason::Unknown), 1);
    assert_eq!(count_reset(&mut cell, BootReason::BrownOut), 0);
}

#[test]
fn count_reset_damaged_cell_starts_again() {
    let mut cell = ResetCounterCell {
        magic: RESET_COUNTER_MAGIC,
        count: 5,
        check: !5u32,
    };
    assert_eq!(count_reset(&mut cell, BootReason::Pin), 6);
    cell.count = 100; // check word no longer matches
    assert_eq!(count_reset(&mut cell, BootReason::Pin), 0);
    assert_eq!(cell.magic, RESET_COUNTER_MAGIC);
    assert_eq!(cell.check, !cell.count);
}

#[test]
fn count_reset_saturates() {
    let mut cell = ResetCounterCell {
        magic: RESET_COUNTER_MAGIC,
        count: 0xFFFF_FFFF,
        check: 0,
    };
    assert_eq!(count_reset(&mut cell, BootReason::Pin), 0xFFFF_FFFF);
    assert_eq!(count_reset(&mut cell, BootReason::Pin), 0xFFFF_FFFF);
}
