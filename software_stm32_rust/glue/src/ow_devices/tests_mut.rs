// Port of test/native/glue/test_owDevices__mut.cpp: the state before temperature_setup(), the
// debug address format, a repeated setup, too many DS2438, the cycle and search lengths, one
// sensor in JSON; and new cases for the edges the mutation gate found (no C++ counterpart).

use super::tests::{hex_address, Rig};
use super::*;

#[test]
fn ow_devices_before_temperature_setup_no_device_no_scan_age() {
    let mut r = Rig::new();
    assert_eq!(r.ow.sensors.no_of_devices, 0);
    assert_eq!(r.ow.sensors.no_of_ds18_devices, 0);
    assert_eq!(r.ow.sensors.no_of_ds2438_devices, 0);
    assert_eq!(r.scan_age(), 0);
    r.board.advance_ms(1000);
    assert_eq!(r.scan_age(), 1);
    assert_eq!(r.print_sensordata(), "{\"cnt\":0}\r\n");
}

#[test]
fn ow_devices_a_cycle_without_temperature_setup_reads_no_ds2438() {
    let mut r = Rig::new();
    assert_ne!(r.cycle(200), 0);
    assert_eq!(r.calls(), vec!["app_temp_cycle_done()"]);
}

#[test]
fn print_address_the_eight_bytes_in_hex_one_leading_zero_below_0x10() {
    let mut r = Rig::new();
    print_address(
        &mut r.dbg,
        &[0x28, 0x0F, 0x10, 0x00, 0xAB, 0x01, 0xFF, 0x05],
    );
    assert_eq!(r.dbg.take_tx(), "{ 28,  0F,  10,  00,  AB,  01,  FF,  05 }");
}

#[test]
fn temperature_setup_again_the_known_sensors_start_over_without_a_temperature() {
    let mut r = Rig::new();
    r.bus.add_one_wire(0x28, 1, 2752);
    r.bus.add_one_wire(0x28, 2, 2752);
    r.setup();
    r.ow.sensors.tempsensors[0].temperature = 215;
    r.ow.sensors.tempsensors[1].temperature = 215;
    r.setup();
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, -500);
    assert_eq!(r.ow.sensors.tempsensors[1].temperature, -500);
}

#[test]
fn set_device_address_at_most_8_ds2438_a_vanished_one_is_cleared() {
    let mut r = Rig::new();
    for i in 0..(MAXDS2438CNT as u8 + 1) {
        r.bus.add_one_wire(0x26, i + 1, 0);
    }
    r.set_device_address();
    assert_eq!(usize::from(r.ow.sensors.no_of_ds2438_devices), MAXDS2438CNT);
    assert_eq!(usize::from(r.ow.sensors.no_of_devices), MAXDS2438CNT + 1);
    r.ow.sensors.voltsensors[0].vad = 450;
    r.bus.devices.clear();
    r.set_device_address();
    assert_eq!(r.ow.sensors.no_of_ds2438_devices, 0);
    assert_eq!(r.ow.sensors.voltsensors[0].address, [0; 8]);
    assert_eq!(r.ow.sensors.voltsensors[0].vad, 0);
}

#[test]
fn temperature_loop_a_cycle_without_ds2438_ends_after_the_last_temperature() {
    let mut r = Rig::new();
    r.bus.add_one_wire(0x28, 1, 2752);
    r.setup();
    r.clear_calls();
    assert_eq!(r.cycle(200), 101);
    assert_eq!(r.calls(), vec!["app_temp_cycle_done()"]);
}

#[test]
fn temperature_loop_a_search_takes_three_calls_after_the_idle_one_counts_are_0_meanwhile() {
    let mut r = Rig::new();
    r.bus.add_one_wire(0x28, 1, 0);
    r.bus.add_one_wire(0x26, 2, 0);
    r.setup();
    assert_eq!(r.ow.sensors.no_of_ds18_devices, 1);
    assert_eq!(r.ow.sensors.no_of_ds2438_devices, 1);
    // T_INIT
    r.step();
    r.temp_command(TEMP_CMD_NEWSEARCH);
    r.clear_calls();
    // T_IDLE -> T_SEARCH
    r.step();
    // begin
    r.step();
    assert_eq!(r.ow.sensors.no_of_ds18_devices, 0);
    assert_eq!(r.ow.sensors.no_of_ds2438_devices, 0);
    // device count
    r.step();
    assert!(r.calls_of("app_match_sensors").is_empty());
    // enumeration
    r.step();
    assert_eq!(r.calls_of("app_match_sensors").len(), 1);
    assert_eq!(r.ow.sensors.no_of_ds18_devices, 1);
}

#[test]
fn print_sensordata_one_sensor_is_a_list_of_one() {
    let mut r = Rig::new();
    let a = r.bus.add_one_wire(0x28, 0x01, 0);
    r.setup();
    r.ow.sensors.tempsensors[0].temperature = 215;
    let rom = r.bus.devices[a].rom;
    assert_eq!(
        r.print_sensordata(),
        format!(
            "{{\"cnt\":1,\"sns\":[{{\"temp\":215,\"add\":\"{}\"}}]}}\r\n",
            hex_address(&rom)
        )
    );
}

// ---------------------------------------------------------------- new cases

#[test]
fn the_wait_follows_the_resolution_of_the_bus() {
    for (bits, steps) in [(9u8, 20 + 9), (10, 20 + 18), (11, 20 + 37), (12, 20 + 75)] {
        let mut r = Rig::new();
        let d = r.bus.add_one_wire(0x28, 1, 0);
        r.bus.devices[d].resolution = bits;
        r.setup();
        // Init, Idle, Request, the waits, the last wait, one read, the end
        assert_eq!(r.cycle(200), 3 + steps + 1 + 1 + 1, "bits {bits}");
    }
}

#[test]
fn the_device_count_of_a_search_is_the_one_of_the_library() {
    let mut r = Rig::new();
    r.bus.add_one_wire(0x28, 1, 0);
    r.setup();
    r.temp_command(TEMP_CMD_NEWSEARCH);
    r.bus.add_one_wire(0x01, 9, 0);
    // Init, Idle -> Search, begin, count
    for _ in 0..4 {
        r.step();
    }
    assert_eq!(r.ow.sensors.no_of_devices, 2);
    // a second search command while one waits is dropped; lock and unlock are not commands
    r.temp_command(TEMP_CMD_NEWSEARCH);
    r.temp_command(7);
    r.step();
    assert_eq!(r.ow.temp_cmd, TEMP_CMD_NONE);
    r.temp_command(7);
    assert_eq!(r.ow.temp_cmd, 7);
    r.temp_command(TEMP_CMD_LOCK);
    assert_eq!(r.ow.temp_cmd, 7);
    assert!(r.temp_locked());
    r.temp_command(TEMP_CMD_UNLOCK);
    assert!(!r.temp_locked());
    assert_eq!(r.ow.temp_cmd, 7);
}

#[test]
fn a_ds2438_reading_is_truncated_after_100_times_in_float() {
    for (v, vad) in [
        (4.5f32, 450),
        (2.675, 267),
        (1.0, 100),
        (-10.0, -1000),
        (0.019, 1),
        (-0.019, -1),
    ] {
        let mut r = Rig::new();
        let d = r.bus.add_one_wire(0x26, 1, 0);
        r.setup();
        let rom = r.bus.devices[d].rom;
        r.bus.vad.insert(rom, v);
        assert_ne!(r.cycle(300), 0);
        assert_eq!(r.ow.sensors.voltsensors[0].vad, vad, "{v}");
    }
}

#[test]
fn the_read_counts_are_limited_by_the_table_sizes() {
    let mut r = Rig::new();
    r.bus.add_one_wire(0x28, 1, 1280);
    r.setup();
    // more sensors claimed than the table holds: 34 reads, then the end of the cycle
    r.ow.sensors.no_of_ds18_devices = 40;
    r.ow.sensors.no_of_ds2438_devices = 9;
    r.clear_calls();
    assert_ne!(r.cycle(400), 0);
    assert_eq!(r.bus.reads, 1 + 2 * 33);
    assert_eq!(r.calls_of("DS2438::readVAD").len(), MAXDS2438CNT);
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, 100);
    assert_eq!(MAXONEWIRECNT, 34);
}

#[test]
fn the_scan_age_counts_whole_seconds_and_wraps_with_millis() {
    let mut r = Rig::new();
    r.board.set_now_us((u64::from(u32::MAX) - 499) * 1000);
    r.set_device_address();
    r.board.advance_ms(1499);
    // millis wrapped: 1499 ms later
    assert_eq!(r.scan_age(), 1);
    r.board.advance_ms(500);
    assert_eq!(r.scan_age(), 1);
    r.board.advance_ms(1);
    assert_eq!(r.scan_age(), 2);
    r.ow.scan_age_s = u32::MAX;
    r.board.advance_ms(1000);
    assert_eq!(r.scan_age(), 0);
}
