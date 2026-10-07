//! C++ `test_mqtt_client.cpp`: session, LWT, subscriptions, inbound commands and their retained
//! leftovers, the regulator state, published values, events and the discovery run with its list
//! file. PubSubClient calls became packets at the fake broker.
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use super::rig::{s, Rig, HOST, PORT};
use super::*;
use crate::mqtt_conn::{MQTT_CONNECTION_TIMEOUT, MQTT_CONNECT_UNAUTHORIZED};
use vdm_esp_core::common::{OneWireId, TEMP_READ_ERROR};
use vdm_esp_core::failsafe::{regulator_cause, RegulatorCause};
use vdm_esp_core::mqtt_topics::TargetPayload;
use vdm_esp_core::stm_codec::STM_FLAG_FS_BLOCKED;
use vdm_esp_core::valve_model::{
    TargetSync, HEALTH_BLOCKED, HEALTH_FAILSAFE, HEALTH_STALE, HEALTH_TARGET_UNCONFIRMED,
    HEALTH_TEMP_FAILED,
};

/// The topic `t` without a segment under the config's root (C++ `topicOf`).
pub(super) fn topic_of(c: &Config, t: Topic) -> String {
    let mut tc = TopicContext::default();
    copy_string(&mut tc.station, mqtt_root_topic(c));
    tc.path_as_root = c.mqtt.path_as_root;
    tc.separate = c.mqtt.separate;
    let mut buf = [0u8; TOPIC_MAX + 1];
    let n = build_topic(&tc, t, b"", &mut buf);
    s(&buf[..n])
}

fn subs(v: &[(&str, u8)]) -> Vec<(String, u8)> {
    v.iter().map(|(f, q)| (f.to_string(), *q)).collect()
}

#[test]
fn begin_the_packet_buffer_is_2304_bytes() {
    let rig = Rig::new();
    let mut c = rig.client();
    c.begin();
    // C++ also checked one setBufferSize() call: the Rust buffer is allocated once by new()
    assert_eq!(c.out.conn.buffer_size(), 2304);
    assert_eq!(rig.shared.status().ha_status, HaStatus::Unknown); // power-on RTC contents
    assert_eq!(rig.shared.regulator_state().ha, HaStatus::Unknown);
}

#[test]
fn mqtt_off_stays_disabled_never_connects_the_regulator_reads_off() {
    let rig = Rig::new();
    let mut c = rig.client();
    c.begin();
    rig.run(&mut c, 2);
    assert_eq!(rig.shared.status().state, MqttState::Disabled);
    assert_eq!(rig.connects(), 0);
    assert_eq!(*rig.delays.borrow(), vec![500, 500]);
    let r = rig.shared.regulator_state();
    assert_eq!(r.mode, MqttMode::Off);
    assert!(!r.broker_connected);
    assert_eq!(regulator_cause(&r), RegulatorCause::Alive);
}

#[test]
fn connects_with_the_mac_client_id_lwt_online_first_wildcard_subscriptions() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    c.begin();
    rig.run(&mut c, 1);
    assert_eq!(rig.connects(), 1);
    let tcp = rig.dev.tcp.connects();
    assert_eq!((tcp[0].host.as_str(), tcp[0].port), (HOST, PORT));
    let b = rig.broker.state();
    let cn = &b.connects[0];
    assert_eq!(cn.keep_alive, 30);
    assert_eq!(s(&cn.client_id), "VdMot-123456"); // fake efuse MAC 24:0a:c4:12:34:56
    assert!(!cn.clean_session());
    assert_eq!(cn.user, None);
    assert_eq!(cn.password, None);
    let lwt = topic_of(&rig.host.state().cfg, Topic::Status);
    assert_eq!(cn.will_topic.as_deref(), Some(lwt.as_bytes()));
    assert_eq!(cn.will_message.as_deref(), Some(&b"offline"[..]));
    assert!(cn.will_retain());
    assert_eq!(cn.will_qos(), 0);
    let first = &b.published[0];
    assert_eq!(first.topic, lwt.as_bytes());
    assert_eq!(first.payload, b"online");
    assert!(first.retain);
    drop(b);
    assert_eq!(
        rig.subscribed(),
        subs(&[
            ("VdMot/valves/+/target/set", 1),
            ("VdMot/valves/+/target/set/set", 1),
            ("VdMot/cmd/#", 0)
        ])
    );
    let st = rig.shared.status();
    assert_eq!(st.state, MqttState::Connected);
    assert_eq!(st.reconnects, 1);
    assert_eq!(s(&st.client_id), "VdMot-123456");
    assert!(!rig.events(EventCode::MqttConnected).is_empty());
    let r = rig.shared.regulator_state();
    assert_eq!(r.mode, MqttMode::Mqtt);
    assert!(r.broker_connected);
}

#[test]
fn the_connack_wait_is_the_socket_timeout_of_5_s() {
    // C++ checked setSocketTimeout(5): a broker that does not answer CONNACK fails the connect
    // after 5 s
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.broker.state().connack.push_back(None);
    c.begin();
    let t0 = rig.now();
    rig.run(&mut c, 1);
    assert_eq!(rig.now() - t0, 5000 + 100);
    let st = rig.shared.status();
    assert_eq!(st.state, MqttState::Error);
    assert_eq!(i32::from(st.rc), MQTT_CONNECTION_TIMEOUT);
}

#[test]
fn a_configured_client_id_of_the_longest_allowed_length_is_reported_whole() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    let id = format!("vdmot-{}9", "x".repeat(CLIENT_ID_MAX - 7));
    assert_eq!(id.len(), CLIENT_ID_MAX);
    rig.cfg(|c| {
        copy_string(&mut c.mqtt.client_id, id.as_bytes());
    });
    c.begin();
    rig.run(&mut c, 1);
    assert_eq!(s(&rig.broker.state().connects[0].client_id), id);
    assert_eq!(rig.shared.status().state, MqttState::Connected);
    assert_eq!(s(&rig.shared.status().client_id), id);
}

#[test]
fn a_configured_client_id_is_used_verbatim_ha_mode_subscriptions() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::MqttHa);
    rig.cfg(|c| {
        copy_string(&mut c.mqtt.client_id, b"my.client-id_1");
        copy_string(&mut c.mqtt.discovery_prefix, b"ha");
        copy_string(&mut c.mqtt.root_topic, b"VdMotFBH");
        copy_string(&mut c.valves[1].topic, b"Bad/WC");
    });
    c.begin();
    rig.run(&mut c, 1);
    let b = rig.broker.state();
    assert_eq!(s(&b.connects[0].client_id), "my.client-id_1");
    assert_eq!(
        b.connects[0].will_topic.as_deref(),
        Some(&b"VdMotFBH/status"[..])
    );
    drop(b);
    assert_eq!(
        rig.subscribed(),
        subs(&[
            ("VdMotFBH/valves/+/target/set", 1),
            ("VdMotFBH/valves/+/target/set/set", 1),
            ("VdMotFBH/cmd/#", 0),
            ("VdMotFBH/valves/Bad/WC/target/set", 1),
            ("VdMotFBH/valves/Bad/WC/target/set/set", 1),
            ("homeassistant/status", 1),
            ("ha/status", 1)
        ])
    );
}

#[test]
fn credentials_go_out_only_as_a_pair() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        copy_string(&mut c.mqtt.user, b"u");
    });
    c.begin();
    rig.run(&mut c, 1);
    let b = rig.broker.state();
    assert_eq!(b.connects[0].user, None);
    assert_eq!(b.connects[0].password, None);
}

#[test]
fn a_refused_connect_is_an_error_with_the_library_state() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.refuse_with(MQTT_CONNECT_UNAUTHORIZED as u8, 1);
    c.begin();
    rig.run(&mut c, 1);
    assert_eq!(rig.shared.status().state, MqttState::Error);
    assert_eq!(i32::from(rig.shared.status().rc), MQTT_CONNECT_UNAUTHORIZED);
    assert_eq!(*rig.delays.borrow(), vec![100]);
}

#[test]
fn a_broker_that_drops_at_once_is_retried_at_2_4_8_s() {
    // W20-3
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    c.begin();
    let mut connected_at = Vec::new();
    let mut seen = 0;
    rig.run_hook(&mut c, 150, &mut |_, now| {
        if rig.connects() != seen {
            seen = rig.connects();
            connected_at.push(now);
        }
        if rig.session_up() {
            rig.broker.drop_connection();
        }
    }); // 15 s of 100 ms passes
    assert_eq!(connected_at.len(), 4);
    assert_eq!(connected_at[1] - connected_at[0], 2100);
    assert_eq!(connected_at[2] - connected_at[1], 4100);
    assert_eq!(connected_at[3] - connected_at[2], 8100);
}

#[test]
fn clean_session_only_after_a_topic_config_change() {
    // W3-4
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    assert!(!rig.clean_session());
    rig.cfg(|c| {
        c.failsafe.timeout_min = 30; // applies live: no reconnect
        c.valves[0].failsafe_pct = 20;
    });
    rig.new_revision();
    rig.run(&mut c, 2);
    assert_eq!(rig.connects(), 1);
    rig.cfg(|c| c.mqtt.separate = false);
    rig.new_revision();
    rig.run(&mut c, 2);
    assert_eq!(rig.connects(), 2);
    assert!(rig.clean_session());
    let all = rig.subscribed();
    assert_eq!(
        all[all.len() - 3..].to_vec(),
        subs(&[
            ("VdMot/valves/+/target", 1),
            ("VdMot/valves/+/target/set", 1),
            ("VdMot/cmd/#", 0)
        ])
    );
    rig.shared.request_reconnect();
    rig.run(&mut c, 2);
    assert_eq!(rig.connects(), 3);
    assert!(!rig.clean_session());
}

#[test]
fn a_station_renamed_at_run_time_reconnects_with_its_client_id_will_and_topics() {
    // reloadConfig: the station is the host part of the client id, the main topic and the
    // topic of the last will; a rename is a topic change (no C++ case either)
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    let first = rig.broker.state().connects[0].clone();
    assert!(
        first.client_id.starts_with(b"VdMot-"),
        "{}",
        s(&first.client_id)
    );
    assert_eq!(
        first.will_topic.as_deref(),
        Some(b"VdMot/status".as_slice())
    );
    rig.cfg(|c| {
        copy_string(&mut c.station, b"Haus");
    });
    rig.new_revision();
    rig.run(&mut c, 2);
    assert_eq!(rig.connects(), 2);
    assert!(rig.clean_session());
    let second = rig.broker.state().connects[1].clone();
    let mac = &first.client_id[b"VdMot".len()..];
    assert_eq!(second.client_id, [b"Haus".as_slice(), mac].concat());
    assert_eq!(
        second.will_topic.as_deref(),
        Some(b"Haus/status".as_slice())
    );
    assert_eq!(rig.last("Haus/status"), "online");
    assert!(rig.subscribed().iter().any(|(t, _)| t == "Haus/cmd/#"));
}

#[test]
fn a_config_reload_takes_no_heap_block_a_topic_change_reconnects_in_the_same_pass() {
    // C++ "without memory for the reload copy the old config stays until the next pass": the
    // Rust reload compares and copies under the config lock without a second copy (design 2.4)
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    assert_eq!(rig.connects(), 1);
    assert!(rig.dev.heap.state().granted.is_empty());
    rig.cfg(|c| c.mqtt.separate = false); // a topic change: a new session
    rig.new_revision();
    rig.dev.heap.state().fail_all = true;
    rig.run(&mut c, 1);
    assert_eq!(rig.connects(), 2);
    assert!(rig.clean_session());
    let h = rig.dev.heap.state();
    assert!(h.granted.is_empty() && h.refused.is_empty());
}

// ---------------------------------------------------------------- inbound

#[test]
fn a_target_is_submitted_and_its_retained_topic_cleared() {
    // W3-5
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 40);
    let before = rig.published_len();
    rig.deliver("VdMot/valves/1/target/set", "40");
    rig.run(&mut c, 1);
    let sub = rig.submitted();
    assert_eq!(sub.len(), 1);
    assert_eq!(sub[0].kind, StmCommandType::SetTarget);
    assert_eq!(sub[0].valve, 0);
    assert_eq!(sub[0].pos, 40);
    assert_eq!(sub[0].source, TargetSource::Mqtt);
    let p = rig.published();
    assert!(p.len() > before);
    assert_eq!(p[before].topic, b"VdMot/valves/1/target/set");
    assert_eq!(p[before].payload, b"");
    assert!(p[before].retain);
    assert_eq!(rig.shared.regulator_state().command_seq, 1);
    // the broker echoes the empty message: nothing happens
    rig.deliver("VdMot/valves/1/target/set", "");
    rig.run(&mut c, 2);
    assert_eq!(rig.submitted().len(), 1);
    assert!(rig.rejected().is_empty());
    assert_eq!(rig.shared.status().commands_rejected, 0);
    assert_eq!(rig.shared.regulator_state().command_seq, 1);
    assert_eq!(rig.payloads("VdMot/valves/1/target/set").len(), 1);
}

#[test]
fn fractions_round_half_up_the_number_form_of_a_named_valve() {
    // W6-3
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        copy_string(&mut c.valves[0].name, b"Bad");
    });
    rig.settle(&mut c, 2);
    rig.deliver("VdMot/valves/1/target/set", "43,7");
    rig.deliver("VdMot/valves/Bad/target/set/set", "43.49");
    rig.run(&mut c, 2);
    let sub = rig.submitted();
    assert_eq!(sub.len(), 2);
    assert_eq!(sub[0].pos, 44);
    assert_eq!(sub[1].pos, 43);
}

#[test]
fn without_separate_the_esps_own_target_is_not_a_command() {
    // W3-7
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| c.mqtt.separate = false);
    rig.link_up();
    rig.snap(|s| {
        s.valves[0].desired_valid = true;
        s.valves[0].desired = 40;
    });
    rig.settle(&mut c, 40);
    assert_eq!(rig.last("VdMot/valves/1/target"), "40");
    let pubs = rig.count("VdMot/valves/1/target");
    rig.deliver("VdMot/valves/1/target", "40");
    rig.run(&mut c, 2);
    assert!(rig.submitted().is_empty());
    rig.deliver("VdMot/valves/1/target", "41");
    rig.run(&mut c, 2);
    let sub = rig.submitted();
    assert_eq!(sub.len(), 1);
    assert_eq!(sub[0].pos, 41);
    assert_eq!(rig.count("VdMot/valves/1/target"), pubs); // no clear on the state form
    rig.deliver("VdMot/valves/1/target/set", "42");
    rig.run(&mut c, 2);
    assert_eq!(rig.submitted().len(), 2);
    assert_eq!(rig.last("VdMot/valves/1/target/set"), ""); // the /set form is cleared
}

#[test]
fn rejections_are_counted_and_logged_once_per_10_s() {
    // E28-3
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.deliver("VdMot/valves/9/target/set", "50");
    rig.run(&mut c, 1);
    let ev = rig.rejected();
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 9);
    assert_eq!(ev[0].arg2, 0);
    assert_eq!(Rig::text(&ev[0]), "inactive");
    assert_eq!(ev[0].valve, NO_VALVE);
    assert_eq!(rig.shared.status().commands_rejected, 1);
    assert_eq!(rig.last("VdMot/valves/9/target/set"), ""); // cleared as well
    for _ in 0..99 {
        rig.deliver("VdMot/valves/9/target/set", "50");
    }
    rig.run(&mut c, 60);
    assert_eq!(rig.rejected().len(), 1);
    assert_eq!(rig.shared.status().commands_rejected, 100);
    assert!(rig.submitted().is_empty());
    // other reasons and valves are logged at once
    rig.deliver("VdMot/valves/1/target/set", "abc");
    rig.deliver("VdMot/valves/Old/target/set", "5");
    rig.deliver("VdMot/cmd/foo", "PRESS");
    rig.run(&mut c, 3);
    let ev = rig.rejected();
    assert_eq!(ev.len(), 4);
    assert_eq!(ev[1].arg1, 1);
    assert_eq!(ev[1].arg2, TargetPayload::NotNumber as i32);
    assert_eq!(Rig::text(&ev[1]), "payload");
    assert_eq!(ev[2].arg1, 0);
    assert_eq!(Rig::text(&ev[2]), "unknown valve");
    assert_eq!(Rig::text(&ev[3]), "unknown command");
    assert_eq!(rig.shared.status().commands_rejected, 103);
}

#[test]
fn messages_beyond_the_4_inbound_slots_of_one_loop_are_rejected_as_queue_full() {
    // PubSubClient 2.8 delivers one message per loop(); the C++ fake's burst (the whole inbox in
    // one loop) is the queue fed directly, as the poll callback does
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        for v in 0..6 {
            c.valves[v].active = true;
        }
    });
    rig.settle(&mut c, 2);
    for v in 1..=6 {
        c.inbound
            .push(format!("VdMot/valves/{v}/target/set").as_bytes(), b"30");
    }
    rig.run(&mut c, 1);
    let sub = rig.submitted();
    assert_eq!(sub.len(), 4);
    for (i, cmd) in sub.iter().enumerate() {
        assert_eq!(usize::from(cmd.valve), i);
    }
    let ev = rig.rejected();
    assert_eq!(ev.len(), 1); // the second loss is counted, its log is rate-limited
    assert_eq!(Rig::text(&ev[0]), "queue full");
    assert_eq!(ev[0].arg1, 0);
    assert_eq!(rig.shared.status().commands_rejected, 2);
    // the overflow is drained: the next loop starts clean
    rig.deliver("VdMot/valves/5/target/set", "30");
    rig.run(&mut c, 1);
    assert_eq!(rig.submitted().len(), 5);
    assert_eq!(rig.shared.status().commands_rejected, 2);
}

#[test]
fn a_button_acts_only_after_the_broker_echoed_its_clear() {
    // H13
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.deliver("VdMot/cmd/restart", "PRESS");
    rig.run(&mut c, 1);
    assert_eq!(rig.last("VdMot/cmd/restart"), "");
    assert!(rig.host.state().restart_requests.is_empty());
    rig.deliver("VdMot/cmd/restart", "");
    rig.run(&mut c, 1);
    assert_eq!(rig.host.state().restart_requests, vec![(0, 1000)]);
    assert_eq!(rig.shared.regulator_state().command_seq, 1);
    assert!(rig.rejected().is_empty());
}

#[test]
fn a_restart_is_rejected_while_the_stm_sector_0_is_not_written() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.host.state().sector0_at_risk = true;
    rig.deliver("VdMot/cmd/restart", "PRESS");
    rig.run(&mut c, 1);
    rig.deliver("VdMot/cmd/restart", "");
    rig.run(&mut c, 1);
    assert!(rig.host.state().restart_requests.is_empty());
    let ev = rig.rejected();
    assert_eq!(ev.len(), 1);
    assert_eq!(Rig::text(&ev[0]), "stm sector 0 pending");
    assert_eq!(rig.shared.regulator_state().command_seq, 0);
    // written: the next press restarts
    rig.host.state().sector0_at_risk = false;
    rig.deliver("VdMot/cmd/restart", "PRESS");
    rig.run(&mut c, 1);
    rig.deliver("VdMot/cmd/restart", "");
    rig.run(&mut c, 1);
    assert_eq!(rig.host.state().restart_requests, vec![(0, 1000)]);
    assert_eq!(rig.rejected().len(), 1);
}

#[test]
fn a_button_without_the_echo_is_rejected_after_5_s() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.deliver("VdMot/cmd/restart", "PRESS");
    rig.run(&mut c, 1);
    rig.run(&mut c, 245); // 4.9 s
    assert!(rig.rejected().is_empty());
    rig.run(&mut c, 10);
    let ev = rig.rejected();
    assert_eq!(ev.len(), 1);
    assert_eq!(Rig::text(&ev[0]), "clear not confirmed");
    assert!(rig.host.state().restart_requests.is_empty());
    rig.deliver("VdMot/cmd/restart", ""); // a late echo does nothing
    rig.run(&mut c, 2);
    assert!(rig.host.state().restart_requests.is_empty());
}

#[test]
fn a_button_whose_clear_publish_fails_is_rejected() {
    // W3-6
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.deliver("VdMot/cmd/restart", "PRESS");
    rig.script.get().fail_next_publish = true;
    rig.run(&mut c, 1);
    assert!(rig.host.state().restart_requests.is_empty());
    let ev = rig.rejected();
    assert_eq!(ev.len(), 1);
    assert_eq!(Rig::text(&ev[0]), "clear not confirmed");
    assert!(rig.shared.status().publish_failures >= 1);
}

#[test]
fn every_cmd_topic_submits_its_command_after_the_echo() {
    // W15-5
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| s.proto = 3);
    rig.settle(&mut c, 2);
    let rows = [
        ("VdMot/cmd/valves/1/calibrate", StmCommandType::Calibrate, 0),
        ("VdMot/cmd/calibrate", StmCommandType::Calibrate, ALL_VALVES),
        ("VdMot/cmd/stmReset", StmCommandType::ResetStm, NO_VALVE),
        ("VdMot/cmd/detect", StmCommandType::Detect, ALL_VALVES),
        ("VdMot/cmd/stop", StmCommandType::StopValve, ALL_VALVES),
        (
            "VdMot/cmd/stmSafeExit",
            StmCommandType::LeaveSafeMode,
            NO_VALVE,
        ),
    ];
    for (n, (topic, kind, valve)) in rows.into_iter().enumerate() {
        rig.deliver(topic, "PRESS");
        rig.run(&mut c, 1);
        assert_eq!(rig.submitted().len(), n, "{topic}");
        assert_eq!(rig.last(topic), "", "{topic}");
        rig.deliver(topic, "");
        rig.run(&mut c, 1);
        let sub = rig.submitted();
        assert_eq!(sub.len(), n + 1, "{topic}");
        assert_eq!(sub[n].kind, kind, "{topic}");
        assert_eq!(sub[n].valve, valve, "{topic}");
    }
    assert_eq!(rig.shared.regulator_state().command_seq, 6);
    // STOP on a valve target (protocol 3)
    rig.deliver("VdMot/valves/1/target/set", "STOP");
    rig.run(&mut c, 1);
    let sub = rig.submitted();
    assert_eq!(sub.len(), 7);
    assert_eq!(sub[6].kind, StmCommandType::StopValve);
    assert_eq!(sub[6].valve, 0);
    // a full queue rejects a button
    rig.host.state().submit_result = false;
    rig.deliver("VdMot/cmd/detect", "PRESS");
    rig.deliver("VdMot/cmd/detect", "");
    rig.run(&mut c, 2);
    let ev = rig.rejected();
    assert_eq!(ev.len(), 1);
    assert_eq!(Rig::text(&ev[0]), "queue full");
}

#[test]
fn stop_and_safe_exit_need_protocol_3() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up(); // protocol 2
    rig.settle(&mut c, 2);
    rig.deliver("VdMot/cmd/stop", "PRESS");
    rig.deliver("VdMot/valves/1/target/set", "STOP");
    rig.run(&mut c, 2);
    assert!(rig.submitted().is_empty());
    let ev = rig.rejected();
    assert_eq!(ev.len(), 2);
    assert_eq!(Rig::text(&ev[0]), "unsupported");
    assert_eq!(ev[1].arg1, 1);
}

#[test]
fn a_refused_target_is_submitted_again_the_newest_wins() {
    // H14
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.host.state().submit_result = false;
    rig.deliver("VdMot/valves/1/target/set", "10");
    rig.deliver("VdMot/valves/1/target/set", "20");
    rig.deliver("VdMot/valves/1/target/set", "30");
    rig.run(&mut c, 3);
    assert!(rig.submitted().is_empty());
    assert!(rig.rejected().is_empty());
    rig.host.state().submit_result = true;
    rig.run(&mut c, 2);
    let sub = rig.submitted();
    assert_eq!(sub.len(), 1);
    assert_eq!(sub[0].pos, 30);
    rig.run(&mut c, 2);
    assert_eq!(rig.submitted().len(), 1);
}

// ---------------------------------------------------------------- HA status

#[test]
fn ha_status_offline_online_commands_discovery_on_the_way_back() {
    // K1-4
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::MqttHa);
    rig.cfg(|c| c.mqtt.ha_discovery_on_connect = false);
    rig.settle(&mut c, 2);
    rig.deliver("homeassistant/status", "offline");
    rig.run(&mut c, 1);
    assert_eq!(rig.shared.regulator_state().ha, HaStatus::Offline);
    assert_eq!(rig.shared.status().ha_status, HaStatus::Offline);
    assert_eq!(
        regulator_cause(&rig.shared.regulator_state()),
        RegulatorCause::HaOffline
    );
    // never cleared
    assert!(rig.payloads("homeassistant/status").is_empty());
    // an accepted command brings it back without a discovery run
    rig.deliver("VdMot/valves/1/target/set", "10");
    rig.run(&mut c, 1);
    assert_eq!(rig.shared.regulator_state().ha, HaStatus::Online);
    assert_eq!(rig.shared.regulator_state().command_seq, 1);
    assert!(!rig.shared.status().discovery_running);
    // the status survives a software restart (RTC)
    rig.deliver("homeassistant/status", "offline");
    rig.run(&mut c, 1);
    c.begin();
    assert_eq!(rig.shared.regulator_state().ha, HaStatus::Offline);
    // online after offline runs discovery when enabled; a second online does not
    rig.cfg(|c| c.mqtt.ha_discovery_on_connect = true);
    rig.new_revision();
    rig.run(&mut c, 3);
    rig.deliver("homeassistant/status", "online");
    rig.run(&mut c, 1);
    assert_eq!(rig.shared.regulator_state().ha, HaStatus::Online);
    assert!(rig.shared.status().discovery_running);
    rig.run(&mut c, 400);
    assert!(!rig.shared.status().discovery_running);
    let runs = rig.events(EventCode::HaDiscoverySent).len();
    rig.deliver("homeassistant/status", "online");
    rig.run(&mut c, 2);
    assert!(!rig.shared.status().discovery_running);
    assert_eq!(rig.events(EventCode::HaDiscoverySent).len(), runs);
    // broker lost
    rig.broker.drop_connection();
    rig.refuse_connects(true);
    rig.run(&mut c, 1);
    assert!(!rig.shared.regulator_state().broker_connected);
    assert_eq!(
        regulator_cause(&rig.shared.regulator_state()),
        RegulatorCause::BrokerDown
    );
    // MQTT switched off
    rig.cfg(|c| c.mqtt.mode = MqttMode::Off);
    rig.new_revision();
    rig.run(&mut c, 1);
    assert_eq!(rig.shared.regulator_state().mode, MqttMode::Off);
    assert_eq!(
        regulator_cause(&rig.shared.regulator_state()),
        RegulatorCause::Alive
    );
}

#[test]
fn ha_status_is_ignored_outside_ha_mode() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.deliver("homeassistant/status", "offline");
    rig.run(&mut c, 1);
    assert_eq!(rig.shared.regulator_state().ha, HaStatus::Unknown);
}

// ---------------------------------------------------------------- values

#[test]
fn stm_status_per_link_state_once_per_change() {
    // K3-4
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.settle(&mut c, 40);
    assert_eq!(rig.payloads("VdMot/stm/status"), vec!["online"]);
    assert!(rig.broker.state().published_to(b"VdMot/stm/status")[0].retain);
    rig.snap(|s| s.link = LinkState::Degraded);
    rig.publish_snap();
    rig.run(&mut c, 3);
    assert_eq!(rig.count("VdMot/stm/status"), 1);
    for l in [
        LinkState::Down,
        LinkState::Unknown,
        LinkState::Booting,
        LinkState::Suspended,
    ] {
        rig.snap(|s| s.link = l);
        rig.publish_snap();
        rig.run(&mut c, 3);
        assert_eq!(rig.last("VdMot/stm/status"), "offline", "{l:?}");
    }
    assert_eq!(rig.count("VdMot/stm/status"), 2);
    rig.snap(|s| s.link = LinkState::Up);
    rig.publish_snap();
    rig.run(&mut c, 3);
    assert_eq!(rig.count("VdMot/stm/status"), 3);
    assert_eq!(rig.last("VdMot/stm/status"), "online");
    // every full publish repeats it
    rig.run(&mut c, 600); // > publish_interval_s (10 s)
    assert!(rig.count("VdMot/stm/status") > 3);
}

#[test]
fn failsafe_lease_valve_and_common_state() {
    // K1-5
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| {
        s.lease.state = LeaseState::Expired;
        s.valves[0].fs_override = true;
        s.valves[0].health = HEALTH_FAILSAFE;
    });
    rig.settle(&mut c, 40);
    assert_eq!(rig.last("VdMot/failsafe"), "1");
    assert!(
        rig.broker
            .state()
            .published_to(b"VdMot/failsafe")
            .last()
            .unwrap()
            .retain
    );
    assert_eq!(rig.last("VdMot/valves/1/failsafe/value"), "lease");
    assert_eq!(rig.last("VdMot/common/state/value"), "info");
    assert_eq!(rig.last("VdMot/diag/stm/lease"), "expired");
    rig.snap(|s| {
        s.lease.state = LeaseState::Running;
        s.valves[0].fs_override = false;
        s.valves[0].health = 0;
    });
    rig.publish_snap();
    rig.run(&mut c, 300); // > min_delay_s
    assert_eq!(rig.last("VdMot/failsafe"), "0");
    assert_eq!(rig.last("VdMot/valves/1/failsafe/value"), "off");
    assert_eq!(rig.last("VdMot/common/state/value"), "ok");
    assert_eq!(rig.last("VdMot/diag/stm/lease"), "running");
}

#[test]
fn a_blocked_valve_at_its_failsafe_position() {
    // K2-3
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| {
        s.proto = 3;
        let v = &mut s.valves[0];
        v.has_v3 = true;
        v.stm_flags = STM_FLAG_FS_BLOCKED;
        v.fs_pct = 50;
        v.status = 9;
        v.health = HEALTH_BLOCKED;
        s.have_status = true;
        s.status.v3 = true;
        s.status.safe_mode = true;
    });
    rig.settle(&mut c, 40);
    assert_eq!(rig.last("VdMot/valves/1/failsafe/value"), "blocked");
    assert_eq!(rig.last("VdMot/valves/1/problem/value"), "1");
    assert_eq!(rig.last("VdMot/common/state/value"), "error");
    assert_eq!(rig.last("VdMot/diag/stm/safeMode"), "1");
}

#[test]
fn requested_sync_and_the_read_back_target() {
    // W5-2
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| {
        let v = &mut s.valves[0];
        v.stm_target_known = true;
        v.stm_target = 30;
        v.desired_valid = true;
        v.desired = 30;
        v.sync = TargetSync::Synced;
    });
    rig.settle(&mut c, 40);
    assert_eq!(rig.last("VdMot/valves/1/target/value"), "30");
    assert_eq!(rig.last("VdMot/valves/1/requested/value"), "30");
    assert_eq!(rig.last("VdMot/valves/1/sync/value"), "synced");
    assert_eq!(rig.last("VdMot/valves/1/problem/value"), "0");
    let targets = rig.count("VdMot/valves/1/target/value");
    rig.snap(|s| {
        let v = &mut s.valves[0];
        v.desired = 60;
        v.source = TargetSource::Mqtt;
        v.sync = TargetSync::AwaitVerify;
    });
    rig.publish_snap();
    rig.run(&mut c, 300); // > min_delay_s
    assert_eq!(rig.last("VdMot/valves/1/requested/value"), "60");
    assert_eq!(rig.last("VdMot/valves/1/sync/value"), "await_verify");
    assert_eq!(rig.last("VdMot/valves/1/target/value"), "30");
    rig.snap(|s| {
        s.valves[0].stm_target = 60;
        s.valves[0].sync = TargetSync::Synced;
    });
    rig.publish_snap();
    rig.run(&mut c, 300);
    assert_eq!(rig.last("VdMot/valves/1/target/value"), "60");
    assert_eq!(rig.last("VdMot/valves/1/sync/value"), "synced");
    assert!(rig.count("VdMot/valves/1/target/value") > targets);
    // delivery failed: problem
    rig.snap(|s| {
        s.valves[0].sync = TargetSync::Failed;
        s.valves[0].health = HEALTH_TARGET_UNCONFIRMED;
    });
    rig.publish_snap();
    rig.run(&mut c, 300);
    assert_eq!(rig.last("VdMot/valves/1/sync/value"), "failed");
    assert_eq!(rig.last("VdMot/valves/1/problem/value"), "1");
    // read-back pending: nothing published for target
    let n = rig.count("VdMot/valves/1/target/value");
    rig.snap(|s| {
        s.valves[0].stm_target_known = false;
        s.valves[0].sync = TargetSync::Pending;
    });
    rig.publish_snap();
    rig.run(&mut c, 300);
    assert_eq!(rig.count("VdMot/valves/1/target/value"), n);
}

#[test]
fn a_failed_temperature_and_link_loss_show_as_problems() {
    // K4-4, W5-3
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| {
        s.valves[0].temp1 = TEMP_READ_ERROR;
        s.valves[0].health = HEALTH_TEMP_FAILED;
    });
    rig.settle(&mut c, 40);
    assert_eq!(rig.last("VdMot/valves/1/temp1/value"), "failed");
    assert_eq!(rig.last("VdMot/valves/1/problem/value"), "1");
    rig.snap(|s| {
        s.valves[0].temp1 = 215;
        s.valves[0].health = 0;
    });
    rig.publish_snap();
    rig.run(&mut c, 300); // > min_delay_s
    assert_eq!(rig.last("VdMot/valves/1/temp1/value"), "21.5");
    assert_eq!(rig.last("VdMot/valves/1/problem/value"), "0");
    rig.snap(|s| {
        s.link = LinkState::Down;
        s.valves[0].health = HEALTH_STALE;
    });
    rig.publish_snap();
    rig.run(&mut c, 300);
    assert_eq!(rig.last("VdMot/stm/status"), "offline");
    assert_eq!(rig.last("VdMot/valves/1/problem/value"), "1");
    assert_eq!(rig.last("VdMot/common/state/value"), "error");
}

fn one_wire(family: u8, last: u8) -> OneWireId {
    let mut id = OneWireId::default();
    id.b[0] = family;
    id.b[7] = last;
    id
}

#[test]
fn unnamed_sensors_use_the_bus_index_inactive_volts_are_published() {
    // E22, E23
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    let t1 = one_wire(0x28, 1);
    let v1 = one_wire(0x26, 2);
    rig.cfg(|c| {
        c.temps[0].active = true;
        c.temps[0].id = t1;
        c.volts[0].id = v1; // inactive
        copy_string(&mut c.volts[0].unit, b"V");
    });
    rig.link_up();
    rig.snap(|s| {
        s.temp_count = 5;
        s.temps[4].id = t1;
        s.temps[4].seen = true;
        s.temps[4].raw = 200;
        s.volt_count = 3;
        s.volts[2].id = v1;
        s.volts[2].seen = true;
        s.volts[2].vad = 1200;
    });
    rig.settle(&mut c, 40);
    assert_eq!(
        rig.last("VdMot/temps/5/id/value"),
        "28-00-00-00-00-00-00-01"
    );
    assert_eq!(rig.last("VdMot/temps/5/value/value"), "20.0");
    assert!(rig.payloads("VdMot/temps/1/id/value").is_empty());
    assert_eq!(
        rig.last("VdMot/sensors/3/id/value"),
        "26-00-00-00-00-00-00-02"
    );
    assert_eq!(rig.last("VdMot/sensors/3/value/value"), "12.000");
    assert_eq!(rig.last("VdMot/sensors/3/unit/value"), "V");
    // off the bus: an unnamed sensor is not published at all
    let n = rig.published_len();
    rig.snap(|s| {
        s.temp_count = 0;
        s.volt_count = 0;
    });
    rig.publish_snap();
    rig.run(&mut c, 700);
    for p in &rig.published()[n..] {
        assert!(!s(&p.topic).contains("VdMot/temps/"), "{}", s(&p.topic));
    }
}

#[test]
fn diag_version_started_calibration_next_counters() {
    // E29-3, W14
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| {
        s.version.valid = true;
        s.version.major = 2;
        s.version.minor = 1;
        copy_string(&mut s.version.suffix, b"-revamped");
        copy_string(&mut s.version.hw, b"C2");
        s.have_status = true;
        s.status.uptime_s = 1000;
    });
    {
        let mut h = rig.host.state();
        h.local_time.valid = true;
        h.local_time.epoch = 1_790_072_393;
        h.calib_next = 1_790_080_000;
    }
    rig.settle(&mut c, 4);
    assert_eq!(rig.last("VdMot/diag/stm/version"), "2.1.0-revamped_C2");
    assert_eq!(
        rig.payloads("VdMot/diag/stm/started"),
        vec!["2026-09-22T10:03:13+00:00"]
    );
    assert_eq!(
        rig.last("VdMot/diag/calibration/next"),
        "2026-09-22T12:26:40+00:00"
    );
    assert_eq!(rig.last("VdMot/diag/mqtt/commandsRejected"), "0");
    assert_eq!(rig.last("VdMot/diag/mqtt/eventsSuppressed"), "0");
    // uptime +10 and epoch +10: nothing new
    rig.snap(|s| s.status.uptime_s = 1010);
    rig.host.state().local_time.epoch = 1_790_072_403;
    rig.publish_snap();
    rig.run(&mut c, 3);
    assert_eq!(rig.count("VdMot/diag/stm/started"), 1);
    // STM reboot: a new start time
    rig.snap(|s| s.status.uptime_s = 5);
    rig.publish_snap();
    rig.run(&mut c, 3);
    assert_eq!(
        rig.last("VdMot/diag/stm/started"),
        "2026-09-22T10:19:58+00:00"
    );
    rig.host.state().calib_next = 0;
    rig.run(&mut c, 2);
    assert_eq!(rig.last("VdMot/diag/calibration/next"), "");
    // counters at most every 10 s
    rig.deliver("VdMot/valves/9/target/set", "1");
    rig.run(&mut c, 2);
    assert_eq!(rig.last("VdMot/diag/mqtt/commandsRejected"), "0");
    rig.run(&mut c, 500);
    assert_eq!(rig.last("VdMot/diag/mqtt/commandsRejected"), "1");
}

#[test]
fn events_12_valves_of_one_code_become_one_message() {
    // W14-4
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    for v in 0..VALVE_COUNT {
        rig.log(EventCode::ValveStale, v, 60, 0);
    }
    rig.run(&mut c, 150); // 3 s
    let ev = rig.payloads("VdMot/events");
    assert_eq!(ev.len(), 1);
    assert!(
        ev[0].contains(
            "\"name\":\"valve_stale\",\"event_type\":\"valve_stale\",\"valve\":null,\
             \"valves\":[1,2,3,4,5,6,7,8,9,10,11,12]"
        ),
        "{}",
        ev[0]
    );
    assert!(!rig.broker.state().published_to(b"VdMot/events")[0].retain);
    // a single system event goes out at once
    rig.log(EventCode::LinkDown, NO_VALVE, 3, 0);
    rig.run(&mut c, 5); // the task starts reading the log from its start again, 4 events a pass
    let ev = rig.payloads("VdMot/events");
    assert_eq!(ev.len(), 2);
    assert!(ev[1].contains("\"code\":302"), "{}", ev[1]);
}

// ---------------------------------------------------------------- discovery

#[test]
fn discovery_the_2_0_0_migration_runs_once_in_ha_mode() {
    // K3-3, K3-5
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::MqttHa);
    rig.cfg(|c| c.mqtt.ha_discovery_on_connect = false);
    rig.host.state().ha_layout = 0;
    rig.snap(|s| s.sensors_settled = true); // automatic runs start at once
    c.begin();
    rig.run(&mut c, 500);
    assert_eq!(
        rig.last("homeassistant/sensor/VdMot/diag_stm_uptime/config"),
        ""
    );
    assert_eq!(rig.host.state().ha_layout_sets, vec![2]);
    assert!(rig
        .last("homeassistant/text/VdMot/state/config")
        .contains("\"unique_id\":\"VdMot.common.state\""));
    let list = rig.dev.fs.read(LIST_FILE).expect("the list");
    assert!(s(&list).starts_with("homeassistant/text/VdMot/state/config\n"));
    assert!(!rig.dev.fs.exists(LIST_TMP));
    let sent = rig.events(EventCode::HaDiscoverySent);
    assert_eq!(sent.len(), 1);
    assert!(sent[0].arg1 > 20);
    // the next connect: no run
    let before = rig.published_len();
    rig.shared.request_reconnect();
    rig.run(&mut c, 100);
    for p in &rig.published()[before..] {
        assert!(!p.topic.starts_with(b"homeassistant/"), "{}", s(&p.topic));
    }
    // delete (empty list) keeps the file
    rig.shared.request_discovery(DiscoveryAction::Delete);
    rig.run(&mut c, 300);
    assert_eq!(rig.dev.fs.read(LIST_FILE).expect("the list"), b"");
    rig.shared
        .request_discovery(DiscoveryAction::DeleteAndPublish);
    rig.run(&mut c, 500);
    let list = rig.dev.fs.read(LIST_FILE).expect("the list");
    assert!(s(&list).starts_with("homeassistant/text/VdMot/state/config\n"));
}

#[test]
fn discovery_a_renamed_valve_loses_its_old_config_first() {
    // W4-8
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::MqttHa);
    rig.cfg(|c| {
        copy_string(&mut c.valves[0].name, b"Old");
    });
    rig.snap(|s| s.sensors_settled = true); // automatic runs start at once
    c.begin();
    rig.run(&mut c, 500);
    assert!(s(&rig.dev.fs.read(LIST_FILE).unwrap()).contains("valves_state_Old"));
    rig.cfg(|c| {
        copy_string(&mut c.valves[0].name, b"New");
    });
    rig.new_revision();
    let before = rig.published_len();
    rig.run(&mut c, 500);
    let p = rig.published();
    let del_old = p.iter().enumerate().skip(before).position(|(_, m)| {
        m.topic == b"homeassistant/text/VdMot/valves_state_Old/config" && m.payload.is_empty()
    });
    let pub_new = p.iter().enumerate().skip(before).position(|(_, m)| {
        m.topic == b"homeassistant/text/VdMot/valves_state_New/config" && !m.payload.is_empty()
    });
    assert!(del_old.is_some() && pub_new.is_some());
    assert!(del_old < pub_new);
    let list = s(&rig.dev.fs.read(LIST_FILE).unwrap());
    assert!(list.contains("valves_state_New"));
    assert!(!list.contains("valves_state_Old"));
    assert!(rig.clean_session());
    // C++ also checked the stdio buffers of the list files (512 B each): the Rust ports are
    // unbuffered, the list is read through the run's 512 B buffer (tests_mut)
}

#[test]
fn discovery_manual_runs_in_mode_1_none_automatic() {
    // E24-2
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run(&mut c, 500);
    assert!(rig
        .last("homeassistant/text/VdMot/state/config")
        .contains("\"name\":\"state\""));
    assert_eq!(rig.events(EventCode::HaDiscoverySent).len(), 1);
    rig.shared.request_reconnect();
    let before = rig.published_len();
    rig.run(&mut c, 100);
    for p in &rig.published()[before..] {
        assert!(!p.topic.starts_with(b"homeassistant/"), "{}", s(&p.topic));
    }
}

#[test]
fn discovery_a_failed_publish_aborts_the_run_the_next_connect_starts_again() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::MqttHa);
    {
        let mut h = rig.host.state();
        h.ha_cleanup_done = false;
        h.ha_layout = 0;
    }
    rig.snap(|s| s.sensors_settled = true); // automatic runs start at once
    c.begin();
    rig.run(&mut c, 30);
    assert!(rig.shared.status().discovery_running);
    rig.script.get().fail_next_publish = true;
    rig.run(&mut c, 1);
    assert!(!rig.shared.status().discovery_running);
    assert_eq!(rig.host.state().ha_cleanup_marks, 0);
    assert!(rig.host.state().ha_layout_sets.is_empty());
    assert!(rig.events(EventCode::HaDiscoverySent).is_empty());
    rig.run(&mut c, 800);
    assert_eq!(rig.host.state().ha_cleanup_marks, 1);
    assert_eq!(rig.host.state().ha_layout_sets, vec![2]);
    assert_eq!(rig.events(EventCode::HaDiscoverySent).len(), 1);
}

// ---------------------------------------------------------------- task

#[test]
fn the_watchdog_is_fed_after_every_publish() {
    // E12-4
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    c.begin();
    rig.run(&mut c, 1);
    let pubs = rig.script.get().publishes;
    let feeds = rig.dev.watchdog.feeds();
    rig.run(&mut c, 1);
    let n = rig.script.get().publishes - pubs;
    assert!(n > 3);
    assert!(rig.dev.watchdog.feeds() - feeds > n); // at least one per publish and the pass
}

#[test]
fn calibration_end_the_end_of_the_last_calibration_since_boot() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    assert_eq!(rig.shared.calibration_end(0), None);
    rig.snap(|s| s.valves[0].calibrating = true);
    rig.settle(&mut c, 2);
    {
        let mut h = rig.host.state();
        h.local_time.valid = true;
        h.local_time.year = 2026;
        h.local_time.epoch = 1_790_000_000;
    }
    rig.snap(|s| s.valves[0].calibrating = false);
    rig.publish_snap();
    rig.run(&mut c, 2);
    let t = rig.shared.calibration_end(0).expect("an end");
    assert_eq!(t.epoch, 1_790_000_000);
    assert_eq!(rig.shared.calibration_end(1), None);
    assert_eq!(rig.shared.calibration_end(12), None);
}

#[test]
fn a_pending_restart_sends_offline_and_disconnects_cleanly() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    c.begin();
    rig.run(&mut c, 1);
    assert!(rig.session_up());
    rig.host.state().restart_pending = true;
    rig.run(&mut c, 2);
    let lwt = topic_of(&rig.host.state().cfg, Topic::Status);
    let last = rig.published().pop().unwrap();
    assert_eq!(last.topic, lwt.as_bytes());
    assert_eq!(last.payload, b"offline");
    assert!(last.retain);
    assert_eq!(rig.disconnects(), 1);
    assert_eq!(rig.shared.status().state, MqttState::Disabled);
    assert_eq!(rig.connects(), 1);
}

#[test]
fn requests_reconnect_and_discovery_are_taken_over_by_the_task() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    c.begin();
    rig.run(&mut c, 1);
    rig.shared.request_reconnect();
    rig.run(&mut c, 1);
    assert_eq!(rig.disconnects(), 1);
    assert_eq!(rig.connects(), 2);
}
