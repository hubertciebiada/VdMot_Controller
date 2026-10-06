// Port of test/native/glue/test_eeprom__mut.cpp: a failed calibration record write, the read
// failure budget of one read, the slot bookkeeping after a successful write and the loop
// return value; and new cases for the edges the mutation gate found (no C++ counterpart).

use super::tests::{valid_slot, Rig};
use super::*;
use vdm_stm_core::config_blocks::CALIB_VALID;
use vdm_stm_core::config_store::{CFG_READ_FAILED, CHANGED_LEARN_TIME};
use vdm_stm_core::replies_v2::EEP_STATE_OK;

/// the consistent configuration of the C++ file: fewer fields than the one of test_eeprom.cpp
fn store_consistent(r: &mut Rig) {
    let mut lay = EepromLayout::default();
    lay.cfg.layout.b_slave = 1;
    lay.cfg.layout.currentbound_low_fac = 18;
    lay.cfg.layout.currentbound_high_fac = 19;
    for v in 0..12u8 {
        valid_slot(&mut lay.cfg.layout.owsensors1[usize::from(v)], v);
        valid_slot(&mut lay.cfg.layout.owsensors2[usize::from(v)], 100 + v);
    }
    lay.cfg.lease_timeout_min = 90;
    assert_eq!(r.write_layout(&lay), 0);
    r.dev.ops.clear();
    r.dbg.take_tx();
}

#[test]
fn eepromloop_returns_0_a_failed_calibration_record_write_is_not_a_successful_step() {
    let mut r = Rig::new();
    store_consistent(&mut r);
    r.boot();
    r.settle(40);
    let steps = r.ee.writes();
    r.dev.fail_writes_from = r.dev.writes + 1;
    let rec = CalibRecord {
        opening_count: 3000,
        closing_count: 3100,
        mean_current: 20,
        flags: CALIB_VALID,
    };
    r.ee.store_calib(2, &rec);
    for _ in 0..3 {
        assert_eq!(r.loop_(), 0);
    }
    assert_eq!(r.dev.ops.len(), 1);
    assert_eq!(r.ee.writes(), steps);
    assert_ne!(r.ee.state(), EEP_STATE_OK);
    assert!(r.dbg.take_tx().contains("write error, aborted"));
    r.dev.fail_writes_from = 0;
    r.settle(40);
    assert_eq!(r.ee.writes(), steps + 1);
}

#[test]
fn eeprom_read_layout_two_failed_transfers_in_one_read_are_retried_the_third_ends_the_read() {
    let mut r = Rig::new();
    store_consistent(&mut r);
    r.dev.fail_reads_from = r.dev.reads + 1;
    r.dev.fail_reads_count = 2;
    assert_eq!(r.boot(), 0);
    assert_eq!(r.ee.cfg_flags(), 0);
    r.dev.fail_reads_from = r.dev.reads + 1;
    r.dev.fail_reads_count = 3;
    assert_eq!(r.boot(), -1);
    assert_eq!(r.ee.cfg_flags(), CFG_READ_FAILED);
    r.dev.fail_reads_from = 0;
    r.dev.fail_reads_count = 0;
    r.settle(40);
}

#[test]
fn a_written_sensor_slot_is_no_longer_taken_from_ram_at_a_later_re_read() {
    let mut r = Rig::new();
    store_consistent(&mut r);
    r.boot();
    r.settle(40);
    // slot 0 changed and written
    valid_slot(&mut r.ee.eep_content.cfg.layout.owsensors1[0], 50);
    r.ee.changed_slot(0);
    r.settle(10);
    assert_eq!(r.ee.state(), EEP_STATE_OK);
    // a failed read; the re-read takes slot 0 from the EEPROM, not from RAM
    r.dev.fail_reads_from = r.dev.reads + 1;
    r.dev.fail_reads_count = 0;
    assert_eq!(r.boot(), -1);
    r.ee.eep_content.cfg.layout.owsensors1[0].familycode = 0;
    r.dev.fail_reads_from = 0;
    r.settle(40);
    assert_eq!(r.ee.eep_content.cfg.layout.owsensors1[0].familycode, 0x28);
    assert_eq!(r.ee.eep_content.cfg.layout.owsensors1[0].romcode[0], 50);
}

// ---------------------------------------------------------------- new cases

#[test]
fn a_failed_block_read_is_retried_and_its_data_taken_when_the_retry_succeeds() {
    let mut r = Rig::new();
    store_consistent(&mut r);
    // the first read of block A fails once
    r.dev.fail_reads_from = r.dev.reads + 2;
    r.dev.fail_reads_count = 1;
    assert_eq!(r.boot(), 0);
    assert_eq!(r.ee.eep_content.cfg.lease_timeout_min, 90);
    assert_eq!(r.ee.lease_source(), LEASE_SOURCE_SETTINGS);
    // a read that failed for good fills the block with 0xFF: block A absent, its defaults
    r.dev.fail_reads_from = r.dev.reads + 2;
    r.dev.fail_reads_count = 3;
    assert_eq!(r.boot(), -1);
    assert_eq!(r.ee.eep_content.cfg.lease_timeout_min, 60);
    assert_eq!(r.ee.eep_content.cfg.learn_time_s, 604800);
    // the first boot read 5 times; the second the layout, then 3 failures, then no more
    assert_eq!(r.dev.reads, 5 + 1 + 3);
}

#[test]
fn every_failure_after_the_budget_is_reported_once() {
    let mut r = Rig::new();
    store_consistent(&mut r);
    r.dev.fail_reads_from = 1;
    r.ee.setup();
    assert_eq!(r.ee.read_layout(&mut r.dev, &mut r.dbg), -1);
    assert_eq!(
        r.dbg.take_tx(),
        "Read eeprom layout from eeprom...read error, using defaults\r\nfinished, cfgFlags 80\r\n"
    );
    assert_eq!(r.dev.reads, 3);
    assert_eq!(EEP_READ_FAILURES_MAX, 3);
}

#[test]
fn changed_slot_marks_one_slot_and_a_slot_beyond_31_none() {
    let mut r = Rig::new();
    r.ee.changed_slot(23);
    assert_eq!(r.ee.eep_changed_slots, 1 << 23);
    r.ee.changed_slot(31);
    assert_eq!(r.ee.eep_changed_slots, (1 << 23) | (1 << 31));
    r.ee.changed_slot(32);
    r.ee.changed_slot(255);
    assert_eq!(r.ee.eep_changed_slots, (1 << 23) | (1 << 31));
    assert_eq!(r.ee.eep_content.status, EepState::Changed);
    r.ee.changed(CHANGED_SENSORS);
    assert_eq!(r.ee.eep_changed_slots, 0x00FF_FFFF);
    r.ee.changed(CHANGED_CALIB);
    assert_eq!(r.ee.eep_changed_calib, 0x0FFF);
    assert_eq!(EEP_RETRY_FIRST_S, 30);
    assert_eq!(EEP_RETRY_MAX_S, 3600);
}

#[test]
fn a_successful_write_step_clears_the_marks_a_failed_one_keeps_them() {
    let mut r = Rig::new();
    store_consistent(&mut r);
    r.boot();
    r.ee.changed_slot(5);
    r.ee.store_calib(
        7,
        &CalibRecord {
            opening_count: 1,
            ..CalibRecord::default()
        },
    );
    r.dev.fail_writes_from = r.dev.writes + 1;
    r.dev.fail_writes_count = 1;
    assert_eq!(r.runs_until_write(10), 3);
    assert_eq!(r.ee.eep_changed_slots, 1 << 5);
    assert_eq!(r.ee.eep_changed_calib, 1 << 7);
    assert_eq!(r.ee.writes(), 0);
    r.settle(10);
    assert_eq!(r.ee.eep_changed_slots, 0);
    assert_eq!(r.ee.eep_changed_calib, 0);
    assert_eq!(r.ee.writes(), 1);
}

#[test]
fn the_write_steps_counter_wraps() {
    let mut r = Rig::new();
    store_consistent(&mut r);
    r.boot();
    r.ee.eep_write_steps = u32::MAX;
    r.ee.changed(CHANGED_LEARN_TIME);
    r.settle(10);
    assert_eq!(r.ee.writes(), 0);
    r.ee.eep_cfg_events = u32::MAX;
    r.dev.bytes[usize::from(EXTENSION_ADDRESS) + 5] ^= 1;
    r.boot();
    assert_eq!(r.ee.cfg_events(), 0);
}

#[test]
fn a_re_read_that_finds_damaged_blocks_writes_their_repair() {
    let mut r = Rig::new();
    store_consistent(&mut r);
    r.dev.fail_reads_from = 1;
    r.boot();
    r.dev.fail_reads_from = 0;
    // block A damaged meanwhile: the re-read rebuilds it
    r.dev.bytes[usize::from(EXTENSION_ADDRESS) + 5] ^= 1;
    for _ in 0..30 {
        r.loop_();
    }
    assert_eq!(r.ee.state(), vdm_stm_core::replies_v2::EEP_STATE_PENDING);
    r.dev.ops.clear();
    assert_eq!(r.runs_until_write(10), 3);
    assert!(r.dev.ops.iter().any(|o| o.address == EXTENSION_ADDRESS));
}
