//! Storage of the spike: the C++ firmware's NVS namespace `vdmrev`, read only, and its LittleFS
//! (partition `spiffs`, mounted at `/littlefs`), never formatted: a mount that fails leaves the
//! files of the C++ firmware as they are.

use esp_idf_svc::fs::littlefs::Littlefs;
use esp_idf_svc::io::vfs::MountedLittlefs;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use esp_idf_sys::EspError;

pub const FS_ROOT: &str = "/littlefs";
const FS_PARTITION: &str = "spiffs";

/// The default NVS partition, initialised without erasing it on any error (esp-idf-svc's
/// `take()` would erase it on "no free pages" and "new version").
pub fn nvs_partition() -> Result<EspDefaultNvsPartition, EspError> {
    EspDefaultNvsPartition::take_with(false)
}

/// What the C++ firmware's namespace holds (a few keys of DESIGN.md section 9), read only.
pub fn vdmrev_summary(part: &EspDefaultNvsPartition) -> String {
    let nvs: EspNvs<NvsDefault> = match EspNvs::new(part.clone(), "vdmrev", false) {
        Ok(n) => n,
        Err(e) => return format!("{{\"open\":false,\"error\":{}}}", e.code()),
    };
    let boots = nvs.get_u32("boots").ok().flatten();
    let cfg_len = nvs.blob_len("cfg").ok().flatten();
    let cfgx_len = nvs.blob_len("cfgx").ok().flatten();
    let imported = nvs.get_u8("imported").ok().flatten();
    format!(
        "{{\"open\":true,\"boots\":{},\"cfgBytes\":{},\"cfgxBytes\":{},\"imported\":{}}}",
        opt(boots),
        opt(cfg_len),
        opt(cfgx_len),
        opt(imported)
    )
}

fn opt<T: core::fmt::Display>(v: Option<T>) -> String {
    v.map_or_else(|| "null".into(), |v| v.to_string())
}

pub type Fs = MountedLittlefs<Littlefs<()>>;

/// Mounts the C++ firmware's LittleFS; `format_if_mount_failed` stays false.
pub fn mount_fs() -> Result<Fs, EspError> {
    // SAFETY: the partition is used by this filesystem only.
    let fs = unsafe { Littlefs::<()>::new_partition(FS_PARTITION)? };
    MountedLittlefs::mount(fs, FS_ROOT)
}

/// Paths and sizes of the files below the root and one directory level deeper.
pub fn list_fs() -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let Ok(top) = std::fs::read_dir(FS_ROOT) else { return out };
    for entry in top.flatten() {
        let path = entry.path();
        let name = path.to_string_lossy().into_owned();
        if path.is_dir() {
            if let Ok(sub) = std::fs::read_dir(&path) {
                for e in sub.flatten() {
                    let len = e.metadata().map_or(0, |m| m.len());
                    out.push((e.path().to_string_lossy().into_owned(), len));
                }
            }
            out.push((format!("{name}/"), 0));
        } else {
            out.push((name, entry.metadata().map_or(0, |m| m.len())));
        }
    }
    out.sort();
    out
}

/// QEMU harness only: appends a line to `/littlefs/rust-spike.txt`, so that a later boot of the
/// C++ firmware mounts a filesystem the Rust firmware wrote to.
#[cfg(feature = "qemu")]
pub fn append_marker(line: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(format!("{FS_ROOT}/rust-spike.txt"))?;
    writeln!(f, "{line}")
}
