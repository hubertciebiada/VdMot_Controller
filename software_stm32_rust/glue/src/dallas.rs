//! DallasTemperature 3.8.1 as vendored in `software_stm32/lib/Arduino-Temperature-Control-Library`
//! (patched: `begin()` stops after 64 search passes), the part the firmware uses: `begin`,
//! `getDeviceCount`, `validAddress`, `validFamily`, `getResolution`, `setWaitForConversion`,
//! `requestTemperatures`, `millisToWaitForConversion` and `getTemp` with the scratchpad checks
//! behind it. The other library state (`ds18Count`, alarms, the external pull-up, user data,
//! `setResolution`) is not used and not ported.

use crate::hal::{Clock, OneWireLine};
use crate::onewire::{crc8, DeviceAddress, OneWire};

// device families (byte 0 of the ROM)
pub const DS18S20MODEL: u8 = 0x10;
pub const DS18B20MODEL: u8 = 0x28;
pub const DS1822MODEL: u8 = 0x22;
pub const DS1825MODEL: u8 = 0x3B;
pub const DS28EA00MODEL: u8 = 0x42;

/// getTemp() of a device whose scratchpad cannot be read (1/128 degC)
pub const DEVICE_DISCONNECTED_RAW: i16 = -7040;

/// upper bound of search passes in begin(), each pass finds at most one device
pub const DALLAS_MAX_SEARCH_PASSES: u8 = 64;

// function commands
const STARTCONVO: u8 = 0x44;
const READSCRATCH: u8 = 0xBE;
const READPOWERSUPPLY: u8 = 0xB4;

// scratchpad locations
const TEMP_LSB: usize = 0;
const TEMP_MSB: usize = 1;
const CONFIGURATION: usize = 4;
const COUNT_REMAIN: usize = 6;
const COUNT_PER_C: usize = 7;
const SCRATCHPAD_CRC: usize = 8;

// configuration register values of the resolutions
const TEMP_9_BIT: u8 = 0x1F;
const TEMP_10_BIT: u8 = 0x3F;
const TEMP_11_BIT: u8 = 0x5F;
const TEMP_12_BIT: u8 = 0x7F;

/// the longest wait for a conversion when the device is polled (ms)
const MAX_CONVERSION_TIMEOUT: u32 = 750;

pub type ScratchPad = [u8; 9];

/// The library object (C++ `DallasTemperature sensors(&oneWire)`); the bus is passed to every
/// call that uses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DallasTemperature {
    devices: u8,
    parasite: bool,
    bit_resolution: u8,
    wait_for_conversion: bool,
}

impl Default for DallasTemperature {
    /// `setOneWire()`: no devices, 9 bit, waiting for conversions
    fn default() -> Self {
        DallasTemperature {
            devices: 0,
            parasite: false,
            bit_resolution: 9,
            wait_for_conversion: true,
        }
    }
}

impl DallasTemperature {
    /// Enumerates the bus: counts the devices with a valid ROM CRC, notes parasite power and
    /// the highest resolution of the DS18 family devices. At most DALLAS_MAX_SEARCH_PASSES
    /// passes (a disturbed bus can keep the search returning devices).
    pub fn begin<L: OneWireLine>(&mut self, ow: &mut OneWire<L>) {
        let mut address: DeviceAddress = [0; 8];
        ow.reset_search();
        self.devices = 0;
        for _ in 0..DALLAS_MAX_SEARCH_PASSES {
            if !ow.search(&mut address, true) {
                break;
            }
            if !Self::valid_address(&address) {
                continue;
            }
            self.devices += 1;
            if Self::valid_family(&address) {
                if !self.parasite && Self::read_power_supply(ow, &address) {
                    self.parasite = true;
                }
                let b = Self::get_resolution_of(ow, &address);
                self.bit_resolution = self.bit_resolution.max(b);
            }
        }
    }

    /// devices found by the last begin()
    pub fn get_device_count(&self) -> u8 {
        self.devices
    }

    /// the ROM CRC matches
    pub fn valid_address(address: &DeviceAddress) -> bool {
        crc8(&address[..7]) == address[7]
    }

    /// a temperature sensor family: DS18S20, DS18B20, DS1822, DS1825, DS28EA00
    pub fn valid_family(address: &DeviceAddress) -> bool {
        matches!(
            address[0],
            DS18S20MODEL | DS18B20MODEL | DS1822MODEL | DS1825MODEL | DS28EA00MODEL
        )
    }

    /// the global resolution: the highest of the devices found (9 at least)
    pub fn get_resolution(&self) -> u8 {
        self.bit_resolution
    }

    /// Resolution of one device: 12 for a DS18S20 (no configuration register), 9..12 from the
    /// configuration register, 0 if the device does not answer.
    pub fn get_resolution_of<L: OneWireLine>(ow: &mut OneWire<L>, address: &DeviceAddress) -> u8 {
        if address[0] == DS18S20MODEL {
            return 12;
        }
        let mut scratch_pad: ScratchPad = [0; 9];
        if Self::is_connected(ow, address, &mut scratch_pad) {
            match scratch_pad[CONFIGURATION] {
                TEMP_12_BIT => return 12,
                TEMP_11_BIT => return 11,
                TEMP_10_BIT => return 10,
                TEMP_9_BIT => return 9,
                _ => {}
            }
        }
        0
    }

    /// true: requestTemperatures() returns when the conversion is done; false: at once
    pub fn set_wait_for_conversion(&mut self, flag: bool) {
        self.wait_for_conversion = flag;
    }

    pub fn get_wait_for_conversion(&self) -> bool {
        self.wait_for_conversion
    }

    /// true if a device on the bus needs parasite power (found by begin())
    pub fn is_parasite_power_mode(&self) -> bool {
        self.parasite
    }

    /// Starts a conversion on every device (SKIP ROM, CONVERT T); with wait_for_conversion it
    /// returns when the conversion is complete.
    pub fn request_temperatures<L: OneWireLine, C: Clock>(
        &mut self,
        ow: &mut OneWire<L>,
        clock: &C,
    ) {
        ow.reset();
        ow.skip();
        ow.write(STARTCONVO);
        if !self.wait_for_conversion {
            return;
        }
        self.block_till_conversion_complete(ow, clock, self.bit_resolution);
    }

    /// Polls the bus until the devices report the end of the conversion (at most
    /// MAX_CONVERSION_TIMEOUT); on a parasite bus a polled device would drop its supply, so the
    /// worst-case time of the resolution is waited instead.
    fn block_till_conversion_complete<L: OneWireLine, C: Clock>(
        &self,
        ow: &mut OneWire<L>,
        clock: &C,
        bit_resolution: u8,
    ) {
        if !self.parasite {
            let start = clock.millis();
            while !ow.read_bit() && clock.millis().wrapping_sub(start) < MAX_CONVERSION_TIMEOUT {}
        } else {
            clock.delay_ms(u32::from(Self::millis_to_wait_for_conversion(
                bit_resolution,
            )));
        }
    }

    /// worst-case conversion time of a resolution (ms)
    pub fn millis_to_wait_for_conversion(bit_resolution: u8) -> u16 {
        match bit_resolution {
            9 => 94,
            10 => 188,
            11 => 375,
            _ => 750,
        }
    }

    /// Temperature of a device in 1/128 degC, DEVICE_DISCONNECTED_RAW if its scratchpad cannot
    /// be read (no presence, all zero, bad CRC).
    pub fn get_temp<L: OneWireLine>(ow: &mut OneWire<L>, address: &DeviceAddress) -> i16 {
        let mut scratch_pad: ScratchPad = [0; 9];
        if Self::is_connected(ow, address, &mut scratch_pad) {
            return Self::calculate_temperature(address, &scratch_pad);
        }
        DEVICE_DISCONNECTED_RAW
    }

    /// The scratchpad of the device was read: presence before and after, not all zero, CRC ok.
    pub fn is_connected<L: OneWireLine>(
        ow: &mut OneWire<L>,
        address: &DeviceAddress,
        scratch_pad: &mut ScratchPad,
    ) -> bool {
        let b = Self::read_scratch_pad(ow, address, scratch_pad);
        b && !Self::is_all_zeros(scratch_pad)
            && crc8(&scratch_pad[..8]) == scratch_pad[SCRATCHPAD_CRC]
    }

    /// READ SCRATCHPAD of one device; false if no device answers the reset before or after.
    pub fn read_scratch_pad<L: OneWireLine>(
        ow: &mut OneWire<L>,
        address: &DeviceAddress,
        scratch_pad: &mut ScratchPad,
    ) -> bool {
        // fail fast without a presence pulse
        if !ow.reset() {
            return false;
        }
        ow.select(address);
        ow.write(READSCRATCH);
        for b in scratch_pad.iter_mut() {
            *b = ow.read();
        }
        ow.reset()
    }

    /// true if the device needs parasite power: it answers READ
    /// POWER SUPPLY with 0
    fn read_power_supply<L: OneWireLine>(ow: &mut OneWire<L>, address: &DeviceAddress) -> bool {
        ow.reset();
        ow.select(address);
        ow.write(READPOWERSUPPLY);
        let parasite_mode = !ow.read_bit();
        ow.reset();
        parasite_mode
    }

    /// Fixed point 1/128 degC from the scratchpad. As C++: the register bytes go through int
    /// and are narrowed to int16_t (wrap); a DS18S20 with COUNT_PER_C != 0 gets the extended
    /// resolution (TEMP_READ - 0.25 + (COUNT_PER_C - COUNT_REMAIN) / COUNT_PER_C).
    pub fn calculate_temperature(address: &DeviceAddress, scratch_pad: &ScratchPad) -> i16 {
        // bits 11.. and 3..10 do not overlap: + is the C++ |
        let fp = (i32::from(scratch_pad[TEMP_MSB]) << 11) + (i32::from(scratch_pad[TEMP_LSB]) << 3);
        let mut fp_temperature = fp as i16;
        let count_per_c = i32::from(scratch_pad[COUNT_PER_C]);
        if address[0] == DS18S20MODEL && count_per_c != 0 {
            let count_remain = i32::from(scratch_pad[COUNT_REMAIN]);
            // fp_temperature & 0xfff0 is an int: the sign extension of the int16_t is masked away
            let base = (i32::from(fp_temperature) & 0xfff0) << 3;
            let fraction = ((count_per_c - count_remain) * 128) / count_per_c;
            fp_temperature = (base - 32 + fraction) as i16;
        }
        fp_temperature
    }

    fn is_all_zeros(scratch_pad: &ScratchPad) -> bool {
        scratch_pad.iter().all(|&b| b == 0)
    }
}

#[cfg(test)]
mod tests;
