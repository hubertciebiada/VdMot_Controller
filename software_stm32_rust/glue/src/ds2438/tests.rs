// New tests (docs/rust/GLUE-DESIGN-STM.md §7.3) of the DS2438 part the firmware uses, on the
// bit-level bus simulator and on a scripted line: begin, the VAD conversion with its float steps
// (R6), the configuration bit and the checks of the scratchpad reads.

use std::collections::VecDeque;

use super::*;
use crate::test_support::io_fakes::{Ev, FakeBoard};
use crate::test_support::onewire_sim::{Event, Kind, OneWireSim, SimDevice};

fn bus(devices: Vec<SimDevice>) -> OneWire<OneWireSim> {
    OneWire::new(OneWireSim::new(devices))
}

fn monitor(ow: &mut OneWire<OneWireSim>, i: usize) -> Ds2438 {
    let mut bm = Ds2438::default();
    let rom = ow.line().devices[i].rom;
    bm.set_address(&rom);
    bm
}

#[test]
fn begin_takes_the_first_device_on_the_bus_whatever_its_family() {
    // serial 2 of family 0x28 is found before serial 1 of family 0x26
    let t = SimDevice::ds18b20(2, 0);
    let v = SimDevice::ds2438(1, 0);
    let (rt, rv) = (t.rom, v.rom);
    let mut ow = bus(vec![v, t]);
    let mut bm = Ds2438::default();
    assert!(bm.begin(&mut ow, DS2438_BEGIN_RETRIES));
    assert_eq!(*bm.address(), rt);
    assert_ne!(*bm.address(), rv);
    // one attempt: a reset of its own and the one of the search
    let resets = ow
        .line()
        .events
        .iter()
        .filter(|e| matches!(e, Event::Reset(_)))
        .count();
    assert_eq!(resets, 2);
    assert_eq!(DS2438_BEGIN_RETRIES, 3);
}

#[test]
fn begin_without_a_device_tries_retries_times() {
    let mut ow = bus(vec![]);
    let mut bm = Ds2438::default();
    assert!(!bm.begin(&mut ow, 3));
    assert_eq!(ow.line().events, vec![Event::Reset(false); 6]);
    assert_eq!(bm.address()[0], 0);
    let mut ow = bus(vec![]);
    assert!(!bm.begin(&mut ow, 0));
    assert!(ow.line().events.is_empty());
}

#[test]
fn begin_rejects_a_rom_with_a_bad_crc() {
    let mut d = SimDevice::ds2438(1, 0);
    d.rom[7] ^= 0x01;
    let mut ow = bus(vec![d]);
    let mut bm = Ds2438::default();
    assert!(!bm.begin(&mut ow, 2));
    // two attempts of 2 resets, the search command and 64 direction bits
    assert_eq!(ow.line().events.len(), 2 * (2 + 1 + 64));
}

/// A line answering resets from a script, reading from a script, recording the bytes written.
struct ScriptLine {
    resets: VecDeque<bool>,
    reads: VecDeque<bool>,
    bits: Vec<bool>,
}

impl ScriptLine {
    fn new(resets: &[bool], pages: &[[u8; 9]]) -> Self {
        let reads = pages
            .iter()
            .flat_map(|p| p.iter().flat_map(|b| (0..8).map(move |i| b >> i & 1 != 0)))
            .collect();
        ScriptLine {
            resets: resets.iter().copied().collect(),
            reads,
            bits: Vec::new(),
        }
    }

    fn written(&self) -> Vec<u8> {
        self.bits
            .chunks(8)
            .map(|c| {
                c.iter()
                    .enumerate()
                    .fold(0u8, |b, (i, &v)| b | (u8::from(v) << i))
            })
            .collect()
    }
}

impl OneWireLine for ScriptLine {
    fn reset(&mut self) -> bool {
        self.resets.pop_front().unwrap_or(false)
    }

    fn write_bit(&mut self, bit: bool) {
        self.bits.push(bit);
    }

    fn read_bit(&mut self) -> bool {
        self.reads.pop_front().unwrap_or(true)
    }
}

/// A page with its CRC.
fn page(data: [u8; 8]) -> [u8; 9] {
    let mut p = [0u8; 9];
    p[..8].copy_from_slice(&data);
    p[8] = crc8(&data);
    p
}

const ROM: DeviceAddress = [0x26, 1, 0, 0, 0, 0, 0, 0];

fn select(cmd: u8) -> Vec<u8> {
    let mut v = vec![0x55];
    v.extend_from_slice(&ROM);
    v.push(cmd);
    v
}

#[test]
fn read_vad_clears_the_ad_bit_converts_waits_10_ms_and_reads_page_0() {
    let mut ow = bus(vec![SimDevice::ds2438(1, 450)]);
    let mut bm = monitor(&mut ow, 0);
    let clock = FakeBoard::new();
    assert_eq!(bm.read_vad(&mut ow, &clock), 4.5);
    assert_eq!(clock.events(), vec![Ev::Delay(10)]);
    let d = &ow.line().devices[0];
    assert_eq!(d.pages[0][0], 0x07);
    assert_eq!(d.scratch_writes, 1);
    assert_eq!(d.copies, 1);
    assert_eq!(d.converts, 1);
    // the second read finds the bit clear: no write
    assert_eq!(bm.read_vad(&mut ow, &clock), 4.5);
    assert_eq!(ow.line().devices[0].scratch_writes, 1);
    assert_eq!(ow.line().devices[0].converts, 2);
}

#[test]
fn read_vad_command_sequence() {
    let rom = SimDevice::ds2438(1, 0).rom;
    let mut sel = vec![0x55];
    sel.extend_from_slice(&rom);
    let with = |cmd: &[u8]| {
        let mut v = sel.clone();
        v.extend_from_slice(cmd);
        v
    };
    let mut ow = bus(vec![SimDevice::ds2438(1, 0)]);
    let mut bm = monitor(&mut ow, 0);
    bm.read_vad(&mut ow, &FakeBoard::new());
    let mut expected = Vec::new();
    // clear the AD bit: read page 0, write it back with the bit clear, copy it
    expected.extend(with(&[0xB8, 0x00]));
    expected.extend(with(&[0xBE, 0x00]));
    expected.extend(with(&[0x4E, 0x00, 0x07]));
    expected.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0]);
    expected.extend(with(&[0x48, 0x00]));
    // convert, then read page 0
    expected.extend(with(&[0xB4]));
    expected.extend(with(&[0xB8, 0x00]));
    expected.extend(with(&[0xBE, 0x00]));
    assert_eq!(ow.line().writes(), expected);
}

#[test]
fn read_vad_float_steps_of_the_c_double_and_float() {
    for (raw, v) in [
        (0u16, 0.0f32),
        (1, 0.01),
        (29, 0.29),
        (450, 4.5),
        (499, 4.99),
        (1023, 10.23),
    ] {
        let mut ow = bus(vec![SimDevice::ds2438(1, raw)]);
        let mut bm = monitor(&mut ow, 0);
        assert_eq!(bm.read_vad(&mut ow, &FakeBoard::new()), v, "raw {raw}");
    }
}

#[test]
fn read_vad_takes_10_bits_of_bytes_3_and_4() {
    let mut ow = bus(vec![SimDevice::ds2438(1, 0x2FF)]);
    ow.line().devices[0].pages[0][4] = 0xFC;
    let mut bm = monitor(&mut ow, 0);
    // byte 4 = 0xFE after the conversion: only its 2 low bits count
    assert_eq!(bm.read_vad(&mut ow, &FakeBoard::new()), 7.67);
}

#[test]
fn read_vad_of_a_page_with_bytes_0_to_3_all_ff_is_invalid() {
    let cfg = page([0x07, 0, 0, 0x2C, 0x01, 0, 0, 0]);
    let mut bm = Ds2438::default();
    bm.set_address(&ROM);
    let all_ff = page([0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0, 0, 0]);
    let mut ow = OneWire::new(ScriptLine::new(&[true; 5], &[cfg, all_ff]));
    assert_eq!(bm.read_vad(&mut ow, &FakeBoard::new()), DS2438_VAD_INVALID);
    // one byte of the four differs: valid
    for (k, v) in [(0, 5.11f32), (1, 5.11), (2, 5.11), (3, 3.0)] {
        let mut p = [0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0, 0, 0];
        p[k] = 0x2C;
        let mut ow = OneWire::new(ScriptLine::new(&[true; 5], &[cfg, page(p)]));
        assert_eq!(bm.read_vad(&mut ow, &FakeBoard::new()), v, "byte {k}");
    }
    assert_eq!(DS2438_VAD_INVALID, -10.0);
}

#[test]
fn read_vad_of_a_bad_crc_or_an_absent_device_is_invalid_and_writes_nothing() {
    let mut ow = bus(vec![SimDevice::ds2438(1, 300)]);
    ow.line().devices[0].bad_crc_reads = 2;
    let mut bm = monitor(&mut ow, 0);
    assert_eq!(bm.read_vad(&mut ow, &FakeBoard::new()), DS2438_VAD_INVALID);
    // the corrupted read of the configuration was not written back
    assert_eq!(ow.line().devices[0].scratch_writes, 0);
    assert_eq!(ow.line().devices[0].pages[0][0], 0x0F);
    let mut ow = bus(vec![SimDevice::ds2438(1, 300)]);
    ow.line().devices[0].present = false;
    let mut bm = monitor(&mut ow, 0);
    assert_eq!(bm.read_vad(&mut ow, &FakeBoard::new()), DS2438_VAD_INVALID);
    ow.line().held_low = true;
    ow.line().devices[0].present = true;
    assert_eq!(bm.read_vad(&mut ow, &FakeBoard::new()), DS2438_VAD_INVALID);
}

#[test]
fn read_scratch_pad_needs_both_presence_pulses() {
    let cfg = page([0x07, 0, 0, 0x2C, 0x01, 0, 0, 0]);
    // resets: config read (2), convert (1), page read (2); a missing pulse anywhere in a read
    for (resets, ok) in [
        ([true, true, true, true, true], true),
        ([true, true, true, false, true], false),
        ([true, true, true, true, false], false),
    ] {
        let mut ow = OneWire::new(ScriptLine::new(&resets, &[cfg, cfg]));
        let mut bm = Ds2438::default();
        bm.set_address(&ROM);
        let v = bm.read_vad(&mut ow, &FakeBoard::new());
        assert_eq!(v, if ok { 3.0 } else { DS2438_VAD_INVALID }, "{resets:?}");
    }
}

#[test]
fn clear_config_bit_needs_a_valid_read_and_a_set_bit() {
    let set = page([0x0F, 0, 0, 0, 0, 0, 0, 0]);
    let clear = page([0x07, 0, 0, 0, 0, 0, 0, 0]);
    // no presence at the first read: nothing written back
    let mut ow = OneWire::new(ScriptLine::new(&[false, true], &[set]));
    let mut bm = Ds2438::default();
    bm.set_address(&ROM);
    bm.clear_config_bit(&mut ow, DS2438_CFG_AD);
    let w = ow.line().written();
    assert_eq!(w, [select(0xB8), vec![0], select(0xBE), vec![0]].concat());
    // the bit already clear: nothing written
    let mut ow = OneWire::new(ScriptLine::new(&[true, true], &[clear]));
    bm.clear_config_bit(&mut ow, DS2438_CFG_AD);
    assert_eq!(ow.line().written().len(), 22);
    // set: written back with only that bit cleared, then copied
    let mut ow = OneWire::new(ScriptLine::new(&[true, true, true, true], &[set]));
    bm.clear_config_bit(&mut ow, DS2438_CFG_AD);
    let w = ow.line().written();
    let mut tail = select(0x4E);
    tail.extend_from_slice(&[0, 0x07, 0, 0, 0, 0, 0, 0, 0]);
    tail.extend(select(0x48));
    tail.push(0);
    assert_eq!(w[22..], tail[..]);
    // another bit: only that one
    let mut ow = OneWire::new(ScriptLine::new(&[true, true, true, true], &[set]));
    bm.clear_config_bit(&mut ow, 0);
    assert_eq!(ow.line().written()[33], 0x0E);
    assert_eq!(DS2438_CFG_AD, 3);
}

#[test]
fn a_device_of_another_kind_is_not_a_monitor() {
    let mut ow = bus(vec![SimDevice::new(0x01, 5, Kind::Other)]);
    let mut bm = monitor(&mut ow, 0);
    assert_eq!(bm.read_vad(&mut ow, &FakeBoard::new()), DS2438_VAD_INVALID);
}
