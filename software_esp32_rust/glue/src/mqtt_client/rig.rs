//! The rig of the MQTT task tests (C++ `glue_test.h` with the sibling fakes of app, storage,
//! logger, net and ota): a booted device, a broker on broker.lan:1884 behind a scripted
//! connector, the fake host, the shared state, and the pass driver (C++ `runTask`).
// host test code: the stack rule of the glue (design 2.4) is for the device
#![allow(clippy::large_stack_frames, clippy::large_stack_arrays)]

use std::cell::RefCell;
use std::sync::{Arc, Mutex, MutexGuard};

use super::*;
use crate::port::{System, TcpRead, TcpStream};
use crate::testkit::broker::Publish;
use crate::testkit::net::FakeTcpStream;
use crate::testkit::{
    lock as tlock, Device, FakeBoard, FakeBroker, FakeClock, FakeFs, FakeHeap, FakeRtc, FakeTcp,
    FakeWatchdog,
};
use vdm_esp_core::event_log::{event_default_severity, make_event, EventLog};

pub(super) const HOST: &str = "broker.lan";
pub(super) const PORT: u16 = 1884;
/// Where the rig keeps the HA status record in the RTC block.
pub(super) const RTC_OFFSET: usize = 64;

// ---------------------------------------------------------------- the connector

/// Scripted failures of the broker connection (C++ `fakes::mqtt()` knobs).
#[derive(Default)]
pub(super) struct Script {
    /// connect calls (C++ `connects`)
    pub attempts: u32,
    /// every connect fails at once: TCP refused (C++ `connectResult = false`, state -2)
    pub refuse_all: bool,
    /// the next PUBLISH write fails (C++ `failPublishAt = publishCalls`)
    pub fail_next_publish: bool,
    /// the next PUBLISH to this topic fails (C++ `failPublishTopic`)
    pub fail_topic: Option<Vec<u8>>,
    /// PUBLISH packets written, failed ones included (C++ `publishCalls` on a live session)
    pub publishes: u64,
}

#[derive(Clone, Default)]
pub(super) struct NetScript(Arc<Mutex<Script>>);

impl NetScript {
    pub fn get(&self) -> MutexGuard<'_, Script> {
        tlock(&self.0)
    }
}

/// The topic of a PUBLISH packet.
fn publish_topic(p: &[u8]) -> Option<&[u8]> {
    let mut i = 1;
    while p.get(i)? & 0x80 != 0 {
        i += 1;
    }
    let n = usize::from(u16::from_be_bytes([*p.get(i + 1)?, *p.get(i + 2)?]));
    p.get(i + 3..i + 3 + n)
}

/// [`FakeTcp`] with the failures of [`Script`].
pub(super) struct ScriptedTcp {
    tcp: FakeTcp,
    script: NetScript,
}

impl TcpConnector for ScriptedTcp {
    type Stream = ScriptedStream;
    fn connect(&self, host: &str, port: u16, timeout_ms: u32) -> Option<ScriptedStream> {
        {
            let mut s = self.script.get();
            s.attempts += 1;
            if s.refuse_all {
                return None;
            }
        }
        let inner = self.tcp.connect(host, port, timeout_ms)?;
        Some(ScriptedStream {
            inner,
            script: self.script.clone(),
        })
    }
}

pub(super) struct ScriptedStream {
    inner: FakeTcpStream,
    script: NetScript,
}

impl TcpStream for ScriptedStream {
    fn read(&mut self, out: &mut [u8]) -> TcpRead {
        self.inner.read(out)
    }
    fn connected(&mut self) -> bool {
        self.inner.connected()
    }
    fn write_all(&mut self, data: &[u8]) -> bool {
        if data.first().is_some_and(|b| b & 0xF0 == 0x30) {
            let mut s = self.script.get();
            s.publishes += 1;
            if s.fail_next_publish {
                s.fail_next_publish = false;
                return false;
            }
            if s.fail_topic.is_some() && s.fail_topic.as_deref() == publish_topic(data) {
                s.fail_topic = None;
                return false;
            }
        }
        self.inner.write_all(data)
    }
    fn close(&mut self) {
        self.inner.close()
    }
}

// ---------------------------------------------------------------- the host

/// The sibling fakes of app, storage, logger, net and ota (C++ `sib::*`).
pub(super) struct HostState {
    clock: FakeClock,
    /// C++ `sib::app().uptimeS`: None = the clock
    pub uptime_s: Option<u32>,
    pub cfg: Box<Config>,
    pub revision: u32,
    pub snap: Box<StmSnapshot>,
    pub snap_revision: u32,
    pub profiles: Box<[Profile; N_VALVES]>,
    pub profile_reads: u32,
    pub submit_result: bool,
    pub submitted: Vec<StmCommand>,
    pub calib_next: i64,
    log: Box<EventLog<512>>,
    pub events: Vec<Event>,
    pub net_up: bool,
    pub ip: u32,
    pub local_time: LocalTime,
    pub restart_pending: bool,
    pub restart_requests: Vec<(u8, u32)>,
    pub fs_ready: bool,
    pub ha_cleanup_done: bool,
    pub ha_cleanup_marks: u32,
    pub ha_layout: u8,
    pub ha_layout_sets: Vec<u8>,
    pub config_reads: u32,
}

#[derive(Clone)]
pub(super) struct FakeHost(Arc<Mutex<HostState>>);

impl FakeHost {
    fn new(clock: FakeClock) -> Self {
        FakeHost(Arc::new(Mutex::new(HostState {
            clock,
            uptime_s: None,
            cfg: Box::default(),
            revision: 0,
            snap: Box::default(),
            snap_revision: 0,
            profiles: Box::new([Profile::default(); N_VALVES]),
            profile_reads: 0,
            submit_result: true,
            submitted: Vec::new(),
            calib_next: 0,
            log: Box::new(EventLog::new()),
            events: Vec::new(),
            net_up: false,
            ip: 0,
            local_time: LocalTime::default(),
            restart_pending: false,
            restart_requests: Vec::new(),
            fs_ready: true,
            ha_cleanup_done: false,
            ha_cleanup_marks: 0,
            ha_layout: 0,
            ha_layout_sets: Vec::new(),
            config_reads: 0,
        })))
    }

    pub fn state(&self) -> MutexGuard<'_, HostState> {
        tlock(&self.0)
    }

    /// C++ `logger::log(const Event&)`: the event as given, with its seq.
    pub fn log_event(&self, e: &Event) -> u32 {
        let mut s = self.state();
        let seq = s.log.append(e);
        let mut copy = e.clone();
        copy.seq = seq;
        s.events.push(copy);
        seq
    }

    /// C++ `logger::lastSeq()`.
    pub fn last_seq(&self) -> u32 {
        self.state().log.last_seq()
    }

    /// C++ `sib::logger().withCode(code)`.
    pub fn with_code(&self, code: EventCode) -> Vec<Event> {
        let s = self.state();
        s.events
            .iter()
            .filter(|e| e.code == code)
            .cloned()
            .collect()
    }
}

impl MqttHost for FakeHost {
    fn uptime_s(&self) -> u32 {
        let s = self.state();
        s.uptime_s.unwrap_or_else(|| s.clock.uptime_s())
    }
    fn submit(&self, cmd: &StmCommand) -> bool {
        let mut s = self.state();
        if !s.submit_result {
            return false;
        }
        s.submitted.push(cmd.clone());
        true
    }
    fn stm_snapshot_revision(&self) -> u32 {
        self.state().snap_revision
    }
    fn read_stm_snapshot(&self, out: &mut StmSnapshot) {
        out.clone_from(&self.state().snap);
    }
    fn read_profile(&self, valve: u8, out: &mut Profile) {
        let mut s = self.state();
        s.profile_reads += 1;
        *out = s
            .profiles
            .get(usize::from(valve))
            .copied()
            .unwrap_or_default();
    }
    fn calib_next_epoch(&self) -> i64 {
        self.state().calib_next
    }
    fn config_revision(&self) -> u32 {
        self.state().revision
    }
    fn with_config(&self, f: &mut dyn FnMut(&Config)) {
        let mut s = self.state();
        s.config_reads += 1;
        f(&s.cfg);
    }
    fn fs_ready(&self) -> bool {
        self.state().fs_ready
    }
    fn ha_cleanup_done(&self) -> bool {
        self.state().ha_cleanup_done
    }
    fn set_ha_cleanup_done(&self) {
        let mut s = self.state();
        s.ha_cleanup_done = true;
        s.ha_cleanup_marks += 1;
    }
    fn ha_layout(&self) -> u8 {
        self.state().ha_layout
    }
    fn set_ha_layout(&self, layout: u8) {
        let mut s = self.state();
        s.ha_layout = layout;
        s.ha_layout_sets.push(layout);
    }
    fn log(&self, code: EventCode, valve: u8, arg1: i32, arg2: i32, text: &[u8]) -> u32 {
        let e = make_event(code, event_default_severity(code), valve, arg1, arg2, text);
        self.log_event(&e)
    }
    fn read_events_since(&self, since: u32, out: &mut [Event]) -> (usize, u32) {
        let f = vdm_esp_core::event_log::EventFilter {
            since_seq: since,
            ..Default::default()
        };
        let mut next = 0;
        let n = self.state().log.read(&f, out, &mut next);
        (n, next)
    }
    fn net_up(&self) -> bool {
        self.state().net_up
    }
    fn net_ip(&self) -> u32 {
        self.state().ip
    }
    fn local_time(&self) -> LocalTime {
        self.state().local_time
    }
    fn restart_pending(&self) -> bool {
        self.state().restart_pending
    }
    fn request_restart(&self, reason: u8, delay_ms: u32) {
        let mut s = self.state();
        s.restart_requests.push((reason, delay_ms));
        s.restart_pending = true;
    }
}

// ---------------------------------------------------------------- the rig

pub(super) type Client<'a> =
    MqttClient<'a, FakeClock, FakeFs, ScriptedTcp, FakeHeap, FakeRtc, FakeWatchdog, FakeHost>;

/// A pass hook: called after every pass with the clock before the pass delay and now (after
/// it), like the C++ `fakes::rtos().onDelay`.
pub(super) type Hook<'h> = &'h mut dyn FnMut(u64, u64);

/// Pass tracking (C++ `Track`): the clock of every pass and the messages published by its end.
#[derive(Default)]
pub(super) struct Track {
    pub at: Vec<u64>,
    pub end: Vec<usize>,
}

impl Track {
    /// The pass that published message `index`.
    pub fn pass_of(&self, index: usize) -> usize {
        self.end
            .iter()
            .position(|&e| e > index)
            .unwrap_or(usize::MAX)
    }

    /// The passes that published to `topic`.
    pub fn passes_of(&self, rig: &Rig, topic: &str) -> Vec<usize> {
        rig.published()
            .iter()
            .enumerate()
            .filter(|(_, p)| p.topic == topic.as_bytes())
            .map(|(i, _)| self.pass_of(i))
            .collect()
    }

    /// The clock of the passes that published to `topic`.
    pub fn times_of(&self, rig: &Rig, topic: &str) -> Vec<u64> {
        self.passes_of(rig, topic)
            .iter()
            .map(|&k| self.at.get(k).copied().unwrap_or(u64::MAX))
            .collect()
    }

    /// Publish times of `topic` in the open interval (from, to).
    pub fn times_between(&self, rig: &Rig, topic: &str, from: u64, to: u64) -> Vec<u64> {
        self.times_of(rig, topic)
            .into_iter()
            .filter(|&t| t > from && t < to)
            .collect()
    }
}

pub(super) struct Rig {
    _board: FakeBoard,
    pub dev: Device,
    pub broker: FakeBroker,
    pub script: NetScript,
    tcp: ScriptedTcp,
    pub host: FakeHost,
    pub shared: MqttShared,
    /// every pass delay (C++ `fakes::rtos().delays`)
    pub delays: RefCell<Vec<u32>>,
}

impl Rig {
    /// C++ `glue::begin()`: a fresh device; the broker serves broker.lan:1884 without SUBACKs
    /// (the C++ fake had none).
    pub fn new() -> Self {
        let board = FakeBoard::new();
        let dev = board.boot();
        let broker = FakeBroker::default();
        broker.attach(&dev.tcp, HOST, PORT);
        {
            let mut b = broker.state();
            b.reply_ms = 0;
            b.sub_ack = false;
        }
        let script = NetScript::default();
        let tcp = ScriptedTcp {
            tcp: dev.tcp.clone(),
            script: script.clone(),
        };
        let host = FakeHost::new(dev.clock.clone());
        Rig {
            _board: board,
            dev,
            broker,
            script,
            tcp,
            host,
            shared: MqttShared::default(),
            delays: RefCell::new(Vec::new()),
        }
    }

    /// The client of the device (C++ the module statics).
    pub fn client(&self) -> Client<'_> {
        MqttClient::new(
            MqttPorts {
                clock: &self.dev.clock,
                fs: &self.dev.fs,
                tcp: &self.tcp,
                gate: &self.dev.heap,
                rtc: &self.dev.rtc,
                rtc_offset: RTC_OFFSET,
                mac: self.dev.system.base_mac(),
            },
            self.host.clone(),
            &self.shared,
        )
    }

    /// C++ `useMqtt(mode)`: broker.lan:1884, keepalive 30 s, station VdMot, valve 1 active, the
    /// discovery cleanup and layout done, the network up, LittleFS mounted.
    pub fn use_mqtt(&self, mode: MqttMode) {
        {
            let mut h = self.host.state();
            let c = &mut h.cfg;
            c.mqtt.mode = mode;
            copy_string(&mut c.mqtt.host, HOST.as_bytes());
            c.mqtt.port = PORT;
            c.mqtt.keep_alive_s = 30;
            copy_string(&mut c.station, b"VdMot");
            c.valves[0].active = true;
            h.revision += 1;
            h.ha_cleanup_done = true;
            h.ha_layout = 2;
            h.net_up = true;
        }
        self.dev.fs.knobs().mounted = true;
    }

    /// Changes the active config (C++ `sib::storage().active`); the task sees it after
    /// [`new_revision`](Self::new_revision) or at `begin`.
    pub fn cfg(&self, f: impl FnOnce(&mut Config)) {
        f(&mut self.host.state().cfg);
    }

    /// C++ `++sib::storage().revision`.
    pub fn new_revision(&self) {
        self.host.state().revision += 1;
    }

    /// Changes the snapshot (C++ `sib::app().snapshot`); the task reads it after
    /// [`publish_snap`](Self::publish_snap).
    pub fn snap(&self, f: impl FnOnce(&mut StmSnapshot)) {
        f(&mut self.host.state().snap);
    }

    /// C++ `publishSnap()`: a new snapshot revision.
    pub fn publish_snap(&self) {
        self.host.state().snap_revision += 1;
    }

    /// C++ `linkUp()`: link Up, protocol 2, valve 1 known, published.
    pub fn link_up(&self) {
        self.snap(|s| {
            s.link = LinkState::Up;
            s.proto = 2;
            s.valves[0].known = true;
        });
        self.publish_snap();
    }

    /// C++ `runTask(passes)`: the task starts (watchdog, event cursor at the start) and runs
    /// `passes` passes.
    pub fn run(&self, c: &mut Client<'_>, passes: usize) {
        self.run_hook(c, passes, &mut |_, _| {});
    }

    /// [`run`](Self::run) with a hook after every pass.
    pub fn run_hook(&self, c: &mut Client<'_>, passes: usize, hook: Hook<'_>) {
        c.start(self.dev.watchdog.clone());
        for _ in 0..passes {
            let d = c.pass();
            let before = self.dev.clock.ms();
            self.delays.borrow_mut().push(d);
            self.dev.clock.advance_ms(u64::from(d));
            hook(before, self.dev.clock.ms());
        }
    }

    /// [`run`](Self::run) with pass tracking; `each(passes tracked, now)` after every pass (C++
    /// `startTracking(each)`).
    pub fn run_tracked(
        &self,
        c: &mut Client<'_>,
        passes: usize,
        track: &mut Track,
        each: &mut dyn FnMut(usize, u64),
    ) {
        self.run_hook(c, passes, &mut |before, now| {
            track.at.push(before);
            track.end.push(self.published_len());
            each(track.at.len(), now);
        });
    }

    /// C++ `settle(passes)`: begin, `passes` passes, connected.
    pub fn settle(&self, c: &mut Client<'_>, passes: usize) {
        c.begin();
        self.run(c, passes);
        assert!(self.session_up(), "no session after {passes} passes");
    }

    /// The task reports a session (C++ `fakes::mqtt().connected`).
    pub fn session_up(&self) -> bool {
        self.shared.status().state == MqttState::Connected
    }

    /// A message from the broker (C++ `deliver`).
    pub fn deliver(&self, topic: &str, payload: &str) {
        self.broker
            .send_publish(topic.as_bytes(), payload.as_bytes(), 0, false, 0);
    }

    /// Every message the broker received, in order.
    pub fn published(&self) -> Vec<Publish> {
        self.broker.state().published.clone()
    }

    /// Messages received so far.
    pub fn published_len(&self) -> usize {
        self.broker.state().published.len()
    }

    /// The payloads published to `topic` (C++ `payloads`).
    pub fn payloads(&self, topic: &str) -> Vec<String> {
        self.broker
            .state()
            .published_to(topic.as_bytes())
            .iter()
            .map(|p| String::from_utf8_lossy(&p.payload).into_owned())
            .collect()
    }

    /// The last payload of `topic`, "<none>" without one (C++ `last`).
    pub fn last(&self, topic: &str) -> String {
        self.payloads(topic)
            .pop()
            .unwrap_or_else(|| "<none>".to_string())
    }

    /// Messages to `topic` (C++ `count`).
    pub fn count(&self, topic: &str) -> usize {
        self.payloads(topic).len()
    }

    /// Messages from index `from` on whose topic starts with `prefix` (C++ `countPrefix`).
    pub fn count_prefix(&self, prefix: &str, from: usize) -> usize {
        self.published()
            .iter()
            .skip(from)
            .filter(|p| p.topic.starts_with(prefix.as_bytes()))
            .count()
    }

    /// Index of the first message to `topic`.
    pub fn index_of(&self, topic: &str) -> Option<usize> {
        self.published()
            .iter()
            .position(|p| p.topic == topic.as_bytes())
    }

    /// The subscriptions in order (filter, QoS) (C++ `subscribed`).
    pub fn subscribed(&self) -> Vec<(String, u8)> {
        self.broker
            .state()
            .subscribed
            .iter()
            .map(|(_, f, q)| (String::from_utf8_lossy(f).into_owned(), *q))
            .collect()
    }

    /// Connect calls (C++ `connects`).
    pub fn connects(&self) -> u32 {
        self.script.get().attempts
    }

    /// The clean-session flag of the last CONNECT (C++ `cleanSession`).
    pub fn clean_session(&self) -> bool {
        self.broker
            .state()
            .connects
            .last()
            .is_some_and(|c| c.clean_session())
    }

    /// DISCONNECT packets (C++ `disconnects`).
    pub fn disconnects(&self) -> u32 {
        self.broker.state().disconnects
    }

    /// Every connect fails at once with state -2 (`true`) or works again (C++
    /// `connectResult`).
    pub fn refuse_connects(&self, refuse: bool) {
        self.script.get().refuse_all = refuse;
    }

    /// The broker answers the next `n` CONNECTs with return code `rc`.
    pub fn refuse_with(&self, rc: u8, n: usize) {
        self.broker.state().connack.extend(vec![Some(rc); n]);
    }

    /// The task's events with this code (C++ `sib::logger().withCode`).
    pub fn events(&self, code: EventCode) -> Vec<Event> {
        self.host.with_code(code)
    }

    /// C++ `logger::log(code, valve, arg1, arg2)`.
    pub fn log(&self, code: EventCode, valve: u8, arg1: i32, arg2: i32) -> u32 {
        self.host.log(code, valve, arg1, arg2, b"")
    }

    /// The rejected commands (C++ `rejected()`).
    pub fn rejected(&self) -> Vec<Event> {
        self.events(EventCode::MqttCommandRejected)
    }

    /// The commands handed to the STM task (C++ `sib::app().submitted`).
    pub fn submitted(&self) -> Vec<StmCommand> {
        self.host.state().submitted.clone()
    }

    /// The fake clock in ms since boot (C++ `fakes::nowMs()`).
    pub fn now(&self) -> u64 {
        self.dev.clock.ms()
    }

    /// Text of an event.
    pub fn text(e: &Event) -> String {
        String::from_utf8_lossy(&e.text).into_owned()
    }
}

/// Text of a byte slice.
pub(super) fn s(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}
