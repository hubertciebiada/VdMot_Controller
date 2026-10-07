//! Rust additions (no C++ counterpart): the contract constants, the enum tables, the payload
//! limit at 2047/2048 bytes, the end of a pure Delete, a refused station that exhausts the DROP
//! list, the context fields at their full length, and the phases of a publish plan without prune.

use super::tests::{all, cls, has, json, owid, run_all, small_ctx, tail, topics, FakePort};
use super::*;
use crate::test_support::assert_text;
use crate::valve_model::ValveState;
use std::boxed::Box;
use std::vec;
use std::vec::Vec;

#[test]
fn contract_constants() {
    assert_eq!(DISCOVERY_TOPIC_MAX, 127);
    assert_eq!(DISCOVERY_PAYLOAD_MAX, 2047);
    assert_eq!(IP_TEXT_MAX, 15);
    assert_eq!(SW_VERSION_MAX, 31);
    assert_eq!(HW_VERSION_MAX, 3);
    assert_eq!(ENTITY_COUNT, 309);
    assert_eq!(TEMPS_FIRST, 244);
    assert_eq!(VOLTS_FIRST, 278);
    assert_eq!(TAIL_FIRST, 286);
    assert_eq!(DiscoveryRun::LINES_PER_STEP, 8);
    let plan = DiscoveryPlan::default();
    assert!(plan.prune);
    assert!(!plan.publish && !plan.remove_all && !plan.drop_legacy && !plan.retire20);
}

#[test]
fn enum_tables() {
    for v in 0..=9u8 {
        assert_eq!(HaComponent::from_raw(v).map(|c| c as u8), Some(v));
    }
    assert_eq!(HaComponent::from_raw(10), None);
    for v in 0..=3u8 {
        assert_eq!(TopicClass::from_raw(v).map(|c| c as u8), Some(v));
    }
    assert_eq!(TopicClass::from_raw(4), None);
    for v in 0..=12u8 {
        assert_eq!(DiscoveryRunPhase::from_raw(v).map(|p| p as u8), Some(v));
    }
    assert_eq!(DiscoveryRunPhase::from_raw(13), None);
    assert_eq!(DiscoveryRunPhase::default(), DiscoveryRunPhase::Idle);
    assert_eq!(HaComponent::default(), HaComponent::Sensor);
}

/// The context of the longest payload: every gate open, the longest station, prefix, ip and
/// hw tag (the C++ "every entity enabled" case without valves and sensors).
fn longest_ctx() -> Box<DiscoveryContext> {
    let mut c = small_ctx();
    copy_string(&mut c.topics.station, b"abcdefghijklmnopqrst");
    c.topics.path_as_root = true;
    c.events = true;
    copy_string(&mut c.hw_version, b"C99");
    copy_string(&mut c.discovery_prefix, b"abcdefghijklmnopqrstuvwxyz012345");
    copy_string(&mut c.ip, b"255.255.255.255");
    c
}

const EVENT_TOPIC: &str =
    "abcdefghijklmnopqrstuvwxyz012345/event/abcdefghijklmnopqrst/events/config";

/// The event entity with the sw_version, hw tag and ip of `fields`: its payload, or None when
/// next() refused it (then the writer is poisoned and the topic is set).
fn event_payload(c: &mut DiscoveryContext, fields: &Fields) -> Option<Vec<u8>> {
    c.sw_version.clear();
    c.sw_version.extend_from_slice(&fields.sw).expect("sw fits");
    c.hw_version.clear();
    c.hw_version.extend_from_slice(&fields.hw).expect("hw fits");
    c.ip.clear();
    c.ip.extend_from_slice(&fields.ip).expect("ip fits");
    let mut it = DiscoveryIterator::new();
    let mut m = DiscoveryMessage::default();
    let mut buf = vec![0u8; 4096];
    let mut jw = JsonWriter::new(&mut buf);
    for _ in 0..309 {
        let got = it.next(c, &mut m, &mut jw);
        if m.topic == EVENT_TOPIC.as_bytes() {
            if got {
                assert!(jw.complete());
                return Some(jw.as_bytes().to_vec());
            }
            assert!(!jw.ok());
            return None;
        }
        assert!(got || !jw.ok(), "the event entity comes before the end");
    }
    panic!("no event entity");
}

/// sw_version, hw tag and ip of their full lengths (31, 3 and 15 bytes).
struct Fields {
    sw: Vec<u8>,
    hw: Vec<u8>,
    ip: Vec<u8>,
}

/// The fields whose JSON form is `extra` bytes longer than the all-plain ones: a control byte
/// is written as \u00XX (+5), a quote as \" (+1).
fn fields_with_extra(extra: usize) -> Fields {
    let (controls, quotes) = (extra / 5, extra % 5);
    let mut all = vec![0x01u8; controls];
    all.extend(core::iter::repeat_n(b'"', quotes));
    assert!(all.len() <= 31 + 3 + 15, "{extra} extra bytes do not fit");
    all.resize(31 + 3 + 15, b'v');
    Fields {
        sw: all[..31].to_vec(),
        hw: all[31..34].to_vec(),
        ip: all[34..].to_vec(),
    }
}

#[test]
fn a_payload_of_2047_bytes_goes_out_and_one_of_2048_does_not() {
    let mut c = longest_ctx();
    let plain = event_payload(&mut c, &fields_with_extra(0)).expect("fits");
    assert!(plain.len() <= DISCOVERY_PAYLOAD_MAX);
    let extra = DISCOVERY_PAYLOAD_MAX - plain.len();
    let at_limit = event_payload(&mut c, &fields_with_extra(extra)).expect("2047 bytes go out");
    assert_eq!(at_limit.len(), DISCOVERY_PAYLOAD_MAX);
    assert!(event_payload(&mut c, &fields_with_extra(extra + 1)).is_none());
    // The run counts it as skipped and lists its topic.
    let mut port = FakePort::new();
    port.exists = false;
    let plan = DiscoveryPlan {
        publish: true,
        ..DiscoveryPlan::default()
    };
    let mut run = DiscoveryRun::new();
    run.start(&plan);
    assert_eq!(run_all(&mut run, &c, &mut port), DiscoveryRunPhase::Done);
    assert_eq!(run.stats().skipped, 1);
    assert!(has(&port.list, EVENT_TOPIC));
    assert!(!port.configs().iter().any(|t| t == EVENT_TOPIC.as_bytes()));
}

#[test]
fn a_pure_delete_ends_after_clearing_the_list_whatever_else_the_plan_says() {
    let c = small_ctx();
    let mut port = FakePort::with_list(b"homeassistant/text/VdMot/old/config\n");
    let plan = DiscoveryPlan {
        remove_all: true,
        prune: false,
        drop_legacy: true,
        retire20: true,
        publish: false,
    };
    let mut run = DiscoveryRun::new();
    run.start(&plan);
    assert_eq!(run.phase(), DiscoveryRunPhase::RemoveList);
    assert_eq!(run_all(&mut run, &c, &mut port), DiscoveryRunPhase::Done);
    // the old line and every current topic, no DROP entity, no uptime topic
    assert_eq!(port.deletes().len(), 1 + topics(&c).len());
    assert!(port.list.is_empty());
    assert_eq!(port.commits, 1);
}

#[test]
fn a_refused_station_exhausts_the_drop_list_until_restart() {
    let mut c = small_ctx();
    copy_string(&mut c.topics.station, b"a+b");
    let mut it = DropListIterator::new();
    let mut m = DiscoveryMessage::default();
    assert!(!it.next(&c, &mut m));
    copy_string(&mut c.topics.station, b"VdMot");
    assert!(!it.next(&c, &mut m));
    assert!(m.topic.is_empty());
    it.restart();
    assert!(it.next(&c, &mut m));
    assert_text(&m.topic, "homeassistant/select/VdMot/heatControl/config");
    // The DiscoveryIterator stops at the end as well.
    let mut d = DiscoveryIterator::new();
    copy_string(&mut c.topics.station, b"a+b");
    assert!(!d.next_topic(&c, &mut m));
    assert_eq!(d.position(), 309);
    copy_string(&mut c.topics.station, b"VdMot");
    assert!(!d.next_topic(&c, &mut m));
}

#[test]
fn the_context_keeps_fields_of_their_full_length_and_protocol_4() {
    let mut cfg = Box::<Config>::default();
    copy_string(&mut cfg.station, b"abcdefghijklmnopqrst");
    cfg.valves[0].active = true;
    copy_string(&mut cfg.valves[0].name, b"ABCDEFGHIJ");
    cfg.temps[0].active = true;
    cfg.temps[0].id = owid(1);
    copy_string(&mut cfg.temps[0].name, b"Temp_01234");
    cfg.volts[0].active = true;
    cfg.volts[0].id = owid(2);
    copy_string(&mut cfg.volts[0].topic, b"Volt/01234");
    let valves = [ValveState::default(); 12];
    let input = DiscoveryInputs {
        cfg: Some(&cfg),
        valves: Some(&valves),
        stm_proto: 4,
        stm_hw: b"C2xy",
        ip: 0xFFFF_FFFF,
        sw_version: b"0123456789012345678901234567890123",
        ..DiscoveryInputs::default()
    };
    let mut c = Box::<DiscoveryContext>::default();
    assert!(build_discovery_context(&input, &mut c));
    assert!(c.stm_v3);
    assert_text(&c.ip, "255.255.255.255");
    assert_text(&c.sw_version, "0123456789012345678901234567890");
    assert_text(&c.hw_version, "C2x");
    assert_text(&c.station, "abcdefghijklmnopqrst");
    assert_text(&c.topics.station, "abcdefghijklmnopqrst");
    assert_text(&c.valves[0].segment, "ABCDEFGHIJ");
    assert_text(&c.valves[0].name, "ABCDEFGHIJ");
    assert_text(&c.temps[0].segment, "Temp_01234");
    assert_text(&c.temps[0].topic_segment, "Temp_01234");
    assert!(c.temps[0].topic_known);
    assert_text(&c.volts[0].segment, "Volt/01234");
    assert_text(&c.volts[0].topic_segment, "Volt/01234");
    let v = all(&c);
    assert!(has(
        &json(
            &v,
            "homeassistant/sensor/abcdefghijklmnopqrst/valves_actual_ABCDEFGHIJ/config"
        ),
        "\"name\":\"ABCDEFGHIJ position\""
    ));
    assert!(has(
        &json(&v, &tail(0).replace("VdMot", "abcdefghijklmnopqrst")),
        "\"configuration_url\":\"http://255.255.255.255/\""
    ));
    assert_eq!(
        cls(
            &c,
            "homeassistant/sensor/abcdefghijklmnopqrst/temps_Temp_01234/config"
        ),
        TopicClass::Current
    );
}

#[test]
fn a_publish_plan_without_prune_goes_from_the_drop_list_to_publish() {
    // The C++ switch falls through DropList -> Prune -> Publish: a plan without prune never
    // enters Prune (the firmware prunes in every publish run, the core takes any plan).
    let c = small_ctx();
    let mut run = DiscoveryRun::new();
    run.start(&DiscoveryPlan {
        publish: true,
        prune: false,
        ..DiscoveryPlan::default()
    });
    assert_eq!(run.phase(), DiscoveryRunPhase::Publish);
    run.start(&DiscoveryPlan {
        publish: true,
        prune: false,
        drop_legacy: true,
        ..DiscoveryPlan::default()
    });
    assert_eq!(run.phase(), DiscoveryRunPhase::DropList);
    let mut port = FakePort::new();
    let mut buf = vec![0u8; 4096];
    let mut jw = JsonWriter::new(&mut buf);
    for _ in 0..1000 {
        if run.phase() != DiscoveryRunPhase::DropList {
            break;
        }
        run.step(&c, &mut port, &mut jw);
    }
    assert_eq!(run.phase(), DiscoveryRunPhase::Publish);
    assert!(!port.deletes().is_empty()); // the DROP entities
    assert_eq!(port.opens, 0); // no list was read
}
