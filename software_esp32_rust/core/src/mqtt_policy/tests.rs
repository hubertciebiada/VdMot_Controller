//! Port of test/native/test_mqtt_policy.cpp: MQTT task decisions: HA status, reconnect pacing,
//! client id, inbound commands, reject throttle, button confirmation, target latch.
//!
//! C++ cases that pass a null pointer have no Rust form (a slice is never null); they are named
//! in comments, and where the C++ treats null like the empty input the empty slice is tested.
//! The C++ out-parameters left alone on false are the None results.

use super::*;
use crate::common::copy_string;
use crate::test_support::{assert_text, CRand};
use std::format;
use std::string::String;
use std::vec::Vec;

type Change = RegulatorWatchChange;

fn client_id(station: &[u8], a: u8, b: u8, c: u8) -> Vec<u8> {
    let mac = [0x24, 0x0a, 0xc4, a, b, c];
    let mut out = [b'X'; 32];
    let n = build_mqtt_client_id(station, &mac, &mut out);
    // C++ n == strlen(out): the text holds no NUL
    assert!(!out[..n].contains(&0));
    out[..n].to_vec()
}

/// C++ `clientId(station)`: mac ..:a1:b2:c3.
fn id(station: &[u8]) -> Vec<u8> {
    client_id(station, 0xa1, 0xb2, 0xc3)
}

/// The C++ `Inbound` fixture: the context is built from the fields (Rust has no struct that
/// points into itself).
struct Inbound {
    topics: TopicContext,
    seg: Segments,
    echo: EchoFilter,
    /// C++ ctx.echo != nullptr
    with_echo: bool,
    active_mask: u16,
    mode: MqttMode,
    stm_v3: bool,
}

impl Inbound {
    fn new(separate: bool, mode: MqttMode) -> Self {
        let mut topics = TopicContext::default();
        copy_string(&mut topics.station, b"VdMot");
        topics.separate = separate;
        let seg: Segments = core::array::from_fn(|i| {
            let mut s = Text::new();
            if i == 0 {
                copy_string(&mut s, b"Bad_1");
            } else {
                copy_string(&mut s, format!("{}", i + 1).as_bytes());
            }
            s
        });
        Self {
            topics,
            seg,
            echo: EchoFilter::default(),
            with_echo: true,
            active_mask: 0x00FF, // valves 1..8 active
            mode,
            stm_v3: true,
        }
    }

    fn ctx(&self) -> InboundContext<'_> {
        InboundContext {
            topics: Some(&self.topics),
            ha_prefix: b"ha",
            segments: Some(&self.seg),
            active_mask: self.active_mask,
            mode: self.mode,
            stm_v3: self.stm_v3,
            echo: self.with_echo.then_some(&self.echo),
        }
    }

    fn decide(&self, topic: &str, payload: &str) -> InboundDecision {
        decide_inbound(&self.ctx(), topic.as_bytes(), payload.as_bytes())
    }

    fn clear_echo(&self, topic: &str, payload: &str) -> bool {
        inbound_is_clear_echo(&self.ctx(), topic.as_bytes(), payload.as_bytes())
    }
}

/// action/valve/pos/reason/detail/clear as one comparable string (the Debug names are the C++
/// kActions names).
fn str(d: &InboundDecision) -> String {
    let mut s = format!("{:?} v{} p{}", d.action, d.valve, d.pos);
    if d.reason != RejectReason::None {
        s += &format!(" '{}'", reject_reason_name(d.reason));
    }
    if d.detail != 0 {
        s += &format!(" d{}", d.detail);
    }
    if d.clear_retained {
        s += " clear";
    }
    s
}

fn decision(a: InboundAction, valve: u8) -> InboundDecision {
    InboundDecision {
        action: a,
        valve,
        ..InboundDecision::default()
    }
}

#[test]
fn regulator_watch_ha_status_transitions() {
    let mut w = RegulatorWatch::default();
    assert_eq!(w.ha_status(), HaStatus::Unknown);
    assert_eq!(w.on_ha_status(b"online"), Change::None); // Unknown -> Online is silent
    assert_eq!(w.ha_status(), HaStatus::Online);
    assert_eq!(w.on_ha_status(b"online"), Change::None);
    assert_eq!(w.on_ha_status(b"offline"), Change::WentOffline);
    assert_eq!(w.ha_status(), HaStatus::Offline);
    assert_eq!(w.on_ha_status(b"offline"), Change::None);
    assert_eq!(w.ha_status(), HaStatus::Offline);
    let ignored: [&[u8]; 6] = [
        b"ONLINE",
        b"offline ",
        b" online",
        b"onlin",
        b"",
        b"offlinex",
    ];
    for p in ignored {
        assert_eq!(w.on_ha_status(p), Change::None, "{}", p.escape_ascii());
        assert_eq!(w.ha_status(), HaStatus::Offline);
    }
    // C++ onHaStatus(nullptr, 6) == None: no Rust form.
    assert_eq!(w.on_ha_status(&b"onlinex"[..6]), Change::CameOnline); // len is authoritative
    assert_eq!(w.ha_status(), HaStatus::Online);
    let mut u = RegulatorWatch::default();
    assert_eq!(u.on_ha_status(b"offline"), Change::WentOffline); // Unknown -> Offline
}

#[test]
fn regulator_watch_a_command_brings_an_offline_ha_back_restore_and_snapshot() {
    let mut w = RegulatorWatch::default();
    assert_eq!(w.on_inbound_command(), Change::None);
    assert_eq!(w.ha_status(), HaStatus::Unknown);
    w.on_ha_status(b"online");
    assert_eq!(w.on_inbound_command(), Change::None);
    w.on_ha_status(b"offline");
    assert_eq!(w.on_inbound_command(), Change::CameOnlineByCommand);
    assert_eq!(w.ha_status(), HaStatus::Online);
    assert_eq!(w.snapshot(), HaStatus::Online);
    w.restore(HaStatus::Offline);
    assert_eq!(w.ha_status(), HaStatus::Offline);
    assert_eq!(w.snapshot(), HaStatus::Offline);
    w.restore(HaStatus::Unknown);
    assert_eq!(w.ha_status(), HaStatus::Unknown);
    w.restore(HaStatus::Online);
    assert_eq!(w.ha_status(), HaStatus::Online);
    // C++ restore(static_cast<HaStatus>(3)) restores Unknown: no Rust form, a HaStatus cannot
    // hold 3.
    assert_eq!(HaStatus::from_raw(3), None);
}

/// The C++ record CRC: low 16 bits of crc32 over magic (LE), status, pad.
fn record_crc(r: &HaStatusRecord) -> u16 {
    let m = r.magic.to_le_bytes();
    (crc32(&[m[0], m[1], m[2], m[3], r.status, r.pad], 0) & 0xFFFF) as u16
}

#[test]
fn ha_status_rtc_record() {
    // power-on garbage (C++ memset 0xA5)
    let garbage = HaStatusRecord {
        magic: 0xA5A5_A5A5,
        status: 0xA5,
        pad: 0xA5,
        crc: 0xA5A5,
    };
    assert_eq!(decode_ha_status_record(&garbage), HaStatus::Unknown);
    let mut r = garbage;
    for s in [HaStatus::Unknown, HaStatus::Online, HaStatus::Offline] {
        encode_ha_status_record(s, &mut r);
        assert_eq!(r.magic, 0x4148_4456);
        assert_eq!(r.status, s as u8);
        assert_eq!(r.pad, 0);
        assert_eq!(decode_ha_status_record(&r), s);
    }
    encode_ha_status_record(HaStatus::Offline, &mut r);
    let mut bad = r;
    bad.magic ^= 1;
    assert_eq!(decode_ha_status_record(&bad), HaStatus::Unknown);
    bad = r;
    bad.crc ^= 0x8000;
    assert_eq!(decode_ha_status_record(&bad), HaStatus::Unknown);
    bad = r;
    bad.pad = 1; // covered by the CRC
    assert_eq!(decode_ha_status_record(&bad), HaStatus::Unknown);
    bad = r;
    bad.status = 1; // a valid status with a stale CRC
    assert_eq!(decode_ha_status_record(&bad), HaStatus::Unknown);
    // A status out of range with a matching CRC (C++ encodeHaStatusRecord of a HaStatus 3: built
    // by hand here, a HaStatus cannot hold 3).
    let mut three = HaStatusRecord {
        magic: HA_STATUS_MAGIC,
        status: 3,
        pad: 0,
        crc: 0,
    };
    three.crc = record_crc(&three);
    assert_eq!(decode_ha_status_record(&three), HaStatus::Unknown);
    // The CRC is the low half of crc32 over the first six bytes.
    encode_ha_status_record(HaStatus::Online, &mut r);
    let b = [0x56, 0x44, 0x48, 0x41, 1, 0];
    assert_eq!(r.crc, (crc32(&b, 0) & 0xFFFF) as u16);
    assert_eq!(HA_STATUS_MAGIC, u32::from_le_bytes(*b"VDHA"));
}

#[test]
fn ha_status_rtc_record_a_wrong_magic_with_a_matching_crc_is_unknown() {
    let mut r = HaStatusRecord {
        magic: HA_STATUS_MAGIC ^ 0x0100,
        status: HaStatus::Offline as u8,
        pad: 0,
        crc: 0,
    };
    r.crc = record_crc(&r);
    assert_eq!(decode_ha_status_record(&r), HaStatus::Unknown);
    // the record layout of the C++ struct (RTC memory)
    assert_eq!(core::mem::size_of::<HaStatusRecord>(), 8);
    assert_eq!(core::mem::offset_of!(HaStatusRecord, crc), 6);
}

#[test]
fn discovery_gate_a_run_waits_for_settled_inputs_at_most_max_wait_ms() {
    const MAX: u32 = DiscoveryGate::MAX_WAIT_MS;
    let mut g = DiscoveryGate::default();
    assert!(!g.pending());
    assert!(!g.due(true, 0)); // nothing requested
    g.request(1000);
    assert!(g.pending());
    assert!(!g.due(false, 1000));
    assert!(g.due(true, 1000)); // settled: at once

    // unsettled: from the first request on, later requests do not move the clock
    g.request(5000);
    assert!(!g.due(false, 1000 + MAX - 1));
    assert!(g.due(false, 1000 + MAX));
    g.clear();
    assert!(!g.pending());
    assert!(!g.due(true, 1000 + MAX));
    // the next request starts a new wait, across the wrap of the clock
    g.request(0xFFFF_F000);
    assert!(!g.due(false, 0xFFFF_F000u32.wrapping_add(MAX - 1)));
    assert!(g.due(false, 0xFFFF_F000u32.wrapping_add(MAX)));
    assert_eq!(MAX, 120000);
}

#[test]
fn reconnect_pacer_back_off_reset_only_after_a_stable_connection() {
    const STABLE: u32 = ReconnectPacer::STABLE_MS;
    let mut p = ReconnectPacer::new(2000, 60000);
    assert!(p.due(0));
    p.on_connected(0);
    p.on_dropped(100); // dropped at once: a failed attempt
    assert!(!p.due(100));
    assert!(!p.due(2099));
    assert!(p.due(2100));
    let mut t: u32 = 2100;
    for wait in [4000u32, 8000, 16000, 32000, 60000, 60000] {
        p.on_connected(t);
        p.tick(t + 50, true);
        p.on_dropped(t + 100);
        assert!(!p.due(t + 100 + wait - 1), "{wait}");
        assert!(p.due(t + 100 + wait), "{wait}");
        t += 100 + wait;
    }
    // Up for 59 999 ms: still backing off.
    p.on_connected(t);
    p.tick(t + STABLE - 1, true);
    p.on_dropped(t + STABLE - 1);
    assert!(!p.due(t + STABLE));
    assert_eq!(p.delay_ms(), 60000);
    // Up for 60 000 ms (tick): the drop is due at once, delay back to min.
    t += 200000;
    p.on_connected(t);
    p.tick(t + STABLE, true);
    assert_eq!(p.delay_ms(), 2000);
    p.on_dropped(t + 70000);
    assert!(p.due(t + 70000));
    // tick while not connected (or before any connect) changes nothing.
    let mut q = ReconnectPacer::new(2000, 60000);
    q.on_attempt_failed(0);
    q.tick(100000, false);
    q.tick(100000, true); // never connected
    assert_eq!(q.delay_ms(), 4000);
    q.on_dropped(5); // not connected: nothing
    assert!(q.due(2000));
    assert!(!q.due(1999));
    // A second tick after stability does not reset a later back-off twice.
    let mut r = ReconnectPacer::new(2000, 60000);
    r.on_connected(0);
    r.tick(60000, true);
    r.on_attempt_failed(60001);
    r.tick(70000, true);
    assert_eq!(r.delay_ms(), 4000);
    // tick with connected false while the pacer thinks it is connected.
    let mut s = ReconnectPacer::new(2000, 60000);
    s.on_connected(0);
    s.tick(60000, false);
    s.on_dropped(60001);
    assert!(!s.due(60001));
    // force_now: due at once.
    s.force_now();
    assert!(s.due(60002));
    assert_eq!(s.delay_ms(), 2000);
    // Failed attempts double as well.
    let mut f = ReconnectPacer::new(2000, 60000);
    f.on_attempt_failed(0);
    assert!(!f.due(1999));
    assert!(f.due(2000));
    f.on_attempt_failed(2000);
    assert!(!f.due(5999));
    assert!(f.due(6000));
    // Across a millis() wrap.
    let mut w = ReconnectPacer::new(2000, 60000);
    w.on_connected(0xFFFF_FF00);
    w.tick(0xFFFF_FF00u32.wrapping_add(STABLE), true);
    assert_eq!(w.delay_ms(), 2000);
    // The C++ default arguments.
    assert_eq!(ReconnectPacer::default(), ReconnectPacer::new(2000, 60000));
    assert_eq!(STABLE, 60000);
}

#[test]
fn mqtt_client_id() {
    assert_text(&id(b"VdMot"), "VdMot-a1b2c3");
    assert_text(&id(b"Dom 1 \xc5\x81azienka EG"), "Dom-1-azienka-EG-a1b2c3");
    assert_text(&id(b"Wohnung-Ost-EG-Bad"), "Wohnung-Ost-EG-B-a1b2c3");
    assert_text(&id(b"abcdefghijklmno-x"), "abcdefghijklmno-a1b2c3");
    assert_text(&id(b"abcdefghijklmn--x"), "abcdefghijklmn-a1b2c3");
    assert_text(&id(b""), "VdMot-a1b2c3");
    assert_text(&client_id(b"VdMot", 0x00, 0x0f, 0xf0), "VdMot-000ff0");
    assert_text(&id(b"abcdefghijklmnopqrst"), "abcdefghijklmnop-a1b2c3");
    let mac = [1, 2, 3, 4, 5, 6];
    let mut out = [0u8; 24];
    assert_eq!(build_mqtt_client_id(b"VdMot", &mac, &mut out), 12);
    assert_text(&out[..12], "VdMot-040506");
    let mut small = [b'X'; 23];
    assert_eq!(build_mqtt_client_id(b"VdMot", &mac, &mut small), 0);
    // C++ a null output (cap 30): no Rust form.
    assert_eq!(build_mqtt_client_id(b"VdMot", &mac, &mut small[..0]), 0);
    // Length <= 23 for every station of 1..20 bytes (fixed seed).
    let mut rng = CRand::new(4242);
    for _ in 0..5000 {
        let mut st = [0u8; 21];
        let len = 1 + (rng.rand() % 20) as usize;
        for c in &mut st[..len] {
            *c = (1 + rng.rand() % 255) as u8;
        }
        let mut id = [0u8; 64];
        let n = build_mqtt_client_id(&st[..len], &mac, &mut id);
        assert!(n <= 23);
        assert!(n >= 8);
        assert!(!id[..n].contains(&0)); // C++ n == strlen(id)
        assert_eq!(id[n - 7], b'-');
    }
}

#[test]
fn echo_filter() {
    let mut e = EchoFilter::default();
    assert!(!e.is_echo(2, 0));
    e.published(2, 40);
    assert!(e.is_echo(2, 40));
    assert!(!e.is_echo(2, 41));
    assert!(!e.is_echo(3, 40));
    e.published(2, 41);
    assert!(!e.is_echo(2, 40));
    assert!(e.is_echo(2, 41));
    e.published(11, 0);
    assert!(e.is_echo(11, 0));
    e.published(12, 5); // out of range: ignored
    assert!(!e.is_echo(12, 5));
    e.reset();
    assert!(!e.is_echo(2, 41));
    assert!(!e.is_echo(11, 0));
}

#[test]
fn reject_reason_names() {
    assert_eq!(reject_reason_name(RejectReason::None), "");
    assert_eq!(reject_reason_name(RejectReason::Payload), "payload");
    assert_eq!(
        reject_reason_name(RejectReason::UnknownValve),
        "unknown valve"
    );
    assert_eq!(reject_reason_name(RejectReason::Inactive), "inactive");
    assert_eq!(reject_reason_name(RejectReason::Unsupported), "unsupported");
    assert_eq!(
        reject_reason_name(RejectReason::UnknownCommand),
        "unknown command"
    );
    assert_eq!(reject_reason_name(RejectReason::QueueFull), "queue full");
    assert_eq!(
        reject_reason_name(RejectReason::ClearNotConfirmed),
        "clear not confirmed"
    );
    // Rust only (D9)
    assert_eq!(
        reject_reason_name(RejectReason::StmSector0Pending),
        "stm sector 0 pending"
    );
    // C++ rejectReasonName(static_cast<RejectReason>(99)) == "": no Rust form.
    assert_eq!(RejectReason::from_raw(99), None);
    assert_eq!(RejectReason::from_raw(9), None);
    for v in 0..9u8 {
        assert_eq!(RejectReason::from_raw(v).map(|r| r as u8), Some(v));
    }
}

#[test]
fn inbound_button_actions() {
    for a in 0..=InboundAction::HaOffline as u8 {
        let x = InboundAction::from_raw(a).unwrap();
        assert_eq!(x as u8, a);
        let button = matches!(
            x,
            InboundAction::CalibrateValve
                | InboundAction::CalibrateAll
                | InboundAction::Restart
                | InboundAction::StmReset
                | InboundAction::Detect
                | InboundAction::StopAll
                | InboundAction::StmSafeExit
        );
        assert_eq!(inbound_is_button(x), button, "{a}");
    }
    assert_eq!(InboundAction::from_raw(13), None);
}

#[test]
fn decide_inbound_targets() {
    let mut inb = Inbound::new(true, MqttMode::MqttHa);
    assert_eq!(
        str(&inb.decide("VdMot/valves/1/target/set", "40")),
        "SetTarget v0 p40 clear"
    );
    assert_eq!(
        str(&inb.decide("VdMot/valves/Bad_1/target/set/set", "43,7")),
        "SetTarget v0 p44 clear"
    );
    assert_eq!(
        str(&inb.decide("VdMot/valves/8/target/set", "OPEN")),
        "SetTarget v7 p100 clear"
    );
    assert_eq!(
        str(&inb.decide("VdMot/valves/9/target/set", "50")),
        "Reject v8 p0 'inactive' clear"
    );
    assert_eq!(
        str(&inb.decide("VdMot/valves/Old/target/set", "50")),
        "Reject v254 p0 'unknown valve' clear"
    );
    assert_eq!(
        str(&inb.decide("VdMot/valves/1/target/set", "abc")),
        "Reject v0 p0 'payload' d2 clear"
    );
    assert_eq!(
        str(&inb.decide("VdMot/valves/1/target/set", "101")),
        "Reject v0 p0 'payload' d3 clear"
    );
    assert_eq!(
        str(&inb.decide("VdMot/valves/1/target/set", "STOP")),
        "StopValve v0 p0 clear"
    );
    inb.stm_v3 = false;
    assert_eq!(
        str(&inb.decide("VdMot/valves/1/target/set", "STOP")),
        "Reject v0 p0 'unsupported' clear"
    );
    // Empty payloads (a retained clear coming back) are ignored everywhere.
    for t in [
        "VdMot/valves/1/target/set",
        "VdMot/valves/9/target/set",
        "VdMot/valves/Old/target/set",
        "VdMot/cmd/restart",
        "VdMot/cmd/foo",
        "VdMot/cmd/valves/1/calibrate",
    ] {
        assert_eq!(str(&inb.decide(t, "")), "Ignore v254 p0", "{t}");
        assert_eq!(str(&inb.decide(t, " \r\n\t")), "Ignore v254 p0", "{t}");
        assert!(inb.clear_echo(t, ""), "{t}");
        assert!(inb.clear_echo(t, " \n"), "{t}");
        assert!(!inb.clear_echo(t, "x"), "{t}");
    }
    // C++ a null payload of length 5: no Rust form, the empty payload gives the same.
    let ctx = inb.ctx();
    assert_eq!(
        decide_inbound(&ctx, b"VdMot/cmd/restart", b"").action,
        InboundAction::Ignore
    );
    assert!(inbound_is_clear_echo(&ctx, b"VdMot/cmd/restart", b""));
    // Not our topics.
    assert_eq!(
        str(&inb.decide("VdMot/common/state", "1")),
        "Ignore v254 p0"
    );
    assert_eq!(
        str(&inb.decide("Other/valves/1/target/set", "1")),
        "Ignore v254 p0"
    );
    assert!(!inb.clear_echo("Other/valves/1/target/set", ""));
    assert!(!inb.clear_echo("homeassistant/status", ""));
    let none = InboundContext::default();
    assert_eq!(
        decide_inbound(&none, b"VdMot/cmd/restart", b"PRESS").action,
        InboundAction::Ignore
    );
    assert!(!inbound_is_clear_echo(&none, b"VdMot/cmd/restart", b""));
}

#[test]
fn decide_inbound_the_state_form_without_separate() {
    let mut inb = Inbound::new(false, MqttMode::MqttHa);
    assert_eq!(
        str(&inb.decide("VdMot/valves/2/target", "40")),
        "SetTarget v1 p40"
    );
    assert_eq!(
        str(&inb.decide("VdMot/valves/2/target/set", "40")),
        "SetTarget v1 p40 clear"
    );
    inb.echo.published(1, 40);
    assert_eq!(
        str(&inb.decide("VdMot/valves/2/target", "40")),
        "Ignore v1 p40"
    ); // own echo
    assert_eq!(
        str(&inb.decide("VdMot/valves/2/target", " 40.0")),
        "Ignore v1 p40"
    );
    assert_eq!(
        str(&inb.decide("VdMot/valves/2/target", "41")),
        "SetTarget v1 p41"
    );
    assert_eq!(
        str(&inb.decide("VdMot/valves/2/target/set", "40")),
        "SetTarget v1 p40 clear"
    );
    assert_eq!(
        str(&inb.decide("VdMot/valves/3/target", "40")),
        "SetTarget v2 p40"
    );
    assert_eq!(
        str(&inb.decide("VdMot/valves/9/target", "40")),
        "Reject v8 p0 'inactive'"
    );
    assert_eq!(
        str(&inb.decide("VdMot/valves/2/target", "x")),
        "Reject v1 p0 'payload' d2"
    );
    inb.with_echo = false;
    assert_eq!(
        str(&inb.decide("VdMot/valves/2/target", "40")),
        "SetTarget v1 p40"
    );
}

#[test]
fn decide_inbound_cmd_topics() {
    let mut inb = Inbound::new(true, MqttMode::MqttHa);
    let cases = [
        (
            "VdMot/cmd/valves/Bad_1/calibrate",
            "PRESS",
            "CalibrateValve v0 p0 clear",
        ),
        (
            "VdMot/cmd/valves/2/calibrate",
            "PRESS",
            "CalibrateValve v1 p0 clear",
        ),
        (
            "VdMot/cmd/valves/9/calibrate",
            "PRESS",
            "Reject v8 p0 'inactive' clear",
        ),
        (
            "VdMot/cmd/valves/Old/calibrate",
            "PRESS",
            "Reject v254 p0 'unknown valve' clear",
        ),
        (
            "VdMot/cmd/valves/1/calibrate",
            "press",
            "Reject v0 p0 'payload' clear",
        ),
        ("VdMot/cmd/calibrate", "PRESS", "CalibrateAll v254 p0 clear"),
        ("VdMot/cmd/restart", "PRESS", "Restart v254 p0 clear"),
        ("VdMot/cmd/stmReset", "PRESS", "StmReset v254 p0 clear"),
        ("VdMot/cmd/detect", "PRESS", "Detect v254 p0 clear"),
        ("VdMot/cmd/stop", "PRESS", "StopAll v254 p0 clear"),
        (
            "VdMot/cmd/stmSafeExit",
            "PRESS",
            "StmSafeExit v254 p0 clear",
        ),
        (
            "VdMot/cmd/restart",
            "press",
            "Reject v254 p0 'payload' clear",
        ),
        (
            "VdMot/cmd/foo",
            "PRESS",
            "Reject v254 p0 'unknown command' clear",
        ),
        (
            "VdMot/cmd/foo",
            "x",
            "Reject v254 p0 'unknown command' clear",
        ),
    ];
    for (t, p, want) in cases {
        assert_eq!(str(&inb.decide(t, p)), want, "{t} {p}");
    }
    inb.stm_v3 = false;
    assert_eq!(
        str(&inb.decide("VdMot/cmd/stop", "PRESS")),
        "Reject v254 p0 'unsupported' clear"
    );
    assert_eq!(
        str(&inb.decide("VdMot/cmd/stmSafeExit", "PRESS")),
        "Reject v254 p0 'unsupported' clear"
    );
    assert_eq!(
        str(&inb.decide("VdMot/cmd/stop", "x")),
        "Reject v254 p0 'payload' clear"
    );
    assert_eq!(
        str(&inb.decide("VdMot/cmd/restart", "PRESS")),
        "Restart v254 p0 clear"
    );
    // The mode does not matter for commands (Off has no subscriptions).
    inb.mode = MqttMode::Mqtt;
    assert_eq!(
        str(&inb.decide("VdMot/cmd/detect", "PRESS")),
        "Detect v254 p0 clear"
    );
}

#[test]
fn decide_inbound_ha_status() {
    let mut inb = Inbound::new(true, MqttMode::MqttHa);
    assert_eq!(
        str(&inb.decide("homeassistant/status", "online")),
        "HaOnline v254 p0"
    );
    assert_eq!(
        str(&inb.decide("homeassistant/status", "offline")),
        "HaOffline v254 p0"
    );
    assert_eq!(
        str(&inb.decide("ha/status", "offline")),
        "HaOffline v254 p0"
    );
    assert_eq!(
        str(&inb.decide("homeassistant/status", "ONLINE")),
        "Ignore v254 p0"
    );
    assert_eq!(
        str(&inb.decide("homeassistant/status", "")),
        "Ignore v254 p0"
    );
    assert_eq!(
        str(&inb.decide("homeassistant/status", "offline ")),
        "Ignore v254 p0"
    );
    // C++ a null payload of length 6: no Rust form, the empty payload gives the same.
    assert_eq!(
        decide_inbound(&inb.ctx(), b"homeassistant/status", b"").action,
        InboundAction::Ignore
    );
    inb.mode = MqttMode::Mqtt;
    assert_eq!(
        str(&inb.decide("homeassistant/status", "offline")),
        "Ignore v254 p0"
    );
    inb.mode = MqttMode::Off;
    assert_eq!(
        str(&inb.decide("homeassistant/status", "online")),
        "Ignore v254 p0"
    );
}

#[test]
fn reject_log_one_log_per_valve_and_reason_per_10_s() {
    let mut r = RejectLog::default();
    assert!(r.should_log(9, RejectReason::Inactive, 0));
    assert!(!r.should_log(9, RejectReason::Inactive, 9999));
    assert!(r.should_log(9, RejectReason::Inactive, 10000));
    assert!(!r.should_log(9, RejectReason::Inactive, 10001));
    assert!(r.should_log(9, RejectReason::Payload, 10002)); // other reason
    assert!(r.should_log(8, RejectReason::Payload, 10003)); // other valve
    assert!(!r.should_log(8, RejectReason::Payload, 10004));
    assert!(r.should_log(9, RejectReason::Inactive, 10005)); // not the last logged one any more
    let mut w = RejectLog::default();
    assert!(w.should_log(0, RejectReason::None, 0xFFFF_FF00));
    assert!(!w.should_log(0, RejectReason::None, 0xFFFF_FF00u32.wrapping_add(9999)));
    assert!(w.should_log(0, RejectReason::None, 0xFFFF_FF00u32.wrapping_add(10000)));
    assert_eq!(RejectLog::REPEAT_MS, 10000);
}

#[test]
fn button_gate_hold_confirm_by_the_empty_echo_expire() {
    const CONFIRM: u32 = ButtonGate::CONFIRM_MS;
    let mut g = ButtonGate::default();
    assert_eq!(g.confirm(b"VdMot/cmd/restart"), None);
    assert!(g.hold(
        &decision(InboundAction::Restart, NO_VALVE),
        b"VdMot/cmd/restart",
        100
    ));
    assert!(g.hold(
        &decision(InboundAction::CalibrateValve, 3),
        b"VdMot/cmd/valves/4/calibrate",
        200
    ));
    assert_eq!(g.confirm(b"VdMot/cmd/restar"), None);
    assert_eq!(g.confirm(b"VdMot/cmd/detect"), None);
    // C++ confirm(nullptr, 17, out): no Rust form.
    assert_eq!(g.expire(100 + CONFIRM - 1), None);
    let out = g.confirm(b"VdMot/cmd/valves/4/calibrate").unwrap();
    assert_eq!(out.action, InboundAction::CalibrateValve);
    assert_eq!(out.valve, 3);
    assert_eq!(g.confirm(b"VdMot/cmd/valves/4/calibrate"), None); // taken
    let out = g.expire(100 + CONFIRM).unwrap();
    assert_eq!(out.action, InboundAction::Restart);
    assert_eq!(g.expire(100000), None);
    // Four slots; reset drops them.
    for _ in 0..4 {
        assert!(g.hold(&decision(InboundAction::Detect, NO_VALVE), b"t", 0));
    }
    assert!(!g.hold(&decision(InboundAction::Detect, NO_VALVE), b"t", 0));
    g.reset();
    assert_eq!(g.confirm(b"t"), None);
    assert!(g.hold(&decision(InboundAction::Detect, NO_VALVE), b"t", 0));
    // A topic longer than TOPIC_MAX is not held; exactly TOPIC_MAX is.
    let longest = [b'a'; TOPIC_MAX + 1];
    assert!(g.hold(
        &decision(InboundAction::Detect, NO_VALVE),
        &longest[..TOPIC_MAX],
        0
    ));
    assert!(g.confirm(&longest[..TOPIC_MAX]).is_some());
    assert!(!g.hold(&decision(InboundAction::Detect, NO_VALVE), &longest, 0));
    // C++ hold(.., nullptr, 1, 0): no Rust form.
    // Same topic twice: confirmed one by one.
    let mut h = ButtonGate::default();
    assert!(h.hold(&decision(InboundAction::Detect, NO_VALVE), b"x", 0));
    assert!(h.hold(&decision(InboundAction::Restart, NO_VALVE), b"x", 1));
    assert_eq!(
        h.confirm(b"x").map(|d| d.action),
        Some(InboundAction::Detect)
    );
    assert_eq!(
        h.confirm(b"x").map(|d| d.action),
        Some(InboundAction::Restart)
    );
    // Expire across a millis() wrap.
    let mut w = ButtonGate::default();
    assert!(w.hold(
        &decision(InboundAction::Detect, NO_VALVE),
        b"x",
        0xFFFF_F000
    ));
    assert_eq!(w.expire(0xFFFF_F000u32.wrapping_add(4999)), None);
    assert!(w.expire(0xFFFF_F000u32.wrapping_add(5000)).is_some());
    assert_eq!(ButtonGate::SLOTS, 4);
    assert_eq!(CONFIRM, 5000);
}

#[test]
fn button_gate_confirm_takes_the_lowest_slot_also_after_a_slot_was_reused() {
    // The C++ header promises the oldest held action; the C++ takes the lowest slot (kept,
    // docs/rust/PORT-NOTES.md).
    let mut g = ButtonGate::default();
    assert!(g.hold(&decision(InboundAction::Detect, 1), b"x", 0)); // slot 0
    assert!(g.hold(&decision(InboundAction::Detect, 2), b"x", 1)); // slot 1
    assert_eq!(g.confirm(b"x").map(|d| d.valve), Some(1)); // slot 0 is free again
    assert!(g.hold(&decision(InboundAction::Detect, 3), b"x", 2)); // slot 0
    assert_eq!(g.confirm(b"x").map(|d| d.valve), Some(3)); // not the older 2
    assert_eq!(g.confirm(b"x").map(|d| d.valve), Some(2));
    assert_eq!(g.confirm(b"x"), None);
}

#[test]
fn target_latch_the_newest_refused_target_per_valve() {
    let mut l = TargetLatch::default();
    assert_eq!(l.next(0), None); // C++ v stays 99
    l.set(3, 40);
    l.set(3, 41); // newer wins
    l.set(7, 10);
    assert!(l.pending(3));
    assert!(l.pending(7));
    assert!(!l.pending(4));
    assert!(!l.pending(12));
    assert_eq!(l.next(0), Some((3, 41)));
    assert_eq!(l.next(4), Some((7, 10)));
    assert_eq!(l.next(8).map(|(v, _)| v), Some(3)); // wraps
    assert_eq!(l.next(3).map(|(v, _)| v), Some(3));
    l.clear(3);
    assert!(!l.pending(3));
    assert_eq!(l.next(0).map(|(v, _)| v), Some(7));
    l.clear(7);
    assert_eq!(l.next(0), None);
    l.set(12, 5); // out of range: ignored
    l.clear(12);
    assert_eq!(l.next(0), None);
    l.set(11, 0);
    assert_eq!(l.next(11), Some((11, 0)));
    assert_eq!(l.next(0).map(|(v, _)| v), Some(11));
}
