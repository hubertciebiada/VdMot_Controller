//! Safe mode after a loop of watchdog resets: 3 watchdog resets within 10 min
//! stop every valve movement until the safe mode is left (ssafe 0, 30 min of
//! uptime, power-on). The cell lives in RAM the start-up code does not clear
//! (.noinit, src/sysstat.cpp). Hardware-free.

use crate::legacy_layout::crc16_ccitt;
use crate::system_stats::BootReason;

pub const SAFE_MODE_RESETS: u8 = 3;
pub const SAFE_MODE_WINDOW_S: u32 = 600;
pub const SAFE_MODE_EXIT_S: u32 = 1800;
/// "VDRG"
pub const RESET_GUARD_MAGIC: u32 = 0x5644_5247;

/// The C++ layout (`#[repr(C)]`, 20 bytes); the CRC covers the 16 bytes before `crc` in
/// the little-endian byte order of the STM32, so a C++ and a Rust firmware read each
/// other's cell after a warm reset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct ResetGuardCell {
    pub magic: u32,
    /// watchdog resets in the current window
    pub count: u8,
    /// 0/1
    pub safe: u8,
    pub pad: u16,
    /// uptime summed over the boots since the first watchdog reset of the window
    pub window_s: u32,
    /// uptime of this boot, updated every second
    pub last_uptime_s: u32,
    /// CRC-16/CCITT-FALSE over the bytes before it
    pub crc: u16,
}

/// The bytes the CRC covers (offsetof(ResetGuardCell, crc) == 16).
fn guard_bytes(c: &ResetGuardCell) -> [u8; 16] {
    let mut b = [0u8; 16];
    b[0..4].copy_from_slice(&c.magic.to_le_bytes());
    b[4] = c.count;
    b[5] = c.safe;
    b[6..8].copy_from_slice(&c.pad.to_le_bytes());
    b[8..12].copy_from_slice(&c.window_s.to_le_bytes());
    b[12..16].copy_from_slice(&c.last_uptime_s.to_le_bytes());
    b
}

fn guard_crc(c: &ResetGuardCell) -> u16 {
    crc16_ccitt(&guard_bytes(c))
}

fn seal(c: &mut ResetGuardCell) {
    c.magic = RESET_GUARD_MAGIC;
    c.pad = 0;
    c.crc = guard_crc(c);
}

fn clear_window(c: &mut ResetGuardCell) {
    c.count = 0;
    c.safe = 0;
    c.window_s = 0;
}

fn is_watchdog(r: BootReason) -> bool {
    matches!(
        r,
        BootReason::IndependentWatchdog | BootReason::WindowWatchdog
    )
}

fn is_cold(r: BootReason) -> bool {
    matches!(
        r,
        BootReason::PowerOn | BootReason::BrownOut | BootReason::Unknown
    )
}

/// Once per start after classify_reset(). Cold (PowerOn, BrownOut, Unknown) or
/// an invalid cell -> all 0. Warm: window_s += last_uptime_s (saturating); a
/// watchdog reason (IndependentWatchdog, WindowWatchdog) starts a new window
/// (count 1, window_s 0) when count == 0 or window_s > SAFE_MODE_WINDOW_S, else
/// count + 1; safe |= count >= SAFE_MODE_RESETS. Returns safe.
pub fn reset_guard_on_boot(c: &mut ResetGuardCell, reason: BootReason) -> bool {
    if is_cold(reason) || c.magic != RESET_GUARD_MAGIC || c.crc != guard_crc(c) {
        clear_window(c);
    } else {
        c.window_s = c.window_s.saturating_add(c.last_uptime_s);
        if is_watchdog(reason) {
            if c.count == 0 || c.window_s > SAFE_MODE_WINDOW_S {
                c.count = 1;
                c.window_s = 0;
            } else {
                c.count = c.count.saturating_add(1);
            }
        }
        if c.count >= SAFE_MODE_RESETS {
            c.safe = 1;
        }
    }
    c.last_uptime_s = 0;
    seal(c);
    c.safe != 0
}

/// Every second: last_uptime_s = uptime_s; safe && uptime_s >= SAFE_MODE_EXIT_S
/// leaves safe mode (count 0, window_s 0). Returns safe.
pub fn reset_guard_alive(c: &mut ResetGuardCell, uptime_s: u32) -> bool {
    c.last_uptime_s = uptime_s;
    if c.safe != 0 && uptime_s >= SAFE_MODE_EXIT_S {
        clear_window(c);
    }
    seal(c);
    c.safe != 0
}

/// ssafe 0: safe mode off, count 0, window_s 0
pub fn reset_guard_clear(c: &mut ResetGuardCell) {
    clear_window(c);
    seal(c);
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
