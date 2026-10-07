//! Support of the ota suites: the [`OtaHost`] fake (the C++ sibling fakes of logger, net, app,
//! storage and stm_service), the loopback HTTP server, and rigs that boot the fake board through
//! the real boot guard (a confirmed image, an image on trial).
#![allow(clippy::large_stack_frames)]

use std::sync::{Arc, Mutex, MutexGuard};

use vdm_esp_core::event_log::{event_code_name, Event, EventCode};
use vdm_esp_core::link_policy::LinkState;
use vdm_esp_core::stm_types::StmSaveState;

use super::{OtaHost, OtaPorts, OtaService, OtaShared, OtaUpload};
use crate::boot_guard::{BootGuard, BootReport, KEY_STM, NVS_NAMESPACE};
use crate::port::{Clock, Nvs, NvsInt, NvsNamespace};
use crate::testkit::board::{TestPlatform, APP_A, APP_B};
use crate::testkit::net::{TcpPeer, Wire};
use crate::testkit::{lock, run, Device, FakeBoard, FakeNvs, Journal, Reset, SlotImage};

/// IPv4 in lwIP order, as `IPAddress(a, b, c, d)`.
pub(super) const fn ip(a: u8, b: u8, c: u8, d: u8) -> u32 {
    u32::from_le_bytes([a, b, c, d])
}

/// The device address of the C++ suites.
pub(super) const DEVICE_IP: u32 = ip(192, 168, 1, 20);

/// Scripted answers and recorded calls of the host.
pub(super) struct HostState {
    // scripted
    pub(super) net_up: bool,
    pub(super) net_ip: u32,
    pub(super) flash_active: bool,
    pub(super) image_upload_active: bool,
    pub(super) link: LinkState,
    pub(super) save_state: StmSaveState,
    // recorded
    pub(super) events: Vec<Event>,
    pub(super) flushes: u32,
    pub(super) save_requests: u32,
    pub(super) restart_flushes: u32,
    pub(super) ota_stm_sets: Vec<bool>,
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            net_up: false,
            net_ip: 0,
            flash_active: false,
            image_upload_active: false,
            link: LinkState::Unknown,
            save_state: StmSaveState::Idle,
            events: Vec::new(),
            flushes: 0,
            save_requests: 0,
            restart_flushes: 0,
            ota_stm_sets: Vec::new(),
        }
    }
}

/// The host of the tests; clones share the state. `set_ota_stm_required` also writes NVS as
/// storage does (u8 1 or removed), for the boot guard of the next boot.
#[derive(Clone)]
pub(super) struct FakeOtaHost {
    state: Arc<Mutex<HostState>>,
    journal: Journal,
    nvs: FakeNvs,
}

impl FakeOtaHost {
    pub(super) fn new(journal: Journal, nvs: FakeNvs) -> Self {
        FakeOtaHost {
            state: Arc::default(),
            journal,
            nvs,
        }
    }
    pub(super) fn state(&self) -> MutexGuard<'_, HostState> {
        lock(&self.state)
    }
    pub(super) fn with_code(&self, code: EventCode) -> Vec<Event> {
        self.state()
            .events
            .iter()
            .filter(|e| e.code == code)
            .cloned()
            .collect()
    }
    pub(super) fn has(&self, code: EventCode) -> bool {
        !self.with_code(code).is_empty()
    }
    pub(super) fn first(&self, code: EventCode) -> Event {
        match self.with_code(code).first() {
            Some(e) => e.clone(),
            None => panic!("no {code:?} event"),
        }
    }
}

impl OtaHost for FakeOtaHost {
    fn log(&mut self, e: &Event) {
        self.state().events.push(e.clone());
        self.journal
            .note(format!("logger.log {}", event_code_name(e.code)));
    }
    fn log_flush(&mut self) {
        self.state().flushes += 1;
        self.journal.note("logger.flush");
    }
    fn net_is_up(&mut self) -> bool {
        self.state().net_up
    }
    fn net_ip(&mut self) -> u32 {
        self.state().net_ip
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
        self.state().save_state
    }
    fn request_stm_save(&mut self) {
        let mut s = self.state();
        s.save_requests += 1;
        s.save_state = StmSaveState::Waiting;
        drop(s);
        self.journal.note("app.requestStmSave");
    }
    fn flush_for_restart(&mut self) {
        self.state().restart_flushes += 1;
        self.journal.note("stm_service.flushForRestart");
    }
    fn set_ota_stm_required(&mut self, on: bool) {
        self.state().ota_stm_sets.push(on);
        if let Some(mut ns) = self.nvs.open(NVS_NAMESPACE, true) {
            if on {
                ns.set_int(KEY_STM, NvsInt::U8, 1);
            } else {
                ns.remove(KEY_STM);
            }
        }
        self.journal.note("storage.setOtaStmRequired");
    }
}

/// The web server on the loopback interface: records every request, answers `first` at once
/// and the `later` parts at their times (fake ms since boot), then closes (C++ `tcpResponder`
/// and `tcpLater`; "" without later parts: the connection is closed).
struct Loopback {
    first: Vec<u8>,
    later: Vec<(u64, Vec<u8>)>,
    requests: Arc<Mutex<Vec<String>>>,
    answered: bool,
}

impl TcpPeer for Loopback {
    fn on_data(&mut self, wire: &mut Wire, data: &[u8], now_ms: u64) {
        lock(&self.requests).push(String::from_utf8_lossy(data).into_owned());
        if self.answered {
            return;
        }
        self.answered = true;
        if !self.first.is_empty() {
            wire.send_at(now_ms, &self.first);
        }
        let mut last = now_ms;
        for (at, bytes) in &self.later {
            wire.send_at(*at, bytes);
            last = *at;
        }
        wire.close_at(last);
    }
}

/// One boot: the device, the shared ota data, the host and the boot guard's decision.
pub(super) struct Rig {
    pub(super) board: FakeBoard,
    pub(super) dev: Device,
    pub(super) shared: OtaShared,
    pub(super) host: FakeOtaHost,
    pub(super) guard: BootGuard,
    pub(super) report: BootReport,
    /// Requests the loopback server got (C++ `gRequests`).
    pub(super) requests: Arc<Mutex<Vec<String>>>,
}

impl Rig {
    /// The next boot of `board` with the boot guard's decision (it must not switch).
    pub(super) fn boot(board: &FakeBoard) -> Self {
        let dev = board.boot();
        let (guard, report) =
            run(|| BootGuard::boot(&dev.nvs, &dev.ota, &dev.rtc, &dev.system)).returned();
        let host = FakeOtaHost::new(dev.journal.clone(), dev.nvs.clone());
        Rig {
            board: board.clone(),
            dev,
            shared: OtaShared::new(),
            host,
            guard,
            report,
            requests: Arc::default(),
        }
    }

    /// A confirmed image (A, its first boot confirmed it: no other image) in its second boot,
    /// without boot events; slot 1 is free for uploads.
    pub(super) fn confirmed() -> Self {
        let board = FakeBoard::new();
        drop(Self::boot(&board));
        board.reset(Reset::Software);
        Self::boot(&board)
    }

    /// An image on trial: B in slot 1, uploaded by A (slot 0, the fallback), not confirmed;
    /// `stm_required`: NVS `otaStm` from the upload.
    pub(super) fn trial(stm_required: bool) -> Self {
        let board = FakeBoard::with_slots([SlotImage::glue(APP_A), SlotImage::glue(APP_B)], 1);
        if stm_required {
            board.nvs().set_u8(NVS_NAMESPACE, KEY_STM, 1);
        }
        Self::boot(&board)
    }

    /// C++ `pendingImage(status, stmRequired)`: an image on trial, the network up at
    /// [`DEVICE_IP`], the loopback server answering `status` at once.
    pub(super) fn pending(status: &[u8], stm_required: bool) -> Self {
        let rig = Self::trial(stm_required);
        {
            let mut s = rig.host.state();
            s.net_up = true;
            s.net_ip = DEVICE_IP;
        }
        rig.loopback(status, Vec::new());
        rig
    }

    /// Starts the loopback server (replaces an earlier one).
    pub(super) fn loopback(&self, first: &[u8], later: Vec<(u64, Vec<u8>)>) {
        let first = first.to_vec();
        let requests = self.requests.clone();
        self.dev.tcp.listen("127.0.0.1", 80, move || {
            Box::new(Loopback {
                first: first.clone(),
                later: later.clone(),
                requests: requests.clone(),
                answered: false,
            })
        });
    }

    pub(super) fn requests(&self) -> Vec<String> {
        lock(&self.requests).clone()
    }

    /// The app-thread part of this boot (not begun).
    pub(super) fn service(&self) -> OtaService<'_, TestPlatform, FakeOtaHost> {
        OtaService::new(
            OtaPorts {
                clock: &self.dev.clock,
                tcp: &self.dev.tcp,
                nvs: &self.dev.nvs,
                ota: &self.dev.ota,
                rtc: &self.dev.rtc,
                system: &self.dev.system,
            },
            &self.shared,
            self.host.clone(),
            self.guard,
        )
    }

    /// The app-thread part after `begin` (C++ `ota::begin()`).
    pub(super) fn begun(&self) -> OtaService<'_, TestPlatform, FakeOtaHost> {
        let mut s = self.service();
        s.begin(&self.report);
        s
    }

    /// The upload of this boot.
    pub(super) fn upload(&self) -> OtaUpload<'_, TestPlatform, FakeOtaHost> {
        OtaUpload::new(
            &self.dev.clock,
            &self.dev.ota,
            self.dev.md5.clone(),
            &self.dev.heap,
            &self.shared,
            self.host.clone(),
        )
    }

    /// C++ `ota::requestRestart(reason, delay, detail)` at the fake time: the request and the
    /// event the caller logs.
    pub(super) fn request_restart(&self, reason: u8, delay_ms: u32, detail: i32) {
        let now = self.dev.clock.now_ms();
        if let Some(e) = self.shared.request_restart(now, reason, delay_ms, detail) {
            self.host.clone().log(&e);
        }
    }

    /// C++ `serviceSeconds(from, to, netOk, linkUp)`: `service` once a second, the fake clock
    /// in step, the web server running.
    pub(super) fn service_seconds(
        &self,
        svc: &mut OtaService<'_, TestPlatform, FakeOtaHost>,
        from: u64,
        to: u64,
        net_ok: bool,
        link_up: bool,
    ) {
        let mut t = from;
        while t <= to {
            self.dev.clock.set_ms(t);
            svc.service(self.dev.clock.now_ms(), net_ok, link_up, true);
            t += 1000;
        }
    }

    /// The `otaOk` record names `app` (the boot guard confirmed it).
    pub(super) fn confirmed_app(&self) -> Option<[u8; 8]> {
        let b = self
            .dev
            .nvs
            .get_blob(NVS_NAMESPACE, crate::boot_guard::KEY_OK);
        (b.len() == 16 && &b[..4] == b"VDOK").then(|| {
            let mut id = [0u8; 8];
            id.copy_from_slice(&b[4..12]);
            id
        })
    }

    /// NVS `otaStm` is set.
    pub(super) fn ota_stm(&self) -> bool {
        self.dev.nvs.has(NVS_NAMESPACE, KEY_STM)
    }
}

/// A firmware image of `n` bytes (first byte 0xE9).
pub(super) fn image(n: usize) -> Vec<u8> {
    let mut v: Vec<u8> = (0..n).map(|i| (i * 7 % 251) as u8).collect();
    if let Some(b) = v.first_mut() {
        *b = 0xE9;
    }
    v
}

/// The NVS of a namespace read through the port (as the boot guard sees it).
pub(super) fn nvs_u8(nvs: &FakeNvs, key: &str) -> Option<i64> {
    nvs.open(NVS_NAMESPACE, false)?.get_int(key, NvsInt::U8)
}
