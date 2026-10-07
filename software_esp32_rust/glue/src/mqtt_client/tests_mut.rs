//! C++ `test_mqtt_client__mut.cpp`, pass by pass: the full publish layout, the on-change windows
//! and their order, the diag budget, values at their boundaries (offsets, buffer sizes, stale
//! sensors, paced counters), events, inbound edge cases, connection back-off and the discovery
//! plans.
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use super::rig::{s, Client, Rig, Track, PORT};
use super::*;
use crate::mqtt_conn::MQTT_CONNECT_UNAUTHORIZED;
use vdm_esp_core::common::{OneWireId, TEMP_READ_ERROR, VAD_FAILED};
use vdm_esp_core::event_log::{make_event, EVENT_TEXT_MAX};
use vdm_esp_core::stm_codec::StopReason;
use vdm_esp_core::valve_model::HEALTH_STALE;

const STATE: &str = "VdMot/common/state/value";
const UPTIME: &str = "VdMot/common/uptime/value";
const MESSAGE: &str = "VdMot/common/message/value";
const IP: &str = "VdMot/common/ip/value";

fn one_wire(family: u8, last: u8) -> OneWireId {
    let mut id = OneWireId::default();
    id.b[0] = family;
    id.b[7] = last;
    id
}

fn up(rig: &Rig) -> u32 {
    (rig.now() & 0xFFFF_FFFF) as u32
}

// ---------------------------------------------------------------- full publish

#[test]
fn full_publish_common_and_a_valve_one_valve_per_pass_four_other_slots_per_pass() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| {
        s.have_status = true;
        s.status.uptime_s = 77;
    });
    let mut t = Track::default();
    c.begin();
    rig.run_tracked(&mut c, 40, &mut t, &mut |_, _| {});
    // pass 0 connects; the full publish takes passes 1 (common + valve 1), 2..12 (valves
    // 2..12), 13..23 (34 temp, 8 volt, STM and system slots, four per pass)
    assert_eq!(t.passes_of(&rig, IP), vec![1]);
    assert_eq!(t.passes_of(&rig, UPTIME), vec![1]);
    assert_eq!(t.passes_of(&rig, "VdMot/valves/1/state/value"), vec![1]);
    assert_eq!(t.passes_of(&rig, "VdMot/diag/stm/uptime"), vec![23]);
    assert_eq!(rig.payloads("VdMot/diag/stm/uptime"), vec!["77"]);
    assert_eq!(t.passes_of(&rig, "VdMot/stm/status"), vec![23]);
    assert_eq!(t.passes_of(&rig, "VdMot/failsafe"), vec![23]);
    assert!(rig.index_of("VdMot/diag/stm/uptime") < rig.index_of("VdMot/stm/status"));
}

#[test]
fn full_publish_no_uptime_topic_without_up_time() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| c.mqtt.up_time = false);
    rig.settle(&mut c, 40);
    assert_eq!(rig.count(UPTIME), 0);
    assert_eq!(rig.count(STATE), 1);
}

#[test]
fn full_publish_the_stm_uptime_needs_new_diag_protocol_2_and_a_status() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| {
        s.proto = 1;
        s.have_status = true;
    });
    rig.settle(&mut c, 40);
    assert_eq!(rig.count("VdMot/diag/stm/uptime"), 0);
    assert_eq!(rig.count("VdMot/diag/stm/resets"), 0);
    assert_eq!(rig.count("VdMot/diag/stm/started"), 0);
}

#[test]
fn full_publish_no_stm_uptime_without_a_status() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.settle(&mut c, 40);
    assert_eq!(rig.count("VdMot/diag/stm/uptime"), 0);
    assert_eq!(rig.count("VdMot/diag/stm/resets"), 0);
}

#[test]
fn full_publish_no_stm_uptime_without_new_diag() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| c.mqtt.new_diag = false);
    rig.link_up();
    rig.snap(|s| s.have_status = true);
    rig.settle(&mut c, 40);
    assert_eq!(rig.count("VdMot/diag/stm/uptime"), 0);
    assert_eq!(rig.count(STATE), 1);
}

#[test]
fn full_publish_common_ip_once_per_connection() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.host.state().uptime_s = Some(1000);
    c.begin();
    rig.run(&mut c, 1 + 1245); // three full publishes
    assert_eq!(rig.count(IP), 1);
    assert_eq!(rig.count(STATE), 3);
    rig.shared.request_reconnect();
    rig.run(&mut c, 40);
    assert_eq!(rig.count(IP), 2);
}

// ---------------------------------------------------------------- on change

/// Valve 1 with a calibration end, temp slot 1 and volt slot 1 on the bus, an STM with a status,
/// a fixed ESP uptime; the clock when the broker session can start.
fn steady_setup(rig: &Rig, c: &mut Client<'_>) -> u64 {
    rig.cfg(|c| {
        c.temps[0].active = true;
        c.temps[0].id = one_wire(0x28, 1);
        c.volts[0].id = one_wire(0x26, 2);
        copy_string(&mut c.volts[0].unit, b"V");
    });
    rig.link_up();
    let now = up(rig);
    rig.snap(|s| {
        s.have_status = true;
        s.status.uptime_s = 500;
        s.temp_count = 1;
        s.temps[0].id = one_wire(0x28, 1);
        s.temps[0].raw = 215;
        s.temps[0].seen = true;
        s.temps[0].last_seen_ms = now;
        s.volt_count = 1;
        s.volts[0].id = one_wire(0x26, 2);
        s.volts[0].vad = 1234;
        s.volts[0].seen = true;
        s.volts[0].last_seen_ms = now;
    });
    {
        let mut h = rig.host.state();
        h.uptime_s = Some(1000);
        let lt = &mut h.local_time;
        lt.valid = true;
        lt.year = 2026;
        lt.month = 9;
        lt.mday = 21;
        lt.wday = 1;
        lt.hour = 14;
        lt.epoch = 1_790_000_000;
        // a calibration ended before the broker session
        h.net_up = false;
    }
    rig.snap(|s| s.valves[0].calibrating = true);
    rig.publish_snap();
    c.begin();
    rig.run(c, 1);
    rig.snap(|s| s.valves[0].calibrating = false);
    rig.publish_snap();
    rig.run(c, 1);
    rig.host.state().net_up = true;
    rig.now()
}

#[test]
fn on_change_steady_values_go_out_with_the_full_publishes_only() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    steady_setup(&rig, &mut c);
    rig.snap(|s| s.lease.state = LeaseState::Expired);
    rig.publish_snap();
    rig.run(&mut c, 1 + 1245); // full publishes at +0.1, +10.1 and +20.1 s
    for t in [
        "VdMot/common/state/value",
        "VdMot/common/uptime/value",
        "VdMot/common/message/value",
        "VdMot/valves/1/state/value",
        "VdMot/valves/1/calibration/date/value",
        "VdMot/temps/1/value/value",
        "VdMot/sensors/1/value/value",
        "VdMot/diag/stm/uptime",
        "VdMot/stm/status",
        "VdMot/failsafe",
    ] {
        assert_eq!(rig.count(t), 3, "{t}");
    }
    assert_eq!(rig.last("VdMot/failsafe"), "1");
    assert_eq!(
        rig.last("VdMot/valves/1/calibration/date/value"),
        "Monday, September 21.2026 14:00:00"
    );
}

#[test]
fn on_change_each_change_goes_out_after_min_delay_before_the_next_full_publish() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| c.valves[1].active = true);
    let t0 = steady_setup(&rig, &mut c);
    rig.snap(|s| s.valves[1].known = true);
    rig.publish_snap();
    // full publishes start at t0 + 100 + 10000 n and end 440 ms later
    let full_end = |n: u64| t0 + 540 + 10000 * n;
    let mut t = Track::default();
    rig.run_tracked(&mut c, 1 + 1795, &mut t, &mut |_, now| {
        if now == full_end(0) + 500 {
            // cycle 0: message, temp value, volt value, calibration
            rig.log(EventCode::LowHeap, NO_VALVE, 1, 2);
            rig.snap(|s| {
                s.temps[0].raw = 225;
                s.volts[0].vad = 1300;
                s.valves[0].calibrating = true;
            });
            rig.publish_snap();
        } else if now == full_end(0) + 1000 {
            rig.snap(|s| s.valves[0].calibrating = false);
            rig.publish_snap();
        } else if now == full_end(1) + 500 {
            // cycle 1: state, temp and volt failed
            rig.snap(|s| {
                s.valves[1].health = HEALTH_STALE;
                s.temps[0].raw = TEMP_READ_ERROR;
                s.volts[0].vad = VAD_FAILED;
            });
            rig.publish_snap();
        } else if now == full_end(2) + 500 {
            // cycle 2: ESP uptime
            rig.host.state().uptime_s = Some(1001);
        }
    }); // to t0 + 36 s
    let ends = t.times_of(&rig, "VdMot/stm/status");
    assert_eq!(ends.len(), 4);
    for (n, e) in ends.iter().enumerate() {
        assert_eq!(*e, full_end(n as u64));
    }
    // two slots per pass: common and a valve first, temp and volt 20 ms later
    let rows: [(u64, &str, u64, Option<&str>); 8] = [
        (0, MESSAGE, 5000, Some("low heap (free 1, min 2)")),
        (0, "VdMot/valves/1/calibration/date/value", 5000, None),
        (0, "VdMot/temps/1/value/value", 5020, Some("22.5")),
        (0, "VdMot/sensors/1/value/value", 5020, Some("13.000")),
        (1, STATE, 5000, Some("info")),
        (1, "VdMot/temps/1/value/value", 5020, Some("failed")),
        (1, "VdMot/sensors/1/value/value", 5020, Some("failed")),
        (2, UPTIME, 5000, Some("0d 0:16:41")),
    ];
    for (cycle, topic, after, payload) in rows {
        let from = full_end(cycle);
        let times = t.times_between(&rig, topic, from, from + 9560);
        assert_eq!(times.len(), 1, "{cycle} {topic}");
        assert_eq!(times[0], from + after, "{cycle} {topic}");
        if let Some(want) = payload {
            let p = rig.payloads(topic);
            for (i, at) in t.times_of(&rig, topic).iter().enumerate() {
                if *at == times[0] {
                    assert_eq!(p[i], want, "{cycle} {topic}");
                }
            }
        }
    }
    // unchanged slots stay quiet in the same windows
    assert_eq!(
        t.times_between(&rig, STATE, full_end(0), full_end(0) + 9560)
            .len(),
        1
    );
    assert!(t
        .times_between(
            &rig,
            "VdMot/temps/1/value/value",
            full_end(2),
            full_end(2) + 9560
        )
        .is_empty());
    assert!(t
        .times_between(
            &rig,
            "VdMot/valves/2/state/value",
            full_end(0),
            full_end(0) + 9560
        )
        .is_empty());
}

#[test]
fn on_change_two_slots_per_pass_in_slot_order_from_common() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        c.valves[1].active = true;
        c.valves[2].active = true;
    });
    rig.link_up();
    rig.snap(|s| {
        s.valves[1].known = true;
        s.valves[2].known = true;
    });
    rig.host.state().uptime_s = Some(1000);
    rig.publish_snap();
    let t0 = rig.now();
    let mut t = Track::default();
    c.begin();
    rig.run_tracked(&mut c, 1 + 300, &mut t, &mut |_, now| {
        if now != t0 + 1040 {
            return;
        }
        rig.log(EventCode::LowHeap, NO_VALVE, 1, 2);
        rig.snap(|s| {
            for v in 0..3 {
                s.valves[v].position = 10 + v as u8;
            }
        });
        rig.publish_snap();
    });
    let st = t.passes_of(&rig, STATE);
    assert_eq!(st.len(), 2);
    let p = st[1];
    assert_eq!(t.at[p], t0 + 5540);
    assert_eq!(t.passes_of(&rig, "VdMot/valves/1/actual/value"), vec![1, p]);
    assert_eq!(
        t.passes_of(&rig, "VdMot/valves/2/actual/value"),
        vec![2, p + 1]
    );
    assert_eq!(
        t.passes_of(&rig, "VdMot/valves/3/actual/value"),
        vec![3, p + 1]
    );
}

#[test]
fn on_change_the_system_state_follows_the_stm_safe_mode_of_a_v3_status_only() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| {
        s.status.v3 = true;
        s.status.safe_mode = true; // without a status
    });
    rig.settle(&mut c, 40);
    assert_eq!(rig.last(STATE), "ok");
    rig.snap(|s| {
        s.have_status = true;
        s.status.v3 = false; // not a v3 status
    });
    rig.publish_snap();
    rig.run(&mut c, 300);
    assert_eq!(rig.last(STATE), "ok");
    rig.snap(|s| s.status.v3 = true);
    rig.publish_snap();
    rig.run(&mut c, 300);
    assert_eq!(rig.last(STATE), "error");
}

// ---------------------------------------------------------------- values

#[test]
fn values_valve_temperatures_with_the_offset_of_their_1_based_slot() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        c.valves[1].active = true;
        c.temps[0].offset = 3;
        c.temps[1].offset = 5;
        c.temps[2].offset = 13;
        c.temps[32].offset = 7;
        c.temps[33].offset = 11;
        // C++: non-zero bytes next to the slot array (a Rust slot index is bounds-checked)
        copy_string(&mut c.valves[11].name, b"ZZZZZZZZZZ");
        copy_string(&mut c.volts[0].name, b"YYYYYYYYYY");
        c.volts[0].offset = 1.1;
    });
    rig.link_up();
    rig.snap(|s| {
        s.valves[0].temp1 = 200;
        s.valves[0].sensor_slot[0] = 1;
        s.valves[0].temp2 = 200;
        s.valves[0].sensor_slot[1] = 34;
        s.valves[1].known = true;
        s.valves[1].temp1 = 200;
        s.valves[1].sensor_slot[0] = 0;
        s.valves[1].temp2 = 200;
        s.valves[1].sensor_slot[1] = 35;
    });
    rig.publish_snap();
    rig.settle(&mut c, 40);
    assert_eq!(rig.last("VdMot/valves/1/temp1/value"), "20.3");
    assert_eq!(rig.last("VdMot/valves/1/temp2/value"), "21.1");
    assert_eq!(rig.last("VdMot/valves/2/temp1/value"), "20.0");
    assert_eq!(rig.last("VdMot/valves/2/temp2/value"), "20.0");
}

#[test]
fn values_the_topic_and_payload_options_of_the_config_reach_the_broker() {
    // retained, pathAsRoot, plainText, germanDecimal and diag as the task applies them, and the
    // payloads of calibration/repetitions and diag/moves (the core pins the formatters; the
    // C++ suite had no task case of them either)
    for flipped in [false, true] {
        let rig = Rig::new();
        let mut c = rig.client();
        rig.use_mqtt(MqttMode::Mqtt);
        rig.cfg(|c| {
            c.mqtt.retained = !flipped;
            c.mqtt.path_as_root = flipped;
            c.mqtt.plain_text = !flipped;
            c.mqtt.german_decimal = flipped;
            c.mqtt.diag = !flipped;
        });
        rig.link_up();
        rig.snap(|s| {
            let v = &mut s.valves[0];
            v.status = 1;
            v.temp1 = 215;
            v.sensor_slot[0] = 0;
            v.calib_retries = 2;
            v.moves = 7;
        });
        rig.publish_snap();
        rig.settle(&mut c, 40);
        let root = if flipped { "/VdMot/" } else { "VdMot/" };
        let last = |t: &str| {
            let topic = format!("{root}{t}");
            let p = rig.published();
            let found = p.iter().rev().find(|p| p.topic == topic.as_bytes());
            found
                .unwrap_or_else(|| panic!("{topic} not published"))
                .clone()
        };
        // the setting topics follow `retained`, the availability topics are always retained
        assert_eq!(last("valves/1/actual/value").retain, !flipped);
        assert_eq!(last("valves/1/state/value").retain, !flipped);
        assert!(last("status").retain);
        assert!(last("stm/status").retain);
        let (state, temp) = if flipped {
            ("1", "21,5")
        } else {
            ("idle", "21.5")
        };
        assert_eq!(s(&last("valves/1/state/value").payload), state);
        assert_eq!(s(&last("valves/1/temp1/value").payload), temp);
        let system = s(&last("common/state/value").payload);
        assert_eq!(
            system.bytes().all(|b| b.is_ascii_digit()),
            flipped,
            "{system}"
        );
        assert_eq!(
            s(&last("valves/1/calibration/repetitions/value").payload),
            "2"
        );
        // the legacy diag topics only with `diag`
        let diag: std::collections::BTreeSet<Vec<u8>> = rig
            .published()
            .into_iter()
            .map(|p| p.topic)
            .filter(|t| t.starts_with(format!("{root}valves/1/diag/").as_bytes()))
            .collect();
        assert_eq!(diag.len(), if flipped { 0 } else { 5 });
        if !flipped {
            assert_eq!(s(&last("valves/1/diag/moves/value").payload), "7");
        }
        // every topic under the root
        assert!(rig
            .published()
            .iter()
            .all(|p| p.topic.starts_with(root.as_bytes())));
    }
}

#[test]
fn values_valve_state_diag_counters_no_calibration_date_inactive_valves() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| {
        let v = &mut s.valves[0];
        v.status = 1;
        v.mean_current = 12;
        v.open_count = 0x8000_0000;
        v.close_count = 7;
        v.moves = 3;
        v.dead_zone = -4;
        s.valves[1].known = true; // inactive
    });
    rig.publish_snap();
    rig.settle(&mut c, 40);
    assert_eq!(rig.last("VdMot/valves/1/state/value"), "idle");
    assert_eq!(rig.last("VdMot/valves/1/diag/meanCurrrent/value"), "12");
    assert_eq!(
        rig.last("VdMot/valves/1/diag/openCount/value"),
        "-2147483648"
    );
    assert_eq!(rig.last("VdMot/valves/1/diag/closeCount/value"), "7");
    assert_eq!(rig.last("VdMot/valves/1/diag/deadZoneCount/value"), "-4");
    assert_eq!(rig.count("VdMot/valves/1/calibration/date/value"), 0);
    assert_eq!(rig.count_prefix("VdMot/valves/2/", 0), 0);
}

#[test]
fn values_without_separate_a_valve_without_a_published_target_has_no_echo() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| c.mqtt.separate = false);
    rig.link_up();
    rig.settle(&mut c, 40);
    assert_eq!(rig.count("VdMot/valves/1/target"), 0);
    rig.deliver("VdMot/valves/1/target", "0");
    rig.run(&mut c, 2);
    let sub = rig.submitted();
    assert_eq!(sub.len(), 1);
    assert_eq!(sub[0].pos, 0);
}

#[test]
fn values_sensor_names_of_10_characters_are_topic_segments() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        copy_string(&mut c.temps[0].name, b"ABCDEFGHIJ");
        c.temps[0].active = true;
        c.temps[0].id = one_wire(0x28, 1);
        copy_string(&mut c.volts[0].name, b"KLMNOPQRST");
        c.volts[0].id = one_wire(0x26, 2);
    });
    rig.link_up();
    rig.snap(|s| {
        s.temp_count = 1;
        s.temps[0].id = one_wire(0x28, 1);
        s.temps[0].raw = 215;
        s.temps[0].seen = true;
        s.volt_count = 1;
        s.volts[0].id = one_wire(0x26, 2);
        s.volts[0].vad = 1234;
        s.volts[0].seen = true;
    });
    rig.publish_snap();
    rig.settle(&mut c, 40);
    assert_eq!(rig.last("VdMot/temps/ABCDEFGHIJ/value/value"), "21.5");
    assert_eq!(rig.last("VdMot/sensors/KLMNOPQRST/value/value"), "12.340");
}

/// Temp slot 1 on the bus, last seen `age` ms before every pass.
fn temp_seen_ago(age: u32) -> Vec<String> {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        c.temps[0].active = true;
        c.temps[0].id = one_wire(0x28, 1);
    });
    rig.link_up();
    let seen = up(&rig).wrapping_sub(age);
    rig.snap(|s| {
        s.temp_count = 1;
        s.temps[0].id = one_wire(0x28, 1);
        s.temps[0].raw = 215;
        s.temps[0].seen = true;
        s.temps[0].last_seen_ms = seen;
    });
    rig.publish_snap();
    let mut t = Track::default();
    c.begin();
    rig.run_tracked(&mut c, 1 + 300, &mut t, &mut |_, now| {
        rig.snap(|s| s.temps[0].last_seen_ms = (now as u32).wrapping_sub(age));
        rig.publish_snap();
    });
    rig.payloads("VdMot/temps/1/value/value")
}

#[test]
fn values_a_sensor_seen_60_s_ago_is_current() {
    assert_eq!(temp_seen_ago(60000), vec!["21.5"]);
}

#[test]
fn values_a_sensor_seen_60_001_s_ago_is_stale() {
    assert_eq!(temp_seen_ago(60001), vec!["failed"]);
}

// ---------------------------------------------------------------- diag

#[test]
fn diag_stm_counters_link_protocol_lease_safe_mode_once_per_change() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| {
        s.have_status = true;
        s.status.resets = 1;
        s.status.rx_overflow = 2;
        s.status.parse_errors = 0;
    });
    rig.publish_snap();
    rig.settle(&mut c, 60);
    let v = |x: &[&str]| x.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(rig.payloads("VdMot/diag/stm/proto"), v(&["2"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/link"), v(&["up"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/resets"), v(&["1"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/rxOverflow"), v(&["2"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/parseErr"), v(&["0"]));
    assert_eq!(rig.payloads("VdMot/diag/calibration/active"), v(&["0"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/lease"), v(&["off"]));
    assert_eq!(rig.payloads("VdMot/diag/calibration/next"), v(&[""]));
    assert!(rig.payloads("VdMot/diag/stm/safeMode").is_empty()); // not a v3 status
    rig.snap(|s| {
        s.link = LinkState::Degraded;
        s.proto = 3;
        s.status.resets = 4;
        s.valves[0].calibrating = true;
        s.lease.state = LeaseState::Running;
    });
    rig.publish_snap();
    rig.host.state().calib_next = 1;
    rig.run(&mut c, 20);
    assert_eq!(rig.payloads("VdMot/diag/stm/proto"), v(&["2", "3"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/link"), v(&["up", "degraded"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/resets"), v(&["1", "4"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/rxOverflow"), v(&["2"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/parseErr"), v(&["0"]));
    assert_eq!(
        rig.payloads("VdMot/diag/calibration/active"),
        v(&["0", "1"])
    );
    assert_eq!(rig.payloads("VdMot/diag/stm/lease"), v(&["off", "running"]));
    assert_eq!(
        rig.payloads("VdMot/diag/calibration/next"),
        v(&["", "1970-01-01T00:00:01+00:00"])
    );
    rig.snap(|s| {
        s.status.rx_overflow = 5;
        s.status.parse_errors = 6;
        s.valves[0].calibrating = false;
        s.status.v3 = true;
    });
    rig.publish_snap();
    rig.run(&mut c, 20);
    assert_eq!(rig.payloads("VdMot/diag/stm/resets"), v(&["1", "4"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/rxOverflow"), v(&["2", "5"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/parseErr"), v(&["0", "6"]));
    assert_eq!(
        rig.payloads("VdMot/diag/calibration/active"),
        v(&["0", "1", "0"])
    );
    assert_eq!(rig.payloads("VdMot/diag/stm/safeMode"), v(&["0"]));
    rig.snap(|s| s.status.safe_mode = true);
    rig.publish_snap();
    rig.run(&mut c, 20);
    assert_eq!(rig.payloads("VdMot/diag/stm/safeMode"), v(&["0", "1"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/proto"), v(&["2", "3"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/link"), v(&["up", "degraded"]));
    assert_eq!(rig.payloads("VdMot/diag/stm/lease"), v(&["off", "running"]));
}

#[test]
fn diag_stm_version_up_to_31_characters_published_once() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| {
        s.version.valid = true;
        s.version.major = 2;
        s.version.minor = 1;
        copy_string(&mut s.version.suffix, b"-abcdefghijklmnopqrstuv"); // 23
        copy_string(&mut s.version.hw, b"C2");
    });
    rig.publish_snap();
    rig.settle(&mut c, 40);
    rig.run(&mut c, 20);
    assert_eq!(
        rig.payloads("VdMot/diag/stm/version"),
        vec!["2.1.0-abcdefghijklmnopqrstuv_C2"]
    );
    // 32 characters do not fit: the last version stays
    rig.snap(|s| {
        copy_string(&mut s.version.suffix, b"-abcdefghijklmnopqrstuvw");
    });
    rig.publish_snap();
    rig.run(&mut c, 20);
    assert_eq!(rig.count("VdMot/diag/stm/version"), 1);
}

#[test]
fn diag_stm_start_time_moves_by_more_than_60_s_needs_protocol_2_status_time() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| {
        s.have_status = true;
        s.status.uptime_s = 1000;
    });
    {
        let mut h = rig.host.state();
        h.local_time.valid = true;
        h.local_time.epoch = 1_790_001_000; // started 1790000000
    }
    rig.publish_snap();
    rig.settle(&mut c, 4);
    let started = "VdMot/diag/stm/started";
    assert_eq!(rig.payloads(started), vec!["2026-09-21T14:13:20+00:00"]);
    rig.host.state().local_time.epoch += 60; // moved by 60 s
    rig.run(&mut c, 3);
    assert_eq!(rig.count(started), 1);
    rig.host.state().local_time.epoch += 1; // 61 s
    rig.run(&mut c, 3);
    assert_eq!(rig.count(started), 2);
    assert_eq!(rig.last(started), "2026-09-21T14:14:21+00:00");
    rig.snap(|s| s.status.uptime_s = 1030); // back by 30 s
    rig.publish_snap();
    rig.run(&mut c, 3);
    assert_eq!(rig.count(started), 2);
    rig.host.state().local_time.epoch += 5000; // far off, but:
    rig.snap(|s| s.proto = 1); // protocol 1
    rig.publish_snap();
    rig.run(&mut c, 3);
    assert_eq!(rig.count(started), 2);
    rig.snap(|s| {
        s.proto = 2;
        s.have_status = false; // no status
    });
    rig.publish_snap();
    rig.run(&mut c, 3);
    assert_eq!(rig.count(started), 2);
    rig.snap(|s| s.have_status = true);
    rig.host.state().local_time.valid = false; // no valid time
    rig.publish_snap();
    rig.run(&mut c, 3);
    assert_eq!(rig.count(started), 2);
    {
        let up = i64::from(rig.host.state().snap.status.uptime_s);
        let mut h = rig.host.state();
        h.local_time.valid = true;
        h.local_time.epoch = up; // the STM started at epoch 0
    }
    rig.run(&mut c, 3);
    assert_eq!(rig.count(started), 2);
    // a new session: a start time close to the epoch is published as well
    rig.host.state().local_time.epoch = 100;
    rig.snap(|s| s.status.uptime_s = 50);
    rig.publish_snap();
    rig.shared.request_reconnect();
    rig.run(&mut c, 40);
    assert_eq!(rig.last(started), "1970-01-01T00:00:50+00:00");
}

#[test]
fn diag_paced_counters_go_out_10_s_after_their_last_publish() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    let topic = "VdMot/diag/mqtt/commandsRejected";
    let t0 = rig.now();
    let first = t0 + 100;
    let mut t = Track::default();
    c.begin();
    rig.run_tracked(&mut c, 1 + 1050, &mut t, &mut |_, now| {
        if now == first + 1000 || now == first + 11000 {
            rig.deliver("VdMot/valves/9/target/set", "50");
        }
        if now == first + 12000 {
            rig.dev.clock.advance_ms(19); // the passes move off the 20 ms grid
        }
    });
    assert_eq!(rig.payloads(topic), vec!["0", "1", "2"]);
    assert_eq!(
        t.times_of(&rig, topic),
        vec![first, first + 10000, first + 20019]
    );
}

#[test]
fn diag_valve_last_move_counters_inactive_and_unextended_valves() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        for v in 0..4 {
            c.valves[v].active = true;
        }
    });
    rig.link_up(); // protocol 2, no status
    rig.snap(|s| {
        for v in 0..5 {
            s.valves[v].known = true;
            s.valves[v].has_extended = v != 3;
        }
        let v0 = &mut s.valves[0];
        v0.move_seq = 1;
        v0.last_move.dir = MoveDir::Close;
        v0.last_move.requested_counts = 100;
        v0.last_move.counted_counts = 90;
        v0.last_move.stop = StopReason::EndStop;
        v0.last_move.peak_current = 55;
        v0.last_move.duration_ms = 1234;
        v0.early_stops = 2;
        v0.cmd_rejected = 3;
        v0.cal_state = 4;
    });
    rig.publish_snap();
    rig.settle(&mut c, 40);
    rig.run(&mut c, 10);
    let stop = stop_reason_name(StopReason::EndStop);
    assert_eq!(
        rig.payloads("VdMot/diag/valves/1/lastMove"),
        vec![format!(
            "{{\"dir\":\"close\",\"req\":100,\"cnt\":90,\"stop\":\"{stop}\",\"peak\":55,\"ms\":1234}}"
        )]
    );
    assert_eq!(rig.payloads("VdMot/diag/valves/1/earlyStops"), vec!["2"]);
    assert_eq!(rig.payloads("VdMot/diag/valves/1/cmdRejected"), vec!["3"]);
    assert_eq!(rig.payloads("VdMot/diag/valves/1/calState"), vec!["4"]);
    assert_eq!(rig.count("VdMot/diag/valves/2/lastMove"), 0); // move_seq 0
    assert_eq!(rig.payloads("VdMot/diag/valves/2/earlyStops"), vec!["0"]);
    assert_eq!(rig.payloads("VdMot/diag/valves/3/calState"), vec!["0"]);
    assert_eq!(rig.count_prefix("VdMot/diag/valves/4/", 0), 0); // no extended data
    assert_eq!(rig.count_prefix("VdMot/diag/valves/5/", 0), 0); // inactive
    assert_eq!(rig.count("VdMot/diag/stm/resets"), 0); // no status
    rig.snap(|s| {
        let v0 = &mut s.valves[0];
        v0.move_seq = 2;
        v0.last_move.dir = MoveDir::Open;
        v0.early_stops = 5;
        s.valves[1].cmd_rejected = 7;
        s.valves[1].cal_state = 1;
        s.valves[2].early_stops = 9;
    });
    rig.publish_snap();
    rig.run(&mut c, 10);
    let moves = rig.payloads("VdMot/diag/valves/1/lastMove");
    assert_eq!(moves.len(), 2);
    assert!(moves[1].starts_with("{\"dir\":\"open\","));
    assert_eq!(
        rig.payloads("VdMot/diag/valves/1/earlyStops"),
        vec!["2", "5"]
    );
    assert_eq!(rig.payloads("VdMot/diag/valves/1/cmdRejected"), vec!["3"]);
    assert_eq!(rig.payloads("VdMot/diag/valves/1/calState"), vec!["4"]);
    assert_eq!(
        rig.payloads("VdMot/diag/valves/2/cmdRejected"),
        vec!["0", "7"]
    );
    assert_eq!(rig.payloads("VdMot/diag/valves/2/calState"), vec!["0", "1"]);
    assert_eq!(rig.payloads("VdMot/diag/valves/2/earlyStops"), vec!["0"]);
    assert_eq!(
        rig.payloads("VdMot/diag/valves/3/earlyStops"),
        vec!["0", "9"]
    );
}

/// A gprof reply: the profile goes to the store (the app fake), the snapshot counts it.
fn set_profile(rig: &Rig, valve: u8, n: u8, seed: u32) {
    let mut h = rig.host.state();
    let p = &mut h.profiles[usize::from(valve)];
    *p = Profile::default();
    p.valve = valve;
    p.count = n;
    for i in 0..n {
        p.samples[usize::from(i)].count = seed + u32::from(i);
        p.samples[usize::from(i)].current = 100 + u16::from(i);
    }
    h.snap.profile_seq[usize::from(valve)] += 1;
}

#[test]
fn diag_four_valve_messages_per_pass_profiles_only_when_new() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        for v in 0..10 {
            c.valves[v].active = true;
        }
    });
    rig.snap(|s| {
        // protocol 0, link unknown
        for v in 0..10 {
            s.valves[v].known = true;
            s.valves[v].has_extended = true;
            s.valves[v].move_seq = v as u32 + 1;
        }
    });
    rig.publish_snap();
    let (mut phase_a, mut phase_b, mut phase_c) = (0, 0, 0);
    let mut t = Track::default();
    c.begin();
    rig.run_tracked(&mut c, 60, &mut t, &mut |passes, _| {
        if passes == 30 {
            // four new profiles, one of a single sample
            set_profile(&rig, 0, 3, 10);
            set_profile(&rig, 1, 3, 20);
            set_profile(&rig, 2, 3, 30);
            set_profile(&rig, 3, 1, 40);
            rig.publish_snap();
            phase_a = passes;
        } else if passes == 40 {
            // valves 1 and 3 move and have new profiles, valve 2 a profile
            rig.snap(|s| s.valves[0].move_seq += 1);
            set_profile(&rig, 0, 3, 11);
            set_profile(&rig, 1, 3, 21);
            rig.snap(|s| s.valves[2].move_seq += 1);
            set_profile(&rig, 2, 3, 31);
            rig.publish_snap();
            phase_b = passes;
        } else if passes == 50 {
            // a profile cleared
            set_profile(&rig, 0, 0, 0);
            rig.publish_snap();
            phase_c = passes;
        }
    });
    assert_eq!(rig.count("VdMot/diag/stm/proto"), 0);
    assert_eq!(rig.payloads("VdMot/diag/stm/link"), vec!["unknown"]);
    // connect pass 0; pass 1: protocol and link take two of four, then two valves per pass;
    // later passes four
    let lm = [1, 1, 2, 2, 2, 2, 3, 3, 3, 3];
    for (v, want) in lm.iter().enumerate() {
        let p = t.passes_of(&rig, &format!("VdMot/diag/valves/{}/lastMove", v + 1));
        assert!(!p.is_empty(), "{v}");
        assert_eq!(p[0], *want, "{v}");
    }
    assert!(phase_a > 0 && phase_b > 0 && phase_c > 0);
    assert_eq!(
        t.passes_of(&rig, "VdMot/diag/valves/1/profile"),
        vec![phase_a, phase_b]
    );
    assert_eq!(
        t.passes_of(&rig, "VdMot/diag/valves/2/profile"),
        vec![phase_a, phase_b]
    );
    assert_eq!(
        t.passes_of(&rig, "VdMot/diag/valves/3/profile"),
        vec![phase_a, phase_b + 1]
    );
    assert_eq!(
        t.passes_of(&rig, "VdMot/diag/valves/4/profile"),
        vec![phase_a]
    );
    assert_eq!(
        t.passes_of(&rig, "VdMot/diag/valves/1/lastMove"),
        vec![1, phase_b]
    );
    assert_eq!(
        t.passes_of(&rig, "VdMot/diag/valves/3/lastMove"),
        vec![2, phase_b]
    );
    let p4 = rig
        .broker
        .state()
        .published_to(b"VdMot/diag/valves/4/profile");
    assert_eq!(p4.len(), 1);
    assert_eq!(
        s(&p4[0].payload),
        "{\"valve\":4,\"count\":1,\"samples\":[[40,100]]}"
    );
    assert!(!p4[0].retain);
}

#[test]
fn diag_a_profile_is_copied_from_the_store_only_after_a_gprof_reply() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| s.valves[0].has_extended = true);
    rig.publish_snap();
    rig.settle(&mut c, 40);
    assert_eq!(rig.host.state().profile_reads, 0); // no reply yet: nothing to copy
    set_profile(&rig, 0, 2, 7);
    rig.publish_snap();
    rig.run(&mut c, 5);
    assert_eq!(rig.host.state().profile_reads, 1);
    assert_eq!(
        rig.payloads("VdMot/diag/valves/1/profile"),
        vec!["{\"valve\":1,\"count\":2,\"samples\":[[7,100],[8,101]]}"]
    );
    // the same profile again (another reply): copied, compared, not published
    rig.snap(|s| s.profile_seq[0] += 1);
    rig.publish_snap();
    rig.run(&mut c, 5);
    assert_eq!(rig.host.state().profile_reads, 2);
    assert_eq!(rig.count("VdMot/diag/valves/1/profile"), 1);
    // other snapshots copy nothing
    rig.snap(|s| s.valves[0].early_stops += 1);
    rig.publish_snap();
    rig.run(&mut c, 5);
    assert_eq!(rig.host.state().profile_reads, 2);
}

#[test]
fn diag_without_memory_for_the_profile_copy_the_next_pass_looks_again() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.snap(|s| s.valves[0].has_extended = true);
    rig.publish_snap();
    rig.settle(&mut c, 40);
    set_profile(&rig, 0, 1, 5);
    rig.publish_snap();
    rig.dev.heap.state().fail_all = true;
    rig.run(&mut c, 5);
    assert_eq!(rig.host.state().profile_reads, 0);
    assert_eq!(rig.count("VdMot/diag/valves/1/profile"), 0);
    rig.dev.heap.state().fail_all = false;
    rig.run(&mut c, 1);
    assert_eq!(rig.host.state().profile_reads, 1);
    assert_eq!(
        rig.payloads("VdMot/diag/valves/1/profile"),
        vec!["{\"valve\":1,\"count\":1,\"samples\":[[5,100]]}"]
    );
}

#[test]
fn diag_the_valve_loop_ends_after_valve_12() {
    // C++ "whatever follows the valves": the bytes after the 12 valve configs and states read
    // like an active, extended valve; a Rust valve index is bounds-checked, so only the last
    // valve is left to check
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        c.valves[11].active = true;
        copy_string(&mut c.temps[0].name, b"ABCDEFGHI\x01");
        c.temps[0].active = true;
    });
    rig.link_up();
    rig.snap(|s| {
        s.valves[11].known = true;
        s.valves[11].has_extended = true;
    });
    rig.publish_snap();
    rig.settle(&mut c, 40);
    assert_eq!(rig.payloads("VdMot/diag/valves/12/earlyStops"), vec!["0"]);
}

// ---------------------------------------------------------------- events

#[test]
fn events_from_the_first_logged_event_four_per_pass() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.log(EventCode::LowHeap, NO_VALVE, 1, 2); // seq 1, before the session
    let mut t = Track::default();
    c.begin();
    rig.run_tracked(&mut c, 20, &mut t, &mut |passes, _| {
        if passes != 5 {
            return;
        }
        rig.log(EventCode::CalibTimeMissing, NO_VALVE, 1, 0);
        rig.log(EventCode::HeapFragmented, NO_VALVE, 1, 2);
        rig.log(EventCode::ConfigRepaired, NO_VALVE, 0, 3);
        rig.log(EventCode::FactoryResetSkipped, NO_VALVE, 0, 0);
        rig.log(EventCode::ValveStale, 12, 60, 0); // not one of the 12 valves: not joined
    });
    assert_eq!(rig.payloads(MESSAGE)[0], "low heap (free 1, min 2)");
    assert_eq!(t.passes_of(&rig, "VdMot/events"), vec![0, 5, 5, 5, 5, 6]);
    assert!(rig
        .last("VdMot/events")
        .contains("\"name\":\"valve_stale\""));
}

#[test]
fn events_only_warning_and_above_set_common_message() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 40);
    rig.log(EventCode::CalibOk, 0, 3000, 3100); // Info
    rig.run(&mut c, 300);
    assert_eq!(rig.last(MESSAGE), "");
    rig.log(EventCode::ValveStale, 0, 60, 0); // Warning
    rig.run(&mut c, 300);
    assert_eq!(rig.last(MESSAGE), "valve 1: no data for 60 s");
}

#[test]
fn events_nothing_on_main_events_without_the_events_option() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| c.mqtt.events = false);
    rig.settle(&mut c, 40);
    rig.log(EventCode::ValveStale, 0, 60, 0);
    rig.log(EventCode::LinkDown, NO_VALVE, 3, 0);
    rig.run(&mut c, 200);
    assert_eq!(rig.count("VdMot/events"), 0);
}

#[test]
fn events_that_never_reach_mqtt_are_not_joined_or_counted() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    let mut t = Track::default();
    c.begin();
    rig.run_tracked(&mut c, 200, &mut t, &mut |passes, _| {
        if passes != 5 {
            return;
        }
        rig.log(EventCode::ValveStateChanged, 0, 1, 2); // Debug
        rig.log(EventCode::ValveStateChanged, 0, 1, 2);
    });
    assert_eq!(rig.shared.status().events_suppressed, 0);
    assert_eq!(rig.payloads("VdMot/diag/mqtt/eventsSuppressed"), vec!["0"]);
}

#[test]
fn events_a_duplicate_valve_event_counts_as_suppressed() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    let mut t = Track::default();
    c.begin();
    rig.run_tracked(&mut c, 1 + 600, &mut t, &mut |passes, _| {
        if passes != 5 {
            return;
        }
        rig.log(EventCode::ValveStale, 0, 60, 0);
        rig.log(EventCode::ValveStale, 0, 60, 0);
    });
    assert_eq!(rig.shared.status().events_suppressed, 1);
    assert_eq!(
        rig.payloads("VdMot/diag/mqtt/eventsSuppressed"),
        vec!["0", "1"]
    );
}

#[test]
fn events_a_fifth_valve_code_flushes_the_oldest_joined_event_at_once() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    let mut t = Track::default();
    c.begin();
    rig.run_tracked(&mut c, 200, &mut t, &mut |passes, _| {
        if passes != 5 {
            return;
        }
        rig.log(EventCode::ValveBlocked, 0, 3, 0);
        rig.log(EventCode::ValveNoValve, 0, 0, 0);
        rig.log(EventCode::CalibRetry, 0, 1, 0);
        rig.log(EventCode::EarlyStop, 0, 4, 0);
        rig.log(EventCode::CmdRejected, 0, 5, 0);
    });
    let ev = rig.payloads("VdMot/events");
    let at = t.passes_of(&rig, "VdMot/events");
    assert_eq!(ev.len(), 5);
    assert_eq!(at[0], 6);
    assert!(ev[0].contains("\"name\":\"valve_blocked\""), "{}", ev[0]);
    assert!(at[1] > 6);
}

/// The `<main>events` JSON of `e` (no buffer limit).
fn json_of(e: &Event) -> String {
    let mut buf = vec![0u8; 2048];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write_mqtt_event_json(&mut jw, e, 0));
    s(jw.as_bytes())
}

/// An event whose `<main>events` JSON has `want` characters.
fn event_of_json_len(valve: u8, seq: u32, want: usize) -> Option<Event> {
    for up in [
        0u32,
        10,
        100,
        1000,
        10000,
        100_000,
        1_000_000,
        10_000_000,
        100_000_000,
        1_000_000_000,
    ] {
        for k in 0..=EVENT_TEXT_MAX {
            for m in 0..=EVENT_TEXT_MAX - k {
                let text = format!("{}{}", "\u{1}".repeat(k), "a".repeat(m));
                let mut e = make_event(
                    EventCode::StmEepromWaitTimeout,
                    Severity::Warning,
                    valve,
                    i32::MIN,
                    1,
                    text.as_bytes(),
                );
                e.seq = seq;
                e.uptime_s = up;
                e.epoch = u32::MAX;
                if json_of(&e).len() == want {
                    return Some(e);
                }
            }
        }
    }
    None
}

#[test]
fn events_an_event_json_of_511_characters_goes_out_512_do_not_fit() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    let mut fits_json = String::new();
    let mut t = Track::default();
    c.begin();
    rig.run_tracked(&mut c, 20, &mut t, &mut |passes, _| {
        if passes != 5 {
            return;
        }
        let seq = rig.host.last_seq() + 1;
        let fits = event_of_json_len(NO_VALVE, seq, 511).expect("511");
        let too_long = event_of_json_len(ALL_VALVES, seq + 1, 512).expect("512");
        fits_json = json_of(&fits);
        assert_eq!(rig.host.log_event(&fits), seq);
        assert_eq!(rig.host.log_event(&too_long), seq + 1);
    });
    assert_eq!(rig.payloads("VdMot/events"), vec![fits_json]);
}

// ---------------------------------------------------------------- inbound

#[test]
fn inbound_payloads_are_cut_at_33_bytes() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.deliver("VdMot/valves/1/target/set", &format!("{}x", " ".repeat(32))); // not blank
    rig.run(&mut c, 2);
    assert_eq!(rig.shared.status().commands_rejected, 1);
    rig.deliver("VdMot/valves/1/target/set", &format!("{}x", " ".repeat(33))); // blank when cut
    rig.run(&mut c, 2);
    assert_eq!(rig.shared.status().commands_rejected, 1);
    assert!(rig.submitted().is_empty());
}

#[test]
fn inbound_topics_up_to_127_characters_are_read_longer_ones_dropped() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    let base = "VdMot/cmd/";
    rig.deliver(&format!("{base}{}", "x".repeat(127 - base.len())), "PRESS");
    rig.run(&mut c, 2);
    let ev = rig.rejected();
    assert_eq!(ev.len(), 1);
    assert_eq!(Rig::text(&ev[0]), "unknown command");
    assert_eq!(rig.shared.status().commands_rejected, 1);
    rig.deliver(&format!("{base}{}", "x".repeat(128 - base.len())), "PRESS");
    rig.run(&mut c, 2);
    assert_eq!(rig.shared.status().commands_rejected, 1);
}

#[test]
fn inbound_an_overflow_is_rejected_without_a_detail() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        for v in 0..6 {
            c.valves[v].active = true;
        }
    });
    rig.settle(&mut c, 2);
    // the C++ fake's burst: the queue fed directly, as the poll callback does
    for v in 1..=5 {
        c.inbound
            .push(format!("VdMot/valves/{v}/target/set").as_bytes(), b"30");
    }
    rig.run(&mut c, 1);
    let ev = rig.rejected();
    assert_eq!(ev.len(), 1);
    assert_eq!(Rig::text(&ev[0]), "queue full");
    assert_eq!(ev[0].arg2, 0);
}

#[test]
fn inbound_an_unconfirmed_button_is_rejected_without_a_detail() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.deliver("VdMot/cmd/restart", "PRESS");
    rig.run(&mut c, 300);
    let ev = rig.rejected();
    assert_eq!(ev.len(), 1);
    assert_eq!(Rig::text(&ev[0]), "clear not confirmed");
    assert_eq!(ev[0].arg2, 0);
}

#[test]
fn inbound_a_button_whose_clear_failed_is_rejected_without_a_detail() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.deliver("VdMot/cmd/restart", "PRESS");
    rig.script.get().fail_next_publish = true;
    rig.run(&mut c, 1);
    let ev = rig.rejected();
    assert_eq!(ev.len(), 1);
    assert_eq!(Rig::text(&ev[0]), "clear not confirmed");
    assert_eq!(ev[0].arg2, 0);
}

#[test]
fn inbound_a_confirmed_button_the_queue_refuses_is_rejected_without_a_detail() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.host.state().submit_result = false;
    rig.deliver("VdMot/cmd/detect", "PRESS");
    rig.deliver("VdMot/cmd/detect", "");
    rig.run(&mut c, 2);
    let ev = rig.rejected();
    assert_eq!(ev.len(), 1);
    assert_eq!(Rig::text(&ev[0]), "queue full");
    assert_eq!(ev[0].arg2, 0);
}

#[test]
fn inbound_a_fifth_held_button_is_rejected_without_a_detail() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    for t in [
        "restart",
        "stmReset",
        "detect",
        "calibrate",
        "valves/1/calibrate",
    ] {
        rig.deliver(&format!("VdMot/cmd/{t}"), "PRESS");
    }
    rig.run(&mut c, 5);
    let ev = rig.rejected();
    assert_eq!(ev.len(), 1);
    assert_eq!(Rig::text(&ev[0]), "queue full");
    assert_eq!(ev[0].arg1, 1);
    assert_eq!(ev[0].arg2, 0);
}

#[test]
fn inbound_a_message_of_the_last_session_is_not_handled_again_after_a_reconnect() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.deliver("VdMot/valves/1/target/set", "40");
    rig.run(&mut c, 2);
    assert_eq!(rig.submitted().len(), 1);
    rig.shared.request_reconnect();
    rig.run(&mut c, 5);
    assert_eq!(rig.connects(), 2);
    assert_eq!(rig.submitted().len(), 1);
}

#[test]
fn inbound_refused_targets_are_submitted_again_valve_after_valve_from_valve_1() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        c.valves[1].active = true;
        c.valves[2].active = true;
    });
    rig.settle(&mut c, 2);
    rig.host.state().submit_result = false;
    // the C++ fake's burst: the queue fed directly, as the poll callback does
    c.inbound.push(b"VdMot/valves/1/target/set", b"10");
    c.inbound.push(b"VdMot/valves/2/target/set", b"20");
    c.inbound.push(b"VdMot/valves/3/target/set", b"30");
    rig.run(&mut c, 1); // all latched; each drain of the pass submits the next one (valves 1, 2)
    assert!(rig.submitted().is_empty());
    rig.host.state().submit_result = true;
    rig.run(&mut c, 2);
    let sub = rig.submitted();
    assert_eq!(sub.len(), 3);
    assert_eq!(sub[0].valve, 2);
    assert_eq!(sub[1].valve, 0);
    assert_eq!(sub[2].valve, 1);
}

// ---------------------------------------------------------------- connection

#[test]
fn connection_a_refused_connect_is_retried_after_the_back_off_only() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.refuse_connects(true);
    c.begin();
    rig.run(&mut c, 10);
    assert_eq!(rig.connects(), 1);
    rig.run(&mut c, 15);
    assert_eq!(rig.connects(), 2);
}

#[test]
fn connection_without_a_host_no_connect_an_error_with_rc_0_then_the_back_off() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.broker.attach(&rig.dev.tcp, "b", PORT);
    rig.cfg(|c| c.mqtt.host.clear());
    c.begin();
    rig.run(&mut c, 3);
    assert_eq!(rig.connects(), 0);
    assert_eq!(rig.shared.status().state, MqttState::Error);
    assert_eq!(rig.shared.status().rc, 0);
    rig.cfg(|c| {
        copy_string(&mut c.mqtt.host, b"b"); // one character
    });
    rig.new_revision();
    rig.run(&mut c, 3);
    assert_eq!(rig.connects(), 0); // the failed attempt waits 2 s
    rig.run(&mut c, 20);
    assert_eq!(rig.connects(), 1);
    assert_eq!(rig.dev.tcp.connects()[0].host, "b");
    assert_eq!(rig.shared.status().state, MqttState::Connected);
    assert_eq!(rig.shared.status().rc, 0);
}

#[test]
fn connection_a_host_and_a_client_id_of_64_characters() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    let host = "h".repeat(64);
    let id = "i".repeat(64);
    rig.broker.attach(&rig.dev.tcp, &host, PORT);
    rig.cfg(|c| {
        copy_string(&mut c.mqtt.host, host.as_bytes());
        copy_string(&mut c.mqtt.client_id, id.as_bytes());
    });
    c.begin();
    rig.run(&mut c, 1);
    assert_eq!(rig.dev.tcp.connects()[0].host, host);
    assert_eq!(s(&rig.broker.state().connects[0].client_id), id);
}

#[test]
fn connection_a_client_id_of_one_character() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        copy_string(&mut c.mqtt.client_id, b"x");
    });
    c.begin();
    rig.run(&mut c, 1);
    assert_eq!(s(&rig.broker.state().connects[0].client_id), "x");
}

#[test]
fn connection_user_and_password_of_one_character_each() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        copy_string(&mut c.mqtt.user, b"u");
        copy_string(&mut c.mqtt.password, b"p");
    });
    c.begin();
    rig.run(&mut c, 1);
    let b = rig.broker.state();
    assert_eq!(b.connects[0].user.as_deref(), Some(&b"u"[..]));
    assert_eq!(b.connects[0].password.as_deref(), Some(&b"p"[..]));
}

#[test]
fn connection_a_restart_after_a_refused_connect_disables_with_rc_0() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.refuse_with(MQTT_CONNECT_UNAUTHORIZED as u8, 1);
    c.begin();
    rig.run(&mut c, 1);
    assert_eq!(rig.shared.status().state, MqttState::Error);
    rig.host.state().restart_pending = true;
    rig.run(&mut c, 2);
    assert_eq!(rig.shared.status().state, MqttState::Disabled);
    assert_eq!(rig.shared.status().rc, 0);
    assert_eq!(rig.delays.borrow().last(), Some(&100));
}

#[test]
fn connection_a_restart_of_a_connected_session_disables_with_rc_0() {
    // C++ also set the fake library's state to 5 to show that rc does not come from it; here the
    // state after the disconnect is -1
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.host.state().restart_pending = true;
    rig.run(&mut c, 2);
    assert_eq!(rig.shared.status().state, MqttState::Disabled);
    assert_eq!(rig.shared.status().rc, 0);
}

#[test]
fn connection_a_network_down_never_connects_mqtt_off_reads_rc_0() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.host.state().net_up = false;
    c.begin();
    rig.run(&mut c, 3);
    assert_eq!(rig.connects(), 0);
    assert_eq!(rig.shared.status().state, MqttState::Connecting);
    assert_eq!(rig.shared.status().rc, 0);
    rig.cfg(|c| c.mqtt.mode = MqttMode::Off);
    rig.new_revision();
    rig.run(&mut c, 2);
    assert_eq!(rig.shared.status().state, MqttState::Disabled);
    assert_eq!(rig.shared.status().rc, 0);
}

#[test]
fn connection_after_60_s_of_a_stable_session_a_drop_reconnects_at_once() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.run(&mut c, 3050); // > 60 s
    rig.broker.drop_connection();
    rig.run(&mut c, 1);
    assert_eq!(rig.connects(), 2);
}

// ---------------------------------------------------------------- discovery

#[test]
fn discovery_none_in_mode_mqtt_after_the_first_run_cleanup() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 300);
    assert_eq!(rig.count_prefix("homeassistant/", 0), 0);
    assert!(rig.events(EventCode::HaDiscoverySent).is_empty());
}

#[test]
fn discovery_the_first_run_cleanup_in_mode_mqtt_deletes_only() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.host.state().ha_cleanup_done = false;
    rig.settle(&mut c, 400); // the cleanup does not wait for the STM inputs
    assert_eq!(rig.host.state().ha_cleanup_marks, 1);
    assert!(rig.count_prefix("homeassistant/", 0) > 0);
    for p in rig.published() {
        if p.topic.starts_with(b"homeassistant/") {
            assert!(p.payload.is_empty(), "{}", s(&p.topic));
        }
    }
    assert!(rig.host.state().ha_layout_sets.is_empty());
}

#[test]
fn discovery_a_manual_run_prunes_stale_entries_retained_configs_no_2_0_cleanup() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    let stale = "homeassistant/text/VdMot/valves_state_Old/config";
    rig.dev.fs.put(LIST_FILE, format!("{stale}\n").as_bytes());
    rig.settle(&mut c, 2);
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run(&mut c, 500);
    assert_eq!(rig.payloads(stale), vec![""]);
    assert_eq!(
        rig.count("homeassistant/sensor/VdMot/diag_stm_uptime/config"),
        0
    );
    let st = rig
        .broker
        .state()
        .published_to(b"homeassistant/text/VdMot/state/config");
    assert_eq!(st.len(), 1);
    assert!(st[0].retain);
    assert!(!s(&rig.dev.fs.read(LIST_FILE).unwrap()).contains(stale));
}

#[test]
fn discovery_layout_1_runs_the_2_0_cleanup_on_connect_in_ha_mode() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::MqttHa);
    rig.cfg(|c| c.mqtt.ha_discovery_on_connect = false);
    rig.host.state().ha_layout = 1;
    rig.snap(|s| s.sensors_settled = true); // automatic runs start at once
    rig.settle(&mut c, 500);
    assert_eq!(
        rig.payloads("homeassistant/sensor/VdMot/diag_stm_uptime/config"),
        vec![""]
    );
    assert_eq!(rig.host.state().ha_layout_sets, vec![2]);
}

#[test]
fn discovery_changed_inputs_run_it_again_after_the_current_run() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::MqttHa);
    rig.link_up();
    rig.snap(|s| s.sensors_settled = true); // automatic runs start at once
    let mut t = Track::default();
    c.begin();
    rig.run_tracked(&mut c, 1000, &mut t, &mut |passes, _| {
        if passes == 10 {
            // during the first run
            rig.snap(|s| {
                copy_string(&mut s.version.hw, b"C2");
            });
            rig.publish_snap();
        }
    });
    let runs = || rig.events(EventCode::HaDiscoverySent).len();
    assert_eq!(runs(), 2);
    assert!(!rig.shared.status().discovery_running);
    rig.publish_snap(); // a new snapshot with the same inputs
    rig.run(&mut c, 500);
    assert_eq!(runs(), 2);
    rig.snap(|s| {
        copy_string(&mut s.version.hw, b"C3");
    });
    rig.publish_snap();
    rig.run(&mut c, 500);
    assert_eq!(runs(), 3);
}

#[test]
fn discovery_a_dropped_session_ends_the_run() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.settle(&mut c, 2);
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run(&mut c, 10);
    assert!(rig.shared.status().discovery_running);
    rig.broker.drop_connection();
    rig.run(&mut c, 1);
    assert!(!rig.shared.status().discovery_running);
    let before = rig.published_len();
    rig.run(&mut c, 600);
    assert_eq!(rig.connects(), 2);
    assert_eq!(rig.count_prefix("homeassistant/", before), 0);
    assert!(rig.events(EventCode::HaDiscoverySent).is_empty());
}

#[test]
fn discovery_without_a_ready_file_system_the_list_is_neither_read_nor_written() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    let stale = "homeassistant/text/VdMot/valves_state_Old/config";
    rig.dev.fs.put(LIST_FILE, format!("{stale}\n").as_bytes());
    rig.host.state().fs_ready = false;
    rig.settle(&mut c, 2);
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run(&mut c, 500);
    assert_eq!(rig.events(EventCode::HaDiscoverySent).len(), 1);
    assert_eq!(rig.count(stale), 0);
    assert_eq!(
        rig.dev.fs.read(LIST_FILE).unwrap(),
        format!("{stale}\n").as_bytes()
    );
    assert!(!rig.dev.fs.exists(LIST_TMP));
}

#[test]
fn discovery_a_list_that_cannot_be_committed_leaves_no_temporary_file() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.dev.fs.fail("rename", "", 1);
    rig.settle(&mut c, 2);
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run(&mut c, 500);
    assert_eq!(rig.events(EventCode::HaDiscoverySent).len(), 1);
    assert!(!rig.dev.fs.exists(LIST_FILE));
    assert!(!rig.dev.fs.exists(LIST_TMP));
}

#[test]
fn discovery_a_run_replacing_an_unfinished_one_drops_its_half_written_list() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        c.temps[0].active = true;
        c.temps[0].id = one_wire(0x28, 1);
    });
    rig.link_up();
    rig.settle(&mut c, 2);
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run(&mut c, 500);
    let before = rig.dev.fs.read(LIST_FILE).unwrap();
    assert!(!before.is_empty());
    // the sensor shows up on the bus: a new entity, the list is rewritten ...
    rig.snap(|s| {
        s.temp_count = 5;
        s.temps[4].id = one_wire(0x28, 1);
        s.temps[4].seen = true;
        s.temps[4].raw = 200;
    });
    rig.publish_snap();
    let mut replaced = false;
    let mut t = Track::default();
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run_tracked(&mut c, 500, &mut t, &mut |_, _| {
        if replaced || !rig.dev.fs.exists(LIST_TMP) {
            return;
        }
        // ... and while it is written the sensor is gone again: a run for the old entities
        rig.snap(|s| s.temp_count = 0);
        rig.publish_snap();
        rig.shared.request_discovery(DiscoveryAction::Publish);
        replaced = true;
    });
    assert!(replaced);
    assert_eq!(rig.events(EventCode::HaDiscoverySent).len(), 2);
    assert_eq!(rig.dev.fs.read(LIST_FILE).unwrap(), before);
    assert!(!rig.dev.fs.exists(LIST_TMP));
}

// ---------------------------------------------------------------- Rust only

#[test]
fn discovery_the_list_is_read_through_the_512_b_buffer_of_the_run() {
    // C++ W4-8 checked the 512 B stdio buffers of the list files; the Rust file port is
    // unbuffered, the run reads the list through its own 512 B buffer: a list of 1300 bytes
    // comes in reads of 512, 512 and 276 bytes
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    let line = "homeassistant/text/VdMot/valves_state_x/config\n";
    let mut list = String::new();
    while list.len() + line.len() <= 1300 {
        list.push_str(line);
    }
    while list.len() < 1300 {
        list.insert(0, '\n');
    }
    rig.dev.fs.put(LIST_FILE, list.as_bytes());
    rig.settle(&mut c, 2);
    c.disc = c.alloc_run();
    let mut got = Vec::new();
    let mut chunks = Vec::new();
    c.with_port(|_, _, port, _| {
        assert!(port.list_open());
        while let Some(b) = port.list_read() {
            if port.files.pos == 1 {
                chunks.push(port.files.len);
            }
            got.push(b);
        }
        port.list_close();
    });
    assert_eq!(got, list.as_bytes());
    assert_eq!(chunks, vec![512, 512, 276]);
    c.disc = None;
}

#[test]
fn the_contract_constants() {
    assert_eq!(BUFFER_SIZE, 2304);
    assert_eq!(SOCKET_TIMEOUT_S, 5);
    assert_eq!((BACKOFF_MIN_MS, BACKOFF_MAX_MS), (2000, 60000));
    assert_eq!((DISCOVERY_PACE_MS, WAIT_MS, IDLE_MS), (20, 100, 500));
    assert_eq!(
        (LIST_FILE, LIST_TMP),
        ("/HADiscovery.cfg", "/HADiscovery.cfg.tmp")
    );
    assert_eq!(HA_STATUS_RECORD_LEN, core::mem::size_of::<HaStatusRecord>());
    // the slots of DESIGN.md "MQTT": 0 common, 1..12 valves, 13..46 temps, 47..54 volts, 55
    // STM, 56 system
    assert_eq!(
        [
            SLOT_COMMON,
            SLOT_VALVE0,
            SLOT_TEMP0,
            SLOT_VOLT0,
            SLOT_STM,
            SLOT_SYSTEM,
            SLOT_COUNT
        ],
        [0, 1, 13, 47, 55, 56, 57]
    );
    const { assert!(SLOT_COUNT <= PublishScheduler::SLOTS) };
    // the largest message fits the packet buffer with its 5 + 2 header bytes
    const { assert!(5 + 2 + TOPIC_MAX + DISCOVERY_PAYLOAD_MAX <= BUFFER_SIZE as usize) };
    assert_eq!(
        (TOPIC_BUF, SEGMENT_BUF, CLIENT_ID_BUF, MESSAGE_BUF),
        (128, 11, 24, 120)
    );
    assert_eq!((PAYLOAD_BUF, LIST_BUFFER, RUN_BYTES), (2048, 512, 2560));
    assert_eq!(
        (INBOUND_SLOTS, INBOUND_PAYLOAD_MAX, PROFILE_JSON_MAX),
        (4, 33, 643)
    );
    assert_eq!(
        (
            FULL_SLOTS_PER_PASS,
            MAX_SLOTS_PER_PASS,
            MAX_DIAG_PER_PASS,
            EVENTS_PER_PASS
        ),
        (4, 2, 4, 4)
    );
    assert_eq!(
        (SENSOR_STALE_MS, COUNTER_PACE_MS, STARTED_TOLERANCE_S),
        (60000, 10000, 60)
    );
}

#[test]
fn values_common_ip_and_the_configuration_url_are_the_address_of_the_network() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.host.state().ip = u32::from_le_bytes([192, 168, 1, 51]);
    rig.settle(&mut c, 40);
    assert_eq!(rig.payloads(IP), vec!["192.168.1.51"]);
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run(&mut c, 500);
    let config = rig.last("homeassistant/text/VdMot/state/config");
    assert!(
        config.contains("\"configuration_url\":\"http://192.168.1.51/\""),
        "{config}"
    );
}

#[test]
fn values_requested_only_with_a_desired_target() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.link_up();
    rig.settle(&mut c, 40);
    assert_eq!(rig.count("VdMot/valves/1/requested/value"), 0);
    assert_eq!(rig.count("VdMot/valves/1/state/value"), 1);
    rig.snap(|s| {
        s.valves[0].desired_valid = true;
        s.valves[0].desired = 55;
    });
    rig.publish_snap();
    rig.run(&mut c, 300);
    assert_eq!(rig.payloads("VdMot/valves/1/requested/value"), vec!["55"]);
}

#[test]
fn values_a_valve_name_of_10_characters_is_its_segment() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        copy_string(&mut c.valves[0].name, b"ABCDEFGHIJ");
    });
    rig.link_up();
    rig.settle(&mut c, 40);
    assert_eq!(rig.count("VdMot/valves/ABCDEFGHIJ/state/value"), 1);
    rig.deliver("VdMot/valves/ABCDEFGHIJ/target/set", "33");
    rig.run(&mut c, 2);
    let sub = rig.submitted();
    assert_eq!(sub.len(), 1);
    assert_eq!((sub[0].valve, sub[0].pos), (0, 33));
}

#[test]
fn on_change_a_volt_change_of_10_mv_goes_out() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        c.volts[0].id = one_wire(0x26, 2);
        copy_string(&mut c.volts[0].unit, b"V");
    });
    rig.link_up();
    rig.snap(|s| {
        s.volt_count = 1;
        s.volts[0].id = one_wire(0x26, 2);
        s.volts[0].vad = 1234;
        s.volts[0].seen = true;
    });
    rig.publish_snap();
    rig.host.state().uptime_s = Some(1000);
    rig.settle(&mut c, 40);
    rig.snap(|s| s.volts[0].vad = 1235);
    rig.publish_snap();
    rig.run(&mut c, 300);
    assert_eq!(
        rig.payloads("VdMot/sensors/1/value/value"),
        vec!["12.340", "12.350"]
    );
}

#[test]
fn on_change_off_publishes_every_publish_interval_only() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        c.mqtt.on_change = false;
        c.mqtt.publish_interval_s = 30;
        c.mqtt.min_delay_s = 20;
    });
    rig.link_up();
    rig.host.state().uptime_s = Some(1000);
    let t0 = rig.now();
    let mut t = Track::default();
    c.begin();
    rig.run_tracked(&mut c, 1 + 2000, &mut t, &mut |_, now| {
        if now == t0 + 2000 {
            rig.snap(|s| s.valves[0].position = 40);
            rig.publish_snap();
        }
    }); // 40 s
    assert_eq!(
        t.times_of(&rig, "VdMot/stm/status"),
        vec![t0 + 540, t0 + 30540]
    );
    assert_eq!(
        t.times_of(&rig, "VdMot/valves/1/actual/value"),
        vec![t0 + 100, t0 + 30100]
    );
    assert_eq!(rig.payloads("VdMot/valves/1/actual/value"), vec!["0", "40"]);
}

#[test]
fn on_change_waits_min_delay_after_the_last_publish_of_the_slot() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        c.mqtt.publish_interval_s = 30;
        c.mqtt.min_delay_s = 20;
    });
    rig.link_up();
    rig.host.state().uptime_s = Some(1000);
    let t0 = rig.now();
    let mut t = Track::default();
    c.begin();
    rig.run_tracked(&mut c, 1 + 1500, &mut t, &mut |_, now| {
        if now == t0 + 2000 {
            rig.snap(|s| s.valves[0].position = 40);
            rig.publish_snap();
        }
    }); // 30 s
        // the full publish ends at t0 + 540; the change goes out 20 s after it
    assert_eq!(
        t.times_of(&rig, "VdMot/valves/1/actual/value"),
        vec![t0 + 100, t0 + 20540]
    );
}

#[test]
fn connection_without_a_last_will_topic_no_connect_an_error_with_rc_0() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        copy_string(&mut c.mqtt.root_topic, b"Vd+Mot"); // no safe name: no topic
    });
    c.begin();
    rig.run(&mut c, 3);
    assert_eq!(rig.connects(), 0);
    assert_eq!(rig.shared.status().state, MqttState::Error);
    assert_eq!(rig.shared.status().rc, 0);
}

#[test]
fn full_publish_without_up_time_the_uptime_is_no_change_of_common() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| c.mqtt.up_time = false);
    rig.settle(&mut c, 400); // 8 s: past min_delay_s after the full publish
    assert_eq!(rig.count(STATE), 1);
    assert_eq!(rig.count(UPTIME), 0);
}

#[test]
fn discovery_a_run_logs_its_configs_and_deletes_without_a_text() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    let stale = "homeassistant/text/VdMot/valves_state_Old/config";
    rig.dev.fs.put(LIST_FILE, format!("{stale}\n").as_bytes());
    rig.settle(&mut c, 2);
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run(&mut c, 500);
    let sent = rig.events(EventCode::HaDiscoverySent);
    assert_eq!(sent.len(), 1);
    let configs = rig
        .published()
        .iter()
        .filter(|p| p.topic.starts_with(b"homeassistant/") && !p.payload.is_empty())
        .count();
    assert_eq!(sent[0].arg1, configs as i32);
    assert_eq!(sent[0].arg2, 1); // the stale entry
    assert_eq!(Rig::text(&sent[0]), "");
}

#[test]
fn discovery_without_a_ready_file_system_a_left_over_temporary_list_stays() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.dev.fs.put(LIST_TMP, b"x\n");
    rig.host.state().fs_ready = false;
    rig.settle(&mut c, 2);
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run(&mut c, 500);
    assert_eq!(rig.events(EventCode::HaDiscoverySent).len(), 1);
    assert_eq!(rig.dev.fs.read(LIST_TMP).unwrap(), b"x\n");
}

#[test]
fn discovery_a_run_that_skipped_entities_says_so() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::Mqtt);
    rig.cfg(|c| {
        // an override no topic can carry: the entities of the valve cannot be built
        copy_string(&mut c.valves[0].topic, b"a//b");
    });
    rig.settle(&mut c, 2);
    rig.shared.request_discovery(DiscoveryAction::Publish);
    rig.run(&mut c, 500);
    let sent = rig.events(EventCode::HaDiscoverySent);
    assert_eq!(sent.len(), 1);
    assert_eq!(Rig::text(&sent[0]), "skipped entities");
}

#[test]
fn discovery_changed_inputs_start_no_run_with_ha_discovery_on_connect_off() {
    // the 2.0 cleanup is still due, so an automatic run would have a plan: only the switch
    // keeps the changed inputs from asking for one
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::MqttHa);
    rig.cfg(|c| c.mqtt.ha_discovery_on_connect = false);
    rig.host.state().ha_layout = 1;
    rig.link_up();
    c.begin();
    rig.run(&mut c, 40); // the connect asks for the cleanup, the STM inputs are not settled
    assert!(rig.session_up());
    assert!(!rig.shared.status().discovery_running);
    rig.shared.request_discovery(DiscoveryAction::Delete); // answers the waiting run
    rig.run(&mut c, 1000);
    let runs = || rig.events(EventCode::HaDiscoverySent).len();
    assert_eq!(runs(), 1);
    assert!(rig.host.state().ha_layout_sets.is_empty());
    let before = rig.published_len();
    rig.snap(|s| {
        s.sensors_settled = true;
        copy_string(&mut s.version.hw, b"C2");
    });
    rig.publish_snap();
    rig.run(&mut c, 1000);
    assert_eq!(runs(), 1);
    assert!(!rig.shared.status().discovery_running);
    assert_eq!(rig.count_prefix("homeassistant/", before), 0);
}

#[test]
fn ha_status_an_accepted_command_outside_ha_mode_keeps_a_restored_offline() {
    let rig = Rig::new();
    let mut c = rig.client();
    rig.use_mqtt(MqttMode::MqttHa);
    rig.cfg(|c| c.mqtt.ha_discovery_on_connect = false);
    rig.settle(&mut c, 2);
    rig.deliver("homeassistant/status", "offline");
    rig.run(&mut c, 1);
    assert_eq!(rig.shared.status().ha_status, HaStatus::Offline);
    // MQTT without HA: a command does not bring HA back
    rig.cfg(|c| c.mqtt.mode = MqttMode::Mqtt);
    rig.new_revision();
    rig.run(&mut c, 2);
    rig.deliver("VdMot/valves/1/target/set", "10");
    rig.run(&mut c, 2);
    assert_eq!(rig.submitted().len(), 1);
    assert_eq!(rig.shared.status().ha_status, HaStatus::Offline);
    assert_eq!(rig.shared.regulator_state().ha, HaStatus::Offline);
}

#[test]
fn shared_is_the_object_main_created() {
    let rig = Rig::new();
    let c = rig.client();
    assert!(core::ptr::eq(c.shared(), &rig.shared));
}

#[test]
fn begin_after_a_restart_reads_the_ha_status_of_rtc_memory_and_the_config() {
    // C++ K1-4 called begin() again on the same module; a Rust restart is a new client
    let rig = Rig::new();
    rig.use_mqtt(MqttMode::MqttHa);
    rig.cfg(|c| c.mqtt.ha_discovery_on_connect = false);
    {
        let mut c = rig.client();
        rig.settle(&mut c, 2);
        rig.deliver("homeassistant/status", "offline");
        rig.run(&mut c, 1);
        assert_eq!(rig.shared.status().ha_status, HaStatus::Offline);
    }
    let shared = MqttShared::default();
    let mut c = rig.client_with(&shared);
    c.begin(); // before any pass
    let r = shared.regulator_state();
    assert_eq!((r.mode, r.ha), (MqttMode::MqttHa, HaStatus::Offline));
    assert!(!r.broker_connected);
    assert_eq!(shared.status().ha_status, HaStatus::Offline);
}
