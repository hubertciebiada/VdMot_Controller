//! The check of the application part before the boot stage starts it (B8, D9, docs/rust/
//! GLUE-DESIGN-STM.md §5.10, §5.11). The ESP flasher writes sector 0 in a pass of its own
//! (before sectors 1..n for an image with this record, after them otherwise), so a flash that
//! fails or stops in the pass of sectors 1..n leaves a whole sector 0 (vector table, ID block,
//! boot stage) in front of erased, partly written or foreign sectors. The boot window still
//! works there, but the boot stage must not call into that code: sector 0 holds a record of the
//! application part it was linked with (magic, first address, length, CRC-32), written after
//! the link (`vdm-stm-image-check patch`), and the boot stage compares it with the flash.
//! Without a match it opens the next window instead of the application, for ever: no IWDG
//! (B6), so the ESP can flash at any time.
//!
//! The CRC runs in small steps in the idle time of the window's polling loop, so it adds no
//! time to the start of the application and never delays the receiver by more than a step.
//!
//! The record (design §5.10, the format the ESP flasher reads): 16 bytes at image offset 0x240
//! ([`RECORD_ADDR`]), four little-endian words: [`APP_CHECK_MAGIC`], [`APP_START`], the length L
//! of the application part in bytes (the image size minus 0x4000, 0 for an image of at most
//! 16 KiB) and the CRC-32/ISO-HDLC ([`crc32`]) of the image bytes 0x4000..0x4000 + L.

use crate::io::FlashRead;

/// The record in sector 0: behind the ID block (0x08000200, at most 64 bytes), in front of the
/// boot stage.
pub const RECORD_ADDR: u32 = 0x0800_0240;
/// "VDAC", little endian: a record written by the patch step.
pub const APP_CHECK_MAGIC: u32 = u32::from_le_bytes(*b"VDAC");
/// The application part starts with sector 1.
pub const APP_START: u32 = 0x0800_4000;
/// D12: images up to 128 KiB, so at most 112 KiB above sector 0.
pub const APP_MAX_LEN: u32 = 0x1_C000;
/// Words of the record in sector 0: magic, first address, length in bytes, CRC-32.
pub const RECORD_WORDS: usize = 4;
/// Bytes of one step: 8 x 40-60 cycles, about 30 us on the 16 MHz HSI, while a byte at
/// 115200 Bd takes 95 us (the USART holds one). 112 KiB take about 0.5 s of the 3 s window.
pub const STEP_BYTES: usize = 8;

/// CRC-32/ISO-HDLC (zlib `crc32`): polynomial 0x04C11DB7 reflected (0xEDB88320), input and
/// output reflected, init 0xFFFFFFFF, final XOR 0xFFFFFFFF; check value of "123456789":
/// 0xCBF43926.
const POLY: u32 = 0xEDB8_8320;

/// Feeds `bytes` into the CRC register `reg` (start with `!0`, the CRC is `!reg`). Bit by bit:
/// a table would be flash data outside the boot stage (D9).
pub fn crc32_update(reg: u32, bytes: &[u8]) -> u32 {
    let mut r = reg;
    for &b in bytes {
        r ^= u32::from(b);
        for _ in 0..8 {
            let mask = (r & 1).wrapping_neg();
            r = r.wrapping_shr(1) ^ (POLY & mask);
        }
    }
    r
}

/// The CRC-32 of `bytes` (zlib `crc32`).
pub fn crc32(bytes: &[u8]) -> u32 {
    !crc32_update(!0, bytes)
}

/// The record of an application part (`patch` writes it, the image check reads it back).
pub fn record(app: &[u8]) -> [u32; RECORD_WORDS] {
    let len = u32::try_from(app.len()).unwrap_or(u32::MAX);
    [APP_CHECK_MAGIC, APP_START, len, crc32(app)]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Progress {
    Running,
    Done(bool),
}

/// The check of one boot: [`AppCheck::step`] from the polling loop of the window,
/// [`AppCheck::finish`] when the window ends without a handshake.
#[derive(Clone, Copy, Debug)]
pub struct AppCheck {
    next: u32,
    end: u32,
    reg: u32,
    expected: u32,
    progress: Progress,
}

impl AppCheck {
    /// The check of the record in sector 0. A record without the magic (an image that was not
    /// patched), with another start or longer than [`APP_MAX_LEN`] fails at once.
    pub fn new(record: [u32; RECORD_WORDS]) -> AppCheck {
        let [magic, start, len, crc] = record;
        let valid = magic == APP_CHECK_MAGIC && start == APP_START && len <= APP_MAX_LEN;
        AppCheck {
            next: APP_START,
            end: APP_START.wrapping_add(if valid { len } else { 0 }),
            reg: !0,
            expected: crc,
            progress: if valid {
                Progress::Running
            } else {
                Progress::Done(false)
            },
        }
    }

    /// The CRC over the next [`STEP_BYTES`] bytes (fewer at the end); the step after the last
    /// byte compares. Nothing once the result is known.
    pub fn step<F: FlashRead>(&mut self, flash: &mut F) {
        if self.progress != Progress::Running {
            return;
        }
        if self.next >= self.end {
            self.progress = Progress::Done(!self.reg == self.expected);
            return;
        }
        let mut buf = [0u8; STEP_BYTES];
        let left = self.end.wrapping_sub(self.next);
        let n = usize::try_from(left).map_or(STEP_BYTES, |l| l.min(STEP_BYTES));
        if let Some(chunk) = buf.get_mut(..n) {
            flash.flash_read(self.next, chunk);
            self.reg = crc32_update(self.reg, chunk);
        }
        // n is 1..=STEP_BYTES
        self.next = self.next.wrapping_add(n as u32);
    }

    /// The steps the window did not take, then the result: true when the application part
    /// matches the record.
    pub fn finish<F: FlashRead>(&mut self, flash: &mut F) -> bool {
        loop {
            if let Progress::Done(ok) = self.progress {
                return ok;
            }
            self.step(flash);
        }
    }
}

#[cfg(test)]
mod tests;
