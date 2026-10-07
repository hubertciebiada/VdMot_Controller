//! `Wire` on I2C1 (PB6 SCL, PB7 SDA, AF4, open drain, external pull-ups) at 100 kHz for the 24LC64
//! (docs/rust/GLUE-DESIGN-STM.md §1.3, §3.1): the registers under the glue's master
//! (`vdm_stm_glue::i2c_master`, which holds the sequence and its decisions), the set-up of the
//! peripheral (`Wire.begin()`), and the two lines as GPIO for the bus recovery
//! (`i2c_bus::recover`, glue) while the peripheral is off.
#![forbid(unsafe_code)]

use stm32_metapac as pac;
use stm32_metapac::gpio::regs::Bsrr;
use stm32_metapac::i2c::regs::Sr1;
use stm32_metapac::i2c::vals::FS;
use vdm_stm_glue::i2c_bus::{self, I2cLine, LineMode, RecoveryPins, Wire};
use vdm_stm_glue::i2c_master::I2cRegs;
use vdm_stm_glue::system::I2cBus;

use crate::board::FwBoard;
use crate::clocks::PCLK1_HZ;

/// I2C1 clock enable / reset (RCC_APB1ENR, RCC_APB1RSTR bit 21)
const APB1_I2C1: u32 = 1 << 21;
const SCL: u32 = 6;
const SDA: u32 = 7;
const AF_I2C1: u32 = 4;
/// 100 kHz, standard mode
const BUS_HZ: u32 = 100_000;

/// I2C1 and its two lines (no state: the registers are the state).
#[derive(Clone, Copy)]
pub struct FwI2c;

fn line_bit(line: I2cLine) -> u32 {
    match line {
        I2cLine::Scl => SCL,
        I2cLine::Sda => SDA,
    }
}

/// MODER of one pin of GPIOB: 00 input, 01 output, 10 alternate function.
fn moder(bit: u32, mode: u32) {
    pac::GPIOB
        .moder()
        .modify(|w| w.0 = (w.0 & !(0b11 << (2 * bit))) | (mode << (2 * bit)));
}

/// The two lines as I2C1 pins: open drain, no pull, AF4.
fn pins_af() {
    let b = pac::GPIOB;
    b.otyper().modify(|w| w.0 |= (1 << SCL) | (1 << SDA));
    b.pupdr()
        .modify(|w| w.0 &= !((0b11 << (2 * SCL)) | (0b11 << (2 * SDA))));
    b.afr(0).modify(|w| {
        w.0 = (w.0 & !((0xF << (4 * SCL)) | (0xF << (4 * SDA))))
            | (AF_I2C1 << (4 * SCL))
            | (AF_I2C1 << (4 * SDA))
    });
    moder(SCL, 0b10);
    moder(SDA, 0b10);
}

/// `Wire.begin()`: clock, reset, timing for 100 kHz from PCLK1, on.
fn peripheral_on() {
    pac::RCC.apb1enr().modify(|w| w.0 |= APB1_I2C1);
    let _ = pac::RCC.apb1enr().read();
    pins_af();
    let r = pac::I2C1;
    r.cr1().modify(|w| w.set_pe(false));
    // RM0368 errata: SWRST after power-up, PE off, before the configuration
    r.cr1().modify(|w| w.set_swrst(true));
    r.cr1().modify(|w| w.set_swrst(false));
    let mhz = PCLK1_HZ / 1_000_000;
    r.cr2().modify(|w| w.set_freq(mhz as u8));
    r.ccr().write(|w| {
        w.set_f_s(FS::STANDARD);
        w.set_ccr((PCLK1_HZ / (2 * BUS_HZ)) as u16);
    });
    r.trise().write(|w| w.set_trise((mhz + 1) as u8));
    r.cr1().modify(|w| w.set_pe(true));
}

impl I2cRegs for FwI2c {
    fn sr1(&self) -> u16 {
        pac::I2C1.sr1().read().0 as u16
    }

    fn sr2(&self) -> u16 {
        pac::I2C1.sr2().read().0 as u16
    }

    /// rc_w0 flags: 0 clears, 1 keeps; no read-modify-write that could clear a flag set meanwhile
    fn clear_sr1(&self, flags: u16) {
        pac::I2C1.sr1().write_value(Sr1(u32::from(!flags)));
    }

    fn start(&self, ack: bool) {
        pac::I2C1.cr1().modify(|w| {
            w.set_ack(ack);
            w.set_start(true);
        });
    }

    fn stop(&self) {
        pac::I2C1.cr1().modify(|w| w.set_stop(true));
    }

    fn set_ack(&self, on: bool) {
        pac::I2C1.cr1().modify(|w| w.set_ack(on));
    }

    fn write_dr(&self, byte: u8) {
        pac::I2C1.dr().write(|w| w.set_dr(byte));
    }

    fn read_dr(&self) -> u8 {
        pac::I2C1.dr().read().dr()
    }
}

impl RecoveryPins for FwI2c {
    fn mode(&mut self, line: I2cLine, mode: LineMode) {
        let bit = line_bit(line);
        match mode {
            LineMode::Input => moder(bit, 0b00),
            LineMode::OutputOpenDrain => {
                pac::GPIOB.otyper().modify(|w| w.0 |= 1 << bit);
                moder(bit, 0b01);
            }
        }
    }

    fn write(&mut self, line: I2cLine, high: bool) {
        let bit = line_bit(line);
        let mask = if high { 1 << bit } else { 1 << (bit + 16) };
        pac::GPIOB.bsrr().write_value(Bsrr(mask));
    }

    fn read(&mut self, line: I2cLine) -> bool {
        pac::GPIOB.idr().read().0 & (1 << line_bit(line)) != 0
    }
}

impl Wire for FwI2c {
    /// `Wire.end()`: the peripheral off, the lines inputs.
    fn end(&mut self) {
        pac::I2C1.cr1().modify(|w| w.set_pe(false));
        moder(SCL, 0b00);
        moder(SDA, 0b00);
    }

    fn begin(&mut self) {
        peripheral_on();
    }
}

impl I2cBus for FwI2c {
    fn recover(&mut self) {
        i2c_bus::recover(self, &FwBoard);
    }

    fn begin(&mut self) {
        peripheral_on();
    }

    fn restart(&mut self) {
        i2c_bus::restart(self, &FwBoard);
    }
}
