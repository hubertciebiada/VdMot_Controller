//! The interrupts of the application stage and the state they share with the main loop
//! (docs/rust/GLUE-DESIGN-STM.md §2.2, §2.3). Every handler clears its flag and hands its
//! registers to the glue; the decisions are the glue's.
//!
//! | handler | priority (glue `irq`) | glue |
//! |---|---|---|
//! | `TIM1_UP_TIM10` | `PRIO_MOTOR` (14) | `motor::timer_handler0` (ADC, soft start, end stops) |
//! | `TIM2` | `PRIO_MOTOR` (14) | `motor::valve_loop` (valve state machine) |
//! | `EXTI4` | `PRIO_EXTI` (6) | `motor::Pulse::on_edge` (REVIN pulses) |
//! | `USART1`, `USART6` | `PRIO_USART` (1) | `serial::Port::irq` |
//!
//! The time driver of embassy (TIM5) runs at P0. TIM1 and TIM2 share P14: they never preempt each
//! other, and the main loop masks exactly them (BASEPRI) while it holds the motor state.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{compiler_fence, Ordering};

use cortex_m::register::{basepri, basepri_max};
use embassy_stm32::interrupt::{self, InterruptExt, Priority};
use stm32_metapac as pac;
use vdm_stm_glue::irq::{CellCore, CellGuard, PRIO_EXTI, PRIO_MOTOR, PRIO_USART};
use vdm_stm_glue::motor::{timer_handler0, valve_loop, IsrFlags, MotorShared, Pulse};
use vdm_stm_glue::serial::Port;
use vdm_stm_glue::system::MotorLock;

use crate::board::{FwAdc, FwBoard, FwUsart};
use crate::fault;

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

/// BASEPRI and the fault handler for the glue's `CellGuard`.
struct FwCore;

impl CellCore for FwCore {
    fn basepri(&self) -> u8 {
        basepri::read()
    }

    fn raise(&self, level: u8) {
        basepri_max::write(level);
        compiler_fence(Ordering::SeqCst);
    }

    fn restore(&self, level: u8) {
        compiler_fence(Ordering::SeqCst);
        // SAFETY: back to the level the caller had; writing BASEPRI only changes what is masked
        unsafe { basepri::write(level) };
    }

    fn fault(&self) -> ! {
        fault::on_panic()
    }
}

/// A value shared by the TIM1 and TIM2 handlers and the main loop: `init` once before those
/// interrupts run, then `with_isr` in the handlers and `lock` (MotorLock) in thread mode. The
/// glue's `CellGuard` decides when the value may be reached; a misuse ends in the fault handler.
pub struct IsrCell<T> {
    value: UnsafeCell<MaybeUninit<T>>,
    guard: CellGuard,
}

// SAFETY: the value is reached only through the guard: after `init` stored it, from the TIM1 and
// TIM2 handlers (one priority: they never preempt each other) and from one thread-mode lock at a
// time, while BASEPRI masks those handlers.
unsafe impl<T: Send> Sync for IsrCell<T> {}

impl<T> IsrCell<T> {
    pub const fn new() -> Self {
        IsrCell {
            value: UnsafeCell::new(MaybeUninit::uninit()),
            guard: CellGuard::new(),
        }
    }

    /// Stores the value; a second call is a fault.
    pub fn init(&self, value: T) {
        self.guard.init(&FwCore, || {
            // SAFETY: the guard runs this once, before the value counts as stored: no handler
            // and no lock reach it yet, and the interrupts that use it are enabled after this
            unsafe { (*self.value.get()).write(value) };
        });
    }

    /// The value for a TIM1 or TIM2 handler.
    pub fn with_isr<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        self.guard.isr(&FwCore, || {
            // SAFETY: stored (the guard checked it); only the P14 handlers call this, they do not
            // preempt each other, and the main loop holds no reference while they can run (its
            // lock masks them)
            f(unsafe { (*self.value.get()).assume_init_mut() })
        })
    }
}

impl MotorLock for IsrCell<MotorShared> {
    fn lock<R>(&self, f: impl FnOnce(&mut MotorShared) -> R) -> R {
        self.guard.lock(&FwCore, || {
            // SAFETY: stored, TIM1 and TIM2 masked by BASEPRI until the guard restores the old
            // level, and no other lock open (the guard checked both): the only reference
            f(unsafe { (*self.value.get()).assume_init_mut() })
        })
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
    line.set_priority(Priority::from(PRIO_MOTOR));
    // SAFETY: the handlers below use only the statics of this module, initialised before
    // (MOTOR, ADC: their guard checks it)
    unsafe { line.enable() };
}

/// EXTI4 at P6 and the two USART interrupts at P1, configured once; EXTI4 stays off until a
/// motor start attaches it.
pub fn priorities() {
    interrupt::EXTI4.set_priority(Priority::from(PRIO_EXTI));
    interrupt::USART1.set_priority(Priority::from(PRIO_USART));
    interrupt::USART6.set_priority(Priority::from(PRIO_USART));
}

/// The receive interrupts of USART1 and USART6 on (the rings are ready).
pub fn usart_irqs_on() {
    pac::USART1.cr1().modify(|w| w.set_rxneie(true));
    // SAFETY: the handler uses the Port statics, which are const-initialised
    unsafe { interrupt::USART1.enable() };
    pac::USART6.cr1().modify(|w| w.set_rxneie(true));
    // SAFETY: as USART1
    unsafe { interrupt::USART6.enable() };
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
    ESP.irq(&FwUsart(pac::USART1));
}

#[allow(non_snake_case)]
#[unsafe(no_mangle)]
unsafe extern "C" fn USART6() {
    DBG.irq(&FwUsart(pac::USART6));
}
