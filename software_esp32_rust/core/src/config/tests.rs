//! Port of test/native/test_config.cpp: defaults, per-key setter (every key, both sides of every
//! range, every type conversion), validation (every per-field and cross-field rule with its
//! path), JSON export (golden), JSON patch reader (syntax, paths, secrets, fuzz), binary NVS
//! encoding (round trip, every error), CRC-32. C++ cases without a Rust form are named in
//! comments where they stood: null pointers, values a Rust member cannot hold (an enum out of
//! range, a bool byte 2, a text without its NUL) and bytes after a text's NUL. After the C++
//! cases: the Rust-only cases.

use super::*;
use crate::common::{copy_string, parse_one_wire_id, OneWireId, TextView};
use crate::json_writer::JsonWriter;
use crate::test_support::{assert_text, Rng};
use std::string::{String, ToString};
use std::vec::Vec;
use std::{format, vec};

pub(super) const ID_A: &str = "28-84-37-94-97-ff-03-23";
pub(super) const ID_B: &str = "28-aa-bb-cc-dd-ee-01-67";
const ID_V: &str = "26-11-22-33-44-55-66-29";

/// C++ S(s): a string value.
fn vs(x: &str) -> ConfigValue<'_> {
    ConfigValue::Str(x.as_bytes())
}
/// C++ SL(s, len): the bytes as given (a NUL inside is data).
fn vsl(x: &[u8]) -> ConfigValue<'_> {
    ConfigValue::Str(x)
}
fn vi(v: i64) -> ConfigValue<'static> {
    ConfigValue::Int(v)
}
fn vf(v: f64) -> ConfigValue<'static> {
    ConfigValue::Float(v)
}
fn vb(v: bool) -> ConfigValue<'static> {
    ConfigValue::Bool(v)
}
fn vn() -> ConfigValue<'static> {
    ConfigValue::Null
}

fn set(c: &mut Config, path: &str, v: ConfigValue<'_>) -> SetResult {
    set_config_value(c, path.as_bytes(), &v, false)
}

fn set_clear(c: &mut Config, path: &str, v: ConfigValue<'_>) -> SetResult {
    set_config_value(c, path.as_bytes(), &v, true)
}

/// A change to a config (C++ lambdas of the table tests).
pub(super) type Edit = fn(&mut Config);

pub(super) fn oid(s: &str) -> OneWireId {
    parse_one_wire_id(s.as_bytes()).expect("a valid id")
}

/// C++ strcpy into a text member.
pub(super) fn strcpy(t: &mut TextView, s: &str) {
    assert!(copy_string(t, s.as_bytes()), "{s} fits");
}

pub(super) fn validate_path(c: &Config) -> String {
    let mut path = [b'x'; 64];
    match validate_config(c, &mut path) {
        Ok(()) => "OK".to_string(),
        Err(n) => String::from_utf8_lossy(&path[..n]).into_owned(),
    }
}

fn export_json(c: &Config) -> String {
    let mut buf = vec![0u8; 8192];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write_config_json(&mut jw, c, None));
    assert!(jw.complete());
    String::from_utf8_lossy(jw.as_bytes()).into_owned()
}

fn patch_bytes(c: &mut Config, json: &[u8]) -> (PatchResult, String) {
    let mut path = [b'x'; 80];
    let (r, n) = apply_config_json(c, json, &mut path);
    (r, String::from_utf8_lossy(&path[..n]).into_owned())
}

fn patch(c: &mut Config, json: &str) -> (PatchResult, String) {
    patch_bytes(c, json.as_bytes())
}

/// Equal stored configs: the `cfg` and the `cfgx` blob.
pub(super) fn same_config(a: &Config, b: &Config) -> bool {
    let (mut ba, mut bb) = (vec![0u8; CONFIG_BLOB_MAX], vec![0u8; CONFIG_BLOB_MAX]);
    let na = encode_config(a, &mut ba);
    let nb = encode_config(b, &mut bb);
    let (mut xa, mut xb) = (
        vec![0u8; CONFIG_EXT_BLOB_MAX],
        vec![0u8; CONFIG_EXT_BLOB_MAX],
    );
    let ma = encode_config_ext(a, &mut xa, &[]);
    let mb = encode_config_ext(b, &mut xb, &[]);
    na > 0 && na == nb && ba[..na] == bb[..nb] && ma > 0 && ma == mb && xa[..ma] == xb[..mb]
}

/// Config with every field of the 2.0.0 blob away from its default, still valid (the keys added
/// later keep their defaults: full_config_ext()).
fn full_config() -> Config {
    let mut c = Config::default();
    let ok = |r: SetResult| assert_eq!(r, SetResult::Ok);
    ok(set(&mut c, "station", vs("Heizung OG")));
    ok(set(&mut c, "net.iface", vi(2)));
    ok(set(&mut c, "net.dhcp", vb(false)));
    ok(set(&mut c, "net.ip", vs("192.168.1.50")));
    ok(set(&mut c, "net.mask", vs("255.255.255.0")));
    ok(set(&mut c, "net.gateway", vs("192.168.1.1")));
    ok(set(&mut c, "net.dns", vs("8.8.8.8")));
    ok(set(&mut c, "net.ssid", vs("My Wifi")));
    ok(set(&mut c, "net.wifiPassword", vs("secret123")));
    ok(set(&mut c, "net.reconnectTimeoutMin", vi(17)));
    ok(set(&mut c, "time.ntpServer", vs("192.168.1.1")));
    ok(set(&mut c, "time.tzName", vs("Europe/Warsaw")));
    ok(set(
        &mut c,
        "time.tzPosix",
        vs("CET-1CEST,M3.5.0,M10.5.0/3x"),
    ));
    ok(set(&mut c, "syslog.level", vi(3)));
    ok(set(&mut c, "syslog.server", vs("10.0.0.9")));
    ok(set(&mut c, "syslog.port", vi(1514)));
    // Plain MQTT: HA mode needs separate topics and the decimal point.
    ok(set(&mut c, "mqtt.mode", vi(1)));
    ok(set(&mut c, "mqtt.host", vs("broker.lan")));
    ok(set(&mut c, "mqtt.port", vi(8883)));
    ok(set(&mut c, "mqtt.user", vs("mq")));
    ok(set(&mut c, "mqtt.password", vs("mqpw")));
    ok(set(&mut c, "mqtt.keepAliveS", vi(30)));
    ok(set(&mut c, "mqtt.publishIntervalS", vi(120)));
    ok(set(&mut c, "mqtt.minDelayS", vi(7)));
    // Every MQTT flag away from its default.
    for flag in [
        "separate",
        "allTemps",
        "upTime",
        "onChange",
        "retained",
        "plainText",
        "diag",
        "newDiag",
        "events",
        "haDiscoveryOnConnect",
    ] {
        ok(set(&mut c, &format!("mqtt.{flag}"), vb(false)));
    }
    ok(set(&mut c, "mqtt.pathAsRoot", vb(true)));
    ok(set(&mut c, "mqtt.germanDecimal", vb(true)));
    ok(set(&mut c, "valves.1.name", vs("Bad")));
    ok(set(&mut c, "valves.1.active", vb(true)));
    ok(set(&mut c, "valves.12.name", vs("Kitchen")));
    ok(set(&mut c, "temps.1.name", vs("t1")));
    ok(set(&mut c, "temps.1.id", vs(ID_A)));
    ok(set(&mut c, "temps.1.active", vb(true)));
    ok(set(&mut c, "temps.1.offset", vf(-1.5)));
    ok(set(&mut c, "temps.34.id", vs(ID_B)));
    ok(set(&mut c, "temps.34.offset", vf(10.0)));
    ok(set(&mut c, "volts.8.name", vs("bat")));
    ok(set(&mut c, "volts.8.id", vs(ID_V)));
    ok(set(&mut c, "volts.8.active", vb(true)));
    ok(set(&mut c, "volts.8.offset", vf(-0.25)));
    ok(set(&mut c, "volts.8.factor", vf(0.01)));
    ok(set(&mut c, "volts.8.unit", vs("V")));
    ok(set(&mut c, "calib.dayMask", vi(127)));
    ok(set(&mut c, "calib.hour", vi(23)));
    ok(set(&mut c, "calib.minute", vi(59)));
    ok(set(&mut c, "persistLog", vb(false)));
    assert_eq!(validate_path(&c), "OK");
    c
}

/// full_config() with every key of the `cfgx` blob away from its default too.
fn full_config_ext() -> Config {
    let mut c = full_config();
    let ok = |r: SetResult| assert_eq!(r, SetResult::Ok);
    ok(set(
        &mut c,
        "web.allowedHosts",
        vs("vdmot.lan, 192.168.1.9"),
    ));
    ok(set(&mut c, "mqtt.rootTopic", vs("VdMotFBH")));
    ok(set(&mut c, "mqtt.clientId", vs("VdMot-east-6c1e51")));
    ok(set(&mut c, "mqtt.discoveryPrefix", vs("ha/discovery")));
    ok(set(&mut c, "failsafe.timeoutMin", vi(1440)));
    ok(set(
        &mut c,
        "valves.1.failsafePct",
        vi(i64::from(FAILSAFE_HOLD)),
    ));
    ok(set(&mut c, "valves.12.failsafePct", vi(0)));
    ok(set(&mut c, "valves.2.topic", vs("Bad/WC")));
    ok(set(&mut c, "valves.12.topic", vs("x")));
    ok(set(&mut c, "temps.1.topic", vs("t/1")));
    ok(set(&mut c, "temps.34.topic", vs("t34")));
    ok(set(&mut c, "volts.8.topic", vs("battery")));
    assert_eq!(validate_path(&c), "OK");
    c
}

pub(super) fn fix_crc(b: &mut [u8]) {
    let n = b.len();
    let crc = crc32(&b[..n - 4], 0);
    b[n - 4..].copy_from_slice(&crc.to_le_bytes());
}

pub(super) fn encode(c: &Config) -> Vec<u8> {
    let mut b = vec![0u8; CONFIG_BLOB_MAX];
    let n = encode_config(c, &mut b);
    assert!(n > 0);
    b.truncate(n);
    b
}

/// Offset of the first payload byte of a field, found by encoding two configs that differ only
/// there.
pub(super) fn diff_offset(a: &Config, b: &Config) -> usize {
    let (ea, eb) = (encode(a), encode(b));
    assert_eq!(ea.len(), eb.len());
    (8..ea.len() - 4)
        .find(|&k| ea[k] != eb[k])
        .expect("a difference")
}

// ---------------------------------------------------------------- defaults

#[test]
fn defaults_are_the_documented_values_and_validate() {
    let mut c = Config::default();
    c.station[0] = b'X';
    c.mqtt.port = 1;
    set_defaults(&mut c);
    assert_eq!(c.schema, 2);
    assert_eq!(CONFIG_BASE_SCHEMA, 1);
    assert_eq!(CONFIG_JSON_SCHEMA, 2);
    assert_text(&c.station, "VdMot");
    assert_eq!(c.net.iface, NetInterface::Auto);
    assert!(c.net.dhcp);
    assert_eq!(c.net.ip, 0);
    assert_eq!(c.net.reconnect_timeout_min, 5);
    assert_text(&c.time.ntp_server, "pool.ntp.org");
    assert_text(&c.time.tz_name, "Europe/Berlin");
    assert_eq!(c.syslog.level, 0);
    assert_eq!(c.syslog.port, 514);
    assert_eq!(c.mqtt.mode, MqttMode::Off);
    assert_eq!(c.mqtt.port, 1883);
    assert_eq!(c.mqtt.keep_alive_s, 60);
    assert_eq!(c.mqtt.publish_interval_s, 10);
    assert_eq!(c.mqtt.min_delay_s, 5);
    assert!(c.mqtt.separate);
    assert!(c.mqtt.all_temps);
    assert!(!c.mqtt.path_as_root);
    assert!(c.mqtt.up_time);
    assert!(c.mqtt.on_change);
    assert!(c.mqtt.retained);
    assert!(c.mqtt.plain_text);
    assert!(c.mqtt.diag);
    assert!(!c.mqtt.german_decimal);
    assert!(c.mqtt.new_diag);
    assert!(c.mqtt.events);
    assert!(c.mqtt.ha_discovery_on_connect);
    assert_eq!(c.volts[7].factor, 1.0);
    assert_eq!(c.calib.day_mask, 9);
    assert_eq!(c.calib.hour, 0);
    assert_eq!(c.calib.minute, 0);
    assert!(c.persist_log);
    // Keys of the cfgx blob.
    assert!(c.web.allowed_hosts.is_empty());
    assert!(c.mqtt.root_topic.is_empty());
    assert!(c.mqtt.client_id.is_empty());
    assert_text(&c.mqtt.discovery_prefix, "homeassistant");
    assert_eq!(c.failsafe.timeout_min, 60);
    for v in &c.valves {
        assert_eq!(v.failsafe_pct, 50);
        assert!(v.topic.is_empty());
    }
    assert!(c.temps[33].topic.is_empty());
    assert!(c.volts[7].topic.is_empty());
    assert_eq!(validate_path(&c), "OK");
}

#[test]
fn result_names() {
    assert_eq!(set_result_name(SetResult::Ok), "ok");
    assert_eq!(set_result_name(SetResult::UnknownKey), "unknown_key");
    assert_eq!(set_result_name(SetResult::WrongType), "wrong_type");
    assert_eq!(set_result_name(SetResult::OutOfRange), "out_of_range");
    assert_eq!(set_result_name(SetResult::ReadOnly), "read_only");
    // C++ setResultName(99) == "unknown": a Rust enum cannot hold 99.
    assert_eq!(SetResult::from_raw(99), None);
    assert_eq!(patch_result_name(PatchResult::Ok), "ok");
    assert_eq!(patch_result_name(PatchResult::Malformed), "malformed");
    assert_eq!(patch_result_name(PatchResult::UnknownKey), "unknown_key");
    assert_eq!(patch_result_name(PatchResult::WrongType), "wrong_type");
    assert_eq!(patch_result_name(PatchResult::OutOfRange), "out_of_range");
    assert_eq!(patch_result_name(PatchResult::ReadOnly), "read_only");
    assert_eq!(patch_result_name(PatchResult::Invalid), "invalid");
    assert_eq!(PatchResult::from_raw(99), None);
}

// ---------------------------------------------------------------- setter: paths

#[test]
fn setter_path_parsing() {
    let mut c = Config::default();
    // C++ set(c, nullptr, ..): no Rust form.
    let unknown = [
        "",
        "nope",
        "net",
        "net.",
        ".net.iface",
        "net..iface",
        "net.iface.x",
        "net.nope",
        "Net.iface",
        "net.ifac",
        "net.ifacee",
    ];
    for p in unknown {
        assert_eq!(set(&mut c, p, vi(1)), SetResult::UnknownKey, "{p}");
    }
    for p in [
        "station.x",
        "valves",
        "valves.name",
        "valves.1",
        "valves.0.name",
        "valves.01.name",
        "valves.13.name",
        "valves.-1.name",
        "valves.1.name.x",
        "valves.1.nope",
        "temps.35.name",
        "volts.9.name",
    ] {
        assert_eq!(set(&mut c, p, vs("a")), SetResult::UnknownKey, "{p}");
    }
    assert_eq!(set(&mut c, "calib.1.hour", vi(1)), SetResult::UnknownKey);
    assert_eq!(set(&mut c, "persistLog.x", vb(true)), SetResult::UnknownKey);
    assert_eq!(set(&mut c, "clearSecrets", vb(true)), SetResult::UnknownKey);
    assert_eq!(set(&mut c, "stationSet", vb(true)), SetResult::UnknownKey);
    assert_eq!(set(&mut c, "net.ssidSet", vb(true)), SetResult::UnknownKey);
    // Path length limit: 64 chars.
    let mut long_path = String::from("valves.1.name");
    long_path.push_str(&"x".repeat(51));
    assert_eq!(long_path.len(), 64);
    assert_eq!(set(&mut c, &long_path, vs("a")), SetResult::UnknownKey);
    long_path.push('x');
    assert_eq!(set(&mut c, &long_path, vs("a")), SetResult::UnknownKey);
    // Array bounds: 1 and N work.
    assert_eq!(set(&mut c, "valves.1.name", vs("a")), SetResult::Ok);
    assert_eq!(set(&mut c, "valves.12.name", vs("b")), SetResult::Ok);
    assert_eq!(set(&mut c, "temps.34.name", vs("t")), SetResult::Ok);
    assert_eq!(set(&mut c, "volts.8.name", vs("v")), SetResult::Ok);
    assert_text(&c.valves[0].name, "a");
    assert_text(&c.valves[11].name, "b");
    assert_text(&c.temps[33].name, "t");
    assert_text(&c.volts[7].name, "v");
    // Nothing else was touched.
    let mut d = Config::default();
    d.valves[0] = c.valves[0].clone();
    d.valves[11] = c.valves[11].clone();
    d.temps[33] = c.temps[33].clone();
    d.volts[7] = c.volts[7].clone();
    assert!(same_config(&c, &d));
}

#[test]
fn schema_is_read_only_except_for_the_known_versions() {
    let mut c = Config::default();
    assert_eq!(set(&mut c, "schema", vi(1)), SetResult::Ok); // a 2.0.0 export
    assert_eq!(set(&mut c, "schema", vs("1")), SetResult::Ok);
    assert_eq!(set(&mut c, "schema", vf(1.0)), SetResult::Ok);
    assert_eq!(set(&mut c, "schema", vi(2)), SetResult::Ok);
    assert_eq!(set(&mut c, "schema", vi(3)), SetResult::ReadOnly);
    assert_eq!(set(&mut c, "schema", vi(0)), SetResult::ReadOnly);
    assert_eq!(set(&mut c, "schema", vi(-1)), SetResult::ReadOnly);
    assert_eq!(set(&mut c, "schema", vs("x")), SetResult::ReadOnly);
    assert_eq!(set(&mut c, "schema", vb(true)), SetResult::ReadOnly);
    assert_eq!(set(&mut c, "schema.x", vi(1)), SetResult::UnknownKey);
    assert_eq!(c.schema, 2);
    // No-op: the posted value is not stored.
    assert_eq!(set(&mut c, "schema", vi(1)), SetResult::Ok);
    assert_eq!(c.schema, 2);
}

// ---------------------------------------------------------------- setter: numbers

fn read_int_key(c: &Config, path: &str) -> i64 {
    match path {
        "net.iface" => c.net.iface as i64,
        "net.reconnectTimeoutMin" => c.net.reconnect_timeout_min.into(),
        "syslog.level" => c.syslog.level.into(),
        "syslog.port" => c.syslog.port.into(),
        "mqtt.mode" => c.mqtt.mode as i64,
        "mqtt.port" => c.mqtt.port.into(),
        "mqtt.keepAliveS" => c.mqtt.keep_alive_s.into(),
        "mqtt.publishIntervalS" => c.mqtt.publish_interval_s.into(),
        "mqtt.minDelayS" => c.mqtt.min_delay_s.into(),
        "calib.dayMask" => c.calib.day_mask.into(),
        "calib.hour" => c.calib.hour.into(),
        "calib.minute" => c.calib.minute.into(),
        _ => panic!("unknown key {path}"),
    }
}

#[test]
fn every_integer_key_accepts_exactly_its_range() {
    let keys: [(&str, i64, i64); 12] = [
        ("net.iface", 0, 2),
        ("net.reconnectTimeoutMin", 0, 240),
        ("syslog.level", 0, 3),
        ("syslog.port", 1, 65535),
        ("mqtt.mode", 0, 2),
        ("mqtt.port", 1, 65535),
        ("mqtt.keepAliveS", 5, 300),
        ("mqtt.publishIntervalS", 2, 3600),
        ("mqtt.minDelayS", 0, 3600),
        ("calib.dayMask", 0, 127),
        ("calib.hour", 0, 23),
        ("calib.minute", 0, 59),
    ];
    for (path, min, max) in keys {
        let mut c = Config::default();
        let before = read_int_key(&c, path);
        assert_eq!(
            set(&mut c, path, vi(min - 1)),
            SetResult::OutOfRange,
            "{path}"
        );
        assert_eq!(
            set(&mut c, path, vi(max + 1)),
            SetResult::OutOfRange,
            "{path}"
        );
        assert_eq!(
            set(&mut c, path, vi(-4294967296)),
            SetResult::OutOfRange,
            "{path}"
        );
        assert_eq!(
            set(&mut c, path, vi(i64::MAX)),
            SetResult::OutOfRange,
            "{path}"
        );
        assert_eq!(read_int_key(&c, path), before, "{path}");
        assert_eq!(set(&mut c, path, vi(min)), SetResult::Ok, "{path}");
        assert_eq!(read_int_key(&c, path), min, "{path}");
        assert_eq!(set(&mut c, path, vi(max)), SetResult::Ok, "{path}");
        assert_eq!(read_int_key(&c, path), max, "{path}");
        // Numbers as strings (legacy UI) and integral floats.
        let min_text = min.to_string();
        assert_eq!(set(&mut c, path, vs(&min_text)), SetResult::Ok, "{path}");
        assert_eq!(read_int_key(&c, path), min, "{path}");
        assert_eq!(set(&mut c, path, vf(max as f64)), SetResult::Ok, "{path}");
        assert_eq!(read_int_key(&c, path), max, "{path}");
        let over = (max + 1).to_string();
        assert_eq!(
            set(&mut c, path, vs(&over)),
            SetResult::OutOfRange,
            "{path}"
        );
        assert_eq!(
            set(&mut c, path, vf(max as f64 + 1.0)),
            SetResult::OutOfRange,
            "{path}"
        );
        // Wrong types.
        assert_eq!(
            set(&mut c, path, vf(min as f64 + 0.5)),
            SetResult::WrongType,
            "{path}"
        );
        assert_eq!(set(&mut c, path, vb(true)), SetResult::WrongType, "{path}");
        assert_eq!(set(&mut c, path, vn()), SetResult::WrongType, "{path}");
        for text in ["", " 3", "0x3", "+3", "abc"] {
            assert_eq!(
                set(&mut c, path, vs(text)),
                SetResult::WrongType,
                "{path} {text}"
            );
        }
        assert_eq!(
            set(&mut c, path, vf(f64::NAN)),
            SetResult::OutOfRange,
            "{path}"
        );
        assert_eq!(
            set(&mut c, path, vf(f64::INFINITY)),
            SetResult::OutOfRange,
            "{path}"
        );
        assert_eq!(
            set(&mut c, path, vf(f64::NEG_INFINITY)),
            SetResult::OutOfRange,
            "{path}"
        );
        assert_eq!(read_int_key(&c, path), max, "{path}");
    }
}

#[test]
fn integer_conversion_corner_cases() {
    let mut c = Config::default();
    assert_eq!(set(&mut c, "calib.hour", vs("7.0")), SetResult::Ok);
    assert_eq!(c.calib.hour, 7);
    assert_eq!(set(&mut c, "calib.hour", vs("1e1")), SetResult::Ok);
    assert_eq!(c.calib.hour, 10);
    assert_eq!(set(&mut c, "calib.hour", vs("-0")), SetResult::Ok);
    assert_eq!(c.calib.hour, 0);
    assert_eq!(set(&mut c, "calib.hour", vs("7.5")), SetResult::WrongType);
    assert_eq!(set(&mut c, "calib.hour", vs("07")), SetResult::WrongType); // JSON number grammar
    assert_eq!(set(&mut c, "calib.hour", vs("7.")), SetResult::WrongType);
    assert_eq!(set(&mut c, "calib.hour", vs(".5")), SetResult::WrongType);
    assert_eq!(set(&mut c, "calib.hour", vs("1e")), SetResult::WrongType);
    assert_eq!(set(&mut c, "calib.hour", vs("-")), SetResult::WrongType);
    assert_eq!(set(&mut c, "calib.hour", vsl(b"5\0")), SetResult::WrongType);
    // Only `len` bytes are parsed, in every part of the grammar.
    assert_eq!(set(&mut c, "calib.hour", vsl(&b"123"[..2])), SetResult::Ok);
    assert_eq!(c.calib.hour, 12);
    assert_eq!(set(&mut c, "calib.hour", vsl(&b"1e12"[..3])), SetResult::Ok);
    assert_eq!(c.calib.hour, 10);
    assert_eq!(
        set(&mut c, "temps.1.offset", vsl(&b"1.55"[..3])),
        SetResult::Ok
    );
    assert_eq!(c.temps[0].offset, 15);
    assert_eq!(set(&mut c, "temps.1.offset", vs("0.9")), SetResult::Ok);
    assert_eq!(c.temps[0].offset, 9);
    assert_eq!(set(&mut c, "temps.1.offset", vs("1.09e0")), SetResult::Ok);
    assert_eq!(c.temps[0].offset, 11);
    assert_eq!(set(&mut c, "calib.hour", vs("9e0")), SetResult::Ok);
    assert_eq!(c.calib.hour, 9);
    assert_eq!(set(&mut c, "calib.hour", vs("19")), SetResult::Ok);
    assert_eq!(c.calib.hour, 19);
    assert_eq!(set(&mut c, "calib.hour", vs("-5")), SetResult::OutOfRange);
    assert_eq!(set(&mut c, "calib.hour", vs("--5")), SetResult::WrongType);
    assert_eq!(set(&mut c, "calib.hour", vs("1e+1")), SetResult::Ok);
    assert_eq!(set(&mut c, "calib.hour", vs("1E1")), SetResult::Ok);
    for text in [
        "1e/", "1./", "1:", "/1", ":1", "e5", ".5e1", "-e5", "1.0:", "1.0/", "1e1:", "1e1/",
    ] {
        assert_eq!(
            set(&mut c, "calib.hour", vs(text)),
            SetResult::WrongType,
            "{text}"
        );
    }
    assert_eq!(
        set(&mut c, "mqtt.port", vf(4294967295.0)),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "mqtt.port", vf(4294967296.0)),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "mqtt.port", vf(-2147483648.0)),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "mqtt.port", vf(-2147483649.0)),
        SetResult::OutOfRange
    );
    // 41+ char number text -> infinite -> out of range, not a crash.
    let long_num = "9".repeat(50);
    assert_eq!(
        set(&mut c, "mqtt.port", vs(&long_num)),
        SetResult::OutOfRange
    );
    // Exactly 40 chars are converted, 41 count as infinite.
    let tiny40 = format!("0.{}1", "0".repeat(37));
    assert_eq!(tiny40.len(), 40);
    assert_eq!(set(&mut c, "volts.1.offset", vs(&tiny40)), SetResult::Ok);
    assert_eq!(c.volts[0].offset, 1e-38f32);
    let tiny41 = format!("0.{}1", "0".repeat(38));
    assert_eq!(
        set(&mut c, "volts.1.offset", vs(&tiny41)),
        SetResult::OutOfRange
    );
    let neg41 = format!("-{}", "1".repeat(40));
    assert_eq!(
        set(&mut c, "volts.1.offset", vs(&neg41)),
        SetResult::OutOfRange
    );
    // C++ String values with a null pointer (mqtt.port, mqtt.separate, station, net.ip,
    // temps.1.id, temps.1.offset): no Rust form.
    assert_eq!(c.mqtt.port, 1883);
}

#[test]
fn every_bool_key_and_its_conversions() {
    let keys = [
        "net.dhcp",
        "mqtt.separate",
        "mqtt.allTemps",
        "mqtt.pathAsRoot",
        "mqtt.upTime",
        "mqtt.onChange",
        "mqtt.retained",
        "mqtt.plainText",
        "mqtt.diag",
        "mqtt.germanDecimal",
        "mqtt.newDiag",
        "mqtt.events",
        "mqtt.haDiscoveryOnConnect",
        "valves.5.active",
        "temps.7.active",
        "volts.3.active",
        "persistLog",
    ];
    for k in keys {
        let mut c = Config::default();
        assert_eq!(set(&mut c, k, vb(true)), SetResult::Ok, "{k}");
        let t = c.clone();
        assert_eq!(set(&mut c, k, vb(false)), SetResult::Ok, "{k}");
        let f = c.clone();
        assert!(!same_config(&t, &f), "{k}");
        assert_eq!(set(&mut c, k, vi(1)), SetResult::Ok, "{k}");
        assert!(same_config(&c, &t), "{k}");
        assert_eq!(set(&mut c, k, vi(0)), SetResult::Ok, "{k}");
        assert!(same_config(&c, &f), "{k}");
        assert_eq!(set(&mut c, k, vs("true")), SetResult::Ok, "{k}");
        assert!(same_config(&c, &t), "{k}");
        assert_eq!(set(&mut c, k, vs("false")), SetResult::Ok, "{k}");
        assert!(same_config(&c, &f), "{k}");
        assert_eq!(set(&mut c, k, vs("1")), SetResult::Ok, "{k}");
        assert!(same_config(&c, &t), "{k}");
        assert_eq!(set(&mut c, k, vs("0")), SetResult::Ok, "{k}");
        assert!(same_config(&c, &f), "{k}");
        assert_eq!(set(&mut c, k, vf(1.0)), SetResult::Ok, "{k}");
        assert!(same_config(&c, &t), "{k}");
        assert_eq!(set(&mut c, k, vi(2)), SetResult::OutOfRange, "{k}");
        assert_eq!(set(&mut c, k, vi(-1)), SetResult::OutOfRange, "{k}");
        assert_eq!(set(&mut c, k, vf(0.5)), SetResult::WrongType, "{k}");
        assert_eq!(set(&mut c, k, vs("yes")), SetResult::WrongType, "{k}");
        assert_eq!(set(&mut c, k, vs("True")), SetResult::WrongType, "{k}");
        assert_eq!(set(&mut c, k, vs("truex")), SetResult::WrongType, "{k}");
        assert_eq!(set(&mut c, k, vs("trux")), SetResult::WrongType, "{k}");
        assert_eq!(set(&mut c, k, vsl(&b"truex"[..4])), SetResult::Ok, "{k}"); // only len bytes
        assert_eq!(set(&mut c, k, vsl(&b"falsex"[..5])), SetResult::Ok, "{k}");
        assert!(same_config(&c, &f), "{k}");
        assert_eq!(set(&mut c, k, vb(true)), SetResult::Ok, "{k}");
        for text in ["falsx", "fals", "11", "00", "2", ""] {
            assert_eq!(set(&mut c, k, vs(text)), SetResult::WrongType, "{k} {text}");
        }
        assert_eq!(set(&mut c, k, vn()), SetResult::WrongType, "{k}");
        assert!(same_config(&c, &t), "{k}");
    }
}

// ---------------------------------------------------------------- setter: strings

#[test]
fn station_name() {
    let mut c = Config::default();
    assert_eq!(set(&mut c, "station", vs("")), SetResult::OutOfRange);
    assert_eq!(set(&mut c, "station", vs("a")), SetResult::Ok);
    assert_text(&c.station, "a");
    assert_eq!(
        set(&mut c, "station", vs("12345678901234567890")),
        SetResult::Ok
    );
    assert_text(&c.station, "12345678901234567890");
    assert_eq!(
        set(&mut c, "station", vs("123456789012345678901")),
        SetResult::OutOfRange
    );
    for bad in [
        &b"a/b"[..],
        b"a+b",
        b"a#b",
        b"a\"b",
        b"a\\b",
        b"a\tb",
        b"a\x7f",
        b"a\xc2\x85", // C1 control
        b"a\xc3",     // truncated UTF-8
        b"a\xe4",     // Latin-1, not UTF-8
        b"ab\0c",
    ] {
        assert_eq!(
            set(&mut c, "station", vsl(bad)),
            SetResult::OutOfRange,
            "{bad:?}"
        );
    }
    assert_eq!(set(&mut c, "station", vi(5)), SetResult::WrongType);
    assert_eq!(set(&mut c, "station", vb(true)), SetResult::WrongType);
    assert_text(&c.station, "12345678901234567890");
    // Legacy names are kept as they are: UTF-8 and spaces at either end.
    assert_eq!(set(&mut c, "station", vs("Fu\u{df}boden")), SetResult::Ok);
    assert_text(&c.station, "Fu\u{df}boden");
    assert_eq!(set(&mut c, "station", vs(" ab")), SetResult::Ok);
    assert_eq!(set(&mut c, "station", vs("ab ")), SetResult::Ok);
    assert_text(&c.station, "ab ");
    // 20 bytes: 10 two-byte characters fit, the 21st byte does not.
    assert_eq!(
        set(&mut c, "station", vs(&"\u{e4}".repeat(10))),
        SetResult::Ok
    );
    let over = format!("x{}", "\u{e4}".repeat(10));
    assert_eq!(set(&mut c, "station", vs(&over)), SetResult::OutOfRange);
    assert_eq!(
        set(&mut c, "station", vs("12345678901234567890")),
        SetResult::Ok
    );
    // Only `len` bytes are used.
    assert_eq!(set(&mut c, "station", vsl(&b"abcdef"[..3])), SetResult::Ok);
    assert_text(&c.station, "abc");
    assert_eq!(set(&mut c, "station", vs("a b")), SetResult::Ok);
}

#[test]
fn item_names_and_units() {
    let mut c = Config::default();
    assert_eq!(set(&mut c, "valves.1.name", vs("")), SetResult::Ok);
    assert_eq!(
        set(&mut c, "valves.1.name", vs("1234567890")),
        SetResult::Ok
    );
    assert_eq!(
        set(&mut c, "valves.1.name", vs("12345678901")),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "temps.1.name", vs("1234567890")), SetResult::Ok);
    assert_eq!(
        set(&mut c, "temps.1.name", vs("12345678901")),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "volts.1.name", vs("1234567890")), SetResult::Ok);
    assert_eq!(
        set(&mut c, "volts.1.name", vs("12345678901")),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "volts.1.name", vs("a/b")),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "volts.1.unit", vs("12345678")), SetResult::Ok);
    assert_text(&c.volts[0].unit, "12345678");
    assert_eq!(
        set(&mut c, "volts.1.unit", vs("123456789")),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "volts.1.unit", vs("")), SetResult::Ok);
    assert_eq!(set(&mut c, "volts.1.unit", vs("m#")), SetResult::OutOfRange);
    assert_eq!(set(&mut c, "valves.2.name", vs(" x")), SetResult::Ok); // legacy: segment "_x"
    assert_eq!(
        set(&mut c, "valves.2.name", vs("K\u{fc}che")),
        SetResult::Ok
    );
    assert_text(&c.valves[1].name, "K\u{fc}che");
    assert_eq!(set(&mut c, "volts.1.unit", vs("\u{b0}C")), SetResult::Ok);
    assert_eq!(
        set(&mut c, "volts.1.unit", vsl(b"\xb0C")),
        SetResult::OutOfRange
    );
}

#[test]
fn network_strings_hosts_and_time_zone() {
    let mut c = Config::default();
    let ssid32 = "s".repeat(32);
    assert_eq!(set(&mut c, "net.ssid", vs(&ssid32)), SetResult::Ok);
    assert_eq!(
        set(&mut c, "net.ssid", vs(&format!("{ssid32}s"))),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "net.ssid", vs("with space")), SetResult::Ok);
    assert_eq!(set(&mut c, "net.ssid", vs("tab\t")), SetResult::OutOfRange);
    assert_eq!(
        set(&mut c, "net.ssid", vs("del\x7f")),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "net.ssid", vs("~!")), SetResult::Ok);
    assert_eq!(set(&mut c, "net.ssid", vs("")), SetResult::Ok);

    let h64 = "h".repeat(64);
    assert_eq!(set(&mut c, "mqtt.host", vs(&h64)), SetResult::Ok);
    assert_eq!(
        set(&mut c, "mqtt.host", vs(&format!("{h64}h"))),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "mqtt.host", vs("broker.local")), SetResult::Ok);
    assert_eq!(set(&mut c, "mqtt.host", vs("192.168.1.2")), SetResult::Ok);
    assert_eq!(
        set(&mut c, "mqtt.host", vs("192.168.1.300")),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "mqtt.host", vs("192.168.1")),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "mqtt.host", vs("1")), SetResult::OutOfRange);
    assert_eq!(set(&mut c, "mqtt.host", vs("1host")), SetResult::Ok);
    assert_eq!(set(&mut c, "mqtt.host", vs("-host")), SetResult::OutOfRange);
    assert_eq!(set(&mut c, "mqtt.host", vs("host.")), SetResult::OutOfRange);
    assert_eq!(set(&mut c, "mqtt.host", vs("ho st")), SetResult::OutOfRange);
    assert_eq!(set(&mut c, "mqtt.host", vs("ho_st")), SetResult::OutOfRange);
    assert_eq!(set(&mut c, "mqtt.host", vs("")), SetResult::Ok);
    assert!(c.mqtt.host.is_empty());
    assert_eq!(set(&mut c, "time.ntpServer", vs("")), SetResult::Ok);
    assert_eq!(
        set(&mut c, "time.ntpServer", vs("de.pool.ntp.org")),
        SetResult::Ok
    );
    assert_eq!(
        set(&mut c, "time.ntpServer", vs("pool ntp")),
        SetResult::OutOfRange
    );

    let tz49 = "z".repeat(49);
    assert_eq!(set(&mut c, "time.tzName", vs("")), SetResult::Ok);
    assert_eq!(
        set(&mut c, "time.tzName", vs("America/Argentina/Buenos Aires")),
        SetResult::Ok
    );
    assert_eq!(set(&mut c, "time.tzName", vs(&tz49)), SetResult::Ok);
    assert_eq!(
        set(&mut c, "time.tzName", vs(&format!("{tz49}z"))),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "time.tzName", vs("x\n")), SetResult::OutOfRange);
    assert_eq!(set(&mut c, "time.tzPosix", vs("")), SetResult::OutOfRange);
    assert_eq!(set(&mut c, "time.tzPosix", vs("U")), SetResult::Ok);
    assert_eq!(set(&mut c, "time.tzPosix", vs(&tz49)), SetResult::Ok);
    assert_eq!(
        set(&mut c, "time.tzPosix", vs(&format!("{tz49}z"))),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "time.tzPosix", vs("CET -1")),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "time.tzPosix", vs("<+0330>-3:30")),
        SetResult::Ok
    );

    let u64s = "u".repeat(64);
    assert_eq!(set(&mut c, "mqtt.user", vs(&u64s)), SetResult::Ok);
    assert_eq!(
        set(&mut c, "mqtt.user", vs(&format!("{u64s}u"))),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "mqtt.user", vs("a:b")), SetResult::Ok);
    assert_eq!(set(&mut c, "mqtt.user", vs("\x01")), SetResult::OutOfRange);
}

#[test]
fn secrets_are_write_only_and_cleared_only_on_request() {
    let mut c = Config::default();
    assert_eq!(set(&mut c, "mqtt.password", vs("pw1")), SetResult::Ok);
    assert_text(&c.mqtt.password, "pw1");
    assert_eq!(set(&mut c, "mqtt.password", vs("")), SetResult::Ok);
    assert_text(&c.mqtt.password, "pw1");
    assert_eq!(set_clear(&mut c, "mqtt.password", vs("")), SetResult::Ok);
    assert!(c.mqtt.password.is_empty());

    let p63 = "p".repeat(63);
    assert_eq!(set(&mut c, "net.wifiPassword", vs(&p63)), SetResult::Ok);
    assert_eq!(
        set(&mut c, "net.wifiPassword", vs(&format!("{p63}p"))),
        SetResult::OutOfRange
    );
    assert_text(&c.net.wifi_password, &p63);
    assert_eq!(set_clear(&mut c, "net.wifiPassword", vs("")), SetResult::Ok);
    assert!(c.net.wifi_password.is_empty());

    let p64 = "p".repeat(64);
    assert_eq!(set(&mut c, "mqtt.password", vs(&p64)), SetResult::Ok);
    assert_eq!(
        set(&mut c, "mqtt.password", vs(&format!("{p64}p"))),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "mqtt.password", vs("\x7f")),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "mqtt.password", vs("a:b \"c\"")), SetResult::Ok);
    assert_eq!(set(&mut c, "mqtt.password", vi(3)), SetResult::WrongType);

    // Export flags are accepted as no-ops (bool only).
    let before = c.clone();
    assert_eq!(set(&mut c, "net.wifiPasswordSet", vb(true)), SetResult::Ok);
    assert_eq!(set(&mut c, "mqtt.passwordSet", vb(true)), SetResult::Ok);
    assert!(same_config(&c, &before));
    assert_eq!(set(&mut c, "mqtt.passwordSet", vi(1)), SetResult::WrongType);
    for p in [
        "mqtt.passwordSe",
        "mqtt.passwordSett",
        "mqtt.hostSet",
        "mqtt.passwordXet",
        "mqtt.passwordSex",
        "mqtt.passwordSxt",
    ] {
        assert_eq!(set(&mut c, p, vb(true)), SetResult::UnknownKey, "{p}");
    }
    // Neighbouring fields survive a maximum-length write.
    let mut n = Config::default();
    assert_eq!(
        set(&mut n, "net.wifiPassword", vs("12345678")),
        SetResult::Ok
    );
    assert_eq!(set(&mut n, "net.ssid", vs(&"s".repeat(32))), SetResult::Ok);
    assert_text(&n.net.wifi_password, "12345678");
    assert_eq!(set(&mut n, "mqtt.user", vs(&"u".repeat(64))), SetResult::Ok);
    assert_eq!(
        set(&mut n, "mqtt.password", vs(&"p".repeat(64))),
        SetResult::Ok
    );
    assert_text(&n.mqtt.user, "u".repeat(64));
    assert_eq!(n.mqtt.keep_alive_s, 60);
    // A one-char secret counts as set in the export.
    let mut o = Config::default();
    assert_eq!(set(&mut o, "mqtt.password", vs("x")), SetResult::Ok);
    assert!(export_json(&o).contains("\"passwordSet\":true,\"keepAliveS\""));
}

// ---------------------------------------------------------------- setter: addresses and numbers

#[test]
fn ipv4_keys() {
    let mut c = Config::default();
    assert_eq!(set(&mut c, "net.ip", vs("192.168.1.2")), SetResult::Ok);
    assert_eq!(c.net.ip, 0x0201_A8C0);
    assert_eq!(
        set(&mut c, "net.gateway", vs("255.255.255.255")),
        SetResult::Ok
    );
    assert_eq!(c.net.gateway, 0xFFFF_FFFF);
    assert_eq!(set(&mut c, "net.dns", vs("1.2.3.4")), SetResult::Ok);
    assert_eq!(c.net.dns, 0x0403_0201);
    assert_eq!(set(&mut c, "syslog.server", vs("10.0.0.1")), SetResult::Ok);
    assert_eq!(c.syslog.server, 0x0100_000A);
    assert_eq!(set(&mut c, "net.ip", vs("")), SetResult::Ok);
    assert_eq!(c.net.ip, 0);
    assert_eq!(set(&mut c, "net.ip", vs("1.2.3")), SetResult::OutOfRange);
    assert_eq!(
        set(&mut c, "net.ip", vs("1.2.3.256")),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "net.ip", vs("host")), SetResult::OutOfRange);
    assert_eq!(set(&mut c, "net.ip", vi(5)), SetResult::WrongType);
    assert_eq!(c.net.ip, 0);

    for m in [
        "0.0.0.0",
        "128.0.0.0",
        "255.0.0.0",
        "255.255.0.0",
        "255.255.255.0",
        "255.255.255.128",
        "255.255.255.254",
        "255.255.255.255",
        "255.255.240.0",
    ] {
        assert_eq!(set(&mut c, "net.mask", vs(m)), SetResult::Ok, "{m}");
    }
    assert_eq!(c.net.mask, 0x00F0_FFFF);
    for m in [
        "0.0.0.255",
        "255.0.255.0",
        "255.255.255.253",
        "254.255.255.0",
        "0.255.255.255",
        "1.0.0.0",
    ] {
        assert_eq!(set(&mut c, "net.mask", vs(m)), SetResult::OutOfRange, "{m}");
    }
    assert_eq!(c.net.mask, 0x00F0_FFFF);
}

#[test]
fn temperature_offset_in_tenths_rounded_half_away_from_zero() {
    let mut c = Config::default();
    let cases: [(f64, i16); 16] = [
        (0.0, 0),
        (0.04, 0),
        (0.05, 1),
        (0.15, 2),
        (-0.15, -2),
        (0.7, 7),
        (-0.7, -7),
        (0.24, 2),
        (0.25, 3),
        (-0.25, -3),
        (9.94, 99),
        (10.0, 100),
        (10.04, 100),
        (-10.0, -100),
        (-10.04, -100),
        (-0.04, 0),
    ];
    for (input, out) in cases {
        assert_eq!(
            set(&mut c, "temps.2.offset", vf(input)),
            SetResult::Ok,
            "{input}"
        );
        assert_eq!(c.temps[1].offset, out, "{input}");
    }
    assert_eq!(set(&mut c, "temps.2.offset", vi(3)), SetResult::Ok);
    assert_eq!(c.temps[1].offset, 30);
    assert_eq!(set(&mut c, "temps.2.offset", vs("-2.5")), SetResult::Ok);
    assert_eq!(c.temps[1].offset, -25);
    for v in [
        10.05,
        -10.05,
        3000.0,
        3000.1,
        1e300,
        f64::NAN,
        f64::NEG_INFINITY,
    ] {
        assert_eq!(
            set(&mut c, "temps.2.offset", vf(v)),
            SetResult::OutOfRange,
            "{v}"
        );
    }
    assert_eq!(set(&mut c, "temps.2.offset", vi(11)), SetResult::OutOfRange);
    assert_eq!(
        set(&mut c, "temps.2.offset", vi(-11)),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "temps.2.offset", vs("x")), SetResult::WrongType);
    assert_eq!(
        set(&mut c, "temps.2.offset", vb(true)),
        SetResult::WrongType
    );
    assert_eq!(set(&mut c, "temps.2.offset", vn()), SetResult::WrongType);
    assert_eq!(c.temps[1].offset, -25);
}

#[test]
fn volt_offset_and_factor() {
    let mut c = Config::default();
    assert_eq!(set(&mut c, "volts.1.offset", vf(1000.0)), SetResult::Ok);
    assert_eq!(c.volts[0].offset, 1000.0);
    assert_eq!(set(&mut c, "volts.1.offset", vf(-1000.0)), SetResult::Ok);
    assert_eq!(
        set(&mut c, "volts.1.offset", vf(1000.001)),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "volts.1.offset", vf(-1000.001)),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "volts.1.offset", vf(0.0)), SetResult::Ok);
    assert_eq!(set(&mut c, "volts.1.offset", vi(-3)), SetResult::Ok);
    assert_eq!(c.volts[0].offset, -3.0);
    assert_eq!(set(&mut c, "volts.1.offset", vs("0.125")), SetResult::Ok);
    assert_eq!(c.volts[0].offset, 0.125);
    assert_eq!(
        set(&mut c, "volts.1.offset", vf(f64::NAN)),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "volts.1.offset", vf(f64::INFINITY)),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "volts.1.offset", vb(false)),
        SetResult::WrongType
    );
    assert_eq!(c.volts[0].offset, 0.125);

    assert_eq!(
        set(&mut c, "volts.1.factor", vf(0.0)),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "volts.1.factor", vf(1e-50)),
        SetResult::OutOfRange
    ); // 0.0f
    assert_eq!(set(&mut c, "volts.1.factor", vi(0)), SetResult::OutOfRange);
    assert_eq!(set(&mut c, "volts.1.factor", vf(-0.001)), SetResult::Ok);
    assert_eq!(c.volts[0].factor, -0.001f32);
    assert_eq!(set(&mut c, "volts.1.factor", vf(1000.0)), SetResult::Ok);
    assert_eq!(set(&mut c, "volts.1.factor", vf(-1000.0)), SetResult::Ok);
    assert_eq!(
        set(&mut c, "volts.1.factor", vf(1001.0)),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "volts.1.factor", vf(1000.001)),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "volts.1.factor", vf(-1000.001)),
        SetResult::OutOfRange
    );
    assert_eq!(c.volts[0].factor, -1000.0);
}

#[test]
fn one_wire_id_keys() {
    let mut c = Config::default();
    assert_eq!(
        set(&mut c, "temps.3.id", vs("28-84-37-94-97-FF-03-23")),
        SetResult::Ok
    );
    assert_eq!(c.temps[2].id, oid(ID_A));
    assert_eq!(set(&mut c, "volts.3.id", vs(ID_V)), SetResult::Ok);
    assert_eq!(c.volts[2].id, oid(ID_V));
    for bad in [
        "28-84-37-94-97-ff-03",
        "28-84-37-94-97-ff-03-2g",
        "28:84:37:94:97:ff:03:23",
    ] {
        assert_eq!(
            set(&mut c, "temps.3.id", vs(bad)),
            SetResult::OutOfRange,
            "{bad}"
        );
    }
    assert_eq!(set(&mut c, "temps.3.id", vi(1)), SetResult::WrongType);
    assert_eq!(c.temps[2].id, oid(ID_A));
    assert_eq!(
        set(&mut c, "temps.3.id", vs("00-00-00-00-00-00-00-00")),
        SetResult::Ok
    );
    assert!(is_zero(&c.temps[2].id));
    assert_eq!(set(&mut c, "temps.3.id", vs(ID_B)), SetResult::Ok);
    assert_eq!(set(&mut c, "temps.3.id", vs("")), SetResult::Ok);
    assert!(is_zero(&c.temps[2].id));
}

// ---------------------------------------------------------------- validation

#[test]
fn validation_reports_every_cross_field_rule_with_its_path() {
    let mut c = Config::default();
    c.net.dhcp = false;
    assert_eq!(validate_path(&c), "net.ip");
    c.net.ip = 1;
    assert_eq!(validate_path(&c), "net.mask");
    c.net.mask = 0x00FF_FFFF;
    assert_eq!(validate_path(&c), "net.gateway");
    c.net.gateway = 1;
    assert_eq!(validate_path(&c), "OK");
    c.net.dhcp = true;
    c.net.ip = 0;
    c.net.mask = 0;
    c.net.gateway = 0;
    assert_eq!(validate_path(&c), "OK");

    strcpy(&mut c.net.ssid, "w");
    assert_eq!(validate_path(&c), "OK"); // open network
    strcpy(&mut c.net.wifi_password, "1");
    assert_eq!(validate_path(&c), "net.wifiPassword");
    strcpy(&mut c.net.ssid, "wlan");
    strcpy(&mut c.net.wifi_password, "1234567");
    assert_eq!(validate_path(&c), "net.wifiPassword");
    strcpy(&mut c.net.wifi_password, "12345678");
    assert_eq!(validate_path(&c), "OK");
    c.net.iface = NetInterface::Wifi;
    assert_eq!(validate_path(&c), "OK");
    c.net.ssid.clear();
    assert_eq!(validate_path(&c), "net.ssid");
    c.net.iface = NetInterface::Ethernet;
    assert_eq!(validate_path(&c), "OK"); // password without ssid is harmless

    c.syslog.level = 1;
    assert_eq!(validate_path(&c), "syslog.server");
    c.net.iface = NetInterface::Wifi;
    strcpy(&mut c.net.ssid, "w");
    c.syslog.level = 0;
    assert_eq!(validate_path(&c), "OK"); // one-char ssid counts as set
    c.net.iface = NetInterface::Ethernet;
    c.syslog.level = 1;
    c.syslog.server = 5;
    assert_eq!(validate_path(&c), "OK");
    c.syslog.level = 0;
    c.syslog.server = 0;

    c.mqtt.mode = MqttMode::Mqtt;
    assert_eq!(validate_path(&c), "mqtt.host");
    strcpy(&mut c.mqtt.host, "b");
    assert_eq!(validate_path(&c), "OK");
    c.mqtt.min_delay_s = 11;
    assert_eq!(validate_path(&c), "mqtt.minDelayS");
    c.mqtt.min_delay_s = 10;
    assert_eq!(validate_path(&c), "OK");
    c.mqtt.separate = false;
    assert_eq!(validate_path(&c), "OK");
    c.mqtt.mode = MqttMode::MqttHa;
    assert_eq!(validate_path(&c), "mqtt.mode");
    c.mqtt.separate = true;
    assert_eq!(validate_path(&c), "OK");
    c.mqtt.mode = MqttMode::Off;
    c.mqtt.host.clear();
    assert_eq!(validate_path(&c), "OK");
}

#[test]
fn valve_names_must_give_unique_mqtt_segments() {
    let mut c = Config::default();
    strcpy(&mut c.valves[0].name, "Bad");
    strcpy(&mut c.valves[4].name, "Bad");
    assert_eq!(validate_path(&c), "valves.5.name");
    strcpy(&mut c.valves[4].name, "bad");
    assert_eq!(validate_path(&c), "OK");
    strcpy(&mut c.valves[0].name, "a b");
    strcpy(&mut c.valves[4].name, "a_b");
    assert_eq!(validate_path(&c), "valves.5.name");
    strcpy(&mut c.valves[4].name, "a_c");
    assert_eq!(validate_path(&c), "OK");

    let mut n = Config::default();
    strcpy(&mut n.valves[4].name, "3"); // valve 3 is unnamed -> "3" is its segment
    assert_eq!(validate_path(&n), "valves.5.name");
    strcpy(&mut n.valves[2].name, "x"); // now valve 3 publishes as "x"
    assert_eq!(validate_path(&n), "OK");
    n.valves[2].name.clear();
    strcpy(&mut n.valves[4].name, "5"); // own number
    assert_eq!(validate_path(&n), "OK");
    strcpy(&mut n.valves[4].name, "12");
    assert_eq!(validate_path(&n), "valves.5.name");
    strcpy(&mut n.valves[4].name, "1");
    assert_eq!(validate_path(&n), "valves.5.name");
    strcpy(&mut n.valves[4].name, "123");
    assert_eq!(validate_path(&n), "OK");
    strcpy(&mut n.valves[4].name, "13");
    assert_eq!(validate_path(&n), "OK");
    strcpy(&mut n.valves[4].name, "03");
    assert_eq!(validate_path(&n), "OK");
    strcpy(&mut n.valves[4].name, "0");
    assert_eq!(validate_path(&n), "OK");
    strcpy(&mut n.valves[11].name, "12");
    strcpy(&mut n.valves[4].name, "x");
    assert_eq!(validate_path(&n), "OK");
    strcpy(&mut n.valves[0].name, "12"); // valve 12 is named "12" -> duplicate of it
    assert_eq!(validate_path(&n), "valves.12.name");
}

#[test]
fn uniqueness_checks_cover_every_index_pair() {
    let mut c = Config::default();
    strcpy(&mut c.valves[2].name, "dup");
    strcpy(&mut c.valves[5].name, "dup");
    assert_eq!(validate_path(&c), "valves.6.name");
    let mut d = Config::default();
    strcpy(&mut d.valves[0].name, "5"); // valve 1 named like unnamed valve 5
    assert_eq!(validate_path(&d), "valves.1.name");
    strcpy(&mut d.valves[11].name, "12x");
    assert_eq!(validate_path(&d), "valves.1.name");
    let mut m = Config::default(); // item names of other tables never clash with valve names
    strcpy(&mut m.valves[0].name, "dup");
    strcpy(&mut m.temps[0].name, "dup");
    strcpy(&mut m.volts[0].name, "dup");
    assert_eq!(validate_path(&m), "OK");
    let mut z = Config::default();
    z.temps[0].id = oid(ID_A);
    z.temps[5].id = oid(ID_A);
    assert_eq!(validate_path(&z), "temps.6.id");
    z.temps[5].id = oid(ID_B);
    z.volts[0].id = oid(ID_V);
    z.volts[4].id = oid(ID_V);
    assert_eq!(validate_path(&z), "volts.5.id");
    let mut t = Config::default();
    t.temps[0].active = true;
    assert_eq!(validate_path(&t), "temps.1.active");
    t.temps[0].active = false;
    t.temps[2].id = oid(ID_A);
    t.temps[33].id = oid(ID_A);
    assert_eq!(validate_path(&t), "temps.34.id");
    let mut v = Config::default();
    v.volts[0].active = true;
    assert_eq!(validate_path(&v), "volts.1.active");
    v.volts[0].active = false;
    v.volts[3].id = oid(ID_V);
    v.volts[7].id = oid(ID_V);
    assert_eq!(validate_path(&v), "volts.8.id");
}

#[test]
fn sensor_slots_need_ids_and_unique_ids() {
    let mut c = Config::default();
    c.temps[3].active = true;
    assert_eq!(validate_path(&c), "temps.4.active");
    c.temps[3].id = oid(ID_A);
    assert_eq!(validate_path(&c), "OK");
    c.temps[9].id = oid(ID_A);
    assert_eq!(validate_path(&c), "temps.10.id");
    c.temps[9].id = oid(ID_B);
    assert_eq!(validate_path(&c), "OK");
    c.volts[0].id = oid(ID_A); // the same id in the volt table is fine
    assert_eq!(validate_path(&c), "OK");
    c.volts[7].active = true;
    assert_eq!(validate_path(&c), "volts.8.active");
    c.volts[7].id = oid(ID_A);
    assert_eq!(validate_path(&c), "volts.8.id");
    c.volts[7].id = oid(ID_V);
    assert_eq!(validate_path(&c), "OK");
}

#[test]
fn validation_rejects_out_of_range_stored_fields() {
    let mut c = Config {
        schema: 3,
        ..Config::default()
    };
    assert_eq!(validate_path(&c), "schema");
    c.schema = 1; // the JSON schema of a config in RAM is always the current one
    assert_eq!(validate_path(&c), "schema");
    let cases: [(Edit, &str); 17] = [
        (|c| c.station.clear(), "station"),
        // C++ station and valve name without a NUL in their arrays, net.iface 3, a net.dhcp byte
        // 2, mqtt.mode 3 and a persistLog byte 7: no Rust form (the decoder case
        // `decode_repairs_an_enum_byte_out_of_range` covers the enums).
        (|c| c.net.mask = 0x00FF_00FF, "net.mask"),
        (
            |c| c.net.reconnect_timeout_min = 241,
            "net.reconnectTimeoutMin",
        ),
        (|c| strcpy(&mut c.time.tz_posix, "a b"), "time.tzPosix"),
        (
            |c| strcpy(&mut c.time.ntp_server, "300.1.1.1"),
            "time.ntpServer",
        ),
        (|c| c.syslog.port = 0, "syslog.port"),
        (|c| c.mqtt.keep_alive_s = 4, "mqtt.keepAliveS"),
        (|c| c.mqtt.keep_alive_s = 301, "mqtt.keepAliveS"),
        (|c| c.mqtt.publish_interval_s = 1, "mqtt.publishIntervalS"),
        (|c| c.temps[33].offset = 101, "temps.34.offset"),
        (|c| c.temps[33].offset = -101, "temps.34.offset"),
        (|c| c.volts[1].factor = 0.0, "volts.2.factor"),
        (|c| c.volts[1].offset = f32::NAN, "volts.2.offset"),
        (|c| c.volts[1].offset = 1000.5, "volts.2.offset"),
        (|c| c.calib.day_mask = 128, "calib.dayMask"),
        (|c| c.calib.hour = 24, "calib.hour"),
        (|c| c.calib.minute = 60, "calib.minute"),
    ];
    for (edit, path) in cases {
        let mut c = Config::default();
        edit(&mut c);
        assert_eq!(validate_path(&c), path);
    }
}

#[test]
fn validation_accepts_every_stored_field_at_both_range_ends() {
    let mut lo = Config::default();
    let mut hi = Config::default();
    lo.mqtt.port = 1;
    lo.mqtt.keep_alive_s = 5;
    lo.mqtt.publish_interval_s = 2;
    lo.mqtt.min_delay_s = 0;
    lo.syslog.port = 1;
    lo.temps[0].offset = -100;
    lo.volts[0].offset = -1000.0;
    lo.volts[0].factor = -1000.0;
    lo.calib.day_mask = 0;
    lo.net.reconnect_timeout_min = 0;
    strcpy(&mut lo.time.tz_posix, "U");
    strcpy(&mut lo.station, "a");
    assert_eq!(validate_path(&lo), "OK");
    hi.mqtt.port = 65535;
    hi.mqtt.keep_alive_s = 300;
    hi.mqtt.publish_interval_s = 3600;
    hi.mqtt.min_delay_s = 3600;
    hi.syslog.port = 65535;
    hi.syslog.level = 3;
    hi.syslog.server = 1;
    hi.net.iface = NetInterface::Wifi;
    strcpy(&mut hi.net.ssid, "12345678901234567890123456789012");
    strcpy(&mut hi.net.wifi_password, &"p".repeat(63));
    hi.mqtt.mode = MqttMode::MqttHa;
    strcpy(&mut hi.mqtt.host, "h");
    hi.temps[0].offset = 100;
    hi.volts[0].offset = 1000.0;
    hi.volts[0].factor = 1000.0;
    hi.calib.day_mask = 127;
    hi.calib.hour = 23;
    hi.calib.minute = 59;
    hi.net.reconnect_timeout_min = 240;
    strcpy(&mut hi.station, &"s".repeat(20));
    strcpy(&mut hi.valves[11].name, &"v".repeat(10));
    strcpy(&mut hi.volts[7].unit, &"u".repeat(8));
    assert_eq!(validate_path(&hi), "OK");
    hi.mqtt.keep_alive_s = 301;
    assert_eq!(validate_path(&hi), "mqtt.keepAliveS");
    lo.mqtt.keep_alive_s = 4;
    assert_eq!(validate_path(&lo), "mqtt.keepAliveS");
    hi.mqtt.keep_alive_s = 300;
    hi.mqtt.publish_interval_s = 3601;
    assert_eq!(validate_path(&hi), "mqtt.publishIntervalS");
}

#[test]
fn validation_path_buffer_handling() {
    let mut c = Config::default();
    c.calib.hour = 24;
    // C++ validateConfig(c, nullptr, 0): the empty slice.
    assert_eq!(validate_config(&c, &mut []), Err(0));
    let mut small = [0u8; 6];
    assert_eq!(validate_config(&c, &mut small), Err(5));
    assert_text(&small[..5], "calib");
    let mut one = *b"x";
    assert_eq!(validate_config(&c, &mut one[..0]), Err(0));
    assert_eq!(one[0], b'x');
    assert_eq!(validate_config(&c, &mut one), Err(0)); // C++: one[0] == '\0'
    let ok = Config::default();
    let mut path = *b"junk\0\0\0\0";
    assert_eq!(validate_config(&ok, &mut path), Ok(())); // C++: path[0] == '\0'
}

// ---------------------------------------------------------------- JSON export

#[test]
fn json_export_of_the_defaults_golden() {
    let mut expected = String::from(
        "{\"schema\":2,\"station\":\"VdMot\",\
\"net\":{\"iface\":0,\"dhcp\":true,\"ip\":\"0.0.0.0\",\"mask\":\"0.0.0.0\",\
\"gateway\":\"0.0.0.0\",\"dns\":\"0.0.0.0\",\"ssid\":\"\",\"wifiPasswordSet\":false,\
\"reconnectTimeoutMin\":5},\
\"time\":{\"ntpServer\":\"pool.ntp.org\",\"tzName\":\"Europe/Berlin\",\
\"tzPosix\":\"CET-1CEST,M3.5.0,M10.5.0/3\"},\
\"syslog\":{\"level\":0,\"server\":\"0.0.0.0\",\"port\":514},\
\"web\":{\"allowedHosts\":\"\"},\
\"mqtt\":{\"mode\":0,\"host\":\"\",\"port\":1883,\"user\":\"\",\"passwordSet\":false,\
\"keepAliveS\":60,\"publishIntervalS\":10,\"minDelayS\":5,\"separate\":true,\
\"allTemps\":true,\"pathAsRoot\":false,\"upTime\":true,\"onChange\":true,\
\"retained\":true,\"plainText\":true,\"diag\":true,\"germanDecimal\":false,\
\"newDiag\":true,\"events\":true,\"haDiscoveryOnConnect\":true,\"rootTopic\":\"\",\
\"clientId\":\"\",\"discoveryPrefix\":\"homeassistant\"},\"valves\":[",
    );
    let valve = "{\"name\":\"\",\"active\":false,\"failsafePct\":50,\"topic\":\"\"}";
    expected.push_str(&[valve; 12].join(","));
    expected.push_str("],\"temps\":[");
    let temp = "{\"name\":\"\",\"active\":false,\"offset\":0.0,\"id\":\"\",\"topic\":\"\"}";
    expected.push_str(&[temp; 34].join(","));
    expected.push_str("],\"volts\":[");
    let volt = "{\"name\":\"\",\"active\":false,\"offset\":0,\"factor\":1,\"unit\":\"\",\
\"id\":\"\",\"topic\":\"\"}";
    expected.push_str(&[volt; 8].join(","));
    expected.push_str(
        "],\"calib\":{\"dayMask\":9,\"hour\":0,\"minute\":0},\"failsafe\":{\"timeoutMin\":60},\
\"persistLog\":true}",
    );
    assert_eq!(export_json(&Config::default()), expected);
}

#[test]
fn json_export_of_set_values() {
    let c = full_config();
    let j = export_json(&c);
    assert!(j.contains("\"station\":\"Heizung OG\""));
    assert!(j.contains(
        "\"iface\":2,\"dhcp\":false,\"ip\":\"192.168.1.50\",\"mask\":\"255.255.255.0\",\
\"gateway\":\"192.168.1.1\",\"dns\":\"8.8.8.8\",\"ssid\":\"My Wifi\",\
\"wifiPasswordSet\":true,\"reconnectTimeoutMin\":17"
    ));
    assert!(j.contains("\"web\":{\"allowedHosts\":\"\"}"));
    assert!(j.contains(
        "\"mode\":1,\"host\":\"broker.lan\",\"port\":8883,\"user\":\"mq\",\
\"passwordSet\":true,\"keepAliveS\":30,\"publishIntervalS\":120,\"minDelayS\":7,\
\"separate\":false,\"allTemps\":false,\"pathAsRoot\":true,\"upTime\":false,\
\"onChange\":false,\"retained\":false,\"plainText\":false,\"diag\":false,\
\"germanDecimal\":true,\"newDiag\":false,\"events\":false,\
\"haDiscoveryOnConnect\":false,"
    ));
    assert!(j.contains(
        "{\"name\":\"t1\",\"active\":true,\"offset\":-1.5,\
\"id\":\"28-84-37-94-97-ff-03-23\",\"topic\":\"\"}"
    ));
    assert!(j.contains(
        "{\"name\":\"\",\"active\":false,\"offset\":10.0,\
\"id\":\"28-aa-bb-cc-dd-ee-01-67\",\"topic\":\"\"}]"
    ));
    assert!(j.contains(
        "{\"name\":\"bat\",\"active\":true,\"offset\":-0.25,\"factor\":0.01,\"unit\":\"V\",\
\"id\":\"26-11-22-33-44-55-66-29\",\"topic\":\"\"}]"
    ));
    assert!(j.contains(
        "\"calib\":{\"dayMask\":127,\"hour\":23,\"minute\":59},\
\"failsafe\":{\"timeoutMin\":60},\"persistLog\":false}"
    ));
    // The keys of the cfgx blob.
    let x = export_json(&full_config_ext());
    assert!(x.contains("\"web\":{\"allowedHosts\":\"vdmot.lan, 192.168.1.9\"}"));
    assert!(x.contains(
        "\"haDiscoveryOnConnect\":false,\"rootTopic\":\"VdMotFBH\",\
\"clientId\":\"VdMot-east-6c1e51\",\"discoveryPrefix\":\"ha/discovery\"}"
    ));
    assert!(x.contains(
        "\"valves\":[{\"name\":\"Bad\",\"active\":true,\"failsafePct\":255,\"topic\":\"\"},\
{\"name\":\"\",\"active\":false,\"failsafePct\":50,\"topic\":\"Bad/WC\"},"
    ));
    assert!(
        x.contains("{\"name\":\"Kitchen\",\"active\":false,\"failsafePct\":0,\"topic\":\"x\"}]")
    );
    assert!(x.contains("\"id\":\"28-84-37-94-97-ff-03-23\",\"topic\":\"t/1\"}"));
    assert!(x.contains("\"id\":\"26-11-22-33-44-55-66-29\",\"topic\":\"battery\"}]"));
    assert!(x.contains("\"failsafe\":{\"timeoutMin\":1440}"));
    // Secrets never appear.
    assert!(!j.contains("secret123"));
    assert!(!j.contains("mqpw"));
}

#[test]
fn json_float_formatting_is_the_shortest_exact_form() {
    let mut c = Config::default();
    let cases: [(f32, &str); 9] = [
        (1.0, "1"),
        (-0.0, "0"),
        (0.1, "0.1"),
        (-2.5, "-2.5"),
        (0.001, "0.001"),
        (1000.0, "1000"),
        (123.456, "123.456"),
        (1e-7, "0.000000"),
        (0.333333, "0.333333"),
    ];
    for (v, text) in cases {
        c.volts[0].offset = v;
        let j = export_json(&c);
        assert!(
            j.contains(&format!("\"offset\":{text},\"factor\"")),
            "{text}"
        );
    }
    c.volts[0].offset = f32::NAN;
    assert!(export_json(&c).contains("\"offset\":null,\"factor\""));
    c.volts[0].offset = 1e30; // invalid, but must still export something bounded
    assert!(export_json(&c).contains("\"offset\":1000000015047466219876688855040,\"factor\""));
}

#[test]
fn json_export_of_maximum_length_strings() {
    let mut c = Config::default();
    strcpy(&mut c.station, &"s".repeat(20));
    strcpy(&mut c.valves[0].name, "1234567890");
    strcpy(&mut c.volts[0].unit, "12345678");
    let j = export_json(&c);
    assert!(j.contains(&format!("\"station\":\"{}\"", "s".repeat(20))));
    assert!(j.contains("\"name\":\"1234567890\""));
    assert!(j.contains("\"unit\":\"12345678\""));
    // C++ the same with a station and a valve name without a NUL in their arrays (cut at 20 and
    // 10): no Rust form, a text member holds at most its capacity.
}

#[test]
fn json_export_fails_cleanly_on_a_small_buffer() {
    let full = export_json(&full_config());
    let c = full_config();
    let mut buf = vec![0u8; full.len() + 1];
    for cap in [0, 1, 50, full.len() / 2, full.len()] {
        let mut jw = JsonWriter::new(&mut buf[..cap]);
        assert!(!write_config_json(&mut jw, &c, None), "{cap}");
    }
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write_config_json(&mut jw, &c, None));
    assert_text(jw.as_bytes(), &full);
}

// ---------------------------------------------------------------- JSON patch

#[test]
fn patch_round_trip_of_an_export() {
    let full = full_config();
    let j = export_json(&full);
    // Secrets are not exported: the import keeps the ones of its target (an ssid without
    // password is an open network).
    let mut c = Config::default();
    assert_eq!(patch(&mut c, &j), (PatchResult::Ok, String::new()));
    let mut no_secrets = full.clone();
    no_secrets.net.wifi_password.clear();
    no_secrets.mqtt.password.clear();
    assert!(same_config(&c, &no_secrets));

    let mut d = full.clone(); // same secrets present -> exact round trip
    assert_eq!(patch(&mut d, &j), (PatchResult::Ok, String::new()));
    assert!(same_config(&d, &full));

    let mut e = Config::default();
    let mut with_secrets = j.clone();
    with_secrets.insert_str(
        1,
        "\"net\":{\"wifiPassword\":\"secret123\"},\"mqtt\":{\"password\":\"mqpw\"},",
    );
    assert_eq!(patch(&mut e, &with_secrets).0, PatchResult::Ok);
    assert!(same_config(&e, &full));
}

#[test]
fn patch_paths_nesting_forms_and_clear_secrets() {
    let mut c = Config::default();
    assert_eq!(patch(&mut c, "{}").0, PatchResult::Ok);
    assert!(same_config(&c, &Config::default()));
    assert_eq!(
        patch(&mut c, " \t\r\n{ \"calib\" : { \"hour\" : 5 } } \n").0,
        PatchResult::Ok
    );
    assert_eq!(c.calib.hour, 5);
    assert_eq!(
        patch(&mut c, "{\"valves\":{\"3\":{\"name\":\"x\"}}}").0,
        PatchResult::Ok
    );
    assert_text(&c.valves[2].name, "x");
    assert_eq!(
        patch(&mut c, "{\"valves.4.name\":\"y\"}").0,
        PatchResult::Ok
    );
    assert_text(&c.valves[3].name, "y");
    assert_eq!(
        patch(
            &mut c,
            "{\"valves\":[{},{\"active\":true},{\"name\":\"\"}]}"
        )
        .0,
        PatchResult::Ok
    );
    assert!(c.valves[1].active);
    assert!(c.valves[2].name.is_empty());
    assert_eq!(
        patch(&mut c, "{\"temps\":[{\"offset\":\"1.25\"}]}").0,
        PatchResult::Ok
    );
    assert_eq!(c.temps[0].offset, 13);
    // Array elements beyond the table and scalar arrays.
    let too_many = format!(
        "{{\"valves\":[{}]}}",
        vec!["{\"active\":false}"; 13].join(",")
    );
    assert_eq!(
        patch(&mut c, &too_many),
        (PatchResult::UnknownKey, "valves.13.active".to_string())
    );
    assert_eq!(
        patch(&mut c, "{\"valves\":[1]}"),
        (PatchResult::UnknownKey, "valves.1".to_string())
    );
    assert_eq!(
        patch(&mut c, "{\"valves\":[[{\"name\":\"a\"}]]}"),
        (PatchResult::UnknownKey, "valves.1.1.name".to_string())
    );
    assert_eq!(
        patch(&mut c, "{\"calib\":{\"hour\":\"x\"}}"),
        (PatchResult::WrongType, "calib.hour".to_string())
    );
    assert_eq!(
        patch(&mut c, "{\"calib\":{\"hour\":24}}"),
        (PatchResult::OutOfRange, "calib.hour".to_string())
    );
    assert_eq!(
        patch(&mut c, "{\"schema\":3}"),
        (PatchResult::ReadOnly, "schema".to_string())
    );
    assert_eq!(patch(&mut c, "{\"schema\":1}").0, PatchResult::Ok);
    assert_eq!(patch(&mut c, "{\"schema\":2}").0, PatchResult::Ok);
    assert_eq!(
        patch(&mut c, "{\"calib\":{\"hour\":null}}").0,
        PatchResult::WrongType
    );
    assert_eq!(patch(&mut c, "{\"nope\":{}}").0, PatchResult::Ok); // empty container: no leaf
    assert_eq!(
        patch(&mut c, "{\"nope\":1}"),
        (PatchResult::UnknownKey, "nope".to_string())
    );
    assert_eq!(
        patch(&mut c, "{\"\":1}"),
        (PatchResult::UnknownKey, String::new())
    );

    // The first failure stops the walk; earlier keys were applied to `c`.
    let mut d = Config::default();
    assert_eq!(
        patch(
            &mut d,
            "{\"calib\":{\"hour\":3,\"minute\":60,\"dayMask\":1}}"
        ),
        (PatchResult::OutOfRange, "calib.minute".to_string())
    );
    assert_eq!(d.calib.hour, 3);
    assert_eq!(d.calib.day_mask, 9);

    // Invalid after all keys: validate_config path.
    let mut e = Config::default();
    assert_eq!(
        patch(&mut e, "{\"mqtt\":{\"mode\":1}}"),
        (PatchResult::Invalid, "mqtt.host".to_string())
    );
    assert_eq!(
        patch(&mut e, "{\"mqtt\":{\"mode\":1,\"host\":\"b\"}}").0,
        PatchResult::Ok
    );

    // clearSecrets anywhere at the root, applied before the secrets.
    let mut s = Config::default();
    strcpy(&mut s.mqtt.password, "p");
    assert_eq!(
        patch(&mut s, "{\"mqtt\":{\"password\":\"\"}}").0,
        PatchResult::Ok
    );
    assert_text(&s.mqtt.password, "p");
    assert_eq!(
        patch(
            &mut s,
            "{\"mqtt\":{\"password\":\"\"},\"clearSecrets\":true}"
        )
        .0,
        PatchResult::Ok
    );
    assert!(s.mqtt.password.is_empty());
    strcpy(&mut s.mqtt.password, "p");
    assert_eq!(
        patch(
            &mut s,
            "{\"clearSecrets\":false,\"mqtt\":{\"password\":\"\"}}"
        )
        .0,
        PatchResult::Ok
    );
    assert_text(&s.mqtt.password, "p");
    assert_eq!(
        patch(&mut s, "{\"clearSecrets\":1}"),
        (PatchResult::WrongType, "clearSecrets".to_string())
    );
    assert_eq!(
        patch(&mut s, "{\"clearSecrets\":\"true\"}").0,
        PatchResult::WrongType
    );
    // Only the root key is special.
    assert_eq!(
        patch(&mut s, "{\"web\":{\"clearSecrets\":true}}"),
        (PatchResult::UnknownKey, "web.clearSecrets".to_string())
    );
    // Last root clearSecrets wins.
    assert_eq!(
        patch(
            &mut s,
            "{\"clearSecrets\":true,\"clearSecrets\":false,\"mqtt\":{\"password\":\"\"}}"
        )
        .0,
        PatchResult::Ok
    );
    assert_text(&s.mqtt.password, "p");
}

#[test]
fn patch_string_decoding() {
    let mut c = Config::default();
    assert_eq!(
        patch(&mut c, "{\"station\":\"A\\u0042\\/\\\\x\"}").0,
        PatchResult::OutOfRange
    );
    assert_eq!(
        patch(&mut c, "{\"station\":\"A\\u0042_x\"}").0,
        PatchResult::Ok
    );
    assert_text(&c.station, "AB_x");
    assert_eq!(
        patch(&mut c, "{\"time\":{\"tzName\":\"a\\/b\"}}").0,
        PatchResult::Ok
    );
    assert_text(&c.time.tz_name, "a/b");
    assert_eq!(
        patch(
            &mut c,
            "{\"mqtt\":{\"user\":\"q\\\"\",\"password\":\"\\\\\"}}"
        )
        .0,
        PatchResult::Ok
    );
    assert_text(&c.mqtt.user, "q\"");
    assert_text(&c.mqtt.password, "\\");
    // Control characters decode fine but fail the field rules.
    let rejected: [&[u8]; 10] = [
        b"\\n",
        b"\\r",
        b"\\t",
        b"\\b",
        b"\\f",
        b"\\u0000",
        b"\\u007f",
        b"\\u0085",
        b"\xc2\x85",
        b"\xc3",
    ];
    for r in rejected {
        let doc = [&b"{\"mqtt\":{\"user\":\"a"[..], r, b"\"}}"].concat();
        assert_eq!(
            patch_bytes(&mut c, &doc),
            (PatchResult::OutOfRange, "mqtt.user".to_string()),
            "{r:?}"
        );
    }
    // UTF-8, raw or escaped, is text.
    let accepted: [(&str, &str); 4] = [
        ("\\u00e4", "\u{e4}"),
        ("\\u20ac", "\u{20ac}"),
        ("\\ud83d\\ude00", "\u{1f600}"),
        ("\u{e4}", "\u{e4}"),
    ];
    for (esc, utf8) in accepted {
        let doc = format!("{{\"mqtt\":{{\"user\":\"a{esc}\"}}}}");
        assert_eq!(patch(&mut c, &doc).0, PatchResult::Ok, "{esc}");
        assert_text(&c.mqtt.user, format!("a{utf8}"));
    }
    // Keys are decoded too.
    assert_eq!(
        patch(&mut c, "{\"c\\u0061lib\":{\"\\u0068our\":7}}").0,
        PatchResult::Ok
    );
    assert_eq!(c.calib.hour, 7);
    assert_eq!(
        patch(&mut c, "{\"cal\\u0000ib\":{\"hour\":7}}").0,
        PatchResult::UnknownKey
    );
    // A 96+ byte string is cut, and then too long for every field.
    let long_value = "a".repeat(200);
    assert_eq!(
        patch(&mut c, &format!("{{\"station\":\"{long_value}\"}}")),
        (PatchResult::OutOfRange, "station".to_string())
    );
    assert_eq!(
        patch(&mut c, &format!("{{\"nope\":\"{long_value}\"}}")).0,
        PatchResult::UnknownKey
    );
    // A long key or path is an unknown key.
    assert_eq!(
        patch(&mut c, &format!("{{\"{long_value}\":1}}")).0,
        PatchResult::UnknownKey
    );
    let deep_key = format!("{{\"calib\":{{\"{}\":1}}}}", "k".repeat(60));
    assert_eq!(
        patch(&mut c, &deep_key),
        (PatchResult::UnknownKey, "calib".to_string())
    );
    // 65 chars of path are an overflow (the path stops at the parent).
    assert_eq!(
        patch(
            &mut c,
            &format!("{{\"calib\":{{\"{}\":1}}}}", "k".repeat(59))
        ),
        (PatchResult::UnknownKey, "calib".to_string())
    );
    // Exactly 64 chars of path are still passed on.
    let key58 = "k".repeat(58); // "calib." + 58 = 64
    assert_eq!(
        patch(&mut c, &format!("{{\"calib\":{{\"{key58}\":1}}}}")),
        (PatchResult::UnknownKey, format!("calib.{key58}"))
    );
}

#[test]
fn patch_hex_escapes_and_invalid_strings_before_a_closing_brace() {
    let mut c = Config::default();
    assert_eq!(
        patch(
            &mut c,
            "{\"time\":{\"tzName\":\"\\u0030\\u0039\\u0041\\u0046\\u0061\\u0066\
\\u004a\\u006B\"}}"
        )
        .0,
        PatchResult::Ok
    );
    assert_text(&c.time.tz_name, "09AFafJk");
    // Every hex digit class at both ends: 0 9 a f A F.
    assert_eq!(
        patch(
            &mut c,
            "{\"time\":{\"tzName\":\"\\u004F\\u006f\\u004A\\u006a\\u0030\\u0039\"}}"
        )
        .0,
        PatchResult::Ok
    );
    assert_text(&c.time.tz_name, "OoJj09");
    // Surrogate ranges: syntax only (the text itself is rejected by the field).
    for e in [
        "\\ud7ff",
        "\\ue000",
        "\\udbff\\udfff",
        "\\ud800\\udc00",
        "\\uD800\\uDC00",
    ] {
        assert_eq!(
            patch(&mut c, &format!("{{\"x\":\"{e}\"}}")).0,
            PatchResult::UnknownKey,
            "{e}"
        );
    }
    for e in [
        "\\udfff",
        "\\udc00",
        "\\udbff\\ue000",
        "\\udbff\\udbff",
        "\\ud800\\ud7ff",
        "\\ud800\\u",
        "\\ud800\\",
    ] {
        assert_eq!(
            patch(&mut c, &format!("{{\"x\":\"{e}\"}}")).0,
            PatchResult::Malformed,
            "{e}"
        );
    }
    // An overlong key below a valid path never falls back to that path.
    assert_eq!(
        patch(
            &mut c,
            &format!("{{\"calib.hour\":{{\"{}\":5}}}}", "k".repeat(60))
        )
        .0,
        PatchResult::UnknownKey
    );
    assert_eq!(c.calib.hour, 0);
    for h in ["G", "g", "/", ":", "@", "`"] {
        assert_eq!(
            patch(
                &mut c,
                &format!("{{\"time\":{{\"tzName\":\"\\u004{h}\"}}}}")
            )
            .0,
            PatchResult::Malformed,
            "{h}"
        );
    }
    // Each of these is malformed; a parser that gave up on the string without failing would see
    // a complete document and report an unknown key.
    let bad: [&str; 7] = [
        "{\"x\":\"\n}",
        "{\"x\":\"\\q}",
        "{\"x\":\"\\ud800}",
        "{\"x\":\"\\ud800\\u0041}",
        "{\"x\":\"\\udc00}",
        "{\"x\":\"\\u12}",
        "{\"x\":\"\x01}",
    ];
    for b in bad {
        assert_eq!(patch(&mut c, b).0, PatchResult::Malformed, "{b:?}");
    }
    // Empty keys never fall back to the parent path.
    assert_eq!(
        patch(&mut c, "{\"calib.hour\":{\"\":5}}").0,
        PatchResult::UnknownKey
    );
    assert_eq!(c.calib.hour, 0);
    assert_eq!(
        patch(&mut c, "{\"calib.hour\":{\"a\\u0000\":5}}").0,
        PatchResult::UnknownKey
    );
    assert_eq!(c.calib.hour, 0);
    // clearSecrets first, secrets after it.
    strcpy(&mut c.mqtt.user, "u");
    strcpy(&mut c.mqtt.password, "p");
    assert_eq!(
        patch(
            &mut c,
            "{\"clearSecrets\":true,\"mqtt\":{\"user\":\"\",\"password\":\"\"}}"
        )
        .0,
        PatchResult::Ok
    );
    assert!(c.mqtt.password.is_empty());
}

#[test]
fn patch_number_forms() {
    let mut c = Config::default();
    assert_eq!(
        patch(&mut c, "{\"calib\":{\"hour\":-0}}").0,
        PatchResult::Ok
    );
    assert_eq!(c.calib.hour, 0);
    assert_eq!(
        patch(&mut c, "{\"calib\":{\"hour\":1e1}}").0,
        PatchResult::Ok
    );
    assert_eq!(c.calib.hour, 10);
    assert_eq!(
        patch(&mut c, "{\"calib\":{\"hour\":2.0E+0}}").0,
        PatchResult::Ok
    );
    assert_eq!(c.calib.hour, 2);
    assert_eq!(
        patch(&mut c, "{\"calib\":{\"hour\":2.5}}").0,
        PatchResult::WrongType
    );
    for v in [
        "-1".to_string(),
        "999999999999999999".to_string(),
        "9999999999999999999".to_string(),
        "-9999999999999999999".to_string(),
        "1".repeat(60),
    ] {
        assert_eq!(
            patch(&mut c, &format!("{{\"calib\":{{\"hour\":{v}}}}}")).0,
            PatchResult::OutOfRange,
            "{v}"
        );
    }
    assert_eq!(
        patch(&mut c, "{\"temps\":[{\"offset\":-0.05}]}").0,
        PatchResult::Ok
    );
    assert_eq!(c.temps[0].offset, -1);
    assert_eq!(
        patch(&mut c, "{\"volts\":[{\"factor\":1e-3}]}").0,
        PatchResult::Ok
    );
    assert_eq!(c.volts[0].factor, 0.001f32);
}

#[test]
fn patch_syntax_errors_report_the_byte_offset() {
    let cases: [(&str, &str); 41] = [
        ("", "@0"),
        ("   ", "@3"),
        ("[]", "@0"),
        ("1", "@0"),
        ("\"x\"", "@0"),
        ("{", "@1"),
        ("{\"a\"", "@4"),
        ("{\"a\":", "@5"),
        ("{\"a\":1", "@6"),
        ("{\"a\":1,}", "@7"),
        ("{\"a\" 1}", "@5"),
        ("{a:1}", "@1"),
        ("{\"a\":1}}", "@7"),
        ("{\"a\":1} x", "@8"),
        ("{\"a\":01}", "@5"),
        ("{\"a\":1.}", "@5"),
        ("{\"a\":.5}", "@5"),
        ("{\"a\":-}", "@5"),
        ("{\"a\":+1}", "@5"),
        ("{\"a\":1e}", "@5"),
        ("{\"a\":0x10}", "@6"),
        ("{\"a\":tru}", "@5"),
        ("{\"a\":nul}", "@5"),
        ("{\"a\":True}", "@5"),
        ("{\"a\":'x'}", "@5"),
        ("{\"a\":\"x}", "@8"),
        ("{\"a\":\"\\x\"}", "@8"),
        ("{\"a\":\"\\u12\"}", "@8"),
        ("{\"a\":\"\\u12g4\"}", "@8"),
        ("{\"a\":\"\\udc00\"}", "@12"),
        ("{\"a\":\"\\ud800\"}", "@12"),
        ("{\"a\":\"\\ud800\\u0041\"}", "@18"),
        ("{\"a\":\"\\ud800x\"}", "@12"),
        ("{\"a\":\"a\nb\"}", "@8"),
        ("{\"a\":[1,]}", "@8"),
        ("{\"a\":[1 2]}", "@8"),
        ("{\"a\":{}", "@7"),
        ("{\"a\":1 \"b\":2}", "@7"),
        ("{,\"a\":1}", "@1"),
        ("{\"a\":1,,\"b\":1}", "@7"),
        ("{\"a\":[}", "@6"),
    ];
    for (json, at) in cases {
        let mut c = Config::default();
        assert_eq!(
            patch(&mut c, json),
            (PatchResult::Malformed, at.to_string()),
            "{json:?}"
        );
        assert!(same_config(&c, &Config::default()), "{json:?}"); // nothing applied
    }
    // A syntax error after a valid key: nothing is applied either.
    let mut c = Config::default();
    assert_eq!(
        patch(&mut c, "{\"calib\":{\"hour\":3},\"x\":}").0,
        PatchResult::Malformed
    );
    assert_eq!(c.calib.hour, 0);
    // Embedded NUL.
    assert_eq!(
        patch_bytes(&mut c, b"{\"calib\":{\"hour\":3}}\0"),
        (PatchResult::Malformed, "@20".to_string())
    );
    // C++ a null pointer: no Rust form. Tiny path buffers.
    let mut p = [0u8; 4];
    assert_eq!(
        apply_config_json(&mut c, b"[", &mut []),
        (PatchResult::Malformed, 0)
    );
    assert_eq!(
        apply_config_json(&mut c, b"{\"calib\":{\"hour\":99}}", &mut p),
        (PatchResult::OutOfRange, 3)
    );
    assert_text(&p[..3], "cal");
}

#[test]
fn patch_nesting_depth_is_bounded() {
    let mut c = Config::default();
    // 8 levels are parsed (then the path is simply unknown).
    assert_eq!(
        patch(&mut c, "{\"a\":[[[[[[[1]]]]]]]}"),
        (PatchResult::UnknownKey, "a.1.1.1.1.1.1.1".to_string())
    );
    assert_eq!(
        patch(&mut c, "{\"a\":[[[[[[[[1]]]]]]]]}"),
        (PatchResult::Malformed, "@12".to_string())
    );
    let very_deep = format!("{{\"a\":{}{}}}", "[".repeat(5000), "]".repeat(5000));
    assert_eq!(patch(&mut c, &very_deep).0, PatchResult::Malformed);
}

#[test]
fn patch_fuzz_with_random_bytes_and_mutated_documents() {
    let mut rng = Rng::new(20260924);
    let base = export_json(&full_config()).into_bytes();
    const ALPHABET: &[u8] = b"{}[]\":,\\/ -0123456789.eEtrufalsn\"abcxyz\x01\xff";
    let letter = |rng: &mut Rng| ALPHABET[rng.below(ALPHABET.len() as u32) as usize];
    for iter in 0..3000 {
        let mut doc: Vec<u8>;
        if iter % 2 == 0 {
            let n = rng.below(64);
            doc = (0..n).map(|_| letter(&mut rng)).collect();
            if iter % 4 == 0 {
                doc.insert(0, b'{');
            }
        } else {
            doc = base.clone();
            let edits = 1 + rng.below(4);
            for _ in 0..edits {
                let at = rng.below(doc.len() as u32) as usize;
                match rng.below(3) {
                    0 => doc[at] = letter(&mut rng),
                    1 => {
                        let end = (at + 1 + rng.below(8) as usize).min(doc.len());
                        doc.drain(at..end);
                    }
                    _ => doc.insert(at, letter(&mut rng)),
                }
                if doc.is_empty() {
                    doc = b"{".to_vec();
                }
            }
        }
        let mut c = full_config();
        let mut path = [0u8; 80];
        let (r, n) = apply_config_json(&mut c, &doc, &mut path);
        assert!(PatchResult::from_raw(r as u8).is_some());
        assert!(n < path.len());
        if r == PatchResult::Ok {
            assert_eq!(validate_path(&c), "OK");
        }
        if r == PatchResult::Malformed {
            assert_eq!(path[0], b'@');
        }
    }
}

// ---------------------------------------------------------------- binary

#[test]
fn crc32_matches_zlib() {
    let check = b"123456789";
    assert_eq!(crc32(check, 0), 0xCBF4_3926);
    assert_eq!(crc32(&check[..0], 0), 0);
    // C++ crc32(nullptr, 5) and crc32(nullptr, 5, 0x1234): no Rust form; an empty slice keeps
    // the previous CRC.
    assert_eq!(crc32(&[], 0x1234), 0x1234);
    assert_eq!(crc32(&check[4..], crc32(&check[..4], 0)), 0xCBF4_3926);
    assert_eq!(crc32(&[0], 0), 0xD202_EF8D);
    assert_eq!(crc32(&[0xFF; 4], 0), 0xFFFF_FFFF);
    assert_eq!(
        crc32(b"The quick brown fox jumps over the lazy dog", 0),
        0x414F_A339
    );
}

#[test]
fn binary_encoding_layout_and_round_trip() {
    let d = encode(&Config::default());
    assert!(d.len() > 12);
    assert_eq!(&d[..4], b"VDMC");
    assert_eq!(d[4], 1);
    assert_eq!(d[5], 0);
    assert_eq!(usize::from(d[6]) | usize::from(d[7]) << 8, d.len() - 12);
    let crc = crc32(&d[..d.len() - 4], 0);
    assert_eq!(d[d.len() - 4], crc as u8);
    assert_eq!(d[d.len() - 1], (crc >> 24) as u8);
    // The payload starts with the station as u8 length + bytes.
    assert_eq!(d[8], 5);
    assert_eq!(&d[9..14], b"VdMot");
    // net.iface u8, dhcp u8, ip u32 LE.
    assert_eq!(d[14], 0);
    assert_eq!(d[15], 1);

    let mut back = Config::default();
    back.calib.hour = 7;
    assert_eq!(decode_config(&d, &mut back, None), DecodeResult::Ok);
    assert!(same_config(&back, &Config::default()));

    let full = full_config();
    let f = encode(&full);
    assert!(f.len() <= CONFIG_BLOB_MAX);
    let mut fb = Config::default();
    assert_eq!(decode_config(&f, &mut fb, None), DecodeResult::Ok);
    assert!(same_config(&fb, &full));
    assert_text(&fb.mqtt.password, "mqpw"); // secrets are persisted
    assert_text(&fb.net.wifi_password, "secret123");
    assert_eq!(fb.volts[7].factor, 0.01f32);
    assert_eq!(fb.temps[0].offset, -15);
    assert_eq!(fb.net.ip, full.net.ip);

    // Little-endian u16: mqtt.port 8883 = 0x22B3 somewhere in the payload.
    let p1 = Config::default();
    let mut p2 = Config::default();
    p2.mqtt.port = 0x1234;
    let at = diff_offset(&p1, &p2);
    assert_eq!(encode(&p2)[at], 0x34);
    assert_eq!(encode(&p2)[at + 1], 0x12);
}

#[test]
fn encode_needs_the_full_capacity() {
    let full = full_config();
    let f = encode(&full);
    let mut buf = vec![0u8; f.len()];
    for cap in 0..f.len() {
        assert_eq!(encode_config(&full, &mut buf[..cap]), 0, "{cap}");
    }
    assert_eq!(encode_config(&full, &mut buf), f.len());
    assert_eq!(buf, f);
    // C++ encodeConfig(full, nullptr, 4096): no Rust form.
}

#[test]
fn decode_rejects_every_kind_of_damage_and_keeps_defaults() {
    let full = full_config();
    let good = encode(&full);
    let mut out = Config::default();

    let expect = |b: &[u8], r: DecodeResult| {
        let mut o = full.clone();
        assert_eq!(decode_config(b, &mut o, None), r);
        if r != DecodeResult::Ok {
            assert!(same_config(&o, &Config::default()));
        }
    };

    // C++ decodeConfig(nullptr, 100, out): no Rust form; the empty slice is n = 0 below.
    for n in 0..12 {
        expect(&good[..n], DecodeResult::TooShort);
    }
    expect(&good[..good.len() - 1], DecodeResult::TooShort);
    {
        // Header + CRC only (12 bytes, empty payload): structurally complete, but the fields
        // are missing.
        let mut b = vec![b'V', b'D', b'M', b'C', 1, 0, 0, 0, 0, 0, 0, 0];
        fix_crc(&mut b);
        expect(&b, DecodeResult::Invalid);
    }
    {
        let mut b = good.clone();
        b.push(0);
        expect(&b, DecodeResult::Invalid);
    }
    for k in 0..4 {
        let mut b = good.clone();
        b[k] ^= 0x20;
        expect(&b, DecodeResult::BadMagic);
    }
    {
        let mut b = good.clone();
        b[20] ^= 1;
        expect(&b, DecodeResult::BadCrc);
        b = good.clone();
        *b.last_mut().expect("bytes") ^= 0x80;
        expect(&b, DecodeResult::BadCrc);
    }
    {
        // Schema 0 never existed; a newer schema is read by its schema-1 prefix.
        let mut b = good.clone();
        b[4] = 0;
        fix_crc(&mut b);
        expect(&b, DecodeResult::UnsupportedSchema);
        let mut info = DecodeInfo::default();
        assert_eq!(
            decode_config(&b, &mut out, Some(&mut info)),
            DecodeResult::UnsupportedSchema
        );
        assert_eq!(info.schema, 0);
        assert!(!info.newer_schema);
        b[4] = 1;
        b[5] = 1; // 257
        fix_crc(&mut b);
        let mut o = Config::default();
        assert_eq!(decode_config(&b, &mut o, Some(&mut info)), DecodeResult::Ok);
        assert_eq!(info.schema, 257);
        assert!(info.newer_schema);
        assert!(same_config(&o, &full));
    }
    {
        // Payload length one short: last field incomplete -> too short a remaining payload vs.
        // the declared blob length.
        let mut b = good.clone();
        let payload = good.len() - 12;
        b[6] = (payload - 1) as u8;
        b[7] = ((payload - 1) >> 8) as u8;
        b.remove(b.len() - 5);
        fix_crc(&mut b);
        expect(&b, DecodeResult::Invalid);
        // Payload with one extra byte.
        b = good.clone();
        b[6] = (payload + 1) as u8;
        b[7] = ((payload + 1) >> 8) as u8;
        b.insert(b.len() - 4, 0);
        fix_crc(&mut b);
        expect(&b, DecodeResult::Invalid);
    }
    {
        // Station length byte larger than its field.
        let mut b = good.clone();
        b[8] = 21;
        fix_crc(&mut b);
        expect(&b, DecodeResult::Invalid);
        // Station with an embedded NUL ("H\0izung OG" would validate as "H").
        b = good.clone();
        b[10] = 0;
        fix_crc(&mut b);
        expect(&b, DecodeResult::Invalid);
        // Station of length 0: structurally fine, repaired (C-6).
        let mut zero_len = encode(&Config::default());
        zero_len.drain(9..14);
        zero_len[8] = 0;
        let pl = zero_len.len() - 12;
        zero_len[6] = pl as u8;
        zero_len[7] = (pl >> 8) as u8;
        fix_crc(&mut zero_len);
        let mut z = Config::default();
        let mut info = DecodeInfo::default();
        assert_eq!(
            decode_config(&zero_len, &mut z, Some(&mut info)),
            DecodeResult::Ok
        );
        assert_text(&z.station, "VdMot");
        assert_eq!(info.repairs.mask, REPAIR_FIELD);
        assert_eq!(info.repairs.count, 1);
        assert_text(&info.repairs.first, "station");
    }
    {
        // Bool byte 2 and out-of-range values.
        let a = Config::default();
        let b2 = Config {
            persist_log: false,
            ..Config::default()
        };
        let at = diff_offset(&a, &b2);
        let mut b = encode(&a);
        b[at] = 2;
        fix_crc(&mut b);
        expect(&b, DecodeResult::Invalid);
        let mut h = Config::default();
        h.calib.hour = 5;
        let hour_at = diff_offset(&a, &h);
        b = encode(&a);
        b[hour_at] = 24;
        fix_crc(&mut b);
        let mut o = Config::default();
        let mut info = DecodeInfo::default();
        assert_eq!(decode_config(&b, &mut o, Some(&mut info)), DecodeResult::Ok);
        assert_eq!(o.calib.hour, 0);
        assert_eq!(info.repairs.mask, REPAIR_FIELD);
        assert_text(&info.repairs.first, "calib.hour");
        b[hour_at] = 23;
        fix_crc(&mut b);
        assert_eq!(decode_config(&b, &mut o, Some(&mut info)), DecodeResult::Ok);
        assert_eq!(o.calib.hour, 23);
        assert_eq!(info.repairs.mask, 0);
    }
}

#[test]
fn the_loader_clears_the_later_of_two_valves_on_one_mqtt_segment() {
    // The 2.0.0 rule: names compare with ' ' == '_', each against every earlier valve; the
    // loader clears the later name instead of dropping the blob.
    let pairs: [(usize, usize, &str, &str); 3] = [
        (0, 1, "Dom", "Dom"),
        (1, 11, "a b", "a_b"),
        (3, 7, "x_y", "x y"),
    ];
    for (a, b, name_a, name_b) in pairs {
        let mut c = Config::default();
        strcpy(&mut c.valves[a].name, name_a);
        strcpy(&mut c.valves[b].name, name_b);
        let blob = encode(&c);
        let mut out = Config::default();
        let mut info = DecodeInfo::default();
        assert_eq!(
            decode_config(&blob, &mut out, Some(&mut info)),
            DecodeResult::Ok
        );
        assert_text(&out.valves[a].name, name_a);
        assert!(out.valves[b].name.is_empty(), "{name_b}");
        assert_eq!(info.repairs.mask, REPAIR_VALVE_NAMES);
        assert_eq!(info.repairs.valve_names, 1 << b);
    }
    // A name equal to the number of an unnamed valve is that valve's segment.
    let mut num = Config::default();
    strcpy(&mut num.valves[4].name, "1");
    let nb = encode(&num);
    let mut num_out = Config::default();
    let mut num_info = DecodeInfo::default();
    assert_eq!(
        decode_config(&nb, &mut num_out, Some(&mut num_info)),
        DecodeResult::Ok
    );
    assert!(num_out.valves[4].name.is_empty());
    assert_text(&num_info.repairs.first, "valves.5.name");
    let mut ok = Config::default();
    strcpy(&mut ok.valves[3].name, "x_y");
    strcpy(&mut ok.valves[7].name, "x-y");
    let b = encode(&ok);
    let mut out = Config::default();
    assert_eq!(decode_config(&b, &mut out, None), DecodeResult::Ok);
    assert_text(&out.valves[7].name, "x-y");
}

#[test]
fn decode_fuzz() {
    let mut rng = Rng::new(424242);
    let good = encode(&full_config());
    for iter in 0..4000 {
        let mut b: Vec<u8>;
        if iter % 3 == 0 {
            let n = rng.below(64) as usize;
            b = (0..n).map(|_| rng.next_u32() as u8).collect();
        } else {
            b = good.clone();
            let flips = 1 + rng.below(3);
            for _ in 0..flips {
                // C++17: the value (right operand) is drawn before the index.
                let v = rng.next_u32() as u8;
                let at = 8 + rng.below((b.len() - 12) as u32) as usize;
                b[at] = v;
            }
            if iter % 3 == 1 {
                fix_crc(&mut b);
            }
        }
        let mut c = full_config();
        let r = decode_config(&b, &mut c, None);
        if r == DecodeResult::Ok {
            assert_eq!(validate_path(&c), "OK");
        } else {
            assert!(same_config(&c, &Config::default()));
        }
    }
}

// ---- mutation-driven cases -------------------------------------------------

#[test]
fn tz_posix_no_space_rule_checks_every_byte_tilde_allowed() {
    let mut c = Config::default();
    set_defaults(&mut c);
    assert_eq!(
        set(&mut c, "time.tzPosix", vs(" CET-1")),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "time.tzPosix", vs("\x7fCET")),
        SetResult::OutOfRange
    );
    assert_eq!(
        set(&mut c, "time.tzPosix", vs("CET-1 ")),
        SetResult::OutOfRange
    );
    assert_eq!(set(&mut c, "time.tzPosix", vs("~")), SetResult::Ok);
    assert_text(&c.time.tz_posix, "~");
    assert_eq!(set(&mut c, "time.tzPosix", vs("!A~")), SetResult::Ok);
    assert!(validate_config(&c, &mut []).is_ok());
    c.time.tz_posix[0] = b' ';
    let mut path = [0u8; 40];
    let n = validate_config(&c, &mut path).expect_err("invalid");
    assert_text(&path[..n], "time.tzPosix");
}

#[test]
fn numeric_strings_are_parsed_within_their_length_only() {
    let mut c = Config::default();
    set_defaults(&mut c);
    // "-5" cut to "-": not a number, even though a digit follows in memory.
    assert_eq!(
        set(&mut c, "mqtt.minDelayS", vsl(&b"-5"[..1])),
        SetResult::WrongType
    );
    // "1." cut to "1": a number, the '.' outside the length does not count.
    assert_eq!(
        set(&mut c, "mqtt.minDelayS", vsl(&b"1."[..1])),
        SetResult::Ok
    );
    assert_eq!(c.mqtt.min_delay_s, 1);
    assert_eq!(
        set(&mut c, "mqtt.minDelayS", vsl(&b"2e"[..1])),
        SetResult::Ok
    );
    assert_eq!(c.mqtt.min_delay_s, 2);
    assert_eq!(
        set(&mut c, "mqtt.minDelayS", vsl(&b"30"[..1])),
        SetResult::Ok
    );
    assert_eq!(c.mqtt.min_delay_s, 3);
    assert_eq!(
        set(&mut c, "mqtt.minDelayS", vsl(b"4.5")),
        SetResult::WrongType
    );
    assert_eq!(set(&mut c, "mqtt.minDelayS", vsl(b"4.0")), SetResult::Ok);
    assert_eq!(c.mqtt.min_delay_s, 4);
    assert_eq!(
        set(&mut c, "mqtt.minDelayS", vsl(&b"5e0"[..2])),
        SetResult::WrongType
    );
    assert_eq!(
        set(&mut c, "mqtt.minDelayS", vsl(b"6e+")),
        SetResult::WrongType
    );
    assert_eq!(set(&mut c, "mqtt.minDelayS", vsl(b"7e+0")), SetResult::Ok);
    assert_eq!(c.mqtt.min_delay_s, 7);
}

/// Applies only the first `len` bytes of `full`.
fn patch_cut(c: &mut Config, full: &str, len: usize) -> (PatchResult, String) {
    patch_bytes(c, &full.as_bytes()[..len])
}

fn at(n: usize) -> String {
    format!("@{n}")
}

#[test]
fn patch_nothing_after_len_is_read() {
    let mut c = Config::default();
    set_defaults(&mut c);
    // Trailing whitespace beyond len.
    assert_eq!(patch_cut(&mut c, "{} ", 2).0, PatchResult::Ok);
    assert_eq!(patch_cut(&mut c, "{}\t\n", 3).0, PatchResult::Ok);
    // A literal cut by len.
    let lit = "{\"persistLog\":true}";
    let t = lit.find("true").expect("true");
    assert_eq!(
        patch_cut(&mut c, lit, t + 3),
        (PatchResult::Malformed, at(t))
    );
    // The literal ends exactly at len: accepted, the object is unterminated.
    assert_eq!(
        patch_cut(&mut c, lit, t + 4),
        (PatchResult::Malformed, at(t + 4))
    );
    let lf = "{\"persistLog\":false}";
    let f = lf.find("false").expect("false");
    assert_eq!(
        patch_cut(&mut c, lf, f + 4),
        (PatchResult::Malformed, at(f))
    );
    assert_eq!(
        patch_cut(&mut c, lf, f + 5),
        (PatchResult::Malformed, at(f + 5))
    );
    let ln = "{\"persistLog\":null}";
    let n = ln.find("null").expect("null");
    assert_eq!(
        patch_cut(&mut c, ln, n + 3),
        (PatchResult::Malformed, at(n))
    );
}

#[test]
fn patch_u_escapes_cut_by_len() {
    let mut c = Config::default();
    set_defaults(&mut c);
    let s = "{\"station\":\"\\u0041\"}";
    let h = s.find("0041").expect("digits");
    for k in 0..4 {
        assert_eq!(
            patch_cut(&mut c, s, h + k),
            (PatchResult::Malformed, at(h)),
            "{k}"
        );
    }
    // All four digits inside len: the string is unterminated at len.
    assert_eq!(
        patch_cut(&mut c, s, h + 4),
        (PatchResult::Malformed, at(h + 4))
    );
    assert_eq!(patch(&mut c, s).0, PatchResult::Ok);
    assert_text(&c.station, "A");

    // Surrogate pair: the low half must be inside len.
    let sp = "{\"station\":\"\\uD83D\\uDE00\"}";
    let lo = sp.find("\\uDE00").expect("low half");
    assert_eq!(patch_cut(&mut c, sp, lo), (PatchResult::Malformed, at(lo)));
    assert_eq!(
        patch_cut(&mut c, sp, lo + 1),
        (PatchResult::Malformed, at(lo))
    );
    assert_eq!(
        patch_cut(&mut c, sp, lo + 2),
        (PatchResult::Malformed, at(lo + 2))
    );
    assert_eq!(
        patch_cut(&mut c, sp, lo + 6),
        (PatchResult::Malformed, at(lo + 6))
    );
}

#[test]
fn patch_u_escapes_encode_exact_utf8() {
    let cases: [(&str, &[u8]); 10] = [
        ("\\u00a0", b"\xc2\xa0"),
        ("\\u07ff", b"\xdf\xbf"),
        ("\\u0800", b"\xe0\xa0\x80"),
        ("\\u20ac", b"\xe2\x82\xac"),
        ("\\uffff", b"\xef\xbf\xbf"),
        ("\\uD800\\uDC00", b"\xf0\x90\x80\x80"),
        ("\\uD83D\\uDE00", b"\xf0\x9f\x98\x80"),
        ("\\uDBFF\\uDFFF", b"\xf4\x8f\xbf\xbf"),
        ("\\uD8C0\\uDC01", b"\xf1\x80\x80\x81"),
        ("\\u0041\\u007e", b"A~"),
    ];
    for (esc, utf8) in cases {
        let mut c = Config::default();
        set_defaults(&mut c);
        assert_eq!(
            patch(&mut c, &format!("{{\"station\":\"{esc}\"}}")).0,
            PatchResult::Ok,
            "{esc}"
        );
        assert_text(&c.station, utf8);
    }
}

#[test]
fn export_all_ones_addresses_are_written_in_full() {
    let mut c = Config::default();
    set_defaults(&mut c);
    assert_eq!(
        set(&mut c, "net.mask", vs("255.255.255.255")),
        SetResult::Ok
    );
    assert_eq!(
        set(&mut c, "syslog.server", vs("255.255.255.254")),
        SetResult::Ok
    );
    let j = export_json(&c);
    assert!(j.contains("\"mask\":\"255.255.255.255\""));
    assert!(j.contains("\"server\":\"255.255.255.254\""));
}

#[test]
fn patch_syntax_errors_at_len_report_the_exact_offset() {
    let mut c = Config::default();
    set_defaults(&mut c);
    // Escape backslash is the last byte inside len.
    let e = "{\"station\":\"a\\n\"}";
    let bs = e.find('\\').expect("backslash");
    assert_eq!(
        patch_cut(&mut c, e, bs + 1),
        (PatchResult::Malformed, at(bs + 1))
    );
    // A number cut by len: digits after len are not part of it.
    let num = "{\"persistLog\":12}";
    let d = num.find('1').expect("digit");
    assert_eq!(
        patch_cut(&mut c, num, d + 1),
        (PatchResult::Malformed, at(d + 1))
    );
    // Value missing: the string after len is not read.
    let st = "{\"station\":\"x\"}";
    let colon = st.find(':').expect("colon");
    assert_eq!(
        patch_cut(&mut c, st, colon + 1),
        (PatchResult::Malformed, at(colon + 1))
    );
    // Object cut after '{', after a key, before ':'.
    assert_eq!(patch_cut(&mut c, "{}", 1), (PatchResult::Malformed, at(1)));
    assert_eq!(
        patch_cut(&mut c, "{\"a\":1}", 1),
        (PatchResult::Malformed, at(1))
    );
    assert_eq!(
        patch_cut(&mut c, "{\"a\":1}", 4),
        (PatchResult::Malformed, at(4))
    );
    assert_eq!(
        patch_cut(&mut c, "{\"a\" :1}", 5),
        (PatchResult::Malformed, at(5))
    );
    assert_eq!(patch_cut(&mut c, "[]", 1), (PatchResult::Malformed, at(0)));
    assert_eq!(
        patch_cut(&mut c, "{\"valves\":[]}", 11),
        (PatchResult::Malformed, at(11))
    );
}

#[test]
fn patch_empty_keys_and_keys_with_nul_name_nothing() {
    let mut c = Config::default();
    set_defaults(&mut c);
    assert_eq!(
        patch(&mut c, "{\"net\":{\"\":1}}"),
        (PatchResult::UnknownKey, "net".to_string())
    );
    assert_eq!(
        patch(&mut c, "{\"net\":{\"dhcp\\u0000\":true}}"),
        (PatchResult::UnknownKey, "net".to_string())
    );
    assert_eq!(
        patch(&mut c, "{\"\":{\"station\":\"x\"}}"),
        (PatchResult::UnknownKey, String::new())
    );
    assert_eq!(
        patch(&mut c, "{\"\":1}"),
        (PatchResult::UnknownKey, String::new())
    );
}

#[test]
fn binary_a_string_length_past_the_payload_is_not_read() {
    // Header + a 1-byte payload that announces a 20-char station name. The buffer is exactly
    // 13 bytes.
    let mut b = vec![b'V', b'D', b'M', b'C', 1, 0, 1, 0, 20, 0, 0, 0, 0];
    fix_crc(&mut b);
    let mut out = Config::default();
    assert_eq!(decode_config(&b, &mut out, None), DecodeResult::Invalid);
    // Same with a 4-byte field read past a 2-byte payload (station "" + iface).
    let c = Config::default();
    let full = encode(&c);
    let mut cut = full[..8 + 2].to_vec();
    cut[6] = 2;
    cut[7] = 0;
    cut.resize(cut.len() + 4, 0);
    fix_crc(&mut cut);
    assert_eq!(decode_config(&cut, &mut out, None), DecodeResult::Invalid);
    // Payload cut right after the length byte of a 12-char string, deep in the blob: the 12
    // bytes must not be read from beyond the payload.
    let mut other = c.clone();
    assert_text(&c.time.ntp_server, "pool.ntp.org");
    other.time.ntp_server[0] = b'x';
    let off = diff_offset(&c, &other) - 1; // ntpServer length byte
    assert_eq!(usize::from(full[off]), c.time.ntp_server.len());
    let mut deep = full[..off + 1].to_vec();
    let payload = deep.len() - 8;
    deep[6] = payload as u8;
    deep[7] = (payload >> 8) as u8;
    deep.resize(deep.len() + 4, 0);
    fix_crc(&mut deep);
    assert_eq!(decode_config(&deep, &mut out, None), DecodeResult::Invalid);
}

#[test]
fn binary_a_full_length_string_is_encoded_with_every_byte() {
    // C++ "an unterminated string is encoded at most cap-1 bytes" fills the station array
    // without a NUL; a Rust text member holds at most its capacity, so the full-length station
    // is the case here.
    let mut c = Config::default();
    set_defaults(&mut c);
    strcpy(&mut c.station, &"x".repeat(20));
    let mut b = vec![0u8; CONFIG_BLOB_MAX];
    let n = encode_config(&c, &mut b);
    assert!(n > 0);
    assert_eq!(b[8], 20);
    let mut out = Config::default();
    assert_eq!(decode_config(&b[..n], &mut out, None), DecodeResult::Ok);
    assert_text(&out.station, "x".repeat(20));
}

// ---------------------------------------------------------------- keys after 2.0.0

#[test]
fn one_byte_fields_are_stored_in_one_byte() {
    let mut c = Config::default();
    assert_eq!(set(&mut c, "valves.2.topic", vs("t")), SetResult::Ok);
    assert_eq!(set(&mut c, "valves.2.failsafePct", vi(7)), SetResult::Ok);
    assert_eq!(c.valves[1].failsafe_pct, 7);
    assert_text(&c.valves[1].topic, "t");
    assert_eq!(set(&mut c, "net.iface", vi(2)), SetResult::Ok);
    assert_eq!(c.net.iface, NetInterface::Wifi);
    assert!(c.net.dhcp);
    assert_eq!(set(&mut c, "calib.minute", vi(9)), SetResult::Ok);
    assert_eq!(set(&mut c, "calib.hour", vi(5)), SetResult::Ok);
    assert_eq!(set(&mut c, "calib.dayMask", vi(3)), SetResult::Ok);
    assert_eq!(c.calib.minute, 9);
    assert_eq!(c.calib.hour, 5);
    assert_eq!(c.calib.day_mask, 3);
    // Two-byte fields keep their high byte.
    assert_eq!(set(&mut c, "failsafe.timeoutMin", vi(1440)), SetResult::Ok);
    assert_eq!(c.failsafe.timeout_min, 1440);
    assert_eq!(set(&mut c, "mqtt.port", vi(65535)), SetResult::Ok);
    assert_eq!(c.mqtt.port, 65535);
    assert_eq!(set(&mut c, "mqtt.port", vi(65536)), SetResult::OutOfRange);
    assert_eq!(c.mqtt.port, 65535);
}

#[test]
fn failsafe_keys_accept_exactly_their_range() {
    let mut c = Config::default();
    for ok in [0, 1, 99, 100, 255] {
        assert_eq!(
            set(&mut c, "valves.3.failsafePct", vi(ok)),
            SetResult::Ok,
            "{ok}"
        );
        assert_eq!(i64::from(c.valves[2].failsafe_pct), ok);
    }
    for bad in [-1, 101, 102, 200, 254, 256, 355, 65535, 65536] {
        assert_eq!(
            set(&mut c, "valves.3.failsafePct", vi(bad)),
            SetResult::OutOfRange,
            "{bad}"
        );
        assert_eq!(c.valves[2].failsafe_pct, 255);
    }
    assert_eq!(
        set(&mut c, "valves.12.failsafePct", vs("42")),
        SetResult::Ok
    );
    assert_eq!(c.valves[11].failsafe_pct, 42);
    assert_eq!(
        set(&mut c, "valves.12.failsafePct", vf(43.0)),
        SetResult::Ok
    );
    assert_eq!(c.valves[11].failsafe_pct, 43);
    assert_eq!(
        set(&mut c, "valves.12.failsafePct", vf(43.5)),
        SetResult::WrongType
    );
    assert_eq!(
        set(&mut c, "valves.12.failsafePct", vb(true)),
        SetResult::WrongType
    );
    assert_eq!(
        set(&mut c, "valves.12.failsafePct", vs("x")),
        SetResult::WrongType
    );
    assert_eq!(c.valves[11].failsafe_pct, 43);
    // The other valves keep their default.
    assert_eq!(c.valves[0].failsafe_pct, 50);

    for ok in [0, 5, 6, 60, 1439, 1440] {
        assert_eq!(
            set(&mut c, "failsafe.timeoutMin", vi(ok)),
            SetResult::Ok,
            "{ok}"
        );
        assert_eq!(i64::from(c.failsafe.timeout_min), ok);
    }
    for bad in [-1, 1, 4, 1441, 65535, 65536, 65541] {
        assert_eq!(
            set(&mut c, "failsafe.timeoutMin", vi(bad)),
            SetResult::OutOfRange,
            "{bad}"
        );
        assert_eq!(c.failsafe.timeout_min, 1440);
    }
    assert_eq!(set(&mut c, "failsafe.timeoutMin", vs("300")), SetResult::Ok);
    assert_eq!(c.failsafe.timeout_min, 300);
    assert_eq!(
        set(&mut c, "failsafe.timeoutMin", vs("")),
        SetResult::WrongType
    );
    assert_eq!(
        set(&mut c, "failsafe.timeoutMin", vb(false)),
        SetResult::WrongType
    );
    assert_eq!(set(&mut c, "failsafe.x", vi(5)), SetResult::UnknownKey);
    assert_eq!(
        set(&mut c, "failsafe.1.timeoutMin", vi(5)),
        SetResult::UnknownKey
    );
    assert_eq!(c.failsafe.timeout_min, 300);
    assert_eq!(validate_path(&c), "OK");
}

#[test]
fn string_keys_after_2_0_0_and_their_rules() {
    let h79 = ["a", "b", "c", "d"].map(|x| x.repeat(19)).join(",");
    let h80 = format!("{h79}d");
    let h81 = format!("{h80}d");
    let label65 = "a".repeat(65);
    let a80 = "a".repeat(80);
    let c64 = "c".repeat(64);
    let c65 = "c".repeat(65);
    let p32 = format!("ha/{}", "p".repeat(29));
    let p33 = format!("{p32}p");
    use SetResult::{Ok as Fine, OutOfRange as Out};
    let rows: Vec<(&str, &str, SetResult)> = vec![
        ("web.allowedHosts", "", Fine),
        ("web.allowedHosts", "vdmot.lan", Fine),
        ("web.allowedHosts", "vdmot.lan, 192.168.1.9", Fine),
        ("web.allowedHosts", "  a  ,  b  ", Fine),
        ("web.allowedHosts", "a,b,c,d", Fine),
        ("web.allowedHosts", &h79, Fine),
        ("web.allowedHosts", &h80, Fine),
        ("web.allowedHosts", &h81, Out),
        ("web.allowedHosts", "a,b,c,d,e", Out),
        ("web.allowedHosts", "a,,b", Out),
        ("web.allowedHosts", "a,", Out),
        ("web.allowedHosts", ",a", Out),
        ("web.allowedHosts", " , ", Out),
        ("web.allowedHosts", " ", Out),
        ("web.allowedHosts", "bad_host!", Out),
        ("web.allowedHosts", "a b", Out),
        ("web.allowedHosts", "a,-b", Out),
        ("web.allowedHosts", &label65, Out),
        ("web.allowedHosts", &a80, Out), // one entry, whole buffer
        ("mqtt.clientId", "azAZ09._-", Fine),
        ("mqtt.clientId", "a`", Out),
        ("mqtt.clientId", "a{", Out),
        ("mqtt.clientId", "A[", Out),
        ("mqtt.discoveryPrefix", "azAZ09_-/x", Fine),
        ("mqtt.discoveryPrefix", "a`", Out),
        ("mqtt.discoveryPrefix", "a{", Out),
        ("mqtt.discoveryPrefix", "A@", Out),
        ("mqtt.discoveryPrefix", "A[", Out),
        ("mqtt.discoveryPrefix", "0:", Out),
        ("mqtt.rootTopic", "", Fine),
        ("mqtt.rootTopic", "VdMotFBH", Fine),
        ("mqtt.rootTopic", "Dom 1", Fine),
        ("mqtt.rootTopic", "12345678901234567890", Fine),
        ("mqtt.rootTopic", "123456789012345678901", Out),
        ("mqtt.rootTopic", "a/b", Out),
        ("mqtt.rootTopic", "a+b", Out),
        ("mqtt.clientId", "", Fine),
        ("mqtt.clientId", "VdMot-east-6c1e51", Fine),
        ("mqtt.clientId", "a.b_c-D9", Fine),
        ("mqtt.clientId", &c64, Fine),
        ("mqtt.clientId", &c65, Out),
        ("mqtt.clientId", "a b", Out),
        ("mqtt.clientId", "a/b", Out),
        ("mqtt.clientId", "a:b", Out),
        ("mqtt.clientId", "a@b", Out),
        ("mqtt.clientId", "@a", Out),
        ("mqtt.clientId", "K\u{fc}che", Out),
        ("mqtt.discoveryPrefix", "ha/discovery", Fine),
        ("mqtt.discoveryPrefix", "a", Fine),
        ("mqtt.discoveryPrefix", "a-b_c/D9/x", Fine),
        ("mqtt.discoveryPrefix", &p32, Fine),
        ("mqtt.discoveryPrefix", &p33, Out),
        ("mqtt.discoveryPrefix", "", Out),
        ("mqtt.discoveryPrefix", "/ha", Out),
        ("mqtt.discoveryPrefix", "ha/", Out),
        ("mqtt.discoveryPrefix", "/", Out),
        ("mqtt.discoveryPrefix", "a//b", Out),
        ("mqtt.discoveryPrefix", "a b", Out),
        ("mqtt.discoveryPrefix", "a.b", Out),
        ("mqtt.discoveryPrefix", "a+", Out),
        ("mqtt.discoveryPrefix", "+a", Out),
        ("valves.1.topic", "", Fine),
        ("valves.1.topic", "Bad/WC", Fine),
        ("valves.1.topic", "a\"b", Fine),
        ("valves.1.topic", "a\\b", Fine),
        ("valves.1.topic", "\u{e4}/\u{f6}", Fine),
        ("valves.1.topic", "1234567890", Fine),
        ("valves.1.topic", "12345678901", Out),
        ("valves.1.topic", "/a", Out),
        ("valves.1.topic", "a/", Out),
        ("valves.1.topic", "/", Out),
        ("valves.1.topic", "a//b", Out),
        ("valves.1.topic", "a b", Out),
        ("valves.1.topic", "a+b", Out),
        ("valves.1.topic", "a#b", Out),
        ("valves.1.topic", "a\x01", Out),
        ("temps.34.topic", "t/1", Fine),
        ("temps.34.topic", "t 1", Out),
        ("volts.8.topic", "v/1", Fine),
        ("volts.8.topic", "v/", Out),
    ];
    for (path, value, r) in rows {
        let mut k = Config::default();
        assert_eq!(set(&mut k, path, vs(value)), r, "{path} {value:?}");
    }
    let mut k = Config::default();
    assert_eq!(set(&mut k, "valves.1.topic", vsl(b"a\xc3")), Out);
    // Stored exactly; a rejected value leaves the field unchanged.
    let mut c = Config::default();
    assert_eq!(set(&mut c, "web.allowedHosts", vs("  a  ,  b  ")), Fine);
    assert_text(&c.web.allowed_hosts, "  a  ,  b  ");
    assert_eq!(set(&mut c, "web.allowedHosts", vs("a,,b")), Out);
    assert_text(&c.web.allowed_hosts, "  a  ,  b  ");
    assert_eq!(set(&mut c, "temps.34.topic", vs("t/1")), Fine);
    assert_text(&c.temps[33].topic, "t/1");
    assert_eq!(set(&mut c, "volts.8.topic", vs("v/1")), Fine);
    assert_text(&c.volts[7].topic, "v/1");
    assert_eq!(set(&mut c, "mqtt.discoveryPrefix", vs("ha")), Fine);
    assert_text(&c.mqtt.discovery_prefix, "ha");
    assert_eq!(set(&mut c, "mqtt.clientId", vi(1)), SetResult::WrongType);
    assert_eq!(
        set(&mut c, "valves.13.topic", vs("x")),
        SetResult::UnknownKey
    );
}

#[test]
fn stored_values_of_the_new_keys_are_validated() {
    let cases: [(Edit, &str); 12] = [
        (|c| c.valves[3].failsafe_pct = 101, "valves.4.failsafePct"),
        (|c| c.valves[3].failsafe_pct = 254, "valves.4.failsafePct"),
        (|c| c.failsafe.timeout_min = 4, "failsafe.timeoutMin"),
        (|c| c.failsafe.timeout_min = 1, "failsafe.timeoutMin"),
        (|c| c.failsafe.timeout_min = 1441, "failsafe.timeoutMin"),
        (
            |c| strcpy(&mut c.web.allowed_hosts, "a,,b"),
            "web.allowedHosts",
        ),
        (|c| c.mqtt.discovery_prefix.clear(), "mqtt.discoveryPrefix"),
        (|c| strcpy(&mut c.mqtt.client_id, "a b"), "mqtt.clientId"),
        (|c| strcpy(&mut c.mqtt.root_topic, "a/b"), "mqtt.rootTopic"),
        (|c| strcpy(&mut c.valves[0].topic, "a b"), "valves.1.topic"),
        (|c| strcpy(&mut c.temps[1].topic, "/t"), "temps.2.topic"),
        (|c| strcpy(&mut c.volts[2].topic, "v/"), "volts.3.topic"),
        // C++ a valve topic without a NUL in its array: no Rust form.
    ];
    for (edit, path) in cases {
        let mut c = Config::default();
        edit(&mut c);
        assert_eq!(validate_path(&c), path);
    }
    let mut ok = Config::default();
    ok.valves[3].failsafe_pct = FAILSAFE_HOLD;
    ok.valves[4].failsafe_pct = 0;
    ok.valves[5].failsafe_pct = 100;
    ok.failsafe.timeout_min = 0;
    assert_eq!(validate_path(&ok), "OK");
    ok.failsafe.timeout_min = 5;
    assert_eq!(validate_path(&ok), "OK");
    ok.failsafe.timeout_min = 1440;
    assert_eq!(validate_path(&ok), "OK");
}

#[test]
fn v1_home_assistant_needs_the_decimal_point() {
    let mut c = Config::default();
    c.mqtt.mode = MqttMode::MqttHa;
    strcpy(&mut c.mqtt.host, "b");
    c.mqtt.german_decimal = true;
    assert_eq!(validate_path(&c), "mqtt.germanDecimal");
    c.mqtt.mode = MqttMode::Mqtt;
    assert_eq!(validate_path(&c), "OK");
    c.mqtt.mode = MqttMode::MqttHa;
    c.mqtt.german_decimal = false;
    assert_eq!(validate_path(&c), "OK");
    // Checked after every older rule.
    c.mqtt.german_decimal = true;
    c.calib.hour = 24;
    assert_eq!(validate_path(&c), "calib.hour");
    let mut p = Config::default();
    assert_eq!(
        patch(
            &mut p,
            "{\"mqtt\":{\"mode\":2,\"host\":\"b\",\"germanDecimal\":true}}"
        ),
        (PatchResult::Invalid, "mqtt.germanDecimal".to_string())
    );
}

#[test]
fn v2_every_valve_gets_its_own_mqtt_segment() {
    let mut c = Config::default();
    strcpy(&mut c.valves[0].topic, "x");
    strcpy(&mut c.valves[1].name, "x");
    assert_eq!(validate_path(&c), "valves.2.name");
    let mut d = Config::default();
    strcpy(&mut d.valves[0].name, "x");
    strcpy(&mut d.valves[1].topic, "x");
    assert_eq!(validate_path(&d), "valves.2.topic");
    let mut e = Config::default(); // an override equal to an unnamed valve's number
    strcpy(&mut e.valves[0].topic, "3");
    assert_eq!(validate_path(&e), "valves.3.name");
    strcpy(&mut e.valves[2].name, "three");
    assert_eq!(validate_path(&e), "OK");
    let mut f = Config::default(); // names map ' ' to '_'
    strcpy(&mut f.valves[4].name, "a b");
    strcpy(&mut f.valves[9].topic, "a_b");
    assert_eq!(validate_path(&f), "valves.10.topic");
    let mut g = Config::default();
    strcpy(&mut g.valves[10].topic, "a/b");
    strcpy(&mut g.valves[11].topic, "a/b");
    assert_eq!(validate_path(&g), "valves.12.topic");
    strcpy(&mut g.valves[11].topic, "a/c");
    assert_eq!(validate_path(&g), "OK");
    let mut h = Config::default(); // the override replaces the name
    strcpy(&mut h.valves[0].name, "same");
    strcpy(&mut h.valves[0].topic, "one");
    strcpy(&mut h.valves[1].topic, "same");
    assert_eq!(validate_path(&h), "OK");
    // V2 is reported before V3: valves 2/3 only share an HA id, valves 5/6 a segment.
    let mut k = Config::default();
    strcpy(&mut k.valves[1].name, "Bad 1");
    strcpy(&mut k.valves[2].name, "Bad.1");
    strcpy(&mut k.valves[4].topic, "x");
    strcpy(&mut k.valves[5].name, "x");
    assert_eq!(validate_path(&k), "valves.6.name");
    k.valves[5].name.clear();
    assert_eq!(validate_path(&k), "valves.3.name");
}

#[test]
fn v3_ha_ids_are_unique_among_valves_and_active_slots() {
    let mut c = Config::default();
    strcpy(&mut c.valves[0].name, "Bad 1");
    strcpy(&mut c.valves[1].name, "Bad.1");
    assert_eq!(validate_path(&c), "valves.2.name");
    strcpy(&mut c.valves[1].name, "Bad-1");
    assert_eq!(validate_path(&c), "OK");
    let mut u = Config::default(); // UTF-8 letters map to their base letter
    strcpy(&mut u.valves[3].name, "\u{141}azienka");
    strcpy(&mut u.valves[7].topic, "Lazienka");
    assert_eq!(validate_path(&u), "valves.8.topic");
    let mut o = Config::default();
    strcpy(&mut o.valves[0].topic, "a/b");
    strcpy(&mut o.valves[1].topic, "a.b");
    assert_eq!(validate_path(&o), "valves.2.topic");

    let mut t = Config::default();
    t.temps[0].id = oid(ID_A);
    t.temps[0].active = true;
    strcpy(&mut t.temps[0].name, "Bad 1");
    t.temps[5].id = oid(ID_B);
    t.temps[5].active = true;
    strcpy(&mut t.temps[5].name, "Bad.1");
    assert_eq!(validate_path(&t), "temps.6.name");
    strcpy(&mut t.temps[5].topic, "Bad/1");
    assert_eq!(validate_path(&t), "temps.6.topic");
    t.temps[5].active = false; // only active slots count
    assert_eq!(validate_path(&t), "OK");
    t.temps[5].active = true;
    t.temps[0].active = false;
    assert_eq!(validate_path(&t), "OK");
    // Unnamed slots use their number.
    let mut n = Config::default();
    n.temps[0].id = oid(ID_A);
    n.temps[0].active = true;
    strcpy(&mut n.temps[0].name, "2");
    n.temps[1].id = oid(ID_B);
    n.temps[1].active = true;
    assert_eq!(validate_path(&n), "temps.2.name");

    let mut v = Config::default();
    v.volts[2].id = oid(ID_A);
    v.volts[2].active = true;
    strcpy(&mut v.volts[2].name, "a b");
    v.volts[7].id = oid(ID_V);
    v.volts[7].active = true;
    strcpy(&mut v.volts[7].name, "a.b");
    assert_eq!(validate_path(&v), "volts.8.name");
    strcpy(&mut v.volts[7].topic, "a/b");
    assert_eq!(validate_path(&v), "volts.8.topic");
    v.volts[2].active = false;
    assert_eq!(validate_path(&v), "OK");
    // Different tables never clash.
    let mut m = Config::default();
    strcpy(&mut m.valves[0].name, "x");
    m.temps[0].id = oid(ID_A);
    m.temps[0].active = true;
    strcpy(&mut m.temps[0].name, "x");
    m.volts[0].id = oid(ID_V);
    m.volts[0].active = true;
    strcpy(&mut m.volts[0].name, "x");
    assert_eq!(validate_path(&m), "OK");
}

/// full_config() with the web login set (user "admin", password "pa ss\"w", protectRead)
/// encoded by the 2.0.0 encoder (58632d6): the blob of 2.0.0, and of 2.1 builds that still had
/// the web login.
const GOLDEN_200: [u8; 780] = [
    0x56, 0x44, 0x4d, 0x43, 0x01, 0x00, 0x00, 0x03, 0x0a, 0x48, 0x65, 0x69, 0x7a, 0x75, 0x6e, 0x67,
    0x20, 0x4f, 0x47, 0x02, 0x00, 0xc0, 0xa8, 0x01, 0x32, 0xff, 0xff, 0xff, 0x00, 0xc0, 0xa8, 0x01,
    0x01, 0x08, 0x08, 0x08, 0x08, 0x07, 0x4d, 0x79, 0x20, 0x57, 0x69, 0x66, 0x69, 0x09, 0x73, 0x65,
    0x63, 0x72, 0x65, 0x74, 0x31, 0x32, 0x33, 0x11, 0x0b, 0x31, 0x39, 0x32, 0x2e, 0x31, 0x36, 0x38,
    0x2e, 0x31, 0x2e, 0x31, 0x0d, 0x45, 0x75, 0x72, 0x6f, 0x70, 0x65, 0x2f, 0x57, 0x61, 0x72, 0x73,
    0x61, 0x77, 0x1b, 0x43, 0x45, 0x54, 0x2d, 0x31, 0x43, 0x45, 0x53, 0x54, 0x2c, 0x4d, 0x33, 0x2e,
    0x35, 0x2e, 0x30, 0x2c, 0x4d, 0x31, 0x30, 0x2e, 0x35, 0x2e, 0x30, 0x2f, 0x33, 0x78, 0x03, 0x0a,
    0x00, 0x00, 0x09, 0xea, 0x05, 0x05, 0x61, 0x64, 0x6d, 0x69, 0x6e, 0x07, 0x70, 0x61, 0x20, 0x73,
    0x73, 0x22, 0x77, 0x01, 0x01, 0x0a, 0x62, 0x72, 0x6f, 0x6b, 0x65, 0x72, 0x2e, 0x6c, 0x61, 0x6e,
    0xb3, 0x22, 0x02, 0x6d, 0x71, 0x04, 0x6d, 0x71, 0x70, 0x77, 0x1e, 0x00, 0x78, 0x00, 0x07, 0x00,
    0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x03, 0x42, 0x61, 0x64,
    0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x07, 0x4b, 0x69, 0x74, 0x63, 0x68, 0x65, 0x6e, 0x00, 0x02, 0x74,
    0x31, 0x01, 0xf1, 0xff, 0x28, 0x84, 0x37, 0x94, 0x97, 0xff, 0x03, 0x23, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x64, 0x00,
    0x28, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x01, 0x67, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x80, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80,
    0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x62, 0x61,
    0x74, 0x01, 0x00, 0x00, 0x80, 0xbe, 0x0a, 0xd7, 0x23, 0x3c, 0x01, 0x56, 0x26, 0x11, 0x22, 0x33,
    0x44, 0x55, 0x66, 0x29, 0x7f, 0x17, 0x3b, 0x00, 0xc0, 0xc3, 0x24, 0xb0,
];

/// The web login in GOLDEN_200: user, password (u8 length + bytes), protectRead.
const STORED_LOGIN: &[u8] = b"\x05admin\x07pa ss\"w\x01";

/// `blob` with the one occurrence of `from` replaced by `to`; payload length and CRC fixed.
fn spliced(blob: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let at = blob
        .windows(from.len())
        .position(|w| w == from)
        .expect("found");
    assert!(
        !blob[at + 1..].windows(from.len()).any(|w| w == from),
        "unique"
    );
    let mut b = [&blob[..at], to, &blob[at + from.len()..]].concat();
    let payload = b.len() - 12;
    b[6] = payload as u8;
    b[7] = (payload >> 8) as u8;
    fix_crc(&mut b);
    b
}

/// A stored web login: u8 length + user, u8 length + password, protectRead.
fn login(user: &[u8], password: &[u8], read: u8) -> Vec<u8> {
    let mut v = vec![user.len() as u8];
    v.extend_from_slice(user);
    v.push(password.len() as u8);
    v.extend_from_slice(password);
    v.push(read);
    v
}

#[test]
fn a_stored_web_login_loads_without_a_repair_and_is_dropped() {
    // C-3: the 2.0.0 blob needs no repair; the keys of the ext blob are defaults.
    let mut back = Config::default();
    let mut info = DecodeInfo::default();
    assert_eq!(
        decode_config(&GOLDEN_200, &mut back, Some(&mut info)),
        DecodeResult::Ok
    );
    assert_eq!(info.repairs.mask, 0);
    assert_eq!(info.repairs.count, 0);
    assert!(!info.newer_schema);
    assert!(same_config(&back, &full_config()));
    // Every login the older firmware could store, also a half one it would have repaired and
    // the longest values: read by their layout, nothing kept.
    let u64s = "u".repeat(64);
    let p64 = "p".repeat(64);
    let logins = [
        login(b"", b"", 0),
        login(b"u", b"", 1),
        login(b"", b"p", 0),
        login(b"a:b", b"pw", 1),
        login(u64s.as_bytes(), p64.as_bytes(), 1),
    ];
    for l in &logins {
        let mut o = Config::default();
        assert_eq!(
            decode_config(
                &spliced(&GOLDEN_200, STORED_LOGIN, l),
                &mut o,
                Some(&mut info)
            ),
            DecodeResult::Ok,
            "{}",
            l.len()
        );
        assert_eq!(info.repairs.mask, 0);
        assert!(same_config(&o, &full_config()));
    }
}

#[test]
fn a_damaged_stored_web_login_is_structural_damage_as_in_2_0_0() {
    // A string of 65 bytes does not fit the 2.0.0 field, a flag above 1 is no bool, a NUL
    // inside a string is damage: the whole blob is Invalid.
    let u65 = "u".repeat(65);
    let p65 = "p".repeat(65);
    let damaged = [
        login(u65.as_bytes(), b"p", 0),
        login(b"u", p65.as_bytes(), 0),
        login(b"u", b"p", 2),
        login(b"u\0v", b"p", 0),
        login(b"u", b"p\0q", 0),
    ];
    for l in &damaged {
        let mut o = full_config();
        assert_eq!(
            decode_config(&spliced(&GOLDEN_200, STORED_LOGIN, l), &mut o, None),
            DecodeResult::Invalid,
            "{}",
            l.len()
        );
        assert!(same_config(&o, &Config::default()));
    }
}

#[test]
fn the_cfg_blob_keeps_the_2_0_0_layout_with_a_neutral_web_login() {
    // The login is written as user "", password "" and protectRead false at its 2.0.0 place,
    // so a rollback reads the blob with the login off.
    let neutral = spliced(&GOLDEN_200, STORED_LOGIN, &[0, 0, 0]);
    assert_eq!(encode(&full_config()), neutral);
    // The keys added later do not touch it.
    assert_eq!(encode(&full_config_ext()), neutral);
    // Round trip.
    let mut back = Config::default();
    let mut info = DecodeInfo::default();
    assert_eq!(
        decode_config(&neutral, &mut back, Some(&mut info)),
        DecodeResult::Ok
    );
    assert_eq!(info.repairs.mask, 0);
    assert!(same_config(&back, &full_config()));
    assert_eq!(encode(&back), neutral);
}

#[test]
fn the_keys_of_the_removed_web_login_are_accepted_and_ignored() {
    let before = full_config_ext();
    let long_text = "x".repeat(200);
    let values = [
        vs("admin"),
        vs(""),
        vs("a:b"),
        vs(&long_text),
        vb(true),
        vb(false),
        vi(7),
        vf(1.5),
        vn(),
    ];
    for path in [
        "web.user",
        "web.password",
        "web.passwordSet",
        "web.protectRead",
    ] {
        for v in values {
            let mut c = before.clone();
            assert_eq!(set(&mut c, path, v), SetResult::Ok, "{path} {v:?}");
            assert_eq!(set_clear(&mut c, path, v), SetResult::Ok, "{path} {v:?}");
            assert!(same_config(&c, &before), "{path} {v:?}");
        }
    }
    // Only a secret had an export flag.
    let mut c = before.clone();
    assert_eq!(set(&mut c, "web.userSet", vb(true)), SetResult::UnknownKey);
    assert_eq!(
        set(&mut c, "web.protectReadSet", vb(true)),
        SetResult::UnknownKey
    );
    assert_eq!(
        set(&mut c, "web.passwordSett", vb(true)),
        SetResult::UnknownKey
    );
    assert_eq!(set(&mut c, "web.users", vs("a")), SetResult::UnknownKey);
    // The web object of an older export (also one with the secrets) imports.
    assert_eq!(
        patch(
            &mut c,
            "{\"web\":{\"user\":\"admin\",\"passwordSet\":true,\"protectRead\":true,\
\"allowedHosts\":\"heating.lan\"}}"
        )
        .0,
        PatchResult::Ok
    );
    assert_text(&c.web.allowed_hosts, "heating.lan");
    assert_eq!(
        patch(
            &mut c,
            "{\"web\":{\"user\":\"u\",\"password\":\"pw\"},\"clearSecrets\":true}"
        )
        .0,
        PatchResult::Ok
    );
    let mut expect = before.clone();
    strcpy(&mut expect.web.allowed_hosts, "heating.lan");
    assert!(same_config(&c, &expect));
    // They are never exported.
    let j = export_json(&c);
    assert!(j.contains("\"web\":{\"allowedHosts\":\"heating.lan\"},\"mqtt\":"));
    assert!(!j.contains("protectRead"));
}

pub(super) fn encode_ext(c: &Config, keep: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; CONFIG_EXT_BLOB_MAX];
    let len = encode_config_ext(c, &mut b, keep);
    assert!(len > 0);
    b.truncate(len);
    b
}

/// A cfgx blob from raw records.
pub(super) fn ext_blob(records: &[u8], version: u8) -> Vec<u8> {
    let n = records.len();
    let mut b = vec![b'V', b'D', b'M', b'X', version, n as u8, (n >> 8) as u8];
    b.extend_from_slice(records);
    b.resize(b.len() + 4, 0);
    fix_crc(&mut b);
    b
}

#[test]
fn cfgx_layout_and_round_trip() {
    let full = full_config_ext();
    let x = encode_ext(&full, &[]);
    assert!(x.len() > 11);
    assert_eq!(&x[..5], b"VDMX\x01");
    assert_eq!(usize::from(x[5]) | usize::from(x[6]) << 8, x.len() - 11);
    let crc = crc32(&x[..x.len() - 4], 0);
    assert_eq!(x[x.len() - 4..], crc.to_le_bytes());
    // Table order: web.allowedHosts first, as tag, element, length, bytes.
    let hosts = b"vdmot.lan, 192.168.1.9";
    assert_eq!(x[7], 1);
    assert_eq!(x[8], 0);
    assert_eq!(usize::from(x[9]), hosts.len());
    assert_eq!(&x[10..10 + hosts.len()], hosts);
    // Then mqtt.rootTopic.
    let root = 10 + hosts.len();
    assert_eq!(x[root], 2);
    assert_eq!(x[root + 1], 0);
    assert_eq!(x[root + 2], 8);
    assert_eq!(&x[root + 3..root + 11], b"VdMotFBH");
    // failsafe.timeoutMin 1440 = 0x05A0: tag 5, element 0, length 2, u16 LE.
    let contains = |pat: &[u8]| x.windows(pat.len()).any(|w| w == pat);
    assert!(contains(&[5, 0, 2, 0xA0, 0x05]));
    // valves.12.failsafePct 0 and its topic "x": tags 6 and 7, element 11.
    assert!(contains(&[6, 11, 1, 0, 7, 11, 1, b'x']));

    // Round trip on top of the decoded base blob.
    let mut back = Config::default();
    assert_eq!(
        decode_config(&encode(&full), &mut back, None),
        DecodeResult::Ok
    );
    assert!(!same_config(&back, &full));
    let mut info = ExtInfo {
        bad: 9,
        ..ExtInfo::default()
    };
    assert_eq!(
        decode_config_ext(&x, &mut back, Some(&mut info), &mut []),
        ExtResult::Ok
    );
    assert!(same_config(&back, &full));
    assert_eq!(info.applied, 1 + 3 + 12 * 2 + 34 + 8 + 1);
    assert_eq!(info.unknown, 0);
    assert_eq!(info.bad, 0);
    assert_eq!(info.keep_len, 0);
    // The defaults: every record present, strings also when empty.
    let d = encode_ext(&Config::default(), &[]);
    let mut z = full.clone();
    assert_eq!(decode_config_ext(&d, &mut z, None, &mut []), ExtResult::Ok);
    let mut expect = full.clone();
    let defaults = Config::default();
    expect.web.allowed_hosts = defaults.web.allowed_hosts.clone();
    expect.mqtt.root_topic = defaults.mqtt.root_topic.clone();
    expect.mqtt.client_id = defaults.mqtt.client_id.clone();
    expect.mqtt.discovery_prefix = defaults.mqtt.discovery_prefix.clone();
    expect.failsafe = defaults.failsafe.clone();
    for v in &mut expect.valves {
        v.failsafe_pct = FAILSAFE_PCT_DEFAULT;
        v.topic.clear();
    }
    for t in &mut expect.temps {
        t.topic.clear();
    }
    for v in &mut expect.volts {
        v.topic.clear();
    }
    assert!(same_config(&z, &expect));
}

#[test]
fn cfgx_encode_needs_the_full_capacity() {
    let full = full_config_ext();
    let x = encode_ext(&full, &[]);
    let mut buf = vec![0u8; x.len()];
    for cap in 0..x.len() {
        assert_eq!(encode_config_ext(&full, &mut buf[..cap], &[]), 0, "{cap}");
    }
    assert_eq!(encode_config_ext(&full, &mut buf, &[]), x.len());
    assert_eq!(buf, x);
    // C++ encodeConfigExt(full, nullptr, kConfigExtBlobMax): no Rust form.
    // The largest possible blob plus the kept records fits CONFIG_EXT_BLOB_MAX.
    let mut big = Config::default();
    strcpy(&mut big.web.allowed_hosts, &"a".repeat(ALLOWED_HOSTS_MAX));
    strcpy(&mut big.mqtt.root_topic, &"r".repeat(STATION_NAME_MAX));
    strcpy(&mut big.mqtt.client_id, &"c".repeat(CLIENT_ID_MAX));
    strcpy(
        &mut big.mqtt.discovery_prefix,
        &"p".repeat(TOPIC_PREFIX_MAX),
    );
    let t = "t".repeat(ITEM_NAME_MAX);
    for v in &mut big.valves {
        strcpy(&mut v.topic, &t);
    }
    for s in &mut big.temps {
        strcpy(&mut s.topic, &t);
    }
    for v in &mut big.volts {
        strcpy(&mut v.topic, &t);
    }
    let keep = vec![0u8; CONFIG_EXT_KEEP_MAX];
    assert!(encode_ext(&big, &keep).len() <= CONFIG_EXT_BLOB_MAX);
}

#[test]
fn cfgx_unknown_records_are_kept_and_written_back() {
    // Known record, unknown tag 200, known record.
    let recs: [u8; 15] = [
        5, 0, 2, 10, 0, // failsafe.timeoutMin 10
        200, 0, 3, b'a', b'b', b'c', // unknown
        6, 2, 1, 70, // valves.3.failsafePct 70
    ];
    let b = ext_blob(&recs, 1);
    let mut c = Config::default();
    let mut info = ExtInfo::default();
    let mut keep = [0u8; CONFIG_EXT_KEEP_MAX];
    assert_eq!(
        decode_config_ext(&b, &mut c, Some(&mut info), &mut keep),
        ExtResult::Ok
    );
    assert_eq!(info.applied, 2);
    assert_eq!(info.unknown, 1);
    assert_eq!(info.bad, 0);
    assert_eq!(info.keep_len, 6);
    assert_eq!(&keep[..6], &recs[5..11]);
    assert_eq!(c.failsafe.timeout_min, 10);
    assert_eq!(c.valves[2].failsafe_pct, 70);
    // Re-emitted verbatim after the own records.
    let x = encode_ext(&c, &keep[..info.keep_len]);
    assert_eq!(&x[x.len() - 10..x.len() - 4], &recs[5..11]);
    let mut again = Config::default();
    let mut i2 = ExtInfo::default();
    let mut keep2 = [0u8; CONFIG_EXT_KEEP_MAX];
    assert_eq!(
        decode_config_ext(&x, &mut again, Some(&mut i2), &mut keep2),
        ExtResult::Ok
    );
    assert_eq!(i2.unknown, 1);
    assert_eq!(i2.keep_len, 6);
    assert!(same_config(&again, &c));
    // Without a keep buffer, or one that is too small, the record is only counted.
    let mut i3 = ExtInfo::default();
    assert_eq!(
        decode_config_ext(&b, &mut again, Some(&mut i3), &mut []),
        ExtResult::Ok
    );
    assert_eq!(i3.unknown, 1);
    assert_eq!(i3.keep_len, 0);
    let mut tiny = [0u8; 5];
    assert_eq!(
        decode_config_ext(&b, &mut again, Some(&mut i3), &mut tiny),
        ExtResult::Ok
    );
    assert_eq!(i3.keep_len, 0);
    let mut exact = [0u8; 6];
    assert_eq!(
        decode_config_ext(&b, &mut again, Some(&mut i3), &mut exact),
        ExtResult::Ok
    );
    assert_eq!(i3.keep_len, 6);
    // At most CONFIG_EXT_KEEP_MAX bytes are kept, whole records only.
    let many: Vec<u8> = (0..70u8).flat_map(|k| [201, 0, 5, 1, 2, 3, 4, k]).collect();
    let mut big = vec![0u8; 1024];
    let mut i4 = ExtInfo::default();
    assert_eq!(
        decode_config_ext(&ext_blob(&many, 1), &mut again, Some(&mut i4), &mut big),
        ExtResult::Ok
    );
    assert_eq!(i4.unknown, 70);
    assert_eq!(i4.keep_len, 512);
    assert_eq!(big[511], 63);
    assert_eq!(big[512], 0);
    let mut i5 = ExtInfo::default();
    assert_eq!(
        decode_config_ext(
            &ext_blob(&many, 1),
            &mut again,
            Some(&mut i5),
            &mut big[..511]
        ),
        ExtResult::Ok
    );
    assert_eq!(i5.keep_len, 504);
}

#[test]
fn cfgx_bad_records_change_nothing() {
    let cases: [(&[u8], &str); 18] = [
        (&[6, 12, 1, 40], "valve element 12"),
        (&[6, 255, 1, 40], "valve element 255"),
        (&[5, 1, 2, 10, 0], "object element 1"),
        (&[6, 0, 2, 40, 0], "pct with 2 bytes"),
        (&[6, 0, 0], "pct with 0 bytes"),
        (&[6, 0, 1, 101], "pct 101"),
        (&[6, 0, 1, 254], "pct 254"),
        (&[5, 0, 1, 10], "timeout with 1 byte"),
        (&[5, 0, 3, 10, 0, 0], "timeout with 3 bytes"),
        (&[5, 0, 2, 4, 0], "timeout 4"),
        (&[5, 0, 2, 0xA1, 0x05], "timeout 1441"),
        (b"\x07\x00\x0baaaaaaaaaaa", "topic of 11"),
        (&[7, 0, 3, b'a', 0, b'b'], "NUL inside"),
        (&[7, 0, 3, b'a', b' ', b'b'], "topic rule"),
        (&[4, 0, 0], "empty discovery prefix"),
        (&[1, 0, 4, b'a', b',', b',', b'b'], "host list rule"),
        (&[8, 34, 1, b't'], "temp element 34"),
        (&[9, 8, 1, b'v'], "volt element 8"),
    ];
    for (rec, why) in cases {
        let mut c = Config::default();
        let mut info = ExtInfo::default();
        assert_eq!(
            decode_config_ext(&ext_blob(rec, 1), &mut c, Some(&mut info), &mut []),
            ExtResult::Ok,
            "{why}"
        );
        assert_eq!(info.bad, 1, "{why}");
        assert_eq!(info.applied, 0, "{why}");
        assert_eq!(info.unknown, 0, "{why}");
        assert!(same_config(&c, &Config::default()), "{why}");
    }
    // The next record still applies; the last record for a field wins.
    let mut c = Config::default();
    let mut info = ExtInfo::default();
    assert_eq!(
        decode_config_ext(
            &ext_blob(
                &[6, 12, 1, 40, 6, 11, 1, 41, 6, 11, 1, 42, 5, 0, 2, 0, 0],
                1
            ),
            &mut c,
            Some(&mut info),
            &mut []
        ),
        ExtResult::Ok
    );
    assert_eq!(info.bad, 1);
    assert_eq!(info.applied, 3);
    assert_eq!(c.valves[11].failsafe_pct, 42);
    assert_eq!(c.failsafe.timeout_min, 0);
    // Longest valid string and the edge values.
    let mut e = Config::default();
    let recs = [
        &b"\x07\x00\x0aabcdefghij"[..],
        &[6, 0, 1, 255],
        &[5, 0, 2, 0xA0, 0x05],
        &[8, 33, 1, b't'],
        &[9, 7, 1, b'v'],
    ]
    .concat();
    assert_eq!(
        decode_config_ext(&ext_blob(&recs, 1), &mut e, Some(&mut info), &mut []),
        ExtResult::Ok
    );
    assert_eq!(info.applied, 5);
    assert_text(&e.valves[0].topic, "abcdefghij");
    assert_eq!(e.valves[0].failsafe_pct, FAILSAFE_HOLD);
    assert_eq!(e.failsafe.timeout_min, 1440);
    assert_text(&e.temps[33].topic, "t");
    assert_text(&e.volts[7].topic, "v");
    // A shorter string replaces a longer one completely.
    assert_eq!(
        decode_config_ext(
            &ext_blob(&[7, 0, 2, b'x', b'y'], 1),
            &mut e,
            Some(&mut info),
            &mut []
        ),
        ExtResult::Ok
    );
    assert_text(&e.valves[0].topic, "xy");
}

#[test]
fn cfgx_damage_applies_nothing() {
    let full = full_config_ext();
    let good = encode_ext(&full, &[]);
    let expect = |b: &[u8], r: ExtResult| {
        let mut c = Config::default();
        let mut info = ExtInfo {
            applied: 7,
            ..ExtInfo::default()
        };
        assert_eq!(decode_config_ext(b, &mut c, Some(&mut info), &mut []), r);
        assert!(same_config(&c, &Config::default()));
        assert_eq!(info.applied, 0);
    };
    let mut c = Config::default();
    let mut info = ExtInfo {
        unknown: 3,
        ..ExtInfo::default()
    };
    // C++ decodeConfigExt(nullptr, 20, ..): no Rust form; the empty slice is Absent.
    assert_eq!(
        decode_config_ext(&good[..0], &mut c, Some(&mut info), &mut []),
        ExtResult::Absent
    );
    assert_eq!(info.unknown, 0);
    assert_eq!(
        decode_config_ext(&good, &mut c, None, &mut []),
        ExtResult::Ok
    ); // info optional
    for n in 1..11 {
        expect(&good[..n], ExtResult::TooShort);
    }
    expect(&good[..good.len() - 1], ExtResult::TooShort);
    {
        let mut b = good.clone(); // L one too large
        let l = good.len() - 11 + 1;
        b[5] = l as u8;
        b[6] = (l >> 8) as u8;
        expect(&b, ExtResult::TooShort);
    }
    for k in 0..4 {
        let mut b = good.clone();
        b[k] ^= 0x20;
        expect(&b, ExtResult::BadMagic);
    }
    {
        let mut b = good.clone();
        b[20] ^= 1;
        expect(&b, ExtResult::BadCrc);
        b = good.clone();
        *b.last_mut().expect("bytes") ^= 0x80;
        expect(&b, ExtResult::BadCrc);
        b = good.clone();
        b[good.len() - 4] ^= 0x01;
        expect(&b, ExtResult::BadCrc);
    }
    // Empty payload: header and CRC only.
    assert_eq!(
        decode_config_ext(&ext_blob(&[], 1), &mut c, Some(&mut info), &mut []),
        ExtResult::Ok
    );
    assert_eq!(info.applied, 0);
    // A newer format version is read like version 1; bytes after the CRC are ignored.
    let mut v2 = Config::default();
    assert_eq!(
        decode_config_ext(&ext_blob(&[5, 0, 2, 30, 0], 2), &mut v2, None, &mut []),
        ExtResult::Ok
    );
    assert_eq!(v2.failsafe.timeout_min, 30);
    let mut tail = ext_blob(&[5, 0, 2, 31, 0], 1);
    tail.push(0xEE);
    assert_eq!(
        decode_config_ext(&tail, &mut v2, None, &mut []),
        ExtResult::Ok
    );
    assert_eq!(v2.failsafe.timeout_min, 31);
    // A record cut by the payload end: counted as bad, the ones before it apply.
    let cuts: [&[u8]; 4] = [
        &[5, 0, 2, 32, 0, 6],
        &[5, 0, 2, 32, 0, 6, 0],
        &[5, 0, 2, 32, 0, 6, 0, 1],
        &[5, 0, 2, 32, 0, 7, 0, 3, b'a', b'b'],
    ];
    for cut in cuts {
        let mut k = Config::default();
        let mut ki = ExtInfo::default();
        assert_eq!(
            decode_config_ext(&ext_blob(cut, 1), &mut k, Some(&mut ki), &mut []),
            ExtResult::Ok
        );
        assert_eq!(ki.applied, 1, "{}", cut.len());
        assert_eq!(ki.bad, 1, "{}", cut.len());
        assert_eq!(k.failsafe.timeout_min, 32);
    }
    // The same with an unknown tag: a cut record is never kept.
    let cuts: [&[u8]; 4] = [
        &[5, 0, 2, 32, 0, 200],
        &[5, 0, 2, 32, 0, 200, 0],
        &[5, 0, 2, 32, 0, 200, 0, 1],
        &[5, 0, 2, 32, 0, 200, 0, 3, b'a', b'b'],
    ];
    for cut in cuts {
        let mut k = Config::default();
        let mut ki = ExtInfo::default();
        let mut keep = [0u8; CONFIG_EXT_KEEP_MAX];
        assert_eq!(
            decode_config_ext(&ext_blob(cut, 1), &mut k, Some(&mut ki), &mut keep),
            ExtResult::Ok
        );
        assert_eq!(ki.applied, 1, "{}", cut.len());
        assert_eq!(ki.bad, 1, "{}", cut.len());
        assert_eq!(ki.unknown, 0, "{}", cut.len());
        assert_eq!(ki.keep_len, 0, "{}", cut.len());
    }
    // A record that ends exactly at the payload end is whole.
    let mut z = Config::default();
    let mut zi = ExtInfo::default();
    strcpy(&mut z.mqtt.root_topic, "x");
    assert_eq!(
        decode_config_ext(
            &ext_blob(&[5, 0, 2, 33, 0, 2, 0, 0], 1),
            &mut z,
            Some(&mut zi),
            &mut []
        ),
        ExtResult::Ok
    );
    assert_eq!(zi.applied, 2);
    assert_eq!(zi.bad, 0);
    assert!(z.mqtt.root_topic.is_empty());
    let mut keep = [0u8; CONFIG_EXT_KEEP_MAX];
    assert_eq!(
        decode_config_ext(
            &ext_blob(&[5, 0, 2, 33, 0, 200, 0, 0], 1),
            &mut z,
            Some(&mut zi),
            &mut keep
        ),
        ExtResult::Ok
    );
    assert_eq!(zi.applied, 1);
    assert_eq!(zi.unknown, 1);
    assert_eq!(zi.bad, 0);
    assert_eq!(zi.keep_len, 3);
}

#[test]
fn restart_reasons() {
    let rows: [(&str, Edit, u8); 15] = [
        (
            "reconnectTimeoutMin",
            |c| c.net.reconnect_timeout_min = 0,
            0,
        ),
        ("dns with DHCP", |c| c.net.dns = 0x0808_0808, 0),
        ("ip with DHCP", |c| c.net.ip = 0x0101_A8C0, 0),
        ("mask with DHCP", |c| c.net.mask = 0x00FF_FFFF, 0),
        ("gateway with DHCP", |c| c.net.gateway = 0x0501_A8C0, 0),
        (
            "iface",
            |c| c.net.iface = NetInterface::Wifi,
            RESTART_NETWORK,
        ),
        ("dhcp", |c| c.net.dhcp = false, RESTART_NETWORK),
        (
            "ssid with Auto",
            |c| strcpy(&mut c.net.ssid, "w"),
            RESTART_NETWORK,
        ),
        (
            "wifiPassword with Auto",
            |c| strcpy(&mut c.net.wifi_password, "12345678"),
            RESTART_NETWORK,
        ),
        (
            "station Dom 1 -> Dom  1",
            |c| strcpy(&mut c.station, "Dom  1"),
            0,
        ),
        (
            "station Dom 1 -> Dom_1",
            |c| strcpy(&mut c.station, "Dom_1"),
            RESTART_HOSTNAME,
        ),
        ("mqtt.host", |c| strcpy(&mut c.mqtt.host, "b"), 0),
        ("calib", |c| c.calib.hour = 3, 0),
        ("failsafe", |c| c.failsafe.timeout_min = 0, 0),
        (
            "both",
            |c| {
                c.net.dhcp = false;
                strcpy(&mut c.station, "Dom_1");
            },
            RESTART_NETWORK | RESTART_HOSTNAME,
        ),
    ];
    let mut before = Config::default();
    strcpy(&mut before.station, "Dom 1"); // host name "Dom-1"
    for (what, edit, reasons) in rows {
        let mut after = before.clone();
        edit(&mut after);
        assert_eq!(config_restart_reasons(&before, &after), reasons, "{what}");
        assert_eq!(config_restart_reasons(&after, &before), reasons, "{what}");
        assert_eq!(config_restart_reasons(&after, &after), 0, "{what}");
        // The parts net keeps give the same answer.
        assert_eq!(
            config_restart_reasons_from(&before.net, &before.station, &after),
            reasons,
            "{what}"
        );
        assert_eq!(
            config_restart_reasons_from(&after.net, &after.station, &before),
            reasons,
            "{what}"
        );
        assert_eq!(
            config_restart_reasons_from(&after.net, &after.station, &after),
            0,
            "{what}"
        );
    }
    // Static addresses: every field counts, a dns of 0.0.0.0 means the gateway.
    let mut st = Config::default();
    st.net.dhcp = false;
    st.net.ip = 0x3201_A8C0;
    st.net.mask = 0x00FF_FFFF;
    st.net.gateway = 0x0101_A8C0;
    let statics: [(&str, Edit, u8); 4] = [
        (
            "dns with static",
            |c| c.net.dns = 0x0808_0808,
            RESTART_NETWORK,
        ),
        ("dns = gateway with static", |c| c.net.dns = 0x0101_A8C0, 0),
        (
            "gateway with static and dns 0.0.0.0",
            |c| c.net.gateway = 0xFE01_A8C0,
            RESTART_NETWORK,
        ),
        (
            "ip with static",
            |c| c.net.ip = 0x3301_A8C0,
            RESTART_NETWORK,
        ),
    ];
    for (what, edit, reasons) in statics {
        let mut after = st.clone();
        edit(&mut after);
        assert_eq!(config_restart_reasons(&st, &after), reasons, "{what}");
    }
    // WiFi credentials do not matter with Ethernet before and after.
    let mut eth = st.clone();
    eth.net.iface = NetInterface::Ethernet;
    let mut eth_wifi = eth.clone();
    strcpy(&mut eth_wifi.net.ssid, "w");
    strcpy(&mut eth_wifi.net.wifi_password, "12345678");
    assert_eq!(config_restart_reasons(&eth, &eth_wifi), 0);
    // Host names "K-che" and "Kuche".
    let mut k1 = Config::default();
    let mut k2 = Config::default();
    strcpy(&mut k1.station, "K\u{fc}che");
    strcpy(&mut k2.station, "Kuche");
    assert_eq!(config_restart_reasons(&k1, &k2), RESTART_HOSTNAME);
    // Station names of full length that differ in their last char.
    strcpy(&mut k1.station, "abcdefghijklmnopqrst");
    strcpy(&mut k2.station, "abcdefghijklmnopqrsu");
    assert_eq!(config_restart_reasons(&k1, &k2), RESTART_HOSTNAME);
}

type NetEdit = fn(&mut NetConfig);

#[test]
fn network_trial_rule() {
    let dhcp = NetConfig::default();
    let st = NetConfig {
        dhcp: false,
        ip: 0x3201_A8C0,
        mask: 0x00FF_FFFF,
        gateway: 0x0101_A8C0,
        ..NetConfig::default()
    };
    assert!(!net_trial_required(&st, &st));
    assert!(!net_trial_required(&dhcp, &dhcp));
    assert!(net_trial_required(&dhcp, &st));
    assert!(net_trial_required(&st, &dhcp));
    let rows: [(&str, NetEdit, bool); 8] = [
        ("ip", |n| n.ip = 0x3301_A8C0, true),
        ("mask", |n| n.mask = 0x0000_FFFF, true),
        ("gateway", |n| n.gateway = 0xFE01_A8C0, true),
        ("dns", |n| n.dns = 0x0808_0808, true),
        ("dns = gateway", |n| n.dns = 0x0101_A8C0, false),
        ("reconnect", |n| n.reconnect_timeout_min = 9, false),
        ("ssid", |n| strcpy(&mut n.ssid, "w"), true),
        (
            "password",
            |n| strcpy(&mut n.wifi_password, "12345678"),
            true,
        ),
    ];
    for (what, edit, trial) in rows {
        let mut after = st.clone();
        edit(&mut after);
        assert_eq!(net_trial_required(&st, &after), trial, "{what}");
        assert_eq!(net_trial_required(&after, &st), trial, "{what}");
    }
    // Static fields do not matter with DHCP after the change.
    let mut d2 = dhcp.clone();
    d2.ip = 5;
    d2.dns = 7;
    assert!(!net_trial_required(&dhcp, &d2));
    // WiFi credentials do not matter when Ethernet is used before and after.
    let mut e1 = NetConfig {
        iface: NetInterface::Ethernet,
        ..NetConfig::default()
    };
    let mut e2 = e1.clone();
    strcpy(&mut e2.ssid, "w");
    strcpy(&mut e2.wifi_password, "12345678");
    assert!(!net_trial_required(&e1, &e2));
    e1.iface = NetInterface::Wifi;
    e2.iface = NetInterface::Wifi;
    assert!(net_trial_required(&e1, &e2));
    strcpy(&mut e1.ssid, "w");
    assert!(net_trial_required(&e1, &e2));
    strcpy(&mut e1.wifi_password, "12345678");
    assert!(!net_trial_required(&e1, &e2));
    // C++ "bytes after the NUL are not part of the text": no Rust form (a text member has no
    // bytes after its end).
}

#[test]
fn effective_dns_mqtt_root_topic_and_item_segment() {
    let mut n = NetConfig {
        gateway: 0x0101_A8C0,
        ..NetConfig::default()
    };
    assert_eq!(effective_dns(&n), 0); // DHCP: the configured dns
    n.dns = 0x0808_0808;
    assert_eq!(effective_dns(&n), 0x0808_0808);
    n.dhcp = false;
    assert_eq!(effective_dns(&n), 0x0808_0808);
    n.dns = 0;
    assert_eq!(effective_dns(&n), 0x0101_A8C0); // static without dns: the gateway

    let mut c = Config::default();
    assert!(core::ptr::eq(mqtt_root_topic(&c), c.station.as_slice()));
    strcpy(&mut c.mqtt.root_topic, "VdMotFBH");
    assert_text(mqtt_root_topic(&c), "VdMotFBH");
    strcpy(&mut c.mqtt.root_topic, "R");
    assert_text(mqtt_root_topic(&c), "R");

    let mut out = [b'x'; 11];
    let mut seg = |c: &Config, kind: ItemKind, idx: u8, cap: usize, want: &str| {
        let n = item_segment(c, kind, idx, &mut out[..cap]);
        assert_text(&out[..n], want);
        n
    };
    assert_eq!(seg(&c, ItemKind::Valve, 2, 11, "3"), 1);
    assert_eq!(seg(&c, ItemKind::Valve, 11, 11, "12"), 2);
    assert_eq!(seg(&c, ItemKind::Temp, 33, 11, "34"), 2);
    assert_eq!(seg(&c, ItemKind::Volt, 7, 11, "8"), 1);
    strcpy(&mut c.valves[2].name, "Bad OG 1");
    assert_eq!(seg(&c, ItemKind::Valve, 2, 11, "Bad_OG_1"), 8);
    strcpy(&mut c.valves[2].topic, "Bad/WC");
    assert_eq!(seg(&c, ItemKind::Valve, 2, 11, "Bad/WC"), 6);
    strcpy(&mut c.temps[0].name, "t 1");
    assert_eq!(seg(&c, ItemKind::Temp, 0, 11, "t_1"), 3);
    strcpy(&mut c.temps[0].topic, "t/1");
    assert_eq!(seg(&c, ItemKind::Temp, 0, 11, "t/1"), 3);
    strcpy(&mut c.volts[7].name, "bat");
    assert_eq!(seg(&c, ItemKind::Volt, 7, 11, "bat"), 3);
    strcpy(&mut c.volts[7].topic, "v/8");
    assert_eq!(seg(&c, ItemKind::Volt, 7, 11, "v/8"), 3);
    strcpy(&mut c.valves[0].name, "abcdefghij");
    assert_eq!(seg(&c, ItemKind::Valve, 0, 11, "abcdefghij"), 10); // exactly fits
                                                                   // Too small, out of range, no buffer.
    assert_eq!(seg(&c, ItemKind::Valve, 0, 10, ""), 0);
    assert_eq!(seg(&c, ItemKind::Valve, 11, 2, ""), 0);
    assert_eq!(seg(&c, ItemKind::Valve, 11, 3, "12"), 2);
    for (kind, idx) in [
        (ItemKind::Valve, 12),
        (ItemKind::Temp, 34),
        (ItemKind::Volt, 8),
    ] {
        assert_eq!(seg(&c, kind, idx, 11, ""), 0);
    }
    // C++ ItemKind 3 and a null output: no Rust form.
    out[0] = b'x';
    assert_eq!(item_segment(&c, ItemKind::Valve, 0, &mut out[..0]), 0);
    assert_eq!(out[0], b'x');
}

#[test]
fn what_changes_the_mqtt_topics() {
    let base = full_config_ext();
    assert!(!mqtt_topic_config_changed(&base, &base));
    let rows: [(&str, Edit, bool); 32] = [
        ("station", |c| strcpy(&mut c.station, "Other"), true),
        ("mqtt.mode", |c| c.mqtt.mode = MqttMode::Off, true),
        ("mqtt.host", |c| strcpy(&mut c.mqtt.host, "other"), true),
        ("mqtt.port", |c| c.mqtt.port = 1, true),
        ("mqtt.password", |c| strcpy(&mut c.mqtt.password, "x"), true),
        ("mqtt.keepAliveS", |c| c.mqtt.keep_alive_s = 60, true),
        ("mqtt.separate", |c| c.mqtt.separate = true, true),
        (
            "mqtt.haDiscoveryOnConnect",
            |c| c.mqtt.ha_discovery_on_connect = true,
            true,
        ),
        (
            "mqtt.rootTopic",
            |c| strcpy(&mut c.mqtt.root_topic, "R"),
            true,
        ),
        (
            "mqtt.clientId",
            |c| strcpy(&mut c.mqtt.client_id, "c"),
            true,
        ),
        (
            "mqtt.discoveryPrefix",
            |c| strcpy(&mut c.mqtt.discovery_prefix, "hb"),
            true,
        ),
        ("valve name", |c| strcpy(&mut c.valves[11].name, "K"), true),
        ("valve active", |c| c.valves[11].active = true, true),
        (
            "valve topic",
            |c| strcpy(&mut c.valves[11].topic, "y"),
            true,
        ),
        ("temp name", |c| strcpy(&mut c.temps[33].name, "n"), true),
        ("temp active", |c| c.temps[33].active = true, true),
        ("temp topic", |c| strcpy(&mut c.temps[33].topic, "z"), true),
        ("temp id", |c| c.temps[33].id.b[7] ^= 1, true),
        ("volt name", |c| strcpy(&mut c.volts[0].name, "n"), true),
        ("volt active", |c| c.volts[7].active = false, true),
        ("volt topic", |c| strcpy(&mut c.volts[7].topic, "b2"), true),
        ("volt id", |c| c.volts[7].id.b[0] ^= 1, true),
        ("failsafe.timeoutMin", |c| c.failsafe.timeout_min = 5, false),
        ("valve failsafePct", |c| c.valves[0].failsafe_pct = 7, false),
        ("temp offset", |c| c.temps[0].offset = 3, false),
        ("volt offset", |c| c.volts[7].offset = 3.0, false),
        ("volt factor", |c| c.volts[7].factor = 3.0, false),
        ("volt unit", |c| strcpy(&mut c.volts[7].unit, "A"), false),
        ("web", |c| strcpy(&mut c.web.allowed_hosts, "h"), false),
        ("net", |c| c.net.reconnect_timeout_min = 1, false),
        ("calib", |c| c.calib.hour = 1, false),
        ("persistLog", |c| c.persist_log = true, false),
        // C++ "after the NUL" (a byte past the host's NUL): no Rust form.
    ];
    for (what, edit, changed) in rows {
        let mut after = base.clone();
        edit(&mut after);
        assert_eq!(mqtt_topic_config_changed(&base, &after), changed, "{what}");
        assert_eq!(mqtt_topic_config_changed(&after, &base), changed, "{what}");
    }
}

#[test]
fn json_export_without_secrets_and_the_apply_members() {
    let mut c = Config::default();
    strcpy(&mut c.net.wifi_password, "w\"1");
    strcpy(&mut c.mqtt.password, "pw");
    let flags = export_json(&c);
    assert!(flags.contains("\"wifiPasswordSet\":true"));
    assert!(flags.contains("\"user\":\"\",\"passwordSet\":true,\"keepAliveS\":60"));
    assert!(!flags.contains("pw"));
    assert!(!flags.contains("w\\\"1"));

    let mut buf = vec![0u8; 8192];
    let mut apply = ApplyInfo {
        restart_required: true,
        ..ApplyInfo::default()
    };
    let mut ja = JsonWriter::new(&mut buf);
    assert!(write_config_json(&mut ja, &c, Some(&apply)));
    let a = String::from_utf8_lossy(ja.as_bytes()).into_owned();
    assert!(a.ends_with("\"persistLog\":true,\"restartRequired\":true,\"netTrial\":false}"));
    apply.restart_required = false;
    apply.net_trial = true;
    let mut jb = JsonWriter::new(&mut buf);
    assert!(write_config_json(&mut jb, &c, Some(&apply)));
    let b = String::from_utf8_lossy(jb.as_bytes()).into_owned();
    assert!(b.ends_with("\"persistLog\":true,\"restartRequired\":false,\"netTrial\":true}"));
    // The apply members are not config keys.
    let mut back = Config::default();
    assert_eq!(
        patch(&mut back, &a),
        (PatchResult::UnknownKey, "restartRequired".to_string())
    );
}

#[test]
fn export_and_patch_round_trip_with_every_key() {
    let full = full_config_ext();
    // The export carries no secret: they are posted along.
    let mut doc = export_json(&full);
    doc.insert_str(
        1,
        "\"net\":{\"wifiPassword\":\"secret123\"},\"mqtt\":{\"password\":\"mqpw\"},",
    );
    let mut c = Config::default();
    assert_eq!(patch(&mut c, &doc), (PatchResult::Ok, String::new()));
    assert!(same_config(&c, &full));
}

#[test]
fn decode_reports_the_header_schema_load_config_blobs_reads_base_then_ext() {
    let base = encode(&full_config_ext());
    let mut info = DecodeInfo {
        newer_schema: true,
        ..DecodeInfo::default()
    };
    info.repairs.count = 3;
    let mut d = Config::default();
    assert_eq!(
        decode_config(&base, &mut d, Some(&mut info)),
        DecodeResult::Ok
    );
    assert_eq!(info.schema, 1);
    assert!(!info.newer_schema);
    assert_eq!(info.repairs.count, 0);
    let mut magic = base.clone();
    magic[0] = b'X';
    assert_eq!(
        decode_config(&magic, &mut d, Some(&mut info)),
        DecodeResult::BadMagic
    );
    assert_eq!(info.schema, 0);
    assert_eq!(
        decode_config(&[], &mut d, Some(&mut info)),
        DecodeResult::TooShort
    );

    let ext = encode_ext(&full_config_ext(), &[]);
    let mut b = StoredBlobs {
        base: &base,
        ext: &ext,
    };
    let mut li = LoadInfo::default();
    let mut out = Config::default();
    assert!(load_config_blobs(&b, &mut out, &mut li, &mut []));
    assert_eq!(li.base, DecodeResult::Ok);
    assert_eq!(li.ext, ExtResult::Ok);
    assert!(li.ext_info.applied > 0);
    assert_eq!(li.decode.schema, 1);
    assert_eq!(li.repairs.mask, 0);
    assert!(same_config(&out, &full_config_ext()));
    // A damaged ext blob leaves the defaults of its keys (C-10).
    let mut bad_ext = ext.clone();
    bad_ext[9] ^= 1;
    b.ext = &bad_ext;
    assert!(load_config_blobs(&b, &mut out, &mut li, &mut []));
    assert_eq!(li.ext, ExtResult::BadCrc);
    assert!(same_config(&out, &full_config()));
    // Unknown records go to `keep`.
    let unknown = ext_blob(&[210, 0, 1, 9], 1);
    b.ext = &unknown;
    let mut keep = [0u8; 16];
    assert!(load_config_blobs(&b, &mut out, &mut li, &mut keep));
    assert_eq!(li.ext_info.unknown, 1);
    assert_eq!(li.ext_info.keep_len, 4);
    assert_eq!(keep[0], 210);
    // An unusable base blob: false, defaults.
    let mut bad_base = base.clone();
    bad_base[9] ^= 1;
    b.base = &bad_base;
    assert!(!load_config_blobs(&b, &mut out, &mut li, &mut []));
    assert_eq!(li.base, DecodeResult::BadCrc);
    assert_eq!(li.ext, ExtResult::Absent);
    assert!(same_config(&out, &Config::default()));
}

#[test]
fn a_stored_config_that_breaks_v1_v3_or_a_2_0_0_rule_is_repaired() {
    let mut ha = Config::default();
    ha.mqtt.mode = MqttMode::MqttHa;
    strcpy(&mut ha.mqtt.host, "b");
    ha.mqtt.german_decimal = true;
    strcpy(&mut ha.valves[0].name, "Bad 1");
    strcpy(&mut ha.valves[1].name, "Bad.1");
    assert_eq!(validate_path(&ha), "mqtt.germanDecimal");
    let blob = encode(&ha);
    let mut back = Config::default();
    let mut info = DecodeInfo::default();
    assert_eq!(
        decode_config(&blob, &mut back, Some(&mut info)),
        DecodeResult::Ok
    );
    assert_eq!(info.repairs.mask, REPAIR_HA_DECIMAL | REPAIR_HA_IDS);
    assert_eq!(info.repairs.count, 2);
    assert_text(&info.repairs.first, "mqtt.germanDecimal");
    assert_eq!(info.repairs.valve_names, 2);
    let mut expect = ha.clone();
    expect.mqtt.german_decimal = false;
    expect.valves[1].name.clear();
    assert!(same_config(&back, &expect));
    assert_eq!(validate_path(&back), "OK");
    // A 2.0.0 rule is repaired as well.
    ha.mqtt.german_decimal = false;
    ha.mqtt.separate = false;
    let broken = encode(&ha);
    assert_eq!(
        decode_config(&broken, &mut back, Some(&mut info)),
        DecodeResult::Ok
    );
    assert_eq!(back.mqtt.mode, MqttMode::Mqtt);
    assert_eq!(info.repairs.mask, REPAIR_HA_SEPARATE | REPAIR_HA_IDS);
}

// ---------------------------------------------------------------- Rust-only cases

#[test]
fn string_field_limits() {
    // The C++ compile-time check fits(): max < cap, cap within the scratch buffers (the size of
    // web.allowedHosts), shorter than what the patch reader keeps; and here also: the table
    // size of every stored string is its member's capacity + 1.
    for &g in &GROUPS {
        for (fi, f) in g.fields.iter().enumerate() {
            if !matches!(f.kind, Kind::Str | Kind::Secret) {
                continue;
            }
            assert!(f.max < i32::from(f.cap), "{}", f.name);
            assert!(usize::from(f.cap) <= ALLOWED_HOSTS_MAX + 1, "{}", f.name);
            assert!((f.max as usize) < PATCH_STR_MAX, "{}", f.name);
            if f.retired {
                continue;
            }
            let mut c = Config::default();
            let long = [b'q'; 100];
            (g.set)(&mut c, 0, fi, Val::Text(&long));
            let stored = match (g.get)(&c, 0, fi) {
                Some(Val::Text(s)) => s.len(),
                _ => 0,
            };
            assert_eq!(stored + 1, usize::from(f.cap), "{}", f.name);
        }
    }
}

#[test]
fn the_path_is_a_c_string() {
    let mut c = Config::default();
    assert_eq!(
        set_config_value(&mut c, b"calib.hour\0junk", &vi(5), false),
        SetResult::Ok
    );
    assert_eq!(c.calib.hour, 5);
    assert_eq!(
        set_config_value(&mut c, b"\0calib.hour", &vi(6), false),
        SetResult::UnknownKey
    );
    assert_eq!(c.calib.hour, 5);
}

#[test]
fn decode_repairs_an_enum_byte_out_of_range() {
    // C++ keeps a stored net.iface or mqtt.mode byte out of range in its enum until the repair
    // resets it; a Rust enum cannot hold it, so the decoder reports the field and the repair
    // treats it like any other bad field (same mask, count, first path and value).
    let a = Config::default();
    let mut wifi = Config::default();
    wifi.net.iface = NetInterface::Wifi;
    let iface_at = diff_offset(&a, &wifi);
    let mut mqtt = Config::default();
    mqtt.mqtt.mode = MqttMode::Mqtt;
    let mode_at = diff_offset(&a, &mqtt);
    for (at, raw, first) in [
        (iface_at, 3, "net.iface"),
        (iface_at, 255, "net.iface"),
        (mode_at, 3, "mqtt.mode"),
        (mode_at, 200, "mqtt.mode"),
    ] {
        let mut b = encode(&a);
        b[at] = raw;
        fix_crc(&mut b);
        let mut out = full_config();
        let mut info = DecodeInfo::default();
        assert_eq!(
            decode_config(&b, &mut out, Some(&mut info)),
            DecodeResult::Ok
        );
        assert_eq!(info.repairs.mask, REPAIR_FIELD, "{first} {raw}");
        assert_eq!(info.repairs.count, 1, "{first} {raw}");
        assert_text(&info.repairs.first, first);
        assert!(same_config(&out, &a), "{first} {raw}");
    }
    // Both at once: two repairs, the first in table order; with a bad station before them.
    let mut b = encode(&a);
    b[iface_at] = 7;
    b[mode_at] = 9;
    fix_crc(&mut b);
    let mut out = Config::default();
    let mut info = DecodeInfo::default();
    assert_eq!(
        decode_config(&b, &mut out, Some(&mut info)),
        DecodeResult::Ok
    );
    assert_eq!(info.repairs.count, 2);
    assert_text(&info.repairs.first, "net.iface");
    // The values in range decode as such (an MQTT mode needs a host; mode comes before it).
    let mut with_host = Config::default();
    strcpy(&mut with_host.mqtt.host, "b");
    for (at, raw, want_iface, want_mode) in [
        (iface_at, 1, NetInterface::Ethernet, MqttMode::Off),
        (mode_at, 2, NetInterface::Auto, MqttMode::MqttHa),
    ] {
        let mut b = encode(&with_host);
        b[at] = raw;
        fix_crc(&mut b);
        let mut out = Config::default();
        let mut info = DecodeInfo::default();
        assert_eq!(
            decode_config(&b, &mut out, Some(&mut info)),
            DecodeResult::Ok
        );
        assert_eq!(info.repairs.mask, 0);
        assert_eq!(out.net.iface, want_iface);
        assert_eq!(out.mqtt.mode, want_mode);
    }
}

#[test]
fn enum_and_result_values() {
    for (v, e) in [
        (0, NetInterface::Auto),
        (1, NetInterface::Ethernet),
        (2, NetInterface::Wifi),
    ] {
        assert_eq!(NetInterface::from_raw(v), Some(e));
        assert_eq!(e as u8, v);
    }
    assert_eq!(NetInterface::from_raw(3), None);
    for (v, e) in [
        (0, MqttMode::Off),
        (1, MqttMode::Mqtt),
        (2, MqttMode::MqttHa),
    ] {
        assert_eq!(MqttMode::from_raw(v), Some(e));
        assert_eq!(e as u8, v);
    }
    assert_eq!(MqttMode::from_raw(3), None);
    for (v, e) in [
        (0, ItemKind::Valve),
        (1, ItemKind::Temp),
        (2, ItemKind::Volt),
    ] {
        assert_eq!(ItemKind::from_raw(v), Some(e));
        assert_eq!(e as u8, v);
    }
    assert_eq!(ItemKind::from_raw(3), None);
    for v in 0..5 {
        assert_eq!(SetResult::from_raw(v).map(|r| r as u8), Some(v));
    }
    assert_eq!(SetResult::from_raw(5), None);
    for v in 0..7 {
        assert_eq!(PatchResult::from_raw(v).map(|r| r as u8), Some(v));
    }
    assert_eq!(PatchResult::from_raw(7), None);
    for v in 0..6 {
        assert_eq!(DecodeResult::from_raw(v).map(|r| r as u8), Some(v));
    }
    assert_eq!(DecodeResult::from_raw(6), None);
    for v in 0..5 {
        assert_eq!(ExtResult::from_raw(v).map(|r| r as u8), Some(v));
    }
    assert_eq!(ExtResult::from_raw(5), None);
    assert_eq!(ExtResult::default(), ExtResult::Absent);
    assert_eq!(LoadInfo::default().base, DecodeResult::TooShort);
    assert_eq!(ConfigValue::default(), ConfigValue::Null);
}

#[test]
fn constants() {
    assert_eq!(SECRET_MAX, 64);
    assert_eq!(HOST_MAX, 64);
    assert_eq!(TZ_NAME_MAX, 49);
    assert_eq!(TZ_POSIX_MAX, 49);
    assert_eq!(ALLOWED_HOSTS_MAX, 80);
    assert_eq!(CLIENT_ID_MAX, 64);
    assert_eq!(TOPIC_PREFIX_MAX, 32);
    assert_eq!(SSID_MAX, 32);
    assert_eq!(CONFIG_JSON_MAX_DEPTH, 8);
    assert_eq!(CONFIG_BLOB_MAX, 4096);
    assert_eq!(CONFIG_EXT_BLOB_MAX, 1536);
    assert_eq!(CONFIG_EXT_KEEP_MAX, 512);
    assert_eq!((RESTART_NETWORK, RESTART_HOSTNAME), (1, 2));
    let bits = [
        REPAIR_FIELD,
        REPAIR_STATIC_IP,
        REPAIR_WIFI_PASSWORD,
        REPAIR_WIFI_IFACE,
        REPAIR_SYSLOG,
        REPAIR_MQTT_HOST,
        REPAIR_MIN_DELAY,
        REPAIR_HA_SEPARATE,
        REPAIR_HA_DECIMAL,
        REPAIR_VALVE_NAMES,
        REPAIR_SLOT_IDS,
        REPAIR_SLOT_ACTIVE,
        REPAIR_TOPICS,
        REPAIR_HA_IDS,
    ];
    let shifts: Vec<u32> = bits.iter().map(|b| b.trailing_zeros()).collect();
    assert_eq!(shifts, [0, 1, 2, 3, 4, 6, 7, 8, 9, 10, 11, 12, 13, 15]);
    assert!(bits.iter().all(|b| b.count_ones() == 1));
}

#[test]
fn a_cfgx_record_with_tag_0_sets_the_first_base_field() {
    // C++ findExt(0) finds the first field without an ext tag, the station; a record with tag 0
    // (no writer emits one) is applied to it. Kept (docs/rust/PORT-NOTES.md).
    let mut c = Config::default();
    let mut info = ExtInfo::default();
    assert_eq!(
        decode_config_ext(
            &ext_blob(b"\x00\x00\x03abc", 1),
            &mut c,
            Some(&mut info),
            &mut []
        ),
        ExtResult::Ok
    );
    assert_eq!(info.applied, 1);
    assert_text(&c.station, "abc");
}
