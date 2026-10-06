// Port of software_stm32/test/native/glue/test_app.cpp (glue_app): start values, configuration
// load, the valve walk of app_loop, the 10 s countdowns, the setters and the soft reset. Protocol
// 3 (lease, failsafe, retries, warm restore, stop, protection guard) in tests_v3.rs.
use std::string::String;
use std::vec::Vec;

use vdm_stm_core::valve_codes::{ST_IDLE, ST_UNKNOWN};

use crate::motor::CalibState;
use crate::test_support::fake_board::{expect_panic, SystemReset};

use super::stub_env::AppBench;
use super::{LEARN_AFTER_TIME_DEFAULT, VALVE_NO_TARGET, VALVE_SENSOR_UNKNOWN};

fn calls(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| String::from(*s)).collect()
}

/// glue::begin(), app_setup(), the call log and the debug output cleared
fn begin() -> AppBench {
    let mut b = AppBench::new();
    b.app_setup();
    b.env.log.clear();
    b.env.take_tx();
    b
}

#[test]
fn app_setup_start_values_of_all_12_valves_the_stored_configuration_loaded() {
    let mut b = AppBench::new();
    assert_eq!(b.app_setup(), 0);
    for v in 0..12 {
        let mot = &b.m.mots[v];
        let valve = &b.m.valves[v];
        assert_eq!(mot.target_position, 50, "valve {v}");
        assert_eq!(mot.actual_position, 50, "valve {v}");
        assert_eq!(mot.status, ST_UNKNOWN, "valve {v}");
        assert!(!mot.calibration, "valve {v}");
        assert_eq!(valve.sensorindex1, VALVE_SENSOR_UNKNOWN, "valve {v}");
        assert_eq!(valve.sensorindex2, VALVE_SENSOR_UNKNOWN, "valve {v}");
        assert_eq!(valve.rejected_target, VALVE_NO_TARGET, "valve {v}");
        // the learn time is spread over the valves; the stored learn movements 0 switch the
        // trigger off
        assert_eq!(
            valve.learn_time,
            LEARN_AFTER_TIME_DEFAULT / 12 * (v as u32 + 1),
            "valve {v}"
        );
        assert_eq!(valve.learn_movements, 0, "valve {v}");
    }
    assert_eq!(b.m.learning_movements, 0);
    // an all-zero EEPROM mirror: the factors are out of range and load their defaults
    assert_eq!(
        b.env.log.calls,
        calls(&[
            "motor_set_params(17, 17, 0, 0, 0)",
            "motor_set_escalation(0, 25, 50)",
            "eeprom_lease_source()"
        ])
    );
    assert_eq!(
        b.env.take_tx(),
        "Read 1-wire sensor addresses from eeprom\r\nlearning_movements: 0\r\n"
    );
}

#[test]
fn app_match_sensors_stored_sensor_addresses_select_the_index_of_the_found_sensor() {
    let mut b = AppBench::new();
    let other = [0x28, 9, 9, 9, 9, 9, 9, 0x11];
    let a = [0x28, 1, 2, 3, 4, 5, 6, 0x77];
    b.env.ds18_count = 2;
    b.env.tempsensors[0] = other;
    b.env.tempsensors[1] = a;
    let slot = &mut b.env.eep.layout.owsensors2[4];
    slot.familycode = 0x28;
    slot.romcode = [1, 2, 3, 4, 5, 6];
    slot.crc = 0x77;
    assert_eq!(b.app_match_sensors(), 0);
    assert_eq!(b.m.valves[4].sensorindex2, 1);
    assert_eq!(b.m.valves[4].sensorindex1, VALVE_SENSOR_UNKNOWN);
    assert_eq!(b.m.valves[3].sensorindex2, VALVE_SENSOR_UNKNOWN);
}

#[test]
fn app_loop_tests_the_unknown_valves_in_the_order_0_2_4_10_1_3_11_0() {
    let mut b = begin();
    for _ in 0..13 {
        b.app_loop();
    }
    assert_eq!(
        b.env.log.calls_of("appsetaction"),
        calls(&[
            "appsetaction(x, 0, 0, 0)",
            "appsetaction(x, 2, 0, 0)",
            "appsetaction(x, 4, 0, 0)",
            "appsetaction(x, 6, 0, 0)",
            "appsetaction(x, 8, 0, 0)",
            "appsetaction(x, 10, 0, 0)",
            "appsetaction(x, 1, 0, 0)",
            "appsetaction(x, 3, 0, 0)",
            "appsetaction(x, 5, 0, 0)",
            "appsetaction(x, 7, 0, 0)",
            "appsetaction(x, 9, 0, 0)",
            "appsetaction(x, 11, 0, 0)",
            "appsetaction(x, 0, 0, 0)"
        ])
    );
    assert_eq!(b.m.valves[0].rejected_target, 50);
}

#[test]
fn app_loop_nothing_while_the_valve_machine_is_busy_or_the_terminal_drives_a_motor() {
    let mut b = begin();
    b.env.idle = false;
    b.app_loop();
    b.env.idle = true;
    b.env.manual_active = true;
    b.app_loop();
    assert!(b.env.log.calls_of("appsetaction").is_empty());
    assert_eq!(
        b.env.log.calls,
        calls(&[
            "terminal_manual_active()",
            "valve_idle()",
            "terminal_manual_active()"
        ])
    );
}

#[test]
fn app_loop_a_known_valve_away_from_its_target_moves_by_the_difference() {
    let mut b = begin();
    for mot in &mut b.m.mots {
        mot.status = ST_IDLE;
        mot.calibrated = true;
    }
    b.m.mots[0].target_position = 70;
    b.m.mots[1].target_position = 100;
    b.m.mots[2].target_position = 20;
    b.m.mots[3].target_position = 0;
    for _ in 0..4 {
        b.app_loop();
    }
    assert_eq!(
        b.env.log.calls_of("appsetaction"),
        calls(&[
            "appsetaction(o, 0, 20, 0)",
            "appsetaction(p, 1, 0, 0)",
            "appsetaction(c, 2, 30, 0)",
            "appsetaction(v, 3, 0, 0)"
        ])
    );
}

#[test]
fn app_10s_loop_the_learn_time_counts_down_10_s_per_call_and_triggers_at_10_or_less() {
    let mut b = begin();
    b.app_set_learntime(120);
    assert_eq!(b.m.valves[0].learn_time, 10);
    assert_eq!(b.m.valves[11].learn_time, 120);
    b.app_10s_loop(10);
    assert!(b.m.valves[0].timed_learn);
    assert_eq!(b.m.valves[0].learn_time, 120);
    assert_eq!(b.m.valves[11].learn_time, 110);
    assert!(!b.m.valves[11].timed_learn);
}

#[test]
fn app_10s_loop_the_movement_trigger_starts_a_calibration_of_a_connected_valve_only() {
    let mut b = begin();
    b.app_set_learnmovements(50);
    b.m.valves[2].learn_movements = 0;
    b.m.valves[3].learn_movements = 0;
    b.m.mots[2].connected = true;
    b.app_10s_loop(10);
    assert!(b.m.mots[2].calibration);
    assert_eq!(b.m.mots[2].calib_state, CalibState::Started);
    assert_eq!(b.m.valves[2].learn_movements, 50);
    assert!(!b.m.mots[3].calibration);
}

#[test]
fn setters_valve_11_and_255_are_taken_12_is_refused() {
    let mut b = begin();
    assert_eq!(b.app_set_valveopen(11), 0);
    assert_eq!(b.m.mots[11].target_position, 100);
    assert!(b.m.valves[11].open_request);
    assert_eq!(b.app_set_valveopen(12), -1);
    assert_eq!(b.app_set_valveopen(255), 0);
    assert_eq!(b.m.mots[0].target_position, 100);
    assert_eq!(b.app_set_valvelearning(12), -1);
    assert_eq!(b.app_set_valvelearning(11), 0);
    assert!(b.m.valves[11].forced_learn);
    assert_eq!(b.app_service_move(12, 0, 100, 20), -1);
    // the learn request of valve 11 is pending
    assert_eq!(b.app_service_move(11, 0, 100, 20), -3);
    assert_eq!(b.app_service_move(10, 0, 100, 20), 0);
    assert_eq!(
        b.env.log.calls_of("appsetservice"),
        calls(&["appsetservice(10, 0, 100, 20)"])
    );
}

#[test]
fn reset_check_the_soft_reset_waits_for_the_eeprom_then_resets_the_controller() {
    let mut b = begin();
    b.app.reset_stm32(&mut b.env);
    b.env.free = false;
    b.app_loop();
    b.env.free = true;
    assert!(expect_panic::<SystemReset>(|| {
        b.app_loop();
    }));
    assert_eq!(
        b.env.take_tx(),
        "prepare for soft reset\r\nApp: valve 0 unknown, try to find out...\r\nsoft reset now\r\n"
    );
}
