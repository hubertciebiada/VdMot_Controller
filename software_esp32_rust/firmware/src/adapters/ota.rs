//! The OTA slots through the raw `esp_ota_*` calls (not `EspOta`: its update erases the whole
//! slot up front and it cannot select an arbitrary slot), and the ROM MD5.

use core::sync::atomic::{AtomicU32, Ordering};

use esp_idf_svc::sys;
use vdm_esp_glue::port::{AppId, EspErr, Md5, Ota, OtaUpdate, SlotInfo};

fn err(e: sys::esp_err_t) -> Result<(), EspErr> {
    if e == sys::ESP_OK {
        Ok(())
    } else {
        Err(EspErr(e))
    }
}

/// The app id of the slot `part`: the first 8 bytes of `esp_app_desc_t.app_elf_sha256`; `None`
/// without a readable app description.
fn app_of(part: *const sys::esp_partition_t) -> Option<AppId> {
    let mut desc = sys::esp_app_desc_t::default();
    // SAFETY: valid partition pointer and out buffer; reads the descriptor only.
    if unsafe { sys::esp_ota_get_partition_description(part, &mut desc) } != sys::ESP_OK {
        return None;
    }
    let mut id = [0u8; 8];
    id.copy_from_slice(desc.app_elf_sha256.get(..8)?);
    Some(AppId(id))
}

fn slot_of(part: *const sys::esp_partition_t) -> Option<SlotInfo> {
    // SAFETY: partition records of the table live as long as the firmware.
    let p = unsafe { part.as_ref() }?;
    Some(SlotInfo {
        address: p.address,
        size: p.size,
        app: app_of(part),
    })
}

/// The app slot at `address`: the running one or the other one (the table has two); null for
/// any other address.
fn partition_at(address: u32) -> *const sys::esp_partition_t {
    // SAFETY: plain lookups; partition records live as long as the firmware.
    unsafe {
        let running = sys::esp_ota_get_running_partition();
        let other = sys::esp_ota_get_next_update_partition(core::ptr::null());
        [running, other]
            .into_iter()
            .find(|p| p.as_ref().is_some_and(|p| p.address == address))
            .unwrap_or(core::ptr::null())
    }
}

/// The two OTA slots and otadata.
pub struct EspOta {
    /// `running_image_size`, read once (0 = not read yet).
    size: AtomicU32,
}

impl EspOta {
    pub const fn new() -> Self {
        EspOta {
            size: AtomicU32::new(0),
        }
    }
}

impl Ota for EspOta {
    type Update = EspOtaUpdate;
    fn running(&self) -> SlotInfo {
        // SAFETY: plain lookup.
        let p = unsafe { sys::esp_ota_get_running_partition() };
        slot_of(p).unwrap_or(SlotInfo {
            address: 0,
            size: 0,
            app: None,
        })
    }
    fn other(&self) -> Option<SlotInfo> {
        // SAFETY: plain lookup; null when there is no second app slot.
        let p = unsafe { sys::esp_ota_get_next_update_partition(core::ptr::null()) };
        slot_of(p)
    }
    fn begin(&self) -> Result<EspOtaUpdate, EspErr> {
        // SAFETY: plain lookup.
        let part = unsafe { sys::esp_ota_get_next_update_partition(core::ptr::null()) };
        if part.is_null() {
            return Err(EspErr::NOT_FOUND);
        }
        let mut handle: sys::esp_ota_handle_t = 0;
        // SAFETY: valid partition; sectors are erased as they are written.
        err(unsafe {
            sys::esp_ota_begin(part, sys::OTA_WITH_SEQUENTIAL_WRITES as usize, &mut handle)
        })?;
        Ok(EspOtaUpdate { handle, part })
    }
    fn set_boot(&self, address: u32) -> Result<(), EspErr> {
        let part = partition_at(address);
        if part.is_null() {
            return Err(EspErr::NOT_FOUND);
        }
        // SAFETY: valid partition; the image is verified before otadata is written.
        err(unsafe { sys::esp_ota_set_boot_partition(part) })
    }
    fn verify(&self, address: u32) -> bool {
        let part = partition_at(address);
        // SAFETY: valid partition record; the check only reads flash.
        let Some(p) = (unsafe { part.as_ref() }) else {
            return false;
        };
        let pos = sys::esp_partition_pos_t {
            offset: p.address,
            size: p.size,
        };
        let mut data = sys::esp_image_metadata_t::default();
        // SAFETY: valid arguments.
        unsafe {
            sys::esp_image_verify(
                sys::esp_image_load_mode_t_ESP_IMAGE_VERIFY_SILENT,
                &pos,
                &mut data,
            ) == sys::ESP_OK
        }
    }
    fn mark_valid(&self) {
        // SAFETY: plain call; the error without a PENDING_VERIFY entry is ignored.
        unsafe { sys::esp_ota_mark_app_valid_cancel_rollback() };
    }
    fn running_image_size(&self) -> u32 {
        let known = self.size.load(Ordering::Relaxed);
        if known != 0 {
            return known;
        }
        // SAFETY: plain lookup.
        let part = unsafe { sys::esp_ota_get_running_partition() };
        // SAFETY: partition records live as long as the firmware.
        let Some(p) = (unsafe { part.as_ref() }) else {
            return 0;
        };
        let pos = sys::esp_partition_pos_t {
            offset: p.address,
            size: p.size,
        };
        let mut data = sys::esp_image_metadata_t::default();
        // SAFETY: valid arguments; reads the whole image once.
        let ok = unsafe {
            sys::esp_image_verify(
                sys::esp_image_load_mode_t_ESP_IMAGE_VERIFY_SILENT,
                &pos,
                &mut data,
            )
        } == sys::ESP_OK;
        let n = if ok { data.image_len } else { 0 };
        self.size.store(n, Ordering::Relaxed);
        n
    }
}

/// An update between `esp_ota_begin` and `esp_ota_end`.
pub struct EspOtaUpdate {
    handle: sys::esp_ota_handle_t,
    part: *const sys::esp_partition_t,
}

// SAFETY: the handle is a number, the partition record is static data of the partition table.
unsafe impl Send for EspOtaUpdate {}

impl OtaUpdate for EspOtaUpdate {
    fn write(&mut self, data: &[u8]) -> Result<(), EspErr> {
        // SAFETY: valid handle and buffer; the first write checks the magic byte 0xE9.
        err(unsafe { sys::esp_ota_write(self.handle, data.as_ptr().cast(), data.len()) })
    }
    fn finish(self) -> Result<(), EspErr> {
        // SAFETY: valid handle (consumed by esp_ota_end); the image is verified there.
        err(unsafe { sys::esp_ota_end(self.handle) })?;
        // SAFETY: the partition the update wrote.
        err(unsafe { sys::esp_ota_set_boot_partition(self.part) })
    }
    fn abort(self) {
        // SAFETY: valid handle, consumed here.
        unsafe { sys::esp_ota_abort(self.handle) };
    }
}

/// MD5 of the ROM.
pub struct RomMd5(sys::md5_context_t);

impl RomMd5 {
    pub fn new() -> Self {
        let mut c = sys::md5_context_t::default();
        // SAFETY: valid context.
        unsafe { sys::esp_rom_md5_init(&mut c) };
        RomMd5(c)
    }
}

impl Md5 for RomMd5 {
    fn reset(&mut self) {
        // SAFETY: valid context.
        unsafe { sys::esp_rom_md5_init(&mut self.0) };
    }
    fn update(&mut self, data: &[u8]) {
        // SAFETY: valid context and buffer.
        unsafe { sys::esp_rom_md5_update(&mut self.0, data.as_ptr().cast(), data.len() as u32) };
    }
    fn digest(&mut self) -> [u8; 16] {
        let mut d = [0u8; 16];
        // SAFETY: valid context and 16-byte digest buffer.
        unsafe { sys::esp_rom_md5_final(d.as_mut_ptr(), &mut self.0) };
        d
    }
}
