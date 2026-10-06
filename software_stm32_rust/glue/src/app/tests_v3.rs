// Port of software_stm32/test/native/glue/test_app_v3.cpp: protocol 3 in app.cpp (glue_app, the
// valve state machine stubbed): lease and failsafe positions, assembly hold, blocked valves and
// automatic retries, warm restore, calibration records, stop, learn time rule, protection guard,
// safe mode, temperature hold.
use core::sync::atomic::Ordering;
use std::string::String;
use std::vec::Vec;

use vdm_stm_core::config_blocks::{CalibRecord, CALIB_FAILED, CALIB_VALID};
use vdm_stm_core::config_store::{
    CFG_READ_FAILED, CFG_SAFETY_CORRUPT, LEASE_SOURCE_DEFAULT, LEASE_SOURCE_SAFETY,
    LEASE_SOURCE_SETTINGS,
};
use vdm_stm_core::move_classifier::StopReason;
use vdm_stm_core::system_stats::BootReason;
use vdm_stm_core::valve_codes::{
    ValveFault, ST_BLOCKED, ST_CLOSING, ST_FAILED, ST_FULL_OPEN, ST_IDLE, ST_OPEN_CIRCUIT,
    ST_PRESENT, ST_UNKNOWN, VLV_FLAG_ASSEMBLY, VLV_FLAG_CAL_RESTORED, VLV_FLAG_EARLY_PENDING,
    VLV_FLAG_FS_BLOCKED, VLV_FLAG_FS_LEASE, VLV_FLAG_NEEDS_REF, VLV_FLAG_RECAL, VLV_FLAG_RETRY,
    VLV_FLAG_SVC_HOLD, VLV_FLAG_UNCALIBRATED,
};

use crate::motor::CalibState;

use super::stub_env::AppBench;
use super::{RETEST_DETECT, RETEST_TEST, SVMOV_HOLD_10S, VALVE_NO_TARGET};

fn calls(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| String::from(*s)).collect()
}

fn none() -> Vec<String> {
    Vec::new()
}

/// app_setup, then every valve idle, calibrated and at its target 50
fn begin() -> AppBench {
    let mut b = AppBench::new();
    b.app_setup();
    for mot in &mut b.m.mots {
        mot.status = ST_IDLE;
        mot.calibrated = true;
        mot.connected = true;
        mot.scaler = 36;
    }
    b.env.log.clear();
    b.env.take_tx();
    b
}

/// the lease of 5 minutes expired
fn expire_lease(b: &mut AppBench) {
    b.app.app_lease_configure(5);
    b.app_1s_tick(300);
    assert_eq!(b.app.app_lease_state(), 2);
}

fn record(opening: u16, closing: u16, mean: u16, flags: u8) -> CalibRecord {
    CalibRecord {
        opening_count: opening,
        closing_count: closing,
        mean_current: mean,
        flags,
    }
}

#[test]
fn lease_default_60_min_slcfg_expiry_valve_polls_heartbeats() {
    let mut b = begin();
    let app = &mut b.app;
    assert_eq!(app.app_lease_timeout(), 60);
    assert_eq!(app.app_lease_state(), 1);
    assert_eq!(app.app_lease_remaining_s(), 3600);
    assert!(!app.app_lease_client());
    app.app_lease_configure(5);
    assert_eq!(app.app_lease_timeout(), 5);
    assert_eq!(app.app_lease_remaining_s(), 300);
    b.app_1s_tick(299);
    let app = &mut b.app;
    assert_eq!(app.app_lease_remaining_s(), 1);
    b.app_1s_tick(1);
    let app = &mut b.app;
    assert_eq!(app.app_lease_state(), 2);
    assert_eq!(app.app_lease_remaining_s(), 0);
    // no lease client: a gvlvx poll renews
    app.app_lease_poll();
    assert_eq!(app.app_lease_state(), 1);
    assert_eq!(app.app_lease_remaining_s(), 300);
    app.app_lease_command();
    assert!(app.app_lease_client());
    b.app_1s_tick(299);
    let app = &mut b.app;
    app.app_lease_poll();
    assert_eq!(app.app_lease_remaining_s(), 1);
    app.app_lease_heartbeat(false);
    assert_eq!(app.app_lease_remaining_s(), 1);
    app.app_lease_heartbeat(true);
    assert_eq!(app.app_lease_remaining_s(), 300);
    app.app_lease_configure(0);
    assert_eq!(app.app_lease_state(), 0);
    assert!(b.env.log.calls.is_empty());
}

#[test]
fn lease_k1_5_gvlvx_polls_of_an_esp_2_0_0_keep_the_lease_gvlvy_polls_without_slhbt_do_not() {
    let mut b = begin();
    b.app.app_lease_configure(5);
    for _ in 0..20 {
        b.app_1s_tick(60);
        b.app.app_lease_poll();
    }
    assert_eq!(b.app.app_lease_state(), 1);
    // a protocol-3 ESP: its polls no longer renew
    b.app.app_lease_heartbeat(false);
    for _ in 0..4 {
        b.app_1s_tick(60);
        b.app.app_lease_poll();
        b.app.app_lease_heartbeat(false);
    }
    assert_eq!(b.app.app_lease_state(), 1);
    b.app_1s_tick(60);
    assert_eq!(b.app.app_lease_state(), 2);
}

#[test]
fn failsafe_k1_4_an_expired_lease_drives_the_valves_to_their_failsafe_positions() {
    let mut b = begin();
    b.app.app_set_failsafe(255, 20);
    b.app.app_set_failsafe(3, 255);
    assert_eq!(b.app.app_failsafe_pct(0), 20);
    assert_eq!(b.app.app_failsafe_pct(3), 255);
    assert_eq!(b.app.app_failsafe_pct(12), 255);
    assert_eq!(b.app_failsafe_mask(), 0);
    assert_eq!(b.loop_actions(1), none());
    expire_lease(&mut b);
    assert_eq!(b.app_failsafe_mask(), 0x0FF7);
    assert_eq!(b.loop_actions(1), calls(&["appsetaction(c, 0, 30, 0)"]));
    assert_eq!(b.m.mots[0].target_position, 50);
    let info = b.v3(0);
    assert_eq!(info.flags, VLV_FLAG_FS_LEASE);
    assert_eq!(info.fs_pct, 20);
    assert_eq!(info.drive, 20);
    assert_eq!(info.fault, 0);
    assert_eq!(info.retry_s, 0);
    assert_eq!(info.retries, 0);
    let info = b.v3(3);
    assert_eq!(info.flags, 0);
    assert_eq!(info.fs_pct, 255);
    assert_eq!(info.drive, 50);
    // slhbt 1: the stored targets again
    b.app.app_lease_heartbeat(true);
    assert_eq!(b.app_failsafe_mask(), 0);
    assert_eq!(b.loop_actions(1), none());
    assert_eq!(b.v3(0).drive, 50);
}

#[test]
fn failsafe_a_failed_or_open_circuit_valve_is_not_driven_a_blocked_one_without_the_lease() {
    let mut b = begin();
    b.app.app_set_failsafe(255, 20);
    b.m.mots[1].status = ST_FAILED;
    b.m.mots[2].status = ST_OPEN_CIRCUIT;
    b.m.mots[3].status = ST_BLOCKED;
    b.m.mots[3].actual_position = 20;
    expire_lease(&mut b);
    assert_eq!(b.app_failsafe_mask(), 0x0FFF & !0x000E);
    assert_eq!(b.v3(1).drive, 50);
    assert_eq!(b.v3(2).drive, 50);
    assert_eq!(b.v3(3).drive, 20);
    // the retry was scheduled meanwhile
    assert_eq!(b.v3(3).flags, VLV_FLAG_FS_BLOCKED + VLV_FLAG_RETRY);
}

#[test]
fn assembly_hold_k1_7_staop_opens_fully_and_keeps_the_valve_at_100_until_the_next_stgtp() {
    let mut b = begin();
    b.app.app_set_failsafe(255, 255);
    b.app.app_set_failsafe(2, 20);
    expire_lease(&mut b);
    assert_eq!(b.app_set_valveopen(2), 0);
    assert!(b.m.valves[2].assembly_hold);
    assert_eq!(b.loop_actions(1), calls(&["appsetaction(p, 2, 0, 0)"]));
    assert_eq!(b.m.mots[2].status, ST_FULL_OPEN);
    b.m.mots[2].status = ST_IDLE;
    b.m.mots[2].actual_position = 100;
    assert_eq!(b.loop_actions(1), none());
    assert_eq!(b.v3(2).flags, VLV_FLAG_ASSEMBLY);
    assert_eq!(b.v3(2).drive, 100);
    assert_eq!(b.app_failsafe_mask(), 0);
    b.m.mots[2].target_position = 40;
    b.app_target_changed(2);
    assert!(!b.m.valves[2].assembly_hold);
    assert_eq!(b.app_failsafe_mask(), 0x0004);
    assert_eq!(b.loop_actions(1), calls(&["appsetaction(c, 2, 80, 0)"]));
    b.app_set_valveopen(255);
    for v in 0..12 {
        assert!(b.m.valves[v].assembly_hold, "valve {v}");
    }
}

#[test]
fn app_target_changed_ends_the_service_and_assembly_holds_and_touches_the_valve() {
    let mut b = begin();
    b.m.valves[4].svc_hold = 9;
    b.m.valves[4].assembly_hold = true;
    b.app_target_changed(4);
    assert_eq!(b.m.valves[4].svc_hold, 0);
    assert!(!b.m.valves[4].assembly_hold);
    assert!(b.m.valves[4].touched);
    assert!(!b.m.valves[5].touched);
    b.app_target_changed(12);
    // touched: a valve without a referenced position makes its reference move at once
    b.m.mots[4].needs_reference = true;
    assert_eq!(
        b.loop_actions(1),
        calls(&["appsetaction(p, 4, 0, 0, 0x02)"])
    );
    assert!(!b.m.valves[4].touched);
}

#[test]
fn blocked_valve_k2_failsafe_move_with_the_status_kept_rejected_targets_retries_1_h_and_6_h() {
    let mut b = begin();
    b.app.app_lease_configure(0);
    b.app.app_set_failsafe(3, 50);
    b.m.mots[3].status = ST_BLOCKED;
    b.m.mots[3].fault_reason = ValveFault::StrokesTooShort as u8;
    b.m.mots[3].actual_position = 0;
    b.m.mots[3].target_position = 30;
    b.m.valves[3].rejected_target = 30;
    assert_eq!(
        b.loop_actions(1),
        calls(&["appsetaction(o, 3, 50, 0, 0x01)"])
    );
    b.m.mots[3].actual_position = 50;
    assert_eq!(b.loop_actions(1), none());
    assert_eq!(b.m.valves[3].cmd_rejected, 0);
    b.m.mots[3].target_position = 80;
    assert_eq!(b.loop_actions(1), none());
    assert_eq!(b.m.valves[3].cmd_rejected, 1);
    b.app_1s_tick(1);
    let info = b.v3(3);
    assert_eq!(info.flags, VLV_FLAG_FS_BLOCKED + VLV_FLAG_RETRY);
    assert_eq!(info.fault, 4);
    assert_eq!(info.fs_pct, 50);
    assert_eq!(info.drive, 50);
    assert_eq!(info.retry_s, 3600);
    assert_eq!(info.retries, 0);
    b.app_1s_tick(3599);
    assert_eq!(b.v3(3).retry_s, 1);
    assert!(!b.m.valves[3].retry_learn);
    b.app_1s_tick(1);
    assert!(b.m.valves[3].retry_learn);
    assert_eq!(b.v3(3).retries, 1);
    assert_eq!(b.v3(3).retry_s, 0);
    assert!(b.app_learn_pending(3, ST_BLOCKED, false));
    assert_eq!(b.loop_actions(1), none());
    assert_eq!(b.m.mots[3].status, ST_PRESENT);
    assert_eq!(b.loop_actions(1), calls(&["appsetaction(l, 3, 0, 0)"]));
    assert!(!b.m.valves[3].retry_learn);
    assert_eq!(b.m.mots[3].calib_state, CalibState::InProgress);
    // blocked again after the retry
    b.m.mots[3].status = ST_BLOCKED;
    b.app_1s_tick(1);
    // still busy: the handed-over calibration
    assert_eq!(b.v3(3).retry_s, 0);
    b.m.mots[3].calib_state = CalibState::Idle;
    b.app_1s_tick(1);
    assert_eq!(b.v3(3).retry_s, 21600);
    assert_eq!(b.v3(3).retries, 1);
    // fine again: reset
    b.m.mots[3].status = ST_IDLE;
    b.app_1s_tick(1);
    assert_eq!(b.v3(3).retries, 0);
    assert_eq!(b.v3(3).flags, 0);
}

#[test]
fn blocked_valve_failsafe_255_keeps_it_where_the_calibration_left_it() {
    let mut b = begin();
    b.app.app_set_failsafe(3, 255);
    b.m.mots[3].status = ST_BLOCKED;
    b.m.mots[3].actual_position = 0;
    assert_eq!(b.loop_actions(1), none());
    assert_eq!(b.v3(3).flags, 0);
}

#[test]
fn short_k2_w10_the_retry_is_a_presence_test_calibration_requests_become_tests() {
    let mut b = begin();
    b.app.app_lease_configure(0);
    b.m.mots[5].status = ST_FAILED;
    b.m.mots[5].fault_reason = ValveFault::Short as u8;
    b.m.mots[5].actual_position = 30;
    b.app_1s_tick(1);
    b.app_1s_tick(3600);
    assert_eq!(b.m.valves[5].retest_request, RETEST_TEST);
    assert!(!b.m.valves[5].retry_learn);
    assert_eq!(b.loop_actions(12), calls(&["appsetaction(x, 5, 0, 0)"]));
    assert_eq!(b.m.valves[5].retest_request, 0);
    assert_eq!(b.m.mots[5].status, ST_UNKNOWN);
    assert_eq!(b.m.mots[5].actual_position, 0);
    assert!(!b.m.mots[5].recal);
    // staln of a shorted valve
    b.m.mots[5].status = ST_FAILED;
    assert_eq!(b.app_set_valvelearning(5), 0);
    b.app_loop();
    assert!(!b.m.valves[5].forced_learn);
    assert!(!b.m.mots[5].calibration);
    assert_eq!(b.m.mots[5].calib_state, CalibState::Idle);
    assert_eq!(b.m.mots[5].status, ST_UNKNOWN);
    // a failed valve with another fault calibrates
    b.m.mots[6].status = ST_FAILED;
    b.m.mots[6].fault_reason = ValveFault::MoveTimeout as u8;
    b.app_set_valvelearning(6);
    b.app_loop();
    assert!(b.m.valves[6].forced_learn);
}

#[test]
fn time_and_movement_triggers_skip_failed_and_blocked_valves() {
    let mut b = begin();
    b.app_set_learntime(120);
    b.app_set_learnmovements(50);
    b.m.mots[0].status = ST_BLOCKED;
    b.m.mots[1].status = ST_FAILED;
    b.m.valves[1].learn_movements = 0;
    b.m.valves[2].learn_movements = 0;
    b.app_10s_loop(10);
    assert!(!b.m.valves[0].timed_learn);
    assert_eq!(b.m.valves[0].learn_time, 120);
    assert!(!b.m.mots[1].calibration);
    assert!(b.m.mots[2].calibration);
}

#[test]
fn learn_time_s3_real_seconds_stagger_the_1200_s_trigger_of_valve_11() {
    let mut b = begin();
    b.app_set_learntime(1200);
    assert_eq!(b.app.app_get_learntime(), 1200);
    assert_eq!(b.m.valves[0].learn_time, 100);
    assert_eq!(b.m.valves[11].learn_time, 1200);
    for i in 1..120 {
        b.app_10s_loop(10);
        assert!(!b.m.valves[11].timed_learn, "call {i}");
    }
    assert_eq!(b.m.valves[11].learn_time, 10);
    b.app_10s_loop(10);
    assert!(b.m.valves[11].timed_learn);
    assert_eq!(b.m.valves[11].learn_time, 1200);
    // a late call counts its real seconds
    b.app_set_learntime(1200);
    b.app_10s_loop(99);
    assert_eq!(b.m.valves[0].learn_time, 1);
}

#[test]
fn learn_time_c7_stored_0_is_the_esp_schedule_only_while_a_lease_client_was_seen_within_24_h() {
    let mut b = begin();
    b.app_set_learntime(0);
    assert_eq!(b.app.app_get_learntime(), 0);
    assert_eq!(b.m.valves[11].learn_time, 604_800);
    b.app.app_lease_command();
    b.app_1s_tick(1);
    assert_eq!(b.m.valves[11].learn_time, 0);
    assert_eq!(b.m.valves[0].learn_time, 0);
    b.app_10s_loop(10);
    assert!(!b.m.valves[0].timed_learn);
    b.app_1s_tick(86398);
    assert_eq!(b.m.valves[11].learn_time, 0);
    b.app_1s_tick(1);
    assert_eq!(b.m.valves[11].learn_time, 604_800);
    assert_eq!(b.m.valves[0].learn_time, 50400);
    b.app_1s_tick(1);
    // no reload while the value stays
    assert_eq!(b.m.valves[0].learn_time, 50400);
    b.app.app_lease_heartbeat(false);
    b.app_1s_tick(1);
    assert_eq!(b.m.valves[11].learn_time, 0);
    b.app_set_learntime(3600);
    assert_eq!(b.m.valves[11].learn_time, 3600);
}

#[test]
fn early_stops_w9_the_second_one_in_a_row_requests_a_calibration_not_of_a_blocked_valve() {
    let mut b = begin();
    b.m.mots[4].early_learn_due = true;
    b.m.mots[6].early_learn_due = true;
    b.m.mots[6].status = ST_BLOCKED;
    b.app.app_set_failsafe(6, 255);
    assert!(!b.app_learn_pending(4, ST_IDLE, false));
    assert_eq!(b.loop_actions(1), none());
    assert!(!b.m.mots[4].early_learn_due);
    assert!(b.m.valves[4].early_learn);
    assert_eq!(b.m.mots[4].status, ST_PRESENT);
    assert!(!b.m.mots[6].early_learn_due);
    assert!(!b.m.valves[6].early_learn);
    assert_eq!(b.loop_actions(1), calls(&["appsetaction(l, 4, 0, 0)"]));
    assert!(!b.m.valves[4].early_learn);
}

#[test]
fn end_stop_latch_the_end_of_a_move_reaches_the_scheduler_one_retry_after_an_early_stop() {
    let mut b = begin();
    b.m.mots[0].target_position = 70;
    assert_eq!(b.loop_actions(1), calls(&["appsetaction(o, 0, 20, 0)"]));
    b.m.mots[0].actual_position = 55;
    b.m.mots[0].move_seq = 1;
    b.env.snapshot[0].diag.last.stop_reason = StopReason::EarlyEndStop as u8;
    b.env.snapshot[0].diag.last_early = true;
    b.env.snapshot[0].status = ST_IDLE;
    b.env.log.clear();
    b.app_loop();
    assert_eq!(
        b.env.log.calls_of("valve_get_snapshot"),
        calls(&["valve_get_snapshot(0)"])
    );
    assert_eq!(
        b.env.log.calls_of("appsetaction"),
        calls(&["appsetaction(o, 0, 15, 0)"])
    );
    b.m.mots[0].move_seq = 2;
    assert_eq!(b.loop_actions(5), none());
    assert_eq!(
        b.env.log.calls_of("valve_get_snapshot"),
        calls(&["valve_get_snapshot(0)"])
    );
    b.m.mots[0].target_position = 80;
    assert_eq!(b.loop_actions(1), calls(&["appsetaction(o, 0, 25, 0)"]));
}

#[test]
fn calibration_records_w2_accepted_and_blocked_calibrations_go_to_the_eeprom_once() {
    let mut b = begin();
    b.m.mots[1].opening_count = 3600;
    b.m.mots[1].closing_count = 3650;
    b.m.mots[1].meancurrent = 25;
    b.m.mots[1].calib_seq = 1;
    b.m.valves[1].calib_restored = true;
    b.env.log.clear();
    b.app_loop();
    assert_eq!(
        b.env.log.calls_of("eeprom_store_calib"),
        calls(&["eeprom_store_calib(1)"])
    );
    assert_eq!(b.env.last_calib, record(3600, 3650, 25, CALIB_VALID));
    assert!(!b.m.valves[1].calib_restored);
    assert_eq!(b.m.valves[1].stored_seq, 1);
    b.env.log.clear();
    b.app_loop();
    assert!(b.env.log.calls_of("eeprom_store_calib").is_empty());
    // blocked: the counts of the last success, marked failed
    b.m.mots[1].calib_seq = 2;
    b.m.mots[1].calib_failed = true;
    b.app_loop();
    assert_eq!(b.env.last_calib.flags, CALIB_VALID + CALIB_FAILED);
    // never calibrated and blocked
    b.m.mots[2].calibrated = false;
    b.m.mots[2].calib_failed = true;
    b.m.mots[2].calib_seq = 5;
    b.app_loop();
    assert_eq!(b.env.last_calib.flags, CALIB_FAILED);
    assert_eq!(b.m.valves[2].stored_seq, 5);
}

#[test]
fn app_restore_w2_calibration_records_after_a_power_on_the_test_decides_the_status() {
    let mut b = begin();
    b.env.eep.calib[2] = record(3600, 3650, 25, CALIB_VALID);
    b.env.eep.calib[4] = record(3000, 2950, 30, CALIB_VALID + CALIB_FAILED);
    b.env.eep.calib[5] = record(0, 0, 0, CALIB_FAILED);
    for mot in &mut b.m.mots {
        mot.calibrated = false;
        mot.status = ST_UNKNOWN;
    }
    b.m.mots[2].calib_seq = 3;
    b.env.reason = BootReason::PowerOn;
    b.app_restore();
    let mot = b.m.mots[2];
    assert_eq!(mot.opening_count, 3600);
    assert_eq!(mot.closing_count, 3650);
    assert_eq!(mot.deadzone_count, 50);
    assert_eq!(mot.scaler, 36);
    assert_eq!(mot.meancurrent, 25);
    assert!(mot.calibrated);
    assert!(!mot.recal);
    assert!(b.m.valves[2].calib_restored);
    assert_eq!(b.m.valves[2].stored_seq, 3);
    assert_eq!(mot.status, ST_UNKNOWN);
    assert_eq!(b.m.mots[4].deadzone_count, -50);
    assert!(b.m.mots[4].recal);
    assert!(b.m.mots[4].calibrated);
    assert!(!b.m.mots[5].calibrated);
    assert!(b.m.mots[5].recal);
    assert!(!b.m.mots[0].calibrated);
    assert!(!b.m.valves[0].calib_restored);
    assert_eq!(b.v3(2).flags, VLV_FLAG_CAL_RESTORED);
    assert_eq!(b.v3(0).flags, VLV_FLAG_UNCALIBRATED);
    assert_eq!(b.v3(4).flags, VLV_FLAG_CAL_RESTORED + VLV_FLAG_RECAL);
}

#[test]
fn app_restore_w2_k1_8_a_warm_reset_restores_positions_holds_retries_and_the_expired_lease() {
    let mut b = begin();
    b.app.app_set_failsafe(255, 40);
    b.m.mots[0].status = ST_BLOCKED;
    b.m.mots[0].actual_position = 40;
    b.m.mots[0].target_position = 30;
    b.m.valves[1].assembly_hold = true;
    b.m.mots[1].actual_position = 100;
    b.m.mots[1].target_position = 100;
    b.m.mots[2].needs_reference = true;
    b.m.mots[2].recal = true;
    b.m.mots[3].status = ST_OPEN_CIRCUIT;
    // moving: not referenced after the reset
    b.m.mots[5].status = ST_CLOSING;
    b.m.mots[6].status = ST_IDLE;
    b.m.mots[6].actual_position = 77;
    // a command for valve 6 is handed over
    b.env.busy = 6;
    expire_lease(&mut b);
    // valve 0: its retry was scheduled by expire_lease()
    b.app_1s_tick(10);
    b.app_1s_tick(100);
    b.app_warm_save();
    // the reset: RAM of app.cpp from app_setup() again (the C++ valve_setup() here is the logging
    // stub of glue_app), the calibration records of the EEPROM
    b.app.app_set_failsafe(255, 50);
    b.app.app_lease_configure(0);
    b.app.app_lease_configure(60);
    b.app_setup();
    for v in 0..12 {
        b.m.mots[v].calibrated = false;
        b.env.eep.calib[v] = record(3600, 3600, 20, CALIB_VALID);
    }
    b.env.reason = BootReason::Pin;
    b.env.cfg_flags = CFG_SAFETY_CORRUPT;
    b.app_restore();
    assert_eq!(b.m.mots[0].status, ST_BLOCKED);
    assert_eq!(b.m.mots[0].actual_position, 40);
    assert_eq!(b.m.mots[0].target_position, 30);
    assert_eq!(b.m.valves[0].rejected_target, 30);
    assert!(b.m.mots[0].connected);
    assert_eq!(b.v3(0).retry_s, 3490);
    // block B damaged: the copy of the last run
    assert_eq!(b.v3(0).fs_pct, 40);
    assert!(b.m.valves[1].assembly_hold);
    assert_eq!(b.m.mots[1].actual_position, 100);
    assert!(b.m.mots[2].needs_reference);
    assert!(b.m.mots[2].recal);
    assert_eq!(b.m.mots[2].status, ST_IDLE);
    assert_eq!(b.m.mots[3].status, ST_OPEN_CIRCUIT);
    assert!(!b.m.mots[3].connected);
    assert_eq!(b.m.mots[5].status, ST_IDLE);
    assert!(b.m.mots[5].needs_reference);
    assert!(b.m.mots[6].needs_reference);
    assert_eq!(b.m.mots[6].actual_position, 77);
    assert!(!b.m.mots[7].needs_reference);
    assert_eq!(b.m.mots[7].status, ST_IDLE);
    // the lease stays expired (the source of its timeout is the default: the copy of 5 min is
    // taken)
    assert_eq!(b.app.app_lease_timeout(), 5);
    assert_eq!(b.app.app_lease_state(), 2);
    // the failsafe applies at once: the first plain move goes to the failsafe position
    assert_eq!(b.loop_actions(1), calls(&["appsetaction(c, 4, 10, 0)"]));
}

#[test]
fn app_restore_a_power_on_or_an_eeprom_lease_source_ignores_the_warm_copies() {
    let mut b = begin();
    b.m.mots[0].actual_position = 10;
    b.app.app_set_failsafe(255, 30);
    b.app.app_lease_configure(5);
    b.app_warm_save();
    b.m.mots[0].actual_position = 50;
    b.app.app_set_failsafe(255, 50);
    b.app.app_lease_configure(60);
    b.env.reason = BootReason::PowerOn;
    b.app_restore();
    assert_eq!(b.m.mots[0].actual_position, 50);
    assert_eq!(b.app.app_failsafe_pct(0), 50);
    assert_eq!(b.app.app_lease_timeout(), 60);
    b.env.reason = BootReason::Software;
    b.env.lease_source = LEASE_SOURCE_SETTINGS;
    b.app_restore();
    assert_eq!(b.m.mots[0].actual_position, 10);
    // block B was fine
    assert_eq!(b.app.app_failsafe_pct(0), 50);
    assert_eq!(b.app.app_lease_timeout(), 60);
    b.env.cfg_flags = CFG_READ_FAILED;
    b.app_restore();
    assert_eq!(b.app.app_failsafe_pct(0), 30);
}

#[test]
fn app_load_config_c5_after_a_warm_reset_the_copies_stand_in_for_what_the_re_read_lacks() {
    let mut b = begin();
    b.app.app_set_failsafe(255, 30);
    b.app.app_lease_configure(5);
    b.app_warm_save();
    b.app.app_set_failsafe(255, 50);
    b.app.app_lease_configure(60);
    b.env.reason = BootReason::Software;
    b.env.cfg_flags = CFG_READ_FAILED;
    b.app_restore();
    assert_eq!(b.app.app_lease_timeout(), 5);
    assert_eq!(b.app.app_failsafe_pct(0), 30);
    // the re-read after the failed read finds neither a lease timeout nor block B
    b.env.eep.failsafe_pct = [50; 12];
    b.env.eep.lease_timeout_min = 60;
    b.env.cfg_flags = CFG_SAFETY_CORRUPT;
    b.env.log.clear();
    b.app_load_config();
    assert_eq!(b.app.app_lease_timeout(), 5);
    assert_eq!(b.app.app_failsafe_pct(0), 30);
    assert_eq!(b.app.app_failsafe_pct(11), 30);
    assert_eq!(b.env.log.calls_of("eeprom_cfg_flags").len(), 1);
    // it finds both: the EEPROM wins
    b.env.lease_source = LEASE_SOURCE_SAFETY;
    b.env.cfg_flags = 0;
    b.env.eep.lease_timeout_min = 90;
    b.env.eep.failsafe_pct[11] = 70;
    b.app_load_config();
    assert_eq!(b.app.app_lease_timeout(), 90);
    assert_eq!(b.app.app_failsafe_pct(0), 50);
    assert_eq!(b.app.app_failsafe_pct(11), 70);
    // a failed read again: the copies of the positions, the lease timeout of the source
    b.env.cfg_flags = CFG_READ_FAILED;
    b.app_load_config();
    assert_eq!(b.app.app_lease_timeout(), 90);
    assert_eq!(b.app.app_failsafe_pct(11), 30);
    b.env.lease_source = LEASE_SOURCE_DEFAULT;
    b.app_load_config();
    assert_eq!(b.app.app_lease_timeout(), 5);
}

#[test]
fn app_load_config_a_failsafe_position_set_after_the_failed_read_wins_over_its_warm_copy() {
    let mut b = begin();
    b.app.app_set_failsafe(255, 30);
    b.app_warm_save();
    // the reset
    b.app.app_set_failsafe(255, 50);
    b.env.reason = BootReason::Software;
    b.env.cfg_flags = CFG_READ_FAILED;
    b.app_restore();
    assert_eq!(b.app.app_failsafe_pct(3), 30);
    // sfspo 3 20 while the EEPROM cannot be read, then the re-read finds block B damaged: the
    // mirror holds the defaults and the marked position (the eeprom module)
    b.app.app_set_failsafe(3, 20);
    b.env.eep.failsafe_pct = [50; 12];
    b.env.eep.failsafe_pct[3] = 20;
    b.env.cfg_flags = CFG_SAFETY_CORRUPT;
    b.app_load_config();
    assert_eq!(b.app.app_failsafe_pct(3), 20);
    assert_eq!(b.app.app_failsafe_pct(2), 30);
    assert_eq!(b.app.app_failsafe_pct(4), 30);
    assert_eq!(b.v3(3).fs_pct, 20);
    // sfspo 255 replaces every copy
    b.app.app_set_failsafe(255, 70);
    b.app_load_config();
    assert_eq!(b.app.app_failsafe_pct(0), 70);
    assert_eq!(b.app.app_failsafe_pct(11), 70);
}

#[test]
fn app_load_config_c5_after_a_power_on_the_defaults_stand_in_60_min_and_block_b_as_loaded() {
    let mut b = begin();
    b.app.app_set_failsafe(255, 30);
    b.app.app_lease_configure(5);
    b.app_warm_save();
    b.env.reason = BootReason::PowerOn;
    b.env.cfg_flags = CFG_READ_FAILED;
    b.app_restore();
    b.env.eep.failsafe_pct = [50; 12];
    b.env.eep.lease_timeout_min = 0;
    b.env.log.clear();
    b.app_load_config();
    assert_eq!(b.app.app_lease_timeout(), 60);
    assert_eq!(b.app.app_failsafe_pct(0), 50);
    // no copies: the flags are not needed
    assert!(b.env.log.calls_of("eeprom_cfg_flags").is_empty());
}

#[test]
fn app_warm_moving_c8_a_valve_handed_a_command_restores_unreferenced() {
    let mut b = begin();
    b.env.eep.calib[3] = record(3600, 3600, 20, CALIB_VALID);
    b.env.eep.calib[4] = record(3600, 3600, 20, CALIB_VALID);
    // before any record: nothing to invalidate
    b.app_warm_moving(3);
    b.app_warm_save();
    b.app_warm_moving(4);
    b.app_warm_moving(12);
    b.env.reason = BootReason::IndependentWatchdog;
    b.app_restore();
    assert!(!b.m.mots[3].needs_reference);
    assert!(b.m.mots[4].needs_reference);
    assert_eq!(b.m.mots[4].status, ST_IDLE);
}

#[test]
fn app_warm_save_an_invalid_kept_valve_takes_the_cold_path_alone() {
    let mut b = begin();
    b.m.mots[2].actual_position = 101;
    b.m.mots[2].status = ST_UNKNOWN;
    b.m.mots[3].actual_position = 20;
    b.app_warm_save();
    b.m.mots[2].actual_position = 50;
    b.m.mots[3].actual_position = 50;
    b.env.reason = BootReason::Pin;
    b.app_restore();
    assert_eq!(b.m.mots[2].actual_position, 50);
    assert_eq!(b.m.mots[3].actual_position, 20);
}

#[test]
fn app_stop_s8_stops_the_valve_cancels_its_requests_leaves_it_in_a_service_hold() {
    let mut b = begin();
    b.m.mots[3].status = ST_PRESENT;
    b.m.valves[3].forced_learn = true;
    b.m.valves[3].timed_learn = true;
    b.m.valves[3].retry_learn = true;
    b.m.valves[3].early_learn = true;
    b.m.mots[3].calibration = true;
    b.m.mots[3].calib_state = CalibState::Started;
    b.m.mots[4].status = ST_PRESENT;
    b.m.mots[4].calibrated = false;
    b.m.mots[5].status = ST_PRESENT;
    b.m.mots[5].recal = true;
    b.m.mots[6].calibration = true;
    b.m.mots[6].calib_active = true;
    b.m.mots[6].calib_state = CalibState::InProgress;
    b.env.stop = 7;
    assert_eq!(b.app_stop(255), 0);
    assert_eq!(b.env.log.calls, calls(&["appstop(255)"]));
    assert_eq!(b.m.mots[3].status, ST_IDLE);
    assert!(!b.m.valves[3].forced_learn);
    assert!(!b.m.valves[3].timed_learn);
    assert!(!b.m.valves[3].retry_learn);
    assert!(!b.m.valves[3].early_learn);
    assert!(!b.m.mots[3].calibration);
    assert_eq!(b.m.mots[3].calib_state, CalibState::Idle);
    // first calibration still pending
    assert_eq!(b.m.mots[4].status, ST_PRESENT);
    assert_eq!(b.m.mots[5].status, ST_PRESENT);
    // the running calibration ends through the stop
    assert!(b.m.mots[6].calibration);
    assert_eq!(b.m.valves[7].svc_hold, SVMOV_HOLD_10S);
    assert_eq!(b.m.valves[3].svc_hold, 0);
    b.env.stop = -1;
    b.app_stop(255);
    assert_eq!(b.m.valves[0].svc_hold, 0);
    b.env.log.clear();
    assert_eq!(b.app_stop(2), 0);
    assert_eq!(b.env.log.calls, calls(&["appstop(2)"]));
    assert_eq!(b.m.valves[2].svc_hold, SVMOV_HOLD_10S);
    assert_eq!(b.m.valves[1].svc_hold, 0);
    b.m.valves[1].forced_learn = true;
    b.app_stop(2);
    assert!(b.m.valves[1].forced_learn);
    b.env.log.clear();
    assert_eq!(b.app_stop(12), -1);
    assert!(b.env.log.calls.is_empty());
    assert_eq!(b.app_stop(11), 0);
    assert_eq!(b.v3(11).flags, VLV_FLAG_SVC_HOLD);
}

#[test]
fn protection_guard_c4_trips_on_3_valves_suspend_the_limits_failed_valves_are_tested_again() {
    let mut b = begin();
    for v in 0..3 {
        b.m.mots[v].status = ST_FAILED;
        b.m.mots[v].fault_reason = ValveFault::Short as u8;
    }
    b.m.mots[4].status = ST_FAILED;
    b.m.mots[4].fault_reason = ValveFault::MoveTimeout as u8;
    // reported only
    b.m.mots[5].fault_reason = ValveFault::InrushTrip as u8;
    b.m.mots[0].trip_seq = 1;
    b.m.mots[1].trip_seq = 3;
    b.env.uptime = 100;
    b.app_loop();
    assert!(!b.app.app_protect_suspended());
    assert!(!b.flags.protect_suspended.load(Ordering::SeqCst));
    assert_eq!(b.m.mots[0].status, ST_FAILED);
    b.env.uptime = 699;
    b.m.mots[2].trip_seq = 1;
    b.app_loop();
    assert!(b.app.app_protect_suspended());
    assert!(b.flags.protect_suspended.load(Ordering::SeqCst));
    assert_eq!(b.m.mots[0].status, ST_UNKNOWN);
    assert_eq!(b.m.mots[2].status, ST_UNKNOWN);
    assert_eq!(b.m.valves[1].rejected_target, VALVE_NO_TARGET);
    assert_eq!(b.m.mots[4].status, ST_FAILED);
    assert_eq!(b.m.mots[5].status, ST_IDLE);
}

#[test]
fn protection_guard_trips_of_one_valve_do_not_suspend() {
    let mut b = begin();
    for i in 1..=5 {
        b.m.mots[0].trip_seq = i;
        b.app_loop();
    }
    assert!(!b.app.app_protect_suspended());
}

#[test]
fn safe_mode_s9_no_command_to_the_valve_state_machine_presence_tests_after_it_ends() {
    let mut b = begin();
    for mot in &mut b.m.mots {
        mot.status = ST_UNKNOWN;
    }
    b.m.mots[1].target_position = 80;
    b.env.safe_mode = true;
    assert_eq!(b.loop_actions(1000), none());
    b.m.mots[2].status = ST_IDLE;
    b.m.mots[2].calibrated = true;
    assert_eq!(b.app_service_move(2, 0, 100, 20), -2);
    assert!(b.env.log.calls_of("appsetservice").is_empty());
    b.env.safe_mode = false;
    assert_eq!(b.loop_actions(1), calls(&["appsetaction(x, 0, 0, 0)"]));
}

#[test]
fn temperature_hold_s1_no_new_motor_command_while_a_cycle_is_due_up_to_3_s() {
    let mut b = begin();
    b.m.mots[0].target_position = 70;
    b.env.board.advance_ms(59999);
    b.app_loop();
    assert!(!b.flags.temp_refresh_request.load(Ordering::SeqCst));
    b.m.mots[0].target_position = 50;
    b.env.board.advance_ms(1);
    assert_eq!(b.app_temp_age_s(), 60);
    b.m.mots[0].target_position = 70;
    assert_eq!(b.loop_actions(1), none());
    assert!(b.flags.temp_refresh_request.load(Ordering::SeqCst));
    b.app_temp_cycle_done();
    assert_eq!(b.app_temp_age_s(), 0);
    assert_eq!(b.loop_actions(1), calls(&["appsetaction(o, 0, 20, 0)"]));
    assert!(!b.flags.temp_refresh_request.load(Ordering::SeqCst));
    b.env.board.advance_ms(5000);
    assert_eq!(b.app_temp_age_s(), 5);
    // a cycle that does not come: the hold ends after 3 s
    b.env.board.advance_ms(55000);
    assert_eq!(b.loop_actions(1), none());
    b.env.board.advance_ms(2999);
    assert_eq!(b.loop_actions(1), none());
    b.env.board.advance_ms(1);
    assert_eq!(b.loop_actions(1), calls(&["appsetaction(o, 0, 20, 0)"]));
}

#[test]
fn temperature_hold_s1_a_timed_out_pause_between_calibration_strokes_ends_the_period() {
    let mut b = begin();
    b.env.board.advance_ms(61000);
    b.app_loop();
    assert!(b.flags.temp_refresh_request.load(Ordering::SeqCst));
    b.flags.temp_gap_timeout.store(true, Ordering::SeqCst);
    b.app_loop();
    assert!(!b.flags.temp_gap_timeout.load(Ordering::SeqCst));
    assert!(!b.flags.temp_refresh_request.load(Ordering::SeqCst));
    b.env.board.advance_ms(59999);
    b.app_loop();
    assert!(!b.flags.temp_refresh_request.load(Ordering::SeqCst));
    b.env.board.advance_ms(1);
    b.app_loop();
    assert!(b.flags.temp_refresh_request.load(Ordering::SeqCst));
}

#[test]
fn stdet_w2_7_every_valve_is_tested_again_and_a_present_one_calibrates_fully() {
    let mut b = begin();
    b.m.valves[3].svc_hold = 4;
    b.app_scan_valves();
    assert_eq!(b.m.valves[3].svc_hold, 0);
    assert_eq!(b.m.valves[3].retest_request, RETEST_DETECT);
    b.app_loop();
    for v in 0..12 {
        assert!(b.m.mots[v].recal, "valve {v}");
        assert_eq!(b.m.mots[v].status, ST_UNKNOWN, "valve {v}");
        assert_eq!(b.m.valves[v].retest_request, 0, "valve {v}");
        let want = if v == 0 { 50 } else { VALVE_NO_TARGET };
        assert_eq!(b.m.valves[v].rejected_target, want, "valve {v}");
    }
}

#[test]
fn app_learn_pending_w12_the_calibration_requirement_does_not_depend_on_the_status() {
    let mut b = begin();
    assert!(!b.app_learn_pending(0, ST_IDLE, false));
    assert!(b.app_learn_pending(0, ST_IDLE, true));
    assert!(b.app_learn_pending(0, ST_PRESENT, false));
    b.m.mots[0].calibrated = false;
    assert!(b.app_learn_pending(0, ST_IDLE, false));
    assert!(b.app_learn_pending(0, ST_FULL_OPEN, false));
    assert!(!b.app_learn_pending(0, ST_FAILED, false));
    assert!(!b.app_learn_pending(0, ST_BLOCKED, false));
    assert!(!b.app_learn_pending(0, ST_UNKNOWN, false));
    assert_eq!(b.app_service_move(0, 0, 100, 20), -3);
    b.m.mots[0].calibrated = true;
    b.m.mots[0].recal = true;
    assert!(b.app_learn_pending(0, ST_IDLE, false));
    b.m.mots[0].recal = false;
    b.m.valves[0].retry_learn = true;
    assert!(b.app_learn_pending(0, ST_IDLE, false));
    b.m.valves[0].retry_learn = false;
    b.m.valves[0].early_learn = true;
    assert!(b.app_learn_pending(0, ST_IDLE, false));
    b.m.valves[0].early_learn = false;
    b.m.valves[0].timed_learn = true;
    assert!(b.app_learn_pending(0, ST_IDLE, false));
    b.m.valves[0].timed_learn = false;
    b.m.valves[0].forced_learn = true;
    assert!(b.app_learn_pending(0, ST_IDLE, false));
    assert!(!b.app_learn_pending(12, ST_PRESENT, true));
}

#[test]
fn staop_w12_2_an_uncalibrated_valve_opens_fully_and_calibrates_at_its_own_target_change() {
    let mut b = begin();
    b.m.mots[4].calibrated = false;
    b.app.app_set_failsafe(255, 255);
    b.app_set_valveopen(4);
    assert_eq!(b.loop_actions(1), calls(&["appsetaction(p, 4, 0, 0)"]));
    b.m.mots[4].status = ST_IDLE;
    b.m.mots[4].actual_position = 100;
    assert_eq!(b.loop_actions(1), none());
    assert!(b.app_learn_pending(4, ST_IDLE, false));
    b.m.mots[4].target_position = 30;
    b.app_target_changed(4);
    assert_eq!(b.loop_actions(1), calls(&["appsetaction(l, 4, 0, 0)"]));
    assert!(!b.m.valves[4].touched);
    assert_eq!(b.m.valves[4].rejected_target, 30);
}

#[test]
fn app_get_valve_v3_flags_of_every_source_an_invalid_valve() {
    let mut b = begin();
    b.m.mots[1].calibrated = false;
    b.m.mots[1].needs_reference = true;
    b.m.mots[1].recal = true;
    b.m.mots[1].fault_reason = 5;
    b.m.valves[1].svc_hold = 1;
    b.m.valves[1].assembly_hold = true;
    b.env.snapshot[1].diag.early_run.on_move(true);
    let info = b.v3(1);
    assert_eq!(
        info.flags,
        VLV_FLAG_UNCALIBRATED
            + VLV_FLAG_NEEDS_REF
            + VLV_FLAG_RECAL
            + VLV_FLAG_EARLY_PENDING
            + VLV_FLAG_ASSEMBLY
            + VLV_FLAG_SVC_HOLD
    );
    assert_eq!(info.fault, 5);
    // the run starts again
    b.env.snapshot[1].diag.early_run.on_move(true);
    assert_eq!(b.v3(1).flags & VLV_FLAG_EARLY_PENDING, 0);
    let info = b.v3(12);
    assert_eq!(info.flags, 0);
    assert_eq!(info.fault, 0);
    assert_eq!(info.fs_pct, 255);
    assert_eq!(info.drive, 0);
    assert_eq!(info.retry_s, 0);
    assert_eq!(info.retries, 0);
}

#[test]
fn app_load_config_lease_timeout_failsafe_positions_and_learn_time_from_the_eeprom_mirror() {
    let mut b = AppBench::new();
    b.env.lease_source = LEASE_SOURCE_SETTINGS;
    b.env.eep.lease_timeout_min = 30;
    b.env.eep.learn_time_s = 3600;
    for v in 0..12 {
        b.env.eep.failsafe_pct[v] = (v * 10) as u8;
    }
    b.env.eep.failsafe_pct[11] = 101;
    b.app_setup();
    assert_eq!(b.app.app_lease_timeout(), 30);
    assert_eq!(b.app.app_get_learntime(), 3600);
    assert_eq!(b.m.valves[11].learn_time, 3600);
    assert_eq!(b.app.app_failsafe_pct(0), 0);
    assert_eq!(b.app.app_failsafe_pct(10), 100);
    assert_eq!(b.app.app_failsafe_pct(11), 50);
    // the mirror is corrected by the eeprom module only
    assert_eq!(b.env.eep.failsafe_pct[11], 101);
    b.env.eep.lease_timeout_min = 3;
    b.app_load_config();
    assert_eq!(b.app.app_lease_timeout(), 60);
    assert_eq!(b.env.eep.lease_timeout_min, 3);
    b.env.lease_source = LEASE_SOURCE_SAFETY;
    b.env.eep.lease_timeout_min = 0;
    b.app_load_config();
    assert_eq!(b.app.app_lease_timeout(), 0);
    b.env.lease_source = LEASE_SOURCE_DEFAULT;
    b.app_load_config();
    assert_eq!(b.app.app_lease_timeout(), 60);
    // the countdowns start again only for a changed learn time
    b.m.valves[0].learn_time = 7;
    b.app_load_config();
    assert_eq!(b.m.valves[0].learn_time, 7);
}
