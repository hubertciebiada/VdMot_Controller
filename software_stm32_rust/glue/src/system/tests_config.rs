// Port of test/native/glue/test_system_config.cpp (glue_system): lease settings, failsafe
// positions and learn time of the whole STM glue across resets, power cycles and a failing
// EEPROM, over the UART with the real eeprom module. Every case also compares its whole
// transcript with the C++ golden.

use std::vec;

use vdm_stm_core::config_store::{CFG_READ_FAILED, CFG_SAFETY_CORRUPT};
use vdm_stm_core::eeprom_layout::SAFETY_BLOCK_ADDRESS;

use super::bench::{glcfg, sim, Boot, Case, K_CFG_FLAGS, K_LEASE, K_LEASE_TIMEOUT};
use super::golden::Reset;

const VALVES: usize = 12;

#[test]
fn k1_6_s3_2_slcfg_sfspo_and_stlnt_survive_a_reset_and_a_power_cycle() {
    let mut case = Case::new(
        "config__k1_6_s3_2_slcfg_sfspo_and_stlnt_survive_a_reset_and_a_power_cycl",
        "system K1-6/S3-2: slcfg, sfspo and stlnt survive a reset and a power cycle",
    );
    let mut b = case.boot(sim(0), |_| {});
    // the first start's configuration write
    b.run_main(5000);
    assert_eq!(b.exchange("slcfg 60\n"), "slcfg ok\r\n");
    assert_eq!(b.exchange("sfspo 255 40\n"), "sfspo 255 ok\r\n");
    assert_eq!(b.exchange("stlnt 0\n"), "stlnt\r\n");
    b.run_main(5000);
    b.reset_controller();

    let mut b = case.boot(sim(0), |_| {});
    assert_eq!(b.last_reset(), Reset::Software);
    assert_eq!(
        b.exchange("glcfg\n"),
        "glcfg 60 40 40 40 40 40 40 40 40 40 40 40 40\r\n"
    );
    assert_eq!(b.exchange("gtlnt\n"), "gtlnt 0\r\n");
    assert_eq!(b.exchange("slcfg 90\n"), "slcfg ok\r\n");
    assert_eq!(b.exchange("sfspo 3 20\n"), "sfspo 3 ok\r\n");
    assert_eq!(b.exchange("stlnt 3600\n"), "stlnt\r\n");
    b.run_main(5000);
    b.reboot(Reset::PowerOn);

    assert_eq!(case.boot_index(), 2);
    let mut b = case.boot(sim(0), |_| {});
    assert_eq!(
        b.exchange("glcfg\n"),
        glcfg(90, &[40, 40, 40, 20, 40, 40, 40, 40, 40, 40, 40, 40])
    );
    assert_eq!(b.exchange("gtlnt\n"), "gtlnt 3600\r\n");
    assert_eq!(b.gstax(K_LEASE_TIMEOUT), 90);
    assert_eq!(b.gstax(K_CFG_FLAGS), 0);
    b.finish();
    case.check_golden();
}

/// `bootController(rig, 0, before)` with every read of boot 1 failing
fn boot_with_failing_reads_at_boot_1(case: &mut Case) -> Boot<'_> {
    let index = case.boot_index();
    case.boot(sim(0), move |s| {
        if index == 1 {
            s.eeprom.borrow_mut().fail_reads_from = 1;
        }
    })
}

#[test]
fn c_5_a_new_chip_and_a_failed_eeprom_read_give_60_min_and_50_the_re_read_the_stored_values() {
    let mut case = Case::new(
        "config__c_5_a_new_chip_and_a_failed_eeprom_read_give_60_min_and_50_the_r",
        "system C-5: a new chip and a failed EEPROM read give 60 min and 50 %, the re-read the stored values",
    );
    let mut b = boot_with_failing_reads_at_boot_1(&mut case);
    assert_eq!(b.exchange("glcfg\n"), glcfg(60, &[50; VALVES]));
    b.run_main(5000);
    assert_eq!(b.exchange("slcfg 90\n"), "slcfg ok\r\n");
    assert_eq!(b.exchange("sfspo 255 20\n"), "sfspo 255 ok\r\n");
    b.run_main(5000);
    b.reboot(Reset::PowerOn);

    let mut b = boot_with_failing_reads_at_boot_1(&mut case);
    // never 0 %: the defaults while the EEPROM cannot be read
    assert_eq!(b.gstax(K_CFG_FLAGS), i64::from(CFG_READ_FAILED));
    assert_eq!(b.exchange("glcfg\n"), glcfg(60, &[50; VALVES]));
    b.eeprom().fail_reads_from = 0;
    // the re-read 30 s later
    b.run_main(31000);
    assert_eq!(b.gstax(K_CFG_FLAGS), 0);
    assert_eq!(b.exchange("glcfg\n"), glcfg(90, &[20; VALVES]));
    assert_eq!(b.gstax(K_LEASE_TIMEOUT), 90);
    b.finish();
    case.check_golden();
}

#[test]
fn slcfg_and_sfspo_of_the_default_values_while_the_eeprom_read_fails_survive_the_re_read() {
    let mut case = Case::new(
        "config__slcfg_and_sfspo_of_the_default_values_while_the_eeprom_read_fail",
        "system: slcfg and sfspo of the default values while the EEPROM read fails survive the re-read",
    );
    let mut b = boot_with_failing_reads_at_boot_1(&mut case);
    // the first start's configuration write
    b.run_main(5000);
    assert_eq!(b.exchange("slcfg 90\n"), "slcfg ok\r\n");
    assert_eq!(b.exchange("sfspo 255 30\n"), "sfspo 255 ok\r\n");
    b.run_main(5000);
    b.reboot(Reset::PowerOn);

    let mut b = boot_with_failing_reads_at_boot_1(&mut case);
    assert_eq!(b.gstax(K_CFG_FLAGS), i64::from(CFG_READ_FAILED));
    // the defaults of the failed read, now requested on purpose: the EEPROM holds 90 and 30
    assert_eq!(b.exchange("slcfg 60\n"), "slcfg ok\r\n");
    assert_eq!(b.exchange("sfspo 255 50\n"), "sfspo 255 ok\r\n");
    b.eeprom().fail_reads_from = 0;
    // the re-read 30 s later
    b.run_main(31000);
    assert_eq!(b.gstax(K_CFG_FLAGS), 0);
    assert_eq!(b.gstax(K_LEASE_TIMEOUT), 60);
    assert_eq!(b.exchange("glcfg\n"), glcfg(60, &[50; VALVES]));
    // and written
    b.run_main(5000);
    b.reboot(Reset::PowerOn);

    assert_eq!(case.boot_index(), 2);
    let mut b = boot_with_failing_reads_at_boot_1(&mut case);
    assert_eq!(b.exchange("glcfg\n"), glcfg(60, &[50; VALVES]));
    b.finish();
    case.check_golden();
}

#[test]
fn sfspo_after_a_warm_reset_with_a_failing_eeprom_read_stays_when_the_re_read_finds_block_b_damaged(
) {
    let mut case = Case::new(
        "config__sfspo_after_a_warm_reset_with_a_failing_eeprom_read_stays_when_t",
        "system: sfspo after a warm reset with a failing EEPROM read stays when the re-read finds block B damaged",
    );
    let mut b = boot_with_failing_reads_at_boot_1(&mut case);
    b.run_main(5000);
    assert_eq!(b.exchange("sfspo 255 30\n"), "sfspo 255 ok\r\n");
    b.run_main(5000);
    b.reset_controller();

    let mut b = boot_with_failing_reads_at_boot_1(&mut case);
    assert_eq!(b.last_reset(), Reset::Software);
    assert_eq!(b.gstax(K_CFG_FLAGS), i64::from(CFG_READ_FAILED));
    // the warm copies
    assert_eq!(b.exchange("glcfg\n"), glcfg(60, &[30; VALVES]));
    assert_eq!(b.exchange("sfspo 3 20\n"), "sfspo 3 ok\r\n");
    // block B damaged
    b.eeprom().bytes[usize::from(SAFETY_BLOCK_ADDRESS) + 3] ^= 1;
    b.eeprom().fail_reads_from = 0;
    // the re-read 30 s later
    b.run_main(31000);
    assert_eq!(b.gstax(K_CFG_FLAGS), i64::from(CFG_SAFETY_CORRUPT));
    let mut fs = vec![30u8; VALVES];
    fs[3] = 20;
    assert_eq!(b.exchange("glcfg\n"), glcfg(60, &fs));
    // block B written again
    b.run_main(5000);
    b.reboot(Reset::PowerOn);

    assert_eq!(case.boot_index(), 2);
    let mut b = boot_with_failing_reads_at_boot_1(&mut case);
    assert_eq!(b.gstax(K_CFG_FLAGS), 0);
    // valve 3
    let reply = b.exchange("glcfg\n");
    assert_eq!(Boot::field(&reply, 5), 20);
    b.finish();
    case.check_golden();
}

#[test]
fn c_5_a_warm_reset_with_the_lease_expired_and_the_eeprom_read_failing_lease_2_at_once() {
    let mut case = Case::new(
        "config__c_5_a_warm_reset_with_the_lease_expired_and_the_eeprom_read_fail",
        "system C-5: a warm reset with the lease expired and the EEPROM read failing: lease 2 at once",
    );
    let mut b = boot_with_failing_reads_at_boot_1(&mut case);
    b.run_main(5000);
    // the failsafe holds every valve
    assert_eq!(b.exchange("sfspo 255 255\n"), "sfspo 255 ok\r\n");
    assert_eq!(b.exchange("sfspo 4 30\n"), "sfspo 4 ok\r\n");
    assert_eq!(b.exchange("slcfg 5\n"), "slcfg ok\r\n");
    b.run_main(5000);
    b.jump_s(300);
    assert_eq!(b.gstax(K_LEASE), 2);
    b.reset_controller();

    let mut b = boot_with_failing_reads_at_boot_1(&mut case);
    assert_eq!(b.last_reset(), Reset::Software);
    assert_eq!(b.gstax(K_CFG_FLAGS), i64::from(CFG_READ_FAILED));
    assert_eq!(b.gstax(K_LEASE), 2);
    assert_eq!(b.gstax(K_LEASE_TIMEOUT), 5);
    let mut fs = vec![255u8; VALVES];
    fs[4] = 30;
    assert_eq!(b.exchange("glcfg\n"), glcfg(5, &fs));
    b.finish();
    case.check_golden();
}
