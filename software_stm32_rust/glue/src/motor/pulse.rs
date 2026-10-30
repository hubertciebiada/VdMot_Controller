//! The revolution pulses of the running motor (EXTI4, rising edge of REVIN): the `isr_*`
//! variables of motor.cpp that the EXTI handler shares with TIM1 and TIM2. EXTI preempts both
//! timers, so these are atomics and need no lock (design §2.3). The brake at the end of a
//! counted move (sbrak, 2.2.9) lives here as well: EXTI starts it on the stopping pulse, TIM1
//! ends it.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::hal::{Out, Pins, RevIrq};

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
    /// the brake of the move in progress (the motor state machine sets the three before the
    /// motor starts): the longest brake in ms, 0 = no brake
    pub(crate) brake_max_ms: AtomicU32,
    /// ms without a pulse that end the brake before its time, 0 = off
    pub(crate) brake_gap_ms: AtomicU32,
    /// the brake ends when no pulse came for this many times the interval before the stopping
    /// pulse, 0 = off
    pub(crate) brake_mult: AtomicU32,
    /// the motor is driven against its motion: the direction flipped on the stopping pulse, the
    /// enable still on
    pub(crate) braking: AtomicBool,
    /// ms of the brake left; TIM1 alone changes it while braking
    brake_left_ms: AtomicU32,
    /// the gap of brake_mult for the brake in progress in ms, 0 = off
    brake_adapt_ms: AtomicU32,
    /// ms since the last pulse while the motor turns: TIM1 counts, EXTI resets
    since_edge_ms: AtomicU32,
}

impl Pulse {
    pub const fn new() -> Self {
        Pulse {
            counter: AtomicU32::new(0),
            target: AtomicU32::new(0),
            turning: AtomicBool::new(false),
            stop_request: AtomicBool::new(false),
            brake_max_ms: AtomicU32::new(0),
            brake_gap_ms: AtomicU32::new(0),
            brake_mult: AtomicU32::new(0),
            braking: AtomicBool::new(false),
            brake_left_ms: AtomicU32::new(0),
            brake_adapt_ms: AtomicU32::new(0),
            since_edge_ms: AtomicU32::new(0),
        }
    }

    /// `isr_count`, the EXTI4 handler: counts the pulse while the motor turns and stops the motor
    /// on the pulse after the target count, or brakes it first when the move has a brake.
    pub fn on_edge(&self, hw: &(impl Pins + RevIrq)) {
        let turning = self.turning.load(Ordering::SeqCst);
        if turning {
            let counted = self.counter.load(Ordering::SeqCst);
            self.counter
                .store(counted.wrapping_add(1), Ordering::SeqCst);
        }
        // the interval before this pulse (TIM1 counted the ms)
        let interval = self.since_edge_ms.swap(0, Ordering::SeqCst);
        if self.braking.load(Ordering::SeqCst) {
            // a pulse of the slowing motor: forward travel, counted above; the brake ends on its
            // time or when the pulses stop (on_ms)
            return;
        }
        let left = self.target.load(Ordering::SeqCst);
        if left > 0 {
            self.target.store(left - 1, Ordering::SeqCst);
        } else if turning && self.brake_max_ms.load(Ordering::SeqCst) > 0 {
            self.start_brake(hw, interval);
        } else {
            self.callback_motorstop(hw);
        }
    }

    /// The brake: the direction flips with the enable on, so the motor is driven against its
    /// motion until on_ms switches it off. The common of the motors sits at half the supply, so
    /// the flip cannot short the motor; it draws the supply and its back EMF through the
    /// winding, about twice the stall current, for at most brake_max_ms. `interval`: the ms
    /// between the two last pulses, the measure of the speed for brake_mult.
    fn start_brake(&self, hw: &impl Pins, interval: u32) {
        hw.set(Out::Dir, !hw.latch(Out::Dir));
        self.brake_left_ms
            .store(self.brake_max_ms.load(Ordering::SeqCst), Ordering::SeqCst);
        self.brake_adapt_ms.store(
            self.brake_mult
                .load(Ordering::SeqCst)
                .saturating_mul(interval),
            Ordering::SeqCst,
        );
        self.braking.store(true, Ordering::SeqCst);
    }

    /// The 1 ms tick of the pulse interval and of the brake (timer_handler0). The brake ends when
    /// its time is up, or earlier when no pulse came for more than brake_gap_ms or for more than
    /// brake_mult times the interval before the stopping pulse (the motor stands; driven longer
    /// it would turn back, and its pulses would count as forward travel).
    pub(crate) fn on_ms(&self, hw: &(impl Pins + RevIrq)) {
        if !self.turning.load(Ordering::SeqCst) {
            return;
        }
        // one read-modify-write: a reset by EXTI between a load and a store would be lost
        let since = self.since_edge_ms.fetch_add(1, Ordering::SeqCst) + 1;
        if !self.braking.load(Ordering::SeqCst) {
            return;
        }
        let left = self.brake_left_ms.load(Ordering::SeqCst).saturating_sub(1);
        self.brake_left_ms.store(left, Ordering::SeqCst);
        let gap = self.brake_gap_ms.load(Ordering::SeqCst);
        let adapt = self.brake_adapt_ms.load(Ordering::SeqCst);
        if left == 0 || (gap > 0 && since > gap) || (adapt > 0 && since > adapt) {
            self.callback_motorstop(hw);
        }
    }

    /// The brake of the next move (the motor state machine, before the motor starts): max_ms 0
    /// means none. A brake still marked in progress is over.
    pub(crate) fn arm_brake(&self, max_ms: u8, gap_ms: u8, mult: u8) {
        self.braking.store(false, Ordering::SeqCst);
        self.brake_max_ms.store(u32::from(max_ms), Ordering::SeqCst);
        self.brake_gap_ms.store(u32::from(gap_ms), Ordering::SeqCst);
        self.brake_mult.store(u32::from(mult), Ordering::SeqCst);
        self.since_edge_ms.store(0, Ordering::SeqCst);
    }

    /// Switches the motor off from interrupt context (EXTI on the stopping pulse, TIM1 at the end
    /// of a brake, the halt of the motor state machine): the pulses are no longer counted, the
    /// enables go off, and the direction output of a brake goes back to the direction of the
    /// move (what enables a motor without setting it, the terminal's sena, sees the move's).
    pub(crate) fn motor_off(&self, hw: &(impl Pins + RevIrq)) {
        hw.detach();
        self.turning.store(false, Ordering::SeqCst);
        let braked = self.braking.swap(false, Ordering::SeqCst);
        ena_all_off(hw);
        if braked {
            hw.set(Out::Dir, !hw.latch(Out::Dir));
        }
    }

    /// `callback_motorstop`: stops the motor at once (from EXTI, or from TIM1 at the end of a
    /// brake); the motor state machine picks the stop up on its next run (TIM2), it is not
    /// reentrant.
    pub(crate) fn callback_motorstop(&self, hw: &(impl Pins + RevIrq)) {
        self.motor_off(hw);
        self.stop_request.store(true, Ordering::SeqCst);
    }
}

impl Default for Pulse {
    fn default() -> Self {
        Self::new()
    }
}
