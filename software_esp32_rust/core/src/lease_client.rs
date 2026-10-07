//! Failsafe lease, ESP side: tells the STM whether the regulator (MQTT/HA) is alive (slhbt),
//! keeps the STM's lease timeout and failsafe positions equal to the ESP config (glcfg / slcfg /
//! sfspo) on protocol 3, and emulates the lease on protocols 1/2 by naming the valves the ESP
//! drives to their failsafe position itself. Reports the lease status and its events (port of
//! `vdm/lease_client.h`). Hardware-free.

use crate::common::{elapsed_ms, ALL_VALVES, NO_VALVE, VALVE_COUNT};
use crate::config::{crc32, Config};
use crate::event_log::{event_default_severity, make_event, Event, EventCode};
use crate::failsafe::{
    LeaseMode, LeaseState, LeaseStatus, RegulatorCause, FAILSAFE_HOLD, FAILSAFE_PCT_DEFAULT,
    FAILSAFE_TIMEOUT_DEFAULT_MIN,
};
use crate::link_policy::Outcome;
use crate::stm_codec::{
    build_get_lease_config, build_heartbeat, build_set_failsafe, build_set_lease_timeout, Cmd,
    LeaseConfigReply, Reply, RequestLine, StmStatus,
};

const REASON_NO_REPLY: u8 = 1;
const REASON_REJECTED: u8 = 2;
const REASON_DIFFERS: u8 = 3;
const SOURCE_STM: i32 = 1;
const SOURCE_ESP: i32 = 2;
const LEASE_MAGIC: [u8; 4] = *b"VDLE";

/// What the STM must hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseConfig {
    pub timeout_min: u16,
    pub failsafe_pct: [u8; VALVE_COUNT as usize],
}

impl Default for LeaseConfig {
    fn default() -> Self {
        Self {
            timeout_min: FAILSAFE_TIMEOUT_DEFAULT_MIN,
            failsafe_pct: [FAILSAFE_PCT_DEFAULT; VALVE_COUNT as usize],
        }
    }
}

/// timeout_min = cfg.failsafe.timeout_min; failsafe_pct[v] = cfg.valves[v].failsafe_pct for
/// active valves, [`FAILSAFE_HOLD`] for inactive ones (the ESP never moves an inactive valve).
/// The C++ writes every field of its out-parameter, so it is the return value.
pub fn effective_lease_config(cfg: &Config) -> LeaseConfig {
    let mut out = LeaseConfig {
        timeout_min: cfg.failsafe.timeout_min,
        ..LeaseConfig::default()
    };
    for (pct, v) in out.failsafe_pct.iter_mut().zip(cfg.valves.iter()) {
        *pct = if v.active {
            v.failsafe_pct
        } else {
            FAILSAFE_HOLD
        };
    }
    out
}

fn same_as_stm(c: &LeaseConfig, r: &LeaseConfigReply) -> bool {
    c.timeout_min == r.timeout_min && c.failsafe_pct == r.failsafe_pct
}

/// Bounded event sink for one call (no cap of its own: at most `out.len()` events; a null
/// `out` is the empty slice).
struct Sink<'a> {
    out: &'a mut [Event],
    n: usize,
}

impl<'a> Sink<'a> {
    fn new(out: &'a mut [Event]) -> Self {
        Self { out, n: 0 }
    }

    fn add(&mut self, code: EventCode, a1: i32, a2: i32) {
        if let Some(slot) = self.out.get_mut(self.n) {
            *slot = make_event(code, event_default_severity(code), NO_VALVE, a1, a2, b"");
            self.n += 1;
        }
    }
}

/// What a request of the lease client is (C++ `LeaseClient::Kind`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeaseClientKind {
    None,
    Heartbeat,
    Read,
    Push,
    Verify,
}

/// The ESP emulation across ESP software restarts (RTC record; C++ `LeaseClient::Snapshot`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeaseClientSnapshot {
    /// regulator dead, lost_elapsed_ms meaningful
    pub lost: bool,
    /// the ESP emulation drives valves
    pub active: bool,
    /// emulated_mask()
    pub mask: u16,
    pub lost_elapsed_ms: u32,
}

#[derive(Clone, Debug)]
pub struct LeaseClient {
    config: LeaseConfig,
    trusted: bool,
    proto: u8,
    last_proto: u8,

    // regulator
    reg_init: bool,
    eff_alive: bool,
    cause: RegulatorCause,
    last_seq: u32,
    lost_valid: bool,
    lost_since_ms: u32,
    alive_since_valid: bool,
    alive_since_ms: u32,
    lost_reported: bool,
    back_pending: bool,
    back_seconds: u32,

    // requests
    in_flight: LeaseClientKind,
    in_flight_req: RequestLine,
    in_flight_at_ms: u32,
    /// in this session
    hb_sent: bool,
    /// a heartbeat of this session was answered
    hb_ok: bool,
    /// completion of the last heartbeat (meaningful once hb_sent)
    hb_done_ms: u32,
    last_sent_alive: bool,

    // config sync
    cfg_step: LeaseClientKind,
    /// else due cfg_wait_ms after cfg_wait_from_ms
    cfg_due_now: bool,
    cfg_wait_from_ms: u32,
    cfg_wait_ms: u32,
    push_timeout: bool,
    push_all: bool,
    push_mask: u16,
    synced: bool,
    config_failed: bool,
    attempts: u8,
    fail_event_pending: bool,
    fail_reason: u8,

    // STM lease (protocol 3)
    stm_lease: LeaseState,
    stm_remain_s: u32,
    stm_remain_at_ms: u32,
    stm_mask: u16,
    have_cfg_events: bool,
    cfg_events: u32,

    // failsafe events
    stm_fs_reported: bool,
    stm_fs_since_ms: u32,
    emu_reported: bool,
    emu_since_ms: u32,
    restored_active: bool,
    restored_mask: u16,
}

impl Default for LeaseClient {
    fn default() -> Self {
        Self {
            config: LeaseConfig::default(),
            trusted: true,
            proto: 0,
            last_proto: 0,
            reg_init: false,
            eff_alive: false,
            cause: RegulatorCause::BrokerDown,
            last_seq: 0,
            lost_valid: false,
            lost_since_ms: 0,
            alive_since_valid: false,
            alive_since_ms: 0,
            lost_reported: false,
            back_pending: false,
            back_seconds: 0,
            in_flight: LeaseClientKind::None,
            in_flight_req: RequestLine::default(),
            in_flight_at_ms: 0,
            hb_sent: false,
            hb_ok: false,
            hb_done_ms: 0,
            last_sent_alive: false,
            cfg_step: LeaseClientKind::Read,
            cfg_due_now: true,
            cfg_wait_from_ms: 0,
            cfg_wait_ms: 0,
            push_timeout: false,
            push_all: false,
            push_mask: 0,
            synced: false,
            config_failed: false,
            attempts: 0,
            fail_event_pending: false,
            fail_reason: 0,
            stm_lease: LeaseState::Off,
            stm_remain_s: 0,
            stm_remain_at_ms: 0,
            stm_mask: 0,
            have_cfg_events: false,
            cfg_events: 0,
            stm_fs_reported: false,
            stm_fs_since_ms: 0,
            emu_reported: false,
            emu_since_ms: 0,
            restored_active: false,
            restored_mask: 0,
        }
    }
}

impl LeaseClient {
    /// slhbt period
    pub const HEARTBEAT_MS: u32 = 60_000;
    /// glcfg compare while synced
    pub const CONFIG_CHECK_MS: u32 = 600_000;
    /// after a failed attempt
    pub const CONFIG_RETRY_MS: u32 = 60_000;
    /// then LeaseConfigFailed; retries every CONFIG_CHECK_MS
    pub const CONFIG_MAX_ATTEMPTS: u8 = 3;
    /// RegulatorLost after this long without the regulator
    pub const REGULATOR_EVENT_MS: u32 = 60_000;
    /// a handed-out request without completion counts as Timeout
    pub const LOST_REQUEST_MS: u32 = 10_000;
    /// during a failsafe: alive this long before renewing
    pub const RENEW_HOLD_MS: u32 = 120_000;

    // ------------------------------------------------------------ inputs

    /// A change re-reads glcfg at once (protocol 3).
    pub fn set_config(&mut self, c: &LeaseConfig) {
        if *c == self.config {
            return;
        }
        self.config = *c;
        self.attempts = 0;
        self.reread();
    }

    pub fn config(&self) -> &LeaseConfig {
        &self.config
    }

    /// False while the ESP runs on a default config nobody saved: glcfg is still compared,
    /// slcfg/sfspo are never sent. Becoming true re-reads.
    pub fn set_config_trusted(&mut self, trusted: bool) {
        if trusted == self.trusted {
            return;
        }
        self.trusted = trusted;
        if trusted {
            self.reread();
        }
    }

    /// 0..3 (higher counts as 3). 0 pauses (the mode stays what the last known protocol gave);
    /// a change to 3 starts a session. `_now_ms` is unused, as in C++.
    pub fn set_protocol(&mut self, proto: u8, _now_ms: u32) {
        let proto = proto.min(3);
        if proto == self.proto {
            return;
        }
        self.proto = proto;
        if proto != 0 {
            self.last_proto = proto;
        }
        self.in_flight = LeaseClientKind::None;
        if proto == 3 {
            self.start_session();
        }
    }

    fn start_session(&mut self) {
        self.in_flight = LeaseClientKind::None;
        self.hb_sent = false;
        self.hb_ok = false;
        self.have_cfg_events = false;
        self.attempts = 0;
        self.reread();
    }

    /// New STM session: heartbeat due, glcfg re-read.
    pub fn on_stm_reboot(&mut self) {
        self.start_session();
    }

    fn reread(&mut self) {
        self.synced = false;
        self.cfg_step = LeaseClientKind::Read;
        self.cfg_due_now = true;
        // The push flags are read in the Push step only; on_config_reply() sets them before it.
        // A config request in flight belongs to the old round: its result is dropped.
        if self.in_flight != LeaseClientKind::Heartbeat {
            self.in_flight = LeaseClientKind::None;
        }
    }

    /// Once per second from the MQTT view. The first call starts the lost timer when the cause
    /// is dead (at ESP boot the regulator counts as lost until MQTT reports it alive). While a
    /// failsafe is active (STM lease expired or ESP emulation) a dead -> alive change counts
    /// only after [`RENEW_HOLD_MS`](Self::RENEW_HOLD_MS) of continuous life, or at once when
    /// `command_seq` changed (an MQTT command arrived).
    pub fn set_regulator(&mut self, c: RegulatorCause, command_seq: u32, now_ms: u32) {
        let seq_changed = self.reg_init && command_seq != self.last_seq;
        self.last_seq = command_seq;
        if c != RegulatorCause::Alive {
            self.alive_since_valid = false;
            self.cause = c;
            if self.eff_alive || !self.lost_valid {
                self.lost_valid = true;
                self.lost_since_ms = now_ms;
                self.lost_reported = false;
            }
            self.eff_alive = false;
            self.reg_init = true;
            return;
        }
        self.reg_init = true;
        if self.eff_alive {
            return;
        }
        if !self.alive_since_valid {
            self.alive_since_valid = true;
            self.alive_since_ms = now_ms;
        }
        if self.failsafe_active(now_ms)
            && !seq_changed
            && elapsed_ms(now_ms, self.alive_since_ms) < Self::RENEW_HOLD_MS
        {
            return; // a short blip does not end a failsafe
        }
        if self.lost_reported {
            self.back_pending = true;
            self.back_seconds = elapsed_ms(now_ms, self.lost_since_ms) / 1000;
        }
        // lost_reported may stay set: the next loss (eff_alive) clears it before tick() reads it
        self.eff_alive = true;
        self.cause = RegulatorCause::Alive;
    }

    /// gstax (protocol 3): lease state and mask; a lease_timeout_min that drifted from a synced
    /// config, or a changed cfg_events, re-reads glcfg.
    pub fn on_status(&mut self, s: &StmStatus, now_ms: u32) {
        if !s.v3 {
            return;
        }
        self.stm_lease = s.lease;
        self.stm_remain_s = s.lease_remain_s;
        self.stm_remain_at_ms = now_ms;
        self.stm_mask = s.failsafe_mask;
        let drift = self.synced && s.lease_timeout_min != self.config.timeout_min;
        let repaired = self.have_cfg_events && s.cfg_events != self.cfg_events;
        self.have_cfg_events = true;
        self.cfg_events = s.cfg_events;
        if drift || repaired {
            self.reread();
        }
    }

    // ------------------------------------------------------------ state

    fn mode(&self) -> LeaseMode {
        let p = if self.proto != 0 {
            self.proto
        } else {
            self.last_proto
        };
        if p == 3 {
            return LeaseMode::Stm;
        }
        if p == 0 {
            return if self.restored_active {
                LeaseMode::Emulated
            } else {
                LeaseMode::None
            };
        }
        if self.config.timeout_min > 0 {
            LeaseMode::Emulated
        } else {
            LeaseMode::None
        }
    }

    fn emulation_active(&self, now_ms: u32) -> bool {
        if self.proto == 0 && self.last_proto == 0 {
            return self.restored_active;
        }
        if self.mode() != LeaseMode::Emulated || self.eff_alive || !self.lost_valid {
            return false;
        }
        elapsed_ms(now_ms, self.lost_since_ms) >= u32::from(self.config.timeout_min) * 60_000
    }

    fn failsafe_active(&self, now_ms: u32) -> bool {
        if self.mode() == LeaseMode::Stm {
            return self.stm_lease == LeaseState::Expired;
        }
        self.emulation_active(now_ms)
    }

    /// Valves with a failsafe position other than hold. The bits are disjoint: their sum is the
    /// C++ `|`.
    fn hold_mask(&self) -> u16 {
        self.config
            .failsafe_pct
            .iter()
            .enumerate()
            .filter(|&(_, &pct)| pct != FAILSAFE_HOLD)
            .map(|(v, _)| 1u16 << v)
            .sum()
    }

    /// Valves the ESP drives to their failsafe position (protocols 1/2): those with a
    /// failsafe_pct other than hold once the regulator has been lost for timeout_min; 0 on
    /// protocol 3, with timeout 0 and while the regulator is alive.
    pub fn emulated_mask(&self, now_ms: u32) -> u16 {
        if !self.emulation_active(now_ms) {
            return 0;
        }
        if self.proto == 0 && self.last_proto == 0 {
            return self.restored_mask;
        }
        self.hold_mask()
    }

    pub fn status(&self, now_ms: u32) -> LeaseStatus {
        let mut s = LeaseStatus {
            mode: self.mode(),
            timeout_min: self.config.timeout_min,
            regulator: self.cause,
            regulator_lost_s: if !self.eff_alive && self.lost_valid {
                elapsed_ms(now_ms, self.lost_since_ms) / 1000
            } else {
                0
            },
            config_synced: self.synced,
            config_failed: self.config_failed,
            config_trusted: self.trusted,
            ..LeaseStatus::default()
        };
        match s.mode {
            LeaseMode::Stm => {
                s.state = self.stm_lease;
                s.failsafe_mask = self.stm_mask;
                if self.stm_lease == LeaseState::Running {
                    let aged = elapsed_ms(now_ms, self.stm_remain_at_ms) / 1000;
                    s.remain_s = self.stm_remain_s.saturating_sub(aged);
                }
            }
            LeaseMode::Emulated => {
                let active = self.emulation_active(now_ms);
                s.state = if active {
                    LeaseState::Expired
                } else {
                    LeaseState::Running
                };
                s.failsafe_mask = self.emulated_mask(now_ms);
                let total = u32::from(self.config.timeout_min) * 60;
                if !active {
                    s.remain_s = total.saturating_sub(s.regulator_lost_s);
                }
            }
            LeaseMode::None => {}
        }
        s
    }

    // ------------------------------------------------------------ requests

    /// Next request, to be queued at Priority::Config; None when nothing is due. One request in
    /// flight at most; a heartbeat goes before any config request, config requests only after
    /// the session's first heartbeat was answered.
    ///
    /// A config value the codec refuses (a timeout of 1..4 or above 1440 min, a percent of
    /// 101..254) gives the empty request, as the C++ `RequestLine{}` with `true`: the link
    /// refuses it and the attempt fails as a lost request (docs/rust/PORT-NOTES.md).
    pub fn next(&mut self, now_ms: u32) -> Option<RequestLine> {
        if self.proto != 3 {
            return None;
        }
        if self.in_flight != LeaseClientKind::None {
            if elapsed_ms(now_ms, self.in_flight_at_ms) < Self::LOST_REQUEST_MS {
                return None;
            }
            let lost = self.in_flight_req.clone();
            self.on_completion(&lost, Outcome::Timeout, None, now_ms);
        }
        let hb_due = !self.hb_sent
            || self.eff_alive != self.last_sent_alive
            || elapsed_ms(now_ms, self.hb_done_ms) >= Self::HEARTBEAT_MS;
        let out = if hb_due {
            self.last_sent_alive = self.eff_alive;
            self.hb_sent = true;
            self.in_flight = LeaseClientKind::Heartbeat;
            build_heartbeat(self.eff_alive)
        } else {
            if !self.hb_ok {
                return None;
            }
            if !self.cfg_due_now && elapsed_ms(now_ms, self.cfg_wait_from_ms) < self.cfg_wait_ms {
                return None;
            }
            let push = self.cfg_step == LeaseClientKind::Push;
            let line = if push && self.push_timeout {
                build_set_lease_timeout(u32::from(self.config.timeout_min))
            } else if push && self.push_all {
                build_set_failsafe(ALL_VALVES, self.config.failsafe_pct[0])
            } else if push && self.push_mask != 0 {
                // the lowest valve still to push; push_mask has bits 0..11 only
                let v = self.push_mask.trailing_zeros();
                let pct = self.config.failsafe_pct.get(v as usize).copied();
                build_set_failsafe(v as u8, pct.unwrap_or(FAILSAFE_HOLD))
            } else {
                if push {
                    self.cfg_step = LeaseClientKind::Verify;
                }
                Some(build_get_lease_config())
            };
            self.in_flight = self.cfg_step;
            line.unwrap_or_default()
        };
        self.in_flight_req = out.clone();
        self.in_flight_at_ms = now_ms;
        Some(out)
    }

    fn config_done(&mut self, now_ms: u32) {
        self.synced = true;
        self.attempts = 0;
        self.config_failed = false;
        self.cfg_step = LeaseClientKind::Read;
        self.cfg_due_now = false;
        self.cfg_wait_from_ms = now_ms;
        self.cfg_wait_ms = Self::CONFIG_CHECK_MS;
    }

    fn fail_attempt(&mut self, reason: u8, now_ms: u32) {
        self.synced = false;
        self.attempts = self.attempts.saturating_add(1);
        let exhausted = self.attempts >= Self::CONFIG_MAX_ATTEMPTS;
        if exhausted && !self.config_failed {
            self.config_failed = true;
            self.fail_event_pending = true;
            self.fail_reason = reason;
        }
        self.cfg_step = LeaseClientKind::Read;
        self.cfg_due_now = false;
        self.cfg_wait_from_ms = now_ms;
        self.cfg_wait_ms = if exhausted {
            Self::CONFIG_CHECK_MS
        } else {
            Self::CONFIG_RETRY_MS
        };
    }

    fn on_config_reply(&mut self, r: &LeaseConfigReply, verify: bool, now_ms: u32) {
        if same_as_stm(&self.config, r) {
            self.config_done(now_ms);
            return;
        }
        if verify {
            self.fail_attempt(REASON_DIFFERS, now_ms);
            return;
        }
        self.synced = false;
        if !self.trusted {
            // Defaults nobody saved must not overwrite what the STM holds.
            self.cfg_due_now = false;
            self.cfg_wait_from_ms = now_ms;
            self.cfg_wait_ms = Self::CONFIG_CHECK_MS;
            return;
        }
        self.push_timeout = r.timeout_min != self.config.timeout_min;
        let first = self.config.failsafe_pct[0];
        let mut diff: u16 = 0;
        let mut all_equal = true;
        for (v, (&stm, &want)) in r
            .failsafe_pct
            .iter()
            .zip(self.config.failsafe_pct.iter())
            .enumerate()
        {
            if stm != want {
                diff += 1 << v; // disjoint bits: + is the C++ |
            }
            if want != first {
                all_equal = false;
            }
        }
        // One sfspo 255 when every valve wants the same value and several differ.
        self.push_all = all_equal && diff.count_ones() >= 2;
        self.push_mask = if self.push_all { 0 } else { diff };
        self.cfg_step = LeaseClientKind::Push;
        self.cfg_due_now = true;
    }

    /// Completion of a request from [`next`](Self::next); `rep` is the matched reply for
    /// Ok/Rejected, else None. Other requests (another command or text) are ignored.
    pub fn on_completion(
        &mut self,
        req: &RequestLine,
        o: Outcome,
        rep: Option<&Reply>,
        now_ms: u32,
    ) {
        if self.in_flight == LeaseClientKind::None
            || req.cmd != self.in_flight_req.cmd
            || req.text != self.in_flight_req.text
        {
            return;
        }
        let kind = self.in_flight;
        self.in_flight = LeaseClientKind::None;
        let rep = rep.filter(|_| o == Outcome::Ok);
        if kind == LeaseClientKind::Heartbeat {
            self.hb_done_ms = now_ms;
            let Some(rep) = rep else {
                return;
            };
            self.hb_ok = true;
            self.stm_lease = rep.heartbeat.lease;
            self.stm_remain_s = rep.heartbeat.remain_s;
            self.stm_remain_at_ms = now_ms;
            return;
        }
        if kind == LeaseClientKind::Push {
            if rep.is_none() {
                let reason = if o == Outcome::Rejected {
                    REASON_REJECTED
                } else {
                    REASON_NO_REPLY
                };
                self.fail_attempt(reason, now_ms);
                return;
            }
            if req.cmd == Cmd::Slcfg {
                self.push_timeout = false;
            } else if req.valve == ALL_VALVES {
                self.push_all = false;
                self.push_mask = 0;
            } else {
                // C++ 1u << valve: valves 0..11 from next(); none of the 16 mask bits for the
                // NO_VALVE of an empty request (UB in C++, no bit on x86)
                let bit = 1u16.checked_shl(u32::from(req.valve)).unwrap_or(0);
                self.push_mask &= !bit;
            }
            return;
        }
        let Some(rep) = rep else {
            self.fail_attempt(REASON_NO_REPLY, now_ms);
            return;
        };
        self.on_config_reply(&rep.lease_config, kind == LeaseClientKind::Verify, now_ms);
    }

    // ------------------------------------------------------------ events

    /// Once per second: RegulatorLost/Back, FailsafeActive/Ended, LeaseConfigFailed. Writes at
    /// most `out.len()` events and returns their number; an event that does not fit is lost
    /// (as in C++, the state moves on).
    pub fn tick(&mut self, now_ms: u32, out: &mut [Event]) -> usize {
        let mut sink = Sink::new(out);
        if !self.eff_alive
            && self.lost_valid
            && !self.lost_reported
            && elapsed_ms(now_ms, self.lost_since_ms) >= Self::REGULATOR_EVENT_MS
        {
            self.lost_reported = true;
            sink.add(EventCode::RegulatorLost, self.cause as i32, 0);
        }
        if self.back_pending {
            self.back_pending = false;
            sink.add(EventCode::RegulatorBack, self.back_seconds as i32, 0);
        }
        let stm_active = self.mode() == LeaseMode::Stm && self.stm_lease == LeaseState::Expired;
        if stm_active && !self.stm_fs_reported {
            self.stm_fs_reported = true;
            self.stm_fs_since_ms = now_ms;
            sink.add(
                EventCode::FailsafeActive,
                i32::from(self.stm_mask),
                SOURCE_STM,
            );
        } else if !stm_active && self.stm_fs_reported {
            self.stm_fs_reported = false;
            let s = elapsed_ms(now_ms, self.stm_fs_since_ms) / 1000;
            sink.add(EventCode::FailsafeEnded, s as i32, SOURCE_STM);
        }
        let emu_active = self.emulation_active(now_ms);
        if emu_active && !self.emu_reported {
            self.emu_reported = true;
            self.emu_since_ms = now_ms;
            let mask = self.emulated_mask(now_ms);
            sink.add(EventCode::FailsafeActive, i32::from(mask), SOURCE_ESP);
        } else if !emu_active && self.emu_reported {
            self.emu_reported = false;
            let s = elapsed_ms(now_ms, self.emu_since_ms) / 1000;
            sink.add(EventCode::FailsafeEnded, s as i32, SOURCE_ESP);
        }
        if self.fail_event_pending {
            self.fail_event_pending = false;
            sink.add(
                EventCode::LeaseConfigFailed,
                i32::from(self.fail_reason),
                i32::from(self.attempts),
            );
        }
        sink.n
    }

    // ------------------------------------------------------------ restarts

    pub fn snapshot(&self, now_ms: u32) -> LeaseClientSnapshot {
        let lost = !self.eff_alive && self.lost_valid;
        LeaseClientSnapshot {
            lost,
            active: self.emulation_active(now_ms),
            mask: self.emulated_mask(now_ms),
            lost_elapsed_ms: if lost {
                elapsed_ms(now_ms, self.lost_since_ms)
            } else {
                0
            },
        }
    }

    /// Boot, before the first set_regulator(): a lost record continues the lost timer; an
    /// active record drives its mask at once (also before the protocol is known) and does not
    /// report failsafe_active again.
    pub fn restore(&mut self, s: &LeaseClientSnapshot, now_ms: u32) {
        if s.lost {
            self.lost_valid = true;
            self.lost_since_ms = now_ms.wrapping_sub(s.lost_elapsed_ms);
            self.lost_reported = s.lost_elapsed_ms >= Self::REGULATOR_EVENT_MS;
        }
        if s.active {
            self.restored_active = true;
            self.restored_mask = s.mask;
            self.emu_reported = true;
            self.emu_since_ms = now_ms;
        }
    }
}

/// RTC record of a [`LeaseClientSnapshot`]: "VDLE" (4), lost (1), active (1), mask u16 LE (2),
/// lost_elapsed_ms u32 LE (4), CRC u16 LE = low 16 bits of [`crc32`] over bytes 0..11 (2).
pub const LEASE_RECORD_SIZE: usize = 14;

/// Writes the record; returns [`LEASE_RECORD_SIZE`].
pub fn encode_lease_record(s: &LeaseClientSnapshot, out: &mut [u8; LEASE_RECORD_SIZE]) -> usize {
    out[..4].copy_from_slice(&LEASE_MAGIC);
    out[4] = u8::from(s.lost);
    out[5] = u8::from(s.active);
    out[6..8].copy_from_slice(&s.mask.to_le_bytes());
    out[8..12].copy_from_slice(&s.lost_elapsed_ms.to_le_bytes());
    let crc = crc32(&out[..12], 0).to_le_bytes();
    out[12..].copy_from_slice(&crc[..2]);
    LEASE_RECORD_SIZE
}

/// None for a wrong length, magic or CRC, or flag bytes above 1 (C++ false with `out` reset to
/// the default snapshot: `unwrap_or_default()`).
pub fn decode_lease_record(data: &[u8]) -> Option<LeaseClientSnapshot> {
    let d: &[u8; LEASE_RECORD_SIZE] = data.try_into().ok()?;
    if d[..4] != LEASE_MAGIC {
        return None;
    }
    let crc = crc32(&d[..12], 0).to_le_bytes();
    if d[12] != crc[0] || d[13] != crc[1] {
        return None;
    }
    if d[4] > 1 || d[5] > 1 {
        return None;
    }
    Some(LeaseClientSnapshot {
        lost: d[4] == 1,
        active: d[5] == 1,
        mask: u16::from_le_bytes([d[6], d[7]]),
        lost_elapsed_ms: u32::from_le_bytes([d[8], d[9], d[10], d[11]]),
    })
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
