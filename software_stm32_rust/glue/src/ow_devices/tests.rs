// Port of test/native/glue/test_owDevices.cpp on the fake 1-Wire libraries: enumeration, one
// temperature cycle, the lock and search commands, the JSON of the terminal.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;
use crate::test_support::io_fakes::{FakeBoard, FakeSerial};
use crate::test_support::ow_fakes::{dash_address, FakeOwBus};
use crate::test_support::stub_log::CallLog;

/// The link-seam stubs of owDevices.cpp (stub_app) and the lock (a variable of owDevices.cpp in
/// C++, so not logged).
pub(super) struct Env {
    pub log: Rc<RefCell<CallLog>>,
    pub locked: bool,
}

impl OwDevicesEnv for Env {
    fn app_temp_cycle_done(&mut self) {
        self.log.borrow_mut().log("app_temp_cycle_done()");
    }

    fn app_match_sensors(&mut self) -> i16 {
        self.log.borrow_mut().log("app_match_sensors()");
        0
    }

    fn temp_locked(&self) -> bool {
        self.locked
    }

    fn set_temp_lock(&mut self, locked: bool) {
        self.locked = locked;
    }
}

/// owDevices.cpp with the fake bus, the fake time, the debug UART and the stubs.
pub(super) struct Rig {
    pub ow: OwDevices,
    pub bus: FakeOwBus,
    pub board: FakeBoard,
    pub env: Env,
    pub dbg: FakeSerial,
    pub log: Rc<RefCell<CallLog>>,
}

impl Rig {
    pub fn new() -> Self {
        let log = Rc::new(RefCell::new(CallLog::default()));
        Rig {
            ow: OwDevices::default(),
            bus: FakeOwBus::new(log.clone()),
            board: FakeBoard::new(),
            env: Env {
                log: log.clone(),
                locked: false,
            },
            dbg: FakeSerial::new(),
            log,
        }
    }

    pub fn setup(&mut self) {
        self.ow.setup(&mut self.bus, &self.board);
    }

    pub fn set_device_address(&mut self) {
        self.ow.set_device_address(&mut self.bus, &self.board);
    }

    pub fn step(&mut self) {
        self.ow.loop_(&mut self.bus, &self.board, &mut self.env);
    }

    pub fn temp_command(&mut self, cmd: i32) {
        self.ow.temp_command(cmd, &mut self.env);
    }

    pub fn temp_locked(&self) -> bool {
        self.ow.temp_locked(&self.env)
    }

    pub fn scan_age(&mut self) -> u32 {
        self.ow.ow_scan_age_s(&self.board)
    }

    pub fn calls(&self) -> Vec<String> {
        self.log.borrow().calls.clone()
    }

    pub fn calls_of(&self, name: &str) -> Vec<String> {
        self.log.borrow().calls_of(name)
    }

    pub fn clear_calls(&self) {
        self.log.borrow_mut().clear();
    }

    /// runs the loop until the cycle reports its end, at most n calls; the number of calls
    pub fn cycle(&mut self, n: u32) -> u32 {
        for i in 1..=n {
            self.step();
            if !self.calls_of("app_temp_cycle_done").is_empty() {
                return i;
            }
        }
        0
    }

    pub fn print_sensordata(&mut self) -> String {
        self.ow.print_sensordata(&mut self.dbg);
        self.dbg.take_tx()
    }
}

fn joined_address(rom: &DeviceAddress, separator: &str) -> String {
    rom.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(separator)
}

pub(super) fn hex_address(rom: &DeviceAddress) -> String {
    joined_address(rom, " ")
}

#[test]
fn temperature_setup_ds18_sensors_and_ds2438_monitors_in_search_order_others_counted_only() {
    let mut r = Rig::new();
    let t0 = r.bus.add_one_wire(0x28, 1, 0);
    let bad = r.bus.add_one_wire(0x28, 2, 0);
    r.bus.devices[bad].rom[7] ^= 0xFF;
    r.bus.add_one_wire(0x01, 3, 0);
    let v0 = r.bus.add_one_wire(0x26, 4, 0);
    let t1 = r.bus.add_one_wire(0x10, 5, 0);
    r.setup();
    let s = &r.ow.sensors;
    assert_eq!(s.no_of_devices, 4);
    assert_eq!(s.no_of_ds18_devices, 2);
    assert_eq!(s.no_of_ds2438_devices, 1);
    assert_eq!(s.tempsensors[0].address, r.bus.devices[t0].rom);
    assert_eq!(s.tempsensors[1].address, r.bus.devices[t1].rom);
    assert_eq!(s.voltsensors[0].address, r.bus.devices[v0].rom);
    assert_eq!(s.tempsensors[0].temperature, -500);
    assert_eq!(s.tempsensors[2].address[0], 0);
    assert_eq!(r.bus.begins, 1);
    assert!(!r.bus.wait_for_conversion);
    assert_eq!(r.calls(), vec!["DS2438::begin(3)"]);
}

#[test]
fn temperature_loop_request_95_waits_one_sensor_per_call_the_ds2438_then_the_cycle_is_done() {
    let mut r = Rig::new();
    // 21.5 degC, -5.25 degC
    r.bus.add_one_wire(0x28, 1, 2752);
    r.bus.add_one_wire(0x28, 2, -672);
    let v0 = r.bus.add_one_wire(0x26, 3, 0);
    r.setup();
    let rom = r.bus.devices[v0].rom;
    r.bus.vad.insert(rom, 4.5);
    r.clear_calls();
    assert_eq!(r.cycle(200), 104);
    assert_eq!(r.bus.requests, 1);
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, 215);
    assert_eq!(r.ow.sensors.tempsensors[1].temperature, -53);
    assert_eq!(r.ow.sensors.voltsensors[0].vad, 450);
    assert_eq!(
        r.calls(),
        vec![
            format!("DS2438::setAddress({})", dash_address(&rom)),
            "DS2438::readVAD()".to_string(),
            "app_temp_cycle_done()".to_string(),
        ]
    );
    assert_eq!(r.bus.reads, 2);
}

#[test]
fn temperature_loop_no_request_while_locked_a_search_re_enumerates_and_matches_the_sensors() {
    let mut r = Rig::new();
    r.bus.add_one_wire(0x28, 1, 0);
    r.setup();
    r.temp_command(TEMP_CMD_LOCK);
    assert!(r.temp_locked());
    for _ in 0..10 {
        r.step();
    }
    assert_eq!(r.bus.requests, 0);
    r.temp_command(TEMP_CMD_UNLOCK);
    assert!(!r.temp_locked());
    // T_IDLE -> T_REQUEST
    r.step();
    assert_eq!(r.bus.requests, 0);
    r.step();
    assert_eq!(r.bus.requests, 1);
    // a search waits for the end of the cycle
    r.bus.add_one_wire(0x28, 2, 0);
    r.temp_command(TEMP_CMD_NEWSEARCH);
    r.clear_calls();
    assert_ne!(r.cycle(200), 0);
    for _ in 0..4 {
        r.step();
    }
    assert_eq!(r.ow.sensors.no_of_ds18_devices, 2);
    assert_eq!(r.calls_of("app_match_sensors").len(), 1);
    assert_eq!(r.scan_age(), 0);
}

#[test]
fn set_device_address_at_most_64_search_passes_at_most_34_ds18_sensors_stale_entries_cleared() {
    let mut r = Rig::new();
    for i in 0..70u8 {
        r.bus.add_one_wire(0x28, i, 0);
    }
    r.set_device_address();
    assert_eq!(r.bus.searches, 64);
    assert_eq!(r.ow.sensors.no_of_devices, 64);
    assert_eq!(usize::from(r.ow.sensors.no_of_ds18_devices), MAXONEWIRECNT);
    r.bus.devices.truncate(1);
    r.ow.sensors.tempsensors[0].temperature = 200;
    r.set_device_address();
    assert_eq!(r.ow.sensors.no_of_ds18_devices, 1);
    // the same sensor keeps its value
    assert_eq!(r.ow.sensors.tempsensors[0].temperature, 200);
    assert_eq!(r.ow.sensors.tempsensors[1].address[0], 0);
    assert_eq!(r.ow.sensors.tempsensors[1].temperature, -500);
}

#[test]
fn print_sensordata_the_json_of_the_terminal_command_getone() {
    let mut r = Rig::new();
    let a = r.bus.add_one_wire(0x28, 0xAB, 2752);
    let b = r.bus.add_one_wire(0x28, 0x01, -672);
    r.setup();
    r.ow.sensors.tempsensors[0].temperature = 215;
    r.ow.sensors.tempsensors[1].temperature = -53;
    let (ra, rb) = (r.bus.devices[a].rom, r.bus.devices[b].rom);
    assert_eq!(
        r.print_sensordata(),
        format!(
            "{{\"cnt\":2,\"sns\":[{{\"temp\":215,\"add\":\"{}\"}},{{\"temp\":-53,\"add\":\"{}\"}}]}}\r\n",
            hex_address(&ra),
            hex_address(&rb)
        )
    );
    r.bus.devices.clear();
    r.set_device_address();
    assert_eq!(r.print_sensordata(), "{\"cnt\":0}\r\n");
}
