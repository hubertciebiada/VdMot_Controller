// Port of software_stm32/test/native/glue/test_main.cpp (glue_main): start-up order,
// setup_system(), the three branches of loop_system() and the watchdog feed, the main loop for
// ever.
//
// C++ "setup: reset cause first, then the valve outputs safe, then the boot window" and the
// BootLoop() part of "loop: the boot window first, ...": the order sysstat_capture_reset ->
// valve_pins_safe -> boot window is the firmware's boot stage in Rust (design §5.2 steps 1, 2 and
// 4-6: firmware/src/boot_hw.rs and vdm-stm-boot, Renode E1/E10), which runs before
// MainLoop::run(); no glue form.
use core::sync::atomic::Ordering;
use std::string::String;
use std::vec;
use std::vec::Vec;

use vdm_stm_core::system_stats::BootReason;

use crate::hal::{In, Out};
use crate::test_support::fake_board::{expect_panic, Ev, Stop};

use super::stub_env::MainBench;

fn calls(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| String::from(*s)).collect()
}

const K10MS_BRANCH: [&str; 5] = [
    "app_loop()",
    "app_warm_save()",
    "communication_loop()",
    "temperature_loop()",
    "terminal_supervise()",
];

#[test]
fn setup_system_watchdog_i2c_pins_modules_and_the_valve_timer_in_this_order() {
    let mut b = MainBench::new();
    b.setup_system();
    assert_eq!(
        b.env.log.calls,
        calls(&[
            "sysstat_boot_reason()",
            "i2c_bus_recover()",
            "Terminal_Init()",
            "sysstat_safe_mode()",
            "communication_setup()",
            "eepromsetup()",
            "eeprom_read_layout(&eep_content)",
            "temperature_setup()",
            "app_setup()",
            "valve_setup()",
            "app_restore()"
        ])
    );
    // C++ IWatchdog.timeoutUs == 8000000 and its begin before Wire.begin(): the firmware starts the
    // watchdog before setup_system() (design §5.4)
    assert_eq!(b.env.dog.reloads, 3);
    // Wire.setSDA(PB7), setSCL(PB6), begin(): one I2C start
    assert_eq!(b.env.board.events_of(|e| *e == Ev::WireBegin).len(), 1);
    // the valve PSU stays off: latch high (the open drain mode is firmware set-up, as the modes of
    // LED, BUTTON and the analog inputs and the 12-bit ADC)
    assert_eq!(b.env.board.writes_of(Out::PsuEna), vec![true]);
    assert_eq!(
        b.env.board.events_of(|e| matches!(e, Ev::Delay(_))),
        vec![Ev::Delay(500)]
    );
    // C++ ITimer1.callback == valve_loop: the firmware's TIM2 vector runs valve_loop
    assert_eq!(b.env.tim2.interval_us, 10000);
    assert!(b.env.tim2.running());
    assert!(b.env.take_tx().is_empty());
}

#[test]
fn setup_system_a_watchdog_reset_is_reported_on_the_terminal() {
    let mut b = MainBench::new();
    b.env.reason = BootReason::IndependentWatchdog;
    b.setup_system();
    assert_eq!(b.env.take_tx(), "reset by watchdog\r\n");
}

#[test]
fn setup_system_safe_mode_is_reported_on_the_terminal() {
    let mut b = MainBench::new();
    b.env.safe_mode = true;
    b.setup_system();
    assert_eq!(b.env.take_tx(), "safe mode\r\n");
}

#[test]
fn loop_system_the_10_ms_100_ms_and_1_s_branches_run_when_more_than_their_period_has_passed() {
    let mut b = MainBench::new();
    b.loop_at(0);
    b.loop_at(10);
    assert!(b.branch_calls().is_empty());
    b.loop_at(11);
    assert_eq!(b.branch_calls(), calls(&K10MS_BRANCH));
    b.env.log.clear();
    b.loop_at(21);
    assert!(b.branch_calls().is_empty());
    b.loop_at(22);
    assert_eq!(b.branch_calls(), calls(&K10MS_BRANCH));
    b.env.log.clear();
    b.loop_at(100);
    assert_eq!(b.branch_calls(), calls(&K10MS_BRANCH));
    b.env.log.clear();
    b.loop_at(101);
    assert_eq!(b.branch_calls(), calls(&["Terminal_Serve()"]));
    b.env.log.clear();
    b.loop_at(1000);
    let mut both = calls(&["Terminal_Serve()"]);
    both.extend(calls(&K10MS_BRANCH));
    assert_eq!(b.branch_calls(), both);
    b.env.log.clear();
    b.env.uptime = 1;
    b.loop_at(1001);
    assert_eq!(
        b.branch_calls(),
        calls(&["sysstat_uptime_s()", "app_1s_tick(1)", "eepromloop()"])
    );
}

#[test]
fn loop_system_app_10s_loop_gets_the_real_seconds_at_every_10th_second_branch() {
    let mut b = MainBench::new();
    let mut ms = 0;
    for s in 1..=20u32 {
        ms += 1001;
        // the 10th branch ran late
        b.env.uptime = if s >= 10 { s + 2 } else { s };
        b.env.log.clear();
        b.loop_at(ms);
        let ten = match s {
            10 => calls(&["app_10s_loop(12)"]),
            20 => calls(&["app_10s_loop(10)"]),
            _ => Vec::new(),
        };
        assert_eq!(b.env.log.calls_of("app_10s_loop"), ten, "second {s}");
        let one = if s == 10 {
            "app_1s_tick(3)"
        } else {
            "app_1s_tick(1)"
        };
        assert_eq!(
            b.env.log.calls_of("app_1s_tick"),
            calls(&[one]),
            "second {s}"
        );
    }
}

#[test]
fn loop_system_the_led_goes_off_at_the_30th_and_on_at_the_31st_100_ms_tick() {
    let mut b = MainBench::new();
    let mut ms = 0;
    for tick in 1..=31 {
        ms += 101;
        b.loop_at(ms);
        if tick < 30 {
            assert!(b.env.board.writes_of(Out::Led).is_empty(), "tick {tick}");
        }
    }
    assert_eq!(b.env.board.writes_of(Out::Led), vec![false, true]);
}

#[test]
fn loop_system_the_button_is_reported_every_other_100_ms_tick_while_it_reads_high() {
    let mut b = MainBench::new();
    b.env.board.set_input(In::Button, true);
    b.loop_at(101);
    b.loop_at(202);
    b.loop_at(303);
    assert_eq!(b.env.take_tx(), "Button pressed\r\nButton pressed\r\n");
}

#[test]
fn loop_system_the_watchdog_is_fed_only_while_the_valve_timer_makes_progress() {
    let mut b = MainBench::new();
    // C++ IWatchdog.begin(8000000): started by the firmware (the stub env)
    b.loop_at(11);
    assert_eq!(b.env.dog.reloads, 0);
    b.flags.valve_loop_ticks.store(1, Ordering::SeqCst);
    b.loop_at(22);
    assert_eq!(b.env.dog.reloads, 1);
    b.loop_at(33);
    assert_eq!(b.env.dog.reloads, 1);
    b.flags.valve_loop_ticks.store(2, Ordering::SeqCst);
    b.flags.valve_loop_stalled.store(true, Ordering::SeqCst);
    b.loop_at(44);
    assert_eq!(b.env.dog.reloads, 1);
    b.flags.valve_loop_stalled.store(false, Ordering::SeqCst);
    b.loop_at(55);
    assert_eq!(b.env.dog.reloads, 2);
}

#[test]
fn run_setup_system_then_the_main_loop_for_ever() {
    let mut b = MainBench::new();
    b.env.app_loop_stop_after = 2;
    b.env.board.auto_advance_us.set(100);
    let MainBench { main, flags, env } = &mut b;
    assert!(expect_panic::<Stop>(|| {
        main.run(flags, env);
    }));
    assert_eq!(b.env.log.calls_of("app_loop").len(), 2);
    assert_eq!(b.env.log.calls_of("app_setup").len(), 1);
}
