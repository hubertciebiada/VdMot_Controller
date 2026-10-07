//! PubSubClient 2.8 behaviour (new: the C++ tests saw a fake of the library): packet bytes,
//! CONNACK codes and timeout, keepalive and -4, the buffer limit, QoS 1 PUBACK, oversized and
//! malformed packets, the state codes.

use super::*;
use crate::testkit::broker::{clock_and_tcp, Rig};
use crate::testkit::broker::{publish_packet, remaining_length};

const HOST: &str = "broker.local";
const PORT: u16 = 1883;

fn args<'a>() -> ConnectArgs<'a> {
    ConnectArgs {
        id: b"VdMot-123456",
        user: Some(b"user"),
        password: Some(b"secret"),
        will: Some(Will {
            topic: b"VdMot/status",
            qos: 0,
            retain: true,
            message: b"offline",
        }),
        clean_session: false,
    }
}

fn mqtt_str(s: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s);
}

fn packet(first: u8, body: &[u8]) -> Vec<u8> {
    let mut p = vec![first];
    remaining_length(body.len(), &mut p);
    p.extend_from_slice(body);
    p
}

/// A broker on HOST:PORT and a connection with the glue's settings (buffer 2304, socket timeout 5 s,
/// keepalive 60 s).
fn setup() -> Rig {
    let rig = clock_and_tcp();
    rig.broker.attach(&rig.tcp, HOST, PORT);
    rig
}

fn conn(rig: &Rig) -> MqttConn<&crate::testkit::FakeTcp, &crate::testkit::FakeClock> {
    let mut c = MqttConn::new(&rig.tcp, &rig.clock, 2304);
    c.set_socket_timeout(5);
    c.set_keep_alive(60);
    c
}

fn written(rig: &Rig, connection: usize) -> Vec<u8> {
    rig.tcp.wire(connection).lock().unwrap().written.clone()
}

/// Polls until nothing more arrives; the messages delivered.
fn drain(
    c: &mut MqttConn<&crate::testkit::FakeTcp, &crate::testkit::FakeClock>,
) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut got = Vec::new();
    for _ in 0..8 {
        c.poll(|t, p| got.push((t.to_vec(), p.to_vec())));
    }
    got
}

// ---------------------------------------------------------------- connect

#[test]
fn connect_sends_the_connect_packet_of_the_library() {
    let rig = setup();
    let mut c = conn(&rig);
    assert_eq!(c.state(), MQTT_DISCONNECTED);
    assert!(!c.connected());
    assert!(c.connect(HOST, PORT, &args()));
    assert_eq!(c.state(), MQTT_CONNECTED);
    assert!(c.connected());
    let mut body = vec![0, 4, b'M', b'Q', b'T', b'T', 4];
    body.push(0x80 | 0x40 | 0x20 | 0x04); // user, password, will retain, will; no clean session
    body.extend_from_slice(&[0, 60]);
    for s in [
        &b"VdMot-123456"[..],
        b"VdMot/status",
        b"offline",
        b"user",
        b"secret",
    ] {
        mqtt_str(s, &mut body);
    }
    assert_eq!(written(&rig, 0), packet(0x10, &body));
    let r = rig.tcp.connects();
    assert_eq!(
        (r[0].host.as_str(), r[0].port, r[0].timeout_ms),
        (HOST, PORT, 3000)
    );
    let b = rig.broker.state();
    let cn = &b.connects[0];
    assert_eq!(cn.client_id, b"VdMot-123456");
    assert!(!cn.clean_session());
    assert!(cn.will_retain());
    assert_eq!(cn.will_qos(), 0);
    assert_eq!(cn.keep_alive, 60);
}

#[test]
fn connect_flags_follow_the_arguments() {
    let cases: [(ConnectArgs, u8, usize); 4] = [
        // no will, no user, clean session
        (
            ConnectArgs {
                id: b"id",
                user: None,
                password: Some(b"pw"),
                will: None,
                clean_session: true,
            },
            0x02,
            1,
        ),
        // user without password; will QoS 1 not retained
        (
            ConnectArgs {
                id: b"id",
                user: Some(b"u"),
                password: None,
                will: Some(Will {
                    topic: b"t",
                    qos: 1,
                    retain: false,
                    message: b"m",
                }),
                clean_session: false,
            },
            0x80 | 0x08 | 0x04,
            4,
        ),
        // will QoS 2 retained, clean
        (
            ConnectArgs {
                id: b"id",
                user: None,
                password: None,
                will: Some(Will {
                    topic: b"t",
                    qos: 2,
                    retain: true,
                    message: b"m",
                }),
                clean_session: true,
            },
            0x20 | 0x10 | 0x04 | 0x02,
            3,
        ),
        // C strings: a NUL ends them
        (
            ConnectArgs {
                id: b"ab\0cd",
                user: None,
                password: None,
                will: None,
                clean_session: false,
            },
            0,
            1,
        ),
    ];
    for (a, flags, strings) in cases {
        let rig = setup();
        let mut c = conn(&rig);
        assert!(c.connect(HOST, PORT, &a));
        let w = written(&rig, 0);
        assert_eq!(w[9], flags);
        let cn = rig.broker.state().connects[0].clone();
        let got = 1
            + usize::from(cn.will_topic.is_some()) * 2
            + usize::from(cn.user.is_some())
            + usize::from(cn.password.is_some());
        assert_eq!(got, strings);
        assert_eq!(cn.client_id, c_str(a.id));
    }
}

#[test]
fn connect_failures_and_their_states() {
    // no TCP connection
    let rig = clock_and_tcp();
    let mut c = conn(&rig);
    assert!(!c.connect(HOST, PORT, &args()));
    assert_eq!(c.state(), MQTT_CONNECT_FAILED);
    // CONNACK refusals: the return code is the state, the socket goes
    let rig = setup();
    let mut c = conn(&rig);
    for rc in 1..=5u8 {
        rig.broker.state().connack.push_back(Some(rc));
        assert!(!c.connect(HOST, PORT, &args()));
        assert_eq!(c.state(), i32::from(rc));
        assert!(!c.connected());
    }
    rig.broker.state().connack.push_back(Some(0x80));
    assert!(!c.connect(HOST, PORT, &args()));
    assert_eq!(c.state(), 128);
    assert_eq!(rig.broker.state().closes, 6);
    assert!(c.connect(HOST, PORT, &args()));
    assert_eq!(rig.tcp.connections(), 7);
}

#[test]
fn no_connack_within_the_socket_timeout_is_state_minus_4() {
    let rig = setup();
    let mut c = conn(&rig);
    rig.broker.state().connack.push_back(None);
    let t0 = rig.clock.ms();
    assert!(!c.connect(HOST, PORT, &args()));
    assert_eq!(c.state(), MQTT_CONNECTION_TIMEOUT);
    assert_eq!(rig.clock.ms() - t0, 5000);
    assert_eq!(rig.broker.state().closes, 1);
    // a CONNACK that comes just in time
    rig.broker.state().connack.push_back(Some(0));
    rig.broker.state().reply_ms = 4999;
    assert!(c.connect(HOST, PORT, &args()));
    // a socket timeout of 0 gives up at once
    let rig = setup();
    let mut c = conn(&rig);
    c.set_socket_timeout(0);
    assert!(!c.connect(HOST, PORT, &args()));
    assert_eq!(c.state(), MQTT_CONNECTION_TIMEOUT);
}

#[test]
fn any_four_byte_answer_with_a_zero_code_connects() {
    // the library checks neither the packet type nor anything else of the CONNACK
    let rig = setup();
    let mut c = conn(&rig);
    rig.broker.state().connack.push_back(None);
    rig.broker.send_raw(&[0x40, 0x02, 0x00, 0x00]);
    assert!(c.connect(HOST, PORT, &args()));
    // a 2-byte answer: not connected, the state stays, the socket goes
    let rig = setup();
    let mut c = conn(&rig);
    rig.broker.state().connack.push_back(None);
    rig.broker.send_raw(&[0xD0, 0x00]);
    assert!(!c.connect(HOST, PORT, &args()));
    assert_eq!(c.state(), MQTT_DISCONNECTED);
    assert_eq!(rig.broker.state().closes, 1);
}

#[test]
fn a_string_that_does_not_fit_closes_the_socket_without_a_state() {
    let rig = setup();
    let mut c = MqttConn::new(&rig.tcp, &rig.clock, 40);
    let long = [b'x'; 40];
    // 5 header bytes + 10 of the preamble, then each string with its two length bytes
    let a = ConnectArgs {
        id: &long[..19],        // 15 + 2 + 19 = 36 fits
        user: Some(&long[..3]), // 36 + 2 + 3 = 41 does not
        password: None,
        will: None,
        clean_session: true,
    };
    assert!(!c.connect(HOST, PORT, &a));
    assert_eq!(c.state(), MQTT_DISCONNECTED);
    assert_eq!(written(&rig, 0), b"");
    assert_eq!(rig.broker.state().closes, 1);
    let a = ConnectArgs {
        user: Some(&long[..2]), // exactly 40
        ..a
    };
    assert!(c.connect(HOST, PORT, &a));
    assert_eq!(c.buffer_size(), 40);
    assert_eq!(MqttConn::new(&rig.tcp, &rig.clock, 3).buffer_size(), 16);
}

#[test]
fn connect_while_connected_sends_nothing() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    let before = written(&rig, 0).len();
    assert!(c.connect(HOST, PORT, &args()));
    assert_eq!(written(&rig, 0).len(), before);
    assert_eq!(rig.tcp.connections(), 1);
}

// ---------------------------------------------------------------- publish, subscribe

#[test]
fn publish_builds_the_packet_in_the_buffer() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(!c.publish(b"t", b"1", false)); // not connected
    assert!(c.connect(HOST, PORT, &args()));
    let start = written(&rig, 0).len();
    assert!(c.publish(b"VdMot/common/state", b"ok", true));
    assert!(c.publish(b"a\0ignored", b"x", false)); // the topic is a C string
    let payload = [b'p'; 200]; // a two-byte remaining length
    assert!(c.publish(b"big", &payload, false));
    let mut want = Vec::new();
    let mut body = Vec::new();
    mqtt_str(b"VdMot/common/state", &mut body);
    body.extend_from_slice(b"ok");
    want.extend(packet(0x31, &body));
    let mut body = Vec::new();
    mqtt_str(b"a", &mut body);
    body.push(b'x');
    want.extend(packet(0x30, &body));
    let mut body = Vec::new();
    mqtt_str(b"big", &mut body);
    body.extend_from_slice(&payload);
    want.extend(packet(0x30, &body));
    assert_eq!(&written(&rig, 0)[start..], &want[..]);
    assert_eq!(want[want.len() - 208..][..3], [0x30, 0xCD, 0x01]);
    let b = rig.broker.state();
    assert!(b.published_to(b"VdMot/common/state")[0].retain);
    assert_eq!(b.published_to(b"a")[0].payload, b"x");
}

#[test]
fn publish_needs_room_for_header_topic_and_payload() {
    let rig = setup();
    let mut c = MqttConn::new(&rig.tcp, &rig.clock, 20);
    assert!(c.connect(
        HOST,
        PORT,
        &ConnectArgs {
            id: b"i",
            user: None,
            password: None,
            will: None,
            clean_session: true
        }
    ));
    assert!(c.publish(b"topic", b"01234567", true)); // 5 + 2 + 5 + 8 = 20
    assert!(!c.publish(b"topic", b"012345678", true)); // 21 > 20
    let topic = [b't'; 30];
    assert!(!c.publish(&topic, b"", true));
    assert_eq!(rig.broker.state().published.len(), 1);
}

#[test]
fn a_failed_socket_write_fails_the_publish() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    rig.tcp.wire(0).lock().unwrap().fail_writes = true;
    rig.clock.advance_ms(7);
    assert!(!c.publish(b"t", b"1", false));
    assert!(c.connected()); // the session stays until the socket reports it gone
    assert!(!c.subscribe(b"t", 0));
}

#[test]
fn subscribe_sends_one_filter_with_the_next_message_id() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(!c.subscribe(b"a/#", 1)); // not connected
    assert!(c.connect(HOST, PORT, &args()));
    let start = written(&rig, 0).len();
    assert!(c.subscribe(b"VdMot/valves/+/target", 1));
    assert!(c.subscribe(b"VdMot/cmd/#\0x", 0));
    assert!(!c.subscribe(b"x", 2));
    let mut want = Vec::new();
    let mut body = vec![0, 2];
    mqtt_str(b"VdMot/valves/+/target", &mut body);
    body.push(1);
    want.extend(packet(0x82, &body));
    let mut body = vec![0, 3];
    mqtt_str(b"VdMot/cmd/#", &mut body);
    body.push(0);
    want.extend(packet(0x82, &body));
    assert_eq!(&written(&rig, 0)[start..], &want[..]);
    // the SUBACKs arrive later and are ignored
    assert!(drain(&mut c).is_empty());
    assert!(c.connected());
    // a filter that does not fit: 9 + filter > buffer
    let mut small = MqttConn::new(&rig.tcp, &rig.clock, 20);
    let a = ConnectArgs {
        id: b"i",
        user: None,
        password: None,
        will: None,
        clean_session: true,
    };
    assert!(small.connect(HOST, PORT, &a));
    assert!(small.subscribe(&[b'f'; 10], 0)); // 5 + 2 + 2 + 10 + 1 = 20
                                              // 9 + 11 passes the library's check, but the QoS byte would go past the buffer (the
                                              // library wrote it there): refused
    assert!(!small.subscribe(&[b'f'; 11], 0));
    assert!(!small.subscribe(&[b'f'; 12], 0));
    assert_eq!(rig.broker.state().subscribed.len(), 3);
}

#[test]
fn message_ids_wrap_to_1() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    c.next_msg_id = 0xFFFE;
    assert!(c.subscribe(b"a", 0));
    assert!(c.subscribe(b"b", 0));
    let ids: Vec<u16> = rig.broker.state().subscribed.iter().map(|s| s.0).collect();
    assert_eq!(ids, vec![0xFFFF, 1]);
    // a new session starts again at 2
    c.disconnect();
    assert!(c.connect(HOST, PORT, &args()));
    assert!(c.subscribe(b"c", 0));
    assert_eq!(rig.broker.state().subscribed[2].0, 2);
}

// ---------------------------------------------------------------- loop

#[test]
fn poll_delivers_one_message_per_call_and_acknowledges_qos_1() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(!c.poll(|_, _| {}));
    assert!(c.connect(HOST, PORT, &args()));
    rig.broker
        .send_publish(b"VdMot/valves/1/target", b"50", 0, false, 0);
    rig.broker
        .send_publish(b"VdMot/cmd/reset", b"PRESS", 1, true, 0x1234);
    rig.clock.advance_ms(1);
    let mut got = Vec::new();
    assert!(c.poll(|t, p| got.push((t.to_vec(), p.to_vec()))));
    assert_eq!(
        got,
        vec![(b"VdMot/valves/1/target".to_vec(), b"50".to_vec())]
    );
    let before = written(&rig, 0).len();
    assert!(c.poll(|t, p| got.push((t.to_vec(), p.to_vec()))));
    assert_eq!(got[1], (b"VdMot/cmd/reset".to_vec(), b"PRESS".to_vec()));
    assert_eq!(&written(&rig, 0)[before..], &[0x40, 2, 0x12, 0x34]);
    assert_eq!(rig.broker.state().pubacks, vec![0x1234]);
    assert!(c.poll(|_, _| panic!("nothing more")));
}

#[test]
fn inbound_topics_are_c_strings_and_empty_payloads_arrive() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    rig.broker.send_publish(b"a\0b", b"", 0, true, 0);
    rig.clock.advance_ms(1);
    assert_eq!(drain(&mut c), vec![(b"a".to_vec(), b"".to_vec())]);
}

#[test]
fn qos_2_is_read_as_qos_0_with_the_id_in_the_payload() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    rig.broker.send_publish(b"t", b"x", 2, false, 0x0102);
    rig.clock.advance_ms(1);
    assert_eq!(drain(&mut c), vec![(b"t".to_vec(), vec![1, 2, b'x'])]);
    assert!(rig.broker.state().pubacks.is_empty());
}

#[test]
fn an_oversized_packet_is_read_and_dropped() {
    let rig = setup();
    let mut c = MqttConn::new(&rig.tcp, &rig.clock, 64);
    c.set_keep_alive(60);
    let a = ConnectArgs {
        id: b"i",
        user: None,
        password: None,
        will: None,
        clean_session: true,
    };
    assert!(c.connect(HOST, PORT, &a));
    let t_connect = rig.clock.ms();
    rig.clock.advance_ms(30_000);
    // header, length, topic length, "t" and 59 bytes fill the 64 bytes; one more does not fit
    rig.broker.send_publish(b"t", &[b'a'; 59], 0, false, 0);
    rig.broker.send_publish(b"t", &[b'b'; 60], 0, false, 0);
    rig.broker.send_publish(b"t", &[b'c'; 58], 1, false, 9); // 65 with its id
    rig.broker.send_publish(b"u", b"after", 0, false, 0);
    let mut got = Vec::new();
    for _ in 0..4 {
        assert!(c.poll(|t, p| got.push((t.to_vec(), p.len()))));
    }
    assert_eq!(got, vec![(b"t".to_vec(), 59), (b"u".to_vec(), 5)]);
    assert!(rig.broker.state().pubacks.is_empty()); // the dropped QoS 1 message is not acknowledged
    assert!(c.connected());
    assert!(rig.clock.ms() - t_connect < 30_010);
}

#[test]
fn a_dropped_packet_does_not_count_as_inbound_traffic() {
    let rig = setup();
    let mut c = MqttConn::new(&rig.tcp, &rig.clock, 32);
    c.set_keep_alive(10);
    let a = ConnectArgs {
        id: b"i",
        user: None,
        password: None,
        will: None,
        clean_session: true,
    };
    assert!(c.connect(HOST, PORT, &a));
    rig.clock.advance_ms(6000);
    rig.broker.send_publish(b"t", &[0; 40], 0, false, 0);
    assert!(c.poll(|_, _| panic!("dropped")));
    rig.clock.advance_ms(4001); // 10 001 ms since the CONNACK
    assert!(c.publish(b"t", b"1", false));
    let pings = rig.broker.state().pingreqs;
    assert!(c.poll(|_, _| {}));
    assert_eq!(rig.broker.state().pingreqs, pings + 1);
}

#[test]
fn a_publish_whose_topic_length_runs_past_the_packet_reaches_no_callback() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    // remaining length 4: topic length 0x00FF, two bytes of "topic"
    rig.broker.send_raw(&[0x30, 4, 0x00, 0xFF, b'a', b'b']);
    rig.broker.send_raw(&[0x32, 5, 0x00, 0x02, b'a', b'b', 7]); // QoS 1 without its id
    rig.broker.send_publish(b"ok", b"1", 0, false, 0);
    rig.clock.advance_ms(1);
    assert_eq!(drain(&mut c), vec![(b"ok".to_vec(), b"1".to_vec())]);
    assert!(rig.broker.state().pubacks.is_empty());
}

#[test]
fn a_byte_that_does_not_come_ends_the_packet_where_it_is() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    // a PUBLISH whose last byte never comes
    let p = publish_packet(b"t", b"xyz", 0, false, 0);
    rig.broker.send_raw(&p[..p.len() - 1]);
    rig.clock.advance_ms(1);
    let t0 = rig.clock.ms();
    assert!(c.poll(|_, _| panic!("cut short")));
    assert_eq!(rig.clock.ms() - t0, 5000);
    // the rest of the stream is read as new packets: 'z' and a remaining length of 0x30 (the
    // next PUBLISH header) swallow that PUBLISH
    rig.broker.send_raw(b"z");
    rig.broker.send_publish(b"t", b"ok", 0, false, 0);
    rig.clock.advance_ms(1);
    let t1 = rig.clock.ms();
    assert!(c.poll(|_, _| panic!("swallowed")));
    assert_eq!(rig.clock.ms() - t1, 5000);
    assert!(drain(&mut c).is_empty());
    assert!(c.connected());
}

#[test]
fn an_invalid_remaining_length_kills_the_connection() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    rig.broker.send_raw(&[0x30, 0xFF, 0xFF, 0xFF, 0xFF, 0x01]);
    rig.clock.advance_ms(1);
    assert!(!c.poll(|_, _| {}));
    assert_eq!(c.state(), MQTT_DISCONNECTED);
    assert!(!c.connected());
    assert_eq!(rig.broker.state().closes, 1);
}

#[test]
fn ping_requests_are_answered_and_other_packets_ignored() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    let before = written(&rig, 0).len();
    rig.broker.send_raw(&[0xC0, 0]);
    rig.broker.send_raw(&[0xB0, 2, 0, 1]); // UNSUBACK
    rig.clock.advance_ms(1);
    assert!(drain(&mut c).is_empty());
    assert_eq!(&written(&rig, 0)[before..], &[0xD0, 0]);
}

// ---------------------------------------------------------------- keepalive, loss

#[test]
fn keepalive_pings_after_the_period_and_times_out_without_answer() {
    let rig = setup();
    let mut c = conn(&rig);
    c.set_keep_alive(15);
    assert!(c.connect(HOST, PORT, &args()));
    // the CONNECT went out 1 ms before the CONNACK came in
    rig.clock.advance_ms(14_999);
    assert!(c.poll(|_, _| {})); // exactly the period since the CONNECT: not yet
    assert_eq!(rig.broker.state().pingreqs, 0);
    rig.clock.advance_ms(1);
    assert!(c.poll(|_, _| {}));
    assert_eq!(rig.broker.state().pingreqs, 1);
    rig.clock.advance_ms(1);
    assert!(c.poll(|_, _| {})); // the PINGRESP
    rig.clock.advance_ms(15_001);
    assert!(c.poll(|_, _| {}));
    assert_eq!(rig.broker.state().pingreqs, 2);
    // the broker stops answering
    rig.broker.state().ping_resp = false;
    rig.clock.advance_ms(1);
    assert!(c.poll(|_, _| {})); // the second PINGRESP arrives
    rig.clock.advance_ms(15_001);
    assert!(c.poll(|_, _| {}));
    assert_eq!(rig.broker.state().pingreqs, 3);
    rig.clock.advance_ms(15_001);
    assert!(!c.poll(|_, _| {}));
    assert_eq!(c.state(), MQTT_CONNECTION_TIMEOUT);
    assert!(!c.connected());
    assert_eq!(rig.broker.state().pingreqs, 3);
}

#[test]
fn outbound_traffic_alone_does_not_hold_off_the_ping() {
    let rig = setup();
    let mut c = conn(&rig);
    c.set_keep_alive(10);
    assert!(c.connect(HOST, PORT, &args()));
    for _ in 0..10 {
        rig.clock.advance_ms(1001);
        assert!(c.publish(b"t", b"1", false));
        assert!(c.poll(|_, _| {}));
    }
    assert_eq!(rig.broker.state().pingreqs, 1);
}

#[test]
fn a_dropped_connection_is_lost_and_a_new_connect_starts_over() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    rig.broker.drop_connection();
    assert!(!c.connected());
    assert_eq!(c.state(), MQTT_CONNECTION_LOST);
    assert!(!c.poll(|_, _| {}));
    assert!(!c.publish(b"t", b"1", false));
    assert!(c.connect(HOST, PORT, &args()));
    assert_eq!(rig.tcp.connections(), 2);
    // the glue drops the socket after a failed publish
    c.drop_socket();
    assert_eq!(c.state(), MQTT_CONNECTED);
    assert!(!c.connected());
    assert_eq!(c.state(), MQTT_CONNECTION_LOST);
}

#[test]
fn disconnect_sends_its_packet_and_closes() {
    let rig = setup();
    let mut c = conn(&rig);
    c.disconnect(); // without a socket
    assert_eq!(c.state(), MQTT_DISCONNECTED);
    assert!(c.connect(HOST, PORT, &args()));
    let before = written(&rig, 0).len();
    rig.clock.advance_ms(3);
    c.disconnect();
    assert_eq!(&written(&rig, 0)[before..], &[0xE0, 0]);
    assert_eq!(c.state(), MQTT_DISCONNECTED);
    assert!(!c.connected());
    let b = rig.broker.state();
    assert_eq!((b.disconnects, b.closes), (1, 1));
}

#[test]
fn state_codes_are_the_library_ones() {
    assert_eq!(
        [
            MQTT_CONNECTION_TIMEOUT,
            MQTT_CONNECTION_LOST,
            MQTT_CONNECT_FAILED,
            MQTT_DISCONNECTED,
            MQTT_CONNECTED,
            MQTT_CONNECT_BAD_PROTOCOL,
            MQTT_CONNECT_BAD_CLIENT_ID,
            MQTT_CONNECT_UNAVAILABLE,
            MQTT_CONNECT_BAD_CREDENTIALS,
            MQTT_CONNECT_UNAUTHORIZED
        ],
        [-4, -3, -2, -1, 0, 1, 2, 3, 4, 5]
    );
    assert_eq!(
        (
            DEFAULT_KEEP_ALIVE_S,
            DEFAULT_SOCKET_TIMEOUT_S,
            CONNECT_TIMEOUT_MS,
            MAX_HEADER_SIZE
        ),
        (15, 15, 3000, 5)
    );
}

#[test]
fn defaults_are_the_library_ones() {
    let rig = setup();
    let mut c = MqttConn::new(&rig.tcp, &rig.clock, 256);
    rig.broker.state().connack.push_back(None);
    assert!(!c.connect(HOST, PORT, &args()));
    assert_eq!(rig.clock.ms(), 15_000); // the default socket timeout
    assert_eq!(rig.broker.state().connects[0].keep_alive, 15);
}

#[test]
fn a_will_qos_above_2_spills_into_the_next_flag_bits_as_in_the_library() {
    // v = 0x04 | (willQos << 3) | (willRetain << 5), then the clean, user and password bits
    let cases: [(u8, bool, bool, bool, u8); 4] = [
        (4, true, false, false, 0x24),          // QoS 4 is the retain bit
        (8, false, true, true, 0xC4),           // QoS 8 is the password bit
        (16, false, true, false, 0x84),         // QoS 16 is the user bit
        (32, false, false, false, 0x04 | 0x02), // shifted out of the byte
    ];
    for (qos, retain, user, password, flags) in cases {
        let rig = setup();
        let mut c = conn(&rig);
        let a = ConnectArgs {
            id: b"id",
            user: user.then_some(&b"u"[..]),
            password: password.then_some(&b"p"[..]),
            will: Some(Will {
                topic: b"t",
                qos,
                retain,
                message: b"m",
            }),
            clean_session: qos == 32,
        };
        assert!(c.connect(HOST, PORT, &a));
        assert_eq!(written(&rig, 0)[9], flags, "qos {qos}");
    }
}

#[test]
fn a_subscribe_the_buffer_cannot_hold_still_takes_its_message_id() {
    // the library's check lets a filter of buffer - 9 bytes through and wrote past the buffer;
    // the id it took stays taken
    let rig = setup();
    let a = ConnectArgs {
        id: b"i",
        user: None,
        password: None,
        will: None,
        clean_session: true,
    };
    let mut small = MqttConn::new(&rig.tcp, &rig.clock, 20);
    assert!(small.connect(HOST, PORT, &a));
    assert!(!small.subscribe(&[b'f'; 11], 0)); // id 2, not sent
    assert!(!small.subscribe(&[b'f'; 12], 0)); // refused before an id
    assert!(small.subscribe(b"g", 0));
    let ids: Vec<u16> = rig.broker.state().subscribed.iter().map(|s| s.0).collect();
    assert_eq!(ids, vec![3]);
}

#[test]
fn long_topics_carry_a_two_byte_length() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    let topic = [b'x'; 300];
    assert!(c.publish(&topic, b"1", false));
    assert!(c.subscribe(&topic, 0));
    let b = rig.broker.state();
    assert_eq!(b.published[0].topic.len(), 300);
    assert_eq!(b.subscribed[0].1.len(), 300);
}

#[test]
fn keepalive_counts_from_the_last_inbound_packet_too() {
    let rig = setup();
    let mut c = conn(&rig);
    c.set_keep_alive(10);
    assert!(c.connect(HOST, PORT, &args())); // CONNACK in at 1 ms
    rig.clock.advance_ms(5000);
    assert!(c.publish(b"t", b"1", false)); // traffic out at 5001
    rig.clock.advance_ms(5000); // 10 000 ms after the CONNACK: not yet
    assert!(c.poll(|_, _| {}));
    assert_eq!(rig.broker.state().pingreqs, 0);
    rig.clock.advance_ms(1);
    assert!(c.poll(|_, _| {}));
    assert_eq!(rig.broker.state().pingreqs, 1);
}

#[test]
fn a_read_that_copies_nothing_is_no_data() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    rig.broker.send_publish(b"a", b"1", 0, false, 0);
    rig.clock.advance_ms(1);
    // the buffer of the first read still holds bytes of that packet
    assert_eq!(drain(&mut c), vec![(b"a".to_vec(), b"1".to_vec())]);
    rig.tcp.wire(0).lock().unwrap().empty_reads = 2;
    let t = rig.clock.ms();
    assert!(c.poll(|_, _| panic!("nothing arrived")));
    assert_eq!(rig.clock.ms(), t); // no packet started: no wait for its bytes
    rig.broker.send_publish(b"b", b"2", 0, false, 0);
    rig.clock.advance_ms(1);
    assert_eq!(drain(&mut c), vec![(b"b".to_vec(), b"2".to_vec())]);
}

#[test]
fn long_inbound_packets_and_topics_arrive_whole() {
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    // a two-byte remaining length (200 + 2 + 3 = 205 bytes) and a topic of 300 bytes
    let payload: Vec<u8> = (0..200).map(|i| i as u8).collect();
    rig.broker.send_publish(b"t/a", &payload, 0, false, 0);
    let topic = [b'x'; 300];
    rig.broker.send_publish(&topic, b"v", 1, false, 0x0101);
    rig.clock.advance_ms(1);
    let got = drain(&mut c);
    assert_eq!(
        got,
        vec![(b"t/a".to_vec(), payload), (topic.to_vec(), b"v".to_vec())]
    );
    assert_eq!(rig.broker.state().pubacks, vec![0x0101]);
}

#[test]
fn a_publish_shorter_than_its_topic_length_swallows_the_next_bytes() {
    // the library reads the two topic-length bytes whatever the remaining length says
    let rig = setup();
    let mut c = conn(&rig);
    assert!(c.connect(HOST, PORT, &args()));
    rig.broker.send_raw(&[0x30, 0x00]);
    rig.broker.send_publish(b"t", b"1", 0, false, 0);
    rig.clock.advance_ms(1);
    let mut got = Vec::new();
    for _ in 0..3 {
        c.poll(|t, p| got.push((t.to_vec(), p.to_vec())));
    }
    assert!(got.is_empty(), "{got:?}");
}

#[test]
fn a_publish_refused_for_its_size_is_no_outbound_traffic() {
    let rig = setup();
    let a = ConnectArgs {
        id: b"i",
        user: None,
        password: None,
        will: None,
        clean_session: true,
    };
    let mut c = MqttConn::new(&rig.tcp, &rig.clock, 20);
    c.set_keep_alive(10);
    assert!(c.connect(HOST, PORT, &a)); // CONNECT out at 0, CONNACK in at 1
    rig.clock.advance_ms(5000);
    assert!(!c.publish(b"topic", b"0123456789", false)); // 5 + 2 + 5 + 10 = 22 > 20
    rig.clock.advance_ms(4999); // 10 000 ms after the CONNECT: not yet
    assert!(c.poll(|_, _| {}));
    assert_eq!(rig.broker.state().pingreqs, 0);
    rig.clock.advance_ms(1);
    assert!(c.poll(|_, _| {}));
    assert_eq!(rig.broker.state().pingreqs, 1);
}

#[test]
fn a_filter_only_the_check_lets_through_takes_the_next_message_id() {
    let rig = setup();
    let a = ConnectArgs {
        id: b"i",
        user: None,
        password: None,
        will: None,
        clean_session: true,
    };
    let mut small = MqttConn::new(&rig.tcp, &rig.clock, 20);
    assert!(small.connect(HOST, PORT, &a));
    assert!(!small.subscribe(&[b'f'; 11], 0)); // 9 + 11 = 20: past the check, id 2, not sent
    assert!(small.subscribe(b"g", 0));
    let ids: Vec<u16> = rig.broker.state().subscribed.iter().map(|s| s.0).collect();
    assert_eq!(ids, vec![3]);
}
