//! The 1.x configuration layout: 309 bytes at EEPROM address 0x0007, written by
//! firmware 1.x and 2.x in exactly this byte order (a downgrade to 1.4.x keeps
//! reading it), and the checks of its content. Hardware-free.
//!
//! Byte order: b_slave (1), descr (25), OneWireCfg (3), currentbound_low_fac (1),
//! currentbound_high_fac (1), numberOfMovements (2), owsensors1\[12\],
//! owsensors2\[12\], owsensors\[10\] (8 each: family, rom\[5\], rom\[4\], rom\[3\],
//! rom\[2\], rom\[1\], rom\[0\], crc), startOnPower (1), noOfMinCounts (2),
//! maxCalibRetries (1). Multi-byte fields little endian.

use crate::onewire_check::crc8;

/// == ACTUATOR_COUNT of the glue
pub const VALVE_COUNT: u8 = 12;
/// == ADDITIONAL_SENSOR_COUNT of the glue
pub const EXTRA_SENSOR_SLOTS: u8 = 10;
pub const LEGACY_IMAGE_SIZE: usize = 309;

const VALVES: usize = VALVE_COUNT as usize;
const EXTRA_SLOTS: usize = EXTRA_SENSOR_SLOTS as usize;

/// One 1-Wire sensor slot: the device address family, rom\[0..5\], crc (C++
/// `ds1820_eeprom_layout`, which the C++ core calls SensorSlot).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SensorSlot {
    /// address byte 0
    pub familycode: u8,
    /// address bytes 1..6
    pub romcode: [u8; 6],
    /// address byte 7
    pub crc: u8,
}

/// The 8 bytes of the device address in address order (the bytes of the C++ struct).
fn slot_address(s: &SensorSlot) -> [u8; 8] {
    let r = s.romcode;
    [s.familycode, r[0], r[1], r[2], r[3], r[4], r[5], s.crc]
}

/// Field names as in the 1.x RAM struct (snake_case), so the glue keeps its accesses like
/// eep_content.owsensors1\[x\].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LegacyLayout {
    /// 0 master, > 0 slave (not used)
    pub b_slave: u8,
    /// system description (not used)
    pub descr: [u8; 25],
    pub one_wire_cfg: [u8; 3],
    pub currentbound_low_fac: u8,
    pub currentbound_high_fac: u8,
    pub number_of_movements: u16,
    /// first sensor of each valve
    pub owsensors1: [SensorSlot; VALVES],
    /// second sensor of each valve
    pub owsensors2: [SensorSlot; VALVES],
    /// additional sensors (not used)
    pub owsensors: [SensorSlot; EXTRA_SLOTS],
    pub start_on_power: u8,
    pub no_of_min_counts: u16,
    pub max_calib_retries: u8,
}

/// Sequential writer over the image.
struct Put<'a> {
    out: &'a mut [u8],
    pos: usize,
}

impl Put<'_> {
    fn u8(&mut self, v: u8) {
        if let Some(b) = self.out.get_mut(self.pos) {
            *b = v;
        }
        self.pos += 1;
    }

    fn bytes(&mut self, v: &[u8]) {
        for &b in v {
            self.u8(b);
        }
    }

    fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }

    /// 1.x stores the rom code of a slot in reverse order
    fn slot(&mut self, s: &SensorSlot) {
        self.u8(s.familycode);
        for &b in s.romcode.iter().rev() {
            self.u8(b);
        }
        self.u8(s.crc);
    }
}

/// Sequential reader over the image.
struct Get<'a> {
    input: &'a [u8],
    pos: usize,
}

impl Get<'_> {
    fn u8(&mut self) -> u8 {
        let v = self.input.get(self.pos).copied().unwrap_or(0);
        self.pos += 1;
        v
    }

    fn bytes(&mut self, out: &mut [u8]) {
        for b in out {
            *b = self.u8();
        }
    }

    fn u16(&mut self) -> u16 {
        let lo = self.u8();
        u16::from_le_bytes([lo, self.u8()])
    }

    fn slot(&mut self, s: &mut SensorSlot) {
        s.familycode = self.u8();
        for b in s.romcode.iter_mut().rev() {
            *b = self.u8();
        }
        s.crc = self.u8();
    }
}

pub fn encode_legacy_layout(input: &LegacyLayout, out: &mut [u8; LEGACY_IMAGE_SIZE]) {
    let mut p = Put { out, pos: 0 };
    p.u8(input.b_slave);
    p.bytes(&input.descr);
    p.bytes(&input.one_wire_cfg);
    p.u8(input.currentbound_low_fac);
    p.u8(input.currentbound_high_fac);
    p.u16(input.number_of_movements);
    for s in &input.owsensors1 {
        p.slot(s);
    }
    for s in &input.owsensors2 {
        p.slot(s);
    }
    for s in &input.owsensors {
        p.slot(s);
    }
    p.u8(input.start_on_power);
    p.u16(input.no_of_min_counts);
    p.u8(input.max_calib_retries);
}

pub fn decode_legacy_layout(input: &[u8; LEGACY_IMAGE_SIZE], out: &mut LegacyLayout) {
    let mut p = Get { input, pos: 0 };
    out.b_slave = p.u8();
    p.bytes(&mut out.descr);
    p.bytes(&mut out.one_wire_cfg);
    out.currentbound_low_fac = p.u8();
    out.currentbound_high_fac = p.u8();
    out.number_of_movements = p.u16();
    for s in &mut out.owsensors1 {
        p.slot(s);
    }
    for s in &mut out.owsensors2 {
        p.slot(s);
    }
    for s in &mut out.owsensors {
        p.slot(s);
    }
    out.start_on_power = p.u8();
    out.no_of_min_counts = p.u16();
    out.max_calib_retries = p.u8();
}

/// CRC-16/CCITT-FALSE: polynomial 0x1021, initial value 0xFFFF, not reflected,
/// no final xor ("123456789" -> 0x29B1). Protects the 1.x layout (block A).
pub fn crc16_ccitt(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _bit in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// A slot that was never assigned (all bytes 0xFF), or one whose CRC-8 over
/// family and rom\[0..5\] matches its crc (the all-zero slot of a cleared
/// assignment included). Anything else is a torn write or a flipped bit.
pub fn sensor_slot_valid(s: &SensorSlot) -> bool {
    let address = slot_address(s);
    let erased = address.iter().all(|&b| b == 0xFF);
    erased || crc8(&address[..7]) == address[7]
}

#[cfg(test)]
mod tests;
