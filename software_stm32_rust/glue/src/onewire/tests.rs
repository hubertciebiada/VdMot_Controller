// New tests (no C++ counterpart, docs/rust/GLUE-DESIGN-STM.md §7.3): the byte I/O, select and
// skip, the ROM search order on the bit-level bus simulator and its edge cases, the CRC-8.

use std::collections::VecDeque;

use super::*;
use crate::test_support::onewire_sim::{Event, Kind, OneWireSim, SimDevice};

fn bus(devices: Vec<SimDevice>) -> OneWire<OneWireSim> {
    OneWire::new(OneWireSim::new(devices))
}

/// Order of the search: at every discrepancy the 0 branch first, the ROM bits from bit 0 of
/// byte 0 on, so ascending by the bit-reversed 64-bit ROM.
fn search_order(roms: &[[u8; 8]]) -> Vec<[u8; 8]> {
    let mut v = roms.to_vec();
    v.sort_by_key(|r| u64::from_le_bytes(*r).reverse_bits());
    v
}

fn search_all(ow: &mut OneWire<OneWireSim>) -> Vec<[u8; 8]> {
    let mut found = Vec::new();
    let mut a = [0u8; 8];
    ow.reset_search();
    for _ in 0..100 {
        if !ow.search(&mut a, true) {
            break;
        }
        found.push(a);
    }
    found
}

/// A line that answers resets and reads from scripts and records what was written.
struct ScriptLine {
    resets: VecDeque<bool>,
    reads: VecDeque<bool>,
    written: Vec<bool>,
}

impl ScriptLine {
    fn new(resets: &[bool], reads: &[bool]) -> Self {
        ScriptLine {
            resets: resets.iter().copied().collect(),
            reads: reads.iter().copied().collect(),
            written: Vec::new(),
        }
    }
}

impl OneWireLine for ScriptLine {
    fn reset(&mut self) -> bool {
        self.resets.pop_front().unwrap_or(false)
    }

    fn write_bit(&mut self, bit: bool) {
        self.written.push(bit);
    }

    fn read_bit(&mut self) -> bool {
        self.reads.pop_front().unwrap_or(true)
    }
}

/// The 64 search reads of one device: each ROM bit and its complement.
fn search_reads(rom: &[u8; 8]) -> Vec<bool> {
    let mut v = Vec::new();
    for pos in 0..64 {
        let b = rom[pos / 8] >> (pos % 8) & 1 != 0;
        v.push(b);
        v.push(!b);
    }
    v
}

fn bits_of(b: u8) -> Vec<bool> {
    (0..8).map(|i| b >> i & 1 != 0).collect()
}

#[test]
fn write_and_read_send_the_least_significant_bit_first() {
    let mut ow = OneWire::new(ScriptLine::new(&[], &bits_of(0xB4)));
    ow.write(0xA5);
    assert_eq!(ow.line().written, bits_of(0xA5));
    assert_eq!(ow.read(), 0xB4);
    // an idle line reads 1s
    assert_eq!(ow.read(), 0xFF);
}

#[test]
fn reset_write_bit_and_read_bit_pass_through_to_the_line() {
    let mut ow = OneWire::new(ScriptLine::new(&[true, false], &[false, true]));
    assert!(ow.reset());
    assert!(!ow.reset());
    ow.write_bit(true);
    ow.write_bit(false);
    assert_eq!(ow.line().written, vec![true, false]);
    assert!(!ow.read_bit());
    assert!(ow.read_bit());
}

#[test]
fn select_writes_match_rom_and_the_address_skip_writes_skip_rom() {
    let d = SimDevice::ds18b20(1, 0);
    let rom = d.rom;
    let mut ow = bus(vec![d]);
    assert!(ow.reset());
    ow.select(&rom);
    ow.reset();
    ow.skip();
    let mut expected = vec![0x55];
    expected.extend_from_slice(&rom);
    expected.push(0xCC);
    assert_eq!(ow.line().writes(), expected);
}

#[test]
fn search_finds_every_device_once_in_rom_bit_order_then_starts_over() {
    let devices = vec![
        SimDevice::new(0x28, 0x01, Kind::Ds18),
        SimDevice::new(0x26, 0x02, Kind::Ds2438),
        SimDevice::new(0x10, 0x03, Kind::Ds18),
        SimDevice::new(0x28, 0x81, Kind::Ds18),
        SimDevice::new(0x28, 0x02, Kind::Ds18),
        SimDevice::new(0x01, 0xFF, Kind::Other),
    ];
    let roms: Vec<[u8; 8]> = devices.iter().map(|d| d.rom).collect();
    let mut ow = bus(devices);
    let found = search_all(&mut ow);
    assert_eq!(found, search_order(&roms));
    // the search ended with false and starts over from the first device
    let mut a = [0u8; 8];
    assert!(ow.search(&mut a, true));
    assert_eq!(a, found[0]);
    assert!(ow.search(&mut a, true));
    assert_eq!(a, found[1]);
}

#[test]
fn search_after_the_last_device_returns_false_once_without_bus_traffic() {
    let d = SimDevice::new(0x28, 0x05, Kind::Ds18);
    let rom = d.rom;
    let mut ow = bus(vec![d]);
    let mut a = [0u8; 8];
    assert!(ow.search(&mut a, true));
    assert_eq!(a, rom);
    let events = ow.line().events.len();
    a = [9; 8];
    assert!(!ow.search(&mut a, true));
    // the last device was found: no reset, the address untouched
    assert_eq!(ow.line().events.len(), events);
    assert_eq!(a, [9; 8]);
    assert!(ow.search(&mut a, true));
    assert_eq!(a, rom);
}

#[test]
fn reset_search_starts_the_next_search_at_the_first_device_again() {
    let devices = vec![
        SimDevice::new(0x28, 0x01, Kind::Ds18),
        SimDevice::new(0x28, 0x02, Kind::Ds18),
    ];
    let roms: Vec<[u8; 8]> = devices.iter().map(|d| d.rom).collect();
    let order = search_order(&roms);
    let mut ow = bus(devices);
    let mut a = [0u8; 8];
    // within a search: the reset forgets the discrepancy, the first device comes again
    assert!(ow.search(&mut a, true));
    assert_eq!(a, order[0]);
    ow.reset_search();
    assert!(ow.search(&mut a, true));
    assert_eq!(a, order[0]);
    // without one the search goes on with the next device
    assert!(ow.search(&mut a, true));
    assert_eq!(a, order[1]);
    // after the last device: the reset forgets that it was the last one
    ow.reset_search();
    assert!(ow.search(&mut a, true));
    assert_eq!(a, order[0]);
}

#[test]
fn search_without_a_presence_pulse_finds_nothing_and_restarts() {
    let mut d = SimDevice::new(0x28, 0x05, Kind::Ds18);
    d.present = false;
    let mut ow = bus(vec![d]);
    let mut a = [7u8; 8];
    assert!(!ow.search(&mut a, true));
    assert_eq!(a, [7; 8]);
    assert_eq!(ow.line().events, vec![Event::Reset(false)]);
    ow.line().devices[0].present = true;
    assert!(ow.search(&mut a, true));
    assert_eq!(a[1], 0x05);
}

#[test]
fn search_of_two_devices_after_a_failed_reset_starts_at_the_first() {
    let devices = vec![
        SimDevice::new(0x28, 0x01, Kind::Ds18),
        SimDevice::new(0x28, 0x02, Kind::Ds18),
    ];
    let roms: Vec<[u8; 8]> = devices.iter().map(|d| d.rom).collect();
    let order = search_order(&roms);
    let mut ow = bus(devices);
    let mut a = [0u8; 8];
    assert!(ow.search(&mut a, true));
    assert_eq!(a, order[0]);
    // the bus drops out: the search state is reset
    ow.line().held_low = true;
    assert!(!ow.search(&mut a, true));
    ow.line().held_low = false;
    assert!(ow.search(&mut a, true));
    assert_eq!(a, order[0]);
    assert!(ow.search(&mut a, true));
    assert_eq!(a, order[1]);
    assert!(!ow.search(&mut a, true));
}

#[test]
fn search_mode_false_sends_the_alarm_search() {
    let mut ow = OneWire::new(ScriptLine::new(&[true], &[true, true]));
    let mut a = [0u8; 8];
    assert!(!ow.search(&mut a, false));
    assert_eq!(ow.line().written, bits_of(0xEC));
    let mut ow = OneWire::new(ScriptLine::new(&[true], &[true, true]));
    assert!(!ow.search(&mut a, true));
    assert_eq!(ow.line().written, bits_of(0xF0));
}

#[test]
fn search_ending_in_the_middle_of_the_rom_finds_nothing_and_restarts() {
    // a device answers 10 bits, then the line reads 1 and 1
    let rom = SimDevice::new(0x28, 0x01, Kind::Ds18).rom;
    let mut reads = search_reads(&rom);
    reads.truncate(20);
    reads.extend_from_slice(&[true, true]);
    let mut ow = OneWire::new(ScriptLine::new(&[true, true], &reads));
    let mut a = [3u8; 8];
    assert!(!ow.search(&mut a, true));
    assert_eq!(a, [3; 8]);
    // 8 command bits and 10 direction bits written
    assert_eq!(ow.line().written.len(), 18);
    // the next search starts with a reset again (it was not the last device)
    ow.line().reads = search_reads(&rom).into_iter().collect();
    assert!(ow.search(&mut a, true));
    assert_eq!(a, rom);
}

#[test]
fn search_takes_a_family_code_0_as_no_device() {
    // a device of family 0: the search reads its 64 bits, then reports nothing
    let mut rom = [0u8, 0x12, 0, 0, 0, 0, 0, 0];
    rom[7] = crc8(&rom[..7]);
    let mut ow = OneWire::new(ScriptLine::new(&[true, true], &search_reads(&rom)));
    let mut a = [5u8; 8];
    assert!(!ow.search(&mut a, true));
    assert_eq!(a, [5; 8]);
    // not taken as the last device: the next search resets the bus again
    let other = SimDevice::new(0x28, 0x07, Kind::Ds18).rom;
    ow.line().reads = search_reads(&other).into_iter().collect();
    assert!(ow.search(&mut a, true));
    assert_eq!(a, other);
}

#[test]
fn search_directions_follow_the_last_discrepancy() {
    // two devices differing in bit 9 (byte 1, bit 1) and bit 20 (byte 2, bit 4)
    let a_rom = [0x28, 0x00, 0x00, 0, 0, 0, 0, 0];
    let b_rom = [0x28, 0x02, 0x00, 0, 0, 0, 0, 0];
    let c_rom = [0x28, 0x02, 0x10, 0, 0, 0, 0, 0];
    let mk = |r: [u8; 8]| {
        let mut d = SimDevice::new(r[0], r[1], Kind::Ds18);
        d.rom = r;
        d.rom[7] = crc8(&d.rom[..7]);
        d
    };
    let devices = vec![mk(c_rom), mk(b_rom), mk(a_rom)];
    let roms: Vec<[u8; 8]> = devices.iter().map(|d| d.rom).collect();
    let mut ow = bus(devices);
    let found = search_all(&mut ow);
    assert_eq!(found, search_order(&roms));
    assert_eq!(found.len(), 3);
    assert_eq!(found[0][1], 0x00);
    assert_eq!(found[1][1], 0x02);
    assert_eq!(found[1][2], 0x00);
    assert_eq!(found[2][2], 0x10);
}

#[test]
fn search_with_many_devices_finds_each_once() {
    let devices: Vec<SimDevice> = (0..40u8)
        .map(|i| {
            SimDevice::new(
                if i % 3 == 0 { 0x26 } else { 0x28 },
                i.wrapping_mul(37),
                Kind::Ds18,
            )
        })
        .collect();
    let roms: Vec<[u8; 8]> = devices.iter().map(|d| d.rom).collect();
    let mut ow = bus(devices);
    let found = search_all(&mut ow);
    assert_eq!(found, search_order(&roms));
}

#[test]
fn crc8_is_the_dallas_crc_of_the_core() {
    let rom = [0x28, 0x84, 0x37, 0x94, 0x97, 0xFF, 0x03];
    assert_eq!(crc8(&rom), vdm_stm_core::onewire_check::crc8(&rom));
    assert_eq!(crc8(&[0x02, 0x1C, 0xB8, 0x01, 0x00, 0x00, 0x00]), 0xA2);
    assert_eq!(crc8(&[]), 0);
}
