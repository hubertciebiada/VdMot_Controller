// Port of software_stm32/test/native/glue/test_otasupport.cpp (glue_otasupport): the boot window
// after reset in which the ESP may start an STM update (DEADBEEF -> BEEFIT -> ROM bootloader).
// The C++ fake's `inject` puts bytes straight into the Serial1 ring: here into the Fifo.
#![allow(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used
)]

use super::*;
use crate::fifo::Fifo;
use crate::id_block::BootId;
use crate::stage::BRR_HSE;
use crate::test_support::{Ev, Fake};

struct Rig {
    fake: Fake,
    fifo: Fifo,
    win: Window,
    ms: u32,
}

impl Rig {
    /// glue::begin() + BootSetup()
    fn setup() -> Rig {
        let mut fake = Fake::new();
        fake.start_ms_clock();
        let mut rig = Rig {
            fake,
            fifo: Fifo::new(),
            win: Window::new(),
            ms: 0,
        };
        setup(&mut rig.fake, BRR_HSE, &mut rig.fifo, &mut rig.ms);
        rig
    }

    fn call(&mut self) -> Step {
        self.win.step(
            &mut self.fake,
            &mut self.fifo,
            &BootId::STANDARD,
            &mut self.ms,
        )
    }

    /// BootLoop() calls until the window ends or n calls; the number of calls
    fn loop_calls(&mut self, n: u32) -> u32 {
        let mut calls = 0;
        while calls < n {
            let step = self.call();
            calls += 1;
            if step != Step::Continue {
                break;
            }
        }
        calls
    }

    fn inject(&mut self, bytes: &[u8]) {
        for &b in bytes {
            assert!(self.fifo.push(b));
        }
    }

    fn led_writes(&self) -> usize {
        self.fake.count(|e| matches!(e, Ev::Led(_)))
    }
}

#[test]
fn boot_setup_led_on_usart1_8e1_and_the_bytes_of_the_first_10_ms_dropped() {
    let mut fake = Fake::new();
    fake.start_ms_clock();
    // the ESP talks during the start: one byte per millisecond
    for i in 0..10u64 {
        fake.send(fake.now_us + 400 + i * 1000, b"x");
    }
    let mut fifo = Fifo::new();
    let mut ms = 0;
    setup(&mut fake, BRR_HSE, &mut fifo, &mut ms);
    assert_eq!(
        fake.events(),
        [Ev::LedBegin, Ev::Led(false), Ev::UartBegin(BRR_HSE)]
    );
    assert!(!fake.led);
    // delay(10)
    assert_eq!(ms, 10);
    assert!(fake.now_us > 9_000 && fake.now_us <= 10_000 + fake.poll_us);
    assert_eq!(fake.rx_reads, 10);
    assert!(fifo.is_empty());
}

#[test]
fn without_deadbeef_the_window_ends_at_call_3001_and_call_3002_starts_the_application() {
    let mut rig = Rig::setup();
    let ms_before = rig.ms;
    assert_eq!(rig.loop_calls(3001), 3001);
    // one delay(1) per call of the window
    assert_eq!(rig.ms - ms_before, 3001);
    assert_eq!(rig.call(), Step::Timeout);
    assert_eq!(rig.ms - ms_before, 3001);
    assert!(rig.fake.tx_bytes.is_empty());
    // the window is over for good
    assert_eq!(rig.call(), Step::Timeout);
}

#[test]
fn the_led_toggles_on_every_102nd_call_of_the_window() {
    let mut rig = Rig::setup();
    let writes_before = rig.led_writes();
    rig.loop_calls(101);
    assert_eq!(rig.led_writes(), writes_before);
    rig.loop_calls(1);
    assert_eq!(rig.led_writes(), writes_before + 1);
    assert!(rig.fake.led);
    rig.loop_calls(101);
    assert_eq!(rig.led_writes(), writes_before + 1);
    rig.loop_calls(1);
    assert!(!rig.fake.led);
    assert_eq!(rig.led_writes(), writes_before + 2);
}

#[test]
fn deadbeef_answers_beefit_and_jumps_into_the_bootloader() {
    let mut rig = Rig::setup();
    rig.loop_calls(5);
    rig.inject(b"DEADBEEF");
    assert_eq!(rig.call(), Step::Continue);
    assert!(!rig.fake.led);
    assert_eq!(rig.fifo.len(), 0);
    let ms_before = rig.ms;
    let log_before = rig.fake.log.len();
    assert_eq!(rig.call(), Step::Jump);
    assert_eq!(rig.fake.tx_bytes, b"BEEFIT\r\n");
    assert_eq!(rig.fake.count(|e| *e == Ev::Flush), 1);
    // LED off (HIGH on the blackpill), delay(10), BEEFIT, flush, delay(200)
    let log = &rig.fake.log[log_before..];
    assert_eq!(log[0].1, Ev::Led(true));
    assert_eq!(log[1].1, Ev::Tx(b'B'));
    let first_tx = log[1].0 - log[0].0;
    assert!(first_tx > 9_000 && first_tx <= 10_000 + 5, "{first_tx}");
    assert_eq!(log[9].1, Ev::Flush);
    let after_flush = rig.fake.now_us - log[9].0;
    assert!(
        after_flush > 199_000 && after_flush <= 200_000 + 5,
        "{after_flush}"
    );
    assert_eq!(log.len(), 10);
    assert_eq!(rig.ms - ms_before, 210);
    assert!(rig.fake.led);
}

#[test]
fn fewer_than_8_bytes_wait_and_8_other_bytes_are_read_and_ignored() {
    let mut rig = Rig::setup();
    rig.inject(b"DEADBEE");
    assert_eq!(rig.call(), Step::Continue);
    assert_eq!(rig.fifo.len(), 7);
    rig.inject(b"X");
    assert_eq!(rig.call(), Step::Continue);
    assert_eq!(rig.fifo.len(), 0);
    assert_eq!(rig.loop_calls(2999), 2999);
    assert_eq!(rig.call(), Step::Timeout);
    assert!(rig.fake.tx_bytes.is_empty());
}

#[test]
fn deadbeef_in_the_last_call_of_the_window_still_starts_the_update() {
    let mut rig = Rig::setup();
    assert_eq!(rig.loop_calls(3000), 3000);
    rig.inject(b"DEADBEEF");
    assert_eq!(rig.call(), Step::Continue);
    assert_eq!(rig.call(), Step::Jump);
    assert_eq!(rig.fake.tx_bytes, b"BEEFIT\r\n");
}

#[test]
fn deadbeef_one_call_after_the_window_is_ignored() {
    let mut rig = Rig::setup();
    assert_eq!(rig.loop_calls(3001), 3001);
    rig.inject(b"DEADBEEF");
    assert_eq!(rig.call(), Step::Timeout);
    assert!(rig.fake.tx_bytes.is_empty());
}

#[test]
fn a_match_on_a_toggle_call_leaves_the_led_toggled_as_in_cpp() {
    // the C++ switches the LED off on the match and still runs the toggle of that call
    let mut rig = Rig::setup();
    rig.loop_calls(101);
    rig.inject(b"DEADBEEF");
    assert_eq!(rig.call(), Step::Continue);
    let leds: std::vec::Vec<_> = rig
        .fake
        .events()
        .into_iter()
        .filter(|e| matches!(e, Ev::Led(_)))
        .collect();
    assert_eq!(&leds[leds.len() - 2..], [Ev::Led(false), Ev::Led(true)]);
    assert_eq!(rig.call(), Step::Jump);
}

#[test]
fn the_pattern_and_the_reply_come_from_the_id_block() {
    let mut rig = Rig::setup();
    let id = BootId {
        pattern: *b"12345678",
        reply: *b"ABCDEF",
    };
    rig.inject(b"DEADBEEF");
    assert_eq!(
        rig.win.step(&mut rig.fake, &mut rig.fifo, &id, &mut rig.ms),
        Step::Continue
    );
    rig.inject(b"12345678");
    assert_eq!(
        rig.win.step(&mut rig.fake, &mut rig.fifo, &id, &mut rig.ms),
        Step::Continue
    );
    assert_eq!(
        rig.win.step(&mut rig.fake, &mut rig.fifo, &id, &mut rig.ms),
        Step::Jump
    );
    assert_eq!(rig.fake.tx_bytes, b"ABCDEF\r\n");
}

#[test]
fn only_one_block_is_read_per_call() {
    let mut rig = Rig::setup();
    rig.inject(b"XXXXXXXXDEADBEEF");
    assert_eq!(rig.call(), Step::Continue);
    assert_eq!(rig.fifo.len(), 8);
    assert_eq!(rig.call(), Step::Continue);
    assert_eq!(rig.call(), Step::Jump);
}

#[test]
fn window_runs_setup_then_the_calls_and_reports_the_end() {
    let mut fake = Fake::new();
    fake.start_ms_clock();
    let mut ms = 0;
    assert_eq!(
        window(&mut fake, &BootId::STANDARD, 0x8B, &mut ms),
        WindowEnd::Timeout
    );
    assert_eq!(ms, 10 + 3001);
    assert_eq!(fake.times_of(&Ev::UartBegin(0x8B)).len(), 1);

    let mut fake = Fake::new();
    fake.start_ms_clock();
    fake.send(20_000, b"DEADBEEF");
    let mut ms = 0;
    assert_eq!(
        window(&mut fake, &BootId::STANDARD, 0x8B, &mut ms),
        WindowEnd::Update
    );
    assert_eq!(fake.tx_bytes, b"BEEFIT\r\n");
}

#[test]
fn wait_ms_counts_periods_and_keeps_the_bytes() {
    let mut fake = Fake::new();
    fake.start_ms_clock();
    fake.uart_begin(BRR_HSE);
    fake.send(100, b"abc");
    let mut fifo = Fifo::new();
    let mut ms = 7;
    wait_ms(&mut fake, 3, &mut fifo, &mut ms);
    assert_eq!(ms, 10);
    assert_eq!(fifo.len(), 3);
    assert_eq!(fifo.pop(), Some(b'a'));
    let t = fake.now_us;
    wait_ms(&mut fake, 0, &mut fifo, &mut ms);
    assert_eq!((fake.now_us, ms), (t, 10));
}

#[test]
fn constants_are_the_cpp_values() {
    assert_eq!(WINDOW_CALLS, 3000);
    assert_eq!(LED_PERIOD, 100);
    assert_eq!(SETUP_DROP_MS, 10);
    assert_eq!(BEFORE_REPLY_MS, 10);
    assert_eq!(AFTER_REPLY_MS, 200);
    assert_eq!(MAX_CALLS, 3002);
}
