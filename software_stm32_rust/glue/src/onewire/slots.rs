//! The bit slots of OneWire 2.3.7 as vendored in `software_stm32/lib/OneWire` (`reset`,
//! `write_bit`, `read_bit`; patched to drive the pin open drain: low drives the line, high
//! releases it) on a pin ([`OneWirePin`], firmware: PB10), and STM32duino's `delayMicroseconds`
//! on a cycle counter ([`spin_us`], firmware: the DWT cycle counter). As in the library, the
//! interrupts are masked inside each timed window. Risk R5: the slot timing is the library's;
//! the bench measures it on the hardware.

use crate::hal::OneWireLine;

/// The pin of the 1-Wire bus and the waits of the slots. Every call is one register access or
/// one wait, no decision.
pub trait OneWirePin {
    /// the pin drives the line low (BSRR reset)
    fn drive_low(&mut self);
    /// the pin releases the line, the pull-up raises it (BSRR set)
    fn release(&mut self);
    /// the level of the line (IDR)
    fn level(&mut self) -> bool;
    /// `delayMicroseconds(us)`
    fn delay_us(&mut self, us: u32);
    /// runs `f` with the interrupts masked (`noInterrupts()` .. `interrupts()`)
    fn masked<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R;
}

/// `OneWire::reset`: `retries` of the wait for a released line; `--retries == 0` ends it, so at
/// most 124 waits of 2 us (the library: "up to 250uS").
const RESET_RETRIES: u8 = 125;
const RESET_POLL_US: u32 = 2;
/// reset pulse, then the sample of the presence pulse after the release, then the rest of the
/// presence time slot
const RESET_LOW_US: u32 = 480;
const RESET_SAMPLE_US: u32 = 70;
const RESET_REST_US: u32 = 410;
/// write slots: (low, rest) of a 1 and of a 0
const WRITE_1_US: (u32, u32) = (10, 55);
const WRITE_0_US: (u32, u32) = (65, 5);
/// read slot: low, sample after the release, rest
const READ_LOW_US: u32 = 3;
const READ_SAMPLE_US: u32 = 10;
const READ_REST_US: u32 = 53;

/// The slots of the library on a pin: the firmware's [`OneWireLine`].
pub struct PinLine<P>(pub P);

impl<P: OneWirePin> OneWireLine for PinLine<P> {
    /// `OneWire::reset`: the line released; it must read high within the retries, else 0 (a
    /// broken or shorted bus); 480 us low, the sample 70 us after the release (a device pulls
    /// the line low: present), 410 us for the presence pulse to end.
    fn reset(&mut self) -> bool {
        let pin = &mut self.0;
        pin.release();
        let mut retries = RESET_RETRIES;
        loop {
            retries -= 1;
            if retries == 0 {
                return false;
            }
            pin.delay_us(RESET_POLL_US);
            if pin.level() {
                break;
            }
        }
        pin.masked(|p| p.drive_low());
        pin.delay_us(RESET_LOW_US);
        let present = pin.masked(|p| {
            p.release();
            p.delay_us(RESET_SAMPLE_US);
            !p.level()
        });
        pin.delay_us(RESET_REST_US);
        present
    }

    /// `OneWire::write_bit`: 1 = 10 us low, 55 us; 0 = 65 us low, 5 us.
    fn write_bit(&mut self, bit: bool) {
        let (low, rest) = if bit { WRITE_1_US } else { WRITE_0_US };
        self.0.masked(|p| {
            p.drive_low();
            p.delay_us(low);
            p.release();
        });
        self.0.delay_us(rest);
    }

    /// `OneWire::read_bit`: 3 us low, the sample 10 us after the release, 53 us.
    fn read_bit(&mut self) -> bool {
        let bit = self.0.masked(|p| {
            p.drive_low();
            p.delay_us(READ_LOW_US);
            p.release();
            p.delay_us(READ_SAMPLE_US);
            p.level()
        });
        self.0.delay_us(READ_REST_US);
        bit
    }
}

/// A free-running 32-bit cycle counter (firmware: DWT CYCCNT, enabled by the application stage).
pub trait CycleCounter {
    fn cycles(&self) -> u32;
}

/// STM32duino's `delayMicroseconds(us)` on a cycle counter that counts `cycles_per_us` per
/// microsecond: spins until `us x cycles_per_us` cycles passed since its first read (the
/// difference wraps with the counter, as the C++ int32 difference).
pub fn spin_us(counter: &impl CycleCounter, us: u32, cycles_per_us: u32) {
    let start = counter.cycles();
    let cycles = us.wrapping_mul(cycles_per_us);
    while counter.cycles().wrapping_sub(start) < cycles {}
}

#[cfg(test)]
mod tests;
