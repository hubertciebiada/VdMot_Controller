// New cases (no C++ counterpart): the delegations of the controller that the glue_system
// scenarios do not reach: the debug terminal on USART6, 1-Wire sensors, receive errors of
// USART1, the escalation and sensor commands, an EEPROM write retry with a bus restart, the LED
// and the button, and the HAL methods of the contexts that no module calls today.

use std::string::String;
use std::vec::Vec;

use vdm_stm_boot::fault_record::{FaultRecord, KIND_HARD_FAULT};
use vdm_stm_core::valve_codes::{ST_IDLE, ST_OPENING, ST_UNKNOWN};

use super::bench::{sim, Bench, Boot, Case, PLAIN};
use super::golden::Reset;
use super::{AppCtx, OwCtx};
use crate::app::AppEnv;
use crate::communication::FirmwareId;
use crate::hal::{Clock, In, Out, Pins, System};
use crate::i2c_bus::{I2cLine, LineMode};
use crate::motor::IsrFlags;
use crate::ow_devices::{OwDevicesEnv, TEMP_CMD_LOCK, TEMP_CMD_NEWSEARCH, TEMP_CMD_UNLOCK};
use crate::serial::{SR_FE, SR_NE, SR_ORE, SR_RXNE};
use crate::test_support::fake_board::Ev;

use core::sync::atomic::Ordering;

fn case() -> Case {
    Case::new("", "a case without a golden")
}

/// The I2C bus calls on the board: line modes and levels of a recovery, Wire.begin/end.
fn bus_events(b: &Boot<'_>) -> Vec<Ev> {
    b.s.board.events_of(|e| {
        matches!(
            e,
            Ev::LineMode(..) | Ev::LineWrite(..) | Ev::WireBegin | Ev::WireEnd
        )
    })
}

/// The four calibrated valves idle at their targets.
fn settled(b: &Boot<'_>) -> bool {
    let m = b.m();
    m.valve_idle()
        && m.mots[..4]
            .iter()
            .all(|v| v.status == ST_IDLE && v.actual_position == v.target_position)
}

#[test]
fn gvers_and_the_banner_report_the_identity_of_the_image() {
    // the parity cases run with the C++ glue_system's identity; an image reports the one of its
    // ID block (D8: the workspace version)
    let mut c = case();
    c.set_id(FirmwareId {
        version: env!("CARGO_PKG_VERSION").as_bytes(),
        tag: b"C1",
        build: b"1",
    });
    let mut b = c.boot(PLAIN, |_| {});
    assert!(b
        .take_dbg()
        .starts_with("VdMot Controller 2.2.0-revamped_C1\r\n"));
    assert_eq!(b.exchange("gvers\n"), "gvers 2.2.0-revamped_C1 1 \r\n");
    assert!(b
        .terminal("gvers\n")
        .contains("Version: 2.2.0-revamped\r\n"));
    b.finish();
}

#[test]
fn a_fault_record_of_the_start_before_follows_the_banner() {
    let mut c = case();
    c.set_fault_record(Some(FaultRecord {
        kind: KIND_HARD_FAULT,
        pc: 0x0800_4321,
        lr: 1,
        xpsr: 2,
        cfsr: 3,
        hfsr: 4,
        bfar: 5,
        count: 6,
    }));
    let mut b = c.boot(PLAIN, |_| {});
    let dbg = b.take_dbg();
    assert!(
        dbg.starts_with(
            "VdMot Controller 2.1.7-revamped_C2\r\nlast fault: HardFault pc 0x8004321 \
             lr 0x1 xpsr 0x2 cfsr 0x3 hfsr 0x4 bfar 0x5 count 6\r\nSERIAL_BUFFER_SIZE"
        ),
        "{dbg}"
    );
    b.finish();
}

#[test]
fn setup_recovers_the_i2c_bus_then_starts_the_driver() {
    let mut c = case();
    let b = c.boot(PLAIN, |_| {});
    let bus = bus_events(&b);
    // i2c_bus_recover(): SDA released first, the lines inputs again at the end; then Wire.begin()
    assert_eq!(bus[0], Ev::LineMode(I2cLine::Sda, LineMode::Input));
    assert_eq!(
        bus[bus.len() - 2],
        Ev::LineMode(I2cLine::Scl, LineMode::Input)
    );
    assert_eq!(bus[bus.len() - 1], Ev::WireBegin);
    assert!(!bus.contains(&Ev::WireEnd));
    assert!(b.s.board.wire_running.get());
    b.finish();
}

#[test]
fn a_failed_eeprom_write_is_retried_after_a_bus_restart() {
    let mut c = case();
    let mut b = c.boot(PLAIN, |_| {});
    // every write fails: the first start's configuration write 3 s after the set-up and its
    // repetitions
    b.eeprom().fail_writes_from = 1;
    b.run_main(8000);
    // eepState 2: the write failed
    assert_eq!(Boot::field(&b.exchange("gstat\n"), 6), 2);
    b.s.board.clear_events();
    // the retry 30 s later starts with i2c_bus_restart(): driver off, recovery, driver on
    b.run_main(31000);
    let bus = bus_events(&b);
    assert_eq!(bus.first(), Some(&Ev::WireEnd));
    assert_eq!(bus.last(), Some(&Ev::WireBegin));
    assert!(bus.contains(&Ev::LineMode(I2cLine::Sda, LineMode::Input)));
    b.eeprom().fail_writes_from = 0;
    b.run_main(70000);
    assert_eq!(b.exchange("eepst\n"), "eepst 1 \r\n");
    b.finish();
}

#[test]
fn receive_errors_of_usart1_reach_gstax() {
    let mut c = case();
    let mut b = c.boot(PLAIN, |_| {});
    b.s.esp.on_irq(SR_RXNE + SR_ORE, b'x');
    b.s.esp.on_irq(SR_RXNE + SR_FE, b'x');
    b.s.esp.on_irq(SR_RXNE + SR_FE, b'x');
    b.s.esp.on_irq(SR_RXNE + SR_NE, b'\n');
    b.run_main(50);
    // uartOre, uartFe, uartNe, rxDropped
    assert_eq!(b.gstax(14), 1);
    assert_eq!(b.gstax(15), 2);
    assert_eq!(b.gstax(16), 1);
    assert_eq!(b.gstax(17), 0);
    b.finish();
}

#[test]
fn escalation_learn_movements_and_motor_values_round_trip() {
    let mut c = case();
    let mut b = c.boot(PLAIN, |_| {});
    b.run_main(5000);
    assert_eq!(b.exchange("scalx 1 10 40\n"), "scalx ok\r\n");
    assert_eq!(b.exchange("gcalx\n"), "gcalx 1 10 40\r\n");
    assert_eq!(b.ctl.modules.eeprom.eep_content.cfg.escalation.step_pct, 10);
    assert_eq!(b.exchange("eepst\n"), "eepst 0 \r\n");
    assert_eq!(b.exchange("stlnm 500\n"), "stlnm\r\n");
    assert_eq!(b.exchange("gtlnm\n"), "gtlnm 500 \r\n");
    assert_eq!(b.exchange("smotc 20 21 40\n"), "smotc\r\n");
    assert_eq!(b.exchange("gmotc\n"), "gmotc 20 21 40 3000 2 \r\n");
    assert_eq!(
        b.ctl.modules.eeprom.eep_content.cfg.layout.start_on_power,
        40
    );
    b.run_main(5000);
    assert_eq!(b.exchange("eepst\n"), "eepst 1 \r\n");
    b.finish();
}

#[test]
fn one_wire_sensors_are_found_matched_to_a_valve_and_read() {
    let mut c = case();
    let mut b = c.boot(sim(0x0FFF), |_| {});
    // the presence tests: the valve machine locks the temperature measurement meanwhile
    b.run_main(40000);
    // 22.5 and 20 degC
    b.ctl.modules.hw.one_wire.add_one_wire(0x28, 1, 2880);
    b.ctl.modules.hw.one_wire.add_one_wire(0x28, 2, 2560);
    assert_eq!(b.exchange("stons\n"), "stons\r\n");
    b.run_main(1000);
    assert_eq!(b.exchange("gonec\n"), "gonec 2 \r\n");
    let list = b.exchange("gonec 255\n");
    let addresses: Vec<String> = list
        .trim_end()
        .split(' ')
        .nth(2)
        .unwrap()
        .split(',')
        .map(String::from)
        .collect();
    assert_eq!(addresses.len(), 2);
    // slot 1 of valve 1 takes sensor 0, slot 2 sensor 1 (the index path)
    assert_eq!(b.exchange("stsnx 1 0\n"), "stsnx\r\n");
    assert_eq!(b.exchange("stsny 1 1\n"), "stsny\r\n");
    assert_eq!(
        b.exchange("gvlon 1\n"),
        std::format!("gvlon 1 {} {} \r\n", addresses[0], addresses[1])
    );
    // valve 0 by address (the EEPROM slots), matched by masns
    let set = std::format!("stvls 0 {} {}\n", addresses[1], addresses[0]);
    assert_eq!(b.exchange(&set), "stvls 0\r\n");
    b.take_dbg();
    assert_eq!(b.exchange("masns\n"), "masns \r\n");
    let dbg = b.take_dbg();
    assert!(dbg.contains("{ 28,  01, "), "{dbg}");
    assert!(
        dbg.contains(" found as 1st sensor at valve: 0:1\r\n"),
        "{dbg}"
    );
    assert!(
        dbg.contains(" found as 2nd sensor at valve: 0\r\n"),
        "{dbg}"
    );
    // a temperature cycle: conversion, then one sensor per call
    b.run_main(3000);
    let data = b.exchange("gvlvd 0\n");
    assert_eq!(Boot::field(&data, 5), 200, "{data}");
    assert_eq!(Boot::field(&data, 6), 225, "{data}");
    // the cycle completed within the last seconds
    assert!(b.gstax(21) <= 2);
    assert!(b.terminal("getone\n").contains("\"cnt\":2"));
    b.finish();
}

#[test]
fn the_stored_escalation_reaches_the_motor_at_the_next_start() {
    let mut c = case();
    let mut b = c.boot(PLAIN, |_| {});
    b.run_main(5000);
    assert_eq!(b.exchange("scalx 1 10 40\n"), "scalx ok\r\n");
    b.run_main(5000);
    assert_eq!(b.exchange("eepst\n"), "eepst 1 \r\n");
    b.reboot(Reset::PowerOn);
    // app_load_config() of the set-up hands it from the EEPROM to the valve state machine
    let mut b = c.boot(PLAIN, |_| {});
    assert_eq!(b.exchange("gcalx\n"), "gcalx 1 10 40\r\n");
    assert_eq!(b.m().motor_get_escalation().step_pct, 10);
    b.finish();
}

#[test]
fn the_sensor_match_prints_outside_the_motor_lock() {
    // design §2.3: the lines of a match (up to 34 sensors, more than the 1023-byte transmit
    // ring) may wait for USART6; TIM1 and TIM2 must run meanwhile, so the 1-Wire search
    // (stons), masns and the EEPROM re-read print them outside the motor lock and take only the
    // sensor indices under it
    let mut c = case();
    let mut b = c.boot(PLAIN, |s| s.eeprom.borrow_mut().fail_reads_from = 1);
    // the set-up matches under the lock before TIM1 and TIM2 run: not counted
    b.take_dbg();
    b.s.dbg_locked.borrow_mut().clear();
    b.ctl.modules.hw.one_wire.add_one_wire(0x28, 1, 2880);
    b.ctl.modules.hw.one_wire.add_one_wire(0x28, 2, 2560);
    assert_eq!(b.exchange("stons\n"), "stons\r\n");
    b.run_main(1000);
    let list = b.exchange("gonec 255\n");
    let addresses: Vec<String> = list
        .trim_end()
        .split(' ')
        .nth(2)
        .unwrap()
        .split(',')
        .map(String::from)
        .collect();
    assert_eq!(addresses.len(), 2);
    let set = std::format!("stvls 0 {} {}\n", addresses[1], addresses[0]);
    assert_eq!(b.exchange(&set), "stvls 0\r\n");
    assert_eq!(b.exchange("masns\n"), "masns \r\n");
    // the EEPROM answers again: the re-read 30 s after the failed one matches once more
    b.eeprom().fail_reads_from = 0;
    b.run_main(31000);
    let dbg = b.take_dbg();
    assert_eq!(
        dbg.matches("Read 1-wire sensor addresses from eeprom\r\n")
            .count(),
        3,
        "{dbg}"
    );
    assert_eq!(
        dbg.matches(" found as 1st sensor at valve: 0:1\r\n")
            .count(),
        2,
        "{dbg}"
    );
    let locked = String::from_utf8_lossy(&b.s.dbg_locked.borrow()).into_owned();
    assert!(!locked.contains("1-wire"), "{locked}");
    assert!(!locked.contains("{ 28"), "{locked}");
    assert!(!locked.contains(" found as "), "{locked}");
    assert!(!locked.contains(" not found"), "{locked}");
    // the rest of the re-read still runs under the lock (and the log sees it)
    assert!(locked.contains("learning_movements: "), "{locked}");
    // the indices reached valve 0: its temperatures after a temperature cycle
    b.run_main(3000);
    let data = b.exchange("gvlvd 0\n");
    assert_eq!(Boot::field(&data, 5), 200, "{data}");
    assert_eq!(Boot::field(&data, 6), 225, "{data}");
    b.finish();
}

#[test]
fn the_terminal_reaches_the_modules() {
    let mut c = case();
    let mut b = c.boot(sim(0x0FF0), |_| {});
    b.run_main(40000);
    b.calibrated_idle(4);
    assert!(b
        .terminal("gvers\n")
        .contains("Version: 2.1.7-revamped\r\n"));
    assert!(b
        .terminal("stlnt 100\n")
        .contains("set valve learning time to 100\r\n"));
    assert_eq!(b.exchange("gtlnt\n"), "gtlnt 100\r\n");
    assert!(b
        .terminal("settar 3 40\n")
        .contains("set valve 3 to 40\r\n"));
    assert_eq!(b.exchange("gtgtp 3\n"), "gtgtp 3 40 \r\n");
    assert!(b
        .terminal("staop 2\n")
        .contains("got open valve request for 2\r\n"));
    assert_eq!(b.exchange("gtgtp 2\n"), "gtgtp 2 100 \r\n");
    assert!(b.terminal("smotc 20 21\n").contains("- valid\r\n"));
    assert_eq!(b.exchange("gmotc\n"), "gmotc 20 21 30 3000 2 \r\n");
    assert!(b
        .terminal("gmotc\n")
        .contains("got get motor characteristics request - low: 20 high: \r\n21\r\n"));
    assert!(b.terminal("getone\n").contains("{\"cnt\":0}\r\n"));
    assert!(b.terminal("stons\n").contains("start new 1-wire search"));
    assert!(b
        .terminal("gvlon 0\n")
        .contains(" - gvlon 0 00-00-00-00-00-00-00-00 00-00-00-00-00-00-00-00 \r\n"));
    assert!(b.terminal("seteep\n").contains("set eeprom layout\r\n"));
    assert_eq!(
        &b.ctl.modules.eeprom.eep_content.cfg.layout.descr[..16],
        b"VdMot Controller"
    );
    assert_eq!(b.exchange("eepst\n"), "eepst 0 \r\n");
    assert!(b.terminal("saveep\n").contains("saved eeprom layout\r\n"));
    assert!(b
        .terminal("stsnx 0 0\n")
        .contains("invalid valve or sensor index\r\n"));
    assert!(b
        .terminal("stvls 0 00-00-00-00-00-00-00-00 00-00-00-00-00-00-00-00\n")
        .contains("Read 1-wire sensor addresses from eeprom\r\n"));
    assert!(b.run_main_until(settled, 60000));
    // smux and sdir while the valve machine is idle (C2: MUX on is low)
    b.terminal("smux 1\n");
    assert!(!b.s.board.out(Out::Mux));
    b.terminal("smux 0\n");
    assert!(b.s.board.out(Out::Mux));
    b.terminal("sdir 1\n");
    assert!(b.s.board.out(Out::Dir));
    assert!(b
        .terminal("stdet 255\n")
        .contains(" - reset all valves\r\nstdet \r\n"));
    b.run_main(30);
    for v in 0..4 {
        assert_eq!(b.m().mots[v].status, ST_UNKNOWN, "valve {v}");
    }
    assert!(b
        .terminal("staln 1\n")
        .contains("start learning for valve 1\r\n"));
    assert_ne!(b.gvlvx(1, 11) & 3, 0);
    b.finish();
}

#[test]
fn the_valve_machine_takes_open_and_learn_from_the_terminal() {
    let mut c = case();
    let mut b = c.boot(sim(0x0FF0), |_| {});
    b.run_main(40000);
    b.calibrated_idle(4);
    let from = b.sim().ms;
    b.terminal("open 0 10\n");
    assert!(b.run_main_until(|b| b.first_status(0, ST_OPENING, from) != 0, 5000));
    assert!(b.run_main_until(settled, 60000));
    b.terminal("learn 1\n");
    assert!(b.run_main_until(|b| b.m().mots[1].calib_active, 10000));
    assert!(b.run_main_until(|b| !b.m().mots[1].calib_active, 60000));
    assert_eq!(b.m().mots[1].calib_seq, 1);
    b.finish();
}

#[test]
fn sena_holds_the_valve_machine_for_2_s() {
    let mut c = case();
    let mut b = c.boot(sim(0x0FF0), |_| {});
    b.run_main(40000);
    b.calibrated_idle(4);
    // the valve on ENA1 with the MUX off (C2: high) is valve 3: without a motor, no current
    b.sim().valve[3].connected = false;
    b.terminal("smux 0\n");
    let enables = b.sim().valve[0].enables;
    let start = b.s.board.now_us();
    assert_eq!(b.terminal("sena 1 1\n"), "");
    assert!(b.s.board.out(Out::Ena1));
    assert!(!b.s.board.out(Out::PsuEna));
    // a target change waits while the output is on from the terminal
    assert_eq!(b.exchange("stgtp 0 70\n"), "stgtp\r\n");
    b.run_main(1500);
    assert!(b.idle());
    assert_eq!(b.sim().valve[0].enables, enables);
    assert!(b.run_main_until(|b| !b.s.board.out(Out::Ena1), 1000));
    let on = b.s.board.now_us() - start;
    assert!((1_900_000..2_200_000).contains(&on), "{on}");
    assert!(b.take_dbg().contains("sena: output off\r\n"));
    // then the valve machine moves valve 0
    assert!(b.run_main_until(|b| b.idle_at(0, 70), 20000));
    assert!(b.sim().valve[0].enables > enables);
    b.finish();
}

#[test]
fn sena_ends_at_the_current_limit() {
    let mut c = case();
    let mut b = c.boot(sim(0x0FF0), |_| {});
    b.run_main(40000);
    b.calibrated_idle(4);
    // the valve on ENA1 with the MUX off (C2: high) is valve 3: shorted, 200 mA
    b.sim().valve[3].shorted = true;
    b.terminal("smux 0\n");
    // switched off by the 60 mA limit within the 150 ms of the terminal line
    assert!(b.terminal("sena 1 1\n").contains("sena: output off\r\n"));
    assert!(!b.s.board.out(Out::Ena1));
    assert!(b.s.board.out(Out::PsuEna));
    b.finish();
}

#[test]
fn the_led_blinks_and_the_button_is_reported() {
    let mut c = case();
    let mut b = c.boot(PLAIN, |_| {});
    b.s.board.clear_events();
    b.run_main(3200);
    assert_eq!(b.s.board.writes_of(Out::Led), std::vec![false, true]);
    b.take_dbg();
    b.s.board.set_input(In::Button, true);
    b.run_main(250);
    assert!(b.take_dbg().contains("Button pressed\r\n"));
    b.finish();
}

#[test]
fn the_hal_methods_of_the_contexts_reach_the_board() {
    let mut c = case();
    let mut b = c.boot(PLAIN, |_| {});
    let board = b.s.board.clone();
    let modules = &mut b.ctl.modules;
    // the main loop's pins and clock
    board.set_latch(Out::Led, true);
    assert!(Pins::latch(modules, Out::Led));
    board.set_latch(Out::Led, false);
    assert!(!Pins::latch(modules, Out::Led));
    let t = board.now_us();
    modules.delay_us(7);
    assert_eq!(board.now_us(), t + 7);
    assert_eq!(Clock::micros(modules), (t + 7) as u32);
    // app.cpp's context
    let mut env = AppCtx::<Bench> {
        board: modules.hw.board,
        noinit: modules.hw.noinit,
        dbg: modules.hw.dbg,
        eeprom: &mut modules.eeprom,
        sysstat: &modules.sysstat,
        sensors: &modules.ow.sensors,
        manual: true,
    };
    env.delay_ms(2);
    env.delay_us(3);
    assert_eq!(board.now_us(), t + 2010);
    assert_eq!(env.micros(), (t + 2010) as u32);
    assert_eq!(env.millis(), ((t + 2010) / 1000) as u32);
    assert_eq!(env.dev_id(), 0x423);
    assert!(env.terminal_manual_active());
    assert_eq!(env.ds18_address(40), [0; 8]);
    // owDevices.cpp's lock
    let flags = IsrFlags::new();
    let mut ow = OwCtx::new(&flags);
    modules.ow.temp_command(TEMP_CMD_LOCK, &mut ow);
    assert!(flags.temp_lock.load(Ordering::SeqCst));
    assert!(ow.temp_locked());
    modules.ow.temp_command(TEMP_CMD_UNLOCK, &mut ow);
    assert!(!flags.temp_lock.load(Ordering::SeqCst));
    assert!(!ow.temp_locked());
    modules.ow.temp_command(TEMP_CMD_NEWSEARCH, &mut ow);
    assert!(!ow.match_sensors && !ow.cycle_done);
    b.finish();
}

// Cases of the mutation gate of system.rs: delegations the cases above do not observe.

#[test]
fn shorts_on_three_valves_suspend_the_limits_and_gstax_reports_it() {
    let mut c = case();
    let mut b = c.boot(sim(0x0FF0), |_| {});
    // shorted motors on valves 0..2 (the short limit: 2000 dmA filtered, for 3 samples)
    for v in 0..3 {
        let mut s = b.sim();
        s.valve[v].shorted = true;
        s.valve[v].short_current_dma = 2600;
    }
    // the presence tests report the shorts: trips of three valves within 600 s
    b.run_main(40000);
    // sysFlags bit 0: the short and inrush limits are suspended
    assert_eq!(b.gstax(23), 1);
    b.finish();
}

/// Valve v's next move, to `pct`, with an inrush above the limit (reported only, the move goes
/// on): one trip; the inrush is gone afterwards.
fn inrush_move(b: &mut Boot<'_>, v: usize, pct: u8) {
    {
        let mut s = b.sim();
        s.valve[v].inrush_peak_dma = 2800;
        s.valve[v].inrush_ms = 60;
    }
    let trips = b.m().mots[v].trip_seq;
    let set = std::format!("stgtp {v} {pct}\n");
    assert_eq!(b.exchange(&set), "stgtp\r\n");
    assert!(b.run_main_until(|b| b.idle_at(v, pct), 60000), "valve {v}");
    assert_eq!(b.m().mots[v].trip_seq, trips.wrapping_add(1), "valve {v}");
    b.sim().valve[v].inrush_peak_dma = 0;
    // the app hands the trip to the protection guard
    b.run_main(100);
}

#[test]
fn inrush_trips_on_three_valves_count_for_the_guard_only_within_600_s_of_uptime() {
    let mut c = case();
    let mut b = c.boot(sim(0x0FF0), |_| {});
    b.run_main(40000);
    b.calibrated_idle(4);
    assert!(b.run_main_until(settled, 60000));
    // the lease off: no failsafe while the time jumps
    assert_eq!(b.exchange("slcfg 0\n"), "slcfg ok\r\n");
    // three valves, each trip more than 600 s of uptime after the one before
    for v in 0..3 {
        if v > 0 {
            b.jump_s(700);
            assert!(b.run_main_until(settled, 60000));
        }
        inrush_move(&mut b, v, 70);
        assert_eq!(b.gstax(23), 0, "valve {v}");
    }
    // two more within 600 s of the last one: three valves within the window
    inrush_move(&mut b, 0, 40);
    assert_eq!(b.gstax(23), 0);
    inrush_move(&mut b, 1, 40);
    assert_eq!(b.gstax(23), 1);
    b.finish();
}

#[test]
fn gprof_reports_the_last_move_and_the_terminal_waits_for_the_valve_machine() {
    let mut c = case();
    let mut b = c.boot(sim(0x0FF0), |_| {});
    b.run_main(40000);
    b.calibrated_idle(4);
    assert!(b.run_main_until(settled, 60000));
    assert_eq!(b.exchange("stgtp 0 70\n"), "stgtp\r\n");
    assert!(b.run_main_until(|b| !b.idle(), 1000));
    // smux and sdir are refused while the valve machine works (it waits 1 s for the PSU)
    let mux = b.s.board.out(Out::Mux);
    assert!(b.terminal("smux 1\n").contains("valve machine busy\r\n"));
    assert_eq!(b.s.board.out(Out::Mux), mux);
    assert!(b.run_main_until(|b| b.idle_at(0, 70), 30000));
    let reply = b.exchange("gprof 0\n");
    assert!(Boot::field(&reply, 2) > 1, "{reply}");
    b.finish();
}

#[test]
fn sena_is_refused_in_safe_mode() {
    let mut c = case();
    for _ in 0..3 {
        let b = c.boot(PLAIN, |_| {});
        b.reboot(Reset::Watchdog);
    }
    let mut b = c.boot(PLAIN, |_| {});
    // three watchdog resets within 10 min: safe mode, the valve machine idles
    assert_eq!(b.gstax(12), 1);
    assert!(b.terminal("sena 1 1\n").contains("valve machine busy\r\n"));
    assert!(!b.s.board.out(Out::Ena1));
    b.finish();
}

#[test]
fn the_terminal_searches_the_1_wire_bus_and_assigns_its_sensors() {
    let mut c = case();
    let mut b = c.boot(sim(0x0FFF), |_| {});
    // the presence tests: the valve machine locks the temperature measurement meanwhile
    b.run_main(40000);
    b.ctl.modules.hw.one_wire.add_one_wire(0x28, 1, 2880);
    b.ctl.modules.hw.one_wire.add_one_wire(0x28, 2, 2560);
    assert!(b.terminal("stons\n").contains("start new 1-wire search"));
    b.run_main(1000);
    assert_eq!(b.exchange("gonec\n"), "gonec 2 \r\n");
    // slot 1 of valve 0 takes sensor 1
    let out = b.terminal("stsnx 0 1\n");
    assert!(out.contains("comm: set 1st sensor index\r\n"), "{out}");
    assert!(!out.contains("invalid valve or sensor index"), "{out}");
    assert_eq!(b.m().valves[0].sensorindex1, 1);
    // valve 12 does not exist
    let out = b.terminal("stvls 12 00-00-00-00-00-00-00-00 00-00-00-00-00-00-00-00\n");
    assert!(out.contains("invalid valve index\r\n"), "{out}");
    assert!(!out.contains("Read 1-wire sensor addresses"), "{out}");
    b.finish();
}

#[test]
fn seteep_is_blocked_while_the_eeprom_cannot_be_read() {
    let mut c = case();
    let mut b = c.boot(PLAIN, |s| s.eeprom.borrow_mut().fail_reads_from = 1);
    let out = b.terminal("seteep\n");
    assert!(
        out.contains("eeprom not readable, write blocked\r\n"),
        "{out}"
    );
    assert!(!out.contains("set eeprom layout"), "{out}");
    b.finish();
}

#[test]
fn saveep_schedules_an_eeprom_write() {
    let mut c = case();
    let mut b = c.boot(PLAIN, |_| {});
    // the first start's configuration write
    b.run_main(5000);
    assert_eq!(b.exchange("eepst\n"), "eepst 1 \r\n");
    assert!(b.terminal("saveep\n").contains("saved eeprom layout\r\n"));
    assert_eq!(b.exchange("eepst\n"), "eepst 0 \r\n");
    b.run_main(5000);
    assert_eq!(b.exchange("eepst\n"), "eepst 1 \r\n");
    b.finish();
}

#[test]
fn the_10_s_branch_runs_the_learn_time_trigger() {
    let mut c = case();
    let mut b = c.boot(PLAIN, |_| {});
    // learn times of 1..12 s over the valves
    b.exchange("stlnt 12\n");
    b.take_dbg();
    b.run_main(12000);
    let dbg = b.take_dbg();
    assert!(
        dbg.contains("App: Valve 0 will be learned soon\r\n"),
        "{dbg}"
    );
    b.finish();
}
