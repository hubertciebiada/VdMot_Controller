//! Port of test/native/test_stm_session__mut.cpp: StmSession details: sensor slot edges and
//! counts, the config, the queue limits, command arguments and events, UART bytes, service
//! moves, flash and reset edge cases.

use super::*;
use crate::common::{parse_one_wire_id, TEMP_POWER_ON, TEMP_READ_ERROR, VAD_FAILED};
use crate::config::MqttMode;
use crate::test_support::line_stm::AnswerCtx;
use crate::test_support::session_rig::{cmd, cmd0, name, Rig, Session, TestPort};
use crate::valve_model::{TargetSource, HEALTH_STALE};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::format;
use std::rc::Rc;
use std::string::{String, ToString};
use std::vec;
use std::vec::Vec;

const ID_A: &str = "28-84-37-94-97-ff-03-23";
const ID_B: &str = "28-aa-bb-cc-dd-ee-01-67";
const ID_C: &str = "28-11-22-33-44-55-66-77";
const ID_V: &str = "26-01-02-03-04-05-06-07";
const ID_W: &str = "26-0a-0b-0c-0d-0e-0f-10";

fn oid(s: &str) -> OneWireId {
    parse_one_wire_id(s.as_bytes()).expect("one-wire id")
}

fn text(e: &Event) -> String {
    String::from_utf8(e.text.to_vec()).expect("ASCII")
}

/// A sensor bus the STM reports: ids and readings per bus index.
#[derive(Default)]
struct Bus {
    temps: Vec<String>,
    /// id -> raw, absent: "goned 0"
    raw: BTreeMap<String, i32>,
    volts: Vec<String>,
    vad: BTreeMap<String, i32>,
}

impl Bus {
    fn list(cmd: &str, ids: &[String], l: &str) -> String {
        let out = format!("{cmd} {} ", ids.len());
        if l == cmd {
            return out;
        }
        out + &ids.join(",") + " "
    }

    fn index(l: &str) -> usize {
        l[6..].parse().expect("index")
    }

    fn attach(bus: &Rc<RefCell<Bus>>, r: &mut Rig) {
        let b = bus.clone();
        r.stm_mut().set_answer("gonec", move |l, _| {
            Bus::list("gonec", &b.borrow().temps, l)
        });
        let b = bus.clone();
        r.stm_mut().set_answer("gowvc", move |l, _| {
            Bus::list("gowvc", &b.borrow().volts, l)
        });
        let b = bus.clone();
        r.stm_mut().set_answer("goned", move |l, _| {
            let b = b.borrow();
            let i = Bus::index(l);
            match b.temps.get(i).and_then(|id| Some((id, b.raw.get(id)?))) {
                Some((id, raw)) => format!("goned {id} {raw} "),
                None => "goned 0".to_string(),
            }
        });
        let b = bus.clone();
        r.stm_mut().set_answer("gowvd", move |l, _| {
            let b = b.borrow();
            let i = Bus::index(l);
            match b.volts.get(i).and_then(|id| Some((id, b.vad.get(id)?))) {
                Some((id, vad)) => format!("gowvd {id} {vad} "),
                None => "gowvd 0".to_string(),
            }
        });
    }
}

fn sensor_events(p: &TestPort) -> Vec<Event> {
    p.events
        .iter()
        .filter(|e| {
            matches!(
                e.code,
                EventCode::TempSensorFailed
                    | EventCode::TempSensorRecovered
                    | EventCode::VoltSensorFailed
                    | EventCode::SensorCountChanged
            )
        })
        .cloned()
        .collect()
}

/// Temps A (slot 1), B (slot 2) and C (slot 3, inactive); volts V (slot 1), W (slot 2).
struct SensorRig {
    r: Rig,
    bus: Rc<RefCell<Bus>>,
}

impl SensorRig {
    fn new() -> Self {
        let mut r = Rig::new(3, 0x001);
        r.cfg.temps[0].active = true;
        r.cfg.temps[0].id = oid(ID_A);
        r.cfg.temps[1].active = true;
        r.cfg.temps[1].id = oid(ID_B);
        r.cfg.temps[2].active = false;
        r.cfg.temps[2].id = oid(ID_C);
        r.cfg.temps[3].active = true; // active without an id
        r.cfg.volts[0].active = true;
        r.cfg.volts[0].id = oid(ID_V);
        r.cfg.volts[1].active = true;
        r.cfg.volts[1].id = oid(ID_W);
        let bus = Rc::new(RefCell::new(Bus {
            temps: vec![ID_A.to_string(), ID_B.to_string(), ID_C.to_string()],
            raw: [(ID_A, 215), (ID_B, 198), (ID_C, 170)]
                .map(|(k, v)| (k.to_string(), v))
                .into(),
            volts: vec![ID_V.to_string(), ID_W.to_string()],
            vad: [(ID_V, 1200), (ID_W, 1300)]
                .map(|(k, v)| (k.to_string(), v))
                .into(),
        }));
        Bus::attach(&bus, &mut r);
        r.start();
        Self { r, bus }
    }

    fn raw(&self, id: &str, v: i32) {
        self.bus.borrow_mut().raw.insert(id.to_string(), v);
    }

    fn vad(&self, id: &str, v: i32) {
        self.bus.borrow_mut().vad.insert(id.to_string(), v);
    }
}

fn service_move(v: u8, counts: u16) -> StmCommand {
    let mut c = cmd(StmCommandType::ServiceMove, v);
    c.dir = crate::stm_codec::MoveDir::Open;
    c.counts = counts;
    c.max_ma = 40;
    c
}

// ================================================================ sensors

#[test]
fn sensor_slots_settled_after_the_grace_readings_in_the_snapshot() {
    let mut t = SensorRig::new();
    let r = &mut t.r;
    r.run(20000);
    assert!(!r.port().last.sensors_settled);
    r.run(20000);
    let last = &r.port().last;
    assert!(last.sensors_settled);
    assert_eq!(last.temp_count, 3);
    assert_eq!(last.temps[0].raw, 215);
    assert_eq!(last.temps[1].raw, 198);
    assert_eq!(last.temps[2].raw, 170);
    assert_eq!(last.volt_count, 2);
    assert_eq!(last.volts[0].vad, 1200);
    assert_eq!(last.volts[1].vad, 1300);
    assert!(sensor_events(r.port()).is_empty());
    // A bus scan starts the grace again.
    r.command(&cmd0(StmCommandType::ScanSensors));
    r.run(1500);
    assert!(!r.port().last.sensors_settled);
    r.run(40000);
    assert!(r.port().last.sensors_settled);
}

#[test]
fn temp_sensor_failure_and_recovery_per_slot_the_id_in_the_text() {
    let mut t = SensorRig::new();
    t.r.run(45000);
    assert!(sensor_events(t.r.port()).is_empty());
    t.raw(ID_A, i32::from(TEMP_READ_ERROR));
    t.raw(ID_C, i32::from(TEMP_READ_ERROR)); // inactive slot: silent
    t.r.run(40000);
    let ev = sensor_events(t.r.port());
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].code, EventCode::TempSensorFailed);
    assert_eq!(ev[0].arg1, 1);
    assert_eq!(ev[0].arg2, i32::from(TEMP_READ_ERROR));
    assert_eq!(text(&ev[0]), ID_A);
    t.raw(ID_A, 216);
    t.r.run(30000);
    let ev = sensor_events(t.r.port());
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[1].code, EventCode::TempSensorRecovered);
    assert_eq!(ev[1].arg1, 1);
    assert!(text(&ev[1]).is_empty());
    // Slot 2 (bus index 1) fails.
    t.raw(ID_B, i32::from(TEMP_POWER_ON));
    t.r.run(40000);
    let ev = sensor_events(t.r.port());
    assert_eq!(ev.len(), 3);
    assert_eq!(ev[2].code, EventCode::TempSensorFailed);
    assert_eq!(ev[2].arg1, 2);
    assert_eq!(ev[2].arg2, i32::from(TEMP_POWER_ON));
    assert_eq!(text(&ev[2]), ID_B);
}

#[test]
fn a_temp_sensor_that_stops_answering_fails_after_60_s() {
    let mut t = SensorRig::new();
    let r = &mut t.r;
    r.run(45000);
    r.stm_mut().set_answer("goned", |_, _| String::new());
    r.run(55000);
    r.run(30000);
    let ev = sensor_events(r.port());
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[0].code, EventCode::TempSensorFailed);
    assert_eq!(ev[1].code, EventCode::TempSensorFailed);
    assert_eq!(ev[0].arg1 + ev[1].arg1, 3);
}

#[test]
fn volt_sensor_failures_per_slot() {
    let mut t = SensorRig::new();
    t.r.run(45000);
    t.vad(ID_V, VAD_FAILED);
    t.r.run(40000);
    let ev = sensor_events(t.r.port());
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].code, EventCode::VoltSensorFailed);
    assert_eq!(ev[0].arg1, 1);
    assert_eq!(ev[0].arg2, VAD_FAILED);
    t.vad(ID_V, 1100);
    t.r.run(30000);
    assert_eq!(sensor_events(t.r.port()).len(), 1); // no recovery event for volts
    t.vad(ID_W, VAD_FAILED - 5);
    t.r.run(40000);
    let ev = sensor_events(t.r.port());
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[1].arg1, 2);
    assert_eq!(ev[1].arg2, VAD_FAILED - 5);
}

#[test]
fn a_volt_sensor_that_stops_answering_fails_after_60_s() {
    let mut t = SensorRig::new();
    let r = &mut t.r;
    r.run(45000);
    r.stm_mut().set_answer("gowvd", |_, _| String::new());
    r.run(55000);
    r.run(30000);
    let ev = sensor_events(r.port());
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[0].code, EventCode::VoltSensorFailed);
    assert_eq!(ev[1].code, EventCode::VoltSensorFailed);
}

#[test]
fn sensor_count_changes_are_logged_per_bus_once_known() {
    let mut t = SensorRig::new();
    t.r.run(45000);
    assert!(sensor_events(t.r.port()).is_empty());
    t.bus.borrow_mut().temps = vec![ID_A.to_string(), ID_B.to_string()];
    t.r.run(35000);
    let ev = t.r.port().with_code(EventCode::SensorCountChanged);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 2);
    assert_eq!(ev[0].arg2, 0);
    t.bus.borrow_mut().volts = vec![ID_V.to_string()];
    t.r.run(35000);
    let ev = t.r.port().with_code(EventCode::SensorCountChanged);
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[1].arg1, 1);
    assert_eq!(ev[1].arg2, 1);
}

#[test]
fn changed_slot_ids_re_read_the_valve_sensors_unchanged_ones_do_not() {
    let mut t = SensorRig::new();
    let r = &mut t.r;
    r.run(45000);
    let n = r.count("gvlon");
    let cfg = r.cfg.clone();
    r.s.apply_config(&cfg, true);
    r.run(3000);
    assert_eq!(r.count("gvlon"), n);
    r.cfg.temps[1].id = oid(ID_C);
    let cfg = r.cfg.clone();
    r.s.apply_config(&cfg, true);
    r.run(3000);
    assert_eq!(r.count("gvlon"), n + 1);
}

// ================================================================ config, targets, publishing

#[test]
fn only_configured_valves_are_active() {
    let mut r = Rig::new(3, 0x002);
    r.start();
    r.run(40000);
    assert!(r.count("gvlvy 1") >= 3);
    assert!(r.count("gvlvy 0") < r.count("gvlvy 1") / 2); // inactive: every 30 s
    assert!(r.port().last.valves[1].known);
    let t0 = r.target(0, 20, TargetSource::Web);
    let t1 = r.target(1, 20, TargetSource::Web);
    r.command(&t0);
    r.command(&t1);
    r.run(3000);
    assert_eq!(r.stm().lines_of("stgtp"), ["stgtp 1 20"]);
}

#[test]
fn config_a_second_begin_and_rejected_targets_publish_without_a_model_change() {
    let mut r = Rig::new(3, 0x001);
    let cfg = r.cfg.clone();
    r.s.apply_config(&cfg, true);
    r.s.begin(
        r.now,
        &PersistedTargets::default(),
        RestoreSource::None,
        None,
    );
    r.s.publish_if_due(r.now);
    let mut p = r.port().publishes;
    r.now += 200;
    r.s.publish_if_due(r.now);
    assert_eq!(r.port().publishes, p);
    r.s.apply_config(&cfg, true);
    r.now += 200;
    r.s.publish_if_due(r.now);
    p += 1;
    assert_eq!(r.port().publishes, p);
    r.s.begin(
        r.now,
        &PersistedTargets::default(),
        RestoreSource::None,
        None,
    );
    r.now += 200;
    r.s.publish_if_due(r.now);
    p += 1;
    assert_eq!(r.port().publishes, p);
    let t = r.target(1, 30, TargetSource::Web); // inactive valve
    r.command(&t);
    r.now += 200;
    r.s.publish_if_due(r.now);
    p += 1;
    assert_eq!(r.port().publishes, p);
    for (v, pos) in [(VALVE_COUNT, 30), (VALVE_COUNT - 1, 30), (0, 101)] {
        let t = r.target(v, pos, TargetSource::Web);
        r.command(&t);
    }
    let ev = r.port().with_code(EventCode::MqttCommandRejected);
    assert_eq!(ev.len(), 4);
    assert_eq!(ev[0].arg1, 2);
    assert_eq!(ev[1].arg1, 0);
    assert_eq!(ev[2].arg1, 12);
    assert_eq!(ev[3].arg1, 1);
    for e in &ev {
        assert_eq!(e.arg2, 0);
        assert_eq!(e.valve, NO_VALVE);
        assert_eq!(text(e), "rejected by model");
    }
}

// ================================================================ queue limits

/// Fills the User part of the queue with distinct service moves of valve 0.
fn fill_queue(r: &mut Rig) {
    let mut counts: u16 = 100;
    while r.s.link().queued_with(Priority::User) + r.s.link().queued_with(Priority::Config)
        < LinkPolicy::QUEUE_CAPACITY
    {
        r.command(&service_move(0, counts));
        counts += 1;
        assert!(counts < 200);
    }
}

#[test]
fn a_full_queue_refuses_commands_and_logs_the_command() {
    let mut r = Rig::new(3, 0x0FF);
    r.start();
    r.run(10000);
    r.stm_mut().silent = true;
    fill_queue(&mut r);
    assert!(!r.port().has(EventCode::StmQueueFull));
    let mut c = cmd(StmCommandType::Calibrate, 3);
    c.scheduled = true;
    c.attempt = 5;
    r.command(&c);
    let calibs = &r.port().calibs;
    assert_eq!(calibs.len(), 1);
    assert_eq!(calibs[0].attempt, 5);
    assert!(!calibs[0].ok);
    assert_eq!(calibs[0].reason, CalibFailure::NotSent);
    let ev = r.port().with_code(EventCode::StmQueueFull);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, Cmd::Staln as i32);
    r.command(&cmd(StmCommandType::Assembly, 4));
    r.s.publish_if_due(r.now + 200);
    assert_ne!(r.port().last.valves[4].source, TargetSource::Assembly);
    let ev = r.port().with_code(EventCode::StmQueueFull);
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[1].arg1, Cmd::Staop as i32);
    // Motor settings: all or nothing.
    let mut m = cmd0(StmCommandType::SetMotorSettings);
    m.has_motor = true;
    r.command(&m);
    let ev = r.port().with_code(EventCode::StmQueueFull);
    assert_eq!(ev.len(), 3);
    assert_eq!(ev[2].arg1, Cmd::Smotc as i32);
}

#[test]
fn a_target_push_that_finds_the_queue_full_is_retried_later() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.stm_mut().silent = true;
    fill_queue(&mut r);
    let t = r.target(0, 30, TargetSource::Web);
    r.command(&t);
    r.run(100);
    assert_eq!(r.count("stgtp"), 0);
    r.stm_mut().silent = false;
    r.run(15000);
    assert_eq!(r.stm().target[0], 30);
}

#[test]
fn motor_settings_need_room_for_every_line() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.stm_mut().silent = true;
    r.command(&service_move(0, 99)); // outstanding
    r.run(4);
    let t = r.target(0, 30, TargetSource::Web); // a queued Config line (stgtp)
    r.command(&t);
    r.run(4);
    assert!(r.s.link().queued_with(Priority::Config) >= 1);
    let mut counts: u16 = 100;
    while r.s.link().queued_with(Priority::User) + r.s.link().queued_with(Priority::Config)
        < LinkPolicy::QUEUE_CAPACITY - 2
    {
        r.command(&service_move(0, counts));
        counts += 1;
    }
    let mut m = cmd0(StmCommandType::SetMotorSettings);
    m.has_motor = true;
    m.has_learn_movements = true;
    m.learn_movements = 100;
    m.has_breakaway = true;
    r.command(&m); // three lines, room for two
    let ev = r.port().with_code(EventCode::StmQueueFull);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, Cmd::Smotc as i32); // refused as a whole (Rust addition)
    m.has_breakaway = false;
    r.command(&m); // two lines fit
    assert_eq!(r.port().with_code(EventCode::StmQueueFull).len(), 1);
    assert_eq!(
        r.s.link().queued_with(Priority::User) + r.s.link().queued_with(Priority::Config),
        LinkPolicy::QUEUE_CAPACITY
    );
}

#[test]
fn motor_settings_each_part_alone_breakaway_only_on_protocol_2_plus() {
    for part in 0..3 {
        let mut r = Rig::new(2, 0x001);
        r.start();
        r.run(10000);
        let gmotc = r.count("gmotc");
        let mut m = cmd0(StmCommandType::SetMotorSettings);
        m.has_motor = part == 0;
        m.has_learn_movements = part == 1;
        m.learn_movements = 120;
        m.has_breakaway = part == 2;
        r.command(&m);
        r.run(2000);
        assert_eq!(r.count("smotc") > 0, part == 0, "part {part}");
        assert_eq!(r.count("stlnm") > 0, part == 1, "part {part}");
        assert_eq!(r.count("scalx") > 0, part == 2, "part {part}");
        assert!(r.count("gmotc") > gmotc, "part {part}"); // read back
    }
    let mut old = Rig::new(1, 0x001);
    old.start();
    old.run(15000);
    let gmotc = old.count("gmotc");
    let mut m = cmd0(StmCommandType::SetMotorSettings);
    m.has_breakaway = true;
    old.command(&m);
    old.run(2000);
    assert_eq!(old.count("scalx"), 0);
    assert_eq!(old.count("gmotc"), gmotc); // nothing sent: nothing to read back
}

// ================================================================ commands

#[test]
fn assembly_detect_valve_sensors_and_profiles_reach_the_stm() {
    let mut r = Rig::new(3, 0x007);
    r.start();
    r.run(10000);
    r.command(&cmd0(StmCommandType::Detect));
    let mut vs = cmd(StmCommandType::SetValveSensors, 1);
    vs.ids = [oid(ID_A), oid(ID_B)];
    r.command(&vs);
    r.command(&cmd(StmCommandType::RequestProfile, 2));
    r.run(15000);
    assert_eq!(r.count("stdet"), 1);
    assert!(!r.stm().lines_of("stvls").is_empty());
    assert_eq!(r.count("masns"), 1);
    assert!(r.count("gprof 2") >= 1);
}

#[test]
fn service_moves_on_protocol_2_plus_done_event_with_the_counted_move() {
    let mut r = Rig::new(3, 0x003);
    r.start();
    r.run(10000);
    r.command(&service_move(1, 300));
    r.run(3000);
    let l = r.stm().lines_of("svmov");
    assert_eq!(l.len(), 1);
    assert!(l[0].starts_with("svmov 1 "));
    // The STM reports a new move sequence for valve 1 (counts 412, stop 3).
    let mut old = Rig::new(1, 0x003);
    old.start();
    old.run(15000);
    old.command(&service_move(1, 300));
    old.run(2000);
    assert_eq!(old.count("svmov"), 0);
}

#[test]
fn rejected_and_unanswered_service_moves_are_logged() {
    let mut r = Rig::new(3, 0x003);
    r.start();
    r.run(10000);
    r.stm_mut().set_answer("svmov", |l, _| {
        if l.starts_with("svmov 1 ") {
            "svmov 1 err 3".to_string()
        } else {
            String::new()
        }
    });
    r.command(&service_move(1, 300));
    r.run(2000);
    let ev = r.port().with_code(EventCode::ServiceMoveDone);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].valve, 1);
    assert_eq!(ev[0].severity, Severity::Warning);
    assert_eq!(ev[0].arg1, -1);
    assert_eq!(ev[0].arg2, 3);
    assert_eq!(text(&ev[0]), "rejected");
    r.command(&service_move(0, 300));
    r.run(3000);
    let ev = r.port().with_code(EventCode::ServiceMoveDone);
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[1].valve, 0);
    assert_eq!(ev[1].arg1, -1);
    assert_eq!(ev[1].arg2, -1);
    assert_eq!(text(&ev[1]), "no reply");
}

#[test]
fn a_rejected_target_of_an_unsupported_stm_still_reads_the_targets_back() {
    let mut r = Rig::new(1, 0x006);
    r.stm_mut().too_old = true;
    r.start();
    r.run(10000);
    assert!(r.count("gtgtp 1") >= 1);
    assert!(r.count("gtgtp 2") >= 1);
    assert_eq!(r.count("gtgtp 0"), 0);
    r.command(&cmd0(StmCommandType::ScanSensors));
    r.command(&cmd(StmCommandType::StopValve, 1));
    r.run(1000);
    assert_eq!(r.count("stons"), 0);
}

// ================================================================ UART bytes

#[test]
fn replies_as_uart_bytes_cr_lf_lines_several_per_read() {
    let mut r = Rig::new(3, 0x001);
    r.via_rx = true;
    r.start();
    r.run(10000);
    assert_eq!(r.port().last.link, LinkState::Up);
    assert_eq!(r.port().last.proto, 3);
    assert!(r.port().last.valves[0].known);
    assert_eq!(r.s.link().stats().parse_errors, 0);
    // Two stray replies in one read, the second split over two reads.
    let now = r.now;
    r.s.on_rx(b"gtgtp 0 61 \r\ngtgtp 0 62 \r\ngtgt", now);
    r.s.on_rx(b"p 0 63 \r\n", now);
    r.s.publish_if_due(now + 200);
    assert_eq!(r.port().last.valves[0].stm_target, 63);
    assert!(r.s.link().stats().stray_lines >= 3);
    r.s.on_rx(b"junk\r\n", now);
    assert_eq!(r.s.link().stats().parse_errors, 1);
}

// ================================================================ commands on older STMs

#[test]
fn an_unsupported_stm_a_manual_calibration_reports_nothing() {
    let mut r = Rig::new(1, 0x001);
    r.stm_mut().too_old = true;
    r.start();
    r.run(10000);
    assert_eq!(r.port().last.support, StmSupport::TooOld);
    r.command(&cmd(StmCommandType::Calibrate, 0));
    r.run(1000);
    assert!(r.port().calibs.is_empty());
    assert_eq!(r.count("staln"), 0);
}

#[test]
fn service_moves_need_protocol_2_the_valve_sensors_line_carries_both_ids() {
    let mut r = Rig::new(2, 0x003);
    r.start();
    r.run(10000);
    r.command(&service_move(1, 300));
    let mut vs = cmd(StmCommandType::SetValveSensors, 1);
    vs.ids = [oid(ID_A), oid(ID_B)];
    r.command(&vs);
    r.run(2000);
    assert!(r.count("svmov") >= 1);
    let l = r.stm().lines_of("stvls");
    assert!(!l.is_empty());
    assert_eq!(l[0], format!("stvls 1 {ID_A} {ID_B}"));
}

#[test]
fn stm_version_exactly_the_minimum_is_compatible() {
    let mut r = Rig::new(3, 0x001);
    r.stm_mut().version = "1.4.0_C2 1712345678 ".to_string();
    r.start();
    r.run(10000);
    assert!(r.port().last.compatible);
    assert_eq!(r.port().last.support, StmSupport::Supported);
    assert!(!r.port().has(EventCode::StmIncompatible));
    let v = r.port().with_code(EventCode::StmVersion);
    assert_eq!(v.len(), 1);
    assert_eq!(text(&v[0]), "1.4.0_C2");
}

// ================================================================ resets and flashing

#[test]
fn a_user_reset_counts_as_a_user_reset_not_by_policy() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.command(&cmd0(StmCommandType::ResetStm));
    r.run(1000);
    assert_eq!(r.port().pulses.len(), 1);
    assert_eq!(r.s.link().stats().user_resets, 1);
    assert_eq!(r.s.link().stats().policy_resets, 0);
}

fn put32(v: &mut [u8], off: usize, x: u32) {
    v[off..off + 4].copy_from_slice(&x.to_le_bytes());
}

/// An image the flasher accepts for board `hw` (0x80 body, vectors, handshake, version and
/// board tag).
fn image(hw: &str) -> Vec<u8> {
    let mut v = vec![0x80u8; 8192];
    put32(&mut v, 0, 0x2002_0000);
    put32(&mut v, 4, 0x0800_01C5);
    let mut s = b"\x01DEADBEEF\0\x01BEEFIT\0\x012.1.0-revamped\0\x01".to_vec();
    assert_eq!(s.len(), 35);
    s.extend_from_slice(b"VDM-HW:");
    s.extend_from_slice(hw.as_bytes());
    v[6000..6000 + s.len()].copy_from_slice(&s);
    v[6000 + s.len()] = 0;
    v
}

fn flash_cmd(image: &str, blank: bool) -> StmCommand {
    let mut c = cmd0(StmCommandType::StartFlash);
    c.image = name(image);
    c.blank = blank;
    c
}

#[test]
fn a_blank_flash_runs_to_the_end_events_last_good_copy_re_sync() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.port_mut().image.data = image("C2");
    r.sim.boot_pin_resets = 1; // blank: the STM starts in the ROM bootloader
    r.command(&flash_cmd("new.bin", true));
    assert!(r.s.flashing());
    let ev = r.port().with_code(EventCode::StmFlashStarted);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 8192);
    assert_eq!(ev[0].arg2, 0);
    assert_eq!(text(&ev[0]), "new.bin");
    // Nothing else while flashing.
    r.command(&flash_cmd("other.bin", true));
    let ev = r.port().with_code(EventCode::StmFlashFailed);
    assert_eq!(ev.len(), 1);
    assert_eq!(text(&ev[0]), "busy");
    assert_eq!(ev[0].arg1, 0);
    assert_eq!(ev[0].arg2, 0);
    r.command(&cmd0(StmCommandType::ResetStm));
    let ev = r.port().with_code(EventCode::StmFlashFailed);
    assert_eq!(ev.len(), 2);
    assert_eq!(text(&ev[1]), "reset refused");
    assert_eq!(ev[1].arg1, 0);
    assert_eq!(ev[1].arg2, 0);
    let mut c = cmd(StmCommandType::Calibrate, 0);
    c.scheduled = true;
    c.attempt = 3;
    r.command(&c);
    let calibs = &r.port().calibs;
    assert_eq!(calibs.len(), 1);
    assert_eq!(calibs[0].attempt, 3);
    assert!(!calibs[0].ok);
    assert_eq!(calibs[0].reason, CalibFailure::NotSent);
    assert!(r.run_until(|r| !r.s.flashing(), 120000));
    let ev = r.port().with_code(EventCode::StmFlashDone);
    assert_eq!(ev.len(), 1);
    assert!(ev[0].arg1 > 0);
    assert_eq!(ev[0].arg2, 0);
    assert_eq!(r.port().last_good, ["new.bin"]);
    assert_eq!(r.port().closed, 1);
    assert_eq!(r.sim.configs.last(), Some(&(115_200, false)));
    r.run(10000);
    assert_eq!(r.port().last.link, LinkState::Up);
    assert_eq!(r.port().last.flash.phase, FlashPhase::Done);
}

#[test]
fn flash_refusals_carry_no_arguments_a_pending_flash_is_published() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.stm_mut().eep = 0;
    r.port_mut().restart = true;
    r.command(&flash_cmd("a.bin", false));
    let ev = r.port().with_code(EventCode::StmFlashFailed);
    assert_eq!(ev.len(), 1);
    assert_eq!(text(&ev[0]), "restart pending");
    assert_eq!(ev[0].arg1, 0);
    assert_eq!(ev[0].arg2, 0);
    r.port_mut().restart = false;
    r.s.publish_if_due(r.now + 200);
    r.command(&flash_cmd("a.bin", false));
    r.s.publish_if_due(r.now + 400);
    assert!(r.port().last.flash_pending);
    r.command(&cmd0(StmCommandType::AbortFlash));
    r.s.publish_if_due(r.now + 600);
    assert!(!r.port().last.flash_pending);
    // An unreadable image: the name in the text, no address.
    r.port_mut().image_ok = false;
    r.command(&flash_cmd("gone.bin", true));
    let ev = r.port().with_code(EventCode::StmFlashFailed);
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[1].arg1, FlashError::ImageRead as i32);
    assert_eq!(ev[1].arg2, 0);
    assert_eq!(text(&ev[1]), "gone.bin");
    assert!(!r.s.flashing());
    r.run(3000);
    assert_eq!(r.port().last.link, LinkState::Up);
}

#[test]
fn without_a_running_stm_the_chosen_board_is_used() {
    let mut r = Rig::new(3, 0x001);
    r.stm_mut().silent = true;
    r.start();
    r.run(1000);
    r.port_mut().image.data = image("C1");
    r.sim.boot_pin_resets = 1;
    let mut c = flash_cmd("c1.bin", true);
    c.board = name("C1");
    r.command(&c);
    assert!(r.s.flashing());
    r.run_until(|r| !r.s.flashing(), 120000);
    assert_eq!(r.port().with_code(EventCode::StmFlashDone).len(), 1);
    // An invalid board choice leaves the board open: the tagged image is refused.
    let mut q = Rig::new(3, 0x001);
    q.stm_mut().silent = true;
    q.start();
    q.run(1000);
    q.port_mut().image.data = image("C1");
    q.sim.boot_pin_resets = 1;
    let mut c = flash_cmd("c1.bin", true);
    c.board = name("X1");
    q.command(&c);
    q.run_until(|q| !q.s.flashing(), 120000);
    assert!(q.port().with_code(EventCode::StmFlashDone).is_empty());
}

#[test]
fn flash_progress_is_published_while_flashing() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.port_mut().image.data = image("C2");
    r.sim.boot_pin_resets = 1;
    r.command(&flash_cmd("new.bin", true));
    let mut seen: Vec<FlashPhase> = Vec::new();
    r.run_until(
        |r| {
            let phase = r.port().last.flash.phase;
            if seen.last() != Some(&phase) {
                seen.push(phase);
            }
            !r.s.flashing()
        },
        120000,
    );
    assert!(seen.len() >= 4);
}

// ================================================================ replies

/// gvlvy reply of the LineStm layout with a chosen status and last move.
fn gvlvy(v: u8, status: u32, target: u8, counted: u32, stop: u32) -> String {
    format!(
        "gvlvy {v} {status} 40 {target} 21 3120 3350 -230 0 57 0 0 0 1 3000 {counted} {stop} 412 \
         8123 0 0 50 {target} 0 0"
    )
}

/// The valve number of a `<cmd> <valve>` request.
fn valve_of(l: &str) -> u8 {
    l[6..].parse().expect("valve")
}

#[test]
fn stray_list_and_reading_replies() {
    let mut t = SensorRig::new();
    let r = &mut t.r;
    r.run(45000);
    assert_eq!(r.port().last.temp_count, 3);
    let tlists0 = r.count("gonec"); // Rust addition: the stray temp count asks for nothing too
    r.s.on_line(b"gonec 1 ", r.now);
    let vlists0 = r.count("gowvc");
    r.s.on_line(b"gowvc 0 ", r.now);
    r.s.publish_if_due(r.now + 200);
    assert_eq!(r.port().last.temp_count, 3);
    assert_eq!(r.port().last.volt_count, 2);
    r.run(1000);
    // a stray count asks for nothing
    assert_eq!(r.count("gowvc"), vlists0);
    assert_eq!(r.count("gonec"), tlists0);
    // A stray reading of a known id lands on its index; an unknown id asks for the list.
    let lists = r.count("gonec");
    let known = format!("goned {ID_B} 205 ");
    r.s.on_line(known.as_bytes(), r.now);
    r.s.publish_if_due(r.now + 400);
    assert_eq!(r.port().last.temps[1].raw, 205);
    r.run(1000);
    assert_eq!(r.count("gonec"), lists);
    r.s.on_line(b"goned 28-01-01-01-01-01-01-01 205 ", r.now);
    r.run(1000);
    assert_eq!(r.count("gonec"), lists + 1);
    r.s.on_line(b"goned 0", r.now);
    r.run(1000);
    assert_eq!(r.count("gonec"), lists + 1);
    let vlists = r.count("gowvc");
    let vknown = format!("gowvd {ID_W} 1250 ");
    r.s.on_line(vknown.as_bytes(), r.now);
    r.s.publish_if_due(r.now + 200);
    assert_eq!(r.port().last.volts[1].vad, 1250);
    r.run(1000);
    assert_eq!(r.count("gowvc"), vlists);
    r.s.on_line(b"gowvd 26-01-01-01-01-01-01-01 1250 ", r.now);
    r.run(1000);
    assert_eq!(r.count("gowvc"), vlists + 1);
    r.s.on_line(b"gowvd 0", r.now);
    r.run(1000);
    assert_eq!(r.count("gowvc"), vlists + 1);
}

#[test]
fn stray_valve_states_are_ignored_motor_and_breakaway_are_published() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    assert!(r.port().last.have_motor);
    assert_eq!(r.port().last.motor.min_counts, 3000);
    assert!(r.port().last.have_breakaway);
    assert_eq!(r.port().last.learn_movements, 2000);
    let status = r.port().last.valves[0].status;
    assert_eq!(status, 1);
    r.s.on_line(b"gvlst 12 9,9,9,9,9,9,9,9,9,9,9,9, ", r.now);
    r.s.publish_if_due(r.now + 200);
    assert_eq!(r.port().last.valves[0].status, status);
}

#[test]
fn a_v1_valve_that_lost_its_counts_is_an_stm_reboot() {
    let mut r = Rig::new(1, 0x001);
    r.start();
    r.run(15000);
    assert!(!r.port().has(EventCode::StmRebootDetected));
    r.stm_mut().set_answer("gvlvd", |l, _| {
        format!("gvlvd {} 0 0 5 -500 -500 0 0 0 0 0 ", &l[6..])
    });
    r.run(5000);
    let ev = r.port().with_code(EventCode::StmRebootDetected);
    assert!(!ev.is_empty());
    assert_eq!(ev[0].arg1, 3);
}

#[test]
fn no_status_after_a_link_recovery_is_an_stm_reboot() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(15000);
    r.stm_mut().silent = true;
    r.run(8000);
    r.stm_mut().set_answer("gstax", |_, _| String::new());
    r.stm_mut().silent = false;
    r.run(8000);
    let ev = r.port().with_code(EventCode::StmRebootDetected);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 4);
}

#[test]
fn stm_counters_baseline_first_then_increases_per_side() {
    let mut r = Rig::new(3, 0x001);
    let ovf = Rc::new(Cell::new(5u32));
    let perr = Rc::new(Cell::new(3u32));
    let (o, p) = (ovf.clone(), perr.clone());
    r.stm_mut().set_answer("gstax", move |_, c: &AnswerCtx| {
        format!(
            "gstax {} 3 2 {} {} 1 1 3540 1 60 0 0 0 0 0 0 0 0 0 0 0 0 0",
            100 + c.now / 1000,
            o.get(),
            p.get()
        )
    });
    r.start();
    r.run(15000);
    assert!(!r.port().has(EventCode::StmRxOverflow));
    assert!(!r.port().has(EventCode::StmParseErrors));
    ovf.set(7);
    perr.set(4);
    r.run(12000);
    let o = r.port().with_code(EventCode::StmRxOverflow);
    assert_eq!(o.len(), 1);
    assert_eq!(o[0].arg1, 7);
    assert_eq!(o[0].arg2, 1);
    let p = r.port().with_code(EventCode::StmParseErrors);
    assert_eq!(p.len(), 1);
    assert_eq!(p[0].arg1, 4);
    assert_eq!(p[0].arg2, 1);
    // ESP side: a malformed line and a parse error.
    let now = r.now;
    r.s.on_rx(b"\x01\x02\r\n", now);
    r.s.on_rx(b"zz\r\n", now);
    r.run(1100);
    let p = r.port().with_code(EventCode::StmParseErrors);
    assert_eq!(p.len(), 2);
    assert_eq!(p[1].arg1, 2);
    assert_eq!(p[1].arg2, 0);
}

#[test]
fn a_valve_without_data_turns_stale() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    assert_eq!(r.port().last.valves[0].health & HEALTH_STALE, 0);
    r.stm_mut().set_answer("gvlvy", |_, _| String::new());
    r.run(70000);
    assert_ne!(r.port().last.valves[0].health & HEALTH_STALE, 0);
}

#[test]
fn protocol_2_targets_during_a_calibration_and_assembly_by_staop() {
    let mut r = Rig::new(2, 0x003);
    r.stm_mut().set_answer("gvlvx", |l, c| {
        let v = valve_of(l);
        let t = c.target[usize::from(v)];
        let status = if v == 0 { "129" } else { "1" };
        format!("gvlvx {v} {status} 40 {t} 21 3120 3350 -230 0 57 0 0 0 1 3000 1450 3 412 8123")
    });
    r.start();
    r.run(15000);
    assert!(r.port().last.valves[0].calibrating);
    let t = r.target(0, 20, TargetSource::Web);
    r.command(&t);
    r.run(3000);
    assert_eq!(r.stm().lines_of("stgtp"), ["stgtp 0 20"]);
    r.command(&cmd(StmCommandType::Assembly, 1));
    r.run(3000);
    assert_eq!(r.count("staop 1"), 1);
    assert_eq!(r.stm().lines_of("stgtp").len(), 1);
}

#[test]
fn stop_read_back_one_valve_or_every_active_valve_only_after_an_ok() {
    let mut r = Rig::new(3, 0x007);
    r.start();
    r.run(10000);
    r.reply_delay_ms = 20;
    r.command(&cmd(StmCommandType::StopValve, 1));
    r.run_until(|r| r.count("sstop") == 1, 100);
    let (v1, v0) = (r.count("gvlvy 1"), r.count("gvlvy 0"));
    r.run(150);
    assert_eq!(r.count("gvlvy 1"), v1 + 1);
    assert_eq!(r.count("gvlvy 0"), v0);
    r.command(&cmd(StmCommandType::StopValve, ALL_VALVES));
    r.run_until(|r| r.count("sstop") == 2, 100);
    let a: Vec<usize> = (0..4).map(|v| r.count(&format!("gvlvy {v}"))).collect();
    r.run(250);
    assert_eq!(r.count("gvlvy 0"), a[0] + 1);
    assert_eq!(r.count("gvlvy 1"), a[1] + 1);
    assert_eq!(r.count("gvlvy 2"), a[2] + 1);
    assert_eq!(r.count("gvlvy 3"), a[3]);
    r.stm_mut()
        .set_answer("sstop", |_, _| "sstop 1 err 2".to_string());
    r.command(&cmd(StmCommandType::StopValve, 1));
    r.run_until(|r| r.count("sstop") == 3, 100);
    let e1 = r.count("gvlvy 1");
    r.run(150);
    assert_eq!(r.count("gvlvy 1"), e1);
}

#[test]
fn masns_re_reads_the_valve_sensors_only_after_an_ok_v2_scans_without_masns() {
    let mut r = Rig::new(1, 0x001);
    r.start();
    r.run(15000);
    r.command(&cmd0(StmCommandType::ScanSensors));
    r.run_until(|r| r.count("masns") == 1, 8000);
    let n = r.count("gvlon 255");
    r.run(1000);
    assert_eq!(r.count("gvlon 255"), n + 1);
    r.stm_mut().set_answer("masns", |_, _| String::new());
    r.command(&cmd0(StmCommandType::ScanSensors));
    r.run_until(|r| r.count("masns") == 2, 8000);
    let m = r.count("gvlon 255");
    r.run(12000);
    assert_eq!(r.count("gvlon 255"), m);
    let mut v2 = Rig::new(2, 0x001);
    v2.start();
    v2.run(10000);
    v2.command(&cmd0(StmCommandType::ScanSensors));
    v2.run(8000);
    assert_eq!(v2.count("stons"), 1);
    assert_eq!(v2.count("masns"), 0);
}

#[test]
fn a_policy_reset_reports_the_timeouts_and_the_silent_time() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(15000);
    let last_reply = r.s.link().stats().last_reply_ms;
    r.stm_mut().silent = true;
    assert!(r.run_until(|r| !r.port().pulses.is_empty(), 180000));
    let ev = r.port().with_code(EventCode::StmResetByPolicy);
    assert_eq!(ev.len(), 1);
    assert!(ev[0].arg1 >= 5);
    assert_eq!(
        ev[0].arg2,
        ((r.port().pulses[0] - last_reply) / 1000) as i32
    );
    assert_eq!(r.s.link().stats().policy_resets, 1);
    assert_eq!(r.s.link().stats().user_resets, 0);
}

#[test]
fn the_silent_time_of_a_policy_reset_is_in_whole_seconds_rounded_down() {
    // Reply delays that put the silent time just below / just above a full second.
    for d in [183u32, 207] {
        let mut r = Rig::new(3, 0);
        r.reply_delay_ms = d;
        r.start();
        r.run(15000);
        r.stm_mut().silent = true;
        let last_reply = r.s.link().stats().last_reply_ms;
        assert!(r.run_until(|r| !r.port().pulses.is_empty(), 180000));
        let silent_ms = r.port().pulses[0] - last_reply;
        assert!(
            silent_ms % 1000 >= 945 || silent_ms % 1000 < 55,
            "delay {d}: {silent_ms}"
        );
        let ev = r.port().with_code(EventCode::StmResetByPolicy);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].arg2, (silent_ms / 1000) as i32, "delay {d}");
    }
}

/// One second driven by hand: every_second() and publish_if_due().
fn second(r: &mut Rig) {
    r.now += 1000;
    let reg = r.reg;
    r.s.every_second(r.now, &reg, StmSaveState::Idle);
    r.s.publish_if_due(r.now);
}

#[test]
fn lease_status_changes_publish_an_unchanged_status_does_not() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    // No STM traffic: after the start-up hold nothing changes but the lease view.
    for _ in 0..8 {
        second(&mut r);
    }
    assert_eq!(r.port().last.link, LinkState::Unknown);
    let mut p = r.port().publishes;
    second(&mut r);
    second(&mut r);
    assert_eq!(r.port().publishes, p);
    r.reg.mode = MqttMode::Mqtt; // broker not connected: the regulator is gone
    second(&mut r);
    p += 1;
    assert_eq!(r.port().publishes, p);
    second(&mut r);
    assert_eq!(r.port().publishes, p);
    // Config trust and timeout alone (apply_config publishes by itself first).
    let cfg = r.cfg.clone();
    r.s.apply_config(&cfg, false);
    r.s.publish_if_due(r.now + 500);
    let p = r.port().publishes;
    second(&mut r);
    assert!(!r.port().last.lease.config_trusted);
    assert_eq!(r.port().publishes, p + 1);
    r.cfg.failsafe.timeout_min = 90;
    let cfg = r.cfg.clone();
    r.s.apply_config(&cfg, false);
    r.s.publish_if_due(r.now + 500);
    let p = r.port().publishes;
    second(&mut r);
    assert_eq!(r.port().last.lease.timeout_min, 90);
    assert_eq!(r.port().publishes, p + 1);
    assert!(!r.port().has(EventCode::TargetsRestored));
}

/// A gvlvy answer whose valve 1 reports `counted`/`stop` as its last move.
fn last_move_answer(r: &mut Rig, counted: &Rc<Cell<u32>>, stop: &Rc<Cell<u32>>) {
    let (counted, stop) = (counted.clone(), stop.clone());
    r.stm_mut().set_answer("gvlvy", move |l, c| {
        let v = valve_of(l);
        let (n, s) = if v == 1 {
            (counted.get(), stop.get())
        } else {
            (1450, 3)
        };
        gvlvy(v, 1, c.target[usize::from(v)], n, s)
    });
}

#[test]
fn service_move_results_the_next_move_of_the_valve_5_min_at_most() {
    let mut r = Rig::new(3, 0x003);
    let counted = Rc::new(Cell::new(1450));
    let stop = Rc::new(Cell::new(3));
    last_move_answer(&mut r, &counted, &stop);
    r.start();
    r.run(10000);
    r.command(&service_move(1, 300));
    r.run(2000);
    assert!(!r.port().has(EventCode::ServiceMoveDone));
    counted.set(1234);
    stop.set(2);
    r.run(3000);
    let ev = r.port().with_code(EventCode::ServiceMoveDone);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].valve, 1);
    assert_eq!(ev[0].arg1, 1234);
    assert_eq!(ev[0].arg2, 2);
    counted.set(1235);
    r.run(3000);
    assert_eq!(r.port().with_code(EventCode::ServiceMoveDone).len(), 1);
    // No new move within 5 min: forgotten.
    r.command(&service_move(1, 310));
    r.run(Session::SERVICE_MOVE_WAIT_MS + 2000);
    counted.set(1300);
    r.run(3000);
    assert_eq!(r.port().with_code(EventCode::ServiceMoveDone).len(), 1);
}

#[test]
fn calib_started_of_a_scheduled_calibration_carries_1_a_manual_one_0() {
    let mut r = Rig::new(3, 0x003);
    let cal = Rc::new(Cell::new([false, false]));
    let c2 = cal.clone();
    r.stm_mut().set_answer("gvlvy", move |l, c| {
        let v = valve_of(l);
        let calibrating = c2.get().get(usize::from(v)).copied().unwrap_or(false);
        gvlvy(
            v,
            if calibrating { 129 } else { 1 },
            c.target[usize::from(v)],
            1450,
            3,
        )
    });
    r.start();
    r.run(10000);
    let mut c = cmd(StmCommandType::Calibrate, ALL_VALVES);
    c.scheduled = true;
    c.attempt = 1;
    r.command(&c);
    r.run(1000);
    cal.set([false, true]);
    r.run(3000);
    let ev = r.port().with_code(EventCode::CalibStarted);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].valve, 1);
    assert_eq!(ev[0].arg1, 1);
    cal.set([false, false]);
    r.run(3000);
    r.command(&cmd(StmCommandType::Calibrate, 0));
    r.run(1000);
    cal.set([true, false]);
    r.run(3000);
    let ev = r.port().with_code(EventCode::CalibStarted);
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[1].valve, 0);
    assert_eq!(ev[1].arg1, 0);
}

#[test]
fn the_scheduled_flag_ends_with_the_4_h_window() {
    let mut r = Rig::new(3, 0x001);
    let cal = Rc::new(Cell::new(false));
    let c2 = cal.clone();
    r.stm_mut().set_answer("gvlvy", move |l, c| {
        let v = valve_of(l);
        let status = if v == 0 && c2.get() { 129 } else { 1 };
        gvlvy(v, status, c.target[usize::from(v)], 1450, 3)
    });
    r.start();
    r.run(10000);
    let mut c = cmd(StmCommandType::Calibrate, ALL_VALVES);
    c.scheduled = true;
    r.command(&c);
    r.run(2000);
    r.now += Session::SCHEDULED_CALIB_WINDOW_MS;
    r.last_second = r.now;
    let reg = r.reg;
    r.s.every_second(r.now, &reg, StmSaveState::Idle);
    r.run(15000);
    cal.set(true);
    r.run(3000);
    let ev = r.port().with_code(EventCode::CalibStarted);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 0);
}

// ================================================================ status, snapshots

#[test]
fn a_repaired_stm_config_is_reported_once_per_stm_start() {
    let mut r = Rig::new(3, 0x001);
    let boot_ms = Rc::new(Cell::new(u64::from(r.now)));
    let b = boot_ms.clone();
    r.stm_mut().set_answer("gstax", move |_, c| {
        format!(
            "gstax {} 3 2 0 0 1 1 3540 1 60 0 0 0 0 0 0 0 0 1 0 0 0 0",
            (c.now - b.get()) / 1000
        )
    });
    r.start();
    r.run(30000);
    assert!(r.port().last.have_status);
    assert_eq!(r.port().with_code(EventCode::StmConfigRepaired).len(), 1);
    // The STM restarts (uptime back to 0): the repair is reported again.
    let now = r.now;
    boot_ms.set(u64::from(now));
    r.stm_mut().reset(u64::from(now));
    r.run(30000);
    assert_eq!(r.port().with_code(EventCode::StmRebootDetected).len(), 1);
    assert_eq!(r.port().with_code(EventCode::StmConfigRepaired).len(), 2);
}

#[test]
fn a_version_reply_alone_is_published() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.s.publish_if_due(r.now + 200);
    r.s.on_line(b"gvers 2.1.7-revamped_C2 1712345678 ", r.now + 200);
    r.s.publish_if_due(r.now + 400);
    assert_eq!(r.port().last.version.patch, 7);
}

#[test]
fn timeouts_on_a_down_link_are_published() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.stm_mut().silent = true;
    r.run(10000);
    assert_eq!(r.port().last.link, LinkState::Down);
    let failed = r.port().last.link_stats.failed_requests;
    r.run(5000);
    assert!(r.port().last.link_stats.failed_requests > failed);
}

#[test]
fn snapshots_at_most_every_100_ms_exactly_then_when_busy() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.port_mut().publish_times.clear();
    for i in 0..20u16 {
        r.command(&service_move(0, 100 + i));
    }
    r.run(3000);
    let times = &r.port().publish_times;
    assert!(times.len() >= 2);
    let min_gap = times.windows(2).map(|w| w[1] - w[0]).min();
    assert_eq!(min_gap, Some(100));
    assert!(times.len() <= 31);
}

/// gvlon answers of an STM whose valve 0 has sensors A and B (`valve0` also in the single
/// valve reply).
fn assigned_sensors(r: &mut Rig, valve0: bool) {
    r.stm_mut().set_answer("gvlon", move |l, _| {
        let z = "00-00-00-00-00-00-00-00";
        if l == "gvlon 255" {
            let mut ids = format!("{ID_A},{ID_B}");
            for _ in 2..24 {
                ids += ",";
                ids += z;
            }
            return format!("gvlon 12 {ids}");
        }
        let v = &l[6..];
        if valve0 && v == "0" {
            format!("gvlon {v} {ID_A} {ID_B} ")
        } else {
            format!("gvlon {v} {z} {z} ")
        }
    });
}

#[test]
fn protocol_2_valve_temperatures_come_from_the_assigned_sensors() {
    let mut t = SensorRig::new();
    let r = &mut t.r;
    r.stm_mut().protocol = 2;
    assigned_sensors(r, true);
    r.run(45000);
    assert_eq!(r.port().last.proto, 2);
    assert_eq!(r.port().last.valves[0].temp1, 215);
    assert_eq!(r.port().last.valves[0].temp2, 198);
    // A stray assignment changes nothing.
    let stray = format!("gvlon 0 {ID_B} {ID_A} ");
    r.s.on_line(stray.as_bytes(), r.now);
    r.run(1500);
    assert_eq!(r.port().last.valves[0].temp1, 215);
}

#[test]
fn protocol_1_valve_temperatures_come_from_gvlvd() {
    let mut t = SensorRig::new();
    t.r.stm_mut().protocol = 1;
    t.raw(ID_A, 230);
    assigned_sensors(&mut t.r, false);
    t.r.run(45000);
    assert_eq!(t.r.port().last.proto, 1);
    assert_eq!(t.r.port().last.valves[0].temp1, 215);
}

#[test]
fn profiles_are_stored_per_valve_stray_valve_states_are_ignored() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.s.on_line(b"gvlst 12 9,9,9,9,9,9,9,9,9,9,9,9, ", r.now);
    r.s.publish_if_due(r.now);
    assert!(!r.port().last.valves[3].known);
    r.run(10000);
    r.stm_mut()
        .set_answer("gprof", |_, _| "gprof 2 2 10:5 20:6".to_string());
    r.command(&cmd(StmCommandType::RequestProfile, 2));
    r.run(2000);
    let profiles = &r.port().profiles;
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].valve, 2);
    assert_eq!(profiles[0].count, 2);
    assert_eq!(profiles[0].samples[1].count, 20);
    for (v, &seq) in r.port().last.profile_seq.iter().enumerate() {
        assert_eq!(seq, u32::from(v == 2), "valve {v}");
    }
    // every reply counts, also one with the same profile
    r.command(&cmd(StmCommandType::RequestProfile, 2));
    r.run(2000);
    assert_eq!(r.port().profiles.len(), 2);
    assert_eq!(r.port().last.profile_seq[2], 2);
    // a stray reply (no request outstanding) of the last valve is stored and counted too
    r.s.on_line(b"gprof 11 1 10:5", r.now);
    r.run(200);
    let profiles = &r.port().profiles;
    assert_eq!(profiles.len(), 3);
    assert_eq!(profiles[2].valve, 11);
    assert_eq!(r.port().last.profile_seq[11], 1);
}

#[test]
fn an_incompatible_stm_is_logged_once_without_arguments() {
    let mut r = Rig::new(1, 0x001);
    r.stm_mut().too_old = true;
    r.start();
    r.run(10000);
    let ev = r.port().with_code(EventCode::StmIncompatible);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 0);
    assert_eq!(ev[0].arg2, 0);
    assert_eq!(text(&ev[0]), "1.3.5_C2");
}

#[test]
fn sensors_settle_exactly_30_s_after_the_start_and_it_is_published() {
    let mut r = Rig::new(3, 0);
    r.start();
    r.run(30000 - 2);
    assert!(!r.port().last.sensors_settled);
    r.run(100);
    assert!(r.port().last.sensors_settled);
}

#[test]
fn an_inactive_volt_slot_with_an_id_and_an_active_one_without_are_silent() {
    let mut r = Rig::new(3, 0x001);
    let bus = Rc::new(RefCell::new(Bus::default()));
    r.cfg.volts[0].active = false;
    r.cfg.volts[0].id = oid(ID_V);
    r.cfg.volts[1].active = true; // no id
    {
        let mut b = bus.borrow_mut();
        b.volts = vec![ID_V.to_string()];
        b.vad.insert(ID_V.to_string(), 1200);
    }
    Bus::attach(&bus, &mut r);
    r.start();
    r.run(45000);
    bus.borrow_mut().vad.insert(ID_V.to_string(), VAD_FAILED);
    r.run(40000);
    assert!(!r.port().has(EventCode::VoltSensorFailed));
}

#[test]
fn a_flash_that_gave_up_waiting_for_the_eeprom_publishes_the_refused_start() {
    let mut r = Rig::new(3, 0);
    r.start();
    r.run(10000);
    r.stm_mut().eep = 0;
    r.port_mut().image_ok = false;
    r.command(&flash_cmd("x.bin", false));
    r.run(1000);
    assert!(r.port().last.flash_pending);
    assert!(r.run_until(|r| r.port().has(EventCode::StmEepromWaitTimeout), 12000));
    assert!(r.port().has(EventCode::StmFlashFailed));
    assert!(!r.port().last.flash_pending);
}

#[test]
fn v1_no_target_push_while_calibrating_assembly_re_pushed_by_stgtp() {
    let mut r = Rig::new(1, 0x003);
    r.stm_mut().set_answer("gvlvd", |l, _| {
        let v = &l[6..];
        let status = if v == "0" { "129" } else { "1" };
        format!("gvlvd {v} 42 18 {status} 215 -500 57 3120 3350 230 0 ")
    });
    r.start();
    r.run(15000);
    assert!(r.port().last.valves[0].calibrating);
    let t = r.target(0, 20, TargetSource::Web);
    r.command(&t);
    r.run(3000);
    assert_eq!(r.count("stgtp 0"), 0);
    r.command(&cmd(StmCommandType::Assembly, 1));
    r.run(2000);
    assert_eq!(r.count("staop 1"), 1);
    r.stm_mut().silent = true; // a link interruption is a reboot on v1
    r.run(8000);
    let now = r.now;
    r.stm_mut().reset(u64::from(now));
    r.stm_mut().silent = false;
    r.run(20000);
    assert!(r.port().has(EventCode::StmRebootDetected));
    assert_eq!(r.count("staop 1"), 1);
    assert_eq!(r.stm().target[1], 100);
}

// ================================================================ EEPROM gate order

/// Index of the first line equal to `s`.
fn find(lines: &[String], s: &str) -> Option<usize> {
    lines.iter().position(|l| l == s)
}

#[test]
fn the_eeprom_poll_waits_for_queued_config_lines() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.reply_delay_ms = 300;
    r.command(&cmd0(StmCommandType::Detect));
    r.run(4);
    let t = r.target(0, 30, TargetSource::Web); // stgtp queued behind the outstanding stdet
    r.command(&t);
    r.run(4);
    r.command(&cmd0(StmCommandType::ResetStm));
    r.run(2000);
    let l = &r.stm().lines;
    let stgtp = find(l, "stgtp 0 30").expect("stgtp");
    let eep = find(l, "eepst").expect("eepst");
    assert!(stgtp < eep);
}

#[test]
fn the_eeprom_poll_waits_for_queued_user_and_config_lines() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.reply_delay_ms = 300;
    r.command(&cmd0(StmCommandType::Detect));
    r.run(4);
    r.command(&cmd(StmCommandType::Calibrate, 0));
    let t = r.target(0, 30, TargetSource::Web);
    r.command(&t);
    r.run(4);
    r.command(&cmd0(StmCommandType::ResetStm));
    r.run(3000);
    let l = &r.stm().lines;
    let stgtp = find(l, "stgtp 0 30").expect("stgtp");
    let eep = find(l, "eepst").expect("eepst");
    assert!(stgtp < eep);
}

#[test]
fn the_eeprom_poll_waits_for_the_retries_of_an_outstanding_config_line() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.stm_mut().set_answer("stgtp", |_, _| String::new());
    let t = r.target(0, 30, TargetSource::Web);
    r.command(&t);
    r.run_until(|r| r.count("stgtp") == 1, 100);
    r.command(&cmd0(StmCommandType::ResetStm));
    r.run(3000);
    let l = &r.stm().lines;
    let eep = find(l, "eepst").expect("eepst");
    assert_eq!(l[..eep].iter().filter(|s| *s == "stgtp 0 30").count(), 3);
}

#[test]
fn the_eeprom_poll_is_sent_every_500_ms_not_more_often() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.stm_mut().eep = 0;
    r.command(&cmd0(StmCommandType::ResetStm));
    r.run(2000);
    assert!(r.count("eepst") >= 3);
    assert!(r.count("eepst") <= 5);
}

// ================================================================ exact time limits

#[test]
fn a_service_move_is_forgotten_exactly_5_min_after_its_answer() {
    let mut r = Rig::new(3, 0x003);
    let counted = Rc::new(Cell::new(1450));
    let stop = Rc::new(Cell::new(3));
    last_move_answer(&mut r, &counted, &stop);
    r.start();
    r.run(10000);
    r.reply_delay_ms = 2;
    r.command(&service_move(1, 300));
    assert!(r.run_until(|r| r.count("svmov") == 1, 200));
    let answered = r.now + 2; // the reply is applied in the next step
    r.run(4);
    // Drive the rest by hand: no polls, exact seconds.
    let reg = r.reg;
    r.now = answered + Session::SERVICE_MOVE_WAIT_MS;
    r.s.every_second(r.now, &reg, StmSaveState::Idle);
    counted.set(1234);
    let y = gvlvy(1, 1, 50, 1234, 2);
    r.s.on_line(y.as_bytes(), r.now);
    r.now += 1000;
    r.s.every_second(r.now, &reg, StmSaveState::Idle);
    assert!(!r.port().has(EventCode::ServiceMoveDone));
}

#[test]
fn a_volt_reading_is_valid_for_exactly_60_s() {
    let mut t = SensorRig::new();
    let r = &mut t.r;
    r.run(45000);
    let seen = r.now;
    let v = format!("gowvd {ID_V} 1200 ");
    r.s.on_line(v.as_bytes(), seen);
    let slot1_failed = |r: &Rig| {
        r.port()
            .with_code(EventCode::VoltSensorFailed)
            .iter()
            .any(|e| e.arg1 == 1)
    };
    // Driven by hand: no further readings.
    let reg = r.reg;
    r.s.every_second(seen + Session::SENSOR_STALE_MS, &reg, StmSaveState::Idle);
    assert!(!slot1_failed(r));
    r.s.every_second(
        seen + Session::SENSOR_STALE_MS + 1000,
        &reg,
        StmSaveState::Idle,
    );
    assert!(slot1_failed(r));
}

#[test]
fn the_scheduled_flag_ends_exactly_4_h_after_the_command() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    let mut c = cmd(StmCommandType::Calibrate, ALL_VALVES);
    c.scheduled = true;
    let at = r.now;
    r.command(&c);
    r.run(2000);
    r.now = at + Session::SCHEDULED_CALIB_WINDOW_MS;
    let y = gvlvy(0, 129, 50, 1450, 3);
    r.s.on_line(y.as_bytes(), r.now);
    let reg = r.reg;
    r.s.every_second(r.now, &reg, StmSaveState::Idle);
    r.s.publish_if_due(r.now);
    let ev = r.port().with_code(EventCode::CalibStarted);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 0);
}

#[test]
fn moving_valves_are_polled_every_500_ms_idle_ones_every_2_s() {
    let mut r = Rig::new(3, 0x007);
    r.stm_mut().set_answer("gvlvy", |l, c| {
        let v = valve_of(l);
        let status = if v < 2 { 2 } else { 1 };
        gvlvy(v, status, c.target[usize::from(v)], 1450, 3)
    });
    r.start();
    r.run(15000);
    let (a, b, c) = (r.count("gvlvy 0"), r.count("gvlvy 1"), r.count("gvlvy 2"));
    r.run(10000);
    assert!(r.count("gvlvy 0") - a >= 15);
    assert!(r.count("gvlvy 1") - b >= 15);
    assert!(r.count("gvlvy 2") - c <= 6);
}

#[test]
fn an_assembly_hold_is_pushed_once_after_an_stm_reboot() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.command(&cmd(StmCommandType::Assembly, 0));
    r.run(2000);
    assert_eq!(r.count("staop 0"), 1);
    let now = r.now;
    r.stm_mut().reset(u64::from(now));
    r.run(20000);
    let n = r.count("staop 0");
    assert!(n >= 2);
    r.run(20000);
    assert_eq!(r.count("staop 0"), n); // acknowledged: not pushed again
}

#[test]
fn an_stm_reset_forgets_pending_service_moves() {
    let mut r = Rig::new(3, 0x003);
    let counted = Rc::new(Cell::new(1450));
    let stop = Rc::new(Cell::new(3));
    last_move_answer(&mut r, &counted, &stop);
    r.start();
    r.run(10000);
    r.command(&service_move(1, 300));
    r.run(1000);
    r.command(&cmd0(StmCommandType::ResetStm));
    r.run(1000);
    assert_eq!(r.port().pulses.len(), 1);
    r.run(8000);
    counted.set(1111);
    r.run(3000);
    assert!(!r.port().has(EventCode::ServiceMoveDone));
}

// ================================================================ Rust additions (mutation)

#[test]
fn a_uart_line_holds_1023_chars() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    let now = r.now;
    let mut line = vec![b'x'; STM_MAX_LINE_LEN];
    line.extend_from_slice(b"\r\n");
    r.s.on_rx(&line, now); // fits: parsed, an unknown command
    assert_eq!(r.s.link().stats().parse_errors, 1);
    let mut long = vec![b'x'; STM_MAX_LINE_LEN + 1];
    long.extend_from_slice(b"\r\n");
    r.s.on_rx(&long, now); // dropped
    assert_eq!(r.s.link().stats().parse_errors, 1);
    r.s.publish_if_due(now);
    assert_eq!(r.port().last.line_overflows, 1);
}

#[test]
fn motor_settings_the_breakaway_line_counts_only_when_it_is_sent() {
    let mut r = Rig::new(3, 0x001);
    r.start();
    r.run(10000);
    r.stm_mut().silent = true;
    let mut counts: u16 = 100;
    while r.s.link().queued_with(Priority::User) + r.s.link().queued_with(Priority::Config)
        < LinkPolicy::QUEUE_CAPACITY - 1
    {
        r.command(&service_move(0, counts));
        counts += 1;
    }
    let mut m = cmd0(StmCommandType::SetMotorSettings);
    m.has_motor = true; // one line, room for one
    r.command(&m);
    assert!(!r.port().has(EventCode::StmQueueFull));
    assert_eq!(
        r.s.link().queued_with(Priority::User) + r.s.link().queued_with(Priority::Config),
        LinkPolicy::QUEUE_CAPACITY
    );
}

#[test]
fn v1_valve_states_mark_unknown_valves_known() {
    let mut r = Rig::new(1, 0x003);
    r.stm_mut().set_answer("gvlvd", |_, _| String::new());
    r.stm_mut().set_answer("gvlst", |_, _| {
        "gvlst 12 8,6,6,8,6,6,6,6,6,6,6,6, ".to_string()
    });
    r.start();
    r.run(30000);
    assert!(r.count("gvlst") >= 1);
    let v = &r.port().last.valves;
    assert!(v[0].known);
    assert_eq!(v[0].status, 8);
    assert!(v[1].known);
    assert_eq!(v[1].status, 6);
}

#[test]
fn no_valve_is_reported_for_active_valves_only() {
    let mut r = Rig::new(3, 0x002);
    r.stm_mut().set_answer("gvlvy", |l, c| {
        let v = valve_of(l);
        let status = if v == 1 || v == 2 { 6 } else { 1 };
        gvlvy(v, status, c.target[usize::from(v)], 1450, 3)
    });
    r.start();
    r.run(15000);
    assert!(r.port().last.valves[2].known);
    let ev = r.port().with_code(EventCode::ValveNoValve);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].valve, 1);
}

#[test]
fn the_scheduled_flag_is_consumed_per_valve() {
    let mut r = Rig::new(3, 0x003);
    let cal = Rc::new(Cell::new([false, false]));
    let c2 = cal.clone();
    r.stm_mut().set_answer("gvlvy", move |l, c| {
        let v = valve_of(l);
        let calibrating = c2.get().get(usize::from(v)).copied().unwrap_or(false);
        let status = if calibrating { 129 } else { 1 };
        gvlvy(v, status, c.target[usize::from(v)], 1450, 3)
    });
    r.start();
    r.run(10000);
    let mut c = cmd(StmCommandType::Calibrate, ALL_VALVES);
    c.scheduled = true;
    c.attempt = 1;
    r.command(&c);
    r.run(1000);
    cal.set([false, true]);
    r.run(3000);
    cal.set([false, false]);
    r.run(3000);
    cal.set([false, true]); // valve 1 again: its flag is used up
    r.run(3000);
    cal.set([true, true]); // valve 0: its flag is still there
    r.run(3000);
    let ev = r.port().with_code(EventCode::CalibStarted);
    let got: Vec<(u8, i32)> = ev.iter().map(|e| (e.valve, e.arg1)).collect();
    assert_eq!(got, [(1, 1), (1, 0), (0, 1)]);
}
