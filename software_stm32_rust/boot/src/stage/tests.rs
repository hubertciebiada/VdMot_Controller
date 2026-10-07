// The boot stage sequence (docs/rust/GLUE-DESIGN-STM.md §5.2) on the fake board: order of
// the steps, the boot clock of D1 and the end of the stage. The ESP patterns are in
// tests_esp.rs.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::panic
)]

use super::*;
use crate::capture::{read_counter, CSR_IWDGRSTF, CSR_PINRSTF, CSR_PORRSTF};
use crate::test_support::{Ev, Fake};
use vdm_stm_core::system_stats::{BootReason, RESET_COUNTER_MAGIC};

fn token(end: BootEnd) -> BootToken {
    match end {
        BootEnd::Timeout(t) => t,
        BootEnd::Update => panic!("the window took an update"),
    }
}

fn index_of(fake: &Fake, e: &Ev) -> usize {
    fake.events().iter().position(|x| x == e).unwrap()
}

#[test]
fn the_steps_run_in_the_order_of_the_cpp_setup() {
    let mut fake = Fake::new();
    fake.csr = CSR_PINRSTF;
    let _ = run(&mut fake);
    let ev = fake.events();
    assert_eq!(
        ev[..12],
        [
            Ev::ResetFlags,
            Ev::ClearResetFlags,
            Ev::Noinit,
            Ev::SetNoinit,
            Ev::OutputsSafe,
            Ev::TickStart(HSI_TICK_RELOAD),
            Ev::HseOn,
            Ev::SysclkHse,
            Ev::TickStart(HSE_TICK_RELOAD),
            Ev::BootId,
            Ev::LedBegin,
            Ev::Led(false),
        ]
    );
    assert_eq!(ev[12], Ev::UartBegin(BRR_HSE));
    assert_eq!(
        &ev[ev.len() - 3..],
        [Ev::UartEnd, Ev::TickStop, Ev::WatchdogStart]
    );
}

#[test]
fn the_capture_is_written_back_before_the_outputs_and_the_window() {
    let mut fake = Fake::new();
    fake.csr = CSR_PORRSTF | CSR_PINRSTF;
    let t = token(run(&mut fake));
    assert_eq!(t.reset().reason, BootReason::PowerOn);
    assert_eq!(t.reset().resets, 0);
    assert!(!t.reset().safe_mode);
    let counter = read_counter(&fake.cells);
    assert_eq!(counter.magic, RESET_COUNTER_MAGIC);
    assert_eq!(fake.csr, 0, "RMVF clears the flags");
    // the next boot counts on what this one wrote
    fake.csr = CSR_IWDGRSTF | CSR_PINRSTF;
    let t = token(run(&mut fake));
    assert_eq!(t.reset().reason, BootReason::IndependentWatchdog);
    assert_eq!(t.reset().resets, 1);
}

#[test]
fn an_hse_ready_within_5_ms_runs_the_window_on_the_hse() {
    let mut fake = Fake::new();
    fake.hse_start_us = Some(2_200);
    let t = token(run(&mut fake));
    assert!(t.hse());
    assert_eq!(fake.sysclk_hz, HSE_HZ);
    assert_eq!(fake.times_of(&Ev::UartBegin(BRR_HSE)).len(), 1);
    assert_eq!(fake.times_of(&Ev::HseOff).len(), 0);
    // 2 SysTick periods of the probe, the 10 ms of BootSetup and 3001 calls
    assert_eq!(t.boot_ms(), 2 + 10 + 3001);
}

#[test]
fn a_dead_hse_is_switched_off_after_5_ms_and_the_window_runs_on_hsi() {
    let mut fake = Fake::new();
    fake.hse_start_us = None;
    let t = token(run(&mut fake));
    assert!(!t.hse());
    assert_eq!(fake.sysclk_hz, HSI_HZ);
    assert_eq!(fake.count(|e| *e == Ev::SysclkHse), 0);
    assert_eq!(fake.count(|e| matches!(e, Ev::TickStart(_))), 1);
    assert_eq!(fake.times_of(&Ev::UartBegin(BRR_HSI)).len(), 1);
    let off = fake.times_of(&Ev::HseOff);
    let on = fake.times_of(&Ev::HseOn);
    assert_eq!(off.len(), 1);
    assert!(
        off[0] - on[0] >= 5_000 && off[0] - on[0] < 5_100,
        "{:?}",
        (on, off)
    );
    assert_eq!(t.boot_ms(), 5 + 10 + 3001);
    // the window still takes 3.011 s on the HSI SysTick
    let begin = fake.times_of(&Ev::UartBegin(BRR_HSI))[0];
    let end = fake.times_of(&Ev::UartEnd)[0];
    assert!(
        end - begin > 3_010_000 && end - begin <= 3_011_100,
        "{}",
        end - begin
    );
}

#[test]
fn the_hse_limit_is_5_ms() {
    let mut fake = Fake::new();
    fake.hse_start_us = Some(4_900);
    assert!(token(run(&mut fake)).hse());
    let mut fake = Fake::new();
    fake.hse_start_us = Some(5_100);
    let t = token(run(&mut fake));
    assert!(!t.hse());
    assert_eq!(fake.count(|e| *e == Ev::HseOff), 1);
}

#[test]
fn the_end_of_the_window_resets_usart1_and_stops_systick() {
    let mut fake = Fake::new();
    let _ = token(run(&mut fake));
    assert!(!fake.uart_on);
    let ev = fake.events();
    assert_eq!(ev.iter().filter(|e| **e == Ev::UartEnd).count(), 1);
    assert_eq!(ev.iter().filter(|e| **e == Ev::TickStop).count(), 1);
    // no byte went out in the window
    assert!(fake.tx_bytes.is_empty());
}

#[test]
fn the_watchdog_starts_once_after_a_window_without_handshake() {
    // B6 and the D9 residual: never before the window ends, then right before the
    // application, so code after the boot stage runs watched even before the application
    // stage sets the watchdog up itself
    let mut fake = Fake::new();
    let _ = token(run(&mut fake));
    let starts = fake.times_of(&Ev::WatchdogStart);
    assert_eq!(starts.len(), 1);
    assert!(starts[0] >= fake.times_of(&Ev::UartEnd)[0]);
    assert_eq!(fake.events().last(), Some(&Ev::WatchdogStart));
    // the window ran its 3001 calls before
    let begin = fake.times_of(&Ev::UartBegin(BRR_HSE))[0];
    assert!(starts[0] - begin > 3_010_000, "{}", starts[0] - begin);
}

#[test]
fn an_update_ends_the_stage_without_resetting_usart1() {
    let mut fake = Fake::new();
    fake.send(30_000, b"DEADBEEF\n");
    assert_eq!(run(&mut fake), BootEnd::Update);
    assert_eq!(fake.tx_bytes, b"BEEFIT\r\n");
    assert_eq!(fake.count(|e| *e == Ev::UartEnd), 0);
    assert_eq!(fake.count(|e| *e == Ev::TickStop), 0);
    // B6: the ROM bootloader session runs without the IWDG
    assert_eq!(fake.count(|e| *e == Ev::WatchdogStart), 0);
}

#[test]
fn the_window_uses_the_pattern_of_the_id_block() {
    let mut fake = Fake::new();
    fake.id = crate::id_block::BootId {
        pattern: *b"DEADBEEG",
        reply: *b"BEEFIT",
    };
    fake.send(30_000, b"DEADBEEF\n");
    assert!(matches!(run(&mut fake), BootEnd::Timeout(_)));
    let mut fake = Fake::new();
    fake.id.reply = *b"BEEFIX";
    fake.send(30_000, b"DEADBEEF\n");
    assert_eq!(run(&mut fake), BootEnd::Update);
    assert_eq!(fake.tx_bytes, b"BEEFIX\r\n");
}

#[test]
fn probe_hse_waits_whole_periods_and_counts_them() {
    let mut fake = Fake::new();
    fake.start_ms_clock();
    fake.hse_start_us = Some(2_500);
    let mut elapsed = 40;
    assert!(probe_hse(&mut fake, 5, &mut elapsed));
    assert_eq!(elapsed, 42);
    let mut fake = Fake::new();
    fake.start_ms_clock();
    fake.hse_start_us = None;
    let mut elapsed = 0;
    assert!(!probe_hse(&mut fake, 3, &mut elapsed));
    assert_eq!(elapsed, 3);
    assert_eq!(fake.events(), [Ev::HseOn, Ev::HseOff]);
    // limit 0: one look
    let mut fake = Fake::new();
    fake.start_ms_clock();
    let mut elapsed = 0;
    assert!(!probe_hse(&mut fake, 0, &mut elapsed));
    assert_eq!(elapsed, 0);
}

#[test]
fn the_application_probe_waits_100_ms_and_stops_systick() {
    let mut fake = Fake::new();
    fake.hse_start_us = None;
    assert!(!probe_app_hse(&mut fake, false));
    assert_eq!(
        fake.events(),
        [
            Ev::TickStart(HSI_TICK_RELOAD),
            Ev::HseOn,
            Ev::HseOff,
            Ev::TickStop
        ]
    );
    let t = fake.times_of(&Ev::HseOff)[0] - fake.times_of(&Ev::HseOn)[0];
    assert!((100_000..100_100).contains(&t), "{t}");

    let mut fake = Fake::new();
    fake.hse_start_us = Some(60_000);
    assert!(probe_app_hse(&mut fake, false));
    assert_eq!(fake.events().last(), Some(&Ev::TickStop));

    // the boot stage runs on the HSE: SysTick on 25 MHz
    let mut fake = Fake::new();
    fake.sysclk_hz = HSE_HZ;
    assert!(probe_app_hse(&mut fake, true));
    assert_eq!(fake.events()[0], Ev::TickStart(HSE_TICK_RELOAD));
}

#[test]
fn clock_constants() {
    assert_eq!(HSI_TICK_RELOAD, 15_999);
    assert_eq!(HSE_TICK_RELOAD, 24_999);
    // BRR = round(f / 115200) with oversampling 16
    assert_eq!(u32::from(BRR_HSI), (HSI_HZ + BAUD / 2) / BAUD);
    assert_eq!(u32::from(BRR_HSE), (HSE_HZ + BAUD / 2) / BAUD);
    assert_eq!(BRR_HSI, 0x8B);
    assert_eq!(BRR_HSE, 0xD9);
    assert_eq!(BOOT_HSE_LIMIT_MS, 5);
    assert_eq!(APP_HSE_LIMIT_MS, 100);
}

#[test]
fn the_token_reports_what_the_stage_saw() {
    let mut fake = Fake::new();
    fake.csr = CSR_PINRSTF;
    let t = token(run(&mut fake));
    assert_eq!(
        t.reset(),
        crate::capture::ResetInfo {
            reason: BootReason::Pin,
            resets: 0,
            safe_mode: false
        }
    );
    assert_eq!(index_of(&fake, &Ev::ResetFlags), 0);
}
