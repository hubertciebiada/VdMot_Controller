//! The STM image store below /stm (C++ storage.cpp, "images"): the index and its slots, upload
//! leftovers, the upload with its limits, the last_good copy after a flash, the image scan, and
//! [`FileImage`] for the flasher and the validator.
//!
//! Images live in `/stm/<name>.bin`. Names follow the `{name}` rule of core `matchApiRoute`,
//! `[A-Za-z0-9._-]{1,31}` without a leading '.', and exclude the ".bin" suffix. "last_good"
//! holds a copy of the last successfully flashed image and cannot be uploaded.

use vdm_esp_core::common::{c_str, copy_string, Text, TextBuf, NO_VALVE};
use vdm_esp_core::config::crc32;
use vdm_esp_core::event_log::EventCode;
use vdm_esp_core::stm_flasher::{validate_image, FlashError, FlashImage, ImageInfo};

pub use vdm_esp_core::image_store::{image_path, normalize_image_name, IMAGE_NAME_MAX};

use super::{as_path, lock, Storage, StorageHost};
use crate::heap::try_bytes;
use crate::port::{Fs, FsFile, HeapGate, Nvs, OpenMode};

/// Largest image.
pub const MAX_IMAGE_SIZE: usize = 512 * 1024;
/// Uploaded images besides last_good.
pub const MAX_UPLOADED_IMAGES: usize = 3;
/// Index slots: the uploaded images and last_good.
pub const IMAGE_SLOTS: usize = MAX_UPLOADED_IMAGES + 1;
/// The copy of the last successfully flashed image.
pub const LAST_GOOD_NAME: &str = "last_good";
/// LittleFS space that must stay free after an upload (log rotation, metadata, the last_good
/// copy).
pub const FS_RESERVE: usize = 2 * 64 * 1024 + 16 * 1024;

/// The last_good image and the file of its running copy.
const LAST_GOOD_PATH: &str = "/stm/last_good.bin";
const LAST_GOOD_PART: &str = "/stm/last_good.bin.part";
/// The last_good copy moves this many bytes per chunk ...
const COPY_CHUNK: usize = 1024;
/// ... and at most this many chunks per app task pass (8 KiB).
const COPY_CHUNKS_PER_CALL: usize = 8;
/// Space that must stay free besides the image for a last_good copy.
const COPY_RESERVE: usize = 16 * 1024;
/// Upload leftovers removed per boot.
const MAX_LEFTOVERS: usize = 8;
/// The C++ path buffers (`char path[48]`): "/stm/" + 31 + ".bin.part" fits.
const PATH_CAP: usize = 48;

/// One image of the index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImageEntry {
    /// The bare name ("" = free slot).
    pub name: Text<IMAGE_NAME_MAX>,
    /// File size in bytes.
    pub size: u32,
    /// `validate_image` ran.
    pub scanned: bool,
    /// Chip-independent checks incl. the handshake strings.
    pub check: FlashError,
    /// CRC-32 of the image, valid when scanned.
    pub crc: u32,
    /// The version string of the image, "" = none found.
    pub version: Text<31>,
    /// Board of the image ("C2"), "" untagged.
    pub hw_tag: Text<3>,
}

/// Outcome of an image operation (the HTTP layer maps it to a status).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageResult {
    /// Done.
    Ok = 0,
    /// Not a valid image name, or last_good.
    BadName = 1,
    /// Above [`MAX_IMAGE_SIZE`].
    TooLarge = 2,
    /// The image and [`FS_RESERVE`] do not fit.
    NoSpace = 3,
    /// [`MAX_UPLOADED_IMAGES`] already.
    TooMany = 4,
    /// Another upload runs, or the last_good copy reads the image.
    Busy = 5,
    /// A file operation failed (or no upload runs).
    Io = 6,
    /// No such image.
    NotFound = 7,
    /// An upload without bytes.
    Empty = 8,
}

/// "ok", "bad_name", "too_large", "no_space", "too_many_images", "busy", "io_error",
/// "not_found", "empty".
pub fn image_result_name(r: ImageResult) -> &'static str {
    match r {
        ImageResult::Ok => "ok",
        ImageResult::BadName => "bad_name",
        ImageResult::TooLarge => "too_large",
        ImageResult::NoSpace => "no_space",
        ImageResult::TooMany => "too_many_images",
        ImageResult::Busy => "busy",
        ImageResult::Io => "io_error",
        ImageResult::NotFound => "not_found",
        ImageResult::Empty => "empty",
    }
}

/// "/stm/<name>.bin" (+ ".part") in the C++ 48-byte buffer.
struct ImagePath {
    buf: [u8; PATH_CAP],
    len: usize,
}

impl ImagePath {
    /// `None` when it does not fit.
    fn new(name: &[u8], part: bool) -> Option<Self> {
        let mut buf = [0u8; PATH_CAP];
        let len = image_path(name, part, &mut buf)?;
        Some(ImagePath { buf, len })
    }
    fn bytes(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or_default()
    }
    fn as_str(&self) -> &str {
        as_path(self.bytes())
    }
}

/// The running upload: the `.part` file open while it runs.
pub(super) struct Upload<T> {
    pub(super) file: Option<T>,
    pub(super) name: Text<IMAGE_NAME_MAX>,
    written: u32,
    crc: u32,
}

/// The last_good copy as the other tasks see it.
#[derive(Default)]
pub(super) struct CopyFlags {
    pub(super) pending: bool,
    pub(super) running: bool,
    pub(super) name: Text<IMAGE_NAME_MAX>,
}

/// The files of a running last_good copy.
pub(super) struct CopyIo<T> {
    src: T,
    dst: T,
    done: u32,
}

/// The index, the upload and the copy flags (the C++ FS lock).
pub(super) struct ImageState<T> {
    entries: [ImageEntry; IMAGE_SLOTS],
    pub(super) upload: Upload<T>,
    pub(super) copy: CopyFlags,
}

impl<T> Default for ImageState<T> {
    fn default() -> Self {
        ImageState {
            entries: Default::default(),
            upload: Upload {
                file: None,
                name: Text::new(),
                written: 0,
                crc: 0,
            },
            copy: CopyFlags::default(),
        }
    }
}

impl<T> ImageState<T> {
    /// Index slot of `name`.
    fn find(&self, name: &[u8]) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| !e.name.is_empty() && e.name.as_slice() == name)
    }

    /// Adds or refreshes an entry (unscanned); false when the index is full.
    fn put(&mut self, name: &[u8], size: u32) -> bool {
        let slot = match self.find(name) {
            Some(i) => self.entries.get_mut(i),
            None => self.entries.iter_mut().find(|e| e.name.is_empty()),
        };
        let Some(e) = slot else {
            return false;
        };
        *e = ImageEntry::default();
        copy_string(&mut e.name, name);
        e.size = size;
        true
    }

    /// Frees the slot of `name`, if it has one.
    fn forget(&mut self, name: &[u8]) {
        if let Some(e) = self.find(name).and_then(|i| self.entries.get_mut(i)) {
            *e = ImageEntry::default();
        }
    }

    /// Uploaded images (not last_good) other than `except`.
    fn uploaded_count(&self, except: &[u8]) -> usize {
        self.entries
            .iter()
            .filter(|e| {
                !e.name.is_empty()
                    && e.name.as_slice() != LAST_GOOD_NAME.as_bytes()
                    && e.name.as_slice() != except
            })
            .count()
    }

    /// The upload of `name` runs.
    fn uploading(&self, name: &[u8]) -> bool {
        self.upload.file.is_some() && self.upload.name.as_slice() == name
    }

    /// `path` is the `.part` file of the running upload or of the running last_good copy. The
    /// upload writes its part under the lock; the copy writes its own outside it and sets
    /// `running` with the file under it.
    pub(super) fn part_in_use(&self, path: &[u8]) -> bool {
        let upload = self.upload.file.is_some()
            && ImagePath::new(&self.upload.name, true).is_some_and(|p| p.bytes() == path);
        upload || (self.copy.running && LAST_GOOD_PART.as_bytes() == path)
    }
}

impl<N: Nvs, F: Fs, G: HeapGate, H: StorageHost> Storage<'_, N, F, G, H> {
    /// Builds the index from /stm and removes upload leftovers (at most
    /// [`MAX_LEFTOVERS`] per boot, paths cut to the C++ 48-byte buffer). Boot only.
    pub(super) fn index_images(&self) {
        let mut leftovers = [([0u8; PATH_CAP], 0usize); MAX_LEFTOVERS];
        let mut n_left = 0;
        self.fs.list("/stm", &mut |e| {
            if e.dir {
                return true;
            }
            if e.name.ends_with(b".part") {
                if let Some((buf, len)) = leftovers.get_mut(n_left) {
                    let mut w = TextBuf::new(buf);
                    w.push_bytes(b"/stm/");
                    w.push_bytes(e.name);
                    *len = w.len();
                    n_left += 1;
                }
                return true;
            }
            let mut name = [0u8; IMAGE_NAME_MAX + 1];
            if e.name.ends_with(b".bin") {
                if let Some(n) = normalize_image_name(e.name, &mut name) {
                    lock(&self.images).put(&name[..n], e.size);
                }
            }
            true
        });
        for (buf, len) in leftovers.iter().take(n_left) {
            self.fs.remove(as_path(buf.get(..*len).unwrap_or_default()));
        }
    }

    /// Free LittleFS bytes (0 without a file system).
    pub(super) fn fs_free(&self) -> usize {
        let (total, used) = self.fs.usage();
        total.saturating_sub(used) as usize
    }

    /// Copies of the index entries in use into `out` (no file I/O, safe from HTTP handlers);
    /// returns their count.
    pub fn list_images(&self, out: &mut [ImageEntry]) -> usize {
        let st = lock(&self.images);
        let mut n = 0;
        for (slot, e) in out
            .iter_mut()
            .zip(st.entries.iter().filter(|e| !e.name.is_empty()))
        {
            slot.clone_from(e);
            n += 1;
        }
        n
    }

    /// A copy of the index entry of `name` (a C string).
    pub fn find_image(&self, name: &[u8]) -> Option<ImageEntry> {
        let st = lock(&self.images);
        let i = st.find(c_str(name))?;
        st.entries.get(i).cloned()
    }

    /// Starts an upload into `/stm/<name>.bin.part` (renamed over `/stm/<name>.bin` on success).
    /// One upload at a time; `name` is a C string ("x" or "x.bin"); `announced` bytes plus
    /// [`FS_RESERVE`] must be free.
    pub fn image_upload_begin(&self, name: &[u8], announced: usize) -> ImageResult {
        if !self.shared.fs_ready() {
            return ImageResult::Io;
        }
        let mut clean = [0u8; IMAGE_NAME_MAX + 1];
        let Some(n) = normalize_image_name(c_str(name), &mut clean) else {
            return ImageResult::BadName;
        };
        let clean = &clean[..n];
        if clean == LAST_GOOD_NAME.as_bytes() {
            return ImageResult::BadName;
        }
        let Some(path) = ImagePath::new(clean, true) else {
            return ImageResult::BadName;
        };
        let mut st = lock(&self.images);
        if st.upload.file.is_some() {
            return ImageResult::Busy;
        }
        // At most 3 uploaded images besides the one being replaced; with last_good that never
        // exceeds IMAGE_SLOTS index entries.
        if st.uploaded_count(clean) >= MAX_UPLOADED_IMAGES {
            return ImageResult::TooMany;
        }
        if self.fs_free() < announced.wrapping_add(FS_RESERVE) {
            return ImageResult::NoSpace;
        }
        let Some(file) = self.fs.open(path.as_str(), OpenMode::Write) else {
            return ImageResult::Io;
        };
        st.upload.file = Some(file);
        copy_string(&mut st.upload.name, clean);
        st.upload.written = 0;
        st.upload.crc = 0;
        ImageResult::Ok
    }

    /// Closes the upload and removes its `.part` file.
    fn abort_upload(&self, st: &mut ImageState<F::File>) {
        st.upload.file = None;
        if let Some(part) = ImagePath::new(&st.upload.name, true) {
            self.fs.remove(part.as_str());
        }
    }

    /// The next bytes of the upload: every write is checked, the size limit holds for the
    /// running total (a failure removes the part file and ends the upload).
    pub fn image_upload_write(&self, data: &[u8]) -> ImageResult {
        let mut st = lock(&self.images);
        if st.upload.file.is_none() {
            return ImageResult::Io;
        }
        if data.len() > MAX_IMAGE_SIZE - st.upload.written as usize {
            self.abort_upload(&mut st);
            return ImageResult::TooLarge;
        }
        let failed = !data.is_empty()
            && st
                .upload
                .file
                .as_mut()
                .is_none_or(|f| f.write(data) != data.len());
        if failed {
            self.abort_upload(&mut st);
            return ImageResult::Io;
        }
        st.upload.crc = crc32(data, st.upload.crc);
        st.upload.written += data.len() as u32;
        ImageResult::Ok
    }

    /// Ends the upload: the part file becomes the image (replacing an old one) and is indexed.
    /// The entry carries the name, the size and the CRC of the bytes.
    pub fn image_upload_end(&self) -> Result<ImageEntry, ImageResult> {
        let mut st = lock(&self.images);
        self.finish_upload(&mut st)?;
        let mut info = ImageEntry::default();
        info.name.clone_from(&st.upload.name);
        info.size = st.upload.written;
        info.crc = st.upload.crc;
        Ok(info)
    }

    /// Closes the part file, renames it over the image and indexes the image.
    fn finish_upload(&self, st: &mut ImageState<F::File>) -> Result<(), ImageResult> {
        if st.upload.file.is_none() {
            return Err(ImageResult::Io);
        }
        if st.upload.written == 0 {
            self.abort_upload(st);
            return Err(ImageResult::Empty);
        }
        st.upload.file = None;
        let name = st.upload.name.clone();
        if !self.rename_part(&name) {
            // the old file is gone as well
            st.forget(&name);
            self.abort_upload(st);
            return Err(ImageResult::Io);
        }
        if !st.put(&name, st.upload.written) {
            // Cannot happen (the slots were checked at begin); never keep an unindexed file.
            if let Some(path) = ImagePath::new(&name, false) {
                self.fs.remove(path.as_str());
            }
            return Err(ImageResult::TooMany);
        }
        Ok(())
    }

    /// Replaces `/stm/<name>.bin` by `/stm/<name>.bin.part`; false when the rename failed (the
    /// old image is removed first).
    fn rename_part(&self, name: &[u8]) -> bool {
        let (Some(part), Some(path)) = (ImagePath::new(name, true), ImagePath::new(name, false))
        else {
            return false;
        };
        self.fs.remove(path.as_str());
        self.fs.rename(part.as_str(), path.as_str())
    }

    /// Ends a running upload and removes its part file.
    pub fn image_upload_abort(&self) {
        let mut st = lock(&self.images);
        if st.upload.file.is_some() {
            self.abort_upload(&mut st);
        }
    }

    /// An upload runs.
    pub fn image_upload_active(&self) -> bool {
        lock(&self.images).upload.file.is_some()
    }

    /// Removes the image `name` (a C string) and its index entry; Busy while the last_good copy
    /// reads it.
    pub fn delete_image(&self, name: &[u8]) -> ImageResult {
        let name = c_str(name);
        let Some(path) = ImagePath::new(name, false) else {
            return ImageResult::BadName;
        };
        let mut st = lock(&self.images);
        if st.find(name).is_none() {
            return ImageResult::NotFound;
        }
        if st.copy.running && st.copy.name.as_slice() == name {
            return ImageResult::Busy;
        }
        if !self.fs.remove(path.as_str()) && self.fs.exists(path.as_str()) {
            return ImageResult::Io;
        }
        st.forget(name);
        ImageResult::Ok
    }

    /// After a successful flash: copy the image `name` (a C string) to last_good (done
    /// incrementally by `service`).
    pub fn request_last_good_copy(&self, name: &[u8]) {
        let mut st = lock(&self.images);
        copy_string(&mut st.copy.name, name);
        st.copy.pending = true;
    }

    /// One bounded step of the last_good copy: it starts (when requested) and moves up to 8 KiB
    /// through a 1 KiB heap buffer taken for this pass (a pass without one copies nothing).
    pub(super) fn service_copy(&self) {
        let taken = lock(&self.copy_io).take();
        let io = match taken {
            Some(io) => io,
            None => match self.start_copy() {
                Some(io) => io,
                None => return,
            },
        };
        let Some(mut buf) = try_bytes(&self.gate, COPY_CHUNK) else {
            // the next pass goes on
            *lock(&self.copy_io) = Some(io);
            return;
        };
        let rest = self.copy_chunks(io, &mut buf);
        *lock(&self.copy_io) = rest;
    }

    /// Opens the image and the part file of a requested copy.
    fn start_copy(&self) -> Option<CopyIo<F::File>> {
        let name = {
            let mut st = lock(&self.images);
            if !st.copy.pending {
                return None;
            }
            st.copy.pending = false;
            st.copy.name.clone()
        };
        if name.as_slice() == LAST_GOOD_NAME.as_bytes() {
            return None; // flashed last_good itself
        }
        let path = ImagePath::new(&name, false)?;
        let src = self.fs.open(path.as_str(), OpenMode::Read)?;
        if self.fs_free() < src.size() as usize + COPY_RESERVE {
            drop(src);
            self.host.log(
                EventCode::StmFlashFailed,
                NO_VALVE,
                0,
                0,
                b"last_good: no space",
            );
            return None;
        }
        // delete_file refuses the .part of a running copy: running is set with the file created
        let mut st = lock(&self.images);
        let dst = self.fs.open(LAST_GOOD_PART, OpenMode::Write)?;
        st.copy.running = true;
        Some(CopyIo { src, dst, done: 0 })
    }

    /// Up to [`COPY_CHUNKS_PER_CALL`] chunks through `buf`; `None` when the copy ended.
    fn copy_chunks(&self, mut io: CopyIo<F::File>, buf: &mut [u8]) -> Option<CopyIo<F::File>> {
        for _ in 0..COPY_CHUNKS_PER_CALL {
            let n = io.src.read(buf);
            if n == 0 {
                let ok = io.done == io.src.size() && io.done > 0;
                self.stop_copy(io, ok);
                return None;
            }
            if io.dst.write(buf.get(..n).unwrap_or_default()) != n {
                self.stop_copy(io, false);
                return None;
            }
            io.done += n as u32;
        }
        Some(io)
    }

    /// Ends the copy: the part file replaces last_good when `ok`; the index follows the files.
    fn stop_copy(&self, io: CopyIo<F::File>, ok: bool) {
        let done = io.done;
        drop(io);
        let mut ok = ok;
        if ok {
            self.fs.remove(LAST_GOOD_PATH);
            ok = self.fs.rename(LAST_GOOD_PART, LAST_GOOD_PATH);
        }
        if !ok {
            self.fs.remove(LAST_GOOD_PART);
        }
        let mut st = lock(&self.images);
        if ok {
            st.put(LAST_GOOD_NAME.as_bytes(), done);
        } else if !self.fs.exists(LAST_GOOD_PATH) {
            // the old last_good was removed only on success; keep the index honest
            st.forget(LAST_GOOD_NAME.as_bytes());
        }
        st.copy.running = false;
    }

    /// Validates the first unscanned image in index order (reads the whole file), unless it is
    /// being uploaded; an image whose size changed meanwhile is scanned again later.
    pub(super) fn service_scan(&self) {
        let entry = {
            let st = lock(&self.images);
            let Some(e) = st.entries.iter().find(|e| !e.name.is_empty() && !e.scanned) else {
                return;
            };
            if st.uploading(&e.name) {
                return;
            }
            e.clone()
        };
        let mut img = FileImage::new(&self.fs, &self.gate);
        let mut info = ImageInfo::default();
        let check = if img.open(&entry.name) {
            validate_image(&mut img, 0, true, &mut info)
        } else {
            FlashError::ImageRead
        };
        let size = img.size();
        img.close();
        let mut st = lock(&self.images);
        let Some(e) = st.find(&entry.name).and_then(|i| st.entries.get_mut(i)) else {
            return;
        };
        if e.size != size {
            return; // replaced meanwhile: scan again later
        }
        e.scanned = true;
        e.check = check;
        e.crc = info.crc;
        e.version = info.version;
        e.hw_tag = info.hw_tag;
    }
}

/// A LittleFS image for the flasher and the validator (core [`FlashImage`]). The first bytes
/// the flasher holds for the sector-0 pass of D9 (`hold_low`, 16 KiB) live in a heap block of
/// the run, freed by `close`.
pub struct FileImage<F: Fs, G: HeapGate> {
    fs: F,
    gate: G,
    file: Option<F::File>,
    size: u32,
    pos: u32,
    low: Option<Vec<u8>>,
}

impl<F: Fs, G: HeapGate> FileImage<F, G> {
    /// A closed image over `fs`; `gate` grants the block of `hold_low`.
    pub fn new(fs: F, gate: G) -> Self {
        FileImage {
            fs,
            gate,
            file: None,
            size: 0,
            pos: 0,
            low: None,
        }
    }

    /// Opens `/stm/<name>.bin` (`name` a bare name, a C string); false when the path does not
    /// fit or the file cannot be opened.
    pub fn open(&mut self, name: &[u8]) -> bool {
        self.close();
        let Some(path) = ImagePath::new(c_str(name), false) else {
            return false;
        };
        let Some(f) = self.fs.open(path.as_str(), OpenMode::Read) else {
            return false;
        };
        self.size = f.size();
        self.pos = 0;
        self.file = Some(f);
        true
    }

    /// Closes the file; the size reads 0, the held bytes are freed.
    pub fn close(&mut self) {
        self.file = None;
        self.size = 0;
        self.pos = 0;
        self.low = None;
    }
}

impl<F: Fs, G: HeapGate> FlashImage for FileImage<F, G> {
    fn size(&self) -> u32 {
        self.size
    }

    /// Reads exactly `out.len()` bytes at `offset`; seeks only when the position moved.
    fn read(&mut self, offset: u32, out: &mut [u8]) -> bool {
        let Some(file) = self.file.as_mut() else {
            return false;
        };
        let len = out.len();
        if offset > self.size || len > (self.size - offset) as usize {
            return false;
        }
        if offset != self.pos {
            if !file.seek(offset) {
                return false;
            }
            self.pos = offset;
        }
        if file.read(out) != len {
            self.pos = u32::MAX; // position unknown: seek next time
            return false;
        }
        self.pos += len as u32;
        true
    }

    /// The bytes [0, len) into a heap block of the run (no memory or a read error: false, nothing
    /// held).
    fn hold_low(&mut self, len: u32) -> bool {
        self.low = None;
        let Some(mut block) = try_bytes(&self.gate, len as usize) else {
            return false;
        };
        if !self.read(0, &mut block) {
            return false;
        }
        self.low = Some(block);
        true
    }

    fn low(&self) -> &[u8] {
        self.low.as_deref().unwrap_or_default()
    }
}
