//! The Arduino-ESP32 2.0.7 `Update` contract (`Updater.cpp`) over the [`Ota`] port: the same
//! calls, return values, error codes and texts (event 107 and the HTTP detail show them), the
//! 4 KiB sector buffer, the magic byte check of the first sector, the MD5 over the written bytes
//! compared at the end, and `end(true)` for an image of unknown size.
//!
//! Only `U_FLASH` updates exist (the glue never used `U_SPIFFS`). Where Arduino wrote the flash
//! itself, this writes through the port (`esp_ota_write`, `esp_ota_end`, `esp_ota_set_boot`), so
//! the image is verified by ESP-IDF; docs/rust/PORT-NOTES.md lists the consequences.
//!
//! Intended deviation: an `end()` that fails never selects a slot. Arduino's `end(true)` ignored
//! a failed last sector and then booted whatever the slot held (an image left complete by an
//! earlier upload of the boot that failed its MD5 check); here only the image this update wrote
//! completely can be selected.

use crate::heap::try_bytes;
use crate::port::{EspErr, HeapGate, Md5, Ota, OtaUpdate};

/// No error.
pub const UPDATE_ERROR_OK: u8 = 0;
/// A flash write (or, through the port, an erase) failed.
pub const UPDATE_ERROR_WRITE: u8 = 1;
/// A flash erase failed (Arduino only: the port reports erase failures as writes).
pub const UPDATE_ERROR_ERASE: u8 = 2;
/// The image could not be enabled or read back.
pub const UPDATE_ERROR_READ: u8 = 3;
/// More bytes than the update may take.
pub const UPDATE_ERROR_SPACE: u8 = 4;
/// Size 0 or larger than the partition.
pub const UPDATE_ERROR_SIZE: u8 = 5;
/// Stream read timeout (`writeStream`; not used by the glue).
pub const UPDATE_ERROR_STREAM: u8 = 6;
/// The MD5 of the written bytes differs from the expected one.
pub const UPDATE_ERROR_MD5: u8 = 7;
/// The first byte of the image is not 0xE9.
pub const UPDATE_ERROR_MAGIC_BYTE: u8 = 8;
/// The image does not verify or cannot be selected for boot.
pub const UPDATE_ERROR_ACTIVATE: u8 = 9;
/// No update partition.
pub const UPDATE_ERROR_NO_PARTITION: u8 = 10;
/// Bad command (not used: only `U_FLASH` exists).
pub const UPDATE_ERROR_BAD_ARGUMENT: u8 = 11;
/// The update was aborted (also an `end(false)` before all bytes arrived).
pub const UPDATE_ERROR_ABORT: u8 = 12;

/// Size of an update whose length is not known up front (multipart uploads).
pub const UPDATE_SIZE_UNKNOWN: u32 = 0xFFFF_FFFF;

/// `SPI_FLASH_SEC_SIZE`: the sector buffer.
const SECTOR: usize = 4096;
/// `ESP_IMAGE_HEADER_MAGIC`.
const IMAGE_MAGIC: u8 = 0xE9;

/// The text of an error code (`_err2str` of `Updater.cpp`).
pub fn error_text(code: u8) -> &'static str {
    match code {
        UPDATE_ERROR_OK => "No Error",
        UPDATE_ERROR_WRITE => "Flash Write Failed",
        UPDATE_ERROR_ERASE => "Flash Erase Failed",
        UPDATE_ERROR_READ => "Flash Read Failed",
        UPDATE_ERROR_SPACE => "Not Enough Space",
        UPDATE_ERROR_SIZE => "Bad Size Given",
        UPDATE_ERROR_STREAM => "Stream Read Timeout",
        UPDATE_ERROR_MD5 => "MD5 Check Failed",
        UPDATE_ERROR_MAGIC_BYTE => "Wrong Magic Byte",
        UPDATE_ERROR_ACTIVATE => "Could Not Activate The Firmware",
        UPDATE_ERROR_NO_PARTITION => "Partition Could Not be Found",
        UPDATE_ERROR_BAD_ARGUMENT => "Bad Argument",
        UPDATE_ERROR_ABORT => "Aborted",
        _ => "UNKNOWN",
    }
}

fn hex_lower(d: &[u8; 16]) -> [u8; 32] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 32];
    for (i, b) in d.iter().enumerate() {
        out[2 * i] = HEX[usize::from(b >> 4)];
        out[2 * i + 1] = HEX[usize::from(b & 15)];
    }
    out
}

/// One OTA update at a time, like Arduino's global `Update`.
pub struct Update<O: Ota, M: Md5, G: HeapGate> {
    ota: O,
    md5: M,
    gate: G,
    error: u8,
    /// The sector buffer (Arduino `_buffer`): allocated by `begin`, freed by every reset.
    buffer: Option<Vec<u8>>,
    buffer_len: usize,
    /// Bytes the update takes (`_size`, 32-bit `size_t` on the ESP32); 0 = not running.
    size: u32,
    progress: u32,
    /// Expected MD5, lowercase hex as given (`_target_md5`); empty = none.
    target_md5: Option<[u8; 32]>,
    /// The port's update between `begin` and its end.
    update: Option<O::Update>,
}

impl<O: Ota, M: Md5, G: HeapGate> Update<O, M, G> {
    /// No update running, no error.
    pub fn new(ota: O, md5: M, gate: G) -> Self {
        Update {
            ota,
            md5,
            gate,
            error: UPDATE_ERROR_OK,
            buffer: None,
            buffer_len: 0,
            size: 0,
            progress: 0,
            target_md5: None,
            update: None,
        }
    }

    /// `_reset()`: buffer freed, progress and size 0; the port's update is abandoned.
    fn reset(&mut self) {
        self.buffer = None;
        self.buffer_len = 0;
        self.progress = 0;
        self.size = 0;
        if let Some(u) = self.update.take() {
            u.abort();
        }
    }

    /// `_abort(err)`.
    fn abort_with(&mut self, err: u8) {
        self.reset();
        self.error = err;
    }

    /// Starts an update of `size` bytes into the other app slot (`UPDATE_SIZE_UNKNOWN`: up to the
    /// partition size). False while one runs (the error stays), for size 0 or one larger than
    /// the partition (`UPDATE_ERROR_SIZE`), without an update partition
    /// (`UPDATE_ERROR_NO_PARTITION`) and without memory for the sector buffer (no error code, as
    /// Arduino's failed `malloc`). The port refusing the update counts as no partition, or as no
    /// memory for `ESP_ERR_NO_MEM`.
    pub fn begin(&mut self, size: u32) -> bool {
        if self.size > 0 {
            return false; // "already running"
        }
        self.reset();
        self.error = UPDATE_ERROR_OK;
        self.target_md5 = None;
        if size == 0 {
            self.error = UPDATE_ERROR_SIZE;
            return false;
        }
        let Some(part) = self.ota.other() else {
            self.error = UPDATE_ERROR_NO_PARTITION;
            return false;
        };
        let size = if size == UPDATE_SIZE_UNKNOWN {
            part.size
        } else if size > part.size {
            self.error = UPDATE_ERROR_SIZE;
            return false;
        } else {
            size
        };
        let Some(buffer) = try_bytes(&self.gate, SECTOR) else {
            return false;
        };
        match self.ota.begin() {
            Ok(u) => self.update = Some(u),
            Err(e) => {
                if e != EspErr::NO_MEM {
                    self.error = UPDATE_ERROR_NO_PARTITION;
                }
                return false;
            }
        }
        self.buffer = Some(buffer);
        self.size = size;
        self.md5.reset();
        true
    }

    /// Expects the MD5 `hex` (32 characters, compared as given: Arduino compares with its
    /// lowercase digest) at the end; false for another length (up to a NUL, as `strlen`).
    pub fn set_md5(&mut self, hex: &[u8]) -> bool {
        let n = hex.iter().position(|&b| b == 0).unwrap_or(hex.len());
        let Ok(md5) = <[u8; 32]>::try_from(&hex[..n]) else {
            return false;
        };
        self.target_md5 = Some(md5);
        true
    }

    /// `_writeBuffer()`: the sector buffer to the flash. The first sector of an image must
    /// start with 0xE9.
    fn write_buffer(&mut self) -> bool {
        let first = self.progress == 0;
        let (Some(buffer), Some(update)) = (self.buffer.as_ref(), self.update.as_mut()) else {
            return false;
        };
        let data = buffer.get(..self.buffer_len).unwrap_or(&[]);
        if first && data.first() != Some(&IMAGE_MAGIC) {
            self.abort_with(UPDATE_ERROR_MAGIC_BYTE);
            return false;
        }
        if update.write(data).is_err() {
            self.abort_with(UPDATE_ERROR_WRITE);
            return false;
        }
        self.md5.update(data);
        self.progress = self.progress.wrapping_add(self.buffer_len as u32);
        self.buffer_len = 0;
        true
    }

    /// Copies `data` into the sector buffer, which goes to the flash whenever it is full and
    /// when it holds the rest of the update. Returns the bytes taken: 0 after an error, when
    /// not running, or for more than [`Update::remaining`] (`UPDATE_ERROR_SPACE`); fewer than
    /// `data.len()` when a flash write failed (the update is aborted then).
    pub fn write(&mut self, data: &[u8]) -> usize {
        if self.has_error() || !self.is_running() {
            return 0;
        }
        let remaining = self.remaining();
        if u32::try_from(data.len()).map_or(true, |n| n > remaining) {
            self.abort_with(UPDATE_ERROR_SPACE);
            return 0;
        }
        let mut done = 0;
        while self.buffer_len + (data.len() - done) > SECTOR {
            let take = SECTOR - self.buffer_len;
            self.fill(&data[done..done + take]);
            if !self.write_buffer() {
                return done;
            }
            done += take;
        }
        self.fill(&data[done..]);
        if self.buffer_len as u32 == self.remaining() && !self.write_buffer() {
            return done;
        }
        data.len()
    }

    fn fill(&mut self, bytes: &[u8]) {
        if let Some(dst) = self
            .buffer
            .as_mut()
            .and_then(|b| b.get_mut(self.buffer_len..self.buffer_len + bytes.len()))
        {
            dst.copy_from_slice(bytes);
            self.buffer_len += bytes.len();
        }
    }

    /// Ends the update. False with an error before or when not running; `end(false)` before all
    /// bytes arrived aborts (`UPDATE_ERROR_ABORT`); `end(true)` takes what was written (the
    /// last partial sector is written, as Arduino did). Then the MD5 of the written bytes is
    /// compared (`UPDATE_ERROR_MD5`) and the image verified and selected for boot
    /// (`UPDATE_ERROR_ACTIVATE`). An update that leaves no image of its own (nothing written, or
    /// a last sector that failed and aborted it) ends with `UPDATE_ERROR_READ` ("Flash Read
    /// Failed", Arduino's code when it had no image to enable) and selects nothing (intended
    /// deviation: Arduino booted what the slot held then, see the module documentation).
    pub fn end(&mut self, even_if_remaining: bool) -> bool {
        if self.has_error() || self.size == 0 {
            return false;
        }
        if !self.is_finished() && !even_if_remaining {
            self.abort_with(UPDATE_ERROR_ABORT);
            return false;
        }
        if even_if_remaining {
            if self.buffer_len > 0 {
                self.write_buffer();
            }
            self.size = self.progress;
        }
        let digest = hex_lower(&self.md5.digest());
        if self.target_md5.is_some_and(|t| t != digest) {
            self.abort_with(UPDATE_ERROR_MD5);
            return false;
        }
        self.verify_end()
    }

    /// `_verifyEnd()`: this update's image is verified and selected. Without one (nothing
    /// written, or the last sector failed) nothing is selected: `UPDATE_ERROR_READ`.
    fn verify_end(&mut self) -> bool {
        let written = self.size > 0;
        let result = match self.update.take() {
            Some(u) if written => u.finish().map_err(|_| UPDATE_ERROR_ACTIVATE),
            unfinished => {
                if let Some(u) = unfinished {
                    u.abort();
                }
                Err(UPDATE_ERROR_READ)
            }
        };
        match result {
            Ok(()) => {
                self.reset();
                true
            }
            Err(e) => {
                self.abort_with(e);
                false
            }
        }
    }

    /// Aborts the update (`UPDATE_ERROR_ABORT`).
    pub fn abort(&mut self) {
        self.abort_with(UPDATE_ERROR_ABORT);
    }

    /// The last error code.
    pub fn error(&self) -> u8 {
        self.error
    }
    /// The text of the last error.
    pub fn error_string(&self) -> &'static str {
        error_text(self.error)
    }
    /// An error is set.
    pub fn has_error(&self) -> bool {
        self.error != UPDATE_ERROR_OK
    }
    /// Forgets the error.
    pub fn clear_error(&mut self) {
        self.error = UPDATE_ERROR_OK;
    }
    /// An update runs (`_size > 0`).
    pub fn is_running(&self) -> bool {
        self.size > 0
    }
    /// Every byte of the update went to the flash.
    pub fn is_finished(&self) -> bool {
        self.progress == self.size
    }
    /// Bytes the update takes.
    pub fn size(&self) -> u32 {
        self.size
    }
    /// Bytes written to the flash so far (without the sector buffer).
    pub fn progress(&self) -> u32 {
        self.progress
    }
    /// `size - progress` (32-bit, wrapping like Arduino's `size_t`).
    pub fn remaining(&self) -> u32 {
        self.size.wrapping_sub(self.progress)
    }
}

#[cfg(test)]
mod tests;
