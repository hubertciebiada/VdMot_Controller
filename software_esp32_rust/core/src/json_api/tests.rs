//! Port of test/native/test_json_api.cpp: golden documents for every builder (empty and fully
//! populated snapshots), null handling, overflow, and the API router (every route, methods,
//! malformed paths, fuzz).
//!
//! The C++ null pointers of the snapshots follow the port rules: a null that writes `null` is
//! `None` (esp version, error detail, image name, a view without state or config); a null that
//! writes "" (station, names, units, health version) has no Rust form, the empty slice gives
//! the same. A null array with a count (`writeValvesJson(jw, nullptr, 12, 0)`) has no Rust form
//! either: the slice is the array and its count. An unterminated C++ `char` array is a full
//! `Text<N>` (the same N characters). After the C++ cases: the route of the Rust firmware's
//! switch back.

use super::*;
use crate::common::{parse_one_wire_id, ITEM_NAME_MAX};
use crate::event_log::{make_event, EventCode, Severity};
use crate::failsafe::{LeaseState, RegulatorCause, FAILSAFE_HOLD};
use crate::stm_codec::{
    StopReason, STM_CFG_LAYOUT_CRC, STM_CFG_READ_FAILED, STM_FLAG_FS_BLOCKED, STM_FLAG_FS_LEASE,
    STM_FLAG_SVC_HOLD,
};
use crate::stm_flasher::{BoardCheck, FlashPhase};
use crate::test_support::{assert_text, Rng};
use crate::valve_model::{
    TargetSource, TargetSync, HEALTH_BLOCKED, HEALTH_FAILSAFE, HEALTH_STALE, HEALTH_STROKE_SHORT,
    HEALTH_TEMP_FAILED,
};
use crate::version::parse_version;
use std::string::{String, ToString};
use std::vec::Vec;
use std::{format, vec};

/// Builds a document into a 16 KB buffer; the builder must succeed and complete the document.
fn build(f: impl FnOnce(&mut JsonWriter<'_>) -> bool) -> String {
    let mut buf = vec![0u8; 16384];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(f(&mut jw));
    assert!(jw.complete());
    String::from_utf8(jw.as_bytes().to_vec()).unwrap()
}

/// Every builder must fail (not crash, not overrun) on every short buffer.
fn check_overflow(f: impl Fn(&mut JsonWriter<'_>) -> bool, full_len: usize) {
    let mut buf = vec![b'#'; full_len + 16];
    let mut cap = 0;
    while cap <= full_len {
        buf.fill(b'#');
        let mut jw = JsonWriter::new(&mut buf[..cap]);
        assert!(!f(&mut jw), "cap {cap}");
        assert!(buf[cap..].iter().all(|&c| c == b'#'), "cap {cap}");
        cap += if cap < 64 { 1 } else { 37 };
    }
    let mut jw = JsonWriter::new(&mut buf[..full_len + 1]);
    assert!(f(&mut jw));
}

fn q(s: &str) -> String {
    format!("\"{s}\"")
}

fn oid(s: &str) -> OneWireId {
    parse_one_wire_id(s.as_bytes()).unwrap()
}

fn text<const N: usize>(s: &[u8]) -> Text<N> {
    Text::from_slice(s).unwrap()
}

/// A text member filled to its capacity: the Rust form of a C++ unterminated `char` array.
fn full<const N: usize>(c: u8) -> Text<N> {
    let mut t = Text::new();
    t.resize(N, c).unwrap();
    t
}

fn local(year: u16, month: u8, mday: u8, hour: u8, minute: u8, second: u8) -> LocalTime {
    LocalTime {
        valid: true,
        year,
        month,
        mday,
        hour,
        minute,
        second,
        ..LocalTime::default()
    }
}

fn status(s: &StatusSnapshot<'_>) -> String {
    build(|jw| write_status_json(jw, s))
}

fn valves(views: &[ValveView<'_>], now_ms: u32) -> String {
    build(|jw| write_valves_json(jw, views, now_ms))
}

#[test]
fn api_state_names() {
    assert_eq!(net_state_name(NetState::Down), "down");
    assert_eq!(net_state_name(NetState::Ethernet), "ethernet");
    assert_eq!(net_state_name(NetState::Wifi), "wifi");
    // C++ netStateName(NetState(9)) == "down": no Rust form, a NetState cannot hold 9.
    assert_eq!(NetState::from_raw(9), None);
    assert_eq!(mqtt_state_name(MqttState::Disabled), "disabled");
    assert_eq!(mqtt_state_name(MqttState::Connecting), "connecting");
    assert_eq!(mqtt_state_name(MqttState::Connected), "connected");
    assert_eq!(mqtt_state_name(MqttState::Error), "error");
    // C++ mqttStateName(MqttState(9)) == "error": no Rust form.
    assert_eq!(MqttState::from_raw(9), None);
    // the C++ numbers
    for (v, s) in [NetState::Down, NetState::Ethernet, NetState::Wifi]
        .into_iter()
        .enumerate()
    {
        assert_eq!(s as u8, v as u8);
        assert_eq!(NetState::from_raw(v as u8), Some(s));
    }
    let states = [
        MqttState::Disabled,
        MqttState::Connecting,
        MqttState::Connected,
        MqttState::Error,
    ];
    for (v, s) in states.into_iter().enumerate() {
        assert_eq!(s as u8, v as u8);
        assert_eq!(MqttState::from_raw(v as u8), Some(s));
    }
    assert_eq!(MqttState::from_raw(4), None);
    assert_eq!(NetState::from_raw(3), None);
}

// ---------------------------------------------------------------- status

#[test]
fn api_status_of_an_empty_snapshot() {
    let s = StatusSnapshot::default();
    let expected = [
        r#"{"station":"","esp":{"version":"","build":null,"uptime":0,"resetReason":"unknown","#,
        r#""boots":0,"heap":{"free":0,"min":0,"largest":0},"flash":{"used":0,"size":0}},"#,
        r#""time":{"valid":false,"epoch":null,"local":null,"lastSync":null},"#,
        r#""net":{"state":"down","ip":"0.0.0.0","mask":"0.0.0.0","gw":"0.0.0.0","#,
        r#""dns":"0.0.0.0","mac":"","rssi":null,"hostname":"","trial":null},"#,
        r#""mqtt":{"state":"disabled","rc":0,"reconnects":0,"publishFailures":0,"#,
        r#""clientId":"","haStatus":"unknown"},"#,
        r#""stm":{"link":"#,
        &q(link_state_name(LinkState::Unknown)),
        r#","proto":null,"version":null,"build":null,"hwId":null,"chip":null,"#,
        r#""compatible":true,"minVersion":"1.4.0","stats":{"sent":0,"answered":0,"#,
        r#""timeouts":0,"failedRequests":0,"strayLines":0,"parseErrors":0,"queueFull":0,"#,
        r#""evictions":0,"policyResets":0,"userResets":0,"consecutiveTimeouts":0,"#,
        r#""lastReplyMs":0},"status":null,"espRx":{"overflow":0,"malformed":0},"#,
        r#""support":"unknown","lease":null,"learnTime":null},"#,
        r#""calibration":{"active":false,"lastScheduled":null,"nextSlot":null,"#,
        r#""next":null},"lastEventSeq":0,"#,
        r#""config":{"source":"stored","repairs":0,"newerSchema":false},"#,
        r#""importReport":false}"#,
    ]
    .concat();
    assert_eq!(status(&s), expected);
}

fn populated_status() -> StatusSnapshot<'static> {
    let mut s = StatusSnapshot {
        esp_version: Some(b"2.0.0-revamped"),
        build_epoch: 1_790_000_000,
        uptime_s: 3600,
        reset_reason: 6,
        boot_count: 17,
        free_heap: 150_000,
        min_free_heap: 90_000,
        largest_free_block: 110_000,
        sketch_size: 987_654,
        sketch_space: 1_310_720,
        time_valid: true,
        epoch: 1_790_000_123,
        local: local(2026, 9, 3, 4, 5, 6),
        last_sync_epoch: 1_790_000_000,
        net: NetState::Wifi,
        ip: 0x3201_A8C0,
        mask: 0x00FF_FFFF,
        gateway: 0x0101_A8C0,
        dns: 0x0808_0808,
        mac: text(b"AA:BB:CC:DD:EE:FF"),
        wifi_rssi: -67,
        hostname: text(b"VdMot \"OG\""),
        mqtt: MqttState::Connected,
        mqtt_rc: -2,
        mqtt_reconnects: 3,
        mqtt_publish_failures: 4,
        link: LinkState::Up,
        ..StatusSnapshot::default()
    };
    s.link_stats.sent = 1;
    s.link_stats.answered = 2;
    s.link_stats.timeouts = 3;
    s.link_stats.failed_requests = 4;
    s.link_stats.stray_lines = 5;
    s.link_stats.parse_errors = 6;
    s.link_stats.queue_full = 7;
    s.link_stats.evictions = 8;
    s.link_stats.policy_resets = 9;
    s.link_stats.user_resets = 10;
    s.link_stats.consecutive_timeouts = 11;
    s.link_stats.last_reply_ms = 12;
    s.stm_proto = 2;
    s.stm_version = parse_version(b"2.0.0-revamped_C2");
    assert!(s.stm_version.valid);
    s.stm_build = 20_260_901;
    s.stm_hw_id = 0x431;
    s.stm_compatible = false;
    s.have_stm_status = true;
    s.stm_status.uptime_s = 100;
    s.stm_status.resets = 2;
    s.stm_status.boot_reason = 3;
    s.stm_status.rx_overflow = 4;
    s.stm_status.parse_errors = 5;
    s.stm_status.eep_state = 1;
    s.esp_line_overflows = 13;
    s.esp_line_malformed = 14;
    s.calibration_active = true;
    s.last_scheduled_calib_epoch = 1_789_990_000;
    s.next_calib_slot = 20_260_927;
    s.last_event_seq = 999;
    s.station = "Dom Północ".as_bytes();
    s.net_trial_active = true;
    s.net_trial_remain_s = 87;
    s.mqtt_client_id = text(b"vdmot-a1b2c3");
    s.mqtt_ha_status = HaStatus::Offline;
    s.stm_support = StmSupport::TooOld;
    s.lease.mode = LeaseMode::Stm;
    s.lease.state = LeaseState::Expired;
    s.lease.remain_s = 0;
    s.lease.timeout_min = 60;
    s.lease.failsafe_mask = 0x0805;
    s.lease.regulator = RegulatorCause::HaOffline;
    s.lease.regulator_lost_s = 3700;
    s.lease.config_synced = true;
    s.lease.config_failed = false;
    s.lease.config_trusted = true;
    s.have_learn_time = true;
    s.learn_time_s = 7200;
    s.next_calib_epoch = 1_790_100_000;
    s.next_calib_local = local(2026, 9, 27, 3, 30, 0);
    s.config_source = 4;
    s.config_repairs = 0x8002;
    s.config_newer_schema = true;
    s.import_report = true;
    s
}

#[test]
fn api_status_of_a_populated_snapshot() {
    let s = populated_status();
    let j = status(&s);
    let expected = [
        r#"{"station":"Dom Północ","esp":{"version":"2.0.0-revamped","#,
        r#""build":1790000000,"uptime":3600,"#,
        r#""resetReason":"task_wdt","boots":17,"heap":{"free":150000,"min":90000,"#,
        r#""largest":110000},"flash":{"used":987654,"size":1310720}},"#,
        r#""time":{"valid":true,"epoch":1790000123,"local":"2026-09-03T04:05:06","#,
        r#""lastSync":1790000000},"#,
        r#""net":{"state":"wifi","ip":"192.168.1.50","mask":"255.255.255.0","#,
        r#""gw":"192.168.1.1","dns":"8.8.8.8","mac":"AA:BB:CC:DD:EE:FF","rssi":-67,"#,
        r#""hostname":"VdMot \"OG\"","trial":{"remainS":87}},"#,
        r#""mqtt":{"state":"connected","rc":-2,"reconnects":3,"publishFailures":4,"#,
        r#""clientId":"vdmot-a1b2c3","haStatus":"offline"},"#,
        r#""stm":{"link":"#,
        &q(link_state_name(LinkState::Up)),
        r#","proto":2,"version":"2.0.0-revamped_C2","build":20260901,"hwId":"0x431","#,
        r#""chip":"#,
        &q(stm_chip_name(0x431)),
        r#","compatible":false,"minVersion":"1.4.0","stats":{"sent":1,"answered":2,"#,
        r#""timeouts":3,"failedRequests":4,"strayLines":5,"parseErrors":6,"queueFull":7,"#,
        r#""evictions":8,"policyResets":9,"userResets":10,"consecutiveTimeouts":11,"#,
        r#""lastReplyMs":12},"status":{"uptime":100,"resets":2,"bootReason":3,"#,
        r#""rxOverflow":4,"parseErr":5,"eepState":1},"espRx":{"overflow":13,"#,
        r#""malformed":14},"support":"too_old","lease":{"mode":"stm","#,
        r#""state":"expired","remainS":0,"timeoutMin":60,"failsafeMask":2053,"#,
        r#""regulator":"ha_offline","regulatorLostS":3700,"configSynced":true,"#,
        r#""configFailed":false,"configTrusted":true},"learnTime":7200},"#,
        r#""calibration":{"active":true,"lastScheduled":1789990000,"nextSlot":20260927,"#,
        r#""next":"2026-09-27T03:30:00"},"#,
        r#""lastEventSeq":999,"config":{"source":"backup","#,
        r#""repairs":32770,"newerSchema":true},"importReport":true}"#,
    ]
    .concat();
    assert_eq!(j, expected);
    check_overflow(|jw| write_status_json(jw, &s), j.len());
}

#[test]
fn api_status_field_rules() {
    let mut s = StatusSnapshot::default();
    // Reset reason names, out-of-range -> unknown.
    let names = [
        "unknown",
        "poweron",
        "ext",
        "sw",
        "panic",
        "int_wdt",
        "task_wdt",
        "wdt",
        "deepsleep",
        "brownout",
        "sdio",
    ];
    for (r, name) in (0u8..).zip(names) {
        s.reset_reason = r;
        let j = status(&s);
        assert!(j.contains(&format!("\"resetReason\":\"{name}\"")), "{r}");
    }
    s.reset_reason = 11;
    let mut j = status(&s);
    assert!(j.contains("\"resetReason\":\"unknown\""));
    s.reset_reason = 255;
    j = status(&s);
    assert!(j.contains("\"resetReason\":\"unknown\""));

    // rssi only on WiFi.
    s.wifi_rssi = -50;
    s.net = NetState::Ethernet;
    j = status(&s);
    assert!(j.contains("\"state\":\"ethernet\""));
    assert!(j.contains("\"rssi\":null"));

    // Local time only with valid time on both flags.
    s.time_valid = true;
    s.epoch = 5;
    j = status(&s);
    assert!(j.contains("\"epoch\":5,\"local\":null"));
    s.time_valid = false;
    s.local.valid = true;
    j = status(&s);
    assert!(j.contains("\"epoch\":null,\"local\":null"));

    s.last_scheduled_calib_epoch = 1;
    j = status(&s);
    assert!(j.contains("\"lastScheduled\":1,"));
    // Negative lastScheduled is "none"; hw id is zero-padded to 3 digits.
    s.last_scheduled_calib_epoch = -5;
    s.stm_hw_id = 0x23;
    s.esp_version = None;
    j = status(&s);
    assert!(j.contains("\"lastScheduled\":null"));
    assert!(j.contains("\"hwId\":\"0x023\""));
    assert!(j.contains("\"version\":null,\"build\":null,\"uptime\""));

    // Unterminated fixed-size strings are cut at the array size (Rust: full texts).
    s.mac = full(b'M');
    s.hostname = full(b'H');
    j = status(&s);
    assert!(j.contains(&format!("\"mac\":\"{}\"", "M".repeat(17))));
    assert!(j.contains(&format!(
        "\"hostname\":\"{}\"",
        "H".repeat(STATION_NAME_MAX)
    )));
}

// ---------------------------------------------------------------- valves

#[test]
fn api_valves_document() {
    let mut st = ValveState {
        known: true,
        last_seen_ms: 1000,
        status: 2,
        calibrating: true,
        position: 55,
        mean_current: 12,
        desired_valid: true,
        desired: 60,
        source: TargetSource::Web,
        stm_target_known: true,
        stm_target: 50,
        sync: TargetSync::Pending,
        moves: 100,
        open_count: 20,
        close_count: 21,
        dead_zone: -3,
        calib_retries: 1,
        health: HEALTH_BLOCKED | HEALTH_STALE | HEALTH_TEMP_FAILED,
        has_extended: true,
        cal_state: 2,
        cal_flags: CAL_FLAG_LAST_FAILED,
        early_stops: 3,
        cmd_rejected: 4,
        move_seq: 9,
        ..ValveState::default()
    };
    st.last_move.dir = MoveDir::Close;
    st.last_move.requested_counts = 3000;
    st.last_move.counted_counts = 1500;
    st.last_move.stop = StopReason::EarlyEndStop;
    st.last_move.peak_current = 123;
    st.last_move.duration_ms = 4567;
    let cfg = ValveConfig {
        name: text(b"Bad"),
        active: true,
        ..ValveConfig::default()
    };

    // views[1]: no state and no config (C++ null pointers)
    let mut views = [ValveView::default(); 3];
    views[0].state = Some(&st);
    views[0].config = Some(&cfg);
    views[0].sensor_slot[0] = 3;
    views[0].sensor_name[0] = b"Flur";
    views[0].sensor_valid[0] = true;
    views[0].sensor_tenths[0] = 215;
    views[0].sensor_slot[1] = 7;
    // C++ sensorName[1] = nullptr: the empty name
    views[0].sensor_name[1] = b"";
    views[0].sensor_tenths[1] = 999; // not valid -> null
    let mut st2 = ValveState {
        known: true,
        last_seen_ms: 0xFFFF_F000, // wrap: 6500 - 0xFFFFF000 = 10596 ms
        status: 9,
        health: 0xFFFF, // bits above HEALTH_STROKE_SHORT have no name
        has_extended: true,
        ..ValveState::default()
    };
    st2.last_move.dir = MoveDir::Open;
    views[2].state = Some(&st2);

    let j = valves(&views, 6500);
    let expected = [
        r#"{"valves":[{"idx":1,"name":"Bad","active":true,"known":true,"state":2,"#,
        r#""stateKey":"#,
        &q(valve_status_key(2)),
        r#","calibrating":true,"pos":55,"target":60,"targetSource":"#,
        &q(target_source_name(TargetSource::Web)),
        r#","sync":"#,
        &q(target_sync_name(TargetSync::Pending)),
        r#","stmTarget":50,"meanCur":12,"moves":100,"oc":20,"cc":21,"dc":-3,"cr":1,"#,
        r#""health":["blocked","stale","tempFailed"],"age":5,"#,
        r#""sensors":[{"sensor":1,"slot":3,"name":"Flur","temp":21.5},"#,
        r#"{"sensor":2,"slot":7,"name":"","temp":null}],"#,
        r#""ext":{"calState":2,"calEarlyStop":false,"#,
        r#""calLastFailed":true,"earlyStops":3,"cmdRejected":4,"#,
        r#""lastMove":{"dir":"close","req":3000,"cnt":1500,"stop":"#,
        &q(stop_reason_name(StopReason::EarlyEndStop)),
        r#","peak":12.3,"ms":4567},"moveSeq":9,"v3":null},"#,
        r#""failsafe":{"state":"off","pct":null},"calibrationEnd":null},"#,
        r#"{"idx":2,"name":"","active":false,"known":false,"state":0,"stateKey":"#,
        &q(valve_status_key(0)),
        r#","calibrating":false,"pos":0,"target":null,"targetSource":"#,
        &q(target_source_name(TargetSource::None)),
        r#","sync":"#,
        &q(target_sync_name(TargetSync::Unknown)),
        r#","stmTarget":null,"meanCur":0,"moves":0,"oc":0,"cc":0,"dc":0,"cr":0,"#,
        r#""health":[],"age":null,"sensors":[],"ext":null,"#,
        r#""failsafe":{"state":"off","pct":null},"calibrationEnd":null},"#,
        r#"{"idx":3,"name":"","active":false,"known":true,"state":9,"stateKey":"#,
        &q(valve_status_key(9)),
        r#","calibrating":false,"pos":0,"target":null,"targetSource":"#,
        &q(target_source_name(TargetSource::None)),
        r#","sync":"#,
        &q(target_sync_name(TargetSync::Unknown)),
        r#","stmTarget":null,"meanCur":0,"moves":0,"oc":0,"cc":0,"dc":0,"cr":0,"#,
        r#""health":["blocked","failed","noValve","calibRetries","earlyStop","#,
        r#""cmdRejected","stale","targetUnconfirmed","tempFailed","failsafe","#,
        r#""strokeShort"],"age":10,"sensors":[],"#,
        r#""ext":{"calState":0,"calEarlyStop":false,"calLastFailed":false,"#,
        r#""earlyStops":0,"cmdRejected":0,"lastMove":{"dir":"open","#,
        r#""req":0,"cnt":0,"stop":"#,
        &q(stop_reason_name(StopReason::None)),
        r#","peak":0.0,"ms":0},"moveSeq":0,"v3":null},"#,
        r#""failsafe":{"state":"off","pct":null},"calibrationEnd":null}]}"#,
    ]
    .concat();
    assert_eq!(j, expected);
    check_overflow(|jw| write_valves_json(jw, &views, 6500), j.len());
    // No views. C++ writeValvesJson(jw, nullptr, 12, 0): no Rust form.
    assert_eq!(valves(&views[..0], 0), "{\"valves\":[]}");
    assert_eq!(valves(&[], 0), "{\"valves\":[]}");

    // Names: at most 10 chars, also when the array is not terminated (Rust: a full text).
    let mut full_cfg = ValveConfig {
        name: text(b"0123456789"),
        ..ValveConfig::default()
    };
    let fv = |c: &ValveConfig| {
        let v = ValveView {
            config: Some(c),
            ..ValveView::default()
        };
        valves(&[v], 0)
    };
    assert!(fv(&full_cfg).contains("\"name\":\"0123456789\","));
    full_cfg.name = full(b'n');
    assert!(fv(&full_cfg).contains("\"name\":\"nnnnnnnnnn\","));
    // Unknown health bits above the table are ignored.
    st2.health = 0xF800;
    let one = |st: &ValveState| {
        let v = ValveView {
            state: Some(st),
            ..ValveView::default()
        };
        valves(&[v], 0)
    };
    assert!(one(&st2).contains("\"health\":[]"));
    st2.health = HEALTH_FAILSAFE;
    assert!(one(&st2).contains("\"health\":[\"failsafe\"]"));
    st2.health = HEALTH_STROKE_SHORT;
    assert!(one(&st2).contains("\"health\":[\"strokeShort\"]"));
}

#[test]
fn api_twelve_full_valves_fit_a_response_slot() {
    let mut st = [ValveState::default(); VALVE_COUNT as usize];
    let mut cfg: [ValveConfig; VALVE_COUNT as usize] = Default::default();
    for (s, c) in st.iter_mut().zip(cfg.iter_mut()) {
        s.known = true;
        s.status = 1;
        s.desired_valid = true;
        s.stm_target_known = true;
        s.moves = 4_000_000_000;
        s.open_count = 4_000_000_000;
        s.close_count = 4_000_000_000;
        s.dead_zone = i32::MIN;
        s.health = 0x1FF;
        s.has_extended = true;
        s.early_stops = 4_000_000_000;
        s.cmd_rejected = 4_000_000_000;
        s.last_move.requested_counts = 4_000_000_000;
        s.last_move.counted_counts = 4_000_000_000;
        s.last_move.duration_ms = 4_000_000_000;
        s.last_move.peak_current = 65535;
        s.move_seq = 4_000_000_000;
        c.name = full(b'"'); // worst-case escaping
        assert_eq!(c.name.len(), ITEM_NAME_MAX);
    }
    let views: Vec<ValveView<'_>> = st
        .iter()
        .zip(cfg.iter())
        .map(|(s, c)| ValveView {
            state: Some(s),
            config: Some(c),
            sensor_slot: [34, 33],
            sensor_name: [b"\"\"\"\"\"\"\"\"\"\"", b"\"\"\"\"\"\"\"\"\"\""],
            sensor_valid: [true, true],
            sensor_tenths: [-1270, -1270],
            ..ValveView::default()
        })
        .collect();
    let mut slot = vec![0u8; 12 * 1024];
    let mut jw = JsonWriter::new(&mut slot);
    assert!(write_valves_json(&mut jw, &views, 0));
    assert!(jw.complete());
    std::println!("worst-case /api/valves: {} bytes", jw.length());
}

// ---------------------------------------------------------------- profile, sensors, events

#[test]
fn api_profile_document() {
    let mut p = Profile {
        valve: 2,
        count: 2,
        ..Profile::default()
    };
    p.samples[0].count = 100;
    p.samples[0].current = 50;
    p.samples[1].count = 4_000_000_000;
    p.samples[1].current = 65535;
    let j = build(|jw| write_profile_json(jw, &p));
    assert_eq!(
        j,
        "{\"valve\":3,\"count\":2,\"samples\":[[100,50],[4000000000,65535]]}"
    );
    check_overflow(|jw| write_profile_json(jw, &p), j.len());

    let empty = Profile::default();
    assert_eq!(
        build(|jw| write_profile_json(jw, &empty)),
        "{\"valve\":1,\"count\":0,\"samples\":[]}"
    );
    let mut over = Profile {
        count: 200, // clamped to the 32 stored samples
        ..Profile::default()
    };
    for (i, s) in (0u32..).zip(over.samples.iter_mut()) {
        s.count = i;
    }
    let o = build(|jw| write_profile_json(jw, &over));
    assert!(o.contains("\"count\":32,"));
    assert!(o.contains("[31,0]]}"));
    over.count = 32;
    assert_eq!(build(|jw| write_profile_json(jw, &over)), o);
    over.count = 31;
    assert!(build(|jw| write_profile_json(jw, &over)).contains("[30,0]]}"));
}

#[test]
fn api_sensors_document() {
    let mut t = [SensorView::default(); 2];
    t[0].slot = 1;
    t[0].name = b"Flur";
    t[0].active = true;
    t[0].on_bus = true;
    t[0].id = oid("28-84-37-94-97-ff-03-23");
    t[0].valid = true;
    t[0].raw = 210;
    t[0].value = 215;
    t[0].age_s = 3;
    t[0].valve = 0;
    // C++ t[1].name = nullptr: the empty name
    t[1].name = b"";
    t[1].raw = -1270;
    t[1].value = 77;
    t[1].valve = 11;
    let mut v = [SensorView::default(); 2];
    v[0].slot = 2;
    v[0].name = b"Akku";
    v[0].active = true;
    v[0].id = oid("26-11-22-33-44-55-66-29");
    v[0].valid = true;
    v[0].raw = 1234;
    v[0].value = -12345;
    v[0].unit = b"V";
    v[0].age_s = 9;
    // C++ v[1].unit = nullptr: the empty unit
    v[1].unit = b"";
    v[1].valve = 3; // ignored for volts
    let j = build(|jw| write_sensors_json(jw, &t, &v));
    let expected = [
        r#"{"temps":[{"slot":1,"name":"Flur","id":"28-84-37-94-97-ff-03-23","#,
        r#""active":true,"onBus":true,"temp":21.5,"raw":210,"age":3,"valve":1},"#,
        r#"{"slot":null,"name":"","id":"","active":false,"onBus":false,"temp":null,"#,
        r#""raw":-1270,"age":0,"valve":12}],"#,
        r#""volts":[{"slot":2,"name":"Akku","id":"26-11-22-33-44-55-66-29","#,
        r#""active":true,"onBus":false,"value":-12.345,"unit":"V","raw":1234,"age":9},"#,
        r#"{"slot":null,"name":"","id":"","active":false,"onBus":false,"value":null,"#,
        r#""unit":"","raw":0,"age":0}]}"#,
    ]
    .concat();
    assert_eq!(j, expected);
    check_overflow(|jw| write_sensors_json(jw, &t, &v), j.len());

    t[1].valve = 12; // out of range -> null
    assert!(build(|jw| write_sensors_json(jw, &t[1..2], &[])).contains("\"valve\":null}"));
    t[1].valve = NO_VALVE;
    assert!(build(|jw| write_sensors_json(jw, &t[1..2], &[])).contains("\"valve\":null}"));
    // C++ writeSensorsJson(jw, nullptr, 5, nullptr, 5): no Rust form, the empty slices.
    assert_eq!(
        build(|jw| write_sensors_json(jw, &[], &[])),
        "{\"temps\":[],\"volts\":[]}"
    );
}

#[test]
fn api_events_document() {
    let mut e = [
        make_event(EventCode::EarlyStop, Severity::Warning, 2, 2, 3, b"x"),
        make_event(EventCode::LinkDown, Severity::Error, NO_VALVE, 5, 0, b""),
    ];
    e[0].seq = 41;
    e[0].epoch = 1_790_000_000;
    e[0].uptime_s = 77;
    e[1].seq = 42;
    let event = |e: &Event| {
        let mut b = [0u8; 512];
        let mut jw = JsonWriter::new(&mut b);
        assert!(write_event_json(&mut jw, e));
        String::from_utf8(jw.as_bytes().to_vec()).unwrap()
    };
    let (ev0, ev1) = (event(&e[0]), event(&e[1]));
    let j = build(|jw| write_events_json(jw, &e, 10, 42, 42, 9));
    assert_eq!(
        j,
        format!("{{\"first\":10,\"last\":42,\"next\":42,\"dropped\":9,\"events\":[{ev0},{ev1}]}}")
    );
    check_overflow(|jw| write_events_json(jw, &e, 10, 42, 42, 9), j.len());
    // C++ writeEventsJson(jw, nullptr, 3, ...): no Rust form, the empty slice.
    assert_eq!(
        build(|jw| write_events_json(jw, &[], 0, 0, 0, 0)),
        "{\"first\":0,\"last\":0,\"next\":0,\"dropped\":0,\"events\":[]}"
    );
    assert_eq!(
        build(|jw| write_events_json(jw, &e[..0], 4_294_967_295, 1, 2, 3)),
        "{\"first\":4294967295,\"last\":1,\"next\":2,\"dropped\":3,\"events\":[]}"
    );
}

// ---------------------------------------------------------------- flash, motor, error

#[test]
fn api_flash_status_document() {
    let idle = FlashStatus::default();
    let j = build(|jw| write_flash_status_json(jw, &idle, None, false));
    let expected = [
        r#"{"phase":"#,
        &q(flash_phase_name(FlashPhase::Idle)),
        r#","status":"#,
        &legacy_flash_status(FlashPhase::Idle).to_string(),
        r#","percent":0,"bytesDone":0,"bytesTotal":0,"chipId":null,"chipName":null,"#,
        r#""bootloaderVersion":null,"attempt":0,"error":null,"startedMs":0,"finishedMs":0,"#,
        r#""image":null,"appVersion":null,"board":"ok","boardHw":null,"#,
        r#""manualReset":false,"baud":0,"pending":false}"#,
    ]
    .concat();
    assert_eq!(j, expected);

    let mut s = FlashStatus {
        phase: FlashPhase::Failed,
        error: FlashError::Nack,
        error_phase: FlashPhase::Writing,
        error_address: 0x0800_0100,
        percent: 42,
        bytes_done: 2048,
        bytes_total: 65536,
        chip_pid: 0x431,
        bootloader_version: 0x31,
        attempt: 2,
        started_ms: 1000,
        finished_ms: 4_294_967_295,
        app_version: parse_version(b"1.4.9_Dev_C2"),
        board: BoardCheck::Mismatch,
        board_hw: text(b"C1"),
        manual_reset: true,
        baud: 57600,
        ..FlashStatus::default()
    };
    assert!(s.app_version.valid);
    s.image.size = 65533;
    s.image.crc = 0xDEAD_BEEF;
    s.image.version = text(b"2.0.0-revamped_C2");
    s.image.hw_tag = text(b"C2");
    let name: &[u8] = b"fw \"1\".bin";
    let f = build(|jw| write_flash_status_json(jw, &s, Some(name), true));
    let fe = [
        r#"{"phase":"#,
        &q(flash_phase_name(FlashPhase::Failed)),
        r#","status":"#,
        &legacy_flash_status(FlashPhase::Failed).to_string(),
        r#","percent":42,"bytesDone":2048,"bytesTotal":65536,"chipId":"0x431","#,
        r#""chipName":"#,
        &q(stm_chip_name(0x431)),
        r#","bootloaderVersion":"3.1","attempt":2,"error":{"code":"#,
        &q(flash_error_name(FlashError::Nack)),
        r#","phase":"#,
        &q(flash_phase_name(FlashPhase::Writing)),
        r#","addr":"0x08000100"},"startedMs":1000,"finishedMs":4294967295,"#,
        r#""image":{"name":"fw \"1\".bin","size":65533,"crc32":"0xdeadbeef","#,
        r#""version":"2.0.0-revamped_C2","hw":"C2"},"appVersion":"1.4.9_Dev_C2","#,
        r#""board":"mismatch","boardHw":"C1","manualReset":true,"baud":57600,"#,
        r#""pending":true}"#,
    ]
    .concat();
    assert_eq!(f, fe);
    check_overflow(
        |jw| write_flash_status_json(jw, &s, Some(name), true),
        f.len(),
    );
    // unterminated tags are cut at their array size (Rust: full texts)
    s.image.hw_tag = full(b'H');
    s.board_hw = full(b'B');
    let cut = build(|jw| write_flash_status_json(jw, &s, Some(b"x"), false));
    assert!(cut.contains("\"hw\":\"HHH\"}"));
    assert!(cut.contains("\"boardHw\":\"BBB\","));
    assert!(cut.contains("\"pending\":false}"));

    // Image without a name, name without an image, unterminated version.
    let mut a = FlashStatus::default();
    a.image.size = 1;
    a.image.crc = 0x1;
    let mut k = build(|jw| write_flash_status_json(jw, &a, None, false));
    assert!(k.contains(concat!(
        r#""image":{"name":null,"size":1,"crc32":"0x00000001","version":null,"#,
        r#""hw":null}"#
    )));
    let mut b = FlashStatus {
        bootloader_version: 0x10,
        ..FlashStatus::default()
    };
    k = build(|jw| write_flash_status_json(jw, &b, Some(b"a.bin"), false));
    assert!(k.contains(concat!(
        r#""image":{"name":"a.bin","size":0,"crc32":"0x00000000","#,
        r#""version":null,"hw":null}"#
    )));
    assert!(k.contains("\"bootloaderVersion\":\"1.0\""));
    b.image.version = full(b'v');
    k = build(|jw| write_flash_status_json(jw, &b, Some(b"a.bin"), false));
    assert!(k.contains(&format!("\"version\":\"{}\"", "v".repeat(31))));
}

#[test]
fn api_motor_document() {
    let mut m = MotorChars::default();
    let j = build(|jw| write_motor_json(jw, &m, 0, None, false));
    assert_eq!(
        j,
        concat!(
            r#"{"motor":{"lowC":17,"highC":17,"startOnPower":30,"noOfMinCount":3000,"#,
            r#""maxCalReps":2},"learnMovements":0,"breakaway":null,"known":false}"#
        )
    );
    m.low_factor = 10;
    m.high_factor = 40;
    m.start_on_power = 100;
    m.min_counts = 60000;
    m.max_calib_retries = 0;
    let b = Breakaway {
        enable: true,
        step_pct: 25,
        max_ma: 45,
    };
    let k = build(|jw| write_motor_json(jw, &m, 65534, Some(&b), true));
    assert_eq!(
        k,
        concat!(
            r#"{"motor":{"lowC":10,"highC":40,"startOnPower":100,"noOfMinCount":60000,"#,
            r#""maxCalReps":0},"learnMovements":65534,"breakaway":{"enable":true,"#,
            r#""stepPct":25,"maxmA":45},"known":true}"#
        )
    );
    check_overflow(
        |jw| write_motor_json(jw, &m, 65534, Some(&b), true),
        k.len(),
    );
}

#[test]
fn api_error_document() {
    assert_eq!(
        build(|jw| write_error_json(jw, b"invalid", Some(b"mqtt.port"))),
        "{\"error\":\"invalid\",\"detail\":\"mqtt.port\"}"
    );
    assert_eq!(
        build(|jw| write_error_json(jw, b"x", None)),
        "{\"error\":\"x\",\"detail\":null}"
    );
    check_overflow(
        |jw| write_error_json(jw, b"a\"b", Some(b"c")),
        "{\"error\":\"a\\\"b\",\"detail\":\"c\"}".len(),
    );
    // Not a root document -> not complete -> false.
    let mut buf = [0u8; 64];
    let mut jw = JsonWriter::new(&mut buf);
    jw.begin_array();
    assert!(!write_error_json(&mut jw, b"a", Some(b"b")));
}

// ---------------------------------------------------------------- routing

fn route(m: HttpMethod, path: &str) -> RouteMatch {
    match_api_route(m, path.as_bytes())
}

const G: HttpMethod = HttpMethod::Get;
const P: HttpMethod = HttpMethod::Post;
const D: HttpMethod = HttpMethod::Delete;

const CASES: [(HttpMethod, &str, ApiRoute); 43] = [
    (G, "/api/status", ApiRoute::Status),
    (G, "/api/valves", ApiRoute::Valves),
    (P, "/api/valves/1/target", ApiRoute::ValveTarget),
    (P, "/api/valves/1/calibrate", ApiRoute::ValveCalibrate),
    (P, "/api/valves/1/assembly", ApiRoute::ValveAssembly),
    (P, "/api/valves/1/service-move", ApiRoute::ValveServiceMove),
    (P, "/api/valves/1/sensors", ApiRoute::ValveSensors),
    (G, "/api/valves/1/profile", ApiRoute::ValveProfile),
    (P, "/api/valves/1/profile", ApiRoute::ValveProfileRefresh),
    (P, "/api/valves/calibrate", ApiRoute::CalibrateAll),
    (P, "/api/valves/assembly", ApiRoute::AssemblyAll),
    (P, "/api/valves/detect", ApiRoute::Detect),
    (G, "/api/sensors", ApiRoute::Sensors),
    (P, "/api/sensors/scan", ApiRoute::SensorsScan),
    (G, "/api/events", ApiRoute::Events),
    (G, "/api/config", ApiRoute::ConfigGet),
    (P, "/api/config", ApiRoute::ConfigPatch),
    (G, "/api/config/export", ApiRoute::ConfigExport),
    (G, "/api/stm/motor", ApiRoute::Motor),
    (P, "/api/stm/motor", ApiRoute::MotorSet),
    (P, "/api/stm/reset", ApiRoute::StmReset),
    (G, "/api/stm/images", ApiRoute::StmImages),
    (P, "/api/stm/images", ApiRoute::StmImageUpload),
    (D, "/api/stm/images/fw.bin", ApiRoute::StmImageDelete),
    (P, "/api/stm/flash", ApiRoute::StmFlash),
    (G, "/api/stm/flash", ApiRoute::StmFlashStatus),
    (P, "/api/stm/flash/abort", ApiRoute::StmFlashAbort),
    (P, "/api/ota/esp", ApiRoute::EspOta),
    (P, "/api/system/reboot", ApiRoute::Reboot),
    (P, "/api/system/factory-reset", ApiRoute::FactoryReset),
    (P, "/api/mqtt/reconnect", ApiRoute::MqttReconnect),
    (P, "/api/mqtt/discovery", ApiRoute::MqttDiscovery),
    (G, "/api/log", ApiRoute::LogDownload),
    (G, "/api/health", ApiRoute::Health),
    (P, "/api/valves/12/stop", ApiRoute::ValveStop),
    (P, "/api/valves/stop", ApiRoute::StopAll),
    (P, "/api/stm/safe-mode/leave", ApiRoute::StmSafeModeLeave),
    (P, "/api/system/network/confirm", ApiRoute::NetConfirm),
    (P, "/api/system/network/revert", ApiRoute::NetRevert),
    (G, "/api/files", ApiRoute::Files),
    (D, "/api/files", ApiRoute::FileDelete),
    (G, "/api/import-report", ApiRoute::ImportReport),
    (D, "/api/import-report", ApiRoute::ImportReportDismiss),
];

#[test]
fn api_every_route_and_method() {
    for &(m, path, r) in &CASES {
        assert_eq!(route(m, path).route, r, "{path}");
        // Every other method on a known path -> 405 (unless another entry uses it).
        for other in [G, P, D, HttpMethod::Other] {
            if other == m {
                continue;
            }
            let also_routed = CASES.iter().any(|c| c.0 == other && c.1 == path);
            if also_routed {
                continue;
            }
            assert_eq!(
                route(other, path).route,
                ApiRoute::MethodNotAllowed,
                "{path}"
            );
        }
    }
    assert_eq!(route(P, "/api/valves/3/stop").valve, 2);
    assert_eq!(route(P, "/api/valves/13/stop").route, ApiRoute::NotFound);
    assert_eq!(route(P, "/api/stm/safe-mode").route, ApiRoute::NotFound);
    assert_eq!(route(P, "/api/system/network").route, ApiRoute::NotFound);
}

#[test]
fn api_route_parameters() {
    for n in 1u8..=12 {
        let m = route(P, &format!("/api/valves/{n}/target"));
        assert_eq!(m.route, ApiRoute::ValveTarget);
        assert_eq!(m.valve, n - 1);
    }
    assert_eq!(route(G, "/api/status").valve, NO_VALVE);
    let bad_valves = ["0", "13", "01", "1a", "", "-1", "+1", "100", "99999999999"];
    for v in bad_valves {
        assert_eq!(
            route(P, &format!("/api/valves/{v}/target")).route,
            ApiRoute::NotFound,
            "{v}"
        );
    }
    let mut m = route(D, "/api/stm/images/STM32F411_C2-rev.1_x.bin");
    assert_eq!(m.route, ApiRoute::StmImageDelete);
    assert_text(&m.name, "STM32F411_C2-rev.1_x.bin");
    let n31 = "n".repeat(31);
    m = route(D, &format!("/api/stm/images/{n31}"));
    assert_eq!(m.route, ApiRoute::StmImageDelete);
    assert_text(&m.name, &n31);
    // C++ m.name[31] == '\0': the Rust name holds exactly the 31 chars.
    assert_eq!(m.name.len(), 31);
    m = route(D, "/api/stm/images/azAZ09._-");
    assert_eq!(m.route, ApiRoute::StmImageDelete);
    assert_text(&m.name, "azAZ09._-");
    let bad_names: [&[u8]; 14] = [
        b".hidden",
        b"a b",
        b"a/b",
        b"a%20",
        b"",
        b"\xc3\xa4",
        b"%ab",
        b"a`",
        b"a{",
        b"a@",
        b"a[",
        b"a:",
        b"a\x7f",
        b"_:",
    ];
    for b in bad_names {
        let mut p = b"/api/stm/images/".to_vec();
        p.extend_from_slice(b);
        assert_eq!(
            match_api_route(D, &p).route,
            ApiRoute::NotFound,
            "{}",
            b.escape_ascii()
        );
    }
    assert_eq!(
        route(D, &format!("/api/stm/images/{n31}n")).route,
        ApiRoute::NotFound
    );
    assert_eq!(
        route(G, "/api/stm/images/a.bin").route,
        ApiRoute::MethodNotAllowed
    );
    assert!(route(G, "/api/stm/images/a.bin").name.is_empty());
    assert_eq!(
        route(D, "/api/stm/images").route,
        ApiRoute::MethodNotAllowed
    );
}

#[test]
fn api_malformed_and_unknown_paths() {
    let not_found = [
        "",
        "/",
        "/api",
        "/api/",
        "api/status",
        "/API/status",
        "/api/status/",
        "/api//status",
        "/api/status?x=1",
        "/api/statuss",
        "/api/statu",
        "/api/valves/",
        "/api/valves/1",
        "/api/valves/1/",
        "/api/valves/1/target/x",
        "/api/valves//target",
        "/api/valves/target",
        "/api/stm",
        "/api/stm/images/a.bin/x",
        "/api/stm/flash/abort/",
        "/apix/status",
        "/api/unknown",
        "//api/status",
        "/api/config/export/x",
    ];
    for p in not_found {
        for m in [G, P, D] {
            let r = route(m, p);
            assert_eq!(r.route, ApiRoute::NotFound, "{p}");
            assert_eq!(r.valve, NO_VALVE, "{p}");
        }
    }
    // C++ matchApiRoute(Get, nullptr, 11): no Rust form.
    assert_eq!(
        match_api_route(G, b"/api/status\0").route,
        ApiRoute::NotFound
    );
    assert_eq!(
        match_api_route(G, b"/api/sta\0us").route,
        ApiRoute::NotFound
    );
    // Only the bytes of the slice are considered (C++ `len`).
    assert_eq!(
        match_api_route(G, &b"/api/statusXYZ"[..11]).route,
        ApiRoute::Status
    );
    assert_eq!(
        match_api_route(G, &b"/api/status"[..10]).route,
        ApiRoute::NotFound
    );
}

#[test]
fn api_router_fuzz() {
    let mut rng = Rng::new(31337);
    let pieces: [&[u8]; 22] = [
        b"/", b"api", b"valves", b"1", b"12", b"13", b"0", b"stm", b"images", b"flash", b"abort",
        b"a.bin", b".x", b"target", b"profile", b"config", b"export", b"", b"%", b"\x01",
        b"status", b"log",
    ];
    for _ in 0..20000 {
        let mut p: Vec<u8> = if rng.below(4) != 0 {
            b"/api/".to_vec()
        } else {
            Vec::new()
        };
        let n = rng.below(6);
        for _ in 0..n {
            p.extend_from_slice(pieces[rng.below(pieces.len() as u32) as usize]);
        }
        let m = HttpMethod::from_raw(rng.below(4) as u8).unwrap();
        let r = match_api_route(m, &p);
        assert!(r.name.len() < 32);
        assert!(r.valve == NO_VALVE || r.valve < VALVE_COUNT);
    }
}

#[test]
fn status_json_all_ones_addresses_are_written_in_full() {
    let s = StatusSnapshot {
        ip: 0xFFFF_FFFF,
        mask: 0xFFFF_FFFF,
        gateway: 0xFEFF_FFFF,
        dns: 0xFFFF_FFFE,
        ..StatusSnapshot::default()
    };
    let mut buf = [0u8; 4096];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write_status_json(&mut jw, &s));
    let j = String::from_utf8(jw.as_bytes().to_vec()).unwrap();
    assert!(j.contains(concat!(
        r#""ip":"255.255.255.255","mask":"255.255.255.255","#,
        r#""gw":"255.255.255.254","dns":"254.255.255.255""#
    )));
}

// ---------------------------------------------------------------- 2.1 members

#[test]
fn status_stm_status_gstax_members_and_field_rules_of_the_2_1_members() {
    let mut s = StatusSnapshot {
        have_stm_status: true,
        ..StatusSnapshot::default()
    };
    s.stm_status.v3 = true;
    s.stm_status.lease = LeaseState::Running;
    s.stm_status.lease_remain_s = 1800;
    s.stm_status.lease_client = true;
    s.stm_status.lease_timeout_min = 60;
    s.stm_status.failsafe_mask = 0x0003;
    s.stm_status.safe_mode = true;
    s.stm_status.wdg_resets = 3;
    s.stm_status.uart_ore = 1;
    s.stm_status.uart_fe = 2;
    s.stm_status.uart_ne = 3;
    s.stm_status.rx_dropped = 4;
    s.stm_status.cfg_flags = STM_CFG_LAYOUT_CRC | STM_CFG_READ_FAILED;
    s.stm_status.cfg_events = 5;
    s.stm_status.eep_writes = 6;
    s.stm_status.temp_age_s = 7;
    s.stm_status.ow_scan_age_s = 8;
    s.stm_status.sys_flags = STM_SYS_PROTECT_SUSPENDED;
    let mut j = status(&s);
    let st = [
        r#""status":{"uptime":0,"resets":0,"bootReason":0,"rxOverflow":0,"parseErr":0,"#,
        r#""eepState":0,"lease":"running","leaseRemainS":1800,"leaseClient":true,"#,
        r#""leaseTimeoutMin":60,"failsafeMask":3,"safeMode":true,"wdgResets":3,"uartOre":1,"#,
        r#""uartFe":2,"uartNe":3,"rxDropped":4,"cfgFlags":["#,
        &q(stm_cfg_flag_name(0)),
        ",",
        &q(stm_cfg_flag_name(7)),
        r#"],"cfgEvents":5,"eepWrites":6,"tempAgeS":7,"owScanAgeS":8,"protectSuspended":true},"#,
    ]
    .concat();
    assert!(j.contains(&st));
    s.stm_status.sys_flags = 0xFE;
    s.stm_status.cfg_flags = 0;
    j = status(&s);
    assert!(j.contains("\"cfgFlags\":[],"));
    assert!(j.contains("\"protectSuspended\":false}"));
    s.stm_status.cfg_flags = 0xFF;
    j = status(&s);
    let all: Vec<String> = (0..8).map(|b| q(stm_cfg_flag_name(b))).collect();
    assert!(j.contains(&format!("\"cfgFlags\":[{}],", all.join(","))));
    // gstat (v2) keeps its six members
    s.stm_status.v3 = false;
    j = status(&s);
    assert!(j.contains("\"eepState\":0},\"espRx\""));

    // lease modes, trial and next calibration rules
    s.lease.mode = LeaseMode::Emulated;
    s.lease.state = LeaseState::Running;
    s.lease.remain_s = 5;
    s.lease.regulator = RegulatorCause::Alive;
    s.lease.config_trusted = false;
    j = status(&s);
    assert!(j.contains(concat!(
        r#""lease":{"mode":"esp","state":"running","remainS":5,"timeoutMin":0,"#,
        r#""failsafeMask":0,"regulator":"alive","regulatorLostS":0,"#,
        r#""configSynced":false,"configFailed":false,"configTrusted":false},"#,
        r#""learnTime":null}"#
    )));
    s.net_trial_active = true;
    s.net_trial_remain_s = 0;
    s.next_calib_epoch = 1;
    j = status(&s);
    assert!(j.contains("\"trial\":{\"remainS\":0}}"));
    assert!(j.contains("\"next\":null}")); // local time not valid
    s.next_calib_local = local(2027, 1, 2, 23, 59, 58);
    j = status(&s);
    assert!(j.contains("\"next\":\"2027-01-02T23:59:58\"}"));
    s.next_calib_epoch = 0;
    j = status(&s);
    assert!(j.contains("\"next\":null}"));
    s.next_calib_epoch = -1;
    j = status(&s);
    assert!(j.contains("\"next\":null}"));
    // station null (C++ nullptr: no Rust form, the empty station), client id unterminated
    // (Rust: a full text)
    s.station = b"";
    s.mqtt_client_id = full(b'c');
    j = status(&s);
    assert!(j.starts_with("{\"station\":\"\",\"esp\":"));
    assert!(j.contains(&format!("\"clientId\":\"{}\",", "c".repeat(CLIENT_ID_MAX))));
    // config source names; out of range -> stored
    let sources = [
        "stored",
        "imported",
        "defaults",
        "defaults_after_error",
        "backup",
        "stored",
        "stored",
    ];
    for (i, source) in (0u8..).zip(sources) {
        s.config_source = if i == 6 { 255 } else { i };
        j = status(&s);
        assert!(
            j.contains(&format!("\"config\":{{\"source\":\"{source}\"")),
            "{i}"
        );
    }
    // support names
    s.stm_support = StmSupport::Supported;
    j = status(&s);
    assert!(j.contains("\"support\":\"ok\""));
    s.mqtt_ha_status = HaStatus::Online;
    j = status(&s);
    assert!(j.contains("\"haStatus\":\"online\""));
}

#[test]
fn valves_v3_fields_failsafe_and_calibration_end() {
    let mut st = ValveState {
        known: true,
        status: 1,
        has_extended: true,
        has_v3: true,
        stm_flags: STM_FLAG_FS_LEASE | STM_FLAG_SVC_HOLD,
        fault: 5,
        drive: 40,
        retry_s: 1200,
        retries: 3,
        fs_pct: 30,
        ..ValveState::default()
    };
    let cfg = ValveConfig::default();
    let view = |st: &ValveState, end_valid: bool| {
        let mut v = ValveView {
            state: Some(st),
            config: Some(&cfg),
            ..ValveView::default()
        };
        v.sensor_slot[1] = 9; // only sensor 2 assigned
        v.sensor_name[1] = b"Wall";
        v.sensor_valid[1] = true;
        v.sensor_tenths[1] = 199;
        v.calibration_end = LocalTime {
            valid: end_valid,
            ..local(2026, 9, 21, 4, 7, 9)
        };
        valves(&[v], 0)
    };
    let mut j = view(&st, true);
    assert!(j.contains("\"sensors\":[{\"sensor\":2,\"slot\":9,\"name\":\"Wall\",\"temp\":19.9}],"));
    let tail = [
        r#""moveSeq":0,"v3":{"flags":["#,
        &q(stm_flag_name(0)),
        ",",
        &q(stm_flag_name(9)),
        r#"],"fault":"#,
        &q(valve_fault_name(5)),
        r#","drive":40,"retryS":1200,"retries":3}},"#,
        r#""failsafe":{"state":"lease","pct":30},"#,
        r#""calibrationEnd":"2026-09-21T04:07:09"}]}"#,
    ]
    .concat();
    assert!(j.len() >= tail.len());
    assert!(j.ends_with(&tail));
    {
        let mut v = ValveView {
            state: Some(&st),
            config: Some(&cfg),
            calibration_end: local(2026, 9, 21, 4, 7, 9),
            ..ValveView::default()
        };
        v.sensor_slot[1] = 9;
        v.sensor_name[1] = b"Wall";
        v.sensor_valid[1] = true;
        v.sensor_tenths[1] = 199;
        check_overflow(|jw| write_valves_json(jw, &[v], 0), j.len());
    }
    // blocked, pct 100 and 0, hold -> null, invalid pct above 100 -> null
    st.stm_flags = STM_FLAG_FS_BLOCKED;
    st.fs_pct = 100;
    j = view(&st, true);
    assert!(j.contains("\"failsafe\":{\"state\":\"blocked\",\"pct\":100}"));
    st.stm_flags = 0xFFFF;
    j = view(&st, true);
    let flags: Vec<String> = (0..16).map(|b| q(stm_flag_name(b))).collect();
    assert!(j.contains(&format!("\"flags\":[{}],", flags.join(","))));
    st.stm_flags = 0;
    st.fs_pct = 0;
    j = view(&st, true);
    assert!(j.contains("\"flags\":[],"));
    assert!(j.contains("\"failsafe\":{\"state\":\"off\",\"pct\":0}"));
    st.fs_pct = 101;
    j = view(&st, true);
    assert!(j.contains("\"pct\":null}"));
    st.fs_pct = FAILSAFE_HOLD;
    st.fs_override = true; // ESP emulation
    j = view(&st, true);
    assert!(j.contains("\"failsafe\":{\"state\":\"lease\",\"pct\":null}"));
    // v3 null without has_v3; calibrationEnd null when not valid
    st.has_v3 = false;
    j = view(&st, false);
    assert!(j.contains("\"v3\":null},"));
    assert!(j.contains("\"calibrationEnd\":null}"));
}

#[test]
fn health_document() {
    let mut h = HealthSnapshot::default();
    let mut j = build(|jw| write_health_json(jw, &h));
    assert_eq!(
        j,
        concat!(
            r#"{"ok":true,"version":"","uptime":0,"heap":{"free":0,"min":0,"largest":0,"#,
            r#""minLargest":0},"tasks":[],"net":{"ip":false,"reachable":false,"#,
            r#""proven":false,"pingArmed":false,"evidence":null,"evidenceAgeS":null,"#,
            r#""ifaceRestarts":0,"trial":null},"ota":null,"log":{"persist":false,"#,
            r#""backlog":0,"flushes":0,"lastFlushAgeS":null,"lost":0,"failures":0}}"#
        )
    );
    h.version = b"2.1.0-revamped";
    h.uptime_s = 1234;
    h.free_heap = 142_336;
    h.min_free_heap = 118_420;
    h.largest_free_block = 90100;
    h.min_largest_free_block = 65536;
    h.tasks[0] = TaskStackInfo {
        name: b"stm",
        stack_bytes: 6144,
        min_free_bytes: 2100,
    };
    h.tasks[1] = TaskStackInfo {
        name: b"app",
        stack_bytes: 8192,
        min_free_bytes: 3200,
    };
    h.task_count = 2;
    h.net.ip_up = true;
    h.net.reachable = true;
    h.net.proven = true;
    h.net.ping_armed = true;
    h.net.evidence = NetEvidence::GatewayPing;
    h.net.evidence_age_s = 12;
    h.net.iface_restarts = 3;
    h.net.trial_active = true;
    h.net.trial_remaining_s = 44;
    h.ota.pending = true;
    h.ota.stm_required = true;
    h.ota.net_ok = true;
    h.ota.http_ok = false;
    h.ota.stm_ok = true;
    h.ota.healthy_for_s = 20;
    h.ota.remaining_s = 280;
    h.log.persist = true;
    h.log.backlog = 3;
    h.log.flushes = 12;
    h.log.flushed = true;
    h.log.last_flush_age_s = 40;
    h.log.lost = 1;
    h.log.failures = 2;
    j = build(|jw| write_health_json(jw, &h));
    let expected = concat!(
        r#"{"ok":true,"version":"2.1.0-revamped","uptime":1234,"#,
        r#""heap":{"free":142336,"min":118420,"largest":90100,"minLargest":65536},"#,
        r#""tasks":[{"name":"stm","stack":6144,"minFree":2100},"#,
        r#"{"name":"app","stack":8192,"minFree":3200}],"#,
        r#""net":{"ip":true,"reachable":true,"proven":true,"pingArmed":true,"#,
        r#""evidence":"ping","evidenceAgeS":12,"ifaceRestarts":3,"trial":{"remainS":44}},"#,
        r#""ota":{"stmRequired":true,"checks":{"net":true,"http":false,"stm":true},"#,
        r#""healthyForS":20,"remainS":280},"#,
        r#""log":{"persist":true,"backlog":3,"flushes":12,"lastFlushAgeS":40,"lost":1,"#,
        r#""failures":2}}"#
    );
    assert_eq!(j, expected);
    check_overflow(|jw| write_health_json(jw, &h), j.len());
    // evidence age unknown, too many tasks, a task without a name (C++ nullptr: the empty
    // name), no version (C++ nullptr: the empty version)
    h.net.evidence_age_s = u32::MAX;
    h.task_count = 200;
    for t in &mut h.tasks[2..] {
        t.name = b"t";
    }
    h.tasks[7].name = b"";
    h.version = b"";
    j = build(|jw| write_health_json(jw, &h));
    assert!(j.contains("\"evidence\":\"ping\",\"evidenceAgeS\":null,"));
    assert_eq!(
        j.matches("\"stack\":").count(),
        usize::from(HEALTH_TASK_MAX)
    );
    assert!(j.contains("{\"name\":\"\",\"stack\":0,\"minFree\":0}]"));
    assert!(j.contains("\"version\":\"\","));
    // no evidence: age is null even when known
    h.net.evidence = NetEvidence::None;
    h.net.evidence_age_s = 5;
    j = build(|jw| write_health_json(jw, &h));
    assert!(j.contains("\"evidence\":null,\"evidenceAgeS\":null,"));
    // one task exactly
    h.task_count = 1;
    j = build(|jw| write_health_json(jw, &h));
    assert!(j.contains("\"tasks\":[{\"name\":\"stm\",\"stack\":6144,\"minFree\":2100}],"));
    h.task_count = HEALTH_TASK_MAX;
    j = build(|jw| write_health_json(jw, &h));
    assert!(j.contains("{\"name\":\"\",\"stack\":0,\"minFree\":0}]"));
}

// ---------------------------------------------------------------- Rust firmware additions

#[test]
fn route_of_the_ota_switch_back() {
    let path = "/api/system/ota/switch-back";
    let m = route(P, path);
    assert_eq!(m.route, ApiRoute::OtaSwitchBack);
    assert_eq!(m.valve, NO_VALVE);
    assert!(m.name.is_empty());
    for other in [G, D, HttpMethod::Other] {
        assert_eq!(route(other, path).route, ApiRoute::MethodNotAllowed);
    }
    for p in [
        "/api/system/ota",
        "/api/system/ota/",
        "/api/system/ota/switch-back/",
        "/api/system/ota/switch",
        "/api/system/switch-back",
        "/api/ota/switch-back",
    ] {
        assert_eq!(route(P, p).route, ApiRoute::NotFound, "{p}");
    }
    // appended after the C++ routes: their numbers stay
    assert_eq!(ApiRoute::ImportReportDismiss as u8, 44);
    assert_eq!(ApiRoute::OtaSwitchBack as u8, 45);
}
