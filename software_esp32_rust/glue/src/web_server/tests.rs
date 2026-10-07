//! `test_web_server.cpp`: routing, the request guard, static assets, uploads, the legacy aliases
//! and the 2.1 routes, through the request driver. Storage and the ESP upload are the real
//! modules over the fake ports, so a C++ check of a sibling fake's record is a check of their
//! effect here (files, NVS, the OTA slot, the log).
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use vdm_esp_core::common::{parse_one_wire_id, LocalTime, ALL_VALVES};
use vdm_esp_core::config::{crc32, Config, MqttMode, CLIENT_ID_MAX, REPAIR_TOPICS};
use vdm_esp_core::event_log::EventCode;
use vdm_esp_core::failsafe::HaStatus;
use vdm_esp_core::file_manager::{file_protect_reason, FileKind};
use vdm_esp_core::stm_types::StmCommandType;
use vdm_esp_core::valve_model::TargetSource;
use vdm_esp_core::version::{parse_version, StmSupport, Version};

use super::rig::*;
use super::{WebStart, MAX_BODY_SIZE};
use crate::mqtt_client::DiscoveryAction;
use crate::storage::{
    LoadSource, BACKUP_BASE, BACKUP_EXT, IMPORT_REPORT_FILE, KEY_CONFIG, KEY_CONFIG_EXT, NAMESPACE,
};
use crate::testkit::FakeHttpServer;

// ---------------------------------------------------------------- server start

#[test]
fn begin_the_server_starts_once() {
    // C++ "web begin: the server starts once, on port 80": the port and the handlers belong to
    // the adapter (one catch-all handler on port 80, design 4.1)
    let mut server = FakeHttpServer::default();
    let mut start = WebStart::default();
    assert!(!start.started());
    start.begin(&mut server);
    start.begin(&mut server);
    assert!(start.started());
    assert_eq!(server.starts(), 1);
}

#[test]
fn begin_a_start_that_fails_is_tried_again() {
    let mut server = FakeHttpServer::default();
    server.set_refuse(true);
    let mut start = WebStart::default();
    start.begin(&mut server);
    assert!(!start.started());
    server.set_refuse(false);
    start.begin(&mut server);
    assert!(start.started());
    start.begin(&mut server);
    assert_eq!(server.starts(), 2);
}

// ---------------------------------------------------------------- documents and routing

#[test]
fn status_answers_json_that_is_not_cached() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/status"));
    assert_eq!(r.status, 200);
    assert_eq!(r.content_type, "application/json");
    assert_eq!(r.header("Cache-Control"), "no-store");
    let body = text(&r);
    assert!(body.starts_with("{\"station\":\"VdMot\","), "{body}");
    assert!(body.ends_with('}'));
}

#[test]
fn the_dashboard_is_served_gzip_with_its_etag_304_when_unchanged() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let index = ASSETS.iter().find(|a| a.path == "/index.html").unwrap();
    let r = perform(&mut web, get("/"));
    assert_eq!(r.status, 200);
    assert_eq!(r.content_type, index.content_type);
    assert_eq!(r.header("Content-Encoding"), "gzip");
    assert_eq!(r.header("ETag"), index.etag);
    assert_eq!(r.header("Cache-Control"), "no-cache");
    assert_eq!(r.body, index.data);
    let again = get("/index.html").with_header("If-None-Match", index.etag);
    let r = perform(&mut web, again);
    assert_eq!(r.status, 304);
    assert!(r.body.is_empty());
}

#[test]
fn unknown_paths_and_methods() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/nothing"));
    assert_eq!(r.status, 404);
    assert_eq!(text(&r), error_body("not_found", "/api/nothing"));
    let r = perform(&mut web, api_del("/api/status"));
    assert_eq!(r.status, 405);
    assert_eq!(text(&r), error_body("method_not_allowed", "/api/status"));
    assert_eq!(perform(&mut web, get("/missing.html")).status, 404);
    assert_eq!(perform(&mut web, post("/", "")).status, 405);
    // the refusal: never read, never 413
    let req = served(&mut web, post("/", "{}"));
    assert_eq!(req.response.status, 405);
    assert_eq!(text(&req.response), error_body("method_not_allowed", "/"));
    assert_eq!(req.body_read(), 0);
}

#[test]
fn guard_a_json_body_over_8192_bytes_is_refused_without_being_read() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let revision = rig.storage_shared.config_revision();
    let big = " ".repeat(MAX_BODY_SIZE + 1);
    let req = served(&mut web, api_post("/api/config", &big));
    assert_eq!(req.response.status, 413);
    assert_eq!(text(&req.response), error_body("too_large", "body"));
    assert_eq!(req.body_read(), 0);
    assert_eq!(rig.storage_shared.config_revision(), revision);
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_CONFIG));
    // 8192 bytes are read (and fail to parse as a patch, not as too large)
    let r = perform(
        &mut web,
        api_post("/api/config", &" ".repeat(MAX_BODY_SIZE)),
    );
    assert_eq!(r.status, 400);
    assert!(text(&r).contains("\"error\":\"invalid\""));
}

#[test]
fn guard_uploads_need_multipart_and_a_length() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_post("/api/stm/images", "x"));
    assert_eq!(r.status, 415);
    assert_eq!(
        text(&r),
        error_body("unsupported_media_type", "multipart/form-data required")
    );
    let mut u = api_upload("/api/stm/images", "fw.bin", b"abc");
    u.content_length = Some(0);
    let r = perform(&mut web, u);
    assert_eq!(r.status, 411);
    assert_eq!(text(&r), error_body("length_required", "Content-Length"));
}

#[test]
fn a_valve_target_is_submitted_and_answered_202() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(
        &mut web,
        api_post("/api/valves/1/target", "{\"target\":50}"),
    );
    assert_eq!(r.status, 202);
    assert_eq!(text(&r), "{\"valve\":1,\"target\":50}");
    let s = rig.submitted();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].kind, StmCommandType::SetTarget);
    assert_eq!(s[0].valve, 0);
    assert_eq!(s[0].pos, 50);
    assert_eq!(s[0].source, TargetSource::Web);
}

#[test]
fn a_target_out_of_range_an_inactive_valve_a_full_queue() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(
        &mut web,
        api_post("/api/valves/1/target", "{\"target\":101}"),
    );
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("out_of_range", "target 0..100"));
    let r = perform(&mut web, api_post("/api/valves/2/target", "{\"target\":1}"));
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("inactive", "valve not active"));
    rig.state().submit_result = false;
    let r = perform(&mut web, api_post("/api/valves/1/target", "{\"target\":1}"));
    assert_eq!(r.status, 503);
    assert_eq!(text(&r), error_body("queue_full", "STM command queue full"));
}

#[test]
fn no_request_needs_credentials_an_authorization_header_is_ignored() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let p = api_post("/api/valves/1/target", "{\"target\":5}")
        .with_header("Authorization", "Basic YWRtaW46d3Jvbmc="); // admin:wrong
    let r = perform(&mut web, p);
    assert_eq!(r.status, 202);
    assert_eq!(r.header("WWW-Authenticate"), "");
    assert_eq!(rig.submitted().len(), 1);
    let u = api_upload("/api/stm/images", "fw.bin", b"abc")
        .with_header("Authorization", "Basic YWRtaW46d3Jvbmc=");
    assert_eq!(perform(&mut web, u).status, 201);
    assert_eq!(rig.dev.fs.read("/stm/fw.bin").unwrap(), b"abc");
    let r = perform(&mut web, post("/setvalve", "{\"valve\":1,\"value\":1}"));
    assert_eq!(r.status, 200);
    assert!(!rig.host.has(EventCode::AuthFailed));
}

#[test]
fn reboot_and_mqtt_reconnect_are_handed_to_their_modules() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    rig.dev.clock.set_ms(5000);
    let r = perform(&mut web, api_post("/api/system/reboot", ""));
    assert_eq!(r.status, 202);
    assert_eq!(text(&r), "{\"result\":\"restarting\"}");
    assert!(rig.ota_shared.restart_pending());
    let ev = rig.host.with_code(EventCode::RebootRequested);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 0);
    assert_restart_due(&rig, 5000, 1000);
    let r = perform(&mut web, api_post("/api/mqtt/reconnect", ""));
    assert_eq!(r.status, 202);
    assert_eq!(text(&r), "{\"result\":\"queued\"}");
    assert_eq!(rig.state().reconnects, 1);
}

#[test]
fn requests_one_after_another_each_get_the_response_buffer() {
    // C++ "two responses in flight use both slots, a third request gets 503": the slot pool is
    // gone (one buffer, requests served one after another, design 4.7 row 1)
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    for _ in 0..3 {
        let r = perform(&mut web, get("/api/status"));
        assert_eq!(r.status, 200);
        assert!(text(&r).starts_with("{\"station\":\"VdMot\","));
    }
}

#[test]
fn upload_an_stm_image_is_stored_and_described() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_upload("/api/stm/images", "fw.bin", b"abc"));
    assert_eq!(r.status, 201);
    assert_eq!(r.content_type, "application/json");
    assert_eq!(rig.dev.fs.read("/stm/fw.bin").unwrap(), b"abc");
    assert_eq!(
        text(&r),
        format!(
            "{{\"name\":\"fw\",\"size\":3,\"crc32\":\"0x{:08x}\",\"version\":null,\"check\":null,\"hw\":null}}",
            crc32(b"abc", 0)
        )
    );
    let e = st.find_image(b"fw").unwrap();
    assert_eq!(e.size, 3);
    assert!(!st.image_upload_active());
}

#[test]
fn upload_an_esp_image_goes_to_ota_and_asks_for_the_restart() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let image = esp_image(64);
    let url = format!("/api/ota/esp?md5={}", md5_hex(&image));
    let r = perform(&mut web, api_upload(&url, "fw.bin", &image));
    assert_eq!(r.status, 200);
    assert_eq!(text(&r), "{\"result\":\"ok\",\"restart\":true}");
    let k = rig.dev.ota.knobs();
    assert_eq!(k.begins, 1);
    assert_eq!(k.written, image);
    assert_eq!(k.finishes, 1);
    drop(k);
    assert!(rig.host.has(EventCode::EspOtaDone));
    let ev = rig.host.with_code(EventCode::RebootRequested);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 1); // an uploaded image
    assert!(rig.ota_shared.restart_pending());
}

// ---------------------------------------------------------------- guard (WG-1..WG-6, WG-16)

#[test]
fn wg1_an_api_write_without_x_vdmot_is_refused_nothing_submitted() {
    let rig = Rig::started();
    put_images(&rig, &[("fw", b"abc".to_vec())]);
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, post("/api/valves/1/target", "{\"target\":50}"));
    assert_eq!(r.status, 403);
    assert_eq!(text(&r), error_body("header_required", "X-VdMot: 1"));
    assert!(rig.submitted().is_empty());
    let zero = post("/api/valves/1/target", "{\"target\":50}").with_header("X-VdMot", "0");
    assert_eq!(perform(&mut web, zero).status, 403);
    let r = perform(
        &mut web,
        api_post("/api/valves/1/target", "{\"target\":50}"),
    );
    assert_eq!(r.status, 202);
    assert_eq!(rig.submitted().len(), 1);
    // DELETE needs it too, GET does not
    assert_eq!(perform(&mut web, del("/api/stm/images/fw")).status, 403);
    assert!(st.find_image(b"fw").is_some());
    assert!(rig.dev.fs.read("/stm/fw.bin").is_some());
    assert_eq!(perform(&mut web, get("/api/valves")).status, 200);
}

#[test]
fn wg2_a_body_must_be_json_parameters_of_the_media_type_are_fine() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let plain = with_type(
        api_post("/api/valves/1/target", "{\"target\":50}"),
        "text/plain",
    );
    let r = perform(&mut web, plain);
    assert_eq!(r.status, 415);
    assert_eq!(
        text(&r),
        error_body("unsupported_media_type", "application/json required")
    );
    assert!(rig.submitted().is_empty());
    let form = with_type(
        api_post("/api/valves/1/target", "target=50"),
        "application/x-www-form-urlencoded",
    );
    assert_eq!(perform(&mut web, form).status, 415);
    let charset = with_type(
        api_post("/api/valves/1/target", "{\"target\":50}"),
        "application/json; charset=utf-8",
    );
    assert_eq!(perform(&mut web, charset).status, 202);
    assert_eq!(rig.submitted().len(), 1);
}

#[test]
fn wg3_the_host_header_must_name_this_device_assets_are_not_checked() {
    let rig = Rig::new();
    rig.state().info.ip = 0x3301_A8C0; // 192.168.1.51
    rig.config(|c| {
        c.valves[0].active = true;
        set_text(&mut c.web.allowed_hosts, b"heating.lan");
    });
    let st = rig.storage();
    let mut web = rig.web(&st);
    let evil = with_host(get("/api/status"), "evil.com");
    let r = perform(&mut web, evil);
    assert_eq!(r.status, 403);
    assert_eq!(
        text(&r),
        error_body(
            "host_not_allowed",
            "evil.com: use 192.168.1.51 or add the name to web.allowedHosts"
        )
    );
    // the connection's own address is named first
    let mut evil = with_host(get("/api/status"), "evil.com");
    evil.local_ip = 0x3401_A8C0;
    let r = perform(&mut web, evil);
    assert_eq!(
        text(&r),
        error_body(
            "host_not_allowed",
            "evil.com: use 192.168.1.52 or add the name to web.allowedHosts"
        )
    );
    for host in [
        "192.168.1.51",
        "192.168.1.51:80",
        "vdmot.local",
        "VdMot",
        "heating.lan",
    ] {
        let r = perform(&mut web, with_host(get("/api/status"), host));
        assert_eq!(r.status, 200, "{host}");
    }
    let mut local = with_host(get("/api/status"), "10.1.1.1");
    local.local_ip = 0x0101_010A; // 10.1.1.1: the address the client connected to
    assert_eq!(perform(&mut web, local).status, 200);
    let asset = with_host(get("/"), "evil.com");
    assert_eq!(perform(&mut web, asset).status, 200);
    // a station change renames the device: the old name is refused
    rig.config(|c| set_text(&mut c.station, b"Dom 1"));
    assert_eq!(perform(&mut web, get("/api/status")).status, 403);
    let renamed = with_host(get("/api/status"), "dom-1.local");
    assert_eq!(perform(&mut web, renamed).status, 200);
}

#[test]
fn wg4_a_foreign_origin_is_refused_the_own_one_accepted() {
    let rig = Rig::started();
    rig.state().info.ip = 0x3301_A8C0; // 192.168.1.51
    let st = rig.storage();
    let mut web = rig.web(&st);
    let evil = api_post("/api/valves/1/target", "{\"target\":50}")
        .with_header("Origin", "http://evil.com");
    let r = perform(&mut web, evil);
    assert_eq!(r.status, 403);
    assert_eq!(
        text(&r),
        error_body("origin_not_allowed", "http://evil.com")
    );
    assert!(rig.submitted().is_empty());
    let own = api_post("/api/valves/1/target", "{\"target\":50}")
        .with_header("Origin", "http://192.168.1.51");
    assert_eq!(perform(&mut web, own).status, 202);
    // a GET with a foreign Origin is refused too
    let g = get("/api/status").with_header("Origin", "null");
    assert_eq!(perform(&mut web, g).status, 403);
}

#[test]
fn wg5_an_upload_without_x_vdmot_never_reaches_storage() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let req = served(&mut web, upload("/api/stm/images", "fw.bin", b"abc", &[]));
    assert_eq!(req.response.status, 403);
    assert_eq!(
        text(&req.response),
        error_body("header_required", "X-VdMot: 1")
    );
    assert_eq!(req.body_read(), 0);
    assert!(rig.dev.fs.read("/stm/fw.bin").is_none());
    assert!(rig.dev.fs.read("/stm/fw.bin.part").is_none());
    let o = perform(
        &mut web,
        upload("/api/ota/esp", "fw.bin", &esp_image(8), &[]),
    );
    assert_eq!(o.status, 403);
    assert_eq!(rig.dev.ota.knobs().begins, 0);
}

#[test]
fn wg6_a_refused_request_is_answered_for_the_header_the_guard_saw() {
    // C++ "the refusal is recomputed after the header filter with the same answer": the
    // library's filter of uninteresting headers is gone; the guard sees every header
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = api_post("/api/valves/1/target", "{\"target\":50}")
        .with_header("Origin", "http://evil.com")
        .with_header("X-Other", "dropped");
    let r = perform(&mut web, r);
    assert_eq!(r.status, 403);
    assert_eq!(
        text(&r),
        error_body("origin_not_allowed", "http://evil.com")
    );
}

#[test]
fn wg16_request_refused_is_logged_once_per_verdict_and_minute() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let no_marker = || post("/api/valves/1/target", "{\"target\":50}");
    let bad_host = || with_host(get("/api/status"), "evil.com");
    assert_eq!(perform(&mut web, no_marker()).status, 403);
    rig.dev.clock.advance_ms(1000);
    assert_eq!(perform(&mut web, no_marker()).status, 403);
    assert_eq!(perform(&mut web, bad_host()).status, 403);
    let ev = rig.host.with_code(EventCode::RequestRefused);
    assert_eq!(ev.len(), 2);
    assert_eq!(ev[0].arg1, 3);
    assert_eq!(ev[0].text.as_slice(), b"192.168.1.50");
    assert_eq!(ev[1].arg1, 1);
    rig.dev.clock.advance_ms(58_999); // 59999 ms after the first
    assert_eq!(perform(&mut web, no_marker()).status, 403);
    assert_eq!(rig.host.with_code(EventCode::RequestRefused).len(), 2);
    rig.dev.clock.advance_ms(1);
    assert_eq!(perform(&mut web, no_marker()).status, 403);
    let ev = rig.host.with_code(EventCode::RequestRefused);
    assert_eq!(ev.len(), 3);
    assert_eq!(ev[2].arg1, 3);
    // a content type refusal is verdict 4
    let plain = with_type(api_post("/api/valves/1/target", "{}"), "text/plain");
    assert_eq!(perform(&mut web, plain).status, 415);
    let ev = rig.host.with_code(EventCode::RequestRefused);
    assert_eq!(ev.last().unwrap().arg1, 4);
}

// ---------------------------------------------------------------- legacy (WG-8)

#[test]
fn wg8_get_valves_answers_the_legacy_document() {
    let rig = Rig::new();
    rig.config(|c| {
        c.valves[0].active = true;
        set_text(&mut c.valves[0].name, b"Bad");
    });
    {
        let mut s = rig.state();
        let v = &mut s.snapshot.valves[0];
        v.status = 1;
        v.position = 40;
        v.desired_valid = true;
        v.desired = 55;
        s.snapshot.valves[1].status = 6; // no valve: skipped
    }
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/valves"));
    assert_eq!(r.status, 200);
    assert_eq!(
        text(&r),
        "{\"valves\":[{\"idx\":1,\"name\":\"Bad\",\"state\":1,\"pos\":40,\"meanCur\":0,\
         \"targetPos\":55,\"link\":0,\"moves\":0,\"oc\":0,\"cc\":0,\"dc\":0,\"cr\":0,\
         \"controlActive\":0}]}"
    );
}

#[test]
fn wg8_temps_and_volts_answer_the_legacy_documents() {
    let rig = Rig::new();
    let t_id = parse_one_wire_id(b"28-84-37-94-97-ff-03-23").unwrap();
    let u_id = parse_one_wire_id(b"26-11-22-33-44-55-66-29").unwrap();
    rig.config(|c| {
        c.valves[0].active = true;
        let t = &mut c.temps[0];
        set_text(&mut t.name, b"Floor");
        t.active = true;
        t.id = t_id;
        t.offset = 5;
        let u = &mut c.volts[0];
        set_text(&mut u.name, b"Supply");
        set_text(&mut u.unit, b"V");
        u.active = true;
        u.factor = 1.0;
        u.offset = 0.0;
        u.id = u_id;
    });
    {
        let mut st = rig.state();
        let s = &mut st.snapshot;
        s.temp_count = 1;
        s.temps[0].id = t_id;
        s.temps[0].raw = 210;
        s.temps[0].seen = true;
        s.temps[0].last_seen_ms = 0;
        s.volt_count = 1;
        s.volts[0].id = u_id;
        s.volts[0].vad = 1208;
        s.volts[0].seen = true;
    }
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/temps"));
    assert_eq!(r.status, 200);
    assert_eq!(
        text(&r),
        "[{\"id\":\"28-84-37-94-97-ff-03-23\",\"name\":\"Floor\",\"temp\":21.5}]"
    );
    let r = perform(&mut web, get("/volts"));
    assert_eq!(r.status, 200);
    assert_eq!(
        text(&r),
        "[{\"id\":\"26-11-22-33-44-55-66-29\",\"name\":\"Supply\",\"unit\":\"V\",\"value\":12.080}]"
    );
    // a temperature used by a valve is listed only with allTemps
    rig.state().snapshot.valves[2].sensor_slot[0] = 1;
    rig.config(|c| c.mqtt.all_temps = false);
    assert_eq!(text(&perform(&mut web, get("/temps"))), "[]");
}

#[test]
fn wg8_post_setvalve_rounds_the_value_and_answers_the_legacy_body() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(
        &mut web,
        post("/setvalve", "{\"valve\":1,\"value\":43.7,\"ctrlValue\":3}"),
    );
    assert_eq!(r.status, 200);
    assert_eq!(text(&r), "{\"res\":\"ok\"}");
    let s = rig.submitted();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].kind, StmCommandType::SetTarget);
    assert_eq!(s[0].valve, 0);
    assert_eq!(s[0].pos, 44);
    assert_eq!(s[0].source, TargetSource::Web);
    let r = perform(&mut web, post("/setvalve", "{\"valve\":2,\"value\":10}"));
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("inactive", "valve not active"));
    for bad in [
        "{\"valve\":0,\"value\":1}",
        "{\"valve\":13,\"value\":1}",
        "{\"valve\":1,\"value\":100.5}",
        "{\"valve\":1,\"value\":\"5\"}",
        "{\"valve\":1}",
        "{\"value\":1}",
    ] {
        let r = perform(&mut web, post("/setvalve", bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(
            text(&r),
            error_body("out_of_range", "valve 1..12, value 0..100")
        );
    }
    rig.state().submit_result = false;
    let r = perform(&mut web, post("/setvalve", "{\"valve\":1,\"value\":1}"));
    assert_eq!(r.status, 503);
    let plain = post_typed("/setvalve", b"{\"valve\":1,\"value\":1}", "text/plain");
    assert_eq!(perform(&mut web, plain).status, 415);
    assert_eq!(rig.submitted().len(), 1);
    assert_eq!(perform(&mut web, get("/setvalve")).status, 405);
    assert_eq!(perform(&mut web, post("/valves", "")).status, 405);
}

#[test]
fn wg8_legacy_paths_answer_410_without_reading_others_404_405_never_413() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/netinfo"));
    assert_eq!(r.status, 410);
    assert_eq!(text(&r), error_body("gone", "/api/status"));
    let req = served(&mut web, post("/netconfig", &"x".repeat(2048)));
    assert_eq!(req.response.status, 410);
    assert_eq!(text(&req.response), error_body("gone", "/api/config"));
    assert_eq!(req.body_read(), 0);
    let r = perform(&mut web, post("/nope", "{\"a\":1}"));
    assert_eq!(r.status, 405);
    assert_eq!(text(&r), error_body("method_not_allowed", "/nope"));
    let r = perform(&mut web, get("/nope"));
    assert_eq!(r.status, 404);
    assert_eq!(text(&r), error_body("not_found", "/nope"));
    let mut get_body = get("/nope");
    get_body.body = b"x".to_vec();
    assert_eq!(perform(&mut web, get_body).status, 404);
    // the legacy aliases go through the guard
    assert_eq!(
        perform(&mut web, with_host(get("/valves"), "evil.com")).status,
        403
    );
    // 410 answers are never guarded
    assert_eq!(
        perform(&mut web, with_host(get("/cmd"), "evil.com")).status,
        410
    );
}

// ---------------------------------------------------------------- targets (WG-9)

#[test]
fn wg9_fractional_targets_are_rounded_other_values_refused() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(
        &mut web,
        api_post("/api/valves/1/target", "{\"target\":55.0}"),
    );
    assert_eq!(r.status, 202);
    assert_eq!(text(&r), "{\"valve\":1,\"target\":55}");
    let r = perform(
        &mut web,
        api_post("/api/valves/1/target", "{\"target\":43.5}"),
    );
    assert_eq!(text(&r), "{\"valve\":1,\"target\":44}");
    let r = perform(
        &mut web,
        api_post("/api/valves/1/target", "{\"target\":100.0}"),
    );
    assert_eq!(text(&r), "{\"valve\":1,\"target\":100}");
    let s = rig.submitted();
    assert_eq!(s.iter().map(|c| c.pos).collect::<Vec<_>>(), [55, 44, 100]);
    for bad in [
        "{\"target\":100.4}",
        "{\"target\":-0.4}",
        "{\"target\":\"5\"}",
        "{\"target\":true}",
        "{\"target\":null}",
        "{}",
        "{\"target\":5,\"x\":1}",
    ] {
        let r = perform(&mut web, api_post("/api/valves/1/target", bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(text(&r), error_body("out_of_range", "target 0..100"));
    }
    assert_eq!(rig.submitted().len(), 3);
}

// ---------------------------------------------------------------- config (WG-10, WG-11)

#[test]
fn wg10_a_save_without_memory_for_its_answer_applies_nothing() {
    // C++ "a config save with both slots held answers 503 and applies nothing": no slots; the
    // response buffer of the answer is taken before anything is applied
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let revision = rig.storage_shared.config_revision();
    // the working set (4 parts), the body, then no response buffer
    rig.dev
        .heap
        .state()
        .next
        .extend([true, true, true, true, true, false]);
    let r = perform(
        &mut web,
        api_post("/api/config", "{\"calib\":{\"hour\":4}}"),
    );
    assert_eq!(r.status, 503);
    assert_eq!(text(&r), error_body("busy", "out of memory"));
    assert_eq!(rig.storage_shared.config_revision(), revision);
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_CONFIG));
    let r = perform(
        &mut web,
        api_post("/api/config", "{\"calib\":{\"hour\":4}}"),
    );
    assert_eq!(r.status, 200);
    assert_eq!(rig.active().calib.hour, 4);
    assert_eq!(rig.storage_shared.config_revision(), revision + 1);
}

#[test]
fn wg10_the_save_answer_tells_whether_a_restart_and_a_trial_follow() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let revision = rig.storage_shared.config_revision();
    let r = perform(
        &mut web,
        api_post("/api/config", "{\"calib\":{\"hour\":4}}"),
    );
    assert_eq!(r.status, 200);
    assert_eq!(rig.active().calib.hour, 4);
    assert!(rig.dev.nvs.has(NAMESPACE, KEY_CONFIG));
    let tail = ",\"restartRequired\":false,\"netTrial\":false}";
    assert!(text(&r).ends_with(tail), "{}", text(&r));
    assert!(rig.host.has(EventCode::ConfigSaved));
    let r = perform(
        &mut web,
        api_post("/api/config", "{\"net\":{\"reconnectTimeoutMin\":9}}"),
    );
    assert!(text(&r).ends_with(tail));
    let r = perform(
        &mut web,
        api_post(
            "/api/config",
            "{\"net\":{\"dhcp\":false,\"ip\":\"192.168.1.60\",\
             \"mask\":\"255.255.255.0\",\"gateway\":\"192.168.1.1\"}}",
        ),
    );
    assert_eq!(r.status, 200);
    assert!(text(&r).ends_with(",\"restartRequired\":true,\"netTrial\":true}"));
    assert_eq!(rig.storage_shared.config_revision(), revision + 3);
    // a station rename restarts without a trial
    let r = perform(&mut web, api_post("/api/config", "{\"station\":\"Other\"}"));
    assert!(text(&r).ends_with(",\"restartRequired\":true,\"netTrial\":false}"));
}

#[test]
fn wg10_a_dry_run_validates_and_answers_without_storing() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let revision = rig.storage_shared.config_revision();
    let r = perform(
        &mut web,
        api_post(
            "/api/config?dryRun=1",
            "{\"net\":{\"dhcp\":false,\"ip\":\"192.168.1.60\",\
             \"mask\":\"255.255.255.0\",\"gateway\":\"192.168.1.1\"}}",
        ),
    );
    assert_eq!(r.status, 200);
    assert_eq!(text(&r), "{\"restartRequired\":true,\"netTrial\":true}");
    let r = perform(
        &mut web,
        api_post("/api/config?dryRun=1", "{\"calib\":{\"hour\":4}}"),
    );
    assert_eq!(text(&r), "{\"restartRequired\":false,\"netTrial\":false}");
    let r = perform(
        &mut web,
        api_post("/api/config?dryRun=1", "{\"calib\":{\"hour\":24}}"),
    );
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("invalid", "calib.hour"));
    // a combination only the whole-config rules reject
    let r = perform(
        &mut web,
        api_post("/api/config?dryRun=1", "{\"net\":{\"dhcp\":false}}"),
    );
    assert_eq!(r.status, 400);
    assert!(text(&r).contains("\"error\":\"invalid\""));
    let r = perform(
        &mut web,
        api_post("/api/config?dryRun=2", "{\"calib\":{\"hour\":4}}"),
    );
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "dryRun=1"));
    assert_eq!(rig.storage_shared.config_revision(), revision);
    assert!(!rig.dev.nvs.has(NAMESPACE, KEY_CONFIG));
    // the copy the dry runs patched is the active config again
    let r = perform(&mut web, get("/api/config"));
    assert!(text(&r).contains("\"hour\":0,"), "{}", text(&r));
    assert!(text(&r).contains("\"dhcp\":true,"));
}

#[test]
fn wg10_a_dry_run_needs_no_response_buffer() {
    // C++ "a dry run needs no response slot"
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(
        &mut web,
        api_post("/api/config?dryRun=1", "{\"calib\":{\"hour\":4}}"),
    );
    assert_eq!(r.status, 200);
    assert!(!rig.dev.heap.state().granted.contains(&super::RESPONSE_SIZE));
}

#[test]
fn wg10_a_failed_save_answers_its_path_and_reloads_the_copy() {
    // C++ "a failed save releases its slot": no slots; a save that fails leaves the copy as the
    // active config. C++ applyPath "net.ip" with applyResult false (a storage refusal other
    // than "nvs"): no Rust form, storage validates what the patch validated
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    rig.dev
        .nvs
        .knobs()
        .fail_set
        .insert(KEY_CONFIG_EXT.to_string());
    let r = perform(
        &mut web,
        api_post("/api/config", "{\"calib\":{\"hour\":4}}"),
    );
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("invalid", "nvs"));
    rig.dev.nvs.knobs().fail_set.clear();
    let r = perform(
        &mut web,
        api_post("/api/config", "{\"calib\":{\"hour\":99}}"),
    );
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("invalid", "calib.hour"));
    assert!(!rig.host.has(EventCode::ConfigSaved));
    let r = perform(&mut web, get("/api/config"));
    assert!(text(&r).contains("\"hour\":0,"), "{}", text(&r));
}

#[test]
fn wg11_the_export_never_carries_a_secret_secrets_1_included() {
    let rig = Rig::new();
    rig.config(|c| set_text(&mut c.mqtt.password, b"brokerpw"));
    let st = rig.storage();
    let mut web = rig.web(&st);
    for url in ["/api/config/export", "/api/config/export?secrets=1"] {
        let r = perform(&mut web, get(url));
        assert_eq!(r.status, 200, "{url}");
        assert_eq!(
            r.header("Content-Disposition"),
            "attachment; filename=\"vdmot-config.json\""
        );
        assert_eq!(r.header("Cache-Control"), "no-store");
        assert!(!text(&r).contains("brokerpw"));
        assert!(text(&r).contains("\"passwordSet\":true"));
    }
}

// ---------------------------------------------------------------- network trial (WG-12)

#[test]
fn wg12_confirm_and_revert_of_a_network_trial() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_post("/api/system/network/confirm", ""));
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("no_trial", "no network trial running"));
    let r = perform(&mut web, api_post("/api/system/network/revert", ""));
    assert_eq!(r.status, 409);
    assert_eq!(rig.state().trial_confirms, 1);
    assert_eq!(rig.state().trial_reverts, 1);
    rig.state().trial_confirm_result = true;
    rig.state().trial_revert_result = true;
    let r = perform(&mut web, api_post("/api/system/network/confirm", ""));
    assert_eq!(r.status, 202);
    assert_eq!(text(&r), "{\"result\":\"queued\"}");
    let r = perform(&mut web, api_post("/api/system/network/revert", ""));
    assert_eq!(r.status, 202);
    assert_eq!(rig.state().trial_confirms, 2);
    assert_eq!(rig.state().trial_reverts, 2);
}

// ---------------------------------------------------------------- files (WG-13, WG-14)

#[test]
fn wg13_the_file_list_and_file_removal() {
    let rig = Rig::started();
    rig.dev.fs.put("/x.bin", &[7u8; 1024]);
    rig.dev.fs.put("/sys/cfg.bak", &[1u8; 300]);
    rig.dev.fs.put("/z.bin", b"z");
    rig.dev.fs.knobs().total_bytes = 1_000_000;
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/files"));
    assert_eq!(r.status, 200);
    assert_eq!(
        text(&r),
        format!(
            "{{\"total\":1000000,\"used\":{},\"truncated\":false,\"files\":[\
             {{\"path\":\"/sys/cfg.bak\",\"size\":300,\"kind\":\"internal\",\"deletable\":false}},\
             {{\"path\":\"/x.bin\",\"size\":1024,\"kind\":\"legacy_image\",\"deletable\":true}},\
             {{\"path\":\"/z.bin\",\"size\":1,\"kind\":\"legacy_image\",\"deletable\":true}}]}}",
            st.fs_used()
        )
    );
    for i in 0..30 {
        rig.dev.fs.put(&format!("/f{i:02}.txt"), b"f");
    }
    let r = perform(&mut web, get("/api/files"));
    assert!(text(&r).contains("\"truncated\":true"));

    let r = perform(&mut web, api_del("/api/files?path=/x.bin"));
    assert_eq!(r.status, 204);
    assert!(rig.dev.fs.read("/x.bin").is_none());
    assert_eq!(rig.host.with_code(EventCode::FilesRemoved).len(), 1);
    let r = perform(&mut web, api_del("/api/files?path=/sys/cfg.bak"));
    assert_eq!(r.status, 403);
    assert_eq!(
        text(&r),
        error_body("protected", file_protect_reason(FileKind::Internal))
    );
    let r = perform(&mut web, api_del("/api/files?path=//x"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_path", "//x"));
    // an invalid path never reaches storage
    assert_eq!(rig.host.with_code(EventCode::FilesRemoved).len(), 1);
    let r = perform(&mut web, api_del("/api/files"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_path", "path"));
    let r = perform(&mut web, api_del("/api/files?path=/y.bin"));
    assert_eq!(r.status, 404);
    assert_eq!(text(&r), error_body("not_found", "/y.bin"));
    // storage: a directory is a bad path
    let r = perform(&mut web, api_del("/api/files?path=/stm"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_path", "/stm"));
    rig.dev.fs.fail("remove", "/z.bin", 1);
    let r = perform(&mut web, api_del("/api/files?path=/z.bin"));
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("io_error", "/z.bin"));
}

#[test]
fn wg14_the_import_report_is_sent_and_dismissed() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/import-report"));
    assert_eq!(r.status, 404);
    assert_eq!(text(&r), error_body("not_found", "no import report"));
    let report = b"{\"imported\":57,\"rejected\":2}";
    rig.dev.fs.put(IMPORT_REPORT_FILE, report);
    let r = perform(&mut web, get("/api/import-report"));
    assert_eq!(r.status, 200);
    assert_eq!(r.content_type, "application/json");
    assert_eq!(r.body, report);
    // C++ the stdio buffer of the read (512): no Rust form, the port has no stdio layer
    assert_eq!(rig.dev.fs.open_handles(), 0);
    let r = perform(&mut web, api_del("/api/import-report"));
    assert_eq!(r.status, 204);
    assert!(rig.dev.fs.read(IMPORT_REPORT_FILE).is_none());
    assert_eq!(perform(&mut web, get("/api/import-report")).status, 404);
    assert_eq!(perform(&mut web, api_del("/api/import-report")).status, 404);
}

// ---------------------------------------------------------------- status (WG-15)

/// The status document of a rig whose storage loaded its config as prepared by `prepare`.
fn status_after_load(prepare: impl FnOnce(&Rig)) -> String {
    let rig = Rig::new();
    prepare(&rig);
    let st = rig.storage();
    let mut c = Box::<Config>::default();
    let mut report = Default::default();
    let mut details = Default::default();
    st.load_config(&mut c, &mut report, &mut details);
    let mut web = rig.web(&st);
    text(&perform(&mut web, get("/api/status")))
}

#[test]
fn wg15_status_members_of_2_1() {
    let rig = Rig::new();
    // the boot load: the backup, a newer firmware's blob whose ext records need a repair (a
    // valve topic equal to another valve's segment)
    let mut c = Box::<Config>::default();
    set_text(&mut c.valves[0].name, b"Bad");
    let mut base = vec![0u8; vdm_esp_core::config::CONFIG_BLOB_MAX];
    let n = vdm_esp_core::config::encode_config(&c, &mut base);
    base.truncate(n);
    base[4] = 2; // base schema 2
    let crc = crc32(&base[..n - 4], 0);
    base[n - 4..].copy_from_slice(&crc.to_le_bytes());
    rig.dev.fs.put(BACKUP_BASE, &base);
    let mut ext = vec![0u8; vdm_esp_core::config::CONFIG_EXT_BLOB_MAX];
    set_text(&mut c.valves[3].topic, b"Bad");
    let x = vdm_esp_core::config::encode_config_ext(&c, &mut ext, &[]);
    rig.dev.fs.put(BACKUP_EXT, &ext[..x]);
    rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, b"broken");
    rig.dev.fs.put(IMPORT_REPORT_FILE, b"{}");
    let st = rig.storage();
    let mut loaded = Box::<Config>::default();
    let mut report = Default::default();
    let mut details = Default::default();
    assert_eq!(
        st.load_config(&mut loaded, &mut report, &mut details),
        LoadSource::Backup
    );
    assert_eq!(rig.load_details().info.repairs.mask, REPAIR_TOPICS);
    rig.config(|c| set_text(&mut c.station, "Dom P\u{f3}\u{142}noc".as_bytes()));
    {
        let mut s = rig.state();
        s.trial.active = true;
        s.trial.remain_s = 97;
        set_text(&mut s.status.client_id, b"vdm-1");
        s.status.ha_status = HaStatus::Online;
        s.snapshot.support = StmSupport::Supported;
        s.snapshot.have_learn_time = true;
        s.snapshot.learn_time_s = 600;
    }
    let mut web = rig.web(&st);
    let host = || with_host(get("/api/status"), "dom-p-noc.local");
    let r = perform(&mut web, host());
    assert_eq!(r.status, 200);
    let body = text(&r);
    assert!(
        body.starts_with("{\"station\":\"Dom P\u{f3}\u{142}noc\",\"esp\":"),
        "{body}"
    );
    assert!(body.contains("\"hostname\":\"Dom-P-noc\",\"trial\":{\"remainS\":97}}"));
    assert!(body.contains("\"clientId\":\"vdm-1\",\"haStatus\":\"online\"}"));
    assert!(body.contains("\"support\":\"ok\",\"lease\":null,\"learnTime\":600}"));
    assert!(body.contains(
        "\"config\":{\"source\":\"backup\",\"repairs\":8192,\"newerSchema\":true},\"importReport\":true}"
    ));
    // the longest client id the config allows is reported whole
    let long_id = "c".repeat(CLIENT_ID_MAX);
    set_text(&mut rig.state().status.client_id, long_id.as_bytes());
    let full = text(&perform(&mut web, host()));
    assert!(full.contains(&format!("\"clientId\":\"{long_id}\",\"haStatus\"")));
    rig.state().trial.active = false;
    assert!(st.dismiss_import_report());
    let n = text(&perform(&mut web, host()));
    assert!(n.contains("\"trial\":null}"));
    assert!(n.contains("\"importReport\":false}"));
}

#[test]
fn wg15_the_config_source_of_every_boot_load() {
    let stored = status_after_load(|rig| {
        let c = Box::<Config>::default();
        let mut base = vec![0u8; vdm_esp_core::config::CONFIG_BLOB_MAX];
        let n = vdm_esp_core::config::encode_config(&c, &mut base);
        rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, &base[..n]);
    });
    assert!(
        stored.contains("\"config\":{\"source\":\"stored\",\"repairs\":0,\"newerSchema\":false}"),
        "{stored}"
    );
    let imported = status_after_load(|rig| rig.dev.nvs.set_str("sysCfg", "stName", b"Legacy"));
    assert!(
        imported.contains("\"config\":{\"source\":\"imported\""),
        "{imported}"
    );
    let defaults = status_after_load(|_| {});
    assert!(
        defaults.contains("\"config\":{\"source\":\"defaults\""),
        "{defaults}"
    );
    let after_error =
        status_after_load(|rig| rig.dev.nvs.set_blob(NAMESPACE, KEY_CONFIG, b"broken"));
    assert!(
        after_error.contains("\"config\":{\"source\":\"defaults_after_error\""),
        "{after_error}"
    );
}

#[test]
fn wg15_calibration_next_is_the_local_time_of_the_next_slot() {
    let rig = Rig::started();
    rig.state().calib.next_epoch = 1_790_000_000; // 2026-09-21 14:13:20 UTC
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = text(&perform(&mut web, get("/api/status")));
    // the fake wall clock runs UTC until a zone is set (the C++ test machine's localtime_r)
    assert!(r.contains("\"next\":\"2026-09-21T14:13:20\"}"), "{r}");
    rig.state().calib.next_epoch = 0;
    let r = text(&perform(&mut web, get("/api/status")));
    assert!(r.contains("\"next\":null}"));
}

#[test]
fn valves_carry_the_sensor_position_and_the_calibration_end() {
    let rig = Rig::new();
    rig.config(|c| {
        c.valves[0].active = true;
        set_text(&mut c.temps[6].name, b"Wall");
        c.temps[6].offset = -3;
    });
    {
        let mut s = rig.state();
        s.snapshot.valves[0].sensor_slot[1] = 7;
        s.snapshot.valves[0].temp2 = 210;
        s.calib_end[0] = Some(LocalTime {
            valid: true,
            year: 2026,
            month: 9,
            mday: 20,
            hour: 3,
            minute: 4,
            second: 5,
            ..LocalTime::default()
        });
    }
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/valves"));
    assert_eq!(r.status, 200);
    let body = text(&r);
    assert!(
        body.contains("\"sensors\":[{\"sensor\":2,\"slot\":7,\"name\":\"Wall\",\"temp\":20.7}]")
    );
    assert!(body.contains("\"calibrationEnd\":\"2026-09-20T03:04:05\"},{\"idx\":2,"));
    assert_eq!(count_of(&body, "\"calibrationEnd\":null"), 11);
}

// ---------------------------------------------------------------- health (WG-17)

#[test]
fn wg17_health_answers_from_the_app_document() {
    // C++ "/api/health needs no response slot": no slots; the document of the app, guarded
    let rig = Rig::started();
    rig.state().info.ip = 0x3301_A8C0;
    rig.host.set_health_version("2.1.0-revamped");
    rig.state().health.uptime_s = 77;
    let st = rig.storage();
    let mut web = rig.web(&st);
    let h = || with_host(get("/api/health"), "192.168.1.51");
    let r = perform(&mut web, h());
    assert_eq!(r.status, 200);
    assert_eq!(r.content_type, "application/json");
    assert_eq!(r.header("Cache-Control"), "no-store");
    let body = text(&r);
    assert!(body.starts_with("{\"ok\":true,\"version\":\"2.1.0-revamped\",\"uptime\":77,"));
    assert!(body.ends_with('}'));
    assert_eq!(rig.state().health_reads, 1);
    let evil = with_host(get("/api/health"), "evil.com");
    assert_eq!(perform(&mut web, evil).status, 403);
    assert_eq!(rig.state().health_reads, 1);
}

// ---------------------------------------------------------------- sys hooks (WG-18)

#[test]
fn wg18_every_served_request_reports_its_client_the_log_is_flushed_first() {
    let rig = Rig::started();
    rig.dev.fs.put("/log/events.1.log", b"#0 old\n");
    rig.dev.fs.put("/log/events.log", b"#1 line\n");
    let st = rig.storage();
    let mut web = rig.web(&st);
    let mut g = get("/api/status");
    g.remote_ip = 0x1401_A8C0; // 192.168.1.20
    perform(&mut web, g);
    assert_eq!(rig.state().inbound, [0x1401_A8C0]);
    let mut refused = post("/api/valves/1/target", "{}");
    refused.remote_ip = 0x1501_A8C0;
    perform(&mut web, refused);
    let mut asset = get("/");
    asset.remote_ip = 0x1601_A8C0;
    perform(&mut web, asset);
    assert_eq!(rig.state().inbound, [0x1401_A8C0, 0x1501_A8C0, 0x1601_A8C0]);
    let r = perform(&mut web, get("/api/log"));
    assert_eq!(rig.state().flush_requests, 1);
    assert_eq!(r.status, 200);
    assert!(r.chunked && r.ended);
    assert_eq!(text(&r), "#0 old\n#1 line\n");
    // C++ both files through 512 B stdio buffers: no Rust form (no stdio layer)
    assert_eq!(rig.dev.fs.open_handles(), 0);
}

// ---------------------------------------------------------------- STM support and v3 routes

#[test]
fn stm_actions_are_refused_while_the_stm_firmware_is_too_old() {
    let rig = Rig::started();
    {
        let mut s = rig.state();
        s.support = StmSupport::TooOld;
        s.snapshot.version = parse_version(b"1.3.9");
        s.proto = 2;
    }
    let st = rig.storage();
    let mut web = rig.web(&st);
    let detail = "STM firmware 1.3.9 is older than 1.4.0: update the STM";
    for url in [
        "/api/valves/1/calibrate",
        "/api/valves/1/assembly",
        "/api/valves/calibrate",
        "/api/valves/assembly",
        "/api/valves/detect",
        "/api/sensors/scan",
        "/api/valves/1/profile",
        "/api/valves/1/stop",
        "/api/valves/stop",
        "/api/stm/safe-mode/leave",
    ] {
        let r = perform(&mut web, api_post(url, ""));
        assert_eq!(r.status, 409, "{url}");
        assert_eq!(text(&r), error_body("stm_unsupported", detail));
    }
    let r = perform(
        &mut web,
        api_post(
            "/api/valves/1/service-move",
            "{\"dir\":\"open\",\"counts\":10,\"maxmA\":20}",
        ),
    );
    assert_eq!(r.status, 409);
    let r = perform(
        &mut web,
        api_post("/api/valves/1/sensors", "{\"slot1\":0,\"slot2\":0}"),
    );
    assert_eq!(r.status, 409);
    let r = perform(
        &mut web,
        api_post("/api/stm/motor", "{\"learnMovements\":0}"),
    );
    assert_eq!(r.status, 409);
    assert!(rig.submitted().is_empty());
    // target and reset still work
    let r = perform(&mut web, api_post("/api/valves/1/target", "{\"target\":5}"));
    assert_eq!(r.status, 202);
    let r = perform(&mut web, api_post("/api/stm/reset", "{\"confirm\":true}"));
    assert_eq!(r.status, 202);
    assert_eq!(rig.submitted().len(), 2);
    // an unreadable version is shown as "?"
    rig.state().snapshot.version = Version::default();
    let r = perform(&mut web, api_post("/api/valves/detect", ""));
    assert_eq!(
        text(&r),
        error_body(
            "stm_unsupported",
            "STM firmware ? is older than 1.4.0: update the STM"
        )
    );
}

#[test]
fn stop_and_safe_mode_leave_need_protocol_3() {
    let rig = Rig::started();
    rig.state().proto = 2;
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_post("/api/valves/3/stop", ""));
    assert_eq!(r.status, 409);
    assert_eq!(
        text(&r),
        error_body("stm_unsupported", "STM protocol 3 required")
    );
    assert_eq!(
        perform(&mut web, api_post("/api/stm/safe-mode/leave", "")).status,
        409
    );
    rig.state().proto = 3;
    let r = perform(&mut web, api_post("/api/valves/3/stop", ""));
    assert_eq!(r.status, 202);
    assert_eq!(text(&r), "{\"result\":\"queued\"}");
    assert_eq!(
        perform(&mut web, api_post("/api/valves/stop", "")).status,
        202
    );
    assert_eq!(
        perform(&mut web, api_post("/api/stm/safe-mode/leave", "")).status,
        202
    );
    let s = rig.submitted();
    assert_eq!(s.len(), 3);
    assert_eq!((s[0].kind, s[0].valve), (StmCommandType::StopValve, 2));
    assert_eq!(
        (s[1].kind, s[1].valve),
        (StmCommandType::StopValve, ALL_VALVES)
    );
    assert_eq!(s[2].kind, StmCommandType::LeaveSafeMode);
    rig.state().flash_active = true;
    assert_eq!(
        perform(&mut web, api_post("/api/valves/stop", "")).status,
        409
    );
    rig.state().flash_active = false;
    rig.state().submit_result = false;
    assert_eq!(
        perform(&mut web, api_post("/api/stm/safe-mode/leave", "")).status,
        503
    );
}

#[test]
fn mqtt_discovery_rules_per_mode() {
    let rig = Rig::started();
    rig.config(|c| c.mqtt.mode = MqttMode::Off);
    let st = rig.storage();
    let mut web = rig.web(&st);
    let publish = || api_post("/api/mqtt/discovery", "{\"action\":\"publish\"}");
    let r = perform(&mut web, publish());
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("disabled", "MQTT is off"));
    rig.config(|c| {
        c.mqtt.mode = MqttMode::Mqtt;
        c.mqtt.separate = false;
    });
    let r = perform(&mut web, publish());
    assert_eq!(r.status, 409);
    assert_eq!(
        text(&r),
        error_body("separate_required", "HA discovery needs separate topics")
    );
    let republish = || api_post("/api/mqtt/discovery", "{\"action\":\"republish\"}");
    assert_eq!(perform(&mut web, republish()).status, 409);
    let delete = api_post("/api/mqtt/discovery", "{\"action\":\"delete\"}");
    assert_eq!(perform(&mut web, delete).status, 202);
    rig.config(|c| c.mqtt.separate = true);
    assert_eq!(perform(&mut web, publish()).status, 202);
    rig.config(|c| c.mqtt.mode = MqttMode::MqttHa);
    assert_eq!(perform(&mut web, republish()).status, 202);
    assert_eq!(
        rig.state().discovery,
        [
            DiscoveryAction::Delete,
            DiscoveryAction::Publish,
            DiscoveryAction::DeleteAndPublish
        ]
    );
}

#[test]
fn a_flash_checks_the_board_revision_of_the_image() {
    let rig = Rig::started();
    put_images(
        &rig,
        &[
            ("fw", stm_image(true, "1.4.9_Dev", "C1")),
            ("raw", stm_image(true, "1.4.9_Dev", "")),
        ],
    );
    let st = rig.storage();
    scan_all(&st);
    assert_eq!(st.find_image(b"fw").unwrap().hw_tag.as_slice(), b"C1");
    let mut web = rig.web(&st);
    // the running STM is unknown: blank mode without a board choice is refused
    let r = perform(
        &mut web,
        api_post("/api/stm/flash", "{\"image\":\"fw\",\"mode\":\"blank\"}"),
    );
    assert_eq!(r.status, 409);
    assert_eq!(
        text(&r),
        error_body("board_required", "choose the board: C1 or C2")
    );
    let r = perform(
        &mut web,
        api_post(
            "/api/stm/flash",
            "{\"image\":\"fw\",\"mode\":\"blank\",\"board\":\"C2\"}",
        ),
    );
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("board_mismatch", "image C1, board C2"));
    assert!(rig.submitted().is_empty());
    let r = perform(
        &mut web,
        api_post(
            "/api/stm/flash",
            "{\"image\":\"fw\",\"mode\":\"blank\",\"board\":\"C3\"}",
        ),
    );
    assert_eq!(r.status, 400);
    let r = perform(
        &mut web,
        api_post(
            "/api/stm/flash",
            "{\"image\":\"fw\",\"mode\":\"blank\",\"board\":\"C1\"}",
        ),
    );
    assert_eq!(r.status, 202);
    let s = rig.submitted();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].board.as_slice(), b"C1");
    assert!(s[0].blank);
    // the running STM's tag wins over the choice; force skips the check
    rig.state().snapshot.version = parse_version(b"2.1.0-revamped_C2");
    let r = perform(
        &mut web,
        api_post("/api/stm/flash", "{\"image\":\"fw\",\"board\":\"C1\"}"),
    );
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("board_mismatch", "image C1, board C2"));
    let r = perform(
        &mut web,
        api_post("/api/stm/flash", "{\"image\":\"fw\",\"force\":true}"),
    );
    assert_eq!(r.status, 202);
    assert_eq!(rig.submitted()[1].board.as_slice(), b"");
    // an untagged image flashes on any board
    let r = perform(&mut web, api_post("/api/stm/flash", "{\"image\":\"raw\"}"));
    assert_eq!(r.status, 202);
}

#[test]
fn images_and_the_flash_status_show_the_board_revision() {
    let rig = Rig::started();
    put_images(
        &rig,
        &[
            ("fw", stm_image(true, "1.4.9_Dev", "C2")),
            ("old", stm_image(true, "1.4.9_Dev", "C1")),
        ],
    );
    rig.state().snapshot.flash_pending = true;
    let st = rig.storage();
    st.service(); // scans fw only
    let mut web = rig.web(&st);
    let r = text(&perform(&mut web, get("/api/stm/images")));
    assert!(r.contains("\"name\":\"fw\""));
    assert!(r.contains("\"hw\":\"C2\"}"));
    assert!(r.contains("\"check\":null,\"hw\":null}"), "{r}");
    let r = text(&perform(&mut web, get("/api/stm/flash")));
    assert!(r.contains("\"pending\":true}"));
}
