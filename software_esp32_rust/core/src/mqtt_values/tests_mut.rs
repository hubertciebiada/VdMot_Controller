//! Port of test/native/test_mqtt_values__mut.cpp: the first and last temperature slot, a sensor
//! on bus index 0, one-character names and topics, calibration ends of valve 0 and out-of-range
//! valves.

use super::*;
use crate::common::copy_string;
use crate::test_support::assert_text;
use std::boxed::Box;
use std::vec::Vec;

fn sensor_id(last: u8) -> OneWireId {
    let mut o = OneWireId::default();
    o.b[0] = 0x28;
    o.b[7] = last;
    o
}

fn segment(c: &Config, k: ItemKind, i: u8, bus: Option<u8>) -> Vec<u8> {
    let mut out = [b'X'; 16];
    let n = sensor_topic_segment(c, k, i, bus, &mut out);
    out[..n].to_vec()
}

#[test]
fn slot_temp_tenths_the_first_and_the_last_slot_the_first_bus_entry() {
    let mut c = Box::new(Config::default());
    c.temps[0].id = sensor_id(1);
    let last = usize::from(TEMP_SLOT_COUNT) - 1;
    c.temps[last].id = sensor_id(2);
    c.temps[last].offset = 3;
    let mut t = [TempReading::default(); 2];
    t[0].id = sensor_id(1);
    t[0].seen = true;
    t[0].raw = 200;
    t[1].id = sensor_id(2);
    t[1].seen = true;
    t[1].raw = 150;
    assert_eq!(slot_temp_tenths(&c, &t, 1, 0, 60000), Some(200));
    assert_eq!(
        slot_temp_tenths(&c, &t, TEMP_SLOT_COUNT, 0, 60000),
        Some(153)
    );
}

#[test]
fn slot_volt_value_a_sensor_on_the_first_bus_entry() {
    let mut c = Box::new(Config::default());
    c.volts[0].id = sensor_id(4);
    let mut v = [VoltReading::default(); 1];
    v[0].id = sensor_id(4);
    v[0].seen = true;
    v[0].vad = 500;
    let value = slot_volt_value(&c, &v, 0, 0, 60000).expect("value");
    assert!((value - 5.0).abs() < 1e-12, "{value}");
}

#[test]
fn find_temp_bus_a_match_on_bus_index_0() {
    let mut t = [TempReading::default(); 2];
    t[0].id = sensor_id(7);
    assert_eq!(find_temp_bus(&t, &sensor_id(7)), Some(0));
}

#[test]
fn sensor_topic_segment_a_one_character_name_or_topic_wins_over_the_bus_index() {
    let mut c = Box::new(Config::default());
    copy_string(&mut c.temps[0].name, b"A");
    assert_text(&segment(&c, ItemKind::Temp, 0, Some(4)), "A");
    copy_string(&mut c.temps[1].topic, b"B");
    assert_text(&segment(&c, ItemKind::Temp, 1, Some(4)), "B");
}

#[test]
fn calib_end_tracker_the_end_of_valve_0_out_of_range_valves_read_valve_0() {
    let mut t = CalibEndTracker::default();
    let mut v = [ValveState::default(); N];
    let a = LocalTime {
        valid: true,
        epoch: 100,
        ..LocalTime::default()
    };
    let b = LocalTime { epoch: 200, ..a };
    v[0].calibrating = true;
    assert_eq!(t.observe(Some(&v), &a), 0);
    v[0].calibrating = false;
    assert_eq!(t.observe(Some(&v), &b), 0x0001);
    assert_eq!(t.end(0).epoch, 200);
    assert_eq!(t.end(1).epoch, 0);
    assert_eq!(t.end(VALVE_COUNT).epoch, 200);
    assert_eq!(t.end(255).epoch, 200);
}

// ---------------------------------------------------------------- Rust additions

#[test]
fn the_compat_mask_and_the_keys_of_the_cpp_firmware() {
    assert_eq!(VALVE_COMPAT_MASK, 0x7CFF);
    // valveCompatKey() of the C++ module (g++, x86-64; the ESP32 is little-endian as well).
    assert_eq!(valve_compat_key(&ValveState::default()), 0x4fca_b240);
    assert_eq!(valve_compat_key(&super::tests::full_valve()), 0x9207_478e);
    let wide = ValveState {
        moves: 0x0102_0304,
        dead_zone: -2,
        mean_current: 0xABCD,
        temp1: -1270,
        health: 0x0401,
        stm_flags: 0x8001,
        ..ValveState::default()
    };
    assert_eq!(valve_compat_key(&wide), 0x019d_bf0d);
}
