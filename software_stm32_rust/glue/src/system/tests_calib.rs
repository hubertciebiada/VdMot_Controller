// Port of test/native/glue/test_system_calib.cpp (glue_system): calibration records of the whole
// STM glue in the real eeprom module: counts, position and status after a reset, counts and a
// reference move after a power cycle, a blocked calibration after a power cycle. Every case
// also compares its whole transcript with the C++ golden.

use vdm_stm_core::valve_codes::{
    ST_BLOCKED, ST_IDLE, ST_PRESENT, VLV_FLAG_CAL_RESTORED, VLV_FLAG_NEEDS_REF, VLV_FLAG_RECAL,
    VLV_FLAG_UNCALIBRATED,
};

use super::bench::{
    sim, Boot, Case, K_CAL_STATE, K_CC, K_FLAGS, K_LAST_MS, K_LAST_REQ, K_LAST_STOP, K_MEAN_CUR,
    K_OC, K_POS, K_STATUS, K_TARGET,
};
use super::golden::Reset;

/// boot 0: valve 0 (the only one connected) calibrated by staln 0, its record written
fn calibrate_valve0(b: &mut Boot<'_>) {
    assert_eq!(b.m().mots[0].status, ST_PRESENT);
    assert_eq!(b.exchange("staln 0\n"), "staln\r\n");
    assert!(b.run_main_until(|b| b.m().mots[0].calib_active, 10000));
    assert!(b.run_main_until(|b| !b.m().mots[0].calib_active && b.idle(), 60000));
    assert!(b.m().mots[0].calibrated);
    // the record is written 3 s later
    b.run_main(5000);
    assert_eq!(b.exchange("eepst\n"), "eepst 1 \r\n");
}

/// C++ `struct Kept { long oc, cc, meanCur, pos, target; }` as the C++ case stores it in the
/// EEPROM's free area at 0x1000 (x86-64: five little-endian 8-byte longs)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Kept {
    oc: i64,
    cc: i64,
    mean_cur: i64,
    pos: i64,
    target: i64,
}

impl Kept {
    fn of_valve0(b: &mut Boot<'_>) -> Kept {
        Kept {
            oc: b.gvlvx(0, K_OC),
            cc: b.gvlvx(0, K_CC),
            mean_cur: b.gvlvx(0, K_MEAN_CUR),
            pos: b.gvlvx(0, K_POS),
            target: b.gvlvx(0, K_TARGET),
        }
    }

    fn store(&self, b: &Boot<'_>) {
        let mut e = b.eeprom();
        for (i, v) in [self.oc, self.cc, self.mean_cur, self.pos, self.target]
            .into_iter()
            .enumerate()
        {
            e.bytes[0x1000 + 8 * i..0x1008 + 8 * i].copy_from_slice(&v.to_le_bytes());
        }
    }

    fn load(b: &Boot<'_>) -> Kept {
        let e = b.eeprom();
        let at = |i: usize| {
            i64::from_le_bytes(e.bytes[0x1000 + 8 * i..0x1008 + 8 * i].try_into().unwrap())
        };
        Kept {
            oc: at(0),
            cc: at(1),
            mean_cur: at(2),
            pos: at(3),
            target: at(4),
        }
    }
}

#[test]
fn w2_5_after_a_calibration_and_a_reset_the_counts_position_and_status_come_back() {
    let mut case = Case::new(
        "calib__w2_5_after_a_calibration_and_a_reset_the_counts_position_and_sta",
        "system W2-5: after a calibration and a reset the counts, position and status come back",
    );
    let mut b = case.boot(sim(0x0FFE), |_| {});
    b.run_main(40000);
    calibrate_valve0(&mut b);
    let before = Kept::of_valve0(&mut b);
    // what the next boot compares with: written into the EEPROM's free area
    before.store(&b);
    b.reset_controller();

    let mut b = case.boot(sim(0x0FFE), |_| {});
    let before = Kept::load(&b);
    assert!(before.oc > 100);
    let after = Kept::of_valve0(&mut b);
    assert_eq!(after.oc, before.oc);
    assert_eq!(after.cc, before.cc);
    assert_eq!(after.mean_cur, before.mean_cur);
    assert_eq!(after.pos, before.pos);
    assert_eq!(after.target, before.target);
    assert_eq!(b.gvlvx(0, K_STATUS), i64::from(ST_IDLE));
    assert_eq!(b.gvlvx(0, K_LAST_STOP), 0);
    assert_eq!(b.gvlvx(0, K_LAST_MS), 0);
    let flags = b.gvlvy(0, K_FLAGS);
    assert_ne!(flags & i64::from(VLV_FLAG_CAL_RESTORED), 0);
    assert_eq!(flags & i64::from(VLV_FLAG_NEEDS_REF), 0);
    assert_eq!(flags & i64::from(VLV_FLAG_UNCALIBRATED), 0);
    b.finish();
    case.check_golden();
}

#[test]
fn w2_6_after_a_power_cycle_the_counts_come_back_a_reference_move_instead_of_a_calibration() {
    let mut case = Case::new(
        "calib__w2_6_after_a_power_cycle_the_counts_come_back_a_reference_move_i",
        "system W2-6: after a power cycle the counts come back, a reference move instead of a calibration",
    );
    let mut b = case.boot(sim(0x0FFE), |_| {});
    b.run_main(40000);
    calibrate_valve0(&mut b);
    let k = Kept::of_valve0(&mut b);
    k.store(&b);
    b.reboot(Reset::PowerOn);

    let mut b = case.boot(sim(0x0FFE), |_| {});
    let before = Kept::load(&b);
    // the presence test
    b.run_main(40000);
    assert_eq!(b.gvlvx(0, K_OC), before.oc);
    assert_eq!(b.gvlvx(0, K_CC), before.cc);
    assert_eq!(b.gvlvx(0, K_STATUS), i64::from(ST_IDLE));
    let flags = b.gvlvy(0, K_FLAGS);
    assert_eq!(flags & i64::from(VLV_FLAG_UNCALIBRATED), 0);
    assert_ne!(flags & i64::from(VLV_FLAG_NEEDS_REF), 0);
    let enables = b.sim().valve[0].enables;
    assert_eq!(b.exchange("stgtp 0 60\n"), "stgtp\r\n");
    assert!(b.run_main_until(
        |b| b.idle_at(0, 60) && !b.m().mots[0].needs_reference,
        30000
    ));
    b.run_main(2000);
    assert_eq!(b.sim().valve[0].enables, enables + 2);
    assert_eq!(b.gvlvx(0, K_LAST_REQ), 40 * (before.oc / 100));
    assert_eq!(b.gvlvx(0, K_CAL_STATE) & 3, 0);
    assert_eq!(b.m().mots[0].calib_seq, 0);
    let flags = b.gvlvy(0, K_FLAGS);
    assert_eq!(flags & i64::from(VLV_FLAG_NEEDS_REF), 0);
    b.finish();
    case.check_golden();
}

#[test]
fn w2_8_a_calibration_that_ended_blocked_needs_a_full_calibration_after_a_power_cycle() {
    let mut case = Case::new(
        "calib__w2_8_a_calibration_that_ended_blocked_needs_a_full_calibration_a",
        "system W2-8: a calibration that ended blocked needs a full calibration after a power cycle",
    );
    let mut b = case.boot(sim(0x0FFE), |_| {});
    b.run_main(40000);
    assert_eq!(b.exchange("smotc 17 17 30 60000 0\n"), "smotc\r\n");
    assert_eq!(b.exchange("staln 0\n"), "staln\r\n");
    assert!(b.run_main_until(|b| b.m().mots[0].calib_active, 10000));
    assert!(b.run_main_until(|b| b.m().mots[0].status == ST_BLOCKED && b.idle(), 60000));
    assert_eq!(b.exchange("smotc 17 17 30 100 0\n"), "smotc\r\n");
    b.run_main(10000);
    assert_eq!(b.exchange("eepst\n"), "eepst 1 \r\n");
    b.reboot(Reset::PowerOn);

    let mut b = case.boot(sim(0x0FFE), |_| {});
    // the presence test
    b.run_main(40000);
    assert_eq!(b.gvlvx(0, K_STATUS), i64::from(ST_PRESENT));
    assert_ne!(b.gvlvy(0, K_FLAGS) & i64::from(VLV_FLAG_RECAL), 0);
    assert_eq!(b.sim().valve[0].enables, 1);
    assert_eq!(b.exchange("stgtp 0 60\n"), "stgtp\r\n");
    assert!(b.run_main_until(|b| b.m().mots[0].calib_active, 10000));
    assert!(b.run_main_until(|b| !b.m().mots[0].calib_active && b.idle(), 60000));
    assert_eq!(b.m().mots[0].calib_seq, 1);
    assert_eq!(b.gvlvx(0, K_STATUS), i64::from(ST_IDLE));
    assert_eq!(b.gvlvx(0, K_POS), 60);
    b.finish();
    case.check_golden();
}
