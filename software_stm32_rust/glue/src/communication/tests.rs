// Port of test/native/glue/test_communication.cpp: today's v1 replies byte-exact against stub
// values, and the line handling of the loop.

use super::*;
use crate::test_support::io_fakes::{FakeBoard, FakeSerial, FakeSystem};
use crate::test_support::stubs::{Stubs, VALVE_SENSOR_UNKNOWN};

/// the -D flags of the C++ release env the glue suites build with
pub(super) const ID: FirmwareId = FirmwareId {
    version: b"2.1.7-revamped",
    tag: b"C2",
    build: b"1",
};

/// communication.cpp with the ESP and debug UARTs, the fake time and the stubs.
pub(super) struct Rig {
    pub comm: Communication,
    pub esp: FakeSerial,
    pub dbg: FakeSerial,
    pub board: FakeBoard,
    pub system: FakeSystem,
    pub stubs: Stubs,
}

/// the request and its reply, the calls it made
pub(super) struct Exchange {
    pub reply: String,
    pub calls: Vec<String>,
}

impl Rig {
    /// before communication_setup() (the boot window opened USART1)
    pub fn new() -> Self {
        Rig {
            comm: Communication::new(ID),
            esp: FakeSerial::new(),
            dbg: FakeSerial::new(),
            board: FakeBoard::new(),
            system: FakeSystem::default(),
            stubs: Stubs::default(),
        }
    }

    pub fn setup(&mut self) {
        self.comm.setup(&mut self.esp, &mut self.dbg);
    }

    /// communication_setup(), then the outputs and the call log cleared
    pub fn begin() -> Self {
        let mut r = Rig::new();
        r.setup();
        r.esp.take_tx();
        r.dbg.take_tx();
        r.stubs.calls.clear();
        r
    }

    /// one loop run; the RX interrupt counted the errors of the injected bytes
    pub fn run(&mut self) -> i16 {
        self.stubs.uart = self.esp.errors;
        self.comm.loop_(
            &mut self.esp,
            &mut self.dbg,
            &self.board,
            &self.system,
            &mut self.stubs,
        )
    }

    pub fn request_bytes(&mut self, line: &[u8]) -> String {
        self.esp.inject(line);
        self.run();
        self.esp.take_tx()
    }

    /// one loop run; what went to the ESP
    pub fn request(&mut self, line: &str) -> String {
        self.request_bytes(line.as_bytes())
    }

    pub fn exchange(&mut self, line: &str) -> Exchange {
        self.stubs.calls.clear();
        let reply = self.request(line);
        Exchange {
            reply,
            calls: self.stubs.calls.calls.clone(),
        }
    }

    pub fn calls(&self) -> Vec<String> {
        self.stubs.calls.calls.clone()
    }

    pub fn calls_of(&self, name: &str) -> Vec<String> {
        self.stubs.calls.calls_of(name)
    }
}

#[test]
fn communication_setup_bytes_of_the_boot_window_dropped() {
    let mut r = Rig::new();
    r.esp.inject(b"\xFF\x12garbage");
    r.setup();
    // C++ also checks PA10/PA9, 115200 and 8N1: the firmware sets USART1 up
    assert_eq!(r.esp.available(), 0);
    assert_eq!(r.dbg.take_tx(), "SERIAL_BUFFER_SIZE TX=1024 RX=1024\r\n");
    assert!(r.esp.take_tx().is_empty());
}

#[test]
fn v1_replies_gvers_gproto_gtgtp_stgtp() {
    let mut r = Rig::begin();
    assert_eq!(r.request("gvers\n"), "gvers 2.1.7-revamped_C2 1 \r\n");
    assert_eq!(r.request("gproto\n"), "gproto 3\r\n");
    r.stubs.valves[0].target_position = 30;
    assert_eq!(r.request("gtgtp 0\n"), "gtgtp 0 30 \r\n");
    assert_eq!(r.request("stgtp 0 50\n"), "stgtp\r\n");
    assert_eq!(r.stubs.valves[0].target_position, 50);
    assert_eq!(r.request("gtgtp 0\n"), "gtgtp 0 50 \r\n");
    assert_eq!(r.calls(), vec!["app_target_changed(0)"]);
}

#[test]
fn v1_replies_gvlvd_with_the_valve_and_its_sensors() {
    let mut r = Rig::begin();
    let v = &mut r.stubs.valves[0];
    v.actual_position = 30;
    v.meancurrent = 21;
    v.status = 1;
    v.calibration = true;
    v.opening_count = 3600;
    v.closing_count = 3610;
    v.deadzone_count = 10;
    v.calib_retries = 1;
    v.sensorindex1 = 2;
    v.sensorindex2 = VALVE_SENSOR_UNKNOWN;
    v.movements = 7;
    r.stubs.sensors.tempsensors[2].temperature = 215;
    assert_eq!(
        r.request("gvlvd 0\n"),
        "gvlvd 0 30 21 129 215 -500 7 3600 3610 10 1 \r\n"
    );
    assert!(r.request("gvlvd 12\n").is_empty());
}

#[test]
fn v1_replies_gstat_counts_the_dropped_and_malformed_lines() {
    let mut r = Rig::begin();
    r.stubs.sysstat.uptime = 7;
    r.stubs.sysstat.resets = 2;
    r.stubs.sysstat.reason = BootReason::Pin;
    r.stubs.eeprom.state = vdm_stm_core::replies_v2::EEP_STATE_PENDING;
    assert_eq!(r.request("gstat\n"), "gstat 7 2 2 0 0 1\r\n");
    // too many arguments
    assert!(r.request("stgtp 1 2 3 4 5 6\n").is_empty());
    assert_eq!(r.request("gstat\n"), "gstat 7 2 2 0 1 1\r\n");
}

#[test]
fn communication_loop_an_unknown_command_and_bad_arguments_get_no_reply() {
    let mut r = Rig::begin();
    assert!(r.request("xyzzy 1\n").is_empty());
    assert!(r.request("stgtp 12 50\n").is_empty());
    assert!(r.request("stgtp 0 101\n").is_empty());
    assert!(r.request("gtgtp\n").is_empty());
    assert!(r.calls().is_empty());
    assert_eq!(
        r.dbg.take_tx(),
        "set target pos\r\ninvalid arguments\r\nset target pos\r\ninvalid arguments\r\n\
         get target pos\r\ninvalid arguments\r\n"
    );
}

#[test]
fn communication_loop_at_most_4_requests_per_call_the_rest_stays_queued() {
    let mut r = Rig::begin();
    r.esp
        .inject_str("gtgtp 0\ngtgtp 1\ngtgtp 2\ngtgtp 3\ngtgtp 4\n");
    assert_eq!(r.run(), 0);
    assert_eq!(
        r.esp.take_tx(),
        "gtgtp 0 0 \r\ngtgtp 1 0 \r\ngtgtp 2 0 \r\ngtgtp 3 0 \r\n"
    );
    assert_eq!(r.esp.available(), 8);
    assert_eq!(r.run(), 0);
    assert_eq!(r.esp.take_tx(), "gtgtp 4 0 \r\n");
    assert_eq!(r.run(), -1);
}

#[test]
fn communication_loop_reads_at_most_512_bytes_per_call() {
    let mut r = Rig::begin();
    let before = r.esp.read_calls;
    r.esp.inject(&[b'a'; 513]);
    assert_eq!(r.run(), -1);
    assert_eq!(r.esp.read_calls - before, 512);
    assert_eq!(r.esp.available(), 1);
}

#[test]
fn communication_loop_an_idle_partial_line_is_dropped_after_more_than_100_ms() {
    let mut r = Rig::begin();
    r.esp.inject_str("gtgtp");
    r.run();
    r.board.advance_ms(100);
    // exactly 100 ms: kept
    r.run();
    assert_eq!(r.request(" 2\n"), "gtgtp 2 0 \r\n");
    r.esp.inject_str("gtgtp");
    r.run();
    r.board.advance_ms(101);
    r.dbg.take_tx();
    // 101 ms: dropped
    r.run();
    assert_eq!(r.dbg.take_tx(), "comm: incomplete line dropped\r\n");
    assert!(r.request(" 2\n").is_empty());
    assert_eq!(r.request("gstat\n"), "gstat 0 0 0 0 1 0\r\n");
}
