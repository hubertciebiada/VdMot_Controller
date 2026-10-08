//! Application wiring (C++ `app.cpp`, `app.h`, `main.cpp`; DESIGN.md section 3, binding): the
//! task table, the boot order, the app thread (config fan-out; every second net, the web server
//! start, OTA, the factory latch, stm_service and the heap guard; every 10 s the heap and stack
//! alarms; every pass the log sinks, storage and the restart path), /api/health and the one RTC
//! layout of the firmware.
//!
//! [`App`] holds the logic. Its calls into the other glue modules go through [`AppHost`] (setup
//! and the app thread) and [`ThreadBegin`] (the begins of the modules that run on their own
//! threads), one method per C++ call. [`wiring`] connects the real modules: every
//! `<Module>Host` of the glue over the shared objects of [`crate::shared`], and the firmware's
//! [`AppHost`].
//!
//! The firmware's `main` (GLUE-DESIGN-ESP.md 6.4): NRST released first, on the bare pins
//! ([`crate::stm_link::release_stm_reset`]), NVS, the boot guard, the shared objects and the boot
//! deadline, the modules of [`wiring`], [`App::setup`], then [`spawn_tasks`]; `main` returns and
//! its stack is freed.

pub mod wiring;

use vdm_esp_core::common::{elapsed_ms, NO_VALVE};
use vdm_esp_core::config::Config;
use vdm_esp_core::event_log::{
    event_default_severity, make_event, Event, EventCode, RebootReason, Severity,
};
use vdm_esp_core::factory_reset::{
    factory_pin_at_boot, factory_pin_runtime_clear, FactoryPinDecision, PinHold, PinHoldState,
};
use vdm_esp_core::json_api::{
    HealthSnapshot, LogHealthInfo, NetHealthInfo, OtaHealthInfo, TaskStackInfo,
};
use vdm_esp_core::legacy_import::ImportReport;
use vdm_esp_core::link_policy::LinkState;
use vdm_esp_core::sys_health::{is_abnormal_reset, HeapGuard, ResourceMonitor};
use vdm_esp_core::version::firmware_version;

use crate::board::{FACTORY_PIN_SETTLE_MS, FACTORY_RESET_HOLD_MS, FACTORY_RESET_SAMPLE_MS};
use crate::heap::{boot_block, try_block, Block};
use crate::port::{Clock, InputPin, Platform, Rtc, System, Watchdog};
use crate::shared::{AppShared, MONITORED_TASKS};
use crate::storage::LoadDetails;

// ---------------------------------------------------------------- tasks

/// One thread of the firmware (C++ `app::TaskSpec`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskSpec {
    /// The FreeRTOS task name (ThreadSpawnConfiguration::name).
    pub name: &'static str,
    pub stack_bytes: u32,
    pub priority: u8,
    pub core: u8,
}

/// The stm thread: UART, NRST, the STM session (D§3). The stack sizes are the peaks measured in
/// QEMU plus about 25 %, not below the C++ size plus about 25 % (GLUE-DESIGN-ESP.md 2.1; the
/// device campaign checks them): here 5,984 B in QEMU, C++ 6656 B.
pub const STM_TASK: TaskSpec = TaskSpec {
    name: "stm",
    stack_bytes: 8192,
    priority: 5,
    core: 1,
};
/// The app thread (8,000 B in QEMU; C++ 7168 B).
pub const APP_TASK: TaskSpec = TaskSpec {
    name: "app",
    stack_bytes: 10_240,
    priority: 3,
    core: 1,
};
/// The mqtt thread (12,344 B in QEMU with a discovery run, whose context passes this stack
/// once; C++ 7168 B).
pub const MQTT_TASK: TaskSpec = TaskSpec {
    name: "mqtt",
    stack_bytes: 16_384,
    priority: 2,
    core: 1,
};
/// The binding task table in spawn order (DESIGN.md section 3, GLUE-DESIGN-ESP.md 2.1).
pub const TASKS: [TaskSpec; 3] = [STM_TASK, APP_TASK, MQTT_TASK];
/// Stack of the esp_http_server task that runs every web handler (11,840 B in QEMU; `async_tcp`
/// had 8960 B); the firmware's server configuration takes it.
pub const HTTPD_STACK_BYTES: u32 = 15_360;
/// Task watchdog: every thread of [`TASKS`] subscribes and feeds it at least this often; the
/// ESP panics (and reboots, reason TASK_WDT) otherwise.
pub const TASK_WDT_TIMEOUT_S: u32 = 30;

/// Tasks whose stacks /api/health and the StackLow alarm watch (name, stack bytes): ours, then
/// the HTTP server, the ESP-IDF event loop and lwIP (GLUE-DESIGN-ESP.md 2.1: `httpd` in place
/// of `async_tcp`, no `arduino_events`). Ours are listed once they exist, the library tasks
/// once a resource sample found them. A `static` (in flash): a `const` would be copied onto the
/// stack where it is iterated.
pub static MONITORED: [(&str, u32); MONITORED_TASKS] = [
    (STM_TASK.name, STM_TASK.stack_bytes),
    (APP_TASK.name, APP_TASK.stack_bytes),
    (MQTT_TASK.name, MQTT_TASK.stack_bytes),
    ("httpd", HTTPD_STACK_BYTES),
    // CONFIG_ESP_SYSTEM_EVENT_TASK_STACK_SIZE 2560 + 512
    ("sys_evt", 3072),
    // CONFIG_LWIP_TCPIP_TASK_STACK_SIZE 2560 + 512
    ("tiT", 3072),
];
/// The first entries of [`MONITORED`] are the tasks of [`TASKS`].
const OWN_TASKS: usize = TASKS.len();

/// One pass of the app thread every 100 ms.
pub const PASS_DELAY_MS: u32 = 100;
/// The once-a-second work.
pub const SECOND_MS: u32 = 1000;
/// The heap and stack alarms.
pub const RESOURCES_MS: u32 = 10_000;
/// The restart of the heap guard waits this long (the common restart path follows).
pub const HEAP_GUARD_RESTART_DELAY_MS: u32 = 1000;

// ---------------------------------------------------------------- RTC layout

/// The RTC layout of the firmware (GLUE-DESIGN-ESP.md 6.1): one record after the other in the
/// `.rtc_noinit` block, each with its own check, so garbage after a power-on (and the layout of
/// the C++ image) reads as absent. The boot guard's mirror comes first (its offset is fixed in
/// `boot_guard`).
pub const RTC_BOOT_GUARD: usize = crate::boot_guard::MIRROR_OFFSET;
/// The network watchdog's restart count ("VNWD"), [`crate::net::RTC_RECORD_LEN`] bytes.
pub const RTC_NET_WATCHDOG: usize = RTC_BOOT_GUARD + crate::boot_guard::MIRROR_LEN;
/// The desired targets of stm_service, [`crate::stm_service::RTC_TARGETS_LEN`] bytes.
pub const RTC_TARGETS: usize = RTC_NET_WATCHDOG + crate::net::RTC_RECORD_LEN;
/// The ESP failsafe emulation of stm_service, [`crate::stm_service::RTC_LEASE_LEN`] bytes.
pub const RTC_LEASE: usize = RTC_TARGETS + crate::stm_service::RTC_TARGETS_LEN;
/// The last Home Assistant status of mqtt_client, [`crate::mqtt_client::HA_STATUS_RECORD_LEN`]
/// bytes.
pub const RTC_HA_STATUS: usize = RTC_LEASE + crate::stm_service::RTC_LEASE_LEN;
/// Bytes of the RTC block the firmware reserves.
pub const RTC_LEN: usize = RTC_HA_STATUS + crate::mqtt_client::HA_STATUS_RECORD_LEN;
/// The two records of stm_service.
pub const RTC_STM_SERVICE: crate::stm_service::RtcRecords = crate::stm_service::RtcRecords {
    targets: RTC_TARGETS,
    lease: RTC_LEASE,
};

// ---------------------------------------------------------------- hosts

/// What setup and the app thread call in the other glue modules (C++ `storage::`, `logger::`,
/// `stm_service::`, `net::`, `ota::`, `mqtt::` and `web::`), one method per C++ call. The
/// firmware's host is [`wiring::Modules`]; the app thread owns it.
pub trait AppHost {
    // ---- setup, in boot order
    /// C++ `storage::factoryLatched()`.
    fn factory_latched(&mut self) -> bool;
    /// C++ `storage::beginFs(formatted)`: LittleFS mounted (formatted only when the mount
    /// failed); false when even the format failed.
    fn begin_fs(&mut self, formatted: &mut bool) -> bool;
    /// C++ `storage::factoryReset()`.
    fn factory_reset(&mut self) -> bool;
    /// C++ `storage::setFactoryLatched(on)`.
    fn set_factory_latched(&mut self, on: bool);
    /// C++ `storage::loadConfig(cfg, report, details)`.
    fn load_config(
        &mut self,
        out: &mut Config,
        report: &mut ImportReport,
        details: &mut LoadDetails,
    );
    /// C++ `storage::setActiveConfig(c)`.
    fn set_active_config(&mut self, c: &Config);
    /// C++ `storage::configRevision()`.
    fn config_revision(&mut self) -> u32;
    /// C++ `storage::incrementBootCount()`.
    fn increment_boot_count(&mut self) -> u32;
    /// C++ `logger::log(e)`.
    fn log(&mut self, e: &Event);
    /// C++ `logger::configure(level, server, port, persist, hostname)`.
    fn logger_configure(
        &mut self,
        syslog_level: u8,
        syslog_server: u32,
        syslog_port: u16,
        persist: bool,
        hostname: &[u8],
    );
    /// C++ `stm_service::begin()`.
    fn stm_service_begin(&mut self);
    /// C++ `net::begin(cfg)`: may put the previous network settings back into `cfg` (an
    /// interrupted network trial).
    fn net_begin(&mut self, cfg: &mut Config);
    /// C++ `ota::begin()`.
    fn ota_begin(&mut self);
    // ---- the app thread
    /// C++ `storage::getConfig(out)`.
    fn get_config(&mut self, out: &mut Config);
    /// C++ `net::reconfigure(cfg)`.
    fn net_reconfigure(&mut self, cfg: &Config);
    /// C++ `net::service(now, mqttConnected)`.
    fn net_service(&mut self, now_ms: u32, mqtt_connected: bool);
    /// C++ `mqtt::status().state == MqttState::Connected`.
    fn mqtt_connected(&mut self) -> bool;
    /// C++ `net::isUp()`.
    fn net_is_up(&mut self) -> bool;
    /// C++ `net::otaNetOk()`.
    fn net_ota_ok(&mut self) -> bool;
    /// C++ `web::started()`.
    fn web_started(&mut self) -> bool;
    /// C++ `web::begin()`.
    fn web_begin(&mut self);
    /// C++ `ota::service(now, netOk, linkUp, webStarted)`.
    fn ota_service(&mut self, now_ms: u32, net_ok: bool, link_up: bool, web_started: bool);
    /// C++ `stm_service::service(now)`.
    fn stm_service_service(&mut self, now_ms: u32);
    /// C++ `ota::uploadActive()`.
    fn ota_upload_active(&mut self) -> bool;
    /// C++ `ota::restartPending()`.
    fn ota_restart_pending(&mut self) -> bool;
    /// C++ `ota::requestRestart(reason, delayMs)` (its RebootRequested event is logged).
    fn ota_request_restart(&mut self, reason: u8, delay_ms: u32);
    /// C++ `logger::service(netUp)`.
    fn logger_service(&mut self, net_up: bool);
    /// C++ `storage::service()`.
    fn storage_service(&mut self);
    /// C++ `ota::serviceRestart(now, netUp, linkUp)`.
    fn ota_service_restart(&mut self, now_ms: u32, net_up: bool, link_up: bool);
}

/// The begins of the modules that run on threads of their own (C++ `stm_link::begin`,
/// `mqtt::begin`), called by [`App::setup`] before the threads exist.
pub trait ThreadBegin {
    /// NRST released, Serial2 opened (first in setup: the IO15 pull-up may hold the STM).
    fn stm_link_begin(&mut self);
    /// The MQTT task's boot (its RTC record, the config, the regulator state).
    fn mqtt_begin(&mut self);
}

// ---------------------------------------------------------------- threads

/// A glue task: the body of one thread (GLUE-DESIGN-ESP.md 2.1).
pub trait Task<W> {
    /// The start on the thread, with the watchdog subscription of that thread.
    fn start(&mut self, watchdog: W);
    /// One loop pass (it feeds the watchdog); returns the delay before the next one in ms.
    fn pass(&mut self) -> u32;
}

/// The body of every thread: [`Task::start`], then a pass and its delay, forever.
pub fn run_task<W, T: Task<W>>(task: &mut T, watchdog: W, clock: &impl Clock) -> ! {
    task.start(watchdog);
    loop {
        let d = task.pass();
        clock.sleep_ms(d);
    }
}

/// What starts the threads: the firmware's spawner (ThreadSpawnConfiguration with the spec's
/// name, priority and core, std::thread::Builder with its stack size; the TWDT subscription made
/// on the new thread).
pub trait Spawner<'a, W> {
    /// Starts `body` as the thread `spec`, giving it the watchdog subscription of that thread.
    /// A thread that cannot be created aborts: without its threads the firmware cannot work.
    fn spawn(&mut self, spec: &TaskSpec, body: Box<dyn FnOnce(W) + Send + 'a>);
}

/// The last step of the boot (C++ `startTask` of setup): the threads of [`TASKS`] in their
/// order, each running its task with [`run_task`]. The watchdog adapter must be `Send` (the
/// tasks keep it).
pub fn spawn_tasks<'a, W, C, S, A, M>(
    spawner: &mut impl Spawner<'a, W>,
    clock: &'a C,
    mut stm: S,
    mut app: A,
    mut mqtt: M,
) where
    W: 'a,
    C: Clock,
    S: Task<W> + Send + 'a,
    A: Task<W> + Send + 'a,
    M: Task<W> + Send + 'a,
{
    spawner.spawn(
        &STM_TASK,
        Box::new(move |w| {
            run_task(&mut stm, w, clock);
        }),
    );
    spawner.spawn(
        &APP_TASK,
        Box::new(move |w| {
            run_task(&mut app, w, clock);
        }),
    );
    spawner.spawn(
        &MQTT_TASK,
        Box::new(move |w| {
            run_task(&mut mqtt, w, clock);
        }),
    );
}

// ---------------------------------------------------------------- boot deadline

/// The boot deadline (GLUE-DESIGN-ESP.md 6.3, 6.4): `main` arms a one-shot timer of
/// [`crate::boot_guard::BOOT_DEADLINE_MS`] at its start and calls this when it fires. An image
/// that hangs in its boot (`setup` never reached the first pass of the app thread) restarts, a
/// boot the boot guard counts (on trial) or an abnormal end of a confirmed image's boot (F5: the
/// mirror in `rtc` says so); once the app thread runs, nothing happens.
pub fn boot_deadline(shared: &AppShared, system: &impl System, rtc: &impl Rtc) {
    if !shared.app_running() {
        crate::boot_guard::note_deadline_restart(rtc);
        system.restart();
    }
}

// ---------------------------------------------------------------- health

/// The parts of /api/health other modules publish (C++ `net::health`, `ota::health`,
/// `logger::stats`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HealthParts {
    pub net: NetHealthInfo,
    pub ota: OtaHealthInfo,
    pub log: LogHealthInfo,
}

/// Everything GET /api/health shows (C++ `app::readHealth`): version, uptime, the 8-bit heap
/// (`ESP.getFreeHeap()` counted ~45 KB of IRAM that only 32-bit accesses reach), the smallest
/// largest block of the resource samples, the watched stacks (ours once they exist, the library
/// tasks once a resource sample found them), network, OTA and log.
pub fn read_health<'s>(
    clock: &impl Clock,
    system: &impl System,
    shared: &AppShared,
    parts: HealthParts,
    out: &mut HealthSnapshot<'s>,
) {
    // every field of `out` is written: the C++ `out = HealthSnapshot{}` without a second copy
    let heap = system.heap();
    out.version = firmware_version().as_bytes();
    out.uptime_s = clock.uptime_s();
    out.free_heap = heap.free;
    out.min_free_heap = heap.min_free;
    out.largest_free_block = heap.largest;
    out.min_largest_free_block = shared.min_largest();
    out.net = parts.net;
    out.ota = parts.ota;
    out.log = parts.log;
    out.task_count = 0;
    let listed = MONITORED
        .iter()
        .enumerate()
        .filter(|(i, _)| *i < OWN_TASKS || shared.task_found(*i))
        .filter_map(|(_, &(name, stack))| {
            system.stack_min_free(name).map(|free| TaskStackInfo {
                name: name.as_bytes(),
                stack_bytes: stack,
                min_free_bytes: free,
            })
        });
    let mut listed = listed.fuse();
    for slot in out.tasks.iter_mut() {
        *slot = match listed.next() {
            Some(t) => {
                out.task_count += 1;
                t
            }
            None => TaskStackInfo::default(),
        };
    }
}

// ---------------------------------------------------------------- the app

/// The ports of the app thread besides its host: the factory pin (IO2, pull-up) is its own.
pub struct AppPorts<'a, P: Platform> {
    pub clock: &'a P::Clock,
    pub system: &'a P::System,
    pub heap: &'a P::HeapGate,
    pub factory_pin: P::InputPin,
}

/// setup and the app thread (C++ `app::setup`, `appTask` and their statics).
pub struct App<'a, P: Platform, H: AppHost> {
    clock: &'a P::Clock,
    system: &'a P::System,
    heap: &'a P::HeapGate,
    factory_pin: P::InputPin,
    shared: &'a AppShared,
    host: H,
    resources: ResourceMonitor,
    heap_guard: HeapGuard,
    /// A factory reset happened and the pin was not seen HIGH since (app thread after setup).
    factory_latched: bool,
    /// The config the app thread's modules follow.
    cfg_revision: u32,
    last_second_ms: u32,
    last_resources_ms: u32,
    watchdog: Option<P::Watchdog>,
}

impl<'a, P: Platform, H: AppHost> App<'a, P, H> {
    /// Nothing done yet: [`App::setup`] follows.
    pub fn new(ports: AppPorts<'a, P>, shared: &'a AppShared, host: H) -> Self {
        App {
            clock: ports.clock,
            system: ports.system,
            heap: ports.heap,
            factory_pin: ports.factory_pin,
            shared,
            host,
            resources: ResourceMonitor::default(),
            heap_guard: HeapGuard::default(),
            factory_latched: false,
            cfg_revision: 0,
            last_second_ms: 0,
            last_resources_ms: 0,
            watchdog: None,
        }
    }

    /// The host (the firmware's modules of the app thread).
    pub fn host(&self) -> &H {
        &self.host
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

    /// The factory pin at boot: its first sample after the pull-up settled and, only when it is
    /// LOW and no latch is set, whether it stays LOW for [`FACTORY_RESET_HOLD_MS`] (sampled
    /// every [`FACTORY_RESET_SAMPLE_MS`]).
    fn check_factory_pin(&mut self, latched: bool) -> FactoryPinDecision {
        self.clock.sleep_ms(FACTORY_PIN_SETTLE_MS);
        let low = self.factory_pin.is_low();
        let mut held = false;
        if low && !latched {
            let mut hold = PinHold::new(FACTORY_RESET_HOLD_MS);
            hold.begin(self.clock.now_ms());
            let mut st = PinHoldState::Holding;
            while st == PinHoldState::Holding {
                self.clock.sleep_ms(FACTORY_RESET_SAMPLE_MS);
                st = hold.sample(self.factory_pin.is_low(), self.clock.now_ms());
            }
            held = st == PinHoldState::Held;
        }
        factory_pin_at_boot(low, held, latched)
    }

    /// The boot order (DESIGN.md section 3): NRST released, the factory pin, LittleFS, the
    /// config (backup, repair, import), the `boot` event, the log sinks configured, stm_service
    /// (boot targets: RTC, else NVS), net, the OTA state, MQTT. Never returns early: a failing
    /// subsystem is logged and the rest keeps running. The logger's ring and file timer exist
    /// before (`main` creates them); the threads follow ([`spawn_tasks`]).
    pub fn setup(&mut self, threads: &mut impl ThreadBegin) {
        // R6: the STM released from reset first thing (the IO15 pull-up may hold it)
        threads.stm_link_begin();
        // GPIO2 jumper: once per fitting (latch in NVS), not at every restart
        let latched = self.host.factory_latched();
        let pin = self.check_factory_pin(latched);
        let mut formatted = false;
        let fs_ok = self.host.begin_fs(&mut formatted);
        let factory_reset = pin == FactoryPinDecision::Reset;
        let reset_ok = factory_reset && self.host.factory_reset();
        // the latch records a reset done: after a failed one the next boot with the jumper still
        // fitted tries again
        if reset_ok {
            self.host.set_factory_latched(true);
        }
        if pin == FactoryPinDecision::ClearLatch {
            self.host.set_factory_latched(false);
        }
        self.factory_latched = reset_ok || pin == FactoryPinDecision::KeepLatched;
        let mut cfg = self.load_boot_config();

        let boots = self.host.increment_boot_count();
        let reason = self.system.reset_reason();
        let sev = if is_abnormal_reset(i32::from(reason)) {
            Severity::Warning
        } else {
            Severity::Info
        };
        let boot = make_event(
            EventCode::Boot,
            sev,
            NO_VALVE,
            i32::from(reason),
            boots as i32,
            firmware_version().as_bytes(),
        );
        self.host.log(&boot);
        if formatted {
            self.log(EventCode::FsFormatted, 0, 0, b"");
        }
        if !fs_ok {
            self.log(EventCode::FsFormatted, -1, 0, b"");
        }
        if factory_reset {
            let result = if reset_ok { 0 } else { -1 };
            self.log(
                EventCode::ConfigSaved,
                self.cfg_revision as i32,
                result,
                b"factory",
            );
        }
        if pin == FactoryPinDecision::KeepLatched {
            self.log(EventCode::FactoryResetSkipped, 0, 0, b"");
        }
        let s = &cfg.syslog;
        self.host
            .logger_configure(s.level, s.server, s.port, cfg.persist_log, &cfg.station);

        self.host.stm_service_begin();
        self.host.net_begin(&mut cfg);
        drop(cfg);
        self.host.ota_begin();
        threads.mqtt_begin();
    }

    /// The boot config (backup, repair, import), a boot block until the modules of setup kept
    /// what they need; published as the active config.
    fn load_boot_config(&mut self) -> Block<Config> {
        let mut cfg = boot_block(Config::default);
        let mut report = ImportReport::default();
        let mut details = LoadDetails::default();
        self.host.load_config(&mut cfg, &mut report, &mut details);
        self.host.set_active_config(&cfg);
        self.cfg_revision = self.host.config_revision();
        cfg
    }

    /// Live effects of a config change (DESIGN.md section 7, apply semantics): the log sinks and
    /// the network. The stm and mqtt threads follow the revision themselves. The copy (2.5 KB)
    /// lives on the heap only for this call: net may store and apply a config meanwhile, which
    /// takes the config lock. Without memory for it the next pass applies the change.
    fn apply_config_change(&mut self) {
        let Some(mut cfg) = try_block(self.heap, Config::default) else {
            return;
        };
        self.cfg_revision = self.host.config_revision();
        self.host.get_config(&mut cfg);
        let s = &cfg.syslog;
        self.host
            .logger_configure(s.level, s.server, s.port, cfg.persist_log, &cfg.station);
        self.host.net_reconfigure(&cfg);
    }

    /// Every [`RESOURCES_MS`]: the heap, fragmentation and stack alarms (core
    /// [`ResourceMonitor`]); a library task is looked up by name until it exists.
    fn sample_resources(&mut self, now_ms: u32) {
        let h = self.system.heap();
        let mut ev: [Event; 2] = Default::default();
        let n = self
            .resources
            .on_heap(h.free, h.min_free, h.largest, now_ms, &mut ev);
        for e in ev.iter().take(n) {
            self.host.log(e);
        }
        for (i, &(name, stack)) in (0u8..).zip(MONITORED.iter()) {
            let Some(free) = self.system.stack_min_free(name) else {
                continue;
            };
            self.shared.mark_task_found(usize::from(i));
            let mut one: [Event; 1] = Default::default();
            if self
                .resources
                .on_stack(i, name.as_bytes(), stack, free, &mut one)
                != 0
            {
                self.host.log(&one[0]);
            }
        }
        self.shared
            .set_min_largest(self.resources.min_largest_block());
    }

    /// Every second: the heap guard (core [`HeapGuard`]). The restart takes the common path
    /// (desired targets, STM EEPROM wait, log flush); the event is logged before it, so that
    /// flush writes it too.
    fn check_heap_guard(&mut self, now_ms: u32) {
        let free = self.system.heap().free;
        let blocked = self.host.ota_upload_active()
            || self.shared.stm_flash_active()
            || self.host.ota_restart_pending();
        if !self.heap_guard.on_sample(free, blocked, now_ms) {
            return;
        }
        let largest = self.system.heap().largest;
        self.log(EventCode::HeapCritical, free as i32, largest as i32, b"");
        self.host
            .ota_request_restart(RebootReason::HeapGuard as u8, HEAP_GUARD_RESTART_DELAY_MS);
    }

    /// Every second while latched: the jumper was removed, so the next fitting resets again.
    fn check_factory_latch(&mut self) {
        let low = self.factory_pin.is_low();
        if !factory_pin_runtime_clear(low, self.factory_latched) {
            return;
        }
        self.factory_latched = false;
        self.host.set_factory_latched(false);
    }
}

impl<'a, P: Platform, H: AppHost> Task<P::Watchdog> for App<'a, P, H> {
    /// The app thread starts: its watchdog, the once-a-second work due at 1000 ms of uptime, the
    /// resource sample 10 s after the start.
    fn start(&mut self, watchdog: P::Watchdog) {
        self.watchdog = Some(watchdog);
        self.last_second_ms = 0;
        self.last_resources_ms = self.clock.now_ms();
    }

    /// One pass of the app thread (C++ `appTask`): a new config revision; every second net,
    /// the web server start once the network is up, OTA validation, the factory latch,
    /// stm_service and the heap guard; every 10 s the resources; then the log sinks, storage and
    /// the restart path. Returns [`PASS_DELAY_MS`].
    fn pass(&mut self) -> u32 {
        if let Some(w) = &self.watchdog {
            w.feed();
        }
        // the boot finished: the boot deadline is disarmed
        self.shared.mark_app_running();
        let now = self.clock.now_ms();
        if self.host.config_revision() != self.cfg_revision {
            self.apply_config_change();
        }
        let link_up = self.shared.stm_link_state() == LinkState::Up;
        if elapsed_ms(now, self.last_second_ms) >= SECOND_MS {
            self.last_second_ms = now;
            let mqtt = self.host.mqtt_connected();
            self.host.net_service(now, mqtt);
            if self.host.net_is_up() && !self.host.web_started() {
                self.host.web_begin();
            }
            let net_ok = self.host.net_ota_ok();
            let web = self.host.web_started();
            self.host.ota_service(now, net_ok, link_up, web);
            self.check_factory_latch();
            self.host.stm_service_service(now);
            self.check_heap_guard(now);
        }
        if elapsed_ms(now, self.last_resources_ms) >= RESOURCES_MS {
            self.last_resources_ms = now;
            self.sample_resources(now);
        }
        let up = self.host.net_is_up();
        self.host.logger_service(up);
        self.host.storage_service();
        let up = self.host.net_is_up();
        self.host.ota_service_restart(now, up, link_up);
        PASS_DELAY_MS
    }
}

#[cfg(test)]
mod rig;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
