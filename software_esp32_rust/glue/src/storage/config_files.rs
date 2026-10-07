//! The config backup files and the legacy import report on LittleFS (C++ storage.cpp, "config
//! files").
//!
//! `/sys/cfg.bak` and `/sys/cfgx.bak` hold the bytes of the last saved blobs, a pair that a load
//! takes together. Both are written to `.tmp` files first (base, then ext) and renamed over the
//! old ones in the same order, so a power cut never leaves a pair of two saves: `cfgx.bak.tmp`
//! without `cfg.bak.tmp` is the ext blob of the renamed base ([`Storage::backup_cut`]), any
//! other `.tmp` file belongs to a write that renamed nothing and left the old pair.

use vdm_esp_core::config::{load_config_blobs, Config, LoadInfo, StoredBlobs};
use vdm_esp_core::json_writer::JsonWriter;
use vdm_esp_core::legacy_import::{write_import_report_json, ImportReport};

use super::{
    encode_blobs, halves, CfgState, Storage, StorageHost, BACKUP_BASE, BACKUP_EXT, BLOBS_LEN,
    IMPORT_REPORT_FILE, KEY_CONFIG, KEY_CONFIG_EXT,
};
use crate::heap::try_bytes;
use crate::port::{Fs, FsFile, HeapGate, Nvs, NvsNamespace, OpenMode};

pub(super) const BACKUP_BASE_TMP: &str = "/sys/cfg.bak.tmp";
pub(super) const BACKUP_EXT_TMP: &str = "/sys/cfgx.bak.tmp";

/// Bytes the comparison of a backup file reads at a time.
const COMPARE_CHUNK: usize = 64;

impl<N: Nvs, F: Fs, G: HeapGate, H: StorageHost> Storage<'_, N, F, G, H> {
    /// A file of at most `out.len()` bytes into `out`; 0 when it is missing, empty, larger or
    /// unreadable.
    pub(super) fn read_file(&self, path: &str, out: &mut [u8]) -> usize {
        if !self.fs.exists(path) {
            return 0;
        }
        let Some(mut f) = self.fs.open(path, OpenMode::Read) else {
            return 0;
        };
        let len = f.size() as usize;
        let Some(dst) = out.get_mut(..len) else {
            return 0;
        };
        if f.read(dst) == len {
            len
        } else {
            0
        }
    }

    /// Creates or truncates `path` with exactly `data`.
    pub(super) fn write_file(&self, path: &str, data: &[u8]) -> bool {
        self.fs
            .open(path, OpenMode::Write)
            .is_some_and(|mut f| f.write(data) == data.len())
    }

    /// The file holds exactly these bytes.
    pub(super) fn same_file(&self, path: &str, data: &[u8]) -> bool {
        if !self.fs.exists(path) {
            return false;
        }
        let Some(mut f) = self.fs.open(path, OpenMode::Read) else {
            return false;
        };
        if f.size() as usize != data.len() {
            return false;
        }
        let mut chunk = [0u8; COMPARE_CHUNK];
        for want in data.chunks(COMPARE_CHUNK) {
            let got = &mut chunk[..want.len()];
            if f.read(got) != want.len() || got != want {
                return false;
            }
        }
        true
    }

    /// The backup files hold the first `n` bytes of the cfg part and the first `x` bytes of the
    /// cfgx part of `blobs`.
    pub(super) fn backup_equals(&self, blobs: &mut [u8], n: usize, x: usize) -> bool {
        let (base, ext) = halves(blobs);
        self.same_file(BACKUP_BASE, base.get(..n).unwrap_or_default())
            && self.same_file(BACKUP_EXT, ext.get(..x).unwrap_or_default())
    }

    /// A backup write cut off between its two renames: `/sys/cfg.bak` is new and its ext blob is
    /// still in `cfgx.bak.tmp`.
    pub(super) fn backup_cut(&self) -> bool {
        !self.fs.exists(BACKUP_BASE_TMP) && self.fs.exists(BACKUP_EXT_TMP)
    }

    /// The backup pair from the active config. Without memory for the blob buffers the backup
    /// stays pending (the next pass tries again); any other failure drops it until the next save.
    pub(super) fn write_backup(&self) {
        let mut st = self.cfg_lock();
        let Some(mut blobs) = try_bytes(&self.gate, BLOBS_LEN) else {
            return;
        };
        st.backup_pending = false;
        // New .tmp files must not meet the ext blob of a cut write: it goes first.
        if self.backup_cut() && !self.fs.rename(BACKUP_EXT_TMP, BACKUP_EXT) {
            return;
        }
        let written = match encode_blobs(st.kept(), &st.active, &mut blobs) {
            Some((n, x)) => {
                let (base, ext) = halves(&mut blobs);
                self.write_file(BACKUP_BASE_TMP, base.get(..n).unwrap_or_default())
                    && self.write_file(BACKUP_EXT_TMP, ext.get(..x).unwrap_or_default())
                    && self.fs.rename(BACKUP_BASE_TMP, BACKUP_BASE)
            }
            None => false,
        };
        if !written {
            // The ext first: a cut in between leaves cfg.bak.tmp, still a write that renamed
            // nothing.
            self.fs.remove(BACKUP_EXT_TMP);
            self.fs.remove(BACKUP_BASE_TMP);
            return;
        }
        // A failure keeps cfgx.bak.tmp as the pair of the new base.
        self.fs.rename(BACKUP_EXT_TMP, BACKUP_EXT);
    }

    /// The backup files as the config (`cfgx.bak.tmp` in place of `cfgx.bak` after a cut
    /// between the renames); on success NVS gets their bytes back (no `cfgx` without an ext
    /// file).
    pub(super) fn load_backup(
        &self,
        st: &mut CfgState,
        ns: &mut N::Ns,
        blobs: &mut [u8],
        out: &mut Config,
        info: &mut LoadInfo,
    ) -> bool {
        if !self.shared.fs_ready() {
            return false;
        }
        let ext_path = if self.backup_cut() {
            BACKUP_EXT_TMP
        } else {
            BACKUP_EXT
        };
        let (base, ext) = halves(blobs);
        let n = self.read_file(BACKUP_BASE, base);
        let x = self.read_file(ext_path, ext);
        let base = base.get(..n).unwrap_or_default();
        let ext = ext.get(..x).unwrap_or_default();
        if !load_config_blobs(&StoredBlobs { base, ext }, out, info, &mut st.keep) {
            return false;
        }
        st.keep_len = info.ext_info.keep_len;
        if ext.is_empty() {
            ns.remove(KEY_CONFIG_EXT);
        } else {
            ns.set_blob(KEY_CONFIG_EXT, ext);
        }
        ns.set_blob(KEY_CONFIG, base);
        true
    }

    /// `/sys/import.json` from the report and `c`; the caller holds the config lock (the cfg
    /// blob buffer is the text). False without a file system, without memory or on a write
    /// error.
    pub(super) fn write_report(&self, report: &ImportReport, c: &Config) -> bool {
        if !self.shared.fs_ready() {
            return false;
        }
        let Some(mut blobs) = try_bytes(&self.gate, BLOBS_LEN) else {
            return false;
        };
        let (text, _) = halves(&mut blobs);
        let mut jw = JsonWriter::new(text);
        write_import_report_json(&mut jw, report, c)
            && self.write_file(IMPORT_REPORT_FILE, jw.as_bytes())
    }

    /// The legacy import report from the active config; false when it was not written (no file
    /// system, no memory for its 4 KB text, a write error).
    pub fn write_import_report(&self, report: &ImportReport) -> bool {
        let st = self.cfg_lock();
        self.write_report(report, &st.active)
    }

    /// The import report file exists.
    pub fn has_import_report(&self) -> bool {
        self.shared.fs_ready() && self.fs.exists(IMPORT_REPORT_FILE)
    }

    /// Removes the import report; false when there was none.
    pub fn dismiss_import_report(&self) -> bool {
        self.shared.fs_ready() && self.fs.remove(IMPORT_REPORT_FILE)
    }
}
