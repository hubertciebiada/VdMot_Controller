//! LittleFS of the C++ firmware: partition `spiffs` at `/littlefs` (joltwallet/littlefs with the
//! Arduino-ESP32 2.0.7 settings of sdkconfig.defaults, disk version 2.0), never formatted by a
//! mount. Files through `std::fs`: `read`/`write` reach `lfs_file_*` directly (no stdio buffer).

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};

use esp_idf_svc::sys;
use vdm_esp_glue::port::{Fs, FsEntry, FsFile, OpenMode};

const LABEL: &core::ffi::CStr = c"spiffs";
const ROOT: &core::ffi::CStr = c"/littlefs";
const ROOT_STR: &str = "/littlefs";
/// "/littlefs" + a path of the glue (LittleFS names have at most 64 bytes, two levels).
const PATH_MAX: usize = 160;

/// The LittleFS partition.
pub struct LittleFs;

/// `/littlefs` + `path` in `buf` as a `&str`; `None` when it does not fit.
fn full<'b>(path: &str, buf: &'b mut [u8; PATH_MAX]) -> Option<&'b str> {
    let (root, rest) = (ROOT_STR.as_bytes(), path.as_bytes());
    let n = root.len() + rest.len();
    buf.get_mut(..root.len())?.copy_from_slice(root);
    buf.get_mut(root.len()..n)?.copy_from_slice(rest);
    core::str::from_utf8(buf.get(..n)?).ok()
}

impl Fs for LittleFs {
    type File = LfsFile;
    fn mount(&self) -> bool {
        let mut conf = sys::esp_vfs_littlefs_conf_t {
            base_path: ROOT.as_ptr(),
            partition_label: LABEL.as_ptr(),
            ..Default::default()
        };
        conf.set_format_if_mount_failed(0);
        // SAFETY: static C strings; the VFS keeps its own copies.
        unsafe { sys::esp_vfs_littlefs_register(&conf) == sys::ESP_OK }
    }
    fn format(&self) -> bool {
        // SAFETY: static label.
        unsafe { sys::esp_littlefs_format(LABEL.as_ptr()) == sys::ESP_OK }
    }
    fn open(&self, path: &str, mode: OpenMode) -> Option<LfsFile> {
        let mut b = [0u8; PATH_MAX];
        let p = full(path, &mut b)?;
        let f = match mode {
            OpenMode::Read => File::open(p),
            OpenMode::Write => File::create(p),
            OpenMode::Append => OpenOptions::new().append(true).create(true).open(p),
        };
        f.ok().map(LfsFile)
    }
    fn exists(&self, path: &str) -> bool {
        let mut b = [0u8; PATH_MAX];
        full(path, &mut b).is_some_and(|p| std::fs::metadata(p).is_ok())
    }
    fn mkdir(&self, path: &str) -> bool {
        let mut b = [0u8; PATH_MAX];
        full(path, &mut b).is_some_and(|p| std::fs::create_dir(p).is_ok())
    }
    fn remove(&self, path: &str) -> bool {
        let mut b = [0u8; PATH_MAX];
        let Some(p) = full(path, &mut b) else {
            return false;
        };
        // LittleFS's unlink removes files and empty directories alike
        std::fs::remove_file(p).is_ok() || std::fs::remove_dir(p).is_ok()
    }
    fn rename(&self, from: &str, to: &str) -> bool {
        let (mut a, mut b) = ([0u8; PATH_MAX], [0u8; PATH_MAX]);
        match (full(from, &mut a), full(to, &mut b)) {
            (Some(f), Some(t)) => std::fs::rename(f, t).is_ok(),
            _ => false,
        }
    }
    fn list(&self, dir: &str, visit: &mut dyn FnMut(&FsEntry) -> bool) {
        let mut b = [0u8; PATH_MAX];
        let Some(p) = full(dir, &mut b) else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(p) else {
            return;
        };
        for e in entries.flatten() {
            let name = e.file_name();
            let meta = e.metadata().ok();
            let entry = FsEntry {
                name: name.as_encoded_bytes(),
                size: meta.as_ref().map_or(0, |m| m.len() as u32),
                dir: meta.as_ref().is_some_and(|m| m.is_dir()),
            };
            if !visit(&entry) {
                return;
            }
        }
    }
    fn usage(&self) -> (u32, u32) {
        let (mut total, mut used) = (0usize, 0usize);
        // SAFETY: static label, valid out pointers.
        unsafe { sys::esp_littlefs_info(LABEL.as_ptr(), &mut total, &mut used) };
        (total as u32, used as u32)
    }
}

/// An open file (closed on drop).
pub struct LfsFile(File);

impl FsFile for LfsFile {
    fn read(&mut self, out: &mut [u8]) -> usize {
        self.0.read(out).unwrap_or(0)
    }
    fn write(&mut self, data: &[u8]) -> usize {
        self.0.write(data).unwrap_or(0)
    }
    fn seek(&mut self, pos: u32) -> bool {
        self.0.seek(SeekFrom::Start(u64::from(pos))).is_ok()
    }
    fn size(&self) -> u32 {
        self.0.metadata().map_or(0, |m| m.len() as u32)
    }
}
