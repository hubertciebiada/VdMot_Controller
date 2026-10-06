//! Port of test/native/test_calib_schedule.cpp: slot keys, window/grace boundaries, weekday
//! mask, minute offset, once per date, reboot, DST spring-forward and fall-back, NTP steps,
//! stale bookings, missing time reporting, next slot, long runs.

use super::*;
use crate::test_support::Rng;

/// Weekday by Sakamoto's method (independent of the implementation).
fn weekday(y: i32, m: i32, d: i32) -> u8 {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    ((y + y / 4 - y / 100 + y / 400 + T[(m - 1) as usize] + d) % 7) as u8
}

/// C++ `at(y, mo, d, h, mi, s)`.
fn at_s(y: i32, mo: i32, d: i32, h: i32, mi: i32, s: i32) -> LocalTime {
    LocalTime {
        valid: true,
        year: y as u16,
        month: mo as u8,
        mday: d as u8,
        wday: weekday(y, mo, d),
        hour: h as u8,
        minute: mi as u8,
        second: s as u8,
        epoch: 0,
    }
}

/// C++ `at(y, mo, d, h, mi)` (s = 0).
fn at(y: i32, mo: i32, d: i32, h: i32, mi: i32) -> LocalTime {
    at_s(y, mo, d, h, mi, 0)
}

/// C++ `cfg(mask, hour, minute)`.
fn cfg_min(mask: u8, hour: u8, minute: u8) -> CalibScheduleConfig {
    CalibScheduleConfig {
        day_mask: mask,
        hour,
        minute,
    }
}

/// C++ `cfg(mask, hour)` (minute = 0).
fn cfg(mask: u8, hour: u8) -> CalibScheduleConfig {
    cfg_min(mask, hour, 0)
}

const SUN: u8 = 1 << 0;
const MON: u8 = 1 << 1;
const WED: u8 = 1 << 3;
const THU: u8 = 1 << 4;
const FRI: u8 = 1 << 5;
const ALL: u8 = 0x7F;
/// uptime well below the no-time report
const UP: u32 = 5000;

fn is_leap_year(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn month_days(y: i32, m: i32) -> i32 {
    const D: [i32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if m == 2 && is_leap_year(y) {
        29
    } else {
        D[(m - 1) as usize]
    }
}

fn next_day(y: &mut i32, m: &mut i32, d: &mut i32) {
    *d += 1;
    if *d > month_days(*y, *m) {
        *d = 1;
        *m += 1;
        if *m > 12 {
            *m = 1;
            *y += 1;
        }
    }
}

fn key_of(y: i32, m: i32, d: i32) -> u32 {
    (y * 10000 + m * 100 + d) as u32
}

#[test]
fn weekday_helper_sanity() {
    assert_eq!(weekday(2026, 9, 23), 3); // Wednesday
    assert_eq!(weekday(2026, 9, 27), 0); // Sunday
    assert_eq!(weekday(2026, 12, 31), 4); // Thursday
    assert_eq!(weekday(2000, 1, 1), 6); // Saturday
}

#[test]
fn slot_key_of_valid_and_invalid_local_times() {
    assert_eq!(calib_slot_key(&at_s(2026, 9, 23, 14, 3, 5)), 20260923);
    assert_eq!(calib_slot_key(&at(2000, 1, 1, 0, 0)), 20000101);
    assert_eq!(calib_slot_key(&at_s(9999, 12, 31, 23, 59, 60)), 99991231);
    assert_eq!(calib_slot_key(&at(2028, 2, 29, 0, 0)), 20280229);
    assert_eq!(calib_slot_key(&at(2000, 2, 29, 0, 0)), 20000229);

    let mut t = at(2026, 9, 23, 0, 0);
    t.valid = false;
    assert_eq!(calib_slot_key(&t), 0);
    t = at(2026, 9, 23, 0, 0);
    t.wday = 4;
    assert_eq!(calib_slot_key(&t), 0);
    t = at(2026, 9, 23, 24, 0);
    assert_eq!(calib_slot_key(&t), 0);
    t = at(2026, 9, 23, 23, 60);
    assert_eq!(calib_slot_key(&t), 0);
    t = at_s(2026, 9, 23, 23, 59, 61);
    assert_eq!(calib_slot_key(&t), 0);

    let mut bad = at(2027, 3, 1, 0, 0);
    bad.month = 2;
    bad.mday = 29; // 2027 is not a leap year
    assert_eq!(calib_slot_key(&bad), 0);
    bad = at(1900, 3, 1, 0, 0);
    bad.month = 2;
    bad.mday = 29;
    assert_eq!(calib_slot_key(&bad), 0);
    bad = at(2026, 4, 30, 0, 0);
    bad.mday = 31;
    assert_eq!(calib_slot_key(&bad), 0);
    bad = at(2026, 1, 1, 0, 0);
    bad.mday = 0;
    assert_eq!(calib_slot_key(&bad), 0);
    bad = at(2026, 1, 1, 0, 0);
    bad.month = 0;
    assert_eq!(calib_slot_key(&bad), 0);
    bad = at(2026, 1, 1, 0, 0);
    bad.month = 13;
    assert_eq!(calib_slot_key(&bad), 0);
    bad = at(2026, 1, 1, 0, 0);
    bad.year = 1999;
    assert_eq!(calib_slot_key(&bad), 0);
    bad = at(2026, 1, 1, 0, 0);
    bad.year = 10000;
    assert_eq!(calib_slot_key(&bad), 0);
    assert_eq!(calib_slot_key(&at(1999, 12, 31, 0, 0)), 0);
    assert_eq!(calib_slot_key(&at(2026, 1, 31, 0, 0)), 20260131);
    let mut feb = at(2026, 3, 1, 0, 0);
    feb.month = 2;
    feb.mday = 28;
    feb.wday = weekday(2026, 2, 28);
    assert_eq!(calib_slot_key(&feb), 20260228);
}

#[test]
fn last_day_of_every_month_and_century_leap_rules() {
    for y in [2026, 2028, 2100, 2400, 2104] {
        for m in 1..=12 {
            let last = month_days(y, m);
            assert_eq!(
                calib_slot_key(&at(y, m, last, 0, 0)),
                key_of(y, m, last),
                "{y}-{m}"
            );
            let mut over = at(y, m, last, 0, 0);
            over.mday = (last + 1) as u8;
            for w in 0..7u8 {
                over.wday = w;
                assert_eq!(calib_slot_key(&over), 0, "{y}-{m} wday {w}");
            }
        }
    }
    assert_eq!(month_days(2100, 2), 28);
    assert_eq!(month_days(2400, 2), 29);
    // Weekdays across centuries come from the date, not from a cache.
    assert_eq!(calib_slot_key(&at(2100, 3, 1, 0, 0)), 21000301);
    assert_eq!(calib_slot_key(&at(9999, 1, 1, 0, 0)), 99990101);
}

#[test]
fn dates_across_centuries() {
    // Every 13th day from 2000 to 2800: key, weekday check and next-day arithmetic agree with
    // an independent calendar.
    let (mut y, mut m, mut d) = (2000, 1, 1);
    let mut count = 0;
    while y < 2800 {
        for _ in 0..13 {
            next_day(&mut y, &mut m, &mut d);
        }
        let (mut ny, mut nm, mut nd) = (y, m, d);
        next_day(&mut ny, &mut nm, &mut nd);
        let s = CalibScheduler::default();
        let t = at(y, m, d, 12, 0);
        assert_eq!(calib_slot_key(&t), key_of(y, m, d));
        assert_eq!(
            s.next_slot(&cfg(ALL, 3), &t),
            key_of(ny, nm, nd),
            "{y}-{m}-{d}"
        );
        let wd = weekday(y, m, d);
        assert_ne!(s.next_slot(&cfg(1 << wd, 3), &t), 0, "{y}-{m}-{d}");
        count += 1;
    }
    assert!(count > 20000);
}

#[test]
fn every_day_around_400_year_era_boundaries() {
    // The civil-date arithmetic has its edge cases at the end of each 400-year era (Feb 29 of
    // 2000/2400) and around century years.
    let spans = [(2000, 2001), (2099, 2101), (2399, 2401)];
    for (from, to) in spans {
        let (mut y, mut m, mut d) = (from, 1, 1);
        while y < to + 1 {
            let (mut ny, mut nm, mut nd) = (y, m, d);
            next_day(&mut ny, &mut nm, &mut nd);
            let s = CalibScheduler::default();
            assert_eq!(
                s.next_slot(&cfg(ALL, 3), &at(y, m, d, 12, 0)),
                key_of(ny, nm, nd),
                "{y}-{m}-{d}"
            );
            (y, m, d) = (ny, nm, nd);
        }
    }
}

#[test]
fn impossible_dates_are_rejected_for_every_weekday_value() {
    let bad = [
        (10000, 1, 1),
        (1999, 12, 31),
        (2026, 13, 1),
        (2026, 0, 1),
        (2026, 1, 0),
        (2026, 1, 32),
        (2100, 2, 29),
        (2026, 2, 29),
        (2026, 4, 31),
    ];
    for (y, m, d) in bad {
        let mut t = at(2026, 1, 1, 0, 0);
        t.year = y as u16;
        t.month = m as u8;
        t.mday = d as u8;
        for w in 0..7u8 {
            t.wday = w;
            assert_eq!(calib_slot_key(&t), 0, "{y}-{m}-{d} wday {w}");
        }
        let mut s = CalibScheduler::default();
        s.restore_last_slot(key_of(y, m, d));
        assert_eq!(s.last_slot(), 0, "{y}-{m}-{d}");
    }
    assert_ne!(calib_slot_key(&at(9999, 12, 31, 0, 0)), 0);
}

#[test]
fn fires_once_in_the_window_on_a_selected_day() {
    let mut s = CalibScheduler::default();
    let c = cfg_min(SUN | WED, 3, 15);
    assert_eq!(s.last_slot(), 0);
    assert_eq!(
        s.evaluate(&c, &at_s(2026, 9, 23, 3, 14, 59), UP),
        CalibDecision::None
    );
    assert_eq!(
        s.evaluate(&c, &at_s(2026, 9, 23, 3, 15, 0), UP),
        CalibDecision::Fire
    );
    assert!(s.on_result(true, UP));
    assert_eq!(s.late_minutes(), 0);
    assert_eq!(s.last_slot(), 20260923);
    assert_eq!(
        s.evaluate(&c, &at_s(2026, 9, 23, 3, 15, 10), UP),
        CalibDecision::None
    );
    assert_eq!(s.late_minutes(), 0);
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 23, 5, 14), UP),
        CalibDecision::None
    );
    // Thursday is not selected.
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 24, 3, 15), UP),
        CalibDecision::None
    );
    assert_eq!(s.last_slot(), 20260923);
    // Sunday is.
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 27, 3, 16), UP),
        CalibDecision::Fire
    );
    assert!(s.on_result(true, UP));
    assert_eq!(s.late_minutes(), 1);
    assert_eq!(s.last_slot(), 20260927);
}

#[test]
fn grace_window_boundaries() {
    let c = cfg_min(ALL, 3, 15);
    {
        let mut s = CalibScheduler::default();
        assert_eq!(
            s.evaluate(&c, &at_s(2026, 9, 23, 5, 14, 59), UP),
            CalibDecision::Fire
        ); // +119
        assert!(s.on_result(true, UP));
        assert_eq!(s.late_minutes(), 119);
    }
    {
        let mut s = CalibScheduler::default();
        assert_eq!(
            s.evaluate(&c, &at(2026, 9, 23, 5, 15), UP),
            CalibDecision::None
        ); // +120
        assert_eq!(s.last_slot(), 0);
    }
    {
        let mut s = CalibScheduler::new(10, 3_600_000);
        assert_eq!(
            s.evaluate(&c, &at(2026, 9, 23, 3, 25), UP),
            CalibDecision::None
        );
        assert_eq!(
            s.evaluate(&c, &at(2026, 9, 23, 3, 24), UP),
            CalibDecision::Fire
        );
        assert!(s.on_result(true, UP));
        assert_eq!(s.late_minutes(), 9);
    }
    {
        let mut s = CalibScheduler::new(0, 3_600_000); // treated as 1 minute
        assert_eq!(
            s.evaluate(&c, &at(2026, 9, 23, 3, 16), UP),
            CalibDecision::None
        );
        assert_eq!(
            s.evaluate(&c, &at(2026, 9, 23, 3, 14), UP),
            CalibDecision::None
        );
        assert_eq!(
            s.evaluate(&c, &at_s(2026, 9, 23, 3, 15, 59), UP),
            CalibDecision::Fire
        );
        assert!(s.on_result(true, UP));
    }
    {
        let mut s = CalibScheduler::new(1440, 3_600_000);
        assert_eq!(
            s.evaluate(&c, &at(2026, 9, 23, 23, 59), UP),
            CalibDecision::Fire
        );
        assert!(s.on_result(true, UP));
        assert_eq!(s.late_minutes(), 20 * 60 + 44);
    }
    {
        // The window never crosses midnight into the next date.
        let mut s = CalibScheduler::default();
        let late = cfg_min(WED, 23, 30);
        assert_eq!(
            s.evaluate(&late, &at(2026, 9, 24, 0, 10), UP),
            CalibDecision::None
        );
        assert_eq!(
            s.evaluate(&late, &at(2026, 9, 23, 23, 59), UP),
            CalibDecision::Fire
        );
        assert!(s.on_result(true, UP));
        assert_eq!(s.late_minutes(), 29);
    }
}

#[test]
fn every_weekday_bit_maps_to_tm_wday() {
    // 2026-09-27 is a Sunday; the next six days cover Monday..Saturday.
    for bit in 0..7u8 {
        let mut s = CalibScheduler::default();
        let c = cfg(1 << bit, 0);
        let mut fired = 0;
        let (mut y, mut m, mut d) = (2026, 9, 27);
        for _ in 0..7 {
            if s.evaluate(&c, &at(y, m, d, 0, 0), UP) == CalibDecision::Fire {
                fired += 1;
                assert_eq!(weekday(y, m, d), bit);
            }
            next_day(&mut y, &mut m, &mut d);
        }
        assert_eq!(fired, 1, "bit {bit}");
    }
}

#[test]
fn disabled_schedules_never_fire() {
    let off = [
        cfg(0, 3),
        cfg(0x80, 3),
        cfg(ALL, 24),
        cfg_min(ALL, 3, 60),
        cfg_min(ALL, 255, 255),
    ];
    for c in &off {
        let mut s = CalibScheduler::default();
        for h in 0..24 {
            assert_eq!(
                s.evaluate(c, &at(2026, 9, 23, h, 0), UP),
                CalibDecision::None
            );
        }
        let none = LocalTime::default();
        assert_eq!(s.evaluate(c, &none, 7_200_000), CalibDecision::None); // no report either
    }
    // Bit 7 is ignored, the other bits still count.
    let mut s = CalibScheduler::default();
    assert_eq!(
        s.evaluate(&cfg(0x80 | WED, 1), &at(2026, 9, 23, 1, 0), UP),
        CalibDecision::Fire
    );
    assert!(s.on_result(true, UP));
    // Boundaries that are still enabled.
    let mut s2 = CalibScheduler::default();
    assert_eq!(
        s2.evaluate(&cfg_min(ALL, 23, 59), &at(2026, 9, 23, 23, 59), UP),
        CalibDecision::Fire
    );
    assert!(s2.on_result(true, UP));
}

#[test]
fn reboot_inside_the_window_does_not_fire_again() {
    let c = cfg(WED, 3);
    let mut a = CalibScheduler::default();
    assert_eq!(
        a.evaluate(&c, &at(2026, 9, 23, 3, 1), UP),
        CalibDecision::Fire
    );
    assert!(a.on_result(true, UP));
    let mut b = CalibScheduler::default(); // after reboot
    b.restore_last_slot(a.last_slot());
    assert_eq!(b.last_slot(), 20260923);
    assert_eq!(
        b.evaluate(&c, &at(2026, 9, 23, 3, 30), UP),
        CalibDecision::None
    );
    assert_eq!(
        b.evaluate(&c, &at(2026, 9, 30, 3, 30), UP),
        CalibDecision::Fire
    );
    assert!(b.on_result(true, UP));
}

#[test]
fn restoring_an_invalid_key_is_ignored() {
    let bad: [u32; 9] = [
        0,
        1,
        20261301,
        20260001,
        20260230,
        20260100,
        99999999,
        0xFFFF_FFFF,
        19991231,
    ];
    for k in bad {
        let mut s = CalibScheduler::default();
        s.restore_last_slot(20260101);
        s.restore_last_slot(k);
        assert_eq!(s.last_slot(), 0, "{k}");
    }
    let mut s = CalibScheduler::default();
    s.restore_last_slot(20280229);
    assert_eq!(s.last_slot(), 20280229);
    s.restore_last_slot(99991231);
    assert_eq!(s.last_slot(), 99991231);
}

#[test]
fn dst_spring_forward_fires_after_the_gap_fall_back_fires_once() {
    // EU 2026: 29 March 02:00 -> 03:00 (Sunday), 25 October 03:00 -> 02:00 (Sunday).
    let c = cfg_min(SUN, 2, 30);
    let mut s = CalibScheduler::default();
    assert_eq!(
        s.evaluate(&c, &at_s(2026, 3, 29, 1, 59, 50), UP),
        CalibDecision::None
    );
    assert_eq!(
        s.evaluate(&c, &at_s(2026, 3, 29, 3, 0, 0), UP),
        CalibDecision::Fire
    );
    assert!(s.on_result(true, UP));
    assert_eq!(s.late_minutes(), 30);

    let mut f = CalibScheduler::default();
    assert_eq!(
        f.evaluate(&c, &at(2026, 10, 25, 2, 30), UP),
        CalibDecision::Fire
    );
    assert!(f.on_result(true, UP));
    assert_eq!(
        f.evaluate(&c, &at(2026, 10, 25, 2, 59), UP),
        CalibDecision::None
    );
    // Clock goes back to 02:00 and passes 02:30 a second time.
    for m in 0..60 {
        assert_eq!(
            f.evaluate(&c, &at(2026, 10, 25, 2, m), UP),
            CalibDecision::None
        );
    }
    assert_eq!(f.last_slot(), 20261025);
}

// C++ "calib: NTP steps and stale bookings": one test per SUBCASE.

#[test]
fn ntp_a_step_back_to_an_earlier_date_never_refires_a_booked_date() {
    let c = cfg(ALL, 3);
    let mut s = CalibScheduler::default();
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 23, 3, 0), UP),
        CalibDecision::Fire
    );
    assert!(s.on_result(true, UP));
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 22, 3, 0), UP),
        CalibDecision::None
    ); // 1 day back
    assert_eq!(s.last_slot(), 20260923);
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 21, 3, 0), UP),
        CalibDecision::None
    ); // 2 days back
    assert_eq!(s.last_slot(), 20260923);
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 24, 3, 0), UP),
        CalibDecision::Fire
    );
    assert!(s.on_result(true, UP));
}

#[test]
fn ntp_a_booking_more_than_2_days_in_the_future_is_discarded() {
    let c = cfg(ALL, 3);
    let mut s = CalibScheduler::default();
    s.restore_last_slot(20260930); // written while the clock was wrong
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 27, 3, 0), UP),
        CalibDecision::Fire
    );
    assert!(s.on_result(true, UP));
    assert_eq!(s.last_slot(), 20260927);
}

#[test]
fn ntp_exactly_2_days_ahead_is_kept() {
    let c = cfg(ALL, 3);
    let mut s = CalibScheduler::default();
    s.restore_last_slot(20260930);
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 28, 3, 0), UP),
        CalibDecision::None
    );
    assert_eq!(s.last_slot(), 20260930);
}

#[test]
fn ntp_discarding_happens_outside_the_window_too_and_across_months_and_years() {
    let c = cfg(ALL, 3);
    let mut s = CalibScheduler::default();
    s.restore_last_slot(20270102);
    assert_eq!(
        s.evaluate(&c, &at(2026, 12, 31, 12, 0), UP),
        CalibDecision::None
    );
    assert_eq!(s.last_slot(), 20270102); // 2 days: kept
    s.restore_last_slot(20270103);
    assert_eq!(
        s.evaluate(&c, &at(2026, 12, 31, 12, 0), UP),
        CalibDecision::None
    );
    assert_eq!(s.last_slot(), 0); // 3 days: discarded
                                  // A far-future booking is discarded even with the schedule off.
    s.restore_last_slot(20990101);
    assert_eq!(
        s.evaluate(&cfg(0, 3), &at(2026, 12, 31, 12, 0), UP),
        CalibDecision::None
    );
    assert_eq!(s.last_slot(), 0);
}

#[test]
fn ntp_a_step_forward_past_the_window_skips_that_date() {
    let c = cfg(ALL, 3);
    let mut s = CalibScheduler::default();
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 23, 2, 59), UP),
        CalibDecision::None
    );
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 23, 5, 0), UP),
        CalibDecision::None
    );
    assert_eq!(s.last_slot(), 0);
}

#[test]
fn missing_time_is_reported_once_per_boot_after_the_delay() {
    let mut s = CalibScheduler::new(120, 3_600_000);
    let c = cfg(ALL, 3);
    let none = LocalTime::default();
    assert_eq!(s.evaluate(&c, &none, 0), CalibDecision::None);
    assert_eq!(s.evaluate(&c, &none, 3_600_000), CalibDecision::None);
    assert_eq!(
        s.evaluate(&c, &none, 3_600_001),
        CalibDecision::SkippedNoTime
    );
    assert_eq!(s.evaluate(&c, &none, 3_600_002), CalibDecision::None);
    assert_eq!(s.evaluate(&c, &none, 99_999_999), CalibDecision::None);
    // Garbage fields count as no time.
    let mut garbage = at(2026, 9, 23, 3, 0);
    garbage.wday = 6;
    assert_eq!(s.evaluate(&c, &garbage, 99_999_999), CalibDecision::None);
    assert_eq!(s.last_slot(), 0);
    // Once time is valid the schedule works normally.
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 23, 3, 0), 99_999_999),
        CalibDecision::Fire
    );
    assert!(s.on_result(true, 99_999_999));

    let mut g = CalibScheduler::new(120, 3_600_000);
    assert_eq!(
        g.evaluate(&c, &garbage, 3_600_001),
        CalibDecision::SkippedNoTime
    );

    let mut quick = CalibScheduler::new(120, 0);
    assert_eq!(quick.evaluate(&c, &none, 0), CalibDecision::None);
    assert_eq!(quick.evaluate(&c, &none, 1), CalibDecision::SkippedNoTime);

    // A report is not consumed while the schedule is off.
    let mut off = CalibScheduler::new(120, 10);
    assert_eq!(off.evaluate(&cfg(0, 3), &none, 20), CalibDecision::None);
    assert_eq!(off.evaluate(&c, &none, 20), CalibDecision::SkippedNoTime);
}

#[test]
fn late_minutes_stays_the_one_of_the_last_firing() {
    let mut s = CalibScheduler::default();
    let c = cfg(ALL, 3);
    assert_eq!(s.late_minutes(), 0);
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 23, 4, 5), UP),
        CalibDecision::Fire
    );
    assert_eq!(s.late_minutes(), 65);
    assert!(s.on_result(true, UP));
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 23, 4, 6), UP),
        CalibDecision::None
    );
    assert_eq!(s.late_minutes(), 65);
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 24, 3, 7), UP),
        CalibDecision::Fire
    );
    assert_eq!(s.late_minutes(), 7);
}

#[test]
fn next_slot() {
    let c = cfg(SUN | WED, 0);
    let mut s = CalibScheduler::default();
    // Wednesday 2026-09-23: window 00:00..02:00.
    assert_eq!(s.next_slot(&c, &at(2026, 9, 23, 1, 59)), 20260923);
    assert_eq!(s.next_slot(&c, &at(2026, 9, 23, 2, 0)), 20260927);
    assert_eq!(s.next_slot(&c, &at(2026, 9, 22, 23, 0)), 20260923);
    assert_eq!(
        s.evaluate(&c, &at(2026, 9, 23, 0, 30), UP),
        CalibDecision::Fire
    );
    assert!(s.on_result(true, UP));
    assert_eq!(s.next_slot(&c, &at(2026, 9, 23, 0, 31)), 20260927);
    // Month and year roll-over.
    let t = CalibScheduler::default();
    assert_eq!(t.next_slot(&cfg(THU, 5), &at(2026, 9, 30, 6, 0)), 20261001);
    assert_eq!(t.next_slot(&cfg(FRI, 5), &at(2026, 12, 31, 6, 0)), 20270101);
    assert_eq!(t.next_slot(&cfg(THU, 5), &at(2026, 12, 31, 7, 0)), 20270107); // 7 days ahead
    assert_eq!(
        t.next_slot(&cfg(THU, 5), &at(2026, 12, 31, 6, 59)),
        20261231
    );
    assert_eq!(t.next_slot(&cfg(MON, 5), &at(2028, 2, 28, 8, 0)), 20280306); // leap year
    assert_eq!(t.next_slot(&cfg(MON, 5), &at(2028, 2, 28, 6, 0)), 20280228);
    // Off or no time.
    assert_eq!(t.next_slot(&cfg(0, 5), &at(2026, 12, 31, 6, 0)), 0);
    assert_eq!(t.next_slot(&cfg(ALL, 24), &at(2026, 12, 31, 6, 0)), 0);
    assert_eq!(t.next_slot(&c, &LocalTime::default()), 0);
    // A booking in the (near) future is skipped.
    let mut u = CalibScheduler::default();
    u.restore_last_slot(20260927);
    assert_eq!(u.next_slot(&c, &at(2026, 9, 26, 0, 0)), 20260930);
    // Every date within 7 days booked -> 0.
    let mut v = CalibScheduler::default();
    v.restore_last_slot(20261230); // today .. today+7 booked, today+8 is not looked at
    assert_eq!(v.next_slot(&cfg(ALL, 5), &at(2026, 12, 23, 6, 0)), 0);
    v.restore_last_slot(20261229);
    assert_eq!(v.next_slot(&cfg(ALL, 5), &at(2026, 12, 23, 6, 0)), 20261230);
    // Minutes count: window 05:30..07:30.
    let w = CalibScheduler::default();
    assert_eq!(
        w.next_slot(&cfg_min(ALL, 5, 30), &at(2026, 12, 23, 7, 29)),
        20261223
    );
    assert_eq!(
        w.next_slot(&cfg_min(ALL, 5, 30), &at(2026, 12, 23, 7, 30)),
        20261224
    );
    assert_eq!(
        w.next_slot(&cfg_min(ALL, 5, 30), &at(2026, 12, 23, 5, 29)),
        20261223
    );
}

#[test]
fn fuzz_a_long_run_fires_exactly_once_per_selected_date() {
    // Every minute for 3 years (incl. a leap day), with random reboots.
    let mut rng = Rng::new(8080);
    let c = cfg_min(SUN | WED, 3, 45);
    let mut s = CalibScheduler::default();
    let mut persisted = 0;
    let (mut fires, mut expected, mut skipped) = (0, 0, 0);
    let (mut y, mut m, mut d) = (2026, 1, 1);
    while y < 2029 {
        let wd = weekday(y, m, d);
        if wd == 0 || wd == 3 {
            expected += 1;
        }
        let mut fired_today = 0;
        for minute in 0..1440 {
            if rng.below(5000) == 0 {
                // ESP reboot
                s = CalibScheduler::default();
                s.restore_last_slot(persisted);
            }
            let dec = s.evaluate(&c, &at(y, m, d, minute / 60, minute % 60), UP);
            if dec == CalibDecision::Fire {
                assert!(s.on_result(true, UP));
                fires += 1;
                fired_today += 1;
                persisted = s.last_slot();
                assert!(minute >= 3 * 60 + 45);
                assert!(minute < 3 * 60 + 45 + 120);
                assert_eq!(s.last_slot(), key_of(y, m, d));
            }
            if dec == CalibDecision::SkippedNoTime {
                skipped += 1;
            }
        }
        assert!(fired_today <= 1);
        next_day(&mut y, &mut m, &mut d);
    }
    assert_eq!(skipped, 0);
    assert_eq!(fires, expected);
    assert!(expected > 300);
}
