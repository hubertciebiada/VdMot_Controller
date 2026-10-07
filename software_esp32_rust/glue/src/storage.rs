//! Persistent storage (C++ `storage.cpp`, `storage.h`): NVS namespace `vdmrev` (the config
//! blobs and small runtime values), read-only access to the legacy namespaces for the one-shot
//! import, the LittleFS mount (partition `spiffs`, shared with the legacy build), the config
//! backup files, the import report, the file list of the file manager and the STM image store
//! below /stm. Every NVS key and file holds the C++ bytes, and the load, repair and backup
//! protocol is the C++ one: the C++ firmware reads them after a switch back (DESIGN.md sections
//! 7 to 9).
//!
//! Parts:
//! - [`StorageShared`]: what storage publishes for the other tasks (LittleFS ready, the active
//!   config and its revision, saved since boot, the boot load result, the boot count). `main`
//!   creates it; its config mutex is the C++ config lock, which also serialises every NVS
//!   access of storage.
//! - [`Storage`]: the operations, shared by every task (`&self`, the C++ locks inside: the
//!   config lock above and the image index lock).
//! - [`StorageHost`]: the calls into the logger and net.
//!
//! Submodules: `config_files` (backup pair, import report), `images` (index, uploads, last_good
//! copy, scan, [`FileImage`]), `files` (file list and delete, legacy images).

mod config_files;
mod files;
mod images;

use core::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use vdm_esp_core::common::{fmt_trunc, TextBuf, TextView, NO_VALVE};
use vdm_esp_core::config::{
    encode_config, encode_config_ext, load_config_blobs, set_defaults, validate_config,
    CalibScheduleConfig, Config, ExtResult, LoadInfo, StoredBlobs, CONFIG_BLOB_MAX,
    CONFIG_EXT_BLOB_MAX, CONFIG_EXT_KEEP_MAX,
};
use vdm_esp_core::event_log::EventCode;
use vdm_esp_core::legacy_import::{import_legacy_config, ImportReport, LegacyNvsReader};

use crate::boot_guard::FACTORY_RESET_KEEPS;
use crate::heap::try_bytes;
use crate::port::{Fs, HeapGate, Nvs, NvsInt, NvsNamespace};

pub use files::FileResult;
pub use images::{
    image_path, image_result_name, normalize_image_name, FileImage, ImageEntry, ImageResult,
    FS_RESERVE, IMAGE_NAME_MAX, IMAGE_SLOTS, LAST_GOOD_NAME, MAX_IMAGE_SIZE, MAX_UPLOADED_IMAGES,
};

use images::{CopyIo, ImageState};

// NVS namespace and keys (binding, DESIGN.md section 9).
/// The namespace of the new firmware.
pub const NAMESPACE: &str = "vdmrev";
/// blob, core `encode_config`
pub const KEY_CONFIG: &str = "cfg";
/// blob, core `encode_config_ext`
pub const KEY_CONFIG_EXT: &str = "cfgx";
/// u8 1 = legacy import done (or not needed)
pub const KEY_IMPORTED: &str = "imported";
/// u32
pub const KEY_BOOT_COUNT: &str = "boots";
/// u32 yyyymmdd of the last scheduled calibration
pub const KEY_CALIB_SLOT: &str = "calSlot";
/// i64 epoch of the last calibration command
pub const KEY_LAST_CALIB: &str = "lastCal";
/// u8 1 = legacy DROP entities deleted
pub const KEY_HA_CLEANUP: &str = "haDrop";
/// blob, the desired targets (stm_service)
pub const KEY_TARGETS: &str = "targets";
/// blob, the network trial record (net)
pub const KEY_NET_TRIAL: &str = "netTrial";
/// u8 1 = factory reset done, pin still set
pub const KEY_FACTORY_LATCH: &str = "frLatch";
/// u8 1 = STM link up at the ESP OTA upload
pub const KEY_OTA_STM: &str = "otaStm";
/// u8 2 = the 2.1 discovery layout was published
pub const KEY_HA_LAYOUT: &str = "haLayout";

/// LittleFS file of the base config blob backup.
pub const BACKUP_BASE: &str = "/sys/cfg.bak";
/// LittleFS file of the ext config blob backup.
pub const BACKUP_EXT: &str = "/sys/cfgx.bak";
/// The legacy import report (`GET /api/import-report`).
pub const IMPORT_REPORT_FILE: &str = "/sys/import.json";

/// The cfg and cfgx encode/decode buffers in one heap block (4 KB + 1.5 KB; the cfg part is also
/// the scratch of the legacy import and the text of the import report).
const BLOBS_LEN: usize = CONFIG_BLOB_MAX + CONFIG_EXT_BLOB_MAX;
/// The larger record of the boot guard (`otaTrial`), which a factory reset keeps.
const GUARD_RECORD_MAX: usize = 32;

/// Where the config came from at boot.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoadSource {
    /// NVS `cfg` (+ `cfgx`), repaired field by field.
    Stored = 0,
    /// The legacy import (a legacy key was found).
    Imported = 1,
    /// Nothing stored (after a factory reset, or a device without legacy data).
    #[default]
    Defaults = 2,
    /// Nothing usable: `LoadDetails::error_code` says why; the bad blob is never overwritten
    /// automatically, the next explicit save replaces it.
    DefaultsAfterError = 3,
    /// The backup files (NVS rewritten from them).
    Backup = 4,
}

/// What `load_config` found: the reason for DefaultsAfterError and Backup in `error_code` (the
/// core `DecodeResult` value, 100 = NVS not usable, 101 = blob unreadable, 102 = no memory for
/// the blob buffers (nothing read, nothing written), 0 = no `cfg` in NVS), and the load details.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LoadDetails {
    /// 0 or the reason (see above).
    pub error_code: u8,
    /// The decode, ext and repair details of the blobs that were loaded or tried last.
    pub info: LoadInfo,
}

/// What storage asks of the other glue modules (C++ sibling calls). The firmware wiring
/// implements it, the tests fake it.
pub trait StorageHost {
    /// `logger::log(code, valve, arg1, arg2, text)`: an event with the code's default severity.
    fn log(&self, code: EventCode, valve: u8, arg1: i32, arg2: i32, text: &[u8]);
    /// `net::trialInfo().active`: a network trial runs.
    fn net_trial_active(&self) -> bool;
}

impl<T: StorageHost + ?Sized> StorageHost for &T {
    fn log(&self, code: EventCode, valve: u8, arg1: i32, arg2: i32, text: &[u8]) {
        (**self).log(code, valve, arg1, arg2, text)
    }
    fn net_trial_active(&self) -> bool {
        (**self).net_trial_active()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // a panic aborts the firmware, so a poisoned lock exists only in a failing test
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A path for the `Fs` port: the bytes as UTF-8, or "" (which names no file) when they are not.
/// Valid file manager paths and image paths are UTF-8; LittleFS names are bytes.
fn as_path(p: &[u8]) -> &str {
    core::str::from_utf8(p).unwrap_or("")
}

/// What the C++ config lock guarded.
struct CfgState {
    active: Box<Config>,
    /// Unknown `cfgx` records (a newer firmware's keys) read at boot and written back by every
    /// save ([`CONFIG_EXT_KEEP_MAX`] bytes).
    keep: Box<[u8]>,
    keep_len: usize,
    /// The blobs in NVS differ from the backup files (`service` writes them).
    backup_pending: bool,
}

impl CfgState {
    fn kept(&self) -> &[u8] {
        self.keep.get(..self.keep_len).unwrap_or_default()
    }
}

#[derive(Default)]
struct BootLoad {
    source: LoadSource,
    details: LoadDetails,
}

/// What storage publishes for the other tasks. `main` creates it at boot (the active config,
/// the kept records and the load details are boot allocations, D§9).
pub struct StorageShared {
    cfg: Mutex<CfgState>,
    revision: AtomicU32,
    fs_ready: AtomicBool,
    saved_since_boot: AtomicBool,
    boot_count: AtomicU32,
    boot: Mutex<Box<BootLoad>>,
}

impl Default for StorageShared {
    fn default() -> Self {
        Self::new()
    }
}

impl StorageShared {
    /// Defaults active, revision 0, LittleFS not mounted.
    pub fn new() -> Self {
        StorageShared {
            cfg: Mutex::new(CfgState {
                active: Box::default(),
                keep: vec![0u8; CONFIG_EXT_KEEP_MAX].into_boxed_slice(),
                keep_len: 0,
                backup_pending: false,
            }),
            revision: AtomicU32::new(0),
            fs_ready: AtomicBool::new(false),
            saved_since_boot: AtomicBool::new(false),
            boot_count: AtomicU32::new(0),
            boot: Mutex::new(Box::default()),
        }
    }

    /// LittleFS is mounted (C++ `fsReady()`).
    pub fn fs_ready(&self) -> bool {
        self.fs_ready.load(Ordering::Relaxed)
    }

    /// +1 on every published config (`set_active_config`, a successful `apply_config`).
    pub fn config_revision(&self) -> u32 {
        self.revision.load(Ordering::Relaxed)
    }

    /// True after the first successful `apply_config` since boot.
    pub fn config_saved_since_boot(&self) -> bool {
        self.saved_since_boot.load(Ordering::Relaxed)
    }

    /// The boot counter after `increment_boot_count` (0 before, or when NVS failed).
    pub fn boot_count(&self) -> u32 {
        self.boot_count.load(Ordering::Relaxed)
    }

    /// What `load_config` did at boot (`/api/status`).
    pub fn boot_load_source(&self) -> LoadSource {
        lock(&self.boot).source
    }

    /// The details of the boot load.
    pub fn boot_load_details(&self) -> LoadDetails {
        lock(&self.boot).details.clone()
    }

    /// Reads the active config under the lock without a copy (design 2.2).
    pub fn with_config<R>(&self, f: impl FnOnce(&Config) -> R) -> R {
        f(&lock(&self.cfg).active)
    }

    /// Copies the active config into `out` (~2.5 KB: callers keep it in a boot or heap block).
    pub fn get_config(&self, out: &mut Config) {
        out.clone_from(&lock(&self.cfg).active);
    }

    /// The calibration schedule of the active config (stm_service reads nothing else).
    pub fn calib_config(&self) -> CalibScheduleConfig {
        lock(&self.cfg).active.calib.clone()
    }

    /// Publishes `c` as the active config (boot, after the load): revision + 1.
    pub fn set_active_config(&self, c: &Config) {
        let mut st = lock(&self.cfg);
        Config::clone_from(&mut st.active, c);
        self.revision.fetch_add(1, Ordering::Relaxed);
    }
}

/// The cfg and cfgx parts of the blob buffers.
fn halves(blobs: &mut [u8]) -> (&mut [u8], &mut [u8]) {
    blobs.split_at_mut(CONFIG_BLOB_MAX.min(blobs.len()))
}

/// The encoded lengths of `c` into `blobs` (cfgx first, with the kept records; then cfg), `None`
/// when one does not fit (never with the full-size buffers).
fn encode_blobs(keep: &[u8], c: &Config, blobs: &mut [u8]) -> Option<(usize, usize)> {
    let (base, ext) = halves(blobs);
    let x = encode_config_ext(c, ext, keep);
    let n = encode_config(c, base);
    Some((NonZeroUsize::new(n)?.get(), NonZeroUsize::new(x)?.get()))
}

/// A stored blob of at most `out.len()` bytes; 0 when it is absent or larger.
fn load_bytes(ns: &impl NvsNamespace, key: &str, out: &mut [u8]) -> usize {
    ns.get_blob(key, out).unwrap_or(0)
}

/// u8 flag: 1 = set, absent = clear.
fn flag(ns: &impl NvsNamespace, key: &str) -> bool {
    ns.get_int(key, NvsInt::U8) == Some(1)
}

fn set_flag(ns: &mut impl NvsNamespace, key: &str, on: bool) {
    if on {
        ns.set_int(key, NvsInt::U8, 1);
    } else {
        ns.remove(key); // false when it was not there: nothing to do
    }
}

/// A boot guard record read before the erase of a factory reset.
struct Kept {
    record: [u8; GUARD_RECORD_MAX],
    len: Option<usize>,
}

impl Kept {
    /// The blob `key` when it is at most [`GUARD_RECORD_MAX`] bytes (no guard record is longer).
    fn read(ns: &impl NvsNamespace, key: &str) -> Kept {
        let mut record = [0u8; GUARD_RECORD_MAX];
        let len = ns.get_blob(key, &mut record);
        Kept { record, len }
    }

    /// Writes the record back, when there was one.
    fn restore(&self, ns: &mut impl NvsNamespace, key: &str) {
        if let Some(record) = self.len.and_then(|n| self.record.get(..n)) {
            ns.set_blob(key, record);
        }
    }
}

/// Erases the namespace except the factory latch (the pin is still set and must not reset
/// again) and the boot guard's records; false when the erase failed.
fn erase_keeping(ns: &mut impl NvsNamespace) -> bool {
    let latched = flag(ns, KEY_FACTORY_LATCH);
    let [ok_key, trial_key] = FACTORY_RESET_KEEPS;
    let ok = Kept::read(ns, ok_key);
    let trial = Kept::read(ns, trial_key);
    let erased = ns.erase_all();
    set_flag(ns, KEY_FACTORY_LATCH, latched);
    ok.restore(ns, ok_key);
    trial.restore(ns, trial_key);
    erased
}

/// Read-only view of the legacy namespaces (types are checked by trying the getters of every
/// integer width).
struct LegacyReader<'n, N> {
    nvs: &'n N,
}

/// The integer types of a legacy key, in the order the C++ tried them.
const LEGACY_INTS: [NvsInt; 7] = [
    NvsInt::U8,
    NvsInt::I8,
    NvsInt::U16,
    NvsInt::I16,
    NvsInt::U32,
    NvsInt::I32,
    NvsInt::I64,
];

impl<N: Nvs> LegacyNvsReader for LegacyReader<'_, N> {
    fn read_int(&mut self, ns: &str, key: &str) -> Option<i64> {
        let h = self.nvs.open(ns, false)?;
        LEGACY_INTS.iter().find_map(|&kind| h.get_int(key, kind))
    }

    fn read_string(&mut self, ns: &str, key: &str, out: &mut TextView) -> Option<bool> {
        out.clear();
        let h = self.nvs.open(ns, false)?;
        // the stored length includes the NUL: 0 is no string
        if h.str_len(key)? == 0 {
            return None;
        }
        let cap = out.capacity();
        let _ = out.resize(cap, 0);
        match h.get_str(key, out) {
            Some(n) => {
                out.truncate(n);
                Some(false)
            }
            None => {
                // longer than the C++ buffer: refused whole
                out.clear();
                Some(true)
            }
        }
    }

    fn read_blob(&mut self, ns: &str, key: &str, out: &mut [u8]) -> Option<usize> {
        let h = self.nvs.open(ns, false)?;
        let stored = h.blob_len(key)?;
        if h.get_blob(key, out).is_none() {
            // NVS cannot read a prefix; the importer rejects the size anyway
            out.fill(0);
        }
        Some(stored)
    }
}

/// The storage operations (C++ `storage::` functions), shared by every task.
pub struct Storage<'a, N, F: Fs, G, H> {
    shared: &'a StorageShared,
    nvs: N,
    fs: F,
    gate: G,
    host: H,
    /// The image index, the upload and the last_good copy flags (the C++ FS lock); file I/O
    /// happens outside the lock except for the upload writes and the short open/rename steps.
    images: Mutex<ImageState<F::File>>,
    /// The files of a running last_good copy (app task only; taken out while it works).
    copy_io: Mutex<Option<CopyIo<F::File>>>,
}

impl<'a, N: Nvs, F: Fs, G: HeapGate, H: StorageHost> Storage<'a, N, F, G, H> {
    /// Storage over its published data and its ports.
    pub fn new(shared: &'a StorageShared, nvs: N, fs: F, gate: G, host: H) -> Self {
        Storage {
            shared,
            nvs,
            fs,
            gate,
            host,
            images: Mutex::new(ImageState::default()),
            copy_io: Mutex::new(None),
        }
    }

    /// The published data.
    pub fn shared(&self) -> &'a StorageShared {
        self.shared
    }

    /// The C++ config lock: the active config, the kept records and every NVS access.
    fn cfg_lock(&self) -> MutexGuard<'a, CfgState> {
        lock(&self.shared.cfg)
    }

    fn prefs(&self) -> Option<N::Ns> {
        self.nvs.open(NAMESPACE, true)
    }

    // ------------------------------------------------------------ LittleFS

    /// Mounts LittleFS. Formats only when mounting fails (`formatted` is then true; the caller
    /// logs FsFormatted). Creates /stm, /log and /sys, removes upload leftovers (*.part) and
    /// indexes the STM images. False when even the format failed (the firmware then runs without
    /// files: no STM images, no log files).
    pub fn begin_fs(&self, formatted: &mut bool) -> bool {
        *formatted = false;
        let mut ready = self.fs.mount();
        if !ready {
            *formatted = self.fs.format();
            ready = *formatted && self.fs.mount();
        }
        self.shared.fs_ready.store(ready, Ordering::Relaxed);
        if ready {
            for dir in ["/stm", "/log", "/sys"] {
                if !self.fs.exists(dir) {
                    self.fs.mkdir(dir);
                }
            }
            self.index_images();
        }
        ready
    }

    /// LittleFS is mounted.
    pub fn fs_ready(&self) -> bool {
        self.shared.fs_ready()
    }

    // ------------------------------------------------------------ config

    /// Loads the config (core `load_config_blobs`: cfg + cfgx, repaired) in the order of
    /// DESIGN.md section 9: usable blobs -> Stored (unknown cfgx records are kept for the next
    /// save; the backup files are rewritten when they differ); unusable cfg -> the backup files
    /// (Backup, NVS rewritten from them), else DefaultsAfterError; missing cfg: `imported` set ->
    /// Defaults, else the backup files, else the legacy import then the save, the import report
    /// file and the removal of the legacy images. `report` is filled when an import ran. Logs
    /// the result (ConfigRepaired, ConfigNewerSchema, ConfigRestored, ConfigImported,
    /// ImportDropped, FilesRemoved, ConfigDefaults) and publishes it as the boot load.
    pub fn load_config(
        &self,
        out: &mut Config,
        report: &mut ImportReport,
        details: &mut LoadDetails,
    ) -> LoadSource {
        *details = LoadDetails::default();
        let src = self.load_stored(out, report, details);
        match src {
            LoadSource::Stored => self.log_stored(details),
            LoadSource::Backup => self.host.log(
                EventCode::ConfigRestored,
                NO_VALVE,
                i32::from(details.error_code),
                0,
                b"",
            ),
            LoadSource::Imported => self.log_imported(report, out),
            LoadSource::DefaultsAfterError => self.host.log(
                EventCode::ConfigDefaults,
                NO_VALVE,
                i32::from(details.error_code),
                0,
                b"",
            ),
            LoadSource::Defaults => {}
        }
        let mut boot = lock(&self.shared.boot);
        boot.source = src;
        boot.details.clone_from(details);
        src
    }

    /// The load order: NVS, the backup files, the legacy import, the defaults.
    fn load_stored(
        &self,
        out: &mut Config,
        report: &mut ImportReport,
        details: &mut LoadDetails,
    ) -> LoadSource {
        let mut st = self.cfg_lock();
        set_defaults(out);
        st.keep_len = 0;
        let Some(mut ns) = self.prefs() else {
            details.error_code = 100;
            return LoadSource::DefaultsAfterError;
        };
        let Some(mut blobs) = try_bytes(&self.gate, BLOBS_LEN) else {
            details.error_code = 102;
            return LoadSource::DefaultsAfterError;
        };
        let len = ns.blob_len(KEY_CONFIG).unwrap_or(0);
        if len > 0 {
            return self.load_present(&mut st, &mut ns, &mut blobs, len, out, details);
        }
        if ns.get_int(KEY_IMPORTED, NvsInt::U8) == Some(1) {
            return LoadSource::Defaults;
        }
        // NVS was erased
        if self.load_backup(&mut st, &mut ns, &mut blobs, out, &mut details.info) {
            return LoadSource::Backup;
        }
        self.import_legacy(&mut st, &mut ns, &mut blobs, out, report)
    }

    /// A `cfg` of `len` bytes is stored: it, else the backup files, else the defaults.
    fn load_present(
        &self,
        st: &mut CfgState,
        ns: &mut N::Ns,
        blobs: &mut [u8],
        len: usize,
        out: &mut Config,
        details: &mut LoadDetails,
    ) -> LoadSource {
        details.error_code = 101;
        if self.load_nvs(st, ns, blobs, len, out, details) {
            return LoadSource::Stored;
        }
        // Never overwritten automatically without a usable backup: the next explicit save
        // replaces it.
        if self.load_backup(st, ns, blobs, out, &mut details.info) {
            return LoadSource::Backup;
        }
        set_defaults(out);
        LoadSource::DefaultsAfterError
    }

    /// The one-shot legacy import (the cfg blob buffer is its scratch) and its save.
    fn import_legacy(
        &self,
        st: &mut CfgState,
        ns: &mut N::Ns,
        blobs: &mut [u8],
        out: &mut Config,
        report: &mut ImportReport,
    ) -> LoadSource {
        set_defaults(out);
        let (base, _) = halves(blobs);
        *report = import_legacy_config(&mut LegacyReader { nvs: &self.nvs }, out, base);
        if report.last_calib_epoch > 0 {
            ns.set_int(KEY_LAST_CALIB, NvsInt::I64, report.last_calib_epoch);
        }
        // "imported" only after the blob is safely stored: a failed save retries the
        // (idempotent) import on the next boot.
        if self.save_blobs(st, ns, blobs, out) {
            ns.set_int(KEY_IMPORTED, NvsInt::U8, 1);
        }
        if report.any_legacy {
            LoadSource::Imported
        } else {
            LoadSource::Defaults
        }
    }

    /// The `len` bytes of NVS `cfg` (and `cfgx`) as the config. On success the backup is due
    /// when it differs from the blobs; `details.error_code` stays 101 for an unreadable blob and
    /// becomes the decode result for an unusable one.
    fn load_nvs(
        &self,
        st: &mut CfgState,
        ns: &N::Ns,
        blobs: &mut [u8],
        len: usize,
        out: &mut Config,
        details: &mut LoadDetails,
    ) -> bool {
        let (base, ext) = halves(blobs);
        let Some(stored) = base.get_mut(..len) else {
            return false;
        };
        if ns.get_blob(KEY_CONFIG, stored) != Some(len) {
            return false;
        }
        let x = load_bytes(ns, KEY_CONFIG_EXT, ext);
        let b = StoredBlobs {
            base: stored,
            ext: ext.get(..x).unwrap_or_default(),
        };
        if !load_config_blobs(&b, out, &mut details.info, &mut st.keep) {
            details.error_code = details.info.base as u8;
            return false;
        }
        details.error_code = 0;
        st.keep_len = details.info.ext_info.keep_len;
        // First boot after the upgrade (or a lost file): the backup follows. A damaged cfgx left
        // the new keys at their defaults; the backup may still hold them, so only the next
        // explicit save replaces it.
        let ext_damaged = !matches!(details.info.ext, ExtResult::Absent | ExtResult::Ok);
        st.backup_pending = !ext_damaged
            && match encode_blobs(st.kept(), out, blobs) {
                Some((n, x)) => self.shared.fs_ready() && !self.backup_equals(blobs, n, x),
                None => false,
            };
        true
    }

    /// Writes the blobs of `c` (cfgx first: a cfg without its cfgx loads with the new keys at
    /// their defaults, a cfgx without its cfg is never read); the backup is due afterwards.
    fn save_blobs(&self, st: &mut CfgState, ns: &mut N::Ns, blobs: &mut [u8], c: &Config) -> bool {
        let Some((n, x)) = encode_blobs(st.kept(), c, blobs) else {
            return false;
        };
        let (base, ext) = halves(blobs);
        let ok = ns.set_blob(KEY_CONFIG_EXT, ext.get(..x).unwrap_or_default())
            && ns.set_blob(KEY_CONFIG, base.get(..n).unwrap_or_default());
        if ok {
            st.backup_pending = true;
        }
        ok
    }

    fn log_stored(&self, d: &LoadDetails) {
        let a = &d.info.decode.repairs;
        let b = &d.info.repairs;
        let mask = a.mask | b.mask;
        if mask != 0 {
            let first = if a.first.is_empty() {
                &b.first
            } else {
                &a.first
            };
            self.host.log(
                EventCode::ConfigRepaired,
                NO_VALVE,
                mask as i32,
                i32::from(a.count) + i32::from(b.count),
                first,
            );
        }
        if d.info.decode.newer_schema || d.info.ext_info.unknown > 0 {
            self.host.log(
                EventCode::ConfigNewerSchema,
                NO_VALVE,
                i32::from(d.info.decode.schema),
                i32::from(d.info.ext_info.unknown),
                b"",
            );
        }
    }

    fn log_imported(&self, report: &ImportReport, c: &Config) {
        self.host.log(
            EventCode::ConfigImported,
            NO_VALVE,
            i32::from(report.imported),
            i32::from(report.rejected),
            &report.first_rejected,
        );
        if report.dropped != 0 || report.pi_valves != 0 {
            let mut text = [0u8; 24];
            let n = fmt_trunc(&mut text, format_args!("ignored {} keys", report.ignored));
            self.host.log(
                EventCode::ImportDropped,
                NO_VALVE,
                i32::from(report.pi_valves),
                i32::from(report.dropped),
                text.get(..n).unwrap_or_default(),
            );
        }
        {
            let _cfg = self.cfg_lock();
            self.write_report(report, c);
        }
        let mut kib = 0;
        let files = self.remove_legacy_images(&mut kib);
        if files > 0 {
            self.host.log(
                EventCode::FilesRemoved,
                NO_VALVE,
                files as i32,
                kib as i32,
                b"legacy images",
            );
        }
    }

    /// Validates, persists (NVS cfgx, then cfg) and publishes a new config. On failure nothing
    /// changes and `path` receives the offending key ("nvs" when a write failed or the 5.5 KB
    /// blob buffers could not be allocated), cut to fit; the error is its length. `service`
    /// then copies the blobs to the backup files, not while a network trial runs.
    pub fn apply_config(&self, c: &Config, path: &mut [u8]) -> Result<(), usize> {
        validate_config(c, path)?;
        let mut st = self.cfg_lock();
        // no memory: fails like NVS
        let ok = match (try_bytes(&self.gate, BLOBS_LEN), self.prefs()) {
            (Some(mut blobs), Some(mut ns)) => self.save_blobs(&mut st, &mut ns, &mut blobs, c),
            _ => false,
        };
        if !ok {
            let mut w = TextBuf::new(path);
            w.push_bytes(b"nvs");
            return Err(w.len());
        }
        Config::clone_from(&mut st.active, c);
        self.shared.revision.fetch_add(1, Ordering::Relaxed);
        self.shared.saved_since_boot.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Factory reset: erases `vdmrev` except `frLatch` and the boot guard's records, sets
    /// `imported` (the legacy namespaces are left untouched and not imported again) and removes
    /// the backup and import report files. Keeping `otaOk` and `otaTrial` is Rust only (design
    /// 6.5; a C++ factory reset erases them and the next Rust boot validates itself again).
    pub fn factory_reset(&self) -> bool {
        let mut st = self.cfg_lock();
        if self.shared.fs_ready() {
            for f in [BACKUP_BASE, BACKUP_EXT, IMPORT_REPORT_FILE] {
                self.fs.remove(f);
            }
        }
        st.backup_pending = false;
        st.keep_len = 0;
        let Some(mut ns) = self.prefs() else {
            return false;
        };
        let erased = erase_keeping(&mut ns);
        ns.set_int(KEY_IMPORTED, NvsInt::U8, 1) && erased
    }

    // ------------------------------------------------------------ NVS values

    /// Adds one to NVS `boots` and returns it (0 when NVS cannot be opened).
    pub fn increment_boot_count(&self) -> u32 {
        let _cfg = self.cfg_lock();
        let Some(mut ns) = self.prefs() else {
            return 0;
        };
        let count = (ns.get_int(KEY_BOOT_COUNT, NvsInt::U32).unwrap_or(0) as u32).wrapping_add(1);
        ns.set_int(KEY_BOOT_COUNT, NvsInt::U32, i64::from(count));
        self.shared.boot_count.store(count, Ordering::Relaxed);
        count
    }

    /// The last scheduled calibration slot (yyyymmdd), 0 when none.
    pub fn load_calib_slot(&self) -> u32 {
        let _cfg = self.cfg_lock();
        self.prefs()
            .and_then(|ns| ns.get_int(KEY_CALIB_SLOT, NvsInt::U32))
            .map_or(0, |v| v as u32)
    }

    /// Books the calibration slot.
    pub fn save_calib_slot(&self, slot: u32) {
        let _cfg = self.cfg_lock();
        if let Some(mut ns) = self.prefs() {
            ns.set_int(KEY_CALIB_SLOT, NvsInt::U32, i64::from(slot));
        }
    }

    /// Epoch of the last calibration command, 0 when none.
    pub fn load_last_calib(&self) -> i64 {
        let _cfg = self.cfg_lock();
        self.prefs()
            .and_then(|ns| ns.get_int(KEY_LAST_CALIB, NvsInt::I64))
            .unwrap_or(0)
    }

    /// Stores the epoch of the last calibration command; 0 and below are ignored.
    pub fn save_last_calib(&self, epoch: i64) {
        let _cfg = self.cfg_lock();
        if let Some(mut ns) = self.prefs() {
            if epoch > 0 {
                ns.set_int(KEY_LAST_CALIB, NvsInt::I64, epoch);
            }
        }
    }

    /// The legacy DROP discovery entities were deleted.
    pub fn ha_cleanup_done(&self) -> bool {
        let _cfg = self.cfg_lock();
        self.prefs().is_some_and(|ns| flag(&ns, KEY_HA_CLEANUP))
    }

    /// Marks the legacy DROP entities deleted.
    pub fn set_ha_cleanup_done(&self) {
        let _cfg = self.cfg_lock();
        if let Some(mut ns) = self.prefs() {
            ns.set_int(KEY_HA_CLEANUP, NvsInt::U8, 1);
        }
    }

    /// Stores the desired targets as encoded by their owner; false for no bytes or a failed
    /// write.
    pub fn save_targets(&self, data: &[u8]) -> bool {
        let _cfg = self.cfg_lock();
        !data.is_empty()
            && self
                .prefs()
                .is_some_and(|mut ns| ns.set_blob(KEY_TARGETS, data))
    }

    /// The stored targets into `out`; 0 when none are stored or they are larger than `out`.
    pub fn load_targets(&self, out: &mut [u8]) -> usize {
        let _cfg = self.cfg_lock();
        self.prefs()
            .map_or(0, |ns| load_bytes(&ns, KEY_TARGETS, out))
    }

    /// The network trial record (raw bytes, the format belongs to net); 0 when none is stored
    /// or it is larger than `out`.
    pub fn load_net_trial_blob(&self, out: &mut [u8]) -> usize {
        let _cfg = self.cfg_lock();
        self.prefs()
            .map_or(0, |ns| load_bytes(&ns, KEY_NET_TRIAL, out))
    }

    /// Stores the network trial record; false for no bytes or a failed write.
    pub fn save_net_trial_blob(&self, data: &[u8]) -> bool {
        let _cfg = self.cfg_lock();
        !data.is_empty()
            && self
                .prefs()
                .is_some_and(|mut ns| ns.set_blob(KEY_NET_TRIAL, data))
    }

    /// Erases the network trial record.
    pub fn clear_net_trial(&self) {
        let _cfg = self.cfg_lock();
        if let Some(mut ns) = self.prefs() {
            ns.remove(KEY_NET_TRIAL);
        }
    }

    /// A factory reset was done while the pin was still set (kept by `factory_reset`).
    pub fn factory_latched(&self) -> bool {
        let _cfg = self.cfg_lock();
        self.prefs().is_some_and(|ns| flag(&ns, KEY_FACTORY_LATCH))
    }

    /// Sets or clears the factory latch.
    pub fn set_factory_latched(&self, on: bool) {
        let _cfg = self.cfg_lock();
        if let Some(mut ns) = self.prefs() {
            set_flag(&mut ns, KEY_FACTORY_LATCH, on);
        }
    }

    /// The STM link was up when an ESP OTA was uploaded (read and cleared at the next boot).
    pub fn ota_stm_required(&self) -> bool {
        let _cfg = self.cfg_lock();
        self.prefs().is_some_and(|ns| flag(&ns, KEY_OTA_STM))
    }

    /// Sets or clears `otaStm`.
    pub fn set_ota_stm_required(&self, on: bool) {
        let _cfg = self.cfg_lock();
        if let Some(mut ns) = self.prefs() {
            set_flag(&mut ns, KEY_OTA_STM, on);
        }
    }

    /// Clears `otaStm`.
    pub fn clear_ota_stm_required(&self) {
        self.set_ota_stm_required(false);
    }

    /// The Home Assistant discovery layout last published (0 = none).
    pub fn ha_layout(&self) -> u8 {
        let _cfg = self.cfg_lock();
        self.prefs()
            .and_then(|ns| ns.get_int(KEY_HA_LAYOUT, NvsInt::U8))
            .map_or(0, |v| v as u8)
    }

    /// Stores the discovery layout.
    pub fn set_ha_layout(&self, layout: u8) {
        let _cfg = self.cfg_lock();
        if let Some(mut ns) = self.prefs() {
            ns.set_int(KEY_HA_LAYOUT, NvsInt::U8, i64::from(layout));
        }
    }

    // ------------------------------------------------------------ app task

    /// App task: writes the pending config backup (not during a network trial: a revert must not
    /// find the trial settings in the backup), then either advances the last_good copy by at
    /// most 8 KiB or validates one unscanned image.
    pub fn service(&self) {
        if !self.shared.fs_ready() {
            return;
        }
        let pending = self.cfg_lock().backup_pending;
        if pending && !self.host.net_trial_active() {
            self.write_backup();
        }
        let copying = {
            let st = lock(&self.images);
            st.copy.running || st.copy.pending
        };
        if copying {
            self.service_copy();
            return;
        }
        self.service_scan();
    }
}

#[cfg(test)]
mod rig;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_images;
#[cfg(test)]
mod tests_values;
