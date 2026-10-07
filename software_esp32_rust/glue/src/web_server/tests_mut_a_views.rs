//! `test_web_server__mut_a_views.cpp`: the documents (sensors, valves, status, events, profile,
//! motor, flash status, the image list, health, files), each compared with the document the
//! core writes for the views the glue is expected to build.
//!
//! Retired (design 5.2): "web events: busy slots answer 503" (the slot pool; the list block
//! without memory has its case below), the slot parts of "a document larger than a response
//! slot answers 500, slot freed"; "fakes::ota().hasRunning = false" of the status case (the
//! running slot always exists on the port).
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use vdm_esp_core::common::{
    OneWireId, ITEM_NAME_MAX, NO_VALVE, TEMP_SLOT_COUNT, TEMP_UNASSIGNED, VAD_FAILED,
    VOLT_SLOT_COUNT,
};
use vdm_esp_core::event_log::{make_event, EventCode, EventFilter, Severity, EVENT_TEXT_MAX};
use vdm_esp_core::json_api::{
    write_events_json, write_flash_status_json, write_health_json, write_motor_json,
    write_profile_json, write_sensors_json, write_valves_json, SensorView, ValveView,
};
use vdm_esp_core::json_writer::JsonWriter;
use vdm_esp_core::stm_codec::{Breakaway, Profile, ProfileSample};
use vdm_esp_core::stm_flasher::{flash_error_name, FlashError, FlashPhase};

use super::rig::*;
use super::RESPONSE_SIZE;

/// Writes a document with `write` into a buffer large enough for any of them.
fn doc(write: impl FnOnce(&mut JsonWriter<'_>) -> bool) -> String {
    let mut buf = vec![0u8; 64 * 1024];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write(&mut jw));
    assert!(jw.complete());
    String::from_utf8_lossy(jw.as_bytes()).into_owned()
}

fn view<'a>(slot: u8, name: &'a [u8], active: bool, id: OneWireId) -> SensorView<'a> {
    SensorView {
        slot,
        name,
        active,
        id,
        ..SensorView::default()
    }
}

fn seen(mut v: SensorView<'_>, raw: i32, value: i32, valid: bool, age_s: u32) -> SensorView<'_> {
    v.on_bus = true;
    v.raw = raw;
    v.value = value;
    v.valid = valid;
    v.age_s = age_s;
    v
}

fn sensors_doc(t: &[SensorView<'_>], v: &[SensorView<'_>]) -> String {
    doc(|jw| write_sensors_json(jw, t, v))
}

const NOW: u64 = 200_000;
const NOW32: u32 = NOW as u32;

fn temp(rig: &Rig, b: usize, id: OneWireId, raw: i16, is_seen: bool, last_seen_ms: u32) {
    let r = &mut rig.state().snapshot.temps[b];
    r.id = id;
    r.raw = raw;
    r.seen = is_seen;
    r.last_seen_ms = last_seen_ms;
}

fn volt(rig: &Rig, b: usize, id: OneWireId, vad: i32, is_seen: bool, last_seen_ms: u32) {
    let r = &mut rig.state().snapshot.volts[b];
    r.id = id;
    r.vad = vad;
    r.seen = is_seen;
    r.last_seen_ms = last_seen_ms;
}

// ---------------------------------------------------------------- sensors

#[test]
fn sensors_temperature_slots_bus_sensors_without_a_slot_staleness_and_age() {
    let rig = Rig::new();
    rig.dev.clock.set_ms(NOW);
    rig.config(|c| {
        let t = &mut c.temps;
        set_text(&mut t[0].name, b"A"); // a name only
        t[1].active = true; // active only
        t[2].id = wid(0x28, 3); // an id only
        set_text(&mut t[4].name, b"Hall");
        t[4].active = true;
        t[4].id = wid(0x28, 5);
        t[4].offset = -3;
        set_text(&mut t[5].name, b"Unseen");
        t[5].id = wid(0x28, 6);
        set_text(&mut t[6].name, b"Stale");
        t[6].id = wid(0x28, 7);
        set_text(&mut t[7].name, b"Sec");
        t[7].id = wid(0x28, 8);
        t[7].offset = 2;
    });
    temp(&rig, 0, wid(0x28, 101), 205, true, NOW32 - 1000);
    temp(&rig, 1, wid(0x28, 3), 215, true, NOW32 - 60000);
    temp(&rig, 2, wid(0x28, 5), 850, true, NOW32); // not a temperature
    temp(&rig, 3, wid(0x28, 6), 200, false, NOW32);
    temp(&rig, 4, wid(0x28, 7), 100, true, NOW32 - 60001);
    temp(&rig, 5, OneWireId::default(), 190, true, NOW32); // an index without an id
    temp(&rig, 6, wid(0x28, 8), 100, true, NOW32 - 999);
    temp(&rig, 7, wid(0x28, 102), 300, true, NOW32 - 999);
    temp(&rig, 8, wid(0x28, 103), 250, false, NOW32);
    temp(&rig, 9, wid(0x28, 104), 850, true, NOW32);
    temp(&rig, 10, wid(0x28, 105), 260, true, NOW32 - 60001);
    temp(&rig, 11, wid(0x28, 106), 270, true, NOW32 - 60000);
    {
        let mut s = rig.state();
        s.snapshot.temp_count = 12;
        s.snapshot.valves[0].sensor_slot[1] = 3;
        s.snapshot.valves[4].sensor_slot[0] = 5;
    }
    let st = rig.storage();
    let mut web = rig.web(&st);
    let zero = OneWireId::default();
    let mut t = vec![
        view(1, b"A", false, zero),
        view(2, b"", true, zero),
        seen(view(3, b"", false, wid(0x28, 3)), 215, 215, true, 60),
        seen(view(5, b"Hall", true, wid(0x28, 5)), 850, 847, false, 0),
        seen(view(6, b"Unseen", false, wid(0x28, 6)), 200, 200, false, 0),
        seen(view(7, b"Stale", false, wid(0x28, 7)), 100, 100, false, 60),
        seen(view(8, b"Sec", false, wid(0x28, 8)), 100, 102, true, 0),
        seen(view(0, b"", false, wid(0x28, 101)), 205, 205, true, 1),
        seen(view(0, b"", false, wid(0x28, 102)), 300, 300, true, 0),
        seen(view(0, b"", false, wid(0x28, 103)), 250, 250, false, 0),
        seen(view(0, b"", false, wid(0x28, 104)), 850, 850, false, 0),
        seen(view(0, b"", false, wid(0x28, 105)), 260, 260, false, 60),
        seen(view(0, b"", false, wid(0x28, 106)), 270, 270, true, 60),
    ];
    t[2].valve = 0;
    t[3].valve = 4;
    let r = perform(&mut web, get("/api/sensors"));
    assert_eq!(r.status, 200);
    assert_eq!(r.content_type, "application/json");
    assert_eq!(text(&r), sensors_doc(&t, &[]));
}

#[test]
fn sensors_voltage_slots_conversion_clamping_and_bus_sensors_without_a_slot() {
    let rig = Rig::new();
    rig.dev.clock.set_ms(NOW);
    rig.config(|c| {
        let v = &mut c.volts;
        set_text(&mut v[0].name, b"V"); // a name only
        v[1].active = true; // active only
        v[2].id = wid(0x26, 3); // an id only
        v[2].offset = 0.5;
        v[2].factor = 2.0;
        set_text(&mut v[3].name, b"Big");
        set_text(&mut v[3].unit, b"V");
        v[3].active = true;
        v[3].id = wid(0x26, 4);
        v[3].offset = 1000.0;
        v[3].factor = 1000.0;
        set_text(&mut v[4].name, b"Neg");
        v[4].id = wid(0x26, 5);
        v[4].offset = 1000.0;
        v[4].factor = -1000.0;
        set_text(&mut v[5].name, b"Off");
        v[5].id = wid(0x26, 6);
        set_text(&mut v[6].name, b"Old");
        v[6].id = wid(0x26, 7);
        set_text(&mut v[7].name, b"Fail");
        v[7].id = wid(0x26, 9);
    });
    volt(&rig, 0, wid(0x26, 101), 700, true, NOW32 - 1000);
    volt(&rig, 1, wid(0x26, 3), 1200, true, NOW32 - 60000);
    volt(&rig, 2, wid(0x26, 4), 150_000, true, NOW32);
    volt(&rig, 3, wid(0x26, 5), 150_000, true, NOW32);
    volt(&rig, 4, wid(0x26, 6), 500, false, NOW32);
    volt(&rig, 5, wid(0x26, 7), 500, true, NOW32 - 60001);
    volt(&rig, 6, wid(0x26, 9), VAD_FAILED, true, NOW32 - 999);
    volt(&rig, 7, wid(0x26, 102), 800, true, NOW32 - 999);
    rig.state().snapshot.volt_count = 8;
    let st = rig.storage();
    let mut web = rig.web(&st);
    let zero = OneWireId::default();
    let mut v = vec![
        view(1, b"V", false, zero),
        view(2, b"", true, zero),
        seen(view(3, b"", false, wid(0x26, 3)), 1200, 25000, true, 60),
        seen(
            view(4, b"Big", true, wid(0x26, 4)),
            150_000,
            2_000_000_000,
            true,
            0,
        ),
        seen(
            view(5, b"Neg", false, wid(0x26, 5)),
            150_000,
            -2_000_000_000,
            true,
            0,
        ),
        seen(view(6, b"Off", false, wid(0x26, 6)), 500, 5000, false, 0),
        seen(view(7, b"Old", false, wid(0x26, 7)), 500, 5000, false, 60),
        seen(
            view(8, b"Fail", false, wid(0x26, 9)),
            VAD_FAILED,
            -10000,
            false,
            0,
        ),
        seen(view(0, b"", false, wid(0x26, 101)), 700, 0, false, 1),
        seen(view(0, b"", false, wid(0x26, 102)), 800, 0, false, 0),
    ];
    v[3].unit = b"V";
    let r = perform(&mut web, get("/api/sensors"));
    assert_eq!(r.status, 200);
    assert_eq!(text(&r), sensors_doc(&[], &v));
    // an unseen bus sensor without a slot has no age
    rig.state().snapshot.volts[7].seen = false;
    v[9].age_s = 0;
    rig.state().snapshot.volts[0].seen = false;
    v[8].age_s = 0;
    let r = perform(&mut web, get("/api/sensors"));
    assert_eq!(text(&r), sensors_doc(&[], &v));
}

#[test]
fn sensors_readings_past_the_bus_count_are_ignored() {
    let rig = Rig::new();
    rig.dev.clock.set_ms(NOW);
    rig.config(|c| {
        set_text(&mut c.temps[0].name, b"T");
        c.temps[0].id = wid(0x28, 1);
        set_text(&mut c.volts[0].name, b"U");
        c.volts[0].id = wid(0x26, 1);
    });
    temp(&rig, 0, wid(0x28, 50), 200, true, NOW32);
    temp(&rig, 1, wid(0x28, 1), 210, true, NOW32); // left over from a longer list
    volt(&rig, 0, wid(0x26, 50), 300, true, NOW32);
    volt(&rig, 1, wid(0x26, 1), 310, true, NOW32);
    {
        let mut s = rig.state();
        s.snapshot.temp_count = 1;
        s.snapshot.volt_count = 1;
    }
    let st = rig.storage();
    let mut web = rig.web(&st);
    let t = [
        view(1, b"T", false, wid(0x28, 1)),
        seen(view(0, b"", false, wid(0x28, 50)), 200, 200, true, 0),
    ];
    let v = [
        view(1, b"U", false, wid(0x26, 1)),
        seen(view(0, b"", false, wid(0x26, 50)), 300, 0, false, 0),
    ];
    assert_eq!(
        text(&perform(&mut web, get("/api/sensors"))),
        sensors_doc(&t, &v)
    );
    // left-over readings of sensors without a slot are not listed either
    temp(&rig, 1, wid(0x28, 51), 220, true, NOW32);
    volt(&rig, 1, wid(0x26, 51), 320, true, NOW32);
    assert_eq!(
        text(&perform(&mut web, get("/api/sensors"))),
        sensors_doc(&t, &v)
    );
}

#[test]
fn sensors_a_document_larger_than_the_response_buffer_answers_500() {
    let rig = Rig::new();
    rig.dev.clock.set_ms(NOW);
    // every slot and every bus position in use, the names escaped to six times their length
    rig.config(|c| {
        for (i, t) in (0u8..).zip(c.temps.iter_mut()) {
            set_text(&mut t.name, &[1u8; ITEM_NAME_MAX]);
            t.active = true;
            t.id = wid(0x28, i + 1);
        }
        for (i, v) in (0u8..).zip(c.volts.iter_mut()) {
            set_text(
                &mut v.name,
                format!("Volt{}", 100_000 + u32::from(i)).as_bytes(),
            );
            v.id = wid(0x26, i + 1);
        }
    });
    for i in 0..TEMP_SLOT_COUNT {
        temp(
            &rig,
            usize::from(i),
            wid(0x28, i + 101),
            -125,
            true,
            NOW32.wrapping_sub(1_000_000),
        );
    }
    for i in 0..VOLT_SLOT_COUNT {
        volt(
            &rig,
            usize::from(i),
            wid(0x26, i + 101),
            -999,
            true,
            NOW32.wrapping_sub(1_000_000),
        );
    }
    {
        let mut s = rig.state();
        s.snapshot.temp_count = TEMP_SLOT_COUNT;
        s.snapshot.volt_count = VOLT_SLOT_COUNT;
    }
    let cfg = rig.active();
    let mut t = Vec::new();
    let mut v = Vec::new();
    for (i, c) in (1u8..).zip(cfg.temps.iter()) {
        t.push(view(i, &c.name, true, c.id));
    }
    for i in 0..TEMP_SLOT_COUNT {
        t.push(seen(
            view(0, b"", false, wid(0x28, i + 101)),
            -125,
            -125,
            false,
            1000,
        ));
    }
    for (i, c) in (1u8..).zip(cfg.volts.iter()) {
        v.push(view(i, &c.name, false, c.id));
    }
    for i in 0..VOLT_SLOT_COUNT {
        v.push(seen(
            view(0, b"", false, wid(0x26, i + 101)),
            -999,
            0,
            false,
            1000,
        ));
    }
    assert!(sensors_doc(&t, &v).len() >= RESPONSE_SIZE);
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/sensors"));
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("internal", "document too large"));
    // the buffer serves the next document
    assert_eq!(perform(&mut web, get("/api/status")).status, 200);
}

// ---------------------------------------------------------------- valves

#[test]
fn valves_the_sensors_of_every_valve_with_name_offset_and_validity() {
    let rig = Rig::new();
    rig.dev.clock.set_ms(NOW);
    rig.config(|c| {
        set_text(&mut c.temps[0].name, b"One");
        c.temps[0].offset = 1;
        set_text(&mut c.temps[2].name, b"Three");
        c.temps[2].offset = -2;
        set_text(&mut c.temps[33].name, b"Last");
    });
    {
        let mut st = rig.state();
        let s = &mut st.snapshot;
        s.valves[1].sensor_slot[0] = 1;
        s.valves[1].temp1 = 215;
        s.valves[1].health = 0x0303; // flags next to the two sensor positions
        s.valves[2].sensor_slot[1] = 34;
        s.valves[2].temp2 = 190;
        s.valves[3].sensor_slot[0] = 35; // no such slot
        s.valves[3].temp1 = 100;
        s.valves[4].sensor_slot[0] = 3;
        s.valves[4].temp1 = TEMP_UNASSIGNED;
        s.valves[4].sensor_slot[1] = 1;
        s.valves[4].temp2 = -45;
    }
    let snap = rig.state().snapshot.clone();
    let cfg = rig.active();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let mut views: Vec<ValveView<'_>> = snap
        .valves
        .iter()
        .zip(cfg.valves.iter())
        .map(|(s, c)| ValveView {
            state: Some(s),
            config: Some(c),
            ..ValveView::default()
        })
        .collect();
    views[1].sensor_slot[0] = 1;
    views[1].sensor_name[0] = b"One";
    views[1].sensor_valid[0] = true;
    views[1].sensor_tenths[0] = 216;
    views[2].sensor_slot[1] = 34;
    views[2].sensor_name[1] = b"Last";
    views[2].sensor_valid[1] = true;
    views[2].sensor_tenths[1] = 190;
    views[4].sensor_slot[0] = 3;
    views[4].sensor_name[0] = b"Three";
    views[4].sensor_valid[0] = false;
    views[4].sensor_tenths[0] = i32::from(TEMP_UNASSIGNED) - 2;
    views[4].sensor_slot[1] = 1;
    views[4].sensor_name[1] = b"One";
    views[4].sensor_valid[1] = true;
    views[4].sensor_tenths[1] = -44;
    let r = perform(&mut web, get("/api/valves"));
    assert_eq!(r.status, 200);
    assert_eq!(text(&r), doc(|jw| write_valves_json(jw, &views, NOW32)));
}

// ---------------------------------------------------------------- status

#[test]
fn status_flash_size_calibration_activity_and_the_next_slot_at_epoch_1() {
    let rig = Rig::new();
    rig.state().snapshot.valves[5].calibrating = true;
    rig.state().calib.next_epoch = 1;
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = text(&perform(&mut web, get("/api/status")));
    assert!(
        r.contains("\"flash\":{\"used\":1091984,\"size\":1310720}"),
        "{r}"
    );
    assert!(r.contains("\"calibration\":{\"active\":true,"));
    // the fake wall clock runs UTC (the C++ test machine's localtime_r)
    assert!(r.contains("\"next\":\"1970-01-01T00:00:01\"}"));
    rig.state().snapshot.valves[5].calibrating = false;
    rig.dev.ota.knobs().image_size = 1_000_000;
    let r = text(&perform(&mut web, get("/api/status")));
    assert!(r.contains("\"flash\":{\"used\":1000000,\"size\":1310720}"));
    assert!(r.contains("\"calibration\":{\"active\":false,"));
}

#[test]
fn status_names_the_newest_event() {
    let rig = Rig::new();
    log_events(&rig, 3, 0, Severity::Info, b"");
    let seq = rig.host.log.last_seq();
    assert!(seq >= 3);
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = text(&perform(&mut web, get("/api/status")));
    assert!(r.contains(&format!("\"lastEventSeq\":{seq},")), "{r}");
}

// ---------------------------------------------------------------- events

fn log_events(rig: &Rig, n: i32, valve: u8, sev: Severity, text: &[u8]) {
    for i in 0..n {
        rig.host
            .log_event(&make_event(EventCode::ConfigSaved, sev, valve, i, 0, text));
    }
}

fn event_count(body: &str) -> usize {
    count_of(body, "{\"seq\":")
}

#[test]
fn events_default_and_maximum_count_parameters_and_their_errors() {
    let rig = Rig::new();
    log_events(&rig, 30, 0, Severity::Info, b"");
    log_events(&rig, 20, 1, Severity::Error, b"");
    log_events(&rig, 10, NO_VALVE, Severity::Warning, b"");
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/events"));
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Cache-Control"), "no-store");
    let body = text(&r);
    assert_eq!(event_count(&body), 50);
    assert!(body
        .starts_with("{\"first\":1,\"last\":60,\"next\":50,\"dropped\":0,\"events\":[{\"seq\":1,"));
    assert_eq!(
        event_count(&text(&perform(&mut web, get("/api/events?limit=50")))),
        50
    );
    let r = perform(&mut web, get("/api/events?limit=1"));
    assert_eq!(r.status, 200);
    assert_eq!(event_count(&text(&r)), 1);
    assert!(text(&r).contains("\"next\":1,"));
    let r = text(&perform(&mut web, get("/api/events?since=55")));
    assert_eq!(event_count(&r), 5);
    assert!(r.contains("\"events\":[{\"seq\":56,"));
    // valve 2 is index 1: its events only (the system events belong to no valve)
    let r = perform(&mut web, get("/api/events?valve=2"));
    assert_eq!(r.status, 200);
    assert_eq!(event_count(&text(&r)), 20);
    assert!(text(&r).contains("\"events\":[{\"seq\":31,"));
    let r = text(&perform(&mut web, get("/api/events?minSeverity=error")));
    assert_eq!(event_count(&r), 20);
    assert!(r.contains("\"events\":[{\"seq\":31,"));
    let params = error_body("bad_request", "since/limit/valve");
    for bad in [
        "/api/events?since=x",
        "/api/events?limit=51",
        "/api/events?limit=0",
        "/api/events?valve=13",
        "/api/events?since=x&limit=5&valve=1",
    ] {
        let r = perform(&mut web, get(bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(text(&r), params);
    }
    let r = perform(&mut web, get("/api/events?valve=0"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "valve"));
    let r = perform(&mut web, get("/api/events?minSeverity=loud"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "minSeverity"));
}

#[test]
fn events_without_memory_for_the_list_answer_503() {
    let rig = Rig::new();
    log_events(&rig, 3, 0, Severity::Info, b"");
    let st = rig.storage();
    let mut web = rig.web(&st);
    assert_eq!(perform(&mut web, get("/api/status")).status, 200);
    rig.dev.heap.state().next.push_back(false);
    let r = perform(&mut web, get("/api/events"));
    assert_eq!(r.status, 503);
    assert_eq!(text(&r), error_body("busy", "out of memory"));
    let r = perform(&mut web, get("/api/events"));
    assert_eq!(r.status, 200);
    assert_eq!(event_count(&text(&r)), 3);
}

#[test]
fn events_a_count_that_does_not_fit_is_halved_until_it_does() {
    let rig = Rig::new();
    // events whose text is escaped six times its length
    log_events(&rig, 50, 0, Severity::Info, &[1u8; EVENT_TEXT_MAX]);
    let st = rig.storage();
    let mut web = rig.web(&st);
    // how many of them fit the response buffer
    let mut ev = vec![vdm_esp_core::event_log::Event::default(); 50];
    let read = rig.host.log.read(&EventFilter::default(), &mut ev);
    assert_eq!(read.count, 50);
    let mut fit = 0;
    for n in 1..=50 {
        let mut buf = vec![0u8; RESPONSE_SIZE];
        let mut jw = JsonWriter::new(&mut buf);
        let next = ev[n - 1].seq;
        if write_events_json(
            &mut jw,
            &ev[..n],
            read.first_seq,
            read.last_seq,
            next,
            read.dropped,
        ) && jw.complete()
        {
            fit = n;
        }
    }
    assert!(fit >= 25);
    assert!(fit < 40); // 50 and 40 do not fit, 25 and 20 do
    let r = perform(&mut web, get("/api/events"));
    assert_eq!(r.status, 200);
    assert_eq!(event_count(&text(&r)), 25);
    assert!(text(&r).contains("\"next\":25,"));
    let forty = perform(&mut web, get("/api/events?limit=40"));
    assert_eq!(forty.status, 200);
    assert_eq!(event_count(&text(&forty)), 20);
}

// ---------------------------------------------------------------- STM documents

#[test]
fn a_valve_profile_404_without_one() {
    let rig = Rig::new();
    let mut p = Profile {
        valve: 1,
        count: 2,
        ..Profile::default()
    };
    p.samples[0] = ProfileSample {
        count: 10,
        current: 120,
    };
    p.samples[1] = ProfileSample {
        count: 90,
        current: 180,
    };
    rig.state().profiles[1] = p;
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/valves/2/profile"));
    assert_eq!(r.status, 200);
    assert_eq!(r.content_type, "application/json");
    assert_eq!(text(&r), doc(|jw| write_profile_json(jw, &p)));
    let r = perform(&mut web, get("/api/valves/1/profile"));
    assert_eq!(r.status, 404);
    assert_eq!(text(&r), error_body("not_found", "no profile"));
    // one profile copied per request, the snapshot is not read
    assert_eq!(rig.state().profile_reads, 2);
    assert_eq!(rig.state().snapshot_reads, 0);
    let r = perform(&mut web, get("/api/valves/2/profile")); // after the 404: valve 2 again
    assert_eq!(r.status, 200);
    assert!(text(&r).contains("\"valve\":2,"));
}

#[test]
fn the_motor_parameters_with_and_without_breakaway() {
    let rig = Rig::new();
    {
        let mut st = rig.state();
        let s = &mut st.snapshot;
        s.have_motor = true;
        s.motor.low_factor = 22;
        s.motor.min_counts = 4000;
        s.learn_movements = 120;
        s.have_breakaway = true;
        s.breakaway = Breakaway {
            enable: true,
            step_pct: 15,
            max_ma: 40,
        };
    }
    let snap = rig.state().snapshot.clone();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/stm/motor"));
    assert_eq!(r.status, 200);
    assert_eq!(
        text(&r),
        doc(|jw| write_motor_json(jw, &snap.motor, 120, Some(&snap.breakaway), true))
    );
    rig.state().snapshot.have_breakaway = false;
    rig.state().snapshot.have_motor = false;
    let r = perform(&mut web, get("/api/stm/motor"));
    assert_eq!(r.status, 200);
    assert_eq!(
        text(&r),
        doc(|jw| write_motor_json(jw, &snap.motor, 120, None, false))
    );
}

#[test]
fn the_flash_status_names_its_image() {
    let rig = Rig::new();
    {
        let mut st = rig.state();
        st.snapshot.flash.phase = FlashPhase::Writing;
        st.snapshot.flash.percent = 40;
        set_text(&mut st.snapshot.flash_image, b"a");
    }
    let flash = rig.state().snapshot.flash.clone();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/stm/flash"));
    assert_eq!(r.status, 200);
    assert_eq!(
        text(&r),
        doc(|jw| write_flash_status_json(jw, &flash, Some(b"a"), false))
    );
    rig.state().snapshot.flash_image.clear();
    let r = perform(&mut web, get("/api/stm/flash"));
    assert_eq!(r.status, 200);
    assert_eq!(
        text(&r),
        doc(|jw| write_flash_status_json(jw, &flash, None, false))
    );
}

#[test]
fn the_image_list_shows_crc_version_check_and_board_only_when_known() {
    let rig = Rig::new();
    let good = stm_image(true, "1.4.9_Dev", "C7");
    put_images(
        &rig,
        &[
            ("fw", good.clone()),
            ("raw", bad_image()),
            ("zold", stm_image(true, "9.9.9", "")),
        ],
    );
    let st = rig.storage();
    st.service(); // fw
    st.service(); // raw; zold stays unscanned
    let mut web = rig.web(&st);
    let check = flash_error_name(FlashError::None);
    let r = perform(&mut web, get("/api/stm/images"));
    assert_eq!(r.status, 200);
    assert_eq!(
        text(&r),
        format!(
            "[{{\"name\":\"fw\",\"size\":4096,\"crc32\":\"0x{:08x}\",\"version\":\"1.4.9_Dev\",\
             \"check\":\"{check}\",\"hw\":\"C7\"}},\
             {{\"name\":\"raw\",\"size\":1024,\"crc32\":\"0x00000000\",\"version\":null,\
             \"check\":\"{}\",\"hw\":null}},\
             {{\"name\":\"zold\",\"size\":4096,\"crc32\":null,\"version\":null,\"check\":null,\
             \"hw\":null}}]",
            st.find_image(b"fw").unwrap().crc,
            flash_error_name(FlashError::ImageBadVectors)
        )
    );
    assert_eq!(
        st.find_image(b"fw").unwrap().crc,
        vdm_esp_core::config::crc32(&good, 0)
    );
}

// ---------------------------------------------------------------- health

#[test]
fn health_the_document_may_use_its_whole_1024_byte_buffer() {
    let rig = Rig::new();
    let st = rig.storage();
    let mut web = rig.web(&st);
    // the longest version whose document fits 1023 characters and the terminator
    let mut n = 0;
    for len in 1..1100 {
        rig.host.set_health_version(&"v".repeat(len));
        let h = rig.state().health;
        let mut buf = [0u8; 1024];
        let mut jw = JsonWriter::new(&mut buf);
        if write_health_json(&mut jw, &h) {
            n = len;
        }
    }
    assert!(n > 0);
    let version = "v".repeat(n);
    rig.host.set_health_version(&version);
    let r = perform(&mut web, get("/api/health"));
    assert_eq!(r.status, 200);
    assert_eq!(r.body.len(), 1023);
    assert!(text(&r).contains(&format!("\"version\":\"{version}\"")));
    rig.host.set_health_version(&"v".repeat(n + 1));
    let r = perform(&mut web, get("/api/health"));
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("internal", "health"));
}

// ---------------------------------------------------------------- files

#[test]
fn files_at_most_32_entries_more_are_reported_as_truncated() {
    let rig = Rig::new();
    for i in 0..33 {
        rig.dev.fs.put(&format!("/f{i:02}.bin"), &[0u8; 10]);
    }
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/files"));
    assert_eq!(r.status, 200);
    let body = text(&r);
    assert!(body.contains("\"truncated\":true,"));
    assert!(body.contains("{\"path\":\"/f31.bin\","));
    assert!(!body.contains("{\"path\":\"/f32.bin\","));
    assert!(st.delete_file(b"/f32.bin") == crate::storage::FileResult::Ok);
    let r = text(&perform(&mut web, get("/api/files")));
    assert!(r.contains("\"truncated\":false,"));
    assert!(r.contains("{\"path\":\"/f31.bin\","));
}
