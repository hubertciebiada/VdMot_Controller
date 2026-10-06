// Port of the capture cases of software_stm32/test/native/glue/test_sysstat.cpp (glue_sysstat):
// boot reason from RCC->CSR, reset counter in .noinit across boots, safe mode after a watchdog
// reset loop (S9). The uptime and the end of the safe mode are the glue's sysstat (later).
// Plus the byte layout of the cells: the C++ 2.1.7 offsets, little endian, padding untouched.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used
)]

use super::*;
use vdm_stm_core::legacy_layout::crc16_ccitt;
use vdm_stm_core::reset_guard::{reset_guard_alive, reset_guard_clear, RESET_GUARD_MAGIC};
use vdm_stm_core::system_stats::RESET_COUNTER_MAGIC;

/// RAM content at power-on in the C++ fakes
fn power_on_ram() -> [u8; NOINIT_LEN] {
    [0xA5; NOINIT_LEN]
}

fn all_a5(cells: &[u8]) -> bool {
    !cells.is_empty() && cells.iter().all(|&b| b == 0xA5)
}

/// One boot of the C++ test runner: capture with these flags (and RMVF), then `uptime_s`
/// seconds of sysstat_loop (resetGuardAlive once per second; the last call counts).
fn boot(cells: &mut [u8; NOINIT_LEN], csr: u32, uptime_s: u32) -> ResetInfo {
    let info = capture_reset(csr, cells);
    if uptime_s > 0 {
        let mut g = read_guard(cells);
        reset_guard_alive(&mut g, uptime_s);
        write_guard(cells, &g);
    }
    info
}

const POWER_ON: u32 = CSR_PORRSTF | CSR_PINRSTF | CSR_BORRSTF;
const PIN: u32 = CSR_PINRSTF;
const SOFTWARE: u32 = CSR_SFTRSTF | CSR_PINRSTF;
const WATCHDOG: u32 = CSR_IWDGRSTF | CSR_PINRSTF;

#[test]
fn the_first_boot_is_a_power_on_with_no_resets() {
    let mut cells = power_on_ram();
    let info = capture_reset(POWER_ON, &mut cells);
    assert_eq!(info.reason, BootReason::PowerOn);
    assert_eq!(info.resets, 0);
    assert!(!info.safe_mode);
}

#[test]
fn each_reset_flag_of_rcc_csr_alone_gives_its_boot_reason() {
    let cases = [
        (CSR_LPWRRSTF, BootReason::LowPower),
        (CSR_WWDGRSTF, BootReason::WindowWatchdog),
        (CSR_IWDGRSTF, BootReason::IndependentWatchdog),
        (CSR_SFTRSTF, BootReason::Software),
        (CSR_PORRSTF, BootReason::PowerOn),
        (CSR_PINRSTF, BootReason::Pin),
        (CSR_BORRSTF, BootReason::BrownOut),
        (0, BootReason::Unknown),
    ];
    for (csr, reason) in cases {
        let mut cells = power_on_ram();
        assert_eq!(capture_reset(csr, &mut cells).reason, reason, "{csr:#x}");
    }
}

#[test]
fn the_flags_of_a_watchdog_software_and_pin_reset_together_give_the_most_specific_reason() {
    let cases = [
        (CSR_IWDGRSTF | CSR_PINRSTF, BootReason::IndependentWatchdog),
        (CSR_SFTRSTF | CSR_PINRSTF, BootReason::Software),
        (POWER_ON, BootReason::PowerOn),
        (CSR_BORRSTF | CSR_PINRSTF, BootReason::BrownOut),
        (CSR_LPWRRSTF | CSR_PINRSTF, BootReason::LowPower),
        (CSR_WWDGRSTF | CSR_PINRSTF, BootReason::WindowWatchdog),
    ];
    for (csr, reason) in cases {
        let mut cells = power_on_ram();
        assert_eq!(capture_reset(csr, &mut cells).reason, reason, "{csr:#x}");
    }
}

#[test]
fn other_csr_bits_do_not_change_the_reason() {
    // LSION, LSIRDY and RMVF read back as set in some states
    let mut cells = power_on_ram();
    assert_eq!(
        capture_reset(PIN | 0x0100_0003, &mut cells).reason,
        BootReason::Pin
    );
}

#[test]
fn the_reset_counter_counts_every_warm_reset_and_starts_at_0_after_a_power_on() {
    // boot: 0 power-on, 1 and 2 pin, 3 software, 4 watchdog, 5 power-on
    let flags = [POWER_ON, PIN, PIN, SOFTWARE, WATCHDOG, POWER_ON];
    let reasons = [
        BootReason::PowerOn,
        BootReason::Pin,
        BootReason::Pin,
        BootReason::Software,
        BootReason::IndependentWatchdog,
        BootReason::PowerOn,
    ];
    let resets = [0, 1, 2, 3, 4, 0];
    let mut cells = power_on_ram();
    for i in 0..6 {
        let info = capture_reset(flags[i], &mut cells);
        assert_eq!(info.reason, reasons[i], "boot {i}");
        assert_eq!(info.resets, resets[i], "boot {i}");
    }
}

#[test]
fn the_counter_lives_in_noinit_written_by_the_capture_and_kept_by_a_pin_reset() {
    let mut cells = power_on_ram();
    assert!(all_a5(
        &cells[RESET_CELL_OFFSET..RESET_CELL_OFFSET + RESET_CELL_LEN]
    ));
    capture_reset(POWER_ON, &mut cells);
    assert!(!all_a5(
        &cells[RESET_CELL_OFFSET..RESET_CELL_OFFSET + RESET_CELL_LEN]
    ));
    assert_eq!(capture_reset(PIN, &mut cells).resets, 1);
}

#[test]
fn a_cell_with_random_content_counts_from_0_after_a_warm_reset() {
    let mut cells = power_on_ram();
    let info = capture_reset(PIN, &mut cells);
    assert_eq!(info.reason, BootReason::Pin);
    assert_eq!(info.resets, 0);
}

#[test]
fn s9_three_watchdog_resets_within_10_min_enter_safe_mode() {
    let mut cells = power_on_ram();
    // boot 0 power-on, then watchdog resets after 100 s each
    let mut info = boot(&mut cells, POWER_ON, 100);
    assert_eq!(read_guard(&cells).count, 0);
    assert!(!info.safe_mode);
    for n in 1..=3u8 {
        info = boot(&mut cells, WATCHDOG, 100);
        assert_eq!(read_guard(&cells).count, n);
        assert_eq!(info.safe_mode, n == 3, "boot {n}");
    }
    // a pin reset keeps the safe mode
    assert!(boot(&mut cells, PIN, 0).safe_mode);
}

#[test]
fn s9_ssafe_0_clears_the_window_and_a_power_on_clears_it_too() {
    let mut cells = power_on_ram();
    boot(&mut cells, POWER_ON, 1);
    for _ in 0..3 {
        boot(&mut cells, WATCHDOG, 1);
    }
    assert!(read_guard(&cells).safe == 1);
    let mut g = read_guard(&cells);
    reset_guard_clear(&mut g);
    write_guard(&mut cells, &g);
    // a new window: one reset counted
    let info = boot(&mut cells, WATCHDOG, 1);
    assert!(!info.safe_mode);
    assert_eq!(read_guard(&cells).count, 1);
    let info = boot(&mut cells, POWER_ON, 0);
    assert!(!info.safe_mode);
    assert_eq!(read_guard(&cells).count, 0);
    assert_eq!(info.reason, BootReason::PowerOn);
}

#[test]
fn s9_watchdog_resets_400_s_apart_never_enter_safe_mode() {
    let mut cells = power_on_ram();
    boot(&mut cells, POWER_ON, 400);
    let counts = [1u8, 2, 1, 2];
    for (i, &n) in counts.iter().enumerate() {
        let info = boot(&mut cells, WATCHDOG, 400);
        assert_eq!(read_guard(&cells).count, n, "boot {}", i + 1);
        assert!(!info.safe_mode);
    }
}

#[test]
fn the_cells_have_the_cpp_offsets_and_byte_order() {
    let mut cells = power_on_ram();
    capture_reset(PIN, &mut cells);
    // ResetCounterCell {magic, count, ~count} at 0xC8
    let counter: [u8; 12] = cells[0xC8..0xD4].try_into().unwrap();
    assert_eq!(
        counter,
        [0x4D, 0x52, 0x44, 0x56, 0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF]
    );
    assert_eq!(
        u32::from_le_bytes(counter[..4].try_into().unwrap()),
        RESET_COUNTER_MAGIC
    );
    // ResetGuardCell {magic, count, safe, pad, windowS, lastUptimeS, crc} at 0xB4
    let guard = &cells[0xB4..0xC8];
    assert_eq!(&guard[..4], [0x47, 0x52, 0x44, 0x56]);
    assert_eq!(
        u32::from_le_bytes(guard[..4].try_into().unwrap()),
        RESET_GUARD_MAGIC
    );
    assert_eq!(&guard[4..16], [0; 12]);
    let crc = crc16_ccitt(&guard[..16]);
    assert_eq!(&guard[16..18], crc.to_le_bytes());
    // tail padding and the warm state are not written
    assert_eq!(&guard[18..20], [0xA5, 0xA5]);
    assert!(all_a5(&cells[..WARM_STATE_LEN]));
}

#[test]
fn a_cpp_written_cell_is_read_and_continued() {
    // cells as the C++ 2.1.7 leaves them: counter 7, guard with one watchdog reset 30 s ago
    let mut cells = [0u8; NOINIT_LEN];
    cells[0xC8..0xD4]
        .copy_from_slice(&[0x4D, 0x52, 0x44, 0x56, 7, 0, 0, 0, 0xF8, 0xFF, 0xFF, 0xFF]);
    let mut g = vdm_stm_core::reset_guard::ResetGuardCell {
        magic: RESET_GUARD_MAGIC,
        count: 1,
        safe: 0,
        pad: 0,
        window_s: 0,
        last_uptime_s: 30,
        crc: 0,
    };
    let mut bytes = [0u8; 16];
    bytes[..4].copy_from_slice(&g.magic.to_le_bytes());
    bytes[4] = 1;
    bytes[12..16].copy_from_slice(&30u32.to_le_bytes());
    g.crc = crc16_ccitt(&bytes);
    cells[0xB4..0xC4].copy_from_slice(&bytes);
    cells[0xC4..0xC6].copy_from_slice(&g.crc.to_le_bytes());
    cells[0xC6..0xC8].copy_from_slice(&[0x12, 0x34]);
    assert_eq!(read_guard(&cells), g);
    let info = capture_reset(WATCHDOG, &mut cells);
    assert_eq!(info.resets, 8);
    let after = read_guard(&cells);
    assert_eq!(
        (after.count, after.window_s, after.last_uptime_s),
        (2, 30, 0)
    );
    assert_eq!(&cells[0xC6..0xC8], [0x12, 0x34]);
}

#[test]
fn reset_flags_maps_every_bit() {
    let f = reset_flags(u32::MAX);
    assert!(f.low_power && f.window_watchdog && f.independent_watchdog);
    assert!(f.software && f.power_on && f.pin && f.brown_out);
    assert_eq!(reset_flags(0), ResetFlags::default());
    assert_eq!(
        reset_flags(CSR_SFTRSTF),
        ResetFlags {
            software: true,
            ..ResetFlags::default()
        }
    );
}

#[test]
fn layout_constants_are_the_cpp_2_1_7_addresses() {
    assert_eq!(NOINIT_ADDR, 0x2000_3234);
    assert_eq!(NOINIT_ADDR + GUARD_CELL_OFFSET as u32, 0x2000_32E8);
    assert_eq!(NOINIT_ADDR + RESET_CELL_OFFSET as u32, 0x2000_32FC);
    assert_eq!(WARM_STATE_OFFSET + WARM_STATE_LEN, GUARD_CELL_OFFSET);
    assert_eq!(GUARD_CELL_OFFSET + GUARD_CELL_LEN, RESET_CELL_OFFSET);
    assert_eq!(RESET_CELL_OFFSET + RESET_CELL_LEN, NOINIT_LEN);
    assert_eq!(NOINIT_LEN, 212);
}

#[test]
fn counter_and_guard_round_trip_through_the_bytes() {
    let mut cells = [0u8; NOINIT_LEN];
    let c = ResetCounterCell {
        magic: 0x0102_0304,
        count: 0x0506_0708,
        check: 0x090A_0B0C,
    };
    write_counter(&mut cells, &c);
    assert_eq!(read_counter(&cells), c);
    assert_eq!(cells[0xC8], 0x04);
    assert_eq!(cells[0xD3], 0x09);
    let g = ResetGuardCell {
        magic: 0x1112_1314,
        count: 0x15,
        safe: 0x16,
        pad: 0x1718,
        window_s: 0x191A_1B1C,
        last_uptime_s: 0x1D1E_1F20,
        crc: 0x2122,
    };
    write_guard(&mut cells, &g);
    assert_eq!(read_guard(&cells), g);
    assert_eq!(
        cells[0xB4..0xC6],
        [
            0x14, 0x13, 0x12, 0x11, 0x15, 0x16, 0x18, 0x17, 0x1C, 0x1B, 0x1A, 0x19, 0x20, 0x1F,
            0x1E, 0x1D, 0x22, 0x21
        ]
    );
}
