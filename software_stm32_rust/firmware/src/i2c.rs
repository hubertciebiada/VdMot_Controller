//! `Wire` on I2C1 (PB6 SCL, PB7 SDA, AF4, open drain, external pull-ups) at 100 kHz for the 24LC64
//! (docs/rust/GLUE-DESIGN-STM.md §1.3, §3.1): a blocking master on the registers, as the STM32
//! core's `endTransmission` / `requestFrom`: each transfer with its own START and STOP, every wait
//! bounded by 100 ms (`I2C_TIMEOUT_TICK`).
//!
//! Not the embassy-stm32 0.6.0 driver: its v1 `blocking_write` of zero bytes (the address probe
//! of the EEPROM's ready polling) waits for BTF, which never comes without a data byte, and a NACK
//! leaves the bus without a STOP. The bus recovery (`i2c_bus::recover`, glue) drives the two
//! lines as GPIO while the peripheral is off.
#![forbid(unsafe_code)]

use embassy_time::{Duration, Instant};
use stm32_metapac as pac;
use stm32_metapac::gpio::regs::Bsrr;
use stm32_metapac::i2c::vals::FS;
use vdm_stm_glue::hal::{I2cMaster, WireStatus, WIRE_ERROR, WIRE_NACK, WIRE_OK, WIRE_TIMEOUT};
use vdm_stm_glue::i2c_bus::{self, I2cLine, LineMode, RecoveryPins, Wire};
use vdm_stm_glue::system::I2cBus;

use crate::board::FwBoard;
use crate::clocks::PCLK1_HZ;

/// I2C1 clock enable / reset (RCC_APB1ENR, RCC_APB1RSTR bit 21)
const APB1_I2C1: u32 = 1 << 21;
const SCL: u32 = 6;
const SDA: u32 = 7;
const AF_I2C1: u32 = 4;
/// `I2C_TIMEOUT_TICK`
const PHASE_TIMEOUT: Duration = Duration::from_millis(100);
/// 100 kHz, standard mode
const BUS_HZ: u32 = 100_000;

/// The I2C1 master and its two lines (no state: the registers are the state).
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

/// Waits for `done` (or an error flag); false after the phase timeout.
fn wait(done: impl Fn(pac::i2c::regs::Sr1) -> bool) -> Result<(), WireStatus> {
    let start = Instant::now();
    loop {
        let sr1 = pac::I2C1.sr1().read();
        if sr1.af() {
            // NACK: clear, STOP
            pac::I2C1.sr1().modify(|w| w.set_af(false));
            stop();
            return Err(WIRE_NACK);
        }
        if sr1.arlo() || sr1.berr() {
            pac::I2C1.sr1().modify(|w| {
                w.set_arlo(false);
                w.set_berr(false);
            });
            stop();
            return Err(WIRE_ERROR);
        }
        if done(sr1) {
            return Ok(());
        }
        if start.elapsed() > PHASE_TIMEOUT {
            stop();
            return Err(WIRE_TIMEOUT);
        }
    }
}

fn stop() {
    pac::I2C1.cr1().modify(|w| w.set_stop(true));
}

/// START, the address byte, ADDR (or NACK, timeout); SR2 read clears ADDR unless `keep_addr`.
fn address(addr: u8, read: bool, keep_addr: bool) -> Result<(), WireStatus> {
    let r = pac::I2C1;
    let start = Instant::now();
    while r.sr2().read().busy() {
        if start.elapsed() > PHASE_TIMEOUT {
            return Err(WIRE_ERROR);
        }
    }
    r.cr1().modify(|w| {
        w.set_ack(read);
        w.set_start(true);
    });
    wait(|s| s.start())?;
    r.dr()
        .write(|w| w.set_dr((addr << 1) + u8::from(read)));
    wait(|s| s.addr())?;
    if !keep_addr {
        let _ = r.sr2().read();
    }
    Ok(())
}

impl I2cMaster for FwI2c {
    fn write(&mut self, addr: u8, bytes: &[u8]) -> WireStatus {
        let r = pac::I2C1;
        let sent = (|| {
            address(addr, false, false)?;
            for &b in bytes {
                wait(|s| s.txe())?;
                r.dr().write(|w| w.set_dr(b));
            }
            if !bytes.is_empty() {
                wait(|s| s.btf())?;
            }
            stop();
            Ok::<(), WireStatus>(())
        })();
        match sent {
            Ok(()) => WIRE_OK,
            Err(status) => status,
        }
    }

    fn read(&mut self, addr: u8, buf: &mut [u8]) -> usize {
        let r = pac::I2C1;
        let Some((last, head)) = buf.split_last_mut() else {
            return 0;
        };
        let received = (|| {
            // one byte: NACK before ADDR is cleared, then STOP (RM0368 §18.3.3)
            address(addr, true, head.is_empty())?;
            if head.is_empty() {
                r.cr1().modify(|w| w.set_ack(false));
                let _ = r.sr2().read();
            }
            for b in head.iter_mut() {
                wait(|s| s.rxne())?;
                *b = r.dr().read().dr();
            }
            // NACK and STOP after the last byte
            r.cr1().modify(|w| {
                w.set_ack(false);
                w.set_stop(true);
            });
            wait(|s| s.rxne())?;
            *last = r.dr().read().dr();
            Ok::<(), WireStatus>(())
        })();
        if received.is_ok() {
            buf.len()
        } else {
            0
        }
    }

    fn restart(&mut self) {
        I2cBus::restart(self);
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
