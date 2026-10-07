//! The part of robtillaart/I2C_EEPROM 1.9.4 (`I2C_eeprom.cpp`) the firmware uses, for its 24LC64
//! (8 KiB, 32-byte pages, two address bytes): `writeBlock`, `readBlock` and `isConnected` with
//! the ready polling behind them. `begin()` is never called by the firmware, so the time of the
//! last write starts at 0; the write protect pin, the extra write cycle time, the one-byte
//! addresses of small chips and the verify/update/size functions are not used.

use crate::hal::{Clock, I2cMaster, WireStatus};

/// I2C address of the controller's 24LC64 (A0, A1, A2 = GND)
pub const DEVICEADDRESS: u8 = 0x50;
/// `I2C_DEVICESIZE_24LC64`
pub const I2C_DEVICESIZE_24LC64: u32 = 8192;
/// page of the 24LC64 (`calculatePageSize(8192)`)
const PAGE_SIZE: u16 = 32;
/// `I2C_BUFFERSIZE` of AVR and STM: the Wire buffer of 32 bytes less the two address bytes
pub const I2C_BUFFERSIZE: u16 = 30;
/// the Wire transmit buffer (STM32duino `BUFFER_LENGTH`): two address bytes and a chunk
const WIRE_BUFFER_LENGTH: usize = 32;
/// `I2C_WRITEDELAY`: the write cycle of the chip (us); the device is polled within it
pub const I2C_WRITEDELAY: u32 = 5000;

/// Block access to the configuration EEPROM: the seam of `eeprom.rs` (the C++ suites link a
/// fake `I2C_eeprom`), implemented by [`I2cEeprom`].
pub trait EepromDevice {
    /// `writeBlock`: 0, or the Wire status of the first chunk that failed (the chunks before it
    /// are written)
    fn write_block(&mut self, memory_address: u16, buffer: &[u8]) -> i32;
    /// `readBlock`: the bytes read; a chunk that fails adds 0 and the read goes on
    fn read_block(&mut self, memory_address: u16, buffer: &mut [u8]) -> u16;
}

/// `I2C_eeprom eeprom(DEVICEADDRESS, EE24LC64MAXBYTES)` on the I2C master.
pub struct I2cEeprom<I, C> {
    i2c: I,
    clock: C,
    device_address: u8,
    /// micros() of the last chunk written
    last_write: u32,
}

impl<I: I2cMaster, C: Clock> I2cEeprom<I, C> {
    pub fn new(i2c: I, clock: C, device_address: u8) -> Self {
        I2cEeprom {
            i2c,
            clock,
            device_address,
            last_write: 0,
        }
    }

    /// The I2C master (the firmware restarts it before a retry, the tests inspect it).
    pub fn i2c(&mut self) -> &mut I {
        &mut self.i2c
    }

    /// The device acknowledges its address.
    pub fn is_connected(&mut self) -> bool {
        self.i2c.write(self.device_address, &[]) == 0
    }

    /// Splits the block at I2C_BUFFERSIZE and at the page boundaries; stops at the first chunk
    /// that fails.
    fn page_block(&mut self, memory_address: u16, buffer: &[u8]) -> i32 {
        let mut address = memory_address;
        let mut rest = buffer;
        while !rest.is_empty() {
            let bytes_until_page_boundary = PAGE_SIZE - address % PAGE_SIZE;
            let count = usize::from(I2C_BUFFERSIZE.min(bytes_until_page_boundary)).min(rest.len());
            let (chunk, tail) = rest.split_at(count);
            let rv = self.write_chunk(address, chunk);
            if rv != 0 {
                return i32::from(rv);
            }
            address = address.wrapping_add(count as u16);
            rest = tail;
        }
        0
    }

    /// `_WriteBlock`: the address bytes and up to I2C_BUFFERSIZE data bytes in one transfer,
    /// once the device answers again (or its write cycle is over).
    fn write_chunk(&mut self, memory_address: u16, chunk: &[u8]) -> WireStatus {
        self.wait_ee_ready();
        let mut frame = [0u8; WIRE_BUFFER_LENGTH];
        let [hi, lo] = memory_address.to_be_bytes();
        frame[0] = hi;
        frame[1] = lo;
        let n = chunk.len().min(I2C_BUFFERSIZE as usize);
        frame[2..2 + n].copy_from_slice(&chunk[..n]);
        let rv = self.i2c.write(self.device_address, &frame[..2 + n]);
        self.last_write = self.clock.micros();
        rv
    }

    /// `_ReadBlock`: the address bytes (STOP), then up to I2C_BUFFERSIZE bytes; 0 if the address
    /// is not acknowledged, else the bytes the device sent.
    fn read_chunk(&mut self, memory_address: u16, chunk: &mut [u8]) -> u16 {
        self.wait_ee_ready();
        if self
            .i2c
            .write(self.device_address, &memory_address.to_be_bytes())
            != 0
        {
            return 0;
        }
        self.i2c.read(self.device_address, chunk).min(chunk.len()) as u16
    }

    /// `_waitEEReady`: within I2C_WRITEDELAY after the last write the device is polled until
    /// it acknowledges (a chip in its write cycle does not); afterwards it is taken as ready.
    fn wait_ee_ready(&mut self) {
        while self.clock.micros().wrapping_sub(self.last_write) <= I2C_WRITEDELAY {
            if self.is_connected() {
                return;
            }
        }
    }
}

impl<I: I2cMaster, C: Clock> EepromDevice for I2cEeprom<I, C> {
    fn write_block(&mut self, memory_address: u16, buffer: &[u8]) -> i32 {
        self.page_block(memory_address, buffer)
    }

    fn read_block(&mut self, memory_address: u16, buffer: &mut [u8]) -> u16 {
        let mut address = memory_address;
        let mut bytes: u16 = 0;
        for chunk in buffer.chunks_mut(usize::from(I2C_BUFFERSIZE)) {
            bytes = bytes.wrapping_add(self.read_chunk(address, chunk));
            address = address.wrapping_add(chunk.len() as u16);
        }
        bytes
    }
}

#[cfg(test)]
mod tests;
