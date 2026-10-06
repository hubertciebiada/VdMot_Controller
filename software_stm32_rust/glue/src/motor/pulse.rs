//! The revolution pulses of the running motor (EXTI4, rising edge of REVIN): the `isr_*`
//! variables of motor.cpp that the EXTI handler shares with TIM1 and TIM2. EXTI preempts both
//! timers, so these are atomics and need no lock (design §2.3).

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::hal::{Pins, RevIrq};

use super::ena_all_off;

pub struct Pulse {
    /// `isr_counter`: pulses counted while the motor turns
    pub(crate) counter: AtomicU32,
    /// `isr_target`: pulses left before the motor stops (counts down to 0)
    pub(crate) target: AtomicU32,
    /// `isr_turning`: the motor is switched on
    pub(crate) turning: AtomicBool,
    /// `isr_stop_request`: set by EXTI when the target count is reached
    pub(crate) stop_request: AtomicBool,
}

impl Pulse {
    pub const fn new() -> Self {
        Pulse {
            counter: AtomicU32::new(0),
            target: AtomicU32::new(0),
            turning: AtomicBool::new(false),
            stop_request: AtomicBool::new(false),
        }
    }

    /// `isr_count`, the EXTI4 handler: counts the pulse while the motor turns and stops the motor
    /// on the pulse after the target count.
    pub fn on_edge(&self, hw: &(impl Pins + RevIrq)) {
        if self.turning.load(Ordering::SeqCst) {
            let counted = self.counter.load(Ordering::SeqCst);
            self.counter
                .store(counted.wrapping_add(1), Ordering::SeqCst);
        }
        let left = self.target.load(Ordering::SeqCst);
        if left > 0 {
            self.target.store(left - 1, Ordering::SeqCst);
        } else {
            self.callback_motorstop(hw);
        }
    }

    /// `callback_motorstop`: stops the motor at once (from EXTI); the motor state machine picks
    /// the stop up on its next run (TIM2), it is not reentrant.
    pub(crate) fn callback_motorstop(&self, hw: &(impl Pins + RevIrq)) {
        hw.detach();
        self.turning.store(false, Ordering::SeqCst);
        ena_all_off(hw);
        self.stop_request.store(true, Ordering::SeqCst);
    }
}

impl Default for Pulse {
    fn default() -> Self {
        Self::new()
    }
}
