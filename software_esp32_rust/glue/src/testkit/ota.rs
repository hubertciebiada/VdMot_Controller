//! Fake OTA: two app slots and otadata (persistent, owned by the board), updates into the other
//! slot, and the devices' bootloader without rollback support.

use std::sync::{Arc, Mutex, MutexGuard};

use super::{lock, Journal};
use crate::port::{AppId, EspErr, Ota, OtaUpdate, SlotInfo};
use vdm_esp_core::config::crc32;

/// Flash addresses of app0 and app1 (the board's default partition table).
pub(crate) const SLOT_ADDR: [u32; 2] = [0x1_0000, 0x15_0000];
/// Size of each app slot.
pub(crate) const SLOT_SIZE: u32 = 0x14_0000;

/// What a slot holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SlotImage {
    /// The app description is readable (`esp_ota_get_partition_description`).
    pub(crate) app: Option<AppId>,
    /// The image verifies (the bootloader loads it, `set_boot` accepts it).
    pub(crate) valid: bool,
    /// The image is not a glue image (the C++ firmware): a boot of it ends the case.
    pub(crate) foreign: bool,
}

impl SlotImage {
    pub(crate) const EMPTY: SlotImage = SlotImage {
        app: None,
        valid: false,
        foreign: false,
    };
    /// A Rust image.
    pub(crate) fn glue(app: AppId) -> Self {
        SlotImage {
            app: Some(app),
            valid: true,
            foreign: false,
        }
    }
    /// The C++ firmware.
    pub(crate) fn foreign(app: AppId) -> Self {
        SlotImage {
            app: Some(app),
            valid: true,
            foreign: true,
        }
    }
    /// An image that does not verify (`app`: what its description still says).
    pub(crate) fn broken(app: Option<AppId>) -> Self {
        SlotImage {
            app,
            valid: false,
            foreign: false,
        }
    }
}

/// Persistent OTA state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OtaStore {
    pub(crate) slots: [SlotImage; 2],
    /// Slot selected for the next boot.
    pub(crate) otadata: usize,
    /// Slot of the running image.
    pub(crate) running: usize,
}

impl OtaStore {
    /// The bootloader of the devices (no rollback support): the otadata slot when its image is
    /// valid, else the other valid slot; `None` when neither loads (a reflash by cable).
    pub(crate) fn bootloader(&mut self) -> Option<usize> {
        let pick = [self.otadata, 1 - self.otadata]
            .into_iter()
            .find(|&i| self.slots[i].valid)?;
        self.running = pick;
        Some(pick)
    }
}

/// Knobs and records of one boot.
pub(crate) struct OtaKnobs {
    /// `other()` reports no second slot.
    pub(crate) no_other: bool,
    pub(crate) begin_err: Option<EspErr>,
    /// Writes fail (with `write_err`) once the update would pass this many bytes.
    pub(crate) fail_write_at: Option<usize>,
    pub(crate) write_err: EspErr,
    pub(crate) finish_err: Option<EspErr>,
    pub(crate) set_boot_err: Option<EspErr>,
    pub(crate) image_size: u32,
    /// App id the next successful `finish` installs (default: derived from the bytes).
    pub(crate) next_app: Option<AppId>,
    pub(crate) begins: u32,
    pub(crate) writes: u32,
    pub(crate) finishes: u32,
    pub(crate) aborts: u32,
    pub(crate) set_boots: Vec<u32>,
    /// Addresses `verify` was asked about.
    pub(crate) verifies: Vec<u32>,
    /// `mark_valid` calls.
    pub(crate) mark_valids: u32,
    /// Bytes of the last update.
    pub(crate) written: Vec<u8>,
}

impl Default for OtaKnobs {
    fn default() -> Self {
        OtaKnobs {
            no_other: false,
            begin_err: None,
            fail_write_at: None,
            write_err: EspErr::FLASH_OP_FAIL,
            finish_err: None,
            set_boot_err: None,
            image_size: 1_091_984,
            next_app: None,
            begins: 0,
            writes: 0,
            finishes: 0,
            aborts: 0,
            set_boots: Vec::new(),
            verifies: Vec::new(),
            mark_valids: 0,
            written: Vec::new(),
        }
    }
}

/// The OTA slots of a boot.
#[derive(Clone)]
pub(crate) struct FakeOta {
    store: Arc<Mutex<OtaStore>>,
    knobs: Arc<Mutex<OtaKnobs>>,
    journal: Journal,
}

/// App id the fake gives an image without a scripted one.
pub(crate) fn derived_app(bytes: &[u8]) -> AppId {
    let mut id = [0u8; 8];
    id[..4].copy_from_slice(&crc32(bytes, 0).to_le_bytes());
    id[4..].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
    AppId(id)
}

impl FakeOta {
    pub(crate) fn new(store: Arc<Mutex<OtaStore>>, journal: Journal) -> Self {
        FakeOta {
            store,
            knobs: Arc::default(),
            journal,
        }
    }
    pub(crate) fn knobs(&self) -> MutexGuard<'_, OtaKnobs> {
        lock(&self.knobs)
    }
    pub(crate) fn store(&self) -> MutexGuard<'_, OtaStore> {
        lock(&self.store)
    }
    fn info(store: &OtaStore, i: usize) -> SlotInfo {
        SlotInfo {
            address: SLOT_ADDR[i],
            size: SLOT_SIZE,
            app: store.slots[i].app,
        }
    }
}

impl Ota for FakeOta {
    type Update = FakeOtaUpdate;
    fn running(&self) -> SlotInfo {
        let s = lock(&self.store);
        Self::info(&s, s.running)
    }
    fn other(&self) -> Option<SlotInfo> {
        let s = lock(&self.store);
        if lock(&self.knobs).no_other {
            return None;
        }
        Some(Self::info(&s, 1 - s.running))
    }
    fn begin(&self) -> Result<FakeOtaUpdate, EspErr> {
        let s = lock(&self.store);
        let mut k = lock(&self.knobs);
        k.begins += 1;
        self.journal.note("ota begin");
        if let Some(e) = k.begin_err {
            return Err(e);
        }
        k.written.clear();
        Ok(FakeOtaUpdate {
            ota: self.clone(),
            slot: 1 - s.running,
            size: 0,
        })
    }
    fn set_boot(&self, address: u32) -> Result<(), EspErr> {
        let mut s = lock(&self.store);
        let mut k = lock(&self.knobs);
        k.set_boots.push(address);
        self.journal.note(format!("ota set_boot {address:#x}"));
        let i = SLOT_ADDR
            .iter()
            .position(|&a| a == address)
            .ok_or(EspErr::INVALID_ARG)?;
        if let Some(e) = k.set_boot_err {
            return Err(e);
        }
        if !s.slots[i].valid {
            return Err(EspErr::OTA_VALIDATE_FAILED);
        }
        s.otadata = i;
        Ok(())
    }
    fn verify(&self, address: u32) -> bool {
        let s = lock(&self.store);
        lock(&self.knobs).verifies.push(address);
        SLOT_ADDR
            .iter()
            .position(|&a| a == address)
            .is_some_and(|i| s.slots[i].valid)
    }
    fn mark_valid(&self) {
        lock(&self.knobs).mark_valids += 1;
        self.journal.note("ota mark_valid");
    }
    fn running_image_size(&self) -> u32 {
        lock(&self.knobs).image_size
    }
}

/// An update of [`FakeOta`].
pub(crate) struct FakeOtaUpdate {
    ota: FakeOta,
    slot: usize,
    size: usize,
}

impl OtaUpdate for FakeOtaUpdate {
    fn write(&mut self, data: &[u8]) -> Result<(), EspErr> {
        let mut s = lock(&self.ota.store);
        let mut k = lock(&self.ota.knobs);
        k.writes += 1;
        if self.size == 0 && data.first().is_some_and(|&b| b != 0xE9) {
            return Err(EspErr::OTA_VALIDATE_FAILED);
        }
        if k.fail_write_at
            .is_some_and(|at| self.size + data.len() > at)
        {
            return Err(k.write_err);
        }
        // sequential writes erase the slot sector by sector: the old image is gone
        s.slots[self.slot] = SlotImage::broken(None);
        k.written.extend_from_slice(data);
        self.size += data.len();
        Ok(())
    }
    fn finish(self) -> Result<(), EspErr> {
        let mut s = lock(&self.ota.store);
        let mut k = lock(&self.ota.knobs);
        k.finishes += 1;
        self.ota.journal.note("ota finish");
        if let Some(e) = k.finish_err {
            return Err(e);
        }
        if self.size == 0 {
            return Err(EspErr::OTA_VALIDATE_FAILED);
        }
        let app = k.next_app.take().unwrap_or_else(|| derived_app(&k.written));
        s.slots[self.slot] = SlotImage::glue(app);
        s.otadata = self.slot;
        Ok(())
    }
    fn abort(self) {
        lock(&self.ota.knobs).aborts += 1;
        self.ota.journal.note("ota abort");
    }
}
