//! The boot stage from reset to the application or the ROM bootloader
//! (docs/rust/GLUE-DESIGN-STM.md §5.2), over the register traits of [`crate::io`]:
//!
//! 1. capture: RCC_CSR, RMVF, reset counter and guard in the no-init cells
//! 2. valve outputs safe (`valve_pins_safe`)
//! 3. boot clock (D1): HSEON, HSERDY within 5 ms -> SYSCLK = HSE 25 MHz without PLL, else
//!    HSE off and HSI 16 MHz
//! 4. `BootSetup`: LED on, USART1 115200 8E1, 10 ms, drop
//! 5. the window ([`crate::window`]), meanwhile the check of the application part
//!    ([`crate::app_check`])
//! 6. `BEEFIT` sent -> [`BootEnd::Update`] (the firmware jumps); timeout with a matching
//!    application part -> USART1 and SysTick back to their reset state, the IWDG started,
//!    [`BootEnd::Timeout`] with the [`BootToken`]; without a match the next window, for ever
//!
//! B2: nothing here waits without a bound or needs interrupts; every wait counts SysTick
//! periods. Only the windows repeat without end, and only when sectors 1..n do not belong to
//! this sector 0 (B8, D9): then the boot stage is the one safe place, and its window the way
//! out. B6: the IWDG starts only at the end of a window without handshake, never before the ROM
//! bootloader (a running IWDG would reset its session). It starts here, in sector 0, and not in
//! the application stage, so that whatever runs after the window is watched, also code the
//! check of the application part would not have expected.

use crate::app_check::AppCheck;
use crate::capture::{capture_reset, ResetInfo};
use crate::io::{BootHw, ClockIo};
use crate::window::{window_with, WindowEnd};

/// HSI after reset.
pub const HSI_HZ: u32 = 16_000_000;
/// The BlackPill crystal.
pub const HSE_HZ: u32 = 25_000_000;
/// The boot window baud rate (8E1).
pub const BAUD: u32 = 115_200;
/// SysTick reload for 1 ms on HSI / on HSE.
pub const HSI_TICK_RELOAD: u32 = HSI_HZ / 1000 - 1;
pub const HSE_TICK_RELOAD: u32 = HSE_HZ / 1000 - 1;
/// USART1 BRR (oversampling 16) at 115200 Bd: 0x8B on HSI (115,108 Bd), 0xD9 on HSE
/// (115,207 Bd).
pub const BRR_HSI: u16 = 0x8B;
pub const BRR_HSE: u16 = 0xD9;
/// D1: the boot window runs on the HSE only when it is ready within 5 ms.
pub const BOOT_HSE_LIMIT_MS: u32 = 5;
/// The application stage waits as long as `HSE_STARTUP_TIMEOUT` of the C++ (100 ms).
pub const APP_HSE_LIMIT_MS: u32 = 100;

/// Proof that the boot stage ran and the window ended without a handshake; the application
/// stage needs it (B1), and only [`run`] creates one.
#[derive(Debug, PartialEq, Eq)]
pub struct BootToken {
    reset: ResetInfo,
    boot_ms: u32,
    hse: bool,
}

impl BootToken {
    /// The reset capture of this start.
    pub fn reset(&self) -> ResetInfo {
        self.reset
    }

    /// Milliseconds the boot stage took (HSE probe and window): the application adds them to
    /// its clock, as the C++ `millis()` counts from `HAL_Init`.
    pub fn boot_ms(&self) -> u32 {
        self.boot_ms
    }

    /// The window ran on the HSE.
    pub fn hse(&self) -> bool {
        self.hse
    }
}

/// How the boot stage ended.
#[derive(Debug, PartialEq, Eq)]
pub enum BootEnd {
    /// `BEEFIT` was sent: jump into the ROM bootloader.
    Update,
    /// No handshake: start the application.
    Timeout(BootToken),
}

/// HSEON, then up to `limit_ms` SysTick periods (SysTick running) for HSERDY; without it the
/// HSE is switched off again. `elapsed` counts the periods.
pub fn probe_hse<C: ClockIo>(io: &mut C, limit_ms: u32, elapsed: &mut u32) -> bool {
    io.hse_on();
    let mut waited = 0u32;
    loop {
        if io.hse_ready() {
            return true;
        }
        if waited >= limit_ms {
            io.hse_off();
            return false;
        }
        if io.tick() {
            waited = waited.saturating_add(1);
            *elapsed = elapsed.wrapping_add(1);
        }
    }
}

/// The HSE probe of the application stage before the PLL set-up (embassy-stm32 0.6.0 waits for
/// HSERDY without a bound, so it only gets an HSE that is ready). `boot_hse`: the boot stage
/// runs on the HSE already. SysTick is off afterwards.
pub fn probe_app_hse<C: ClockIo>(io: &mut C, boot_hse: bool) -> bool {
    io.tick_start(if boot_hse {
        HSE_TICK_RELOAD
    } else {
        HSI_TICK_RELOAD
    });
    let mut elapsed = 0u32;
    let hse = probe_hse(io, APP_HSE_LIMIT_MS, &mut elapsed);
    io.tick_stop();
    hse
}

/// Steps 1 to 6 of the boot stage.
pub fn run<H: BootHw>(hw: &mut H) -> BootEnd {
    // 1. capture, before anything else reads the flags or the cells
    let csr = hw.reset_flags();
    hw.clear_reset_flags();
    let mut cells = hw.noinit();
    let reset = capture_reset(csr, &mut cells);
    hw.set_noinit(&cells);

    // 2. valve outputs safe
    hw.outputs_safe();

    // 3. boot clock (D1)
    let mut boot_ms = 0u32;
    hw.tick_start(HSI_TICK_RELOAD);
    let hse = probe_hse(hw, BOOT_HSE_LIMIT_MS, &mut boot_ms);
    let brr = if hse {
        hw.sysclk_hse();
        hw.tick_start(HSE_TICK_RELOAD);
        BRR_HSE
    } else {
        BRR_HSI
    };

    // 4, 5. BootSetup and the window, with the pattern and the reply from the ID block; the
    // check of the application part (D9) runs in the idle time of the window
    let id = hw.boot_id();
    let mut check = AppCheck::new(hw.app_record());
    loop {
        let end = window_with(hw, &id, brr, &mut boot_ms, &mut |io: &mut H| check.step(io));
        match end {
            WindowEnd::Update => return BootEnd::Update,
            WindowEnd::Timeout => {
                if check.finish(hw) {
                    break;
                }
                // sectors 1..n are not the image of this sector 0 (a flash that failed or
                // stopped in their pass): no application, the next window (B6: no IWDG, the
                // ESP may flash at any time)
            }
        }
    }
    // 6b. the application stage sets USART1 up again at 8N1 and its own clocks
    hw.uart_end();
    hw.tick_stop();
    // last, right before the application: whatever runs from here on is watched (B6: the
    // window is over)
    hw.watchdog_start();
    BootEnd::Timeout(BootToken {
        reset,
        boot_ms,
        hse,
    })
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_esp;
