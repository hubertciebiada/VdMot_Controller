//! The rig of the app tests (C++ `glue_test.h` with the sibling fakes of storage, logger,
//! stm_service, net, ota, mqtt, web and stm_link): a booted device, the fake host noting its
//! calls into the device's journal (C++ `fakes::journal`), the app over the device's ports and
//! the task driver (C++ `runAppTask`).
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames)]

use std::sync::{Arc, Mutex, MutexGuard};

use super::*;
use crate::testkit::board::TestPlatform;
use crate::testkit::uart::FakeInputPin;
use crate::testkit::{lock as tlock, run, Device, Ended, FakeBoard, Journal};
use vdm_esp_core::event_log::event_code_name;

type OnBegin = Box<dyn FnMut(&mut Config) + Send>;

/// The sibling fakes (C++ `sib::*`).
pub(super) struct HostState {
    // storage
    pub begin_fs_result: bool,
    pub formatted: bool,
    pub factory_reset_result: bool,
    pub factory_latched: bool,
    pub boot_count: u32,
    pub revision: u32,
    /// getConfig() (C++ `sib::storage().active`)
    pub active: Box<Config>,
    /// loadConfig() output (C++ `loadedConfig`)
    pub loaded: Box<Config>,
    pub factory_resets: u32,
    pub factory_latch_sets: Vec<bool>,
    pub storage_services: u32,
    // logger
    pub events: Vec<Event>,
    /// level, server, port, persist, hostname
    pub configures: Vec<(u8, u32, u16, bool, Vec<u8>)>,
    pub logger_services: Vec<bool>,
    // stm_service
    pub stm_service_begins: u32,
    pub stm_service_services: Vec<u32>,
    // net
    pub net_on_begin: Option<OnBegin>,
    pub net_begun_with: Option<Box<Config>>,
    pub net_services: Vec<(u32, bool)>,
    pub net_up: bool,
    pub ota_net_ok: bool,
    /// the station names of the reconfigures
    pub reconfigures: Vec<Vec<u8>>,
    // ota
    pub ota_begins: u32,
    /// now, net ok, link up, web started
    pub ota_services: Vec<(u32, bool, bool, bool)>,
    /// now, net up, link up
    pub service_restarts: Vec<(u32, bool, bool)>,
    pub upload_active: bool,
    pub restart_pending: bool,
    /// reason, delay
    pub restart_requests: Vec<(u8, u32)>,
    // mqtt
    pub mqtt_connected: bool,
    // web
    pub web_started: bool,
    pub web_begins: u32,
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            begin_fs_result: true,
            formatted: false,
            factory_reset_result: true,
            factory_latched: false,
            boot_count: 0,
            revision: 0,
            active: Box::default(),
            loaded: Box::default(),
            factory_resets: 0,
            factory_latch_sets: Vec::new(),
            storage_services: 0,
            events: Vec::new(),
            configures: Vec::new(),
            logger_services: Vec::new(),
            stm_service_begins: 0,
            stm_service_services: Vec::new(),
            net_on_begin: None,
            net_begun_with: None,
            net_services: Vec::new(),
            net_up: false,
            ota_net_ok: false,
            reconfigures: Vec::new(),
            ota_begins: 0,
            ota_services: Vec::new(),
            service_restarts: Vec::new(),
            upload_active: false,
            restart_pending: false,
            restart_requests: Vec::new(),
            mqtt_connected: false,
            web_started: false,
            web_begins: 0,
        }
    }
}

/// The host of the tests; clones share the state and note into the journal.
#[derive(Clone)]
pub(super) struct FakeHost {
    state: Arc<Mutex<HostState>>,
    journal: Journal,
}

impl FakeHost {
    pub fn new(journal: Journal) -> Self {
        FakeHost {
            state: Arc::default(),
            journal,
        }
    }
    pub fn s(&self) -> MutexGuard<'_, HostState> {
        tlock(&self.state)
    }
    fn note(&self, entry: impl Into<String>) {
        self.journal.note(entry);
    }
    pub fn with_code(&self, code: EventCode) -> Vec<Event> {
        self.s()
            .events
            .iter()
            .filter(|e| e.code == code)
            .cloned()
            .collect()
    }
    pub fn has(&self, code: EventCode) -> bool {
        !self.with_code(code).is_empty()
    }
    pub fn first(&self, code: EventCode) -> Event {
        match self.with_code(code).first() {
            Some(e) => e.clone(),
            None => panic!("no {code:?} event"),
        }
    }
}

impl AppHost for FakeHost {
    fn factory_latched(&mut self) -> bool {
        self.s().factory_latched
    }
    fn begin_fs(&mut self, formatted: &mut bool) -> bool {
        self.note("storage.beginFs");
        let s = self.s();
        *formatted = s.formatted;
        s.begin_fs_result
    }
    fn factory_reset(&mut self) -> bool {
        self.note("storage.factoryReset");
        let mut s = self.s();
        s.factory_resets += 1;
        s.factory_reset_result
    }
    fn set_factory_latched(&mut self, on: bool) {
        let mut s = self.s();
        s.factory_latch_sets.push(on);
        s.factory_latched = on;
    }
    fn load_config(
        &mut self,
        out: &mut Config,
        _report: &mut ImportReport,
        _details: &mut LoadDetails,
    ) {
        self.note("storage.loadConfig");
        out.clone_from(&self.s().loaded);
    }
    fn set_active_config(&mut self, c: &Config) {
        self.note("storage.setActiveConfig");
        let mut s = self.s();
        Config::clone_from(&mut s.active, c);
        s.revision += 1;
    }
    fn config_revision(&mut self) -> u32 {
        self.s().revision
    }
    fn increment_boot_count(&mut self) -> u32 {
        let mut s = self.s();
        s.boot_count += 1;
        s.boot_count
    }
    fn log(&mut self, e: &Event) {
        self.note(format!("logger.log {}", event_code_name(e.code)));
        self.s().events.push(e.clone());
    }
    fn logger_configure(&mut self, level: u8, server: u32, port: u16, persist: bool, host: &[u8]) {
        self.note("logger.configure");
        self.s()
            .configures
            .push((level, server, port, persist, host.to_vec()));
    }
    fn stm_service_begin(&mut self) {
        self.note("stm_service.begin");
        self.s().stm_service_begins += 1;
    }
    fn net_begin(&mut self, cfg: &mut Config) {
        self.note("net.begin");
        let hook = self.s().net_on_begin.take();
        if let Some(mut f) = hook {
            f(cfg);
        }
        self.s().net_begun_with = Some(Box::new(cfg.clone()));
    }
    fn ota_begin(&mut self) {
        self.note("ota.begin");
        self.s().ota_begins += 1;
    }
    fn get_config(&mut self, out: &mut Config) {
        out.clone_from(&self.s().active);
    }
    fn net_reconfigure(&mut self, cfg: &Config) {
        self.note("net.reconfigure");
        self.s().reconfigures.push(cfg.station.to_vec());
    }
    fn net_service(&mut self, now_ms: u32, mqtt_connected: bool) {
        self.note(format!("net.service {now_ms}"));
        self.s().net_services.push((now_ms, mqtt_connected));
    }
    fn mqtt_connected(&mut self) -> bool {
        self.s().mqtt_connected
    }
    fn net_is_up(&mut self) -> bool {
        self.s().net_up
    }
    fn net_ota_ok(&mut self) -> bool {
        self.s().ota_net_ok
    }
    fn web_started(&mut self) -> bool {
        self.s().web_started
    }
    fn web_begin(&mut self) {
        self.note("web.begin");
        let mut s = self.s();
        s.web_begins += 1;
        s.web_started = true;
    }
    fn ota_service(&mut self, now_ms: u32, net_ok: bool, link_up: bool, web_started: bool) {
        self.note(format!("ota.service {now_ms}"));
        self.s()
            .ota_services
            .push((now_ms, net_ok, link_up, web_started));
    }
    fn stm_service_service(&mut self, now_ms: u32) {
        self.s().stm_service_services.push(now_ms);
    }
    fn ota_upload_active(&mut self) -> bool {
        self.s().upload_active
    }
    fn ota_restart_pending(&mut self) -> bool {
        self.s().restart_pending
    }
    fn ota_request_restart(&mut self, reason: u8, delay_ms: u32) {
        self.note(format!("ota.requestRestart {reason} {delay_ms}"));
        let mut s = self.s();
        s.restart_requests.push((reason, delay_ms));
        s.restart_pending = true;
    }
    fn logger_service(&mut self, net_up: bool) {
        self.s().logger_services.push(net_up);
    }
    fn storage_service(&mut self) {
        self.s().storage_services += 1;
    }
    fn ota_service_restart(&mut self, now_ms: u32, net_up: bool, link_up: bool) {
        self.s().service_restarts.push((now_ms, net_up, link_up));
    }
}

/// The begins of the thread modules (C++ `sib::stmLink`, `sib::mqtt`).
pub(super) struct FakeThreads(pub Journal);

impl ThreadBegin for FakeThreads {
    fn stm_link_begin(&mut self) {
        self.0.note("stm_link.begin");
    }
    fn mqtt_begin(&mut self) {
        self.0.note("mqtt.begin");
    }
}

pub(super) type TestApp<'a> = App<'a, TestPlatform, FakeHost>;

/// One boot: the device, the shared app data and the host.
pub(super) struct Rig {
    pub dev: Device,
    pub shared: AppShared,
    pub host: FakeHost,
}

impl Rig {
    pub fn new() -> Rig {
        let dev = FakeBoard::new().boot();
        let host = FakeHost::new(dev.journal.clone());
        Rig {
            dev,
            shared: AppShared::new(),
            host,
        }
    }

    /// The factory pin (IO2).
    pub fn factory_pin(&self) -> FakeInputPin {
        self.dev.gpio.input(2)
    }

    /// The app over the device's ports.
    pub fn app(&self) -> TestApp<'_> {
        let ports = AppPorts {
            clock: &self.dev.clock,
            system: &self.dev.system,
            heap: &self.dev.heap,
            factory_pin: self.factory_pin(),
        };
        App::new(ports, &self.shared, self.host.clone())
    }

    /// `app::setup()` with the fake thread modules.
    pub fn setup(&self, app: &mut TestApp<'_>) {
        app.setup(&mut FakeThreads(self.dev.journal.clone()));
    }

    /// C++ `runAppTask(passes)`: the app thread (a fresh start) for `passes` passes.
    pub fn run_app(&self, app: &mut TestApp<'_>, passes: u64) {
        self.dev.clock.stop_after_sleeps(passes);
        let wd = self.dev.watchdog.clone();
        let clock = &self.dev.clock;
        assert_eq!(
            run(|| {
                run_task(app, wd, clock);
            }),
            Ended::Stopped
        );
    }

    /// Sleeps go into the journal as "delay <ms>" (C++ `fakes::journal` of `delay()`).
    pub fn journal_sleeps(&self) {
        let j = self.dev.journal.clone();
        self.dev
            .clock
            .on_sleep(move |ms| j.note(format!("delay {ms}")));
    }

    /// The task stack figures the system reports (C++ `stackHighWater`; a task exists once it
    /// has one).
    pub fn stack(&self, task: &str, min_free: u32) {
        self.dev
            .system
            .state()
            .stacks
            .insert(task.to_string(), min_free);
    }
}
