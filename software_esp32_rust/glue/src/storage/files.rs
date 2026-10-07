//! The file manager's LittleFS walk and removal (C++ storage.cpp, "files"; the rules are core
//! `file_manager`): the file list (the root and one directory level below it), the delete of
//! one file, the removal of the images the legacy firmware left in the root, the usage figures.

use vdm_esp_core::common::{copy_string, TextBuf, NO_VALVE};
use vdm_esp_core::event_log::EventCode;
use vdm_esp_core::file_manager::{
    classify_fs_path, file_deletable, fs_path_valid, is_legacy_image_file, FileEntry, FS_PATH_MAX,
};

use super::{as_path, lock, Storage, StorageHost};
use crate::heap::try_block;
use crate::port::{Fs, FsEntry, FsFile, HeapGate, Nvs, OpenMode};

/// Legacy images collected per listing of the root: no removal while the root is listed.
const LEGACY_BATCH: usize = 8;
/// A path buffer: a file manager path and its NUL (the C++ buffers).
const PATH_BUF: usize = FS_PATH_MAX + 1;

/// Outcome of a file delete (the HTTP layer maps it to a status).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileResult {
    /// Removed.
    Ok = 0,
    /// Not a valid path, or a directory.
    BadPath = 1,
    /// A kind the dashboard may not delete, or the part file of a running upload or copy.
    Protected = 2,
    /// No such file.
    NotFound = 3,
    /// No file system, or a file operation failed.
    Io = 4,
}

/// KiB of `bytes`, rounded up (the C++ 32-bit arithmetic).
fn kib_of(bytes: u32) -> u32 {
    bytes.wrapping_add(1023) / 1024
}

/// "<dir>/<name>" in `buf` (a file manager path and its NUL); `None` when it does not fit.
fn entry_path(dir: &[u8], name: &[u8], buf: &mut [u8]) -> Option<usize> {
    let mut w = TextBuf::new(buf);
    w.push_bytes(dir);
    w.push(b'/');
    w.push_bytes(name);
    w.fit_opt()
}

/// The legacy images of one listing of the root.
struct LegacyBatch {
    paths: [[u8; PATH_BUF]; LEGACY_BATCH],
    lens: [usize; LEGACY_BATCH],
    sizes: [u32; LEGACY_BATCH],
    n: usize,
}

impl LegacyBatch {
    const EMPTY: Self = LegacyBatch {
        paths: [[0; PATH_BUF]; LEGACY_BATCH],
        lens: [0; LEGACY_BATCH],
        sizes: [0; LEGACY_BATCH],
        n: 0,
    };

    /// Adds `e` when it is a legacy image (its path cut to the C++ buffer: never on LittleFS,
    /// whose names have at most 64 bytes); false when the batch is full: the rest of the root
    /// waits for the next listing.
    fn collect(&mut self, e: &FsEntry) -> bool {
        let n = self.n;
        let (Some(buf), Some(len), Some(size)) = (
            self.paths.get_mut(n),
            self.lens.get_mut(n),
            self.sizes.get_mut(n),
        ) else {
            return false;
        };
        let Some(l) = entry_path(b"", e.name, buf) else {
            return true;
        };
        if e.dir || !is_legacy_image_file(buf.get(..l).unwrap_or_default()) {
            return true;
        }
        *len = l;
        *size = e.size;
        self.n += 1;
        true
    }

    /// The collected paths with their sizes, in listing order.
    fn entries(&self) -> impl Iterator<Item = (&[u8], u32)> {
        self.paths
            .iter()
            .zip(&self.lens)
            .zip(&self.sizes)
            .take(self.n)
            .map(|((p, &l), &s)| (p.get(..l).unwrap_or_default(), s))
    }
}

/// One listed file of `dir`; false when the list is full (truncated). A path that does not fit
/// a [`FileEntry`] is left out.
fn add_file(out: &mut [FileEntry], n: &mut usize, dir: &[u8], e: &FsEntry) -> bool {
    let Some(slot) = out.get_mut(*n) else {
        return false;
    };
    let mut buf = [0u8; PATH_BUF];
    if let Some(len) = entry_path(dir, e.name, &mut buf) {
        copy_string(&mut slot.path, buf.get(..len).unwrap_or_default());
        slot.size = e.size;
        *n += 1;
    }
    true
}

impl<N: Nvs, F: Fs, G: HeapGate, H: StorageHost> Storage<'_, N, F, G, H> {
    /// The files of the root and of the directories directly below it, in LittleFS order, at
    /// most `out.len()` of them (`truncated` when more exist). Returns their count.
    pub fn list_files(&self, out: &mut [FileEntry], truncated: &mut bool) -> usize {
        *truncated = false;
        let mut n = 0;
        if !self.shared.fs_ready() {
            return 0;
        }
        self.fs.list("/", &mut |e| {
            if !e.dir {
                *truncated = !add_file(out, &mut n, b"", e);
                return !*truncated;
            }
            // one level below the root
            let mut buf = [0u8; PATH_BUF];
            if let Some(len) = entry_path(b"", e.name, &mut buf) {
                let dir = buf.get(..len).unwrap_or_default();
                self.fs.list(as_path(dir), &mut |g| {
                    if !g.dir {
                        *truncated = !add_file(out, &mut n, dir, g);
                    }
                    !*truncated
                });
            }
            !*truncated
        });
        n
    }

    /// `path` names a directory (its parent lists it as one).
    fn is_dir(&self, path: &str) -> bool {
        let (parent, name) = match path.rfind('/') {
            Some(0) => ("/", path.get(1..).unwrap_or_default()),
            Some(i) => (
                path.get(..i).unwrap_or_default(),
                path.get(i + 1..).unwrap_or_default(),
            ),
            None => return false,
        };
        let mut dir = false;
        self.fs.list(parent, &mut |e| {
            if e.name == name.as_bytes() {
                dir = e.dir;
                return false;
            }
            true
        });
        dir
    }

    /// Removes one file the dashboard may delete (core `file_deletable`) and logs FilesRemoved
    /// (1 file, its KiB, the path). The part file of a running upload or last_good copy is
    /// protected; a directory is a bad path.
    pub fn delete_file(&self, path: &[u8]) -> FileResult {
        if !fs_path_valid(path) {
            return FileResult::BadPath;
        }
        if !file_deletable(classify_fs_path(path)) {
            return FileResult::Protected;
        }
        if !self.shared.fs_ready() {
            return FileResult::Io;
        }
        let p = as_path(path);
        let size = {
            let st = lock(&self.images);
            if st.part_in_use(path) {
                return FileResult::Protected;
            }
            if !self.fs.exists(p) {
                return FileResult::NotFound;
            }
            if self.is_dir(p) {
                return FileResult::BadPath;
            }
            let Some(f) = self.fs.open(p, OpenMode::Read) else {
                return FileResult::Io;
            };
            let size = f.size();
            drop(f);
            if !self.fs.remove(p) {
                return FileResult::Io;
            }
            size
        };
        self.host.log(
            EventCode::FilesRemoved,
            NO_VALVE,
            1,
            kib_of(size) as i32,
            path,
        );
        FileResult::Ok
    }

    /// Removes the images the legacy firmware left in the root (core `is_legacy_image_file`), in
    /// batches of [`LEGACY_BATCH`] collected before they are removed; a file that cannot be
    /// removed ends the run after its batch. Returns the number of files, `kib` the space freed.
    pub fn remove_legacy_images(&self, kib: &mut u32) -> u32 {
        *kib = 0;
        if !self.shared.fs_ready() {
            return 0;
        }
        // The batch (about 0.8 KB) takes a heap block for this call instead of the C++ stack
        // arrays (design 2.4); without memory no image is removed.
        let Some(mut batch) = try_block(&self.gate, || LegacyBatch::EMPTY) else {
            return 0;
        };
        let mut files = 0;
        let mut bytes = 0;
        while self.remove_legacy_batch(&mut batch, &mut files, &mut bytes) {}
        *kib = kib_of(bytes);
        files
    }

    /// One listing of the root collects up to [`LEGACY_BATCH`] legacy images, then they are
    /// removed (no removal while the root is listed). True when another batch may follow: this
    /// one was full and every removal worked.
    fn remove_legacy_batch(&self, b: &mut LegacyBatch, files: &mut u32, bytes: &mut u32) -> bool {
        b.n = 0;
        self.fs.list("/", &mut |e| b.collect(e));
        let mut more = b.n == LEGACY_BATCH;
        for (path, size) in b.entries() {
            if self.fs.remove(as_path(path)) {
                *files += 1;
                *bytes = bytes.wrapping_add(size);
            } else {
                more = false; // it would be found again
            }
        }
        more
    }

    /// Total bytes of the partition (0 without a file system).
    pub fn fs_total(&self) -> u32 {
        if self.shared.fs_ready() {
            self.fs.usage().0
        } else {
            0
        }
    }

    /// Used bytes of the partition (0 without a file system).
    pub fn fs_used(&self) -> u32 {
        if self.shared.fs_ready() {
            self.fs.usage().1
        } else {
            0
        }
    }
}
