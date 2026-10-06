//! I2C bus (EEPROM) recovery (`software_stm32/src/i2c_bus.cpp`): a slave that was reset in the
//! middle of a transfer (the EEPROM during a read when the STM was reset) may hold SDA low for
//! ever; SCL is clocked until it releases SDA, then a STOP is sent.

use crate::hal::Clock;

/// 100 kHz
const I2C_RECOVERY_HALF_CLOCK_US: u32 = 5;
/// clocks a slave needs at most to finish the byte it sends
const I2C_RECOVERY_CLOCKS: u8 = 9;

/// The lines of I2C1: SCL (PB6), SDA (PB7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum I2cLine {
    Scl,
    Sda,
}

/// Pin modes of the recovery (C++ `INPUT`, `OUTPUT_OPEN_DRAIN`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineMode {
    Input,
    OutputOpenDrain,
}

/// The two lines as GPIO while the I2C driver is off (`pinMode`, `digitalWrite`, `digitalRead`
/// on I2C_SCL_PIN and I2C_SDA_PIN). The board pulls both lines up.
pub trait RecoveryPins {
    fn mode(&mut self, line: I2cLine, mode: LineMode);
    fn write(&mut self, line: I2cLine, high: bool);
    fn read(&mut self, line: I2cLine) -> bool;
}

/// The I2C driver on the EEPROM pins (`Wire.end()`; `Wire.setSDA`, `Wire.setSCL`,
/// `Wire.begin()`).
pub trait Wire {
    fn end(&mut self);
    fn begin(&mut self);
}

/// `i2c_bus_recover`: frees the bus from a slave that holds SDA low after a reset in the
/// middle of a transfer; call before the driver starts.
pub fn recover(pins: &mut impl RecoveryPins, clock: &impl Clock) {
    pins.mode(I2cLine::Sda, LineMode::Input);
    pins.write(I2cLine::Scl, true);
    pins.mode(I2cLine::Scl, LineMode::OutputOpenDrain);
    clock.delay_us(I2C_RECOVERY_HALF_CLOCK_US);

    for _ in 0..I2C_RECOVERY_CLOCKS {
        if pins.read(I2cLine::Sda) {
            break;
        }
        pins.write(I2cLine::Scl, false);
        clock.delay_us(I2C_RECOVERY_HALF_CLOCK_US);
        pins.write(I2cLine::Scl, true);
        clock.delay_us(I2C_RECOVERY_HALF_CLOCK_US);
    }

    // STOP: SDA rises while SCL is high
    pins.write(I2cLine::Scl, false);
    pins.write(I2cLine::Sda, false);
    pins.mode(I2cLine::Sda, LineMode::OutputOpenDrain);
    clock.delay_us(I2C_RECOVERY_HALF_CLOCK_US);
    pins.write(I2cLine::Scl, true);
    clock.delay_us(I2C_RECOVERY_HALF_CLOCK_US);
    pins.write(I2cLine::Sda, true);
    clock.delay_us(I2C_RECOVERY_HALF_CLOCK_US);

    pins.mode(I2cLine::Sda, LineMode::Input);
    pins.mode(I2cLine::Scl, LineMode::Input);
}

/// `i2c_bus_restart`: a retry of a failed EEPROM transfer may find the bus stuck the same way
/// (a slave that lost clocks when the transfer was disturbed): the driver is stopped, the bus
/// freed and the driver started again on the EEPROM pins.
pub fn restart<B: Wire + RecoveryPins>(bus: &mut B, clock: &impl Clock) {
    bus.end();
    recover(bus, clock);
    bus.begin();
}

#[cfg(test)]
mod tests;
