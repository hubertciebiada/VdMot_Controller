// Port of software_stm32/test/native/glue/test_app__mut.cpp (glue_app): return values, the debug
// lines of two-digit valves, the 10 s backstop and countdowns, the setters for one and all valves,
// sensor matching, warm copies of the lease client and the retry, calibration requests of a
// shorted valve, the retry pause while a valve is busy.
use std::format;
use std::string::String;
use std::vec::Vec;

use vdm_stm_core::config_blocks::{CalibRecord, CALIB_VALID};
use vdm_stm_core::system_stats::BootReason;
use vdm_stm_core::valve_codes::{
    ValveFault, ST_BLOCKED, ST_FAILED, ST_IDLE, ST_UNKNOWN, VLV_FLAG_RETRY,
};

use crate::motor::CalibState;

use super::stub_env::AppBench;
use super::{
    CALIB_START_TICKS, LEARN_AFTER_TIME_DEFAULT, RETEST_TEST, SVMOV_HOLD_10S, VALVE_SENSOR_UNKNOWN,
};

fn calls(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| String::from(*s)).collect()
}

/// app_setup: every valve UNKNOWN
fn begin_cold() -> AppBench {
    let mut b = AppBench::new();
    b.app_setup();
    b.env.log.clear();
    b.env.take_tx();
    b
}

/// app_setup, then every valve idle, calibrated, connected and at its target 50
fn begin() -> AppBench {
    let mut b = begin_cold();
    for mot in &mut b.m.mots {
        mot.status = ST_IDLE;
        mot.calibrated = true;
        mot.connected = true;
        mot.scaler = 36;
    }
    b
}

#[test]
fn app_loop_and_app_set_learnmovements_return_0() {
    let mut b = begin_cold();
    assert_eq!(b.app_loop(), 0);
    b.env.idle = false;
    assert_eq!(b.app_loop(), 0);
    b.env.idle = true;
    b.env.manual_active = true;
    assert_eq!(b.app_loop(), 0);
    assert_eq!(b.app_set_learnmovements(60), 0);
}

#[test]
fn app_setup_the_learn_time_spread_stays_when_the_eeprom_holds_the_default() {
    let mut b = AppBench::new();
    b.env.eep.learn_time_s = LEARN_AFTER_TIME_DEFAULT;
    b.app_setup();
    for v in 0..12u32 {
        assert_eq!(
            b.m.valves[v as usize].learn_time,
            LEARN_AFTER_TIME_DEFAULT * (v + 1) / 12,
            "valve {v}"
        );
    }
}

#[test]
fn app_loop_debug_presence_tests_of_valves_9_and_10_name_them_in_decimal() {
    let mut b = begin_cold();
    for _ in 0..12 {
        b.app_loop();
    }
    let mut want = String::new();
    for v in [0, 2, 4, 6, 8, 10, 1, 3, 5, 7, 9, 11] {
        want += &format!("App: valve {v} unknown, try to find out...\r\n");
    }
    assert_eq!(b.env.take_tx(), want);
}

#[test]
fn app_loop_debug_the_calibration_start_of_valve_10_the_time_trigger_is_cleared() {
    let mut b = begin();
    assert_eq!(b.app_set_valvelearning(10), 0);
    b.m.valves[10].timed_learn = true;
    for _ in 0..3 {
        b.app_loop();
    }
    assert_eq!(
        b.env.log.calls_of("appsetaction"),
        calls(&["appsetaction(l, 10, 0, 0)"])
    );
    assert_eq!(b.env.take_tx(), "App: learning started for valve 10\r\n");
    assert!(!b.m.valves[10].timed_learn);
    assert!(!b.m.valves[10].forced_learn);
}

#[test]
fn app_10s_loop_the_time_trigger_names_valves_0_10_in_decimal() {
    let mut b = begin_cold();
    b.app_set_learntime(120);
    b.app_10s_loop(110);
    let mut want = String::new();
    for v in 0..=10 {
        want += &format!("App: Valve {v} will be learned soon\r\n");
    }
    assert_eq!(b.env.take_tx(), want);
    assert!(b.m.valves[10].timed_learn);
    assert!(!b.m.valves[11].timed_learn);
}

#[test]
fn app_10s_loop_a_learn_time_of_1_s_counts_down() {
    let mut b = begin_cold();
    b.app_set_learntime(1);
    b.app_10s_loop(10);
    assert!(b.m.valves[0].timed_learn);
}

#[test]
fn app_warm_moving_an_invalid_warm_record_stays_invalid() {
    let mut b = begin();
    b.app_warm_moving(0);
    b.env.reason = BootReason::Software;
    b.m.mots[0].actual_position = 33;
    b.app_restore();
    assert_eq!(b.m.mots[0].actual_position, 33);
    assert_eq!(b.app.app_lease_timeout(), 60);
}

#[test]
fn app_10s_loop_a_running_calibration_satisfies_the_time_trigger() {
    let mut b = begin();
    b.app_set_learntime(120);
    b.m.mots[0].calib_active = true;
    b.app_10s_loop(10);
    assert!(!b.m.valves[0].timed_learn);
    assert_eq!(b.m.valves[0].learn_time, 120);
}

#[test]
fn app_10s_loop_the_movement_trigger_of_valves_0_9_and_10() {
    let mut b = begin_cold();
    b.app_set_learnmovements(50);
    for v in [0, 9, 10] {
        b.m.valves[v].learn_movements = 0;
        b.m.valves[v].movements = 5;
        b.m.mots[v].connected = true;
    }
    b.app_10s_loop(10);
    assert_eq!(
        b.env.take_tx(),
        "App: Valve 0 will be learned soon\r\nApp: Valve 9 will be learned soon\r\n\
         App: Valve 10 will be learned soon\r\n"
    );
    for v in [0, 9, 10] {
        let mot = &b.m.mots[v];
        assert!(mot.calibration, "valve {v}");
        assert_eq!(mot.calib_time, 10, "valve {v}");
        assert_eq!(mot.calib_state, CalibState::Started, "valve {v}");
        assert_eq!(b.m.valves[v].movements, 0, "valve {v}");
        assert_eq!(b.m.valves[v].learn_movements, 50, "valve {v}");
    }
}

#[test]
fn app_10s_loop_movements_0_switch_the_trigger_off_1_is_a_trigger() {
    let mut b = begin_cold();
    b.app_set_learnmovements(0);
    b.m.mots[2].connected = true;
    b.app_10s_loop(10);
    assert!(!b.m.mots[2].calibration);
    b.app_set_learnmovements(1);
    b.m.valves[2].learn_movements = 0;
    b.app_10s_loop(10);
    assert!(b.m.mots[2].calibration);
}

#[test]
fn app_10s_loop_service_holds_count_down_the_calibration_backstop() {
    let mut b = begin_cold();
    b.m.valves[0].svc_hold = 2;
    b.m.valves[11].svc_hold = 2;
    b.m.mots[3].calib_state = CalibState::InProgress;
    b.m.mots[3].calibration = true;
    b.m.mots[3].calib_active = false;
    b.m.mots[3].calib_time = 2;
    b.m.mots[4].calib_state = CalibState::InProgress;
    b.m.mots[4].calibration = true;
    b.m.mots[4].calib_active = true;
    b.m.mots[4].calib_time = 0;
    b.m.mots[5].calib_state = CalibState::Started;
    b.m.mots[5].calibration = true;
    b.m.mots[5].calib_time = 2;
    b.app_10s_loop(10);
    assert_eq!(b.m.valves[0].svc_hold, 1);
    assert_eq!(b.m.valves[11].svc_hold, 1);
    assert_eq!(b.m.mots[3].calib_time, 1);
    assert_eq!(b.m.mots[4].calib_time, CALIB_START_TICKS);
    assert_eq!(b.m.mots[5].calib_time, 2);
    b.app_10s_loop(10);
    assert_eq!(b.m.valves[0].svc_hold, 0);
    assert_eq!(b.m.mots[3].calib_time, 0);
    assert!(b.m.mots[3].calibration);
    assert_eq!(b.m.mots[3].calib_state, CalibState::InProgress);
    b.app_10s_loop(10);
    assert_eq!(b.m.valves[0].svc_hold, 0);
    assert_eq!(b.m.mots[3].calib_time, 0);
    assert!(!b.m.mots[3].calibration);
    assert_eq!(b.m.mots[3].calib_state, CalibState::Idle);
    assert!(b.m.mots[4].calibration);
    assert!(b.m.mots[5].calibration);
    assert_eq!(b.m.mots[5].calib_state, CalibState::Started);
}

#[test]
fn app_set_valvelearning_one_valve_and_all_connected_valves() {
    let mut b = begin_cold();
    for v in [0, 3, 11] {
        b.m.valves[v].svc_hold = 5;
        b.m.valves[v].movements = 7;
        b.m.mots[v].connected = true;
    }
    b.app_set_learnmovements(60);
    assert_eq!(b.m.valves[0].movements, 0);
    assert_eq!(b.m.valves[11].movements, 0);
    b.m.valves[3].movements = 7;
    assert_eq!(b.app_set_valvelearning(3), 0);
    assert!(b.m.valves[3].forced_learn);
    assert_eq!(b.m.valves[3].svc_hold, 0);
    assert!(b.m.mots[3].calibration);
    assert_eq!(b.m.mots[3].calib_state, CalibState::Started);
    assert_eq!(b.m.mots[3].calib_time, 10);
    assert_eq!(b.m.valves[3].movements, 0);
    b.m.valves[0].movements = 7;
    b.m.valves[11].movements = 7;
    assert_eq!(b.app_set_valvelearning(255), 0);
    for v in [0, 11] {
        assert!(b.m.valves[v].forced_learn, "valve {v}");
        assert_eq!(b.m.valves[v].svc_hold, 0, "valve {v}");
        assert!(b.m.mots[v].calibration, "valve {v}");
        assert_eq!(b.m.mots[v].calib_state, CalibState::Started, "valve {v}");
        assert_eq!(b.m.mots[v].calib_time, 10, "valve {v}");
        assert_eq!(b.m.valves[v].movements, 0, "valve {v}");
        assert_eq!(b.m.valves[v].learn_movements, 60, "valve {v}");
    }
    // not connected
    assert!(!b.m.valves[1].forced_learn);
}

#[test]
fn app_set_valveopen_255_every_valve_gets_the_request_the_service_hold_ends() {
    let mut b = begin_cold();
    b.m.valves[0].svc_hold = 5;
    b.m.valves[11].svc_hold = 5;
    assert_eq!(b.app_set_valveopen(255), 0);
    for v in [0, 11] {
        assert!(b.m.valves[v].open_request, "valve {v}");
        assert_eq!(b.m.valves[v].svc_hold, 0, "valve {v}");
        assert!(b.m.valves[v].assembly_hold, "valve {v}");
    }
}

#[test]
fn app_match_sensors_every_byte_of_the_rom_code_counts_the_debug_lines() {
    let mut b = AppBench::new();
    let exact = [0x28, 1, 2, 3, 4, 5, 6, 0x77];
    let last = [0x28, 1, 2, 3, 4, 5, 9, 0x77];
    let first = [0x28, 9, 2, 3, 4, 5, 6, 0x77];
    let second = [0x28, 7, 7, 7, 7, 7, 7, 0x55];
    let none = [0x28, 1, 1, 1, 1, 1, 1, 0x99];
    b.env.ds18_count = 5;
    b.env.tempsensors[0] = exact;
    b.env.tempsensors[1] = last;
    b.env.tempsensors[2] = first;
    b.env.tempsensors[3] = second;
    b.env.tempsensors[4] = none;
    let slot = &mut b.env.eep.layout.owsensors1[0];
    slot.familycode = 0x28;
    slot.romcode = [1, 2, 3, 4, 5, 6];
    slot.crc = 0x77;
    let slot = &mut b.env.eep.layout.owsensors2[11];
    slot.familycode = 0x28;
    slot.romcode = [7; 6];
    slot.crc = 0x55;
    b.m.valves[0].sensorindex2 = 4;
    b.env.take_tx();
    assert_eq!(b.app_match_sensors(), 0);
    assert_eq!(b.m.valves[0].sensorindex1, 0);
    assert_eq!(b.m.valves[0].sensorindex2, VALVE_SENSOR_UNKNOWN);
    assert_eq!(b.m.valves[11].sensorindex2, 3);
    assert_eq!(b.m.valves[11].sensorindex1, VALVE_SENSOR_UNKNOWN);
    assert_eq!(
        b.env.take_tx(),
        "Read 1-wire sensor addresses from eeprom\r\n \
         found as 1st sensor at valve: 0:0\r\n \
         not found\r\n \
         not found\r\n \
         found as 2nd sensor at valve: 11\r\n \
         not found\r\n"
    );
}

#[test]
fn reset_stm32_sets_the_request_flag_to_1() {
    let mut b = begin_cold();
    b.app.reset_stm32(&mut b.env);
    assert_eq!(b.app.reset_request, 1);
}

#[test]
fn app_restore_the_scaler_is_the_opening_count_over_100() {
    let mut b = begin();
    b.env.eep.calib[0] = CalibRecord {
        opening_count: 9900,
        closing_count: 9900,
        mean_current: 20,
        flags: CALIB_VALID,
    };
    b.env.reason = BootReason::PowerOn;
    b.app_restore();
    assert_eq!(b.m.mots[0].scaler, 99);
}

#[test]
fn app_warm_save_the_lease_client_and_an_unscheduled_retry_survive_a_warm_reset() {
    let mut b = begin();
    b.app.app_lease_command();
    assert!(b.app.app_lease_client());
    b.app_warm_save();
    b.env.reason = BootReason::Software;
    b.app_restore();
    assert!(b.app.app_lease_client());
    assert_eq!(b.v3(0).flags & VLV_FLAG_RETRY, 0);
}

#[test]
fn app_warm_save_no_lease_client_stays_none_after_a_warm_reset() {
    let mut b = begin();
    assert!(!b.app.app_lease_client());
    b.app_warm_save();
    b.env.reason = BootReason::Software;
    b.app_restore();
    assert!(!b.app.app_lease_client());
}

#[test]
fn app_stop_255_requests_of_valve_0_end_the_stopped_valve_gets_the_hold() {
    let mut b = begin();
    b.m.valves[0].forced_learn = true;
    b.m.valves[11].forced_learn = true;
    b.env.stop = 0;
    assert_eq!(b.app_stop(255), 0);
    assert!(!b.m.valves[0].forced_learn);
    assert!(!b.m.valves[11].forced_learn);
    assert_eq!(b.m.valves[0].svc_hold, SVMOV_HOLD_10S);
    assert_eq!(b.m.valves[1].svc_hold, 0);
}

#[test]
fn short_w10_a_time_retry_or_early_stop_request_of_a_shorted_valve_becomes_a_test() {
    let mut b = begin();
    for v in 0..3 {
        b.m.mots[v].status = ST_FAILED;
        b.m.mots[v].fault_reason = ValveFault::Short as u8;
    }
    b.m.valves[0].timed_learn = true;
    b.m.valves[1].retry_learn = true;
    b.m.valves[2].early_learn = true;
    b.app_loop();
    for v in 0..3 {
        let valve = &b.m.valves[v];
        assert!(!valve.timed_learn, "valve {v}");
        assert!(!valve.retry_learn, "valve {v}");
        assert!(!valve.early_learn, "valve {v}");
        assert_eq!(valve.retest_request, 0, "valve {v}");
        assert_eq!(b.m.mots[v].status, ST_UNKNOWN, "valve {v}");
    }
}

#[test]
fn calibration_records_an_uncalibrated_result_is_stored_without_flags() {
    let mut b = begin();
    b.m.mots[0].calibrated = false;
    b.m.mots[0].calib_failed = false;
    b.m.mots[0].calib_seq = b.m.mots[0].calib_seq.wrapping_add(1);
    b.app_loop();
    assert_eq!(
        b.env.log.calls_of("eeprom_store_calib"),
        calls(&["eeprom_store_calib(0)"])
    );
    assert_eq!(b.env.last_calib.flags, 0);
}

#[test]
fn retry_k2_no_countdown_while_a_test_or_calibration_of_the_valve_is_requested() {
    let mut b = begin();
    b.app.app_lease_configure(0);
    for v in 0..4 {
        b.m.mots[v].status = ST_BLOCKED;
    }
    b.m.valves[0].retest_request = RETEST_TEST;
    b.m.valves[1].forced_learn = true;
    b.m.mots[2].calibration = true;
    b.app_1s_tick(1);
    b.app_1s_tick(3600);
    assert!(b.m.valves[3].retry_learn);
    assert!(!b.m.valves[0].retry_learn);
    assert!(!b.m.valves[1].retry_learn);
    assert!(!b.m.valves[2].retry_learn);
}
