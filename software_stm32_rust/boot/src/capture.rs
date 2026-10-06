//! Reset capture before the window (port of `sysstat_capture_reset`, `src/sysstat.cpp`): the
//! boot reason from RCC_CSR, the reset counter and the watchdog reset guard in the no-init
//! RAM cells of C++ 2.1.7, byte for byte.
//!
//! The cells keep the C++ 2.1.7 addresses (D3, docs/rust/GLUE-DESIGN-STM.md §3.2): one
//! region of 212 bytes at 0x20003234 that the start-up code never clears. The firmware copies
//! it with volatile word accesses; this module sees it as bytes with the C++ offsets and
//! layouts, so a C++ and a Rust image read each other's cells after a warm reset or a flash.
//! Fields are read and written one by one, as the C++ does: struct padding is never written.

use vdm_stm_core::reset_guard::{reset_guard_on_boot, ResetGuardCell};
use vdm_stm_core::system_stats::{classify_reset, count_reset, BootReason, ResetCounterCell, ResetFlags};

/// Bytes of the no-init region (C++ 2.1.7 `.noinit`: 0x20003234..0x20003308).
pub const NOINIT_LEN: usize = 0xD4;
/// Address of the region (the same in the four C++ release images, *measured*).
pub const NOINIT_ADDR: u32 = 0x2000_3234;
/// `warm_state` (`vdm::WarmState`, 178 bytes + 2 bytes padding), `app.cpp`.
pub const WARM_STATE_OFFSET: usize = 0;
pub const WARM_STATE_LEN: usize = 180;
/// `guard_cell` (`vdm::ResetGuardCell`, 18 bytes + 2 bytes padding), `sysstat.cpp`.
pub const GUARD_CELL_OFFSET: usize = 0xB4;
pub const GUARD_CELL_LEN: usize = 20;
/// `reset_cell` (`vdm::ResetCounterCell`, 12 bytes), `sysstat.cpp`.
pub const RESET_CELL_OFFSET: usize = 0xC8;
pub const RESET_CELL_LEN: usize = 12;

/// RCC_CSR reset flags (RM0368 / RM0383 §6.3.21).
pub const CSR_BORRSTF: u32 = 1 << 25;
pub const CSR_PINRSTF: u32 = 1 << 26;
pub const CSR_PORRSTF: u32 = 1 << 27;
pub const CSR_SFTRSTF: u32 = 1 << 28;
pub const CSR_IWDGRSTF: u32 = 1 << 29;
pub const CSR_WWDGRSTF: u32 = 1 << 30;
pub const CSR_LPWRRSTF: u32 = 1 << 31;

/// What the capture found: the C++ `boot_reason`, `reset_count` and `safe_mode` statics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResetInfo {
    pub reason: BootReason,
    /// resets since the last power-on
    pub resets: u32,
    /// three watchdog resets within 10 min: no valve moves until it is left
    pub safe_mode: bool,
}

/// The flags of an RCC_CSR value (`sysstat_capture_reset`).
pub fn reset_flags(csr: u32) -> ResetFlags {
    ResetFlags {
        low_power: csr & CSR_LPWRRSTF != 0,
        window_watchdog: csr & CSR_WWDGRSTF != 0,
        independent_watchdog: csr & CSR_IWDGRSTF != 0,
        software: csr & CSR_SFTRSTF != 0,
        power_on: csr & CSR_PORRSTF != 0,
        pin: csr & CSR_PINRSTF != 0,
        brown_out: csr & CSR_BORRSTF != 0,
    }
}

/// `sysstat_capture_reset` without the register accesses: classifies `csr`, counts the reset
/// in the counter cell and runs the reset guard on the guard cell (core `classify_reset`,
/// `count_reset`, `reset_guard_on_boot`, in the C++ order). The warm state is not touched.
pub fn capture_reset(csr: u32, cells: &mut [u8; NOINIT_LEN]) -> ResetInfo {
    let reason = classify_reset(&reset_flags(csr));
    let mut counter = read_counter(cells);
    let resets = count_reset(&mut counter, reason);
    write_counter(cells, &counter);
    let mut guard = read_guard(cells);
    let safe_mode = reset_guard_on_boot(&mut guard, reason);
    write_guard(cells, &guard);
    ResetInfo {
        reason,
        resets,
        safe_mode,
    }
}

/// The reset counter cell of the region.
pub fn read_counter(cells: &[u8; NOINIT_LEN]) -> ResetCounterCell {
    let at = RESET_CELL_OFFSET;
    ResetCounterCell {
        magic: le32(cells, at),
        count: le32(cells, at.wrapping_add(4)),
        check: le32(cells, at.wrapping_add(8)),
    }
}

pub fn write_counter(cells: &mut [u8; NOINIT_LEN], c: &ResetCounterCell) {
    let at = RESET_CELL_OFFSET;
    put32(cells, at, c.magic);
    put32(cells, at.wrapping_add(4), c.count);
    put32(cells, at.wrapping_add(8), c.check);
}

/// The reset guard cell of the region (offsets of `vdm::ResetGuardCell`).
pub fn read_guard(cells: &[u8; NOINIT_LEN]) -> ResetGuardCell {
    let at = GUARD_CELL_OFFSET;
    ResetGuardCell {
        magic: le32(cells, at),
        count: byte(cells, at.wrapping_add(4)),
        safe: byte(cells, at.wrapping_add(5)),
        pad: le16(cells, at.wrapping_add(6)),
        window_s: le32(cells, at.wrapping_add(8)),
        last_uptime_s: le32(cells, at.wrapping_add(12)),
        crc: le16(cells, at.wrapping_add(16)),
    }
}

/// Writes the fields; the 2 bytes of tail padding (offsets 18, 19) keep their content.
pub fn write_guard(cells: &mut [u8; NOINIT_LEN], c: &ResetGuardCell) {
    let at = GUARD_CELL_OFFSET;
    put32(cells, at, c.magic);
    put8(cells, at.wrapping_add(4), c.count);
    put8(cells, at.wrapping_add(5), c.safe);
    put16(cells, at.wrapping_add(6), c.pad);
    put32(cells, at.wrapping_add(8), c.window_s);
    put32(cells, at.wrapping_add(12), c.last_uptime_s);
    put16(cells, at.wrapping_add(16), c.crc);
}

fn byte(b: &[u8], at: usize) -> u8 {
    b.get(at).copied().unwrap_or(0)
}

fn le16(b: &[u8], at: usize) -> u16 {
    b.get(at..)
        .and_then(|s| s.first_chunk::<2>())
        .map_or(0, |c| u16::from_le_bytes(*c))
}

fn le32(b: &[u8], at: usize) -> u32 {
    b.get(at..)
        .and_then(|s| s.first_chunk::<4>())
        .map_or(0, |c| u32::from_le_bytes(*c))
}

fn put8(b: &mut [u8], at: usize, v: u8) {
    if let Some(slot) = b.get_mut(at) {
        *slot = v;
    }
}

fn put16(b: &mut [u8], at: usize, v: u16) {
    if let Some(c) = b.get_mut(at..).and_then(|s| s.first_chunk_mut::<2>()) {
        *c = v.to_le_bytes();
    }
}

fn put32(b: &mut [u8], at: usize, v: u32) {
    if let Some(c) = b.get_mut(at..).and_then(|s| s.first_chunk_mut::<4>()) {
        *c = v.to_le_bytes();
    }
}

#[cfg(test)]
mod tests;
