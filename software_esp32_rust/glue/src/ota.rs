//! ESP firmware OTA (C++ `ota.cpp`): the streaming upload into the other app slot through
//! [`update`] (the Arduino `Update` contract, optional MD5), the validation of a new image (the
//! health checks of D§16 with the loopback self-check) and its confirmation or switch back by
//! the boot guard (GLUE-DESIGN-ESP.md section 6), and the restart path every restart takes.
//!
//! [`OtaUpload`] runs in the HTTP thread (the upload handler), [`OtaService`] in the app thread
//! (validation every second, the restart path every pass); [`OtaShared`] holds what both of them
//! and the other modules read: the restart request, the health for /api/health and the upload
//! flags. The calls into other glue modules go through [`OtaHost`].
//!
//! Boot guard instead of PENDING_VERIFY: the image is on trial when the guard says so at boot
//! (`otaStm` was read into the trial record by the guard), MarkValid confirms it
//! ([`BootGuard::mark_valid`], event 108), the rollback of D§16 (restart reason 4) and the
//! manual switch back (reason 7) end the restart path with [`BootGuard::switch_to_fallback`]
//! instead of `esp_ota_mark_app_invalid_rollback_and_reboot`; an ESP upload is refused while the
//! image is on trial (the upload would overwrite the fallback).

pub mod update;

use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use vdm_esp_core::common::{c_str, elapsed_ms, format_ipv4, time_reached, TextBuf, NO_VALVE};
use vdm_esp_core::event_log::{
    event_default_severity, make_event, Event, EventCode, RebootReason, Severity,
};
use vdm_esp_core::json_api::OtaHealthInfo;
use vdm_esp_core::link_policy::LinkState;
use vdm_esp_core::ota_policy::{normalize_md5, OtaValidator, OtaValidatorDecision};
use vdm_esp_core::restart_gate::{RestartGate, RestartGateStep};
use vdm_esp_core::stm_types::StmSaveState;
use vdm_esp_core::sys_health::http_status_ok;

use crate::boot_guard::{BootGuard, BootReport, BootVerdict, SwitchCause, STABLE_S};
use crate::port::{Clock, Ota, Platform, System, TcpConnector, TcpRead, TcpStream};
use update::{Update, UPDATE_SIZE_UNKNOWN};

/// Multipart framing around the firmware file (boundaries, part headers and an optional MD5
/// field) is far below this: Content-Length may be the partition size plus this.
pub const FRAMING_SLACK: u32 = 16 * 1024;
/// Loopback self-check: connect limit.
pub const SELF_CHECK_CONNECT_MS: u32 = 1000;
/// Loopback self-check: the status line must arrive within this.
pub const SELF_CHECK_ANSWER_MS: u32 = 3000;
/// Wait between two looks at a silent self-check connection.
pub const SELF_CHECK_POLL_MS: u32 = 10;
/// Bytes of the status line the self-check reads ("HTTP/1.1 200").
pub const SELF_CHECK_STATUS_BYTES: usize = 12;
/// The restart into an uploaded image and the rollback restart wait this long (the HTTP
/// response, MQTT `offline`).
pub const RESTART_DELAY_MS: u32 = 1000;

const USER: u8 = RebootReason::User as u8;
const OTA: u8 = RebootReason::Ota as u8;
const NET_WATCHDOG: u8 = RebootReason::NetWatchdog as u8;
const FACTORY_RESET: u8 = RebootReason::FactoryReset as u8;
const ROLLBACK: u8 = RebootReason::Rollback as u8;
const NET_REVERT: u8 = RebootReason::NetRevert as u8;
const HEAP_GUARD: u8 = RebootReason::HeapGuard as u8;
const SWITCH_BACK: u8 = RebootReason::SwitchBack as u8;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// RebootRequested (109) with the severity of its reason: Warning for the network watchdog
/// (2), a rollback (4), a network revert (5) and the heap guard (6), Info otherwise.
pub fn reboot_event(reason: u8, detail: i32) -> Event {
    let sev = if matches!(reason, NET_WATCHDOG | ROLLBACK | NET_REVERT | HEAP_GUARD) {
        Severity::Warning
    } else {
        Severity::Info
    };
    make_event(
        EventCode::RebootRequested,
        sev,
        NO_VALVE,
        i32::from(reason),
        detail,
        b"",
    )
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RestartState {
    pending: bool,
    at_ms: u32,
    reason: u8,
}

/// What ota publishes for the other threads (C++ file statics behind `gMux` and the volatile
/// flags).
#[derive(Debug, Default)]
pub struct OtaShared {
    restart: Mutex<RestartState>,
    health: Mutex<OtaHealthInfo>,
    upload_active: AtomicBool,
    /// The STM link was up when the last upload of this boot ended (written by the upload, read
    /// by the restart path).
    upload_stm_up: AtomicBool,
    on_trial: AtomicBool,
}

impl OtaShared {
    /// No restart pending, no upload, not on trial.
    pub fn new() -> Self {
        Self::default()
    }

    /// A deferred restart from any thread (C++ `ota::requestRestart`): the restart path runs it
    /// `delay_ms` after `now_ms`, so responses and MQTT `offline` can go out. The first request
    /// wins and later ones are ignored, except that an uploaded image (reason 1) replaces a
    /// pending rollback (4), which would discard it, and a pending switch back (7, Rust only),
    /// which would select the uploaded slot as the confirmed image without its trial (its time
    /// stands). Returns the RebootRequested event (`detail`: outage minutes of the network
    /// watchdog, missing checks of a rollback) when the request counts; the caller logs it.
    #[must_use]
    pub fn request_restart(
        &self,
        now_ms: u32,
        reason: u8,
        delay_ms: u32,
        detail: i32,
    ) -> Option<Event> {
        let mut r = lock(&self.restart);
        if !r.pending {
            *r = RestartState {
                pending: true,
                at_ms: now_ms.wrapping_add(delay_ms),
                reason,
            };
        } else if reason == OTA && matches!(r.reason, ROLLBACK | SWITCH_BACK) {
            r.reason = reason;
        } else {
            return None;
        }
        drop(r);
        Some(reboot_event(reason, detail))
    }

    /// The validator gave up: the rollback restart (reason 4) in 1 s through the common path. A
    /// restart already pending becomes the rollback and keeps its time, except the restart into
    /// an uploaded image: that image is selected already and the rollback would discard it.
    /// Returns the event to log.
    fn request_rollback(&self, now_ms: u32, missing: i32) -> Option<Event> {
        let mut r = lock(&self.restart);
        if r.pending && r.reason == OTA {
            return None;
        }
        if !r.pending {
            r.pending = true;
            r.at_ms = now_ms.wrapping_add(RESTART_DELAY_MS);
        }
        r.reason = ROLLBACK;
        Some(reboot_event(ROLLBACK, missing))
    }

    /// The reason of a restart that is due at `now_ms`.
    fn due_restart(&self, now_ms: u32) -> Option<u8> {
        let r = lock(&self.restart);
        (r.pending && time_reached(now_ms, r.at_ms)).then_some(r.reason)
    }

    fn clear_restart(&self) {
        lock(&self.restart).pending = false;
    }

    /// A restart is pending (C++ `ota::restartPending`): MQTT goes offline, no STM flash starts.
    pub fn restart_pending(&self) -> bool {
        lock(&self.restart).pending
    }

    /// Validator state for /api/health (C++ `ota::health`; its time argument was not used).
    pub fn health(&self) -> OtaHealthInfo {
        *lock(&self.health)
    }

    /// An ESP upload runs (C++ `ota::uploadActive`).
    pub fn upload_active(&self) -> bool {
        self.upload_active.load(Ordering::SeqCst)
    }

    /// The running image is on trial (the boot guard): ESP uploads are refused.
    pub fn on_trial(&self) -> bool {
        self.on_trial.load(Ordering::SeqCst)
    }
}

/// What ota calls in the other glue modules (C++ `logger::`, `net::`, `app::`, `storage::`,
/// `stm_service::`), one method per C++ call; the firmware wiring implements it over their
/// shared objects. The boot guard replaced `storage::otaStmRequired` and its clear at boot.
pub trait OtaHost {
    /// C++ `logger::log(e)`.
    fn log(&mut self, e: &Event);
    /// C++ `logger::flush()`: the whole backlog to the file now (restart path).
    fn log_flush(&mut self);
    /// C++ `net::isUp()`.
    fn net_is_up(&mut self) -> bool;
    /// C++ `net::info().ip`.
    fn net_ip(&mut self) -> u32;
    /// C++ `app::stmFlashActive()`.
    fn stm_flash_active(&mut self) -> bool;
    /// Rust only (D9): the STM's sector 0 is erased and not written yet
    /// (`AppShared::stm_sector0_at_risk`).
    fn stm_sector0_at_risk(&mut self) -> bool;
    /// C++ `storage::imageUploadActive()`: an STM image upload runs.
    fn image_upload_active(&mut self) -> bool;
    /// C++ `app::stmLinkState()`.
    fn stm_link_state(&mut self) -> LinkState;
    /// C++ `app::stmSaveState()`.
    fn stm_save_state(&mut self) -> StmSaveState;
    /// C++ `app::requestStmSave()`: the stm task lets a pending EEPROM write finish.
    fn request_stm_save(&mut self);
    /// C++ `stm_service::flushForRestart()`: the desired targets to NVS.
    fn flush_for_restart(&mut self);
    /// C++ `storage::setOtaStmRequired(on)`: NVS `otaStm` (u8 1, or removed), read by the boot
    /// guard of the next boot.
    fn set_ota_stm_required(&mut self, on: bool);
}

/// The ESP upload (C++ `ota::upload*`), in the HTTP thread: one upload at a time, none while
/// an STM flash or an STM image upload runs or while the image is on trial.
pub struct OtaUpload<'a, P: Platform, H: OtaHost> {
    clock: &'a P::Clock,
    ota: &'a P::Ota,
    update: Update<&'a P::Ota, P::Md5, &'a P::HeapGate>,
    shared: &'a OtaShared,
    host: H,
    failed: bool,
    error: &'static str,
    written: usize,
}

impl<'a, P: Platform, H: OtaHost> OtaUpload<'a, P, H> {
    /// No upload yet.
    pub fn new(
        clock: &'a P::Clock,
        ota: &'a P::Ota,
        md5: P::Md5,
        heap: &'a P::HeapGate,
        shared: &'a OtaShared,
        host: H,
    ) -> Self {
        OtaUpload {
            clock,
            ota,
            update: Update::new(ota, md5, heap),
            shared,
            host,
            failed: false,
            error: "",
            written: 0,
        }
    }

    fn log(&mut self, code: EventCode, arg1: i32) {
        let e = make_event(code, event_default_severity(code), NO_VALVE, arg1, 0, b"");
        self.host.log(&e);
    }

    /// A failed upload: the error text, the update aborted, event 107 with `code`.
    fn fail(&mut self, text: &'static str, code: i32) {
        self.error = text;
        if self.update.is_running() {
            self.update.abort();
        }
        self.shared.upload_active.store(false, Ordering::SeqCst);
        self.failed = true;
        self.log(EventCode::EspOtaFailed, code);
    }

    /// [`OtaUpload::fail`] with the error of the update.
    fn fail_with_update(&mut self) {
        let (text, code) = (self.update.error_string(), self.update.error());
        self.fail(text, i32::from(code));
    }

    /// Starts an upload. False when an upload runs ("busy"), while the image is on trial ("image
    /// on trial"), while an STM flash or an STM image upload runs, without an update partition,
    /// when `announced` (the request's Content-Length, 0 = unknown) exceeds the partition plus
    /// [`FRAMING_SLACK`], or for an `md5` (a C string, "" = none) that is not 32 hex digits;
    /// [`OtaUpload::upload_error`] names the reason. A refused update start is event 107 with
    /// the update's code. `Update` gets the MD5 in lowercase and checks it at the end.
    pub fn upload_begin(&mut self, announced: usize, md5: &[u8]) -> bool {
        if self.shared.upload_active() {
            self.error = "busy";
            return false;
        }
        self.failed = false;
        if self.shared.on_trial() {
            self.error = "image on trial";
            return false;
        }
        if self.host.stm_flash_active() || self.host.image_upload_active() {
            self.error = "stm flash or upload running";
            return false;
        }
        let Some(target) = self.ota.other() else {
            self.error = "no ota partition";
            return false;
        };
        if announced as u64 > u64::from(target.size) + u64::from(FRAMING_SLACK) {
            self.error = "image too large";
            return false;
        }
        let lower = if c_str(md5).is_empty() {
            None
        } else {
            let Some(l) = normalize_md5(md5) else {
                self.error = "md5 invalid";
                return false;
            };
            Some(l)
        };
        if !self.update.begin(UPDATE_SIZE_UNKNOWN) {
            self.error = self.update.error_string();
            let code = i32::from(self.update.error());
            self.log(EventCode::EspOtaFailed, code);
            return false;
        }
        if let Some(l) = lower {
            if !self.update.set_md5(&l) {
                self.update.abort();
                self.error = "md5 invalid";
                return false;
            }
        }
        self.shared.upload_active.store(true, Ordering::SeqCst);
        self.error = "";
        self.written = 0;
        self.log(EventCode::EspOtaStarted, announced as i32);
        true
    }

    /// The next bytes of the image, in order. A failed write aborts the upload (later bytes are
    /// refused) with the update's error.
    pub fn upload_write(&mut self, data: &[u8]) -> bool {
        if !self.shared.upload_active() {
            return false;
        }
        if data.is_empty() {
            return true;
        }
        if self.update.write(data) != data.len() {
            self.fail_with_update();
            return false;
        }
        self.written = self.written.wrapping_add(data.len());
        true
    }

    /// `commit`: the image is complete: the update ends with what was written (the image
    /// verified, the MD5 checked when one was given, the slot selected for boot), event 106,
    /// and the restart into it is requested in 1 s (after the HTTP response). Without `commit`
    /// the upload is aborted (event 107 -1); an empty image fails with -2.
    pub fn upload_end(&mut self, commit: bool) -> bool {
        if !self.shared.upload_active() {
            if !self.failed {
                self.error = "no upload";
            }
            return false;
        }
        if !commit {
            self.fail("aborted", -1);
            return false;
        }
        if self.written == 0 {
            self.fail("empty image", -2);
            return false;
        }
        // size unknown up front (multipart): end(true) takes what was written
        if !self.update.end(true) {
            self.fail_with_update();
            return false;
        }
        self.shared.upload_active.store(false, Ordering::SeqCst);
        self.log(EventCode::EspOtaDone, self.written as i32);
        // the new image must prove the STM link only when it was up now; the NVS write happens
        // in the restart path
        let stm_up = self.host.stm_link_state() == LinkState::Up;
        self.shared.upload_stm_up.store(stm_up, Ordering::SeqCst);
        if let Some(e) = self
            .shared
            .request_restart(self.clock.now_ms(), OTA, RESTART_DELAY_MS, 0)
        {
            self.host.log(&e);
        }
        true
    }

    /// An upload runs.
    pub fn upload_active(&self) -> bool {
        self.shared.upload_active()
    }

    /// Why the last upload step failed; "unknown" when nothing failed.
    pub fn upload_error(&self) -> &'static str {
        if self.error.is_empty() {
            "unknown"
        } else {
            self.error
        }
    }
}

/// The ports of the app-thread part: the clock, the loopback connection, and the stores the
/// boot guard writes.
pub struct OtaPorts<'a, P: Platform> {
    pub clock: &'a P::Clock,
    pub tcp: &'a P::Tcp,
    pub nvs: &'a P::Nvs,
    pub ota: &'a P::Ota,
    pub rtc: &'a P::Rtc,
    pub system: &'a P::System,
}

/// Validation of a new image and the restart path (C++ `ota::begin`, `ota::service`,
/// `ota::serviceRestart`), in the app thread.
pub struct OtaService<'a, P: Platform, H: OtaHost> {
    ports: OtaPorts<'a, P>,
    shared: &'a OtaShared,
    host: H,
    guard: BootGuard,
    validator: OtaValidator,
    gate: RestartGate,
    /// D9: the due restart logged that it waits for the STM's sector 0
    deferral_logged: bool,
    /// F5: the boot has run [`STABLE_S`] and told the boot guard
    stable_noted: bool,
}

impl<'a, P: Platform, H: OtaHost> OtaService<'a, P, H> {
    /// `guard`: the boot guard's decision of this boot ([`BootGuard::boot`] in `main`).
    pub fn new(ports: OtaPorts<'a, P>, shared: &'a OtaShared, host: H, guard: BootGuard) -> Self {
        OtaService {
            ports,
            shared,
            host,
            guard,
            validator: OtaValidator::default(),
            gate: RestartGate::default(),
            deferral_logged: false,
            stable_noted: false,
        }
    }

    fn log(&mut self, code: EventCode, arg1: i32, arg2: i32, text: &[u8]) {
        let e = make_event(
            code,
            event_default_severity(code),
            NO_VALVE,
            arg1,
            arg2,
            text,
        );
        self.host.log(&e);
    }

    fn publish_trial(&self) {
        self.shared
            .on_trial
            .store(self.guard.on_trial(), Ordering::SeqCst);
    }

    /// In `app::setup` once the logger runs: logs the boot guard's events of this boot (event
    /// 107: -4 the previous image failed its trial, -3 no switch possible) and arms the
    /// validator: an image on trial is validated, with the STM link when NVS `otaStm` said it
    /// was up at the upload.
    pub fn begin(&mut self, report: &BootReport) {
        for e in report.events.iter().flatten() {
            let (arg1, arg2) = e.args();
            self.log(EventCode::EspOtaFailed, arg1, arg2, b"");
        }
        let (pending, stm_required) = match self.guard.verdict() {
            BootVerdict::Trial { stm_required, .. } => (true, stm_required),
            BootVerdict::Confirmed => (false, false),
        };
        self.validator
            .begin(pending, stm_required, self.ports.clock.now_ms());
        *lock(&self.shared.health) = OtaHealthInfo {
            pending,
            stm_required,
            ..OtaHealthInfo::default()
        };
        self.publish_trial();
    }

    /// The image on trial passed: the boot guard confirms it, event 108 (seconds since boot).
    fn mark_valid(&mut self) {
        let p = &self.ports;
        self.guard.mark_valid(p.nvs, p.ota, p.rtc);
        self.publish_trial();
        let uptime_s = self.ports.clock.uptime_s() as i32;
        self.log(EventCode::AppMarkedValid, uptime_s, 0, b"");
    }

    /// GET /api/health over the loopback interface; true when the status line says 200. Blocks
    /// the app thread for at most about 4 s, only while the image is on trial.
    fn self_check(&mut self) -> bool {
        let mut ip = [0u8; 16];
        let n = format_ipv4(self.host.net_ip(), &mut ip);
        let mut buf = [0u8; SELF_CHECK_STATUS_BYTES];
        let mut got = 0;
        if let Some(mut c) = self
            .ports
            .tcp
            .connect("127.0.0.1", 80, SELF_CHECK_CONNECT_MS)
        {
            let mut req = [0u8; 96];
            let mut w = TextBuf::new(&mut req);
            w.push_bytes(b"GET /api/health HTTP/1.1\r\nHost: ");
            w.push_bytes(&ip[..n]);
            w.push_bytes(b"\r\nConnection: close\r\n\r\n");
            c.write_all(w.as_bytes());
            let start = self.ports.clock.now_ms();
            while got < buf.len()
                && elapsed_ms(self.ports.clock.now_ms(), start) < SELF_CHECK_ANSWER_MS
            {
                match c.read(&mut buf[got..]) {
                    TcpRead::Data(r) if r > 0 => got = (got + r).min(buf.len()),
                    TcpRead::Data(_) | TcpRead::Empty if c.connected() => {
                        self.ports.clock.sleep_ms(SELF_CHECK_POLL_MS);
                    }
                    TcpRead::Data(_) | TcpRead::Empty | TcpRead::Closed => break,
                }
            }
            c.close();
        }
        http_status_ok(&buf[..got])
    }

    /// App thread, every second: the loopback self-check when due (while on trial and the web
    /// server runs, every 10 s), the validator: MarkValid confirms the image (event 108),
    /// Rollback requests restart reason 4 with the missing checks through the restart path.
    /// `net_ok`: the network check of `NetShared::ota_net_ok`; `web_started`: the HTTP server
    /// runs. F5: once the boot has run [`STABLE_S`], the boot guard's crash streak ends.
    pub fn service(&mut self, now_ms: u32, net_ok: bool, link_up: bool, web_started: bool) {
        if !self.stable_noted && self.ports.clock.uptime_s() >= STABLE_S {
            self.stable_noted = true;
            self.guard.note_stable(self.ports.rtc);
        }
        if self.validator.self_check_due(web_started, now_ms) && self.host.net_is_up() {
            let ok = self.self_check();
            self.validator.on_self_check(ok, now_ms);
        }
        match self.validator.update(net_ok, link_up, now_ms) {
            OtaValidatorDecision::MarkValid => self.mark_valid(),
            OtaValidatorDecision::Rollback => {
                let missing = i32::from(self.validator.missing());
                if let Some(e) = self.shared.request_rollback(now_ms, missing) {
                    self.host.log(&e);
                }
            }
            OtaValidatorDecision::NotPending | OtaValidatorDecision::Wait => {}
        }
        let v = &self.validator;
        let h = OtaHealthInfo {
            pending: v.pending(),
            stm_required: v.stm_required(),
            net_ok,
            http_ok: v.http_ok(now_ms),
            stm_ok: link_up,
            healthy_for_s: v.healthy_for_ms(now_ms) / 1000,
            remaining_s: v.remaining_ms(now_ms) / 1000,
        };
        *lock(&self.shared.health) = h;
    }

    /// App thread, every pass: a requested restart when it is due and no STM flash runs: the
    /// STM EEPROM save (the restart gate, 12 s guard: with jumper X20 the ESP restart also
    /// resets the STM) and the desired targets (except for a factory reset), then the
    /// confirmation of an image on trial by a user restart (reasons 0 and 3, when the network,
    /// the loopback self-check of the last 30 s and the required STM link are up; otherwise the
    /// restart counts as a boot of the trial), `otaStm` for an upload and no confirmed image
    /// left behind ([`BootGuard::leave_for_upload`]), the log flush, and the
    /// restart, or for a rollback (4) and a switch back (7) the boot guard's switch to the other
    /// image. A switch that cannot select the other image returns: this image keeps running as
    /// confirmed (event 107 -3) and later restarts go through the gate again. A restart that
    /// waits while the STM's sector 0 is not written (D9) logs once why (event 123).
    pub fn service_restart(&mut self, now_ms: u32, net_up: bool, link_up: bool) {
        let Some(reason) = self.shared.due_restart(now_ms) else {
            return;
        };
        // never in the middle of an STM flash (the STM would be left half-erased): retried
        // every pass until it is done; while its sector 0 is not written that may take long
        // (the restart would leave the STM without a vector table), so it says once why
        if self.host.stm_flash_active() {
            if !self.deferral_logged && self.host.stm_sector0_at_risk() {
                self.deferral_logged = true;
                self.log(EventCode::RestartDeferred, i32::from(reason), 0, b"");
            }
            return;
        }
        match self.gate.update(self.host.stm_save_state(), now_ms) {
            RestartGateStep::RequestStmSave => {
                self.host.request_stm_save();
                if reason != FACTORY_RESET {
                    self.host.flush_for_restart();
                }
                return;
            }
            RestartGateStep::Wait => return,
            RestartGateStep::Proceed => {}
        }
        if self.gate.guard_expired() {
            let waited = self.gate.waited_ms(now_ms) as i32;
            self.log(
                EventCode::StmEepromWaitTimeout,
                waited,
                3,
                b"stm task silent",
            );
        }
        // reboot button, network/station settings (0) or factory reset (3) during a trial, with
        // a fresh self-check of the web server: reason 0 also comes over MQTT (`cmd/restart`),
        // which proves no web server, and a confirmed image without one takes no upload
        let user = reason == USER || reason == FACTORY_RESET;
        if self.validator.http_ok(now_ms)
            && self.validator.confirm_before_restart(user, net_up, link_up)
        {
            self.mark_valid();
        }
        if reason == OTA {
            let stm_up = self.shared.upload_stm_up.load(Ordering::SeqCst);
            self.host.set_ota_stm_required(stm_up);
            self.guard.leave_for_upload(self.ports.nvs);
        }
        self.host.log_flush(); // the whole backlog, with the reason, before going down
        let cause = match reason {
            ROLLBACK => SwitchCause::Health,
            SWITCH_BACK => SwitchCause::Manual,
            _ => self.ports.system.restart(),
        };
        let p = &self.ports;
        let refused = self
            .guard
            .switch_to_fallback(p.nvs, p.ota, p.rtc, p.system, cause);
        // only without another image to select: keep running this one (better than a boot
        // loop); a trial ended as confirmed, so its validation stops too
        let (arg1, arg2) = refused.args();
        self.log(EventCode::EspOtaFailed, arg1, arg2, b"");
        if self.validator.pending() {
            let stm_required = self.validator.stm_required();
            self.validator.begin(false, stm_required, now_ms);
        }
        self.publish_trial();
        self.gate.reset();
        self.deferral_logged = false;
        self.shared.clear_restart();
    }
}

#[cfg(test)]
mod support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_boots;
#[cfg(test)]
mod tests_mut;
