//! MQTT topic tree, command-topic parsing, payload formatting and publish scheduling (port of
//! `vdm/mqtt_topics.h`). The compat part is byte-exact with the legacy firmware
//! (software_esp32/src/mqtt.cpp); the new part is described in DESIGN.md "MQTT". Hardware-free.

use crate::common::{
    bounded_length, c_str, contains_bytes, elapsed_ms, fmt_fit, format_f64_fixed, is_safe_name,
    parse_uint, round_target_percent, LocalTime, Text, TextBuf, ITEM_NAME_MAX, STATION_NAME_MAX,
    VALVE_COUNT,
};
use crate::config::MqttMode;
use crate::valve_model::valve_status_text;

/// chars without NUL; every built topic fits
pub const TOPIC_MAX: usize = 127;
/// A scratch buffer that holds any topic and its NUL (C++ `char buf[kTopicMax + 1]`).
const TOPIC_BUF: usize = TOPIC_MAX + 1;
/// valve/sensor segment chars
pub const SEGMENT_MAX: usize = ITEM_NAME_MAX;

const FALLBACK_MAIN: &[u8] = b"VdMotFBH/";
const TARGET_PAYLOAD_MAX: usize = 16;
const HA_DEFAULT_PREFIX: &[u8] = b"homeassistant";
/// The status topic of [`HA_DEFAULT_PREFIX`], subscribed and recognised whatever the prefix.
const HA_DEFAULT_STATUS: &[u8] = b"homeassistant/status";

/// Settings that shape topics (subset of MqttConfig).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicContext {
    /// The MQTT root (`mqtt_root_topic()`: mqtt.root_topic, else the station), validated with
    /// is_safe_name.
    pub station: Text<STATION_NAME_MAX>,
    /// legacy publishPathAsRoot: leading '/'
    pub path_as_root: bool,
    /// legacy publishSeparate: "/value" + "/set"
    pub separate: bool,
}

impl Default for TopicContext {
    fn default() -> Self {
        Self {
            station: Text::new(),
            path_as_root: false,
            separate: true,
        }
    }
}

/// The topic segment of every valve (`item_segment()`), index 0..11: the C++
/// `const char segments[kValveCount][kSegmentMax + 1]`, read up to a NUL like the C++ arrays.
pub type Segments = [Text<SEGMENT_MAX>; VALVE_COUNT as usize];

/// `s` up to its first NUL, at most `max` bytes (C++ `boundedLength` of a char array).
fn bounded(s: &[u8], max: usize) -> &[u8] {
    s.get(..bounded_length(s, max)).unwrap_or_default()
}

/// Bounded topic builder: after an overflow or [`fail`](Self::fail) the result is "" and length
/// 0. An empty `out` (C++ cap 0) gets nothing written.
struct Builder<'a> {
    w: TextBuf<'a>,
    ok: bool,
}

impl<'a> Builder<'a> {
    fn new(out: &'a mut [u8]) -> Self {
        Self {
            w: TextBuf::new(out),
            ok: true,
        }
    }

    fn add(&mut self, s: &[u8]) {
        if self.ok {
            self.ok = self.w.push_bytes(s);
        }
    }

    fn fail(&mut self) {
        self.ok = false;
    }

    /// The text so far; empty after a failure.
    fn text(&self) -> &[u8] {
        if self.ok {
            self.w.as_bytes()
        } else {
            &[]
        }
    }

    fn finish(&self) -> usize {
        self.text().len()
    }
}

/// `text` when it fits `out`, else 0 (C++ `putf(out, cap, "%s", text)`).
fn put_text(out: &mut [u8], text: &[u8]) -> usize {
    let mut w = TextBuf::new(out);
    w.push_bytes(text);
    w.fit()
}

/// main = ["/"] + (station != "" ? station + "/" : "VdMotFBH/") into `b`; fails `b` for a
/// station that is no safe name.
fn add_main(b: &mut Builder<'_>, ctx: &TopicContext) {
    let station = c_str(&ctx.station);
    // is_safe_name also bounds the length
    if !is_safe_name(station, STATION_NAME_MAX, true) {
        b.fail();
    }
    if ctx.path_as_root {
        b.add(b"/");
    }
    if station.is_empty() {
        b.add(FALLBACK_MAIN);
    } else {
        b.add(station);
        b.add(b"/");
    }
}

/// main = ["/"] + (station != "" ? station + "/" : "VdMotFBH/").
/// Returns chars written (0 = does not fit).
pub fn build_main_topic(ctx: &TopicContext, out: &mut [u8]) -> usize {
    let mut b = Builder::new(out);
    add_main(&mut b, ctx);
    b.finish()
}

/// Valve/sensor segment: `name` (up to a NUL) with ' ' -> '_', or the 1-based index when the
/// name is empty ("3"). idx0 is 0-based. Returns chars written; 0 when the name is no safe name
/// of at most [`SEGMENT_MAX`] bytes or the segment does not fit.
pub fn build_segment(name: &[u8], idx0: u8, out: &mut [u8]) -> usize {
    let name = c_str(name);
    if name.is_empty() {
        return fmt_fit(out, format_args!("{}", u32::from(idx0) + 1));
    }
    if !is_safe_name(name, SEGMENT_MAX, false) {
        return 0;
    }
    let mut w = TextBuf::new(out);
    for &c in name {
        w.push(if c == b' ' { b'_' } else { c });
    }
    w.fit()
}

/// A segment as it may appear in a topic: 1..[`SEGMENT_MAX`] bytes (all of `seg`) without NUL,
/// '+' and '#'; '/' only between two non-empty parts (topic overrides such as "Bad/WC" of the
/// legacy firmware, config `item_segment()`).
pub fn topic_segment_valid(seg: &[u8]) -> bool {
    match seg {
        [] | [b'/', ..] | [.., b'/'] => false,
        _ => {
            seg.len() <= SEGMENT_MAX
                && !seg.iter().any(|&c| matches!(c, 0 | b'+' | b'#'))
                && !contains_bytes(seg, b"//")
        }
    }
}

/// Every topic the ESP publishes or subscribes. The numbers have no external meaning (2.1 topics
/// were appended).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topic {
    // ---- compat (legacy tree; "/value" appended when ctx.separate) ----
    /// `<main>common/ip`
    CommonIp = 0,
    /// `<main>common/state`
    CommonState = 1,
    /// `<main>common/uptime`
    CommonUptime = 2,
    /// `<main>common/message`
    CommonMessage = 3,
    /// `<main>valves/<V>/target` (also the command topic)
    ValveTarget = 4,
    /// `<main>valves/<V>/state`
    ValveState = 5,
    /// `<main>valves/<V>/calibration/date`
    ValveCalibDate = 6,
    /// `<main>valves/<V>/calibration/repetitions`
    ValveCalibRepetitions = 7,
    /// `<main>valves/<V>/diag/meanCurrrent` (sic, three r)
    ValveMeanCurrent = 8,
    /// `<main>valves/<V>/diag/openCount`
    ValveOpenCount = 9,
    /// `<main>valves/<V>/diag/closeCount`
    ValveCloseCount = 10,
    /// `<main>valves/<V>/diag/deadZoneCount`
    ValveDeadZoneCount = 11,
    /// `<main>valves/<V>/diag/moves`
    ValveMoves = 12,
    /// `<main>valves/<V>/temp1`
    ValveTemp1 = 13,
    /// `<main>valves/<V>/temp2`
    ValveTemp2 = 14,
    /// `<main>valves/<V>/actual` NEW, follows the compat suffix rule
    ValveActual = 15,
    /// `<main>temps/<T>/id`
    TempId = 16,
    /// `<main>temps/<T>/value` ("/value/value" when separate, legacy)
    TempValue = 17,
    /// `<main>sensors/<S>/id`
    VoltId = 18,
    /// `<main>sensors/<S>/value`
    VoltValue = 19,
    /// `<main>sensors/<S>/unit`
    VoltUnit = 20,
    // ---- new (never suffixed) ----
    /// `<main>diag/valves/<V>/lastMove` JSON
    DiagValveLastMove = 21,
    /// `<main>diag/valves/<V>/earlyStops`
    DiagValveEarlyStops = 22,
    /// `<main>diag/valves/<V>/cmdRejected`
    DiagValveCmdRejected = 23,
    /// `<main>diag/valves/<V>/calState`
    DiagValveCalState = 24,
    /// `<main>diag/valves/<V>/profile` JSON, not retained
    DiagValveProfile = 25,
    /// `<main>diag/stm/proto`
    DiagStmProto = 26,
    /// `<main>diag/stm/uptime`
    DiagStmUptime = 27,
    /// `<main>diag/stm/resets`
    DiagStmResets = 28,
    /// `<main>diag/stm/rxOverflow`
    DiagStmRxOverflow = 29,
    /// `<main>diag/stm/parseErr`
    DiagStmParseErr = 30,
    /// `<main>diag/stm/link`
    DiagStmLink = 31,
    /// `<main>diag/calibration/active` "0"/"1", retained
    DiagCalibrationActive = 32,
    /// `<main>events` JSON, not retained
    Events = 33,
    /// `<main>status` "online"/"offline", retained LWT
    Status = 34,
    // ---- 2.1 (internal numbering, no external meaning) ----
    /// `<main>valves/<V>/requested` desired target, suffix rule
    ValveRequested = 35,
    /// `<main>valves/<V>/sync` target_sync_name(), suffix rule
    ValveSync = 36,
    /// `<main>valves/<V>/failsafe` off/lease/blocked, suffix rule
    ValveFailsafe = 37,
    /// `<main>valves/<V>/problem` "1"/"0", suffix rule
    ValveProblem = 38,
    /// `<main>stm/status` "online"/"offline", retained
    StmStatus = 39,
    /// `<main>failsafe` "1"/"0", retained
    Failsafe = 40,
    /// `<main>diag/stm/version`
    DiagStmVersion = 41,
    /// `<main>diag/stm/started` UTC timestamp
    DiagStmStarted = 42,
    /// `<main>diag/stm/lease` off/running/expired
    DiagStmLease = 43,
    /// `<main>diag/stm/safeMode` "1"/"0"
    DiagStmSafeMode = 44,
    /// `<main>diag/mqtt/eventsSuppressed`
    DiagMqttEventsSuppressed = 45,
    /// `<main>diag/mqtt/commandsRejected`
    DiagMqttCommandsRejected = 46,
    /// `<main>diag/calibration/next` UTC timestamp or ""
    DiagCalibrationNext = 47,
    /// `<main>cmd/valves/<V>/calibrate` commands (buttons), never retained
    CmdValveCalibrate = 48,
    /// `<main>cmd/calibrate`
    CmdCalibrate = 49,
    /// `<main>cmd/restart`
    CmdRestart = 50,
    /// `<main>cmd/stmReset`
    CmdStmReset = 51,
    /// `<main>cmd/detect`
    CmdDetect = 52,
    /// `<main>cmd/stop` protocol >= 3
    CmdStop = 53,
    /// `<main>cmd/stmSafeExit` protocol >= 3
    CmdStmSafeExit = 54,
}

pub const TOPIC_COUNT: u8 = 55;

/// Every topic in number order.
const ALL_TOPICS: [Topic; TOPIC_COUNT as usize] = [
    Topic::CommonIp,
    Topic::CommonState,
    Topic::CommonUptime,
    Topic::CommonMessage,
    Topic::ValveTarget,
    Topic::ValveState,
    Topic::ValveCalibDate,
    Topic::ValveCalibRepetitions,
    Topic::ValveMeanCurrent,
    Topic::ValveOpenCount,
    Topic::ValveCloseCount,
    Topic::ValveDeadZoneCount,
    Topic::ValveMoves,
    Topic::ValveTemp1,
    Topic::ValveTemp2,
    Topic::ValveActual,
    Topic::TempId,
    Topic::TempValue,
    Topic::VoltId,
    Topic::VoltValue,
    Topic::VoltUnit,
    Topic::DiagValveLastMove,
    Topic::DiagValveEarlyStops,
    Topic::DiagValveCmdRejected,
    Topic::DiagValveCalState,
    Topic::DiagValveProfile,
    Topic::DiagStmProto,
    Topic::DiagStmUptime,
    Topic::DiagStmResets,
    Topic::DiagStmRxOverflow,
    Topic::DiagStmParseErr,
    Topic::DiagStmLink,
    Topic::DiagCalibrationActive,
    Topic::Events,
    Topic::Status,
    Topic::ValveRequested,
    Topic::ValveSync,
    Topic::ValveFailsafe,
    Topic::ValveProblem,
    Topic::StmStatus,
    Topic::Failsafe,
    Topic::DiagStmVersion,
    Topic::DiagStmStarted,
    Topic::DiagStmLease,
    Topic::DiagStmSafeMode,
    Topic::DiagMqttEventsSuppressed,
    Topic::DiagMqttCommandsRejected,
    Topic::DiagCalibrationNext,
    Topic::CmdValveCalibrate,
    Topic::CmdCalibrate,
    Topic::CmdRestart,
    Topic::CmdStmReset,
    Topic::CmdDetect,
    Topic::CmdStop,
    Topic::CmdStmSafeExit,
];

impl Topic {
    pub fn from_raw(v: u8) -> Option<Self> {
        ALL_TOPICS.get(usize::from(v)).copied()
    }
}

#[derive(Clone, Copy)]
enum RetainRule {
    Setting,
    Always,
    Never,
}

struct TopicDef {
    /// path before the segment (or the whole path)
    head: &'static [u8],
    /// path after the segment; None = no segment
    tail: Option<&'static [u8]>,
    /// "/value" appended with `separate`
    suffix: bool,
    retain: RetainRule,
}

const S: RetainRule = RetainRule::Setting;
const A: RetainRule = RetainRule::Always;
const N: RetainRule = RetainRule::Never;

/// A per-item topic: head + segment + tail.
const fn item(
    head: &'static [u8],
    tail: &'static [u8],
    suffix: bool,
    retain: RetainRule,
) -> TopicDef {
    TopicDef {
        head,
        tail: Some(tail),
        suffix,
        retain,
    }
}

/// A topic without a segment.
const fn path(path: &'static [u8], suffix: bool, retain: RetainRule) -> TopicDef {
    TopicDef {
        head: path,
        tail: None,
        suffix,
        retain,
    }
}

/// Indexed by Topic.
const TOPICS: [TopicDef; TOPIC_COUNT as usize] = [
    path(b"common/ip", true, S),
    path(b"common/state", true, S),
    path(b"common/uptime", true, S),
    path(b"common/message", true, S),
    item(b"valves/", b"/target", true, S),
    item(b"valves/", b"/state", true, S),
    item(b"valves/", b"/calibration/date", true, S),
    item(b"valves/", b"/calibration/repetitions", true, S),
    item(b"valves/", b"/diag/meanCurrrent", true, S),
    item(b"valves/", b"/diag/openCount", true, S),
    item(b"valves/", b"/diag/closeCount", true, S),
    item(b"valves/", b"/diag/deadZoneCount", true, S),
    item(b"valves/", b"/diag/moves", true, S),
    item(b"valves/", b"/temp1", true, S),
    item(b"valves/", b"/temp2", true, S),
    item(b"valves/", b"/actual", true, S),
    item(b"temps/", b"/id", true, S),
    item(b"temps/", b"/value", true, S),
    item(b"sensors/", b"/id", true, S),
    item(b"sensors/", b"/value", true, S),
    item(b"sensors/", b"/unit", true, S),
    item(b"diag/valves/", b"/lastMove", false, S),
    item(b"diag/valves/", b"/earlyStops", false, S),
    item(b"diag/valves/", b"/cmdRejected", false, S),
    item(b"diag/valves/", b"/calState", false, S),
    item(b"diag/valves/", b"/profile", false, N),
    path(b"diag/stm/proto", false, S),
    path(b"diag/stm/uptime", false, S),
    path(b"diag/stm/resets", false, S),
    path(b"diag/stm/rxOverflow", false, S),
    path(b"diag/stm/parseErr", false, S),
    path(b"diag/stm/link", false, S),
    path(b"diag/calibration/active", false, A),
    path(b"events", false, N),
    path(b"status", false, A),
    item(b"valves/", b"/requested", true, S),
    item(b"valves/", b"/sync", true, S),
    item(b"valves/", b"/failsafe", true, S),
    item(b"valves/", b"/problem", true, S),
    path(b"stm/status", false, A),
    path(b"failsafe", false, A),
    path(b"diag/stm/version", false, S),
    path(b"diag/stm/started", false, S),
    path(b"diag/stm/lease", false, S),
    path(b"diag/stm/safeMode", false, S),
    path(b"diag/mqtt/eventsSuppressed", false, S),
    path(b"diag/mqtt/commandsRejected", false, S),
    path(b"diag/calibration/next", false, S),
    item(b"cmd/valves/", b"/calibrate", false, N),
    path(b"cmd/calibrate", false, N),
    path(b"cmd/restart", false, N),
    path(b"cmd/stmReset", false, N),
    path(b"cmd/detect", false, N),
    path(b"cmd/stop", false, N),
    path(b"cmd/stmSafeExit", false, N),
];
const _: () = assert!(
    Topic::CmdStmSafeExit as u8 + 1 == TOPIC_COUNT,
    "TOPICS out of sync"
);

fn topic_def(t: Topic) -> &'static TopicDef {
    // in range: one entry per Topic value (the assertion above)
    &TOPICS[t as usize]
}

/// Main topic + path (without the compat suffix); fails `b` on bad input.
fn add_base(b: &mut Builder<'_>, ctx: &TopicContext, t: Topic, segment: &[u8]) {
    add_main(b, ctx);
    let d = topic_def(t);
    b.add(d.head);
    if let Some(tail) = d.tail {
        let seg = bounded(segment, SEGMENT_MAX + 1);
        if !topic_segment_valid(seg) {
            b.fail();
        }
        b.add(seg);
        b.add(tail);
    }
}

/// True for topics that take the "/value" suffix with `separate`: the legacy tree plus
/// valves/<V>/{requested,sync,failsafe,problem}.
pub fn topic_is_compat(t: Topic) -> bool {
    topic_def(t).suffix
}

/// Retain flag: Status, StmStatus, Failsafe and DiagCalibrationActive are always retained;
/// Events, DiagValveProfile and the cmd/ topics never; every other topic follows
/// `publish_retained`.
pub fn topic_retained(t: Topic, publish_retained: bool) -> bool {
    match topic_def(t).retain {
        RetainRule::Always => true,
        RetainRule::Never => false,
        RetainRule::Setting => publish_retained,
    }
}

/// Full publish topic. `segment` (up to a NUL) is the valve/sensor segment for per-item topics
/// (ignored otherwise; required non-empty for them; the C++ null segment is the empty one).
/// Returns chars written, 0 on overflow or missing segment.
pub fn build_topic(ctx: &TopicContext, t: Topic, segment: &[u8], out: &mut [u8]) -> usize {
    let mut b = Builder::new(out);
    add_base(&mut b, ctx, t, segment);
    if ctx.separate && topic_is_compat(t) {
        b.add(b"/value");
    }
    b.finish()
}

/// Subscription filter for the valve target command of one valve:
/// separate: "<main>valves/<V>/target/set", else "<main>valves/<V>/target".
pub fn build_target_command_topic(ctx: &TopicContext, segment: &[u8], out: &mut [u8]) -> usize {
    let mut b = Builder::new(out);
    add_base(&mut b, ctx, Topic::ValveTarget, segment);
    if ctx.separate {
        b.add(b"/set");
    }
    b.finish()
}

/// HA status topic "<prefix>/status" (HA birth / last will), `prefix` up to a NUL. 0 when it
/// does not fit or the prefix is empty (also the C++ null prefix).
pub fn build_ha_status_topic(prefix: &[u8], out: &mut [u8]) -> usize {
    let prefix = c_str(prefix);
    let mut b = Builder::new(out);
    if prefix.is_empty() {
        b.fail();
    }
    b.add(prefix);
    b.add(b"/status");
    b.finish()
}

/// Subscriptions of one connection.
pub const MAX_SUBSCRIPTIONS: usize = 29;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Subscription {
    pub filter: Text<TOPIC_MAX>,
    pub qos: u8,
}

/// Where the entries go: from entry `skip` on into `out`; the entries before `skip` are only
/// counted ([`build_subscription`] takes one entry at a time).
struct SubscriptionSink<'a> {
    out: &'a mut [Subscription],
    skip: usize,
    seen: usize,
    n: usize,
}

/// Appends one entry (skipped when it does not fit or the filter is empty).
fn add_subscription(s: &mut SubscriptionSink<'_>, filter: &[u8], qos: u8) {
    if filter.is_empty() {
        return;
    }
    let index = s.seen;
    s.seen += 1;
    if index < s.skip {
        return;
    }
    if let Some(slot) = s.out.get_mut(s.n) {
        slot.filter.clear();
        // a filter is at most TOPIC_MAX bytes: it was built in a TOPIC_BUF buffer
        let _ = slot.filter.extend_from_slice(filter);
        slot.qos = qos;
        s.n += 1;
    }
}

/// "<main>valves/<seg>/target[/set]" and the same + "/set".
fn add_target_filters(ctx: &TopicContext, main: &[u8], seg: &[u8], s: &mut SubscriptionSink<'_>) {
    let mut f = [0u8; TOPIC_BUF];
    let mut b = Builder::new(&mut f);
    b.add(main);
    b.add(b"valves/");
    b.add(seg);
    b.add(if ctx.separate {
        b"/target/set"
    } else {
        b"/target"
    });
    add_subscription(s, b.text(), 1);
    b.add(b"/set");
    add_subscription(s, b.text(), 1);
}

/// Every entry of [`build_subscriptions`] in order (a mode other than Off).
fn add_subscriptions(
    ctx: &TopicContext,
    mode: MqttMode,
    ha_prefix: &[u8],
    segments: Option<&Segments>,
    s: &mut SubscriptionSink<'_>,
) {
    let mut main = [0u8; TOPIC_BUF];
    let ml = build_main_topic(ctx, &mut main);
    let main = main.get(..ml).unwrap_or_default();
    if !main.is_empty() {
        add_target_filters(ctx, main, b"+", s);
        let mut f = [0u8; TOPIC_BUF];
        let mut b = Builder::new(&mut f);
        b.add(main);
        b.add(b"cmd/#");
        add_subscription(s, b.text(), 0);
        for seg in segments.into_iter().flatten() {
            let seg = bounded(seg, SEGMENT_MAX);
            if seg.contains(&b'/') && topic_segment_valid(seg) {
                add_target_filters(ctx, main, seg, s);
            }
        }
    }
    if mode == MqttMode::MqttHa {
        add_subscription(s, HA_DEFAULT_STATUS, 1);
        if c_str(ha_prefix) != HA_DEFAULT_PREFIX {
            let mut f = [0u8; TOPIC_BUF];
            let n = build_ha_status_topic(ha_prefix, &mut f);
            add_subscription(s, f.get(..n).unwrap_or_default(), 1);
        }
    }
}

/// Modes Mqtt and MqttHa: the two target command filters with a '+' for the valve (separate:
/// "<main>valves/+/target/set" and ".../target/set/set"; not separate: "<main>valves/+/target"
/// and ".../target/set"), QoS 1; then "<main>cmd/#", QoS 0; then for every valve whose segment
/// contains '/' the same two filters spelled out (QoS 1: '+' matches one level only); MqttHa also
/// "homeassistant/status" and "<ha_prefix>/status" when the prefix differs (QoS 1). Off: none.
/// Entries that do not fit `out` (the C++ cap) or [`TOPIC_MAX`] are skipped. `segments` may be
/// None (no spelled-out filters); `ha_prefix` is read up to a NUL (the C++ null prefix is the
/// empty one: no entry of its own).
pub fn build_subscriptions(
    ctx: &TopicContext,
    mode: MqttMode,
    ha_prefix: &[u8],
    segments: Option<&Segments>,
    out: &mut [Subscription],
) -> usize {
    if mode == MqttMode::Off {
        return 0;
    }
    let mut s = SubscriptionSink {
        out,
        skip: 0,
        seen: 0,
        n: 0,
    };
    add_subscriptions(ctx, mode, ha_prefix, segments, &mut s);
    s.n
}

/// Entry `index` (0-based) of [`build_subscriptions`]; None when there is no such entry. The
/// glue subscribes one filter at a time and keeps no table of all of them.
pub fn build_subscription(
    ctx: &TopicContext,
    mode: MqttMode,
    ha_prefix: &[u8],
    segments: Option<&Segments>,
    index: usize,
) -> Option<Subscription> {
    if mode == MqttMode::Off {
        return None;
    }
    let mut one = [Subscription::default()];
    let mut s = SubscriptionSink {
        out: &mut one,
        skip: index,
        seen: 0,
        n: 0,
    };
    add_subscriptions(ctx, mode, ha_prefix, segments, &mut s);
    let found = s.n == 1;
    let [sub] = one;
    found.then_some(sub)
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InboundKind {
    /// not one of our topics
    #[default]
    None = 0,
    /// "homeassistant/status" or "<ha_prefix>/status"
    HaStatus = 1,
    /// `<main>valves/<seg>/target` command
    Target = 2,
    /// `<main>cmd/valves/<seg>/calibrate`
    CalibrateValve = 3,
    /// `<main>cmd/calibrate`
    CalibrateAll = 4,
    /// `<main>cmd/restart`
    Restart = 5,
    /// `<main>cmd/stmReset`
    StmReset = 6,
    /// `<main>cmd/detect`
    Detect = 7,
    /// `<main>cmd/stop`
    StopAll = 8,
    /// `<main>cmd/stmSafeExit`
    StmSafeExit = 9,
    /// `<main>cmd/<anything else>`
    UnknownCommand = 10,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InboundTopic {
    pub kind: InboundKind,
    /// Target / CalibrateValve: 0..11; None (C++ -1) = the segment names no valve
    pub valve: Option<u8>,
    /// Target without separate: exactly "<main>valves/<seg>/target"
    pub state_form: bool,
}

/// The fixed cmd/ topics.
const CMD_NAMES: [(&[u8], InboundKind); 6] = [
    (b"calibrate", InboundKind::CalibrateAll),
    (b"restart", InboundKind::Restart),
    (b"stmReset", InboundKind::StmReset),
    (b"detect", InboundKind::Detect),
    (b"stop", InboundKind::StopAll),
    (b"stmSafeExit", InboundKind::StmSafeExit),
];

/// The valve a segment names: configured segments first, then 1..12 (no leading zero).
fn match_segment(seg: &[u8], segments: Option<&Segments>) -> Option<u8> {
    let named = segments.and_then(|t| t.iter().position(|s| bounded(s, SEGMENT_MAX) == seg));
    if let Some(i) = named {
        return u8::try_from(i).ok();
    }
    if seg.first() == Some(&b'0') {
        return None;
    }
    parse_uint(seg, u32::from(VALVE_COUNT))
        .and_then(|n| n.checked_sub(1))
        .and_then(|v| u8::try_from(v).ok())
}

/// Classifies an inbound topic, all of `topic` (a NUL byte in it -> None):
///  - "homeassistant/status" and "<ha_prefix>/status" -> HaStatus;
///  - a leading '/' on the topic and on main is ignored for the rest;
///  - "<main>valves/<seg>/" + ("target/set" | "target/set/set") when separate,
///    + ("target" | "target/set") when not -> Target;
///  - "<main>cmd/valves/<seg>/calibrate" -> CalibrateValve; the fixed cmd/ topics -> their kind;
///    any other "<main>cmd/..." -> UnknownCommand.
///
/// `<seg>` is matched against `segments` (`item_segment()` of every valve, active or not, '/'
/// allowed) in index order, first match wins; otherwise a strict number 1..12 without leading
/// zero selects that valve; any other non-empty segment gives valve None; an empty one gives
/// None as the kind.
pub fn parse_inbound_topic(
    ctx: &TopicContext,
    ha_prefix: &[u8],
    topic: &[u8],
    segments: Option<&Segments>,
) -> InboundTopic {
    let none = InboundTopic::default();
    if topic.is_empty() || topic.len() > TOPIC_MAX || topic.contains(&0) {
        return none;
    }
    let mut ha = [0u8; TOPIC_BUF];
    let hl = build_ha_status_topic(ha_prefix, &mut ha);
    if topic == HA_DEFAULT_STATUS || topic == ha.get(..hl).unwrap_or_default() {
        return InboundTopic {
            kind: InboundKind::HaStatus,
            ..none
        };
    }
    let topic = topic.strip_prefix(b"/").unwrap_or(topic);
    let mut main = [0u8; TOPIC_BUF];
    let ml = build_main_topic(ctx, &mut main);
    let main = main.get(..ml).unwrap_or_default();
    if main.is_empty() {
        return none;
    }
    let main = main.strip_prefix(b"/").unwrap_or(main);
    let Some(rest) = topic.strip_prefix(main) else {
        return none;
    };
    if let Some(rest) = rest.strip_prefix(b"valves/") {
        let suffixes: [&[u8]; 2] = if ctx.separate {
            [b"/target/set", b"/target/set/set"]
        } else {
            [b"/target", b"/target/set"]
        };
        for (k, suffix) in suffixes.into_iter().enumerate() {
            if let Some(seg) = rest.strip_suffix(suffix).filter(|s| !s.is_empty()) {
                return InboundTopic {
                    kind: InboundKind::Target,
                    valve: match_segment(seg, segments),
                    state_form: !ctx.separate && k == 0,
                };
            }
        }
        return none;
    }
    let Some(rest) = rest.strip_prefix(b"cmd/") else {
        return none;
    };
    let valve_seg = rest
        .strip_prefix(b"valves/")
        .and_then(|r| r.strip_suffix(b"/calibrate"))
        .filter(|s| !s.is_empty());
    if let Some(seg) = valve_seg {
        return InboundTopic {
            kind: InboundKind::CalibrateValve,
            valve: match_segment(seg, segments),
            state_form: false,
        };
    }
    let kind = CMD_NAMES
        .iter()
        .find(|(name, _)| *name == rest)
        .map_or(InboundKind::UnknownCommand, |&(_, kind)| kind);
    InboundTopic { kind, ..none }
}

/// Why a target payload is no target: the C++ `TargetPayload` values except `Ok` (0), which is
/// the `Ok` of [`parse_target_payload`]. The numbers are the `detail` of a payload reject.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetPayload {
    Empty = 1,
    NotNumber = 2,
    OutOfRange = 3,
    Stop = 4,
}

impl TargetPayload {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Empty),
            2 => Some(Self::NotNumber),
            3 => Some(Self::OutOfRange),
            4 => Some(Self::Stop),
            _ => None,
        }
    }
}

fn is_blank(c: &u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n')
}

/// `p` without surrounding spaces, tabs, CR and LF.
fn trim_blanks(p: &[u8]) -> &[u8] {
    let start = p.iter().position(|c| !is_blank(c)).unwrap_or(p.len());
    let end = p
        .iter()
        .rposition(|c| !is_blank(c))
        .map_or(start, |i| i + 1);
    p.get(start..end).unwrap_or_default()
}

/// Target payload: optional surrounding spaces/tabs/CR/LF, at most 16 bytes in all; "OPEN" ->
/// 100, "CLOSE" -> 0, "STOP" -> Err(Stop); otherwise 1+ digits, optionally '.' or ',' and 1+
/// digits, converted with `round_target_percent()` (exact range 0..100, half up: 43.5 -> 44,
/// 99.5 -> 100, 100.01 -> OutOfRange). Rejects: sign, exponent, hex, "nan"/"inf", a separator
/// without digits on either side, a second separator. (C++: the result Ok and `out`; `out` is
/// left alone otherwise.)
pub fn parse_target_payload(p: &[u8]) -> Result<u8, TargetPayload> {
    if p.is_empty() {
        return Err(TargetPayload::Empty);
    }
    if p.len() > TARGET_PAYLOAD_MAX {
        return Err(TargetPayload::NotNumber);
    }
    let s = trim_blanks(p);
    match s {
        [] => return Err(TargetPayload::Empty),
        b"OPEN" => return Ok(100),
        b"CLOSE" => return Ok(0),
        b"STOP" => return Err(TargetPayload::Stop),
        _ => {}
    }
    // 1+ digits, then nothing or one separator and 1+ digits
    let int_len = s.iter().take_while(|c| c.is_ascii_digit()).count();
    let number = match s.get(int_len..).unwrap_or_default() {
        [] => true,
        [b'.' | b',', frac @ ..] => !frac.is_empty() && frac.iter().all(u8::is_ascii_digit),
        _ => false,
    };
    if int_len == 0 || !number {
        return Err(TargetPayload::NotNumber);
    }
    // "<int>[.<frac>]" (at most 16 bytes) through the correctly rounded decimal conversion, like
    // the C++ strtod()
    let mut num = [0u8; TARGET_PAYLOAD_MAX];
    for (d, &c) in num.iter_mut().zip(s) {
        *d = if c == b',' { b'.' } else { c };
    }
    let v = num
        .get(..s.len())
        .and_then(|n| core::str::from_utf8(n).ok())
        .and_then(|t| t.parse::<f64>().ok());
    v.and_then(round_target_percent)
        .ok_or(TargetPayload::OutOfRange)
}

/// Button payload: exactly "PRESS" (HA default payload_press).
pub fn parse_button_payload(p: &[u8]) -> bool {
    p == b"PRESS"
}

// ---------------------------------------------------------------- payloads

/// Temperature tenths -> "21.5" / "-0.5" / "21,5" (german_comma). Invalid raw (temp_raw_valid
/// false) -> "failed". Returns chars written (0 when it does not fit).
pub fn format_temp(tenths: i32, valid: bool, german_comma: bool, out: &mut [u8]) -> usize {
    if !valid {
        return put_text(out, b"failed");
    }
    let sign = if tenths < 0 { "-" } else { "" };
    let mag = tenths.unsigned_abs();
    let sep = if german_comma { ',' } else { '.' };
    fmt_fit(out, format_args!("{sign}{}{sep}{}", mag / 10, mag % 10))
}

/// Volt value with 3 decimals ("12.345", comma option; C++ `%.3f`), "failed" when !valid or not
/// finite. Returns chars written (0 when it does not fit).
pub fn format_volt(value: f64, valid: bool, german_comma: bool, out: &mut [u8]) -> usize {
    if !valid || !value.is_finite() {
        return put_text(out, b"failed");
    }
    let n = format_f64_fixed(value, 3, out);
    if german_comma {
        if let Some(dot) = out.iter_mut().take(n).find(|c| **c == b'.') {
            *dot = b',';
        }
    }
    n
}

/// Legacy uptime "%ud %u:%02u:%02u", e.g. "3d 4:05:09".
pub fn format_uptime(seconds: u32, out: &mut [u8]) -> usize {
    fmt_fit(
        out,
        format_args!(
            "{}d {}:{:02}:{:02}",
            seconds / 86400,
            seconds % 86400 / 3600,
            seconds % 3600 / 60,
            seconds % 60
        ),
    )
}

/// Valve state: plain_text -> valve_status_text(status); else decimal status.
pub fn format_valve_state(status: u8, plain_text: bool, out: &mut [u8]) -> usize {
    if plain_text {
        put_text(out, valve_status_text(status).as_bytes())
    } else {
        fmt_fit(out, format_args!("{status}"))
    }
}

const SYSTEM_STATE_NAMES: [&str; 3] = ["ok", "info", "error"];

/// System state 0 ok / 1 info / 2 error; plain "ok","info","error", >= 3 "".
pub fn format_system_state(state: u8, plain_text: bool, out: &mut [u8]) -> usize {
    if plain_text {
        let name = SYSTEM_STATE_NAMES.get(usize::from(state)).copied();
        put_text(out, name.unwrap_or("").as_bytes())
    } else {
        fmt_fit(out, format_args!("{state}"))
    }
}

const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Legacy calibration date "%A, %B %d.%Y %H:%M:%S" with English names, e.g.
/// "Monday, September 21.2026 14:03:05"; "Failed to obtain time" when !t.valid (or a field is
/// out of range).
pub fn format_calib_date(t: &LocalTime, out: &mut [u8]) -> usize {
    let day = DAYS.get(usize::from(t.wday));
    let month = MONTHS.get(usize::from(t.month).wrapping_sub(1));
    let (Some(day), Some(month)) = (day, month) else {
        return put_text(out, b"Failed to obtain time");
    };
    if !t.valid || !(1..=31).contains(&t.mday) || t.hour > 23 || t.minute > 59 || t.second > 60 {
        return put_text(out, b"Failed to obtain time");
    }
    fmt_fit(
        out,
        format_args!(
            "{day}, {month} {:02}.{} {:02}:{:02}:{:02}",
            t.mday, t.year, t.hour, t.minute, t.second
        ),
    )
}

/// Legacy uint32 counters are published like itoa(int): values > i32::MAX print negative
/// (compat). Returns chars written.
pub fn format_legacy_counter(v: u32, out: &mut [u8]) -> usize {
    // itoa(int) semantics: the bit pattern read as int32
    fmt_fit(out, format_args!("{}", v as i32))
}

// ---------------------------------------------------------------- scheduling

/// Publish cadence (C++ `PublishScheduler::Params`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublishSchedulerParams {
    pub on_change: bool,
    /// >= 2000
    pub publish_interval_ms: u32,
    /// <= publish_interval_ms
    pub min_delay_ms: u32,
}

impl Default for PublishSchedulerParams {
    fn default() -> Self {
        Self {
            on_change: true,
            publish_interval_ms: 10000,
            min_delay_ms: 5000,
        }
    }
}

const SLOTS: usize = PublishScheduler::SLOTS as usize;

/// Legacy publish cadence, per item slot:
///  - after (re)connect: everything once (full publish);
///  - periodic mode (!on_change): full publish every publish_interval_ms;
///  - on-change mode: an item goes out when changed and min_delay_ms has passed since its last
///    publish, and at least every publish_interval_ms (item heartbeat); a full publish every
///    publish_interval_ms as well.
///
/// Slots are caller-defined small integers (DESIGN.md: common 0, valves 1..12, temps 13..46,
/// volts 47..54, stm diag 55).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishScheduler {
    p: PublishSchedulerParams,
    force_full: bool,
    last_full_ms: u32,
    last_item_ms: [u32; SLOTS],
    item_seen: [bool; SLOTS],
}

impl Default for PublishScheduler {
    fn default() -> Self {
        Self {
            p: PublishSchedulerParams::default(),
            force_full: true,
            last_full_ms: 0,
            last_item_ms: [0; SLOTS],
            item_seen: [false; SLOTS],
        }
    }
}

impl PublishScheduler {
    pub const SLOTS: u8 = 64;

    /// The interval is raised to 2000 ms at least, the minimum delay cut to the interval.
    pub fn configure(&mut self, p: PublishSchedulerParams) {
        let publish_interval_ms = p.publish_interval_ms.max(2000);
        self.p = PublishSchedulerParams {
            on_change: p.on_change,
            publish_interval_ms,
            min_delay_ms: p.min_delay_ms.min(publish_interval_ms),
        };
    }

    /// Forces the next full publish.
    pub fn on_connected(&mut self, _now_ms: u32) {
        self.force_full = true;
        self.item_seen = [false; SLOTS];
    }

    /// Full publish due now (first after connect, or periodic)? Consumes it.
    pub fn take_full_publish(&mut self, now_ms: u32) -> bool {
        if !self.force_full && elapsed_ms(now_ms, self.last_full_ms) < self.p.publish_interval_ms {
            return false;
        }
        self.force_full = false;
        self.last_full_ms = now_ms;
        true
    }

    /// Item decision in on-change mode; in periodic mode always false (items only go out with
    /// the full publish). Consumes the decision (marks the slot published at `now_ms`) when it
    /// returns true.
    pub fn take_item(&mut self, slot: u8, changed: bool, now_ms: u32) -> bool {
        let i = usize::from(slot);
        let (Some(seen), Some(last)) = (self.item_seen.get_mut(i), self.last_item_ms.get_mut(i))
        else {
            return false;
        };
        if !self.p.on_change {
            return false;
        }
        if *seen {
            let since = elapsed_ms(now_ms, *last);
            if !(changed && since >= self.p.min_delay_ms) && since < self.p.publish_interval_ms {
                return false;
            }
        }
        *seen = true;
        *last = now_ms;
        true
    }

    /// After a full publish every slot counts as just published.
    pub fn mark_all_published(&mut self, now_ms: u32) {
        self.item_seen = [true; SLOTS];
        self.last_item_ms = [now_ms; SLOTS];
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
