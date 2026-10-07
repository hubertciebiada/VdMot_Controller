//! OneWire 2.3.7 as vendored in `software_stm32/lib/OneWire` (patched to drive the line open
//! drain): the ROM search, the CRC-8, the ROM select and the byte I/O over the bit slots of a
//! [`OneWireLine`]. The slot timing (reset 480/70/410 us, write 10/55 and 65/5 us, read 3/10/53
//! us, interrupts masked inside each window) belongs to the firmware's line.
//!
//! Not ported: `write(v, power)` keeps the line driven high after a byte for parasite devices;
//! with the open-drain output of the patch a high level is the released line, so `power` has no
//! effect and the Rust `write` has no such argument. `depower`, `target_search`,
//! `write_bytes`/`read_bytes` and the CRC-16 are not used by the firmware.

use crate::hal::OneWireLine;
use vdm_stm_core::onewire_check;

/// A ROM address: family code, 48-bit serial, CRC-8 (C++ `DeviceAddress`).
pub type DeviceAddress = [u8; 8];

/// ROM commands
const MATCH_ROM: u8 = 0x55;
const SKIP_ROM: u8 = 0xCC;
const SEARCH_ROM: u8 = 0xF0;
const ALARM_SEARCH: u8 = 0xEC;

/// The bus master: the line and the state of the ROM search.
pub struct OneWire<L> {
    line: L,
    rom_no: DeviceAddress,
    last_discrepancy: u8,
    last_device_flag: bool,
}

impl<L: OneWireLine> OneWire<L> {
    /// `OneWire(pin)`: the line is set up by the firmware; the search starts from the beginning.
    pub fn new(line: L) -> Self {
        OneWire {
            line,
            rom_no: [0; 8],
            last_discrepancy: 0,
            last_device_flag: false,
        }
    }

    /// The line (the firmware's pin, the tests' bus simulator).
    pub fn line(&mut self) -> &mut L {
        &mut self.line
    }

    /// Reset pulse; true if a device answered with a presence pulse.
    pub fn reset(&mut self) -> bool {
        self.line.reset()
    }

    pub fn write_bit(&mut self, v: bool) {
        self.line.write_bit(v);
    }

    pub fn read_bit(&mut self) -> bool {
        self.line.read_bit()
    }

    /// One byte, least significant bit first.
    pub fn write(&mut self, v: u8) {
        for bit in 0..8 {
            self.line.write_bit((v >> bit) & 1 != 0);
        }
    }

    /// One byte, least significant bit first.
    pub fn read(&mut self) -> u8 {
        let mut r = 0u8;
        for bit in 0..8 {
            if self.line.read_bit() {
                // distinct bits: + is the C++ |=
                r += 1 << bit;
            }
        }
        r
    }

    /// MATCH ROM: the device with this address answers the next function command.
    pub fn select(&mut self, rom: &DeviceAddress) {
        self.write(MATCH_ROM);
        for &b in rom {
            self.write(b);
        }
    }

    /// SKIP ROM: every device answers the next function command.
    pub fn skip(&mut self) {
        self.write(SKIP_ROM);
    }

    /// The next search starts from the beginning.
    pub fn reset_search(&mut self) {
        self.last_discrepancy = 0;
        self.last_device_flag = false;
        self.rom_no = [0; 8];
    }

    /// The 1-Wire search algorithm of the Dallas application note: finds the next device and
    /// copies its address to `new_addr`. False when no (further) device answers; the search
    /// then starts over. A device whose family code reads 0 counts as none. (C++ default for
    /// search_mode: true, the normal search; false is the alarm search.)
    pub fn search(&mut self, new_addr: &mut DeviceAddress, search_mode: bool) -> bool {
        let mut id_bit_number: u8 = 1;
        let mut last_zero: u8 = 0;
        let mut search_result = false;

        // if the last call was not the last one
        if !self.last_device_flag {
            if !self.line.reset() {
                self.restart_search();
                return false;
            }
            self.write(if search_mode {
                SEARCH_ROM
            } else {
                ALARM_SEARCH
            });
            'rom: for byte in self.rom_no.iter_mut() {
                for bit in 0..8 {
                    let mask = 1u8 << bit;
                    // a bit and its complement
                    let id_bit = self.line.read_bit();
                    let cmp_id_bit = self.line.read_bit();
                    // no device on the bus (or none left)
                    if id_bit && cmp_id_bit {
                        break 'rom;
                    }
                    let direction = if id_bit != cmp_id_bit {
                        // every device still in the search has this bit
                        id_bit
                    } else {
                        // a discrepancy: before the last one take the same way as last time,
                        // at it take 1, after it 0
                        let d = if id_bit_number < self.last_discrepancy {
                            *byte & mask != 0
                        } else {
                            id_bit_number == self.last_discrepancy
                        };
                        if !d {
                            last_zero = id_bit_number;
                        }
                        d
                    };
                    if direction {
                        *byte |= mask;
                    } else {
                        *byte &= !mask;
                    }
                    self.line.write_bit(direction);
                    id_bit_number += 1;
                }
            }
            // all 64 bits read
            if id_bit_number == 65 {
                self.last_discrepancy = last_zero;
                if last_zero == 0 {
                    self.last_device_flag = true;
                }
                search_result = true;
            }
        }

        if !search_result || self.rom_no[0] == 0 {
            self.restart_search();
            return false;
        }
        *new_addr = self.rom_no;
        true
    }

    /// The state of the search after its end (the ROM bits of the last search stay).
    fn restart_search(&mut self) {
        self.last_discrepancy = 0;
        self.last_device_flag = false;
    }
}

/// `OneWire::crc8`: the Dallas CRC-8 of `addr` (the library's 2x16 table gives the same values
/// as the bitwise CRC of vdm-stm-core).
pub fn crc8(addr: &[u8]) -> u8 {
    onewire_check::crc8(addr)
}

#[cfg(test)]
mod tests;
