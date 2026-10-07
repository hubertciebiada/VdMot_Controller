//! Port of test/native/test_ha_discovery.cpp: legacy KEEP entities (object ids, names and
//! unique_ids of the legacy firmware), the 2.1 entities with their availability, DROP
//! deletions, the classification of list lines and the discovery run over a fake port.
//!
//! The C++ fills context arrays without their NUL in a few cases (`memset` of the whole
//! field). A Rust text holds at most the C++ capacity without the NUL, which the C++ reads the
//! same way (bounded), except a station or a segment of the full C++ size: the C++ rejects it
//! as too long, a Rust text cannot hold it (no Rust form, named where it stood). The C++ null
//! topic of classifyDiscoveryTopic() has no Rust form either. The iterators take the context
//! at every call; C++ `reset(other)` is `restart()` with the other context passed from then on.
//! Helpers that iterate assert progress, so a mutant that stops an iterator fails instead of
//! hanging.

use super::*;
use crate::common::{contains_bytes, OneWireId, TEMP_READ_ERROR};
use crate::event_log::event_mqtt_names;
use crate::test_support::{assert_text, Lcg};
use crate::valve_model::ValveState;
use std::boxed::Box;
use std::collections::BTreeMap;
use std::format;
use std::string::{String, ToString};
use std::vec;
use std::vec::Vec;

const DEVICE: &str = "\"device\":{\"identifiers\":\"VdMot\",\"name\":\"VdMot\",\
                      \"sw_version\":\"2.1.0-revamped\",\"model\":\"VdMot Revamped\",\
                      \"manufacturer\":\"Lenti84/Surfgargano\",\
                      \"configuration_url\":\"http://192.168.1.50/\"}}";
const ESP: &str = "\"availability\":[{\"topic\":\"VdMot/status\"}],";
const ESP_STM: &str = "\"availability\":[{\"topic\":\"VdMot/status\"},\
                       {\"topic\":\"VdMot/stm/status\"}],\"availability_mode\":\"all\",";
const TEMPLATE: &str = "\"value_template\":\"{{ value | replace(',', '.') | float(None) }}\",";

fn set<const N: usize>(t: &mut Text<N>, s: &[u8]) {
    copy_string(t, s);
}

/// C++ `memset(field, ch, sizeof field)`: the field without its NUL, read bounded.
fn fill_all<const N: usize>(t: &mut Text<N>, ch: u8) {
    t.clear();
    while t.push(ch).is_ok() {}
}

fn base(c: &mut DiscoveryContext) {
    *c = DiscoveryContext::default();
    set(&mut c.topics.station, b"VdMot");
    set(&mut c.ip, b"192.168.1.50");
    set(&mut c.sw_version, b"2.1.0-revamped");
}

fn new_ctx() -> Box<DiscoveryContext> {
    let mut c = Box::<DiscoveryContext>::default();
    base(&mut c);
    c
}

fn valve(c: &mut DiscoveryContext, i: usize, seg: &str, t1: bool, t2: bool, name: &[u8]) {
    let v = &mut c.valves[i];
    v.active = true;
    set(&mut v.segment, seg.as_bytes());
    set(&mut v.name, name);
    v.has_temp1 = t1;
    v.has_temp2 = t2;
}

fn temp(c: &mut DiscoveryContext, i: usize, seg: &str, id: &str, published: bool, topic: &str) {
    let s = &mut c.temps[i];
    s.active = true;
    s.published = published;
    set(&mut s.segment, seg.as_bytes());
    let topic = if topic.is_empty() { seg } else { topic };
    set(&mut s.topic_segment, topic.as_bytes());
    set(&mut s.id, id.as_bytes());
}

/// C++ `temp(c, i, seg, id)`: published, the published segment is the slot segment.
fn temp1(c: &mut DiscoveryContext, i: usize, seg: &str, id: &str) {
    temp(c, i, seg, id, true, "");
}

fn volt(c: &mut DiscoveryContext, i: usize, seg: &str, name: &str, unit: &str, id: &str) {
    let s = &mut c.volts[i];
    s.active = true;
    set(&mut s.segment, seg.as_bytes());
    set(&mut s.topic_segment, seg.as_bytes());
    set(&mut s.name, name.as_bytes());
    set(&mut s.unit, unit.as_bytes());
    set(&mut s.id, id.as_bytes());
}

pub(super) struct Msg {
    pub(super) topic: Vec<u8>,
    pub(super) json: Vec<u8>,
}

/// Every message of a fresh iterator (at most the 309 entities).
pub(super) fn all(c: &DiscoveryContext) -> Vec<Msg> {
    let mut it = DiscoveryIterator::new();
    let mut out = Vec::new();
    let mut m = DiscoveryMessage::default();
    let mut buf = vec![0u8; 4096];
    let mut jw = JsonWriter::new(&mut buf);
    while it.next(c, &mut m, &mut jw) {
        assert!(!m.remove);
        assert!(jw.complete());
        out.push(Msg {
            topic: m.topic.to_vec(),
            json: jw.as_bytes().to_vec(),
        });
        assert!(out.len() <= 309, "the iterator makes progress");
    }
    assert!(jw.ok()); // ended because the list is exhausted
    out
}

pub(super) fn topics(c: &DiscoveryContext) -> Vec<Vec<u8>> {
    let mut it = DiscoveryIterator::new();
    let mut out = Vec::new();
    let mut m = DiscoveryMessage::default();
    while it.next_topic(c, &mut m) {
        out.push(m.topic.to_vec());
        assert!(out.len() <= 309, "the iterator makes progress");
    }
    assert!(m.topic.is_empty());
    out
}

fn find<'a>(v: &'a [Msg], topic: &str) -> Option<&'a Msg> {
    v.iter().find(|m| m.topic == topic.as_bytes())
}

#[track_caller]
pub(super) fn json(v: &[Msg], topic: &str) -> Vec<u8> {
    find(v, topic)
        .unwrap_or_else(|| panic!("no message for {topic}"))
        .json
        .clone()
}

pub(super) fn has(s: &[u8], part: impl AsRef<[u8]>) -> bool {
    contains_bytes(s, part.as_ref())
}

fn drops(c: &DiscoveryContext) -> Vec<Vec<u8>> {
    let mut it = DropListIterator::new();
    let mut out = Vec::new();
    let mut m = DiscoveryMessage::default();
    while it.next(c, &mut m) {
        assert!(m.remove);
        out.push(m.topic.to_vec());
        assert!(out.len() <= 200, "the iterator makes progress");
    }
    out
}

pub(super) fn cls(c: &DiscoveryContext, t: impl AsRef<[u8]>) -> TopicClass {
    classify_discovery_topic(c, t.as_ref())
}

const VALVE_KIND_TOPICS: [&str; 20] = [
    "text/VdMot/valves_state_",
    "valve/VdMot/valves_target_",
    "sensor/VdMot/valves_actual_",
    "sensor/VdMot/valves_temp1_",
    "sensor/VdMot/valves_temp2_",
    "text/VdMot/valves_calibration_date_",
    "text/VdMot/valves_calibration_repetitions_",
    "text/VdMot/valves_diag_openCount_",
    "text/VdMot/valves_diag_closeCount_",
    "text/VdMot/valves_diag_deadZoneCount_",
    "text/VdMot/valves_diag_moves_",
    "text/VdMot/valves_diag_meanCurrrent_",
    "sensor/VdMot/diag_earlyStops_",
    "sensor/VdMot/diag_cmdRejected_",
    "sensor/VdMot/diag_lastStop_",
    "sensor/VdMot/diag_calState_",
    "binary_sensor/VdMot/valves_problem_",
    "sensor/VdMot/valves_failsafe_",
    "sensor/VdMot/valves_sync_",
    "button/VdMot/valves_calibrate_",
];

fn valve_topics(seg: &str, t1: bool, t2: bool) -> Vec<String> {
    let mut v = Vec::new();
    for (k, kind) in VALVE_KIND_TOPICS.iter().enumerate() {
        if (k == 3 && !t1) || (k == 4 && !t2) {
            continue;
        }
        v.push(format!("homeassistant/{kind}{seg}/config"));
    }
    v
}

const TAIL_TOPICS: [&str; 23] = [
    "binary_sensor/VdMot/diag_esp_online",
    "binary_sensor/VdMot/diag_stm_online",
    "binary_sensor/VdMot/diag_failsafe",
    "sensor/VdMot/diag_stm_lease",
    "binary_sensor/VdMot/diag_stm_safeMode",
    "sensor/VdMot/diag_stm_link",
    "sensor/VdMot/diag_stm_proto",
    "sensor/VdMot/diag_stm_version",
    "sensor/VdMot/diag_stm_started",
    "sensor/VdMot/diag_stm_resets",
    "sensor/VdMot/diag_stm_rxOverflow",
    "sensor/VdMot/diag_stm_parseErr",
    "binary_sensor/VdMot/diag_calibration_active",
    "sensor/VdMot/diag_calibration_next",
    "sensor/VdMot/diag_mqtt_eventsSuppressed",
    "sensor/VdMot/diag_mqtt_commandsRejected",
    "event/VdMot/events",
    "button/VdMot/cmd_calibrate_all",
    "button/VdMot/cmd_detect",
    "button/VdMot/cmd_stop",
    "button/VdMot/cmd_stm_reset",
    "button/VdMot/cmd_esp_restart",
    "button/VdMot/cmd_stm_safe_exit",
];

pub(super) fn tail(k: usize) -> String {
    format!("homeassistant/{}/config", TAIL_TOPICS[k])
}

fn bytes_of(v: &[String]) -> Vec<Vec<u8>> {
    v.iter().map(|s| s.as_bytes().to_vec()).collect()
}

#[test]
fn component_names() {
    assert_eq!(ha_component_name(HaComponent::Sensor), "sensor");
    assert_eq!(
        ha_component_name(HaComponent::BinarySensor),
        "binary_sensor"
    );
    assert_eq!(ha_component_name(HaComponent::Text), "text");
    assert_eq!(ha_component_name(HaComponent::Valve), "valve");
    assert_eq!(ha_component_name(HaComponent::Number), "number");
    assert_eq!(ha_component_name(HaComponent::Select), "select");
    assert_eq!(ha_component_name(HaComponent::Switch), "switch");
    assert_eq!(ha_component_name(HaComponent::Climate), "climate");
    assert_eq!(ha_component_name(HaComponent::Button), "button");
    assert_eq!(ha_component_name(HaComponent::Event), "event");
    // C++ haComponentName(static_cast<HaComponent>(10)) == "": a HaComponent cannot hold 10.
    assert_eq!(HaComponent::from_raw(10), None);
}

#[test]
fn discovery_golden_order() {
    let mut c = new_ctx();
    valve(&mut c, 0, "Bad_1", true, false, b"Bad 1");
    valve(&mut c, 2, "3", false, true, b"");
    temp1(&mut c, 0, "1", "28-84-37-94-97-ff-03-23");
    volt(&mut c, 1, "Batt", "Batt", "V", "26-00-00-00-00-00-00-01");
    let mut expected: Vec<String> = [
        "homeassistant/text/VdMot/state/config",
        "homeassistant/text/VdMot/message/config",
        "homeassistant/text/VdMot/uptime/config",
        "homeassistant/text/VdMot/ip/config",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    expected.extend(valve_topics("Bad_1", true, false));
    expected.extend(valve_topics("3", false, true));
    expected.push("homeassistant/sensor/VdMot/temps_1/config".to_string());
    expected.push("homeassistant/sensor/VdMot/volts_Batt/config".to_string());
    for k in 0..23 {
        if k != 19 && k != 22 {
            expected.push(tail(k)); // stop / safe exit need protocol 3
        }
    }
    let v = all(&c);
    assert_eq!(v.len(), expected.len());
    assert_eq!(v.len(), 65);
    for (m, e) in v.iter().zip(&expected) {
        assert_text(&m.topic, e);
    }
    assert_eq!(topics(&c), bytes_of(&expected));
    // Every current topic classifies as Current.
    for t in &expected {
        assert_eq!(cls(&c, t), TopicClass::Current, "{t}");
    }
    // With protocol 3 the two buttons follow.
    c.stm_v3 = true;
    let t3 = topics(&c);
    assert_eq!(t3.len(), 67);
    assert_text(&t3[63], tail(19));
    assert_text(&t3[66], tail(22));
}

#[test]
fn discovery_keep_entities_carry_no_availability_k3_1_k4_1_w3_8() {
    let mut c = new_ctx();
    valve(&mut c, 0, "Bad_1", true, false, b"Bad 1");
    temp1(&mut c, 0, "1", "28-84-37-94-97-ff-03-23");
    volt(&mut c, 1, "Batt", "Batt", "V", "26-00-00-00-00-00-00-01");
    let v = all(&c);
    let dev = DEVICE;
    assert_text(
        &json(&v, "homeassistant/text/VdMot/state/config"),
        format!(
            "{{\"name\":\"state\",\"unique_id\":\"VdMot.common.state\",\
             \"state_topic\":\"VdMot/common/state/value\",\
             \"command_topic\":\"VdMot/common/state/set\",\"icon\":\"mdi:state-machine\",{dev}"
        ),
    );
    assert_text(
        &json(&v, "homeassistant/text/VdMot/ip/config"),
        format!(
            "{{\"name\":\"ip\",\"unique_id\":\"VdMot.common.ip\",\
             \"state_topic\":\"VdMot/common/ip/value\",\
             \"command_topic\":\"VdMot/common/ip/set\",\"icon\":\"mdi:message\",{dev}"
        ),
    );
    assert_text(
        &json(&v, "homeassistant/text/VdMot/valves_state_Bad_1/config"),
        format!(
            "{{\"name\":\"valves.Bad_1.state\",\"unique_id\":\"VdMot.valves.Bad_1.state\",\
             \"state_topic\":\"VdMot/valves/Bad_1/state/value\",\
             \"command_topic\":\"VdMot/valves/Bad_1/state/set\",\
             \"icon\":\"mdi:state-machine\",{dev}"
        ),
    );
    assert_text(
        &json(&v, "homeassistant/valve/VdMot/valves_target_Bad_1/config"),
        format!(
            "{{\"name\":\"valves.Bad_1.target\",\"unique_id\":\"VdMot.valves.Bad_1.target\",\
             \"state_topic\":\"VdMot/valves/Bad_1/target/value\",\
             \"command_topic\":\"VdMot/valves/Bad_1/target/set\",\"icon\":\"mdi:valve\",\
             \"device_class\":\"water\",\"reports_position\":true,\"qos\":1,{dev}"
        ),
    );
    assert_text(
        &json(&v, "homeassistant/sensor/VdMot/valves_temp1_Bad_1/config"),
        format!(
            "{{\"name\":\"valves.Bad_1.temp1\",\"unique_id\":\"VdMot.valves.Bad_1.temp1\",\
             \"state_topic\":\"VdMot/valves/Bad_1/temp1/value\",{TEMPLATE}\
             \"icon\":\"mdi:thermometer\",\"device_class\":\"temperature\",\
             \"state_class\":\"measurement\",\"unit_of_measurement\":\"\u{b0}C\",\
             \"expire_after\":60,{dev}"
        ),
    );
    assert_text(
        &json(
            &v,
            "homeassistant/text/VdMot/valves_calibration_date_Bad_1/config",
        ),
        format!(
            "{{\"name\":\"valves.Bad_1.calibration.date\",\
             \"unique_id\":\"VdMot.valves.Bad_1.calibration.date\",\
             \"state_topic\":\"VdMot/valves/Bad_1/calibration/date/value\",\
             \"command_topic\":\"VdMot/valves/Bad_1/calibration/date/set\",\
             \"icon\":\"mdi:timelapse\",{dev}"
        ),
    );
    for x in [
        "openCount",
        "closeCount",
        "deadZoneCount",
        "moves",
        "meanCurrrent",
    ] {
        assert_text(
            &json(&v, &format!("homeassistant/text/VdMot/valves_diag_{x}_Bad_1/config")),
            format!(
                "{{\"name\":\"valves.Bad_1.diag.{x}\",\"unique_id\":\"VdMot.valves.Bad_1.diag.{x}\",\
                 \"state_topic\":\"VdMot/valves/Bad_1/diag/{x}/value\",\
                 \"command_topic\":\"VdMot/valves/Bad_1/diag/{x}/set\",\"icon\":\"mdi:valve\",{dev}"
            ),
        );
    }
    assert_text(
        &json(&v, "homeassistant/sensor/VdMot/temps_1/config"),
        format!(
            "{{\"name\":\"temps.1\",\"unique_id\":\"VdMot.28-84-37-94-97-ff-03-23\",\
             \"state_topic\":\"VdMot/temps/1/value/value\",{TEMPLATE}\
             \"icon\":\"mdi:thermometer\",\"device_class\":\"temperature\",\
             \"state_class\":\"measurement\",\"unit_of_measurement\":\"\u{b0}C\",\
             \"expire_after\":60,{dev}"
        ),
    );
    assert_text(
        &json(&v, "homeassistant/sensor/VdMot/volts_Batt/config"),
        format!(
            "{{\"name\":\"volts.Batt\",\"unique_id\":\"VdMot.26-00-00-00-00-00-00-01\",\
             \"state_topic\":\"VdMot/sensors/Batt/value/value\",{TEMPLATE}\
             \"device_class\":\"voltage\",\"state_class\":\"measurement\",\
             \"unit_of_measurement\":\"V\",\"expire_after\":60,{dev}"
        ),
    );
    // KEEP entities: no availability at all.
    for m in &v {
        let keep = has(&m.topic, "/text/")
            || has(&m.topic, "valves_target_")
            || has(&m.topic, "valves_temp")
            || has(&m.topic, "temps_")
            || has(&m.topic, "volts_")
            || has(&m.topic, "diag_esp_online");
        let t = m.topic.escape_ascii();
        assert_eq!(has(&m.json, "availability"), !keep, "{t}");
        assert!(has(&m.json, dev), "{t}");
    }
    // expire_after = max(3 x interval, 60).
    for (interval, expire) in [(30u16, 90), (3600, 10800), (2, 60), (20, 60), (21, 63)] {
        c.publish_interval_s = interval;
        let w = all(&c);
        let want = format!("\"expire_after\":{expire},");
        assert!(has(
            &json(&w, "homeassistant/sensor/VdMot/valves_temp1_Bad_1/config"),
            &want
        ));
        assert!(has(
            &json(&w, "homeassistant/sensor/VdMot/volts_Batt/config"),
            &want
        ));
    }
    // payload_stop only with protocol 3.
    c.stm_v3 = true;
    assert!(has(
        &json(
            &all(&c),
            "homeassistant/valve/VdMot/valves_target_Bad_1/config"
        ),
        "\"reports_position\":true,\"qos\":1,\"payload_stop\":\"STOP\",\"device\""
    ));
}

#[test]
fn discovery_new_per_valve_entities_e29_1_w15_1() {
    let mut c = new_ctx();
    valve(&mut c, 0, "Bad_1", false, false, b"Bad 1");
    valve(&mut c, 1, "2", false, false, b"");
    let v = all(&c);
    let dev = DEVICE;
    assert_text(
        &json(&v, "homeassistant/sensor/VdMot/valves_actual_Bad_1/config"),
        format!(
            "{{\"name\":\"Bad 1 position\",\"unique_id\":\"VdMot.valves.Bad_1.actual\",\
             \"state_topic\":\"VdMot/valves/Bad_1/actual/value\",\"icon\":\"mdi:valve\",\
             \"state_class\":\"measurement\",\"unit_of_measurement\":\"%\",{ESP_STM}{dev}"
        ),
    );
    assert_text(
        &json(
            &v,
            "homeassistant/sensor/VdMot/diag_earlyStops_Bad_1/config",
        ),
        format!(
            "{{\"name\":\"Bad 1 early stops\",\"unique_id\":\"VdMot.diag.Bad_1.earlyStops\",\
             \"state_topic\":\"VdMot/diag/valves/Bad_1/earlyStops\",\
             \"icon\":\"mdi:alert-outline\",\"state_class\":\"total_increasing\",\
             \"entity_category\":\"diagnostic\",{ESP_STM}{dev}"
        ),
    );
    assert_text(
        &json(&v, "homeassistant/sensor/VdMot/diag_cmdRejected_2/config"),
        format!(
            "{{\"name\":\"Valve 2 rejected commands\",\"unique_id\":\"VdMot.diag.2.cmdRejected\",\
             \"state_topic\":\"VdMot/diag/valves/2/cmdRejected\",\
             \"icon\":\"mdi:alert-outline\",\"state_class\":\"total_increasing\",\
             \"entity_category\":\"diagnostic\",{ESP_STM}{dev}"
        ),
    );
    assert_text(
        &json(&v, "homeassistant/sensor/VdMot/diag_lastStop_Bad_1/config"),
        format!(
            "{{\"name\":\"Bad 1 last stop\",\"unique_id\":\"VdMot.diag.Bad_1.lastStop\",\
             \"state_topic\":\"VdMot/diag/valves/Bad_1/lastMove\",\
             \"value_template\":\"{{{{ value_json.stop }}}}\",\
             \"icon\":\"mdi:stop-circle-outline\",\"device_class\":\"enum\",\
             \"options\":[\"none\",\"target\",\"endstop\",\"early_endstop\",\"timeout\",\
             \"undercurrent\",\"safety_overcurrent\",\"aborted\"],\
             \"entity_category\":\"diagnostic\",{ESP_STM}{dev}"
        ),
    );
    assert_text(
        &json(&v, "homeassistant/sensor/VdMot/diag_calState_Bad_1/config"),
        format!(
            "{{\"name\":\"Bad 1 calibration state\",\"unique_id\":\"VdMot.diag.Bad_1.calState\",\
             \"state_topic\":\"VdMot/diag/valves/Bad_1/calState\",\
             \"icon\":\"mdi:progress-wrench\",\"entity_category\":\"diagnostic\",{ESP_STM}{dev}"
        ),
    );
    assert_text(
        &json(
            &v,
            "homeassistant/binary_sensor/VdMot/valves_problem_Bad_1/config",
        ),
        format!(
            "{{\"name\":\"Bad 1 problem\",\"unique_id\":\"VdMot.valves.Bad_1.problem\",\
             \"state_topic\":\"VdMot/valves/Bad_1/problem/value\",\"device_class\":\"problem\",\
             \"payload_on\":\"1\",\"payload_off\":\"0\",{ESP}{dev}"
        ),
    );
    assert_text(
        &json(
            &v,
            "homeassistant/sensor/VdMot/valves_failsafe_Bad_1/config",
        ),
        format!(
            "{{\"name\":\"Bad 1 failsafe\",\"unique_id\":\"VdMot.valves.Bad_1.failsafe\",\
             \"state_topic\":\"VdMot/valves/Bad_1/failsafe/value\",\
             \"icon\":\"mdi:shield-alert-outline\",\"device_class\":\"enum\",\
             \"options\":[\"off\",\"lease\",\"blocked\"],{ESP_STM}{dev}"
        ),
    );
    assert_text(
        &json(&v, "homeassistant/sensor/VdMot/valves_sync_Bad_1/config"),
        format!(
            "{{\"name\":\"Bad 1 target delivery\",\"unique_id\":\"VdMot.valves.Bad_1.sync\",\
             \"state_topic\":\"VdMot/valves/Bad_1/sync/value\",\"device_class\":\"enum\",\
             \"options\":[\"unknown\",\"synced\",\"pending\",\"await_ack\",\"await_verify\",\
             \"failed\"],\"entity_category\":\"diagnostic\",{ESP}{dev}"
        ),
    );
    assert_text(
        &json(
            &v,
            "homeassistant/button/VdMot/valves_calibrate_Bad_1/config",
        ),
        format!(
            "{{\"name\":\"Bad 1 calibrate\",\"unique_id\":\"VdMot.valves.Bad_1.calibrate\",\
             \"command_topic\":\"VdMot/cmd/valves/Bad_1/calibrate\",\
             \"icon\":\"mdi:tune-vertical\",\"entity_category\":\"config\",{ESP_STM}{dev}"
        ),
    );
    // Not separate: the suffix goes, the buttons stay.
    c.topics.separate = false;
    let p = all(&c);
    assert!(has(
        &json(
            &p,
            "homeassistant/binary_sensor/VdMot/valves_problem_2/config"
        ),
        "\"state_topic\":\"VdMot/valves/2/problem\","
    ));
    assert!(has(
        &json(&p, "homeassistant/valve/VdMot/valves_target_2/config"),
        "\"state_topic\":\"VdMot/valves/2/target\",\"command_topic\":\"VdMot/valves/2/target\","
    ));
}

#[test]
fn discovery_device_entities_k3_2_e29_1_w15_2() {
    let mut c = new_ctx();
    c.stm_v3 = true;
    let v = all(&c);
    assert_eq!(v.len(), 4 + 23);
    let dev = DEVICE;
    let e = ESP;
    let es = ESP_STM;
    assert_text(
        &json(&v, &tail(0)),
        format!(
            "{{\"name\":\"ESP online\",\"unique_id\":\"VdMot.diag.esp.online\",\
             \"state_topic\":\"VdMot/status\",\"device_class\":\"connectivity\",\
             \"payload_on\":\"online\",\"payload_off\":\"offline\",\
             \"entity_category\":\"diagnostic\",{dev}"
        ),
    );
    assert_text(
        &json(&v, &tail(1)),
        format!(
            "{{\"name\":\"STM link\",\"unique_id\":\"VdMot.diag.stm.online\",\
             \"state_topic\":\"VdMot/stm/status\",\"device_class\":\"connectivity\",\
             \"payload_on\":\"online\",\"payload_off\":\"offline\",\
             \"entity_category\":\"diagnostic\",{e}{dev}"
        ),
    );
    assert_text(
        &json(&v, &tail(2)),
        format!(
            "{{\"name\":\"Failsafe active\",\"unique_id\":\"VdMot.diag.failsafe\",\
             \"state_topic\":\"VdMot/failsafe\",\"device_class\":\"problem\",\
             \"payload_on\":\"1\",\"payload_off\":\"0\",{es}{dev}"
        ),
    );
    assert_text(
        &json(&v, &tail(3)),
        format!(
            "{{\"name\":\"Lease\",\"unique_id\":\"VdMot.diag.stm.lease\",\
             \"state_topic\":\"VdMot/diag/stm/lease\",\"device_class\":\"enum\",\
             \"options\":[\"off\",\"running\",\"expired\"],\
             \"entity_category\":\"diagnostic\",{es}{dev}"
        ),
    );
    assert_text(
        &json(&v, &tail(4)),
        format!(
            "{{\"name\":\"STM safe mode\",\"unique_id\":\"VdMot.diag.stm.safeMode\",\
             \"state_topic\":\"VdMot/diag/stm/safeMode\",\"device_class\":\"problem\",\
             \"payload_on\":\"1\",\"payload_off\":\"0\",\
             \"entity_category\":\"diagnostic\",{es}{dev}"
        ),
    );
    assert_text(
        &json(&v, &tail(5)),
        format!(
            "{{\"name\":\"STM link state\",\"unique_id\":\"VdMot.diag.stm.link\",\
             \"state_topic\":\"VdMot/diag/stm/link\",\"icon\":\"mdi:lan-connect\",\
             \"device_class\":\"enum\",\"options\":[\"unknown\",\"up\",\"degraded\",\"down\",\
             \"booting\",\"suspended\"],\"entity_category\":\"diagnostic\",{e}{dev}"
        ),
    );
    assert_text(
        &json(&v, &tail(6)),
        format!(
            "{{\"name\":\"STM protocol\",\"unique_id\":\"VdMot.diag.stm.proto\",\
             \"state_topic\":\"VdMot/diag/stm/proto\",\
             \"entity_category\":\"diagnostic\",{es}{dev}"
        ),
    );
    assert_text(
        &json(&v, &tail(7)),
        format!(
            "{{\"name\":\"STM firmware\",\"unique_id\":\"VdMot.diag.stm.version\",\
             \"state_topic\":\"VdMot/diag/stm/version\",\"icon\":\"mdi:chip\",\
             \"entity_category\":\"diagnostic\",{es}{dev}"
        ),
    );
    assert_text(
        &json(&v, &tail(8)),
        format!(
            "{{\"name\":\"STM started\",\"unique_id\":\"VdMot.diag.stm.started\",\
             \"state_topic\":\"VdMot/diag/stm/started\",\"device_class\":\"timestamp\",\
             \"entity_category\":\"diagnostic\",{es}{dev}"
        ),
    );
    let counters = [
        ("resets", "STM resets"),
        ("rxOverflow", "STM receive overflows"),
        ("parseErr", "STM parse errors"),
    ];
    for (k, (key, name)) in counters.iter().enumerate() {
        assert_text(
            &json(&v, &tail(9 + k)),
            format!(
                "{{\"name\":\"{name}\",\"unique_id\":\"VdMot.diag.stm.{key}\",\
                 \"state_topic\":\"VdMot/diag/stm/{key}\",\
                 \"state_class\":\"total_increasing\",\
                 \"entity_category\":\"diagnostic\",{es}{dev}"
            ),
        );
    }
    assert_text(
        &json(&v, &tail(12)),
        format!(
            "{{\"name\":\"Calibration running\",\"unique_id\":\"VdMot.diag.calibration.active\",\
             \"state_topic\":\"VdMot/diag/calibration/active\",\"device_class\":\"running\",\
             \"payload_on\":\"1\",\"payload_off\":\"0\",\
             \"entity_category\":\"diagnostic\",{es}{dev}"
        ),
    );
    assert_text(
        &json(&v, &tail(13)),
        format!(
            "{{\"name\":\"Next calibration\",\"unique_id\":\"VdMot.diag.calibration.next\",\
             \"state_topic\":\"VdMot/diag/calibration/next\",\
             \"device_class\":\"timestamp\",{e}{dev}"
        ),
    );
    assert_text(
        &json(&v, &tail(14)),
        format!(
            "{{\"name\":\"Suppressed events\",\
             \"unique_id\":\"VdMot.diag.mqtt.eventsSuppressed\",\
             \"state_topic\":\"VdMot/diag/mqtt/eventsSuppressed\",\
             \"state_class\":\"total_increasing\",\"entity_category\":\"diagnostic\",{e}{dev}"
        ),
    );
    assert_text(
        &json(&v, &tail(15)),
        format!(
            "{{\"name\":\"Rejected MQTT commands\",\
             \"unique_id\":\"VdMot.diag.mqtt.commandsRejected\",\
             \"state_topic\":\"VdMot/diag/mqtt/commandsRejected\",\
             \"state_class\":\"total_increasing\",\"entity_category\":\"diagnostic\",{e}{dev}"
        ),
    );
    let mut names = [""; 128];
    let n = event_mqtt_names(&mut names);
    assert!(n > 30);
    assert!(n <= 128);
    let types = names[..n]
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(",");
    assert_text(
        &json(&v, &tail(16)),
        format!(
            "{{\"name\":\"Events\",\"unique_id\":\"VdMot.events\",\
             \"state_topic\":\"VdMot/events\",\"icon\":\"mdi:bell-alert-outline\",\
             \"event_types\":[{types}],{e}{dev}"
        ),
    );
    let buttons = [
        ("Calibrate all valves", "cmd.calibrate", "calibrate"),
        ("Detect valves", "cmd.detect", "detect"),
        ("Stop valves", "cmd.stop", "stop"),
        ("Reset STM", "cmd.stmReset", "stmReset"),
        ("Restart ESP", "cmd.restart", "restart"),
        ("Leave STM safe mode", "cmd.stmSafeExit", "stmSafeExit"),
    ];
    for (k, (name, uid, cmd)) in buttons.iter().enumerate() {
        let restart = k == 3 || k == 4;
        let class = if restart {
            "\"device_class\":\"restart\","
        } else {
            ""
        };
        let avail = if restart { e } else { es };
        assert_text(
            &json(&v, &tail(17 + k)),
            format!(
                "{{\"name\":\"{name}\",\"unique_id\":\"VdMot.{uid}\",\
                 \"command_topic\":\"VdMot/cmd/{cmd}\",{class}\
                 \"entity_category\":\"config\",{avail}{dev}"
            ),
        );
    }
    // Gates: new_diag and events.
    c.new_diag = false;
    let t = topics(&c);
    assert_eq!(t.len(), 4 + 3 + 1 + 6);
    c.events = false;
    let t = topics(&c);
    assert_eq!(t.len(), 4 + 3 + 6);
    assert_text(&t[7], tail(17));
}

#[test]
fn discovery_gates_and_counts() {
    let mut c = new_ctx();
    valve(&mut c, 1, "2", true, true, b"");
    assert_eq!(topics(&c).len(), 4 + 20 + 21);
    c.publish_uptime = false;
    assert_eq!(topics(&c).len(), 3 + 20 + 21);
    c.publish_diag = false;
    assert_eq!(topics(&c).len(), 3 + 15 + 21);
    c.new_diag = false;
    let t = topics(&c);
    assert_eq!(t.len(), 3 + 11 + 8);
    // valves_actual stays without new_diag (regulator feedback).
    assert_text(&t[5], "homeassistant/sensor/VdMot/valves_actual_2/config");
    c.valves[1].has_temp1 = false;
    assert_eq!(topics(&c).len(), 3 + 10 + 8);
    c.valves[1].has_temp2 = false;
    assert_eq!(topics(&c).len(), 3 + 9 + 8);
    c.valves[1].active = false;
    assert_eq!(topics(&c).len(), 3 + 8);
    c.plain_text = false;
    c.publish_all_temps = false;
    assert_eq!(topics(&c).len(), 3 + 8);

    // Sensors.
    base(&mut c);
    c.new_diag = false;
    c.events = false;
    temp1(&mut c, 0, "1", "28-00-00-00-00-00-00-01");
    temp(&mut c, 5, "Living", "28-00-00-00-00-00-00-02", false, ""); // not published
    temp1(&mut c, 6, "7", ""); // no id
    temp1(&mut c, 33, "34", "28-00-00-00-00-00-00-03");
    c.temps[33].active = false;
    temp1(&mut c, 32, "33", "28-00-00-00-00-00-00-04");
    temp1(&mut c, 20, "21", "x"); // any non-empty id counts
    volt(&mut c, 0, "1", "", "mV", "26-00-00-00-00-00-00-05");
    volt(&mut c, 1, "Cur", "Cur", "A", "26-00-00-00-00-00-00-06");
    volt(&mut c, 2, "x", "x", "", "26-00-00-00-00-00-00-07");
    volt(&mut c, 3, "My_V", "My V", "V", "26-00-00-00-00-00-00-08");
    volt(&mut c, 4, "5", "", "V", ""); // no id
    volt(&mut c, 7, "8", "", "V", "26-00-00-00-00-00-00-09");
    c.volts[7].active = false;
    let v = all(&c);
    assert_eq!(v.len(), 4 + 3 + 4 + 7);
    assert_text(&v[4].topic, "homeassistant/sensor/VdMot/temps_1/config");
    assert_text(&v[5].topic, "homeassistant/sensor/VdMot/temps_21/config");
    assert!(has(&v[5].json, "\"unique_id\":\"VdMot.x\""));
    assert_text(&v[6].topic, "homeassistant/sensor/VdMot/temps_33/config");
    assert!(has(
        &v[6].json,
        "\"unique_id\":\"VdMot.28-00-00-00-00-00-00-04\""
    ));
    assert_text(&v[7].topic, "homeassistant/sensor/VdMot/volts_1/config");
    assert!(has(&v[7].json, "\"name\":\"volts.\",")); // legacy: raw (empty) name
    assert!(has(&v[7].json, "\"device_class\":\"voltage\""));
    assert!(has(&v[7].json, "\"unit_of_measurement\":\"mV\""));
    assert!(!has(&v[8].json, "device_class"));
    assert!(has(&v[8].json, "\"unit_of_measurement\":\"A\""));
    assert!(!has(&v[9].json, "unit_of_measurement"));
    assert!(!has(&v[9].json, "device_class"));
    assert_text(&v[10].topic, "homeassistant/sensor/VdMot/volts_My_V/config");
    assert!(has(&v[10].json, "\"name\":\"volts.My V\""));
    assert!(has(
        &v[10].json,
        "\"state_topic\":\"VdMot/sensors/My_V/value/value\""
    ));
    set(&mut c.volts[2].id, b"z");
    let v = all(&c);
    assert!(has(&v[9].json, "\"unique_id\":\"VdMot.z\""));
}

#[test]
fn discovery_e22_published_segment_and_topic_known() {
    let mut c = new_ctx();
    // slot 1 at bus index 4
    temp(&mut c, 0, "1", "28-00-00-00-00-00-00-01", true, "5");
    let v = all(&c);
    assert!(has(
        &json(&v, "homeassistant/sensor/VdMot/temps_1/config"),
        "\"state_topic\":\"VdMot/temps/5/value/value\""
    ));
    assert_eq!(
        cls(&c, "homeassistant/sensor/VdMot/temps_1/config"),
        TopicClass::Current
    );
    c.temps[0].topic_known = false; // not on the bus
    let v = all(&c);
    assert!(find(&v, "homeassistant/sensor/VdMot/temps_1/config").is_none());
    assert_eq!(
        cls(&c, "homeassistant/sensor/VdMot/temps_1/config"),
        TopicClass::KeptUnknown
    );
    volt(&mut c, 2, "3", "", "V", "26-00-00-00-00-00-00-01");
    c.volts[2].topic_known = false;
    assert_eq!(topics(&c).len(), 4 + 21);
    assert_eq!(
        cls(&c, "homeassistant/sensor/VdMot/volts_3/config"),
        TopicClass::KeptUnknown
    );
    c.temps[0].topic_known = true;
    c.temps[0].topic_segment.clear(); // known but unusable: an error, skipped
    let mut it = DiscoveryIterator::new();
    let mut m = DiscoveryMessage::default();
    let mut buf = vec![0u8; 4096];
    let mut jw = JsonWriter::new(&mut buf);
    for _ in 0..4 {
        assert!(it.next(&c, &mut m, &mut jw));
    }
    assert!(!it.next(&c, &mut m, &mut jw));
    assert!(!jw.ok());
    assert!(m.topic.is_empty());
}

#[test]
fn discovery_ha_safe_ids_raw_names_root_and_prefix_e20_2_h5() {
    let mut c = new_ctx();
    c.new_diag = false;
    c.events = false;
    set(&mut c.topics.station, b"Dom 1");
    valve(
        &mut c,
        0,
        "\u{141}azienka",
        false,
        false,
        "\u{141}azienka".as_bytes(),
    );
    let v = all(&c);
    let m = find(&v, "homeassistant/text/Dom_1/valves_state_Lazienka/config").expect("message");
    assert!(has(
        &m.json,
        "\"name\":\"valves.\u{141}azienka.state\",\"unique_id\":\"Dom_1.valves.\u{141}azienka.state\""
    ));
    assert!(has(
        &m.json,
        "\"state_topic\":\"Dom 1/valves/\u{141}azienka/state/value\""
    ));
    assert!(has(&m.json, "\"identifiers\":\"Dom 1\",\"name\":\"Dom 1\""));
    assert!(has(
        &json(
            &v,
            "homeassistant/sensor/Dom_1/valves_actual_Lazienka/config"
        ),
        "\"name\":\"\u{141}azienka position\""
    ));
    set(&mut c.discovery_prefix, b"ha");
    assert_text(&topics(&c)[4], "ha/text/Dom_1/valves_state_Lazienka/config");
    c.discovery_prefix.clear(); // empty: the legacy prefix
    assert_text(
        &topics(&c)[4],
        "homeassistant/text/Dom_1/valves_state_Lazienka/config",
    );
    let bad: [&[u8]; 8] = [
        b"a b",
        b"/ha",
        b"ha/",
        b"a//b",
        b"h+",
        b"h#",
        b"\xc3\xa4",
        b"/",
    ];
    for b in bad {
        set(&mut c.discovery_prefix, b);
        assert_text(&topics(&c)[0], "homeassistant/text/Dom_1/state/config");
    }
    for ok in ["a/b-c_D/9", "Z", "-"] {
        set(&mut c.discovery_prefix, ok.as_bytes());
        assert_text(&topics(&c)[0], format!("{ok}/text/Dom_1/state/config"));
    }
    fill_all(&mut c.discovery_prefix, b'p'); // unterminated: 32 chars
    assert_text(
        &topics(&c)[0],
        format!("{}/text/Dom_1/state/config", "p".repeat(32)),
    );

    // Legacy override "Bad/WC": topics with the '/', unique_id raw, object id safe.
    base(&mut c);
    c.new_diag = false;
    c.events = false;
    valve(&mut c, 2, "Bad/WC", false, false, b"");
    let v = all(&c);
    let m = find(&v, "homeassistant/text/VdMot/valves_state_Bad_WC/config").expect("message");
    assert!(has(
        &m.json,
        "\"unique_id\":\"VdMot.valves.Bad/WC.state\",\
         \"state_topic\":\"VdMot/valves/Bad/WC/state/value\""
    ));
    assert!(has(
        &json(&v, "homeassistant/valve/VdMot/valves_target_Bad_WC/config"),
        "\"command_topic\":\"VdMot/valves/Bad/WC/target/set\""
    ));
    assert!(has(
        &json(
            &v,
            "homeassistant/button/VdMot/valves_calibrate_Bad_WC/config"
        ),
        "\"command_topic\":\"VdMot/cmd/valves/Bad/WC/calibrate\""
    ));
    // Root VdMotFBH with the station VdMot: topics under the root, ids under the station.
    set(&mut c.station, b"VdMot");
    set(&mut c.topics.station, b"VdMotFBH");
    let v = all(&c);
    let m = find(&v, "homeassistant/text/VdMot/valves_state_Bad_WC/config").expect("message");
    assert!(has(
        &m.json,
        "\"unique_id\":\"VdMot.valves.Bad/WC.state\",\
         \"state_topic\":\"VdMotFBH/valves/Bad/WC/state/value\""
    ));
    assert!(has(
        &json(
            &v,
            "homeassistant/binary_sensor/VdMot/diag_esp_online/config"
        ),
        "\"state_topic\":\"VdMotFBH/status\""
    ));
    assert!(has(
        &json(
            &v,
            "homeassistant/binary_sensor/VdMot/diag_stm_online/config"
        ),
        "\"availability\":[{\"topic\":\"VdMotFBH/status\"}]"
    ));
    assert!(has(&m.json, "\"identifiers\":\"VdMot\""));
}

#[test]
fn discovery_v20_topic_the_2_0_0_form_of_changed_ids_e20_3() {
    let mut c = new_ctx();
    set(&mut c.topics.station, b"Dom 1");
    valve(&mut c, 0, "\u{141}azienka", false, false, b"");
    valve(&mut c, 1, "Bad_1", false, false, b"");
    set(&mut c.discovery_prefix, b"homeassistant");
    let mut it = DiscoveryIterator::new();
    let mut m = DiscoveryMessage::default();
    let mut old = DiscoveryMessage::default();
    assert!(!it.v20_topic(&c, &mut old)); // nothing produced yet
    let mut olds: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    while it.next_topic(&c, &mut m) {
        if it.v20_topic(&c, &mut old) {
            olds.insert(m.topic.to_vec(), old.topic.to_vec());
        }
        assert!(olds.len() <= 309);
    }
    let old_of = |t: &str| olds.get(t.as_bytes()).cloned().unwrap_or_default();
    assert_text(
        &old_of("homeassistant/text/Dom_1/valves_state_Lazienka/config"),
        "homeassistant/text/Dom 1/valves_state_\u{141}azienka/config",
    );
    assert_text(
        &old_of("homeassistant/text/Dom_1/state/config"),
        "homeassistant/text/Dom 1/state/config",
    );
    assert_text(
        &old_of("homeassistant/binary_sensor/Dom_1/diag_esp_online/config"),
        "homeassistant/binary_sensor/Dom 1/diag_esp_online/config",
    );
    // The same with a safe station and segment: nothing differs.
    base(&mut c);
    valve(&mut c, 1, "Bad_1", false, false, b"");
    let mut same = DiscoveryIterator::new();
    let mut produced = 0;
    while same.next_topic(&c, &mut m) {
        produced += 1;
        assert!(!same.v20_topic(&c, &mut old));
        assert!(old.topic.is_empty());
        assert!(produced <= 309);
    }
    assert!(produced > 20);
    // Another prefix: the legacy prefix differs.
    set(&mut c.discovery_prefix, b"ha");
    let mut pre = DiscoveryIterator::new();
    assert!(pre.next_topic(&c, &mut m));
    assert_text(&m.topic, "ha/text/VdMot/state/config");
    assert!(pre.v20_topic(&c, &mut old));
    assert_text(&old.topic, "homeassistant/text/VdMot/state/config");
}

#[test]
fn discovery_device_block_with_hw_version_and_variants_w15_4() {
    let mut c = new_ctx();
    set(&mut c.hw_version, b"C2");
    let v = all(&c);
    assert!(has(
        &v[0].json,
        "\"sw_version\":\"2.1.0-revamped\",\"hw_version\":\"C2\",\"model\""
    ));
    c.hw_version.clear();
    let v = all(&c);
    assert!(!has(&v[0].json, "hw_version"));
    fill_all(&mut c.hw_version, b'C'); // unterminated: bounded
    let v = all(&c);
    assert!(has(&v[0].json, "\"hw_version\":\"CCC\""));
    c.hw_version.clear();
    c.topics.path_as_root = true;
    let v = all(&c);
    assert!(has(
        &v[0].json,
        "\"state_topic\":\"/VdMot/common/state/value\""
    ));
    assert!(has(
        &v[0].json,
        "\"command_topic\":\"/VdMot/common/state/set\""
    ));
    assert!(has(
        &json(&v, &tail(1)),
        "\"availability\":[{\"topic\":\"/VdMot/status\"}]"
    ));
    c.topics.path_as_root = false;
    c.ip.clear();
    set(&mut c.sw_version, b"2.0\"x\\");
    let v = all(&c);
    assert!(!has(&v[0].json, "configuration_url"));
    assert!(has(&v[0].json, "\"sw_version\":\"2.0\\\"x\\\\\""));
    set(&mut c.ip, b"1");
    let v = all(&c);
    assert!(has(&v[0].json, "\"configuration_url\":\"http://1/\""));
    fill_all(&mut c.ip, b'1');
    fill_all(&mut c.sw_version, b'v');
    let v = all(&c);
    assert!(has(
        &v[0].json,
        format!("\"configuration_url\":\"http://{}/\"", "1".repeat(15))
    ));
    assert!(has(
        &v[0].json,
        format!("\"sw_version\":\"{}\"", "v".repeat(31))
    ));
    // A station with a space: raw in names and device, '_' in unique_ids and node ids.
    base(&mut c);
    set(&mut c.topics.station, b"My Station");
    let v = all(&c);
    assert_text(&v[0].topic, "homeassistant/text/My_Station/state/config");
    assert!(has(&v[0].json, "\"unique_id\":\"My_Station.common.state\""));
    assert!(has(
        &v[0].json,
        "\"state_topic\":\"My Station/common/state/value\""
    ));
    assert!(has(
        &v[0].json,
        "\"identifiers\":\"My Station\",\"name\":\"My Station\""
    ));
}

#[test]
fn discovery_refuses_a_missing_or_unsafe_station() {
    let mut c = new_ctx();
    valve(&mut c, 0, "1", false, false, b"");
    let mut buf = vec![0u8; 4096];
    let bad: [&[u8]; 4] = [b"", b"a+b", b"a/b", b"x\xc3"];
    for b in bad {
        set(&mut c.topics.station, b);
        let mut it = DiscoveryIterator::new();
        let mut m = DiscoveryMessage::default();
        let mut jw = JsonWriter::new(&mut buf);
        assert!(!it.next(&c, &mut m, &mut jw));
        assert!(jw.ok());
        assert!(m.topic.is_empty());
        let mut t = DiscoveryIterator::new();
        assert!(!t.next_topic(&c, &mut m));
        let mut d = DropListIterator::new();
        assert!(!d.next(&c, &mut m));
        assert_eq!(
            cls(&c, "homeassistant/text/x/state/config"),
            TopicClass::Stale
        );
    }
    // C++ a station array of 21 'x' without a NUL ("off, not a failed entity"): no Rust form,
    // a Text<20> holds at most 20 bytes; the longest valid station follows.
    let mut m = DiscoveryMessage::default();
    let mut jw = JsonWriter::new(&mut buf);
    // The longest valid station works; the explicit station wins over the root.
    fill_all(&mut c.topics.station, b'x');
    let mut ok = DiscoveryIterator::new();
    assert!(ok.next(&c, &mut m, &mut jw));
    assert_text(
        &m.topic,
        format!("homeassistant/text/{}/state/config", "x".repeat(20)),
    );
    set(&mut c.station, b"St");
    let mut st = DiscoveryIterator::new();
    assert!(st.next(&c, &mut m, &mut jw));
    assert_text(&m.topic, "homeassistant/text/St/state/config");
    set(&mut c.station, b"a+b"); // an unsafe station is not replaced by the root
    let mut bad = DiscoveryIterator::new();
    assert!(!bad.next(&c, &mut m, &mut jw));
    assert!(jw.ok());
}

#[test]
fn discovery_skips_an_entity_that_cannot_be_built_and_continues() {
    let mut c = new_ctx();
    c.new_diag = false;
    c.publish_diag = false;
    c.publish_uptime = false;
    c.events = false;
    valve(&mut c, 0, "a//b", false, false, b""); // invalid segment
    valve(&mut c, 1, "2", false, false, b"");
    let mut it = DiscoveryIterator::new();
    let mut m = DiscoveryMessage::default();
    let mut buf = vec![0u8; 4096];
    let mut jw = JsonWriter::new(&mut buf);
    for _ in 0..3 {
        assert!(it.next(&c, &mut m, &mut jw));
    }
    assert_eq!(it.position(), 4);
    // Valve 0: state, target, actual, calibration x2, problem, failsafe, sync, calibrate.
    for _ in 0..9 {
        assert!(!it.next(&c, &mut m, &mut jw));
        assert!(!jw.ok());
        assert!(m.topic.is_empty());
    }
    assert!(it.next(&c, &mut m, &mut jw));
    assert!(jw.ok());
    assert_text(&m.topic, "homeassistant/text/VdMot/valves_state_2/config");
    let mut rest = 0;
    while it.next(&c, &mut m, &mut jw) {
        rest += 1;
        assert!(rest <= 309);
    }
    assert_eq!(rest, 8 + 7);
    assert!(jw.ok());
    assert_eq!(it.position(), 309);
    assert!(!it.next(&c, &mut m, &mut jw));
    // next_topic passes unbuildable entities over.
    assert_eq!(topics(&c).len(), 3 + 9 + 7);
    // Unterminated / oversize and quoted segments are rejected as well.
    let bad: [&[u8]; 5] = [b"a b", b"a\"b", b"a\\b", b"+", b"#"];
    for b in bad {
        set(&mut c.valves[0].segment, b);
        assert_eq!(topics(&c).len(), 3 + 9 + 7);
    }
    // C++ a segment array of 11 'x' without a NUL (too long): no Rust form, a Text<10> holds
    // at most 10 bytes (a valid segment).
    // A segment that build_ha_id() cannot turn into an id (none survives).
    c.valves[0].segment.clear();
    assert_eq!(topics(&c).len(), 3 + 9 + 7);
    // A writer too small for a payload: failure with the topic, then the iterator moves on.
    set(&mut c.valves[0].segment, b"1");
    it.restart();
    assert_eq!(it.position(), 0);
    let mut small = [0u8; 200];
    let mut sw = JsonWriter::new(&mut small);
    assert!(!it.next(&c, &mut m, &mut sw));
    assert!(!sw.ok());
    assert_text(&m.topic, "homeassistant/text/VdMot/state/config");
    assert_eq!(it.position(), 1);
}

#[test]
fn discovery_with_every_entity_enabled_fits_the_mqtt_buffer_w15_2() {
    let mut c = new_ctx();
    set(&mut c.topics.station, b"abcdefghijklmnopqrst");
    c.topics.path_as_root = true;
    c.stm_v3 = true;
    set(&mut c.hw_version, b"C99");
    set(&mut c.discovery_prefix, b"abcdefghijklmnopqrstuvwxyz012345");
    set(&mut c.ip, b"255.255.255.255");
    set(&mut c.sw_version, b"2.1.0-revamped-dev-0123456789ab");
    for i in 0..usize::from(VALVE_COUNT) {
        let seg = format!("Valve_{i:04}");
        valve(
            &mut c,
            i,
            &seg,
            true,
            true,
            b"\xc5\x81\xc5\x81\xc5\x81\xc5\x81\xc5\x81",
        );
    }
    for i in 0..usize::from(TEMP_SLOT_COUNT) {
        temp1(
            &mut c,
            i,
            &format!("Temp__{i:04}"),
            "28-84-37-94-97-ff-03-23",
        );
    }
    for i in 0..usize::from(VOLT_SLOT_COUNT) {
        volt(
            &mut c,
            i,
            &format!("Volt__{i:04}"),
            "Volt  0000",
            "mV",
            "26-84-37-94-97-ff-03-23",
        );
    }
    let v = all(&c);
    assert_eq!(v.len(), 309);
    let mut longest = 0;
    for m in &v {
        longest = longest.max(m.json.len());
        assert!(m.topic.len() <= DISCOVERY_TOPIC_MAX);
        assert_eq!(cls(&c, &m.topic), TopicClass::Current);
    }
    assert!(longest <= DISCOVERY_PAYLOAD_MAX);
    assert_eq!(
        json(
            &v,
            "abcdefghijklmnopqrstuvwxyz012345/event/abcdefghijklmnopqrst/events/config"
        )
        .len(),
        longest
    );
}

#[test]
fn drop_list_legacy_entities_of_both_segment_forms_once_each() {
    let mut c = new_ctx();
    valve(&mut c, 0, "Bad_1", false, false, b"");
    c.valves[0].active = false; // inactive valves are cleaned up too
    set(&mut c.valves[2].segment, b"3"); // unnamed
    set(&mut c.valves[4].segment, b"2"); // named like valve 2's number
    set(&mut c.valves[5].segment, b"a//b"); // invalid: index form only
    set(&mut c.valves[6].segment, b"a/b"); // override: index form only
    set(&mut c.discovery_prefix, b"ha"); // the literal legacy prefix
    let d = drops(&c);
    let kinds = [
        ("climate", "climate_"),
        ("number", "valves_control_dynOffs_"),
        ("number", "valves_control_min_"),
        ("number", "valves_control_max_"),
        ("select", "valves_window_state_"),
        ("switch", "valves_window_state_"),
        ("number", "valves_window_target_"),
    ];
    let mut expected = vec![
        "homeassistant/select/VdMot/heatControl/config".to_string(),
        "homeassistant/number/VdMot/parkPosition/config".to_string(),
    ];
    let mut add = |seg: &str| {
        for (comp, prefix) in kinds {
            expected.push(format!("homeassistant/{comp}/VdMot/{prefix}{seg}/config"));
        }
    };
    for i in 0..12 {
        if i == 0 {
            add("Bad_1");
        }
        if i == 4 {
            add("2");
        }
        add(&(i + 1).to_string());
    }
    assert_eq!(d.len(), expected.len());
    assert_eq!(d.len(), 2 + 7 * 14);
    for (got, want) in d.iter().zip(&expected) {
        assert_text(got, want);
    }

    let mut it = DropListIterator::new();
    let mut m = DiscoveryMessage::default();
    assert!(it.next(&c, &mut m));
    it.restart();
    assert!(it.next(&c, &mut m));
    assert_text(&m.topic, &expected[0]);
    assert!(m.remove);
    // Station with a space stays raw in the topic.
    set(&mut c.topics.station, b"My St");
    let mut s = DropListIterator::new();
    assert!(s.next(&c, &mut m));
    assert_text(&m.topic, "homeassistant/select/My St/heatControl/config");
    // Worst case lengths fit.
    set(&mut c.topics.station, b"abcdefghijklmnopqrst");
    for v in c.valves.iter_mut() {
        set(&mut v.segment, b"0123456789");
    }
    let worst = drops(&c);
    assert_eq!(worst.len(), 2 + 7 * 24);
    for t in &worst {
        assert!(t.len() <= DISCOVERY_TOPIC_MAX);
    }
    // C++ reset(o) binds another context: restart() and the other context.
    let mut o = new_ctx();
    set(&mut o.topics.station, b"Other");
    it.restart();
    assert!(it.next(&o, &mut m));
    assert_text(&m.topic, "homeassistant/select/Other/heatControl/config");
}

#[test]
fn classify_discovery_list_lines_w4_1() {
    let mut c = new_ctx();
    valve(&mut c, 0, "Bad_1", true, false, b"");
    valve(&mut c, 1, "2", false, false, b"");
    c.valves[1].temps_known = false;
    valve(&mut c, 4, "5", false, false, b"");
    temp1(&mut c, 0, "1", "28-84-37-94-97-ff-03-23");
    for t in topics(&c) {
        assert_eq!(cls(&c, &t), TopicClass::Current);
    }
    assert_eq!(
        cls(&c, "homeassistant/text/VdMot/state/config\r\n"),
        TopicClass::Current
    );
    assert_eq!(
        cls(&c, "homeassistant/text/VdMot/state/config \t"),
        TopicClass::Current
    );
    assert_eq!(
        cls(&c, "homeassistant/sensor/VdMot/valves_temp1_2/config"),
        TopicClass::KeptUnknown
    );
    assert_eq!(
        cls(&c, "homeassistant/sensor/VdMot/valves_temp2_2/config"),
        TopicClass::KeptUnknown
    );
    assert_eq!(
        cls(&c, "homeassistant/sensor/VdMot/valves_temp2_Bad_1/config"),
        TopicClass::Stale
    );
    assert_eq!(
        cls(&c, "homeassistant/text/VdMot/valves_state_5/config"),
        TopicClass::Current
    );
    c.valves[4].active = false;
    assert_eq!(
        cls(&c, "homeassistant/text/VdMot/valves_state_5/config"),
        TopicClass::Stale
    );
    assert_eq!(
        cls(&c, "homeassistant/text/OldSt/state/config"),
        TopicClass::Stale
    );
    assert_eq!(
        cls(&c, "homeassistant/sensor/VdMot/diag_stm_uptime/config"),
        TopicClass::Stale
    );
    assert_eq!(
        cls(&c, "homeassistant/climate/VdMot/climate_Bad_1/config"),
        TopicClass::Stale
    );
    // other component
    assert_eq!(
        cls(&c, "homeassistant/sensor/VdMot/state/config"),
        TopicClass::Stale
    );
    assert_eq!(
        cls(
            &c,
            "homeassistant/text/Dom 1/valves_state_\u{141}azienka/config"
        ),
        TopicClass::Stale
    );
    // other prefix
    assert_eq!(cls(&c, "ha/text/VdMot/state/config"), TopicClass::Foreign);
    let foreign: Vec<Vec<u8>> = vec![
        b"homeassistant/+/x/config".to_vec(),
        b"homeassistant/text/VdMot/#/config".to_vec(),
        b"foo/bar".to_vec(),
        vec![b'a'; 128],
        b"".to_vec(),
        b"\r\n".to_vec(),
        b"homeassistant/text/VdMot/state/confi".to_vec(),
        b"homeassistant/text/VdMot/state/config/".to_vec(),
        b" homeassistant/text/VdMot/state/config".to_vec(),
        b"homeassistant/text/VdMot/config".to_vec(),
        b"homeassistant/text/VdMot/a/b/config".to_vec(),
        b"homeassistant//VdMot/state/config".to_vec(),
        b"homeassistant/text//state/config".to_vec(),
        b"homeassistant/text/VdMot//config".to_vec(),
        b"homeassistant/text/Vd\x01Mot/state/config".to_vec(),
        b"homeassistant/text/Vd\x7fMot/state/config".to_vec(),
        b"homeassistantx/text/VdMot/state/config".to_vec(),
        b"homeassistant".to_vec(),
        b"homeassistant/".to_vec(),
        b"homeassistant/text/VdMot/state/configx".to_vec(),
    ];
    for f in &foreign {
        assert_eq!(cls(&c, f), TopicClass::Foreign, "{}", f.escape_ascii());
    }
    // C++ classifyDiscoveryTopic(c, nullptr, 5) == Foreign: no Rust form.
    // A 127-char line is still a candidate.
    let edge = format!(
        "homeassistant/text/VdMot/{}/config",
        "x".repeat(127 - 25 - 7)
    );
    assert_eq!(edge.len(), 127);
    assert_eq!(cls(&c, &edge), TopicClass::Stale);
    // len is authoritative.
    let line = b"homeassistant/text/VdMot/ip/configXYZ";
    assert_eq!(cls(&c, &line[..line.len() - 3]), TopicClass::Current);
    // The configured prefix: its lines are candidates, the legacy ones stale.
    set(&mut c.discovery_prefix, b"ha");
    assert_eq!(cls(&c, "ha/text/VdMot/state/config"), TopicClass::Current);
    assert_eq!(
        cls(&c, "homeassistant/text/VdMot/state/config"),
        TopicClass::Stale
    );
    set(&mut c.discovery_prefix, b"a/b");
    assert_eq!(cls(&c, "a/b/text/VdMot/state/config"), TopicClass::Current);
    assert_eq!(cls(&c, "a/text/VdMot/state/config"), TopicClass::Foreign);
    // Gated off -> stale.
    set(&mut c.discovery_prefix, b"homeassistant");
    c.publish_uptime = false;
    assert_eq!(
        cls(&c, "homeassistant/text/VdMot/uptime/config"),
        TopicClass::Stale
    );
    c.new_diag = false;
    assert_eq!(
        cls(&c, "homeassistant/sensor/VdMot/diag_stm_link/config"),
        TopicClass::Stale
    );
    // A valve whose sensors are known: its missing temps are stale.
    c.valves[1].temps_known = true;
    assert_eq!(
        cls(&c, "homeassistant/sensor/VdMot/valves_temp1_2/config"),
        TopicClass::Stale
    );
}

#[test]
fn classify_fuzz_only_exact_current_topics_match() {
    let mut c = new_ctx();
    valve(&mut c, 0, "Bad_1", true, true, b"");
    valve(&mut c, 5, "6", false, false, b"");
    temp1(&mut c, 2, "3", "28-84-37-94-97-ff-03-23");
    volt(&mut c, 1, "Bat", "Bat", "V", "26-00-00-00-00-00-00-01");
    let cur = topics(&c);
    assert!(cur.len() > 20);
    let mut lcg = Lcg::numerical_recipes(0x5EED_0002);
    let mut rnd = move || lcg.next_state() >> 8;
    let n = cur.len() as u32;
    let mut hits = 0;
    for _ in 0..5000 {
        let mut t = cur[(rnd() % n) as usize].clone();
        let op = rnd() % 5;
        if op == 0 {
            // C++17: the assigned value before the index
            let ch = (rnd() & 0xFF) as u8;
            let i = (rnd() % t.len() as u32) as usize;
            t[i] = ch;
        } else if op == 1 {
            let i = (rnd() % t.len() as u32) as usize;
            t.remove(i);
        } else if op == 2 {
            // GCC evaluates the arguments of insert() right to left
            let ch = (rnd() & 0xFF) as u8;
            let i = (rnd() % (t.len() as u32 + 1)) as usize;
            t.insert(i, ch);
        } else if op == 3 {
            let len = (rnd() % 160) as usize;
            t = (0..len).map(|_| (rnd() & 0xFF) as u8).collect();
        }
        let mut trimmed: &[u8] = &t;
        while let [rest @ .., b'\r' | b'\n' | b' ' | b'\t'] = trimmed {
            trimmed = rest;
        }
        let expect = cur.iter().any(|x| x.as_slice() == trimmed);
        let got = cls(&c, &t) == TopicClass::Current;
        assert_eq!(got, expect, "{}", t.escape_ascii());
        if got {
            hits += 1;
        }
    }
    assert!(hits > 800, "{hits}");
}

/// C++ `fill(f, n)` of the discovery fuzz for a field of C size n = N + 1: a length 0..=n of
/// random or printable bytes; a Rust text keeps at most N of them (no NUL needed).
fn fuzz_fill<const N: usize>(rnd: &mut impl FnMut() -> u32, f: &mut Text<N>) {
    let n = N as u32 + 1;
    let len = rnd() % (n + 1);
    f.clear();
    for _ in 0..len {
        let _ = f.push(fill_byte(rnd));
    }
}

/// One byte of the C++ fill(): any byte with probability 1/4, else printable ASCII.
fn fill_byte(rnd: &mut impl FnMut() -> u32) -> u8 {
    if rnd().is_multiple_of(4) {
        (rnd() & 0xFF) as u8
    } else {
        (0x20 + rnd() % 95) as u8
    }
}

#[test]
fn discovery_fuzz_random_context_bytes_never_overflow_or_emit_bad_json_framing() {
    let mut c = new_ctx();
    let mut lcg = Lcg::numerical_recipes(0x5EED_0003);
    let mut rnd = move || lcg.next_state() >> 8;
    let mut buf = vec![0u8; 4096];
    let mut messages = 0;
    for _ in 0..150 {
        base(&mut c);
        if rnd() % 3 == 0 {
            fuzz_fill(&mut rnd, &mut c.topics.station);
        }
        if rnd() % 3 == 0 {
            fuzz_fill(&mut rnd, &mut c.station);
        }
        if rnd() % 3 == 0 {
            fuzz_fill(&mut rnd, &mut c.discovery_prefix);
        }
        if rnd() % 3 == 0 {
            fuzz_fill(&mut rnd, &mut c.ip);
        }
        if rnd() % 3 == 0 {
            fuzz_fill(&mut rnd, &mut c.sw_version);
        }
        if rnd() % 3 == 0 {
            fuzz_fill(&mut rnd, &mut c.hw_version);
        }
        c.topics.path_as_root = rnd() % 2 != 0;
        c.publish_diag = rnd() % 2 != 0;
        c.new_diag = rnd() % 2 != 0;
        c.publish_uptime = rnd() % 2 != 0;
        c.stm_v3 = rnd() % 2 != 0;
        for v in c.valves.iter_mut() {
            v.active = rnd() % 2 != 0;
            v.has_temp1 = rnd() % 2 != 0;
            v.has_temp2 = rnd() % 2 != 0;
            v.temps_known = rnd() % 2 != 0;
            fuzz_fill(&mut rnd, &mut v.name);
            if rnd() % 2 != 0 {
                fuzz_fill(&mut rnd, &mut v.segment);
            } else {
                let n = rnd() % 12 + 1;
                set(&mut v.segment, n.to_string().as_bytes());
            }
        }
        for s in c.temps.iter_mut().chain(c.volts.iter_mut()) {
            s.active = rnd() % 2 != 0;
            s.published = rnd() % 2 != 0;
            s.topic_known = rnd() % 2 != 0;
            fuzz_fill(&mut rnd, &mut s.segment);
            fuzz_fill(&mut rnd, &mut s.topic_segment);
            fuzz_fill(&mut rnd, &mut s.name);
            fuzz_fill(&mut rnd, &mut s.id);
            fuzz_fill(&mut rnd, &mut s.unit);
        }
        let mut it = DiscoveryIterator::new();
        let mut m = DiscoveryMessage::default();
        let mut jw = JsonWriter::new(&mut buf);
        for _ in 0..1000 {
            let got = it.next(&c, &mut m, &mut jw);
            assert!(m.topic.len() <= DISCOVERY_TOPIC_MAX);
            if got {
                messages += 1;
                assert!(jw.complete());
                let j = jw.as_bytes();
                assert!(j.len() <= DISCOVERY_PAYLOAD_MAX);
                assert_eq!(j.first(), Some(&b'{'));
                assert_eq!(j.last(), Some(&b'}'));
                assert!(has(j, "\"unique_id\":\""));
                assert!(has(&m.topic, "/config"));
                assert!(j.iter().all(|&ch| ch >= 0x20)); // clean
                                                         // node and object ids
                assert!(m
                    .topic
                    .iter()
                    .all(|&ch| ch != b' ' && ch != b'+' && ch != b'#'));
            } else if jw.ok() {
                break;
            }
        }
        let mut drop = DropListIterator::new();
        for _ in 0..200 {
            if !drop.next(&c, &mut m) {
                break;
            }
            assert!(m.remove);
            assert!(m.topic.len() <= DISCOVERY_TOPIC_MAX);
        }
        let mut line: Text<DISCOVERY_TOPIC_MAX> = Text::new();
        // C++ fill(line, sizeof line - 1): a field of C size 127
        let len = rnd() % 128;
        for _ in 0..len {
            let _ = line.push(fill_byte(&mut rnd));
        }
        let _ = classify_discovery_topic(&c, c_str(&line));
    }
    assert!(messages > 500, "{messages}");
}

#[test]
fn discovery_a_volt_unit_of_the_full_8_characters_is_kept_whole() {
    let mut c = new_ctx();
    volt(
        &mut c,
        0,
        "v1",
        "Pump",
        "kWh/m3ab",
        "26-11-22-33-44-55-66-29",
    );
    assert_eq!(c.volts[0].unit.len(), 8);
    assert!(has(
        &json(&all(&c), "homeassistant/sensor/VdMot/volts_v1/config"),
        "\"unit_of_measurement\":\"kWh/m3ab\""
    ));
    fill_all(&mut c.volts[0].unit, b'u');
    assert!(has(
        &json(&all(&c), "homeassistant/sensor/VdMot/volts_v1/config"),
        "\"unit_of_measurement\":\"uuuuuuuu\","
    ));
}

// ---------------------------------------------------------------- context

pub(super) fn owid(last: u8) -> OneWireId {
    let mut o = OneWireId::default();
    o.b[0] = 0x28;
    o.b[7] = last;
    o
}

/// Every field of `c` away from its default (the C++ memset of 0x5A before the build).
fn junk(c: &mut DiscoveryContext) {
    fill_all(&mut c.topics.station, b'Z');
    c.topics.path_as_root = true;
    c.topics.separate = false;
    fill_all(&mut c.station, b'Z');
    fill_all(&mut c.discovery_prefix, b'Z');
    c.plain_text = false;
    c.publish_diag = false;
    c.publish_uptime = false;
    c.publish_all_temps = false;
    c.new_diag = false;
    c.events = false;
    c.publish_interval_s = 0x5A5A;
    c.stm_v3 = true;
    fill_all(&mut c.ip, b'Z');
    fill_all(&mut c.sw_version, b'Z');
    fill_all(&mut c.hw_version, b'Z');
    for v in c.valves.iter_mut() {
        v.active = true;
        fill_all(&mut v.segment, b'Z');
        fill_all(&mut v.name, b'Z');
        v.has_temp1 = true;
        v.has_temp2 = true;
        v.temps_known = false;
    }
    for s in c.temps.iter_mut().chain(c.volts.iter_mut()) {
        s.active = true;
        s.published = true;
        fill_all(&mut s.segment, b'Z');
        fill_all(&mut s.topic_segment, b'Z');
        s.topic_known = false;
        fill_all(&mut s.name, b'Z');
        fill_all(&mut s.id, b'Z');
        fill_all(&mut s.unit, b'Z');
    }
}

#[test]
fn build_discovery_context_and_discovery_input_key() {
    let mut cfg = Box::<Config>::default();
    set(&mut cfg.station, b"VdMot");
    set(&mut cfg.mqtt.root_topic, b"VdMotFBH");
    set(&mut cfg.mqtt.discovery_prefix, b"ha");
    cfg.mqtt.path_as_root = true;
    cfg.mqtt.separate = true;
    cfg.mqtt.plain_text = false;
    cfg.mqtt.diag = false;
    cfg.mqtt.up_time = false;
    cfg.mqtt.all_temps = false;
    cfg.mqtt.new_diag = false;
    cfg.mqtt.events = false;
    cfg.mqtt.publish_interval_s = 33;
    cfg.valves[0].active = true;
    set(&mut cfg.valves[0].name, b"Bad 1");
    set(&mut cfg.valves[1].topic, b"Bad/WC");
    cfg.temps[0].active = true;
    cfg.temps[0].id = owid(1);
    cfg.temps[1].active = true;
    cfg.temps[1].id = owid(2);
    set(&mut cfg.temps[1].name, b"Wohn zi");
    cfg.volts[0].id = owid(3); // inactive
    cfg.volts[1].id = owid(4);
    cfg.volts[1].active = true;
    set(&mut cfg.volts[1].unit, b"V");
    let mut valves = [ValveState::default(); 12];
    valves[0].known = true;
    valves[0].temp1 = 215;
    valves[1].temp2 = TEMP_READ_ERROR;
    let mut temps = [TempReading::default(); 3];
    temps[2].id = owid(1); // slot 1 at bus index 2
    let mut volts = [VoltReading::default(); 2];
    volts[1].id = owid(4);
    let mut c = Box::<DiscoveryContext>::default();
    junk(&mut c);
    {
        let input = DiscoveryInputs {
            cfg: Some(&cfg),
            valves: Some(&valves),
            temps: &temps,
            volts: &volts,
            sensors_settled: true,
            stm_proto: 3,
            stm_hw: b"C2",
            ip: 0x3201_A8C0, // 192.168.1.50
            sw_version: b"2.1.0-revamped",
        };
        assert!(build_discovery_context(&input, &mut c));
    }
    assert_text(&c.topics.station, "VdMotFBH");
    assert!(c.topics.path_as_root);
    assert!(c.topics.separate);
    assert_text(&c.station, "VdMot");
    assert_text(&c.discovery_prefix, "ha");
    assert!(!c.plain_text);
    assert!(!c.publish_diag);
    assert!(!c.publish_uptime);
    assert!(!c.publish_all_temps);
    assert!(!c.new_diag);
    assert!(!c.events);
    assert_eq!(c.publish_interval_s, 33);
    assert!(c.stm_v3);
    assert_text(&c.ip, "192.168.1.50");
    assert_text(&c.sw_version, "2.1.0-revamped");
    assert_text(&c.hw_version, "C2");
    assert!(c.valves[0].active);
    assert_text(&c.valves[0].segment, "Bad_1");
    assert_text(&c.valves[0].name, "Bad 1");
    assert!(c.valves[0].has_temp1);
    assert!(!c.valves[0].has_temp2);
    assert!(c.valves[0].temps_known);
    assert!(!c.valves[1].active);
    assert_text(&c.valves[1].segment, "Bad/WC");
    assert!(!c.valves[1].has_temp1);
    assert!(c.valves[1].has_temp2);
    assert!(!c.valves[1].temps_known); // not known yet
    assert_text(&c.valves[2].segment, "3");
    assert!(c.temps[0].active);
    assert!(c.temps[0].published);
    assert_text(&c.temps[0].segment, "1");
    assert_text(&c.temps[0].topic_segment, "3");
    assert!(c.temps[0].topic_known);
    assert_text(&c.temps[0].id, "28-00-00-00-00-00-00-01");
    assert_text(&c.temps[1].segment, "Wohn_zi");
    assert_text(&c.temps[1].topic_segment, "Wohn_zi");
    assert!(c.temps[1].topic_known);
    assert_text(&c.temps[1].name, "Wohn zi");
    assert!(!c.temps[2].active);
    assert!(!c.temps[2].topic_known);
    assert!(c.temps[2].id.is_empty());
    assert!(!c.volts[0].active); // inactive: not announced
    assert!(!c.volts[0].id.is_empty());
    assert!(!c.volts[0].topic_known);
    assert!(c.volts[1].active);
    assert!(c.volts[1].published);
    assert_text(&c.volts[1].topic_segment, "2");
    assert_text(&c.volts[1].unit, "V");
    // all_temps off: a temp assigned to a valve is not published.
    valves[3].sensor_slot[0] = 2;
    let mut input = DiscoveryInputs {
        cfg: Some(&cfg),
        valves: Some(&valves),
        temps: &temps,
        volts: &volts,
        sensors_settled: true,
        stm_proto: 3,
        stm_hw: b"C2",
        ip: 0x3201_A8C0,
        sw_version: b"2.1.0-revamped",
    };
    build_discovery_context(&input, &mut c);
    assert!(!c.temps[1].published);
    assert!(c.temps[0].published);
    // No ip, no valves: defaults.
    input.ip = 0;
    input.valves = None;
    input.stm_proto = 2;
    build_discovery_context(&input, &mut c);
    assert!(c.ip.is_empty());
    assert!(!c.stm_v3);
    assert!(!c.valves[0].temps_known);
    assert!(!c.valves[0].has_temp1);
    // Without a config: false and an empty context.
    let none = DiscoveryInputs::default();
    junk(&mut c);
    assert!(!build_discovery_context(&none, &mut c));
    assert!(!c.valves[0].active);
    assert_text(&c.discovery_prefix, "homeassistant");
    assert_eq!(discovery_input_key(&none), 0);

    // The key follows the snapshot-dependent parts only (the snapshot changes in place, the
    // inputs are taken from it for every key).
    let mut s = Snapshot {
        valves,
        temps,
        volts,
        hw: b"C2",
        proto: 3,
        ip: 0x3201_A8C0,
        sw: b"2.1.0-revamped",
    };
    let k0 = s.key(&cfg);
    assert_ne!(k0, 0);
    assert_eq!(s.key(&cfg), k0);
    s.ip = 1; // not in the key
    s.sw = b"x";
    assert_eq!(s.key(&cfg), k0);
    s.valves[5].temp1 = 200;
    let k1 = s.key(&cfg);
    assert_ne!(k1, k0);
    s.valves[5].temp2 = 200;
    let k2 = s.key(&cfg);
    assert_ne!(k2, k1);
    s.valves[5].known = true;
    let k3 = s.key(&cfg);
    assert_ne!(k3, k2);
    s.temps[2].id = owid(9); // slot 1 leaves the bus
    let k4 = s.key(&cfg);
    assert_ne!(k4, k3);
    s.temps[1].id = owid(1); // back at another index
    let k5 = s.key(&cfg);
    assert_ne!(k5, k4);
    s.volts[1].id = owid(8);
    let k6 = s.key(&cfg);
    assert_ne!(k6, k5);
    s.hw = b"C1";
    let k7 = s.key(&cfg);
    assert_ne!(k7, k6);
    s.proto = 2;
    let k8 = s.key(&cfg);
    assert_ne!(k8, k7);
    s.valves[3].sensor_slot[0] = 0; // temp 2 published again
    assert_ne!(s.key(&cfg), k8);
}

/// The STM snapshot of the key checks (settled sensors).
struct Snapshot {
    valves: [ValveState; 12],
    temps: [TempReading; 3],
    volts: [VoltReading; 2],
    hw: &'static [u8],
    proto: u8,
    ip: u32,
    sw: &'static [u8],
}

impl Snapshot {
    fn key(&self, cfg: &Config) -> u32 {
        discovery_input_key(&DiscoveryInputs {
            cfg: Some(cfg),
            valves: Some(&self.valves),
            temps: &self.temps,
            volts: &self.volts,
            sensors_settled: true,
            stm_proto: self.proto,
            stm_hw: self.hw,
            ip: self.ip,
            sw_version: self.sw,
        })
    }
}

// ---------------------------------------------------------------- list and run

/// In-memory LittleFS + broker for DiscoveryRun.
#[derive(Clone)]
pub(super) struct FakePort {
    pub(super) exists: bool,
    pub(super) list: Vec<u8>,
    pub(super) tmp: Vec<u8>,
    pub(super) writing: bool,
    pub(super) read_pos: usize,
    pub(super) reading: bool,
    /// C++ -1: never
    pub(super) fail_publish_at: Option<usize>,
    pub(super) fail_begin: bool,
    pub(super) fail_write_at: Option<usize>,
    pub(super) fail_commit: bool,
    pub(super) published: Vec<(Vec<u8>, Vec<u8>)>,
    pub(super) opens: usize,
    pub(super) reads: usize,
    pub(super) closes: usize,
    pub(super) begins: usize,
    pub(super) writes: usize,
    pub(super) commits: usize,
    pub(super) aborts: usize,
}

impl FakePort {
    pub(super) fn new() -> Self {
        Self {
            exists: true,
            list: Vec::new(),
            tmp: Vec::new(),
            writing: false,
            read_pos: 0,
            reading: false,
            fail_publish_at: None,
            fail_begin: false,
            fail_write_at: None,
            fail_commit: false,
            published: Vec::new(),
            opens: 0,
            reads: 0,
            closes: 0,
            begins: 0,
            writes: 0,
            commits: 0,
            aborts: 0,
        }
    }

    pub(super) fn with_list(list: &[u8]) -> Self {
        let mut p = Self::new();
        p.list = list.to_vec();
        p
    }

    pub(super) fn deletes(&self) -> Vec<Vec<u8>> {
        self.published
            .iter()
            .filter(|p| p.1.is_empty())
            .map(|p| p.0.clone())
            .collect()
    }

    pub(super) fn configs(&self) -> Vec<Vec<u8>> {
        self.published
            .iter()
            .filter(|p| !p.1.is_empty())
            .map(|p| p.0.clone())
            .collect()
    }
}

impl DiscoveryPort for FakePort {
    fn publish(&mut self, topic: &[u8], payload: &[u8]) -> bool {
        if Some(self.published.len()) == self.fail_publish_at {
            return false;
        }
        self.published.push((topic.to_vec(), payload.to_vec()));
        true
    }

    fn list_open(&mut self) -> bool {
        self.opens += 1;
        if !self.exists {
            return false;
        }
        self.reading = true;
        self.read_pos = 0;
        true
    }

    fn list_read(&mut self) -> Option<u8> {
        self.reads += 1;
        if !self.reading {
            return None;
        }
        let c = self.list.get(self.read_pos).copied()?;
        self.read_pos += 1;
        Some(c)
    }

    fn list_close(&mut self) {
        self.closes += 1;
        self.reading = false;
    }

    fn list_begin(&mut self) -> bool {
        self.begins += 1;
        if self.fail_begin {
            return false;
        }
        self.writing = true;
        self.tmp.clear();
        true
    }

    fn list_write(&mut self, topic: &[u8]) -> bool {
        let n = self.writes;
        self.writes += 1;
        if Some(n) == self.fail_write_at {
            return false;
        }
        self.tmp.extend_from_slice(topic);
        self.tmp.push(b'\n');
        true
    }

    fn list_commit(&mut self) -> bool {
        self.commits += 1;
        self.writing = false;
        if self.fail_commit {
            return false;
        }
        self.list = self.tmp.clone();
        self.exists = true;
        true
    }

    fn list_abort(&mut self) {
        self.aborts += 1;
        self.writing = false;
        self.tmp.clear();
    }
}

/// Steps the run until it ends (at most `max_steps` steps).
pub(super) fn run_all_max(
    run: &mut DiscoveryRun,
    c: &DiscoveryContext,
    port: &mut FakePort,
    max_steps: usize,
) -> DiscoveryRunPhase {
    let mut buf = vec![0u8; 4096];
    let mut jw = JsonWriter::new(&mut buf);
    for _ in 0..max_steps {
        if !run.running() {
            break;
        }
        run.step(c, port, &mut jw);
    }
    run.phase()
}

pub(super) fn run_all(
    run: &mut DiscoveryRun,
    c: &DiscoveryContext,
    port: &mut FakePort,
) -> DiscoveryRunPhase {
    run_all_max(run, c, port, 5000)
}

pub(super) fn lines(v: &[Vec<u8>]) -> Vec<u8> {
    let mut s = Vec::new();
    for t in v {
        s.extend_from_slice(t);
        s.push(b'\n');
    }
    s
}

pub(super) fn small(c: &mut DiscoveryContext) {
    base(c);
    c.new_diag = false;
    c.events = false;
    c.publish_diag = false;
    c.publish_uptime = false;
}

pub(super) fn small_ctx() -> Box<DiscoveryContext> {
    let mut c = Box::<DiscoveryContext>::default();
    small(&mut c);
    c
}

fn publish_plan() -> DiscoveryPlan {
    DiscoveryPlan {
        publish: true,
        ..DiscoveryPlan::default()
    }
}

fn delete_plan() -> DiscoveryPlan {
    DiscoveryPlan {
        remove_all: true,
        prune: false,
        ..DiscoveryPlan::default()
    }
}

#[test]
fn read_list_line_cr_lf_empty_and_overlong_lines() {
    let mut list = b"a\r\n\nbb\n".to_vec();
    list.extend_from_slice(&[b'x'; 128]);
    list.extend_from_slice(b"\ncc\n");
    list.extend_from_slice(&[b'y'; 127]);
    list.extend_from_slice(b"\rlast");
    let mut p = FakePort::with_list(&list);
    assert!(p.list_open());
    assert_text(&read_list_line(&mut p).expect("line"), "a");
    assert_text(&read_list_line(&mut p).expect("line"), "bb");
    // the 128-char line is dropped
    assert_text(&read_list_line(&mut p).expect("line"), "cc");
    assert_text(&read_list_line(&mut p).expect("line"), [b'y'; 127]);
    assert_text(&read_list_line(&mut p).expect("line"), "last");
    // C++ false with out = "": None
    assert!(read_list_line(&mut p).is_none());
    let mut e = FakePort::with_list(b"\n\r\n");
    assert!(e.list_open());
    assert!(read_list_line(&mut e).is_none());
    let mut o = FakePort::with_list(&[b'z'; 200]); // overlong at the end
    assert!(o.list_open());
    assert!(read_list_line(&mut o).is_none());
}

#[test]
fn discovery_run_prune_before_publish_rewrite_the_list_w4_2_w4_3() {
    let mut c = small_ctx();
    valve(&mut c, 0, "New", false, false, b"");
    temp1(&mut c, 0, "B", "28-00-00-00-00-00-00-01");
    let mut port = FakePort::with_list(
        b"homeassistant/text/VdMot/state/config\n\
          homeassistant/text/VdMot/valves_state_Old/config\n\
          homeassistant/sensor/VdMot/temps_A/config\n",
    );
    let mut run = DiscoveryRun::new();
    assert_eq!(run.phase(), DiscoveryRunPhase::Idle);
    let plan = publish_plan();
    run.start(&plan);
    assert_eq!(run.phase(), DiscoveryRunPhase::Prune);
    assert!(run.running());
    assert_eq!(run_all(&mut run, &c, &mut port), DiscoveryRunPhase::Done);
    let cur = topics(&c);
    let mut expected = vec![
        b"homeassistant/text/VdMot/valves_state_Old/config".to_vec(),
        b"homeassistant/sensor/VdMot/temps_A/config".to_vec(),
    ];
    expected.extend(cur.iter().cloned());
    let order: Vec<Vec<u8>> = port.published.iter().map(|p| p.0.clone()).collect();
    assert_eq!(order, expected);
    assert_eq!(port.deletes().len(), 2);
    assert_eq!(port.configs(), cur);
    assert_eq!(port.list, lines(&cur));
    assert_eq!(run.stats().deletes, 2);
    assert_eq!(usize::from(run.stats().configs), cur.len());
    assert_eq!(run.stats().skipped, 0);
    assert!(run.stats().list_written);
    assert_eq!(port.commits, 1);
    assert!(!port.reading);
    // Second run: nothing to delete, no list write.
    let mut again = port.clone();
    again.published.clear();
    again.begins = 0;
    again.commits = 0;
    let mut second = DiscoveryRun::new();
    second.start(&plan);
    assert_eq!(
        run_all(&mut second, &c, &mut again),
        DiscoveryRunPhase::Done
    );
    assert!(again.deletes().is_empty());
    assert_eq!(again.begins, 0);
    assert!(!second.stats().list_written);
    assert_eq!(usize::from(second.stats().configs), cur.len());
    assert_eq!(again.list, lines(&cur));
}

#[test]
fn discovery_run_kept_unknown_lines_are_carried_first_and_deleted_once_known_w4_4() {
    let mut c = small_ctx();
    valve(&mut c, 1, "2", false, false, b"");
    c.valves[1].temps_known = false;
    let kept = b"homeassistant/sensor/VdMot/valves_temp1_2/config".to_vec();
    let mut port = FakePort::with_list(&lines(core::slice::from_ref(&kept)));
    let plan = publish_plan();
    let mut run = DiscoveryRun::new();
    run.start(&plan);
    assert_eq!(run_all(&mut run, &c, &mut port), DiscoveryRunPhase::Done);
    assert!(port.deletes().is_empty());
    let cur = topics(&c);
    let mut want = lines(core::slice::from_ref(&kept));
    want.extend(lines(&cur));
    assert_eq!(port.list, want);
    // The valve's sensors become known and there is none: the next run deletes it.
    c.valves[1].temps_known = true;
    port.published.clear();
    let mut next = DiscoveryRun::new();
    next.start(&plan);
    assert_eq!(run_all(&mut next, &c, &mut port), DiscoveryRunPhase::Done);
    assert_eq!(port.deletes(), vec![kept]);
    assert_eq!(port.list, lines(&cur));
}

#[test]
fn discovery_run_delete_and_delete_and_publish_w4_5() {
    let mut c = small_ctx();
    valve(&mut c, 0, "1", false, false, b"");
    let cur = topics(&c);
    let mut list = b"homeassistant/text/VdMot/old/config\nfoo/bar\n".to_vec();
    list.extend(lines(&cur));
    let mut port = FakePort::with_list(&list);
    let del = delete_plan();
    let mut run = DiscoveryRun::new();
    run.start(&del);
    assert_eq!(run.phase(), DiscoveryRunPhase::RemoveList);
    assert_eq!(run_all(&mut run, &c, &mut port), DiscoveryRunPhase::Done);
    let mut expected = vec![b"homeassistant/text/VdMot/old/config".to_vec()];
    expected.extend(cur.iter().cloned()); // list lines
    expected.extend(cur.iter().cloned()); // current topics
    assert_eq!(port.deletes(), expected);
    assert!(port.configs().is_empty());
    assert!(port.list.is_empty());
    assert_eq!(port.commits, 1);
    assert!(run.stats().list_written);
    assert_eq!(usize::from(run.stats().deletes), expected.len());
    // DeleteAndPublish: the same deletes, then every config, then the new list.
    port.list = b"homeassistant/text/VdMot/old/config\n".to_vec();
    port.published.clear();
    let both = DiscoveryPlan {
        remove_all: true,
        publish: true,
        ..DiscoveryPlan::default()
    };
    let mut r2 = DiscoveryRun::new();
    r2.start(&both);
    assert_eq!(run_all(&mut r2, &c, &mut port), DiscoveryRunPhase::Done);
    let mut dels = vec![b"homeassistant/text/VdMot/old/config".to_vec()];
    dels.extend(cur.iter().cloned());
    assert_eq!(port.deletes(), dels);
    assert_eq!(port.configs(), cur);
    assert_eq!(port.list, lines(&cur));
    // A publish failure in the middle: Aborted, the tmp list abandoned, no commit.
    let mut f = FakePort::with_list(&lines(&cur));
    f.fail_publish_at = Some(2);
    let mut r3 = DiscoveryRun::new();
    r3.start(&del);
    assert_eq!(run_all(&mut r3, &c, &mut f), DiscoveryRunPhase::Aborted);
    assert_eq!(f.published.len(), 2);
    assert_eq!(f.commits, 0);
    assert!(!f.reading);
    assert!(!r3.running());
    // A failure while the current topics are removed: the list stays as it was.
    let mut w = FakePort::with_list(b"homeassistant/text/VdMot/x/config\n");
    w.fail_publish_at = Some(2);
    let mut r4 = DiscoveryRun::new();
    r4.start(&both);
    assert_eq!(run_all(&mut r4, &c, &mut w), DiscoveryRunPhase::Aborted);
    assert_eq!(w.commits, 0);
    assert_eq!(w.begins, 0);
    assert_text(&w.list, "homeassistant/text/VdMot/x/config\n");
    // abort() of the glue (connection lost) while the tmp list is written.
    let mut g = FakePort::new();
    g.exists = false;
    let pubp = publish_plan();
    let mut r5 = DiscoveryRun::new();
    r5.start(&pubp);
    let mut buf = vec![0u8; 4096];
    let mut jw = JsonWriter::new(&mut buf);
    let mut steps = 0;
    while r5.phase() != DiscoveryRunPhase::WriteCurrent {
        assert_ne!(r5.step(&c, &mut g, &mut jw), DiscoveryRunPhase::Done);
        steps += 1;
        assert!(steps < 5000);
    }
    r5.abort(&mut g);
    assert_eq!(r5.phase(), DiscoveryRunPhase::Aborted);
    assert_eq!(g.aborts, 1);
    assert_eq!(g.commits, 0);
    r5.abort(&mut g); // idempotent
    assert_eq!(g.aborts, 1);
    assert_eq!(r5.step(&c, &mut g, &mut jw), DiscoveryRunPhase::Aborted);
}

#[test]
fn discovery_run_missing_list_budget_failures_w4_6_w4_7_w4_10() {
    let mut c = small_ctx();
    valve(&mut c, 0, "1", false, false, b"");
    valve(&mut c, 1, "2", false, false, b"");
    let cur = topics(&c);
    let mut port = FakePort::new();
    port.exists = false;
    let plan = publish_plan();
    let mut run = DiscoveryRun::new();
    run.start(&plan);
    let mut buf = vec![0u8; 4096];
    let mut jw = JsonWriter::new(&mut buf);
    let mut steps = 0;
    while run.running() {
        let pubs = port.published.len();
        let reads = port.reads;
        let writes = port.writes;
        run.step(&c, &mut port, &mut jw);
        steps += 1;
        assert!(port.published.len() - pubs <= 1);
        assert!(writes + usize::from(DiscoveryRun::LINES_PER_STEP) >= port.writes);
        assert!(reads <= port.reads);
        assert!(steps < 1000);
    }
    assert_eq!(run.phase(), DiscoveryRunPhase::Done);
    assert!(port.deletes().is_empty());
    assert_eq!(port.list, lines(&cur));
    // Reading is bounded too: 8 lines per step.
    let mut many = Vec::new();
    for _ in 0..40 {
        many.extend_from_slice(b"homeassistant/text/VdMot/state/config\n");
    }
    let mut big = FakePort::with_list(&many);
    let cleanup = DiscoveryPlan::default(); // no publish: Current lines are carried
    let mut r = DiscoveryRun::new();
    r.start(&cleanup);
    assert_eq!(r.phase(), DiscoveryRunPhase::Prune);
    r.step(&c, &mut big, &mut jw);
    assert_eq!(
        big.reads,
        8 * b"homeassistant/text/VdMot/state/config\n".len()
    );
    assert_eq!(run_all(&mut r, &c, &mut big), DiscoveryRunPhase::Done);
    assert_eq!(big.list, many); // unchanged content: no rewrite
    assert_eq!(big.begins, 0);
    // An unsafe station: Done after one step, nothing touched.
    let mut u = small_ctx();
    set(&mut u.topics.station, b"a+b");
    let mut up = FakePort::with_list(b"homeassistant/text/x/state/config\n");
    let mut ru = DiscoveryRun::new();
    ru.start(&plan);
    assert_eq!(ru.step(&u, &mut up, &mut jw), DiscoveryRunPhase::Done);
    assert!(up.published.is_empty());
    assert_eq!(up.opens, 0);
    assert_text(&up.list, "homeassistant/text/x/state/config\n");
    // list_begin failing: every config published, the list not written.
    let mut nb = FakePort::new();
    nb.exists = false;
    nb.fail_begin = true;
    let mut rb = DiscoveryRun::new();
    rb.start(&plan);
    assert_eq!(run_all(&mut rb, &c, &mut nb), DiscoveryRunPhase::Done);
    assert_eq!(nb.configs(), cur);
    assert!(!rb.stats().list_written);
    assert_eq!(nb.aborts, 1);
    assert_eq!(nb.commits, 0);
    // list_write failing: abandoned, Done.
    let mut nw = FakePort::new();
    nw.exists = false;
    nw.fail_write_at = Some(3);
    let mut rw = DiscoveryRun::new();
    rw.start(&plan);
    assert_eq!(run_all(&mut rw, &c, &mut nw), DiscoveryRunPhase::Done);
    assert!(!rw.stats().list_written);
    assert_eq!(nw.aborts, 1);
    assert_eq!(nw.commits, 0);
    assert!(!nw.exists);
    // list_commit failing.
    let mut nc = FakePort::new();
    nc.exists = false;
    nc.fail_commit = true;
    let mut rc = DiscoveryRun::new();
    rc.start(&plan);
    assert_eq!(run_all(&mut rc, &c, &mut nc), DiscoveryRunPhase::Done);
    assert!(!rc.stats().list_written);
    assert_eq!(nc.aborts, 1);
    // A carried line failing to write.
    let mut kw = FakePort::with_list(
        b"homeassistant/text/VdMot/state/config\nhomeassistant/text/VdMot/x/config\n",
    );
    kw.fail_write_at = Some(0);
    let mut rk = DiscoveryRun::new();
    rk.start(&cleanup);
    assert_eq!(run_all(&mut rk, &c, &mut kw), DiscoveryRunPhase::Done);
    assert_eq!(kw.aborts, 1);
    assert!(!rk.stats().list_written);
    // A ClearList whose list_begin fails.
    let mut cb = FakePort::new();
    cb.fail_begin = true;
    let del = delete_plan();
    let mut rd = DiscoveryRun::new();
    rd.start(&del);
    assert_eq!(run_all(&mut rd, &c, &mut cb), DiscoveryRunPhase::Done);
    assert_eq!(cb.aborts, 1);
    assert!(!rd.stats().list_written);
    // A run that was never started does nothing.
    let mut idle = DiscoveryRun::new();
    assert_eq!(idle.step(&c, &mut port, &mut jw), DiscoveryRunPhase::Idle);
    assert!(!idle.running());
    idle.abort(&mut port);
    assert_eq!(idle.phase(), DiscoveryRunPhase::Idle);
}

#[test]
fn discovery_run_first_run_cleanup_migration_foreign_and_oversize_entries() {
    let mut c = small_ctx();
    set(&mut c.topics.station, b"Dom 1");
    valve(&mut c, 0, "1", false, false, b"");
    let cur = topics(&c);
    // Mode 1 cleanup: DROP list and prune, Current and KeptUnknown carried, no publish.
    let mut list = b"foo/bar\n".to_vec();
    list.extend_from_slice(&cur[0]);
    list.extend_from_slice(b"\nhomeassistant/text/Dom 1/state/config\n");
    let mut port = FakePort::with_list(&list);
    let cleanup = DiscoveryPlan {
        drop_legacy: true,
        ..DiscoveryPlan::default()
    };
    let mut run = DiscoveryRun::new();
    run.start(&cleanup);
    assert_eq!(run.phase(), DiscoveryRunPhase::DropList);
    assert_eq!(run_all(&mut run, &c, &mut port), DiscoveryRunPhase::Done);
    let d = port.deletes();
    assert_eq!(d.len(), 2 + 7 * 12 + 1);
    assert_text(&d[0], "homeassistant/select/Dom 1/heatControl/config");
    assert_text(
        d.last().expect("deletes"),
        "homeassistant/text/Dom 1/state/config",
    );
    assert!(port.configs().is_empty());
    assert_eq!(port.list, lines(&cur[..1]));
    assert!(run.stats().list_written);
    // Migration: uptime and the 2.0.0 forms of changed ids, then the configs.
    let mut mig = FakePort::new();
    mig.exists = false;
    let plan = DiscoveryPlan {
        publish: true,
        retire20: true,
        ..DiscoveryPlan::default()
    };
    let mut m = DiscoveryRun::new();
    m.start(&plan);
    assert_eq!(m.phase(), DiscoveryRunPhase::Retired);
    assert_eq!(run_all(&mut m, &c, &mut mig), DiscoveryRunPhase::Done);
    let mut dels = vec![b"homeassistant/sensor/Dom 1/diag_stm_uptime/config".to_vec()];
    for t in &cur {
        let s = String::from_utf8(t.clone()).expect("utf-8");
        dels.push(s.replacen("Dom_1", "Dom 1", 1).into_bytes());
    }
    assert_eq!(mig.deletes(), dels);
    assert_eq!(mig.configs(), cur);
    // retire20 without publish is ignored.
    let r20 = DiscoveryPlan {
        retire20: true,
        ..DiscoveryPlan::default()
    };
    let mut rn = DiscoveryRun::new();
    rn.start(&r20);
    assert_eq!(rn.phase(), DiscoveryRunPhase::Prune);
    // An entity whose payload does not fit: skipped, its topic still listed.
    let mut big = small_ctx();
    big.events = true;
    let mut bp = FakePort::new();
    bp.exists = false;
    let mut rb = DiscoveryRun::new();
    rb.start(&plan);
    let mut tiny = [0u8; 1200];
    let mut jw = JsonWriter::new(&mut tiny);
    for _ in 0..2000 {
        if !rb.running() {
            break;
        }
        rb.step(&big, &mut bp, &mut jw);
    }
    assert_eq!(rb.phase(), DiscoveryRunPhase::Done);
    assert_eq!(rb.stats().skipped, 1); // the event entity
    assert!(has(&bp.list, "homeassistant/event/VdMot/events/config\n"));
    assert_eq!(bp.configs().len() + 1, topics(&big).len());
}

// ---------------------------------------------------------------- edge cases

#[test]
fn discovery_a_one_char_station_and_a_one_char_prefix() {
    let mut c = small_ctx();
    set(&mut c.station, b"X");
    set(&mut c.discovery_prefix, b"a");
    let v = all(&c);
    assert!(!v.is_empty());
    assert_text(&v[0].topic, "a/text/X/state/config");
    assert!(has(&v[0].json, "\"identifiers\":\"X\""));
    assert!(has(&v[0].json, "\"unique_id\":\"X.common.state\""));
}

#[test]
fn discovery_expire_after_is_three_publish_intervals_at_least_60_s() {
    let mut c = small_ctx();
    valve(&mut c, 0, "1", true, false, b"");
    for (interval, want) in [(19u16, "60"), (20, "60"), (21, "63")] {
        c.publish_interval_s = interval;
        assert!(
            has(
                &json(&all(&c), "homeassistant/sensor/VdMot/valves_temp1_1/config"),
                format!("\"expire_after\":{want},")
            ),
            "{interval}"
        );
    }
}

#[test]
fn discovery_a_valve_name_of_the_full_10_chars_is_kept_whole_in_entity_names() {
    let mut c = small_ctx();
    valve(&mut c, 0, "1", false, false, b"ABCDEFGHIJ");
    assert!(has(
        &json(
            &all(&c),
            "homeassistant/sensor/VdMot/valves_actual_1/config"
        ),
        "\"name\":\"ABCDEFGHIJ position\""
    ));
}

#[test]
fn discovery_the_event_entity_lists_fewer_than_127_event_types() {
    assert!(event_mqtt_names(&mut []) < 127);
}

#[test]
fn classify_the_shape_of_a_config_topic() {
    let c = small_ctx();
    assert_eq!(cls(&c, "a/b/c/config"), TopicClass::Foreign);
    let foreign = [
        "Xa/b/c/config".to_string(),
        "abcdefghijklm/sensor/VdMot/x/config".to_string(),
        "homeassistant/sensor/VdMot/x/confiG".to_string(),
        format!("homeassistant/sensor/VdMot/{}/config", "x".repeat(110)),
    ];
    for t in &foreign {
        assert_eq!(cls(&c, t), TopicClass::Foreign, "{t}");
    }
    let one_char = "homeassistant/s/VdMot/x/config";
    assert_eq!(cls(&c, one_char), TopicClass::Stale);
}

#[test]
fn v20_topic_nothing_for_an_unsafe_station_or_a_skipped_last_entity() {
    let mut c = small_ctx();
    set(&mut c.station, b"a/b");
    let mut bad = DiscoveryIterator::new();
    let mut m = DiscoveryMessage::default();
    assert!(!bad.next_topic(&c, &mut m));
    assert_eq!(bad.position(), 309);
    assert!(!bad.v20_topic(&c, &mut m));
    small(&mut c);
    set(&mut c.station, b"Dom 1");
    let mut it = DiscoveryIterator::new();
    let mut n = 0;
    while it.next_topic(&c, &mut m) {
        n += 1;
        assert!(n <= 309);
    }
    assert_eq!(it.position(), 309); // the last entity (leave safe mode) needs STM v3
    assert!(!it.v20_topic(&c, &mut m));
}

#[test]
fn discovery_iterator_reset_starts_again_at_the_first_entity() {
    let c = small_ctx();
    let mut it = DiscoveryIterator::new();
    let mut first = DiscoveryMessage::default();
    let mut m = DiscoveryMessage::default();
    assert!(it.next_topic(&c, &mut first));
    assert!(it.next_topic(&c, &mut m));
    // C++ reset(c): restart() with the same context.
    it.restart();
    assert_eq!(it.position(), 0);
    assert!(it.next_topic(&c, &mut m));
    assert_eq!(m.topic, first.topic);
}

#[test]
fn read_list_line_a_nul_byte_does_not_end_a_line() {
    let mut p = FakePort::with_list(b"ab\0cd\n");
    assert!(p.list_open());
    assert_text(&read_list_line(&mut p).expect("line"), "ab");
    assert!(read_list_line(&mut p).is_none());
}

#[test]
fn discovery_input_key_crc32_over_the_snapshot_parts_in_their_order() {
    let mut cfg = Box::<Config>::default();
    set(&mut cfg.station, b"VdMot");
    cfg.temps[0].active = true;
    cfg.temps[0].id = owid(1);
    cfg.temps[3].id = owid(5); // inactive with an id: not active
    cfg.volts[1].active = true;
    cfg.volts[1].id = owid(4);
    let mut valves = [ValveState::default(); 12];
    valves[0].known = true;
    valves[0].temp1 = 215;
    let mut temps = [TempReading::default(); 1];
    temps[0].id = owid(1);
    let mut volts = [VoltReading::default(); 2];
    volts[1].id = owid(4);
    let input = DiscoveryInputs {
        cfg: Some(&cfg),
        valves: Some(&valves),
        temps: &temps,
        volts: &volts,
        sensors_settled: true,
        stm_proto: 3,
        stm_hw: b"C2",
        ..DiscoveryInputs::default()
    };
    let mut c = Box::<DiscoveryContext>::default();
    assert!(build_discovery_context(&input, &mut c));
    assert!(!c.temps[3].active);
    assert_text(&c.volts[1].topic_segment, "2");
    assert!(c.volts[1].topic_known);
    let mut crc = 0;
    for v in &c.valves {
        let b = [
            u8::from(v.has_temp1),
            u8::from(v.has_temp2),
            u8::from(v.temps_known),
        ];
        crc = crc32(&b, crc);
    }
    for s in c.temps.iter().chain(c.volts.iter()) {
        let b = [u8::from(s.published), u8::from(s.topic_known)];
        crc = crc32(&b, crc);
        // the whole C++ array: the segment, NUL-padded to 11 bytes
        let mut seg = [0u8; 11];
        seg[..s.topic_segment.len()].copy_from_slice(&s.topic_segment);
        crc = crc32(&seg, crc);
    }
    let mut hw = [0u8; 4];
    hw[..c.hw_version.len()].copy_from_slice(&c.hw_version);
    crc = crc32(&hw, crc);
    assert_eq!(discovery_input_key(&input), crc32(&[1], crc));
}

#[test]
fn discovery_run_a_prune_without_publish_writes_a_missing_list() {
    let c = small_ctx();
    let mut port = FakePort::new();
    port.exists = false;
    let plan = DiscoveryPlan::default(); // prune only
    let mut run = DiscoveryRun::new();
    run.start(&plan);
    assert_eq!(run_all(&mut run, &c, &mut port), DiscoveryRunPhase::Done);
    assert_eq!(port.commits, 1);
    assert!(run.stats().list_written);
    assert!(port.list.is_empty());
}

#[test]
fn discovery_run_the_same_topics_in_another_order_are_written_again() {
    let c = small_ctx();
    let cur = topics(&c);
    let reversed: Vec<Vec<u8>> = cur.iter().rev().cloned().collect();
    let mut port = FakePort::with_list(&lines(&reversed));
    let plan = publish_plan();
    let mut run = DiscoveryRun::new();
    run.start(&plan);
    assert_eq!(run_all(&mut run, &c, &mut port), DiscoveryRunPhase::Done);
    assert_eq!(port.commits, 1);
    assert_eq!(port.list, lines(&cur));
    assert_eq!(port.closes, port.opens);
}

#[test]
fn discovery_run_an_unbuildable_entity_adds_no_list_line() {
    let mut c = small_ctx();
    valve(&mut c, 0, "a//b", false, false, b"");
    let mut port = FakePort::with_list(&lines(&topics(&c)));
    let plan = publish_plan();
    let mut run = DiscoveryRun::new();
    run.start(&plan);
    assert_eq!(run_all(&mut run, &c, &mut port), DiscoveryRunPhase::Done);
    assert_eq!(run.stats().skipped, 9);
    assert_eq!(port.begins, 0);
    assert_eq!(port.commits, 0);
}

#[test]
fn discovery_run_eight_list_lines_per_step() {
    let c = small_ctx();
    let mut port = FakePort::new();
    for _ in 0..20 {
        port.list.extend_from_slice(b"x\n");
    }
    let plan = delete_plan();
    let mut run = DiscoveryRun::new();
    run.start(&plan);
    assert_eq!(run.phase(), DiscoveryRunPhase::RemoveList);
    let mut buf = vec![0u8; 4096];
    let mut jw = JsonWriter::new(&mut buf);
    run.step(&c, &mut port, &mut jw);
    assert_eq!(port.read_pos, 16);
    assert_eq!(run.phase(), DiscoveryRunPhase::RemoveList);
    assert_eq!(run_all(&mut run, &c, &mut port), DiscoveryRunPhase::Done);
    assert_eq!(port.closes, port.opens);
}

#[test]
fn discovery_run_eight_current_topics_per_list_write_step() {
    let c = new_ctx();
    let mut port = FakePort::new();
    port.exists = false;
    let plan = publish_plan();
    let mut run = DiscoveryRun::new();
    run.start(&plan);
    let mut buf = vec![0u8; 4096];
    let mut jw = JsonWriter::new(&mut buf);
    for _ in 0..5000 {
        if run.phase() == DiscoveryRunPhase::WriteCurrent {
            break;
        }
        run.step(&c, &mut port, &mut jw);
    }
    assert_eq!(run.phase(), DiscoveryRunPhase::WriteCurrent);
    assert_eq!(port.writes, 0);
    run.step(&c, &mut port, &mut jw);
    assert_eq!(port.writes, 8);
    assert_eq!(run_all(&mut run, &c, &mut port), DiscoveryRunPhase::Done);
    assert_eq!(port.list, lines(&topics(&c)));
    assert_eq!(port.commits, 1);
}

#[test]
fn discovery_run_delete_and_publish_with_prune_closes_and_commits_each_list_once() {
    let c = small_ctx();
    let mut port = FakePort::with_list(&lines(&topics(&c)));
    let plan = DiscoveryPlan {
        remove_all: true,
        publish: true,
        ..DiscoveryPlan::default()
    };
    let mut run = DiscoveryRun::new();
    run.start(&plan);
    assert_eq!(run_all(&mut run, &c, &mut port), DiscoveryRunPhase::Done);
    assert_eq!(port.aborts, 0);
    assert_eq!(port.closes, port.opens);
    assert_eq!(port.commits, 2); // the cleared list, then the new one
    assert_eq!(port.list, lines(&topics(&c)));
}
