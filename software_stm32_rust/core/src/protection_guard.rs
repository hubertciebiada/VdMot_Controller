//! Common-mode guard of the short and inrush limits: when the presence-test
//! short check or the inrush limit trips on several different valves within a
//! short time, the limits are more likely wrong for this installation (supply,
//! actuator type) than all the valves shorted. Then both limits stay off until
//! the next start. Hardware-free; time in uptime seconds.

use crate::legacy_layout::VALVE_COUNT;

/// different valves ...
pub const PROTECT_TRIP_VALVES: u8 = 3;
/// ... with a trip less than this apart
pub const PROTECT_WINDOW_S: u32 = 600;

/// Build switch of the short and inrush limits: false = report only (the trip is
/// detected and reported as fault 3 / 5, but the status of the valve does not
/// change and the move or test goes on as without the limit). Set to true only
/// after the limits were measured on the hardware (spec-stm C-4).
/// (C++ `vdm::kProtectEnforce`, a constexpr in protection_guard.h, not a -D flag.)
pub const PROTECT_ENFORCE: bool = false;

#[derive(Clone, Copy, Debug, Default)]
pub struct ProtectionGuard {
    last_trip_s: [u32; VALVE_COUNT as usize],
    /// bit v: last_trip_s\[v\] is valid
    tripped: u16,
    suspended: bool,
}

impl ProtectionGuard {
    /// A short verdict or an inrush trip of the valve. Returns suspended().
    pub fn on_trip(&mut self, valve: u8, uptime_s: u32) -> bool {
        let Some(slot) = self.last_trip_s.get_mut(usize::from(valve)) else {
            return self.suspended;
        };
        *slot = uptime_s;
        self.tripped |= 1 << valve;
        let mut recent = 0u8;
        for (v, &last) in self.last_trip_s.iter().enumerate() {
            if self.tripped & (1 << v) != 0 && uptime_s.wrapping_sub(last) < PROTECT_WINDOW_S {
                recent += 1;
            }
        }
        if recent >= PROTECT_TRIP_VALVES {
            self.suspended = true;
        }
        self.suspended
    }

    pub fn suspended(&self) -> bool {
        self.suspended
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
