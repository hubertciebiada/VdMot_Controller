//! Health figures of gstat in the main loop (`software_stm32/src/sysstat.cpp`,
//! `include/sysstat.h`): the uptime across the wrap of millis() and the safe mode after a loop
//! of watchdog resets. The reset capture (`sysstat_capture_reset`) runs in the boot stage
//! (vdm-stm-boot `capture`), which hands its result to [`Sysstat::new`].

use crate::hal::Clock;
use vdm_stm_core::reset_guard::{reset_guard_alive, reset_guard_clear, ResetGuardCell};
use vdm_stm_core::system_stats::{BootReason, UptimeCounter};

/// The no-init cell of the reset guard (C++ `guard_cell`, `.noinit`): written back whenever the
/// module changes it, so the next boot finds it (the firmware writes it into the no-init region
/// with the layout of vdm-stm-boot `capture`).
pub trait SysstatEnv {
    fn store_guard_cell(&mut self, cell: &ResetGuardCell);
}

/// The module state (the C++ statics of sysstat.cpp after the capture).
#[derive(Clone, Copy, Debug, Default)]
pub struct Sysstat {
    uptime: UptimeCounter,
    boot_reason: BootReason,
    reset_count: u32,
    safe_mode: bool,
    guard_cell: ResetGuardCell,
}

impl Sysstat {
    /// The result of the capture: the boot reason, the resets since the last power-on, the safe
    /// mode and the guard cell as the capture left it.
    pub fn new(
        boot_reason: BootReason,
        resets: u32,
        safe_mode: bool,
        guard_cell: ResetGuardCell,
    ) -> Self {
        Sysstat {
            uptime: UptimeCounter::default(),
            boot_reason,
            reset_count: resets,
            safe_mode,
            guard_cell,
        }
    }

    /// `sysstat_loop`: call from the main loop at least once per 49 days. Once per second the
    /// uptime of this boot goes into the guard cell (reset window, end of the safe mode).
    pub fn loop_(&mut self, clock: &impl Clock, env: &mut impl SysstatEnv) {
        let last = self.uptime.seconds();
        self.uptime.update(clock.millis());
        if self.uptime.seconds() != last {
            self.safe_mode = reset_guard_alive(&mut self.guard_cell, self.uptime.seconds());
            env.store_guard_cell(&self.guard_cell);
        }
    }

    /// seconds since the start of the application (does not wrap after 49 days)
    pub fn uptime_s(&self) -> u32 {
        self.uptime.seconds()
    }

    /// resets since the last power-on
    pub fn resets(&self) -> u32 {
        self.reset_count
    }

    pub fn boot_reason(&self) -> BootReason {
        self.boot_reason
    }

    /// safe mode after a loop of watchdog resets: no valve moves until it is left
    pub fn safe_mode(&self) -> bool {
        self.safe_mode
    }

    /// watchdog resets in the current window
    pub fn wdg_resets(&self) -> u8 {
        self.guard_cell.count
    }

    /// `sysstat_leave_safe_mode` (ssafe 0): safe mode off, the window cleared
    pub fn leave_safe_mode(&mut self, env: &mut impl SysstatEnv) {
        reset_guard_clear(&mut self.guard_cell);
        env.store_guard_cell(&self.guard_cell);
        self.safe_mode = false;
    }
}

#[cfg(test)]
mod tests;
