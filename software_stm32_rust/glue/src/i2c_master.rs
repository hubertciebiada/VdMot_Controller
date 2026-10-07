//! The I2C master behind `Wire` (I2C1 on PB6/PB7, 100 kHz) on the STM32F4 I2C v1 peripheral
//! (docs/rust/GLUE-DESIGN-STM.md §1.3): the register sequence of RM0368 §18.3.3 as blocking
//! polls, like the STM32 core's `endTransmission` / `requestFrom`. Each transfer has its own
//! START and STOP; every wait is bounded by `I2C_TIMEOUT_TICK` (100 ms).
//!
//! Not the embassy-stm32 0.6.0 driver: its v1 `blocking_write` of zero bytes (the address probe
//! of the EEPROM's ready polling) waits for BTF, which never comes without a data byte, and a
//! NACK leaves the bus without a STOP. The firmware implements [`I2cRegs`] on `pac::I2C1`; the
//! sequence and its decisions live here.

use crate::hal::{Clock, I2cMaster, WireStatus, WIRE_ERROR, WIRE_NACK, WIRE_OK, WIRE_TIMEOUT};
use crate::i2c_bus::{self, RecoveryPins, Wire};

/// SR1: start bit sent (master)
pub const SR1_SB: u16 = 1 << 0;
/// SR1: address sent and acknowledged (master)
pub const SR1_ADDR: u16 = 1 << 1;
/// SR1: byte transfer finished
pub const SR1_BTF: u16 = 1 << 2;
/// SR1: data register not empty (receiver)
pub const SR1_RXNE: u16 = 1 << 6;
/// SR1: data register empty (transmitter)
pub const SR1_TXE: u16 = 1 << 7;
/// SR1: misplaced START or STOP
pub const SR1_BERR: u16 = 1 << 8;
/// SR1: arbitration lost
pub const SR1_ARLO: u16 = 1 << 9;
/// SR1: acknowledge failure (NACK)
pub const SR1_AF: u16 = 1 << 10;
/// SR2: a transfer is on the bus
pub const SR2_BUSY: u16 = 1 << 1;
/// `I2C_TIMEOUT_TICK`: the longest wait of one phase (ms)
pub const I2C_TIMEOUT_MS: u32 = 100;

/// The registers of the I2C peripheral (firmware: `pac::I2C1`). Every call is one register
/// access, no decision.
pub trait I2cRegs {
    /// reads SR1
    fn sr1(&self) -> u16;
    /// reads SR2; after an SR1 read this clears ADDR
    fn sr2(&self) -> u16;
    /// writes 0 to the given SR1 flags (they are rc_w0), 1 to the others
    fn clear_sr1(&self, flags: u16);
    /// CR1: ACK as given, START
    fn start(&self, ack: bool);
    /// CR1: STOP
    fn stop(&self);
    /// CR1: ACK
    fn set_ack(&self, on: bool);
    fn write_dr(&self, byte: u8);
    fn read_dr(&self) -> u8;
}

/// The master on its registers and a clock for the timeouts.
pub struct I2cV1<R, C> {
    pub regs: R,
    pub clock: C,
}

impl<R: I2cRegs, C: Clock> I2cV1<R, C> {
    pub fn new(regs: R, clock: C) -> Self {
        I2cV1 { regs, clock }
    }

    /// Polls SR1 until a flag of `done` is set. A NACK clears AF and sends a STOP (2), a lost
    /// arbitration or a bus error clears its flag and sends a STOP (4), more than 100 ms send a
    /// STOP (5).
    fn wait(&self, done: u16) -> Result<(), WireStatus> {
        let start = self.clock.millis();
        loop {
            let sr1 = self.regs.sr1();
            if sr1 & SR1_AF != 0 {
                self.regs.clear_sr1(SR1_AF);
                self.regs.stop();
                return Err(WIRE_NACK);
            }
            if sr1 & (SR1_ARLO | SR1_BERR) != 0 {
                self.regs.clear_sr1(SR1_ARLO | SR1_BERR);
                self.regs.stop();
                return Err(WIRE_ERROR);
            }
            if sr1 & done != 0 {
                return Ok(());
            }
            if self.clock.millis().wrapping_sub(start) > I2C_TIMEOUT_MS {
                self.regs.stop();
                return Err(WIRE_TIMEOUT);
            }
        }
    }

    /// The bus free (else 4 after 100 ms, nothing sent), START, the address byte, ADDR. The SR2
    /// read that clears ADDR follows unless `keep_addr` (a one-byte read clears ACK first).
    fn address(&self, addr: u8, read: bool, keep_addr: bool) -> Result<(), WireStatus> {
        let start = self.clock.millis();
        while self.regs.sr2() & SR2_BUSY != 0 {
            if self.clock.millis().wrapping_sub(start) > I2C_TIMEOUT_MS {
                return Err(WIRE_ERROR);
            }
        }
        self.regs.start(read);
        self.wait(SR1_SB)?;
        self.regs.write_dr((addr << 1) + u8::from(read));
        self.wait(SR1_ADDR)?;
        if !keep_addr {
            self.regs.sr2();
        }
        Ok(())
    }

    fn write_bytes(&self, addr: u8, bytes: &[u8]) -> Result<(), WireStatus> {
        self.address(addr, false, false)?;
        for &b in bytes {
            self.wait(SR1_TXE)?;
            self.regs.write_dr(b);
        }
        if !bytes.is_empty() {
            self.wait(SR1_BTF)?;
        }
        self.regs.stop();
        Ok(())
    }

    /// ACK on for the address; one byte: ACK off before ADDR is cleared; more: the bytes but
    /// the last on RXNE. Then ACK off and STOP before the last byte (RM0368 §18.3.3).
    fn read_bytes(&self, addr: u8, buf: &mut [u8]) -> Result<(), WireStatus> {
        let Some((last, head)) = buf.split_last_mut() else {
            return Ok(());
        };
        let single = head.is_empty();
        self.address(addr, true, single)?;
        if single {
            self.regs.set_ack(false);
            self.regs.sr2();
        }
        for b in head.iter_mut() {
            self.wait(SR1_RXNE)?;
            *b = self.regs.read_dr();
        }
        self.regs.set_ack(false);
        self.regs.stop();
        self.wait(SR1_RXNE)?;
        *last = self.regs.read_dr();
        Ok(())
    }
}

impl<R: I2cRegs + RecoveryPins + Wire, C: Clock> I2cMaster for I2cV1<R, C> {
    fn write(&mut self, addr: u8, bytes: &[u8]) -> WireStatus {
        match self.write_bytes(addr, bytes) {
            Ok(()) => WIRE_OK,
            Err(status) => status,
        }
    }

    fn read(&mut self, addr: u8, buf: &mut [u8]) -> usize {
        match self.read_bytes(addr, buf) {
            Ok(()) => buf.len(),
            Err(_) => 0,
        }
    }

    /// `i2c_bus_restart`: the peripheral off, the bus freed, the peripheral on.
    fn restart(&mut self) {
        i2c_bus::restart(&mut self.regs, &self.clock);
    }
}

#[cfg(test)]
mod tests;
