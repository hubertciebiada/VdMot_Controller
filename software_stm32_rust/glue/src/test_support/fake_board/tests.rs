// Port of the board cases of software_stm32/test/native/glue/test_fakes.cpp (glue_fakes): the
// glue suites trust the fakes, so each fake behaviour the glue depends on is pinned here. The
// Print, UART, Wire, 24LC64 and 1-Wire cases belong to the fakes of the I/O modules; the runner
// hooks (reboots across the persistent stores) to the system suite.
use std::cell::Cell;
use std::format;
use std::string::String;
use std::vec;
use std::vec::Vec;

use crate::hal::CurrentAdc;
use crate::hal::{Clock, In, Out, Pins, RevIrq, System, Watchdog};
use crate::test_support::valve_sim::FixedAdc;

use super::{
    expect_panic, BoardWatchdog, Ev, FakeBoard, FakeSystem, FakeTimer, FakeWatchdog, SystemReset,
    WatchdogReset,
};

#[test]
fn time_millis_wraps_at_2_32_ms_delay_and_delay_us_advance_it() {
    let board = FakeBoard::new();
    board.set_now_us((1u64 << 32) * 1000 - 2000);
    assert_eq!(board.millis(), 0xFFFF_FFFE);
    board.delay_ms(3);
    assert_eq!(board.millis(), 1);
    board.delay_us(1500);
    assert_eq!(board.micros(), board.now_us() as u32);
    assert_eq!(board.millis(), 2);
    assert_eq!(
        board.events_of(|e| matches!(e, Ev::Delay(_))),
        vec![Ev::Delay(3)]
    );
    assert_eq!(
        board.events_of(|e| matches!(e, Ev::DelayUs(_))),
        vec![Ev::DelayUs(1500)]
    );
}

#[test]
fn time_auto_advance_lets_a_busy_loop_on_millis_see_time_pass() {
    let board = FakeBoard::new();
    board.auto_advance_us.set(250);
    let start = board.millis();
    let mut polls = 0;
    while board.millis().wrapping_sub(start) < 10 {
        polls += 1;
    }
    assert_eq!(polls, 39);
}

#[test]
fn pins_writes_are_recorded_in_one_sequence_latches_and_inputs_read_back() {
    // C++ also pins the modes, the open-drain read of PB9 and the pull-ups of the I2C lines: the
    // pin modes are firmware set-up in Rust, the I2C lines belong to the I2C fakes
    let board = FakeBoard::new();
    // the firmware's set-up leaves the PSU enable high (off)
    assert!(board.latch(Out::PsuEna));
    board.set(Out::PsuEna, true);
    board.set(Out::Dir, true);
    let events = board.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, Ev::Write(Out::PsuEna, true));
    assert!(events[0].seq < events[1].seq);
    assert!(board.latch(Out::Dir));
    board.set(Out::PsuEna, false);
    assert!(!board.latch(Out::PsuEna));
    assert!(!board.read(In::Button));
    board.set_input(In::Button, true);
    assert!(board.read(In::Button));
    assert!(!board.read(In::RevIn));
}

#[test]
fn adc_the_conversions_come_as_12_bit_values() {
    // C++ analogRead() maps to analogReadResolution(): CurrentAdc always delivers 12 bit
    let mut adc = FixedAdc {
        current: 4095,
        reference: 2048,
    };
    assert_eq!(adc.sample(), (4095, 2048));
}

#[test]
fn exti_runs_an_attached_handler_not_a_detached_one_not_with_interrupts_disabled() {
    let board = FakeBoard::new();
    let calls = Cell::new(0);
    let count = || calls.set(calls.get() + 1);
    board.fire_exti(count);
    assert_eq!(calls.get(), 0);
    board.attach();
    board.fire_exti(count);
    assert_eq!(calls.get(), 1);
    board.masked.set(true);
    board.fire_exti(count);
    board.masked.set(false);
    assert_eq!(calls.get(), 1);
    board.detach();
    board.fire_exti(count);
    assert_eq!(calls.get(), 1);
    assert_eq!(board.events_of(|e| *e == Ev::Attach).len(), 1);
    assert_eq!(board.events_of(|e| *e == Ev::Detach).len(), 1);
    // C++ "PRIMASK: __get/__set restore the previous state": the glue never touches the mask, the
    // firmware's IsrCell::lock does (design §2.3)
}

#[test]
fn timers_run_at_their_interval_tim1_before_tim2_not_while_masked() {
    let board = FakeBoard::new();
    let mut t1 = FakeTimer::default();
    let mut t2 = FakeTimer::default();
    let mut ticks: Vec<String> = Vec::new();
    assert!(t2.attach_at(board.now_us(), 2000));
    assert!(t1.attach_at(board.now_us(), 1000));
    let tick = |t1: &mut FakeTimer, t2: &mut FakeTimer, ticks: &mut Vec<String>| {
        let now = board.now_us();
        if t1.due(now) {
            ticks.push(format!("tim1@{}", now / 1000));
        }
        if t2.due(now) {
            ticks.push(format!("tim2@{}", now / 1000));
        }
    };
    board.advance_us_with(2000, &mut || tick(&mut t1, &mut t2, &mut ticks));
    assert_eq!(ticks, vec!["tim1@1", "tim1@2", "tim2@2"]);
    ticks.clear();
    board.masked.set(true);
    board.advance_us_with(3000, &mut || tick(&mut t1, &mut t2, &mut ticks));
    board.masked.set(false);
    assert!(ticks.is_empty());
    t1.fail_attach = true;
    assert!(!t1.attach_at(board.now_us(), 1000));
    assert_eq!(t1.attaches, 2);
}

#[test]
fn watchdog_time_passing_its_timeout_without_a_reload_is_a_watchdog_reset() {
    let board = FakeBoard::new();
    let mut dog = FakeWatchdog::default();
    // not started: reload() does nothing
    BoardWatchdog {
        board: &board,
        dog: &mut dog,
    }
    .reload();
    assert_eq!(dog.reloads, 0);
    // below IWDG_TIMEOUT_MIN
    dog.begin(board.now_us(), 100);
    assert!(!dog.enabled);
    dog.begin(board.now_us(), 8_000_000);
    assert_eq!(dog.timeout_us, 8_000_000);
    board.advance_ms(7999);
    dog.check(board.now_us());
    BoardWatchdog {
        board: &board,
        dog: &mut dog,
    }
    .reload();
    assert_eq!(dog.last_reload_us, 7_999_000);
    board.advance_ms(7999);
    dog.check(board.now_us());
    board.advance_ms(1);
    assert!(expect_panic::<WatchdogReset>(|| dog.check(board.now_us())));
    assert!(!dog.enabled);
    // C++ WatchdogBegin event: the firmware starts the watchdog
    assert_eq!(board.events_of(|e| *e == Ev::Reload).len(), 1);
}

#[test]
fn hal_a_system_reset_panics_the_device_id_is_the_f401ccs() {
    assert!(expect_panic::<SystemReset>(|| FakeSystem.reset()));
    assert_eq!(FakeSystem.dev_id(), 0x423);
}
