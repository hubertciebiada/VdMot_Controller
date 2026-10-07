//! The configuration in the I2C EEPROM (`software_stm32/src/eeprom.cpp`, `include/eeprom.h`): the
//! RAM mirror `eep_content`, the start-up read through `resolve_config` (core), the write of the
//! changed blocks 3 s after the last change (at most 30 s after the first), retries with a bus
//! restart, the re-read after a failed read that keeps the changes made meanwhile, and the
//! counters of gstax. The layout belongs to the core (`eeprom_layout`, `config_store`).

use crate::eeprom24::EepromDevice;
use crate::print::{Print, HEX};
use vdm_stm_core::config_blocks::{encode_calib, encode_safety, CalibRecord, SafetyBlock};
use vdm_stm_core::config_store::{
    blocks_for, merge_changes, repairs_config, resolve_config, ChangeSet, ConfigImage, LoadResult,
    RawImages, BLOCK_CALIB, BLOCK_LAYOUT, BLOCK_SAFETY, BLOCK_SETTINGS, CHANGED_ALL, CHANGED_CALIB,
    CHANGED_LEASE, CHANGED_SENSORS, LEASE_SOURCE_DEFAULT, LEASE_SOURCE_SETTINGS,
};
use vdm_stm_core::eeprom_layout::{
    encode_extension, StoredExtension, CALIB_BLOCK_ADDRESS, CALIB_BLOCK_SIZE, EXTENSION_ADDRESS,
    EXTENSION_BLOCK_SIZE, LEGACY_LAYOUT_ADDRESS, SAFETY_BLOCK_ADDRESS, SAFETY_BLOCK_SIZE,
    STORED_EXTENSION_DEFAULT,
};
use vdm_stm_core::legacy_layout::{
    crc16_ccitt, encode_legacy_layout, LegacyLayout, LEGACY_IMAGE_SIZE, VALVE_COUNT,
};
use vdm_stm_core::replies_v2::EEP_STATE_PENDING;
use vdm_stm_core::store_scheduler::{Step, StoreScheduler};

/// Failed block transfers allowed in one read of the configuration, whichever blocks they hit:
/// each failure blocks for up to ~0.2 s, so a read on a marginal bus ends in well under a
/// second.
pub const EEP_READ_FAILURES_MAX: u8 = 3;
/// a failed read or write is retried after 30 s, 60 s, ... up to 1 h
pub const EEP_RETRY_FIRST_S: u32 = 30;
pub const EEP_RETRY_MAX_S: u32 = 3600;
/// owsensors1[0..11], owsensors2[0..11]
const EEP_ALL_SLOTS: u32 = 0x00FF_FFFF;
/// calibration records of the 12 valves
const EEP_ALL_CALIB: u16 = 0x0FFF;

/// `enum EEP_STATE` (hardware.h): the RAM mirror against the EEPROM.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum EepState {
    /// not loaded yet
    #[default]
    Init = 0,
    Valid = 1,
    /// a write waits
    Changed = 2,
}

/// The RAM mirror of the EEPROM (C++ `struct eeprom_layout : vdm::ConfigImage`): the fields of
/// the 1.x layout and of the blocks A, B and C, and the state of the mirror.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EepromLayout {
    pub cfg: ConfigImage,
    pub status: EepState,
}

/// Calls of eeprom.cpp into other glue modules.
pub trait EepromEnv {
    /// i2c_bus.cpp: the driver stopped, the bus recovered, the driver started again
    fn i2c_bus_restart(&mut self);
    /// app.cpp: the configuration changed by a re-read after a failed read
    fn app_load_config(&mut self);
}

/// The state of one read of the configuration.
#[derive(Clone, Copy, Debug, Default)]
struct ReadState {
    /// set once the read has used up its failures (EEP_READ_FAILURES_MAX)
    error: bool,
    /// failed block transfers of the read
    failures: u8,
}

/// The module state (the C++ statics of eeprom.cpp).
pub struct Eeprom {
    /// the RAM mirror (C++ global `eep_content`)
    pub eep_content: EepromLayout,
    /// when the configuration is written and read again (`eepromloop()` runs once per second)
    eep_store: StoreScheduler,
    /// the sensor slots and calibration records changed in RAM and not written yet (a re-read
    /// after a failed read keeps them, see core `merge_changes`)
    eep_changed_slots: u32,
    eep_changed_calib: u16,
    read: ReadState,
    /// CFG_* of the last load
    eep_cfg_flags: u8,
    /// loads that repaired or defaulted a block
    eep_cfg_events: u32,
    /// successful write steps
    eep_write_steps: u32,
    eep_lease_source: u8,
    /// the blocks read and the resolved configuration (static in C++: off the main loop stack)
    eep_raw: RawImages,
    eep_loaded: LoadResult,
    /// the encoded 1.x layout of a write step (static in C++)
    image: [u8; LEGACY_IMAGE_SIZE],
}

impl Default for Eeprom {
    fn default() -> Self {
        Eeprom {
            eep_content: EepromLayout::default(),
            eep_store: StoreScheduler::new(EEP_RETRY_FIRST_S, EEP_RETRY_MAX_S),
            eep_changed_slots: 0,
            eep_changed_calib: 0,
            read: ReadState::default(),
            eep_cfg_flags: 0,
            eep_cfg_events: 0,
            eep_write_steps: 0,
            eep_lease_source: LEASE_SOURCE_DEFAULT,
            eep_raw: RawImages {
                layout: [0; LEGACY_IMAGE_SIZE],
                settings: [0; EXTENSION_BLOCK_SIZE],
                safety: [0; SAFETY_BLOCK_SIZE],
                calib: [[0; CALIB_BLOCK_SIZE]; VALVE_COUNT as usize],
                read_failed: false,
            },
            eep_loaded: LoadResult::default(),
            image: [0; LEGACY_IMAGE_SIZE],
        }
    }
}

/// every I2C transfer on a disturbed bus costs up to ~0.2 s: stop at the first error
fn eeprom_write_failed(dbg: &mut impl Print) -> i16 {
    dbg.print(b"write error, aborted\r\n");
    -1
}

/// Writes the blocks holding `fields` (CHANGED_*), in the order C(v) (valves in `calib`), B,
/// 1.x layout, A: a write cut off by a reset leaves every block either old or new and block A
/// (the layout CRC) last, see core `resolve_config`. Returns 0, or -1 if an I2C write failed
/// (the blocks may be partly written).
fn eeprom_write_blocks(
    dev: &mut impl EepromDevice,
    dbg: &mut impl Print,
    lay: &ConfigImage,
    fields: u16,
    calib: u16,
    image: &mut [u8; LEGACY_IMAGE_SIZE],
) -> i16 {
    let blocks = blocks_for(fields);

    dbg.print(b"write eeprom layout to eeprom...\r\n");

    if blocks & BLOCK_CALIB != 0 {
        for (v, rec) in (0u8..).zip(lay.calib.iter()) {
            if calib & (1 << v) == 0 {
                continue;
            }
            let mut record = [0u8; CALIB_BLOCK_SIZE];
            let n = encode_calib(rec, v, &mut record);
            let address = CALIB_BLOCK_ADDRESS + u16::from(v) * CALIB_BLOCK_SIZE as u16;
            if dev.write_block(address, &record[..n]) != 0 {
                return eeprom_write_failed(dbg);
            }
        }
    }

    if blocks & BLOCK_SAFETY != 0 {
        let l: &LegacyLayout = &lay.layout;
        // (C++ also sets leaseValid, which only the decoder writes: encode_safety ignores it)
        let mut safety = SafetyBlock {
            failsafe_pct: lay.failsafe_pct,
            lease_timeout_min: lay.lease_timeout_min,
            ..SafetyBlock::default()
        };
        safety.shadow.low_fac = l.currentbound_low_fac;
        safety.shadow.high_fac = l.currentbound_high_fac;
        safety.shadow.movements = l.number_of_movements;
        safety.shadow.start_on_power = l.start_on_power;
        safety.shadow.min_counts = l.no_of_min_counts;
        safety.shadow.max_retries = l.max_calib_retries;
        let mut block = [0u8; SAFETY_BLOCK_SIZE];
        let n = encode_safety(&safety, &mut block);
        if dev.write_block(SAFETY_BLOCK_ADDRESS, &block[..n]) != 0 {
            return eeprom_write_failed(dbg);
        }
    }

    encode_legacy_layout(&lay.layout, image);
    if blocks & BLOCK_LAYOUT != 0 && dev.write_block(LEGACY_LAYOUT_ADDRESS, image) != 0 {
        return eeprom_write_failed(dbg);
    }

    if blocks & BLOCK_SETTINGS != 0 {
        let ext = StoredExtension {
            escalation: lay.escalation,
            learn_time_s: lay.learn_time_s,
            lease_timeout_min: lay.lease_timeout_min,
            layout_crc: crc16_ccitt(image),
            ..STORED_EXTENSION_DEFAULT
        };
        let mut extbuf = [0u8; EXTENSION_BLOCK_SIZE];
        let n = encode_extension(&ext, &mut extbuf);
        if dev.write_block(EXTENSION_ADDRESS, &extbuf[..n]) != 0 {
            return eeprom_write_failed(dbg);
        }
    }

    dbg.print(b"finished\r\n");

    0
}

/// `eeprom_write_layout`: writes the 1.x layout with the blocks B and A (not the calibration
/// records). Returns 0, or -1 if an I2C write failed (the layout may be partly written).
pub fn eeprom_write_layout(
    dev: &mut impl EepromDevice,
    dbg: &mut impl Print,
    lay: &EepromLayout,
) -> i16 {
    let mut image = [0u8; LEGACY_IMAGE_SIZE];
    eeprom_write_blocks(dev, dbg, &lay.cfg, CHANGED_ALL, 0, &mut image)
}

/// Reads a block, retried while the read has failures left; if it cannot be read the buffer is
/// filled with 0xFF like an erased EEPROM, so the load falls back to the defaults. The
/// failures are counted over the whole read, not per block: after EEP_READ_FAILURES_MAX of them
/// the bus is not used again in this read, so an intermittent bus (each failed transfer blocks
/// for up to ~0.2 s) cannot stretch the read past the watchdog.
fn eeprom_read_block(
    read: &mut ReadState,
    dev: &mut impl EepromDevice,
    dbg: &mut impl Print,
    address: u16,
    buf: &mut [u8],
) {
    while !read.error {
        if usize::from(dev.read_block(address, buf)) == buf.len() {
            return;
        }
        read.failures += 1;
        if read.failures >= EEP_READ_FAILURES_MAX {
            dbg.print(b"read error, using defaults\r\n");
            read.error = true;
        }
    }
    buf.fill(0xFF);
}

impl Eeprom {
    /// `eepromsetup()`, call from the set-up: the RAM mirror is not loaded yet
    pub fn setup(&mut self) -> i16 {
        self.eep_content.status = EepState::Init;
        0
    }

    /// the RAM mirror follows the storage state: Changed while a write waits
    fn sync_status(&mut self) {
        self.eep_content.status = if self.eep_store.eep_state() == EEP_STATE_PENDING {
            EepState::Changed
        } else {
            EepState::Valid
        };
    }

    /// one write step of the loop: the fields changed since the last successful write
    fn write_step(&mut self, dev: &mut impl EepromDevice, dbg: &mut impl Print) {
        let ok = eeprom_write_blocks(
            dev,
            dbg,
            &self.eep_content.cfg,
            self.eep_store.dirty(),
            self.eep_changed_calib,
            &mut self.image,
        ) == 0;
        self.eep_store.write_result(ok);
        if ok {
            self.eep_changed_slots = 0;
            self.eep_changed_calib = 0;
            self.eep_write_steps = self.eep_write_steps.wrapping_add(1);
        }
    }

    /// reads the 1.x layout and the blocks A, B, C0..C11 and resolves them into eep_loaded
    fn load(&mut self, dev: &mut impl EepromDevice, dbg: &mut impl Print) {
        dbg.print(b"Read eeprom layout from eeprom...");

        self.read = ReadState::default();
        let raw = &mut self.eep_raw;
        eeprom_read_block(
            &mut self.read,
            dev,
            dbg,
            LEGACY_LAYOUT_ADDRESS,
            &mut raw.layout,
        );
        eeprom_read_block(
            &mut self.read,
            dev,
            dbg,
            EXTENSION_ADDRESS,
            &mut raw.settings,
        );
        eeprom_read_block(
            &mut self.read,
            dev,
            dbg,
            SAFETY_BLOCK_ADDRESS,
            &mut raw.safety,
        );
        eeprom_read_block(
            &mut self.read,
            dev,
            dbg,
            CALIB_BLOCK_ADDRESS,
            raw.calib.as_flattened_mut(),
        );
        raw.read_failed = self.read.error;

        resolve_config(&self.eep_raw, &mut self.eep_loaded);
        self.eep_cfg_flags = self.eep_loaded.cfg_flags;
        if repairs_config(self.eep_loaded.cfg_flags) {
            self.eep_cfg_events = self.eep_cfg_events.wrapping_add(1);
        }
        self.eep_store.read_result(!self.eep_raw.read_failed);

        dbg.print(b"finished, cfgFlags ");
        dbg.print_unsigned(u32::from(self.eep_loaded.cfg_flags), HEX);
        dbg.print(b"\r\n");
    }

    /// `eeprom_read_layout(&eep_content)`: reads the configuration at start-up; damaged or
    /// missing blocks are repaired (written back by the loop). If a block cannot be read,
    /// writing is disabled and the read is retried by the loop. Returns 0, or -1 if a block
    /// could not be read.
    pub fn read_layout(&mut self, dev: &mut impl EepromDevice, dbg: &mut impl Print) -> i16 {
        self.load(dev, dbg);
        self.eep_content.cfg = self.eep_loaded.image;
        self.eep_lease_source = self.eep_loaded.lease_source;
        let rewrite = self.eep_loaded.rewrite;
        if rewrite != 0 {
            self.changed(rewrite);
        }
        self.eep_content.status = if rewrite != 0 {
            EepState::Changed
        } else {
            EepState::Valid
        };
        if self.eep_raw.read_failed {
            -1
        } else {
            0
        }
    }

    /// Retry of a failed read: the stored configuration is taken over, except the fields
    /// changed since (they are newer and get written), and writing is enabled again.
    fn reread(
        &mut self,
        dev: &mut impl EepromDevice,
        dbg: &mut impl Print,
        env: &mut impl EepromEnv,
    ) {
        self.load(dev, dbg);
        if self.eep_raw.read_failed {
            return;
        }

        dbg.print(b"eeprom read after failure\r\n");
        let changes = ChangeSet {
            fields: self.eep_store.dirty(),
            slots: self.eep_changed_slots,
            calib: self.eep_changed_calib,
        };
        merge_changes(&mut self.eep_loaded.image, &self.eep_content.cfg, &changes);
        self.eep_content.cfg = self.eep_loaded.image;
        // the failed read left the default source: the stored timeout (or one set by slcfg
        // since) applies now
        self.eep_lease_source = if changes.fields & CHANGED_LEASE != 0 {
            LEASE_SOURCE_SETTINGS
        } else {
            self.eep_loaded.lease_source
        };
        let rewrite = self.eep_loaded.rewrite;
        if rewrite != 0 {
            self.changed(rewrite);
        }
        env.app_load_config();
    }

    /// `eepromloop()`, once per second: writes and re-reads as the scheduler says, a retry
    /// after a bus restart.
    pub fn loop_(
        &mut self,
        dev: &mut impl EepromDevice,
        dbg: &mut impl Print,
        env: &mut impl EepromEnv,
    ) -> i16 {
        let step = self.eep_store.tick();
        if step != Step::None {
            // a retry may find the bus stuck the way the failed transfer left it
            if self.eep_store.retrying() {
                env.i2c_bus_restart();
            }
            if step == Step::Reread {
                self.reread(dev, dbg, env);
            } else {
                self.write_step(dev, dbg);
            }
        }
        self.sync_status();
        0
    }

    /// `eeprom_changed(fields)`: CHANGED_* fields of eep_content changed (they select the blocks
    /// to write; a configuration that could not be read is merged with them)
    pub fn changed(&mut self, fields: u16) {
        if fields & CHANGED_SENSORS != 0 {
            self.eep_changed_slots = EEP_ALL_SLOTS;
        }
        if fields & CHANGED_CALIB != 0 {
            self.eep_changed_calib = EEP_ALL_CALIB;
        }
        self.eep_store.changed(fields);
        self.sync_status();
    }

    /// `eeprom_changed_slot(slot)`: one sensor slot changed (0..11 owsensors1, 12..23
    /// owsensors2): only this slot is taken over at a re-read after a failed read. (C++ shifts
    /// 1ul by the slot, undefined from 32 on; Rust marks no slot then.)
    pub fn changed_slot(&mut self, slot: u8) {
        self.eep_changed_slots |= 1u32.checked_shl(u32::from(slot)).unwrap_or(0);
        self.eep_store.changed(CHANGED_SENSORS);
        self.sync_status();
    }

    /// `eeprom_store_calib(valve, rec)`: the calibration record of a valve, stored and written
    /// only when it differs
    pub fn store_calib(&mut self, valve: u8, rec: &CalibRecord) {
        let Some(stored) = self.eep_content.cfg.calib.get_mut(usize::from(valve)) else {
            return;
        };
        if *stored == *rec {
            return;
        }
        *stored = *rec;
        self.eep_changed_calib |= 1 << valve;
        self.eep_store.changed(CHANGED_CALIB);
        self.sync_status();
    }

    /// gstax cfgFlags: CFG_* of the last load
    pub fn cfg_flags(&self) -> u8 {
        self.eep_cfg_flags
    }

    /// gstax cfgEvents: loads since start-up that repaired or defaulted a block
    pub fn cfg_events(&self) -> u32 {
        self.eep_cfg_events
    }

    /// gstax eepWrites: successful write steps since start-up
    pub fn writes(&self) -> u32 {
        self.eep_write_steps
    }

    /// where the start-up load took the lease timeout from (LEASE_SOURCE_*)
    pub fn lease_source(&self) -> u8 {
        self.eep_lease_source
    }

    /// gstat eepState: EEP_STATE_OK, _PENDING, _WRITE_FAILED, _READ_FAILED
    pub fn state(&self) -> u8 {
        self.eep_store.eep_state()
    }

    /// true if no write is waiting: a reset may happen now
    pub fn free(&self) -> bool {
        self.eep_store.free()
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
