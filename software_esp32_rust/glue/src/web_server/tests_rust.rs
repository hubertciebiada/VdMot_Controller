//! New cases of the Rust server (design 4.7 and 7): the switch back to the other image, uploads
//! refused while the image is on trial, the serial server (stalled and silent clients, one
//! buffer), the multipart framing of the uploads, and the parts of the pipeline the C++ suites
//! reached only through the library (URL decoding, the simple STM routes, the time members).
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use std::collections::VecDeque;

use vdm_esp_core::common::ALL_VALVES;
use vdm_esp_core::event_log::{EventCode, Severity};
use vdm_esp_core::json_api::HttpMethod;
use vdm_esp_core::stm_types::StmCommandType;

use super::rig::*;
use super::{parse_epoch, BODY_TIMEOUTS};
use crate::boot_guard::BootGuard;
use crate::ota::{OtaPorts, OtaService};
use crate::port::{BodyRead, Clock, HttpRequest};
use crate::storage::ImageResult;
use crate::testkit::board::TestPlatform;
use crate::testkit::ota::SLOT_ADDR;
use crate::testkit::{run, Ended, FakeRequest};

/// A request whose first `zeros` body reads deliver nothing (`Data(0)`).
struct ZeroReads {
    inner: FakeRequest,
    zeros: u32,
}

impl HttpRequest for ZeroReads {
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
        if self.zeros > 0 {
            self.zeros -= 1;
            return BodyRead::Data(0);
        }
        self.inner.read_body(out)
    }
    fn respond(&mut self, status: u16, ct: &str, h: &[(&str, &str)], body: &[u8]) -> bool {
        self.inner.respond(status, ct, h, body)
    }
    fn begin_chunked(&mut self, status: u16, ct: &str, h: &[(&str, &str)]) -> bool {
        self.inner.begin_chunked(status, ct, h)
    }
    fn chunk(&mut self, data: &[u8]) -> bool {
        self.inner.chunk(data)
    }
}

/// `r` stalls `n` times (receive timeouts) once `at` body bytes were read (the segments end
/// there).
fn stalled(mut r: FakeRequest, at: usize, n: usize) -> FakeRequest {
    r.stalls = VecDeque::from(vec![at; n]);
    r.segment = at;
    r
}

// ---------------------------------------------------------------- switch back (design 6.3)

const SWITCH_BACK: &str = "/api/system/ota/switch-back";
const CONFIRMED: &str = "{\"confirm\":\"switch-back\"}";

#[test]
fn switch_back_restarts_into_the_other_image() {
    let rig = Rig::with_fallback();
    let st = rig.storage();
    let mut web = rig.web(&st);
    rig.dev.clock.set_ms(7000);
    let r = perform(&mut web, api_post(SWITCH_BACK, CONFIRMED));
    assert_eq!(r.status, 202);
    assert_eq!(r.content_type, "application/json");
    assert_eq!(text(&r), "{\"result\":\"restarting\"}");
    assert!(rig.ota_shared.restart_pending());
    let ev = rig.host.with_code(EventCode::RebootRequested);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].arg1, 7);
    assert_eq!(ev[0].severity, Severity::Info);
    // the restart path selects the other slot and restarts
    let d = &rig.dev;
    let (guard, _) = run(|| BootGuard::boot(&d.nvs, &d.ota, &d.rtc, &d.system)).returned();
    let ports = OtaPorts::<TestPlatform> {
        clock: &d.clock,
        tcp: &d.tcp,
        nvs: &d.nvs,
        ota: &d.ota,
        rtc: &d.rtc,
        system: &d.system,
    };
    let mut svc = OtaService::new(ports, &rig.ota_shared, &rig.host, guard);
    d.clock.set_ms(7999);
    svc.service_restart(d.clock.now_ms(), true, false);
    assert_eq!(rig.state().save_requests, 0);
    d.clock.set_ms(8000);
    svc.service_restart(d.clock.now_ms(), true, false); // asks the STM to save
    let ended = run(|| svc.service_restart(d.clock.now_ms(), true, false));
    assert_eq!(ended, Ended::Reset(crate::testkit::Reset::Software));
    assert_eq!(d.ota.knobs().set_boots, [SLOT_ADDR[1]]);
}

#[test]
fn switch_back_needs_the_confirmation() {
    let rig = Rig::with_fallback();
    let st = rig.storage();
    let mut web = rig.web(&st);
    for bad in [
        "{}",
        "{\"confirm\":\"switchback\"}",
        "{\"confirm\":true}",
        "{\"confirm\":\"factory-reset\"}",
    ] {
        let r = perform(&mut web, api_post(SWITCH_BACK, bad));
        assert_eq!(r.status, 400, "{bad}");
        assert_eq!(
            text(&r),
            error_body("confirm_required", "{\\\"confirm\\\":\\\"switch-back\\\"}")
        );
    }
    let r = perform(&mut web, api_post(SWITCH_BACK, ""));
    assert_eq!(text(&r), error_body("bad_request", "JSON body required"));
    let r = perform(&mut web, post(SWITCH_BACK, CONFIRMED)); // no X-VdMot
    assert_eq!(r.status, 403);
    assert!(!rig.ota_shared.restart_pending());
}

#[test]
fn switch_back_is_refused_during_an_upload_or_a_flash_and_while_restarting() {
    let rig = Rig::with_fallback();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let busy = error_body("busy", "upload or flash running");
    rig.state().flash_active = true;
    let r = perform(&mut web, api_post(SWITCH_BACK, CONFIRMED));
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), busy);
    rig.state().flash_active = false;
    let mut other = rig.ota_upload();
    assert!(other.upload_begin(10, b""));
    assert_eq!(
        text(&perform(&mut web, api_post(SWITCH_BACK, CONFIRMED))),
        busy
    );
    assert!(!other.upload_end(false));
    assert_eq!(st.image_upload_begin(b"x", 10), ImageResult::Ok);
    assert_eq!(
        text(&perform(&mut web, api_post(SWITCH_BACK, CONFIRMED))),
        busy
    );
    st.image_upload_abort();
    assert!(!rig.ota_shared.restart_pending());
    // a pending restart: another request is refused
    let now = rig.dev.clock.now_ms();
    let _ = rig.ota_shared.request_restart(now, 0, 1000, 0);
    let r = perform(&mut web, api_post(SWITCH_BACK, CONFIRMED));
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("restarting", "ESP restart pending"));
    assert!(rig.host.with_code(EventCode::RebootRequested).is_empty());
}

#[test]
fn switch_back_without_an_image_in_the_other_slot() {
    let rig = Rig::new(); // slot 1 empty
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_post(SWITCH_BACK, CONFIRMED));
    assert_eq!(r.status, 409);
    assert_eq!(
        text(&r),
        error_body("no_fallback", "no image in the other slot")
    );
    // nor without a second slot
    let rig = Rig::with_fallback();
    rig.dev.ota.knobs().no_other = true;
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_post(SWITCH_BACK, CONFIRMED));
    assert_eq!(r.status, 409);
    assert!(!rig.ota_shared.restart_pending());
}

// ---------------------------------------------------------------- the image on trial (4.7 row 10)

#[test]
fn an_esp_upload_is_refused_while_the_image_is_on_trial() {
    let rig = Rig::with_fallback(); // A runs, not confirmed: on trial with B as the fallback
    let d = &rig.dev;
    let (guard, report) = run(|| BootGuard::boot(&d.nvs, &d.ota, &d.rtc, &d.system)).returned();
    let ports = OtaPorts::<TestPlatform> {
        clock: &d.clock,
        tcp: &d.tcp,
        nvs: &d.nvs,
        ota: &d.ota,
        rtc: &d.rtc,
        system: &d.system,
    };
    let mut svc = OtaService::new(ports, &rig.ota_shared, &rig.host, guard);
    svc.begin(&report);
    assert!(rig.ota_shared.on_trial());
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(
        &mut web,
        api_upload("/api/ota/esp", "fw.bin", &esp_image(64)),
    );
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("upload_failed", "image on trial"));
    assert_eq!(d.ota.knobs().begins, 0);
    // an STM image is not refused
    assert_eq!(
        perform(&mut web, api_upload("/api/stm/images", "fw.bin", b"abc")).status,
        201
    );
}

// ---------------------------------------------------------------- the serial server (4.7)

#[test]
fn body_a_client_that_stalls_three_times_in_a_row_gets_no_answer() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let mut r = stalled(api_post("/api/valves/1/target", "{\"target\":50}"), 4, 3);
    r.unchecked = true; // no answer: the adapter closes the session
    let r = served(&mut web, r);
    assert_eq!(r.response.answers, 0);
    assert_eq!(r.body_read(), 4);
    drop(r);
    assert!(rig.submitted().is_empty());
    // two timeouts, then the rest: answered; the count starts again after data
    let r = api_post("/api/valves/1/target", "{\"target\":50}");
    let mut r = stalled(r, 4, 2);
    r.stalls.extend([8, 8]);
    assert_eq!(perform(&mut web, r).status, 202);
    assert_eq!(rig.submitted().len(), 1);
    assert_eq!(BODY_TIMEOUTS, 3);
}

#[test]
fn body_reads_that_deliver_nothing_count_as_stalls() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let mut silent = ZeroReads {
        inner: api_post("/api/valves/1/target", "{\"target\":50}"),
        zeros: 3,
    };
    silent.inner.unchecked = true;
    web.handle(&mut silent);
    assert_eq!(silent.inner.response.answers, 0);
    let mut slow = ZeroReads {
        inner: api_post("/api/valves/1/target", "{\"target\":50}"),
        zeros: 2,
    };
    web.handle(&mut slow);
    assert_eq!(slow.inner.response.status, 202);
    // an upload too
    let mut up = ZeroReads {
        inner: api_upload("/api/stm/images", "fw.bin", b"abc"),
        zeros: 3,
    };
    up.inner.unchecked = true;
    web.handle(&mut up);
    assert_eq!(up.inner.response.answers, 0);
    let mut up = ZeroReads {
        inner: api_upload("/api/stm/images", "fw.bin", b"abc"),
        zeros: 2,
    };
    web.handle(&mut up);
    assert_eq!(up.inner.response.status, 201);
}

#[test]
fn upload_a_client_that_stalls_three_times_in_a_row_aborts_it() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let mut u = stalled(
        api_upload("/api/stm/images", "fw.bin", &[b'x'; 3000]),
        1460,
        3,
    );
    u.unchecked = true;
    let u = served(&mut web, u);
    assert_eq!(u.response.answers, 0);
    drop(u);
    assert!(!st.image_upload_active());
    assert!(rig.dev.fs.read("/stm/fw.bin.part").is_none());
    let mut o = stalled(
        api_upload("/api/ota/esp", "fw.bin", &esp_image(3000)),
        1460,
        3,
    );
    o.unchecked = true;
    let o = served(&mut web, o);
    assert_eq!(o.response.answers, 0);
    drop(o);
    assert!(!rig.ota_shared.upload_active());
    assert_eq!(rig.host.with_code(EventCode::EspOtaFailed)[0].arg1, -1);
    // two stalls only: the upload completes
    let u = stalled(
        api_upload("/api/stm/images", "fw.bin", &[b'y'; 3000]),
        1460,
        2,
    );
    assert_eq!(perform(&mut web, u).status, 201);
    assert_eq!(rig.dev.fs.read("/stm/fw.bin").unwrap(), [b'y'; 3000]);
}

#[test]
fn upload_sleeps_one_tick_per_chunk() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let u = api_upload("/api/stm/images", "fw.bin", &[b'z'; 4000]);
    let chunks = u.body.len().div_ceil(1460);
    assert_eq!(perform(&mut web, u).status, 201);
    assert_eq!(rig.dev.clock.sleeps(), vec![1; chunks]);
    // a JSON body does not sleep
    assert_eq!(
        perform(&mut web, api_post("/api/valves/1/target", "{\"target\":5}")).status,
        202
    );
    assert_eq!(rig.dev.clock.sleeps().len(), chunks);
}

// ---------------------------------------------------------------- multipart framing (4.7 row 7)

#[test]
fn upload_bytes_after_the_close_delimiter_are_ignored() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let mut u = api_upload("/api/stm/images", "fw.bin", b"abc");
    u.body.extend_from_slice(b"epilogue of the client");
    let r = perform(&mut web, u);
    assert_eq!(r.status, 201);
    assert_eq!(rig.dev.fs.read("/stm/fw.bin").unwrap(), b"abc");
}

#[test]
fn upload_a_body_without_the_boundary_has_no_file() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    // a preamble before the first boundary
    let mut u = api_upload("/api/stm/images", "fw.bin", b"abc");
    u.body.splice(0..0, b"preamble\r\n".iter().copied());
    let r = perform(&mut web, u);
    assert_eq!(r.status, 400);
    assert_eq!(text(&r), error_body("bad_request", "no file in request"));
    // no boundary in the Content-Type
    let u = with_type(
        api_upload("/api/stm/images", "fw.bin", b"abc"),
        "multipart/form-data",
    );
    let r = perform(&mut web, u);
    assert_eq!(text(&r), error_body("bad_request", "no file in request"));
    // an empty file part begins no upload
    let empty = multipart("/api/stm/images", &[Part::file("file", "fw.bin", b"")])
        .with_header("X-VdMot", "1");
    let r = perform(&mut web, empty);
    assert_eq!(text(&r), error_body("bad_request", "no file in request"));
    assert!(st.find_image(b"fw").is_none());
}

#[test]
fn upload_an_empty_file_part_before_the_file_begins_nothing() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let parts = multipart(
        "/api/stm/images",
        &[
            Part::file("file", "a.bin", b""),
            Part::file("file", "fw.bin", b"abc"),
        ],
    )
    .with_header("X-VdMot", "1");
    let r = perform(&mut web, parts);
    assert_eq!(r.status, 201);
    assert!(text(&r).starts_with("{\"name\":\"fw\","));
    assert!(st.find_image(b"a").is_none());
}

#[test]
fn upload_md5_fields_count_only_before_the_file_the_first_of_a_name_wins() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    let image = esp_image(64);
    let md5 = md5_hex(&image);
    let wrong = "0123456789abcdef0123456789abcdef";
    let url = "/api/ota/esp";
    // a field after the file part does not count
    let after = multipart(
        url,
        &[
            Part::file("file", "fw.bin", &image),
            Part::field("md5", wrong),
        ],
    )
    .with_header("X-VdMot", "1");
    assert_eq!(perform(&mut web, after).status, 200);
    // the first md5 field wins
    let twice = multipart(
        url,
        &[
            Part::field("md5", &md5),
            Part::field("md5", wrong),
            Part::file("file", "fw.bin", &image),
        ],
    )
    .with_header("X-VdMot", "1");
    assert_eq!(perform(&mut web, twice).status, 200);
    let twice = multipart(
        url,
        &[
            Part::field("md5", wrong),
            Part::field("md5", &md5),
            Part::file("file", "fw.bin", &image),
        ],
    )
    .with_header("X-VdMot", "1");
    assert_eq!(perform(&mut web, twice).status, 500);
    // "md5" before "MD5", another field name is no MD5
    let both = multipart(
        url,
        &[
            Part::field("MD5", wrong),
            Part::field("md5", &md5),
            Part::field("Md5", wrong),
            Part::file("file", "fw.bin", &image),
        ],
    )
    .with_header("X-VdMot", "1");
    assert_eq!(perform(&mut web, both).status, 200);
    let upper = multipart(
        url,
        &[
            Part::field("MD5", &md5),
            Part::file("file", "fw.bin", &image),
        ],
    )
    .with_header("X-VdMot", "1")
    .with_header("X-Update-MD5", wrong);
    assert_eq!(perform(&mut web, upper).status, 200);
}

// ---------------------------------------------------------------- the pipeline

#[test]
fn the_path_is_decoded_before_it_is_routed() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    assert_eq!(perform(&mut web, get("/api/st%61tus")).status, 200);
    // a NUL in the path: no route, the detail ends at it (a C string)
    let r = perform(&mut web, get("/api/status%00x"));
    assert_eq!(r.status, 404);
    assert_eq!(text(&r), error_body("not_found", "/api/status"));
    // a dashboard path is compared as a C string
    let r = perform(&mut web, get("/index.html%00junk"));
    assert_eq!(r.status, 200);
    assert_eq!(r.header("ETag"), "\"1a2b3c4d\"");
    // a query is not part of the path
    assert_eq!(perform(&mut web, get("/?x=1")).status, 200);
}

#[test]
fn every_simple_stm_route_queues_its_command() {
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web(&st);
    for url in [
        "/api/valves/2/calibrate",
        "/api/valves/3/assembly",
        "/api/valves/calibrate",
        "/api/valves/assembly",
        "/api/valves/detect",
        "/api/sensors/scan",
    ] {
        let r = perform(&mut web, api_post(url, ""));
        assert_eq!(r.status, 202, "{url}");
        assert_eq!(text(&r), "{\"result\":\"queued\"}");
    }
    let s: Vec<(StmCommandType, u8)> = rig.submitted().iter().map(|c| (c.kind, c.valve)).collect();
    assert_eq!(
        s,
        [
            (StmCommandType::Calibrate, 1),
            (StmCommandType::Assembly, 2),
            (StmCommandType::Calibrate, ALL_VALVES),
            (StmCommandType::Assembly, ALL_VALVES),
            (StmCommandType::Detect, ALL_VALVES),
            (StmCommandType::ScanSensors, vdm_esp_core::common::NO_VALVE),
        ]
    );
    rig.state().flash_active = true;
    let r = perform(&mut web, api_post("/api/valves/detect", ""));
    assert_eq!(r.status, 409);
    assert_eq!(text(&r), error_body("flashing", "STM flash in progress"));
}

#[test]
fn status_shows_the_clock_the_last_sync_and_the_board_figures() {
    let rig = Rig::started();
    rig.state().last_sync = 1_790_000_000;
    rig.dev.wall.set(1_790_000_100);
    rig.state().info.ip = 0x3301_A8C0;
    rig.state().snapshot.proto = 3;
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = text(&perform(&mut web, get("/api/status")));
    assert!(
        r.contains("\"time\":{\"valid\":true,\"epoch\":1790000100,\"local\":\"2026-09-21T14:15:00\",\"lastSync\":1790000000}"),
        "{r}"
    );
    assert!(r.contains("\"ip\":\"192.168.1.51\""));
    assert!(r.contains("\"proto\":3,"));
    assert!(r.contains("\"heap\":{\"free\":150000,\"min\":120000,\"largest\":110000}"));
}

#[test]
fn answers_without_content_carry_no_content_type() {
    let rig = Rig::started();
    rig.dev.fs.put("/x.bin", b"x");
    let st = rig.storage();
    let mut web = rig.web(&st);
    let r = perform(&mut web, api_del("/api/files?path=/x.bin"));
    assert_eq!((r.status, r.content_type.as_str()), (204, ""));
    let etag = ASSETS[0].etag;
    let r = perform(
        &mut web,
        get(ASSETS[0].path).with_header("If-None-Match", etag),
    );
    assert_eq!((r.status, r.content_type.as_str()), (304, ""));
    assert!(r.headers.is_empty());
}

/// The table of the firmware (`--features vdm-esp-glue/dashboard`): every file of the dashboard,
/// gzip as gen_web_assets.py writes it (the trailer proves the source), its ETag, served.
#[cfg(feature = "dashboard")]
#[test]
fn the_dashboard_is_every_file_of_web_gzipped_with_its_etag() {
    use super::assets::DASHBOARD;
    use vdm_esp_core::config::crc32;
    assert_eq!(crc32(b"123456789", 0), 0xCBF4_3926); // the CRC-32 of zlib
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../software_esp32_revamped/web");
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| !n.starts_with('.'))
        .map(|n| format!("/{n}"))
        .collect();
    files.sort();
    let paths: Vec<String> = DASHBOARD.iter().map(|a| a.path.to_string()).collect();
    assert_eq!(paths, files);
    let mut total = 0;
    for a in DASHBOARD {
        let src = std::fs::read(dir.join(&a.path[1..])).unwrap();
        let d = a.data;
        let n = d.len();
        // magic, deflate, no flags, mtime 0, XFL 2 (level 9), OS 255 (Python's gzip)
        assert_eq!(
            d[..10],
            [0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 2, 0xff],
            "{}",
            a.path
        );
        assert_eq!(d[n - 8..n - 4], crc32(&src, 0).to_le_bytes(), "{}", a.path);
        assert_eq!(d[n - 4..], (src.len() as u32).to_le_bytes(), "{}", a.path);
        assert_eq!(a.etag, format!("\"{:08x}\"", crc32(d, 0)));
        let ext = a.path.rsplit('.').next().unwrap();
        let want = match ext {
            "html" => "text/html; charset=utf-8",
            "js" => "application/javascript; charset=utf-8",
            "css" => "text/css; charset=utf-8",
            other => panic!("unexpected file type {other}"),
        };
        assert_eq!(a.content_type, want);
        total += n;
    }
    assert!(total <= 160 * 1024);
    let rig = Rig::started();
    let st = rig.storage();
    let mut web = rig.web_with(&st, DASHBOARD);
    for a in DASHBOARD {
        let r = perform(&mut web, get(a.path));
        assert_eq!(r.status, 200);
        assert_eq!(r.body, a.data);
        assert_eq!(r.header("ETag"), a.etag);
    }
    let index = DASHBOARD.iter().find(|a| a.path == "/index.html").unwrap();
    assert_eq!(perform(&mut web, get("/")).body, index.data);
}

#[test]
fn a_build_epoch_is_read_from_its_decimal_text() {
    assert_eq!(parse_epoch(b"1790000000"), 1_790_000_000);
    assert_eq!(parse_epoch(b"0"), 0);
    assert_eq!(parse_epoch(b""), 0);
    assert_eq!(parse_epoch(b"17x"), 0);
    assert_eq!(parse_epoch(b"4294967295"), u32::MAX);
}
