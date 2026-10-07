//! The interrupts of the application stage and the state they share with the main loop
//! (docs/rust/GLUE-DESIGN-STM.md §2.2, §2.3). Every handler reads its registers and hands them to
//! the glue; the decisions are the glue's.
//!
//! | handler | priority | glue |
//! |---|---|---|
//! | `TIM1_UP_TIM10` | P14 | `motor::timer_handler0` (ADC, soft start, end stops) |
//! | `TIM2` | P14 | `motor::valve_loop` (valve state machine) |
//! | `EXTI4` | P6 | `motor::Pulse::on_edge` (REVIN pulses) |
//! | `USART1`, `USART6` | P1 | `serial::Port::on_irq` |
//!
//! The time driver of embassy (TIM5) runs at P0. TIM1 and TIM2 share P14: they never preempt each
//! other, and the main loop masks exactly them (BASEPRI) while it holds the motor state.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{compiler_fence, AtomicBool, Ordering};

use cortex_m::register::{basepri, basepri_max};
use embassy_stm32::interrupt::{self, InterruptExt, Priority};
use stm32_metapac as pac;
use vdm_stm_glue::motor::{timer_handler0, valve_loop, IsrFlags, MotorShared, Pulse};
use vdm_stm_glue::serial::{Port, Tx, SR_FE, SR_NE, SR_ORE, SR_PE, SR_RXNE};
use vdm_stm_glue::system::MotorLock;

use crate::board::{FwAdc, FwBoard};
use crate::fault;

/// TIM1 and TIM2 (STM32duino: TIM_IRQ_PRIO 14)
pub const PRIO_MOTOR: Priority = Priority::P14;
/// EXTI4 (STM32duino: EXTI_IRQ_PRIO 6)
pub const PRIO_EXTI: Priority = Priority::P6;
/// USART1, USART6 (STM32duino: UART_IRQ_PRIO 1)
pub const PRIO_USART: Priority = Priority::P1;

/// The motor state of TIM1, TIM2 and the main loop.
pub static MOTOR: IsrCell<MotorShared> = IsrCell::new();
/// The current inputs of TIM1 (only TIM1 uses them).
pub static ADC: IsrCell<FwAdc> = IsrCell::new();
/// The revolution pulses (EXTI4 preempts the timers: atomics).
pub static PULSE: Pulse = Pulse::new();
/// What TIM2 shares with the main loop outside the lock (atomics).
pub static FLAGS: IsrFlags = IsrFlags::new();
/// USART1 (ESP) and USART6 (debug terminal).
pub static ESP: Port = Port::new();
pub static DBG: Port = Port::new();

/// A value shared by the TIM1 and TIM2 handlers and the main loop: `init` once before those
/// interrupts run, then `with_isr` in the handlers and `lock` (MotorLock) in thread mode.
pub struct IsrCell<T> {
    value: UnsafeCell<MaybeUninit<T>>,
    ready: AtomicBool,
    /// a thread-mode lock is open (it hands out `&mut`: no nesting)
    locked: AtomicBool,
}

// SAFETY: the value is reached only through `init` (once, before the interrupts that use it are
// enabled), `with_isr` (from the TIM1 and TIM2 handlers, which share one priority and never
// preempt each other) and `lock` (thread mode, with BASEPRI masking that priority, not nested).
unsafe impl<T: Send> Sync for IsrCell<T> {}

impl<T> IsrCell<T> {
    pub const fn new() -> Self {
        IsrCell {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            ready: AtomicBool::new(false),
            locked: AtomicBool::new(false),
        }
    }

    /// Stores the value; a second call is a fault.
    pub fn init(&self, value: T) {
        if self.ready.load(Ordering::SeqCst) {
            fault::on_panic();
        }
        // SAFETY: not ready yet, so no handler and no lock reads it; the interrupts that use it
        // are enabled after this call
        unsafe { (*self.value.get()).write(value) };
        self.ready.store(true, Ordering::SeqCst);
    }

    /// The value for a TIM1 or TIM2 handler.
    pub fn with_isr<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        if !self.ready.load(Ordering::SeqCst) {
            fault::on_panic();
        }
        // SAFETY: ready (initialised); only the P14 handlers call this, they do not preempt each
        // other, and the main loop holds no reference while they can run (lock masks them)
        f(unsafe { (*self.value.get()).assume_init_mut() })
    }
}

impl MotorLock for IsrCell<MotorShared> {
    fn lock<R>(&self, f: impl FnOnce(&mut MotorShared) -> R) -> R {
        let old = basepri::read();
        basepri_max::write(PRIO_MOTOR as u8);
        compiler_fence(Ordering::SeqCst);
        if !self.ready.load(Ordering::SeqCst) || self.locked.swap(true, Ordering::SeqCst) {
            fault::on_panic();
        }
        // SAFETY: ready; TIM1 and TIM2 are masked by BASEPRI until the old level is back, and
        // the lock is not nested (checked above), so this is the only reference
        let r = f(unsafe { (*self.value.get()).assume_init_mut() });
        self.locked.store(false, Ordering::SeqCst);
        compiler_fence(Ordering::SeqCst);
        // SAFETY: back to the caller's level; raising and restoring BASEPRI only masks
        unsafe { basepri::write(old) };
        r
    }
}

/// The control timers.
#[derive(Clone, Copy)]
pub enum TimerIrq {
    Tim1,
    Tim2,
}

/// The update interrupt of a control timer on, at P14 (`attachInterruptInterval`).
pub fn timer_irq_on(irq: TimerIrq) {
    let line = match irq {
        TimerIrq::Tim1 => interrupt::TIM1_UP_TIM10,
        TimerIrq::Tim2 => interrupt::TIM2,
    };
    line.set_priority(PRIO_MOTOR);
    // SAFETY: the handlers below use only the statics of this module, initialised before
    // (MOTOR, ADC: IsrCell checks it)
    unsafe { line.enable() };
}

/// EXTI4 at P6 and the two USART interrupts at P1, configured once; EXTI4 stays off until a
/// motor start attaches it.
pub fn priorities() {
    interrupt::EXTI4.set_priority(PRIO_EXTI);
    interrupt::USART1.set_priority(PRIO_USART);
    interrupt::USART6.set_priority(PRIO_USART);
}

/// The receive interrupts of USART1 and USART6 on (the rings are ready).
pub fn usart_irqs_on() {
    for (u, line) in [
        (pac::USART1, interrupt::USART1),
        (pac::USART6, interrupt::USART6),
    ] {
        u.cr1().modify(|w| w.set_rxneie(true));
        // SAFETY: the handler uses the Port statics, which are const-initialised
        unsafe { line.enable() };
    }
}

/// `attachInterrupt(REVINPIN, ..., RISING)` / `detachInterrupt` (stm32_interrupt_enable/disable):
/// EXTI line 4 masked or not, its NVIC line on or off; a pending edge stays pending.
pub fn rev_irq(on: bool) {
    cortex_m::interrupt::free(|_| pac::EXTI.imr(0).modify(|w| w.set_line(4, on)));
    if on {
        // SAFETY: the handler uses the Pulse atomics and the board pins
        unsafe { interrupt::EXTI4.enable() };
    } else {
        interrupt::EXTI4.disable();
    }
}

#[allow(non_snake_case)]
#[unsafe(no_mangle)]
unsafe extern "C" fn TIM1_UP_TIM10() {
    pac::TIM1.sr().modify(|w| w.set_uif(false));
    ADC.with_isr(|adc| MOTOR.with_isr(|m| timer_handler0(m, &PULSE, adc, &FwBoard)));
}

#[allow(non_snake_case)]
#[unsafe(no_mangle)]
unsafe extern "C" fn TIM2() {
    pac::TIM2.sr().modify(|w| w.set_uif(false));
    MOTOR.with_isr(|m| valve_loop(m, &PULSE, &FLAGS, &FwBoard));
}

#[allow(non_snake_case)]
#[unsafe(no_mangle)]
unsafe extern "C" fn EXTI4() {
    // HAL_GPIO_EXTI_IRQHandler clears the line before the callback
    pac::EXTI.pr(0).write(|w| w.set_line(4, true));
    PULSE.on_edge(&FwBoard);
}

#[allow(non_snake_case)]
#[unsafe(no_mangle)]
unsafe extern "C" fn USART1() {
    usart(pac::USART1, &ESP);
}

#[allow(non_snake_case)]
#[unsafe(no_mangle)]
unsafe extern "C" fn USART6() {
    usart(pac::USART6, &DBG);
}

/// One USART interrupt: SR, then DR (the pair clears RXNE and the error flags), the glue decides.
fn usart(u: pac::usart::Usart, port: &Port) {
    let sr = u.sr().read().0;
    let dr = if sr & (SR_RXNE + SR_ORE + SR_NE + SR_FE + SR_PE) != 0 {
        u.dr().read().0 as u8
    } else {
        0
    };
    match port.on_irq(sr, dr) {
        Tx::Send(byte) => u
            .dr()
            .write_value(pac::usart::regs::Dr(u32::from(byte))),
        Tx::Idle => u.cr1().modify(|w| w.set_txeie(false)),
        Tx::None => {}
    }
}
