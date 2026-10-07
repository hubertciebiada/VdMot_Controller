//! Network (C++ `net.cpp`): Ethernet (LAN8720) first, the WiFi station when configured, DHCP or a
//! static address, the host name from the station name, SNTP with the POSIX TZ string. The
//! adapters keep the interface events in their state (link, address, GOT_IP counts); [`Net`]
//! runs the policy in the app thread every second: the state and its NetUp/NetDown events, the
//! WiFi fallback and its back-off, the end-to-end reachability with the gateway ping, the network
//! trial and the network watchdog (DESIGN.md section 16).
//!
//! What other modules read lives in [`NetShared`] (info, health, trial state, the trial requests
//! and the inbound HTTP counter); the calls into other glue modules go through [`NetHost`]. The
//! watchdog's restart count is kept in an RTC record of [`RTC_RECORD_LEN`] bytes at the offset
//! the firmware's RTC layout gives [`Net::new`].
//!
//! Port forms: Arduino's `WiFi.begin()` started the station driver again when it was off; here
//! [`Wifi::begin`] builds it once and a failed build is tried again with the next connect. The
//! one retry Arduino-ESP32 made after the first unrequested WiFi disconnect since boot (D§16) is
//! made here, in the next pass. A gateway probe whose session cannot be started is deleted at
//! once (the C++ deleted a session that did not start; one that could not be created had nothing
//! to delete). The last SNTP sync epoch reaches [`NetShared`] with the next pass.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use vdm_esp_core::common::{
    build_hostname, copy_string, elapsed_ms, format_ipv4, Backoff, LocalTime, Text, NO_VALVE,
    STATION_NAME_MAX,
};
use vdm_esp_core::config::{
    config_restart_reasons_from, effective_dns, net_trial_required, Config, NetConfig,
    NetInterface, TimeConfig, HOST_MAX, RESTART_NETWORK, TZ_POSIX_MAX,
};
use vdm_esp_core::event_log::{
    event_default_severity, make_event, Event, EventCode, RebootReason, Severity,
};
use vdm_esp_core::json_api::{NetHealthInfo, NetState};
use vdm_esp_core::net_policy::{
    NetEvidence, NetReachability, NetReachabilityChange, NetWatchdog, NetWatchdogAction,
};
use vdm_esp_core::net_trial::{
    apply_net_trial_fields, decode_net_trial, encode_net_trial, format_net_address,
    net_trial_at_boot, net_trial_fields_crc, NetTrial, NetTrialBoot, NetTrialDecision,
    NetTrialRecord, NetTrialRevert, NetTrialState, NET_TRIAL_BLOB_MAX, NET_TRIAL_WINDOW_MS,
};

use crate::heap::try_block;
use crate::port::{Clock, Ethernet, IpInfo, IpSetup, Pinger, Platform, Rtc, Sntp, WallClock, Wifi};

/// 2020-01-01 00:00:00 UTC: an earlier wall clock has not been set by SNTP.
pub const MIN_VALID_EPOCH: i64 = 1_577_836_800;
/// Interface auto: WiFi starts after this long without an Ethernet address.
pub const WIFI_FALLBACK_MS: u32 = 30_000;
/// First wait between two WiFi connects while the station has no address.
pub const WIFI_BACKOFF_MIN_MS: u32 = 5_000;
/// Longest wait between two WiFi connects.
pub const WIFI_BACKOFF_MAX_MS: u32 = 60_000;
/// A probe reports within 1 s (the reply or the timeout); a session that has not reported by
/// then is deleted, so a lost report cannot stop the probes.
pub const PING_SESSION_MAX_MS: u32 = 10_000;
/// Check word of the RTC record ("VNWD").
pub const RTC_MAGIC: u32 = 0x564E_5744;
/// Size of the RTC record: the check word and the watchdog restarts of the outage in progress,
/// both u32 little endian.
pub const RTC_RECORD_LEN: usize = 8;
/// First octet of the loopback network (in the low byte).
const LOOPBACK_NET: u32 = 127;
/// The ESP restart of a network change waits for the HTTP response.
const RECONFIGURE_RESTART_DELAY_MS: u32 = 1500;
/// The ESP restart of a revert or of the watchdog lets MQTT `offline` go out.
const RESTART_DELAY_MS: u32 = 1000;
/// Text of the NetDown event of an Ethernet driver that did not start.
const ETH_INIT_FAILED: &[u8] = b"eth init failed";

/// State of the network (C++ `net::Info`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NetInfo {
    /// The interface with an address, Down without one.
    pub state: NetState,
    /// Address configuration of that interface (kept as read when an interface has no address).
    pub ip: u32,
    pub mask: u32,
    pub gateway: u32,
    pub dns: u32,
    /// MAC of the interface as Arduino's `macAddress()`: "24:0A:C4:12:34:56".
    pub mac: Text<17>,
    /// dBm on WiFi, 0 otherwise or without a valid reading.
    pub rssi: i8,
    /// When the state or the address last changed.
    pub up_since_ms: u32,
    /// Changes to a state with an address since boot.
    pub reconnects: u32,
}

/// The network trial for the API (C++ `net::TrialInfo`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrialInfo {
    /// A trial runs.
    pub active: bool,
    /// Whole seconds of its window left.
    pub remain_s: u32,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What `net` publishes for the other threads (C++ file statics behind `gMux` and the volatile
/// flags): written by [`Net`] in the app thread, read anywhere; the trial requests and the
/// inbound counter are written by the web server.
#[derive(Debug, Default)]
pub struct NetShared {
    info: Mutex<NetInfo>,
    health: Mutex<NetHealthInfo>,
    confirm: AtomicBool,
    revert: AtomicBool,
    inbound: AtomicU32,
    last_sync: AtomicU32,
}

impl NetShared {
    /// Nothing known yet: Down, no trial.
    pub fn new() -> Self {
        Self::default()
    }

    /// An interface has an address (C++ `net::isUp`).
    pub fn is_up(&self) -> bool {
        lock(&self.info).state != NetState::Down
    }

    /// The state of the network (C++ `net::info`).
    pub fn info(&self) -> NetInfo {
        lock(&self.info).clone()
    }

    /// Reachability and trial state for /api/health (C++ `net::health`; its time argument was
    /// not used).
    pub fn health(&self) -> NetHealthInfo {
        *lock(&self.health)
    }

    /// The network check of the OTA validator: reachable and proven end to end (C++
    /// `net::otaNetOk`).
    pub fn ota_net_ok(&self) -> bool {
        let h = self.health();
        h.reachable && h.proven
    }

    /// A request from `remote_ip` reached the web server: evidence of a working network, except
    /// from the loopback network and from the device's own address (C++ `net::noteInboundHttp`).
    pub fn note_inbound_http(&self, remote_ip: u32) {
        if (remote_ip & 0xFF) == LOOPBACK_NET || remote_ip == lock(&self.info).ip {
            return;
        }
        self.inbound.fetch_add(1, Ordering::SeqCst);
    }

    /// Keeps the settings of a running network trial; true when a trial runs (acted on in the
    /// next pass, C++ `net::requestTrialConfirm`).
    pub fn request_trial_confirm(&self) -> bool {
        let active = self.health().trial_active;
        if active {
            self.confirm.store(true, Ordering::SeqCst);
        }
        active
    }

    /// Reverts a running network trial; true when a trial runs (acted on in the next pass, C++
    /// `net::requestTrialRevert`).
    pub fn request_trial_revert(&self) -> bool {
        let active = self.health().trial_active;
        if active {
            self.revert.store(true, Ordering::SeqCst);
        }
        active
    }

    /// The network trial (C++ `net::trialInfo`).
    pub fn trial_info(&self) -> TrialInfo {
        let h = self.health();
        TrialInfo {
            active: h.trial_active,
            remain_s: h.trial_remaining_s,
        }
    }

    /// Epoch of the last SNTP sync, 0 before the first (C++ `net::lastSyncEpoch`).
    pub fn last_sync_epoch(&self) -> u32 {
        self.last_sync.load(Ordering::SeqCst)
    }
}

/// Local time; valid only once SNTP set the clock (2020 or later; C++ `net::localTime`).
pub fn local_time(wall: &impl WallClock) -> LocalTime {
    let epoch = wall.epoch();
    if epoch < MIN_VALID_EPOCH {
        return LocalTime::default();
    }
    match wall.local_time(epoch) {
        Some(t) => LocalTime {
            valid: true,
            epoch,
            ..t
        },
        None => LocalTime::default(),
    }
}

/// The wall clock was set by SNTP (C++ `net::timeValid`).
pub fn time_valid(wall: &impl WallClock) -> bool {
    wall.epoch() >= MIN_VALID_EPOCH
}

/// What `net` calls in the other glue modules (C++ `storage::`, `logger::` and `ota::`), one
/// method per C++ call; the firmware wiring implements it over their shared objects.
pub trait NetHost {
    /// C++ `logger::log(e)`: records the event (the logger fills seq, uptime and epoch).
    fn log(&mut self, e: &Event);
    /// C++ `ota::requestRestart(reason, delay_ms, detail)`: the ESP restart path.
    fn request_restart(&mut self, reason: u8, delay_ms: u32, detail: i32);
    /// C++ `storage::getConfig(out)`: the active config.
    fn get_config(&mut self, out: &mut Config);
    /// C++ `storage::applyConfig(c, nullptr, 0)`: validates, stores and activates `c`.
    fn apply_config(&mut self, c: &Config) -> bool;
    /// C++ `storage::loadNetTrialBlob(out, cap)`: the stored trial record into `out`, its length
    /// (0: none, or longer than `out`).
    fn load_net_trial_blob(&mut self, out: &mut [u8]) -> usize;
    /// C++ `storage::saveNetTrialBlob(data, len)`.
    fn save_net_trial_blob(&mut self, data: &[u8]) -> bool;
    /// C++ `storage::clearNetTrial()`.
    fn clear_net_trial(&mut self);
}

/// The ports `net` drives: the interfaces, SNTP and the ping sessions are its own, the clocks,
/// the RTC block and the heap gate are shared.
pub struct NetPorts<'a, P: Platform> {
    pub clock: &'a P::Clock,
    pub wall: &'a P::WallClock,
    pub rtc: &'a P::Rtc,
    pub heap: &'a P::HeapGate,
    pub eth: P::Ethernet,
    pub wifi: P::Wifi,
    pub sntp: P::Sntp,
    pub pinger: P::Pinger,
}

/// The parts of the time settings `net` applies.
struct TimeKept {
    ntp_server: Text<HOST_MAX>,
    tz_posix: Text<TZ_POSIX_MAX>,
}

/// UTF-8 of a config text for a port (config texts are ASCII; anything else is "").
fn text_str(t: &[u8]) -> &str {
    core::str::from_utf8(t).unwrap_or("")
}

/// How an interface starts: the host name, and the static addresses unless DHCP (without a DNS
/// server the gateway resolves).
fn ip_setup<'h>(hostname: &'h [u8], n: &NetConfig) -> IpSetup<'h> {
    IpSetup {
        hostname: text_str(hostname),
        fixed: (!n.dhcp).then(|| IpInfo {
            ip: n.ip,
            mask: n.mask,
            gateway: n.gateway,
            dns: effective_dns(n),
        }),
    }
}

/// The station and host name before [`Net::begin`].
fn default_station() -> Text<STATION_NAME_MAX> {
    let mut t = Text::new();
    copy_string(&mut t, b"VdMot");
    t
}

/// Arduino's `macAddress()` text: upper-case hex pairs separated by ':'.
fn format_mac(mac: [u8; 6]) -> Text<17> {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut t = Text::new();
    for (i, b) in mac.iter().enumerate() {
        if i > 0 {
            let _ = t.push(b':');
        }
        let _ = t.push(HEX[usize::from(b >> 4)]);
        let _ = t.push(HEX[usize::from(b & 15)]);
    }
    t
}

/// The network (C++ `net.cpp`), owned by the app thread.
pub struct Net<'a, P: Platform, H: NetHost> {
    clock: &'a P::Clock,
    wall: &'a P::WallClock,
    rtc: &'a P::Rtc,
    heap: &'a P::HeapGate,
    eth: P::Ethernet,
    wifi: P::Wifi,
    sntp: P::Sntp,
    pinger: P::Pinger,
    shared: &'a NetShared,
    host: H,
    rtc_offset: usize,
    /// The network settings in use; the restart rules compare them with the next config.
    net: NetConfig,
    time: TimeKept,
    station: Text<STATION_NAME_MAX>,
    hostname: Text<STATION_NAME_MAX>,
    static_ip: bool,
    watchdog: NetWatchdog,
    reach: NetReachability,
    trial: NetTrial,
    /// Settings before the running trial.
    trial_prev: NetConfig,
    /// Previous settings of the record armed during this boot.
    armed_prev: NetConfig,
    armed_this_boot: bool,
    eth_started: bool,
    wifi_started: bool,
    /// The one retry after the first unrequested WiFi disconnect since boot was made.
    wifi_retried: bool,
    eth_down_known: bool,
    eth_down_since_ms: u32,
    wifi_backoff: Backoff,
    sync_seen: u32,
    time_synced_once: bool,
    /// Wall clock and millis at the previous pass (the step of a sync).
    clock_ref_epoch: i64,
    clock_ref_ms: u32,
    got_ip_seen: u32,
    /// Replies of the probe in flight already counted.
    ping_seen: u32,
    inbound_seen: u32,
    /// Start of the probe in flight (C++ `gPing` != nullptr, `gPingStartMs`).
    probe: Option<u32>,
}

impl<'a, P: Platform, H: NetHost> Net<'a, P, H> {
    /// Nothing started; [`Net::begin`] starts the interfaces. `rtc_offset`: place of the RTC
    /// record ([`RTC_RECORD_LEN`] bytes) in the firmware's RTC layout.
    pub fn new(ports: NetPorts<'a, P>, shared: &'a NetShared, host: H, rtc_offset: usize) -> Self {
        let mut net = Net {
            clock: ports.clock,
            wall: ports.wall,
            rtc: ports.rtc,
            heap: ports.heap,
            eth: ports.eth,
            wifi: ports.wifi,
            sntp: ports.sntp,
            pinger: ports.pinger,
            shared,
            host,
            rtc_offset,
            net: NetConfig::default(),
            time: TimeKept {
                ntp_server: Text::new(),
                tz_posix: Text::new(),
            },
            station: default_station(),
            hostname: default_station(),
            static_ip: false,
            watchdog: NetWatchdog::default(),
            reach: NetReachability::default(),
            trial: NetTrial::default(),
            trial_prev: NetConfig::default(),
            armed_prev: NetConfig::default(),
            armed_this_boot: false,
            eth_started: false,
            wifi_started: false,
            wifi_retried: false,
            eth_down_known: false,
            eth_down_since_ms: 0,
            wifi_backoff: Backoff::new(WIFI_BACKOFF_MIN_MS, WIFI_BACKOFF_MAX_MS),
            sync_seen: 0,
            time_synced_once: false,
            clock_ref_epoch: 0,
            clock_ref_ms: 0,
            got_ip_seen: 0,
            ping_seen: 0,
            inbound_seen: 0,
            probe: None,
        };
        net.keep_default_time();
        net
    }

    /// The time settings before [`Net::begin`] (C++ `gTime` with its member initialisers).
    fn keep_default_time(&mut self) {
        self.keep_time(&TimeConfig::default());
    }

    /// Host name in use (from the station name at [`Net::begin`]).
    pub fn hostname(&self) -> &str {
        text_str(&self.hostname)
    }

    fn log(&mut self, code: EventCode, sev: Severity, arg1: i32, arg2: i32, text: &[u8]) {
        self.host
            .log(&make_event(code, sev, NO_VALVE, arg1, arg2, text));
    }

    fn log_default(&mut self, code: EventCode, arg1: i32, arg2: i32, text: &[u8]) {
        self.log(code, event_default_severity(code), arg1, arg2, text);
    }

    // ---------------------------------------------------------------- RTC record

    /// Watchdog restarts of the outage in progress, kept across software restarts; 0 without
    /// the check word (power-on garbage) or above 255.
    fn load_outage_restarts(&self) -> u8 {
        let mut r = [0u8; RTC_RECORD_LEN];
        self.rtc.load(self.rtc_offset, &mut r);
        let [m0, m1, m2, m3, n0, n1, n2, n3] = r;
        if u32::from_le_bytes([m0, m1, m2, m3]) != RTC_MAGIC {
            return 0;
        }
        u8::try_from(u32::from_le_bytes([n0, n1, n2, n3])).unwrap_or(0)
    }

    fn store_outage_restarts(&self, n: u8) {
        let mut r = [0u8; RTC_RECORD_LEN];
        r[..4].copy_from_slice(&RTC_MAGIC.to_le_bytes());
        r[4..].copy_from_slice(&u32::from(n).to_le_bytes());
        self.rtc.store(self.rtc_offset, &r);
    }

    // ---------------------------------------------------------------- interfaces

    fn eth_up(&self) -> bool {
        self.eth.link() && (self.eth.has_ip() || self.static_ip)
    }

    fn wifi_wanted(&self) -> bool {
        self.net.iface != NetInterface::Ethernet && !self.net.ssid.is_empty()
    }

    /// `net` starts, connects and stops WiFi (at boot, then in the app thread). The station is
    /// built once with the host name before its start; a build that failed is tried again by the
    /// next connect.
    fn start_wifi(&mut self) {
        if !self.wifi_started {
            self.wifi_started = self.wifi.begin(&ip_setup(&self.hostname, &self.net));
        }
        if self.wifi_started {
            self.wifi.connect(&self.net.ssid, &self.net.wifi_password);
        }
    }

    fn stop_wifi(&mut self) {
        if !self.wifi_started {
            return;
        }
        self.wifi.stop();
        self.wifi_started = false;
    }

    /// Arduino-ESP32 retried once after the first WiFi disconnect since boot that it had not
    /// asked for (`WiFi.disconnect(); WiFi.begin()`); the automatic reconnect stays off.
    fn retry_wifi_once(&mut self) {
        if self.wifi.take_unrequested_disconnect() && !self.wifi_retried {
            self.wifi_retried = true;
            self.wifi.reconnect();
        }
    }

    /// The state from the interfaces; NetUp names the address, NetDown the interface lost.
    fn refresh_info(&mut self, now_ms: u32) {
        let mut i = NetInfo::default();
        if self.eth_up() {
            let a = self.eth.info();
            i.state = NetState::Ethernet;
            (i.ip, i.mask, i.gateway, i.dns) = (a.ip, a.mask, a.gateway, a.dns);
            i.mac = format_mac(self.eth.mac());
        } else if self.wifi.up() {
            let a = self.wifi.info();
            i.state = NetState::Wifi;
            (i.ip, i.mask, i.gateway, i.dns) = (a.ip, a.mask, a.gateway, a.dns);
            // a positive RSSI is no valid reading
            i.rssi = self.wifi.rssi().min(0);
            i.mac = format_mac(self.wifi.mac());
        }
        if i.state != NetState::Down && i.ip == 0 {
            i.state = NetState::Down;
        }
        let (before, changed) = {
            let mut g = lock(&self.shared.info);
            let before = g.state;
            let changed = i.state != g.state || i.ip != g.ip;
            i.up_since_ms = if changed { now_ms } else { g.up_since_ms };
            i.reconnects = g
                .reconnects
                .wrapping_add(u32::from(changed && i.state != NetState::Down));
            *g = i.clone();
            (before, changed)
        };
        if !changed {
            return;
        }
        if i.state == NetState::Down {
            self.log_default(EventCode::NetDown, before as i32, 0, b"");
        } else {
            let mut ip = [0u8; 16];
            let n = format_ipv4(i.ip, &mut ip);
            self.log_default(EventCode::NetUp, i.state as i32, 0, &ip[..n]);
        }
    }

    // ---------------------------------------------------------------- time

    /// SNTP with the configured server and the POSIX TZ string; without a server SNTP stops
    /// and only TZ is set (C++ `configTzTime`: SNTP first, then TZ).
    fn apply_time(&mut self) {
        let server = (!self.time.ntp_server.is_empty()).then(|| text_str(&self.time.ntp_server));
        self.sntp.configure(server);
        self.wall.set_time_zone(text_str(&self.time.tz_posix));
    }

    /// TimeSynced: the step is the wall clock after the sync minus the clock expected from the
    /// previous pass (Info the first time, Debug afterwards). True when a sync happened since
    /// the last pass.
    fn check_time_sync(&mut self, now_ms: u32) -> bool {
        let now_epoch = self.wall.epoch();
        let count = self.sntp.sync_count();
        let synced = count != self.sync_seen;
        if synced {
            self.sync_seen = count;
            let expected = self
                .clock_ref_epoch
                .saturating_add(i64::from(elapsed_ms(now_ms, self.clock_ref_ms) / 1000));
            let step = now_epoch
                .saturating_sub(expected)
                .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
            let sev = if self.time_synced_once {
                Severity::Debug
            } else {
                Severity::Info
            };
            self.log(EventCode::TimeSynced, sev, step, 0, b"");
            self.time_synced_once = true;
        }
        self.clock_ref_epoch = now_epoch;
        self.clock_ref_ms = now_ms;
        synced
    }

    // ---------------------------------------------------------------- reachability

    /// The session of a probe that reported goes, and one without a report after
    /// [`PING_SESSION_MAX_MS`]: a session owns a task and a socket, needed only while its probe
    /// runs. The app thread deletes it, never the ping task.
    fn end_probe(&mut self, now_ms: u32) {
        let Some(start) = self.probe else {
            return;
        };
        if !self.pinger.done() && elapsed_ms(now_ms, start) < PING_SESSION_MAX_MS {
            return;
        }
        self.pinger.delete();
        self.probe = None;
    }

    /// One echo to the gateway in a session of its own; the reply counts as evidence in a later
    /// pass. False while the previous probe still runs (the probe stays due). A session that
    /// cannot be started is deleted and counts as sent: the next probe tries again.
    fn probe_gateway(&mut self, gateway: u32, now_ms: u32) -> bool {
        if self.probe.is_some() {
            return false;
        }
        self.ping_seen = 0;
        if self.pinger.start(gateway) {
            self.probe = Some(now_ms);
        } else {
            self.pinger.delete();
        }
        true
    }

    /// Watchdog stage 1: restarts the interfaces that run. 1 Ethernet, 2 WiFi, 3 both, 0 none
    /// (no Ethernet link since boot: the driver refuses the restart).
    fn restart_interface(&mut self) -> i32 {
        let mut mask = 0;
        if self.eth_started && self.eth.restart() {
            mask += 1;
        }
        if self.wifi_started {
            self.wifi.reconnect();
            mask += 2;
        }
        mask
    }

    fn note_evidence(&mut self, now_ms: u32, mqtt_connected: bool, synced: bool) {
        if self.probe.is_some() {
            let replies = self.pinger.replies();
            if replies != self.ping_seen {
                self.ping_seen = replies;
                self.reach.on_evidence(NetEvidence::GatewayPing, now_ms);
            }
        }
        if mqtt_connected {
            self.reach.on_evidence(NetEvidence::Mqtt, now_ms);
        }
        if synced {
            self.reach.on_evidence(NetEvidence::TimeSync, now_ms);
        }
        let inbound = self.shared.inbound.load(Ordering::SeqCst);
        if inbound != self.inbound_seen {
            self.inbound_seen = inbound;
            self.reach.on_evidence(NetEvidence::InboundHttp, now_ms);
        }
        // a static configuration raises GOT_IP as well: only a DHCP lease counts
        let got_ip = self
            .eth
            .got_ip_count()
            .wrapping_add(self.wifi.got_ip_count());
        if got_ip != self.got_ip_seen {
            self.got_ip_seen = got_ip;
            if self.net.dhcp {
                self.reach.on_evidence(NetEvidence::DhcpLease, now_ms);
            }
        }
    }

    fn report_reachability(&mut self, now_ms: u32) {
        match self.reach.change(now_ms) {
            NetReachabilityChange::Lost => {
                let age = self.reach.evidence_age_ms(now_ms);
                let age_s = if age == u32::MAX {
                    -1
                } else {
                    (age / 1000) as i32
                };
                let evidence = self.reach.last_evidence() as i32;
                self.log_default(EventCode::NetUnreachable, age_s, evidence, b"");
            }
            NetReachabilityChange::Regained => {
                let lost_s = (self.reach.lost_for_ms(now_ms) / 1000) as i32;
                self.log_default(EventCode::NetReachable, lost_s, 0, b"");
            }
            NetReachabilityChange::None => {}
        }
    }

    // ---------------------------------------------------------------- trial

    /// Stores `base` (`None`: the active config) with the trial fields of `fields`. A revert is
    /// rare, so its copy of the config lives on the heap for this moment only. False when
    /// nothing was stored, also for want of memory.
    fn store_net_fields(
        heap: &P::HeapGate,
        host: &mut H,
        base: Option<&Config>,
        fields: &NetConfig,
    ) -> bool {
        let Some(mut c) = try_block(heap, Config::default) else {
            return false;
        };
        match base {
            Some(b) => c.clone_from(b),
            None => host.get_config(&mut c),
        }
        apply_net_trial_fields(&mut c.net, fields);
        host.apply_config(&c)
    }

    /// The trial settings failed (or the user asked): the previous fields go back into the
    /// stored config and the ESP restarts. A failed persist leaves the record Running, so the
    /// next boot reverts before the interfaces start.
    fn revert_trial(&mut self, reason: NetTrialRevert) {
        apply_net_trial_fields(&mut self.net, &self.trial_prev);
        let mut addr = [0u8; 16];
        let n = format_net_address(&self.trial_prev, &mut addr);
        let ok = Self::store_net_fields(self.heap, &mut self.host, None, &self.trial_prev);
        if ok {
            self.host.clear_net_trial();
        }
        self.log_default(
            EventCode::NetTrialReverted,
            reason as i32,
            if ok { 0 } else { -1 },
            &addr[..n],
        );
        self.host
            .request_restart(RebootReason::NetRevert as u8, RESTART_DELAY_MS, 0);
    }

    fn service_trial(&mut self, now_ms: u32, up: bool) {
        if self.shared.confirm.swap(false, Ordering::SeqCst) {
            let up_for = self.trial.up_for_ms(now_ms);
            if self.trial.confirm() {
                self.host.clear_net_trial();
                self.log_default(EventCode::NetTrialConfirmed, (up_for / 1000) as i32, 0, b"");
            }
        }
        if self.shared.revert.swap(false, Ordering::SeqCst) && self.trial.confirm() {
            self.revert_trial(NetTrialRevert::User);
        }
        if self.trial.update(up, now_ms) == NetTrialDecision::Revert {
            self.revert_trial(self.trial.reason());
        }
    }

    /// A change of the trial fields: its record (Armed, with the settings before the first
    /// change of this boot) is stored for the boot after the restart. A change saved during a
    /// running trial came over the trial network: it confirms that trial, and the running
    /// settings become the previous ones. False when the record could not be stored.
    fn arm_trial(&mut self, cfg: &Config) -> bool {
        let mut r = NetTrialRecord {
            trial_crc: net_trial_fields_crc(&cfg.net),
            ..NetTrialRecord::default()
        };
        if !self.armed_this_boot {
            let up_for = self.trial.up_for_ms(self.clock.now_ms());
            if self.trial.confirm() {
                self.log_default(EventCode::NetTrialConfirmed, (up_for / 1000) as i32, 1, b"");
            }
            self.armed_prev.clone_from(&self.net);
            self.armed_this_boot = true;
        }
        // two changes before the restart: the first one's previous settings are the proven ones
        r.previous.clone_from(&self.armed_prev);
        if !self.save_trial_record(&r) {
            return false;
        }
        let mut addr = [0u8; 16];
        let n = format_net_address(&cfg.net, &mut addr);
        let window_s = (NET_TRIAL_WINDOW_MS / 1000) as i32;
        self.log_default(EventCode::NetTrialStarted, window_s, 0, &addr[..n]);
        true
    }

    /// Without its record the next boot would run the new settings without a trial: the stored
    /// config (`cfg` with the settings in use) goes back, and no record of an earlier change of
    /// this boot is left either.
    fn revert_unstored(&mut self, cfg: &Config) {
        self.host.clear_net_trial();
        apply_net_trial_fields(&mut self.net, &self.armed_prev);
        let mut addr = [0u8; 16];
        let n = format_net_address(&self.armed_prev, &mut addr);
        let ok = Self::store_net_fields(self.heap, &mut self.host, Some(cfg), &self.armed_prev);
        self.log_default(
            EventCode::NetTrialReverted,
            NetTrialRevert::NotStored as i32,
            if ok { 0 } else { -1 },
            &addr[..n],
        );
    }

    fn save_trial_record(&mut self, r: &NetTrialRecord) -> bool {
        let mut blob = [0u8; NET_TRIAL_BLOB_MAX];
        let n = encode_net_trial(r, &mut blob);
        self.host.save_net_trial_blob(&blob[..n])
    }

    /// Boot, before the interfaces start: the previous fields go into `cfg` and the stored
    /// config; the record is erased only when that was stored (a failed persist reverts again
    /// at the next boot).
    fn revert_at_boot(&mut self, cfg: &mut Config, previous: &NetConfig, reason: NetTrialRevert) {
        apply_net_trial_fields(&mut cfg.net, previous);
        let mut addr = [0u8; 16];
        let n = format_net_address(previous, &mut addr);
        let ok = self.host.apply_config(cfg);
        if ok {
            self.host.clear_net_trial();
        }
        self.log_default(
            EventCode::NetTrialReverted,
            reason as i32,
            if ok { 0 } else { -1 },
            &addr[..n],
        );
    }

    /// Boot: a record Armed by the previous boot starts the trial; a Running one (that boot
    /// ended during the trial) is reverted at once, before the interfaces start.
    fn begin_trial(&mut self, cfg: &mut Config, now_ms: u32) {
        let mut blob = [0u8; NET_TRIAL_BLOB_MAX];
        let n = self.host.load_net_trial_blob(&mut blob);
        let mut rec = NetTrialRecord::default();
        let have = decode_net_trial(blob.get(..n).unwrap_or_default(), &mut rec);
        if n != 0 && !have {
            self.host.clear_net_trial();
        }
        match net_trial_at_boot(have.then_some(&rec), &cfg.net) {
            NetTrialBoot::None => {}
            NetTrialBoot::Stale => self.host.clear_net_trial(),
            NetTrialBoot::Start => {
                rec.state = NetTrialState::Running;
                if self.save_trial_record(&rec) {
                    self.trial_prev = rec.previous;
                    self.trial.start(now_ms);
                } else {
                    // With the record still Armed, a boot that ends during the trial or a revert
                    // that cannot be stored would start the trial again instead of reverting
                    // it: it does not run.
                    self.revert_at_boot(cfg, &rec.previous, NetTrialRevert::NotStored);
                }
            }
            NetTrialBoot::RevertNow => {
                self.revert_at_boot(cfg, &rec.previous, NetTrialRevert::Interrupted);
            }
        }
    }

    fn publish_health(&self, now_ms: u32) {
        let age = self.reach.evidence_age_ms(now_ms);
        let h = NetHealthInfo {
            ip_up: self.reach.ip_up(),
            reachable: self.reach.reachable(now_ms),
            proven: self.reach.proven(),
            ping_armed: self.reach.armed(),
            evidence: self.reach.last_evidence(),
            evidence_age_s: if age == u32::MAX {
                u32::MAX
            } else {
                age / 1000
            },
            iface_restarts: self.watchdog.interface_restarts(),
            trial_active: self.trial.active(),
            trial_remaining_s: self.trial.remaining_ms(now_ms) / 1000,
        };
        *lock(&self.shared.health) = h;
        self.shared
            .last_sync
            .store(self.sntp.last_sync_epoch(), Ordering::SeqCst);
    }

    fn keep_time(&mut self, t: &TimeConfig) {
        copy_string(&mut self.time.ntp_server, &t.ntp_server);
        copy_string(&mut self.time.tz_posix, &t.tz_posix);
    }

    fn keep(&mut self, cfg: &Config) {
        self.net.clone_from(&cfg.net);
        self.keep_time(&cfg.time);
        copy_string(&mut self.station, &cfg.station);
    }

    // ---------------------------------------------------------------- entry points

    /// Starts the interfaces per config: Ethernet unless the interface is WiFi, WiFi at once
    /// only with the interface WiFi (auto starts it in [`Net::service`] as the fallback). Also
    /// SNTP when a server is set, and TZ. May put the previous network settings back into `cfg`
    /// (an interrupted network trial).
    pub fn begin(&mut self, cfg: &mut Config) {
        self.begin_trial(cfg, self.clock.now_ms());
        self.keep(cfg);
        // DHCP needs a host name; the station name may hold spaces and UTF-8
        let mut name = [0u8; STATION_NAME_MAX + 1];
        let n = build_hostname(&cfg.station, &mut name);
        copy_string(&mut self.hostname, &name[..n]);
        self.static_ip = !cfg.net.dhcp;
        self.watchdog.configure(cfg.net.reconnect_timeout_min);
        self.watchdog
            .set_restarts_in_outage(self.load_outage_restarts());
        if cfg.net.iface != NetInterface::Wifi {
            self.eth_started = self.eth.begin(&ip_setup(&self.hostname, &self.net));
            if !self.eth_started {
                self.log_default(EventCode::NetDown, 1, 0, ETH_INIT_FAILED);
            }
        }
        if cfg.net.iface == NetInterface::Wifi && self.wifi_wanted() {
            self.start_wifi();
        }
        self.apply_time();
        self.clock_ref_epoch = self.wall.epoch();
        self.clock_ref_ms = self.clock.now_ms();
        self.publish_health(self.clock.now_ms());
    }

    /// App thread, every second: the state (NetUp/NetDown), WiFi (with the interface auto the
    /// fallback after [`WIFI_FALLBACK_MS`] without an Ethernet address, at once when the
    /// Ethernet driver did not start, off again once Ethernet has an address; connects with a
    /// back-off of 5 s .. 60 s while WiFi has no address), the end-to-end reachability (gateway
    /// probe every 60 s, evidence, NetUnreachable / NetReachable), the network trial and the
    /// watchdog: an interface restart after `reconnect_timeout_min`, then the ESP restart
    /// (reason 2) with a wait growing per restart of one outage. `mqtt_connected`: the MQTT
    /// session is up (proof that the network works end to end).
    pub fn service(&mut self, now_ms: u32, mqtt_connected: bool) {
        self.retry_wifi_once();
        self.refresh_info(now_ms);
        let synced = self.check_time_sync(now_ms);
        let eth = self.eth_up();

        // Ethernet preferred: WiFi off while Ethernet has an address (interface auto)
        if self.net.iface == NetInterface::Auto && eth && self.wifi_started {
            self.stop_wifi();
        }
        if eth || !self.eth_started {
            self.eth_down_known = false;
        } else if !self.eth_down_known {
            self.eth_down_known = true;
            self.eth_down_since_ms = now_ms;
        }
        // WiFi when it is the configured interface, or the auto fallback after
        // WIFI_FALLBACK_MS without Ethernet (or when Ethernet failed to start)
        let wifi_needed = self.wifi_wanted()
            && !eth
            && (self.net.iface == NetInterface::Wifi
                || !self.eth_started
                || (self.eth_down_known
                    && elapsed_ms(now_ms, self.eth_down_since_ms) >= WIFI_FALLBACK_MS));
        if self.wifi.up() || !wifi_needed {
            self.wifi_backoff.reset();
        } else if self.wifi_backoff.due(now_ms) {
            self.start_wifi();
            self.wifi_backoff.on_failure(now_ms); // counts as failed until WiFi has an address
        }

        let i = self.shared.info();
        let up = i.state != NetState::Down;

        // end-to-end reachability: evidence, gateway probe, Lost/Regained
        self.reach.update(up, i.gateway, now_ms);
        self.note_evidence(now_ms, mqtt_connected, synced);
        self.end_probe(now_ms);
        if self.reach.probe_due(now_ms) && self.probe_gateway(i.gateway, now_ms) {
            self.reach.on_probe_sent(now_ms);
        }
        self.report_reachability(now_ms);

        self.service_trial(now_ms, up);

        match self.watchdog.update(self.reach.reachable(now_ms), now_ms) {
            NetWatchdogAction::RestartInterface => {
                let mask = self.restart_interface();
                if mask != 0 {
                    let outage_s = (self.watchdog.outage_ms(now_ms) / 1000) as i32;
                    self.log_default(EventCode::NetInterfaceRestart, outage_s, mask, b"");
                }
            }
            NetWatchdogAction::RestartEsp => {
                let outage_min = (self.watchdog.outage_ms(now_ms) / 60_000) as i32;
                self.host.request_restart(
                    RebootReason::NetWatchdog as u8,
                    RESTART_DELAY_MS,
                    outage_min,
                );
            }
            NetWatchdogAction::None => {}
        }
        let restarts = self.watchdog.restarts_in_outage();
        if restarts != self.load_outage_restarts() {
            self.store_outage_restarts(restarts);
        }
        self.publish_health(now_ms);
    }

    /// Applies changed settings. TZ and the NTP server apply live; a change of the restart
    /// reasons of `config_restart_reasons` (interface, address, WiFi credentials in use, host
    /// name) restarts the ESP after the HTTP response, because a network driver is not started
    /// twice; with jumper X20 fitted the ESP restart also resets the STM. A change of the trial
    /// fields runs on trial: its record is stored before the restart, and a change saved during
    /// a running trial confirms that trial first (208 arg2 1). A record that cannot be stored
    /// puts the settings in use back into the stored config at once (209 reason 5).
    pub fn reconfigure(&mut self, cfg: &Config) {
        let mut restart = config_restart_reasons_from(&self.net, &self.station, cfg);
        let unstored = net_trial_required(&self.net, &cfg.net) && !self.arm_trial(cfg);
        let time_changed =
            cfg.time.ntp_server != self.time.ntp_server || cfg.time.tz_posix != self.time.tz_posix;
        self.keep(cfg);
        if unstored {
            self.revert_unstored(cfg);
            restart &= !RESTART_NETWORK;
        }
        self.watchdog.configure(cfg.net.reconnect_timeout_min);
        if time_changed {
            self.apply_time();
        }
        if restart != 0 {
            self.host
                .request_restart(RebootReason::User as u8, RECONFIGURE_RESTART_DELAY_MS, 0);
        }
    }
}

#[cfg(test)]
mod support;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_boots;
#[cfg(test)]
mod tests_edges;
#[cfg(test)]
mod tests_eq;
