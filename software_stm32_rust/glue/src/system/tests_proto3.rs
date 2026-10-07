// Port of test/native/glue/test_system_proto3.cpp (glue_system): protocol 3 over the UART of the
// whole STM glue with the valve sim: the failsafe with slhbt and legacy polls, the assembly
// hold, a blocked calibration, a service move and its stop, safe mode after watchdog resets.
// Lease time is time jumps. Every case also compares its whole transcript with the C++ golden.

use vdm_stm_core::move_classifier::StopReason;
use vdm_stm_core::valve_codes::{
    ValveFault, ST_BLOCKED, ST_PRESENT, ST_UNKNOWN, VLV_FLAG_ASSEMBLY, VLV_FLAG_FS_BLOCKED,
    VLV_FLAG_FS_LEASE, VLV_FLAG_RETRY, VLV_FLAG_UNCALIBRATED,
};

use super::bench::{
    sim, Case, K_CAL_STATE, K_CMD_REJECTED, K_DRIVE, K_FAILSAFE_MASK, K_FAULT, K_FLAGS, K_LAST_CNT,
    K_LAST_STOP, K_LEASE, K_LEASE_TIMEOUT, K_POS, K_RETRY_S, K_SAFE_MODE, K_STATUS,
};
use super::golden::Reset;

// ---------------------------------------------------------------- lease and failsafe

#[test]
fn k1_4_slhbt_0_does_not_renew_the_failsafe_drives_slhbt_1_returns_to_the_targets() {
    let mut case = Case::new(
        "proto3__k1_4_slhbt_0_does_not_renew_the_failsafe_drives_slhbt_1_returns",
        "system K1-4: slhbt 0 does not renew, the failsafe drives, slhbt 1 returns to the targets",
    );
    let mut b = case.boot(sim(0x0FF0), |_| {});
    b.run_main(40000);
    b.calibrated_idle(4);
    for v in 0..4 {
        assert_eq!(b.exchange(&std::format!("stgtp {v} 60\n")), "stgtp\r\n");
    }
    assert!(b.run_main_until(
        |b| b.idle_at(0, 60) && b.idle_at(1, 60) && b.idle_at(2, 60) && b.idle_at(3, 60),
        20000
    ));
    assert_eq!(b.exchange("sfspo 255 20\n"), "sfspo 255 ok\r\n");
    assert_eq!(b.exchange("sfspo 3 255\n"), "sfspo 3 ok\r\n");
    assert!(b.exchange("slhbt 1\n").starts_with("slhbt 1 "));
    assert_eq!(b.exchange("slcfg 5\n"), "slcfg ok\r\n");
    for _ in 0..4 {
        b.jump_s(60);
        assert!(b.exchange("slhbt 0\n").starts_with("slhbt 1 "));
    }
    let enables3 = b.sim().valve[3].enables;
    b.jump_s(60);
    assert_eq!(b.exchange("slhbt 0\n"), "slhbt 2 0\r\n");
    assert!(b.run_main_until(
        |b| b.idle_at(0, 20) && b.idle_at(1, 20) && b.idle_at(2, 20),
        20000
    ));
    assert_eq!(b.m().mots[3].actual_position, 60);
    assert_eq!(b.sim().valve[3].enables, enables3);
    assert_eq!(b.exchange("gtgtp 0\n"), "gtgtp 0 60 \r\n");
    assert_eq!(b.gstax(K_LEASE), 2);
    assert_eq!(b.gstax(K_FAILSAFE_MASK), 0x0007);
    assert_eq!(b.gvlvy(0, K_FLAGS), i64::from(VLV_FLAG_FS_LEASE));
    assert_eq!(b.gvlvy(0, K_DRIVE), 20);
    assert_eq!(b.gvlvy(3, K_FLAGS), 0);
    // a new target during the failsafe is stored, the valve stays
    let enables0 = b.sim().valve[0].enables;
    assert_eq!(b.exchange("stgtp 0 30\n"), "stgtp\r\n");
    b.run_main(5000);
    assert_eq!(b.exchange("gtgtp 0\n"), "gtgtp 0 30 \r\n");
    assert_eq!(b.m().mots[0].actual_position, 20);
    assert_eq!(b.sim().valve[0].enables, enables0);
    // slhbt 1: every valve back to its stored target
    assert_eq!(b.exchange("slhbt 1\n"), "slhbt 1 300\r\n");
    assert!(b.run_main_until(
        |b| b.idle_at(0, 30) && b.idle_at(1, 60) && b.idle_at(2, 60),
        20000
    ));
    assert_eq!(b.gstax(K_LEASE), 1);
    assert_eq!(b.gstax(K_FAILSAFE_MASK), 0);
    assert_eq!(b.sim().conflicts, 0);
    b.finish();
    case.check_golden();
}

#[test]
fn k1_5_gvlvx_polls_of_an_esp_2_0_0_keep_a_stored_lease_gvlvy_polls_do_not() {
    let mut case = Case::new(
        "proto3__k1_5_gvlvx_polls_of_an_esp_2_0_0_keep_a_stored_lease_gvlvy_polls",
        "system K1-5: gvlvx polls of an ESP 2.0.0 keep a stored lease, gvlvy polls do not",
    );
    let mut b = case.boot(sim(0x0FFF), |_| {});
    b.run_main(5000);
    assert_eq!(b.exchange("slcfg 5\n"), "slcfg ok\r\n");
    b.run_main(5000);
    b.reboot(Reset::PowerOn);

    let mut b = case.boot(sim(0x0FFF), |_| {});
    // no lease command in this run: the legacy polls renew
    assert_eq!(b.gstax(K_LEASE_TIMEOUT), 5);
    for _ in 0..20 {
        b.jump_s(60);
        b.exchange("gvlvx 0\n");
        assert_eq!(b.gstax(K_LEASE), 1);
    }
    for _ in 0..4 {
        b.jump_s(60);
        b.exchange("gvlvy 0\n");
        assert_eq!(b.gstax(K_LEASE), 1);
    }
    b.jump_s(60);
    b.exchange("gvlvy 0\n");
    assert_eq!(b.gstax(K_LEASE), 2);
    b.finish();
    case.check_golden();
}

#[test]
fn k1_7_staop_during_the_failsafe_opens_fully_and_holds_100_until_the_next_stgtp() {
    let mut case = Case::new(
        "proto3__k1_7_staop_during_the_failsafe_opens_fully_and_holds_100_until_t",
        "system K1-7: staop during the failsafe opens fully and holds 100 until the next stgtp",
    );
    let mut b = case.boot(sim(0x0FF0), |_| {});
    b.run_main(40000);
    b.calibrated_idle(4);
    assert_eq!(b.exchange("sfspo 255 255\n"), "sfspo 255 ok\r\n");
    assert_eq!(b.exchange("sfspo 2 20\n"), "sfspo 2 ok\r\n");
    assert_eq!(b.exchange("slcfg 5\n"), "slcfg ok\r\n");
    b.jump_s(300);
    assert_eq!(b.gstax(K_LEASE), 2);
    assert!(b.run_main_until(|b| b.idle_at(2, 20), 20000));
    assert_eq!(b.exchange("staop 2\n"), "staop \r\n");
    assert!(b.run_main_until(|b| b.m().mots[2].actual_position == 100 && b.idle(), 20000));
    b.run_main(20000);
    assert_eq!(b.m().mots[2].actual_position, 100);
    assert_eq!(b.gvlvy(2, K_FLAGS), i64::from(VLV_FLAG_ASSEMBLY));
    assert_eq!(b.gvlvy(2, K_DRIVE), 100);
    assert_eq!(b.gstax(K_FAILSAFE_MASK), 0);
    assert_eq!(b.exchange("stgtp 2 70\n"), "stgtp\r\n");
    assert!(b.run_main_until(|b| b.idle_at(2, 20), 20000));
    assert_eq!(b.gvlvy(2, K_FLAGS), i64::from(VLV_FLAG_FS_LEASE));
    assert_eq!(b.gstax(K_FAILSAFE_MASK), 0x0004);
    assert_eq!(b.sim().conflicts, 0);
    b.finish();
    case.check_golden();
}

// ---------------------------------------------------------------- blocked valve

#[test]
fn k2_3_a_calibration_with_every_stroke_too_short_ends_blocked_at_the_failsafe_position() {
    let mut case = Case::new(
        "proto3__k2_3_a_calibration_with_every_stroke_too_short_ends_blocked_at_t",
        "system K2-3: a calibration with every stroke too short ends blocked at the failsafe position",
    );
    let mut b = case.boot(sim(0x0FFF & !(1 << 3)), |_| {});
    // a stroke of 100 % at the default scaler (89 counts per %): the failsafe move ends at 50 %
    b.sim().valve[3].stroke = 8900;
    b.run_main(40000);
    assert_eq!(b.m().mots[3].status, ST_PRESENT);
    assert_eq!(b.exchange("slcfg 0\n"), "slcfg ok\r\n");
    assert_eq!(b.exchange("smotc 17 17 30 60000 0\n"), "smotc\r\n");
    assert_eq!(b.exchange("staln 3\n"), "staln\r\n");
    assert!(b.run_main_until(|b| b.m().mots[3].calib_active, 10000));
    assert!(b.run_main_until(|b| b.m().mots[3].status == ST_BLOCKED && b.idle(), 60000));
    assert!(b.run_main_until(|b| b.m().mots[3].actual_position == 50 && b.idle(), 20000));
    b.run_main(2000);
    assert_eq!(b.gvlvx(3, K_STATUS), i64::from(ST_BLOCKED));
    assert_eq!(b.gvlvx(3, K_POS), 50);
    assert_ne!(b.gvlvx(3, K_CAL_STATE) & 8, 0);
    // FS_BLOCKED | RETRY, and UNCALIBRATED: no calibration of this valve ever succeeded
    assert_eq!(
        b.gvlvy(3, K_FLAGS),
        i64::from(VLV_FLAG_FS_BLOCKED + VLV_FLAG_RETRY + VLV_FLAG_UNCALIBRATED)
    );
    assert_eq!(b.gvlvy(3, K_FAULT), ValveFault::StrokesTooShort as i64);
    let retry_s = b.gvlvy(3, K_RETRY_S);
    assert!(retry_s > 3500);
    assert!(retry_s <= 3600);
    let rejected = b.gvlvx(3, K_CMD_REJECTED);
    let enables = b.sim().valve[3].enables;
    assert_eq!(b.exchange("stgtp 3 80\n"), "stgtp\r\n");
    b.run_main(5000);
    assert_eq!(b.gvlvx(3, K_CMD_REJECTED), rejected + 1);
    assert_eq!(b.gvlvx(3, K_POS), 50);
    assert_eq!(b.sim().valve[3].enables, enables);
    // failsafe hold: the valve stays where the calibration left it
    assert_eq!(b.exchange("sfspo 3 255\n"), "sfspo 3 ok\r\n");
    assert_eq!(b.exchange("staln 3\n"), "staln\r\n");
    assert!(b.run_main_until(|b| b.m().mots[3].calib_active, 10000));
    assert!(b.run_main_until(|b| !b.m().mots[3].calib_active && b.idle(), 60000));
    b.run_main(5000);
    assert_eq!(b.gvlvx(3, K_STATUS), i64::from(ST_BLOCKED));
    assert_eq!(b.gvlvx(3, K_POS), 0);
    assert_eq!(b.sim().conflicts, 0);
    b.finish();
    case.check_golden();
}

// ---------------------------------------------------------------- service move and stop

#[test]
fn s8_2_sstop_ends_a_service_move_where_it_is_the_valve_is_not_moved_back() {
    let mut case = Case::new(
        "proto3__s8_2_sstop_ends_a_service_move_where_it_is_the_valve_is_not_move",
        "system S8-2: sstop ends a service move where it is, the valve is not moved back",
    );
    let mut b = case.boot(sim(0x0FF0), |_| {});
    b.run_main(40000);
    b.calibrated_idle(4);
    b.sim().valve[2].pulses_per_ms = 0.5;
    let pos0 = b.gvlvx(2, K_POS);
    assert_eq!(b.exchange("svmov 2 0 10000 40\n"), "svmov 2 ok\r\n");
    b.run_main(2500);
    assert_eq!(b.exchange("sstop 2\n"), "sstop 2 ok\r\n");
    assert!(b.run_main_until(|b| b.idle(), 5000));
    let last_cnt = b.gvlvx(2, K_LAST_CNT);
    assert_eq!(b.gvlvx(2, K_LAST_STOP), StopReason::Aborted as i64);
    assert!(last_cnt > 360);
    assert!(last_cnt < 10000);
    // the position follows the counted pulses (scaler 36 counts per %)
    let pos = b.gvlvx(2, K_POS);
    assert!(pos >= pos0 + last_cnt / 36 - 1);
    assert!(pos <= pos0 + last_cnt / 36 + 1);
    let enables = b.sim().valve[2].enables;
    b.jump_s(150);
    b.jump_s(150);
    b.run_main(5000);
    assert_eq!(b.sim().valve[2].enables, enables);
    assert_eq!(b.gvlvx(2, K_POS), pos);
    b.finish();
    case.check_golden();
}

// ---------------------------------------------------------------- safe mode

#[test]
fn s9_2_after_three_watchdog_resets_no_valve_moves_until_ssafe_0() {
    let mut case = Case::new(
        "proto3__s9_2_after_three_watchdog_resets_no_valve_moves_until_ssafe_0",
        "system S9-2: after three watchdog resets no valve moves until ssafe 0",
    );
    while case.boot_index() < 3 {
        let mut b = case.boot(sim(0x0FF0), |_| {});
        b.run_main(2000);
        b.reboot(Reset::Watchdog);
    }
    let mut b = case.boot(sim(0x0FF0), |_| {});
    assert_eq!(b.gstax(K_SAFE_MODE), 1);
    b.run_main(40000);
    for v in 0..4 {
        assert_eq!(b.sim().valve[v].enables, 0, "valve {v}");
        assert_eq!(b.m().mots[v].status, ST_UNKNOWN, "valve {v}");
    }
    assert_eq!(b.exchange("ssafe 1\n"), "ssafe err\r\n");
    assert_eq!(b.exchange("ssafe 0\n"), "ssafe ok\r\n");
    assert_eq!(b.gstax(K_SAFE_MODE), 0);
    b.run_main(40000);
    for v in 0..4 {
        assert!(b.sim().valve[v].enables > 0, "valve {v}");
        assert_eq!(b.m().mots[v].status, ST_PRESENT, "valve {v}");
    }
    b.finish();
    case.check_golden();
}
