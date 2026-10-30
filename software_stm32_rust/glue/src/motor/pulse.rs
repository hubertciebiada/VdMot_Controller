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
    /// the brake of the move in progress: the longest brake in ms, 0 = no brake (the motor state
    /// machine sets it before the motor starts)
    pub(crate) brake_max_ms: AtomicU32,
    /// ms without a pulse that end the brake before its time, 0 = the time only
    pub(crate) brake_gap_ms: AtomicU32,
    /// the motor is driven against its motion: the direction flipped on the stopping pulse, the
    /// enable still on
    pub(crate) braking: AtomicBool,
    /// ms of the brake left
    brake_left_ms: AtomicU32,
    /// ms since the last pulse while braking
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
            braking: AtomicBool::new(false),
            brake_left_ms: AtomicU32::new(0),
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
        if self.braking.load(Ordering::SeqCst) {
            // a pulse of the slowing motor: forward travel, counted above; the brake ends on its
            // time or when the pulses stop (on_ms)
            self.since_edge_ms.store(0, Ordering::SeqCst);
            return;
        }
        let left = self.target.load(Ordering::SeqCst);
        if left > 0 {
            self.target.store(left - 1, Ordering::SeqCst);
        } else if turning && self.brake_max_ms.load(Ordering::SeqCst) > 0 {
            self.start_brake(hw);
        } else {
            self.callback_motorstop(hw);
        }
    }

    /// The brake: the direction flips with the enable on, so the motor is driven against its
    /// motion until on_ms switches it off. The common of the motors sits at half the supply, so
    /// the flip cannot short the motor; it draws the supply and its back EMF through the
    /// winding, about twice the stall current, for at most brake_max_ms.
    fn start_brake(&self, hw: &impl Pins) {
        hw.set(Out::Dir, !hw.latch(Out::Dir));
        self.since_edge_ms.store(0, Ordering::SeqCst);
        self.brake_left_ms
            .store(self.brake_max_ms.load(Ordering::SeqCst), Ordering::SeqCst);
        self.braking.store(true, Ordering::SeqCst);
    }

    /// The 1 ms tick of the brake (timer_handler0): it ends when its time is up, or earlier when
    /// no pulse came for more than brake_gap_ms (the motor stands; driven longer it would turn
    /// back, and its pulses would count as forward travel).
    pub(crate) fn on_ms(&self, hw: &(impl Pins + RevIrq)) {
        if !self.braking.load(Ordering::SeqCst) {
            return;
        }
        let left = self.brake_left_ms.load(Ordering::SeqCst).saturating_sub(1);
        self.brake_left_ms.store(left, Ordering::SeqCst);
        let since = self.since_edge_ms.load(Ordering::SeqCst).saturating_add(1);
        self.since_edge_ms.store(since, Ordering::SeqCst);
        let gap = self.brake_gap_ms.load(Ordering::SeqCst);
        if left == 0 || (gap > 0 && since > gap) {
            self.callback_motorstop(hw);
        }
    }

    /// The brake of the next move (the motor state machine, before the motor starts): max_ms 0
    /// means none. A brake still marked in progress is over.
    pub(crate) fn arm_brake(&self, max_ms: u8, gap_ms: u8) {
        self.braking.store(false, Ordering::SeqCst);
        self.brake_max_ms.store(u32::from(max_ms), Ordering::SeqCst);
        self.brake_gap_ms.store(u32::from(gap_ms), Ordering::SeqCst);
    }

    /// `callback_motorstop`: stops the motor at once (from EXTI, or from TIM1 at the end of a
    /// brake); the motor state machine picks the stop up on its next run (TIM2), it is not
    /// reentrant.
    pub(crate) fn callback_motorstop(&self, hw: &(impl Pins + RevIrq)) {
        hw.detach();
        self.turning.store(false, Ordering::SeqCst);
        self.braking.store(false, Ordering::SeqCst);
        ena_all_off(hw);
        self.stop_request.store(true, Ordering::SeqCst);
    }
}

impl Default for Pulse {
    fn default() -> Self {
        Self::new()
    }
}
