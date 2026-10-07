//! The HTTP API and the dashboard (C++ `web_server.cpp`, `web_server.h`; docs/revamped/API.md,
//! DESIGN.md section 12, docs/rust/GLUE-DESIGN-ESP.md section 4): every request of the
//! esp_http_server task goes through [`Web::handle`], one at a time, over the [`HttpRequest`]
//! port. Handlers never block on the STM and never touch the UART: STM actions are commands
//! through [`WebHost::submit`], NVS writes go through [`Storage::apply_config`].
//!
//! The pipeline of a request (design 4.2):
//! 1. [`WebHost::note_inbound_http`] (network evidence), for every request.
//! 2. The working set (snapshot and config copies, the JSON document, the multipart parser):
//!    allocated by the first request and kept; without memory the request is answered
//!    `503 busy "out of memory"`, the next one allocates what is missing. The config copy is
//!    reloaded when the revision moved.
//! 3. Refusals before any body byte is read: outside `/api/` the legacy table (410 with the
//!    replacement, 405, the guard of the aliases, 404/405 for any other path with a body);
//!    inside `/api/` the route, the request guard (Host, Origin, X-VdMot, Content-Type; event
//!    213 once per verdict per minute), then the limits of an upload (multipart, Content-Length
//!    1..limit, no upload or flash running) or of a body (no multipart, at most
//!    [`MAX_BODY_SIZE`]). Static assets and 410 answers skip the guard.
//! 4. A body is read whole into a heap buffer of its Content-Length; an upload streams through
//!    the response buffer into storage or OTA ([`uploads`]). A client that goes away, or stalls
//!    for [`BODY_TIMEOUTS`] receive timeouts in a row, gets no answer.
//! 5. The handler answers exactly once.
//!
//! Memory: the response buffer ([`RESPONSE_SIZE`]) is allocated at its first use and kept; the
//! lists a handler builds (valve and sensor views, events, files) are a transient heap block of
//! the request (the C++ shared scratch, which safe Rust cannot retype); a JSON body takes a heap
//! block of its length for its request.
//!
//! What the server needs from the other glue modules comes through [`WebHost`] (the C++ calls
//! into `app`, `logger`, `net` and `mqtt`); storage and the ESP upload are used directly
//! ([`Storage`], [`OtaUpload`], [`OtaShared`]).

#[cfg(feature = "dashboard")]
pub mod assets;
mod uploads;
mod views;

use core::mem::size_of;
use core::ops::RangeInclusive;

use vdm_esp_core::common::{
    build_hostname, c_str, crc_valid, fmt_trunc, format_ipv4, is_zero, round_target_percent,
    LocalTime, OneWireId, Text, TextBuf, ALL_VALVES, NO_VALVE, STATION_NAME_MAX, TEMP_SLOT_COUNT,
    VALVE_COUNT,
};
use vdm_esp_core::config::{
    apply_config_json, config_restart_reasons, net_trial_required, write_config_json, ApplyInfo,
    Config, MqttMode, PatchResult,
};
use vdm_esp_core::event_log::{
    event_default_severity, make_event, Event, EventCode, EventFilter, RebootReason,
};
use vdm_esp_core::json_api::{
    match_api_route, write_error_json, ApiRoute, HealthSnapshot, HttpMethod, RouteMatch,
};
use vdm_esp_core::json_writer::JsonWriter;
use vdm_esp_core::legacy_http::{match_legacy_route, LegacyMatch, LegacyRoute};
use vdm_esp_core::stm_codec::{
    breakaway_valid, learn_movements_valid, motor_chars_valid, Breakaway, MotorChars, MoveDir,
    Profile,
};
use vdm_esp_core::stm_flasher::{check_board, flash_error_name, BoardCheck, FlashError};
use vdm_esp_core::stm_types::{StmCommand, StmCommandType, StmSnapshot};
use vdm_esp_core::valve_model::TargetSource;
use vdm_esp_core::version::{format_version, StmSupport};
use vdm_esp_core::web_guard::{
    check_request, guard_detail, guard_error_code, guard_http_status, GuardRequest, GuardScope,
    GuardVerdict, HostPolicy, RepeatLimiter,
};

use crate::heap::{try_block, try_bytes, Block};
use crate::http_parse::Multipart;
use crate::http_parse::{is_multipart, media_type, query_param, split_target, url_decode};
use crate::json_body::{JsonDocument, JsonObjectConst, JsonVariantConst};
use crate::logger::LogRead;
use crate::mqtt_client::{DiscoveryAction, MqttStatus};
use crate::net::{NetInfo, TrialInfo};
use crate::ota::{OtaHost, OtaShared, OtaUpload};
use crate::port::Nvs;
use crate::port::{BodyRead, Clock, Fs, HeapGate, HttpRequest, HttpServer, Ota, Platform};
use crate::storage::{
    normalize_image_name, FileResult, ImageEntry, ImageResult, Storage, StorageHost, IMAGE_NAME_MAX,
};

// ---------------------------------------------------------------- binding numbers

/// The response buffer: one, allocated at its first use and kept (C++ two slots of this size).
/// A document is at most one byte shorter (the C++ terminator).
pub const RESPONSE_SIZE: usize = 12 * 1024;
/// A JSON body is received whole into a heap buffer of its Content-Length; a larger one is
/// refused (413) before any of it is read.
pub const MAX_BODY_SIZE: usize = 8192;
/// Connections the server holds at a time (`max_open_sockets` of the adapter; C++ the listen
/// backlog of AsyncTCP).
pub const MAX_CONNECTIONS: usize = 4;
/// STM image file size limit.
pub const MAX_STM_IMAGE_SIZE: usize = 512 * 1024;
/// Allowance for the multipart framing around the file in an upload's Content-Length.
pub const MULTIPART_SLACK: usize = 8 * 1024;
/// A body read gives up after this many receive timeouts (5 s each) in a row: no answer.
pub const BODY_TIMEOUTS: u8 = 3;

const JSON: &str = "application/json";
/// A sensor reading older than this is not valid.
const SENSOR_STALE_MS: u32 = 60_000;
/// `?limit=` of GET /api/events.
const MAX_EVENTS_PER_RESPONSE: u32 = 50;
/// GET /api/files lists at most this many files.
const MAX_FILES_PER_RESPONSE: usize = 32;
/// GET /api/health: the document is at most one byte shorter.
const HEALTH_BUF_SIZE: usize = 1024;
/// The detail of a guard refusal (C++ `GuardDetail`).
const GUARD_DETAIL_SIZE: usize = 160;
/// An error answer (C++ `sendError`'s buffer); a longer one becomes `{"error":"internal"}`.
const ERROR_BUF_SIZE: usize = 200;
/// The decoded path: esp_http_server takes URIs of at most 512 bytes (CONFIG_HTTPD_MAX_URI_LEN).
const PATH_MAX: usize = 512;
/// Host, Origin and Content-Type values: a longer value is read as its first 256 bytes (no
/// allowed host is that long, design 4.1 bounds the header block to 1024 bytes).
const HEADER_MAX: usize = 256;
/// A query value: longer ones are cut, which keeps them invalid (numbers have at most 10
/// digits, a severity name 8 characters, `dryRun` is "1").
const QUERY_MAX: usize = 32;
/// The `path` of DELETE /api/files: longer than any error answer can echo.
const QUERY_PATH_MAX: usize = 256;
/// The path of the member a config patch or save fails at (C++ `char path[72]`).
const CONFIG_PATH_SIZE: usize = 72;
/// The deferred restarts of the reboot, the factory reset and the switch back.
const RESTART_DELAY_MS: u32 = 1000;
/// The epoch of a development build (`VDM_BUILD_EPOCH`, C++ the build flag), 0 = none.
const BUILD_EPOCH: u32 = match option_env!("VDM_BUILD_EPOCH") {
    Some(s) => parse_epoch(s.as_bytes()),
    None => 0,
};

/// The decimal `VDM_BUILD_EPOCH`; 0 for anything else.
const fn parse_epoch(s: &[u8]) -> u32 {
    let mut v: u32 = 0;
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if !c.is_ascii_digit() {
            return 0;
        }
        v = v.wrapping_mul(10).wrapping_add((c - b'0') as u32);
        i += 1;
    }
    v
}

// ---------------------------------------------------------------- what the server uses

/// One file of the dashboard (C++ `web_assets::Asset`, generated by `gen_web_assets.py`; the
/// firmware passes [`assets::DASHBOARD`], the tests their own tables).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Asset {
    /// URL path, e.g. "/index.html".
    pub path: &'static str,
    /// Content-Type of the file.
    pub content_type: &'static str,
    /// The gzip bytes (served with `Content-Encoding: gzip`).
    pub data: &'static [u8],
    /// The quoted CRC32 of the gzip bytes, e.g. `"1a2b3c4d"` with the quotes.
    pub etag: &'static str,
}

/// The calibration schedule for /api/status (C++ `app::CalibInfo`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CalibInfo {
    /// Epoch of the last scheduled calibration, 0 = none.
    pub last_scheduled_epoch: i64,
    /// The next slot (yyyymmdd), 0 = none.
    pub next_slot: u32,
    /// UTC epoch of the next scheduled calibration, 0 = none.
    pub next_epoch: i64,
}

/// What the web server asks of the other glue modules: one method per C++ call into `app`,
/// `logger`, `net` and `mqtt`. The firmware wiring implements it over their shared objects
/// (the app's, LoggerShared with the Logger, NetShared, MqttShared), the tests fake it.
pub trait WebHost {
    /// `app::submit(c)`: queues a command for the STM task (never blocks); false when the queue
    /// is full.
    fn submit(&self, cmd: &StmCommand) -> bool;
    /// `app::readStmSnapshot(out)`: copies the latest STM snapshot.
    fn read_stm_snapshot(&self, out: &mut StmSnapshot);
    /// `app::readProfile(valve, out)`: the stored profile of `valve` (count 0: none).
    fn read_profile(&self, valve: u8, out: &mut Profile);
    /// `app::stmFlashActive()`: an STM flash runs.
    fn stm_flash_active(&self) -> bool;
    /// `app::stmSupport()`: the STM firmware is supported, too old or unknown.
    fn stm_support(&self) -> StmSupport;
    /// `app::stmProtocol()`: the protocol of the STM link (0 unknown, 1..3).
    fn stm_protocol(&self) -> u8;
    /// `app::calibInfo()`.
    fn calib_info(&self) -> CalibInfo;
    /// `app::readHealth(out)`: the document of GET /api/health.
    fn read_health<'s>(&'s self, out: &mut HealthSnapshot<'s>);
    /// `logger::log(e)`: records the event (the logger fills seq, uptime and epoch).
    fn log(&self, e: &Event);
    /// `logger::read(f, out, ...)`: the events of `f` into `out` with the ring's bounds.
    fn read_events(&self, f: &EventFilter, out: &mut [Event]) -> LogRead;
    /// `logger::lastSeq()`: seq of the newest event, 0 when the ring is empty.
    fn last_event_seq(&self) -> u32;
    /// `logger::requestFlush()`: the file sink writes the whole backlog in its next pass.
    fn request_log_flush(&self);
    /// `net::noteInboundHttp(ip)`: a request from `remote_ip` reached the server.
    fn note_inbound_http(&self, remote_ip: u32);
    /// `net::info()`.
    fn net_info(&self) -> NetInfo;
    /// `net::trialInfo()`.
    fn net_trial(&self) -> TrialInfo;
    /// `net::requestTrialConfirm()`: true when a network trial runs.
    fn request_trial_confirm(&self) -> bool;
    /// `net::requestTrialRevert()`: true when a network trial runs.
    fn request_trial_revert(&self) -> bool;
    /// `net::lastSyncEpoch()`: epoch of the last SNTP sync, 0 before the first.
    fn last_sync_epoch(&self) -> u32;
    /// `mqtt::status()`.
    fn mqtt_status(&self) -> MqttStatus;
    /// `mqtt::calibrationEnd(valve, out)`: end of the last calibration of `valve` seen since
    /// boot.
    fn calibration_end(&self, valve: u8) -> Option<LocalTime>;
    /// `mqtt::requestReconnect()`.
    fn request_mqtt_reconnect(&self);
    /// `mqtt::requestDiscovery(a)`.
    fn request_discovery(&self, a: DiscoveryAction);
}

impl<T: WebHost + ?Sized> WebHost for &T {
    fn submit(&self, cmd: &StmCommand) -> bool {
        (**self).submit(cmd)
    }
    fn read_stm_snapshot(&self, out: &mut StmSnapshot) {
        (**self).read_stm_snapshot(out)
    }
    fn read_profile(&self, valve: u8, out: &mut Profile) {
        (**self).read_profile(valve, out)
    }
    fn stm_flash_active(&self) -> bool {
        (**self).stm_flash_active()
    }
    fn stm_support(&self) -> StmSupport {
        (**self).stm_support()
    }
    fn stm_protocol(&self) -> u8 {
        (**self).stm_protocol()
    }
    fn calib_info(&self) -> CalibInfo {
        (**self).calib_info()
    }
    fn read_health<'s>(&'s self, out: &mut HealthSnapshot<'s>) {
        (**self).read_health(out)
    }
    fn log(&self, e: &Event) {
        (**self).log(e)
    }
    fn read_events(&self, f: &EventFilter, out: &mut [Event]) -> LogRead {
        (**self).read_events(f, out)
    }
    fn last_event_seq(&self) -> u32 {
        (**self).last_event_seq()
    }
    fn request_log_flush(&self) {
        (**self).request_log_flush()
    }
    fn note_inbound_http(&self, remote_ip: u32) {
        (**self).note_inbound_http(remote_ip)
    }
    fn net_info(&self) -> NetInfo {
        (**self).net_info()
    }
    fn net_trial(&self) -> TrialInfo {
        (**self).net_trial()
    }
    fn request_trial_confirm(&self) -> bool {
        (**self).request_trial_confirm()
    }
    fn request_trial_revert(&self) -> bool {
        (**self).request_trial_revert()
    }
    fn last_sync_epoch(&self) -> u32 {
        (**self).last_sync_epoch()
    }
    fn mqtt_status(&self) -> MqttStatus {
        (**self).mqtt_status()
    }
    fn calibration_end(&self, valve: u8) -> Option<LocalTime> {
        (**self).calibration_end(valve)
    }
    fn request_mqtt_reconnect(&self) {
        (**self).request_mqtt_reconnect()
    }
    fn request_discovery(&self, a: DiscoveryAction) {
        (**self).request_discovery(a)
    }
}

/// The ports of the server: time, the files of the log and the import report, the OTA slots,
/// the chip, and the heap gate of the working set, the response buffer, bodies and lists.
pub struct WebPorts<'a, P: Platform> {
    pub clock: &'a P::Clock,
    pub wall: &'a P::WallClock,
    pub fs: &'a P::Fs,
    pub ota: &'a P::Ota,
    pub system: &'a P::System,
    pub gate: &'a P::HeapGate,
}

/// C++ `web::begin()` and `web::started()`: the server is started once, by the app task when
/// the network has an address; a start that fails is tried again by the next `begin`.
#[derive(Debug, Default)]
pub struct WebStart {
    started: bool,
}

impl WebStart {
    /// Starts the server unless it runs (idempotent).
    pub fn begin(&mut self, server: &mut impl HttpServer) {
        if !self.started {
            self.started = server.start();
        }
    }

    /// The server runs.
    pub fn started(&self) -> bool {
        self.started
    }
}

// ---------------------------------------------------------------- working set

/// The parts of the working set allocated so far (all of them are tried by every request until
/// the set is complete).
#[derive(Default)]
struct Parts {
    snap: Option<Block<StmSnapshot>>,
    cfg: Option<Block<Config>>,
    doc: Option<Block<JsonDocument>>,
    multipart: Option<Block<Multipart>>,
}

/// The working set of the handlers (C++ the parts of `allocWork` and the response slots): kept
/// from the first request on. Taken out of the server for the request that uses it.
struct WorkSet {
    /// The copy of the STM snapshot a handler reads.
    snap: Block<StmSnapshot>,
    /// The copy of the active config, reloaded when its revision moved (the target of a config
    /// patch, which reloads it when the patch is not saved).
    cfg: Block<Config>,
    /// The JSON document of the small bodies.
    doc: Block<JsonDocument>,
    /// The multipart parser of the uploads.
    multipart: Block<Multipart>,
    /// `build_hostname(cfg.station)`: the name the guard accepts.
    hostname: Text<STATION_NAME_MAX>,
    /// Revision of `cfg`; `None` before the first load.
    cfg_revision: Option<u32>,
    /// The response buffer, allocated at its first use.
    response: Option<Vec<u8>>,
}

/// The response buffer, allocated by its first use ([`RESPONSE_SIZE`] bytes) and kept; `None`
/// without memory.
fn response_buf<'r>(
    response: &'r mut Option<Vec<u8>>,
    gate: &impl HeapGate,
) -> Option<&'r mut [u8]> {
    if response.is_none() {
        *response = try_bytes(gate, RESPONSE_SIZE);
    }
    response.as_deref_mut()
}

/// A transient list of a handler (C++ the scratch buffer): room for `cap` elements, `None`
/// when the gate refuses the bytes or the heap has no such block.
fn scratch<T>(gate: &impl HeapGate, cap: usize) -> Option<Vec<T>> {
    if !gate.grant(cap.saturating_mul(size_of::<T>())) {
        return None;
    }
    let mut v = Vec::new();
    v.try_reserve_exact(cap).ok()?;
    Some(v)
}

// ---------------------------------------------------------------- the request

/// What the pipeline knows of a request before its body.
struct Head<'b> {
    method: HttpMethod,
    /// The decoded path (C++ `req->url()`).
    path: &'b [u8],
    /// Content-Length.
    len: usize,
    /// The Content-Type value.
    ctype: Option<&'b [u8]>,
}

impl Head<'_> {
    /// C++ `contentType().startsWith("multipart/")`.
    fn multipart(&self) -> bool {
        self.ctype.is_some_and(is_multipart)
    }
}

/// The value of header `name` in `buf`; `None` when the header is absent. A value longer than
/// `buf` is read as its first `buf.len()` bytes.
fn header<'b>(req: &dyn HttpRequest, name: &str, buf: &'b mut [u8]) -> Option<&'b [u8]> {
    let n = req.header(name, buf)?;
    let buf: &'b [u8] = buf;
    buf.get(..n.min(buf.len()))
}

/// The decoded value of the query parameter `name` (the first one) in `buf`; `None` when there
/// is none. A value longer than `buf` is read as its first `buf.len()` bytes.
fn query<'b>(req: &dyn HttpRequest, name: &[u8], buf: &'b mut [u8]) -> Option<&'b [u8]> {
    let n = query_param(split_target(req.target()).1, name, buf)?;
    let buf: &'b [u8] = buf;
    buf.get(..n.min(buf.len()))
}

/// C++ `queryUint`: a strict unsigned query parameter up to `max`; a missing one keeps `out`.
fn query_uint(req: &dyn HttpRequest, name: &[u8], max: u32, out: &mut u32) -> bool {
    let mut buf = [0u8; QUERY_MAX];
    let Some(v) = query(req, name, &mut buf) else {
        return true;
    };
    match vdm_esp_core::common::parse_uint(v, max) {
        Some(x) => {
            *out = x;
            true
        }
        None => false,
    }
}

// ---------------------------------------------------------------- answers

/// An answer without extra headers (C++ `req->send(code, type, text)`). An empty content type
/// sends none (204, 304).
fn send(req: &mut dyn HttpRequest, code: u16, content_type: &str, body: &[u8]) {
    req.respond(code, content_type, &[], body);
}

/// `{"error":"<error>","detail":"<detail>"}` (C++ `sendError`); `{"error":"internal"}` when
/// it does not fit its 200-byte buffer.
fn send_error(req: &mut dyn HttpRequest, code: u16, error: &str, detail: &[u8]) {
    let mut buf = [0u8; ERROR_BUF_SIZE];
    let mut jw = JsonWriter::new(&mut buf);
    let body: &[u8] = if write_error_json(&mut jw, error.as_bytes(), Some(detail)) {
        jw.as_bytes()
    } else {
        b"{\"error\":\"internal\"}"
    };
    send(req, code, JSON, body);
}

/// `202 {"result":"queued"}` (C++ `sendAccepted`).
fn accepted(req: &mut dyn HttpRequest) {
    send(req, 202, JSON, b"{\"result\":\"queued\"}");
}

/// A JSON document, not cached (C++ `sendSlot`), optionally as a download.
fn send_document(req: &mut dyn HttpRequest, code: u16, body: &[u8], attachment: Option<&str>) {
    match attachment {
        Some(a) => req.respond(
            code,
            JSON,
            &[("Cache-Control", "no-store"), ("Content-Disposition", a)],
            body,
        ),
        None => req.respond(code, JSON, &[("Cache-Control", "no-store")], body),
    };
}

/// `503 busy "out of memory"`.
fn out_of_memory(req: &mut dyn HttpRequest) {
    send_error(req, 503, "busy", b"out of memory");
}

/// The limits of a JSON body: no multipart, at most [`MAX_BODY_SIZE`] bytes; true when the
/// request was answered.
fn body_refused(req: &mut dyn HttpRequest, h: &Head<'_>) -> bool {
    if h.multipart() && h.len > 0 {
        send_error(
            req,
            415,
            "unsupported_media_type",
            b"application/json expected",
        );
    } else if h.len > MAX_BODY_SIZE {
        send_error(req, 413, "too_large", b"body");
    } else {
        return false;
    }
    true
}

// ---------------------------------------------------------------- JSON input

/// C++ `parseBody`: the body as a JSON object, `None` when an error was answered (no body,
/// ArduinoJson's error, not an object).
fn parse<'d>(
    req: &mut dyn HttpRequest,
    doc: &'d mut JsonDocument,
    body: &'d mut [u8],
) -> Option<JsonObjectConst<'d>> {
    if body.is_empty() {
        send_error(req, 400, "bad_request", b"JSON body required");
        return None;
    }
    match doc.deserialize(body) {
        Err(e) => {
            send_error(req, 400, "bad_request", e.c_str().as_bytes());
            None
        }
        Ok(root) => {
            let o = root.as_object();
            if o.is_none() {
                send_error(req, 400, "bad_request", b"object expected");
            }
            o
        }
    }
}

/// C++ `intField`: an integer member (not a bool, a double or a string) in `range`; a missing
/// member gives `missing` (the C++ `out` it keeps, or `None` for a required one).
fn int_field(
    o: &JsonObjectConst<'_>,
    key: &[u8],
    range: RangeInclusive<i64>,
    missing: Option<i64>,
) -> Option<i64> {
    let v = o.get(key);
    if v.is_null() {
        return missing;
    }
    if !v.is_i64() {
        return None;
    }
    Some(v.as_i64()).filter(|x| range.contains(x))
}

/// C++ `boolField`: a bool member; a missing member gives `missing`.
fn bool_field(o: &JsonObjectConst<'_>, key: &[u8], missing: Option<bool>) -> Option<bool> {
    let v = o.get(key);
    if v.is_null() {
        return missing;
    }
    v.is_bool().then(|| v.as_bool())
}

/// C++ `onlyKeys`: no member besides `keys`.
fn only_keys(o: &JsonObjectConst<'_>, keys: &[&[u8]]) -> bool {
    o.iter().all(|(k, _)| keys.contains(&k))
}

/// C++ `targetField`: a JSON number (integer or fraction, never a bool or a string) rounded by
/// core `round_target_percent`.
fn target_field(v: JsonVariantConst<'_>) -> Option<u8> {
    if !v.is_f64() {
        return None;
    }
    round_target_percent(v.as_f64())
}

/// An STM command of `kind` for `valve`.
fn command(kind: StmCommandType, valve: u8) -> StmCommand {
    StmCommand {
        kind,
        valve,
        ..StmCommand::default()
    }
}

// ---------------------------------------------------------------- members of STM commands

/// An error answer: status, error code, detail (a constant: the checks return references).
#[derive(Debug)]
struct Refusal(u16, &'static str, &'static [u8]);

impl Refusal {
    fn send(&self, req: &mut dyn HttpRequest) {
        send_error(req, self.0, self.1, self.2);
    }
}

/// The members of a service move into `cmd`: `dir` open|close, `counts` 1..10000, `maxmA`
/// 5..60, nothing else; false when one is missing, unknown or invalid.
fn move_fields(o: &JsonObjectConst<'_>, cmd: &mut StmCommand) -> bool {
    let dir = match o.get(b"dir").as_str() {
        Some(b"open") => MoveDir::Open,
        Some(b"close") => MoveDir::Close,
        _ => return false,
    };
    let fields = (
        only_keys(o, &[b"dir", b"counts", b"maxmA"]),
        int_field(o, b"counts", 1..=10_000, None),
        int_field(o, b"maxmA", 5..=60, None),
    );
    let (true, Some(counts), Some(max_ma)) = fields else {
        return false;
    };
    cmd.dir = dir;
    cmd.counts = counts as u16;
    cmd.max_ma = max_ma as u8;
    true
}

/// The slots of POST /api/valves/{n}/sensors: `slot1` and `slot2`, 0..34, nothing else, the
/// same slot not twice (0 = none).
fn sensor_slots(o: &JsonObjectConst<'_>) -> Option<[i64; 2]> {
    let max = i64::from(TEMP_SLOT_COUNT);
    let fields = (
        only_keys(o, &[b"slot1", b"slot2"]),
        int_field(o, b"slot1", 0..=max, None),
        int_field(o, b"slot2", 0..=max, None),
    );
    let (true, Some(slot1), Some(slot2)) = fields else {
        return None;
    };
    if slot1 != 0 && slot1 == slot2 {
        return None;
    }
    Some([slot1, slot2])
}

/// The sensor ids of the configured `slots` into `ids` (slot 0: none); the detail of the first
/// slot without a valid id.
fn slot_ids(cfg: &Config, slots: [i64; 2], ids: &mut [OneWireId; 2]) -> Result<(), &'static [u8]> {
    let errors: [&'static [u8]; 2] = [
        b"slot1 has no valid sensor id",
        b"slot2 has no valid sensor id",
    ];
    for ((slot, error), id_out) in slots.into_iter().zip(errors).zip(ids.iter_mut()) {
        if slot == 0 {
            continue;
        }
        let id = cfg.temps.get(slot as usize - 1).map(|t| t.id);
        match id {
            Some(id) if !is_zero(&id) && crc_valid(&id) => *id_out = id,
            _ => return Err(error),
        }
    }
    Ok(())
}

/// The members of POST /api/stm/motor merged with the values read from the STM (`snap`) into
/// `cmd`: `motor`, then `learnMovements`, then `breakaway` (which needs protocol 2), at least
/// one of them.
fn motor_fields(
    o: &JsonObjectConst<'_>,
    snap: &StmSnapshot,
    protocol: u8,
    cmd: &mut StmCommand,
) -> Result<(), &'static Refusal> {
    if !only_keys(o, &[b"motor", b"learnMovements", b"breakaway"]) {
        return Err(&Refusal(
            400,
            "unknown_key",
            b"motor/learnMovements/breakaway",
        ));
    }
    let mv = o.get(b"motor");
    if !mv.is_null() {
        cmd.motor = motor_member(mv, snap)?;
        cmd.has_motor = true;
    }
    learn_member(o, cmd)?;
    let bv = o.get(b"breakaway");
    if !bv.is_null() {
        cmd.breakaway = breakaway_member(bv, snap, protocol)?;
        cmd.has_breakaway = true;
    }
    if !cmd.has_motor && !cmd.has_learn_movements && !cmd.has_breakaway {
        return Err(&Refusal(400, "bad_request", b"nothing to set"));
    }
    Ok(())
}

/// `motor`: the motor characteristics, the members not given from the STM (all five while
/// they were not read).
fn motor_member(
    mv: JsonVariantConst<'_>,
    snap: &StmSnapshot,
) -> Result<MotorChars, &'static Refusal> {
    let keys: &[&[u8]] = &[
        b"lowC",
        b"highC",
        b"startOnPower",
        b"noOfMinCount",
        b"maxCalReps",
    ];
    let Some(m) = mv.as_object().filter(|m| only_keys(m, keys)) else {
        return Err(&Refusal(400, "invalid", b"motor"));
    };
    if m.size() != 5 && !snap.have_motor {
        return Err(&Refusal(
            409,
            "unknown",
            b"motor parameters not read yet: send all five",
        ));
    }
    let mut mc = snap.motor;
    if !motor_values(&m, &mut mc) || !motor_chars_valid(&mc) {
        return Err(&Refusal(400, "out_of_range", b"motor"));
    }
    Ok(mc)
}

/// The members of `motor` in their ranges into `mc` (which holds the values of the missing
/// ones); false when one is out of range.
fn motor_values(m: &JsonObjectConst<'_>, mc: &mut MotorChars) -> bool {
    let fields = (
        int_field(m, b"lowC", 10..=40, Some(i64::from(mc.low_factor))),
        int_field(m, b"highC", 10..=40, Some(i64::from(mc.high_factor))),
        int_field(
            m,
            b"startOnPower",
            0..=100,
            Some(i64::from(mc.start_on_power)),
        ),
        int_field(
            m,
            b"noOfMinCount",
            0..=60_000,
            Some(i64::from(mc.min_counts)),
        ),
        int_field(
            m,
            b"maxCalReps",
            0..=2,
            Some(i64::from(mc.max_calib_retries)),
        ),
    );
    let (Some(low), Some(high), Some(sop), Some(min_counts), Some(reps)) = fields else {
        return false;
    };
    mc.low_factor = low as u8;
    mc.high_factor = high as u8;
    mc.start_on_power = sop as u8;
    mc.min_counts = min_counts as u16;
    mc.max_calib_retries = reps as u8;
    mc.field_count = 5;
    true
}

/// `learnMovements`: 0 or 50..65534.
fn learn_member(o: &JsonObjectConst<'_>, cmd: &mut StmCommand) -> Result<(), &'static Refusal> {
    let present = !o.get(b"learnMovements").is_null();
    let learn = int_field(o, b"learnMovements", 0..=65_534, Some(0))
        .filter(|&n| !present || learn_movements_valid(n as u32));
    let Some(learn) = learn else {
        return Err(&Refusal(
            400,
            "out_of_range",
            b"learnMovements 0 or 50..65534",
        ));
    };
    if present {
        cmd.has_learn_movements = true;
        cmd.learn_movements = learn as u16;
    }
    Ok(())
}

/// `breakaway` (protocol 2): the members not given from the STM (all three while they were not
/// read).
fn breakaway_member(
    bv: JsonVariantConst<'_>,
    snap: &StmSnapshot,
    protocol: u8,
) -> Result<Breakaway, &'static Refusal> {
    let keys: &[&[u8]] = &[b"enable", b"stepPct", b"maxmA"];
    let Some(b) = bv.as_object().filter(|b| only_keys(b, keys)) else {
        return Err(&Refusal(400, "invalid", b"breakaway"));
    };
    if protocol < 2 {
        return Err(&Refusal(409, "unsupported", b"STM protocol v2 required"));
    }
    if b.size() != 3 && !snap.have_breakaway {
        return Err(&Refusal(
            409,
            "unknown",
            b"breakaway not read yet: send all three",
        ));
    }
    let mut ba = snap.breakaway;
    if !breakaway_values(&b, &mut ba) || !breakaway_valid(&ba) {
        return Err(&Refusal(400, "out_of_range", b"breakaway"));
    }
    Ok(ba)
}

/// The members of `breakaway` in their ranges into `ba` (which holds the values of the missing
/// ones); false when one is out of range.
fn breakaway_values(b: &JsonObjectConst<'_>, ba: &mut Breakaway) -> bool {
    let fields = (
        bool_field(b, b"enable", Some(ba.enable)),
        int_field(b, b"stepPct", 0..=100, Some(i64::from(ba.step_pct))),
        int_field(b, b"maxmA", 20..=60, Some(i64::from(ba.max_ma))),
    );
    let (Some(enable), Some(step), Some(max_ma)) = fields else {
        return false;
    };
    ba.enable = enable;
    ba.step_pct = step as u8;
    ba.max_ma = max_ma as u8;
    true
}

/// The members of POST /api/stm/flash into `cmd`: `image` (normalized), `mode`, `force`,
/// `board`, nothing else; false when one is missing, unknown or invalid.
fn flash_fields(o: &JsonObjectConst<'_>, cmd: &mut StmCommand) -> bool {
    let fields = (
        only_keys(o, &[b"image", b"mode", b"force", b"board"]),
        o.get(b"image").as_str(),
        flash_board(o.get(b"board")),
        flash_mode(o),
    );
    let (true, Some(image), Some(board), Some((blank, force))) = fields else {
        return false;
    };
    let mut name = [0u8; IMAGE_NAME_MAX + 1];
    let Some(n) = normalize_image_name(image, &mut name) else {
        return false;
    };
    let _ = cmd
        .image
        .extend_from_slice(name.get(..n).unwrap_or_default());
    let _ = cmd.board.extend_from_slice(board);
    cmd.blank = blank;
    cmd.force = force;
    true
}

/// `board` of a flash: C1|C2, "" when absent.
fn flash_board<'d>(v: JsonVariantConst<'d>) -> Option<&'d [u8]> {
    if v.is_null() {
        return Some(b"");
    }
    v.as_str().filter(|b| matches!(*b, b"" | b"C1" | b"C2"))
}

/// `mode` (normal|blank, default normal) and `force` (default false) of a flash: (blank,
/// force).
fn flash_mode(o: &JsonObjectConst<'_>) -> Option<(bool, bool)> {
    let v = o.get(b"mode");
    let mode: &[u8] = if v.is_null() {
        b"normal"
    } else {
        v.as_str().unwrap_or_default()
    };
    let blank = mode == b"blank";
    let force = bool_field(o, b"force", Some(false)).filter(|_| blank || mode == b"normal")?;
    Some((blank, force))
}

/// The `action` of POST /api/mqtt/discovery.
fn discovery_action(v: JsonVariantConst<'_>) -> Option<DiscoveryAction> {
    match v.as_str()? {
        b"publish" => Some(DiscoveryAction::Publish),
        b"delete" => Some(DiscoveryAction::Delete),
        b"republish" => Some(DiscoveryAction::DeleteAndPublish),
        _ => None,
    }
}

/// The answer of a dry run of POST /api/config: what a save would do.
fn dry_run_answer(req: &mut dyn HttpRequest, info: &ApplyInfo) {
    let mut buf = [0u8; 64];
    let n = fmt_trunc(
        &mut buf,
        format_args!(
            "{{\"restartRequired\":{},\"netTrial\":{}}}",
            info.restart_required, info.net_trial
        ),
    );
    send(req, 200, JSON, buf.get(..n).unwrap_or_default());
}

/// HTTP status of a storage image result (C++ `imageHttpCode`).
fn image_http_code(r: ImageResult) -> u16 {
    match r {
        ImageResult::BadName | ImageResult::Empty => 400,
        ImageResult::TooLarge => 413,
        ImageResult::NoSpace | ImageResult::TooMany => 507,
        ImageResult::Busy => 409,
        ImageResult::Ok | ImageResult::Io | ImageResult::NotFound => 500,
    }
}

// ---------------------------------------------------------------- the server

/// The web server of the firmware: the working set and the request pipeline (design 4.2). One
/// instance, owned by the HTTP thread; [`Web::handle`] serves one request.
pub struct Web<'a, P: Platform, N, F: Fs, G, S, O: OtaHost, H> {
    ports: WebPorts<'a, P>,
    storage: &'a Storage<'a, N, F, G, S>,
    ota: &'a OtaShared,
    upload: OtaUpload<'a, P, O>,
    host: H,
    assets: &'a [Asset],
    parts: Parts,
    work: Option<WorkSet>,
    /// RequestRefused once per verdict per 60 s.
    refused: RepeatLimiter,
}

impl<'a, P, N, F, G, S, O, H> Web<'a, P, N, F, G, S, O, H>
where
    P: Platform,
    N: Nvs,
    F: Fs,
    G: HeapGate,
    S: StorageHost,
    O: OtaHost,
    H: WebHost,
{
    /// The server over its ports, storage, the ESP upload, its host and the dashboard
    /// `assets`. Allocates nothing: the first request takes the working set.
    pub fn new(
        ports: WebPorts<'a, P>,
        storage: &'a Storage<'a, N, F, G, S>,
        ota: &'a OtaShared,
        upload: OtaUpload<'a, P, O>,
        host: H,
        assets: &'a [Asset],
    ) -> Self {
        Web {
            ports,
            storage,
            ota,
            upload,
            host,
            assets,
            parts: Parts::default(),
            work: None,
            refused: RepeatLimiter::default(),
        }
    }

    /// Serves one request: answers it exactly once, or not at all when the client went away
    /// or stalled.
    pub fn handle(&mut self, req: &mut dyn HttpRequest) {
        self.host.note_inbound_http(req.remote_ip());
        let Some(mut work) = self.take_work() else {
            return out_of_memory(req);
        };
        self.refresh_config(&mut work);
        self.serve(req, &mut work);
        self.work = Some(work);
    }

    /// The working set, allocating the parts still missing (C++ `allocWork`): `None` while one
    /// of them cannot be allocated (the others are kept).
    fn take_work(&mut self) -> Option<WorkSet> {
        if let Some(w) = self.work.take() {
            return Some(w);
        }
        let gate = self.ports.gate;
        let p = core::mem::take(&mut self.parts);
        let snap = p.snap.or_else(|| try_block(gate, StmSnapshot::default));
        let cfg = p.cfg.or_else(|| try_block(gate, Config::default));
        let doc = p.doc.or_else(|| try_block(gate, || JsonDocument::EMPTY));
        let multipart = p.multipart.or_else(|| try_block(gate, || Multipart::EMPTY));
        match (snap, cfg, doc, multipart) {
            (Some(snap), Some(cfg), Some(doc), Some(multipart)) => Some(WorkSet {
                snap,
                cfg,
                doc,
                multipart,
                hostname: Text::new(),
                cfg_revision: None,
                response: None,
            }),
            (snap, cfg, doc, multipart) => {
                self.parts = Parts {
                    snap,
                    cfg,
                    doc,
                    multipart,
                };
                None
            }
        }
    }

    /// C++ `refreshConfig`: the config copy and the host name follow the active config.
    fn refresh_config(&self, w: &mut WorkSet) {
        let shared = self.storage.shared();
        let rev = shared.config_revision();
        if w.cfg_revision == Some(rev) {
            return;
        }
        w.cfg_revision = Some(rev);
        shared.get_config(&mut w.cfg);
        let mut name = [0u8; STATION_NAME_MAX + 1];
        let n = build_hostname(&w.cfg.station, &mut name);
        w.hostname.clear();
        let _ = w
            .hostname
            .extend_from_slice(name.get(..n).unwrap_or_default());
    }

    /// Reloads the config copy after a patch that was not saved.
    fn reload_config(&self, w: &mut WorkSet) {
        self.storage.shared().get_config(&mut w.cfg);
    }

    // The pipeline is split into small functions: every frame stays below the 1 KiB of
    // clippy::large_stack_frames (design 2.4), the path and header buffers included.

    /// The decoded path (C++ `req->url()`), then the rest of the request.
    fn serve(&mut self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        let mut path = [0u8; PATH_MAX];
        let n = url_decode(split_target(req.target()).0, &mut path);
        self.serve_path(req, w, path.get(..n.min(PATH_MAX)).unwrap_or_default());
    }

    /// The head of the request, then /api/* or the other paths.
    fn serve_path(&mut self, req: &mut dyn HttpRequest, w: &mut WorkSet, path: &[u8]) {
        let mut ctype = [0u8; HEADER_MAX];
        let h = Head {
            method: req.method(),
            path,
            len: req.content_length(),
            ctype: header(req, "Content-Type", &mut ctype),
        };
        if h.path.starts_with(b"/api/") {
            self.serve_api(req, w, &h);
        } else {
            self.serve_legacy(req, w, &h);
        }
    }

    /// Paths outside /api/: the refusals, then the aliases and the dashboard files (C++
    /// `handleNonApi`).
    fn serve_legacy(&mut self, req: &mut dyn HttpRequest, w: &mut WorkSet, h: &Head<'_>) {
        let lm = match_legacy_route(h.method, h.path);
        if self.legacy_refused(req, w, h, &lm) {
            return;
        }
        let Some(mut body) = self.read_body(req, h.len) else {
            return;
        };
        match lm.route {
            LegacyRoute::Valves => self.legacy_valves(req, w),
            LegacyRoute::Temps => self.legacy_sensors(req, w, true),
            LegacyRoute::Volts => self.legacy_sensors(req, w, false),
            LegacyRoute::SetValve => self.set_valve(req, w, &mut body),
            _ if h.method == HttpMethod::Get => self.static_asset(req, h.path),
            _ => send_error(req, 405, "method_not_allowed", h.path),
        }
    }

    /// The 410 table, 405 for an alias with another method, the guard of the aliases, every
    /// other request with a body refused (404/405, never read) (C++ `legacyRefusal`); true when
    /// the request was answered.
    fn legacy_refused(
        &mut self,
        req: &mut dyn HttpRequest,
        w: &WorkSet,
        h: &Head<'_>,
        lm: &LegacyMatch,
    ) -> bool {
        match lm.route {
            LegacyRoute::Gone => send_error(req, 410, "gone", lm.replacement.as_bytes()),
            LegacyRoute::MethodNotAllowed => send_error(req, 405, "method_not_allowed", h.path),
            LegacyRoute::Valves | LegacyRoute::Temps | LegacyRoute::Volts => {
                return self.guard(req, w, h, GuardScope::LegacyRead, false);
            }
            LegacyRoute::SetValve => {
                if self.guard(req, w, h, GuardScope::LegacyWrite, false) {
                    return true;
                }
                if h.len > MAX_BODY_SIZE {
                    send_error(req, 413, "too_large", b"body");
                    return true;
                }
                return false;
            }
            LegacyRoute::None => {
                if h.len > 0 {
                    let (code, error) = if h.method == HttpMethod::Get {
                        (404, "not_found")
                    } else {
                        (405, "method_not_allowed")
                    };
                    send_error(req, code, error, h.path);
                    return true;
                }
                return false;
            }
        }
        true
    }

    /// /api/*: the route, the guard, the limits of an upload or a body (C++ `refusal`), then
    /// the handler (C++ `handleApi`).
    fn serve_api(&mut self, req: &mut dyn HttpRequest, w: &mut WorkSet, h: &Head<'_>) {
        let m = match_api_route(h.method, h.path);
        let upload = matches!(m.route, ApiRoute::StmImageUpload | ApiRoute::EspOta);
        if self.guard(req, w, h, GuardScope::Api, upload) {
            return;
        }
        if upload {
            if !self.upload_refused(req, h, m.route) {
                self.upload(req, w, h, m.route);
            }
            return;
        }
        if body_refused(req, h) {
            return;
        }
        let Some(mut body) = self.read_body(req, h.len) else {
            return;
        };
        self.api(req, w, h, &m, &mut body);
    }

    /// The limits of an upload: multipart, a Content-Length of 1..limit, no upload or flash
    /// running; true when the request was answered.
    fn upload_refused(&self, req: &mut dyn HttpRequest, h: &Head<'_>, route: ApiRoute) -> bool {
        if !h.multipart() {
            send_error(
                req,
                415,
                "unsupported_media_type",
                b"multipart/form-data required",
            );
        } else if h.len == 0 {
            send_error(req, 411, "length_required", b"Content-Length");
        } else if h.len > self.upload_limit(route) {
            send_error(req, 413, "too_large", b"file");
        } else if self.upload_busy() {
            send_error(req, 409, "busy", b"upload or flash running");
        } else {
            return false;
        }
        true
    }

    /// The request guard of an /api/* request or a legacy alias (C++ `guardRefusal`); true when
    /// the request was refused (answered, and logged as RequestRefused once per verdict and
    /// minute).
    fn guard(
        &mut self,
        req: &mut dyn HttpRequest,
        w: &WorkSet,
        h: &Head<'_>,
        scope: GuardScope,
        upload: bool,
    ) -> bool {
        let mut host = [0u8; HEADER_MAX];
        let host = header(req, "Host", &mut host).unwrap_or_default();
        let p = HostPolicy {
            local_ip: req.local_ip(),
            iface_ip: self.host.net_info().ip,
            hostname: &w.hostname,
            allowed: &w.cfg.web.allowed_hosts,
        };
        self.guard_request(req, h, scope, upload, host, &p)
    }

    /// The guard with the Host value read.
    fn guard_request(
        &mut self,
        req: &mut dyn HttpRequest,
        h: &Head<'_>,
        scope: GuardScope,
        upload: bool,
        host: &[u8],
        p: &HostPolicy<'_>,
    ) -> bool {
        let mut origin = [0u8; HEADER_MAX];
        let mut marker = [0u8; 2];
        let g = GuardRequest {
            method: h.method,
            scope,
            upload,
            has_body: h.len > 0,
            host,
            origin: header(req, "Origin", &mut origin),
            marker: header(req, "X-VdMot", &mut marker),
            content_type: h.ctype.map(media_type),
        };
        let v = check_request(&g, p);
        if v == GuardVerdict::Allow {
            return false;
        }
        self.refuse(req, v, &g, p);
        true
    }

    /// The answer of a guard refusal, logged once per verdict and minute.
    fn refuse(
        &mut self,
        req: &mut dyn HttpRequest,
        v: GuardVerdict,
        g: &GuardRequest<'_>,
        p: &HostPolicy<'_>,
    ) {
        let mut detail = [0u8; GUARD_DETAIL_SIZE];
        let n = guard_detail(v, g, p, &mut detail);
        if self.refused.allow(v as u8, self.ports.clock.now_ms()) {
            let mut ip = [0u8; 16];
            let k = format_ipv4(req.remote_ip(), &mut ip);
            self.log(
                EventCode::RequestRefused,
                v as i32,
                ip.get(..k).unwrap_or_default(),
            );
        }
        send_error(
            req,
            guard_http_status(v),
            guard_error_code(v),
            detail.get(..n).unwrap_or_default(),
        );
    }

    /// Content-Length limit of an upload: the STM image plus the framing, or the update slot
    /// plus the framing (only the framing without one).
    fn upload_limit(&self, route: ApiRoute) -> usize {
        if route == ApiRoute::StmImageUpload {
            return MAX_STM_IMAGE_SIZE + MULTIPART_SLACK;
        }
        let slot = self.ports.ota.other().map_or(0, |s| s.size as usize);
        slot + MULTIPART_SLACK
    }

    /// An upload or an STM flash runs (C++ `uploadBusy`; the server's own upload is over before
    /// the next request).
    fn upload_busy(&self) -> bool {
        self.ota.upload_active()
            || self.storage.image_upload_active()
            || self.host.stm_flash_active()
    }

    /// The body (Content-Length bytes) in a heap buffer of its length; `None` when the request
    /// was answered (no memory: 503) or the client went away or stalled (no answer).
    fn read_body(&self, req: &mut dyn HttpRequest, len: usize) -> Option<Vec<u8>> {
        if len == 0 {
            return Some(Vec::new());
        }
        let Some(mut body) = try_bytes(self.ports.gate, len) else {
            out_of_memory(req);
            return None;
        };
        let mut got = 0;
        let mut stalls = 0;
        loop {
            // End once Content-Length bytes were read (the rest of `body` is then empty)
            match req.read_body(body.get_mut(got..).unwrap_or_default()) {
                BodyRead::Data(n) if n > 0 => {
                    got += n;
                    stalls = 0;
                }
                BodyRead::Data(_) | BodyRead::Timeout => {
                    stalls += 1;
                    if stalls >= BODY_TIMEOUTS {
                        return None;
                    }
                }
                BodyRead::End => break,
                BodyRead::Closed => return None,
            }
        }
        body.truncate(got);
        Some(body)
    }

    /// An event with the code's default severity and no valve (C++ `logger::log`).
    fn log(&self, code: EventCode, arg1: i32, text: &[u8]) {
        let e = make_event(code, event_default_severity(code), NO_VALVE, arg1, 0, text);
        self.host.log(&e);
    }

    /// C++ `ota::requestRestart(reason, 1000)`: the restart path runs it in 1 s; the event of a
    /// request that counts is logged.
    fn request_restart(&self, reason: RebootReason) {
        let now = self.ports.clock.now_ms();
        if let Some(e) = self
            .ota
            .request_restart(now, reason as u8, RESTART_DELAY_MS, 0)
        {
            self.host.log(&e);
        }
    }

    /// Queues `cmd`; false (503 `queue_full` answered) when the queue is full.
    fn submit(&self, req: &mut dyn HttpRequest, cmd: &StmCommand) -> bool {
        if self.host.submit(cmd) {
            return true;
        }
        send_error(req, 503, "queue_full", b"STM command queue full");
        false
    }

    // ------------------------------------------------------------ refusals of STM actions

    /// C++ `refuseWhileFlashing`: true (409 `flashing` answered) while an STM flash runs.
    fn refuse_while_flashing(&self, req: &mut dyn HttpRequest) -> bool {
        if !self.host.stm_flash_active() {
            return false;
        }
        send_error(req, 409, "flashing", b"STM flash in progress");
        true
    }

    /// C++ `refuseTooOld`: true (409 `stm_unsupported` answered) while the STM firmware is
    /// older than 1.4.0.
    fn refuse_too_old(&self, req: &mut dyn HttpRequest, snap: &mut StmSnapshot) -> bool {
        if self.host.stm_support() != StmSupport::TooOld {
            return false;
        }
        self.host.read_stm_snapshot(snap);
        let mut version = [0u8; 40];
        let n = format_version(&snap.version, &mut version);
        let version: &[u8] = if n == 0 {
            b"?"
        } else {
            version.get(..n).unwrap_or_default()
        };
        let mut detail = [0u8; 96];
        let mut t = TextBuf::new(&mut detail);
        t.push_bytes(b"STM firmware ");
        t.push_bytes(version);
        t.push_bytes(b" is older than 1.4.0: update the STM");
        send_error(req, 409, "stm_unsupported", t.as_bytes());
        true
    }

    /// C++ `refuseBelowV3`: protocol 3 commands (stop, safe mode).
    fn refuse_below_v3(&self, req: &mut dyn HttpRequest, snap: &mut StmSnapshot) -> bool {
        if self.refuse_too_old(req, snap) {
            return true;
        }
        if self.host.stm_protocol() >= 3 {
            return false;
        }
        send_error(req, 409, "stm_unsupported", b"STM protocol 3 required");
        true
    }

    // ------------------------------------------------------------ dispatch
    //
    // Four matches, each passing the routes it does not serve on to the next one: one match of
    // all routes would exceed the stack frame limit.

    /// The documents (GET), then the other routes.
    fn api(
        &self,
        req: &mut dyn HttpRequest,
        w: &mut WorkSet,
        h: &Head<'_>,
        m: &RouteMatch,
        body: &mut [u8],
    ) {
        match m.route {
            ApiRoute::Status => self.status(req, w),
            ApiRoute::Valves => self.valves(req, w),
            ApiRoute::ValveProfile => self.profile(req, w, m.valve),
            ApiRoute::Sensors => self.sensors(req, w),
            ApiRoute::Events => self.events(req, w),
            ApiRoute::ConfigGet => self.config_get(req, w, false),
            ApiRoute::ConfigExport => self.config_get(req, w, true),
            ApiRoute::Motor => self.motor(req, w),
            ApiRoute::StmImages => self.images(req, w),
            ApiRoute::StmFlashStatus => self.flash_status(req, w),
            ApiRoute::LogDownload => self.log_download(req, w),
            ApiRoute::Health => self.health(req, w),
            ApiRoute::Files => self.files(req, w),
            ApiRoute::ImportReport => self.import_report(req, w),
            _ => self.api_valves(req, w, h, m, body),
        }
    }

    /// The STM commands of the valves and the sensors, then the other routes.
    fn api_valves(
        &self,
        req: &mut dyn HttpRequest,
        w: &mut WorkSet,
        h: &Head<'_>,
        m: &RouteMatch,
        body: &mut [u8],
    ) {
        use StmCommandType as C;
        let v = m.valve;
        match m.route {
            ApiRoute::ValveTarget => self.target(req, w, v, body),
            ApiRoute::ValveCalibrate => self.simple(req, w, C::Calibrate, v),
            ApiRoute::ValveAssembly => self.simple(req, w, C::Assembly, v),
            ApiRoute::ValveServiceMove => self.service_move(req, w, v, body),
            ApiRoute::ValveSensors => self.valve_sensors(req, w, v, body),
            ApiRoute::ValveProfileRefresh => self.profile_refresh(req, w, v),
            ApiRoute::ValveStop => self.stop(req, w, v),
            ApiRoute::CalibrateAll => self.simple(req, w, C::Calibrate, ALL_VALVES),
            ApiRoute::AssemblyAll => self.simple(req, w, C::Assembly, ALL_VALVES),
            ApiRoute::Detect => self.simple(req, w, C::Detect, ALL_VALVES),
            ApiRoute::StopAll => self.stop(req, w, ALL_VALVES),
            ApiRoute::SensorsScan => self.simple(req, w, C::ScanSensors, NO_VALVE),
            _ => self.api_stm(req, w, h, m, body),
        }
    }

    /// The STM settings, images and flash, then the other routes.
    fn api_stm(
        &self,
        req: &mut dyn HttpRequest,
        w: &mut WorkSet,
        h: &Head<'_>,
        m: &RouteMatch,
        body: &mut [u8],
    ) {
        match m.route {
            ApiRoute::MotorSet => self.motor_set(req, w, body),
            ApiRoute::StmReset => self.stm_reset(req, w, body),
            ApiRoute::StmSafeModeLeave => self.safe_mode_leave(req, w),
            ApiRoute::StmImageDelete => self.image_delete(req, &m.name),
            ApiRoute::StmFlash => self.flash(req, w, body),
            ApiRoute::StmFlashAbort => self.flash_abort(req),
            _ => self.api_system(req, w, h, m.route, body),
        }
    }

    /// The config, the system, MQTT, the network and the files; 405, and 404 for the rest
    /// (NotFound; the uploads were served before).
    fn api_system(
        &self,
        req: &mut dyn HttpRequest,
        w: &mut WorkSet,
        h: &Head<'_>,
        route: ApiRoute,
        body: &mut [u8],
    ) {
        match route {
            ApiRoute::ConfigPatch => self.config_patch(req, w, body),
            ApiRoute::Reboot => {
                self.request_restart(RebootReason::User);
                send(req, 202, JSON, b"{\"result\":\"restarting\"}");
            }
            ApiRoute::FactoryReset => self.factory_reset(req, w, body),
            ApiRoute::OtaSwitchBack => self.switch_back(req, w, body),
            ApiRoute::MqttReconnect => {
                self.host.request_mqtt_reconnect();
                accepted(req);
            }
            ApiRoute::MqttDiscovery => self.discovery(req, w, body),
            ApiRoute::NetConfirm => self.net_trial(req, true),
            ApiRoute::NetRevert => self.net_trial(req, false),
            ApiRoute::FileDelete => self.file_delete(req),
            ApiRoute::ImportReportDismiss => self.import_report_dismiss(req),
            ApiRoute::MethodNotAllowed => send_error(req, 405, "method_not_allowed", h.path),
            _ => send_error(req, 404, "not_found", h.path),
        }
    }

    // ------------------------------------------------------------ STM actions

    /// An STM command without parameters (C++ `handleSimple`).
    fn simple(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, kind: StmCommandType, valve: u8) {
        if self.refuse_while_flashing(req) {
            return;
        }
        if kind != StmCommandType::ResetStm && self.refuse_too_old(req, &mut w.snap) {
            return;
        }
        if self.submit(req, &command(kind, valve)) {
            accepted(req);
        }
    }

    /// Queues a web target (C++ `submitTarget`); false when an error was answered.
    fn submit_target(
        &self,
        req: &mut dyn HttpRequest,
        cfg: &Config,
        valve: u8,
        target: u8,
    ) -> bool {
        let active = cfg.valves.get(usize::from(valve)).is_some_and(|v| v.active);
        if !active {
            send_error(req, 409, "inactive", b"valve not active");
            return false;
        }
        let cmd = StmCommand {
            pos: target,
            source: TargetSource::Web,
            ..command(StmCommandType::SetTarget, valve)
        };
        self.submit(req, &cmd)
    }

    /// POST /api/valves/{n}/target `{"target":0..100}`.
    fn target(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, valve: u8, body: &mut [u8]) {
        let Some(o) = parse(req, &mut w.doc, body) else {
            return;
        };
        let target = if only_keys(&o, &[b"target"]) {
            target_field(o.get(b"target"))
        } else {
            None
        };
        let Some(target) = target else {
            return send_error(req, 400, "out_of_range", b"target 0..100");
        };
        if !self.submit_target(req, &w.cfg, valve, target) {
            return;
        }
        let mut buf = [0u8; 48];
        let n = fmt_trunc(
            &mut buf,
            format_args!(
                "{{\"valve\":{},\"target\":{}}}",
                u32::from(valve) + 1,
                target
            ),
        );
        send(req, 202, JSON, buf.get(..n).unwrap_or_default());
    }

    /// POST /api/valves/{n}/service-move `{"dir":"open|close","counts":1..10000,"maxmA":5..60}`.
    fn service_move(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, valve: u8, body: &mut [u8]) {
        if self.refuse_while_flashing(req) || self.refuse_too_old(req, &mut w.snap) {
            return;
        }
        if self.host.stm_protocol() < 2 {
            return send_error(req, 409, "unsupported", b"STM protocol v2 required");
        }
        let Some(o) = parse(req, &mut w.doc, body) else {
            return;
        };
        let mut cmd = command(StmCommandType::ServiceMove, valve);
        if !move_fields(&o, &mut cmd) {
            return send_error(
                req,
                400,
                "out_of_range",
                b"dir open|close, counts 1..10000, maxmA 5..60",
            );
        }
        if self.submit(req, &cmd) {
            accepted(req);
        }
    }

    /// POST /api/valves/{n}/sensors `{"slot1":0..34,"slot2":0..34}`: the configured ids of the
    /// slots (0 = none).
    fn valve_sensors(
        &self,
        req: &mut dyn HttpRequest,
        w: &mut WorkSet,
        valve: u8,
        body: &mut [u8],
    ) {
        if self.refuse_while_flashing(req) || self.refuse_too_old(req, &mut w.snap) {
            return;
        }
        let Some(o) = parse(req, &mut w.doc, body) else {
            return;
        };
        let Some(slots) = sensor_slots(&o) else {
            return send_error(req, 400, "out_of_range", b"slot1/slot2 0..34, distinct");
        };
        let mut cmd = command(StmCommandType::SetValveSensors, valve);
        if let Err(error) = slot_ids(&w.cfg, slots, &mut cmd.ids) {
            return send_error(req, 400, "invalid", error);
        }
        if self.submit(req, &cmd) {
            accepted(req);
        }
    }

    /// POST /api/valves/{n}/profile: a fresh profile (protocol 2).
    fn profile_refresh(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, valve: u8) {
        if self.refuse_too_old(req, &mut w.snap) {
            return;
        }
        if self.host.stm_protocol() < 2 {
            return send_error(req, 409, "unsupported", b"STM protocol v2 required");
        }
        self.simple(req, w, StmCommandType::RequestProfile, valve);
    }

    /// POST /api/stm/motor: any of `motor`, `learnMovements`, `breakaway`, merged with the
    /// values read from the STM, as one command.
    fn motor_set(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, body: &mut [u8]) {
        if self.refuse_while_flashing(req) || self.refuse_too_old(req, &mut w.snap) {
            return;
        }
        let Some(o) = parse(req, &mut w.doc, body) else {
            return;
        };
        self.host.read_stm_snapshot(&mut w.snap);
        let mut cmd = command(StmCommandType::SetMotorSettings, NO_VALVE);
        let protocol = self.host.stm_protocol();
        if let Err(r) = motor_fields(&o, &w.snap, protocol, &mut cmd) {
            return r.send(req);
        }
        if self.submit(req, &cmd) {
            accepted(req);
        }
    }

    /// POST /api/stm/reset `{"confirm":true}`.
    fn stm_reset(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, body: &mut [u8]) {
        if self.refuse_while_flashing(req) {
            return;
        }
        let Some(o) = parse(req, &mut w.doc, body) else {
            return;
        };
        if bool_field(&o, b"confirm", None) != Some(true) {
            return send_error(req, 400, "confirm_required", b"{\"confirm\":true}");
        }
        self.simple(req, w, StmCommandType::ResetStm, NO_VALVE);
    }

    /// POST /api/valves/{n}/stop and /api/valves/stop (protocol 3).
    fn stop(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, valve: u8) {
        if self.refuse_while_flashing(req) || self.refuse_below_v3(req, &mut w.snap) {
            return;
        }
        if self.submit(req, &command(StmCommandType::StopValve, valve)) {
            accepted(req);
        }
    }

    /// POST /api/stm/safe-mode/leave (protocol 3).
    fn safe_mode_leave(&self, req: &mut dyn HttpRequest, w: &mut WorkSet) {
        if self.refuse_while_flashing(req) || self.refuse_below_v3(req, &mut w.snap) {
            return;
        }
        if self.submit(req, &command(StmCommandType::LeaveSafeMode, NO_VALVE)) {
            accepted(req);
        }
    }

    // ------------------------------------------------------------ images and flash

    /// DELETE /api/stm/images/{name}.
    fn image_delete(&self, req: &mut dyn HttpRequest, raw: &[u8]) {
        let mut name = [0u8; IMAGE_NAME_MAX + 1];
        let Some(n) = normalize_image_name(c_str(raw), &mut name) else {
            return send_error(req, 400, "bad_name", raw);
        };
        let name = name.get(..n).unwrap_or_default();
        if self.host.stm_flash_active() {
            return send_error(req, 409, "flashing", b"STM flash in progress");
        }
        match self.storage.delete_image(name) {
            ImageResult::Ok => send(req, 204, "", b""),
            ImageResult::NotFound => send_error(req, 404, "not_found", name),
            ImageResult::Busy => send_error(req, 409, "busy", name),
            _ => send_error(req, 500, "io_error", name),
        }
    }

    /// POST /api/stm/flash `{"image":"<name>","mode":"normal|blank","force":false,"board":"C1|C2"}`.
    fn flash(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, body: &mut [u8]) {
        let Some(o) = parse(req, &mut w.doc, body) else {
            return;
        };
        let mut cmd = command(StmCommandType::StartFlash, NO_VALVE);
        if !flash_fields(&o, &mut cmd) {
            return send_error(
                req,
                400,
                "bad_request",
                b"image, mode normal|blank, force, board C1|C2",
            );
        }
        if self.flash_refused(req, &mut w.snap, &cmd) {
            return;
        }
        if self.submit(req, &cmd) {
            accepted(req);
        }
    }

    /// The checks of a flash before it is queued: no upload or flash running, no restart
    /// pending, the image stored, scanned and valid (`force` accepts one without the
    /// handshake), the board; true when the request was answered.
    fn flash_refused(
        &self,
        req: &mut dyn HttpRequest,
        snap: &mut StmSnapshot,
        cmd: &StmCommand,
    ) -> bool {
        let name = cmd.image.as_slice();
        if self.upload_busy() {
            send_error(req, 409, "busy", b"upload or flash running");
            return true;
        }
        if self.ota.restart_pending() {
            send_error(req, 409, "restarting", b"ESP restart pending");
            return true;
        }
        let Some(e) = self.storage.find_image(name) else {
            send_error(req, 404, "not_found", name);
            return true;
        };
        if !e.scanned {
            send_error(req, 409, "validating", b"image check pending, retry");
            return true;
        }
        if e.check != FlashError::None && !(cmd.force && e.check == FlashError::ImageNoHandshake) {
            send_error(
                req,
                400,
                "invalid_image",
                flash_error_name(e.check).as_bytes(),
            );
            return true;
        }
        self.board_refused(req, snap, &e, cmd)
    }

    /// The board revision of a flash: the running STM's tag, else the user's choice; true when
    /// it does not fit the image or is needed and missing (unless `force`), answered.
    fn board_refused(
        &self,
        req: &mut dyn HttpRequest,
        snap: &mut StmSnapshot,
        e: &ImageEntry,
        cmd: &StmCommand,
    ) -> bool {
        self.host.read_stm_snapshot(snap);
        let running = c_str(&snap.version.hw);
        let board_hw = if running.is_empty() {
            cmd.board.as_slice()
        } else {
            running
        };
        let bc = check_board(&e.hw_tag, board_hw);
        if !cmd.force && bc == BoardCheck::Mismatch {
            let mut detail = [0u8; 40];
            let mut t = TextBuf::new(&mut detail);
            t.push_bytes(b"image ");
            t.push_bytes(c_str(&e.hw_tag).get(..3).unwrap_or(c_str(&e.hw_tag)));
            t.push_bytes(b", board ");
            t.push_bytes(board_hw.get(..3).unwrap_or(board_hw));
            send_error(req, 409, "board_mismatch", t.as_bytes());
            return true;
        }
        if !cmd.force && bc == BoardCheck::BoardRequired {
            send_error(req, 409, "board_required", b"choose the board: C1 or C2");
            return true;
        }
        false
    }

    /// POST /api/stm/flash/abort.
    fn flash_abort(&self, req: &mut dyn HttpRequest) {
        if !self.host.stm_flash_active() {
            return send_error(req, 409, "idle", b"no flash running");
        }
        if self.submit(req, &command(StmCommandType::AbortFlash, NO_VALVE)) {
            accepted(req);
        }
    }

    // ------------------------------------------------------------ config and system

    /// POST /api/config (and `?dryRun=1`): the patch is applied to the config copy and checked
    /// as a whole (core `apply_config_json`); a save stores it, a dry run only answers what a
    /// save would do. Every patch that is not saved reloads the copy.
    fn config_patch(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, body: &[u8]) {
        let mut q = [0u8; QUERY_MAX];
        let dry_run = query(req, b"dryRun", &mut q);
        if dry_run.is_some_and(|v| v != b"1") {
            return send_error(req, 400, "bad_request", b"dryRun=1");
        }
        let dry_run = dry_run.is_some();
        if body.is_empty() {
            return send_error(req, 400, "bad_request", b"JSON body required");
        }
        // the answer of a save goes out of the response buffer: without one nothing is applied
        if !dry_run && response_buf(&mut w.response, self.ports.gate).is_none() {
            return out_of_memory(req);
        }
        let Some(info) = self.patch(req, w, body) else {
            return;
        };
        if dry_run {
            self.reload_config(w);
            return dry_run_answer(req, &info);
        }
        self.save_config(req, w, &info);
    }

    /// The patch applied to the config copy and checked as a whole: what saving it changes;
    /// `None` when its error was answered (the copy reloaded).
    fn patch(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, body: &[u8]) -> Option<ApplyInfo> {
        let mut path = [0u8; CONFIG_PATH_SIZE];
        let (r, n) = apply_config_json(&mut w.cfg, body, &mut path);
        if r != PatchResult::Ok {
            self.reload_config(w);
            send_error(req, 400, "invalid", path.get(..n).unwrap_or_default());
            return None;
        }
        Some(self.storage.shared().with_config(|active| ApplyInfo {
            restart_required: config_restart_reasons(active, &w.cfg) != 0,
            net_trial: net_trial_required(&active.net, &w.cfg.net),
        }))
    }

    /// Saves the patched copy and answers the config with what the save changes; a save that
    /// fails reloads the copy.
    fn save_config(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, info: &ApplyInfo) {
        let mut path = [0u8; CONFIG_PATH_SIZE];
        if let Err(n) = self.storage.apply_config(&w.cfg, &mut path) {
            self.reload_config(w);
            let path = path.get(..n).unwrap_or_default();
            let code = if path == b"nvs" { 500 } else { 400 };
            return send_error(req, code, "invalid", path);
        }
        self.refresh_config(w);
        let revision = self.storage.shared().config_revision();
        self.log(EventCode::ConfigSaved, revision as i32, b"web");
        let cfg = &*w.cfg;
        let Some(buf) = response_buf(&mut w.response, self.ports.gate) else {
            return out_of_memory(req);
        };
        let mut jw = JsonWriter::new(buf);
        if !write_config_json(&mut jw, cfg, Some(info)) {
            return send_error(req, 500, "internal", b"document too large");
        }
        send_document(req, 200, jw.as_bytes(), None);
    }

    /// POST /api/system/factory-reset `{"confirm":"factory-reset"}`.
    fn factory_reset(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, body: &mut [u8]) {
        let Some(o) = parse(req, &mut w.doc, body) else {
            return;
        };
        if o.get(b"confirm").as_str() != Some(b"factory-reset") {
            return send_error(
                req,
                400,
                "confirm_required",
                b"{\"confirm\":\"factory-reset\"}",
            );
        }
        if self.host.stm_flash_active() {
            return send_error(req, 409, "flashing", b"STM flash in progress");
        }
        if !self.storage.factory_reset() {
            return send_error(req, 500, "nvs", b"erase failed");
        }
        self.request_restart(RebootReason::FactoryReset);
        send(req, 202, JSON, b"{\"result\":\"restarting\"}");
    }

    /// POST /api/system/ota/switch-back `{"confirm":"switch-back"}` (Rust firmware, design
    /// 6.3): restart into the other image; works in any state of the boot guard.
    fn switch_back(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, body: &mut [u8]) {
        let Some(o) = parse(req, &mut w.doc, body) else {
            return;
        };
        if o.get(b"confirm").as_str() != Some(b"switch-back") {
            return send_error(
                req,
                400,
                "confirm_required",
                b"{\"confirm\":\"switch-back\"}",
            );
        }
        if self.upload_busy() {
            return send_error(req, 409, "busy", b"upload or flash running");
        }
        if self.ota.restart_pending() {
            return send_error(req, 409, "restarting", b"ESP restart pending");
        }
        if self.ports.ota.other().and_then(|s| s.app).is_none() {
            return send_error(req, 409, "no_fallback", b"no image in the other slot");
        }
        self.request_restart(RebootReason::SwitchBack);
        send(req, 202, JSON, b"{\"result\":\"restarting\"}");
    }

    /// POST /api/mqtt/discovery `{"action":"publish|delete|republish"}`.
    fn discovery(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, body: &mut [u8]) {
        let Some(o) = parse(req, &mut w.doc, body) else {
            return;
        };
        if w.cfg.mqtt.mode == MqttMode::Off {
            return send_error(req, 409, "disabled", b"MQTT is off");
        }
        let Some(a) = discovery_action(o.get(b"action")) else {
            return send_error(req, 400, "bad_request", b"action publish|delete|republish");
        };
        if a != DiscoveryAction::Delete && !w.cfg.mqtt.separate {
            return send_error(
                req,
                409,
                "separate_required",
                b"HA discovery needs separate topics",
            );
        }
        self.host.request_discovery(a);
        accepted(req);
    }

    /// POST /api/system/network/confirm and /revert.
    fn net_trial(&self, req: &mut dyn HttpRequest, confirm: bool) {
        let ok = if confirm {
            self.host.request_trial_confirm()
        } else {
            self.host.request_trial_revert()
        };
        if !ok {
            return send_error(req, 409, "no_trial", b"no network trial running");
        }
        accepted(req);
    }

    /// DELETE /api/import-report.
    fn import_report_dismiss(&self, req: &mut dyn HttpRequest) {
        if !self.storage.dismiss_import_report() {
            return send_error(req, 404, "not_found", b"no import report");
        }
        send(req, 204, "", b"");
    }

    /// DELETE /api/files?path=<path>.
    fn file_delete(&self, req: &mut dyn HttpRequest) {
        let mut buf = [0u8; QUERY_PATH_MAX];
        let Some(path) = query(req, b"path", &mut buf) else {
            return send_error(req, 400, "bad_path", b"path");
        };
        self.delete_file(req, path);
    }

    /// The file `path` of DELETE /api/files deleted unless it is protected (C++ the rest of
    /// `handleFileDelete`).
    fn delete_file(&self, req: &mut dyn HttpRequest, path: &[u8]) {
        if !vdm_esp_core::file_manager::fs_path_valid(path) {
            return send_error(req, 400, "bad_path", path);
        }
        match self.storage.delete_file(path) {
            FileResult::Ok => send(req, 204, "", b""),
            FileResult::BadPath => send_error(req, 400, "bad_path", path),
            FileResult::Protected => {
                let kind = vdm_esp_core::file_manager::classify_fs_path(path);
                let reason = vdm_esp_core::file_manager::file_protect_reason(kind);
                send_error(req, 403, "protected", reason.as_bytes());
            }
            FileResult::NotFound => send_error(req, 404, "not_found", path),
            FileResult::Io => send_error(req, 500, "io_error", path),
        }
    }

    // ------------------------------------------------------------ legacy aliases

    /// POST /setvalve `{"valve":1..12,"value":<number>}`; other members (legacy PI values) are
    /// ignored.
    fn set_valve(&self, req: &mut dyn HttpRequest, w: &mut WorkSet, body: &mut [u8]) {
        let Some(o) = parse(req, &mut w.doc, body) else {
            return;
        };
        let fields = (
            int_field(&o, b"valve", 1..=i64::from(VALVE_COUNT), None),
            target_field(o.get(b"value")),
        );
        let (Some(valve), Some(target)) = fields else {
            return send_error(req, 400, "out_of_range", b"valve 1..12, value 0..100");
        };
        if !self.submit_target(req, &w.cfg, valve as u8 - 1, target) {
            return;
        }
        send(req, 200, JSON, b"{\"res\":\"ok\"}");
    }
}

#[cfg(test)]
mod rig;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut_a;
#[cfg(test)]
mod tests_mut_a_views;
#[cfg(test)]
mod tests_mut_b1;
#[cfg(test)]
mod tests_mut_b2;
#[cfg(test)]
mod tests_rust;
#[cfg(test)]
mod tests_work;
