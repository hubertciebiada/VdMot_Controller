//! Health figures reported by gstat: uptime, reset cause and reset counter.
//! Hardware-free.

/// Seconds since start from a wrapping millisecond clock (millis() wraps
/// after 49.7 days). update() must be called at least once per wrap period.
#[derive(Clone, Copy, Debug, Default)]
pub struct UptimeCounter {
    last_ms: u32,
    remainder_ms: u32,
    seconds: u32,
}

impl UptimeCounter {
    pub fn update(&mut self, now_ms: u32) {
        // unsigned difference stays right across the wrap of the millisecond clock
        let elapsed = now_ms.wrapping_sub(self.last_ms);
        self.last_ms = now_ms;
        let total = u64::from(self.remainder_ms) + u64::from(elapsed);
        self.seconds = self.seconds.wrapping_add((total / 1000) as u32);
        self.remainder_ms = (total % 1000) as u32;
    }

    pub fn seconds(&self) -> u32 {
        self.seconds
    }
}

/// Wire values of gstat bootReason.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum BootReason {
    #[default]
    Unknown = 0,
    PowerOn = 1,
    /// NRST (e.g. reset by the ESP)
    Pin = 2,
    /// soft reset (reset command, after flashing)
    Software = 3,
    IndependentWatchdog = 4,
    WindowWatchdog = 5,
    LowPower = 6,
    BrownOut = 7,
}

/// Reset flags as found in RCC->CSR of the STM32F4.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResetFlags {
    pub low_power: bool,
    pub window_watchdog: bool,
    pub independent_watchdog: bool,
    pub software: bool,
    pub power_on: bool,
    pub pin: bool,
    pub brown_out: bool,
}

/// A power-on also sets the pin and brown-out flags, a software or watchdog
/// reset also sets the pin flag, so the most specific flag wins.
pub fn classify_reset(f: &ResetFlags) -> BootReason {
    if f.independent_watchdog {
        BootReason::IndependentWatchdog
    } else if f.window_watchdog {
        BootReason::WindowWatchdog
    } else if f.low_power {
        BootReason::LowPower
    } else if f.software {
        BootReason::Software
    } else if f.power_on {
        BootReason::PowerOn
    } else if f.brown_out {
        BootReason::BrownOut
    } else if f.pin {
        BootReason::Pin
    } else {
        BootReason::Unknown
    }
}

/// Resets since the last power-on, kept in RAM that the start-up code does not
/// clear. The magic word tells a warm start from random RAM content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct ResetCounterCell {
    pub magic: u32,
    pub count: u32,
    /// !count
    pub check: u32,
}

/// "VDRM"
pub const RESET_COUNTER_MAGIC: u32 = 0x5644_524D;

/// Called once per start. Starts again at 0 after a power-on or brown-out and
/// when the cell does not hold a valid value; otherwise counts this reset.
/// Returns the new count.
pub fn count_reset(cell: &mut ResetCounterCell, reason: BootReason) -> u32 {
    let cold_start = reason == BootReason::PowerOn || reason == BootReason::BrownOut;
    let valid = cell.magic == RESET_COUNTER_MAGIC && cell.check == !cell.count;

    if cold_start || !valid {
        cell.count = 0;
    } else {
        cell.count = cell.count.saturating_add(1);
    }
    cell.magic = RESET_COUNTER_MAGIC;
    cell.check = !cell.count;
    cell.count
}
