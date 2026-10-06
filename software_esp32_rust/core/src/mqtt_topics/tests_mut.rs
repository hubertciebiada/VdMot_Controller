//! Port of test/native/test_mqtt_topics__mut.cpp: HA prefixes at the topic length limit, reused
//! subscription arrays, segments with '/' of the full length and command topics that only look
//! like valve calibrations.

use super::*;
use crate::test_support::assert_text;
use std::vec;
use std::vec::Vec;

fn parse(c: &TopicContext, prefix: &[u8], t: &[u8]) -> InboundTopic {
    parse_inbound_topic(c, prefix, t, None)
}

fn filled(c: u8) -> Subscription {
    let mut s = Subscription::default();
    s.filter.resize(TOPIC_MAX, c).unwrap();
    s
}

fn status_of(prefix: &[u8]) -> Vec<u8> {
    let mut t = prefix.to_vec();
    t.extend_from_slice(b"/status");
    t
}

#[test]
fn build_subscriptions_a_ha_prefix_whose_status_topic_is_exactly_topic_max_long() {
    let c = TopicContext::default();
    // + "/status" = TOPIC_MAX
    let prefix = vec![b'h'; TOPIC_MAX - 7];
    // reused entries: every filter is replaced as a whole
    let mut out: [Subscription; 8] = core::array::from_fn(|_| filled(b'x'));
    let n = build_subscriptions(&c, MqttMode::MqttHa, &prefix, None, &mut out);
    assert_eq!(n, 5);
    assert_text(&out[0].filter, "VdMotFBH/valves/+/target/set");
    assert_text(&out[1].filter, "VdMotFBH/valves/+/target/set/set");
    assert_text(&out[2].filter, "VdMotFBH/cmd/#");
    assert_text(&out[3].filter, "homeassistant/status");
    assert_text(&out[4].filter, status_of(&prefix));
    assert_eq!(out[4].qos, 1);
    // the entries past the last one are left alone
    assert_eq!(out[5], filled(b'x'));
}

#[test]
fn build_subscriptions_a_ha_prefix_one_char_too_long_adds_no_entry() {
    let c = TopicContext::default();
    let prefix = vec![b'h'; TOPIC_MAX - 6];
    let mut out: [Subscription; 8] = Default::default();
    assert_eq!(
        build_subscriptions(&c, MqttMode::MqttHa, &prefix, None, &mut out),
        4
    );
    assert_text(&out[3].filter, "homeassistant/status");
    assert!(out[4].filter.is_empty());
}

#[test]
fn build_subscriptions_a_configured_segment_with_slash_of_the_full_segment_length() {
    let c = TopicContext {
        separate: false,
        ..TopicContext::default()
    };
    let mut seg: Segments = Default::default();
    seg[2].extend_from_slice(b"abcd/fghij").unwrap();
    assert_eq!(seg[2].len(), SEGMENT_MAX);
    let mut out: [Subscription; 8] = Default::default();
    // C++ a null HA prefix: the empty prefix gives the same
    assert_eq!(
        build_subscriptions(&c, MqttMode::Mqtt, b"", Some(&seg), &mut out),
        5
    );
    assert_text(&out[3].filter, "VdMotFBH/valves/abcd/fghij/target");
    assert_text(&out[4].filter, "VdMotFBH/valves/abcd/fghij/target/set");
}

#[test]
fn parse_inbound_topic_the_ha_status_topic_of_a_prefix_at_the_length_limit() {
    let c = TopicContext::default();
    let prefix = vec![b'h'; TOPIC_MAX - 7];
    let t = status_of(&prefix);
    assert_eq!(parse(&c, &prefix, &t).kind, InboundKind::HaStatus);
    assert_eq!(
        parse(&c, &prefix, &t[..t.len() - 1]).kind,
        InboundKind::None
    );
}

#[test]
fn parse_inbound_topic_a_topic_longer_than_topic_max_is_none_of_ours() {
    let c = TopicContext::default();
    let mut t = b"VdMotFBH/valves/".to_vec();
    t.resize(t.len() + TOPIC_MAX, b'1');
    t.extend_from_slice(b"/target/set");
    assert_eq!(parse(&c, b"ha", &t).kind, InboundKind::None);
    assert_eq!(
        parse(&c, b"ha", b"VdMotFBH/valves/1/target/set").kind,
        InboundKind::Target
    );
}

#[test]
fn parse_inbound_topic_commands_that_only_look_like_a_valve_calibration() {
    let c = TopicContext::default();
    assert_eq!(
        parse(&c, b"ha", b"VdMotFBH/cmd/abcdefg1/calibrate").kind,
        InboundKind::UnknownCommand
    );
    assert_eq!(
        parse(&c, b"ha", b"VdMotFBH/cmd/valvesX1/calibrate").kind,
        InboundKind::UnknownCommand
    );
    assert_eq!(
        parse(&c, b"ha", b"VdMotFBH/cmd/valves/1234567890x").kind,
        InboundKind::UnknownCommand
    );
    let r = parse(&c, b"ha", b"VdMotFBH/cmd/valves/3/calibrate");
    assert_eq!(r.kind, InboundKind::CalibrateValve);
    assert_eq!(r.valve, Some(2));
}
