//! `test_web_server__work.cpp`: the working set. The first request allocates it (four parts:
//! snapshot, config, JSON document, multipart parser) and the response buffer at its first use;
//! both are kept, a request that gets no memory is answered 503 and the next one allocates what
//! is missing. A JSON body takes a heap block of its length for its request; the lists a
//! handler builds are a transient block of the request (design deviation, PORT-NOTES: the C++
//! shared scratch cannot be retyped without unsafe).
//!
//! Retired (design 5.2): "two responses in flight get both slots, later ones reuse them" (the
//! slot pool), "a body refused for memory is never collected, also when memory returns" (503
//! `retry`), the 409 of a second body in "a client gone mid-body frees the buffer for the next
//! body", and "every body buffer goes back to the heap" (a LeakSanitizer case: a body is a `Vec`
//! of the request, freed by its owner).
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use core::mem::size_of;

use vdm_esp_core::config::{Config, CONFIG_BLOB_MAX, CONFIG_EXT_BLOB_MAX};
use vdm_esp_core::event_log::{Event, EventCode};
use vdm_esp_core::file_manager::FileEntry;
use vdm_esp_core::json_api::{SensorView, ValveView};
use vdm_esp_core::stm_types::StmSnapshot;

use super::rig::*;
use super::{MAX_BODY_SIZE, RESPONSE_SIZE};
use crate::http_parse::Multipart;
use crate::json_body::{JsonDocument, SLOT_COUNT};
use crate::logger::EVENT_CAPACITY;

/// The kept blocks in the order of their first use: the four parts and the response buffer.
fn kept() -> [usize; 5] {
    [
        size_of::<StmSnapshot>(),
        size_of::<Config>(),
        size_of::<JsonDocument>(),
        size_of::<Multipart>(),
        RESPONSE_SIZE,
    ]
}

const TARGET1: &str = "/api/valves/1/target";
const SAVE: &str = "{\"calib\":{\"hour\":4}}";

/// A station name the default config does not have: a status document shows that the config was
/// loaded into the working set. The requests keep the host of the default name.
fn rig() -> Rig {
    let rig = Rig::new();
    rig.config(|c| {
        c.valves[0].active = true;
        set_text(&mut c.station, b"Boiler");
        set_text(&mut c.web.allowed_hosts, b"vdmot.local");
    });
    rig
}

fn is_status(r: &crate::testkit::http::Response) -> bool {
    r.status == 200 && text(r).starts_with("{\"station\":\"Boiler\",")
}

fn granted(rig: &Rig) -> Vec<usize> {
    rig.dev.heap.state().granted.clone()
}

// ---------------------------------------------------------------- allocation

#[test]
fn working_set_the_first_request_allocates_it_and_the_response_buffer_both_are_kept() {
    let rig = rig();
    let st = rig.storage();
    let mut web = rig.web(&st);
    assert!(granted(&rig).is_empty()); // new() allocates nothing
    assert!(is_status(&perform(&mut web, get("/api/status"))));
    assert_eq!(granted(&rig), kept());
    rig.dev.clock.advance_ms(86_400_000); // a day without a web client
    assert!(is_status(&perform(&mut web, get("/api/status"))));
    assert_eq!(granted(&rig), kept());
    // the handlers build their lists in a block of the request, the kept blocks stay
    let valves = 12 * size_of::<ValveView<'_>>();
    let sensors = 2 * (34 + 8) * size_of::<SensorView<'_>>();
    let events = 50.min(EVENT_CAPACITY) * size_of::<Event>();
    let files = 32 * size_of::<FileEntry>();
    let mut want = kept().to_vec();
    for (url, block) in [
        ("/api/valves", Some(valves)),
        ("/api/sensors", Some(sensors)),
        ("/api/events", Some(events)),
        ("/api/health", None),
        ("/api/stm/images", None),
        ("/api/files", Some(files)),
        ("/valves", Some(valves)),
        ("/temps", Some(sensors)),
        ("/volts", Some(sensors)),
    ] {
        assert_eq!(perform(&mut web, get(url)).status, 200, "{url}");
        want.extend(block);
        assert_eq!(granted(&rig), want, "{url}");
    }
    assert!(is_status(&perform(&mut web, get("/api/status"))));
    assert_eq!(granted(&rig), want);
}

// ---------------------------------------------------------------- no memory

#[test]
fn working_set_without_memory_the_first_request_is_answered_503_the_next_allocates() {
    let rig = rig();
    let st = rig.storage();
    let mut web = rig.web(&st);
    rig.dev.heap.state().fail_all = true;
    let r = perform(&mut web, get("/api/status"));
    assert_eq!(r.status, 503);
    assert_eq!(text(&r), error_body("busy", "out of memory"));
    rig.dev.heap.state().fail_all = false;
    assert!(is_status(&perform(&mut web, get("/api/status"))));
    assert_eq!(granted(&rig), kept());
}

#[test]
fn working_set_without_memory_a_body_is_answered_503_and_never_read() {
    let rig = rig();
    let st = rig.storage();
    let mut web = rig.web(&st);
    rig.dev.heap.state().fail_all = true; // no working set
    let req = served(&mut web, api_post(TARGET1, "{\"target\":50}"));
    rig.dev.heap.state().fail_all = false;
    assert_eq!(req.response.status, 503);
    assert_eq!(text(&req.response), error_body("busy", "out of memory"));
    assert_eq!(req.body_read(), 0);
    drop(req);
    assert!(rig.submitted().is_empty());
    assert_eq!(
        perform(&mut web, api_post(TARGET1, "{\"target\":7}")).status,
        202
    );
    let s = rig.submitted();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].pos, 7);
}

#[test]
fn working_set_each_of_the_parts_that_cannot_be_allocated_answers_503() {
    for k in 0..4 {
        let rig = rig();
        let st = rig.storage();
        let mut web = rig.web(&st);
        // every part is tried; part k cannot be allocated
        let mut next = vec![true; 4];
        next[k] = false;
        rig.dev.heap.state().next.extend(next);
        let r = perform(&mut web, get("/api/status"));
        assert_eq!(r.status, 503, "part {k}");
        assert_eq!(text(&r), error_body("busy", "out of memory"));
        let parts: Vec<usize> = kept()[..4].to_vec();
        let mut want = parts.clone();
        want.remove(k);
        assert_eq!(granted(&rig), want, "part {k}");
        assert_eq!(rig.dev.heap.state().refused, [parts[k]]);
        // the next request allocates the missing part and the response buffer
        assert!(is_status(&perform(&mut web, get("/api/status"))));
        want.push(parts[k]);
        want.push(RESPONSE_SIZE);
        assert_eq!(granted(&rig), want, "part {k}");
    }
}

#[test]
fn working_set_a_response_buffer_that_cannot_be_allocated() {
    // C++ "slot buffers that cannot be allocated": one buffer, 503 "out of memory"
    let rig = rig();
    let st = rig.storage();
    let mut web = rig.web(&st);
    rig.dev
        .heap
        .state()
        .next
        .extend([true, true, true, true, false]);
    let r = perform(&mut web, get("/api/status"));
    assert_eq!(r.status, 503);
    assert_eq!(text(&r), error_body("busy", "out of memory"));
    assert!(is_status(&perform(&mut web, get("/api/status"))));
    assert_eq!(granted(&rig), kept());
}

// ---------------------------------------------------------------- bodies

#[test]
fn body_bodies_are_answered_without_the_response_buffer() {
    // C++ "bodies are answered while both response slots are busy": the small answers of the
    // bodies need no response buffer; a save does, and is refused before anything is applied
    let rig = rig();
    let st = rig.storage();
    let mut web = rig.web(&st);
    assert_eq!(
        perform(&mut web, api_post(TARGET1, "{\"target\":50}")).status,
        202
    );
    assert_eq!(
        perform(&mut web, api_post("/api/config?dryRun=1", "{\"mqtt\":{}}")).status,
        200
    );
    let legacy = post("/setvalve", "{\"valve\":1,\"value\":30}");
    assert_eq!(perform(&mut web, legacy).status, 200);
    let s = rig.submitted();
    assert_eq!(s.len(), 2);
    assert_eq!(s[1].pos, 30);
    assert!(!granted(&rig).contains(&RESPONSE_SIZE));
    let revision = rig.storage_shared.config_revision();
    rig.dev.heap.state().next.extend([true, false]); // the body, then no response buffer
    let save = perform(&mut web, api_post("/api/config", SAVE));
    assert_eq!(save.status, 503);
    assert_eq!(text(&save), error_body("busy", "out of memory"));
    assert_eq!(rig.storage_shared.config_revision(), revision);
    assert_eq!(perform(&mut web, api_post("/api/config", SAVE)).status, 200);
    assert_eq!(rig.storage_shared.config_revision(), revision + 1);
}

#[test]
fn body_the_buffer_of_a_body_has_its_length_for_its_request_only() {
    let rig = rig();
    let st = rig.storage();
    let mut web = rig.web(&st);
    assert!(is_status(&perform(&mut web, get("/api/status"))));
    let body = "{\"target\":50}";
    let before = granted(&rig).len();
    assert_eq!(perform(&mut web, api_post(TARGET1, body)).status, 202);
    let g = granted(&rig);
    assert_eq!(g.len(), before + 1);
    assert_eq!(*g.last().unwrap(), body.len());
    // a save: its body, then storage's blob buffers (no patched copy: the copy of the web is
    // patched, design 4.3); the answer goes out of the response buffer
    let r = perform(&mut web, api_post("/api/config", SAVE));
    assert_eq!(r.status, 200);
    assert!(text(&r).contains("\"station\":\"Boiler\""));
    assert!(text(&r).ends_with(",\"restartRequired\":false,\"netTrial\":false}"));
    let g = granted(&rig);
    assert_eq!(g.len(), before + 3);
    assert_eq!(g[before + 1], SAVE.len());
    assert_eq!(g[before + 2], CONFIG_BLOB_MAX + CONFIG_EXT_BLOB_MAX);
    // a body of exactly 8192 bytes is read whole (and fails to parse as a patch)
    let big = perform(
        &mut web,
        api_post("/api/config", &" ".repeat(MAX_BODY_SIZE)),
    );
    assert_eq!(big.status, 400);
    assert_eq!(*granted(&rig).last().unwrap(), MAX_BODY_SIZE);
}

#[test]
fn body_a_body_without_memory_for_its_buffer_is_refused_503_nothing_runs() {
    let rig = rig();
    let st = rig.storage();
    let mut web = rig.web(&st);
    assert!(is_status(&perform(&mut web, get("/api/status"))));
    rig.dev.heap.state().fail_all = true;
    let r = perform(&mut web, api_post(TARGET1, "{\"target\":50}"));
    assert_eq!(r.status, 503);
    assert_eq!(text(&r), error_body("busy", "out of memory"));
    let r = perform(&mut web, post("/setvalve", "{\"valve\":1,\"value\":30}"));
    assert_eq!(r.status, 503);
    assert_eq!(text(&r), error_body("busy", "out of memory"));
    rig.dev.heap.state().fail_all = false;
    assert!(rig.submitted().is_empty());
    assert_eq!(
        perform(&mut web, api_post(TARGET1, "{\"target\":50}")).status,
        202
    );
    assert_eq!(rig.submitted().len(), 1);
}

#[test]
fn body_a_client_gone_mid_body_leaves_nothing_behind() {
    let rig = rig();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let revision = rig.storage_shared.config_revision();
    let mut gone = api_post("/api/config", SAVE);
    gone.gone_at = Some(4);
    let gone = served(&mut web, gone);
    assert_eq!(gone.response.answers, 0);
    drop(gone);
    assert_eq!(rig.storage_shared.config_revision(), revision);
    assert_eq!(
        perform(&mut web, api_post(TARGET1, "{\"target\":50}")).status,
        202
    );
    assert_eq!(perform(&mut web, api_post("/api/config", SAVE)).status, 200);
}

#[test]
fn config_without_memory_for_the_blob_buffers_a_save_answers_nvs_nothing_applied() {
    // C++ "without memory for the patched copy a save answers 503": the copy is the web's own
    // (design 4.3); storage's blob buffers without memory fail like NVS
    let rig = rig();
    let st = rig.storage();
    let mut web = rig.web(&st);
    assert!(is_status(&perform(&mut web, get("/api/status"))));
    let revision = rig.storage_shared.config_revision();
    rig.dev.heap.state().next.extend([true, false]); // the body, then no blob buffers
    let r = perform(&mut web, api_post("/api/config", SAVE));
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("invalid", "nvs"));
    assert_eq!(rig.storage_shared.config_revision(), revision);
    assert!(!rig.host.has(EventCode::ConfigSaved));
    // the copy is the active config again, a dry run takes nothing but its body
    let r = perform(&mut web, get("/api/config"));
    assert!(text(&r).contains("\"hour\":0,"));
    let before = granted(&rig).len();
    let r = perform(&mut web, api_post("/api/config?dryRun=1", SAVE));
    assert_eq!(r.status, 200);
    assert_eq!(granted(&rig).len(), before + 1);
    let r = perform(&mut web, api_post("/api/config", SAVE));
    assert_eq!(r.status, 200);
    assert_eq!(rig.storage_shared.config_revision(), revision + 1);
}

// ---------------------------------------------------------------- working set

#[test]
fn working_set_the_json_document_holds_32_members() {
    let rig = rig();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let object = |members: usize| {
        let mut body = String::from("{\"target\":5");
        for i in 1..members {
            body.push_str(&format!(",\"k{i}\":1"));
        }
        body.push('}');
        body
    };
    // parsed: the extra members are refused one step later
    let r = perform(&mut web, api_post(TARGET1, &object(SLOT_COUNT)));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("out_of_range", "target 0..100"));
    let r = perform(&mut web, api_post(TARGET1, &object(SLOT_COUNT + 1)));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "NoMemory"));
}
