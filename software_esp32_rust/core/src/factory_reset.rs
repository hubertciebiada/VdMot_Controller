//! Factory reset by the GPIO2 jumper: held LOW for the hold time at boot, once per fitting of
//! the jumper (a latch in NVS); port of `vdm/factory_reset.h`. Hardware-free.

use crate::common::elapsed_ms;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactoryPinDecision {
    Idle,
    Reset,
    KeepLatched,
    ClearLatch,
}

/// `pin_low`: first sample at boot; `held`: LOW for the whole hold time (measured only when
/// pin_low && !latched); `latched`: a pin reset happened and the pin was not seen HIGH since.
///  - !pin_low -> latched ? ClearLatch : Idle
///  - pin_low && latched -> KeepLatched (no wait)
///  - pin_low && !latched -> held ? Reset : Idle
pub fn factory_pin_at_boot(pin_low: bool, held: bool, latched: bool) -> FactoryPinDecision {
    if !pin_low {
        return if latched {
            FactoryPinDecision::ClearLatch
        } else {
            FactoryPinDecision::Idle
        };
    }
    if latched {
        return FactoryPinDecision::KeepLatched;
    }
    if held {
        FactoryPinDecision::Reset
    } else {
        FactoryPinDecision::Idle
    }
}

/// Run time (every second): true when the latch is to be cleared (latched && !pin_low).
pub fn factory_pin_runtime_clear(pin_low: bool, latched: bool) -> bool {
    latched && !pin_low
}

/// [`PinHold::sample`] (C++ `PinHold::State`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinHoldState {
    Holding,
    Held,
    Released,
}

/// Debounced hold detector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PinHold {
    hold_ms: u32,
    start_ms: u32,
    state: PinHoldState,
}

impl PinHold {
    pub fn new(hold_ms: u32) -> Self {
        Self {
            hold_ms,
            start_ms: 0,
            state: PinHoldState::Holding,
        }
    }

    pub fn begin(&mut self, now_ms: u32) {
        self.start_ms = now_ms;
        self.state = PinHoldState::Holding;
    }

    /// Released on the first HIGH sample (final); Held once LOW for hold_ms (final).
    pub fn sample(&mut self, low: bool, now_ms: u32) -> PinHoldState {
        if self.state != PinHoldState::Holding {
            return self.state;
        }
        if !low {
            self.state = PinHoldState::Released;
        } else if elapsed_ms(now_ms, self.start_ms) >= self.hold_ms {
            self.state = PinHoldState::Held;
        }
        self.state
    }
}

#[cfg(test)]
mod tests;
