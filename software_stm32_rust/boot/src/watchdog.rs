//! The independent watchdog (docs/rust/GLUE-DESIGN-STM.md §5.4): `IWatchdog.begin(8000000)` of
//! the C++ on the registers. The boot stage starts it at the end of a window without handshake
//! (B6: never before the ROM bootloader), the application stage sets it up once more before
//! `embassy_stm32::init`.

use crate::io::IwdgIo;
use crate::poll::spin_until;

/// IWDG_KR: unlocks PR and RLR.
pub const KEY_ENABLE: u16 = 0x5555;
/// IWDG_KR: reloads the counter.
pub const KEY_RELOAD: u16 = 0xAAAA;
/// IWDG_KR: starts the watchdog (a running one keeps running).
pub const KEY_START: u16 = 0xCCCC;
/// IWDG_PR: prescaler /64.
pub const PR_DIV64: u32 = 4;
/// IWDG_RLR: 3999, with /64 at 32 kHz 8 s (5.4-15 s with the LSI spread).
pub const RLR_8S: u32 = 3999;

/// `IWatchdog.begin(8000000)`: start, unlock, /64, reload value 3999, then the wait (bounded)
/// until both values are in the LSI domain, then a reload. A second call changes nothing: the
/// start key leaves a running IWDG running, PR and RLR get the same values, the reload
/// restarts the 8 s.
pub fn start<W: IwdgIo>(io: &mut W) {
    io.iwdg_key(KEY_START);
    io.iwdg_key(KEY_ENABLE);
    io.iwdg_prescaler(PR_DIV64);
    io.iwdg_reload_value(RLR_8S);
    spin_until(|| !io.iwdg_updating());
    io.iwdg_key(KEY_RELOAD);
}

#[cfg(test)]
mod tests;
