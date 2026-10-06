//! Port of test/native/test_reset_guard.cpp. The C++ reads the raw bytes of the cell
//! (memset, reinterpret_cast for the CRC); here the little-endian bytes of the C++ layout
//! come from `cell_bytes` and `cell_from_bytes`, placed by `offset_of!` where the fields lie
//! in memory.

use super::*;
use core::mem::{offset_of, size_of};

const CELL_SIZE: usize = size_of::<ResetGuardCell>();
/// offsetof(ResetGuardCell, crc): the bytes the CRC covers
const CRC_OFFSET: usize = offset_of!(ResetGuardCell, crc);

fn put(b: &mut [u8], at: usize, v: &[u8]) {
    b[at..at + v.len()].copy_from_slice(v);
}

fn get<const N: usize>(b: &[u8], at: usize) -> [u8; N] {
    let mut v = [0u8; N];
    v.copy_from_slice(&b[at..at + N]);
    v
}

/// The cell as the STM32 holds it in RAM (padding bytes 0).
fn cell_bytes(c: &ResetGuardCell) -> [u8; CELL_SIZE] {
    let mut b = [0u8; CELL_SIZE];
    put(
        &mut b,
        offset_of!(ResetGuardCell, magic),
        &c.magic.to_le_bytes(),
    );
    put(&mut b, offset_of!(ResetGuardCell, count), &[c.count]);
    put(&mut b, offset_of!(ResetGuardCell, safe), &[c.safe]);
    put(
        &mut b,
        offset_of!(ResetGuardCell, pad),
        &c.pad.to_le_bytes(),
    );
    put(
        &mut b,
        offset_of!(ResetGuardCell, window_s),
        &c.window_s.to_le_bytes(),
    );
    put(
        &mut b,
        offset_of!(ResetGuardCell, last_uptime_s),
        &c.last_uptime_s.to_le_bytes(),
    );
    put(&mut b, CRC_OFFSET, &c.crc.to_le_bytes());
    b
}

/// The cell found in RAM holding these bytes.
fn cell_from_bytes(b: &[u8; CELL_SIZE]) -> ResetGuardCell {
    ResetGuardCell {
        magic: u32::from_le_bytes(get(b, offset_of!(ResetGuardCell, magic))),
        count: b[offset_of!(ResetGuardCell, count)],
        safe: b[offset_of!(ResetGuardCell, safe)],
        pad: u16::from_le_bytes(get(b, offset_of!(ResetGuardCell, pad))),
        window_s: u32::from_le_bytes(get(b, offset_of!(ResetGuardCell, window_s))),
        last_uptime_s: u32::from_le_bytes(get(b, offset_of!(ResetGuardCell, last_uptime_s))),
        crc: u16::from_le_bytes(get(b, CRC_OFFSET)),
    }
}

/// C++ crc16Ccitt(reinterpret_cast<const uint8_t*>(&c), offsetof(ResetGuardCell, crc))
fn raw_crc(c: &ResetGuardCell) -> u16 {
    crc16_ccitt(&cell_bytes(c)[..CRC_OFFSET])
}

/// C++ memset(&c, 0xA5, sizeof c)
fn garbage() -> ResetGuardCell {
    cell_from_bytes(&[0xA5; CELL_SIZE])
}

/// one boot with the reason, then alive with the uptime; returns safe after the boot
fn boot(c: &mut ResetGuardCell, r: BootReason, uptime: u32) -> bool {
    let safe = reset_guard_on_boot(c, r);
    reset_guard_alive(c, uptime);
    safe
}

fn sealed(c: &ResetGuardCell) -> bool {
    c.magic == RESET_GUARD_MAGIC && c.crc == raw_crc(c)
}

#[test]
fn constants() {
    assert_eq!(SAFE_MODE_RESETS, 3);
    assert_eq!(SAFE_MODE_WINDOW_S, 600);
    assert_eq!(SAFE_MODE_EXIT_S, 1800);
    assert_eq!(RESET_GUARD_MAGIC, 0x5644_5247);
    assert_eq!(offset_of!(ResetGuardCell, crc), 16);
}

#[test]
fn power_on_brown_out_and_unknown_clear_the_cell() {
    let cold = [
        BootReason::PowerOn,
        BootReason::BrownOut,
        BootReason::Unknown,
    ];
    for r in cold {
        let mut c = garbage();
        assert!(!reset_guard_on_boot(&mut c, r), "{r:?}");
        assert_eq!(c.count, 0, "{r:?}");
        assert_eq!(c.safe, 0, "{r:?}");
        assert_eq!(c.window_s, 0, "{r:?}");
        assert_eq!(c.last_uptime_s, 0, "{r:?}");
        assert_eq!(c.pad, 0, "{r:?}");
        assert!(sealed(&c), "{r:?}");
    }
    let mut c = garbage();
    boot(&mut c, BootReason::PowerOn, 50);
    boot(&mut c, BootReason::IndependentWatchdog, 50);
    boot(&mut c, BootReason::IndependentWatchdog, 50);
    assert_eq!(c.count, 2);
    assert!(!boot(&mut c, BootReason::PowerOn, 50));
    assert_eq!(c.count, 0);
}

#[test]
fn three_watchdog_boots_100_s_apart_enter_safe_mode_on_the_third() {
    let mut c = garbage();
    assert!(!boot(&mut c, BootReason::PowerOn, 100));
    assert!(!boot(&mut c, BootReason::IndependentWatchdog, 100));
    assert_eq!(c.count, 1);
    assert_eq!(c.window_s, 0);
    assert!(!boot(&mut c, BootReason::IndependentWatchdog, 100));
    assert_eq!(c.count, 2);
    assert_eq!(c.window_s, 100);
    assert!(boot(&mut c, BootReason::WindowWatchdog, 100));
    assert_eq!(c.count, 3);
    assert_eq!(c.window_s, 200);
    assert_eq!(c.safe, 1);
    // stays safe across a pin reset
    assert!(boot(&mut c, BootReason::Pin, 100));
    assert!(sealed(&c));
}

#[test]
fn watchdog_boots_400_s_apart_leave_the_window_and_count_again() {
    let mut c = garbage();
    boot(&mut c, BootReason::PowerOn, 400);
    assert!(!boot(&mut c, BootReason::IndependentWatchdog, 400));
    assert!(!boot(&mut c, BootReason::IndependentWatchdog, 400));
    assert_eq!(c.count, 2);
    assert_eq!(c.window_s, 400);
    assert!(!boot(&mut c, BootReason::IndependentWatchdog, 400));
    assert_eq!(c.count, 1);
    assert_eq!(c.window_s, 0);
}

#[test]
fn the_window_may_reach_exactly_600_s() {
    let mut c = garbage();
    boot(&mut c, BootReason::PowerOn, 0);
    boot(&mut c, BootReason::IndependentWatchdog, 300);
    boot(&mut c, BootReason::IndependentWatchdog, 300);
    assert!(boot(&mut c, BootReason::IndependentWatchdog, 0));
    assert_eq!(c.window_s, 600);
    let mut d = garbage();
    boot(&mut d, BootReason::PowerOn, 0);
    boot(&mut d, BootReason::IndependentWatchdog, 300);
    boot(&mut d, BootReason::IndependentWatchdog, 301);
    assert!(!boot(&mut d, BootReason::IndependentWatchdog, 0));
    assert_eq!(d.count, 1);
}

#[test]
fn a_pin_reset_between_watchdog_resets_keeps_the_count_and_adds_its_uptime() {
    let mut c = garbage();
    boot(&mut c, BootReason::PowerOn, 0);
    boot(&mut c, BootReason::IndependentWatchdog, 100);
    assert!(!boot(&mut c, BootReason::Pin, 150));
    assert_eq!(c.count, 1);
    assert_eq!(c.window_s, 100);
    assert!(!boot(&mut c, BootReason::Software, 50));
    assert_eq!(c.window_s, 250);
    assert!(!boot(&mut c, BootReason::IndependentWatchdog, 10));
    assert_eq!(c.count, 2);
    assert_eq!(c.window_s, 300);
}

#[test]
fn an_invalid_cell_on_a_warm_boot_is_cleared() {
    let mut c = garbage();
    boot(&mut c, BootReason::PowerOn, 0);
    boot(&mut c, BootReason::IndependentWatchdog, 10);
    boot(&mut c, BootReason::IndependentWatchdog, 10);
    c.count ^= 0x40; // CRC no longer matches
    assert!(!reset_guard_on_boot(
        &mut c,
        BootReason::IndependentWatchdog
    ));
    assert_eq!(c.count, 0);
    assert!(sealed(&c));
    let mut m = garbage();
    boot(&mut m, BootReason::PowerOn, 0);
    m.magic = 0x5644_5248;
    m.crc = raw_crc(&m);
    m.count = 2;
    m.crc = raw_crc(&m);
    assert!(!reset_guard_on_boot(
        &mut m,
        BootReason::IndependentWatchdog
    ));
    assert_eq!(m.count, 0);
}

#[test]
fn the_window_sum_saturates() {
    let mut c = garbage();
    boot(&mut c, BootReason::PowerOn, 0);
    boot(&mut c, BootReason::Pin, u32::MAX);
    assert_eq!(c.window_s, 0);
    reset_guard_on_boot(&mut c, BootReason::Pin);
    assert_eq!(c.window_s, u32::MAX);
    reset_guard_alive(&mut c, 5);
    reset_guard_on_boot(&mut c, BootReason::Pin);
    assert_eq!(c.window_s, u32::MAX);
}

#[test]
fn reset_guard_alive_records_the_uptime_and_leaves_safe_mode_after_1800_s() {
    let mut c = garbage();
    boot(&mut c, BootReason::PowerOn, 0);
    boot(&mut c, BootReason::IndependentWatchdog, 1);
    boot(&mut c, BootReason::IndependentWatchdog, 1);
    assert!(reset_guard_on_boot(&mut c, BootReason::IndependentWatchdog));
    assert!(reset_guard_alive(&mut c, 1799));
    assert_eq!(c.last_uptime_s, 1799);
    assert!(sealed(&c));
    assert!(!reset_guard_alive(&mut c, 1800));
    assert_eq!(c.count, 0);
    assert_eq!(c.window_s, 0);
    assert_eq!(c.safe, 0);
    assert_eq!(c.last_uptime_s, 1800);
    // not safe: the uptime does not clear the count
    let mut d = garbage();
    boot(&mut d, BootReason::PowerOn, 0);
    reset_guard_on_boot(&mut d, BootReason::IndependentWatchdog);
    assert!(!reset_guard_alive(&mut d, 5000));
    assert_eq!(d.count, 1);
}

#[test]
fn reset_guard_clear_ssafe_0_leaves_safe_mode_and_clears_the_window() {
    let mut c = garbage();
    boot(&mut c, BootReason::PowerOn, 0);
    boot(&mut c, BootReason::IndependentWatchdog, 1);
    boot(&mut c, BootReason::IndependentWatchdog, 1);
    assert!(reset_guard_on_boot(&mut c, BootReason::IndependentWatchdog));
    reset_guard_alive(&mut c, 20);
    reset_guard_clear(&mut c);
    assert_eq!(c.safe, 0);
    assert_eq!(c.count, 0);
    assert_eq!(c.window_s, 0);
    assert_eq!(c.last_uptime_s, 20);
    assert!(sealed(&c));
    assert!(!reset_guard_on_boot(
        &mut c,
        BootReason::IndependentWatchdog
    ));
    assert_eq!(c.count, 1);
}
