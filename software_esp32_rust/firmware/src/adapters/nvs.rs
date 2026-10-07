//! NVS of the default partition through the raw `nvs_*` calls: every set, remove and erase
//! commits (Arduino `Preferences`); blobs are written without erasing the key first (a power cut
//! between an erase and the write would lose the old `cfg`); strings are bytes.

use core::ffi::{c_char, CStr};

use esp_idf_svc::sys;
use vdm_esp_glue::port::{Nvs, NvsInt, NvsNamespace};

/// The default NVS partition.
pub struct EspNvs;

impl EspNvs {
    /// `nvs_flash_init`, as Arduino-ESP32's `initArduino`: on "no free pages" and "new version
    /// found" the partition is erased and initialised again; any other error leaves it as it is
    /// (the opens then fail and the firmware runs on its defaults). Returns the error of the
    /// last init (`ESP_OK` = 0).
    pub fn init(&self) -> i32 {
        // SAFETY: plain init calls at boot, before any NVS user.
        unsafe {
            let err = sys::nvs_flash_init();
            if err == sys::ESP_ERR_NVS_NO_FREE_PAGES || err == sys::ESP_ERR_NVS_NEW_VERSION_FOUND {
                sys::nvs_flash_erase();
                return sys::nvs_flash_init();
            }
            err
        }
    }
}

/// A key or namespace as a C string (NVS names have at most 15 characters).
fn key<'b>(name: &str, buf: &'b mut [u8; 16]) -> Option<&'b CStr> {
    super::c_name(name, buf)
}

impl Nvs for EspNvs {
    type Ns = NvsHandle;
    fn open(&self, namespace: &str, write: bool) -> Option<NvsHandle> {
        let mut b = [0u8; 16];
        let ns = key(namespace, &mut b)?;
        let mode = if write {
            sys::nvs_open_mode_t_NVS_READWRITE
        } else {
            sys::nvs_open_mode_t_NVS_READONLY
        };
        let mut h: sys::nvs_handle_t = 0;
        // SAFETY: NUL-terminated name, valid out pointer.
        (unsafe { sys::nvs_open(ns.as_ptr(), mode, &mut h) } == sys::ESP_OK).then_some(NvsHandle(h))
    }
}

/// An open namespace (closed on drop).
pub struct NvsHandle(sys::nvs_handle_t);

impl Drop for NvsHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from nvs_open.
        unsafe { sys::nvs_close(self.0) };
    }
}

impl NvsHandle {
    fn commit(&self, err: i32) -> bool {
        // SAFETY: valid handle.
        err == sys::ESP_OK && unsafe { sys::nvs_commit(self.0) } == sys::ESP_OK
    }
}

impl NvsNamespace for NvsHandle {
    fn get_int(&self, name: &str, kind: NvsInt) -> Option<i64> {
        let mut b = [0u8; 16];
        let k = key(name, &mut b)?.as_ptr();
        let h = self.0;
        // SAFETY: valid handle, key and out pointers of the matching types.
        unsafe {
            match kind {
                NvsInt::U8 => {
                    let mut v = 0u8;
                    (sys::nvs_get_u8(h, k, &mut v) == sys::ESP_OK).then_some(i64::from(v))
                }
                NvsInt::I8 => {
                    let mut v = 0i8;
                    (sys::nvs_get_i8(h, k, &mut v) == sys::ESP_OK).then_some(i64::from(v))
                }
                NvsInt::U16 => {
                    let mut v = 0u16;
                    (sys::nvs_get_u16(h, k, &mut v) == sys::ESP_OK).then_some(i64::from(v))
                }
                NvsInt::I16 => {
                    let mut v = 0i16;
                    (sys::nvs_get_i16(h, k, &mut v) == sys::ESP_OK).then_some(i64::from(v))
                }
                NvsInt::U32 => {
                    let mut v = 0u32;
                    (sys::nvs_get_u32(h, k, &mut v) == sys::ESP_OK).then_some(i64::from(v))
                }
                NvsInt::I32 => {
                    let mut v = 0i32;
                    (sys::nvs_get_i32(h, k, &mut v) == sys::ESP_OK).then_some(i64::from(v))
                }
                NvsInt::I64 => {
                    let mut v = 0i64;
                    (sys::nvs_get_i64(h, k, &mut v) == sys::ESP_OK).then_some(v)
                }
            }
        }
    }
    fn set_int(&mut self, name: &str, kind: NvsInt, v: i64) -> bool {
        let mut b = [0u8; 16];
        let Some(k) = key(name, &mut b) else {
            return false;
        };
        let (h, k) = (self.0, k.as_ptr());
        // SAFETY: valid handle and key; the value is truncated to the type, as the port says.
        let err = unsafe {
            match kind {
                NvsInt::U8 => sys::nvs_set_u8(h, k, v as u8),
                NvsInt::I8 => sys::nvs_set_i8(h, k, v as i8),
                NvsInt::U16 => sys::nvs_set_u16(h, k, v as u16),
                NvsInt::I16 => sys::nvs_set_i16(h, k, v as i16),
                NvsInt::U32 => sys::nvs_set_u32(h, k, v as u32),
                NvsInt::I32 => sys::nvs_set_i32(h, k, v as i32),
                NvsInt::I64 => sys::nvs_set_i64(h, k, v),
            }
        };
        self.commit(err)
    }
    fn blob_len(&self, name: &str) -> Option<usize> {
        let mut b = [0u8; 16];
        let k = key(name, &mut b)?;
        let mut len = 0usize;
        // SAFETY: a null buffer asks for the length.
        let err = unsafe { sys::nvs_get_blob(self.0, k.as_ptr(), core::ptr::null_mut(), &mut len) };
        (err == sys::ESP_OK).then_some(len)
    }
    fn get_blob(&self, name: &str, out: &mut [u8]) -> Option<usize> {
        let mut b = [0u8; 16];
        let k = key(name, &mut b)?;
        let mut len = out.len();
        // SAFETY: buffer and length match; ESP_ERR_NVS_INVALID_LENGTH when it is too small.
        let err =
            unsafe { sys::nvs_get_blob(self.0, k.as_ptr(), out.as_mut_ptr().cast(), &mut len) };
        (err == sys::ESP_OK).then_some(len)
    }
    fn set_blob(&mut self, name: &str, data: &[u8]) -> bool {
        let mut b = [0u8; 16];
        let Some(k) = key(name, &mut b) else {
            return false;
        };
        // SAFETY: valid handle, key and data.
        let err =
            unsafe { sys::nvs_set_blob(self.0, k.as_ptr(), data.as_ptr().cast(), data.len()) };
        self.commit(err)
    }
    fn str_len(&self, name: &str) -> Option<usize> {
        let mut b = [0u8; 16];
        let k = key(name, &mut b)?;
        let mut len = 0usize;
        // SAFETY: a null buffer asks for the length (with the NUL).
        let err = unsafe { sys::nvs_get_str(self.0, k.as_ptr(), core::ptr::null_mut(), &mut len) };
        (err == sys::ESP_OK).then_some(len)
    }
    fn get_str(&self, name: &str, out: &mut [u8]) -> Option<usize> {
        let with_nul = self.str_len(name)?;
        let text = with_nul.checked_sub(1)?;
        if text > out.len() {
            return None;
        }
        let mut b = [0u8; 16];
        let k = key(name, &mut b)?;
        // nvs_get_str writes the NUL too: a buffer of the string's size
        let mut tmp: Vec<u8> = Vec::new();
        tmp.try_reserve_exact(with_nul).ok()?;
        tmp.resize(with_nul, 0);
        let mut len = with_nul;
        // SAFETY: buffer and length match.
        let err = unsafe {
            sys::nvs_get_str(
                self.0,
                k.as_ptr(),
                tmp.as_mut_ptr() as *mut c_char,
                &mut len,
            )
        };
        if err != sys::ESP_OK {
            return None;
        }
        out.get_mut(..text)?.copy_from_slice(tmp.get(..text)?);
        Some(text)
    }
    fn remove(&mut self, name: &str) -> bool {
        let mut b = [0u8; 16];
        let Some(k) = key(name, &mut b) else {
            return false;
        };
        // SAFETY: valid handle and key; ESP_ERR_NVS_NOT_FOUND when absent.
        let err = unsafe { sys::nvs_erase_key(self.0, k.as_ptr()) };
        self.commit(err)
    }
    fn erase_all(&mut self) -> bool {
        // SAFETY: valid handle.
        let err = unsafe { sys::nvs_erase_all(self.0) };
        self.commit(err)
    }
}
