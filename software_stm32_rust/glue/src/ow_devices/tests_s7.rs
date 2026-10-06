// Port of test/native/glue/test_owDevices_s7.cpp, S7: a failed DS18B20 read is repeated once, a
// failure after it keeps the last good value for 2 cycles (then -1270), 85.0 degC counts only
// after a reading of at least 75.0 degC, a new sensor at an index starts without history, and
// the bus is enumerated again 24 h after the last enumeration while the temperature machine is
// not locked.

use super::tests::Rig;
use super::*;

impl Rig {
    /// one complete temperature cycle (at most 400 calls); true if it ended
    fn full_cycle(&mut self) -> bool {
        self.clear_calls();
        for _ in 0..400 {
            self.step();
            if !self.calls_of("app_temp_cycle_done").is_empty() {
                return true;
            }
        }
        false
    }

    fn setup_one(&mut self, raw: i16) -> usize {
        let d = self.bus.add_one_wire(0x28, 1, raw);
        self.setup();
        d
    }

    fn advance_s(&self, s: u64) {
        self.board.advance_us(s * 1_000_000);
    }
}

#[test]
fn a_failed_read_is_repeated_once_in_the_same_cycle() {
    let mut r = Rig::new();
    let d = r.setup_one(2752);
    r.bus.devices[d].fail_reads = 1;
    assert!(r.full_cycle());
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, 215);
    assert_eq!(r.bus.reads, 2);
    assert!(r.full_cycle());
    assert_eq!(r.bus.reads, 3);
}

#[test]
fn a_sensor_that_fails_the_last_good_value_for_2_cycles_then_minus_1270_a_good_read_ends_it() {
    let mut r = Rig::new();
    let d = r.setup_one(2752);
    assert!(r.full_cycle());
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, 215);
    for e in [215, 215, -1270, -1270] {
        r.bus.devices[d].fail_reads = 2;
        assert!(r.full_cycle());
        assert_eq!(r.ow.sensors.tempsensors[0].temperature, e);
    }
    r.bus.devices[d].raw = -672;
    assert!(r.full_cycle());
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, -53);
    r.bus.devices[d].fail_reads = 2;
    assert!(r.full_cycle());
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, -53);
}

#[test]
fn a_sensor_without_a_good_read_reports_minus_1270_at_once_85_degc_only_after_75_degc() {
    let mut r = Rig::new();
    // 85.0 degC: power-on value
    let d = r.setup_one(10880);
    assert!(r.full_cycle());
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, -1270);
    // 76.0 degC
    r.bus.devices[d].raw = 9728;
    assert!(r.full_cycle());
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, 760);
    r.bus.devices[d].raw = 10880;
    assert!(r.full_cycle());
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, 850);
}

#[test]
fn a_different_sensor_at_an_index_starts_without_the_history_of_the_old_one() {
    let mut r = Rig::new();
    r.setup_one(2752);
    assert!(r.full_cycle());
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, 215);
    r.bus.devices.clear();
    let other = r.bus.add_one_wire(0x28, 9, 0);
    r.set_device_address();
    r.bus.devices[other].fail_reads = 2;
    assert!(r.full_cycle());
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, -1270);
}

#[test]
fn the_bus_is_enumerated_again_24_h_after_the_last_enumeration_not_while_locked() {
    let mut r = Rig::new();
    r.setup_one(2752);
    assert_eq!(r.scan_age(), 0);
    r.advance_s(86399);
    assert_eq!(r.scan_age(), 86399);
    assert!(r.full_cycle());
    assert_eq!(r.bus.begins, 1);
    r.board.advance_us(999_999);
    assert_eq!(r.scan_age(), 86399);
    r.board.advance_us(1);
    assert_eq!(r.scan_age(), 86400);
    r.bus.add_one_wire(0x28, 2, 1280);
    r.temp_command(TEMP_CMD_LOCK);
    for _ in 0..20 {
        r.step();
    }
    assert_eq!(r.bus.begins, 1);
    assert_eq!(r.ow.sensors.no_of_ds18_devices, 1);
    r.temp_command(TEMP_CMD_UNLOCK);
    r.clear_calls();
    for _ in 0..4 {
        r.step();
    }
    assert_eq!(r.bus.begins, 2);
    assert_eq!(r.ow.sensors.no_of_ds18_devices, 2);
    assert_eq!(r.calls_of("app_match_sensors").len(), 1);
    assert_eq!(r.scan_age(), 0);
    // the next one 24 h later; the time counts on in whole seconds
    r.advance_s(3600);
    r.board.advance_us(1_500_000);
    assert_eq!(r.scan_age(), 3601);
    r.board.advance_us(500_000);
    assert_eq!(r.scan_age(), 3602);
}

#[test]
fn stons_searches_at_once_also_while_the_temperature_machine_is_locked() {
    let mut r = Rig::new();
    r.setup_one(2752);
    r.advance_s(90000);
    r.temp_command(TEMP_CMD_NEWSEARCH);
    r.temp_command(TEMP_CMD_LOCK);
    assert!(r.temp_locked());
    for _ in 0..5 {
        r.step();
    }
    assert_eq!(r.bus.begins, 2);
    assert_eq!(r.scan_age(), 0);
    for _ in 0..20 {
        r.step();
    }
    assert_eq!(r.bus.begins, 2);
}
