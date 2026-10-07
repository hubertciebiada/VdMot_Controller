//! `test_web_server__mut_b2.cpp`: config save and dry run, the import report, health, the log
//! download, static assets, the JSON body, and uploads (each storage and OTA failure, several
//! files, disconnects).
//!
//! Retired (design 5.2): "a second body while one arrives is refused and never mixed in" and "a
//! second upload while one runs is refused and never mixed in" (concurrent bodies), "a finished
//! upload's disconnect leaves the next upload alone" (AsyncWebServer's disconnect callbacks),
//! the busy-slot parts of "a failed dry run holds no response slot" and "busy slots, an empty
//! and an oversized file", the concurrent download of "one download at a time", and the response
//! allocation failures of the library (`failNextResponse`: health, log, static). No Rust form:
//! storage results the web never meets at an upload's begin (Empty, TooLarge, NotFound; Busy
//! and OTA "busy" are refused before, 409 `busy`), and the 160-byte image answer (a fresh
//! upload's entry is never scanned: at most about 100 bytes).
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use vdm_esp_core::common::NO_VALVE;
use vdm_esp_core::event_log::EventCode;
use vdm_esp_core::json_api::HttpMethod;

use super::rig::*;
use super::{image_http_code, RESPONSE_SIZE};
use crate::logger::{LOG_FILE, LOG_FILE_OLD};
use crate::port::{BodyRead, HttpRequest};
use crate::storage::{ImageResult, FS_RESERVE, IMPORT_REPORT_FILE, MAX_IMAGE_SIZE};
use crate::testkit::FakeRequest;

const STM_UPLOAD: &str = "/api/stm/images";
const ESP_UPLOAD: &str = "/api/ota/esp";

/// A request whose client goes away after `chunks` chunks of a chunked answer.
struct GoneAfterChunks {
    inner: FakeRequest,
    chunks: u32,
}

impl HttpRequest for GoneAfterChunks {
    fn method(&self) -> HttpMethod {
        self.inner.method()
    }
    fn target(&self) -> &[u8] {
        self.inner.target()
    }
    fn header(&self, name: &str, out: &mut [u8]) -> Option<usize> {
        self.inner.header(name, out)
    }
    fn content_length(&self) -> usize {
        self.inner.content_length()
    }
    fn remote_ip(&self) -> u32 {
        self.inner.remote_ip()
    }
    fn local_ip(&self) -> u32 {
        self.inner.local_ip()
    }
    fn read_body(&mut self, out: &mut [u8]) -> BodyRead {
        self.inner.read_body(out)
    }
    fn respond(&mut self, status: u16, ct: &str, h: &[(&str, &str)], body: &[u8]) -> bool {
        self.inner.respond(status, ct, h, body)
    }
    fn begin_chunked(&mut self, status: u16, ct: &str, h: &[(&str, &str)]) -> bool {
        self.inner.begin_chunked(status, ct, h)
    }
    fn chunk(&mut self, data: &[u8]) -> bool {
        if self.chunks == 0 {
            self.inner.client_gone = true;
        }
        self.chunks = self.chunks.saturating_sub(1);
        self.inner.chunk(data)
    }
}

// ---------------------------------------------------------------- config

#[test]
fn config_get_is_inline_no_download() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/config"));
    assert_eq!(r.status, 200);
    assert_eq!(r.header("Content-Disposition"), "");
    assert_eq!(r.header("Cache-Control"), "no-store");
    assert!(text(&r).starts_with("{\"schema\":"));
}

#[test]
fn config_a_save_needs_a_body_also_after_an_earlier_body() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let revision = rig.storage_shared.config_revision();
    let r = perform(&mut web, api_post("/api/config", ""));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "JSON body required"));
    assert_eq!(
        perform(
            &mut web,
            api_post("/api/config", "{\"calib\":{\"hour\":4}}")
        )
        .status,
        200
    );
    let r = perform(&mut web, api_post("/api/config", ""));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "JSON body required"));
    let r = perform(&mut web, api_post("/api/config?dryRun=1", ""));
    assert_eq!(text(&r), error_body("bad_request", "JSON body required"));
    // a one-byte body is parsed (and malformed)
    let r = perform(&mut web, api_post("/api/config", "{"));
    assert_eq!(r.status, 400);
    assert!(text(&r).starts_with("{\"error\":\"invalid\",\"detail\":\"@"));
    assert_eq!(rig.storage_shared.config_revision(), revision + 1);
}

#[test]
fn config_a_failed_dry_run_leaves_the_copy_as_the_active_config() {
    // C++ "a failed dry run holds no response slot": no slots; the patched copy is reloaded
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(
        &mut web,
        api_post("/api/config?dryRun=1", "{\"calib\":{\"hour\":24}}"),
    );
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("invalid", "calib.hour"));
    let r = perform(
        &mut web,
        api_post("/api/config?dryRun=1", "{\"station\":\"Renamed\"}"),
    );
    assert_eq!(r.status, 200);
    // the guard still knows the device by its name, the document shows the active config
    let r = perform(&mut web, get("/api/config"));
    assert_eq!(r.status, 200);
    assert!(text(&r).contains("\"station\":\"VdMot\""));
    assert!(text(&r).contains("\"hour\":0,"));
}

#[test]
fn config_a_save_logs_the_new_revision() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let revision = rig.storage_shared.config_revision();
    let r = perform(
        &mut web,
        api_post("/api/config", "{\"calib\":{\"hour\":4}}"),
    );
    assert_eq!(r.status, 200);
    let ev = rig.host.with_code(EventCode::ConfigSaved);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].valve, NO_VALVE);
    assert_eq!(ev[0].arg1, (revision + 1) as i32);
    assert_eq!(ev[0].arg2, 0);
    assert_eq!(ev[0].text.as_slice(), b"web");
    assert_eq!(r.header("Cache-Control"), "no-store");
    assert!(text(&r).contains("\"hour\":4,"));
}

// ---------------------------------------------------------------- import report

#[test]
fn import_report_an_empty_and_an_oversized_file() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    rig.dev.fs.put(IMPORT_REPORT_FILE, b"x");
    let r = perform(&mut web, get("/api/import-report"));
    assert_eq!(r.status, 200);
    assert_eq!(text(&r), "x");
    assert_eq!(r.header("Cache-Control"), "no-store");
    rig.dev.fs.put(IMPORT_REPORT_FILE, b"");
    let r = perform(&mut web, get("/api/import-report"));
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("io_error", IMPORT_REPORT_FILE));
    rig.dev
        .fs
        .put(IMPORT_REPORT_FILE, &vec![b'a'; RESPONSE_SIZE]);
    let r = perform(&mut web, get("/api/import-report"));
    assert_eq!(r.status, 200);
    assert_eq!(r.body.len(), RESPONSE_SIZE);
    rig.dev
        .fs
        .put(IMPORT_REPORT_FILE, &vec![b'a'; RESPONSE_SIZE + 1]);
    let r = perform(&mut web, get("/api/import-report"));
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("io_error", IMPORT_REPORT_FILE));
    assert_eq!(rig.dev.fs.open_handles(), 0);
    // a report that cannot be opened is none
    rig.dev.fs.fail("open", IMPORT_REPORT_FILE, 1);
    let r = perform(&mut web, get("/api/import-report"));
    assert_eq!(r.status, 404);
    assert_eq!(text(&r), error_body("not_found", "no import report"));
}

// ---------------------------------------------------------------- health

#[test]
fn health_a_document_too_large_and_no_memory_for_the_response() {
    let rig = Rig::started();
    rig.host.set_health_version(&"v".repeat(1100));
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/health"));
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("internal", "health"));
    let rig = Rig::started();
    rig.host.set_health_version("2.1.0");
    let st = rig.storage();
    let mut web = rig.web(&st);
    // the working set, then no response buffer
    rig.dev
        .heap
        .state()
        .next
        .extend([true, true, true, true, false]);
    let r = perform(&mut web, get("/api/health"));
    assert_eq!(r.status, 503);
    assert_eq!(text(&r), error_body("busy", "out of memory"));
    assert_eq!(perform(&mut web, get("/api/health")).status, 200);
}

// ---------------------------------------------------------------- log download

#[test]
fn log_the_previous_file_first_then_the_current_one() {
    let rig = Rig::started();
    rig.dev.fs.put(LOG_FILE_OLD, b"#1 old\n");
    rig.dev.fs.put(LOG_FILE, b"#2 new\n");
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/log"));
    assert_eq!(r.status, 200);
    assert!(r.chunked);
    assert!(r.ended);
    assert_eq!(r.content_type, "text/plain; charset=utf-8");
    assert_eq!(
        r.header("Content-Disposition"),
        "attachment; filename=\"vdmot-events.log\""
    );
    assert_eq!(text(&r), "#1 old\n#2 new\n");
    assert_eq!(r.chunks, 2);
    // one-byte files are not lost
    rig.dev.fs.put(LOG_FILE_OLD, b"A");
    rig.dev.fs.put(LOG_FILE, b"B");
    let r = perform(&mut web, get("/api/log"));
    assert_eq!(r.status, 200);
    assert_eq!(text(&r), "AB");
    // only the previous file
    assert!(crate::port::Fs::remove(&rig.dev.fs, LOG_FILE));
    assert_eq!(text(&perform(&mut web, get("/api/log"))), "A");
    // a file larger than the buffer goes out in chunks of the buffer
    rig.dev.fs.put(LOG_FILE, &vec![b'x'; RESPONSE_SIZE + 5]);
    let r = perform(&mut web, get("/api/log"));
    assert_eq!(r.body.len(), 1 + RESPONSE_SIZE + 5);
    assert_eq!(r.chunks, 3);
}

#[test]
fn log_a_client_gone_ends_the_download_none_without_a_file_system() {
    let rig = Rig::started();
    rig.dev.fs.put(LOG_FILE_OLD, &vec![b'o'; RESPONSE_SIZE * 2]);
    rig.dev.fs.put(LOG_FILE, b"#1 line\n");
    let st = rig.storage();
    let mut web = rig.web(&st);
    let mut gone = GoneAfterChunks {
        inner: get("/api/log"),
        chunks: 1,
    };
    web.handle(&mut gone);
    assert_eq!(gone.inner.response.chunks, 2); // the second went to a client that was gone
    assert!(!gone.inner.response.ended);
    assert_eq!(rig.dev.fs.open_handles(), 0);
    drop(gone);
    // the next download is whole
    let r = perform(&mut web, get("/api/log"));
    assert_eq!(r.status, 200);
    assert_eq!(r.body.len(), RESPONSE_SIZE * 2 + 8);
    drop(web);
    let rig = Rig::started();
    let st = rig.storage_unmounted();
    let mut web = rig.web(&st);
    let r = perform(&mut web, get("/api/log"));
    assert_eq!(r.status, 503);
    assert_eq!(text(&r), error_body("unavailable", "no file system"));
    assert_eq!(rig.state().flush_requests, 0);
}

// ---------------------------------------------------------------- static assets

#[test]
fn static_every_asset_is_served() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    assert!(!ASSETS.is_empty());
    for a in ASSETS {
        let r = perform(&mut web, get(a.path));
        assert_eq!(r.status, 200, "{}", a.path);
        assert_eq!(r.header("ETag"), a.etag);
        assert_eq!(r.body, a.data);
        assert_eq!(r.content_type, a.content_type);
    }
    // an ETag of another file is no match
    let other = get("/app.js").with_header("If-None-Match", ASSETS[0].etag);
    assert_eq!(perform(&mut web, other).status, 200);
    // a HEAD is not a GET (C++ HTTP_HEAD): 405
    let head = request(HttpMethod::Other, "/");
    assert_eq!(perform(&mut web, head).status, 405);
}

// ---------------------------------------------------------------- JSON body

#[test]
fn body_a_client_gone_mid_body_gets_no_answer_the_next_body_is_read() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let mut a = api_post("/api/valves/1/target", "{\"target\":50}");
    a.gone_at = Some(4);
    let a = served(&mut web, a);
    assert_eq!(a.response.answers, 0);
    assert!(a.client_gone);
    drop(a);
    let r = perform(&mut web, api_post("/api/valves/1/target", "{\"target\":9}"));
    assert_eq!(r.status, 202);
    let s = rig.submitted();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].pos, 9);
}

// ---------------------------------------------------------------- uploads

#[test]
fn upload_each_storage_result_of_the_begin_has_its_status() {
    let rig = Rig::started();
    for (name, size) in [("a1", 1), ("a2", 1), ("a3", 1)] {
        rig.dev
            .fs
            .put(&format!("/stm/{name}.bin"), &vec![0u8; size]);
    }
    let st = rig.storage();
    let mut web = rig.web(&st);
    for (filename, code, error) in [
        ("a b.bin", 400, ImageResult::BadName),
        ("last_good.bin", 400, ImageResult::BadName),
        ("fw.bin", 507, ImageResult::TooMany),
    ] {
        let r = perform(&mut web, api_upload(STM_UPLOAD, filename, b"abc"));
        assert_eq!(r.status, code, "{filename}");
        assert_eq!(
            text(&r),
            error_body("upload_failed", crate::storage::image_result_name(error))
        );
    }
    assert!(st.delete_image(b"a3") == ImageResult::Ok);
    let used = st.fs_used() as usize;
    rig.dev.fs.knobs().total_bytes = (used + FS_RESERVE) as u32;
    let r = perform(&mut web, api_upload(STM_UPLOAD, "fw.bin", b"abc"));
    assert_eq!(r.status, 507);
    assert_eq!(text(&r), error_body("upload_failed", "no_space"));
    rig.dev.fs.knobs().total_bytes = 0x17_0000;
    rig.dev.fs.fail("open", "/stm/fw.bin.part", 1);
    let r = perform(&mut web, api_upload(STM_UPLOAD, "fw.bin", b"abc"));
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("upload_failed", "io_error"));
    assert!(rig.dev.fs.read("/stm/fw.bin").is_none());
    assert_eq!(rig.dev.ota.knobs().begins, 0);
    // without a file system
    drop(web);
    let rig = Rig::started();
    let st = rig.storage_unmounted();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_upload(STM_UPLOAD, "fw.bin", b"abc"));
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("upload_failed", "io_error"));
}

#[test]
fn upload_the_status_of_every_storage_result() {
    use ImageResult as R;
    let codes = [
        (R::Ok, 500),
        (R::BadName, 400),
        (R::TooLarge, 413),
        (R::NoSpace, 507),
        (R::TooMany, 507),
        (R::Busy, 409),
        (R::Io, 500),
        (R::NotFound, 500),
        (R::Empty, 400),
    ];
    for (r, code) in codes {
        assert_eq!(image_http_code(r), code, "{r:?}");
    }
}

#[test]
fn upload_a_failed_write_or_end_of_an_stm_image() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(
        &mut web,
        api_upload(STM_UPLOAD, "fw.bin", &vec![1u8; MAX_IMAGE_SIZE + 1]),
    );
    assert_eq!(r.status, 413);
    assert_eq!(text(&r), error_body("upload_failed", "too_large"));
    assert!(rig.dev.fs.read("/stm/fw.bin.part").is_none());
    rig.dev.fs.fail("write", "/stm/fw.bin.part", 1);
    let r = perform(&mut web, api_upload(STM_UPLOAD, "fw.bin", b"abc"));
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("upload_failed", "io_error"));
    assert!(rig.dev.fs.read("/stm/fw.bin.part").is_none());
    // the end: the rename fails
    rig.dev.fs.fail("rename", "/stm/fw.bin.part", 1);
    let r = perform(&mut web, api_upload(STM_UPLOAD, "fw.bin", b"abc"));
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("upload_failed", "io_error"));
    assert!(st.find_image(b"fw").is_none());
    assert!(!st.image_upload_active());
}

#[test]
fn upload_esp_image_begin_write_and_end_failures() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let image = esp_image(64);
    rig.state().image_upload_active = true;
    let r = perform(&mut web, api_upload(ESP_UPLOAD, "fw.bin", &image));
    assert_eq!(r.status, 400);
    assert_eq!(
        text(&r),
        error_body("upload_failed", "stm flash or upload running")
    );
    rig.state().image_upload_active = false;
    let r = perform(
        &mut web,
        api_upload(&format!("{ESP_UPLOAD}?md5=xyz"), "fw.bin", &image),
    );
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("upload_failed", "md5 invalid"));
    assert_eq!(rig.dev.ota.knobs().begins, 0);
    rig.dev.ota.knobs().fail_write_at = Some(0);
    let r = perform(&mut web, api_upload(ESP_UPLOAD, "fw.bin", &esp_image(5000)));
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("upload_failed", "Flash Write Failed"));
    assert!(!rig.ota_shared.upload_active());
    rig.dev.ota.knobs().fail_write_at = None;
    let wrong = "0123456789abcdef0123456789abcdef";
    let r = perform(
        &mut web,
        api_upload(&format!("{ESP_UPLOAD}?md5={wrong}"), "fw.bin", &image),
    );
    assert_eq!(r.status, 500);
    assert_eq!(text(&r), error_body("upload_failed", "MD5 Check Failed"));
    assert!(!rig.ota_shared.restart_pending());
    assert!(st.find_image(b"fw").is_none());
}

#[test]
fn upload_the_md5_from_a_form_field_or_a_header() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let image = esp_image(64);
    let md5 = md5_hex(&image);
    let wrong = "fedcba9876543210fedcba9876543210";
    let field =
        |v: &str| upload(ESP_UPLOAD, "fw.bin", &image, &[("md5", v)]).with_header("X-VdMot", "1");
    assert_eq!(perform(&mut web, field(&md5)).status, 200);
    let r = perform(&mut web, field(wrong));
    assert_eq!(text(&r), error_body("upload_failed", "MD5 Check Failed"));
    let upper = upload(ESP_UPLOAD, "fw.bin", &image, &[("MD5", wrong)]).with_header("X-VdMot", "1");
    assert_eq!(perform(&mut web, upper).status, 500);
    let header =
        |name: &str, v: &str| api_upload(ESP_UPLOAD, "fw.bin", &image).with_header(name, v);
    assert_eq!(perform(&mut web, header("X-Update-MD5", &md5)).status, 200);
    assert_eq!(perform(&mut web, header("X-Update-MD5", wrong)).status, 500);
    assert_eq!(perform(&mut web, header("X-MD5", wrong)).status, 500);
    assert_eq!(
        perform(&mut web, api_upload(ESP_UPLOAD, "fw.bin", &image)).status,
        200
    );
    // the query comes first, then the field, then the header
    let q = upload(
        &format!("{ESP_UPLOAD}?md5={md5}"),
        "fw.bin",
        &image,
        &[("md5", wrong)],
    )
    .with_header("X-VdMot", "1")
    .with_header("X-Update-MD5", wrong);
    assert_eq!(perform(&mut web, q).status, 200);
    let f = upload(ESP_UPLOAD, "fw.bin", &image, &[("md5", &md5)])
        .with_header("X-VdMot", "1")
        .with_header("X-Update-MD5", wrong);
    assert_eq!(perform(&mut web, f).status, 200);
    // an empty query value is a given one: no MD5
    let empty = upload(
        &format!("{ESP_UPLOAD}?md5="),
        "fw.bin",
        &image,
        &[("md5", wrong)],
    )
    .with_header("X-VdMot", "1");
    assert_eq!(perform(&mut web, empty).status, 200);
}

#[test]
fn upload_one_file_per_request() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let parts = |first: &str| {
        multipart(
            STM_UPLOAD,
            &[
                Part::file("file", first, b"abc"),
                Part::file("file2", "fw2.bin", b"def"),
            ],
        )
        .with_header("X-VdMot", "1")
    };
    let r = perform(&mut web, parts("fw.bin"));
    assert_eq!(r.status, 400);
    assert_eq!(
        text(&r),
        error_body("upload_failed", "one file per request")
    );
    assert_eq!(rig.dev.fs.read("/stm/fw.bin").unwrap(), b"abc");
    assert!(rig.dev.fs.read("/stm/fw2.bin").is_none());
    // a file whose begin failed stays the answer, a second file does not replace it
    let r = perform(&mut web, parts("a b.bin"));
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("upload_failed", "bad_name"));
    assert!(rig.dev.fs.read("/stm/fw2.bin").is_none());
}

#[test]
fn upload_a_request_without_a_file_part() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let none = multipart(STM_UPLOAD, &[Part::field("md5", "x")]).with_header("X-VdMot", "1");
    let r = perform(&mut web, none);
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "no file in request"));
    assert!(st.find_image(b"fw").is_none());
}

#[test]
fn upload_a_file_cut_short_is_incomplete_and_aborted() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let mut cut = api_upload(STM_UPLOAD, "fw.bin", &[b'x'; 100]);
    cut.content_length = Some(cut.body.len() - 10);
    cut.segment = 50; // a segment ends inside the file data: its first piece is delivered
    let r = perform(&mut web, cut);
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("upload_failed", "incomplete file"));
    assert!(rig.dev.fs.read("/stm/fw.bin.part").is_none());
    assert!(st.find_image(b"fw").is_none());
    let mut esp = api_upload(ESP_UPLOAD, "fw.bin", &esp_image(100));
    esp.content_length = Some(esp.body.len() - 10);
    esp.segment = 50;
    let r = perform(&mut web, esp);
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("upload_failed", "incomplete file"));
    let ev = rig.host.with_code(EventCode::EspOtaFailed);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, -1); // aborted
    assert!(!rig.ota_shared.upload_active());
}

#[test]
fn upload_a_client_gone_mid_upload_aborts_it() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let mut e = api_upload(STM_UPLOAD, "fw.bin", &[b'x'; 3000]);
    e.gone_at = Some(1000);
    let e = served(&mut web, e);
    assert_eq!(e.response.answers, 0);
    drop(e);
    assert!(!st.image_upload_active());
    assert!(rig.dev.fs.read("/stm/fw.bin.part").is_none());
    let mut o = api_upload(ESP_UPLOAD, "fw.bin", &esp_image(3000));
    o.gone_at = Some(1000);
    let o = served(&mut web, o);
    assert_eq!(o.response.answers, 0);
    drop(o);
    let ev = rig.host.with_code(EventCode::EspOtaFailed);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, -1);
    assert!(!rig.ota_shared.upload_active());
    // the state is free for the next upload
    assert_eq!(
        perform(&mut web, api_upload(STM_UPLOAD, "fw.bin", b"abc")).status,
        201
    );
}

#[test]
fn upload_the_answer_of_the_longest_name_and_size_is_whole() {
    // C++ "the image answer fits 159 bytes, a longer one is {}": the answer of a fresh upload
    // is never scanned, so it stays far below 160 bytes
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let name = "n".repeat(31);
    let data = vec![7u8; MAX_IMAGE_SIZE];
    let r = perform(
        &mut web,
        api_upload(STM_UPLOAD, &format!("{name}.bin"), &data),
    );
    assert_eq!(r.status, 201);
    assert_eq!(
        text(&r),
        format!(
            "{{\"name\":\"{name}\",\"size\":{MAX_IMAGE_SIZE},\"crc32\":\"0x{:08x}\",\
             \"version\":null,\"check\":null,\"hw\":null}}",
            vdm_esp_core::config::crc32(&data, 0)
        )
    );
    assert!(r.body.len() < 160);
}
