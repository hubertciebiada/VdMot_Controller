//! The MQTT task (C++ `mqtt_client.cpp`; docs/revamped/MQTT.md, DESIGN.md sections 10 and 11,
//! docs/rust/GLUE-DESIGN-ESP.md 3.1 row `mqtt_client`): the broker session over [`MqttConn`]
//! (last will `<main>status` = `offline`, `online` after the connect, the subscriptions one filter
//! at a time, a clean session only after a change of the topic settings, the reconnect back-off),
//! the legacy topic tree and the 2.1 topics spread over the loop passes (a full publish of one
//! valve or four other slots per pass, two changed slots per pass, four diag messages per pass),
//! the inbound commands with their retained clears, the button gate and the target latch, the
//! Home Assistant status and the regulator state of the failsafe lease, the events of the log,
//! and the HA discovery runs with their list file `/HADiscovery.cfg`.
//!
//! What the task needs from the other glue modules comes through [`MqttHost`] (one method per
//! C++ call into `app`, `storage`, `logger`, `net` and `ota`); what it hands to them lives in
//! [`MqttShared`]. The thread body is `client.start(watchdog); loop { let d = client.pass();
//! clock.sleep_ms(d) }`: [`MqttClient::pass`] feeds the watchdog at its start and after every
//! publish, as the C++ task did with `esp_task_wdt_reset()`.

use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use vdm_esp_core::common::{
    c_str, copy_string, elapsed_ms, fmt_fit, format_ipv4, format_one_wire_id, LocalTime, Text,
    ALL_VALVES, NO_VALVE, ONE_WIRE_ID_TEXT_LEN, TEMP_SLOT_COUNT, TEMP_UNASSIGNED, VALVE_COUNT,
    VOLT_SLOT_COUNT,
};
use vdm_esp_core::config::{
    crc32, item_segment, mqtt_root_topic, mqtt_topic_config_changed, Config, ItemKind, MqttMode,
    CLIENT_ID_MAX,
};
use vdm_esp_core::event_limiter::{EventAggregator, EventRateLimiter, PublishEvent};
use vdm_esp_core::event_log::{
    event_reaches_mqtt, format_event_message, format_utc_timestamp, write_mqtt_event_json, Event,
    EventCode, Severity,
};
use vdm_esp_core::failsafe::{
    failsafe_kind_name, lease_state_name, HaStatus, LeaseState, RegulatorInput,
};
use vdm_esp_core::ha_discovery::{
    build_discovery_context, discovery_input_key, DiscoveryContext, DiscoveryInputs, DiscoveryPlan,
    DiscoveryPort, DiscoveryRun, DiscoveryRunPhase, DISCOVERY_PAYLOAD_MAX,
};
use vdm_esp_core::json_api::{write_profile_json, MqttState};
use vdm_esp_core::json_writer::JsonWriter;
use vdm_esp_core::link_policy::{link_state_name, LinkState};
use vdm_esp_core::mqtt_policy::{
    build_mqtt_client_id, decide_inbound, decode_ha_status_record, encode_ha_status_record,
    inbound_is_button, inbound_is_clear_echo, reject_reason_name, ButtonGate, DiscoveryGate,
    EchoFilter, HaStatusRecord, InboundAction, InboundContext, InboundDecision, ReconnectPacer,
    RegulatorWatch, RegulatorWatchChange, RejectLog, RejectReason, TargetLatch,
};
use vdm_esp_core::mqtt_topics::{
    build_subscription, build_topic, format_calib_date, format_legacy_counter, format_system_state,
    format_temp, format_uptime, format_valve_state, format_volt, topic_retained, PublishScheduler,
    PublishSchedulerParams, Segments, Topic, TopicContext, SEGMENT_MAX, TOPIC_MAX,
};
use vdm_esp_core::mqtt_values::{
    active_valve_mask, find_temp_bus, find_volt_bus, published_target, sensor_topic_segment,
    slot_temp_tenths, slot_volt_value, stm_online, system_state, temp_published, valve_compat_key,
    valve_problem, volt_published_mqtt, CalibEndTracker, SystemFlags,
};
use vdm_esp_core::stm_codec::{
    stop_reason_name, MoveDir, Profile, ProfileSample, PROFILE_MAX_SAMPLES,
};
use vdm_esp_core::stm_types::{StmCommand, StmCommandType, StmSnapshot};
use vdm_esp_core::valve_model::{
    failsafe_kind, target_sync_name, temp_raw_valid, TargetSource, TempReading, ValveState,
    VoltReading,
};
use vdm_esp_core::version::{firmware_version, format_version};

use crate::heap::{try_block, try_bytes, Block};
use crate::mqtt_conn::{ConnectArgs, MqttConn, Will};
use crate::port::{Clock, Fs, FsFile, HeapGate, OpenMode, Rtc, TcpConnector, Watchdog};

// ---------------------------------------------------------------- binding numbers

/// PubSubClient packet buffer: topic + payload (discovery payloads up to 2047 B).
pub const BUFFER_SIZE: u16 = 2304;
/// CONNACK and packet byte wait, independent of the keepalive.
pub const SOCKET_TIMEOUT_S: u16 = 5;
/// Reconnect back-off: the first wait, doubled per failure up to [`BACKOFF_MAX_MS`].
pub const BACKOFF_MIN_MS: u32 = 2000;
/// Longest reconnect back-off.
pub const BACKOFF_MAX_MS: u32 = 60000;
/// Delay of a pass with a session: discovery messages go out 20 ms apart.
pub const DISCOVERY_PACE_MS: u32 = 20;
/// Delay of a pass without a session (connecting, back-off, restart pending).
pub const WAIT_MS: u32 = 100;
/// Delay of a pass with MQTT off or the network down.
pub const IDLE_MS: u32 = 500;
/// The discovery topics this device published, one per line (LittleFS).
pub const LIST_FILE: &str = "/HADiscovery.cfg";
/// The list being written; renamed over [`LIST_FILE`] when complete.
pub const LIST_TMP: &str = "/HADiscovery.cfg.tmp";
/// The HA status record in RTC memory: {u32 magic, u8 status, u8 pad, u16 crc}, little endian.
pub const HA_STATUS_RECORD_LEN: usize = 8;

// Every message built here fits the buffer with its 5 + 2 header bytes, so a publish only fails
// on a dead connection or a socket write that stalled (tests: the contract constants).

/// PublishScheduler slots: common, valves, temperatures, voltages, STM diag, system.
const SLOT_COMMON: u8 = 0;
const SLOT_VALVE0: u8 = 1;
const SLOT_TEMP0: u8 = SLOT_VALVE0 + VALVE_COUNT;
const SLOT_VOLT0: u8 = SLOT_TEMP0 + TEMP_SLOT_COUNT;
const SLOT_STM: u8 = SLOT_VOLT0 + VOLT_SLOT_COUNT;
/// stm/status and failsafe
const SLOT_SYSTEM: u8 = SLOT_STM + 1;
const SLOT_COUNT: u8 = SLOT_SYSTEM + 1;

/// A topic and its C++ NUL (the C++ `char topic[kTopicMax + 1]`).
const TOPIC_BUF: usize = TOPIC_MAX + 1;
/// A segment and its C++ NUL.
const SEGMENT_BUF: usize = SEGMENT_MAX + 1;
/// The automatic client id: at most 23 characters and the NUL.
const CLIENT_ID_BUF: usize = 24;
/// The payload buffer of a discovery run (payloads up to DISCOVERY_PAYLOAD_MAX).
const PAYLOAD_BUF: usize = DISCOVERY_PAYLOAD_MAX + 1;
/// The bytes block of a discovery run: the payload buffer, then the list buffer.
const RUN_BYTES: usize = PAYLOAD_BUF + LIST_BUFFER;

/// Slots of a full publish per pass; a valve slot uses up the pass.
const FULL_SLOTS_PER_PASS: u8 = 4;
/// On-change slots per pass.
const MAX_SLOTS_PER_PASS: u8 = 2;
/// Diag messages per pass.
const MAX_DIAG_PER_PASS: u8 = 4;
/// Log events read per pass.
const EVENTS_PER_PASS: usize = 4;
/// A sensor reading older than this is "failed".
const SENSOR_STALE_MS: u32 = 60000;
/// diag/mqtt/* at most this often.
const COUNTER_PACE_MS: u32 = 10000;
/// diag/stm/started goes out again when the start time moved by more than this.
const STARTED_TOLERANCE_S: u64 = 60;
/// Inbound messages one poll may deliver before they are handled.
const INBOUND_SLOTS: usize = 4;
/// Longer payloads are cut (every valid one is shorter).
const INBOUND_PAYLOAD_MAX: usize = 33;
/// common/message: the message of the latest Warning+ event (C++ `char[120]`).
const MESSAGE_MAX: usize = 119;
/// The message and its C++ NUL.
const MESSAGE_BUF: usize = MESSAGE_MAX + 1;
/// writeProfileJson() of PROFILE_MAX_SAMPLES samples with the largest values:
/// {"valve":12,"count":32,"samples":[[4294967295,65535],...]} is 643 bytes.
const PROFILE_JSON_MAX: usize = 643;
/// The read buffer of the list file (C++ `storage::kFileBufferSize`, its stdio buffer).
const LIST_BUFFER: usize = 512;
/// HA discovery layout of 2.1 (storage `haLayout`).
const HA_LAYOUT_21: u8 = 2;
/// ESP restart through `cmd/restart`: reason 0 (user), after 1 s.
const RESTART_REASON_USER: u8 = 0;
const RESTART_DELAY_MS: u32 = 1000;

const N_VALVES: usize = VALVE_COUNT as usize;
const N_TEMPS: usize = TEMP_SLOT_COUNT as usize;
const N_VOLTS: usize = VOLT_SLOT_COUNT as usize;

// ---------------------------------------------------------------- shared with other tasks

/// Manual discovery runs (`POST /api/mqtt/discovery`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiscoveryAction {
    /// Prune and publish.
    Publish = 0,
    /// Every listed and every current topic gets an empty config.
    Delete = 1,
    /// Delete, then publish.
    DeleteAndPublish = 2,
}

/// The session for `/api/status` (C++ `mqtt::Status`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MqttStatus {
    pub state: MqttState,
    /// The library state of the last failed connect or drop (PubSubClient codes -4..5).
    pub rc: i8,
    /// Sessions opened since boot.
    pub reconnects: u32,
    pub publish_failures: u32,
    pub commands_rejected: u32,
    pub events_suppressed: u32,
    pub discovery_running: bool,
    /// The client id of the current session.
    pub client_id: Text<CLIENT_ID_MAX>,
    /// The last `homeassistant/status` seen.
    pub ha_status: HaStatus,
}

/// What the MQTT task hands to the other tasks, and their requests to it (C++ the `gMux`
/// section and the `volatile` request flags). Created by `main`, read by web and stm.
#[derive(Default)]
pub struct MqttShared {
    status: Mutex<MqttStatus>,
    regulator: Mutex<RegulatorInput>,
    calib: Mutex<CalibEndTracker>,
    reconnect: AtomicBool,
    discovery: Mutex<Option<DiscoveryAction>>,
}

/// A lock that a panic elsewhere cannot poison for good (the firmware aborts on a panic).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl MqttShared {
    /// The session state (any task).
    pub fn status(&self) -> MqttStatus {
        lock(&self.status).clone()
    }

    /// What the failsafe lease needs to know about the regulator: MQTT mode, broker session,
    /// Home Assistant status, accepted commands (any task).
    pub fn regulator_state(&self) -> RegulatorInput {
        *lock(&self.regulator)
    }

    /// End of the last calibration of `valve` (0-based) seen since boot; `None` when there was
    /// none (C++ `calibrationEnd()` false).
    pub fn calibration_end(&self, valve: u8) -> Option<LocalTime> {
        let c = lock(&self.calib);
        c.ended(valve).then(|| *c.end(valve))
    }

    /// A web handler asks for a new session (applied by the next pass).
    pub fn request_reconnect(&self) {
        self.reconnect.store(true, Ordering::Release);
    }

    /// A web handler asks for a discovery run (applied by the next pass with a session; a newer
    /// request replaces one that waits).
    pub fn request_discovery(&self, a: DiscoveryAction) {
        *lock(&self.discovery) = Some(a);
    }

    fn set_state(&self, state: MqttState, rc: i8) {
        let mut s = lock(&self.status);
        s.state = state;
        s.rc = rc;
    }

    fn count(&self, field: fn(&mut MqttStatus) -> &mut u32) {
        let mut s = lock(&self.status);
        let v = field(&mut s);
        *v = v.wrapping_add(1);
    }
}

/// What the MQTT task needs from the other glue modules: one method per call of the C++ task into
/// `app`, `storage`, `logger`, `net` and `ota`. The firmware implements it over their shared
/// objects, the tests with a fake.
pub trait MqttHost {
    /// `app::uptimeS()`: seconds since boot.
    fn uptime_s(&self) -> u32;
    /// `app::submit()`: queues a command for the STM task (never blocks); false when the queue
    /// is full.
    fn submit(&self, cmd: &StmCommand) -> bool;
    /// `app::stmSnapshotRevision()`: changes whenever the STM task publishes a snapshot.
    fn stm_snapshot_revision(&self) -> u32;
    /// `app::readStmSnapshot()`: copies the latest snapshot into `out`.
    fn read_stm_snapshot(&self, out: &mut StmSnapshot);
    /// `app::readProfile()`: copies the stored profile of `valve` into `out` (count 0: none).
    fn read_profile(&self, valve: u8, out: &mut Profile);
    /// `app::calibInfo().nextEpoch`: the next scheduled calibration (UTC); 0 = none.
    fn calib_next_epoch(&self) -> i64;
    /// `storage::configRevision()`: +1 on every applied config.
    fn config_revision(&self) -> u32;
    /// `storage::getConfig()`: calls `f` with the active config.
    fn with_config(&self, f: &mut dyn FnMut(&Config));
    /// `storage::fsReady()`: LittleFS is mounted.
    fn fs_ready(&self) -> bool;
    /// `storage::haCleanupDone()`: the first-run cleanup of the legacy discovery ran.
    fn ha_cleanup_done(&self) -> bool;
    /// `storage::setHaCleanupDone()`.
    fn set_ha_cleanup_done(&self);
    /// `storage::haLayout()`: the discovery layout last published (0 = none, 2 = 2.1).
    fn ha_layout(&self) -> u8;
    /// `storage::setHaLayout()`.
    fn set_ha_layout(&self, layout: u8);
    /// `logger::log(code, valve, arg1, arg2, text)` with the default severity of `code` (`text`
    /// a C string, "" for the C++ null); returns the seq.
    fn log(&self, code: EventCode, valve: u8, arg1: i32, arg2: i32, text: &[u8]) -> u32;
    /// `logger::readSince()`: the events after seq `since`, oldest first, into `out`; their
    /// count and the cursor of the next call.
    fn read_events_since(&self, since: u32, out: &mut [Event]) -> (usize, u32);
    /// `net::isUp()`.
    fn net_up(&self) -> bool;
    /// `net::info().ip` (first octet in the low byte).
    fn net_ip(&self) -> u32;
    /// `net::localTime()`: valid only after the time sync.
    fn local_time(&self) -> LocalTime;
    /// `ota::restartPending()`.
    fn restart_pending(&self) -> bool;
    /// `ota::requestRestart(reason, delayMs)`.
    fn request_restart(&self, reason: u8, delay_ms: u32);
}

impl<T: MqttHost + ?Sized> MqttHost for &T {
    fn uptime_s(&self) -> u32 {
        (**self).uptime_s()
    }
    fn submit(&self, cmd: &StmCommand) -> bool {
        (**self).submit(cmd)
    }
    fn stm_snapshot_revision(&self) -> u32 {
        (**self).stm_snapshot_revision()
    }
    fn read_stm_snapshot(&self, out: &mut StmSnapshot) {
        (**self).read_stm_snapshot(out)
    }
    fn read_profile(&self, valve: u8, out: &mut Profile) {
        (**self).read_profile(valve, out)
    }
    fn calib_next_epoch(&self) -> i64 {
        (**self).calib_next_epoch()
    }
    fn config_revision(&self) -> u32 {
        (**self).config_revision()
    }
    fn with_config(&self, f: &mut dyn FnMut(&Config)) {
        (**self).with_config(f)
    }
    fn fs_ready(&self) -> bool {
        (**self).fs_ready()
    }
    fn ha_cleanup_done(&self) -> bool {
        (**self).ha_cleanup_done()
    }
    fn set_ha_cleanup_done(&self) {
        (**self).set_ha_cleanup_done()
    }
    fn ha_layout(&self) -> u8 {
        (**self).ha_layout()
    }
    fn set_ha_layout(&self, layout: u8) {
        (**self).set_ha_layout(layout)
    }
    fn log(&self, code: EventCode, valve: u8, arg1: i32, arg2: i32, text: &[u8]) -> u32 {
        (**self).log(code, valve, arg1, arg2, text)
    }
    fn read_events_since(&self, since: u32, out: &mut [Event]) -> (usize, u32) {
        (**self).read_events_since(since, out)
    }
    fn net_up(&self) -> bool {
        (**self).net_up()
    }
    fn net_ip(&self) -> u32 {
        (**self).net_ip()
    }
    fn local_time(&self) -> LocalTime {
        (**self).local_time()
    }
    fn restart_pending(&self) -> bool {
        (**self).restart_pending()
    }
    fn request_restart(&self, reason: u8, delay_ms: u32) {
        (**self).request_restart(reason, delay_ms)
    }
}

/// The ports and boot values of the task.
pub struct MqttPorts<'a, C, F, N, G, R> {
    pub clock: &'a C,
    pub fs: &'a F,
    pub tcp: &'a N,
    pub gate: &'a G,
    pub rtc: &'a R,
    /// Offset of the HA status record ([`HA_STATUS_RECORD_LEN`] bytes) in the RTC block (the
    /// firmware's RTC layout).
    pub rtc_offset: usize,
    /// The factory MAC (`System::base_mac`): the automatic client id.
    pub mac: [u8; 6],
}

// ---------------------------------------------------------------- task state

/// The last publish of a sensor slot (on-change detection).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SensorPub {
    /// an entry was published since connect
    valid: bool,
    ok: bool,
    /// tenths of a degree, or the voltage in thousandths of its unit
    value: i32,
}

impl SensorPub {
    const EMPTY: Self = Self {
        valid: false,
        ok: false,
        value: 0,
    };
}

const TEMPS_EMPTY: [SensorPub; N_TEMPS] = [SensorPub::EMPTY; N_TEMPS];

/// The new diag topics of a valve: last published values (valid after connect).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DiagPub {
    valid: bool,
    move_seq: u32,
    early_stops: u32,
    cmd_rejected: u32,
    cal_state: u8,
    /// StmSnapshot::profile_seq of the last profile check
    profile_seq: u32,
    profile_crc: u32,
}

impl DiagPub {
    const EMPTY: Self = Self {
        valid: false,
        move_seq: 0,
        early_stops: 0,
        cmd_rejected: 0,
        cal_state: 0,
        profile_seq: 0,
        profile_crc: 0,
    };
}

const DIAG_EMPTY: [DiagPub; N_VALVES] = [DiagPub::EMPTY; N_VALVES];

/// A counter of diag/mqtt: the last published value and when.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Paced {
    valid: bool,
    value: u32,
    at_ms: u32,
}

/// The STM level diag topics: last published values (valid after connect).
#[derive(Default)]
struct StmPub {
    valid: bool,
    proto: u8,
    link: LinkState,
    resets: u32,
    rx_overflow: u32,
    parse_err: u32,
    status_valid: bool,
    calib_active: Option<bool>,
    version: Text<31>,
    started: Option<i64>,
    lease: Option<LeaseState>,
    safe_mode: Option<bool>,
    next: Option<i64>,
    suppressed: Paced,
    rejected: Paced,
}

/// The last published values of the compat slots and the system slot.
struct Published {
    /// per valve the key of its compat fields (valve_compat_key), not the whole ValveState
    valve_key: [u32; N_VALVES],
    valve_valid: [bool; N_VALVES],
    volts: [SensorPub; N_VOLTS],
    state: Option<u8>,
    uptime: Option<u32>,
    stm_online: Option<bool>,
    failsafe: Option<bool>,
    /// message of the latest Warning+ event
    message: Text<MESSAGE_MAX>,
    message_changed: bool,
    /// common/ip only on the first full publish per connection
    first_publish: bool,
    full_running: bool,
    full_cursor: u8,
    change_cursor: u8,
    stm: StmPub,
}

impl Default for Published {
    fn default() -> Self {
        Self {
            valve_key: [0; N_VALVES],
            valve_valid: [false; N_VALVES],
            volts: [SensorPub::EMPTY; N_VOLTS],
            state: None,
            uptime: None,
            stm_online: None,
            failsafe: None,
            message: Text::new(),
            message_changed: true,
            first_publish: true,
            full_running: false,
            full_cursor: 0,
            change_cursor: 0,
            stm: StmPub::default(),
        }
    }
}

impl Published {
    /// Valve `i` went out with these compat fields.
    fn valve_published(&mut self, i: usize, key: u32) {
        if let Some(k) = self.valve_key.get_mut(i) {
            *k = key;
        }
        if let Some(v) = self.valve_valid.get_mut(i) {
            *v = true;
        }
    }
}

/// Client id and last will topic of the next session.
#[derive(Default)]
struct Names {
    client_id: Text<CLIENT_ID_MAX>,
    lwt: Text<TOPIC_MAX>,
}

/// One inbound message, copied by the poll callback and handled after the poll.
#[derive(Clone)]
struct Inbound {
    topic: Text<TOPIC_MAX>,
    payload: heapless::Vec<u8, INBOUND_PAYLOAD_MAX>,
}

/// The messages of one poll (C++ `gInbound`, `gInboundCount`, `gInboundOverflow`).
struct InboundQueue {
    slots: [Inbound; INBOUND_SLOTS],
    count: usize,
    /// messages lost to a full queue since the last drain
    overflow: u32,
}

impl InboundQueue {
    const EMPTY: Self = Self {
        slots: [const {
            Inbound {
                topic: Text::new(),
                payload: heapless::Vec::new(),
            }
        }; INBOUND_SLOTS],
        count: 0,
        overflow: 0,
    };

    /// The boot block of the queue.
    fn boxed() -> Box<Self> {
        Box::new(Self::EMPTY)
    }

    /// The poll callback: only copies (C++ `onMessage`). A topic longer than TOPIC_MAX is not
    /// one of ours (kept as an empty topic); the payload is cut at INBOUND_PAYLOAD_MAX bytes.
    fn push(&mut self, topic: &[u8], payload: &[u8]) {
        let Some(m) = self.slots.get_mut(self.count) else {
            self.overflow = self.overflow.saturating_add(1);
            return;
        };
        self.count += 1;
        m.topic.clear();
        if topic.len() <= TOPIC_MAX {
            let _ = m.topic.extend_from_slice(topic);
        }
        m.payload.clear();
        let n = payload.len().min(INBOUND_PAYLOAD_MAX);
        let _ = m
            .payload
            .extend_from_slice(payload.get(..n).unwrap_or_default());
    }
}

/// The open list files of a discovery run and the read position in its buffer.
struct ListFiles<File> {
    read: Option<File>,
    write: Option<File>,
    pos: usize,
    len: usize,
}

impl<File> Default for ListFiles<File> {
    fn default() -> Self {
        Self {
            read: None,
            write: None,
            pos: 0,
            len: 0,
        }
    }
}

/// The memory of a discovery run, on the heap for that run only: the context, and one block of
/// the payload buffer followed by the list read buffer.
struct DiscWork {
    ctx: Block<DiscoveryContext>,
    /// DISCOVERY_PAYLOAD_MAX + 1 bytes of payload, then LIST_BUFFER bytes of list
    bytes: Vec<u8>,
}

/// A profile copied for one check, and its JSON (on the heap for that check).
struct ProfileWork {
    profile: Profile,
    json: [u8; PROFILE_JSON_MAX + 1],
}

const PROFILE_WORK_EMPTY: ProfileWork = ProfileWork {
    profile: Profile {
        valve: 0,
        count: 0,
        samples: [ProfileSample {
            count: 0,
            current: 0,
        }; PROFILE_MAX_SAMPLES as usize],
    },
    json: [0; PROFILE_JSON_MAX + 1],
};

/// The broker connection and what a publish needs (C++ `gClient`, `publishRaw`, `publish`).
struct Out<'a, C: Clock, N: TcpConnector, W: Watchdog> {
    conn: MqttConn<&'a N, &'a C>,
    wdt: Option<W>,
    shared: &'a MqttShared,
    topics: TopicContext,
    /// mqtt.retained of the config
    retained: bool,
}

impl<C: Clock, N: TcpConnector, W: Watchdog> Out<'_, C, N, W> {
    fn feed(&self) {
        if let Some(w) = &self.wdt {
            w.feed();
        }
    }

    /// One message. A failed publish on a live connection is a socket write that waited its
    /// whole timeout (broker gone without FIN/RST): every further publish of the pass would wait
    /// as long, past the task watchdog, so the socket is dropped and the back-off takes over.
    fn publish_raw(&mut self, topic: &[u8], payload: &[u8], retained: bool) -> bool {
        let ok = self.conn.publish(topic, payload, retained);
        self.feed(); // a pass may publish many messages
        if ok {
            return true;
        }
        self.shared.count(|s| &mut s.publish_failures);
        if self.conn.connected() {
            self.conn.drop_socket();
        }
        false
    }

    /// `segment` for the per-item topics ("" otherwise); false also when the topic cannot be
    /// built.
    fn publish(&mut self, t: Topic, segment: &[u8], payload: &[u8]) -> bool {
        let mut topic = [0u8; TOPIC_BUF];
        let n = build_topic(&self.topics, t, segment, &mut topic);
        if n == 0 {
            return false;
        }
        let retained = topic_retained(t, self.retained);
        self.publish_raw(topic.get(..n).unwrap_or_default(), payload, retained)
    }

    fn publish_uint(&mut self, t: Topic, segment: &[u8], v: u32) -> bool {
        let mut buf = [0u8; 12];
        let n = fmt_fit(&mut buf, format_args!("{v}"));
        self.publish(t, segment, buf.get(..n).unwrap_or_default())
    }

    fn publish_counter(&mut self, t: Topic, segment: &[u8], v: u32) -> bool {
        let mut buf = [0u8; 12];
        let n = format_legacy_counter(v, &mut buf);
        self.publish(t, segment, buf.get(..n).unwrap_or_default())
    }

    /// valves/<V>/target; the value when it went out on the state form without `separate` (the
    /// echo filter knows it then).
    fn valve_target(&mut self, v: &ValveState, seg: &[u8], separate: bool) -> Option<u8> {
        let target = published_target(v, separate)?;
        let sent = self.publish_uint(Topic::ValveTarget, seg, u32::from(target));
        if separate {
            return None;
        }
        sent.then_some(target)
    }

    /// requested, sync, failsafe, problem, state and actual of a valve.
    fn valve_state(&mut self, v: &ValveState, seg: &[u8], plain_text: bool) {
        if v.desired_valid {
            self.publish_uint(Topic::ValveRequested, seg, u32::from(v.desired));
        }
        self.publish(Topic::ValveSync, seg, target_sync_name(v.sync).as_bytes());
        let fs = failsafe_kind_name(failsafe_kind(v));
        self.publish(Topic::ValveFailsafe, seg, fs.as_bytes());
        let problem: &[u8] = if valve_problem(v) { b"1" } else { b"0" };
        self.publish(Topic::ValveProblem, seg, problem);
        let mut buf = [0u8; 48];
        let n = format_valve_state(v.status, plain_text, &mut buf);
        self.publish(Topic::ValveState, seg, buf.get(..n).unwrap_or_default());
        self.publish_uint(Topic::ValveActual, seg, u32::from(v.position));
    }

    /// The end of the last calibration of valve `i` (when one was seen since boot) and the
    /// repetitions of the last calibration.
    fn valve_calibration(&mut self, i: u8, v: &ValveState, seg: &[u8]) {
        let end = {
            let c = lock(&self.shared.calib);
            c.ended(i).then(|| *c.end(i))
        };
        if let Some(end) = end {
            let mut buf = [0u8; 48];
            let n = format_calib_date(&end, &mut buf);
            if self.publish(Topic::ValveCalibDate, seg, buf.get(..n).unwrap_or_default()) {
                lock(&self.shared.calib).clear_dirty(i);
            }
        }
        self.publish_uint(
            Topic::ValveCalibRepetitions,
            seg,
            u32::from(v.calib_retries),
        );
    }

    /// The legacy diag topics of a valve.
    fn valve_diag(&mut self, v: &ValveState, seg: &[u8]) {
        self.publish_uint(Topic::ValveMeanCurrent, seg, u32::from(v.mean_current));
        self.publish_counter(Topic::ValveOpenCount, seg, v.open_count);
        self.publish_counter(Topic::ValveCloseCount, seg, v.close_count);
        let mut buf = [0u8; 12];
        let n = fmt_fit(&mut buf, format_args!("{}", v.dead_zone));
        self.publish(
            Topic::ValveDeadZoneCount,
            seg,
            buf.get(..n).unwrap_or_default(),
        );
        self.publish_counter(Topic::ValveMoves, seg, v.moves);
    }

    /// A counter topic of diag/mqtt: on change, at most every COUNTER_PACE_MS.
    fn publish_paced(&mut self, t: Topic, value: u32, p: &mut Paced, now: u32) {
        if p.valid && (value == p.value || elapsed_ms(now, p.at_ms) < COUNTER_PACE_MS) {
            return;
        }
        if !self.publish_uint(t, b"", value) {
            return;
        }
        *p = Paced {
            valid: true,
            value,
            at_ms: now,
        };
    }
}

/// The discovery port of one step: retained publishes and the list files (C++ `ListPort`).
struct ListPort<'p, 'a, C: Clock, F: Fs, N: TcpConnector, W: Watchdog, H: MqttHost> {
    out: &'p mut Out<'a, C, N, W>,
    fs: &'a F,
    host: &'p H,
    files: &'p mut ListFiles<F::File>,
    buf: &'p mut [u8],
}

impl<C: Clock, F: Fs, N: TcpConnector, W: Watchdog, H: MqttHost> DiscoveryPort
    for ListPort<'_, '_, C, F, N, W, H>
{
    fn publish(&mut self, topic: &[u8], payload: &[u8]) -> bool {
        self.out.publish_raw(topic, payload, true)
    }
    fn list_open(&mut self) -> bool {
        if !self.host.fs_ready() {
            return false;
        }
        self.files.read = self.fs.open(LIST_FILE, OpenMode::Read);
        self.files.pos = 0;
        self.files.len = 0;
        self.files.read.is_some()
    }
    fn list_read(&mut self) -> Option<u8> {
        let f = self.files.read.as_mut()?;
        if self.files.pos >= self.files.len {
            self.files.len = f.read(self.buf);
            self.files.pos = 0;
        }
        if self.files.pos >= self.files.len {
            return None;
        }
        let b = self.buf.get(self.files.pos).copied();
        self.files.pos += 1;
        b
    }
    fn list_close(&mut self) {
        self.files.read = None;
    }
    fn list_begin(&mut self) -> bool {
        if !self.host.fs_ready() {
            return false;
        }
        self.files.write = self.fs.open(LIST_TMP, OpenMode::Write);
        self.files.write.is_some()
    }
    fn list_write(&mut self, topic: &[u8]) -> bool {
        let Some(f) = self.files.write.as_mut() else {
            return false;
        };
        f.write(topic) == topic.len() && f.write(b"\n") == 1
    }
    fn list_commit(&mut self) -> bool {
        self.files.write = None;
        self.fs.rename(LIST_TMP, LIST_FILE)
    }
    fn list_abort(&mut self) {
        self.files.write = None;
        if self.host.fs_ready() {
            self.fs.remove(LIST_TMP);
        }
    }
}

/// CRC-32 of a profile's bytes as the C++ laid them out (valve, count, 2 bytes of padding, then
/// per sample count, current and 2 bytes of padding, little endian), the padding as zeros: only
/// the fields may make a profile new.
fn profile_crc(p: &Profile) -> u32 {
    let mut crc = crc32(&[p.valve, p.count, 0, 0], 0);
    for s in &p.samples {
        let [c0, c1, c2, c3] = s.count.to_le_bytes();
        let [i0, i1] = s.current.to_le_bytes();
        crc = crc32(&[c0, c1, c2, c3, i0, i1, 0, 0], crc);
    }
    crc
}

/// The temperature readings of a snapshot (C++ `temps` with `tempCount`).
fn temp_readings(s: &StmSnapshot) -> &[TempReading] {
    s.temps.get(..usize::from(s.temp_count)).unwrap_or(&s.temps)
}

/// The voltage readings of a snapshot (C++ `volts` with `voltCount`).
fn volt_readings(s: &StmSnapshot) -> &[VoltReading] {
    s.volts.get(..usize::from(s.volt_count)).unwrap_or(&s.volts)
}

/// What discovery reads from the config and the snapshot.
fn discovery_inputs<'s>(cfg: &'s Config, snap: &'s StmSnapshot, ip: u32) -> DiscoveryInputs<'s> {
    DiscoveryInputs {
        cfg: Some(cfg),
        valves: Some(&snap.valves),
        temps: temp_readings(snap),
        volts: volt_readings(snap),
        sensors_settled: snap.sensors_settled,
        stm_proto: snap.proto,
        stm_hw: &snap.version.hw,
        ip,
        sw_version: firmware_version().as_bytes(),
    }
}

/// The publish cadence of the config.
fn scheduler_params(c: &Config) -> PublishSchedulerParams {
    PublishSchedulerParams {
        on_change: c.mqtt.on_change,
        publish_interval_ms: u32::from(c.mqtt.publish_interval_s) * 1000,
        min_delay_ms: u32::from(c.mqtt.min_delay_s) * 1000,
    }
}

/// The volt value of a slot in thousandths for the on-change comparison (C++
/// `static_cast<int32_t>(value * 1000.0)`; Rust saturates where that cast is undefined).
fn milli(value: f64) -> i32 {
    (value * 1000.0) as i32
}

/// The segment of valve `i` (0-based).
fn segment(segments: &Segments, i: usize) -> &[u8] {
    segments.get(i).map(|s| s.as_slice()).unwrap_or_default()
}

/// The MQTT task.
pub struct MqttClient<'a, C, F, N, G, R, W, H>
where
    C: Clock,
    F: Fs,
    N: TcpConnector,
    G: HeapGate,
    R: Rtc,
    W: Watchdog,
    H: MqttHost,
{
    clock: &'a C,
    fs: &'a F,
    gate: &'a G,
    rtc: &'a R,
    rtc_offset: usize,
    mac: [u8; 6],
    host: H,
    out: Out<'a, C, N, W>,
    // the task's copies (boot blocks: too large for a stack)
    cfg: Box<Config>,
    cfg_revision: u32,
    snap: Box<StmSnapshot>,
    snap_revision: Option<u32>,
    segments: Box<Segments>,
    names: Box<Names>,
    clean_next: bool,
    config_loaded: bool,
    scheduler: Box<PublishScheduler>,
    limiter: Box<EventRateLimiter>,
    aggregator: Box<EventAggregator>,
    event_cursor: u32,
    published: Box<Published>,
    temps: Box<[SensorPub; N_TEMPS]>,
    diag: Box<[DiagPub; N_VALVES]>,
    // discovery
    disc: Option<DiscWork>,
    run: Box<DiscoveryRun>,
    list: Box<ListFiles<F::File>>,
    disc_key: u32,
    disc_key_revision: Option<u32>,
    auto_run: DiscoveryGate,
    // inbound
    inbound: Box<InboundQueue>,
    echo: EchoFilter,
    reject_log: RejectLog,
    buttons: Box<ButtonGate>,
    latch: TargetLatch,
    latch_cursor: u8,
    regulator: RegulatorWatch,
    // session
    offline_sent: bool,
    connected: bool,
    pacer: ReconnectPacer,
}

impl<'a, C, F, N, G, R, W, H> MqttClient<'a, C, F, N, G, R, W, H>
where
    C: Clock,
    F: Fs,
    N: TcpConnector,
    G: HeapGate,
    R: Rtc,
    W: Watchdog,
    H: MqttHost,
{
    /// Boot: the client with its packet buffer ([`BUFFER_SIZE`], allocated once) and the task's
    /// boot blocks; no session yet. [`begin`](Self::begin) follows.
    pub fn new(ports: MqttPorts<'a, C, F, N, G, R>, host: H, shared: &'a MqttShared) -> Self {
        Self {
            clock: ports.clock,
            fs: ports.fs,
            gate: ports.gate,
            rtc: ports.rtc,
            rtc_offset: ports.rtc_offset,
            mac: ports.mac,
            host,
            out: Out {
                conn: MqttConn::new(ports.tcp, ports.clock, BUFFER_SIZE),
                wdt: None,
                shared,
                topics: TopicContext::default(),
                retained: false,
            },
            cfg: Box::default(),
            cfg_revision: 0,
            snap: Box::default(),
            snap_revision: None,
            segments: Box::default(),
            names: Box::default(),
            clean_next: false,
            config_loaded: false,
            scheduler: Box::default(),
            limiter: Box::default(),
            aggregator: Box::default(),
            event_cursor: 0,
            published: Box::default(),
            temps: Box::new(TEMPS_EMPTY),
            diag: Box::new(DIAG_EMPTY),
            disc: None,
            run: Box::default(),
            list: Box::default(),
            disc_key: 0,
            disc_key_revision: None,
            auto_run: DiscoveryGate::default(),
            inbound: InboundQueue::boxed(),
            echo: EchoFilter::default(),
            reject_log: RejectLog::default(),
            buttons: Box::default(),
            latch: TargetLatch::default(),
            latch_cursor: 0,
            regulator: RegulatorWatch::default(),
            offline_sent: false,
            connected: false,
            pacer: ReconnectPacer::new(BACKOFF_MIN_MS, BACKOFF_MAX_MS),
        }
    }

    /// Boot (C++ `mqtt::begin`): the HA status of the RTC record, the config, the regulator
    /// state.
    pub fn begin(&mut self) {
        let mut b = [0u8; HA_STATUS_RECORD_LEN];
        self.rtc.load(self.rtc_offset, &mut b);
        let [m0, m1, m2, m3, status, pad, c0, c1] = b;
        let record = HaStatusRecord {
            magic: u32::from_le_bytes([m0, m1, m2, m3]),
            status,
            pad,
            crc: u16::from_le_bytes([c0, c1]),
        };
        self.regulator.restore(decode_ha_status_record(&record));
        self.reload_config();
        self.update_regulator();
        self.store_ha_status();
    }

    /// The start of the task on its own thread: its watchdog subscription; the events since boot
    /// feed `common/message` and `<main>events` (the cursor starts at the first event).
    pub fn start(&mut self, watchdog: W) {
        self.out.wdt = Some(watchdog);
        self.event_cursor = 0;
    }

    /// One loop pass; returns the delay before the next one in ms ([`DISCOVERY_PACE_MS`] with a
    /// session, [`WAIT_MS`] without, [`IDLE_MS`] with MQTT off or the network down).
    pub fn pass(&mut self) -> u32 {
        self.out.feed();
        let now = self.clock.now_ms();
        if self.host.config_revision() != self.cfg_revision {
            self.reload_config();
        }
        self.observe();
        self.update_regulator();

        if self.host.restart_pending() {
            if !self.offline_sent {
                self.offline_sent = true;
                self.disconnect_clean();
                self.out.shared.set_state(MqttState::Disabled, 0);
            }
            return WAIT_MS;
        }
        let off = self.cfg.mqtt.mode == MqttMode::Off;
        if off || !self.host.net_up() {
            self.disconnect_clean();
            let state = if off {
                MqttState::Disabled
            } else {
                MqttState::Connecting
            };
            self.out.shared.set_state(state, 0);
            self.service_events(now); // keep the message tracking and the cursor current
            return IDLE_MS;
        }
        if self.out.shared.reconnect.swap(false, Ordering::AcqRel) {
            self.disconnect_clean();
            self.pacer.force_now();
        }
        if !self.out.conn.connected() {
            self.on_disconnected();
            if self.pacer.due(now) && !self.connect(now) {
                self.pacer.on_attempt_failed(self.clock.now_ms());
            }
            self.update_regulator();
            self.service_events(now);
            return WAIT_MS;
        }
        self.pacer.tick(now, true);
        let requested = lock(&self.out.shared.discovery).take();
        if let Some(a) = requested {
            // without memory for the run the request stays for the next pass
            if !self.start_requested(a) {
                lock(&self.out.shared.discovery).get_or_insert(a);
            }
        } else {
            self.check_discovery_inputs(now);
            self.service_auto_run(now);
        }
        self.pump();
        self.service_events(now);
        self.service_full_publish(now);
        self.pump();
        self.service_on_change(now);
        self.service_diag(now);
        self.service_discovery();
        DISCOVERY_PACE_MS
    }

    /// What the task hands to the other tasks.
    pub fn shared(&self) -> &'a MqttShared {
        self.out.shared
    }

    // ------------------------------------------------------------ regulator, config

    /// The HA status goes to the status, the regulator state and the RTC record.
    fn store_ha_status(&mut self) {
        let s = self.regulator.ha_status();
        let mut r = HaStatusRecord::default();
        encode_ha_status_record(s, &mut r);
        let [m0, m1, m2, m3] = r.magic.to_le_bytes();
        let [c0, c1] = r.crc.to_le_bytes();
        let bytes = [m0, m1, m2, m3, r.status, r.pad, c0, c1];
        self.rtc.store(self.rtc_offset, &bytes);
        lock(&self.out.shared.status).ha_status = s;
        lock(&self.out.shared.regulator).ha = s;
    }

    /// The regulator view of the lease: mode, broker session, HA status.
    fn update_regulator(&mut self) {
        let mut r = lock(&self.out.shared.regulator);
        r.mode = self.cfg.mqtt.mode;
        r.broker_connected = self.connected;
        r.ha = self.regulator.ha_status();
    }

    fn reload_config(&mut self) {
        self.cfg_revision = self.host.config_revision();
        let reconnect = self.copy_config();
        self.apply_topics();
        self.apply_names();
        self.scheduler.configure(scheduler_params(&self.cfg));
        // the config found at boot is the one the broker session belongs to
        if reconnect && self.config_loaded {
            self.clean_next = true;
            if self.out.conn.connected() {
                self.out.conn.disconnect(); // clean: the broker does not send the last will
                self.on_disconnected();
                self.pacer.force_now(); // not a failed attempt
            }
        }
        self.config_loaded = true;
    }

    /// The active config into the task's copy; true when its topics or session differ.
    fn copy_config(&mut self) -> bool {
        let mut reconnect = false;
        let cfg: &mut Config = &mut self.cfg;
        self.host.with_config(&mut |active: &Config| {
            reconnect = mqtt_topic_config_changed(active, cfg);
            cfg.clone_from(active);
        });
        reconnect
    }

    /// The topic settings and the valve segments of the config.
    fn apply_topics(&mut self) {
        copy_string(&mut self.out.topics.station, mqtt_root_topic(&self.cfg));
        self.out.topics.path_as_root = self.cfg.mqtt.path_as_root;
        self.out.topics.separate = self.cfg.mqtt.separate;
        self.out.retained = self.cfg.mqtt.retained;
        for (i, seg) in (0..VALVE_COUNT).zip(self.segments.iter_mut()) {
            let mut buf = [0u8; SEGMENT_BUF];
            let n = item_segment(&self.cfg, ItemKind::Valve, i, &mut buf);
            copy_string(seg, buf.get(..n).unwrap_or_default());
        }
    }

    /// Client id (configured, else from the station and the MAC) and last will topic.
    fn apply_names(&mut self) {
        if c_str(&self.cfg.mqtt.client_id).is_empty() {
            let mut buf = [0u8; CLIENT_ID_BUF];
            let n = build_mqtt_client_id(&self.cfg.station, &self.mac, &mut buf);
            copy_string(&mut self.names.client_id, buf.get(..n).unwrap_or_default());
        } else {
            copy_string(&mut self.names.client_id, &self.cfg.mqtt.client_id);
        }
        let mut lwt = [0u8; TOPIC_BUF];
        let n = build_topic(&self.out.topics, Topic::Status, b"", &mut lwt);
        copy_string(&mut self.names.lwt, lwt.get(..n).unwrap_or_default());
    }

    // ------------------------------------------------------------ discovery

    /// Runs `f` with the run, its context, the port of one step and the payload buffer; `None`
    /// without the memory of a run.
    fn with_port<T>(
        &mut self,
        f: impl FnOnce(
            &mut DiscoveryRun,
            &DiscoveryContext,
            &mut ListPort<'_, 'a, C, F, N, W, H>,
            &mut [u8],
        ) -> T,
    ) -> Option<T> {
        let work = self.disc.as_mut()?;
        let (payload, buf) = work.bytes.split_at_mut_checked(PAYLOAD_BUF)?;
        let mut port = ListPort {
            out: &mut self.out,
            fs: self.fs,
            host: &self.host,
            files: &mut self.list,
            buf,
        };
        Some(f(&mut self.run, &work.ctx, &mut port, payload))
    }

    fn abort_run(&mut self) {
        self.with_port(|run, _, port, _| run.abort(port));
    }

    fn set_discovery_running(&self, on: bool) {
        lock(&self.out.shared.status).discovery_running = on;
    }

    /// The context and the buffers of a run (two heap blocks for the run).
    fn alloc_run(&self) -> Option<DiscWork> {
        let ctx = try_block(self.gate, DiscoveryContext::default)?;
        let bytes = try_bytes(self.gate, RUN_BYTES)?;
        Some(DiscWork { ctx, bytes })
    }

    /// False without memory for the run: the caller keeps its request for the next pass. A run
    /// also answers a waiting automatic one (the user's request wins).
    fn start_run(&mut self, plan: &DiscoveryPlan) -> bool {
        if self.run.running() {
            self.abort_run();
        }
        if self.disc.is_none() {
            self.disc = self.alloc_run();
        }
        let Some(work) = self.disc.as_mut() else {
            return false;
        };
        let input = discovery_inputs(&self.cfg, &self.snap, self.host.net_ip());
        build_discovery_context(&input, &mut work.ctx);
        self.disc_key = discovery_input_key(&input);
        self.disc_key_revision = self.snap_revision;
        self.run.start(plan);
        self.auto_run.clear();
        self.set_discovery_running(true);
        true
    }

    fn end_run(&mut self) {
        self.disc = None;
        self.set_discovery_running(false);
    }

    /// A publish run (with the first-run cleanup and the 2.0.0 migration when due).
    fn publish_plan(&self) -> DiscoveryPlan {
        DiscoveryPlan {
            publish: true,
            drop_legacy: !self.host.ha_cleanup_done(),
            retire20: self.host.ha_layout() < HA_LAYOUT_21,
            ..DiscoveryPlan::default()
        }
    }

    /// Manual requests run in modes 1 and 2; false: no memory, try again.
    fn start_requested(&mut self, a: DiscoveryAction) -> bool {
        if self.cfg.mqtt.mode == MqttMode::Off {
            return true;
        }
        let mut p = if a == DiscoveryAction::Delete {
            DiscoveryPlan::default()
        } else {
            self.publish_plan()
        };
        p.remove_all = a != DiscoveryAction::Publish;
        p.prune = a != DiscoveryAction::Delete;
        self.start_run(&p)
    }

    /// The automatic run of a session: the publish run in HA mode (with haDiscoveryOnConnect, or
    /// the 2.0.0 migration due), else the legacy cleanup while it is due.
    fn auto_plan(&self) -> Option<DiscoveryPlan> {
        let ha = self.cfg.mqtt.mode == MqttMode::MqttHa;
        if ha && (self.cfg.mqtt.ha_discovery_on_connect || self.host.ha_layout() < HA_LAYOUT_21) {
            return Some(self.publish_plan());
        }
        if self.host.ha_cleanup_done() {
            return None;
        }
        Some(DiscoveryPlan {
            drop_legacy: true,
            ..DiscoveryPlan::default()
        })
    }

    /// After a connect: a publish run waits for the gate; the legacy cleanup of mode 1 publishes
    /// no config and keeps what is not known yet, so it starts at once (without memory it goes
    /// through the gate too).
    fn start_on_connect(&mut self, now: u32) {
        let Some(p) = self.auto_plan() else {
            return;
        };
        if p.publish || !self.start_run(&p) {
            self.auto_run.request(now);
        }
    }

    /// A requested automatic run starts once the STM inputs settled (or the gate stopped
    /// waiting), never over a running one.
    fn service_auto_run(&mut self, now: u32) {
        if self.run.running() || !self.auto_run.due(self.snap.sensors_settled, now) {
            return;
        }
        match self.auto_plan() {
            None => self.auto_run.clear(),
            // clears the request; without memory it stays for the next pass
            Some(p) => {
                self.start_run(&p);
            }
        }
    }

    fn service_discovery(&mut self) {
        if !self.run.running() {
            return;
        }
        let step = self.with_port(|run, ctx, port, payload| {
            let mut jw = JsonWriter::new(payload);
            run.step(ctx, port, &mut jw)
        });
        let Some(ph) = step else {
            return;
        };
        if ph == DiscoveryRunPhase::Aborted {
            return self.end_run();
        }
        if ph != DiscoveryRunPhase::Done {
            return;
        }
        self.end_run();
        let s = *self.run.stats();
        let text: &[u8] = if s.skipped > 0 {
            b"skipped entities"
        } else {
            b""
        };
        self.host.log(
            EventCode::HaDiscoverySent,
            NO_VALVE,
            i32::from(s.configs),
            i32::from(s.deletes),
            text,
        );
        if self.run.plan().drop_legacy {
            self.host.set_ha_cleanup_done();
        }
        if self.run.plan().publish {
            self.host.set_ha_layout(HA_LAYOUT_21);
        }
    }

    /// A valve sensor or a published sensor segment changed after the last run (the STM
    /// reports assignments only after its first 1-Wire read): another automatic run, through
    /// the gate like the one after a connect.
    fn check_discovery_inputs(&mut self, now: u32) {
        if self.run.running()
            || self.cfg.mqtt.mode != MqttMode::MqttHa
            || !self.cfg.mqtt.ha_discovery_on_connect
            || self.disc_key_revision == self.snap_revision
        {
            return;
        }
        self.disc_key_revision = self.snap_revision;
        let input = discovery_inputs(&self.cfg, &self.snap, self.host.net_ip());
        if discovery_input_key(&input) != self.disc_key {
            self.auto_run.request(now);
        }
    }

    // ------------------------------------------------------------ inbound

    fn reject_command(&mut self, reason: RejectReason, valve: u8, detail: i32) {
        self.out.shared.count(|s| &mut s.commands_rejected);
        let key = if (0..VALVE_COUNT).contains(&valve) {
            valve + 1
        } else {
            0
        };
        if !self.reject_log.should_log(key, reason, self.clock.now_ms()) {
            return;
        }
        self.host.log(
            EventCode::MqttCommandRejected,
            NO_VALVE,
            i32::from(key),
            detail,
            reject_reason_name(reason).as_bytes(),
        );
    }

    fn accepted(&mut self) {
        {
            let mut r = lock(&self.out.shared.regulator);
            r.command_seq = r.command_seq.wrapping_add(1);
        }
        if self.cfg.mqtt.mode == MqttMode::MqttHa
            && self.regulator.on_inbound_command() != RegulatorWatchChange::None
        {
            self.store_ha_status();
        }
    }

    fn submit(&self, kind: StmCommandType, valve: u8) -> bool {
        self.host.submit(&StmCommand {
            kind,
            valve,
            ..StmCommand::default()
        })
    }

    fn submit_target(&mut self, valve: u8, pos: u8) {
        let ok = self.host.submit(&StmCommand {
            kind: StmCommandType::SetTarget,
            valve,
            pos,
            source: TargetSource::Mqtt,
            ..StmCommand::default()
        });
        if ok {
            self.latch.clear(valve);
        } else {
            self.latch.set(valve, pos); // submitted again until the queue takes it
        }
    }

    fn act(&mut self, d: &InboundDecision) {
        let ok = match d.action {
            InboundAction::SetTarget => {
                self.submit_target(d.valve, d.pos);
                true
            }
            InboundAction::StopValve => self.submit(StmCommandType::StopValve, d.valve),
            InboundAction::CalibrateValve => self.submit(StmCommandType::Calibrate, d.valve),
            InboundAction::CalibrateAll => self.submit(StmCommandType::Calibrate, ALL_VALVES),
            InboundAction::Restart => {
                self.host
                    .request_restart(RESTART_REASON_USER, RESTART_DELAY_MS);
                true
            }
            InboundAction::StmReset => self.submit(StmCommandType::ResetStm, NO_VALVE),
            InboundAction::Detect => self.submit(StmCommandType::Detect, ALL_VALVES),
            InboundAction::StopAll => self.submit(StmCommandType::StopValve, ALL_VALVES),
            InboundAction::StmSafeExit => self.submit(StmCommandType::LeaveSafeMode, NO_VALVE),
            InboundAction::Ignore
            | InboundAction::Reject
            | InboundAction::HaOnline
            | InboundAction::HaOffline => return,
        };
        if ok {
            self.accepted();
        } else {
            self.reject_command(RejectReason::QueueFull, d.valve, 0);
        }
    }

    fn on_ha_status(&mut self, d: &InboundDecision) {
        let p: &[u8] = if d.action == InboundAction::HaOnline {
            b"online"
        } else {
            b"offline"
        };
        let ch = self.regulator.on_ha_status(p);
        self.store_ha_status();
        if ch == RegulatorWatchChange::CameOnline
            && self.cfg.mqtt.ha_discovery_on_connect
            && !self.run.running()
            && !self.start_run(&self.publish_plan())
        {
            self.auto_run.request(self.clock.now_ms()); // no memory now: an automatic run later
        }
    }

    fn handle_inbound(&mut self, m: &Inbound) {
        let ic = InboundContext {
            topics: Some(&self.out.topics),
            ha_prefix: &self.cfg.mqtt.discovery_prefix,
            segments: Some(&self.segments),
            active_mask: active_valve_mask(&self.cfg),
            mode: self.cfg.mqtt.mode,
            stm_v3: self.snap.proto >= 3,
            echo: Some(&self.echo),
        };
        if inbound_is_clear_echo(&ic, &m.topic, &m.payload) {
            if let Some(d) = self.buttons.confirm(&m.topic) {
                self.act(&d);
            }
            return;
        }
        let d = decide_inbound(&ic, &m.topic, &m.payload);
        if d.action == InboundAction::Ignore {
            return;
        }
        if matches!(d.action, InboundAction::HaOnline | InboundAction::HaOffline) {
            return self.on_ha_status(&d);
        }
        let cleared = !d.clear_retained || self.out.publish_raw(&m.topic, b"", true);
        if d.action == InboundAction::Reject {
            self.reject_command(d.reason, d.valve, d.detail);
        } else if !inbound_is_button(d.action) {
            self.act(&d);
        } else if !cleared {
            self.reject_command(RejectReason::ClearNotConfirmed, d.valve, 0);
        } else if !self.buttons.hold(&d, &m.topic, self.clock.now_ms()) {
            self.reject_command(RejectReason::QueueFull, d.valve, 0);
        }
    }

    fn drain_inbound(&mut self) {
        for i in 0..self.inbound.count {
            let Some(m) = self.inbound.slots.get(i).cloned() else {
                break;
            };
            self.handle_inbound(&m);
        }
        self.inbound.count = 0;
        while self.inbound.overflow > 0 {
            self.inbound.overflow -= 1;
            self.reject_command(RejectReason::QueueFull, NO_VALVE, 0);
        }
        while let Some(d) = self.buttons.expire(self.clock.now_ms()) {
            self.reject_command(RejectReason::ClearNotConfirmed, d.valve, 0);
        }
        if let Some((valve, pos)) = self.latch.next(self.latch_cursor) {
            self.latch_cursor = (valve + 1) % VALVE_COUNT;
            self.submit_target(valve, pos);
        }
    }

    /// One poll of the connection plus the handling of what it delivered.
    fn pump(&mut self) {
        let inbound = &mut self.inbound;
        self.out
            .conn
            .poll(|topic, payload| inbound.push(topic, payload));
        self.drain_inbound();
    }

    // ------------------------------------------------------------ snapshot

    /// Reads a new snapshot when there is one and tracks the calibration ends (needed for
    /// calibration/date even while disconnected).
    fn observe(&mut self) {
        let rev = self.host.stm_snapshot_revision();
        if self.snap_revision == Some(rev) {
            return;
        }
        self.snap_revision = Some(rev);
        self.host.read_stm_snapshot(&mut self.snap);
        let now = self.host.local_time();
        lock(&self.out.shared.calib).observe(Some(&self.snap.valves), &now);
    }

    fn failsafe_active(&self) -> bool {
        self.snap.lease.state == LeaseState::Expired
    }

    fn safe_mode(&self) -> bool {
        self.snap.have_status && self.snap.status.v3 && self.snap.status.safe_mode
    }

    fn current_system_state(&self) -> u8 {
        let f = SystemFlags {
            safe_mode: self.safe_mode(),
            failsafe: self.failsafe_active(),
        };
        system_state(
            self.snap.link,
            &self.snap.valves,
            active_valve_mask(&self.cfg),
            f,
        )
    }

    fn slot_temp(&self, slot1: u8) -> Option<i32> {
        slot_temp_tenths(
            &self.cfg,
            temp_readings(&self.snap),
            slot1,
            self.clock.now_ms(),
            SENSOR_STALE_MS,
        )
    }

    fn slot_volt(&self, i: u8) -> Option<f64> {
        slot_volt_value(
            &self.cfg,
            volt_readings(&self.snap),
            i,
            self.clock.now_ms(),
            SENSOR_STALE_MS,
        )
    }

    // ------------------------------------------------------------ compat slots

    fn publish_common(&mut self, full: bool) {
        let mut buf = [0u8; TOPIC_BUF];
        if full && self.published.first_publish {
            let n = format_ipv4(self.host.net_ip(), &mut buf);
            let ip = buf.get(..n).unwrap_or_default();
            if self.out.publish(Topic::CommonIp, b"", ip) {
                self.published.first_publish = false;
            }
        }
        let st = self.current_system_state();
        let n = format_system_state(st, self.cfg.mqtt.plain_text, &mut buf);
        if self
            .out
            .publish(Topic::CommonState, b"", buf.get(..n).unwrap_or_default())
        {
            self.published.state = Some(st);
        }
        if self.cfg.mqtt.up_time {
            let up = self.host.uptime_s();
            let n = format_uptime(up, &mut buf);
            if self
                .out
                .publish(Topic::CommonUptime, b"", buf.get(..n).unwrap_or_default())
            {
                self.published.uptime = Some(up);
            }
        }
        if self
            .out
            .publish(Topic::CommonMessage, b"", &self.published.message)
        {
            self.published.message_changed = false;
        }
    }

    fn publish_valve_temp(&mut self, t: Topic, i: usize, raw: i16, slot1: u8) {
        if raw == TEMP_UNASSIGNED {
            return;
        }
        let mut tenths = i32::from(raw);
        if (1..=TEMP_SLOT_COUNT).contains(&slot1) {
            if let Some(s) = self.cfg.temps.get(usize::from(slot1) - 1) {
                tenths += i32::from(s.offset);
            }
        }
        let mut buf = [0u8; 16];
        let german = self.cfg.mqtt.german_decimal;
        let n = format_temp(tenths, temp_raw_valid(raw), german, &mut buf);
        let seg = segment(&self.segments, i);
        self.out.publish(t, seg, buf.get(..n).unwrap_or_default());
    }

    fn publish_valve(&mut self, i: usize) {
        let Some(v) = self.snap.valves.get(i) else {
            return;
        };
        self.published.valve_published(i, valve_compat_key(v));
        if !self.cfg.valves.get(i).is_some_and(|c| c.active) || !v.known {
            return;
        }
        let seg = segment(&self.segments, i);
        let m = &self.cfg.mqtt;
        if let Some(target) = self.out.valve_target(v, seg, m.separate) {
            self.echo.published(i as u8, target);
        }
        self.out.valve_state(v, seg, m.plain_text);
        self.out.valve_calibration(i as u8, v, seg);
        if m.diag {
            self.out.valve_diag(v, seg);
        }
        let (t1, t2, [s1, s2]) = (v.temp1, v.temp2, v.sensor_slot);
        self.publish_valve_temp(Topic::ValveTemp1, i, t1, s1);
        self.publish_valve_temp(Topic::ValveTemp2, i, t2, s2);
    }

    fn publish_temp(&mut self, i: u8) {
        let value = self.slot_temp(i + 1);
        if let Some(p) = self.temps.get_mut(usize::from(i)) {
            *p = SensorPub {
                valid: true,
                ok: value.is_some(),
                value: value.unwrap_or(0),
            };
        }
        if !temp_published(&self.cfg, Some(&self.snap.valves), i) {
            return;
        }
        let Some(s) = self.cfg.temps.get(usize::from(i)) else {
            return;
        };
        let mut seg = [0u8; SEGMENT_BUF];
        let bus = find_temp_bus(temp_readings(&self.snap), &s.id);
        let sl = sensor_topic_segment(&self.cfg, ItemKind::Temp, i, bus, &mut seg);
        if sl == 0 {
            return; // unnamed and not on the bus
        }
        let seg = seg.get(..sl).unwrap_or_default();
        let mut buf = [0u8; ONE_WIRE_ID_TEXT_LEN + 1];
        let n = format_one_wire_id(&s.id, &mut buf);
        self.out
            .publish(Topic::TempId, seg, buf.get(..n).unwrap_or_default());
        let german = self.cfg.mqtt.german_decimal;
        let n = format_temp(value.unwrap_or(0), value.is_some(), german, &mut buf);
        self.out
            .publish(Topic::TempValue, seg, buf.get(..n).unwrap_or_default());
    }

    fn publish_volt(&mut self, i: u8) {
        let value = self.slot_volt(i);
        if let Some(p) = self.published.volts.get_mut(usize::from(i)) {
            *p = SensorPub {
                valid: true,
                ok: value.is_some(),
                value: value.map_or(0, milli),
            };
        }
        if !volt_published_mqtt(&self.cfg, i) {
            return;
        }
        let Some(s) = self.cfg.volts.get(usize::from(i)) else {
            return;
        };
        let mut seg = [0u8; SEGMENT_BUF];
        let bus = find_volt_bus(volt_readings(&self.snap), &s.id);
        let sl = sensor_topic_segment(&self.cfg, ItemKind::Volt, i, bus, &mut seg);
        if sl == 0 {
            return;
        }
        let seg = seg.get(..sl).unwrap_or_default();
        let mut buf = [0u8; ONE_WIRE_ID_TEXT_LEN + 1];
        let n = format_one_wire_id(&s.id, &mut buf);
        self.out
            .publish(Topic::VoltId, seg, buf.get(..n).unwrap_or_default());
        let german = self.cfg.mqtt.german_decimal;
        let n = format_volt(value.unwrap_or(0.0), value.is_some(), german, &mut buf);
        self.out
            .publish(Topic::VoltValue, seg, buf.get(..n).unwrap_or_default());
        self.out.publish(Topic::VoltUnit, seg, &s.unit);
    }

    fn publish_stm_full(&mut self) {
        if !self.cfg.mqtt.new_diag || self.snap.proto < 2 || !self.snap.have_status {
            return;
        }
        self.out
            .publish_uint(Topic::DiagStmUptime, b"", self.snap.status.uptime_s);
    }

    /// stm/status and failsafe: always published, retained.
    fn publish_system(&mut self) {
        let online = stm_online(self.snap.link);
        let status: &[u8] = if online { b"online" } else { b"offline" };
        if self.out.publish(Topic::StmStatus, b"", status) {
            self.published.stm_online = Some(online);
        }
        let fs = self.failsafe_active();
        let failsafe: &[u8] = if fs { b"1" } else { b"0" };
        if self.out.publish(Topic::Failsafe, b"", failsafe) {
            self.published.failsafe = Some(fs);
        }
    }

    fn publish_slot(&mut self, slot: u8, full: bool) {
        if slot == SLOT_COMMON {
            self.publish_common(full);
        } else if slot < SLOT_TEMP0 {
            self.publish_valve(usize::from(slot - SLOT_VALVE0));
        } else if slot < SLOT_VOLT0 {
            self.publish_temp(slot - SLOT_TEMP0);
        } else if slot < SLOT_STM {
            self.publish_volt(slot - SLOT_VOLT0);
        } else if slot == SLOT_STM {
            self.publish_stm_full();
        } else {
            self.publish_system();
        }
    }

    fn system_changed(&self) -> bool {
        self.published.stm_online != Some(stm_online(self.snap.link))
            || self.published.failsafe != Some(self.failsafe_active())
    }

    fn slot_changed(&self, slot: u8) -> bool {
        if slot == SLOT_COMMON {
            // the uptime text changes every second; min_delay_s spaces it further
            return (self.cfg.mqtt.up_time && self.published.uptime != Some(self.host.uptime_s()))
                || self.published.message_changed
                || self.published.state != Some(self.current_system_state());
        }
        if slot < SLOT_TEMP0 {
            let v = slot - SLOT_VALVE0;
            let i = usize::from(v);
            let valid = self.published.valve_valid.get(i).copied().unwrap_or(false);
            let key = self.published.valve_key.get(i).copied().unwrap_or(0);
            let dirty = lock(&self.out.shared.calib).dirty(v);
            let state = self.snap.valves.get(i);
            return !valid || dirty || state.is_some_and(|s| valve_compat_key(s) != key);
        }
        if slot < SLOT_VOLT0 {
            let i = slot - SLOT_TEMP0;
            let value = self.slot_temp(i + 1);
            let p = self.temps.get(usize::from(i)).copied().unwrap_or_default();
            return !p.valid || p.ok != value.is_some() || value.is_some_and(|v| p.value != v);
        }
        if slot < SLOT_STM {
            let i = slot - SLOT_VOLT0;
            let value = self.slot_volt(i);
            let volts = &self.published.volts;
            let p = volts.get(usize::from(i)).copied().unwrap_or_default();
            return !p.valid
                || p.ok != value.is_some()
                || value.is_some_and(|v| p.value != milli(v));
        }
        false // the STM uptime goes out with the full publish only
    }

    /// Full publish, spread over passes: one valve (or four other slots) per pass with a poll
    /// in between, so inbound commands are never starved.
    fn service_full_publish(&mut self, now: u32) {
        if self.scheduler.take_full_publish(now) {
            self.published.full_running = true;
            self.published.full_cursor = 0;
        }
        if !self.published.full_running {
            return;
        }
        let mut budget = FULL_SLOTS_PER_PASS;
        while budget > 0 && self.published.full_cursor < SLOT_COUNT {
            let slot = self.published.full_cursor;
            self.published.full_cursor += 1;
            self.publish_slot(slot, true);
            budget = if (SLOT_VALVE0..SLOT_TEMP0).contains(&slot) {
                0
            } else {
                budget - 1
            };
        }
        if self.published.full_cursor >= SLOT_COUNT {
            self.published.full_running = false;
            self.scheduler.mark_all_published(now);
        }
    }

    fn service_on_change(&mut self, now: u32) {
        if self.published.full_running {
            return;
        }
        // stm/status and failsafe go out on every change, whatever on_change says
        if self.system_changed() {
            self.publish_system();
        }
        if !self.cfg.mqtt.on_change {
            return;
        }
        let mut sent = 0;
        for _ in 0..SLOT_SYSTEM {
            if sent >= MAX_SLOTS_PER_PASS {
                break;
            }
            let slot = self.published.change_cursor;
            self.published.change_cursor = (slot + 1) % SLOT_SYSTEM;
            let changed = self.slot_changed(slot);
            if self.scheduler.take_item(slot, changed, now) {
                self.publish_slot(slot, false);
                sent += 1;
            }
        }
    }

    // ------------------------------------------------------------ new diag

    fn publish_last_move(&mut self, i: usize) {
        let Some(v) = self.snap.valves.get(i) else {
            return;
        };
        let m = &v.last_move;
        let mut json = [0u8; 160];
        let mut jw = JsonWriter::new(&mut json);
        jw.begin_object();
        jw.kv(
            "dir",
            if m.dir == MoveDir::Open {
                "open"
            } else {
                "close"
            },
        );
        jw.kv("req", m.requested_counts);
        jw.kv("cnt", m.counted_counts);
        jw.kv("stop", stop_reason_name(m.stop));
        jw.kv("peak", u32::from(m.peak_current));
        jw.kv("ms", m.duration_ms);
        jw.end_object();
        if jw.complete() {
            let seg = segment(&self.segments, i);
            self.out
                .publish(Topic::DiagValveLastMove, seg, jw.as_bytes());
        }
    }

    /// After a gprof reply (StmSnapshot::profile_seq moved) the profile is copied from the store
    /// for this check only; without memory for the copy the next pass looks again. Not retained:
    /// only new profiles (CRC-32) go out, not the one known at connect, whose CRC the first pass
    /// of the valve stores whatever the budget.
    fn check_profile(&mut self, i: usize, budget: &mut u8) {
        let seq = self.snap.profile_seq.get(i).copied().unwrap_or(0);
        let d = self.diag.get(i).copied().unwrap_or_default();
        if seq == d.profile_seq || (*budget == 0 && d.valid) {
            return;
        }
        let Some(mut w) = try_block(self.gate, || PROFILE_WORK_EMPTY) else {
            return;
        };
        let ProfileWork { profile, json } = &mut *w;
        self.host.read_profile(i as u8, profile);
        let crc = if profile.count > 0 {
            profile_crc(profile)
        } else {
            0
        };
        if crc != d.profile_crc && d.valid && profile.count > 0 {
            let mut jw = JsonWriter::new(json);
            // C++ `writeProfileJson(jw, p) && jw.complete()`: the writer's result is its ok(),
            // which complete() includes
            write_profile_json(&mut jw, profile);
            if jw.complete() {
                let seg = segment(&self.segments, i);
                self.out
                    .publish(Topic::DiagValveProfile, seg, jw.as_bytes());
            }
            *budget -= 1;
        }
        if let Some(d) = self.diag.get_mut(i) {
            d.profile_crc = crc;
            d.profile_seq = seq;
        }
    }

    /// STM version, start time, lease, safe mode, the next calibration, counters.
    fn service_system_diag(&mut self, now: u32) {
        self.diag_version();
        self.diag_started();
        self.diag_lease();
        self.diag_next();
        let suppressed = self
            .limiter
            .suppressed()
            .wrapping_add(self.aggregator.duplicates());
        let s = &mut self.published.stm;
        let t = Topic::DiagMqttEventsSuppressed;
        self.out
            .publish_paced(t, suppressed, &mut s.suppressed, now);
        let rejected = lock(&self.out.shared.status).commands_rejected;
        let t = Topic::DiagMqttCommandsRejected;
        self.out.publish_paced(t, rejected, &mut s.rejected, now);
    }

    /// diag/stm/version when it changed (a version of 32 characters does not fit).
    fn diag_version(&mut self) {
        let version = &self.snap.version;
        if !version.valid {
            return;
        }
        let mut buf = [0u8; 32];
        let n = format_version(version, &mut buf);
        let v = buf.get(..n).unwrap_or_default();
        let s = &mut self.published.stm;
        if n > 0 && v != s.version.as_slice() && self.out.publish(Topic::DiagStmVersion, b"", v) {
            copy_string(&mut s.version, v);
        }
    }

    /// diag/stm/started when the STM start time moved by more than STARTED_TOLERANCE_S.
    fn diag_started(&mut self) {
        let snap = &self.snap;
        let t = self.host.local_time();
        let up = i64::from(snap.status.uptime_s);
        if snap.proto < 2 || !snap.have_status || !t.valid || t.epoch <= up {
            return;
        }
        let started = t.epoch - up;
        let s = &mut self.published.stm;
        let moved = s.started.map(|old| started.abs_diff(old));
        if moved.is_some_and(|m| m <= STARTED_TOLERANCE_S) {
            return;
        }
        let mut buf = [0u8; 32];
        let n = format_utc_timestamp(started as u32, &mut buf);
        if self
            .out
            .publish(Topic::DiagStmStarted, b"", buf.get(..n).unwrap_or_default())
        {
            s.started = Some(started);
        }
    }

    /// diag/stm/lease and (v3 status) diag/stm/safeMode on change.
    fn diag_lease(&mut self) {
        let snap = &self.snap;
        let s = &mut self.published.stm;
        let lease = snap.lease.state;
        if s.lease != Some(lease)
            && self
                .out
                .publish(Topic::DiagStmLease, b"", lease_state_name(lease).as_bytes())
        {
            s.lease = Some(lease);
        }
        if !snap.have_status || !snap.status.v3 {
            return;
        }
        let sm = snap.status.safe_mode;
        let payload: &[u8] = if sm { b"1" } else { b"0" };
        if s.safe_mode != Some(sm) && self.out.publish(Topic::DiagStmSafeMode, b"", payload) {
            s.safe_mode = Some(sm);
        }
    }

    /// diag/calibration/next on change ("" for none).
    fn diag_next(&mut self) {
        let next = self.host.calib_next_epoch();
        let s = &mut self.published.stm;
        if s.next == Some(next) {
            return;
        }
        let mut buf = [0u8; 32];
        let n = if next > 0 {
            format_utc_timestamp(next as u32, &mut buf)
        } else {
            0
        };
        let text = buf.get(..n).unwrap_or_default();
        if self.out.publish(Topic::DiagCalibrationNext, b"", text) {
            s.next = Some(next);
        }
    }

    /// The STM level diag topics: protocol, link, counters, calibration active.
    fn service_stm_diag(&mut self, budget: &mut u8) {
        let snap = &self.snap;
        let out = &mut self.out;
        let s = &mut self.published.stm;
        if !s.valid || s.proto != snap.proto {
            if snap.proto != 0 && out.publish_uint(Topic::DiagStmProto, b"", u32::from(snap.proto))
            {
                s.proto = snap.proto;
            }
            *budget -= 1;
        }
        if !s.valid || s.link != snap.link {
            if out.publish(
                Topic::DiagStmLink,
                b"",
                link_state_name(snap.link).as_bytes(),
            ) {
                s.link = snap.link;
            }
            *budget -= 1;
        }
        s.valid = true;
        if snap.proto >= 2 && snap.have_status {
            let st = &snap.status;
            if !s.status_valid || s.resets != st.resets {
                out.publish_uint(Topic::DiagStmResets, b"", st.resets);
            }
            if !s.status_valid || s.rx_overflow != st.rx_overflow {
                out.publish_uint(Topic::DiagStmRxOverflow, b"", st.rx_overflow);
            }
            if !s.status_valid || s.parse_err != st.parse_errors {
                out.publish_uint(Topic::DiagStmParseErr, b"", st.parse_errors);
            }
            s.resets = st.resets;
            s.rx_overflow = st.rx_overflow;
            s.parse_err = st.parse_errors;
            s.status_valid = true;
        }
        let calib_active = snap.valves.iter().any(|v| v.calibrating);
        let payload: &[u8] = if calib_active { b"1" } else { b"0" };
        if s.calib_active != Some(calib_active)
            && out.publish(Topic::DiagCalibrationActive, b"", payload)
        {
            s.calib_active = Some(calib_active);
        }
    }

    /// Publishes changed diag values; bounded messages per pass.
    fn service_diag(&mut self, now: u32) {
        if !self.cfg.mqtt.new_diag {
            return;
        }
        let mut budget = MAX_DIAG_PER_PASS;
        self.service_stm_diag(&mut budget);
        self.service_system_diag(now);
        // per valve (v2 only)
        for i in 0..N_VALVES {
            if budget == 0 {
                break;
            }
            let active = self.cfg.valves.get(i).is_some_and(|c| c.active);
            let Some(v) = self.snap.valves.get(i) else {
                break;
            };
            if !active || !v.has_extended {
                continue;
            }
            let (move_seq, early_stops, cmd_rejected, cal_state) =
                (v.move_seq, v.early_stops, v.cmd_rejected, v.cal_state);
            let d = self.diag.get(i).copied().unwrap_or_default();
            if !d.valid || d.move_seq != move_seq {
                if move_seq != 0 {
                    self.publish_last_move(i);
                }
                budget -= 1;
            }
            let seg = segment(&self.segments, i);
            if !d.valid || d.early_stops != early_stops {
                self.out
                    .publish_uint(Topic::DiagValveEarlyStops, seg, early_stops);
            }
            if !d.valid || d.cmd_rejected != cmd_rejected {
                self.out
                    .publish_uint(Topic::DiagValveCmdRejected, seg, cmd_rejected);
            }
            if !d.valid || d.cal_state != cal_state {
                self.out
                    .publish_uint(Topic::DiagValveCalState, seg, u32::from(cal_state));
            }
            if let Some(d) = self.diag.get_mut(i) {
                d.move_seq = move_seq;
                d.early_stops = early_stops;
                d.cmd_rejected = cmd_rejected;
                d.cal_state = cal_state;
            }
            self.check_profile(i, &mut budget);
            if let Some(d) = self.diag.get_mut(i) {
                d.valid = true;
            }
            self.pump();
        }
    }

    // ------------------------------------------------------------ events

    fn publish_event(&mut self, pe: &PublishEvent, now: u32) {
        if !self.limiter.allow(&pe.event, now) {
            return;
        }
        let mut json = [0u8; 512];
        let mut jw = JsonWriter::new(&mut json);
        // C++ `writeMqttEventJson(...) && jw.complete()`: the writer's result is its ok(), which
        // complete() includes
        write_mqtt_event_json(&mut jw, &pe.event, pe.valve_mask);
        if jw.complete() {
            self.out.publish(Topic::Events, b"", jw.as_bytes());
        }
    }

    fn service_events(&mut self, now: u32) {
        let mut ev: [Event; EVENTS_PER_PASS] = Default::default();
        let (n, next) = self.host.read_events_since(self.event_cursor, &mut ev);
        self.event_cursor = next;
        let out = self.connected && self.cfg.mqtt.events;
        for e in ev.iter().take(n) {
            self.handle_event(e, out, now);
        }
        if out {
            while let Some(pe) = self.aggregator.poll(now, false) {
                self.publish_event(&pe, now);
            }
        }
        let suppressed = self
            .limiter
            .suppressed()
            .wrapping_add(self.aggregator.duplicates());
        lock(&self.out.shared.status).events_suppressed = suppressed;
    }

    /// One event of the log: common/message from Warning on; `<main>events` when `out`, valve
    /// events through the aggregator.
    fn handle_event(&mut self, e: &Event, out: bool, now: u32) {
        if e.severity >= Severity::Warning {
            let mut buf = [0u8; MESSAGE_BUF];
            let len = format_event_message(e, &mut buf);
            copy_string(
                &mut self.published.message,
                buf.get(..len).unwrap_or_default(),
            );
            self.published.message_changed = true;
        }
        if !out || !event_reaches_mqtt(e) {
            return;
        }
        if e.valve < VALVE_COUNT {
            if let Some(pe) = self.aggregator.offer(e, now) {
                self.publish_event(&pe, now);
            }
        } else {
            let pe = PublishEvent {
                event: e.clone(),
                valve_mask: 0,
            };
            self.publish_event(&pe, now);
        }
    }

    // ------------------------------------------------------------ connection

    fn reset_published_state(&mut self) {
        let p = &mut self.published;
        p.valve_valid = [false; N_VALVES];
        p.volts = [SensorPub::EMPTY; N_VOLTS];
        p.stm = StmPub::default();
        p.state = None;
        p.uptime = None;
        p.stm_online = None;
        p.failsafe = None;
        p.message_changed = true;
        p.first_publish = true;
        p.full_running = false;
        p.full_cursor = 0;
        self.temps.fill(SensorPub::EMPTY);
        self.diag.fill(DiagPub::EMPTY);
    }

    fn connect(&mut self, now: u32) -> bool {
        if !self.open_session() {
            return false;
        }
        self.clean_next = false;
        self.pacer.on_connected(now);
        self.reset_published_state();
        self.inbound.count = 0;
        self.echo.reset();
        self.buttons.reset();
        self.out.publish_raw(&self.names.lwt, b"online", true);
        self.subscribe();
        self.scheduler.on_connected(now);
        self.connected = true;
        self.offline_sent = false;
        copy_string(
            &mut lock(&self.out.shared.status).client_id,
            &self.names.client_id,
        );
        self.out.shared.set_state(MqttState::Connected, 0);
        self.out.shared.count(|s| &mut s.reconnects);
        self.host.log(EventCode::MqttConnected, NO_VALVE, 0, 0, b"");
        self.start_on_connect(now);
        true
    }

    /// TCP connect and CONNECT with the last will; false (state Error with the library state)
    /// when it fails or no host or last will topic is set.
    fn open_session(&mut self) -> bool {
        let host = c_str(&self.cfg.mqtt.host);
        if host.is_empty() || self.names.lwt.is_empty() {
            self.out.shared.set_state(MqttState::Error, 0);
            return false;
        }
        let host = core::str::from_utf8(host).unwrap_or("");
        let m = &self.cfg.mqtt;
        self.out.conn.set_keep_alive(m.keep_alive_s);
        self.out.conn.set_socket_timeout(SOCKET_TIMEOUT_S);
        let auth = !c_str(&m.user).is_empty() && !c_str(&m.password).is_empty();
        self.out.shared.set_state(MqttState::Connecting, 0);
        self.out.feed(); // a connect may take 3 s TCP + 5 s CONNACK
        let args = ConnectArgs {
            id: &self.names.client_id,
            user: auth.then_some(m.user.as_slice()),
            password: auth.then_some(m.password.as_slice()),
            will: Some(Will {
                topic: &self.names.lwt,
                qos: 0,
                retain: true,
                message: b"offline",
            }),
            clean_session: self.clean_next,
        };
        let ok = self.out.conn.connect(host, m.port, &args);
        self.out.feed();
        if !ok {
            let rc = self.out.conn.state() as i8;
            self.out.shared.set_state(MqttState::Error, rc);
        }
        ok
    }

    /// The subscriptions of the session, one filter at a time (the subscribe copies it into the
    /// packet).
    fn subscribe(&mut self) {
        let mut i = 0;
        while let Some(sub) = build_subscription(
            &self.out.topics,
            self.cfg.mqtt.mode,
            &self.cfg.mqtt.discovery_prefix,
            Some(&self.segments),
            i,
        ) {
            self.out.conn.subscribe(&sub.filter, sub.qos);
            i += 1;
        }
    }

    fn on_disconnected(&mut self) {
        if !self.connected {
            return;
        }
        self.connected = false;
        self.pacer.on_dropped(self.clock.now_ms());
        if self.run.running() {
            self.abort_run();
        }
        self.list.read = None;
        self.list.write = None;
        self.end_run();
        self.auto_run.clear(); // the next connect asks again
        self.aggregator.reset();
        self.buttons.reset();
        self.inbound.count = 0;
        let rc = self.out.conn.state();
        self.host
            .log(EventCode::MqttDisconnected, NO_VALVE, rc, 0, b"");
        self.out.shared.set_state(MqttState::Connecting, rc as i8);
    }

    fn disconnect_clean(&mut self) {
        if self.out.conn.connected() {
            self.out.publish_raw(&self.names.lwt, b"offline", true);
            self.out.conn.disconnect();
        }
        self.on_disconnected();
    }
}

#[cfg(test)]
mod rig;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_eq;
#[cfg(test)]
mod tests_gate;
#[cfg(test)]
mod tests_mut;
