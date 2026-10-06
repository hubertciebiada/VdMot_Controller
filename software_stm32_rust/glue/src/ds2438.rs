//! The DS2438 battery monitor (`software_stm32/src/DS2438.cpp`, Rob Tillaart's library 0.1.1 as
//! changed for VdMot: presence pulses and the scratchpad CRC are checked, an all-zero page is
//! rejected, a corrupted read is never written back), the part the firmware uses: `begin`,
//! `setAddress` and `readVAD` with the configuration bit and scratchpad page I/O behind it.

use crate::hal::{Clock, OneWireLine};
use crate::onewire::{crc8, DeviceAddress, OneWire};
use vdm_stm_core::onewire_check::is_valid_scratchpad;

/// readVAD() of a page that could not be read (V)
pub const DS2438_VAD_INVALID: f32 = -10.0;
/// C++ default for begin(retries): 3
pub const DS2438_BEGIN_RETRIES: u8 = 3;

// function commands
const DS2438_READ_VOLTAGE: u8 = 0xB4;
const DS2438_RECALL_SCRATCH: u8 = 0xB8;
const DS2438_READ_SCRATCH: u8 = 0xBE;
const DS2438_WRITE_SCRATCH: u8 = 0x4E;
const DS2438_COPY_SCRATCH: u8 = 0x48;

/// wait for a voltage conversion (ms)
const DS2438_CONVERSION_DELAY: u32 = 10;
/// configuration register bit AD: 1 converts VDD, 0 converts VAD
const DS2438_CFG_AD: u8 = 3;
/// the page of the configuration register and of the voltage
const PAGE_0: u8 = 0;

/// The library object (C++ `DS2438 bm(&oneWire)`); the bus is passed to every call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ds2438 {
    scratch_pad: [u8; 9],
    address: DeviceAddress,
}

impl Ds2438 {
    /// `isConnected(retries)`: up to `retries` times reset, restart the search and take the
    /// first device on the bus as the address; true once that address is not 0 and its ROM CRC
    /// matches. (Any family: the firmware sets the address of each monitor before a read.)
    pub fn begin<L: OneWireLine>(&mut self, ow: &mut OneWire<L>, retries: u8) -> bool {
        let mut address_found = false;
        for _ in 0..retries {
            if address_found {
                break;
            }
            ow.reset();
            ow.reset_search();
            self.address[0] = 0x00;
            ow.search(&mut self.address, true);
            address_found = self.address[0] != 0x00 && crc8(&self.address[..7]) == self.address[7];
        }
        address_found
    }

    pub fn set_address(&mut self, address: &DeviceAddress) {
        self.address = *address;
    }

    /// The device the next calls talk to.
    pub fn address(&self) -> &DeviceAddress {
        &self.address
    }

    /// Converts the VAD input and returns it in V (10 mV steps; the C++ computes in double and
    /// stores a float), DS2438_VAD_INVALID if the page cannot be read or its bytes 0..3 are all
    /// 0xFF.
    pub fn read_vad<L: OneWireLine, C: Clock>(&mut self, ow: &mut OneWire<L>, clock: &C) -> f32 {
        self.clear_config_bit(ow, DS2438_CFG_AD);

        // request the voltage
        ow.reset();
        ow.select(&self.address);
        ow.write(DS2438_READ_VOLTAGE);
        clock.delay_ms(DS2438_CONVERSION_DELAY);
        let valid = self.read_scratch_pad(ow);
        if !valid || !self.check_scratchpad() {
            return DS2438_VAD_INVALID;
        }
        let raw = u32::from(self.scratch_pad[4] & 0x03) * 256 + u32::from(self.scratch_pad[3]);
        (f64::from(raw) * 0.01) as f32
    }

    /// false if bytes 0..3 of the scratchpad are all 0xFF (a bus without the device)
    fn check_scratchpad(&self) -> bool {
        self.scratch_pad[..4].iter().any(|&b| b != 0xFF)
    }

    /// Clears a bit of the configuration register (page 0, byte 0); a page that cannot be read
    /// is never written back (the write is copied to the device EEPROM).
    fn clear_config_bit<L: OneWireLine>(&mut self, ow: &mut OneWire<L>, bit: u8) {
        let mask = 1u8 << bit;
        if !self.read_scratch_pad(ow) {
            return;
        }
        // already 0
        if self.scratch_pad[0] & mask == 0 {
            return;
        }
        self.scratch_pad[0] &= !mask;
        self.write_scratch_pad(ow);
    }

    /// Reads page 0 with its CRC byte; false if no device answered the resets or the page is
    /// corrupted (bad CRC, or all zero from a bus held low). (C++ `readScratchPad(page)`: the
    /// firmware reads page 0 only.)
    fn read_scratch_pad<L: OneWireLine>(&mut self, ow: &mut OneWire<L>) -> bool {
        let mut present = ow.reset();
        ow.select(&self.address);
        ow.write(DS2438_RECALL_SCRATCH);
        ow.write(PAGE_0);
        present = ow.reset() && present;
        ow.select(&self.address);
        ow.write(DS2438_READ_SCRATCH);
        ow.write(PAGE_0);
        for b in self.scratch_pad.iter_mut() {
            *b = ow.read();
        }
        present && is_valid_scratchpad(&self.scratch_pad)
    }

    /// Writes bytes 0..7 of the scratchpad to page 0 and copies it to the device memory. (C++
    /// `writeScratchPad(page)`: the firmware writes page 0 only.)
    fn write_scratch_pad<L: OneWireLine>(&mut self, ow: &mut OneWire<L>) {
        ow.reset();
        ow.select(&self.address);
        ow.write(DS2438_WRITE_SCRATCH);
        ow.write(PAGE_0);
        for &b in &self.scratch_pad[..8] {
            ow.write(b);
        }
        ow.reset();
        ow.select(&self.address);
        ow.write(DS2438_COPY_SCRATCH);
        ow.write(PAGE_0);
    }
}

#[cfg(test)]
mod tests;
