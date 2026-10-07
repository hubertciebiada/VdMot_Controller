//! Decisions of the MQTT task: Home Assistant status, reconnect pacing, the start of automatic
//! discovery runs, client id, inbound commands (echoes, retained leftovers, rejections), button
//! confirmation and the latest target per valve (port of `vdm/mqtt_policy.h`). Hardware-free.

use core::fmt::Write as _;

use crate::common::{build_hostname, elapsed_ms, Backoff, Text, TextBuf, NO_VALVE, VALVE_COUNT};
use crate::config::{crc32, MqttMode};
use crate::failsafe::HaStatus;
use crate::mqtt_topics::{
    parse_button_payload, parse_inbound_topic, parse_target_payload, InboundKind, InboundTopic,
    Segments, TargetPayload, TopicContext, TOPIC_MAX,
};

/// Valves (the arrays of the echo filter and the latch).
const N: usize = VALVE_COUNT as usize;

// ---------------------------------------------------------------- RegulatorWatch

/// What a message on the HA status topic or a command changed (C++ `RegulatorWatch::Change`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RegulatorWatchChange {
    #[default]
    None = 0,
    WentOffline = 1,
    CameOnline = 2,
    CameOnlineByCommand = 3,
}

/// K1: what Home Assistant said on its status topic. The status survives broker reconnects (HA's
/// birth and last will are often not retained, so a reconnect must not turn a known Offline into
/// Unknown) and ESP software restarts (RTC record); power-on starts Unknown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegulatorWatch {
    status: HaStatus,
}

impl RegulatorWatch {
    /// Exact payload bytes: "offline" -> Offline (WentOffline unless it was Offline); "online" ->
    /// Online (CameOnline only when it was Offline; Unknown -> Online is silent); anything else is
    /// ignored.
    pub fn on_ha_status(&mut self, payload: &[u8]) -> RegulatorWatchChange {
        let was_offline = self.status == HaStatus::Offline;
        match payload {
            b"offline" => {
                self.status = HaStatus::Offline;
                if was_offline {
                    RegulatorWatchChange::None
                } else {
                    RegulatorWatchChange::WentOffline
                }
            }
            b"online" => {
                self.status = HaStatus::Online;
                if was_offline {
                    RegulatorWatchChange::CameOnline
                } else {
                    RegulatorWatchChange::None
                }
            }
            _ => RegulatorWatchChange::None,
        }
    }

    /// An accepted inbound command in MqttHa mode: somebody is controlling, so an Offline HA reads
    /// as Online again (CameOnlineByCommand: no discovery run).
    pub fn on_inbound_command(&mut self) -> RegulatorWatchChange {
        if self.status != HaStatus::Offline {
            return RegulatorWatchChange::None;
        }
        self.status = HaStatus::Online;
        RegulatorWatchChange::CameOnlineByCommand
    }

    pub fn ha_status(&self) -> HaStatus {
        self.status
    }

    pub fn snapshot(&self) -> HaStatus {
        self.status
    }

    /// The C++ restores Unknown for a value out of range; a HaStatus is always in range
    /// ([`decode_ha_status_record`] gives Unknown for such a record).
    pub fn restore(&mut self, s: HaStatus) {
        self.status = s;
    }
}

/// RTC record of the HA status: {u32 magic "VDHA", u8 status, u8 pad, u16 crc} with crc = low 16
/// bits of crc32 over the first 6 bytes (the magic little endian, as the C++ reads it on the
/// ESP32).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HaStatusRecord {
    pub magic: u32,
    pub status: u8,
    pub pad: u8,
    pub crc: u16,
}

const _: () = assert!(core::mem::size_of::<HaStatusRecord>() == 8);
const _: () = assert!(core::mem::offset_of!(HaStatusRecord, status) == 4);
const _: () = assert!(core::mem::offset_of!(HaStatusRecord, pad) == 5);
const _: () = assert!(core::mem::offset_of!(HaStatusRecord, crc) == 6);

/// "VDHA" little endian
pub const HA_STATUS_MAGIC: u32 = 0x4148_4456;

fn ha_record_crc(r: &HaStatusRecord) -> u16 {
    let [m0, m1, m2, m3] = r.magic.to_le_bytes();
    // the low 16 bits
    crc32(&[m0, m1, m2, m3, r.status, r.pad], 0) as u16
}

pub fn encode_ha_status_record(s: HaStatus, out: &mut HaStatusRecord) {
    out.magic = HA_STATUS_MAGIC;
    out.status = s as u8;
    out.pad = 0;
    out.crc = ha_record_crc(out);
}

/// Unknown for a record with a wrong magic, CRC or status (power-on garbage).
pub fn decode_ha_status_record(r: &HaStatusRecord) -> HaStatus {
    if r.magic != HA_STATUS_MAGIC || r.crc != ha_record_crc(r) {
        return HaStatus::Unknown;
    }
    HaStatus::from_raw(r.status).unwrap_or(HaStatus::Unknown)
}

// ---------------------------------------------------------------- ReconnectPacer

/// W20: reconnect pacing. The back-off (min_ms doubling to max_ms) is reset only after a
/// connection stayed up for [`STABLE_MS`](Self::STABLE_MS); a drop before that counts as a
/// failed attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReconnectPacer {
    backoff: Backoff,
    connected: bool,
    stable: bool,
    connected_ms: u32,
}

impl Default for ReconnectPacer {
    /// The C++ default arguments: 2000 ms doubling to 60000 ms.
    fn default() -> Self {
        Self::new(2000, 60000)
    }
}

impl ReconnectPacer {
    pub const STABLE_MS: u32 = 60000;

    /// C++ defaults for min_ms and max_ms: 2000 and 60000 ([`Default`]).
    pub fn new(min_ms: u32, max_ms: u32) -> Self {
        Self {
            backoff: Backoff::new(min_ms, max_ms),
            connected: false,
            stable: false,
            connected_ms: 0,
        }
    }

    pub fn due(&self, now_ms: u32) -> bool {
        self.backoff.due(now_ms)
    }

    pub fn on_attempt_failed(&mut self, now_ms: u32) {
        self.backoff.on_failure(now_ms);
    }

    pub fn on_connected(&mut self, now_ms: u32) {
        self.connected = true;
        self.stable = false;
        self.connected_ms = now_ms;
    }

    /// Connection lost: a failure unless it was stable.
    pub fn on_dropped(&mut self, now_ms: u32) {
        if !self.connected {
            return;
        }
        self.connected = false;
        if !self.stable {
            self.backoff.on_failure(now_ms);
        }
    }

    /// Stable after [`STABLE_MS`](Self::STABLE_MS) -> back-off reset.
    pub fn tick(&mut self, now_ms: u32, connected: bool) {
        if !connected || !self.connected || self.stable {
            return;
        }
        if elapsed_ms(now_ms, self.connected_ms) < Self::STABLE_MS {
            return;
        }
        self.stable = true;
        self.backoff.reset();
    }

    /// User reconnect: due at once, delay back to min.
    pub fn force_now(&mut self) {
        self.backoff.reset();
    }

    pub fn delay_ms(&self) -> u32 {
        self.backoff.delay_ms()
    }
}

// ---------------------------------------------------------------- DiscoveryGate

/// The automatic HA discovery runs (after a connect, after a discovery input changed) wait for
/// settled STM inputs (`StmSnapshot::sensors_settled`: link up, re-sync done, sensor grace over),
/// so the set goes out once with what the STM reports instead of once per step of its start-up
/// (2.1.5 sent it up to six times in 90 s after a boot). Without an STM that settles a run starts
/// [`MAX_WAIT_MS`](Self::MAX_WAIT_MS) after the first request, with the inputs known by then.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiscoveryGate {
    pending: bool,
    since_ms: u32,
}

impl DiscoveryGate {
    pub const MAX_WAIT_MS: u32 = 120000;

    /// A run is wanted; the wait counts from the first request.
    pub fn request(&mut self, now_ms: u32) {
        if self.pending {
            return;
        }
        self.pending = true;
        self.since_ms = now_ms;
    }

    /// A run started (any run answers the request), or the session ended.
    pub fn clear(&mut self) {
        self.pending = false;
    }

    pub fn pending(&self) -> bool {
        self.pending
    }

    /// The requested run may start now.
    pub fn due(&self, settled: bool, now_ms: u32) -> bool {
        self.pending && (settled || elapsed_ms(now_ms, self.since_ms) >= Self::MAX_WAIT_MS)
    }
}

// ---------------------------------------------------------------- client id

/// W20: "<host>-<mac>" = build_hostname(station) cut to 16 chars (a '-' left at the end of the
/// cut is dropped), '-', mac[3..5] as 6 lowercase hex digits. At most 23 chars (MQTT 3.1.1
/// guaranteed length). Returns the length; 0 when `out` has fewer than 24 bytes.
pub fn build_mqtt_client_id(station: &[u8], mac: &[u8; 6], out: &mut [u8]) -> usize {
    if out.len() < 24 {
        return 0;
    }
    let mut host = [0u8; 65];
    let n = build_hostname(station, &mut host);
    let host = host.get(..n.min(16)).unwrap_or_default();
    let end = host.iter().rposition(|&c| c != b'-').map_or(0, |i| i + 1);
    let mut w = TextBuf::new(out);
    w.push_bytes(host.get(..end).unwrap_or_default());
    let _ = write!(w, "-{:02x}{:02x}{:02x}", mac[3], mac[4], mac[5]);
    w.len()
}

// ---------------------------------------------------------------- EchoFilter

/// Without `separate` the target state topic is also a command topic: the ESP remembers the
/// value it published there last per valve (since connect); an inbound message on the state form
/// with that value is its own echo.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EchoFilter {
    value: [Option<u8>; N],
}

impl EchoFilter {
    pub fn reset(&mut self) {
        self.value = [None; N];
    }

    /// Valves out of range are ignored.
    pub fn published(&mut self, valve: u8, value: u8) {
        if let Some(v) = self.value.get_mut(usize::from(valve)) {
            *v = Some(value);
        }
    }

    pub fn is_echo(&self, valve: u8, value: u8) -> bool {
        self.value.get(usize::from(valve)) == Some(&Some(value))
    }
}

// ---------------------------------------------------------------- inbound

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RejectReason {
    #[default]
    None = 0,
    Payload = 1,
    UnknownValve = 2,
    Inactive = 3,
    Unsupported = 4,
    UnknownCommand = 5,
    QueueFull = 6,
    ClearNotConfirmed = 7,
    /// Rust only (D9): `cmd/restart` while the STM's sector 0 is not written
    StmSector0Pending = 8,
}

impl RejectReason {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::None,
            Self::Payload,
            Self::UnknownValve,
            Self::Inactive,
            Self::Unsupported,
            Self::UnknownCommand,
            Self::QueueFull,
            Self::ClearNotConfirmed,
            Self::StmSector0Pending,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// "", "payload", "unknown valve", "inactive", "unsupported", "unknown command", "queue full",
/// "clear not confirmed", "stm sector 0 pending".
pub fn reject_reason_name(r: RejectReason) -> &'static str {
    match r {
        RejectReason::None => "",
        RejectReason::Payload => "payload",
        RejectReason::UnknownValve => "unknown valve",
        RejectReason::Inactive => "inactive",
        RejectReason::Unsupported => "unsupported",
        RejectReason::UnknownCommand => "unknown command",
        RejectReason::QueueFull => "queue full",
        RejectReason::ClearNotConfirmed => "clear not confirmed",
        RejectReason::StmSector0Pending => "stm sector 0 pending",
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InboundAction {
    #[default]
    Ignore = 0,
    Reject = 1,
    SetTarget = 2,
    StopValve = 3,
    CalibrateValve = 4,
    CalibrateAll = 5,
    Restart = 6,
    StmReset = 7,
    Detect = 8,
    StopAll = 9,
    StmSafeExit = 10,
    HaOnline = 11,
    HaOffline = 12,
}

impl InboundAction {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::Ignore,
            Self::Reject,
            Self::SetTarget,
            Self::StopValve,
            Self::CalibrateValve,
            Self::CalibrateAll,
            Self::Restart,
            Self::StmReset,
            Self::Detect,
            Self::StopAll,
            Self::StmSafeExit,
            Self::HaOnline,
            Self::HaOffline,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// Button actions: run only after the broker echoed the clearing publish.
pub fn inbound_is_button(a: InboundAction) -> bool {
    matches!(
        a,
        InboundAction::CalibrateValve
            | InboundAction::CalibrateAll
            | InboundAction::Restart
            | InboundAction::StmReset
            | InboundAction::Detect
            | InboundAction::StopAll
            | InboundAction::StmSafeExit
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InboundDecision {
    pub action: InboundAction,
    /// valve actions and valve rejects (0..11); NO_VALVE otherwise
    pub valve: u8,
    /// SetTarget
    pub pos: u8,
    pub reason: RejectReason,
    /// Reject Payload: the TargetPayload value
    pub detail: i32,
    /// publish "" retained to the topic first
    pub clear_retained: bool,
}

impl Default for InboundDecision {
    fn default() -> Self {
        Self {
            action: InboundAction::Ignore,
            valve: NO_VALVE,
            pos: 0,
            reason: RejectReason::None,
            detail: 0,
            clear_retained: false,
        }
    }
}

/// What [`decide_inbound`] needs to know (C++ `InboundContext`).
#[derive(Clone, Copy, Debug)]
pub struct InboundContext<'a> {
    /// None (C++ null): every message is ignored
    pub topics: Option<&'a TopicContext>,
    /// read up to a NUL; C++ default "homeassistant"
    pub ha_prefix: &'a [u8],
    /// the segment of every valve; None: numbers only
    pub segments: Option<&'a Segments>,
    pub active_mask: u16,
    pub mode: MqttMode,
    /// STM protocol >= 3 (sstop, ssafe)
    pub stm_v3: bool,
    pub echo: Option<&'a EchoFilter>,
}

impl Default for InboundContext<'_> {
    fn default() -> Self {
        Self {
            topics: None,
            ha_prefix: b"homeassistant",
            segments: None,
            active_mask: 0,
            mode: MqttMode::Off,
            stm_v3: false,
            echo: None,
        }
    }
}

fn is_blank(c: &u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

/// Payload without surrounding blanks is empty.
fn blank_payload(p: &[u8]) -> bool {
    p.iter().all(is_blank)
}

fn reject(reason: RejectReason, valve: u8, detail: i32) -> InboundDecision {
    InboundDecision {
        action: InboundAction::Reject,
        valve,
        reason,
        detail,
        ..InboundDecision::default()
    }
}

/// Bit `valve` of `mask`.
fn active(mask: u16, valve: u8) -> bool {
    mask.checked_shr(u32::from(valve))
        .is_some_and(|m| m & 1 != 0)
}

/// Target and CalibrateValve: the valve, then the payload.
fn valve_command(c: &InboundContext<'_>, t: &InboundTopic, payload: &[u8]) -> InboundDecision {
    let Some(valve) = t.valve else {
        return reject(RejectReason::UnknownValve, NO_VALVE, 0);
    };
    if !active(c.active_mask, valve) {
        return reject(RejectReason::Inactive, valve, 0);
    }
    let decided = |action, pos| InboundDecision {
        action,
        valve,
        pos,
        ..InboundDecision::default()
    };
    if t.kind == InboundKind::CalibrateValve {
        return if parse_button_payload(payload) {
            decided(InboundAction::CalibrateValve, 0)
        } else {
            reject(RejectReason::Payload, valve, 0)
        };
    }
    match parse_target_payload(payload) {
        Ok(pos) => {
            let echo = t.state_form && c.echo.is_some_and(|e| e.is_echo(valve, pos));
            let action = if echo {
                InboundAction::Ignore
            } else {
                InboundAction::SetTarget
            };
            decided(action, pos)
        }
        Err(TargetPayload::Stop) if c.stm_v3 => decided(InboundAction::StopValve, 0),
        Err(TargetPayload::Stop) => reject(RejectReason::Unsupported, valve, 0),
        Err(r) => reject(RejectReason::Payload, valve, i32::from(r as u8)),
    }
}

/// The fixed cmd/ topics: payload PRESS, StopAll and StmSafeExit need protocol 3.
fn button_command(c: &InboundContext<'_>, kind: InboundKind, payload: &[u8]) -> InboundDecision {
    let action = match kind {
        InboundKind::CalibrateAll => InboundAction::CalibrateAll,
        InboundKind::Restart => InboundAction::Restart,
        InboundKind::StmReset => InboundAction::StmReset,
        InboundKind::Detect => InboundAction::Detect,
        InboundKind::StopAll => InboundAction::StopAll,
        InboundKind::StmSafeExit => InboundAction::StmSafeExit,
        // the other kinds never get here (decide_inbound)
        _ => return InboundDecision::default(),
    };
    let v3_only = matches!(action, InboundAction::StopAll | InboundAction::StmSafeExit);
    if !parse_button_payload(payload) {
        reject(RejectReason::Payload, NO_VALVE, 0)
    } else if v3_only && !c.stm_v3 {
        reject(RejectReason::Unsupported, NO_VALVE, 0)
    } else {
        InboundDecision {
            action,
            ..InboundDecision::default()
        }
    }
}

/// Rules:
///  - not our topic -> Ignore; an empty payload (after trimming blanks) on any command topic ->
///    Ignore (the echo of a retained clear, or someone clearing);
///  - HaStatus: mode != MqttHa -> Ignore; exactly "online"/"offline" -> HaOnline/HaOffline; else
///    Ignore; never cleared;
///  - Target: valve None -> Reject UnknownValve; inactive -> Reject Inactive; payload Ok ->
///    SetTarget (state form && echo.is_echo(valve, pos) -> Ignore); Stop -> StopValve with
///    stm_v3, else Reject Unsupported; NotNumber/OutOfRange -> Reject Payload (detail = the
///    TargetPayload);
///  - CalibrateValve: valve and inactive as Target, payload must be PRESS (else Reject Payload);
///  - other cmd topics: payload PRESS (else Reject Payload); StopAll and StmSafeExit need stm_v3
///    (else Unsupported); UnknownCommand -> Reject;
///  - clear_retained: every non-empty message on a command topic (actions and rejects), except
///    the state form "<main>valves/<seg>/target" without separate (the ESP's own retained state).
///
/// The C++ null payload is the empty one.
pub fn decide_inbound(c: &InboundContext<'_>, topic: &[u8], payload: &[u8]) -> InboundDecision {
    let ignore = InboundDecision::default();
    let Some(topics) = c.topics else {
        return ignore;
    };
    let t = parse_inbound_topic(topics, c.ha_prefix, topic, c.segments);
    match t.kind {
        InboundKind::None => return ignore,
        InboundKind::HaStatus => {
            let action = match (c.mode, payload) {
                (MqttMode::MqttHa, b"online") => InboundAction::HaOnline,
                (MqttMode::MqttHa, b"offline") => InboundAction::HaOffline,
                _ => InboundAction::Ignore,
            };
            return InboundDecision { action, ..ignore };
        }
        _ => {}
    }
    if blank_payload(payload) {
        return ignore;
    }
    let mut d = match t.kind {
        InboundKind::Target | InboundKind::CalibrateValve => valve_command(c, &t, payload),
        InboundKind::UnknownCommand => reject(RejectReason::UnknownCommand, NO_VALVE, 0),
        kind => button_command(c, kind, payload),
    };
    d.clear_retained = !t.state_form;
    d
}

/// True for an empty (blank) payload on a command topic: the broker echoing a retained clear back
/// ([`ButtonGate::confirm`]). The C++ null payload is the empty one.
pub fn inbound_is_clear_echo(c: &InboundContext<'_>, topic: &[u8], payload: &[u8]) -> bool {
    let Some(topics) = c.topics else {
        return false;
    };
    if !blank_payload(payload) {
        return false;
    }
    let kind = parse_inbound_topic(topics, c.ha_prefix, topic, c.segments).kind;
    !matches!(kind, InboundKind::None | InboundKind::HaStatus)
}

// ---------------------------------------------------------------- RejectLog

/// E28: logging throttle for rejected commands: a rejection is logged when its (valve, reason)
/// differs from the last logged one or [`REPEAT_MS`](Self::REPEAT_MS) passed since it; the caller
/// counts every rejection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RejectLog {
    /// the last logged (valve key, reason, time)
    last: Option<(u8, RejectReason, u32)>,
}

impl RejectLog {
    pub const REPEAT_MS: u32 = 10000;

    /// valve_key 0 = no valve, 1..12.
    pub fn should_log(&mut self, valve_key: u8, reason: RejectReason, now_ms: u32) -> bool {
        if let Some((key, last_reason, last_ms)) = self.last {
            if key == valve_key
                && last_reason == reason
                && elapsed_ms(now_ms, last_ms) < Self::REPEAT_MS
            {
                return false;
            }
        }
        self.last = Some((valve_key, reason, now_ms));
        true
    }
}

// ---------------------------------------------------------------- ButtonGate

#[derive(Clone, Debug, PartialEq, Eq)]
struct ButtonSlot {
    d: InboundDecision,
    since_ms: u32,
    topic: Text<TOPIC_MAX>,
}

/// Button actions wait for the broker to echo the empty retained message the ESP published to
/// their topic (then a retained PRESS cannot repeat at the next connect); without the echo within
/// [`CONFIRM_MS`](Self::CONFIRM_MS) they are rejected.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ButtonGate {
    slots: [Option<ButtonSlot>; ButtonGate::SLOTS],
}

impl ButtonGate {
    pub const SLOTS: usize = 4;
    pub const CONFIRM_MS: u32 = 5000;

    /// Queues a decided button action for `topic` (all of it, at most [`TOPIC_MAX`] bytes); false
    /// when the topic is longer or every slot is busy (the caller rejects it: queue full).
    pub fn hold(&mut self, d: &InboundDecision, topic: &[u8], now_ms: u32) -> bool {
        let Some(free) = self.slots.iter_mut().find(|s| s.is_none()) else {
            return false;
        };
        let Ok(topic) = Text::from_slice(topic) else {
            return false;
        };
        *free = Some(ButtonSlot {
            d: *d,
            since_ms: now_ms,
            topic,
        });
        true
    }

    /// The empty message arrived on `topic`: the oldest held action of that topic (removed); None
    /// when there is none. Like the C++: the action in the lowest slot, which after a slot was
    /// reused is not always the oldest (docs/rust/PORT-NOTES.md).
    pub fn confirm(&mut self, topic: &[u8]) -> Option<InboundDecision> {
        let slot = self
            .slots
            .iter_mut()
            .find(|s| s.as_ref().is_some_and(|s| s.topic.as_slice() == topic))?;
        slot.take().map(|s| s.d)
    }

    /// A held action older than [`CONFIRM_MS`](Self::CONFIRM_MS) (removed); the caller rejects it
    /// (clear not confirmed). None when there is none.
    pub fn expire(&mut self, now_ms: u32) -> Option<InboundDecision> {
        let slot = self.slots.iter_mut().find(|s| {
            s.as_ref()
                .is_some_and(|s| elapsed_ms(now_ms, s.since_ms) >= Self::CONFIRM_MS)
        })?;
        slot.take().map(|s| s.d)
    }

    /// Disconnect: held actions are dropped.
    pub fn reset(&mut self) {
        self.slots = Default::default();
    }
}

// ---------------------------------------------------------------- TargetLatch

/// H14: the newest target per valve that app::submit() refused (full queue); the task re-submits
/// it until the queue takes it, a newer one replaces it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TargetLatch {
    pos: [Option<u8>; N],
}

impl TargetLatch {
    /// Valves out of range are ignored.
    pub fn set(&mut self, valve: u8, pos: u8) {
        if let Some(p) = self.pos.get_mut(usize::from(valve)) {
            *p = Some(pos);
        }
    }

    pub fn clear(&mut self, valve: u8) {
        if let Some(p) = self.pos.get_mut(usize::from(valve)) {
            *p = None;
        }
    }

    /// The first latched valve at or after `from` (wrapping): (valve, pos); None when none.
    pub fn next(&self, from: u8) -> Option<(u8, u8)> {
        (0..N)
            .map(|k| (usize::from(from) + k) % N)
            .find_map(|v| Some((u8::try_from(v).ok()?, self.pos.get(v).copied().flatten()?)))
    }

    pub fn pending(&self, valve: u8) -> bool {
        self.pos
            .get(usize::from(valve))
            .is_some_and(Option::is_some)
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
