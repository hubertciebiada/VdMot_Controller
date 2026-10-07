//! The 1-Wire line on PB10 (docs/rust/GLUE-DESIGN-STM.md §1.3): OneWire 2.3.7 as vendored and
//! patched in `software_stm32/lib/OneWire` (the pin always open drain: low drives the line,
//! high releases it), the bit slots with the library's timing and interrupts masked inside each
//! timed window. The microsecond waits count CPU cycles on the DWT cycle counter, as STM32duino's
//! `delayMicroseconds`. Risk R5: the slot timing is the library's; the bench measures it.
#![forbid(unsafe_code)]

use cortex_m::peripheral::DWT;
use stm32_metapac as pac;
use stm32_metapac::gpio::regs::Bsrr;
use vdm_stm_glue::hal::OneWireLine;

use crate::clocks::SYSCLK_HZ;

const PIN: u32 = 1 << 10;
const CYCLES_PER_US: u32 = SYSCLK_HZ / 1_000_000;

/// PB10 (set up by `app::run`: open drain, latch high, input buffer on).
pub struct FwOneWire;

fn drive_low() {
    pac::GPIOB.bsrr().write_value(Bsrr(PIN << 16));
}

fn release() {
    pac::GPIOB.bsrr().write_value(Bsrr(PIN));
}

fn level() -> bool {
    pac::GPIOB.idr().read().0 & PIN != 0
}

/// `delayMicroseconds(us)` on the cycle counter (enabled by `app::run`).
fn delay_us(us: u32) {
    let start = DWT::cycle_count();
    let cycles = us.wrapping_mul(CYCLES_PER_US);
    while DWT::cycle_count().wrapping_sub(start) < cycles {}
}

impl OneWireLine for FwOneWire {
    /// `OneWire::reset`: wait (at most 250 us) for a released line, 480 us low, sample 70 us
    /// after the release, 410 us for the presence pulse to end.
    fn reset(&mut self) -> bool {
        release();
        let mut retries: u8 = 125;
        loop {
            retries -= 1;
            if retries == 0 {
                return false;
            }
            delay_us(2);
            if level() {
                break;
            }
        }
        cortex_m::interrupt::free(|_| drive_low());
        delay_us(480);
        let present = cortex_m::interrupt::free(|_| {
            release();
            delay_us(70);
            !level()
        });
        delay_us(410);
        present
    }

    /// `OneWire::write_bit`: 1 = 10 us low, 55 us; 0 = 65 us low, 5 us.
    fn write_bit(&mut self, bit: bool) {
        let (low, rest) = if bit { (10, 55) } else { (65, 5) };
        cortex_m::interrupt::free(|_| {
            drive_low();
            delay_us(low);
            release();
        });
        delay_us(rest);
    }

    /// `OneWire::read_bit`: 3 us low, sample 10 us after the release, 53 us.
    fn read_bit(&mut self) -> bool {
        let bit = cortex_m::interrupt::free(|_| {
            drive_low();
            delay_us(3);
            release();
            delay_us(10);
            level()
        });
        delay_us(53);
        bit
    }
}
