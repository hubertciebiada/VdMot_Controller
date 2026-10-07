//! The rig of the web server tests (C++ `glue_test.h` with the sibling fakes of app, logger,
//! net, mqtt and ota, and `fakes::http`): a booted device with LittleFS mounted (the C++ default
//! `fsReady`), the real storage and ESP upload over the fake ports, one fake [`Host`] for the
//! web, storage and OTA host roles, request builders and the request driver.
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays, dead_code)]

use std::sync::{Mutex, MutexGuard};

use vdm_esp_core::common::{copy_string, LocalTime, OneWireId, VALVE_COUNT};
use vdm_esp_core::config::Config;
use vdm_esp_core::event_log::{Event, EventCode, EventFilter};
use vdm_esp_core::json_api::{HealthSnapshot, HttpMethod};
use vdm_esp_core::link_policy::LinkState;
use vdm_esp_core::stm_codec::Profile;
use vdm_esp_core::stm_types::{StmCommand, StmSaveState, StmSnapshot};
use vdm_esp_core::version::StmSupport;

use super::{Asset, CalibInfo, Web, WebHost, WebPorts};
use crate::logger::{LogRead, Logger, LoggerShared};
use crate::mqtt_client::{DiscoveryAction, MqttStatus};
use crate::net::{NetInfo, TrialInfo};
use crate::ota::{OtaHost, OtaShared, OtaUpload};
use crate::storage::{LoadDetails, Storage, StorageHost, StorageShared};
use crate::testkit::board::TestPlatform;
use crate::testkit::board::{APP_A, APP_B};
use crate::testkit::http::Response;
use crate::testkit::{
    lock, Device, FakeBoard, FakeClock, FakeConsole, FakeFs, FakeHeap, FakeNvs, FakeRequest,
    FakeWall, SlotImage,
};

/// What the sibling fakes of the C++ suite scripted and recorded (`sib::app()`, `sib::net()`,
/// `sib::mqtt()`, `sib::logger()`), for the [`Host`].
pub(super) struct HostState {
    // app: scripted
    pub(super) submit_result: bool,
    pub(super) snapshot: Box<StmSnapshot>,
    pub(super) profiles: [Profile; VALVE_COUNT as usize],
    pub(super) flash_active: bool,
    pub(super) proto: u8,
    pub(super) support: StmSupport,
    pub(super) calib: CalibInfo,
    pub(super) health: HealthSnapshot<'static>,
    // app: recorded
    pub(super) submitted: Vec<StmCommand>,
    pub(super) profile_reads: u32,
    pub(super) snapshot_reads: u32,
    pub(super) health_reads: u32,
    // logger: recorded
    pub(super) flush_requests: u32,
    // net: scripted
    pub(super) info: NetInfo,
    pub(super) trial: TrialInfo,
    pub(super) trial_confirm_result: bool,
    pub(super) trial_revert_result: bool,
    pub(super) last_sync: u32,
    // net: recorded
    pub(super) trial_confirms: u32,
    pub(super) trial_reverts: u32,
    pub(super) inbound: Vec<u32>,
    // mqtt: scripted
    pub(super) status: MqttStatus,
    pub(super) calib_end: [Option<LocalTime>; VALVE_COUNT as usize],
    // mqtt: recorded
    pub(super) reconnects: u32,
    pub(super) discovery: Vec<DiscoveryAction>,
    // the OTA host of the ESP upload: scripted (C++ `storage::imageUploadActive()`, the link)
    pub(super) image_upload_active: bool,
    pub(super) link: LinkState,
    pub(super) save_requests: u32,
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            submit_result: true,
            snapshot: Box::default(),
            profiles: [Profile::default(); VALVE_COUNT as usize],
            flash_active: false,
            proto: 0,
            support: StmSupport::Unknown,
            calib: CalibInfo::default(),
            health: HealthSnapshot::default(),
            submitted: Vec::new(),
            profile_reads: 0,
            snapshot_reads: 0,
            health_reads: 0,
            flush_requests: 0,
            info: NetInfo::default(),
            trial: TrialInfo::default(),
            trial_confirm_result: false,
            trial_revert_result: false,
            last_sync: 0,
            trial_confirms: 0,
            trial_reverts: 0,
            inbound: Vec::new(),
            status: MqttStatus::default(),
            calib_end: [None; VALVE_COUNT as usize],
            reconnects: 0,
            discovery: Vec::new(),
            image_upload_active: false,
            link: LinkState::Unknown,
            save_requests: 0,
        }
    }
}

/// The fake host of a boot: the web's siblings, and the host of storage and of the ESP upload
/// (their events go to the same log, a real ring as the C++ fake logger kept one).
pub(super) struct Host {
    st: Mutex<HostState>,
    pub(super) log: LoggerShared,
    clock: FakeClock,
    wall: FakeWall,
    console: FakeConsole,
}

impl Host {
    fn new(dev: &Device) -> Self {
        Host {
            st: Mutex::new(HostState::default()),
            log: LoggerShared::new(),
            clock: dev.clock.clone(),
            wall: dev.wall.clone(),
            console: dev.console.clone(),
        }
    }

    /// The scripted and recorded state.
    pub(super) fn state(&self) -> MutexGuard<'_, HostState> {
        lock(&self.st)
    }

    fn logger(&self) -> Logger<'_, FakeClock, FakeWall, FakeConsole> {
        Logger::new(
            &self.log,
            self.clock.clone(),
            self.wall.clone(),
            self.console.clone(),
        )
    }

    /// Every event of the log, oldest first (C++ `sib::logger().events`).
    pub(super) fn events(&self) -> Vec<Event> {
        let mut out = vec![Event::default(); 512];
        let r = self.log.read(&EventFilter::default(), &mut out);
        out.truncate(r.count);
        out
    }

    /// The events of `code` (C++ `withCode`).
    pub(super) fn with_code(&self, code: EventCode) -> Vec<Event> {
        self.events()
            .into_iter()
            .filter(|e| e.code == code)
            .collect()
    }

    /// An event of `code` was logged.
    pub(super) fn has(&self, code: EventCode) -> bool {
        !self.with_code(code).is_empty()
    }

    /// Logs an event as a sibling module would (C++ `logger::logSev`).
    pub(super) fn log_event(&self, e: &Event) -> u32 {
        self.logger().log_event(e)
    }

    /// The version of the health document, kept for the rest of the case.
    pub(super) fn set_health_version(&self, version: &str) {
        let v: &'static str = Box::leak(version.to_string().into_boxed_str());
        self.state().health.version = v.as_bytes();
    }
}

impl WebHost for Host {
    fn submit(&self, cmd: &StmCommand) -> bool {
        let mut s = self.state();
        if !s.submit_result {
            return false;
        }
        s.submitted.push(cmd.clone());
        true
    }
    fn read_stm_snapshot(&self, out: &mut StmSnapshot) {
        let mut s = self.state();
        s.snapshot_reads += 1;
        out.clone_from(&s.snapshot);
    }
    fn read_profile(&self, valve: u8, out: &mut Profile) {
        let mut s = self.state();
        s.profile_reads += 1;
        *out = s
            .profiles
            .get(usize::from(valve))
            .copied()
            .unwrap_or_default();
    }
    fn stm_flash_active(&self) -> bool {
        self.state().flash_active
    }
    fn stm_support(&self) -> StmSupport {
        self.state().support
    }
    fn stm_protocol(&self) -> u8 {
        self.state().proto
    }
    fn calib_info(&self) -> CalibInfo {
        self.state().calib
    }
    fn read_health<'s>(&'s self, out: &mut HealthSnapshot<'s>) {
        let mut s = self.state();
        s.health_reads += 1;
        *out = s.health;
    }
    fn log(&self, e: &Event) {
        self.logger().log_event(e);
    }
    fn read_events(&self, f: &EventFilter, out: &mut [Event]) -> LogRead {
        self.log.read(f, out)
    }
    fn last_event_seq(&self) -> u32 {
        self.log.last_seq()
    }
    fn request_log_flush(&self) {
        self.state().flush_requests += 1;
        self.log.request_flush();
    }
    fn note_inbound_http(&self, remote_ip: u32) {
        self.state().inbound.push(remote_ip);
    }
    fn net_info(&self) -> NetInfo {
        self.state().info.clone()
    }
    fn net_trial(&self) -> TrialInfo {
        self.state().trial
    }
    fn request_trial_confirm(&self) -> bool {
        let mut s = self.state();
        s.trial_confirms += 1;
        s.trial_confirm_result
    }
    fn request_trial_revert(&self) -> bool {
        let mut s = self.state();
        s.trial_reverts += 1;
        s.trial_revert_result
    }
    fn last_sync_epoch(&self) -> u32 {
        self.state().last_sync
    }
    fn mqtt_status(&self) -> MqttStatus {
        self.state().status.clone()
    }
    fn calibration_end(&self, valve: u8) -> Option<LocalTime> {
        self.state()
            .calib_end
            .get(usize::from(valve))
            .copied()
            .flatten()
    }
    fn request_mqtt_reconnect(&self) {
        self.state().reconnects += 1;
    }
    fn request_discovery(&self, a: DiscoveryAction) {
        self.state().discovery.push(a);
    }
}

impl StorageHost for Host {
    fn log(&self, code: EventCode, valve: u8, arg1: i32, arg2: i32, text: &[u8]) {
        self.logger().log(code, valve, arg1, arg2, text);
    }
    fn net_trial_active(&self) -> bool {
        self.state().trial.active
    }
}

impl OtaHost for &Host {
    fn log(&mut self, e: &Event) {
        self.logger().log_event(e);
    }
    fn log_flush(&mut self) {}
    fn net_is_up(&mut self) -> bool {
        false
    }
    fn net_ip(&mut self) -> u32 {
        self.state().info.ip
    }
    fn stm_flash_active(&mut self) -> bool {
        self.state().flash_active
    }
    fn image_upload_active(&mut self) -> bool {
        self.state().image_upload_active
    }
    fn stm_link_state(&mut self) -> LinkState {
        self.state().link
    }
    fn stm_save_state(&mut self) -> StmSaveState {
        StmSaveState::Unavailable
    }
    fn request_stm_save(&mut self) {
        self.state().save_requests += 1;
    }
    fn flush_for_restart(&mut self) {}
    fn set_ota_stm_required(&mut self, _on: bool) {}
}

pub(super) type TestStorage<'a> = Storage<'a, FakeNvs, FakeFs, FakeHeap, &'a Host>;
pub(super) type TestWeb<'a> =
    Web<'a, TestPlatform, FakeNvs, FakeFs, FakeHeap, &'a Host, &'a Host, &'a Host>;

/// The dashboard of the tests (the design: the firmware passes the generated table, tests their
/// own): three files with their gzip bytes and ETags.
pub(super) const ASSETS: &[Asset] = &[
    Asset {
        path: "/app.css",
        content_type: "text/css; charset=utf-8",
        data: b"\x1f\x8b\x08\x00css",
        etag: "\"0000c55a\"",
    },
    Asset {
        path: "/app.js",
        content_type: "application/javascript; charset=utf-8",
        data: b"\x1f\x8b\x08\x00js",
        etag: "\"000000a5\"",
    },
    Asset {
        path: "/index.html",
        content_type: "text/html; charset=utf-8",
        data: b"\x1f\x8b\x08\x00index",
        etag: "\"1a2b3c4d\"",
    },
];

/// One boot of a board: the device, the shared objects of storage and OTA, the host.
pub(super) struct Rig {
    pub(super) board: FakeBoard,
    pub(super) dev: Device,
    pub(super) storage_shared: StorageShared,
    pub(super) ota_shared: OtaShared,
    pub(super) host: Host,
}

impl Rig {
    /// A board fresh from the factory (image A in slot 0, slot 1 empty), booted, LittleFS
    /// formatted.
    pub(super) fn new() -> Self {
        Self::on(FakeBoard::new())
    }

    /// A board with a second image in the update slot (the boot guard's fallback).
    pub(super) fn with_fallback() -> Self {
        Self::on(FakeBoard::with_slots(
            [SlotImage::glue(APP_A), SlotImage::glue(APP_B)],
            0,
        ))
    }

    fn on(board: FakeBoard) -> Self {
        let dev = board.boot();
        dev.fs.set_formatted(true);
        let host = Host::new(&dev);
        Rig {
            board,
            dev,
            storage_shared: StorageShared::new(),
            ota_shared: OtaShared::new(),
            host,
        }
    }

    /// Storage of this boot with LittleFS mounted (the C++ default: `fsReady`).
    pub(super) fn storage(&self) -> TestStorage<'_> {
        let st = self.storage_unmounted();
        let mut formatted = false;
        assert!(st.begin_fs(&mut formatted));
        st
    }

    /// Storage of this boot without a file system.
    pub(super) fn storage_unmounted(&self) -> TestStorage<'_> {
        Storage::new(
            &self.storage_shared,
            self.dev.nvs.clone(),
            self.dev.fs.clone(),
            self.dev.heap.clone(),
            &self.host,
        )
    }

    /// The ESP upload of this boot (the web's, or a second one over the same shared data).
    pub(super) fn ota_upload(&self) -> OtaUpload<'_, TestPlatform, &Host> {
        OtaUpload::new(
            &self.dev.clock,
            &self.dev.ota,
            self.dev.md5.clone(),
            &self.dev.heap,
            &self.ota_shared,
            &self.host,
        )
    }

    /// The web server of this boot over `st` with the test dashboard.
    pub(super) fn web<'r>(&'r self, st: &'r TestStorage<'r>) -> TestWeb<'r> {
        self.web_with(st, ASSETS)
    }

    /// The web server of this boot over `st` with the dashboard `assets`.
    pub(super) fn web_with<'r>(
        &'r self,
        st: &'r TestStorage<'r>,
        assets: &'r [Asset],
    ) -> TestWeb<'r> {
        Web::new(
            WebPorts {
                clock: &self.dev.clock,
                wall: &self.dev.wall,
                fs: &self.dev.fs,
                ota: &self.dev.ota,
                system: &self.dev.system,
                gate: &self.dev.heap,
            },
            st,
            &self.ota_shared,
            self.ota_upload(),
            &self.host,
            assets,
        )
    }

    /// Changes the active config (C++ `sib::storage().active` and `++revision`).
    pub(super) fn config(&self, f: impl FnOnce(&mut Config)) {
        let mut c = Box::<Config>::default();
        self.storage_shared.get_config(&mut c);
        f(&mut c);
        self.storage_shared.set_active_config(&c);
    }

    /// The C++ `start()` of most suites: valve 1 active.
    pub(super) fn started() -> Self {
        let rig = Self::new();
        rig.config(|c| c.valves[0].active = true);
        rig
    }

    /// The scripted and recorded state of the host.
    pub(super) fn state(&self) -> MutexGuard<'_, HostState> {
        self.host.state()
    }

    /// The commands submitted so far.
    pub(super) fn submitted(&self) -> Vec<StmCommand> {
        self.state().submitted.clone()
    }

    /// The active config.
    pub(super) fn active(&self) -> Box<Config> {
        let mut c = Box::<Config>::default();
        self.storage_shared.get_config(&mut c);
        c
    }

    /// The boot load details storage published.
    pub(super) fn load_details(&self) -> LoadDetails {
        self.storage_shared.boot_load_details()
    }
}

// ---------------------------------------------------------------- requests

/// The boundary of the multipart bodies (C++ `fakes::http::Request::boundary`).
pub(super) const BOUNDARY: &str = "----vdmotBoundary7MA4YWxk";

/// One part of a multipart body (C++ `fakes::http::Part`).
pub(super) struct Part {
    pub(super) name: String,
    /// "" = a form field
    pub(super) filename: String,
    pub(super) data: Vec<u8>,
    pub(super) content_type: String,
}

impl Part {
    pub(super) fn field(name: &str, value: &str) -> Self {
        Part {
            name: name.to_string(),
            filename: String::new(),
            data: value.as_bytes().to_vec(),
            content_type: String::new(),
        }
    }
    pub(super) fn file(name: &str, filename: &str, data: &[u8]) -> Self {
        Part {
            name: name.to_string(),
            filename: filename.to_string(),
            data: data.to_vec(),
            content_type: "application/octet-stream".to_string(),
        }
    }
}

/// The body of `parts` as it goes over the wire (C++ `Request::wireBody`).
pub(super) fn wire_body(parts: &[Part]) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        out.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{}\"", p.name).as_bytes(),
        );
        if !p.filename.is_empty() {
            out.extend_from_slice(format!("; filename=\"{}\"", p.filename).as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        if !p.filename.is_empty() {
            out.extend_from_slice(format!("Content-Type: {}\r\n", p.content_type).as_bytes());
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&p.data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    out
}

/// A request as the C++ request driver built it: Host "vdmot.local", client 192.168.1.50, the
/// connection's local address unknown (0).
pub(super) fn request(method: HttpMethod, url: &str) -> FakeRequest {
    let mut r = FakeRequest::new(method, url);
    r.local_ip = 0;
    r
}

/// C++ `fakes::http::get(url)`.
pub(super) fn get(url: &str) -> FakeRequest {
    request(HttpMethod::Get, url)
}

/// C++ `fakes::http::del(url)`.
pub(super) fn del(url: &str) -> FakeRequest {
    request(HttpMethod::Delete, url)
}

/// C++ `fakes::http::post(url, body)` (JSON).
pub(super) fn post(url: &str, body: &str) -> FakeRequest {
    post_typed(url, body.as_bytes(), "application/json")
}

/// C++ `fakes::http::post(url, body, contentType)`; "" sends no Content-Type.
pub(super) fn post_typed(url: &str, body: &[u8], content_type: &str) -> FakeRequest {
    let mut r = request(HttpMethod::Post, url);
    r.body = body.to_vec();
    if content_type.is_empty() {
        r
    } else {
        r.with_header("Content-Type", content_type)
    }
}

/// POST on /api/* as the dashboard sends it (with `X-VdMot: 1`).
pub(super) fn api_post(url: &str, body: &str) -> FakeRequest {
    post(url, body).with_header("X-VdMot", "1")
}

/// DELETE on /api/* as the dashboard sends it.
pub(super) fn api_del(url: &str) -> FakeRequest {
    del(url).with_header("X-VdMot", "1")
}

/// C++ `fakes::http::upload(url, filename, data, fields)`: the fields first, then the file part
/// "file".
pub(super) fn upload(
    url: &str,
    filename: &str,
    data: &[u8],
    fields: &[(&str, &str)],
) -> FakeRequest {
    let mut parts: Vec<Part> = fields.iter().map(|(n, v)| Part::field(n, v)).collect();
    parts.push(Part::file("file", filename, data));
    multipart(url, &parts)
}

/// A multipart POST of `parts`.
pub(super) fn multipart(url: &str, parts: &[Part]) -> FakeRequest {
    let mut r = request(HttpMethod::Post, url);
    r.body = wire_body(parts);
    r.with_header(
        "Content-Type",
        &format!("multipart/form-data; boundary={BOUNDARY}"),
    )
}

/// An upload as the dashboard sends it (with `X-VdMot: 1`).
pub(super) fn api_upload(url: &str, filename: &str, data: &[u8]) -> FakeRequest {
    upload(url, filename, data, &[]).with_header("X-VdMot", "1")
}

/// The request with another Host header (C++ `Request::host`).
pub(super) fn with_host(mut r: FakeRequest, host: &str) -> FakeRequest {
    r.headers.retain(|(n, _)| !n.eq_ignore_ascii_case("Host"));
    r.with_header("Host", host)
}

/// The request with another Content-Type (C++ `Request::contentType`).
pub(super) fn with_type(mut r: FakeRequest, content_type: &str) -> FakeRequest {
    r.headers
        .retain(|(n, _)| !n.eq_ignore_ascii_case("Content-Type"));
    r.with_header("Content-Type", content_type)
}

/// Serves `r` and returns its answer (C++ `fakes::http::perform`); the request checks that it
/// was answered exactly once.
pub(super) fn perform(web: &mut TestWeb<'_>, mut r: FakeRequest) -> Response {
    web.handle(&mut r);
    r.response.clone()
}

/// `{"error":"<code>","detail":"<detail>"}`.
pub(super) fn error_body(code: &str, detail: &str) -> String {
    format!("{{\"error\":\"{code}\",\"detail\":\"{detail}\"}}")
}

/// The body as text.
pub(super) fn text(r: &Response) -> String {
    String::from_utf8_lossy(&r.body).into_owned()
}

/// `n` occurrences of `what` in `s`.
pub(super) fn count_of(s: &str, what: &str) -> usize {
    s.matches(what).count()
}

/// A 1-Wire id with a valid CRC byte (Dallas/Maxim CRC-8 over the first 7 bytes).
pub(super) fn valid_id(family: u8, serial: u8) -> OneWireId {
    let mut b = [0u8; 8];
    b[0] = family;
    for (i, v) in b.iter_mut().enumerate().take(7).skip(1) {
        *v = serial.wrapping_add(i as u8);
    }
    let mut crc = 0u8;
    for &byte in &b[..7] {
        let mut x = byte;
        for _ in 0..8 {
            let mix = (crc ^ x) & 0x01;
            crc >>= 1;
            if mix != 0 {
                crc ^= 0x8C;
            }
            x >>= 1;
        }
    }
    b[7] = crc;
    OneWireId { b }
}

/// A 1-Wire id `family`, `n` .. `n` (C++ `wid`).
pub(super) fn wid(family: u8, n: u8) -> OneWireId {
    let mut b = [0u8; 8];
    b[0] = family;
    b[1] = n;
    b[7] = n;
    OneWireId { b }
}

/// Copies `s` into a text member.
pub(super) fn set_text<const N: usize>(t: &mut vdm_esp_core::common::Text<N>, s: &[u8]) {
    copy_string(t, s);
}

// ---------------------------------------------------------------- STM images

fn put32(s: &mut [u8], at: usize, v: u32) {
    s[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// A 4 KiB STM image that passes the chip-independent checks: SP and reset vector, the
/// handshake strings (optional), the version string and the board marker (`hw` "" = none).
pub(super) fn stm_image(handshake: bool, version: &str, hw: &str) -> Vec<u8> {
    let mut s = vec![0x80u8; 4096];
    put32(&mut s, 0, 0x2002_0000);
    put32(&mut s, 4, 0x0800_0101);
    let mut at = 3000;
    let mut put = |s: &mut Vec<u8>, b: &[u8]| {
        s[at..at + b.len()].copy_from_slice(b);
        at += b.len();
    };
    if handshake {
        put(&mut s, b"\x01DEADBEEF\0\x01BEEFIT\0");
    }
    if !version.is_empty() {
        put(&mut s, format!("\x01{version}\0").as_bytes());
    }
    if !hw.is_empty() {
        put(&mut s, format!("\x01VDM-HW:{hw}\0").as_bytes());
    }
    s
}

/// An image whose vectors fail the header check.
pub(super) fn bad_image() -> Vec<u8> {
    vec![0xFFu8; 1024]
}

/// Puts the images into /stm (before storage mounts and indexes them).
pub(super) fn put_images(rig: &Rig, images: &[(&str, Vec<u8>)]) {
    for (name, data) in images {
        rig.dev.fs.put(&format!("/stm/{name}.bin"), data);
    }
}

/// Scans every image of the index (one per pass).
pub(super) fn scan_all(st: &TestStorage<'_>) {
    for _ in 0..crate::storage::IMAGE_SLOTS {
        st.service();
    }
}

/// An ESP firmware image of `n` bytes (first byte 0xE9).
pub(super) fn esp_image(n: usize) -> Vec<u8> {
    let mut v: Vec<u8> = (0..n).map(|i| (i * 7 % 251) as u8).collect();
    if let Some(b) = v.first_mut() {
        *b = 0xE9;
    }
    v
}

/// The restart requested at `requested_ms` is due `delay_ms` later: the restart path of the ota
/// glue asks for the STM save then, and not a millisecond before (C++
/// `sib::ota().restartRequests[].delayMs`).
pub(super) fn assert_restart_due(rig: &Rig, requested_ms: u64, delay_ms: u64) {
    use crate::boot_guard::BootGuard;
    use crate::ota::{OtaPorts, OtaService};
    use crate::port::Clock;
    use crate::testkit::run;
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
    let before = rig.state().save_requests;
    d.clock.set_ms(requested_ms + delay_ms - 1);
    svc.service_restart(d.clock.now_ms(), false, false);
    assert_eq!(
        rig.state().save_requests,
        before,
        "due before {delay_ms} ms"
    );
    d.clock.set_ms(requested_ms + delay_ms);
    svc.service_restart(d.clock.now_ms(), false, false);
    assert_eq!(
        rig.state().save_requests,
        before + 1,
        "not due after {delay_ms} ms"
    );
}

/// Serves `r` and returns the request with its answer, for the checks of what it read.
pub(super) fn served(web: &mut TestWeb<'_>, mut r: FakeRequest) -> FakeRequest {
    web.handle(&mut r);
    r
}

/// The MD5 of `data` in lowercase hex.
pub(super) fn md5_hex(data: &[u8]) -> String {
    use crate::port::Md5;
    let mut m = crate::testkit::FakeMd5::default();
    m.reset();
    m.update(data);
    m.digest().iter().map(|b| format!("{b:02x}")).collect()
}
