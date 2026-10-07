//! The firmware's wiring: every `<Module>Host` of the glue over the shared objects of
//! [`Shared`], [`Storage`] and the ports, one forwarding call per C++ sibling call, and the
//! constructors `main` builds the modules with. Every object here borrows `'a`: `main` leaks the
//! long-lived ones (`&'static`), a test keeps them on its stack.
//!
//! Construction order (each borrows only what exists before it): [`Shared`], [`Ports`], the
//! logger ([`Ports::logger`]), [`storage`], [`sinks`], [`stm_service`], then the modules of the
//! threads ([`stm_link`], [`mqtt`], [`net`], [`ota`]), the web server of the HTTP thread
//! ([`web`] with [`Wire`] as its `WebHost`, its ESP upload [`web_upload`]) and the app ([`app`]
//! over [`Modules`], whose [`AppWeb`] is a [`WebBegin`] of the HTTP server port). The app
//! thread's log sinks and stm_service sit behind mutexes because the OTA restart path (inside
//! [`Modules`]) flushes them; only the app thread locks them.

use std::sync::{Mutex, MutexGuard, PoisonError};

use vdm_esp_core::calib_schedule::CalibFailure;
use vdm_esp_core::common::LocalTime;
use vdm_esp_core::config::{CalibScheduleConfig, Config};
use vdm_esp_core::event_log::{Event, EventCode, EventFilter};
use vdm_esp_core::failsafe::RegulatorInput;
use vdm_esp_core::json_api::{HealthSnapshot, MqttState};
use vdm_esp_core::lease_client::LeaseClientSnapshot;
use vdm_esp_core::legacy_import::ImportReport;
use vdm_esp_core::link_policy::LinkState;
use vdm_esp_core::stm_codec::Profile;
use vdm_esp_core::stm_types::{StmCommand, StmSaveState, StmSnapshot};
use vdm_esp_core::target_store::{PersistedTargets, RestoreSource};
use vdm_esp_core::version::StmSupport;

use super::{
    read_health, App, AppHost, AppPorts, HealthParts, Task, ThreadBegin, RTC_HA_STATUS,
    RTC_NET_WATCHDOG, RTC_STM_SERVICE,
};
use crate::boot_guard::{BootGuard, BootReport};
use crate::logger::{LogRead, LogSinks, Logger, LoggerHost};
use crate::mqtt_client::{DiscoveryAction, MqttClient, MqttHost, MqttPorts, MqttStatus};
use crate::net::{self, Net, NetHost, NetInfo, NetPorts, TrialInfo};
use crate::ota::{OtaHost, OtaPorts, OtaService, OtaUpload};
use crate::port::{Clock, Fs, HeapGate, HttpServer, Platform, Rtc, System, TcpConnector, Watchdog};
use crate::shared::{CalibInfo, Shared};
use crate::stm_link::{StmLink, StmLinkHost, StmLinkPorts};
use crate::stm_service::{StmService, StmServiceHost, StmServiceLink, StmServicePorts};
use crate::storage::{LoadDetails, LoadSource, Storage, StorageHost};
use crate::web_server::{Asset, Web, WebHost, WebPorts, WebStart};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // a panic aborts the firmware, so a poisoned lock exists only in a failing test
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------- ports, logger

/// The ports the threads share, by reference.
pub struct Ports<'a, P: Platform> {
    pub clock: &'a P::Clock,
    pub wall: &'a P::WallClock,
    pub console: &'a P::Console,
    pub nvs: &'a P::Nvs,
    pub fs: &'a P::Fs,
    pub tcp: &'a P::Tcp,
    pub ota: &'a P::Ota,
    pub system: &'a P::System,
    pub heap: &'a P::HeapGate,
    pub rtc: &'a P::Rtc,
}

impl<P: Platform> Clone for Ports<'_, P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: Platform> Copy for Ports<'_, P> {}

/// The logger of the firmware (any thread logs through it).
pub type FwLogger<'a, P> = Logger<
    'a,
    &'a <P as Platform>::Clock,
    &'a <P as Platform>::WallClock,
    &'a <P as Platform>::Console,
>;

impl<'a, P: Platform> Ports<'a, P> {
    /// A logger over the shared ring (it holds only references: every host has its own).
    pub fn logger(self, shared: &'a Shared) -> FwLogger<'a, P> {
        Logger::new(&shared.logger, self.clock, self.wall, self.console)
    }
}

// ---------------------------------------------------------------- storage, logger sinks

/// storage's host (C++ `logger::log`, `net::trialInfo`).
pub struct StorageWire<'a, P: Platform> {
    logger: FwLogger<'a, P>,
    shared: &'a Shared,
}

impl<P: Platform> StorageHost for StorageWire<'_, P> {
    fn log(&self, code: EventCode, valve: u8, arg1: i32, arg2: i32, text: &[u8]) {
        self.logger.log(code, valve, arg1, arg2, text);
    }
    fn net_trial_active(&self) -> bool {
        self.shared.net.trial_info().active
    }
}

/// The storage of the firmware.
pub type FwStorage<'a, P> = Storage<
    'a,
    &'a <P as Platform>::Nvs,
    &'a <P as Platform>::Fs,
    &'a <P as Platform>::HeapGate,
    StorageWire<'a, P>,
>;

/// The storage operations over NVS, LittleFS and the heap gate.
pub fn storage<'a, P: Platform>(ports: Ports<'a, P>, shared: &'a Shared) -> FwStorage<'a, P> {
    let host = StorageWire {
        logger: ports.logger(shared),
        shared,
    };
    Storage::new(&shared.storage, ports.nvs, ports.fs, ports.heap, host)
}

/// The log sinks' host (C++ `storage::fsReady`).
pub struct LoggerWire<'a> {
    shared: &'a Shared,
}

impl LoggerHost for LoggerWire<'_> {
    fn fs_ready(&self) -> bool {
        self.shared.storage.fs_ready()
    }
}

/// The log sinks of the firmware (the app thread).
pub type FwSinks<'a, P> = LogSinks<
    'a,
    &'a <P as Platform>::Clock,
    &'a <P as Platform>::WallClock,
    &'a <P as Platform>::Console,
    &'a <P as Platform>::Fs,
    <P as Platform>::Udp,
    LoggerWire<'a>,
>;

/// The file and syslog sinks of `logger` (C++ `logger::begin`: the flush timer starts now).
pub fn sinks<'a, P: Platform>(
    ports: Ports<'a, P>,
    logger: &'a FwLogger<'a, P>,
    shared: &'a Shared,
    udp: P::Udp,
) -> Mutex<FwSinks<'a, P>> {
    Mutex::new(LogSinks::new(logger, ports.fs, udp, LoggerWire { shared }))
}

// ---------------------------------------------------------------- the host of the modules

/// The host of stm_service, net, mqtt_client and stm_link: the shared objects, storage, the
/// ports and a logger.
pub struct Wire<'a, P: Platform> {
    ports: Ports<'a, P>,
    shared: &'a Shared,
    storage: &'a FwStorage<'a, P>,
    logger: FwLogger<'a, P>,
}

impl<P: Platform> Clone for Wire<'_, P> {
    fn clone(&self) -> Self {
        Wire::new(self.ports, self.shared, self.storage)
    }
}

impl<'a, P: Platform> Wire<'a, P> {
    /// The host over these objects.
    pub fn new(ports: Ports<'a, P>, shared: &'a Shared, storage: &'a FwStorage<'a, P>) -> Self {
        Wire {
            ports,
            shared,
            storage,
            logger: ports.logger(shared),
        }
    }

    /// stm_service's calls of the stm thread over the RTC records of the layout.
    fn stm_service_link(&self) -> StmServiceLink<'a, P::Rtc> {
        StmServiceLink::new(&self.shared.stm_service, self.ports.rtc, RTC_STM_SERVICE)
    }

    /// C++ `ota::requestRestart(reason, delayMs, detail)`: its RebootRequested event is logged
    /// when the request counts.
    fn request_restart(&self, reason: u8, delay_ms: u32, detail: i32) {
        let now = self.ports.clock.now_ms();
        if let Some(e) = self
            .shared
            .ota
            .request_restart(now, reason, delay_ms, detail)
        {
            self.logger.log_event(&e);
        }
    }

    /// GET /api/health (C++ `app::readHealth`, the web server's call).
    pub fn read_health<'s>(&self, out: &mut HealthSnapshot<'s>) {
        let s = self.shared;
        let parts = HealthParts {
            net: s.net.health(),
            ota: s.ota.health(),
            log: s.logger.stats(self.ports.clock.now_ms()),
        };
        read_health(self.ports.clock, self.ports.system, &s.app, parts, out);
    }
}

impl<P: Platform> StmServiceHost for Wire<'_, P> {
    fn log(&mut self, e: &Event) {
        self.logger.log_event(e);
    }
    fn submit(&mut self, cmd: &StmCommand) -> bool {
        self.shared.app.submit(cmd)
    }
    fn calib_info(&mut self) -> CalibInfo {
        self.shared.app.calib_info()
    }
    fn set_calib_info(&mut self, c: &CalibInfo) {
        self.shared.app.set_calib_info(c);
    }
    fn config_revision(&mut self) -> u32 {
        self.shared.storage.config_revision()
    }
    fn calib_config(&mut self) -> CalibScheduleConfig {
        self.shared.storage.calib_config()
    }
    fn load_calib_slot(&mut self) -> u32 {
        self.storage.load_calib_slot()
    }
    fn save_calib_slot(&mut self, slot: u32) {
        self.storage.save_calib_slot(slot);
    }
    fn load_last_calib(&mut self) -> i64 {
        self.storage.load_last_calib()
    }
    fn save_last_calib(&mut self, epoch: i64) {
        self.storage.save_last_calib(epoch);
    }
    fn load_targets(&mut self, out: &mut [u8]) -> usize {
        self.storage.load_targets(out)
    }
    fn save_targets(&mut self, data: &[u8]) -> bool {
        self.storage.save_targets(data)
    }
    fn local_time(&mut self) -> LocalTime {
        net::local_time(self.ports.wall)
    }
}

impl<P: Platform> NetHost for Wire<'_, P> {
    fn log(&mut self, e: &Event) {
        self.logger.log_event(e);
    }
    fn request_restart(&mut self, reason: u8, delay_ms: u32, detail: i32) {
        Wire::request_restart(self, reason, delay_ms, detail);
    }
    fn get_config(&mut self, out: &mut Config) {
        self.shared.storage.get_config(out);
    }
    fn apply_config(&mut self, c: &Config) -> bool {
        // C++ applyConfig(c, nullptr, 0): no path wanted
        self.storage.apply_config(c, &mut []).is_ok()
    }
    fn load_net_trial_blob(&mut self, out: &mut [u8]) -> usize {
        self.storage.load_net_trial_blob(out)
    }
    fn save_net_trial_blob(&mut self, data: &[u8]) -> bool {
        self.storage.save_net_trial_blob(data)
    }
    fn clear_net_trial(&mut self) {
        self.storage.clear_net_trial();
    }
}

impl<P: Platform> MqttHost for Wire<'_, P> {
    fn uptime_s(&self) -> u32 {
        self.ports.clock.uptime_s()
    }
    fn submit(&self, cmd: &StmCommand) -> bool {
        self.shared.app.submit(cmd)
    }
    fn stm_snapshot_revision(&self) -> u32 {
        self.shared.app.stm_snapshot_revision()
    }
    fn read_stm_snapshot(&self, out: &mut StmSnapshot) {
        self.shared.app.read_stm_snapshot(out);
    }
    fn read_profile(&self, valve: u8, out: &mut Profile) {
        self.shared.app.read_profile(valve, out);
    }
    fn calib_next_epoch(&self) -> i64 {
        self.shared.app.calib_info().next_epoch
    }
    fn config_revision(&self) -> u32 {
        self.shared.storage.config_revision()
    }
    fn with_config(&self, f: &mut dyn FnMut(&Config)) {
        self.shared.storage.with_config(f);
    }
    fn fs_ready(&self) -> bool {
        self.shared.storage.fs_ready()
    }
    fn ha_cleanup_done(&self) -> bool {
        self.storage.ha_cleanup_done()
    }
    fn set_ha_cleanup_done(&self) {
        self.storage.set_ha_cleanup_done();
    }
    fn ha_layout(&self) -> u8 {
        self.storage.ha_layout()
    }
    fn set_ha_layout(&self, layout: u8) {
        self.storage.set_ha_layout(layout);
    }
    fn log(&self, code: EventCode, valve: u8, arg1: i32, arg2: i32, text: &[u8]) -> u32 {
        self.logger.log(code, valve, arg1, arg2, text)
    }
    fn read_events_since(&self, since: u32, out: &mut [Event]) -> (usize, u32) {
        let mut next = since;
        let n = self.shared.logger.read_since(since, out, &mut next);
        (n, next)
    }
    fn net_up(&self) -> bool {
        self.shared.net.is_up()
    }
    fn net_ip(&self) -> u32 {
        self.shared.net.info().ip
    }
    fn local_time(&self) -> LocalTime {
        net::local_time(self.ports.wall)
    }
    fn restart_pending(&self) -> bool {
        self.shared.ota.restart_pending()
    }
    fn request_restart(&self, reason: u8, delay_ms: u32) {
        Wire::request_restart(self, reason, delay_ms, 0);
    }
}

impl<P: Platform> StmLinkHost for Wire<'_, P> {
    fn log_event(&mut self, e: &Event) {
        self.logger.log_event(e);
    }
    fn publish(&mut self, s: &StmSnapshot) {
        self.shared.app.publish_stm_snapshot(s);
    }
    fn store_profile(&mut self, p: &Profile) {
        self.shared.app.store_profile(p);
    }
    fn request_last_good_copy(&mut self, image: &[u8]) {
        self.storage.request_last_good_copy(image);
    }
    fn restart_pending(&mut self) -> bool {
        self.shared.ota.restart_pending()
    }
    fn mark_flash_active(&mut self) {
        self.shared.app.mark_stm_flash_active();
    }
    fn store_desired_targets(&mut self, t: &PersistedTargets) {
        self.stm_service_link().store_desired_targets(t);
    }
    fn post_scheduled_calib_result(&mut self, attempt: u16, ok: bool, reason: CalibFailure) {
        self.stm_service_link()
            .post_scheduled_calib_result(attempt, ok, reason);
    }
    fn set_stm_save_state(&mut self, s: StmSaveState) {
        self.shared.app.set_stm_save_state(s);
    }
    fn store_lease_record(&mut self, s: &LeaseClientSnapshot) {
        self.stm_service_link().store_lease_record(s);
    }
    fn config_revision(&mut self) -> u32 {
        self.shared.storage.config_revision()
    }
    fn with_config(&mut self, f: &mut dyn FnMut(&Config)) {
        self.shared.storage.with_config(f);
    }
    fn boot_load_source(&mut self) -> LoadSource {
        self.shared.storage.boot_load_source()
    }
    fn config_saved_since_boot(&mut self) -> bool {
        self.shared.storage.config_saved_since_boot()
    }
    fn boot_targets(&mut self) -> (PersistedTargets, RestoreSource) {
        self.stm_service_link().boot_targets()
    }
    fn boot_lease(&mut self) -> Option<LeaseClientSnapshot> {
        self.stm_service_link().boot_lease()
    }
    fn receive(&mut self) -> Option<StmCommand> {
        self.shared.app.receive()
    }
    fn regulator_state(&mut self) -> RegulatorInput {
        self.shared.mqtt.regulator_state()
    }
    fn stm_save_state(&mut self) -> StmSaveState {
        self.shared.app.stm_save_state()
    }
}

/// The web server's calls into `app`, `logger`, `net` and `mqtt` (C++ `web_server.cpp`), one
/// forwarding call each.
impl<P: Platform> WebHost for Wire<'_, P> {
    fn submit(&self, cmd: &StmCommand) -> bool {
        self.shared.app.submit(cmd)
    }
    fn read_stm_snapshot(&self, out: &mut StmSnapshot) {
        self.shared.app.read_stm_snapshot(out);
    }
    fn read_profile(&self, valve: u8, out: &mut Profile) {
        self.shared.app.read_profile(valve, out);
    }
    fn stm_flash_active(&self) -> bool {
        self.shared.app.stm_flash_active()
    }
    fn stm_support(&self) -> StmSupport {
        self.shared.app.stm_support()
    }
    fn stm_protocol(&self) -> u8 {
        self.shared.app.stm_protocol()
    }
    fn calib_info(&self) -> CalibInfo {
        self.shared.app.calib_info()
    }
    fn read_health<'s>(&'s self, out: &mut HealthSnapshot<'s>) {
        Wire::read_health(self, out);
    }
    fn log(&self, e: &Event) {
        self.logger.log_event(e);
    }
    fn read_events(&self, f: &EventFilter, out: &mut [Event]) -> LogRead {
        self.shared.logger.read(f, out)
    }
    fn last_event_seq(&self) -> u32 {
        self.shared.logger.last_seq()
    }
    fn request_log_flush(&self) {
        self.shared.logger.request_flush();
    }
    fn note_inbound_http(&self, remote_ip: u32) {
        self.shared.net.note_inbound_http(remote_ip);
    }
    fn net_info(&self) -> NetInfo {
        self.shared.net.info()
    }
    fn net_trial(&self) -> TrialInfo {
        self.shared.net.trial_info()
    }
    fn request_trial_confirm(&self) -> bool {
        self.shared.net.request_trial_confirm()
    }
    fn request_trial_revert(&self) -> bool {
        self.shared.net.request_trial_revert()
    }
    fn last_sync_epoch(&self) -> u32 {
        self.shared.net.last_sync_epoch()
    }
    fn mqtt_status(&self) -> MqttStatus {
        self.shared.mqtt.status()
    }
    fn calibration_end(&self, valve: u8) -> Option<LocalTime> {
        self.shared.mqtt.calibration_end(valve)
    }
    fn request_mqtt_reconnect(&self) {
        self.shared.mqtt.request_reconnect();
    }
    fn request_discovery(&self, a: DiscoveryAction) {
        self.shared.mqtt.request_discovery(a);
    }
}

// ---------------------------------------------------------------- module constructors

/// stm_service of the firmware: the app thread's part behind a mutex (the restart path of OTA
/// flushes it).
pub type FwService<'a, P> = StmService<'a, P, Wire<'a, P>>;

/// stm_service over the RTC records of the layout; `main` puts it behind a mutex.
pub fn stm_service<'a, P: Platform>(
    ports: Ports<'a, P>,
    shared: &'a Shared,
    storage: &'a FwStorage<'a, P>,
) -> FwService<'a, P> {
    let p = StmServicePorts {
        clock: ports.clock,
        wall: ports.wall,
        rtc: ports.rtc,
    };
    let host = Wire::new(ports, shared, storage);
    StmService::new(p, &shared.stm_service, host, RTC_STM_SERVICE)
}

/// The stm thread of the firmware.
pub type FwLink<'a, P> = StmLink<'a, P, Wire<'a, P>>;

/// The stm thread with its UART, NRST (IO15) and BOOT0 (IO14).
pub fn stm_link<'a, P: Platform>(
    ports: Ports<'a, P>,
    shared: &'a Shared,
    storage: &'a FwStorage<'a, P>,
    uart: P::Uart,
    nrst: P::OutputPin,
    boot0: P::OutputPin,
) -> FwLink<'a, P> {
    let p = StmLinkPorts {
        clock: ports.clock,
        fs: ports.fs,
        uart,
        nrst,
        boot0,
    };
    StmLink::new(p, Wire::new(ports, shared, storage))
}

/// The MQTT thread of the firmware.
pub type FwMqtt<'a, P> = MqttClient<
    'a,
    <P as Platform>::Clock,
    <P as Platform>::Fs,
    <P as Platform>::Tcp,
    <P as Platform>::HeapGate,
    <P as Platform>::Rtc,
    <P as Platform>::Watchdog,
    Wire<'a, P>,
>;

/// The MQTT thread: its packet buffer and boot blocks, the HA status record of the layout, the
/// client id from the factory MAC.
pub fn mqtt<'a, P: Platform>(
    ports: Ports<'a, P>,
    shared: &'a Shared,
    storage: &'a FwStorage<'a, P>,
) -> FwMqtt<'a, P> {
    let p = MqttPorts {
        clock: ports.clock,
        fs: ports.fs,
        tcp: ports.tcp,
        gate: ports.heap,
        rtc: ports.rtc,
        rtc_offset: RTC_HA_STATUS,
        mac: ports.system.base_mac(),
    };
    MqttClient::new(p, Wire::new(ports, shared, storage), &shared.mqtt)
}

/// net of the firmware (the app thread).
pub type FwNet<'a, P> = Net<'a, P, Wire<'a, P>>;

/// net with its interfaces, SNTP and ping sessions, the watchdog record of the layout.
#[allow(clippy::too_many_arguments)]
pub fn net<'a, P: Platform>(
    ports: Ports<'a, P>,
    shared: &'a Shared,
    storage: &'a FwStorage<'a, P>,
    eth: P::Ethernet,
    wifi: P::Wifi,
    sntp: P::Sntp,
    pinger: P::Pinger,
) -> FwNet<'a, P> {
    let p = NetPorts {
        clock: ports.clock,
        wall: ports.wall,
        rtc: ports.rtc,
        heap: ports.heap,
        eth,
        wifi,
        sntp,
        pinger,
    };
    let host = Wire::new(ports, shared, storage);
    Net::new(p, &shared.net, host, RTC_NET_WATCHDOG)
}

/// ota's host (C++ `logger::`, `net::`, `app::`, `storage::`, `stm_service::` calls): the app
/// thread's log sinks and stm_service for the restart path, [`Wire`] for the rest. The web
/// server's `OtaUpload` takes one too (its paths call neither flush).
pub struct OtaWire<'a, P: Platform> {
    wire: Wire<'a, P>,
    sinks: &'a Mutex<FwSinks<'a, P>>,
    service: &'a Mutex<FwService<'a, P>>,
}

impl<'a, P: Platform> OtaWire<'a, P> {
    /// ota's host over these objects.
    pub fn new(
        wire: Wire<'a, P>,
        sinks: &'a Mutex<FwSinks<'a, P>>,
        service: &'a Mutex<FwService<'a, P>>,
    ) -> Self {
        OtaWire {
            wire,
            sinks,
            service,
        }
    }
}

impl<P: Platform> OtaHost for OtaWire<'_, P> {
    fn log(&mut self, e: &Event) {
        self.wire.logger.log_event(e);
    }
    fn log_flush(&mut self) {
        lock(self.sinks).flush();
    }
    fn net_is_up(&mut self) -> bool {
        self.wire.shared.net.is_up()
    }
    fn net_ip(&mut self) -> u32 {
        self.wire.shared.net.info().ip
    }
    fn stm_flash_active(&mut self) -> bool {
        self.wire.shared.app.stm_flash_active()
    }
    fn image_upload_active(&mut self) -> bool {
        self.wire.storage.image_upload_active()
    }
    fn stm_link_state(&mut self) -> LinkState {
        self.wire.shared.app.stm_link_state()
    }
    fn stm_save_state(&mut self) -> StmSaveState {
        self.wire.shared.app.stm_save_state()
    }
    fn request_stm_save(&mut self) {
        self.wire.shared.app.request_stm_save();
    }
    fn flush_for_restart(&mut self) {
        lock(self.service).flush_for_restart();
    }
    fn set_ota_stm_required(&mut self, on: bool) {
        self.wire.storage.set_ota_stm_required(on);
    }
}

/// OTA validation and the restart path of the firmware (the app thread).
pub type FwOta<'a, P> = OtaService<'a, P, OtaWire<'a, P>>;

/// ota over the boot guard's decision of this boot.
pub fn ota<'a, P: Platform>(
    ports: Ports<'a, P>,
    shared: &'a Shared,
    host: OtaWire<'a, P>,
    guard: BootGuard,
) -> FwOta<'a, P> {
    let p = OtaPorts {
        clock: ports.clock,
        tcp: ports.tcp,
        nvs: ports.nvs,
        ota: ports.ota,
        rtc: ports.rtc,
        system: ports.system,
    };
    OtaService::new(p, &shared.ota, host, guard)
}

// ---------------------------------------------------------------- the HTTP thread

/// The web server of the firmware: one instance, served by the HTTP thread
/// ([`Web::handle`] per request).
pub type FwWeb<'a, P> = Web<
    'a,
    P,
    &'a <P as Platform>::Nvs,
    &'a <P as Platform>::Fs,
    &'a <P as Platform>::HeapGate,
    StorageWire<'a, P>,
    OtaWire<'a, P>,
    Wire<'a, P>,
>;

/// The ESP upload of the web server.
pub type FwUpload<'a, P> = OtaUpload<'a, P, OtaWire<'a, P>>;

/// The web server over the shared objects and storage, with its ESP upload (the update slot,
/// `md5` for the image, an [`OtaWire`] as its OTA host) and the dashboard `assets`. Allocates
/// nothing: the first request takes the working set.
pub fn web<'a, P: Platform>(
    ports: Ports<'a, P>,
    shared: &'a Shared,
    storage: &'a FwStorage<'a, P>,
    upload: FwUpload<'a, P>,
    assets: &'a [Asset],
) -> FwWeb<'a, P> {
    let p = WebPorts {
        clock: ports.clock,
        wall: ports.wall,
        fs: ports.fs,
        ota: ports.ota,
        system: ports.system,
        gate: ports.heap,
    };
    let host = Wire::new(ports, shared, storage);
    Web::new(p, storage, &shared.ota, upload, host, assets)
}

/// The ESP upload into the update slot, `md5` for the image, its OTA host an [`OtaWire`] (its
/// paths flush neither the log nor stm_service).
pub fn web_upload<'a, P: Platform>(
    ports: Ports<'a, P>,
    shared: &'a Shared,
    storage: &'a FwStorage<'a, P>,
    sinks: &'a Mutex<FwSinks<'a, P>>,
    service: &'a Mutex<FwService<'a, P>>,
    md5: P::Md5,
) -> FwUpload<'a, P> {
    let host = OtaWire::new(Wire::new(ports, shared, storage), sinks, service);
    OtaUpload::new(ports.clock, ports.ota, md5, ports.heap, &shared.ota, host)
}

// ---------------------------------------------------------------- the app thread

/// The app thread's calls into the web server (C++ `web::begin`, `web::started`).
pub trait AppWeb {
    /// The server runs.
    fn started(&self) -> bool;
    /// Starts the server (idempotent; a failed start is tried again by the next call).
    fn begin(&mut self);
}

/// The firmware's [`AppWeb`]: the HTTP server port, started through web_server's [`WebStart`]
/// (C++ `web::begin`, `web::started`).
pub struct WebBegin<S> {
    server: S,
    start: WebStart,
}

impl<S: HttpServer> WebBegin<S> {
    /// Not started.
    pub fn new(server: S) -> Self {
        WebBegin {
            server,
            start: WebStart::default(),
        }
    }
}

impl<S: HttpServer> AppWeb for WebBegin<S> {
    fn started(&self) -> bool {
        self.start.started()
    }
    fn begin(&mut self) {
        self.start.begin(&mut self.server);
    }
}

/// The app thread's modules: the firmware's [`AppHost`].
pub struct Modules<'a, P: Platform, W> {
    wire: Wire<'a, P>,
    sinks: &'a Mutex<FwSinks<'a, P>>,
    service: &'a Mutex<FwService<'a, P>>,
    net: FwNet<'a, P>,
    ota: FwOta<'a, P>,
    /// The boot guard's report of this boot (logged by `ota.begin`).
    report: BootReport,
    web: W,
}

impl<'a, P: Platform, W: AppWeb> Modules<'a, P, W> {
    /// The app thread's modules.
    pub fn new(
        wire: Wire<'a, P>,
        sinks: &'a Mutex<FwSinks<'a, P>>,
        service: &'a Mutex<FwService<'a, P>>,
        net: FwNet<'a, P>,
        ota: FwOta<'a, P>,
        report: BootReport,
        web: W,
    ) -> Self {
        Modules {
            wire,
            sinks,
            service,
            net,
            ota,
            report,
            web,
        }
    }

    /// The web server's start.
    pub fn web(&self) -> &W {
        &self.web
    }
}

impl<P: Platform, W: AppWeb> AppHost for Modules<'_, P, W> {
    fn factory_latched(&mut self) -> bool {
        self.wire.storage.factory_latched()
    }
    fn begin_fs(&mut self, formatted: &mut bool) -> bool {
        self.wire.storage.begin_fs(formatted)
    }
    fn factory_reset(&mut self) -> bool {
        self.wire.storage.factory_reset()
    }
    fn set_factory_latched(&mut self, on: bool) {
        self.wire.storage.set_factory_latched(on);
    }
    fn load_config(
        &mut self,
        out: &mut Config,
        report: &mut ImportReport,
        details: &mut LoadDetails,
    ) {
        self.wire.storage.load_config(out, report, details);
    }
    fn set_active_config(&mut self, c: &Config) {
        self.wire.shared.storage.set_active_config(c);
    }
    fn config_revision(&mut self) -> u32 {
        self.wire.shared.storage.config_revision()
    }
    fn increment_boot_count(&mut self) -> u32 {
        self.wire.storage.increment_boot_count()
    }
    fn log(&mut self, e: &Event) {
        self.wire.logger.log_event(e);
    }
    fn logger_configure(
        &mut self,
        syslog_level: u8,
        syslog_server: u32,
        syslog_port: u16,
        persist: bool,
        hostname: &[u8],
    ) {
        self.wire.shared.logger.configure(
            syslog_level,
            syslog_server,
            syslog_port,
            persist,
            hostname,
        );
    }
    fn stm_service_begin(&mut self) {
        lock(self.service).begin();
    }
    fn net_begin(&mut self, cfg: &mut Config) {
        self.net.begin(cfg);
    }
    fn ota_begin(&mut self) {
        self.ota.begin(&self.report);
    }
    fn get_config(&mut self, out: &mut Config) {
        self.wire.shared.storage.get_config(out);
    }
    fn net_reconfigure(&mut self, cfg: &Config) {
        self.net.reconfigure(cfg);
    }
    fn net_service(&mut self, now_ms: u32, mqtt_connected: bool) {
        self.net.service(now_ms, mqtt_connected);
    }
    fn mqtt_connected(&mut self) -> bool {
        self.wire.shared.mqtt.status().state == MqttState::Connected
    }
    fn net_is_up(&mut self) -> bool {
        self.wire.shared.net.is_up()
    }
    fn net_ota_ok(&mut self) -> bool {
        self.wire.shared.net.ota_net_ok()
    }
    fn web_started(&mut self) -> bool {
        self.web.started()
    }
    fn web_begin(&mut self) {
        self.web.begin();
    }
    fn ota_service(&mut self, now_ms: u32, net_ok: bool, link_up: bool, web_started: bool) {
        self.ota.service(now_ms, net_ok, link_up, web_started);
    }
    fn stm_service_service(&mut self, now_ms: u32) {
        lock(self.service).service(now_ms);
    }
    fn ota_upload_active(&mut self) -> bool {
        self.wire.shared.ota.upload_active()
    }
    fn ota_restart_pending(&mut self) -> bool {
        self.wire.shared.ota.restart_pending()
    }
    fn ota_request_restart(&mut self, reason: u8, delay_ms: u32) {
        self.wire.request_restart(reason, delay_ms, 0);
    }
    fn logger_service(&mut self, net_up: bool) {
        lock(self.sinks).service(net_up);
    }
    fn storage_service(&mut self) {
        self.wire.storage.service();
    }
    fn ota_service_restart(&mut self, now_ms: u32, net_up: bool, link_up: bool) {
        self.ota.service_restart(now_ms, net_up, link_up);
    }
}

/// The app of the firmware.
pub type FwApp<'a, P, W> = App<'a, P, Modules<'a, P, W>>;

/// The app thread over its modules, with the factory pin (IO2, pull-up).
pub fn app<'a, P: Platform, W: AppWeb>(
    ports: Ports<'a, P>,
    shared: &'a Shared,
    factory_pin: P::InputPin,
    modules: Modules<'a, P, W>,
) -> FwApp<'a, P, W> {
    let p = AppPorts {
        clock: ports.clock,
        system: ports.system,
        heap: ports.heap,
        factory_pin,
    };
    App::new(p, &shared.app, modules)
}

/// setup's begins of the stm and mqtt threads' modules.
pub struct Threads<'x, 'a, P: Platform> {
    pub link: &'x mut FwLink<'a, P>,
    pub mqtt: &'x mut FwMqtt<'a, P>,
}

impl<P: Platform> ThreadBegin for Threads<'_, '_, P> {
    fn stm_link_begin(&mut self) {
        self.link.begin();
    }
    fn mqtt_begin(&mut self) {
        self.mqtt.begin();
    }
}

impl<'a, P: Platform, H: StmLinkHost + Clone> Task<P::Watchdog> for StmLink<'a, P, H> {
    fn start(&mut self, watchdog: P::Watchdog) {
        StmLink::start(self, watchdog);
    }
    fn pass(&mut self) -> u32 {
        StmLink::pass(self)
    }
}

impl<'a, C, F, N, G, R, W, H> Task<W> for MqttClient<'a, C, F, N, G, R, W, H>
where
    C: Clock,
    F: Fs,
    N: TcpConnector,
    G: HeapGate,
    R: Rtc,
    W: Watchdog,
    H: MqttHost,
{
    fn start(&mut self, watchdog: W) {
        MqttClient::start(self, watchdog);
    }
    fn pass(&mut self) -> u32 {
        MqttClient::pass(self)
    }
}

#[cfg(test)]
mod tests;
