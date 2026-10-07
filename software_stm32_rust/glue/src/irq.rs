//! The interrupt model of the application stage (docs/rust/GLUE-DESIGN-STM.md §2.2, §2.3): the
//! NVIC priorities of the handlers, when the calling context keeps one of them out, and the
//! access protocol of the state the TIM1 and TIM2 handlers share with the main loop. The firmware
//! holds that state in its `IsrCell` and reaches it only through a [`CellGuard`], which decides
//! when it may.
//!
//! STM32duino's priorities (NVIC_PRIORITYGROUP_4: every bit is a preemption bit; the Rust images
//! keep AIRCR.PRIGROUP 0, the same split): USART1 and USART6 1, EXTI4 6, TIM1 and TIM2 14. The
//! STM32F4 implements the upper 4 bits of a priority byte, so level n is the register value
//! n << 4, and a lower value preempts a higher one.

use core::sync::atomic::{AtomicBool, Ordering};

/// `UART_IRQ_PRIO` 1: USART1 (ESP) and USART6 (debug terminal)
pub const PRIO_USART: u8 = 1 << 4;
/// `EXTI_IRQ_PRIO` 6: EXTI4 (the REVIN pulses)
pub const PRIO_EXTI: u8 = 6 << 4;
/// `TIM_IRQ_PRIO` 14: TIM1 (1 ms) and TIM2 (10 ms)
pub const PRIO_MOTOR: u8 = 14 << 4;

/// The interrupt masks of the core as the calling context sees them (firmware: PRIMASK and
/// BASEPRI).
pub trait Masks {
    /// PRIMASK is 1: every interrupt of configurable priority is masked
    fn primask(&self) -> bool;
    /// BASEPRI: 0 masks nothing, any other value every interrupt whose priority value is the
    /// same or higher
    fn basepri(&self) -> u8;
}

/// An interrupt of priority `prio` cannot run in the calling context: PRIMASK is set, or
/// BASEPRI masks its level. Only thread mode asks (no handler writes to a serial port), so the
/// priority of an active handler does not enter.
pub fn blocked(masks: &impl Masks, prio: u8) -> bool {
    if masks.primask() {
        return true;
    }
    let level = masks.basepri();
    level != 0 && level <= prio
}

/// What the access protocol of a shared cell needs of the core (firmware: BASEPRI and the fault
/// handler). Every call is one register access, no decision.
pub trait CellCore {
    /// reads BASEPRI
    fn basepri(&self) -> u8;
    /// BASEPRI_MAX: BASEPRI to `level` unless it masks more already; the memory accesses after
    /// the call stay after it
    fn raise(&self, level: u8);
    /// writes BASEPRI; the memory accesses before the call stay before it
    fn restore(&self, level: u8);
    /// a misuse of the cell: the fault handler (valve outputs off, fault record, IWDG reset)
    fn fault(&self) -> !;
}

/// The access protocol of a value that the TIM1 and TIM2 handlers share with the main loop
/// (design §2.3, firmware `IsrCell`): stored once before those interrupts run; then reached by
/// the two handlers, which share one priority and never preempt each other, and by the main loop
/// under a lock that masks exactly TIM1 and TIM2 (BASEPRI at [`PRIO_MOTOR`]: EXTI4 and the
/// USARTs go on) and is never nested. Every other use ends in the fault handler.
pub struct CellGuard {
    /// the value is stored
    ready: AtomicBool,
    /// a thread-mode lock is open (it hands out the only reference)
    locked: AtomicBool,
}

impl Default for CellGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl CellGuard {
    pub const fn new() -> Self {
        CellGuard {
            ready: AtomicBool::new(false),
            locked: AtomicBool::new(false),
        }
    }

    /// `store` puts the value in, before the interrupts that use it are enabled. A second call
    /// is a fault and stores nothing.
    pub fn init(&self, core: &impl CellCore, store: impl FnOnce()) {
        if self.ready.load(Ordering::SeqCst) {
            core.fault();
        }
        store();
        self.ready.store(true, Ordering::SeqCst);
    }

    /// `f` reaches the value from the TIM1 or the TIM2 handler; before [`CellGuard::init`] a
    /// fault.
    pub fn isr<R>(&self, core: &impl CellCore, f: impl FnOnce() -> R) -> R {
        if !self.ready.load(Ordering::SeqCst) {
            core.fault();
        }
        f()
    }

    /// `f` reaches the value from thread mode with TIM1 and TIM2 masked (`MotorLock::lock`);
    /// then BASEPRI is back at the caller's level. Before [`CellGuard::init`] or inside another
    /// lock: a fault, with the timers masked already.
    pub fn lock<R>(&self, core: &impl CellCore, f: impl FnOnce() -> R) -> R {
        let old = core.basepri();
        core.raise(PRIO_MOTOR);
        if !self.ready.load(Ordering::SeqCst) || self.locked.swap(true, Ordering::SeqCst) {
            core.fault();
        }
        let r = f();
        self.locked.store(false, Ordering::SeqCst);
        core.restore(old);
        r
    }
}

#[cfg(test)]
mod tests;
