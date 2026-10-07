//! Support of the net suites: the [`NetHost`] fake (the C++ sibling fakes of storage, logger and
//! ota), the rig of one boot, and the helpers of the C++ suites (`tick`, `run`, the configs).
#![allow(clippy::large_stack_frames)]

use std::sync::{Arc, Mutex, MutexGuard};

use vdm_esp_core::common::copy_string;
use vdm_esp_core::config::{Config, NetInterface};
use vdm_esp_core::event_log::{event_code_name, Event, EventCode};
use vdm_esp_core::net_trial::{
    decode_net_trial, encode_net_trial, net_trial_fields_crc, NetTrialRecord, NetTrialState,
    NET_TRIAL_BLOB_MAX,
};

use super::{Net, NetHost, NetPorts, NetShared, RTC_MAGIC, RTC_RECORD_LEN};
use crate::port::{Clock, IpInfo};
use crate::testkit::board::TestPlatform;
use crate::testkit::{Device, FakeBoard, Journal};

/// IPv4 in lwIP order (first octet in the low byte), as `IPAddress(a, b, c, d)`.
pub(super) const fn ip(a: u8, b: u8, c: u8, d: u8) -> u32 {
    u32::from_le_bytes([a, b, c, d])
}

pub(super) const IP: u32 = ip(192, 168, 1, 20);
pub(super) const NEW_IP: u32 = ip(192, 168, 1, 50);
/// 15 characters, the longest address text.
pub(super) const LONG_IP: u32 = ip(192, 168, 100, 200);
pub(super) const GW: u32 = ip(192, 168, 1, 1);
pub(super) const MASK: u32 = ip(255, 255, 255, 0);

/// The place of net's record in the RTC layout of the tests (after the boot guard mirror).
pub(super) const RTC_OFFSET: usize = 28;

/// Defaults with the station "Heating Floor".
pub(super) fn config() -> Config {
    let mut c = Config::default();
    copy_string(&mut c.station, b"Heating Floor");
    c
}

/// Ethernet with a static address.
pub(super) fn static_config(addr: u32) -> Config {
    let mut c = config();
    c.net.iface = NetInterface::Ethernet;
    c.net.dhcp = false;
    c.net.ip = addr;
    c.net.gateway = GW;
    c.net.mask = MASK;
    c
}

/// The interface WiFi with `ssid`.
pub(super) fn wifi_config(ssid: &[u8]) -> Config {
    let mut c = config();
    c.net.iface = NetInterface::Wifi;
    copy_string(&mut c.net.ssid, ssid);
    c
}

/// One `ota::requestRestart` call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct RestartRequest {
    pub(super) reason: u8,
    pub(super) delay_ms: u32,
    pub(super) detail: i32,
}

type Hook = Box<dyn FnMut() + Send>;

/// Scripted results and recorded calls of the host (C++ `sib::storage()`, `sib::logger()`,
/// `sib::ota()`).
pub(super) struct HostState {
    // scripted
    /// `get_config` answers this (the active config).
    pub(super) active: Box<Config>,
    pub(super) apply_result: bool,
    pub(super) save_net_trial_result: bool,
    /// The stored trial record.
    pub(super) net_trial: Vec<u8>,
    /// Runs at every `apply_config` (order checks).
    pub(super) on_apply: Option<Hook>,
    // recorded
    pub(super) events: Vec<Event>,
    pub(super) restart_requests: Vec<RestartRequest>,
    /// Every `apply_config` attempt.
    pub(super) applied: Vec<Box<Config>>,
    pub(super) net_trial_saves: u32,
    pub(super) net_trial_clears: u32,
    pub(super) get_configs: u32,
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            active: Box::default(),
            apply_result: true,
            save_net_trial_result: true,
            net_trial: Vec::new(),
            on_apply: None,
            events: Vec::new(),
            restart_requests: Vec::new(),
            applied: Vec::new(),
            net_trial_saves: 0,
            net_trial_clears: 0,
            get_configs: 0,
        }
    }
}

/// The host of the tests; clones share the state (the test keeps one, `Net` owns one).
#[derive(Clone)]
pub(super) struct FakeNetHost {
    state: Arc<Mutex<HostState>>,
    journal: Journal,
}

impl FakeNetHost {
    pub(super) fn new(journal: Journal) -> Self {
        FakeNetHost {
            state: Arc::default(),
            journal,
        }
    }
    pub(super) fn state(&self) -> MutexGuard<'_, HostState> {
        crate::testkit::lock(&self.state)
    }
    /// The logged events of `code`, in order.
    pub(super) fn with_code(&self, code: EventCode) -> Vec<Event> {
        self.state()
            .events
            .iter()
            .filter(|e| e.code == code)
            .cloned()
            .collect()
    }
    /// An event of `code` was logged.
    pub(super) fn has(&self, code: EventCode) -> bool {
        !self.with_code(code).is_empty()
    }
    /// The first event of `code` (panics without one, like the C++ `.at(0)`).
    pub(super) fn first(&self, code: EventCode) -> Event {
        match self.with_code(code).first() {
            Some(e) => e.clone(),
            None => panic!("no {code:?} event"),
        }
    }
    pub(super) fn restarts(&self) -> Vec<RestartRequest> {
        self.state().restart_requests.clone()
    }
    /// The stored trial record (C++ `storedRecord()`).
    pub(super) fn stored_record(&self) -> NetTrialRecord {
        let mut r = NetTrialRecord::default();
        assert!(
            decode_net_trial(&self.state().net_trial, &mut r),
            "no decodable trial record stored"
        );
        r
    }
    /// The last applied config.
    pub(super) fn last_applied(&self) -> Box<Config> {
        match self.state().applied.last() {
            Some(c) => c.clone(),
            None => panic!("no config applied"),
        }
    }
}

impl NetHost for FakeNetHost {
    fn log(&mut self, e: &Event) {
        self.state().events.push(e.clone());
        self.journal
            .note(format!("logger.log {}", event_code_name(e.code)));
    }
    fn request_restart(&mut self, reason: u8, delay_ms: u32, detail: i32) {
        self.state().restart_requests.push(RestartRequest {
            reason,
            delay_ms,
            detail,
        });
        self.journal
            .note(format!("ota.requestRestart {reason} {delay_ms}"));
    }
    fn get_config(&mut self, out: &mut Config) {
        let mut s = self.state();
        s.get_configs += 1;
        out.clone_from(&s.active);
    }
    fn apply_config(&mut self, c: &Config) -> bool {
        let hook = self.state().on_apply.take();
        if let Some(mut h) = hook {
            h();
            self.state().on_apply = Some(h);
        }
        let mut s = self.state();
        s.applied.push(Box::new(c.clone()));
        self.journal.note("storage.applyConfig");
        if !s.apply_result {
            return false;
        }
        s.active.as_mut().clone_from(c);
        true
    }
    fn load_net_trial_blob(&mut self, out: &mut [u8]) -> usize {
        let s = self.state();
        let t = &s.net_trial;
        if t.is_empty() || t.len() > out.len() {
            return 0;
        }
        out[..t.len()].copy_from_slice(t);
        t.len()
    }
    fn save_net_trial_blob(&mut self, data: &[u8]) -> bool {
        let mut s = self.state();
        s.net_trial_saves += 1;
        if !s.save_net_trial_result || data.is_empty() {
            return false;
        }
        s.net_trial = data.to_vec();
        true
    }
    fn clear_net_trial(&mut self) {
        let mut s = self.state();
        s.net_trial_clears += 1;
        s.net_trial.clear();
    }
}

/// The blob of a trial record.
pub(super) fn blob(r: &NetTrialRecord) -> Vec<u8> {
    let mut b = vec![0u8; NET_TRIAL_BLOB_MAX];
    let n = encode_net_trial(r, &mut b);
    b.truncate(n);
    b
}

/// One boot of a board: its ports, the shared net data and the host.
pub(super) struct Rig {
    pub(super) board: FakeBoard,
    pub(super) dev: Device,
    pub(super) shared: NetShared,
    pub(super) host: FakeNetHost,
}

impl Rig {
    /// A fresh board, first boot.
    pub(super) fn new() -> Self {
        Self::boot(&FakeBoard::new(), None)
    }

    /// The next boot of `board`; the host starts with the stored config and trial record of
    /// `storage` (what an earlier boot left in NVS), its records empty.
    pub(super) fn boot(board: &FakeBoard, storage: Option<&FakeNetHost>) -> Self {
        let dev = board.boot();
        let host = FakeNetHost::new(dev.journal.clone());
        if let Some(old) = storage {
            let (active, trial) = {
                let s = old.state();
                (s.active.clone(), s.net_trial.clone())
            };
            let mut s = host.state();
            s.active = active;
            s.net_trial = trial;
        }
        Rig {
            board: board.clone(),
            dev,
            shared: NetShared::new(),
            host,
        }
    }

    /// The net of this boot.
    pub(super) fn net(&self) -> Net<'_, TestPlatform, FakeNetHost> {
        Net::new(
            NetPorts {
                clock: &self.dev.clock,
                wall: &self.dev.wall,
                rtc: &self.dev.rtc,
                heap: &self.dev.heap,
                eth: self.dev.eth.clone(),
                wifi: self.dev.wifi.clone(),
                sntp: self.dev.sntp.clone(),
                pinger: self.dev.pinger.clone(),
            },
            &self.shared,
            self.host.clone(),
            RTC_OFFSET,
        )
    }

    /// Ethernet link with an address (DHCP: GOT_IP; static: the link is enough).
    pub(super) fn ethernet_up(&self, addr: u32, gateway: u32) {
        self.dev.eth.got_ip(IpInfo {
            ip: addr,
            mask: MASK,
            gateway,
            dns: 0,
        });
    }

    /// One pass of the app thread at `t`, then the ping task answers a started probe.
    pub(super) fn tick(&self, net: &mut Net<'_, TestPlatform, FakeNetHost>, t: u64, mqtt: bool) {
        self.dev.clock.set_ms(t);
        net.service(self.dev.clock.now_ms(), mqtt);
        self.dev.pinger.step();
    }

    /// Passes every `step` ms in [from, to]; stops after the first restart request.
    pub(super) fn run(
        &self,
        net: &mut Net<'_, TestPlatform, FakeNetHost>,
        from: u64,
        to: u64,
        step: u64,
    ) {
        let mut t = from;
        while t <= to {
            self.tick(net, t, false);
            if !self.host.state().restart_requests.is_empty() {
                return;
            }
            t += step;
        }
    }

    /// The service pass at `t` without the ping task.
    pub(super) fn service(&self, net: &mut Net<'_, TestPlatform, FakeNetHost>, t: u64) {
        self.dev.clock.set_ms(t);
        net.service(self.dev.clock.now_ms(), false);
    }

    /// A trial record for `on_trial` with the previous settings of `prev`, stored as the
    /// previous boot left it, and the active config (C++ `bootWithRecord` without the begin).
    pub(super) fn store_record(&self, st: NetTrialState, prev: &Config, on_trial: &Config) {
        let r = NetTrialRecord {
            state: st,
            previous: prev.net.clone(),
            trial_crc: net_trial_fields_crc(&on_trial.net),
        };
        let mut s = self.host.state();
        s.net_trial = blob(&r);
        *s.active = on_trial.clone();
    }

    /// The RTC record of net: (check word, count).
    pub(super) fn rtc_record(&self) -> (u32, u32) {
        let b = self.dev.rtc.snapshot();
        let r = &b[RTC_OFFSET..RTC_OFFSET + RTC_RECORD_LEN];
        (
            u32::from_le_bytes([r[0], r[1], r[2], r[3]]),
            u32::from_le_bytes([r[4], r[5], r[6], r[7]]),
        )
    }

    /// Writes the RTC record of net.
    pub(super) fn set_rtc_record(&self, magic: u32, count: u32) {
        let mut r = [0u8; RTC_RECORD_LEN];
        r[..4].copy_from_slice(&magic.to_le_bytes());
        r[4..].copy_from_slice(&count.to_le_bytes());
        crate::port::Rtc::store(&self.dev.rtc, RTC_OFFSET, &r);
    }
}

/// The check word of a valid record.
pub(super) const MAGIC: u32 = RTC_MAGIC;
