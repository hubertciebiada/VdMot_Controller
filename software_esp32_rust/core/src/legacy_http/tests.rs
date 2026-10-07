//! Port of test/native/test_legacy_http.cpp: the legacy route table (aliases, 405, 410 with
//! replacements) and golden documents of /valves, /temps and /volts.
//!
//! A C++ null name or unit is the empty slice (the C++ writes "" for both); a null path or a
//! null array with a count has no Rust form (named where they stood).

use super::*;
use crate::common::{copy_string, parse_one_wire_id, OneWireId, NO_VALVE};
use crate::config::ValveConfig;
use crate::valve_model::ValveState;
use std::string::String;
use std::vec;

fn build(f: impl FnOnce(&mut JsonWriter<'_>) -> bool) -> String {
    let mut buf = vec![0u8; 8192];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(f(&mut jw));
    assert!(jw.complete());
    String::from_utf8(jw.as_bytes().to_vec()).unwrap()
}

/// No buffer below the full length gives a complete document; nothing is written past it.
fn check_overflow(f: impl Fn(&mut JsonWriter<'_>) -> bool, full_len: usize) {
    let mut buf = vec![b'#'; full_len + 16];
    for cap in 0..=full_len {
        buf.fill(b'#');
        let mut jw = JsonWriter::new(&mut buf[..cap]);
        assert!(!(f(&mut jw) && jw.complete()), "cap {cap}");
        assert!(buf[cap..].iter().all(|&c| c == b'#'), "cap {cap}");
    }
}

fn oid(s: &str) -> OneWireId {
    parse_one_wire_id(s.as_bytes()).unwrap()
}

fn matched(m: HttpMethod, p: &str) -> LegacyMatch {
    match_legacy_route(m, p.as_bytes())
}

const METHODS: [HttpMethod; 4] = [
    HttpMethod::Get,
    HttpMethod::Post,
    HttpMethod::Delete,
    HttpMethod::Other,
];

#[test]
fn legacy_routes_aliases_per_method() {
    let rows = [
        ("/valves", LegacyRoute::Valves, HttpMethod::Get),
        ("/temps", LegacyRoute::Temps, HttpMethod::Get),
        ("/volts", LegacyRoute::Volts, HttpMethod::Get),
        ("/setvalve", LegacyRoute::SetValve, HttpMethod::Post),
    ];
    for (path, route, method) in rows {
        for m in METHODS {
            let lm = matched(m, path);
            assert_eq!(lm.replacement, "", "{path} {m:?}");
            assert_eq!(
                lm.route,
                if m == method {
                    route
                } else {
                    LegacyRoute::MethodNotAllowed
                },
                "{path} {m:?}"
            );
        }
    }
}

#[test]
fn legacy_routes_the_410_table() {
    let rows = [
        ("/netinfo", "/api/status"),
        ("/sysinfo", "/api/status"),
        ("/sysdyninfo", "/api/status"),
        ("/update/identity", "/api/status"),
        ("/netconfig", "/api/config"),
        ("/protconfig", "/api/config"),
        ("/valvesconfig", "/api/config"),
        ("/tempsconfig", "/api/config"),
        ("/voltsconfig", "/api/config"),
        ("/sysconfig", "/api/config"),
        ("/sysLogCfg", "/api/config"),
        ("/motorconfig", "/api/stm/motor"),
        ("/tempsensorsid", "/api/sensors"),
        ("/voltsensorsid", "/api/sensors"),
        ("/fsdir", "/api/files"),
        ("/fupload", "/api/stm/images"),
        ("/stmupdate", "/#maintenance"),
        ("/stmupdstatus", "/api/stm/flash"),
        ("/stmdoupdate", "/api/stm/flash"),
        ("/update", "/api/ota/esp"),
        (
            "/cmd",
            concat!(
                "/api/system/reboot, /api/valves/calibrate, /api/valves/assembly, ",
                "/api/valves/detect, /api/sensors/scan, /api/mqtt/reconnect, /api/mqtt/discovery"
            ),
        ),
        ("/valvesctrlconfig", "removed: PI control"),
        ("/msgconfig", "removed: messenger"),
        ("/testPO", "removed: messenger"),
        ("/testEmail", "removed: messenger"),
        ("/ssidinfo", "removed: WiFi scan"),
        ("/auth", "removed: web login"),
    ];
    for (path, replacement) in rows {
        for m in METHODS {
            let lm = matched(m, path);
            assert_eq!(lm.route, LegacyRoute::Gone, "{path}");
            assert_eq!(lm.replacement, replacement, "{path}");
        }
    }
}

#[test]
fn legacy_routes_everything_else_is_none() {
    let paths = [
        "/valves/",
        "/Valves",
        "/valve",
        "/valvess",
        "/",
        "",
        "/api/valves",
        "/netinfo/",
        "/NETINFO",
        "/update/",
        "/cm",
        "/setvalve/1",
        "/temps?x=1",
    ];
    for p in paths {
        let lm = matched(HttpMethod::Get, p);
        assert_eq!(lm.route, LegacyRoute::None, "{p}");
        assert_eq!(lm.replacement, "", "{p}");
    }
    // C++ matchLegacyRoute(Get, nullptr, 7): no Rust form.
    // only the bytes of the slice count (C++ `len`)
    assert_eq!(
        match_legacy_route(HttpMethod::Get, &b"/valvesX"[..7]).route,
        LegacyRoute::Valves
    );
    assert_eq!(
        match_legacy_route(HttpMethod::Get, &b"/valves"[..6]).route,
        LegacyRoute::None
    );
}

#[test]
fn legacy_valves_document() {
    let mut st = [ValveState::default(); 5];
    let mut cfg: [ValveConfig; 5] = Default::default();
    copy_string(&mut cfg[0].name, b"Bad");
    st[0].status = 1;
    st[0].position = 40;
    st[0].mean_current = 12;
    st[0].desired_valid = true;
    st[0].desired = 55;
    st[0].stm_target_known = true;
    st[0].stm_target = 50;
    st[0].moves = 7;
    st[0].open_count = 300;
    st[0].close_count = 310;
    st[0].dead_zone = 4;
    st[0].calib_retries = 2;
    st[1].status = 6; // no valve: skipped
    st[2].status = 0; // no data: skipped
    copy_string(&mut cfg[3].name, b"WC");
    st[3].status = 2;
    st[3].position = 10;
    st[3].stm_target_known = true;
    st[3].stm_target = 30;
    st[3].calibrating = true;
    st[4].status = 9;
    st[4].position = 77;
    let mut v: [ValveView<'_>; 5] = core::array::from_fn(|i| ValveView {
        state: Some(&st[i]),
        config: Some(&cfg[i]),
        ..ValveView::default()
    });
    v[0].sensor_slot[0] = 3;
    v[0].sensor_name[0] = b"Floor";
    v[0].sensor_valid[0] = true;
    v[0].sensor_tenths[0] = 215;
    v[0].sensor_slot[1] = 4;
    v[0].sensor_name[1] = b"Wall";
    v[0].sensor_valid[1] = false;
    v[3].sensor_slot[1] = 9;
    // C++ sensorName[1] = nullptr: the empty name
    v[3].sensor_name[1] = b"";
    v[3].sensor_valid[1] = true;
    v[3].sensor_tenths[1] = -5;
    let j = build(|jw| write_legacy_valves_json(jw, &v));
    let expected = concat!(
        r#"{"valves":["#,
        r#"{"idx":1,"name":"Bad","state":1,"pos":40,"meanCur":12,"targetPos":55,"#,
        r#""link":0,"moves":7,"oc":300,"cc":310,"dc":4,"cr":2,"#,
        r#""tIdxName1":"Floor","temp1":21.5,"tIdxName2":"Wall","temp2":"failed","#,
        r#""controlActive":0},"#,
        r#"{"idx":4,"name":"WC","state":2,"pos":10,"meanCur":0,"targetPos":30,"#,
        r#""link":0,"moves":0,"oc":0,"cc":0,"dc":0,"cr":0,"#,
        r#""tIdxName2":"","temp2":-0.5,"controlActive":0,"calibration":1},"#,
        r#"{"idx":5,"name":"","state":9,"pos":77,"meanCur":0,"targetPos":77,"#,
        r#""link":0,"moves":0,"oc":0,"cc":0,"dc":0,"cr":0,"controlActive":0}]}"#
    );
    assert_eq!(j, expected);
    check_overflow(|jw| write_legacy_valves_json(jw, &v), j.len());
    // C++ writeLegacyValvesJson(jw, nullptr, 5): no Rust form, the empty slice.
    assert_eq!(
        build(|jw| write_legacy_valves_json(jw, &[])),
        "{\"valves\":[]}"
    );
    assert_eq!(
        build(|jw| write_legacy_valves_json(jw, &v[..0])),
        "{\"valves\":[]}"
    );
    // a view without state or config is skipped
    v[0].state = None;
    v[4].config = None;
    let k = build(|jw| write_legacy_valves_json(jw, &v));
    assert!(!k.contains("\"idx\":1,"));
    assert!(!k.contains("\"idx\":5,"));
    assert!(k.contains("\"idx\":4,"));
}

#[test]
fn legacy_temps_document() {
    let mut t = [SensorView::default(); 6];
    t[0].slot = 1;
    t[0].name = b"Floor";
    t[0].active = true;
    t[0].id = oid("28-84-37-94-97-ff-03-23");
    t[0].valid = true;
    t[0].value = 215;
    t[0].valve = 2; // used by a valve
    t[1].slot = 2;
    t[1].name = b"Outside";
    t[1].active = true;
    t[1].id = oid("28-11-22-33-44-55-66-8c");
    t[1].valid = false; // stale
    t[1].value = 100;
    t[2].slot = 3; // inactive
    t[2].id = oid("28-11-22-33-44-55-66-8c");
    t[3].slot = 4; // no id
    t[3].active = true;
    t[4].slot = 0; // bus sensor without a slot
    t[4].active = true;
    t[4].id = oid("28-84-37-94-97-ff-03-23");
    t[5].slot = 6;
    // C++ name = nullptr: the empty name
    t[5].name = b"";
    t[5].active = true;
    t[5].id = oid("28-84-37-94-97-ff-03-23");
    t[5].valid = true;
    t[5].value = -12;
    t[5].valve = NO_VALVE;
    let j = build(|jw| write_legacy_temps_json(jw, &t, false));
    let expected = concat!(
        r#"[{"id":"28-11-22-33-44-55-66-8c","name":"Outside","temp":"failed"},"#,
        r#"{"id":"28-84-37-94-97-ff-03-23","name":"","temp":-1.2}]"#
    );
    assert_eq!(j, expected);
    let all = build(|jw| write_legacy_temps_json(jw, &t, true));
    assert_eq!(
        all,
        String::from(r#"[{"id":"28-84-37-94-97-ff-03-23","name":"Floor","temp":21.5},"#)
            + &expected[1..]
    );
    check_overflow(|jw| write_legacy_temps_json(jw, &t, true), all.len());
    t[0].valve = VALVE_COUNT; // out of range counts as unused
    assert_eq!(
        build(|jw| write_legacy_temps_json(jw, &t[..1], false)),
        r#"[{"id":"28-84-37-94-97-ff-03-23","name":"Floor","temp":21.5}]"#
    );
    // C++ writeLegacyTempsJson(jw, nullptr, 3, true): no Rust form, the empty slice.
    assert_eq!(build(|jw| write_legacy_temps_json(jw, &[], true)), "[]");
}

#[test]
fn legacy_volts_document() {
    let mut v = [SensorView::default(); 4];
    v[0].slot = 1;
    v[0].name = b"Supply";
    v[0].active = true;
    v[0].id = oid("26-11-22-33-44-55-66-29");
    v[0].valid = true;
    v[0].raw = 1208;
    v[0].value = 12080; // (1208 / 100 + 0) x 1, in milli-units
    v[0].unit = b"V";
    v[1].slot = 2;
    // C++ name and unit = nullptr: the empty texts
    v[1].name = b"";
    v[1].active = true;
    v[1].id = oid("26-11-22-33-44-55-66-29");
    v[1].valid = false;
    v[1].unit = b"";
    v[2].slot = 3; // inactive
    v[2].id = oid("26-11-22-33-44-55-66-29");
    v[3].slot = 0; // unconfigured
    v[3].active = true;
    v[3].id = oid("26-11-22-33-44-55-66-29");
    let j = build(|jw| write_legacy_volts_json(jw, &v));
    assert_eq!(
        j,
        concat!(
            r#"[{"id":"26-11-22-33-44-55-66-29","name":"Supply","unit":"V","#,
            r#""value":12.080},"#,
            r#"{"id":"26-11-22-33-44-55-66-29","name":"","unit":"","value":"failed"}]"#
        )
    );
    check_overflow(|jw| write_legacy_volts_json(jw, &v), j.len());
    // C++ writeLegacyVoltsJson(jw, nullptr, 3): no Rust form, the empty slice.
    assert_eq!(build(|jw| write_legacy_volts_json(jw, &[])), "[]");
    v[0].active = false;
    v[1].id = OneWireId::default();
    assert_eq!(build(|jw| write_legacy_volts_json(jw, &v)), "[]");
}
