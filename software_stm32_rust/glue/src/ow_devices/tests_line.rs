// New tests (no C++ counterpart): owDevices on the firmware's bus, the ports of OneWire,
// DallasTemperature and DS2438 on the bit-level bus simulator.

use std::cell::RefCell;
use std::rc::Rc;

use super::tests::Env;
use super::*;
use crate::test_support::io_fakes::FakeBoard;
use crate::test_support::onewire_sim::{Event, Kind, OneWireSim, SimDevice};
use crate::test_support::stub_log::CallLog;

struct LineRig {
    ow: OwDevices,
    bus: LineBus<OneWireSim, FakeBoard>,
    board: FakeBoard,
    env: Env,
}

impl LineRig {
    fn new(devices: Vec<SimDevice>) -> Self {
        let board = FakeBoard::new();
        LineRig {
            ow: OwDevices::default(),
            bus: LineBus::new(OneWireSim::new(devices), board.clone()),
            board,
            env: Env {
                log: Rc::new(RefCell::new(CallLog::default())),
                locked: false,
            },
        }
    }

    fn sim(&mut self) -> &mut OneWireSim {
        self.bus.ow.line()
    }

    fn step(&mut self) {
        self.ow.loop_(&mut self.bus, &self.board, &mut self.env);
    }

    fn cycle(&mut self) -> u32 {
        self.env.log.borrow_mut().clear();
        for i in 1..=400 {
            self.step();
            if !self
                .env
                .log
                .borrow()
                .calls_of("app_temp_cycle_done")
                .is_empty()
            {
                return i;
            }
        }
        0
    }
}

fn devices() -> Vec<SimDevice> {
    vec![
        // 21.5 degC, -5.25 degC
        SimDevice::ds18b20(1, 344),
        SimDevice::ds18b20(2, -84),
        SimDevice::ds2438(3, 450),
        SimDevice::new(0x01, 4, Kind::Other),
    ]
}

#[test]
fn setup_enumerates_the_bus_in_rom_search_order() {
    let mut r = LineRig::new(devices());
    r.ow.setup(&mut r.bus, &r.board);
    let roms: Vec<DeviceAddress> = r.sim().devices.iter().map(|d| d.rom).collect();
    let s = r.ow.sensors;
    assert_eq!(s.no_of_devices, 4);
    assert_eq!(s.no_of_ds18_devices, 2);
    assert_eq!(s.no_of_ds2438_devices, 1);
    // serial 2 before serial 1 (bit 0 of byte 1), family 0x28 before 0x26 (bit 1 of byte 0)
    assert_eq!(s.tempsensors[0].address, roms[1]);
    assert_eq!(s.tempsensors[1].address, roms[0]);
    assert_eq!(s.voltsensors[0].address, roms[2]);
    // the library counted the devices and does not wait for conversions
    assert_eq!(r.bus.sensors.get_device_count(), 4);
    assert!(!r.bus.sensors.get_wait_for_conversion());
    assert_eq!(r.bus.sensors_device_count(), 4);
    assert_eq!(r.bus.sensors_resolution(), 12);
    // DS2438 begin took the first device of the bus
    assert_eq!(*r.bus.bm.address(), roms[1]);
}

#[test]
fn a_cycle_reads_the_temperatures_and_the_voltage_on_the_bus() {
    let mut r = LineRig::new(devices());
    r.ow.setup(&mut r.bus, &r.board);
    r.sim().events.clear();
    assert_eq!(r.cycle(), 104);
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, -53);
    assert_eq!(r.ow.sensors.tempsensors[1].temperature, 215);
    assert_eq!(r.ow.sensors.voltsensors[0].vad, 450);
    // one conversion started on every device, no wait in the library
    let converts: Vec<u32> = r.sim().devices.iter().map(|d| d.converts).collect();
    assert_eq!(converts, vec![1, 1, 1, 0]);
    let writes = r.sim().writes();
    assert_eq!(writes[..2], [0xCC, 0x44]);
    // the DS2438 conversion waited 10 ms
    assert_eq!(r.board.now_us(), 10_000);
}

#[test]
fn a_new_search_finds_a_device_added_on_the_bus() {
    let mut r = LineRig::new(devices());
    r.ow.setup(&mut r.bus, &r.board);
    r.sim().devices.push(SimDevice::ds18b20(5, 160));
    r.ow.temp_command(TEMP_CMD_NEWSEARCH, &mut r.env);
    // Init, Idle -> Search, begin, count, enumeration
    for _ in 0..5 {
        r.step();
    }
    assert_eq!(r.ow.sensors.no_of_devices, 5);
    assert_eq!(r.ow.sensors.no_of_ds18_devices, 3);
    assert_eq!(r.env.log.borrow().calls_of("app_match_sensors").len(), 1);
    assert_ne!(r.cycle(), 0);
    let t: Vec<i32> = r.ow.sensors.tempsensors[..3]
        .iter()
        .map(|t| t.temperature)
        .collect();
    // serial 2, serial 1, serial 5: 0b010 before 0b001 before 0b101 (bit 0, then bit 1)
    assert_eq!(t, vec![-53, 215, 100]);
}

#[test]
fn a_sensor_that_stops_answering_reports_the_hold_then_minus_1270() {
    let mut r = LineRig::new(vec![SimDevice::ds18b20(1, 344)]);
    r.ow.setup(&mut r.bus, &r.board);
    assert_ne!(r.cycle(), 0);
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, 215);
    r.sim().devices[0].present = false;
    for want in [215, 215, -1270] {
        assert_ne!(r.cycle(), 0);
        assert_eq!(r.ow.sensors.tempsensors[0].temperature, want);
    }
    // a read with a bad CRC is repeated at once
    r.sim().devices[0].present = true;
    r.sim().devices[0].bad_crc_reads = 1;
    assert_ne!(r.cycle(), 0);
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, 215);
}

#[test]
fn a_ds2438_without_presence_reads_minus_10_v() {
    let mut r = LineRig::new(vec![SimDevice::ds2438(1, 450)]);
    r.ow.setup(&mut r.bus, &r.board);
    assert!(r.bus.bm_begin());
    r.sim().devices[0].present = false;
    assert_ne!(r.cycle(), 0);
    assert_eq!(r.ow.sensors.voltsensors[0].vad, -1000);
    assert!(r.sim().events.contains(&Event::Reset(false)));
    r.sim().devices.clear();
    assert!(!r.bus.bm_begin());
}

#[test]
fn line_bus_search_and_reset_search_pass_through() {
    let mut r = LineRig::new(devices());
    let mut a = [0u8; 8];
    assert!(r.bus.search(&mut a));
    assert_eq!(a[0], 0x28);
    r.bus.reset_search();
    let mut b = [0u8; 8];
    assert!(r.bus.search(&mut b));
    assert_eq!(a, b);
    let t = r.bus.sensors_get_temp(&a);
    assert_eq!(t, -84 * 8);
    r.bus.sensors_set_wait_for_conversion(false);
    r.bus.sensors_request_temperatures();
    assert!(!r.bus.sensors.get_wait_for_conversion());
    let monitor = r.sim().devices[2].rom;
    r.bus.bm_set_address(&monitor);
    assert_eq!(*r.bus.bm.address(), monitor);
    assert_eq!(r.bus.bm_read_vad(), 4.5);
    r.bus.sensors_begin();
    assert_eq!(r.bus.sensors_device_count(), 4);
}
