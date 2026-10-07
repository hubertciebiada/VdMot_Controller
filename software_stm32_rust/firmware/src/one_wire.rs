//! The 1-Wire pin PB10 (docs/rust/GLUE-DESIGN-STM.md §1.3) under the glue's bit slots
//! (`vdm_stm_glue::onewire::PinLine`: OneWire 2.3.7 as vendored and patched in
//! `software_stm32/lib/OneWire`, the pin always open drain: low drives the line, high releases
//! it). The microsecond waits count CPU cycles on the DWT cycle counter (glue `spin_us`, as
//! STM32duino's `delayMicroseconds`); a masked window is the single-core critical section.
#![forbid(unsafe_code)]

use cortex_m::peripheral::DWT;
use stm32_metapac as pac;
use stm32_metapac::gpio::regs::Bsrr;
use vdm_stm_glue::onewire::{spin_us, CycleCounter, OneWirePin};

use crate::clocks::SYSCLK_HZ;

const PIN: u32 = 1 << 10;
const CYCLES_PER_US: u32 = SYSCLK_HZ / 1_000_000;

/// PB10 (set up by `app::run`: open drain, latch high, input buffer on).
pub struct FwOneWire;

/// The DWT cycle counter (enabled by `app::run`).
struct Dwt;

impl CycleCounter for Dwt {
    fn cycles(&self) -> u32 {
        DWT::cycle_count()
    }
}

impl OneWirePin for FwOneWire {
    fn drive_low(&mut self) {
        pac::GPIOB.bsrr().write_value(Bsrr(PIN << 16));
    }

    fn release(&mut self) {
        pac::GPIOB.bsrr().write_value(Bsrr(PIN));
    }

    fn level(&mut self) -> bool {
        pac::GPIOB.idr().read().0 & PIN != 0
    }

    fn delay_us(&mut self, us: u32) {
        spin_us(&Dwt, us, CYCLES_PER_US);
    }

    fn masked<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        cortex_m::interrupt::free(|_| f(self))
    }
}
