//! Port of test/native/test_config__repairs.cpp: sanitize_config (every per-field reset kind,
//! every cross-field repair alone with its bit, path and item mask, the V2/V3 clash resolution,
//! fuzzed configs end valid), the repairing decode_config (newer schema prefix) and
//! load_config_blobs (repairs after the ext records). Values a Rust member cannot hold (an enum
//! out of range, a text without its NUL) have no Rust form; the enum case goes through the
//! decoder, which reports such a byte.

use super::tests::{
    diff_offset, encode, encode_ext, fix_crc, oid, same_config, strcpy, validate_path, Edit, ID_A,
    ID_B,
};
use super::*;
use crate::common::OneWireId;
use crate::test_support::{assert_text, Rng};
use std::string::String;

/// One repair: the mask, the count, the first path and the details.
struct Outcome {
    mask: u32,
    count: u16,
    first: String,
    r: Repairs,
}

fn repair(c: &mut Config) -> Outcome {
    let mut r = Repairs::default();
    let mask = sanitize_config(c, Some(&mut r));
    assert_eq!(mask, r.mask);
    assert_eq!(validate_path(c), "OK");
    Outcome {
        mask,
        count: r.count,
        first: String::from_utf8_lossy(&r.first).into_owned(),
        r,
    }
}

#[test]
fn a_valid_config_is_left_alone_out_may_be_none() {
    let mut c = Config::default();
    let mut r = Repairs {
        mask: 7,
        count: 3,
        valve_names: 1,
        temp_ids: 1,
        ..Repairs::default()
    };
    strcpy(&mut r.first, "x");
    assert_eq!(sanitize_config(&mut c, Some(&mut r)), 0);
    assert_eq!(r.mask, 0);
    assert_eq!(r.count, 0);
    assert_eq!(r.valve_names, 0);
    assert_eq!(r.temp_ids, 0);
    assert!(r.first.is_empty());
    assert!(same_config(&c, &Config::default()));
    c.calib.hour = 30;
    assert_eq!(sanitize_config(&mut c, None), REPAIR_FIELD);
    assert_eq!(c.calib.hour, 0);
}

type Check = fn(&Config) -> bool;

#[test]
fn every_field_kind_outside_its_rule_gets_its_default() {
    let cases: [(&str, Edit, Check); 18] = [
        (
            "schema",
            |c| c.schema = 1,
            |c| c.schema == CONFIG_JSON_SCHEMA,
        ),
        (
            "station",
            |c| c.station.clear(),
            |c| c.station.as_slice() == b"VdMot",
        ),
        // C++ net.iface 3: no Rust form; `net_iface_byte_out_of_range` below.
        (
            "net.mask",
            |c| c.net.mask = 0x00FF_00FF,
            |c| c.net.mask == 0,
        ),
        (
            "net.wifiPassword",
            |c| strcpy(&mut c.net.wifi_password, "a\x01"),
            |c| c.net.wifi_password.is_empty(),
        ),
        (
            "net.reconnectTimeoutMin",
            |c| c.net.reconnect_timeout_min = 241,
            |c| c.net.reconnect_timeout_min == 5,
        ),
        (
            "time.ntpServer",
            |c| strcpy(&mut c.time.ntp_server, "a b"),
            |c| c.time.ntp_server.as_slice() == b"pool.ntp.org",
        ),
        (
            "time.tzPosix",
            |c| c.time.tz_posix.clear(),
            |c| c.time.tz_posix.as_slice() == b"CET-1CEST,M3.5.0,M10.5.0/3",
        ),
        (
            "syslog.port",
            |c| c.syslog.port = 0,
            |c| c.syslog.port == 514,
        ),
        (
            "web.allowedHosts",
            |c| strcpy(&mut c.web.allowed_hosts, "a,,b"),
            |c| c.web.allowed_hosts.is_empty(),
        ),
        (
            "mqtt.keepAliveS",
            |c| c.mqtt.keep_alive_s = 4,
            |c| c.mqtt.keep_alive_s == 60,
        ),
        (
            "mqtt.discoveryPrefix",
            |c| c.mqtt.discovery_prefix.clear(),
            |c| c.mqtt.discovery_prefix.as_slice() == b"homeassistant",
        ),
        (
            "valves.4.failsafePct",
            |c| c.valves[3].failsafe_pct = 101,
            |c| c.valves[3].failsafe_pct == 50,
        ),
        (
            "valves.12.topic",
            |c| strcpy(&mut c.valves[11].topic, "a b"),
            |c| c.valves[11].topic.is_empty(),
        ),
        (
            "temps.34.offset",
            |c| c.temps[33].offset = -101,
            |c| c.temps[33].offset == 0,
        ),
        (
            "volts.2.offset",
            |c| c.volts[1].offset = f32::NAN,
            |c| c.volts[1].offset == 0.0,
        ),
        (
            "volts.8.factor",
            |c| c.volts[7].factor = 0.0,
            |c| c.volts[7].factor == 1.0,
        ),
        (
            "calib.minute",
            |c| c.calib.minute = 60,
            |c| c.calib.minute == 0,
        ),
        (
            "failsafe.timeoutMin",
            |c| c.failsafe.timeout_min = 4,
            |c| c.failsafe.timeout_min == 60,
        ),
    ];
    for (path, break_it, is_default) in cases {
        let mut c = Config::default();
        break_it(&mut c);
        let o = repair(&mut c);
        assert_eq!(o.mask, REPAIR_FIELD, "{path}");
        assert_eq!(o.count, 1, "{path}");
        assert_eq!(o.first, path);
        assert!(is_default(&c), "{path}");
    }
    // C++ a string without a NUL inside its array is reset as a whole: no Rust form.
    // Several fields: every one counted, the first path in table order.
    let mut d = Config::default();
    d.calib.hour = 24;
    d.mqtt.port = 0;
    d.temps[2].offset = 500;
    let od = repair(&mut d);
    assert_eq!(od.mask, REPAIR_FIELD);
    assert_eq!(od.count, 3);
    assert_eq!(od.first, "mqtt.port");
    assert_eq!(d.mqtt.port, 1883);
}

#[test]
fn net_iface_byte_out_of_range() {
    // The C++ case net.iface = 3 of the table above, through the decoder.
    let a = Config::default();
    let mut wifi = Config::default();
    wifi.net.iface = NetInterface::Wifi;
    let mut b = encode(&a);
    b[diff_offset(&a, &wifi)] = 3;
    fix_crc(&mut b);
    let mut c = Config::default();
    let mut info = DecodeInfo::default();
    assert_eq!(decode_config(&b, &mut c, Some(&mut info)), DecodeResult::Ok);
    assert_eq!(info.repairs.mask, REPAIR_FIELD);
    assert_eq!(info.repairs.count, 1);
    assert_text(&info.repairs.first, "net.iface");
    assert_eq!(c.net.iface, NetInterface::Auto);
}

#[test]
fn each_cross_field_repair_alone_sets_exactly_its_bit() {
    let cases: [(u32, &str, Edit, Check); 13] = [
        (
            REPAIR_STATIC_IP,
            "net.dhcp",
            |c| {
                c.net.dhcp = false;
                c.net.ip = 1;
                c.net.mask = 0x00FF_FFFF;
                c.net.gateway = 0;
            },
            |c| c.net.dhcp && c.net.ip == 1,
        ),
        (
            REPAIR_WIFI_PASSWORD,
            "net.wifiPassword",
            |c| {
                strcpy(&mut c.net.ssid, "w");
                strcpy(&mut c.net.wifi_password, "1234567");
            },
            |c| c.net.ssid.is_empty() && c.net.wifi_password.is_empty(),
        ),
        (
            REPAIR_WIFI_IFACE,
            "net.iface",
            |c| c.net.iface = NetInterface::Wifi,
            |c| c.net.iface == NetInterface::Auto,
        ),
        (
            REPAIR_SYSLOG,
            "syslog.level",
            |c| c.syslog.level = 1,
            |c| c.syslog.level == 0,
        ),
        (
            REPAIR_MQTT_HOST,
            "mqtt.mode",
            |c| c.mqtt.mode = MqttMode::Mqtt,
            |c| c.mqtt.mode == MqttMode::Off,
        ),
        (
            REPAIR_MIN_DELAY,
            "mqtt.minDelayS",
            |c| {
                c.mqtt.publish_interval_s = 20;
                c.mqtt.min_delay_s = 21;
            },
            |c| c.mqtt.min_delay_s == 20,
        ),
        (
            REPAIR_HA_SEPARATE,
            "mqtt.mode",
            |c| {
                c.mqtt.mode = MqttMode::MqttHa;
                strcpy(&mut c.mqtt.host, "b");
                c.mqtt.separate = false;
            },
            |c| c.mqtt.mode == MqttMode::Mqtt,
        ),
        (
            REPAIR_HA_DECIMAL,
            "mqtt.germanDecimal",
            |c| {
                c.mqtt.mode = MqttMode::MqttHa;
                strcpy(&mut c.mqtt.host, "b");
                c.mqtt.german_decimal = true;
            },
            |c| !c.mqtt.german_decimal && c.mqtt.mode == MqttMode::MqttHa,
        ),
        (
            REPAIR_VALVE_NAMES,
            "valves.3.name",
            |c| {
                strcpy(&mut c.valves[0].name, "Dom");
                strcpy(&mut c.valves[2].name, "Dom");
            },
            |c| c.valves[2].name.is_empty() && c.valves[0].name.first() == Some(&b'D'),
        ),
        (
            REPAIR_SLOT_IDS,
            "temps.2.id",
            |c| {
                c.temps[0].id = oid(ID_A);
                c.temps[1].id = oid(ID_A);
            },
            |c| is_zero(&c.temps[1].id) && !is_zero(&c.temps[0].id),
        ),
        (
            REPAIR_SLOT_ACTIVE,
            "volts.8.active",
            |c| c.volts[7].active = true,
            |c| !c.volts[7].active,
        ),
        (
            REPAIR_TOPICS,
            "valves.2.topic",
            |c| {
                strcpy(&mut c.valves[0].name, "Bad");
                strcpy(&mut c.valves[1].topic, "Bad");
            },
            |c| c.valves[1].topic.is_empty(),
        ),
        (
            REPAIR_HA_IDS,
            "valves.2.name",
            |c| {
                strcpy(&mut c.valves[0].name, "Bad 1");
                strcpy(&mut c.valves[1].name, "Bad.1");
            },
            |c| c.valves[1].name.is_empty(),
        ),
    ];
    for (bit, first, break_it, repaired) in cases {
        let mut c = Config::default();
        break_it(&mut c);
        let o = repair(&mut c);
        assert_eq!(o.mask, bit, "{first}");
        assert_eq!(o.count, 1, "{first}");
        assert_eq!(o.first, first);
        assert!(repaired(&c), "{first}");
    }
}

#[test]
fn the_cross_field_rules_keep_what_is_valid_boundaries() {
    let mut c = Config::default();
    c.net.dhcp = false;
    c.net.ip = 1;
    c.net.mask = 0x00FF_FFFF;
    c.net.gateway = 2;
    strcpy(&mut c.net.ssid, "w");
    strcpy(&mut c.net.wifi_password, "12345678");
    c.net.iface = NetInterface::Wifi;
    c.syslog.level = 3;
    c.syslog.server = 9;
    c.mqtt.mode = MqttMode::MqttHa;
    strcpy(&mut c.mqtt.host, "b");
    c.mqtt.publish_interval_s = 20;
    c.mqtt.min_delay_s = 20;
    c.temps[0].id = oid(ID_A);
    c.temps[0].active = true;
    c.temps[1].id = oid(ID_B);
    c.temps[1].active = true;
    let before = c.clone();
    let o = repair(&mut c);
    assert_eq!(o.mask, 0);
    assert!(same_config(&c, &before));
    // An open network (no password) is kept.
    c.net.wifi_password.clear();
    assert_eq!(repair(&mut c).mask, 0);
    assert_text(&c.net.ssid, "w");
    // Every part of an incomplete static address counts.
    for part in 0..3 {
        let mut s = before.clone();
        match part {
            0 => s.net.ip = 0,
            1 => s.net.mask = 0,
            _ => s.net.gateway = 0,
        }
        assert_eq!(repair(&mut s).mask, REPAIR_STATIC_IP, "{part}");
        assert!(s.net.dhcp);
    }
}

#[test]
fn v2_and_v3_clashes_clear_the_later_item_else_the_earlier_one() {
    {
        // V2: the later valve's name equals an earlier override.
        let mut c = Config::default();
        strcpy(&mut c.valves[0].topic, "x");
        strcpy(&mut c.valves[1].name, "x");
        let o = repair(&mut c);
        assert_eq!(o.mask, REPAIR_VALVE_NAMES);
        assert_eq!(o.first, "valves.2.name");
        assert_eq!(o.r.valve_names, 2);
        assert_eq!(o.r.valve_topics, 0);
        assert_text(&c.valves[0].topic, "x");
    }
    {
        // V2: the unnamed later valve uses its number, an earlier override takes it.
        let mut c = Config::default();
        strcpy(&mut c.valves[0].topic, "2");
        let o = repair(&mut c);
        assert_eq!(o.mask, REPAIR_TOPICS);
        assert_eq!(o.first, "valves.1.topic");
        assert_eq!(o.r.valve_topics, 1);
        assert_eq!(o.r.valve_names, 0);
        assert!(c.valves[0].topic.is_empty());
    }
    {
        // V3 on temp slots: the later name goes; inactive slots do not count.
        let mut c = Config::default();
        c.temps[0].id = oid(ID_A);
        c.temps[0].active = true;
        strcpy(&mut c.temps[0].name, "a b");
        c.temps[3].id = oid(ID_B);
        strcpy(&mut c.temps[3].name, "a.b");
        assert_eq!(repair(&mut c).mask, 0);
        c.temps[3].active = true;
        let o = repair(&mut c);
        assert_eq!(o.mask, REPAIR_HA_IDS);
        assert_eq!(o.first, "temps.4.name");
        assert_eq!(o.r.valve_names, 0);
        assert!(c.temps[3].name.is_empty());
        assert_text(&c.temps[0].name, "a b");
    }
    {
        // V3: the unnamed later slot keeps its number, the earlier slot named like it loses its
        // name.
        let mut c = Config::default();
        c.temps[0].id = oid(ID_A);
        c.temps[0].active = true;
        strcpy(&mut c.temps[0].name, "2");
        c.temps[1].id = oid(ID_B);
        c.temps[1].active = true;
        let o = repair(&mut c);
        assert_eq!(o.mask, REPAIR_HA_IDS);
        assert_eq!(o.first, "temps.1.name");
        assert!(c.temps[0].name.is_empty());
    }
    {
        // V3 on volt slots: the later override goes before its name.
        let mut c = Config::default();
        c.volts[0].id = oid(ID_A);
        c.volts[0].active = true;
        strcpy(&mut c.volts[0].name, "bat");
        c.volts[5].id = oid(ID_B);
        c.volts[5].active = true;
        strcpy(&mut c.volts[5].name, "other");
        strcpy(&mut c.volts[5].topic, "bat");
        let o = repair(&mut c);
        assert_eq!(o.mask, REPAIR_HA_IDS);
        assert_eq!(o.first, "volts.6.topic");
        assert!(c.volts[5].topic.is_empty());
        assert_text(&c.volts[5].name, "other");
    }
    {
        // V3 on valves with an override: the later override goes.
        let mut c = Config::default();
        strcpy(&mut c.valves[0].name, "Bad 1");
        strcpy(&mut c.valves[4].topic, "Bad.1");
        let o = repair(&mut c);
        assert_eq!(o.mask, REPAIR_HA_IDS);
        assert_eq!(o.first, "valves.5.topic");
        assert_eq!(o.r.valve_topics, 1 << 4);
    }
}

#[test]
fn clearing_a_name_can_make_a_new_clash_repeated_until_stable() {
    // Valve 5 is named "3": fine while valve 3 has a name. Valve 3 is a duplicate of valve 1 and
    // loses its name, then valve 5 clashes with the number of the unnamed valve 3.
    let mut c = Config::default();
    strcpy(&mut c.valves[0].name, "Dom");
    strcpy(&mut c.valves[2].name, "Dom");
    strcpy(&mut c.valves[4].name, "3");
    let o = repair(&mut c);
    assert_eq!(o.mask, REPAIR_VALVE_NAMES);
    assert_eq!(o.count, 2);
    assert_eq!(o.first, "valves.3.name");
    assert_eq!(o.r.valve_names, (1 << 2) | (1 << 4));
    assert_text(&c.valves[0].name, "Dom");
}

#[test]
fn a_valve_named_like_the_number_of_a_named_valve_keeps_its_name() {
    // "3" clashes only with the number segment of an unnamed valve 3.
    let mut c = Config::default();
    strcpy(&mut c.valves[0].name, "3");
    strcpy(&mut c.valves[2].name, "x");
    let before = c.clone();
    let o = repair(&mut c);
    assert_eq!(o.mask, 0);
    assert_eq!(o.r.valve_names, 0);
    assert!(same_config(&c, &before));
}

#[test]
fn duplicate_slot_ids_the_active_flag_follows_the_id() {
    let mut c = Config::default();
    c.temps[0].id = oid(ID_A);
    c.temps[5].id = oid(ID_A);
    c.temps[5].active = true;
    c.temps[33].id = oid(ID_A);
    c.volts[0].id = oid(ID_B);
    c.volts[7].id = oid(ID_B);
    let o = repair(&mut c);
    assert_eq!(o.mask, REPAIR_SLOT_IDS | REPAIR_SLOT_ACTIVE);
    assert_eq!(o.count, 4);
    assert_eq!(o.first, "temps.6.id");
    assert_eq!(o.r.temp_ids, (1 << 5) | (1 << 33));
    assert_eq!(o.r.temp_active, 1 << 5);
    assert_eq!(o.r.volt_ids, 1 << 7);
    assert_eq!(o.r.volt_active, 0);
    assert!(!is_zero(&c.temps[0].id));
    assert!(!c.temps[5].active);
    let mut v = Config::default();
    v.volts[2].active = true;
    let ov = repair(&mut v);
    assert_eq!(ov.r.volt_active, 1 << 2);
    assert_eq!(ov.r.temp_active, 0);
}

#[test]
fn fuzzed_configs_always_end_valid_and_stay_put_on_a_second_pass() {
    let mut rng = Rng::new(20260925);
    let strs: [&str; 15] = [
        "", "a", "a b", "2", "3", "Dom", "Dom", "/x", "x/y", "a+b", "\x01", "ha/x", "a,,b",
        "Bad.1", "Bad 1",
    ];
    let ids = [ID_A, ID_B, ""];
    let pick = |rng: &mut Rng, v: &[&'static str]| v[rng.below(v.len() as u32) as usize];
    let id_of = |s: &str| {
        if s.is_empty() {
            OneWireId::default()
        } else {
            oid(s)
        }
    };
    for _ in 0..4000 {
        let mut c = Config::default();
        let edits = 1 + rng.below(12);
        for _ in 0..edits {
            let v = rng.below(u32::from(VALVE_COUNT)) as usize;
            let t = rng.below(u32::from(TEMP_SLOT_COUNT)) as usize;
            let u = rng.below(u32::from(VOLT_SLOT_COUNT)) as usize;
            match rng.below(20) {
                0 => strcpy(&mut c.station, pick(&mut rng, &strs)),
                1 => {
                    c.net.dhcp = rng.below(2) != 0;
                    c.net.ip = rng.below(3);
                }
                2 => {
                    strcpy(&mut c.net.ssid, pick(&mut rng, &strs));
                    strcpy(&mut c.net.wifi_password, pick(&mut rng, &strs));
                }
                3 => {
                    // C++ also draws 3, an iface a Rust enum cannot hold: the iface stays.
                    if let Some(i) = NetInterface::from_raw(rng.below(4) as u8) {
                        c.net.iface = i;
                    }
                }
                4 => {
                    c.syslog.level = rng.below(5) as u8;
                    c.syslog.server = rng.below(2);
                }
                5 => {
                    strcpy(&mut c.mqtt.user, pick(&mut rng, &strs));
                    strcpy(&mut c.mqtt.password, pick(&mut rng, &strs));
                }
                6 => {
                    c.mqtt.mode = MqttMode::from_raw(rng.below(3) as u8).expect("0..2");
                    strcpy(&mut c.mqtt.host, pick(&mut rng, &strs));
                }
                7 => {
                    c.mqtt.min_delay_s = rng.below(30) as u16;
                    c.mqtt.publish_interval_s = rng.below(30) as u16;
                }
                8 => {
                    c.mqtt.separate = rng.below(2) != 0;
                    c.mqtt.german_decimal = rng.below(2) != 0;
                }
                9 => strcpy(&mut c.valves[v].name, pick(&mut rng, &strs)),
                10 => strcpy(&mut c.valves[v].topic, pick(&mut rng, &strs)),
                11 => c.valves[v].failsafe_pct = rng.next_u32() as u8,
                12 => {
                    strcpy(&mut c.temps[t].name, pick(&mut rng, &strs));
                    c.temps[t].active = rng.below(2) != 0;
                }
                13 => c.temps[t].id = id_of(pick(&mut rng, &ids)),
                14 => strcpy(&mut c.temps[t].topic, pick(&mut rng, &strs)),
                15 => {
                    strcpy(&mut c.volts[u].name, pick(&mut rng, &strs));
                    c.volts[u].active = rng.below(2) != 0;
                }
                16 => c.volts[u].id = id_of(pick(&mut rng, &ids)),
                17 => {
                    strcpy(&mut c.volts[u].topic, pick(&mut rng, &strs));
                    c.volts[u].factor = rng.below(3) as f32;
                }
                18 => {
                    strcpy(&mut c.mqtt.discovery_prefix, pick(&mut rng, &strs));
                    strcpy(&mut c.web.allowed_hosts, pick(&mut rng, &strs));
                }
                _ => c.failsafe.timeout_min = rng.below(2000) as u16,
            }
        }
        let valid = validate_config(&c, &mut []).is_ok();
        let mut r = Repairs::default();
        let mask = sanitize_config(&mut c, Some(&mut r));
        assert_eq!(validate_path(&c), "OK");
        assert_eq!(mask == 0, valid);
        assert_eq!(r.count == 0, mask == 0);
        assert_eq!(r.first.is_empty(), mask == 0);
        let once = c.clone();
        assert_eq!(sanitize_config(&mut c, None), 0);
        assert!(same_config(&c, &once));
    }
}

#[test]
fn decode_a_newer_base_schema_is_read_by_its_schema_1_prefix() {
    let mut c = Config::default();
    strcpy(&mut c.station, "Newer");
    c.calib.hour = 7;
    let mut b = encode(&c);
    // Schema 2 with 5 more payload bytes (a newer firmware's fields).
    b[4] = 2;
    let at = b.len() - 4;
    b.splice(at..at, [1, 2, 3, 4, 5]);
    let payload = b.len() - 12;
    b[6] = payload as u8;
    b[7] = (payload >> 8) as u8;
    fix_crc(&mut b);
    let mut out = Config::default();
    let mut info = DecodeInfo::default();
    assert_eq!(
        decode_config(&b, &mut out, Some(&mut info)),
        DecodeResult::Ok
    );
    assert_eq!(info.schema, 2);
    assert!(info.newer_schema);
    assert_eq!(info.repairs.mask, 0);
    assert!(same_config(&out, &c));
    // Schema 2 exactly as long as schema 1: still Ok.
    let mut same = encode(&c);
    same[4] = 2;
    fix_crc(&mut same);
    assert_eq!(
        decode_config(&same, &mut out, Some(&mut info)),
        DecodeResult::Ok
    );
    assert!(info.newer_schema);
    // Schema 2 shorter than the schema-1 fields: Invalid, defaults, not newer.
    let mut short_b = encode(&c);
    short_b.remove(short_b.len() - 5);
    short_b[4] = 2;
    let pl = short_b.len() - 12;
    short_b[6] = pl as u8;
    short_b[7] = (pl >> 8) as u8;
    fix_crc(&mut short_b);
    assert_eq!(
        decode_config(&short_b, &mut out, Some(&mut info)),
        DecodeResult::Invalid
    );
    assert!(!info.newer_schema);
    assert!(same_config(&out, &Config::default()));
    // Schema 1 with the same extra bytes: Invalid (the length rule of 2.0.0).
    b[4] = 1;
    fix_crc(&mut b);
    assert_eq!(
        decode_config(&b, &mut out, Some(&mut info)),
        DecodeResult::Invalid
    );
    assert!(!info.newer_schema);
    assert_eq!(decode_config(&b, &mut out, None), DecodeResult::Invalid);
}

#[test]
fn decode_value_damage_is_repaired_and_reported_the_blob_itself_stays_ok() {
    let mut c = Config::default();
    c.net.dhcp = false; // static without an address: repaired to DHCP
    c.syslog.level = 2; // without a server: syslog off
    let b = encode(&c);
    let mut out = Config::default();
    let mut info = DecodeInfo::default();
    assert_eq!(
        decode_config(&b, &mut out, Some(&mut info)),
        DecodeResult::Ok
    );
    assert_eq!(info.repairs.mask, REPAIR_STATIC_IP | REPAIR_SYSLOG);
    assert_eq!(info.repairs.count, 2);
    assert_text(&info.repairs.first, "net.dhcp");
    assert!(out.net.dhcp);
    assert_eq!(out.syslog.level, 0);
}

#[test]
fn load_config_blobs_ext_records_that_break_v2_are_repaired_after_the_ext() {
    let mut base = Config::default();
    strcpy(&mut base.valves[0].name, "Bad");
    let mut ext = base.clone();
    strcpy(&mut ext.valves[3].topic, "Bad");
    let b = encode(&base);
    let x = encode_ext(&ext, &[]);
    let mut blobs = StoredBlobs { base: &b, ext: &x };
    let mut out = Config::default();
    let mut li = LoadInfo::default();
    assert!(load_config_blobs(&blobs, &mut out, &mut li, &mut []));
    assert_eq!(li.decode.repairs.mask, 0);
    assert_eq!(li.repairs.mask, REPAIR_TOPICS);
    assert_eq!(li.repairs.valve_topics, 1 << 3);
    assert_text(&li.repairs.first, "valves.4.topic");
    assert!(out.valves[3].topic.is_empty());
    assert_text(&out.valves[0].name, "Bad");
    assert_eq!(validate_path(&out), "OK");
    // Base repairs are in `decode`, the final pass finds nothing more.
    let mut bad = base.clone();
    bad.calib.hour = 24;
    let bb = encode(&bad);
    blobs.base = &bb;
    blobs.ext = &[];
    assert!(load_config_blobs(&blobs, &mut out, &mut li, &mut []));
    assert_eq!(li.ext, ExtResult::Absent);
    assert_eq!(li.decode.repairs.mask, REPAIR_FIELD);
    assert_eq!(li.repairs.mask, 0);
    assert!(li.repairs.first.is_empty());
}

#[test]
fn valve_names_clash_also_when_the_later_valve_has_a_topic_override() {
    // Rust-only (expected values from the C++ on the differential drivers): the name rule
    // compares the names (' ' == '_'), not the segments, so the override of the later valve
    // does not keep its name.
    for (earlier, later) in [("a b", "a_b"), ("Bad", "Bad")] {
        let mut c = Config::default();
        strcpy(&mut c.valves[0].name, earlier);
        strcpy(&mut c.valves[1].name, later);
        strcpy(&mut c.valves[1].topic, "x");
        assert_eq!(validate_path(&c), "valves.2.name", "{later}");
        let mut r = Repairs::default();
        assert_eq!(sanitize_config(&mut c, Some(&mut r)), REPAIR_VALVE_NAMES);
        assert_eq!((r.count, r.valve_names, r.valve_topics), (1, 1 << 1, 0));
        assert_text(&r.first, "valves.2.name");
        assert!(c.valves[1].name.is_empty());
        assert_text(&c.valves[1].topic, "x");
        assert_eq!(validate_path(&c), "OK");
    }
}
