//! `test_web_server__mut_a.cpp`: the error buffer, the request guard, JSON input and the upload
//! limits.
//!
//! Retired (design 5.2, the AsyncWebServer and slot-pool cases): "a document whose response
//! cannot be allocated answers 500, slot freed" (the library's response object; the port sends
//! from the buffer without allocating), "bodies arriving while one is buffered are answered
//! 409, each its own" and "one mark per connection the server holds, none gives way" (marks of
//! concurrent bodies), "a refusal that no longer holds when the body ends answers retry" (503
//! `retry`); the concurrent parts of "a body to a path outside /api/ is refused, also while one
//! is buffered" and "an upload waits for every other upload and the flash". The serial server
//! has its own cases in `tests_rust.rs`.
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use vdm_esp_core::common::{parse_one_wire_id, NO_VALVE};
use vdm_esp_core::event_log::EventCode;
use vdm_esp_core::json_api::write_error_json;
use vdm_esp_core::json_writer::JsonWriter;
use vdm_esp_core::stm_codec::Breakaway;

use super::rig::*;
use super::{MAX_BODY_SIZE, MAX_STM_IMAGE_SIZE, MULTIPART_SLACK};
use crate::port::EspErr;
use crate::storage::{ImageResult, FS_RESERVE};
use crate::testkit::ota::SLOT_SIZE;

const TARGET1: &str = "/api/valves/1/target";

// ---------------------------------------------------------------- documents and errors

#[test]
fn a_document_after_another_carries_its_own() {
    // C++ "the second slot carries its own document": one buffer, written anew per request
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let s = perform(&mut web, get("/api/status"));
    let v = perform(&mut web, get("/api/valves"));
    assert_eq!(v.status, 200);
    assert!(text(&v).starts_with("{\"valves\":[{\"idx\":1,"));
    assert_eq!(s.status, 200);
    assert!(text(&s).starts_with("{\"station\":\"VdMot\","));
}

#[test]
fn an_error_answer_longer_than_its_200_byte_buffer_becomes_internal() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    // the longest path whose not_found answer fits 199 characters and the terminator
    let mut n = 0;
    for len in 1..300 {
        let mut buf = [0u8; 200];
        let mut jw = JsonWriter::new(&mut buf);
        let url = format!("/api/{}", "a".repeat(len));
        if write_error_json(&mut jw, b"not_found", Some(url.as_bytes())) {
            n = len;
        }
    }
    let fits = format!("/api/{}", "a".repeat(n));
    let r = perform(&mut web, get(&fits));
    assert_eq!(r.status, 404);
    assert_eq!(text(&r), error_body("not_found", &fits));
    assert_eq!(r.body.len(), 199);
    let r = perform(&mut web, get(&format!("{fits}a")));
    assert_eq!(r.status, 404);
    assert_eq!(text(&r), "{\"error\":\"internal\"}");
}

#[test]
fn a_body_to_a_path_outside_api_is_refused_without_being_read() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let req = served(&mut web, post("/nope", "{\"a\":1}"));
    assert_eq!(req.response.status, 405);
    assert_eq!(
        text(&req.response),
        error_body("method_not_allowed", "/nope")
    );
    assert_eq!(req.body_read(), 0);
    let mut get_body = get("/nope");
    get_body.body = b"x".to_vec();
    let req = served(&mut web, get_body);
    assert_eq!(req.response.status, 404);
    assert_eq!(text(&req.response), error_body("not_found", "/nope"));
    assert_eq!(req.body_read(), 0);
    assert_eq!(
        perform(&mut web, api_post(TARGET1, "{\"target\":5}")).status,
        202
    );
}

// ---------------------------------------------------------------- guard

#[test]
fn guard_the_longest_client_address_is_logged_in_full() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let mut refused = post(TARGET1, "{\"target\":5}");
    refused.remote_ip = 0xFFFF_FFFF;
    assert_eq!(perform(&mut web, refused).status, 403);
    let ev = rig.host.with_code(EventCode::RequestRefused);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].text.as_slice(), b"255.255.255.255");
    assert_eq!(ev[0].arg1, 3);
    assert_eq!(ev[0].arg2, 0);
    assert_eq!(ev[0].valve, NO_VALVE);
}

#[test]
fn guard_a_post_without_a_body_needs_no_content_type_a_one_byte_body_does() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let bare = post_typed("/api/system/reboot", b"", "").with_header("X-VdMot", "1");
    assert_eq!(perform(&mut web, bare).status, 202);
    let one = post_typed("/api/system/reboot", b"x", "text/plain").with_header("X-VdMot", "1");
    let r = perform(&mut web, one);
    assert_eq!(r.status, 415);
    assert_eq!(
        text(&r),
        error_body("unsupported_media_type", "application/json required")
    );
    assert_eq!(rig.host.with_code(EventCode::RebootRequested).len(), 1);
}

#[test]
fn guard_an_oversized_body_is_refused_legacy_alias_or_api() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let big = " ".repeat(MAX_BODY_SIZE + 1);
    for r in [post("/setvalve", &big), api_post("/api/config", &big)] {
        let req = served(&mut web, r);
        assert_eq!(req.response.status, 413);
        assert_eq!(text(&req.response), error_body("too_large", "body"));
        // never read, no buffer for it
        assert_eq!(req.body_read(), 0);
    }
    assert!(!rig.dev.heap.state().granted.contains(&(MAX_BODY_SIZE + 1)));
    assert!(rig.submitted().is_empty());
    // the next body is read
    assert_eq!(
        perform(&mut web, api_post(TARGET1, "{\"target\":5}")).status,
        202
    );
}

#[test]
fn guard_setvalve_reads_a_body_of_8192_bytes() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, post("/setvalve", &" ".repeat(MAX_BODY_SIZE)));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "EmptyInput"));
    assert!(rig.dev.heap.state().granted.contains(&MAX_BODY_SIZE));
}

#[test]
fn guard_a_get_alias_ignores_its_body_and_takes_at_most_8192_bytes_for_it() {
    // C++ handleBody: a buffer of the body up to 8 KB (503 without it), a longer body only sets
    // the overflow, which the documents of the GET aliases ignore
    let rig = Rig::new();
    let t_id = parse_one_wire_id(b"28-84-37-94-97-ff-03-23").unwrap();
    let u_id = parse_one_wire_id(b"26-11-22-33-44-55-66-29").unwrap();
    rig.config(|c| {
        c.temps[0].active = true;
        c.temps[0].id = t_id;
        c.volts[0].active = true;
        c.volts[0].factor = 1.0;
        c.volts[0].id = u_id;
    });
    {
        let mut st = rig.state();
        let s = &mut st.snapshot;
        s.temp_count = 1;
        s.temps[0].id = t_id;
        s.temps[0].raw = 210;
        s.temps[0].seen = true;
        s.volt_count = 1;
        s.volts[0].id = u_id;
        s.volts[0].vad = 1208;
        s.volts[0].seen = true;
    }
    let st = rig.storage();
    let mut web = rig.web(&st);
    let docs: Vec<String> = ["/valves", "/temps", "/volts"]
        .iter()
        .map(|p| text(&perform(&mut web, get(p))))
        .collect();
    assert_ne!(docs[1], docs[2]);
    for (path, doc) in ["/valves", "/temps", "/volts"].iter().zip(&docs) {
        for len in [10, MAX_BODY_SIZE, MAX_BODY_SIZE + 1, 4 * MAX_BODY_SIZE] {
            let before = rig.dev.heap.state().granted.len();
            let mut r = get(path);
            r.body = vec![b' '; len];
            let req = served(&mut web, r);
            assert_eq!(req.response.status, 200, "{path} {len}");
            assert_eq!(&text(&req.response), doc, "{path} {len}");
            let granted = rig.dev.heap.state().granted[before..].to_vec();
            assert_eq!(granted[0], len.min(MAX_BODY_SIZE), "{path} {len}");
            // a body that fits its buffer is read, a longer one is left to the server
            let read = if len > MAX_BODY_SIZE { 0 } else { len };
            assert_eq!(req.body_read(), read, "{path} {len}");
        }
    }
    // no buffer for the body: 503, nothing else runs
    for len in [10, MAX_BODY_SIZE + 1] {
        rig.dev.heap.state().next.push_back(false);
        let mut r = get("/temps");
        r.body = vec![b' '; len];
        let r = perform(&mut web, r);
        assert_eq!(r.status, 503, "{len}");
        assert_eq!(text(&r), error_body("busy", "out of memory"));
    }
    assert!(rig.submitted().is_empty());
}

#[test]
fn guard_a_multipart_body_outside_the_upload_routes_is_refused() {
    let rig = Rig::started();
    rig.dev.fs.put("/x.bin", b"legacy");
    let st = rig.storage();
    let mut web = rig.web(&st);
    let mut d = with_type(
        api_del("/api/files?path=/x.bin"),
        "multipart/form-data; boundary=b",
    );
    d.body = b"x".to_vec();
    let r = perform(&mut web, d);
    assert_eq!(r.status, 415);
    assert_eq!(
        text(&r),
        error_body("unsupported_media_type", "application/json expected")
    );
    assert!(rig.dev.fs.read("/x.bin").is_some());
    // without a body the request reaches storage
    let empty = with_type(
        api_del("/api/files?path=/x.bin"),
        "multipart/form-data; boundary=b",
    );
    assert_eq!(perform(&mut web, empty).status, 204);
    assert!(rig.dev.fs.read("/x.bin").is_none());
}

#[test]
fn guard_an_stm_image_may_be_512_kib_plus_8_kib_of_framing() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let limit = MAX_STM_IMAGE_SIZE + MULTIPART_SLACK;
    let sized = |cl: usize| {
        let mut u = api_upload("/api/stm/images", "fw.bin", b"abc");
        u.content_length = Some(cl);
        u
    };
    // storage reserves room for the Content-Length: one byte short of it is no space
    let used = st.fs_used() as usize;
    rig.dev.fs.knobs().total_bytes = (used + limit + FS_RESERVE - 1) as u32;
    let r = perform(&mut web, sized(limit));
    assert_eq!(r.status, 507);
    assert_eq!(text(&r), error_body("upload_failed", "no_space"));
    rig.dev.fs.knobs().total_bytes += 1;
    // the body ends at its close delimiter, before the Content-Length
    let r = perform(&mut web, sized(limit));
    assert_eq!(r.status, 201);
    assert_eq!(rig.dev.fs.read("/stm/fw.bin").unwrap(), b"abc");
    let req = served(&mut web, sized(limit + 1));
    assert_eq!(req.response.status, 413);
    assert_eq!(text(&req.response), error_body("too_large", "file"));
    assert_eq!(req.body_read(), 0);
}

#[test]
fn guard_an_esp_image_may_fill_the_update_slot_plus_8_kib_of_framing() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let limit = SLOT_SIZE as usize + MULTIPART_SLACK;
    let image = esp_image(32);
    let sized = |cl: usize| {
        let mut u = api_upload("/api/ota/esp", "fw.bin", &image);
        u.content_length = Some(cl);
        u
    };
    let r = perform(&mut web, sized(limit));
    assert_eq!(r.status, 200);
    assert_eq!(rig.dev.ota.knobs().begins, 1);
    let ev = rig.host.with_code(EventCode::EspOtaStarted);
    assert_eq!(ev[0].arg1, limit as i32); // the announced bytes
    let r = perform(&mut web, sized(limit + 1));
    assert_eq!(r.status, 413);
    assert_eq!(text(&r), error_body("too_large", "file"));
    // without a slot to write only the framing fits (and the update refuses: no partition)
    rig.dev.ota.knobs().no_other = true;
    let r = perform(&mut web, sized(MULTIPART_SLACK + 1));
    assert_eq!(r.status, 413);
    let r = perform(&mut web, sized(MULTIPART_SLACK));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("upload_failed", "no ota partition"));
    assert_eq!(rig.dev.ota.knobs().begins, 1);
}

#[test]
fn guard_an_upload_waits_for_every_other_upload_and_the_flash() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let busy = error_body("busy", "upload or flash running");
    let u = || api_upload("/api/stm/images", "fw.bin", b"abc");
    // an ESP upload runs (C++ ota::uploadActive)
    let mut other = rig.ota_upload();
    assert!(other.upload_begin(100, b""));
    let r = perform(&mut web, u());
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), busy);
    assert!(!other.upload_end(false));
    // an STM image upload runs (C++ storage::imageUploadActive)
    assert_eq!(st.image_upload_begin(b"other", 10), ImageResult::Ok);
    let r = perform(&mut web, u());
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), busy);
    st.image_upload_abort();
    rig.state().flash_active = true;
    let r = perform(&mut web, u());
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), busy);
    rig.state().flash_active = false;
    assert!(rig.dev.fs.read("/stm/fw.bin").is_none());
    assert_eq!(perform(&mut web, u()).status, 201);
}

// ---------------------------------------------------------------- uploads

#[test]
fn upload_the_longest_error_of_the_update_is_answered_whole() {
    // C++ "the error of a failed upload is kept up to 47 characters": the texts are static (the
    // longest has 31 characters, PORT-NOTES ota), so the 48-byte cut has no Rust form
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    rig.dev.ota.knobs().finish_err = Some(EspErr::OTA_VALIDATE_FAILED);
    let r = perform(
        &mut web,
        api_upload("/api/ota/esp", "fw.bin", &esp_image(16)),
    );
    assert_eq!(r.status, 500);
    assert_eq!(
        text(&r),
        error_body("upload_failed", "Could Not Activate The Firmware")
    );
}

#[test]
fn upload_a_file_part_without_its_end_is_an_incomplete_file() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let mut u = api_upload("/api/stm/images", "fw.bin", &[b'd'; 2000]);
    u.content_length = Some(u.body.len() - 12); // the closing delimiter never arrives
    let r = perform(&mut web, u);
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("upload_failed", "incomplete file"));
    // what arrived before the end was written, then the upload was aborted: no part, no image
    assert!(rig.dev.fs.knobs().bytes_written >= 1000);
    assert!(rig.dev.fs.read("/stm/fw.bin.part").is_none());
    assert!(rig.dev.fs.read("/stm/fw.bin").is_none());
    assert!(st.find_image(b"fw").is_none());
    assert!(!st.image_upload_active());
}

// ---------------------------------------------------------------- JSON input

#[test]
fn json_a_request_without_a_body_never_sees_the_previous_one() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    assert_eq!(
        perform(&mut web, api_post(TARGET1, "{\"target\":5}")).status,
        202
    );
    let r = perform(&mut web, api_post(TARGET1, ""));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "JSON body required"));
    let r = perform(&mut web, api_post(TARGET1, "7"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "object expected"));
    let r = perform(&mut web, api_post(TARGET1, "{"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "IncompleteInput"));
    assert_eq!(rig.submitted().len(), 1);
}

#[test]
fn json_optional_members_strict_types_and_the_upper_bound() {
    let rig = Rig::started();
    {
        let mut s = rig.state();
        s.proto = 2;
        s.snapshot.have_breakaway = true;
        s.snapshot.breakaway = Breakaway {
            enable: false,
            step_pct: 10,
            max_ma: 30,
        };
    }
    let st = rig.storage();
    let mut web = rig.web(&st);
    let motor = "/api/stm/motor";
    let r = perform(
        &mut web,
        api_post(motor, "{\"breakaway\":{\"enable\":true}}"),
    );
    assert_eq!(r.status, 202);
    let s = rig.submitted();
    assert_eq!(s.len(), 1);
    assert!(s[0].has_breakaway);
    assert!(s[0].breakaway.enable);
    assert_eq!(s[0].breakaway.step_pct, 10);
    assert_eq!(s[0].breakaway.max_ma, 30);
    let r = perform(&mut web, api_post(motor, "{\"breakaway\":{\"enable\":1}}"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("out_of_range", "breakaway"));
    let learn = error_body("out_of_range", "learnMovements 0 or 50..65534");
    let r = perform(&mut web, api_post(motor, "{\"learnMovements\":\"100\"}"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), learn);
    let r = perform(&mut web, api_post(motor, "{\"learnMovements\":65535}"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), learn);
    let r = perform(&mut web, api_post(motor, "{\"learnMovements\":65534}"));
    assert_eq!(r.status, 202);
    let s = rig.submitted();
    assert_eq!(s.len(), 2);
    assert!(s[1].has_learn_movements);
    assert_eq!(s[1].learn_movements, 65534);
}

// ---------------------------------------------------------------- hostname

#[test]
fn guard_a_station_name_of_20_characters_is_the_whole_hostname() {
    let rig = Rig::new();
    rig.config(|c| set_text(&mut c.station, b"Abcdefghijklmnopqrst"));
    let st = rig.storage();
    let mut web = rig.web(&st);
    let full = with_host(get("/api/status"), "abcdefghijklmnopqrst.local");
    let r = perform(&mut web, full);
    assert_eq!(r.status, 200);
    // the status document shows it whole too
    assert!(text(&r).contains("\"hostname\":\"Abcdefghijklmnopqrst\","));
    let cut = with_host(get("/api/status"), "abcdefghijklmnopqrs.local");
    assert_eq!(perform(&mut web, cut).status, 403);
}
