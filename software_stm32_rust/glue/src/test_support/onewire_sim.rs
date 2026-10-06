//! A 1-Wire bus at the bit level (the tests of onewire, dallas, ds2438 and the real bus of
//! ow_devices): devices answer the ROM layer (search, match, skip) on the wired-AND line and the
//! function commands of the DS18x20 and DS2438 that the firmware uses.

use std::collections::VecDeque;

use crate::hal::OneWireLine;
use vdm_stm_core::onewire_check::crc8;

/// What a device answers beyond the ROM layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// DS18B20, DS1822, DS1825, DS28EA00, DS18S20: 9-byte scratchpad
    Ds18,
    /// DS2438: 8 memory pages of 8 bytes and their scratchpad copies
    Ds2438,
    /// another device: ROM layer only
    Other,
}

#[derive(Clone, Debug)]
pub struct SimDevice {
    pub rom: [u8; 8],
    pub kind: Kind,
    pub present: bool,
    /// DS18: answers READ POWER SUPPLY with 0
    pub parasite: bool,
    /// DS18: scratchpad bytes 0..7 (the CRC is computed when it is read)
    pub scratch: [u8; 8],
    /// DS2438: memory and scratchpad pages
    pub pages: [[u8; 8]; 8],
    pub scratch_pages: [[u8; 8]; 8],
    /// DS2438: VAD and VDD in 10 mV
    pub vad: u16,
    pub vdd: u16,
    /// the next reads of a scratchpad arrive with a wrong CRC
    pub bad_crc_reads: u32,
    /// read slots after CONVERT T that read 0 (conversion running)
    pub busy_reads: u32,
    /// CONVERT T and CONVERT V commands received
    pub converts: u32,
    /// DS2438 WRITE SCRATCHPAD and COPY SCRATCHPAD commands received
    pub scratch_writes: u32,
    pub copies: u32,
}

impl SimDevice {
    /// A device with a valid ROM: family, serial, 0, 0, 0, 0, 0, CRC.
    pub fn new(family: u8, serial: u8, kind: Kind) -> Self {
        let mut rom = [family, serial, 0, 0, 0, 0, 0, 0];
        rom[7] = crc8(&rom[..7]);
        SimDevice {
            rom,
            kind,
            present: true,
            parasite: false,
            scratch: [0x50, 0x05, 0x4B, 0x46, 0x7F, 0xFF, 0x0C, 0x10],
            pages: [[0; 8]; 8],
            scratch_pages: [[0; 8]; 8],
            vad: 0,
            vdd: 0,
            bad_crc_reads: 0,
            busy_reads: 0,
            converts: 0,
            scratch_writes: 0,
            copies: 0,
        }
    }

    /// A DS18B20 (family 0x28) with this temperature register (1/16 degC) and 12-bit
    /// resolution.
    pub fn ds18b20(serial: u8, raw16: i16) -> Self {
        let mut d = SimDevice::new(0x28, serial, Kind::Ds18);
        d.set_temp_register(raw16);
        d
    }

    pub fn set_temp_register(&mut self, raw: i16) {
        let [lsb, msb] = raw.to_le_bytes();
        self.scratch[0] = lsb;
        self.scratch[1] = msb;
    }

    /// A DS2438 (family 0x26) measuring this VAD (10 mV).
    pub fn ds2438(serial: u8, vad: u16) -> Self {
        let mut d = SimDevice::new(0x26, serial, Kind::Ds2438);
        d.vad = vad;
        d.pages[0][0] = 0x0F;
        d.scratch_pages = d.pages;
        d
    }

    fn scratchpad_bytes(&mut self, data: [u8; 8]) -> [u8; 9] {
        let mut out = [0u8; 9];
        out[..8].copy_from_slice(&data);
        out[8] = crc8(&data);
        if self.bad_crc_reads > 0 {
            self.bad_crc_reads -= 1;
            out[8] ^= 0xFF;
        }
        out
    }
}

/// What the master did on the bus, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// a reset pulse and whether a device answered
    Reset(bool),
    /// a byte written in a byte phase (commands, addresses, arguments, data)
    Write(u8),
    /// the direction bit written at a search position
    Direction(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// after a function command without arguments: writes are ignored
    Idle,
    RomCommand,
    MatchRom,
    /// position 0..63; step 0 reads the bit, 1 its complement, 2 takes the direction
    Search {
        pos: u8,
        step: u8,
    },
    Function,
    /// collecting the arguments of a DS2438 command
    Arg {
        cmd: u8,
    },
}

pub struct OneWireSim {
    pub devices: Vec<SimDevice>,
    /// a shorted bus: no presence, every slot reads 0
    pub held_low: bool,
    pub events: Vec<Event>,
    pub reads: u32,
    phase: Phase,
    active: Vec<bool>,
    byte: u8,
    bits: u8,
    args: Vec<u8>,
    match_rom: Vec<u8>,
    queues: Vec<VecDeque<bool>>,
}

impl OneWireSim {
    pub fn new(devices: Vec<SimDevice>) -> Self {
        let n = devices.len();
        OneWireSim {
            devices,
            held_low: false,
            events: Vec::new(),
            reads: 0,
            phase: Phase::Idle,
            active: vec![false; n],
            byte: 0,
            bits: 0,
            args: Vec::new(),
            match_rom: Vec::new(),
            queues: vec![VecDeque::new(); n],
        }
    }

    /// The bytes written since the last reset that answered.
    pub fn writes(&self) -> Vec<u8> {
        self.events
            .iter()
            .filter_map(|e| match e {
                Event::Write(b) => Some(*b),
                _ => None,
            })
            .collect()
    }

    fn sync_len(&mut self) {
        let n = self.devices.len();
        self.active.resize(n, false);
        self.queues.resize(n, VecDeque::new());
    }

    fn queue_bytes(&mut self, i: usize, bytes: &[u8]) {
        for &b in bytes {
            for bit in 0..8 {
                self.queues[i].push_back(b >> bit & 1 != 0);
            }
        }
    }

    fn on_byte(&mut self, b: u8) {
        self.events.push(Event::Write(b));
        match self.phase {
            Phase::RomCommand => match b {
                0xF0 | 0xEC => self.phase = Phase::Search { pos: 0, step: 0 },
                0x55 => {
                    self.match_rom.clear();
                    self.phase = Phase::MatchRom;
                }
                0xCC => self.phase = Phase::Function,
                _ => self.phase = Phase::Idle,
            },
            Phase::MatchRom => {
                self.match_rom.push(b);
                if self.match_rom.len() == 8 {
                    for (i, d) in self.devices.iter().enumerate() {
                        self.active[i] = self.active[i] && d.rom[..] == self.match_rom[..];
                    }
                    self.phase = Phase::Function;
                }
            }
            Phase::Function => self.on_function(b),
            Phase::Arg { cmd } => {
                self.args.push(b);
                self.on_arg(cmd);
            }
            Phase::Idle | Phase::Search { .. } => {}
        }
    }

    fn on_function(&mut self, cmd: u8) {
        self.phase = Phase::Idle;
        for i in 0..self.devices.len() {
            if !self.active[i] {
                continue;
            }
            let kind = self.devices[i].kind;
            match (kind, cmd) {
                (Kind::Ds18, 0x44) => {
                    self.devices[i].converts += 1;
                    let busy = self.devices[i].busy_reads;
                    for _ in 0..busy {
                        self.queues[i].push_back(false);
                    }
                }
                (Kind::Ds18, 0xBE) => {
                    let data = self.devices[i].scratch;
                    let bytes = self.devices[i].scratchpad_bytes(data);
                    self.queue_bytes(i, &bytes);
                }
                (Kind::Ds18, 0xB4) => {
                    let parasite = self.devices[i].parasite;
                    self.queues[i].push_back(!parasite);
                }
                (Kind::Ds2438, 0xB8 | 0xBE | 0x4E | 0x48) => {
                    self.args.clear();
                    self.phase = Phase::Arg { cmd };
                }
                (Kind::Ds2438, 0xB4) => {
                    let d = &mut self.devices[i];
                    d.converts += 1;
                    let v = if d.pages[0][0] & 0x08 != 0 {
                        d.vdd
                    } else {
                        d.vad
                    };
                    let [lo, hi] = v.to_le_bytes();
                    d.pages[0][3] = lo;
                    d.pages[0][4] = (d.pages[0][4] & !0x03) | (hi & 0x03);
                }
                _ => self.active[i] = false,
            }
        }
    }

    fn on_arg(&mut self, cmd: u8) {
        let page = usize::from(self.args[0] & 0x07);
        if cmd == 0x4E && self.args.len() < 9 {
            return;
        }
        self.phase = Phase::Idle;
        for i in 0..self.devices.len() {
            if !self.active[i] {
                continue;
            }
            match cmd {
                0xB8 => self.devices[i].scratch_pages[page] = self.devices[i].pages[page],
                0xBE => {
                    let data = self.devices[i].scratch_pages[page];
                    let bytes = self.devices[i].scratchpad_bytes(data);
                    self.queue_bytes(i, &bytes);
                }
                0x4E => {
                    let d = &mut self.devices[i];
                    d.scratch_writes += 1;
                    d.scratch_pages[page].copy_from_slice(&self.args[1..9]);
                }
                _ => {
                    let d = &mut self.devices[i];
                    d.copies += 1;
                    d.pages[page] = d.scratch_pages[page];
                }
            }
        }
    }

    fn rom_bit(d: &SimDevice, pos: u8) -> bool {
        d.rom[usize::from(pos / 8)] >> (pos % 8) & 1 != 0
    }
}

impl OneWireLine for OneWireSim {
    fn reset(&mut self) -> bool {
        self.sync_len();
        let presence = !self.held_low && self.devices.iter().any(|d| d.present);
        self.events.push(Event::Reset(presence));
        for (i, d) in self.devices.iter().enumerate() {
            self.active[i] = d.present;
            self.queues[i].clear();
        }
        self.byte = 0;
        self.bits = 0;
        self.phase = if presence {
            Phase::RomCommand
        } else {
            Phase::Idle
        };
        presence
    }

    fn write_bit(&mut self, bit: bool) {
        if self.held_low {
            return;
        }
        if let Phase::Search { pos, step: 2 } = self.phase {
            self.events.push(Event::Direction(bit));
            for (i, d) in self.devices.iter().enumerate() {
                self.active[i] = self.active[i] && Self::rom_bit(d, pos) == bit;
            }
            self.phase = if pos == 63 {
                Phase::Idle
            } else {
                Phase::Search {
                    pos: pos + 1,
                    step: 0,
                }
            };
            return;
        }
        if bit {
            self.byte |= 1 << self.bits;
        }
        self.bits += 1;
        if self.bits == 8 {
            let b = self.byte;
            self.byte = 0;
            self.bits = 0;
            self.on_byte(b);
        }
    }

    fn read_bit(&mut self) -> bool {
        self.reads += 1;
        if self.held_low {
            return false;
        }
        if let Phase::Search { pos, step } = self.phase {
            if step == 2 {
                return true;
            }
            let mut level = true;
            for (i, d) in self.devices.iter().enumerate() {
                if self.active[i] {
                    let b = Self::rom_bit(d, pos);
                    level &= if step == 0 { b } else { !b };
                }
            }
            self.phase = Phase::Search {
                pos,
                step: step + 1,
            };
            return level;
        }
        let mut level = true;
        for i in 0..self.devices.len() {
            if self.active[i] {
                if let Some(b) = self.queues[i].pop_front() {
                    level &= b;
                }
            }
        }
        level
    }
}
