// New tests (docs/rust/GLUE-DESIGN-STM.md §7.3) of the DallasTemperature port on the bit-level
// bus simulator, and the two 1-Wire cases of test/native/glue/test_fakes.cpp, which pin the
// library behaviour the C++ fake stood for.

use std::collections::VecDeque;

use super::*;
use crate::test_support::io_fakes::{Ev, FakeBoard};
use crate::test_support::onewire_sim::{Event, Kind, OneWireSim, SimDevice};

fn bus(devices: Vec<SimDevice>) -> OneWire<OneWireSim> {
    OneWire::new(OneWireSim::new(devices))
}

fn with_config(mut d: SimDevice, config: u8) -> SimDevice {
    d.scratch[CONFIGURATION] = config;
    d
}

/// A DS18S20 with the temperature register (0.5 degC steps), COUNT_REMAIN and COUNT_PER_C.
fn ds18s20(serial: u8, reg: i16, remain: u8, per_c: u8) -> SimDevice {
    let mut d = SimDevice::new(DS18S20MODEL, serial, Kind::Ds18);
    d.set_temp_register(reg);
    d.scratch[COUNT_REMAIN] = remain;
    d.scratch[COUNT_PER_C] = per_c;
    d
}

// ---------------------------------------------------------------- test_fakes.cpp, 1-Wire

#[test]
fn fakes_search_in_list_order_crc_of_lib_core_dallas_temperature_on_the_bus() {
    let t = SimDevice::ds18b20(1, 21 * 16);
    let v = SimDevice::new(0x26, 2, Kind::Ds2438);
    let mut gone = SimDevice::ds18b20(3, 0);
    gone.present = false;
    let mut ow = bus(vec![t, v, gone]);
    let mut dallas = DallasTemperature::default();
    dallas.begin(&mut ow);
    assert_eq!(dallas.get_device_count(), 2);
    assert_eq!(dallas.get_resolution(), 12);
    assert_eq!(
        DallasTemperature::millis_to_wait_for_conversion(dallas.get_resolution()),
        750
    );
    assert_eq!(DallasTemperature::millis_to_wait_for_conversion(9), 94);
    let mut a = [0u8; 8];
    ow.reset_search();
    assert!(ow.search(&mut a, true));
    assert_eq!(a[1], 1);
    assert!(DallasTemperature::valid_address(&a));
    assert!(DallasTemperature::valid_family(&a));
    // getTempC() == 21.0 of the C++ case: getTemp() in 1/128 degC
    assert_eq!(DallasTemperature::get_temp(&mut ow, &a), 21 * 128);
    assert!(ow.search(&mut a, true));
    assert!(!DallasTemperature::valid_family(&a));
    assert!(!ow.search(&mut a, true));
    a[7] ^= 1;
    assert!(!DallasTemperature::valid_address(&a));
}

#[test]
fn fakes_get_temp_gives_device_disconnected_raw_for_a_failing_read_and_an_absent_sensor() {
    let mut s = SimDevice::ds18b20(1, 85 * 16);
    s.bad_crc_reads = 1;
    let rom = s.rom;
    let mut ow = bus(vec![s]);
    assert_eq!(
        DallasTemperature::get_temp(&mut ow, &rom),
        DEVICE_DISCONNECTED_RAW
    );
    assert_eq!(DallasTemperature::get_temp(&mut ow, &rom), 85 * 128);
    ow.line().devices[0].present = false;
    assert_eq!(
        DallasTemperature::get_temp(&mut ow, &rom),
        DEVICE_DISCONNECTED_RAW
    );
}

// ---------------------------------------------------------------- begin

#[test]
fn begin_counts_valid_addresses_and_takes_the_highest_ds18_resolution() {
    let mut bad = SimDevice::ds18b20(9, 0);
    bad.rom[7] ^= 0x55;
    let devices = vec![
        with_config(SimDevice::ds18b20(1, 0), TEMP_9_BIT),
        with_config(SimDevice::ds18b20(2, 0), TEMP_11_BIT),
        SimDevice::new(0x26, 3, Kind::Ds2438),
        SimDevice::new(0x01, 4, Kind::Other),
        bad,
    ];
    let mut ow = bus(devices);
    let mut dallas = DallasTemperature::default();
    assert_eq!(dallas.get_resolution(), 9);
    dallas.begin(&mut ow);
    assert_eq!(dallas.get_device_count(), 4);
    assert_eq!(dallas.get_resolution(), 11);
    assert!(!dallas.is_parasite_power_mode());
    // a second begin counts again from 0; the resolution only grows
    ow.line().devices.truncate(1);
    dallas.begin(&mut ow);
    assert_eq!(dallas.get_device_count(), 1);
    assert_eq!(dallas.get_resolution(), 11);
}

#[test]
fn begin_resolution_of_each_configuration_and_of_a_ds18s20() {
    for (config, res) in [
        (TEMP_9_BIT, 9),
        (TEMP_10_BIT, 10),
        (TEMP_11_BIT, 11),
        (TEMP_12_BIT, 12),
        (0x00, 9),
    ] {
        let mut ow = bus(vec![with_config(SimDevice::ds18b20(1, 0), config)]);
        let mut dallas = DallasTemperature::default();
        dallas.begin(&mut ow);
        assert_eq!(dallas.get_resolution(), res, "config {config:#x}");
    }
    let mut ow = bus(vec![ds18s20(1, 0, 0, 0)]);
    let mut dallas = DallasTemperature::default();
    dallas.begin(&mut ow);
    assert_eq!(dallas.get_resolution(), 12);
}

#[test]
fn begin_notes_parasite_power_once() {
    // serial 2 is found first (bit 0 of byte 1 is 0)
    let mut p = SimDevice::ds18b20(2, 0);
    p.parasite = true;
    let normal = SimDevice::ds18b20(1, 0);
    let mut ow = bus(vec![p, normal]);
    let mut dallas = DallasTemperature::default();
    dallas.begin(&mut ow);
    assert!(dallas.is_parasite_power_mode());
    // READ POWER SUPPLY (0xB4) went to the first device found only
    let asked = ow.line().writes().iter().filter(|&&b| b == 0xB4).count();
    assert_eq!(asked, 1);
}

#[test]
fn begin_skips_families_without_a_temperature_register() {
    let mut ow = bus(vec![SimDevice::new(0x26, 1, Kind::Ds2438)]);
    let mut dallas = DallasTemperature::default();
    dallas.begin(&mut ow);
    assert_eq!(dallas.get_device_count(), 1);
    assert_eq!(ow.line().writes().iter().filter(|&&b| b == 0xB4).count(), 0);
    assert_eq!(dallas.get_resolution(), 9);
}

#[test]
fn begin_stops_after_64_search_passes() {
    let devices: Vec<SimDevice> = (0..70u8).map(|i| SimDevice::ds18b20(i, 0)).collect();
    let mut ow = bus(devices);
    let mut dallas = DallasTemperature::default();
    dallas.begin(&mut ow);
    assert_eq!(dallas.get_device_count(), 64);
    assert_eq!(DALLAS_MAX_SEARCH_PASSES, 64);
}

// ---------------------------------------------------------------- resolution, families, waits

#[test]
fn get_resolution_of_a_device() {
    let d = with_config(SimDevice::ds18b20(1, 0), TEMP_10_BIT);
    let rom = d.rom;
    let mut ow = bus(vec![d]);
    assert_eq!(DallasTemperature::get_resolution_of(&mut ow, &rom), 10);
    for (config, res) in [(TEMP_9_BIT, 9), (TEMP_11_BIT, 11), (TEMP_12_BIT, 12)] {
        ow.line().devices[0].scratch[CONFIGURATION] = config;
        assert_eq!(DallasTemperature::get_resolution_of(&mut ow, &rom), res);
    }
    ow.line().devices[0].scratch[CONFIGURATION] = 0x20;
    assert_eq!(DallasTemperature::get_resolution_of(&mut ow, &rom), 0);
    ow.line().devices[0].scratch[CONFIGURATION] = TEMP_12_BIT;
    ow.line().devices[0].present = false;
    assert_eq!(DallasTemperature::get_resolution_of(&mut ow, &rom), 0);
    // a DS18S20 has no configuration register: 12, without a bus transfer
    let s = ds18s20(2, 0, 0, 0);
    let srom = s.rom;
    let mut ow = bus(vec![s]);
    assert_eq!(DallasTemperature::get_resolution_of(&mut ow, &srom), 12);
    assert!(ow.line().events.is_empty());
}

#[test]
fn valid_family_takes_the_five_temperature_families() {
    for f in [0x10, 0x28, 0x22, 0x3B, 0x42] {
        assert!(DallasTemperature::valid_family(&[f, 0, 0, 0, 0, 0, 0, 0]));
    }
    for f in [0x00, 0x01, 0x26, 0x29, 0x3A, 0x43, 0xFF] {
        assert!(!DallasTemperature::valid_family(&[f, 0, 0, 0, 0, 0, 0, 0]));
    }
}

#[test]
fn millis_to_wait_for_conversion_of_each_resolution() {
    let t = DallasTemperature::millis_to_wait_for_conversion;
    assert_eq!([t(9), t(10), t(11), t(12)], [94, 188, 375, 750]);
    assert_eq!([t(0), t(8), t(13), t(255)], [750, 750, 750, 750]);
}

#[test]
fn defaults_wait_for_conversion_and_9_bit() {
    let mut dallas = DallasTemperature::default();
    assert!(dallas.get_wait_for_conversion());
    assert_eq!(dallas.get_device_count(), 0);
    dallas.set_wait_for_conversion(false);
    assert!(!dallas.get_wait_for_conversion());
}

// ---------------------------------------------------------------- request

#[test]
fn request_temperatures_without_waiting_starts_the_conversion_on_every_device() {
    let mut ow = bus(vec![SimDevice::ds18b20(1, 0), SimDevice::ds18b20(2, 0)]);
    let clock = FakeBoard::new();
    let mut dallas = DallasTemperature::default();
    dallas.set_wait_for_conversion(false);
    dallas.request_temperatures(&mut ow, &clock);
    assert_eq!(ow.line().writes(), vec![0xCC, 0x44]);
    assert_eq!(ow.line().events[0], Event::Reset(true));
    assert_eq!(ow.line().devices[0].converts, 1);
    assert_eq!(ow.line().devices[1].converts, 1);
    assert_eq!(ow.line().reads, 0);
}

#[test]
fn request_temperatures_waiting_polls_until_the_conversion_is_done() {
    let mut d = SimDevice::ds18b20(1, 0);
    d.busy_reads = 5;
    let mut ow = bus(vec![d]);
    let clock = FakeBoard::new();
    // time passes while it polls (a poll that never ends fails instead of hanging)
    clock.0.borrow_mut().auto_advance_us = 1000;
    let mut dallas = DallasTemperature::default();
    dallas.request_temperatures(&mut ow, &clock);
    // 5 slots read 0, the sixth 1
    assert_eq!(ow.line().reads, 6);
    assert!(clock.events().is_empty());
}

#[test]
fn request_temperatures_waiting_gives_up_after_750_ms() {
    let mut d = SimDevice::ds18b20(1, 0);
    d.busy_reads = 1_000_000;
    let mut ow = bus(vec![d]);
    let clock = FakeBoard::new();
    clock.0.borrow_mut().auto_advance_us = 1000;
    let mut dallas = DallasTemperature::default();
    dallas.request_temperatures(&mut ow, &clock);
    // millis() 0 at the start, then one ms per call: the 750th poll sees 750 ms
    assert_eq!(ow.line().reads, 750);
}

#[test]
fn request_temperatures_on_a_parasite_bus_waits_the_time_of_the_resolution() {
    let mut p = with_config(SimDevice::ds18b20(1, 0), TEMP_10_BIT);
    p.parasite = true;
    let mut ow = bus(vec![p]);
    let clock = FakeBoard::new();
    let mut dallas = DallasTemperature::default();
    dallas.begin(&mut ow);
    let reads = ow.line().reads;
    dallas.request_temperatures(&mut ow, &clock);
    assert_eq!(clock.events(), vec![Ev::Delay(188)]);
    assert_eq!(ow.line().reads, reads);
}

// ---------------------------------------------------------------- getTemp and the scratchpad

#[test]
fn get_temp_reads_the_scratchpad_of_the_selected_device() {
    let a = SimDevice::ds18b20(1, 0x0191);
    let b = SimDevice::ds18b20(2, -0x00A2);
    let (ra, rb) = (a.rom, b.rom);
    let mut ow = bus(vec![a, b]);
    // +25.0625 degC: 0x0191 * 8 = 3208 / 128
    assert_eq!(DallasTemperature::get_temp(&mut ow, &ra), 3208);
    // -10.125 degC
    assert_eq!(DallasTemperature::get_temp(&mut ow, &rb), -1296);
    let mut expected = vec![0x55];
    expected.extend_from_slice(&rb);
    expected.push(0xBE);
    let w = ow.line().writes();
    assert_eq!(w[w.len() - 10..], expected[..]);
}

#[test]
fn get_temp_rejects_an_all_zero_scratchpad_and_a_bad_crc() {
    let mut d = SimDevice::ds18b20(1, 0);
    d.scratch = [0; 8];
    let rom = d.rom;
    let mut ow = bus(vec![d]);
    // all zero has a valid CRC (0) but is what a bus held low reads
    assert_eq!(
        DallasTemperature::get_temp(&mut ow, &rom),
        DEVICE_DISCONNECTED_RAW
    );
    ow.line().devices[0].scratch[2] = 1;
    assert_eq!(DallasTemperature::get_temp(&mut ow, &rom), 0);
    ow.line().devices[0].bad_crc_reads = 1;
    assert_eq!(
        DallasTemperature::get_temp(&mut ow, &rom),
        DEVICE_DISCONNECTED_RAW
    );
}

/// A line answering two resets as given and the 9 scratchpad bytes.
struct ScriptLine {
    resets: VecDeque<bool>,
    reads: VecDeque<bool>,
}

impl OneWireLine for ScriptLine {
    fn reset(&mut self) -> bool {
        self.resets.pop_front().unwrap_or(false)
    }

    fn write_bit(&mut self, _bit: bool) {}

    fn read_bit(&mut self) -> bool {
        self.reads.pop_front().unwrap_or(true)
    }
}

fn scratch_bits(bytes: &[u8; 9]) -> VecDeque<bool> {
    bytes
        .iter()
        .flat_map(|b| (0..8).map(move |i| b >> i & 1 != 0))
        .collect()
}

#[test]
fn read_scratch_pad_needs_the_presence_pulse_before_and_after() {
    let mut pad = [0x50, 0x05, 0x4B, 0x46, 0x7F, 0xFF, 0x0C, 0x10, 0];
    pad[8] = crc8(&pad[..8]);
    let rom = [0x28, 1, 0, 0, 0, 0, 0, 0];
    for (resets, ok) in [
        ([true, true], true),
        ([true, false], false),
        ([false, true], false),
    ] {
        let mut ow = OneWire::new(ScriptLine {
            resets: resets.into_iter().collect(),
            reads: scratch_bits(&pad),
        });
        let mut got = [0u8; 9];
        assert_eq!(
            DallasTemperature::read_scratch_pad(&mut ow, &rom, &mut got),
            ok
        );
        if resets[0] {
            assert_eq!(got, pad);
        } else {
            assert_eq!(got, [0; 9]);
        }
        let mut sp = [0u8; 9];
        assert_eq!(
            DallasTemperature::is_connected(
                &mut OneWire::new(ScriptLine {
                    resets: resets.into_iter().collect(),
                    reads: scratch_bits(&pad),
                }),
                &rom,
                &mut sp
            ),
            ok
        );
    }
}

// ---------------------------------------------------------------- calculateTemperature

fn calc(address0: u8, pad: [u8; 9]) -> i16 {
    DallasTemperature::calculate_temperature(&[address0, 0, 0, 0, 0, 0, 0, 0], &pad)
}

#[test]
fn calculate_temperature_ds18b20_through_int_and_a_narrowing_to_int16() {
    let t = |lsb: u8, msb: u8| calc(DS18B20MODEL, [lsb, msb, 0, 0, 0x7F, 0, 0, 0, 0]);
    assert_eq!(t(0xD0, 0x07), 16000); // +125 degC
    assert_eq!(t(0x50, 0x05), 10880); // +85 degC
    assert_eq!(t(0x08, 0x00), 64); // +0.5 degC
    assert_eq!(t(0x00, 0x00), 0);
    assert_eq!(t(0xF8, 0xFF), -64); // -0.5 degC
    assert_eq!(t(0x5E, 0xFF), -1296); // -10.125 degC
    assert_eq!(t(0x90, 0xFC), -7040); // -55 degC: the value of DEVICE_DISCONNECTED_RAW
                                      // bit 4 of the MSB lands in bit 15: the int16_t narrowing wraps
    assert_eq!(t(0x00, 0x10), i16::MIN);
    assert_eq!(t(0xFF, 0x0F), 0x7FF8);
}

#[test]
fn calculate_temperature_ds18s20_extended_resolution() {
    let s = |reg: i16, remain: u8, per_c: u8| {
        let [lsb, msb] = reg.to_le_bytes();
        calc(DS18S20MODEL, [lsb, msb, 0, 0, 0xFF, 0xFF, remain, per_c, 0])
    };
    // +25.0 degC read as 50 (0.5 degC steps), count remain 12 of 16: 25.0 - 0.25 + 4/16 = 25.0
    assert_eq!(s(50, 12, 16), 25 * 128);
    // +25.0625: remain 11
    assert_eq!(s(50, 11, 16), 3208);
    // -7.0 degC: register -14, the int16 sign extension masked off by & 0xfff0, then wrapped
    assert_eq!(s(-14, 12, 16), -896);
    // remain above count per C: a negative fraction, truncated toward zero
    assert_eq!(s(50, 20, 16), 3200 - 32 - 32);
    assert_eq!(s(50, 13, 3), 3200 - 32 - 426);
    // remain 0: + 128
    assert_eq!(s(0, 0, 16), -32 + 128);
    // COUNT_PER_C 0: no extended resolution (division guard), the DS18B20 formula
    assert_eq!(s(50, 12, 0), 400);
    // another family with the same bytes: no extended resolution
    assert_eq!(
        calc(DS18B20MODEL, [50, 0, 0, 0, 0xFF, 0xFF, 12, 16, 0]),
        400
    );
}
