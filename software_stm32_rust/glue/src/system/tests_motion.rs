// Port of test/native/glue/test_system_motion.cpp (glue_system): valve control of the whole STM
// glue with the valve sim: a blocked or jammed valve at its failsafe position and its automatic
// retry (K2, C-1), the failsafe after a cold boot with a silent ESP (C-2), targets before
// calibrations (S8), the stop of a calibration series (S8), warm resets with an expired lease and
// in a hand-over (K1-8, W2, C-8). The 3 h of C-1 are time jumps. Every case also compares its
// whole transcript with the C++ golden.
//
// The C++ bootController() of this file also sets every failsafe position of the EEPROM mirror
// to 255 before setup(): eeprom_read_layout() replaces the whole mirror at the set-up, so the
// value never reaches the controller and the port leaves it out.

use vdm_stm_core::valve_codes::{
    ValveFault, ST_BLOCKED, ST_CLOSING, ST_IDLE, ST_OPEN_CIRCUIT, ST_PRESENT, VLV_FLAG_FS_BLOCKED,
    VLV_FLAG_RETRY, VLV_FLAG_UNCALIBRATED,
};

use super::bench::{sim, Boot, Case, K_CAL_STATE};
use super::golden::Reset;
use crate::motor::CMD_A_OPEN;

#[test]
fn k2_c_1_a_blocked_valve_makes_one_failsafe_move_and_waits_for_its_retry() {
    let mut case = Case::new(
        "motion__k2_c_1_a_blocked_valve_makes_one_failsafe_move_and_waits_for_its",
        "system K2/C-1: a blocked valve makes one failsafe move and waits for its retry",
    );
    let mut b = case.boot(sim(0x0FFF & !(1 << 3)), |_| {});
    b.sim().valve[3].stroke = 2500;
    b.sim().valve[3].position = 1200;
    b.run_main(40000);
    assert_eq!(b.m().mots[3].status, ST_PRESENT);
    b.app().app_set_failsafe(3, 50);
    assert_eq!(b.exchange("stgtp 3 30\n"), "stgtp\r\n");
    assert!(b.run_main_until(|b| b.m().mots[3].status == ST_BLOCKED && b.idle(), 60000));
    assert!(b.run_main_until(|b| b.m().mots[3].actual_position != 0 && b.idle(), 20000));
    assert_eq!(b.m().mots[3].status, ST_BLOCKED);
    assert_eq!(b.m().mots[3].actual_position, (2500 / 89) as u8);
    assert_eq!(
        b.m().mots[3].fault_reason,
        ValveFault::StrokesTooShort as u8
    );
    let enables = b.sim().valve[3].enables;
    b.run_main(60000);
    assert_eq!(b.sim().valve[3].enables, enables);
    let info = b.app_get_valve_v3(3);
    assert_eq!(
        info.flags,
        VLV_FLAG_FS_BLOCKED + VLV_FLAG_UNCALIBRATED + VLV_FLAG_RETRY
    );
    assert_eq!(info.retries, 0);
    assert!(info.retry_s > 3500);
    assert!(info.retry_s <= 3600);
    // a new target is rejected and counted, the valve stays
    assert_eq!(b.exchange("stgtp 3 80\n"), "stgtp\r\n");
    b.run_main(5000);
    assert_eq!(b.m().valves[3].cmd_rejected, 1);
    assert_eq!(b.sim().valve[3].enables, enables);
    // the retry: one calibration series, blocked again, one failsafe move, the next retry in 6 h
    b.jump_s(info.retry_s);
    assert!(b.run_main_until(|b| b.m().mots[3].calib_active, 20000));
    assert!(b.run_main_until(|b| b.m().mots[3].status == ST_BLOCKED && b.idle(), 60000));
    assert!(b.run_main_until(|b| b.m().mots[3].actual_position != 0 && b.idle(), 20000));
    let after = b.sim().valve[3].enables;
    b.run_main(30000);
    assert_eq!(b.sim().valve[3].enables, after);
    let info = b.app_get_valve_v3(3);
    assert_eq!(info.retries, 1);
    assert!(info.retry_s > 21500);
    assert_eq!(b.sim().conflicts, 0);
    b.finish();
    case.check_golden();
}

#[test]
fn c_1_a_valve_jammed_at_25_makes_one_calibration_series_and_one_failsafe_move_per_retry() {
    let mut case = Case::new(
        "motion__c_1_a_valve_jammed_at_25_makes_one_calibration_series_and_one_fa",
        "system C-1: a valve jammed at 25 % makes one calibration series and one failsafe move per retry",
    );
    let mut b = case.boot(sim(0x0FFF & !(1 << 5)), |_| {});
    b.sim().valve[5].position = 600;
    b.sim().valve[5].jam_from = 900;
    b.sim().valve[5].jam_to = 3600;
    b.run_main(40000);
    assert_eq!(b.m().mots[5].status, ST_PRESENT);
    b.app().app_set_failsafe(5, 50);
    assert_eq!(b.exchange("stgtp 5 30\n"), "stgtp\r\n");
    assert!(b.run_main_until(|b| b.m().mots[5].status == ST_BLOCKED && b.idle(), 60000));
    assert_eq!(b.m().mots[5].calib_seq, 1);
    let blocked = b.sim().valve[5].enables;
    b.run_main(60000);
    // at most one failsafe move, then the valve stays
    assert!(b.sim().valve[5].enables <= blocked + 1);
    let enables = b.sim().valve[5].enables;
    assert_eq!(b.m().mots[5].status, ST_BLOCKED);
    let info = b.app_get_valve_v3(5);
    assert_eq!(info.retries, 0);
    assert!(info.retry_s > 60);
    assert!(info.retry_s <= 3600);
    // nothing until retryS reaches 0
    b.jump_s(info.retry_s - 60);
    assert_eq!(b.sim().valve[5].enables, enables);
    assert_eq!(b.m().mots[5].calib_seq, 1);
    b.jump_s(60);
    assert!(b.run_main_until(|b| b.m().mots[5].calib_active, 20000));
    assert!(b.run_main_until(|b| b.m().mots[5].status == ST_BLOCKED && b.idle(), 60000));
    assert_eq!(b.m().mots[5].calib_seq, 2);
    let again = b.sim().valve[5].enables;
    b.run_main(60000);
    assert!(b.sim().valve[5].enables <= again + 1);
    let info = b.app_get_valve_v3(5);
    assert_eq!(info.retries, 1);
    assert!(info.retry_s > 21000);
    assert_eq!(b.sim().conflicts, 0);
    b.finish();
    case.check_golden();
}

/// C-2: boot 0 stores startOnPower = failsafe = pct, the lease timeout 5 min and the calibration
/// of valve 0; after a power cycle the ESP stays silent and the lease expires
fn cold_boot_failsafe(slug: &'static str, name: &str, pct: u8) {
    let mut case = Case::new(slug, name);
    let mut b = case.boot(sim(0x0FF0), |_| {});
    b.run_main(40000);
    let p = b.motor_get_params();
    let sop = pct.to_string();
    assert_eq!(
        b.exchange(&std::format!("smotc {} {} {sop}\n", p.low_fac, p.high_fac)),
        "smotc\r\n"
    );
    b.exchange(&std::format!("sfspo 255 {sop}\n"));
    assert_eq!(b.exchange("slcfg 5\n"), "slcfg ok\r\n");
    assert_eq!(b.exchange("staln 0\n"), "staln\r\n");
    assert!(b.run_main_until(
        |b| {
            let m = b.m();
            m.mots[0].calib_seq != 0 && !m.mots[0].calib_active && m.valve_idle()
        },
        60000
    ));
    assert!(b.m().mots[0].calibrated);
    b.run_main(10000);
    b.reboot(Reset::PowerOn);

    let mut b = case.boot(sim(0x0FF0), |_| {});
    assert_eq!(b.app().app_lease_timeout(), 5);
    b.run_main(40000);
    let mut enables = [0u32; 4];
    let mut calibrated = [false; 4];
    for v in 0..4 {
        assert_eq!(b.app().app_failsafe_pct(v as u16), pct, "valve {v}");
        let mot = b.m().mots[v];
        assert_eq!(mot.actual_position, pct, "valve {v}");
        // every present valve is unreferenced after a cold boot
        if mot.calibrated {
            assert!(mot.needs_reference, "valve {v}");
        } else {
            assert_eq!(mot.status, ST_PRESENT, "valve {v}");
        }
        enables[v] = b.sim().valve[v].enables;
        calibrated[v] = mot.calibrated;
    }
    assert!(calibrated[0]);
    // nothing moves while the lease runs
    b.jump_s(200);
    assert_ne!(b.app().app_lease_state(), 2);
    for (v, &e) in enables.iter().enumerate() {
        assert_eq!(b.sim().valve[v].enables, e, "valve {v}");
    }
    b.jump_s(60);
    assert_eq!(b.app().app_lease_state(), 2);
    assert!(b.run_main_until(
        |b| {
            let m = b.m();
            m.mots[..4]
                .iter()
                .all(|mot| mot.status == ST_IDLE && !mot.needs_reference)
                && m.valve_idle()
        },
        300000
    ));
    for v in 0..4 {
        assert!(b.sim().valve[v].enables > enables[v], "valve {v}");
        let mot = b.m().mots[v];
        // a reference move (needsReference ends at an end stop) or a calibration
        if calibrated[v] {
            assert_eq!(mot.calib_seq, 0, "valve {v}");
        } else {
            assert!(mot.calib_seq >= 1, "valve {v}");
        }
        assert!(mot.calibrated, "valve {v}");
        assert_eq!(mot.actual_position, pct, "valve {v}");
    }
    assert_eq!(b.sim().conflicts, 0);
    b.finish();
    case.check_golden();
}

#[test]
fn c_2_failsafe_50_after_a_power_cycle_with_a_silent_esp_reference_or_calibration() {
    cold_boot_failsafe(
        "motion__c_2_failsafe_50_after_a_power_cycle_with_a_silent_esp_reference",
        "system C-2: failsafe 50 after a power cycle with a silent ESP, reference or calibration",
        50,
    );
}

#[test]
fn c_2_failsafe_30_after_a_power_cycle_with_a_silent_esp_reference_or_calibration() {
    cold_boot_failsafe(
        "motion__c_2_failsafe_30_after_a_power_cycle_with_a_silent_esp_reference",
        "system C-2: failsafe 30 after a power cycle with a silent ESP, reference or calibration",
        30,
    );
}

#[test]
fn s8_4_a_target_change_of_another_valve_goes_between_two_calibrations() {
    let mut case = Case::new(
        "motion__s8_4_a_target_change_of_another_valve_goes_between_two_calibrati",
        "system S8-4: a target change of another valve goes between two calibrations",
    );
    let mut b = case.boot(sim(0x0FF0), |_| {});
    b.run_main(40000);
    {
        let mut m = b.m_mut();
        for v in 0..4 {
            assert_eq!(m.mots[v].status, ST_PRESENT);
            m.mots[v].status = ST_IDLE;
            m.mots[v].calibrated = true;
            m.mots[v].scaler = 36;
        }
    }
    assert_eq!(b.exchange("staln 0\n"), "staln\r\n");
    assert_eq!(b.exchange("staln 1\n"), "staln\r\n");
    assert!(b.run_main_until(|b| b.m().mots[0].calib_active, 10000));
    let from = b.sim().ms;
    assert_eq!(b.exchange("stgtp 3 20\n"), "stgtp\r\n");
    assert!(b.run_main_until(|b| b.m().mots[1].calib_active, 60000));
    b.run_main(100);
    let move3 = b.first_status(3, ST_CLOSING, from);
    let learn1 = b.first_status(1, ST_CLOSING, from);
    let end0 = b.first_status(0, ST_IDLE, from);
    assert_ne!(end0, 0);
    assert!(move3 > end0);
    assert!(move3 < learn1);
    assert!(b.run_main_until(|b| !b.m().mots[1].calib_active && b.idle(), 60000));
    assert_eq!(b.m().mots[3].actual_position, 20);
    b.finish();
    case.check_golden();
}

#[test]
fn s8_3_sstop_255_ends_the_running_calibration_and_the_requested_ones() {
    let mut case = Case::new(
        "motion__s8_3_sstop_255_ends_the_running_calibration_and_the_requested_on",
        "system S8-3: sstop 255 ends the running calibration and the requested ones",
    );
    let mut b = case.boot(sim(0x0FF0), |_| {});
    b.run_main(40000);
    {
        let mut m = b.m_mut();
        for v in 0..4 {
            m.mots[v].status = ST_IDLE;
            m.mots[v].calibrated = true;
        }
    }
    assert_eq!(b.exchange("staln 255\n"), "staln\r\n");
    assert!(b.run_main_until(
        |b| b.m().mots[0].calib_active || b.m().mots[1].calib_active,
        10000
    ));
    assert_eq!(b.exchange("sstop 255\n"), "sstop 255 ok\r\n");
    b.run_main(20000);
    for v in 0..4u32 {
        let vi = v as usize;
        assert!(!b.m().mots[vi].calib_active, "valve {v}");
        assert!(!b.app_learn_pending(v as u16), "valve {v}");
        assert_eq!(b.m().mots[vi].status, ST_IDLE, "valve {v}");
        // calState bits 0/1 (requested, running) of gvlvx
        let reply = b.exchange(&std::format!("gvlvx {v}\n"));
        assert_eq!(Boot::field(&reply, K_CAL_STATE) & 3, 0, "valve {v}");
    }
    assert_eq!(b.exchange("sstop 20\n"), "sstop -1 err 1\r\n");
    b.finish();
    case.check_golden();
}

#[test]
fn k1_8_w2_a_warm_reset_keeps_the_expired_lease_and_the_valve_states_no_presence_test() {
    let mut case = Case::new(
        "motion__k1_8_w2_a_warm_reset_keeps_the_expired_lease_and_the_valve_state",
        "system K1-8/W2: a warm reset keeps the expired lease and the valve states, no presence test",
    );
    let mut b = case.boot(sim(0x0F00), |_| {});
    b.run_main(40000);
    assert_eq!(b.m().mots[0].status, ST_PRESENT);
    assert_eq!(b.m().mots[8].status, ST_OPEN_CIRCUIT);
    // stored like the ESP does it, the EEPROM values win over the defaults after the reset; the
    // failsafe holds every valve where it is
    assert_eq!(b.exchange("sfspo 255 255\n"), "sfspo 255 ok\r\n");
    assert_eq!(b.exchange("slcfg 5\n"), "slcfg ok\r\n");
    b.jump_s(300);
    assert_eq!(b.app().app_lease_state(), 2);
    b.run_main(1000);
    b.inject("reset\n");
    let reset = b.run(|b| {
        b.run_main(2000);
        panic!("the controller did not reset");
    });
    assert_eq!(reset, Some(Reset::Software));

    assert_eq!(case.boot_index(), 1);
    let mut b = case.boot(sim(0x0F00), |_| {});
    assert_eq!(b.app().app_lease_state(), 2);
    assert_eq!(b.app().app_lease_timeout(), 5);
    b.run_main(10000);
    for v in 0..12 {
        assert_eq!(b.sim().valve[v].enables, 0, "valve {v}");
        let want = if v < 8 { ST_PRESENT } else { ST_OPEN_CIRCUIT };
        assert_eq!(b.m().mots[v].status, want, "valve {v}");
    }
    assert_eq!(b.app().app_lease_state(), 2);
    b.finish();
    case.check_golden();
}

#[test]
fn c_8_a_reset_right_after_a_command_hand_over_tests_that_valve_again() {
    let mut case = Case::new(
        "motion__c_8_a_reset_right_after_a_command_hand_over_tests_that_valve_aga",
        "system C-8: a reset right after a command hand-over tests that valve again",
    );
    let mut b = case.boot(sim(0), |_| {});
    b.run_main(40000);
    b.run_main(100);
    assert!(b.idle());
    assert_eq!(b.appsetaction(CMD_A_OPEN, 4, 10), 0);
    b.reboot(Reset::Pin);

    let mut b = case.boot(sim(0), |_| {});
    b.run_main(10000);
    for v in 0..12 {
        let want = u32::from(v == 4);
        assert_eq!(b.sim().valve[v].enables, want, "valve {v}");
        assert_eq!(b.m().mots[v].status, ST_PRESENT, "valve {v}");
    }
    b.finish();
    case.check_golden();
}
