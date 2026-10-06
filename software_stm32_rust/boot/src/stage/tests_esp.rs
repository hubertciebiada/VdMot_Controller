// The boot window against the senders it meets (docs/rust/GLUE-DESIGN-STM.md §7.3): the ESP 2.1
// flasher (`DEADBEEF\n` 20 ms after its NRST release, then every 100 ms), stray bytes, the
// legacy ESP 1.x (`DEADBEEF\r\n` once at 1.5 s), a flood of garbage. Bytes arrive at 115200
// 8E1 (95 us each). Fake time 0 is the start of the boot stage; `start_ms` shifts the ESP's
// NRST release before it (cortex-m-rt start-up and a slow reset take a few ms).
#![allow(clippy::indexing_slicing, clippy::arithmetic_side_effects, clippy::unwrap_used)]

use super::*;
use crate::test_support::{esp21_sends, Ev, Fake, BYTE_US};

/// Time of the first `BEEFIT` byte, or `None` when the window ended without an update.
fn beefit_at(fake: &mut Fake) -> Option<u64> {
    match run(fake) {
        BootEnd::Update => {
            assert_eq!(fake.tx_bytes, b"BEEFIT\r\n");
            Some(fake.times_of(&Ev::Tx(b'B'))[0])
        }
        BootEnd::Timeout(_) => {
            assert!(fake.tx_bytes.is_empty());
            None
        }
    }
}

/// ESP 2.1 sends with its NRST release `start_ms` before the boot stage starts.
fn esp21(fake: &mut Fake, start_ms: u64) -> std::vec::Vec<u64> {
    esp21_sends(fake, 20_000 - start_ms * 1000)
}

#[test]
fn esp21_the_first_send_is_aligned_and_beefit_follows_within_15_ms() {
    let mut fake = Fake::new();
    let sends = esp21(&mut fake, 0);
    let t = beefit_at(&mut fake).unwrap();
    let block_done = sends[0] + 8 * BYTE_US;
    assert!(t > block_done && t <= block_done + 15_000, "{t}");
}

#[test]
fn esp21_with_a_dead_hse_and_a_late_start_the_first_send_is_dropped_and_the_second_matches() {
    // stage 7 ms after the release, HSE probe 5 ms: the drop ends 22 ms after the release
    let mut fake = Fake::new();
    fake.hse_start_us = None;
    let sends = esp21(&mut fake, 7);
    let t = beefit_at(&mut fake).unwrap();
    assert!(t > sends[1] && t <= sends[1] + 9 * BYTE_US + 15_000, "{t}");
}

#[test]
fn a_stray_byte_after_the_drop_realigns_on_the_8th_send() {
    let mut fake = Fake::new();
    let sends = esp21(&mut fake, 0);
    fake.send(15_000 - BYTE_US, &[0x00]);
    let t = beefit_at(&mut fake).unwrap();
    // send 8 is the first one that starts on a block boundary: (1 + 9 * 7) % 8 == 0
    assert!(t > sends[7] && t <= sends[7] + 9 * BYTE_US + 15_000, "{t}");
}

#[test]
fn a_stray_byte_inside_the_first_10_ms_is_dropped() {
    let mut fake = Fake::new();
    let sends = esp21(&mut fake, 0);
    fake.send(5_000, &[0xFF]);
    let t = beefit_at(&mut fake).unwrap();
    assert!(t > sends[0] && t < sends[1], "{t}");
}

#[test]
fn a_handshake_inside_the_first_10_ms_is_dropped_too() {
    let mut fake = Fake::new();
    fake.send(3_000, b"DEADBEEF");
    fake.send(105_000, b"DEADBEEF\n");
    let t = beefit_at(&mut fake).unwrap();
    assert!(t > 105_000, "{t}");
}

#[test]
fn legacy_esp_1x_once_at_1500_ms() {
    let mut fake = Fake::new();
    fake.send(1_500_000, b"DEADBEEF\r\n");
    let t = beefit_at(&mut fake).unwrap();
    assert!(t > 1_500_000 && t <= 1_500_000 + 8 * BYTE_US + 15_000, "{t}");
}

#[test]
fn without_a_handshake_the_window_ends_after_3011_ms_without_sending() {
    let mut fake = Fake::new();
    assert_eq!(beefit_at(&mut fake), None);
    let end = fake.times_of(&Ev::UartEnd)[0];
    assert!(end > 3_011_000 && end < 3_012_000, "{end}");
}

#[test]
fn deadbeef_complete_at_the_start_of_the_last_call_still_starts_the_update() {
    // HSE ready at 0.5 ms: SysTick periods end at k + 0.5 ms, BootSetup ends at 10.5 ms and
    // call n starts at 9.5 + n ms; call 3001 starts at 3010.5 ms
    let mut fake = Fake::new();
    fake.send(3_010_500 - 8 * BYTE_US - 50, b"DEADBEEF");
    assert!(beefit_at(&mut fake).is_some());
    // one period later the window is over
    let mut fake = Fake::new();
    fake.send(3_011_500 - 8 * BYTE_US - 50, b"DEADBEEF");
    assert_eq!(beefit_at(&mut fake), None);
}

#[test]
fn a_flood_of_garbage_neither_matches_nor_stretches_the_window() {
    let mut fake = Fake::new();
    let garbage: std::vec::Vec<u8> = (0..34_000u32).map(|i| 0x80 | (i % 0x80) as u8).collect();
    fake.send(0, &garbage);
    assert_eq!(beefit_at(&mut fake), None);
    let end = fake.times_of(&Ev::UartEnd)[0];
    assert!(end > 3_011_000 && end < 3_012_000, "{end}");
    assert_eq!(fake.rx_overruns, 0);
}

#[test]
fn after_a_flood_the_esp_pattern_realigns() {
    let mut fake = Fake::new();
    let garbage: std::vec::Vec<u8> = (0..10_000u32).map(|i| (i % 251) as u8 | 0x80).collect();
    fake.send(0, &garbage);
    let mut sends = std::vec::Vec::new();
    let mut at = 1_020_000;
    while at < 2_500_000 {
        fake.send(at, b"DEADBEEF\n");
        sends.push(at);
        at += 100_000;
    }
    let t = beefit_at(&mut fake).unwrap();
    // the ring drains 8 bytes per ms, then the 9-byte period aligns within 8 sends
    assert!(t < sends[10], "{t}");
}
