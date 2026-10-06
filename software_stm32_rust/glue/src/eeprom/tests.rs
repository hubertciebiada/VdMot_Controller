// Port of test/native/glue/test_eeprom.cpp on the fake 24LC64: the blocks C(v), B, 1.x layout, A
// and their write order, the load through resolve_config (flags, events, repairs written back,
// lease source), the write schedule of the loop (StoreScheduler: debounce, attempts, backoff),
// the I2C bus restart before every retry (S5), the re-read after a failed read that keeps the
// slots and records changed meanwhile (W11), calibration records and the counters of gstax.

use super::*;
use crate::test_support::eeprom_fake::FakeEeprom;
use crate::test_support::io_fakes::FakeSerial;
use crate::test_support::stub_log::CallLog;
use vdm_stm_core::config_blocks::{
    decode_calib, decode_safety, BlockState, CALIB_FAILED, CALIB_VALID,
};
use vdm_stm_core::config_store::{
    CFG_LAYOUT_CRC, CFG_READ_FAILED, CFG_SAFETY_CORRUPT, CFG_SENSOR_SLOT, CFG_SETTINGS_CORRUPT,
    CFG_UNVERIFIED, CHANGED_ESCALATION, CHANGED_FAILSAFE, CHANGED_LEARN_TIME, CHANGED_MOTOR,
    LEASE_SOURCE_SAFETY,
};
use vdm_stm_core::eeprom_layout::{decode_extension, ExtensionState};
use vdm_stm_core::legacy_layout::SensorSlot;
use vdm_stm_core::onewire_check::crc8;
use vdm_stm_core::replies_v2::{
    EEP_STATE_OK, EEP_STATE_PENDING, EEP_STATE_READ_FAILED, EEP_STATE_WRITE_FAILED,
};

/// The link-seam stubs of eeprom.cpp (stub_i2c_bus, stub_app).
#[derive(Default)]
pub(super) struct Env {
    pub calls: CallLog,
}

impl EepromEnv for Env {
    fn i2c_bus_restart(&mut self) {
        self.calls.log("i2c_bus_restart()");
    }

    fn app_load_config(&mut self) {
        self.calls.log("app_load_config()");
    }
}

/// eeprom.cpp with the fake 24LC64, the debug UART and the stubs.
#[derive(Default)]
pub(super) struct Rig {
    pub ee: Eeprom,
    pub dev: FakeEeprom,
    pub dbg: FakeSerial,
    pub env: Env,
}

impl Rig {
    pub fn new() -> Self {
        Rig::default()
    }

    pub fn write_layout(&mut self, lay: &EepromLayout) -> i16 {
        eeprom_write_layout(&mut self.dev, &mut self.dbg, lay)
    }

    /// start-up read into eep_content
    pub fn boot(&mut self) -> i16 {
        self.ee.setup();
        let r = self.ee.read_layout(&mut self.dev, &mut self.dbg);
        self.dev.ops.clear();
        self.dbg.take_tx();
        self.env.calls.clear();
        r
    }

    pub fn loop_(&mut self) -> i16 {
        self.ee.loop_(&mut self.dev, &mut self.dbg, &mut self.env)
    }

    /// the loop runs until the EEPROM is written or n runs; the number of runs, 0 if none wrote
    pub fn runs_until_write(&mut self, n: u32) -> u32 {
        let writes = self.dev.writes;
        for i in 1..=n {
            self.loop_();
            if self.dev.writes != writes {
                return i;
            }
        }
        0
    }

    /// the loop until the storage state is ok (at most n runs)
    pub fn settle(&mut self, n: u32) {
        for _ in 0..n {
            if self.ee.state() == EEP_STATE_OK {
                return;
            }
            self.loop_();
        }
    }

    /// a consistent configuration in the EEPROM (1.x layout, A, B, no calibration records)
    pub fn store_consistent(&mut self) {
        let mut lay = EepromLayout::default();
        fill_layout(&mut lay);
        assert_eq!(self.write_layout(&lay), 0);
        self.dev.ops.clear();
        self.dbg.take_tx();
    }
}

/// a DS18B20 address with a valid CRC: family 0x28, serial n
pub(super) fn valid_slot(s: &mut SensorSlot, n: u8) {
    let mut a = [0x28, n, 0, 0, 0, 0, 0, 0];
    a[7] = crc8(&a[..7]);
    s.familycode = a[0];
    s.romcode.copy_from_slice(&a[1..7]);
    s.crc = a[7];
}

fn descr(text: &[u8]) -> [u8; 25] {
    let mut d = [0u8; 25];
    d[..text.len()].copy_from_slice(text);
    d
}

fn fill_layout(lay: &mut EepromLayout) {
    *lay = EepromLayout::default();
    let c = &mut lay.cfg;
    c.layout.b_slave = 1;
    c.layout.descr = descr(b"VdMot Controller");
    c.layout.currentbound_low_fac = 18;
    c.layout.currentbound_high_fac = 19;
    c.layout.number_of_movements = 0x1234;
    for v in 0..12u8 {
        valid_slot(&mut c.layout.owsensors1[usize::from(v)], v);
        valid_slot(&mut c.layout.owsensors2[usize::from(v)], 100 + v);
    }
    c.layout.start_on_power = 40;
    c.layout.no_of_min_counts = 0xABCD;
    c.layout.max_calib_retries = 2;
    c.escalation.enable = 1;
    c.escalation.step_pct = 30;
    c.escalation.max_ma = 40;
    c.learn_time_s = 3600;
    c.lease_timeout_min = 90;
    for v in 0..12u8 {
        c.failsafe_pct[usize::from(v)] = 10 + v;
    }
}

fn record(oc: u16, cc: u16, ma: u16, flags: u8) -> CalibRecord {
    CalibRecord {
        opening_count: oc,
        closing_count: cc,
        mean_current: ma,
        flags,
    }
}

#[test]
fn eepromsetup_the_ram_mirror_starts_as_not_loaded_nothing_pending() {
    let mut r = Rig::new();
    r.ee.eep_content.status = EepState::Changed;
    assert_eq!(r.ee.setup(), 0);
    assert_eq!(r.ee.eep_content.status, EepState::Init);
    assert!(r.ee.free());
    assert_eq!(r.ee.state(), EEP_STATE_OK);
    assert_eq!(r.ee.writes(), 0);
    assert_eq!(r.ee.cfg_flags(), 0);
    assert_eq!(r.ee.cfg_events(), 0);
    assert_eq!(r.ee.lease_source(), LEASE_SOURCE_DEFAULT);
}

#[test]
fn eeprom_write_layout_block_b_the_1x_layout_in_one_transfer_then_block_a_with_the_layout_crc() {
    let mut r = Rig::new();
    let mut lay = EepromLayout::default();
    fill_layout(&mut lay);
    lay.cfg.calib[0] = record(3600, 3650, 20, CALIB_VALID);
    assert_eq!(r.write_layout(&lay), 0);
    let ops = r.dev.ops.clone();
    assert_eq!(ops.len(), 3);
    assert_eq!(ops[0].address, SAFETY_BLOCK_ADDRESS);
    assert_eq!(ops[0].length, 25);
    assert_eq!(ops[1].address, LEGACY_LAYOUT_ADDRESS);
    assert_eq!(usize::from(ops[1].length), LEGACY_IMAGE_SIZE);
    assert_eq!(ops[2].address, EXTENSION_ADDRESS);
    assert_eq!(ops[2].length, 14);
    let mut image = [0u8; LEGACY_IMAGE_SIZE];
    encode_legacy_layout(&lay.cfg.layout, &mut image);
    assert_eq!(
        r.dev.stored(LEGACY_LAYOUT_ADDRESS, image.len()),
        image.to_vec()
    );
    let mut ext = [0u8; EXTENSION_BLOCK_SIZE];
    ext.copy_from_slice(&r.dev.stored(EXTENSION_ADDRESS, EXTENSION_BLOCK_SIZE));
    let mut back = StoredExtension::default();
    assert_eq!(decode_extension(&ext, &mut back), ExtensionState::Valid);
    assert_eq!(back.layout_crc, crc16_ccitt(&image));
    assert_eq!(back.learn_time_s, 3600);
    assert_eq!(back.lease_timeout_min, 90);
    assert_eq!(back.escalation.max_ma, 40);
    let mut safety = [0u8; SAFETY_BLOCK_SIZE];
    safety.copy_from_slice(&r.dev.stored(SAFETY_BLOCK_ADDRESS, SAFETY_BLOCK_SIZE));
    let mut b = SafetyBlock::default();
    assert_eq!(decode_safety(&safety, &mut b), BlockState::Valid);
    assert_eq!(b.failsafe_pct[0], 10);
    assert_eq!(b.failsafe_pct[11], 21);
    assert_eq!(b.shadow.low_fac, 18);
    assert_eq!(b.shadow.high_fac, 19);
    assert_eq!(b.shadow.movements, 0x1234);
    assert_eq!(b.shadow.start_on_power, 40);
    assert_eq!(b.shadow.min_counts, 0xABCD);
    assert_eq!(b.shadow.max_retries, 2);
    assert!(b.lease_valid);
    assert_eq!(b.lease_timeout_min, 90);
    // no calibration record: they are written by store_calib() only
    assert_eq!(r.dev.stored(CALIB_BLOCK_ADDRESS, 16), vec![0xFF; 16]);
    assert_eq!(
        r.dbg.take_tx(),
        "write eeprom layout to eeprom...\r\nfinished\r\n"
    );
}

#[test]
fn eeprom_write_layout_stops_at_the_first_failed_transfer_for_every_one_of_the_3() {
    for k in 1..=3u32 {
        let mut r = Rig::new();
        let mut lay = EepromLayout::default();
        fill_layout(&mut lay);
        r.dev.fail_writes_from = k;
        assert_eq!(r.write_layout(&lay), -1, "k {k}");
        assert_eq!(r.dev.ops.len(), k as usize);
        assert_eq!(
            r.dbg.take_tx(),
            "write eeprom layout to eeprom...\r\nwrite error, aborted\r\n"
        );
    }
}

#[test]
fn eeprom_read_layout_a_consistent_configuration_loads_without_flags_and_without_a_write() {
    let mut r = Rig::new();
    r.store_consistent();
    // C++ reads into a zeroed layout `back`: the Rust read goes into eep_content
    assert_eq!(r.ee.read_layout(&mut r.dev, &mut r.dbg), 0);
    let ops = r.dev.ops.clone();
    assert_eq!(ops.len(), 4);
    assert_eq!(ops[0].address, LEGACY_LAYOUT_ADDRESS);
    assert_eq!(ops[0].length, 309);
    assert_eq!(ops[1].address, EXTENSION_ADDRESS);
    assert_eq!(ops[1].length, 16);
    assert_eq!(ops[2].address, SAFETY_BLOCK_ADDRESS);
    assert_eq!(ops[2].length, 32);
    assert_eq!(ops[3].address, CALIB_BLOCK_ADDRESS);
    assert_eq!(ops[3].length, 192);
    let back = r.ee.eep_content;
    assert_eq!(back.status, EepState::Valid);
    assert_eq!(back.cfg.layout.b_slave, 1);
    assert_eq!(back.cfg.layout.descr, descr(b"VdMot Controller"));
    assert_eq!(back.cfg.layout.number_of_movements, 0x1234);
    assert_eq!(back.cfg.layout.owsensors1[11].romcode[0], 11);
    assert_eq!(back.cfg.layout.owsensors2[11].romcode[0], 111);
    assert_eq!(back.cfg.layout.no_of_min_counts, 0xABCD);
    assert_eq!(back.cfg.layout.max_calib_retries, 2);
    assert_eq!(back.cfg.escalation.step_pct, 30);
    assert_eq!(back.cfg.learn_time_s, 3600);
    assert_eq!(back.cfg.lease_timeout_min, 90);
    assert_eq!(back.cfg.failsafe_pct[5], 15);
    assert_eq!(back.cfg.calib[0].flags, 0);
    assert_eq!(r.ee.state(), EEP_STATE_OK);
    assert_eq!(r.ee.cfg_flags(), 0);
    assert_eq!(r.ee.cfg_events(), 0);
    assert_eq!(r.ee.lease_source(), LEASE_SOURCE_SETTINGS);
    assert_eq!(
        r.dbg.take_tx(),
        "Read eeprom layout from eeprom...finished, cfgFlags 0\r\n"
    );
    for _ in 0..40 {
        r.loop_();
    }
    assert_eq!(r.dev.writes, 3);
    assert_eq!(r.ee.writes(), 0);
}

#[test]
fn first_start_on_a_new_chip_defaults_unverified_one_write_of_b_layout_and_a() {
    let mut r = Rig::new();
    assert_eq!(r.boot(), 0);
    assert_eq!(r.ee.cfg_flags(), CFG_UNVERIFIED);
    assert_eq!(r.ee.cfg_events(), 0);
    assert_eq!(r.ee.lease_source(), LEASE_SOURCE_DEFAULT);
    assert_eq!(r.ee.eep_content.cfg.lease_timeout_min, 60);
    assert_eq!(r.ee.eep_content.cfg.failsafe_pct[0], 50);
    assert_eq!(r.ee.eep_content.cfg.learn_time_s, 604800);
    assert_eq!(r.ee.eep_content.status, EepState::Changed);
    assert_eq!(r.ee.state(), EEP_STATE_PENDING);
    assert!(!r.ee.free());
    assert_eq!(r.runs_until_write(10), 3);
    assert_eq!(r.dev.ops.len(), 3);
    assert_eq!(r.ee.writes(), 1);
    assert_eq!(r.ee.state(), EEP_STATE_OK);
    assert_eq!(r.ee.eep_content.status, EepState::Valid);
    // a first write is no retry: no bus restart
    assert!(r.env.calls.calls.is_empty());
    assert_eq!(r.boot(), 0);
    assert_eq!(r.ee.cfg_flags(), 0);
    assert_eq!(r.ee.state(), EEP_STATE_OK);
}

#[test]
fn a_damaged_byte_of_the_1x_motor_fields_taken_from_block_b_flagged_repaired_by_one_write() {
    let mut r = Rig::new();
    r.store_consistent();
    // low byte of noOfMinCounts
    r.dev.bytes[0x0139] ^= 0x8B;
    assert_eq!(r.boot(), 0);
    assert_eq!(r.ee.eep_content.cfg.layout.no_of_min_counts, 0xABCD);
    assert_eq!(r.ee.cfg_flags(), CFG_LAYOUT_CRC);
    assert_eq!(r.ee.cfg_events(), 1);
    assert_eq!(r.ee.state(), EEP_STATE_PENDING);
    assert_eq!(r.runs_until_write(10), 3);
    assert_eq!(r.boot(), 0);
    assert_eq!(r.ee.cfg_flags(), 0);
    assert_eq!(r.ee.cfg_events(), 1);
    assert_eq!(r.ee.eep_content.cfg.layout.no_of_min_counts, 0xABCD);
}

#[test]
fn lease_source_block_a_else_the_copy_in_b_else_the_default() {
    let mut r = Rig::new();
    r.store_consistent();
    r.boot();
    assert_eq!(r.ee.lease_source(), LEASE_SOURCE_SETTINGS);
    // block A damaged
    r.dev.bytes[usize::from(EXTENSION_ADDRESS) + 5] ^= 1;
    r.boot();
    assert_eq!(r.ee.lease_source(), LEASE_SOURCE_SAFETY);
    assert_eq!(r.ee.eep_content.cfg.lease_timeout_min, 90);
    assert_eq!(r.ee.cfg_flags(), CFG_SETTINGS_CORRUPT);
    assert_eq!(r.ee.cfg_events(), 1);
    // and block B
    r.dev.bytes[usize::from(SAFETY_BLOCK_ADDRESS) + 3] ^= 1;
    r.boot();
    assert_eq!(r.ee.lease_source(), LEASE_SOURCE_DEFAULT);
    assert_eq!(r.ee.eep_content.cfg.lease_timeout_min, 60);
    assert_eq!(r.ee.cfg_flags(), CFG_SETTINGS_CORRUPT | CFG_SAFETY_CORRUPT);
    assert_eq!(r.ee.cfg_events(), 2);
}

#[test]
fn eepromloop_a_change_is_written_3_s_after_the_last_change_at_most_30_s_after_the_first() {
    let mut r = Rig::new();
    r.store_consistent();
    r.boot();
    r.ee.changed(CHANGED_LEARN_TIME);
    assert_eq!(r.ee.state(), EEP_STATE_PENDING);
    assert_eq!(r.ee.eep_content.status, EepState::Changed);
    assert!(!r.ee.free());
    r.loop_();
    r.loop_();
    r.ee.changed(CHANGED_LEARN_TIME);
    assert_eq!(r.runs_until_write(10), 3);
    // the learn time is in block A only
    assert_eq!(r.dev.ops.len(), 1);
    assert_eq!(r.dev.ops[0].address, EXTENSION_ADDRESS);
    assert_eq!(r.ee.eep_content.status, EepState::Valid);
    assert_eq!(r.ee.state(), EEP_STATE_OK);
    assert!(r.ee.free());
    assert_eq!(r.ee.writes(), 1);
    // a change every second: written after 30
    let mut n = 0;
    let writes = r.dev.writes;
    while r.dev.writes == writes && n < 40 {
        r.ee.changed(CHANGED_FAILSAFE);
        r.loop_();
        n += 1;
    }
    assert_eq!(n, 30);
    assert_eq!(r.dev.ops.last().unwrap().address, SAFETY_BLOCK_ADDRESS);
    assert_eq!(r.ee.writes(), 2);
    assert!(r.env.calls.calls.is_empty());
}

#[test]
fn eepromloop_blocks_per_field_calibration_records_first_block_a_last() {
    let mut r = Rig::new();
    r.store_consistent();
    r.boot();
    r.ee.store_calib(3, &record(3600, 3650, 20, CALIB_VALID));
    r.ee.store_calib(1, &record(1000, 1100, 30, CALIB_VALID | CALIB_FAILED));
    r.ee.changed(CHANGED_ESCALATION | CHANGED_LEASE);
    assert_eq!(r.runs_until_write(10), 3);
    let ops = r.dev.ops.clone();
    assert_eq!(ops.len(), 4);
    assert_eq!(ops[0].address, CALIB_BLOCK_ADDRESS + 16);
    assert_eq!(ops[0].length, 12);
    assert_eq!(ops[1].address, CALIB_BLOCK_ADDRESS + 48);
    assert_eq!(ops[2].address, SAFETY_BLOCK_ADDRESS);
    assert_eq!(ops[3].address, EXTENSION_ADDRESS);
    let mut raw = [0u8; CALIB_BLOCK_SIZE];
    raw.copy_from_slice(&r.dev.stored(CALIB_BLOCK_ADDRESS + 48, CALIB_BLOCK_SIZE));
    let mut back = CalibRecord::default();
    assert_eq!(decode_calib(&raw, 3, &mut back), BlockState::Valid);
    assert_eq!(back.opening_count, 3600);
    assert_eq!(back.closing_count, 3650);
    assert_eq!(back.mean_current, 20);
    assert_eq!(back.flags, CALIB_VALID);
    // the next write step has no record left
    r.dev.ops.clear();
    r.ee.changed(CHANGED_FAILSAFE);
    assert_eq!(r.runs_until_write(10), 3);
    assert_eq!(r.dev.ops.len(), 1);
    assert_eq!(r.dev.ops[0].address, SAFETY_BLOCK_ADDRESS);
    // the records load at the next start
    r.boot();
    assert_eq!(r.ee.eep_content.cfg.calib[3].opening_count, 3600);
    assert_eq!(
        r.ee.eep_content.cfg.calib[1].flags,
        CALIB_VALID | CALIB_FAILED
    );
    assert_eq!(r.ee.eep_content.cfg.calib[0].flags, 0);
}

#[test]
fn eeprom_store_calib_an_unchanged_record_or_an_invalid_valve_is_not_written() {
    let mut r = Rig::new();
    r.store_consistent();
    r.boot();
    r.ee.store_calib(3, &record(0, 0, 0, 0));
    r.ee.store_calib(12, &record(3600, 3650, 20, CALIB_VALID));
    assert_eq!(r.ee.state(), EEP_STATE_OK);
    r.ee.store_calib(3, &record(3600, 3650, 20, CALIB_VALID));
    assert_eq!(r.ee.state(), EEP_STATE_PENDING);
    assert_eq!(r.ee.eep_content.status, EepState::Changed);
    assert_eq!(r.ee.eep_content.cfg.calib[3].closing_count, 3650);
    r.runs_until_write(10);
    assert_eq!(r.ee.state(), EEP_STATE_OK);
    // the same record again (C++: a loop over one record)
    r.ee.store_calib(3, &record(3600, 3650, 20, CALIB_VALID));
    assert_eq!(r.ee.state(), EEP_STATE_OK);
    let changed = [
        record(3601, 3650, 20, 1),
        record(3600, 3651, 20, 1),
        record(3600, 3650, 21, 1),
        record(3600, 3650, 20, 3),
    ];
    for rec in changed {
        r.dev.ops.clear();
        r.ee.store_calib(3, &rec);
        assert_eq!(r.ee.state(), EEP_STATE_PENDING, "{rec:?}");
        assert_eq!(r.runs_until_write(10), 3);
        assert_eq!(r.dev.ops.len(), 1);
        assert_eq!(r.dev.ops[0].address, CALIB_BLOCK_ADDRESS + 48);
    }
}

#[test]
fn eepromloop_a_failed_write_is_repeated_with_a_bus_restart_after_3_attempts_eepstate_2_and_backoff(
) {
    let mut r = Rig::new();
    r.store_consistent();
    r.boot();
    r.dev.fail_writes_from = r.dev.writes + 1;
    r.ee.changed(CHANGED_LEARN_TIME);
    for _ in 0..3 {
        r.loop_();
    }
    assert_eq!(r.dev.writes, 4);
    assert!(r.env.calls.calls.is_empty());
    r.loop_();
    assert_eq!(r.env.calls.calls, vec!["i2c_bus_restart()"]);
    r.loop_();
    assert_eq!(r.dev.writes, 6);
    assert_eq!(r.ee.state(), EEP_STATE_WRITE_FAILED);
    assert!(r.ee.free());
    assert_eq!(r.ee.writes(), 0);
    assert_eq!(
        r.env.calls.calls,
        vec!["i2c_bus_restart()", "i2c_bus_restart()"]
    );
    // retried after 30 s with a bus restart
    r.dev.fail_writes_from = 0;
    r.env.calls.clear();
    let n = r.runs_until_write(40);
    assert_eq!(n, 30);
    assert_eq!(r.env.calls.calls, vec!["i2c_bus_restart()"]);
    assert_eq!(r.ee.state(), EEP_STATE_OK);
    assert_eq!(r.ee.writes(), 1);
    assert!(r.dbg.take_tx().contains("write error, aborted"));
}

#[test]
fn a_failed_start_up_read_defaults_eepstate_3_nothing_written_the_re_read_after_a_bus_restart() {
    let mut r = Rig::new();
    r.store_consistent();
    r.dev.fail_reads_from = 2;
    r.dev.fail_reads_count = 3;
    assert_eq!(r.boot(), -1);
    // the layout was read, block A was not
    assert_eq!(r.ee.eep_content.cfg.layout.b_slave, 1);
    assert_ne!(r.ee.eep_content.cfg.escalation.step_pct, 30);
    assert_eq!(r.ee.state(), EEP_STATE_READ_FAILED);
    assert_eq!(r.ee.cfg_flags(), CFG_READ_FAILED);
    assert_eq!(r.ee.cfg_events(), 0);
    assert_eq!(r.ee.eep_content.status, EepState::Valid);
    r.ee.changed(CHANGED_MOTOR);
    assert_eq!(r.ee.state(), EEP_STATE_READ_FAILED);
    assert!(r.ee.free());
    let writes = r.dev.writes;
    for _ in 0..29 {
        r.loop_();
    }
    assert_eq!(r.dev.writes, writes);
    assert!(r.env.calls.calls.is_empty());
    // 30 s: re-read, then the change is written
    r.loop_();
    assert_eq!(
        r.env.calls.calls,
        vec!["i2c_bus_restart()", "app_load_config()"]
    );
    assert_eq!(r.ee.state(), EEP_STATE_PENDING);
    assert_eq!(r.ee.eep_content.cfg.escalation.step_pct, 30);
    assert_eq!(r.ee.cfg_flags(), 0);
    assert!(r.dbg.take_tx().contains("eeprom read after failure\r\n"));
    // the debounce restarts with the re-read
    assert_eq!(r.runs_until_write(10), 3);
    assert_eq!(r.ee.state(), EEP_STATE_OK);
}

#[test]
fn a_failed_re_read_keeps_the_backoff_the_next_one_after_60_s() {
    let mut r = Rig::new();
    r.store_consistent();
    r.dev.fail_reads_from = 1;
    assert_eq!(r.boot(), -1);
    for _ in 0..30 {
        r.loop_();
    }
    assert_eq!(r.env.calls.calls, vec!["i2c_bus_restart()"]);
    assert_eq!(r.ee.state(), EEP_STATE_READ_FAILED);
    r.dev.fail_reads_from = 0;
    r.env.calls.clear();
    for _ in 0..59 {
        r.loop_();
    }
    assert!(r.env.calls.calls.is_empty());
    r.loop_();
    assert_eq!(
        r.env.calls.calls,
        vec!["i2c_bus_restart()", "app_load_config()"]
    );
    assert_eq!(r.ee.state(), EEP_STATE_OK);
}

#[test]
fn re_read_after_a_failed_read_the_lease_source_of_the_blocks_read_or_of_a_timeout_set_meanwhile() {
    let mut r = Rig::new();
    r.store_consistent();
    r.dev.fail_reads_from = 1;
    r.boot();
    assert_eq!(r.ee.lease_source(), LEASE_SOURCE_DEFAULT);
    assert_eq!(r.ee.eep_content.cfg.lease_timeout_min, 60);
    r.dev.fail_reads_from = 0;
    for _ in 0..30 {
        r.loop_();
    }
    assert_eq!(r.ee.state(), EEP_STATE_OK);
    assert_eq!(r.ee.lease_source(), LEASE_SOURCE_SETTINGS);
    assert_eq!(r.ee.eep_content.cfg.lease_timeout_min, 90);
    // block A damaged: the copy in B, also after a change of another field
    r.dev.bytes[usize::from(EXTENSION_ADDRESS) + 5] ^= 1;
    r.dev.fail_reads_from = 1;
    r.boot();
    r.ee.changed(CHANGED_MOTOR);
    r.dev.fail_reads_from = 0;
    for _ in 0..120 {
        r.loop_();
    }
    assert_eq!(r.ee.lease_source(), LEASE_SOURCE_SAFETY);
    assert_eq!(r.ee.eep_content.cfg.lease_timeout_min, 90);
    // slcfg while the EEPROM could not be read: its timeout counts as configured
    r.dev.fail_reads_from = 1;
    r.boot();
    r.ee.eep_content.cfg.lease_timeout_min = 0;
    r.ee.changed(CHANGED_LEASE);
    r.dev.fail_reads_from = 0;
    for _ in 0..120 {
        r.loop_();
    }
    assert_eq!(r.ee.lease_source(), LEASE_SOURCE_SETTINGS);
    assert_eq!(r.ee.eep_content.cfg.lease_timeout_min, 0);
}

#[test]
fn re_read_after_a_failed_read_only_the_changed_sensor_slot_and_record_are_taken_from_ram() {
    let mut r = Rig::new();
    r.store_consistent();
    r.dev.fail_reads_from = 1;
    r.boot();
    // RAM holds 0xFF fallbacks; stsnx 2 (slot 2) and a calibration record of valve 4 meanwhile
    valid_slot(&mut r.ee.eep_content.cfg.layout.owsensors1[2], 77);
    r.ee.changed_slot(2);
    valid_slot(&mut r.ee.eep_content.cfg.layout.owsensors2[5], 78);
    r.ee.changed_slot(17);
    r.ee.store_calib(4, &record(2000, 2100, 25, CALIB_VALID));
    r.dev.fail_reads_from = 0;
    for _ in 0..30 {
        r.loop_();
    }
    let c = r.ee.eep_content.cfg;
    assert_eq!(r.ee.state(), EEP_STATE_PENDING);
    assert_eq!(c.layout.owsensors1[2].romcode[0], 77);
    assert_eq!(c.layout.owsensors2[5].romcode[0], 78);
    assert_eq!(c.layout.owsensors1[3].romcode[0], 3);
    assert_eq!(c.layout.owsensors1[1].romcode[0], 1);
    assert_eq!(c.layout.owsensors2[4].romcode[0], 104);
    assert_eq!(c.layout.owsensors2[6].romcode[0], 106);
    assert_eq!(c.calib[4].opening_count, 2000);
    assert_eq!(c.layout.no_of_min_counts, 0xABCD);
    r.dev.ops.clear();
    assert_eq!(r.runs_until_write(10), 3);
    assert_eq!(r.dev.ops.len(), 4);
    assert_eq!(r.dev.ops[0].address, CALIB_BLOCK_ADDRESS + 64);
    // stored: the other slots as before, the new ones
    r.boot();
    let c = r.ee.eep_content.cfg;
    assert_eq!(c.layout.owsensors1[2].romcode[0], 77);
    assert_eq!(c.layout.owsensors1[3].romcode[0], 3);
    assert_eq!(c.layout.owsensors2[5].romcode[0], 78);
    assert_eq!(c.calib[4].closing_count, 2100);
    assert_eq!(r.ee.cfg_flags(), 0);
}

#[test]
fn re_read_eeprom_changed_sensors_takes_every_slot_calib_every_record() {
    let mut r = Rig::new();
    r.store_consistent();
    r.dev.fail_reads_from = 1;
    r.boot();
    let c = &mut r.ee.eep_content.cfg;
    c.layout.owsensors1 = [SensorSlot::default(); 12];
    c.layout.owsensors2 = [SensorSlot::default(); 12];
    c.calib[11] = record(1500, 1500, 20, CALIB_VALID);
    r.ee.changed(CHANGED_SENSORS | CHANGED_CALIB);
    r.dev.fail_reads_from = 0;
    for _ in 0..30 {
        r.loop_();
    }
    let c = r.ee.eep_content.cfg;
    assert_eq!(c.layout.owsensors1[0].familycode, 0);
    assert_eq!(c.layout.owsensors1[11].familycode, 0);
    assert_eq!(c.layout.owsensors2[0].familycode, 0);
    assert_eq!(c.layout.owsensors2[11].familycode, 0);
    assert_eq!(c.calib[11].opening_count, 1500);
    assert_eq!(c.layout.number_of_movements, 0x1234);
}

#[test]
fn a_sensor_slot_with_a_flipped_bit_is_cleared_flagged_and_written_back() {
    let mut r = Rig::new();
    r.store_consistent();
    // owsensors1[4]
    r.dev.bytes[usize::from(LEGACY_LAYOUT_ADDRESS) + 33 + 8 * 4 + 1] ^= 0x10;
    r.boot();
    assert_eq!(r.ee.cfg_flags(), CFG_LAYOUT_CRC | CFG_SENSOR_SLOT);
    assert_eq!(r.ee.eep_content.cfg.layout.owsensors1[4].familycode, 0);
    assert_eq!(r.ee.eep_content.cfg.layout.owsensors1[5].familycode, 0x28);
    assert_eq!(r.ee.state(), EEP_STATE_PENDING);
}
