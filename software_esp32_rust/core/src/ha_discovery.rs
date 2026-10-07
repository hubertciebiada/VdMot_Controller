//! Home Assistant MQTT discovery: the deterministic list of entity configs (KEEP entities with
//! their legacy object ids and unique_ids, plus the 2.1 entities), the legacy DROP entities to
//! delete, the classification of the lines of the discovery list /HADiscovery.cfg and the
//! discovery run that prunes, publishes and rewrites that list (port of `vdm/ha_discovery.h`).
//! Hardware-free. The MQTT glue steps the run once per loop pass (no blocking, no delay()).
//!
//! The iterators and the run keep no reference to their [`DiscoveryContext`]: every call takes
//! it (the glue owns the context of a run and its payload buffer in one heap block), and a run
//! must see the same context at every step. The context fields are read up to a NUL, like the
//! C++ arrays; a field that fills its whole Rust text is the C++ field without a NUL (read
//! bounded). Topics, payloads, unique_ids, object ids and the list format are the C++ bytes.

use crate::common::{
    build_ha_id, c_str, copy_string, fmt_fit, format_ipv4, format_one_wire_id, is_safe_name,
    is_zero, OneWireId, Text, ITEM_NAME_MAX, ONE_WIRE_ID_TEXT_LEN, STATION_NAME_MAX,
    TEMP_SLOT_COUNT, TEMP_UNASSIGNED, UNIT_MAX, VALVE_COUNT, VOLT_SLOT_COUNT,
};
use crate::config::{crc32, item_segment, mqtt_root_topic, Config, ItemKind, TOPIC_PREFIX_MAX};
use crate::event_log::event_mqtt_name;
use crate::json_writer::JsonWriter;
use crate::mqtt_topics::{
    build_target_command_topic, build_topic, topic_segment_valid, Topic, TopicContext, SEGMENT_MAX,
    TOPIC_MAX,
};
use crate::mqtt_values::{
    find_temp_bus, find_volt_bus, sensor_topic_segment, temp_published, valve_display_name,
    volt_announced, ValveSnapshot,
};
use crate::valve_model::{TempReading, VoltReading};

/// chars of a config topic, without NUL
pub const DISCOVERY_TOPIC_MAX: usize = 127;
/// The event entity lists every event type that reaches MQTT (about 1.8 KB with the 2.1
/// registry). PubSubClient buffer: mqtt::kBufferSize (2304).
pub const DISCOVERY_PAYLOAD_MAX: usize = 2047;
/// chars of [`DiscoveryContext::ip`] (C++ `char ip[16]`)
pub const IP_TEXT_MAX: usize = 15;
/// chars of [`DiscoveryContext::sw_version`] (C++ `char swVersion[32]`)
pub const SW_VERSION_MAX: usize = 31;
/// chars of [`DiscoveryContext::hw_version`] (C++ `char hwVersion[4]`)
pub const HW_VERSION_MAX: usize = 3;

// ---------------------------------------------------------------- context

/// A valve of [`DiscoveryContext`] (C++ `DiscoveryContext::Valve`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryContextValve {
    pub active: bool,
    /// item_segment()
    pub segment: Text<SEGMENT_MAX>,
    /// configured name (entity names), "" = "Valve <n>"
    pub name: Text<ITEM_NAME_MAX>,
    /// STM reports an assigned sensor 1
    pub has_temp1: bool,
    pub has_temp2: bool,
    /// False while the STM has not reported this valve's sensors yet (link re-sync not
    /// settled): has_temp1/2 are then a guess, and their configs are KeptUnknown (never
    /// deleted on a guess).
    pub temps_known: bool,
}

impl Default for DiscoveryContextValve {
    fn default() -> Self {
        Self {
            active: false,
            segment: Text::new(),
            name: Text::new(),
            has_temp1: false,
            has_temp2: false,
            temps_known: true,
        }
    }
}

/// A temperature or voltage slot of [`DiscoveryContext`] (C++ `DiscoveryContext::Sensor`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryContextSensor {
    /// configured, active, id present
    pub active: bool,
    /// will actually be published (all_temps / unassigned)
    pub published: bool,
    /// item_segment(): object id (legacy slot form)
    pub segment: Text<SEGMENT_MAX>,
    /// published segment (E22: bus index when unnamed)
    pub topic_segment: Text<SEGMENT_MAX>,
    /// false: unnamed and not on the bus (no config, KeptUnknown)
    pub topic_known: bool,
    pub name: Text<ITEM_NAME_MAX>,
    /// unique_id source
    pub id: Text<ONE_WIRE_ID_TEXT_LEN>,
    /// volts only
    pub unit: Text<UNIT_MAX>,
}

impl Default for DiscoveryContextSensor {
    fn default() -> Self {
        Self {
            active: false,
            published: false,
            segment: Text::new(),
            topic_segment: Text::new(),
            topic_known: true,
            name: Text::new(),
            id: Text::new(),
            unit: Text::new(),
        }
    }
}

/// Everything discovery needs, filled by glue ([`build_discovery_context`]) from the config and
/// the STM snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryContext {
    /// MQTT root, path_as_root, separate
    pub topics: TopicContext,
    /// Station name (device, node id, unique_ids); "" = topics.station.
    pub station: Text<STATION_NAME_MAX>,
    /// config topics
    pub discovery_prefix: Text<TOPIC_PREFIX_MAX>,
    /// legacy publishPlainText
    pub plain_text: bool,
    /// legacy publishDiag (valves/<V>/diag/*)
    pub publish_diag: bool,
    /// legacy publishUpTime
    pub publish_uptime: bool,
    /// legacy publishAllTemps
    pub publish_all_temps: bool,
    /// new diag/* entities
    pub new_diag: bool,
    /// <main>events and its event entity
    pub events: bool,
    /// expire_after of the numeric sensors
    pub publish_interval_s: u16,
    /// STM protocol >= 3: payload_stop, stop and safe-exit buttons
    pub stm_v3: bool,
    /// for configuration_url
    pub ip: Text<IP_TEXT_MAX>,
    /// firmware_version()
    pub sw_version: Text<SW_VERSION_MAX>,
    /// STM board tag ("C1", "C2"), "" = unknown (key omitted)
    pub hw_version: Text<HW_VERSION_MAX>,
    pub valves: [DiscoveryContextValve; VALVE_COUNT as usize],
    pub temps: [DiscoveryContextSensor; TEMP_SLOT_COUNT as usize],
    pub volts: [DiscoveryContextSensor; VOLT_SLOT_COUNT as usize],
}

impl Default for DiscoveryContext {
    fn default() -> Self {
        let mut prefix = Text::new();
        copy_string(&mut prefix, LEGACY_PREFIX);
        Self {
            topics: TopicContext::default(),
            station: Text::new(),
            discovery_prefix: prefix,
            plain_text: true,
            publish_diag: true,
            publish_uptime: true,
            publish_all_temps: true,
            new_diag: true,
            events: true,
            publish_interval_s: 10,
            stm_v3: false,
            ip: Text::new(),
            sw_version: Text::new(),
            hw_version: Text::new(),
            valves: core::array::from_fn(|_| DiscoveryContextValve::default()),
            temps: core::array::from_fn(|_| DiscoveryContextSensor::default()),
            volts: core::array::from_fn(|_| DiscoveryContextSensor::default()),
        }
    }
}

/// What the glue feeds into [`build_discovery_context`].
#[derive(Clone, Copy, Debug)]
pub struct DiscoveryInputs<'a> {
    /// C++ null: None (no context)
    pub cfg: Option<&'a Config>,
    /// [`VALVE_COUNT`] entries; C++ null: None
    pub valves: Option<&'a ValveSnapshot>,
    /// the temperature readings (C++ temps + tempCount)
    pub temps: &'a [TempReading],
    /// the voltage readings (C++ volts + voltCount)
    pub volts: &'a [VoltReading],
    pub sensors_settled: bool,
    pub stm_proto: u8,
    /// Version::hw of gvers (up to a NUL)
    pub stm_hw: &'a [u8],
    /// legacy NVS layout (format_ipv4)
    pub ip: u32,
    /// up to a NUL
    pub sw_version: &'a [u8],
}

impl Default for DiscoveryInputs<'_> {
    fn default() -> Self {
        Self {
            cfg: None,
            valves: None,
            temps: &[],
            volts: &[],
            sensors_settled: false,
            stm_proto: 0,
            stm_hw: b"",
            ip: 0,
            sw_version: b"",
        }
    }
}

// ---------------------------------------------------------------- components, messages

/// HA components used.
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HaComponent {
    #[default]
    Sensor = 0,
    BinarySensor = 1,
    Text = 2,
    Valve = 3,
    Number = 4,
    Select = 5,
    Switch = 6,
    Climate = 7,
    Button = 8,
    Event = 9,
}

impl HaComponent {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::Sensor,
            Self::BinarySensor,
            Self::Text,
            Self::Valve,
            Self::Number,
            Self::Select,
            Self::Switch,
            Self::Climate,
            Self::Button,
            Self::Event,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// "sensor", "binary_sensor", "text", "valve", "number", "select", "switch", "climate",
/// "button", "event".
pub fn ha_component_name(c: HaComponent) -> &'static str {
    match c {
        HaComponent::Sensor => "sensor",
        HaComponent::BinarySensor => "binary_sensor",
        HaComponent::Text => "text",
        HaComponent::Valve => "valve",
        HaComponent::Number => "number",
        HaComponent::Select => "select",
        HaComponent::Switch => "switch",
        HaComponent::Climate => "climate",
        HaComponent::Button => "button",
        HaComponent::Event => "event",
    }
}

/// One discovery message: topic "<prefix>/<component>/<node>/<objectId>/config", retained,
/// payload JSON (empty payload = delete).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscoveryMessage {
    pub topic: Text<DISCOVERY_TOPIC_MAX>,
    /// true -> publish empty retained payload
    pub remove: bool,
}

// ---------------------------------------------------------------- layout

const COMMON_COUNT: usize = 4;
const VALVE_KINDS: usize = 20;
const VALVES_FIRST: usize = COMMON_COUNT;
const TEMPS_FIRST: usize = VALVES_FIRST + VALVE_COUNT as usize * VALVE_KINDS;
const VOLTS_FIRST: usize = TEMPS_FIRST + TEMP_SLOT_COUNT as usize;
const TAIL_FIRST: usize = VOLTS_FIRST + VOLT_SLOT_COUNT as usize;
const TAIL_COUNT: usize = 23;
/// entity count (DESIGN.md)
const ENTITY_COUNT: u16 = (TAIL_FIRST + TAIL_COUNT) as u16;
const _: () = assert!(ENTITY_COUNT == 309, "entity count (DESIGN.md)");

const DROP_GLOBAL: usize = 2;
const DROP_KINDS: usize = 7;
/// DropListIterator position after a refused station: past every entry (C++ kDropCount; the
/// list ends after the last valve).
const DROP_EXHAUSTED: u16 = u16::MAX;

/// object id, name or unique_id part (C++ `char[kIdMax + 1]`)
const ID_MAX: usize = 47;
const EVENT_TYPES_MAX: usize = 128;
const LEGACY_PREFIX: &[u8] = b"homeassistant";
/// The C++ topic buffers, `char[kTopicMax + 1]`.
const TOPIC_BUF: usize = 128;
/// "°C"
const DEGREES_C: &[u8] = b"\xC2\xB0C";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// legacy read-only value exposed as text: command "<base>/set"
    Text,
    /// target: HA valve entity
    Valve,
    /// temperature sensor (template, expire_after)
    Temp,
    /// position sensor, %
    Actual,
    /// v2 diag counter
    Counter,
    /// v2 last move stop reason (enum)
    LastStop,
    /// plain sensor
    Plain,
    /// binary sensor "1"/"0", device class problem
    Problem,
    /// enum off/lease/blocked
    Failsafe,
    /// enum of target_sync_name()
    Sync,
    /// command only
    Button,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gate {
    Always,
    Temp1,
    Temp2,
    Diag,
    NewDiag,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Avail {
    #[default]
    None,
    Esp,
    EspStm,
}

/// Object-id / name / unique_id style of per-valve entities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Style {
    /// object id valves_<key>_<Rid>, name = uid = valves.<R>.<dotted>
    Legacy,
    /// object id diag_<key>_<Rid>, uid diag.<R>.<key>, name "<V> <label>"
    Diag,
    /// object id valves_<key>_<Rid>, uid valves.<R>.<key>, name "<V> <label>"
    New,
}

struct ValveDef {
    comp: HaComponent,
    kind: Kind,
    gate: Gate,
    style: Style,
    avail: Avail,
    /// object id part
    key: &'static [u8],
    /// legacy name / unique_id part, or the readable label
    dotted: &'static [u8],
    /// state topic (command topic for buttons)
    topic: Topic,
    icon: Option<&'static str>,
}

#[allow(clippy::too_many_arguments)]
const fn vdef(
    comp: HaComponent,
    kind: Kind,
    gate: Gate,
    style: Style,
    avail: Avail,
    key: &'static [u8],
    dotted: &'static [u8],
    topic: Topic,
    icon: Option<&'static str>,
) -> ValveDef {
    ValveDef {
        comp,
        kind,
        gate,
        style,
        avail,
        key,
        dotted,
        topic,
        icon,
    }
}

/// Order is the discovery order (DESIGN.md "HA discovery"). One row per entity kind, as in C++.
#[rustfmt::skip]
const VALVE_DEFS: [ValveDef; VALVE_KINDS] = {
    use Avail::{Esp, EspStm, None as NoAvail};
    use Gate::{Always, Diag, NewDiag, Temp1, Temp2};
    use HaComponent as C;
    use Style::{Diag as DiagStyle, Legacy, New};
    [
        vdef(C::Text, Kind::Text, Always, Legacy, NoAvail, b"state", b"state", Topic::ValveState, Some("mdi:state-machine")),
        vdef(C::Valve, Kind::Valve, Always, Legacy, NoAvail, b"target", b"target", Topic::ValveTarget, Some("mdi:valve")),
        vdef(C::Sensor, Kind::Actual, Always, New, EspStm, b"actual", b"position", Topic::ValveActual, Some("mdi:valve")),
        vdef(C::Sensor, Kind::Temp, Temp1, Legacy, NoAvail, b"temp1", b"temp1", Topic::ValveTemp1, Some("mdi:thermometer")),
        vdef(C::Sensor, Kind::Temp, Temp2, Legacy, NoAvail, b"temp2", b"temp2", Topic::ValveTemp2, Some("mdi:thermometer")),
        vdef(C::Text, Kind::Text, Always, Legacy, NoAvail, b"calibration_date", b"calibration.date", Topic::ValveCalibDate, Some("mdi:timelapse")),
        vdef(C::Text, Kind::Text, Always, Legacy, NoAvail, b"calibration_repetitions", b"calibration.repetitions", Topic::ValveCalibRepetitions, Some("mdi:valve")),
        vdef(C::Text, Kind::Text, Diag, Legacy, NoAvail, b"diag_openCount", b"diag.openCount", Topic::ValveOpenCount, Some("mdi:valve")),
        vdef(C::Text, Kind::Text, Diag, Legacy, NoAvail, b"diag_closeCount", b"diag.closeCount", Topic::ValveCloseCount, Some("mdi:valve")),
        vdef(C::Text, Kind::Text, Diag, Legacy, NoAvail, b"diag_deadZoneCount", b"diag.deadZoneCount", Topic::ValveDeadZoneCount, Some("mdi:valve")),
        vdef(C::Text, Kind::Text, Diag, Legacy, NoAvail, b"diag_moves", b"diag.moves", Topic::ValveMoves, Some("mdi:valve")),
        vdef(C::Text, Kind::Text, Diag, Legacy, NoAvail, b"diag_meanCurrrent", b"diag.meanCurrrent", Topic::ValveMeanCurrent, Some("mdi:valve")),
        vdef(C::Sensor, Kind::Counter, NewDiag, DiagStyle, EspStm, b"earlyStops", b"early stops", Topic::DiagValveEarlyStops, Some("mdi:alert-outline")),
        vdef(C::Sensor, Kind::Counter, NewDiag, DiagStyle, EspStm, b"cmdRejected", b"rejected commands", Topic::DiagValveCmdRejected, Some("mdi:alert-outline")),
        vdef(C::Sensor, Kind::LastStop, NewDiag, DiagStyle, EspStm, b"lastStop", b"last stop", Topic::DiagValveLastMove, Some("mdi:stop-circle-outline")),
        vdef(C::Sensor, Kind::Plain, NewDiag, DiagStyle, EspStm, b"calState", b"calibration state", Topic::DiagValveCalState, Some("mdi:progress-wrench")),
        vdef(C::BinarySensor, Kind::Problem, Always, New, Esp, b"problem", b"problem", Topic::ValveProblem, None),
        vdef(C::Sensor, Kind::Failsafe, Always, New, EspStm, b"failsafe", b"failsafe", Topic::ValveFailsafe, Some("mdi:shield-alert-outline")),
        vdef(C::Sensor, Kind::Sync, Always, New, Esp, b"sync", b"target delivery", Topic::ValveSync, None),
        vdef(C::Button, Kind::Button, Always, New, EspStm, b"calibrate", b"calibrate", Topic::CmdValveCalibrate, Some("mdi:tune-vertical")),
    ]
};

struct CommonDef {
    key: &'static [u8],
    topic: Topic,
    icon: &'static str,
}

const COMMON_DEFS: [CommonDef; COMMON_COUNT] = [
    CommonDef {
        key: b"state",
        topic: Topic::CommonState,
        icon: "mdi:state-machine",
    },
    CommonDef {
        key: b"message",
        topic: Topic::CommonMessage,
        icon: "mdi:message",
    },
    CommonDef {
        key: b"uptime",
        topic: Topic::CommonUptime,
        icon: "mdi:timelapse",
    },
    CommonDef {
        key: b"ip",
        topic: Topic::CommonIp,
        icon: "mdi:message",
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TailGate {
    Always,
    NewDiag,
    Events,
    StmV3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Category {
    #[default]
    None,
    Diagnostic,
    Config,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Payloads {
    #[default]
    None,
    OneZero,
    OnlineOffline,
}

const LEASE_OPTIONS: [&str; 3] = ["off", "running", "expired"];
const LINK_OPTIONS: [&str; 6] = ["unknown", "up", "degraded", "down", "booting", "suspended"];
const STOP_OPTIONS: [&str; 8] = [
    "none",
    "target",
    "endstop",
    "early_endstop",
    "timeout",
    "undercurrent",
    "safety_overcurrent",
    "aborted",
];
const FAILSAFE_OPTIONS: [&str; 3] = ["off", "lease", "blocked"];
const SYNC_OPTIONS: [&str; 6] = [
    "unknown",
    "synced",
    "pending",
    "await_ack",
    "await_verify",
    "failed",
];

struct TailDef {
    comp: HaComponent,
    object_id: &'static [u8],
    name: &'static [u8],
    uid: &'static [u8],
    /// state topic, command topic for buttons
    topic: Topic,
    avail: Avail,
    gate: TailGate,
    device_class: Option<&'static str>,
    state_class: Option<&'static str>,
    icon: Option<&'static str>,
    category: Category,
    payloads: Payloads,
    options: &'static [&'static str],
}

#[allow(clippy::too_many_arguments)]
const fn tdef(
    comp: HaComponent,
    object_id: &'static [u8],
    name: &'static [u8],
    uid: &'static [u8],
    topic: Topic,
    avail: Avail,
    gate: TailGate,
    device_class: Option<&'static str>,
    state_class: Option<&'static str>,
    icon: Option<&'static str>,
    category: Category,
    payloads: Payloads,
    options: &'static [&'static str],
) -> TailDef {
    TailDef {
        comp,
        object_id,
        name,
        uid,
        topic,
        avail,
        gate,
        device_class,
        state_class,
        icon,
        category,
        payloads,
        options,
    }
}

/// The device entities in discovery order. One row per entity, as in C++.
#[rustfmt::skip]
const TAIL_DEFS: [TailDef; TAIL_COUNT] = {
    use Avail::{Esp, EspStm, None as NoAvail};
    use Category::{Config as Cfg, Diagnostic as Diag, None as NoCat};
    use HaComponent as C;
    use Payloads::{None as NoPl, OneZero, OnlineOffline};
    use TailGate::{Always, Events, NewDiag, StmV3};
    const NO: &[&str] = &[];
    [
        tdef(C::BinarySensor, b"diag_esp_online", b"ESP online", b"diag.esp.online", Topic::Status, NoAvail, Always, Some("connectivity"), None, None, Diag, OnlineOffline, NO),
        tdef(C::BinarySensor, b"diag_stm_online", b"STM link", b"diag.stm.online", Topic::StmStatus, Esp, Always, Some("connectivity"), None, None, Diag, OnlineOffline, NO),
        tdef(C::BinarySensor, b"diag_failsafe", b"Failsafe active", b"diag.failsafe", Topic::Failsafe, EspStm, Always, Some("problem"), None, None, NoCat, OneZero, NO),
        tdef(C::Sensor, b"diag_stm_lease", b"Lease", b"diag.stm.lease", Topic::DiagStmLease, EspStm, NewDiag, Some("enum"), None, None, Diag, NoPl, &LEASE_OPTIONS),
        tdef(C::BinarySensor, b"diag_stm_safeMode", b"STM safe mode", b"diag.stm.safeMode", Topic::DiagStmSafeMode, EspStm, NewDiag, Some("problem"), None, None, Diag, OneZero, NO),
        tdef(C::Sensor, b"diag_stm_link", b"STM link state", b"diag.stm.link", Topic::DiagStmLink, Esp, NewDiag, Some("enum"), None, Some("mdi:lan-connect"), Diag, NoPl, &LINK_OPTIONS),
        tdef(C::Sensor, b"diag_stm_proto", b"STM protocol", b"diag.stm.proto", Topic::DiagStmProto, EspStm, NewDiag, None, None, None, Diag, NoPl, NO),
        tdef(C::Sensor, b"diag_stm_version", b"STM firmware", b"diag.stm.version", Topic::DiagStmVersion, EspStm, NewDiag, None, None, Some("mdi:chip"), Diag, NoPl, NO),
        tdef(C::Sensor, b"diag_stm_started", b"STM started", b"diag.stm.started", Topic::DiagStmStarted, EspStm, NewDiag, Some("timestamp"), None, None, Diag, NoPl, NO),
        tdef(C::Sensor, b"diag_stm_resets", b"STM resets", b"diag.stm.resets", Topic::DiagStmResets, EspStm, NewDiag, None, Some("total_increasing"), None, Diag, NoPl, NO),
        tdef(C::Sensor, b"diag_stm_rxOverflow", b"STM receive overflows", b"diag.stm.rxOverflow", Topic::DiagStmRxOverflow, EspStm, NewDiag, None, Some("total_increasing"), None, Diag, NoPl, NO),
        tdef(C::Sensor, b"diag_stm_parseErr", b"STM parse errors", b"diag.stm.parseErr", Topic::DiagStmParseErr, EspStm, NewDiag, None, Some("total_increasing"), None, Diag, NoPl, NO),
        tdef(C::BinarySensor, b"diag_calibration_active", b"Calibration running", b"diag.calibration.active", Topic::DiagCalibrationActive, EspStm, NewDiag, Some("running"), None, None, Diag, OneZero, NO),
        tdef(C::Sensor, b"diag_calibration_next", b"Next calibration", b"diag.calibration.next", Topic::DiagCalibrationNext, Esp, NewDiag, Some("timestamp"), None, None, NoCat, NoPl, NO),
        tdef(C::Sensor, b"diag_mqtt_eventsSuppressed", b"Suppressed events", b"diag.mqtt.eventsSuppressed", Topic::DiagMqttEventsSuppressed, Esp, NewDiag, None, Some("total_increasing"), None, Diag, NoPl, NO),
        tdef(C::Sensor, b"diag_mqtt_commandsRejected", b"Rejected MQTT commands", b"diag.mqtt.commandsRejected", Topic::DiagMqttCommandsRejected, Esp, NewDiag, None, Some("total_increasing"), None, Diag, NoPl, NO),
        tdef(C::Event, b"events", b"Events", b"events", Topic::Events, Esp, Events, None, None, Some("mdi:bell-alert-outline"), NoCat, NoPl, NO),
        tdef(C::Button, b"cmd_calibrate_all", b"Calibrate all valves", b"cmd.calibrate", Topic::CmdCalibrate, EspStm, Always, None, None, None, Cfg, NoPl, NO),
        tdef(C::Button, b"cmd_detect", b"Detect valves", b"cmd.detect", Topic::CmdDetect, EspStm, Always, None, None, None, Cfg, NoPl, NO),
        tdef(C::Button, b"cmd_stop", b"Stop valves", b"cmd.stop", Topic::CmdStop, EspStm, StmV3, None, None, None, Cfg, NoPl, NO),
        tdef(C::Button, b"cmd_stm_reset", b"Reset STM", b"cmd.stmReset", Topic::CmdStmReset, Esp, Always, Some("restart"), None, None, Cfg, NoPl, NO),
        tdef(C::Button, b"cmd_esp_restart", b"Restart ESP", b"cmd.restart", Topic::CmdRestart, Esp, Always, Some("restart"), None, None, Cfg, NoPl, NO),
        tdef(C::Button, b"cmd_stm_safe_exit", b"Leave STM safe mode", b"cmd.stmSafeExit", Topic::CmdStmSafeExit, EspStm, StmV3, None, None, None, Cfg, NoPl, NO),
    ]
};

struct DropDef {
    comp: HaComponent,
    prefix: &'static [u8],
}

const DROP_GLOBALS: [DropDef; DROP_GLOBAL] = [
    DropDef {
        comp: HaComponent::Select,
        prefix: b"heatControl",
    },
    DropDef {
        comp: HaComponent::Number,
        prefix: b"parkPosition",
    },
];

const DROP_VALVE: [DropDef; DROP_KINDS] = [
    DropDef {
        comp: HaComponent::Climate,
        prefix: b"climate_",
    },
    DropDef {
        comp: HaComponent::Number,
        prefix: b"valves_control_dynOffs_",
    },
    DropDef {
        comp: HaComponent::Number,
        prefix: b"valves_control_min_",
    },
    DropDef {
        comp: HaComponent::Number,
        prefix: b"valves_control_max_",
    },
    DropDef {
        comp: HaComponent::Select,
        prefix: b"valves_window_state_",
    },
    DropDef {
        comp: HaComponent::Switch,
        prefix: b"valves_window_state_",
    },
    DropDef {
        comp: HaComponent::Number,
        prefix: b"valves_window_target_",
    },
];

const TEMP_TEMPLATE: &str = "{{ value | replace(',', '.') | float(None) }}";

// ---------------------------------------------------------------- helpers

/// `parts` joined into a text of capacity N (the C++ `Str` over a `char[N + 1]`); None when
/// they do not fit.
fn cat<const N: usize>(parts: &[&[u8]]) -> Option<Text<N>> {
    let mut t = Text::new();
    for p in parts {
        t.extend_from_slice(p).ok()?;
    }
    Some(t)
}

/// The text a topic builder writes into a C++ topic buffer; None for its 0 (does not fit, bad
/// input).
fn topic_text(build: impl FnOnce(&mut [u8]) -> usize) -> Option<Text<TOPIC_MAX>> {
    let mut buf = [0u8; TOPIC_BUF];
    let n = build(&mut buf);
    let t = buf.get(..n).filter(|t| !t.is_empty())?;
    Text::from_slice(t).ok()
}

/// build_ha_id() of a segment into the C++ `char id[kSegmentMax + 1]`; None when nothing
/// survives.
fn segment_id(seg: &[u8]) -> Option<Text<SEGMENT_MAX>> {
    let mut buf = [0u8; SEGMENT_MAX + 1];
    let n = build_ha_id(seg, &mut buf);
    let id = buf.get(..n).filter(|id| !id.is_empty())?;
    Text::from_slice(id).ok()
}

/// The station as it appears in names, unique_ids and the device block: non-empty and safe
/// (is_safe_name also bounds its length), else discovery is off.
fn station_of(ctx: &DiscoveryContext) -> Option<&[u8]> {
    let own = c_str(&ctx.station);
    let s = if own.is_empty() {
        c_str(&ctx.topics.station)
    } else {
        own
    };
    is_safe_name(s, STATION_NAME_MAX, false).then_some(s)
}

/// Node id: build_ha_id(station) into the C++ `char[kStationNameMax + 1]`.
fn node_of(ctx: &DiscoveryContext) -> Option<Text<STATION_NAME_MAX>> {
    let station = station_of(ctx)?;
    let mut buf = [0u8; STATION_NAME_MAX + 1];
    let n = build_ha_id(station, &mut buf);
    let node = buf.get(..n).filter(|node| !node.is_empty())?;
    Text::from_slice(node).ok()
}

/// Levels of [A-Za-z0-9_-] separated by single '/' (config TopicPath).
fn topic_path(s: &[u8]) -> bool {
    s.split(|&c| c == b'/').all(|level| {
        !level.is_empty()
            && level
                .iter()
                .all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    })
}

/// Discovery prefix: a TopicPath of 1..32 chars, else the legacy prefix.
fn prefix_of(ctx: &DiscoveryContext) -> &[u8] {
    let p = c_str(&ctx.discovery_prefix);
    if topic_path(p) {
        p
    } else {
        LEGACY_PREFIX
    }
}

/// A valve/sensor segment as filled by glue: must be a valid topic segment without space, '"'
/// and '\\'.
fn segment_of(src: &[u8]) -> Option<&[u8]> {
    let seg = c_str(src);
    let ok = topic_segment_valid(seg) && !seg.iter().any(|&c| matches!(c, b' ' | b'"' | b'\\'));
    ok.then_some(seg)
}

/// "<prefix>/<component>/<node>/<objectId>/config" into msg.topic; false when it does not fit
/// (never: the longest topic has 107 chars), msg.topic then unchanged.
fn build_config_topic(
    prefix: &[u8],
    node: &[u8],
    comp: HaComponent,
    object_id: &[u8],
    msg: &mut DiscoveryMessage,
) -> bool {
    let comp = ha_component_name(comp).as_bytes();
    match cat(&[prefix, b"/", comp, b"/", node, b"/", object_id, b"/config"]) {
        Some(t) => {
            msg.topic = t;
            true
        }
        None => false,
    }
}

// ---------------------------------------------------------------- entities

#[derive(Default)]
struct Entity {
    comp: HaComponent,
    /// with build_ha_id() segments
    object_id: Text<ID_MAX>,
    /// the same with raw segments (2.0.0 form)
    object_id_raw: Text<ID_MAX>,
    name: Text<ID_MAX>,
    /// without the "<station>." prefix
    uid: Text<ID_MAX>,
    state: Text<TOPIC_MAX>,
    command: Text<TOPIC_MAX>,
    unit: Text<UNIT_MAX>,
    icon: Option<&'static str>,
    device_class: Option<&'static str>,
    state_class: Option<&'static str>,
    value_template: Option<&'static str>,
    options: &'static [&'static str],
    expire_after_s: u32,
    reports_position: bool,
    qos1: bool,
    payload_stop: bool,
    payloads: Payloads,
    category: Category,
    event_types: bool,
    avail: Avail,
    /// described only to keep its config (KeptUnknown)
    kept: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Describe {
    Skip,
    Ok,
    Error,
}

/// Ok when the entity was built, Error when it cannot be.
fn built(entity: Option<()>) -> Describe {
    match entity {
        Some(()) => Describe::Ok,
        None => Describe::Error,
    }
}

/// expire_after of the numeric sensors: 3 x publish_interval_s, at least 60.
fn expire_after(ctx: &DiscoveryContext) -> u32 {
    (3 * u32::from(ctx.publish_interval_s)).max(60)
}

/// "<base>/set": the legacy command topic of a text entity (never subscribed).
fn text_command(ctx: &DiscoveryContext, t: Topic, segment: &[u8]) -> Option<Text<TOPIC_MAX>> {
    let mut plain = ctx.topics.clone();
    plain.separate = false;
    let base = topic_text(|out| build_topic(&plain, t, segment, out))?;
    cat(&[&base, b"/set"])
}

fn state_topic(ctx: &DiscoveryContext, t: Topic, segment: &[u8]) -> Option<Text<TOPIC_MAX>> {
    topic_text(|out| build_topic(&ctx.topics, t, segment, out))
}

fn describe_common(ctx: &DiscoveryContext, k: usize, e: &mut Entity) -> Describe {
    let Some(d) = COMMON_DEFS.get(k) else {
        return Describe::Skip;
    };
    if d.topic == Topic::CommonUptime && !ctx.publish_uptime {
        return Describe::Skip;
    }
    built(common_entity(ctx, d, e))
}

fn common_entity(ctx: &DiscoveryContext, d: &CommonDef, e: &mut Entity) -> Option<()> {
    e.comp = HaComponent::Text;
    e.icon = Some(d.icon);
    e.object_id = cat(&[d.key])?;
    e.object_id_raw = cat(&[d.key])?;
    e.name = cat(&[d.key])?;
    e.uid = cat(&[b"common.", d.key])?;
    e.state = state_topic(ctx, d.topic, b"")?;
    e.command = text_command(ctx, d.topic, b"")?;
    Some(())
}

/// `keep`: also describe the temp entities of a valve whose sensors are not known yet
/// (classification only; never published on a guess).
fn describe_valve(ctx: &DiscoveryContext, v: u8, k: usize, e: &mut Entity, keep: bool) -> Describe {
    let (Some(valve), Some(d)) = (ctx.valves.get(usize::from(v)), VALVE_DEFS.get(k)) else {
        return Describe::Skip;
    };
    if !valve.active {
        return Describe::Skip;
    }
    let shown = match d.gate {
        Gate::Always => true,
        Gate::Temp1 | Gate::Temp2 => {
            let has = if d.gate == Gate::Temp1 {
                valve.has_temp1
            } else {
                valve.has_temp2
            };
            if !has {
                if !keep || valve.temps_known {
                    return Describe::Skip;
                }
                e.kept = true;
            }
            true
        }
        Gate::Diag => ctx.publish_diag,
        Gate::NewDiag => ctx.new_diag,
    };
    if !shown {
        return Describe::Skip;
    }
    built(valve_entity(ctx, v, valve, d, e))
}

fn valve_entity(
    ctx: &DiscoveryContext,
    v: u8,
    valve: &DiscoveryContextValve,
    d: &ValveDef,
    e: &mut Entity,
) -> Option<()> {
    let seg = segment_of(&valve.segment)?;
    e.comp = d.comp;
    e.icon = d.icon;
    e.avail = d.avail;
    if d.style == Style::Legacy {
        e.name = cat(&[b"valves.", seg, b".", d.dotted])?;
        e.uid = e.name.clone();
    } else {
        // "Valve 12" or a name of up to 10 bytes (the C++ buffer holds 16)
        let mut display = [0u8; ITEM_NAME_MAX + 1];
        let n = valve_display_name(c_str(&valve.name), v, &mut display);
        let display = display.get(..n).unwrap_or_default();
        e.name = cat(&[display, b" ", d.dotted])?;
        let head: &[u8] = if d.style == Style::Diag {
            b"diag."
        } else {
            b"valves."
        };
        e.uid = cat(&[head, seg, b".", d.key])?;
    }
    // Object ids "<head><key>_<id>": the id from build_ha_id(), the raw segment in the 2.0.0
    // form.
    let id = segment_id(seg)?;
    let head: &[u8] = if d.style == Style::Diag {
        b"diag_"
    } else {
        b"valves_"
    };
    e.object_id = cat(&[head, d.key, b"_", &id])?;
    e.object_id_raw = cat(&[head, d.key, b"_", seg])?;
    if d.kind == Kind::Button {
        e.command = topic_text(|out| build_topic(&ctx.topics, d.topic, seg, out))?;
        e.category = Category::Config;
        return Some(());
    }
    e.state = state_topic(ctx, d.topic, seg)?;
    match d.kind {
        Kind::Text => e.command = text_command(ctx, d.topic, seg)?,
        Kind::Valve => {
            e.command = topic_text(|out| build_target_command_topic(&ctx.topics, seg, out))?;
            e.device_class = Some("water");
            e.reports_position = true;
            e.qos1 = true;
            e.payload_stop = ctx.stm_v3;
        }
        Kind::Temp => {
            e.value_template = Some(TEMP_TEMPLATE);
            e.device_class = Some("temperature");
            e.state_class = Some("measurement");
            e.expire_after_s = expire_after(ctx);
            e.unit = cat(&[DEGREES_C])?;
        }
        Kind::Actual => {
            e.state_class = Some("measurement");
            e.unit = cat(&[b"%"])?;
        }
        Kind::Counter => {
            e.state_class = Some("total_increasing");
            e.category = Category::Diagnostic;
        }
        Kind::LastStop => {
            e.value_template = Some("{{ value_json.stop }}");
            e.device_class = Some("enum");
            e.options = &STOP_OPTIONS;
            e.category = Category::Diagnostic;
        }
        Kind::Plain => e.category = Category::Diagnostic,
        Kind::Problem => {
            e.device_class = Some("problem");
            e.payloads = Payloads::OneZero;
        }
        Kind::Failsafe => {
            e.device_class = Some("enum");
            e.options = &FAILSAFE_OPTIONS;
        }
        Kind::Sync => {
            e.device_class = Some("enum");
            e.options = &SYNC_OPTIONS;
            e.category = Category::Diagnostic;
        }
        Kind::Button => {}
    }
    Some(())
}

/// Temp and volt sensors: object id from the slot segment (legacy), state topic from the
/// published segment (E22).
fn describe_sensor(
    ctx: &DiscoveryContext,
    s: &DiscoveryContextSensor,
    temp: bool,
    e: &mut Entity,
    keep: bool,
) -> Describe {
    let id = c_str(&s.id);
    if !s.active || (temp && !s.published) || id.is_empty() {
        return Describe::Skip;
    }
    if !s.topic_known {
        if !keep {
            return Describe::Skip;
        }
        e.kept = true;
    }
    built(sensor_entity(ctx, s, temp, id, e))
}

fn sensor_entity(
    ctx: &DiscoveryContext,
    s: &DiscoveryContextSensor,
    temp: bool,
    id: &[u8],
    e: &mut Entity,
) -> Option<()> {
    let seg = segment_of(&s.segment)?;
    e.comp = HaComponent::Sensor;
    e.state_class = Some("measurement");
    e.value_template = Some(TEMP_TEMPLATE);
    e.expire_after_s = expire_after(ctx);
    if temp {
        e.icon = Some("mdi:thermometer");
        e.device_class = Some("temperature");
        e.unit = cat(&[DEGREES_C])?;
        e.name = cat(&[b"temps.", seg])?;
    } else {
        let unit = c_str(&s.unit);
        e.unit = cat(&[unit])?;
        if unit == b"V" || unit == b"mV" {
            e.device_class = Some("voltage");
        }
        // Legacy: the raw configured name (spaces kept, "volts." when unnamed).
        e.name = cat(&[b"volts.", c_str(&s.name)])?;
    }
    let ha = segment_id(seg)?;
    let head: &[u8] = if temp { b"temps_" } else { b"volts_" };
    e.object_id = cat(&[head, &ha])?;
    e.object_id_raw = cat(&[head, seg])?;
    e.uid = cat(&[id])?;
    if e.kept {
        return Some(());
    }
    let published = segment_of(&s.topic_segment)?;
    let t = if temp {
        Topic::TempValue
    } else {
        Topic::VoltValue
    };
    e.state = state_topic(ctx, t, published)?;
    Some(())
}

fn describe_tail(ctx: &DiscoveryContext, k: usize, e: &mut Entity) -> Describe {
    let Some(d) = TAIL_DEFS.get(k) else {
        return Describe::Skip;
    };
    let shown = match d.gate {
        TailGate::Always => true,
        TailGate::NewDiag => ctx.new_diag,
        TailGate::Events => ctx.events,
        TailGate::StmV3 => ctx.stm_v3,
    };
    if !shown {
        return Describe::Skip;
    }
    built(tail_entity(ctx, d, e))
}

fn tail_entity(ctx: &DiscoveryContext, d: &TailDef, e: &mut Entity) -> Option<()> {
    e.comp = d.comp;
    e.icon = d.icon;
    e.device_class = d.device_class;
    e.state_class = d.state_class;
    e.category = d.category;
    e.payloads = d.payloads;
    e.options = d.options;
    e.avail = d.avail;
    e.event_types = d.comp == HaComponent::Event;
    e.object_id = cat(&[d.object_id])?;
    e.object_id_raw = cat(&[d.object_id])?;
    e.name = cat(&[d.name])?;
    e.uid = cat(&[d.uid])?;
    if d.comp == HaComponent::Button {
        e.command = topic_text(|out| build_topic(&ctx.topics, d.topic, b"", out))?;
    } else {
        e.state = state_topic(ctx, d.topic, b"")?;
    }
    Some(())
}

fn describe(ctx: &DiscoveryContext, pos: u16, e: &mut Entity, keep: bool) -> Describe {
    let pos = usize::from(pos);
    if pos < VALVES_FIRST {
        return describe_common(ctx, pos, e);
    }
    if pos < TEMPS_FIRST {
        let r = pos - VALVES_FIRST;
        let v = u8::try_from(r / VALVE_KINDS).unwrap_or(u8::MAX);
        return describe_valve(ctx, v, r % VALVE_KINDS, e, keep);
    }
    if pos < VOLTS_FIRST {
        return match ctx.temps.get(pos - TEMPS_FIRST) {
            Some(s) => describe_sensor(ctx, s, true, e, keep),
            None => Describe::Skip,
        };
    }
    if pos < TAIL_FIRST {
        return match ctx.volts.get(pos - VOLTS_FIRST) {
            Some(s) => describe_sensor(ctx, s, false, e, keep),
            None => Describe::Skip,
        };
    }
    describe_tail(ctx, pos - TAIL_FIRST, e)
}

/// Makes jw report failure (ok() false) for a message that cannot be built.
fn poison(jw: &mut JsonWriter<'_>) {
    jw.reset();
    jw.end_object();
}

fn write_availability_topic(ctx: &DiscoveryContext, t: Topic, jw: &mut JsonWriter<'_>) {
    let mut topic = [0u8; TOPIC_BUF];
    let n = build_topic(&ctx.topics, t, b"", &mut topic);
    jw.begin_object();
    jw.kv("topic", topic.get(..n).unwrap_or_default());
    jw.end_object();
}

fn write_availability(ctx: &DiscoveryContext, a: Avail, jw: &mut JsonWriter<'_>) {
    if a == Avail::None {
        return;
    }
    jw.key("availability");
    jw.begin_array();
    write_availability_topic(ctx, Topic::Status, jw);
    if a == Avail::EspStm {
        write_availability_topic(ctx, Topic::StmStatus, jw);
    }
    jw.end_array();
    if a == Avail::EspStm {
        jw.kv("availability_mode", "all");
    }
}

/// The event_types of the event entity: every event that reaches MQTT, never a partial list
/// (more than 128 names poison the writer).
fn write_event_types(jw: &mut JsonWriter<'_>) {
    jw.key("event_types");
    jw.begin_array();
    for (i, name) in (0..).map_while(event_mqtt_name).enumerate() {
        if i == EVENT_TYPES_MAX {
            poison(jw);
            return;
        }
        jw.value(name);
    }
    jw.end_array();
}

fn write_entity(ctx: &DiscoveryContext, station: &[u8], e: &Entity, jw: &mut JsonWriter<'_>) {
    // "<station>.<uid>" with ' ' -> '_' (at most 20 + 1 + 47 chars; the C++ buffer holds 68)
    let mut uid: Text<TOPIC_MAX> = cat(&[station, b".", &e.uid]).unwrap_or_default();
    for c in uid.iter_mut() {
        if *c == b' ' {
            *c = b'_';
        }
    }
    let ip = c_str(&ctx.ip);
    let sw = c_str(&ctx.sw_version);
    let hw = c_str(&ctx.hw_version);

    jw.begin_object();
    jw.kv("name", &e.name);
    jw.kv("unique_id", &uid);
    if !e.state.is_empty() {
        jw.kv("state_topic", &e.state);
    }
    if !e.command.is_empty() {
        jw.kv("command_topic", &e.command);
    }
    if let Some(t) = e.value_template {
        jw.kv("value_template", t);
    }
    if let Some(icon) = e.icon {
        jw.kv("icon", icon);
    }
    if let Some(dc) = e.device_class {
        jw.kv("device_class", dc);
    }
    if let Some(sc) = e.state_class {
        jw.kv("state_class", sc);
    }
    if !e.unit.is_empty() {
        jw.kv("unit_of_measurement", &e.unit);
    }
    if !e.options.is_empty() {
        jw.key("options");
        jw.begin_array();
        for o in e.options {
            jw.value(*o);
        }
        jw.end_array();
    }
    if e.expire_after_s > 0 {
        jw.kv("expire_after", e.expire_after_s);
    }
    if e.reports_position {
        jw.kv("reports_position", true);
    }
    if e.qos1 {
        jw.kv("qos", 1u32);
    }
    if e.payload_stop {
        jw.kv("payload_stop", "STOP");
    }
    match e.payloads {
        Payloads::OneZero => {
            jw.kv("payload_on", "1");
            jw.kv("payload_off", "0");
        }
        Payloads::OnlineOffline => {
            jw.kv("payload_on", "online");
            jw.kv("payload_off", "offline");
        }
        Payloads::None => {}
    }
    match e.category {
        Category::Diagnostic => jw.kv("entity_category", "diagnostic"),
        Category::Config => jw.kv("entity_category", "config"),
        Category::None => {}
    }
    if e.event_types {
        write_event_types(jw);
    }
    write_availability(ctx, e.avail, jw);
    jw.key("device");
    jw.begin_object();
    jw.kv("identifiers", station);
    jw.kv("name", station);
    jw.kv("sw_version", sw);
    if !hw.is_empty() {
        jw.kv("hw_version", hw);
    }
    jw.kv("model", "VdMot Revamped");
    jw.kv("manufacturer", "Lenti84/Surfgargano");
    if !ip.is_empty() {
        // "http://" + ip + "/" (at most 23 chars, the C++ buffer holds 23)
        let url: Text<TOPIC_MAX> = cat(&[b"http://", ip, b"/"]).unwrap_or_default();
        jw.kv("configuration_url", &url);
    }
    jw.end_object();
    jw.end_object();
}

/// "<p>/<component>/<id>/<id>/config" with p = the legacy or the configured prefix, no
/// wildcards or control characters, at most [`DISCOVERY_TOPIC_MAX`] chars.
fn config_topic_shape(ctx: &DiscoveryContext, t: &[u8]) -> bool {
    if t.len() > DISCOVERY_TOPIC_MAX
        || t.iter()
            .any(|&c| c == b'+' || c == b'#' || c < 0x20 || c == 0x7F)
    {
        return false;
    }
    // the levels after "<p>/"; the configured prefix wins when both match
    let mut rest = None;
    for p in [LEGACY_PREFIX, prefix_of(ctx)] {
        if let Some(r) = t.strip_prefix(p).and_then(|r| r.strip_prefix(b"/")) {
            rest = Some(r);
        }
    }
    let Some(rest) = rest else {
        return false;
    };
    // Exactly three non-empty levels, then "config".
    let mut levels = rest.split(|&c| c == b'/');
    let (Some(comp), Some(node), Some(object), Some(last), None) = (
        levels.next(),
        levels.next(),
        levels.next(),
        levels.next(),
        levels.next(),
    ) else {
        return false;
    };
    !comp.is_empty() && !node.is_empty() && !object.is_empty() && last == b"config"
}

// ---------------------------------------------------------------- DiscoveryIterator

/// Iterates the current entity set in a fixed order: common (state, message, uptime*, ip), per
/// active valve 20 kinds (state, target, actual, temp1*, temp2*, calibration date and
/// repetitions, diag x5*, early stops*, rejected commands*, last stop*, calibration state*,
/// problem, failsafe, target delivery, calibrate button), temps*, volts*, then the device
/// entities (ESP/STM online, failsafe, lease*, safe mode*, link*, protocol*, version*,
/// started*, resets*, overflows*, parse errors*, calibration running*, next calibration*,
/// suppressed events*, rejected commands*, events*, buttons calibrate all, detect, stop*, reset
/// STM, restart ESP, leave safe mode*). (* = gated.)
///
/// Node id = build_ha_id(station), object ids from build_ha_id(segment); names and unique_ids
/// ("<station with ' ' -> '_'>.<uid>") keep the raw segment. KEEP entities (legacy component,
/// object id, name and unique_id) carry no availability, so they keep working under the legacy
/// firmware after a rollback; new entities name <main>status (esp) or <main>status and
/// <main>stm/status (esp+stm).
///
/// Only the position is kept; every call takes the context (C++ `reset(ctx)` is
/// [`restart`](Self::restart) with the other context passed from then on).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiscoveryIterator {
    pos: u16,
}

impl DiscoveryIterator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes the next message: topic into msg, JSON payload into jw. Returns false when the
    /// list is exhausted or a message does not fit (then jw.ok() is false, msg.topic is set when
    /// the entity got that far, and the caller skips it).
    pub fn next(
        &mut self,
        ctx: &DiscoveryContext,
        msg: &mut DiscoveryMessage,
        jw: &mut JsonWriter<'_>,
    ) -> bool {
        *msg = DiscoveryMessage::default();
        jw.reset();
        let (Some(station), Some(node)) = (station_of(ctx), node_of(ctx)) else {
            self.pos = ENTITY_COUNT;
            return false;
        };
        let prefix = prefix_of(ctx);
        while self.pos < ENTITY_COUNT {
            let mut e = Entity::default();
            let d = describe(ctx, self.pos, &mut e, false);
            self.pos += 1;
            match d {
                Describe::Skip => continue,
                Describe::Error => {
                    poison(jw);
                    return false;
                }
                Describe::Ok => {}
            }
            if !build_config_topic(prefix, &node, e.comp, &e.object_id, msg) {
                *msg = DiscoveryMessage::default();
                poison(jw);
                return false;
            }
            write_entity(ctx, station, &e, jw);
            if !jw.complete() || jw.length() > DISCOVERY_PAYLOAD_MAX {
                poison(jw);
                return false;
            }
            return true;
        }
        false
    }

    /// Next config topic only (no JSON); entities that cannot be built are passed over. false
    /// at the end.
    pub fn next_topic(&mut self, ctx: &DiscoveryContext, msg: &mut DiscoveryMessage) -> bool {
        *msg = DiscoveryMessage::default();
        let Some(node) = node_of(ctx) else {
            self.pos = ENTITY_COUNT;
            return false;
        };
        let prefix = prefix_of(ctx);
        while self.pos < ENTITY_COUNT {
            let mut e = Entity::default();
            let d = describe(ctx, self.pos, &mut e, false);
            self.pos += 1;
            if d == Describe::Ok && build_config_topic(prefix, &node, e.comp, &e.object_id, msg) {
                return true;
            }
        }
        *msg = DiscoveryMessage::default();
        false
    }

    /// The 2.0.0 form of the entity just produced by next()/next_topic(): prefix
    /// "homeassistant", raw station, raw segment in the object id. false when it equals the
    /// current form.
    pub fn v20_topic(&self, ctx: &DiscoveryContext, msg: &mut DiscoveryMessage) -> bool {
        *msg = DiscoveryMessage::default();
        let Some(last) = self.pos.checked_sub(1) else {
            return false;
        };
        let (Some(station), Some(node)) = (station_of(ctx), node_of(ctx)) else {
            return false;
        };
        let mut e = Entity::default();
        if describe(ctx, last, &mut e, false) != Describe::Ok {
            return false;
        }
        let mut cur = DiscoveryMessage::default();
        let both = build_config_topic(prefix_of(ctx), &node, e.comp, &e.object_id, &mut cur)
            && build_config_topic(LEGACY_PREFIX, station, e.comp, &e.object_id_raw, msg);
        if both && cur.topic != msg.topic {
            return true;
        }
        *msg = DiscoveryMessage::default();
        false
    }

    pub fn restart(&mut self) {
        self.pos = 0;
    }

    pub fn position(&self) -> u16 {
        self.pos
    }
}

// ---------------------------------------------------------------- DropListIterator

/// Iterates the legacy DROP entities to delete once (empty retained payload), for every valve
/// index whether active or not and for both possible valve segments (name-based and 1-based
/// index), with the literal legacy prefix "homeassistant/" and the raw station:
///   climate/<st>/climate_<R>, number/<st>/valves_control_dynOffs_<R>,
///   number/<st>/valves_control_min_<R>, number/<st>/valves_control_max_<R>,
///   select/<st>/valves_window_state_<R>, switch/<st>/valves_window_state_<R>,
///   number/<st>/valves_window_target_<R>, select/<st>/heatControl,
///   number/<st>/parkPosition.
///
/// Only the position is kept; every call takes the context.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DropListIterator {
    pos: u16,
}

impl DropListIterator {
    pub fn new() -> Self {
        Self::default()
    }

    /// msg.remove is always true.
    pub fn next(&mut self, ctx: &DiscoveryContext, msg: &mut DiscoveryMessage) -> bool {
        *msg = DiscoveryMessage::default();
        let Some(station) = station_of(ctx) else {
            self.pos = DROP_EXHAUSTED;
            return false;
        };
        loop {
            let pos = usize::from(self.pos);
            if let Some(d) = DROP_GLOBALS.get(pos) {
                self.pos += 1;
                if build_config_topic(LEGACY_PREFIX, station, d.comp, d.prefix, msg) {
                    msg.remove = true;
                    return true;
                }
                continue;
            }
            // per valve 7 kinds in the name form, then 7 in the index form; the list ends
            // after the last valve
            let r = pos - DROP_GLOBAL;
            let valve = r / (2 * DROP_KINDS);
            let Some(cfg) = ctx.valves.get(valve) else {
                return false;
            };
            self.pos += 1;
            let index_form = (r / DROP_KINDS) % 2 == 1;
            let Some(d) = DROP_VALVE.get(r % DROP_KINDS) else {
                continue;
            };
            let mut number = [0u8; 4];
            let n = fmt_fit(&mut number, format_args!("{}", valve + 1));
            let number = number.get(..n).unwrap_or_default();
            let named = segment_of(&cfg.segment).filter(|seg| !seg.contains(&b'/'));
            let seg = match (index_form, named) {
                // already sent in the name form
                (true, Some(name)) if name == number => continue,
                (true, _) => number,
                (false, Some(name)) => name,
                (false, None) => continue,
            };
            let Some(object_id) = cat::<ID_MAX>(&[d.prefix, seg]) else {
                continue;
            };
            if build_config_topic(LEGACY_PREFIX, station, d.comp, &object_id, msg) {
                msg.remove = true;
                return true;
            }
        }
    }

    pub fn restart(&mut self) {
        self.pos = 0;
    }
}

// ---------------------------------------------------------------- classification

/// What a line of the discovery list is to the current context.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopicClass {
    /// not "<homeassistant|prefix>/<component>/<id>/<id>/config", wildcards, control chars,
    /// > DISCOVERY_TOPIC_MAX: dropped from the list, never published to
    Foreign = 0,
    /// the config topic of an entity the iterator produces now
    Current = 1,
    /// the config topic of an entity kept on a guess: valve temp1/temp2 while !temps_known, a
    /// temp/volt entity while !topic_known (never deleted)
    KeptUnknown = 2,
    /// anything else: deleted (renamed, deactivated, gated off, old prefix/station)
    Stale = 3,
}

impl TopicClass {
    pub fn from_raw(v: u8) -> Option<Self> {
        [Self::Foreign, Self::Current, Self::KeptUnknown, Self::Stale]
            .get(usize::from(v))
            .copied()
    }
}

/// Classifies `topic` (all of it is the line; the C++ len). Trailing CR, LF, space and tab of
/// the line are ignored.
pub fn classify_discovery_topic(ctx: &DiscoveryContext, topic: &[u8]) -> TopicClass {
    let mut t = topic;
    while let [rest @ .., b'\r' | b'\n' | b' ' | b'\t'] = t {
        t = rest;
    }
    if !config_topic_shape(ctx, t) {
        return TopicClass::Foreign;
    }
    let Some(node) = node_of(ctx) else {
        return TopicClass::Stale;
    };
    let prefix = prefix_of(ctx);
    let mut msg = DiscoveryMessage::default();
    for pos in 0..ENTITY_COUNT {
        let mut e = Entity::default();
        if describe(ctx, pos, &mut e, true) != Describe::Ok {
            continue;
        }
        if !build_config_topic(prefix, &node, e.comp, &e.object_id, &mut msg) {
            continue;
        }
        if msg.topic.as_slice() == t {
            return if e.kept {
                TopicClass::KeptUnknown
            } else {
                TopicClass::Current
            };
        }
    }
    TopicClass::Stale
}

// ---------------------------------------------------------------- list file

/// Byte source and list files of a discovery run (glue: PubSubClient + LittleFS).
pub trait DiscoveryPort {
    /// retained; false = connection lost
    fn publish(&mut self, topic: &[u8], payload: &[u8]) -> bool;
    /// /HADiscovery.cfg for reading; false = missing
    fn list_open(&mut self) -> bool;
    /// next byte, None at the end (C++ -1)
    fn list_read(&mut self) -> Option<u8>;
    fn list_close(&mut self);
    /// create/truncate /HADiscovery.cfg.tmp
    fn list_begin(&mut self) -> bool;
    /// topic + '\n'
    fn list_write(&mut self, topic: &[u8]) -> bool;
    /// rename tmp over the list (replace)
    fn list_commit(&mut self) -> bool;
    /// close and remove tmp
    fn list_abort(&mut self);
}

/// One line of the list from port.list_read(): CR/LF separated, empty lines skipped, lines
/// longer than [`DISCOVERY_TOPIC_MAX`] dropped; None at the end. The line ends at a NUL byte
/// inside it (the C++ char array).
pub fn read_list_line(port: &mut dyn DiscoveryPort) -> Option<Text<DISCOVERY_TOPIC_MAX>> {
    let mut line: Text<DISCOVERY_TOPIC_MAX> = Text::new();
    let mut overlong = false;
    loop {
        let c = port.list_read();
        match c {
            None | Some(b'\n' | b'\r') => {
                if !line.is_empty() && !overlong {
                    let n = c_str(&line).len();
                    line.truncate(n);
                    return Some(line);
                }
                c?;
                // empty line, CRLF or the end of an overlong line
                line.clear();
                overlong = false;
            }
            Some(b) => {
                if line.push(b).is_err() {
                    overlong = true;
                }
            }
        }
    }
}

// ---------------------------------------------------------------- DiscoveryRun

/// What a discovery run does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiscoveryPlan {
    /// actions Delete and DeleteAndPublish
    pub remove_all: bool,
    /// Publish, DeleteAndPublish, automatic runs in mode 2
    pub publish: bool,
    /// legacy DROP entities (first run)
    pub drop_legacy: bool,
    /// 2.0.0 -> 2.1 migration (publish runs only)
    pub retire20: bool,
    /// false only for a pure Delete
    pub prune: bool,
}

impl Default for DiscoveryPlan {
    fn default() -> Self {
        Self {
            remove_all: false,
            publish: false,
            drop_legacy: false,
            retire20: false,
            prune: true,
        }
    }
}

/// A list line that is written again: KeptUnknown, and Current without publish.
fn carried(cls: TopicClass, publish: bool) -> bool {
    cls == TopicClass::KeptUnknown || (cls == TopicClass::Current && !publish)
}

/// Phase of a [`DiscoveryRun`] (C++ `DiscoveryRun::Phase`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiscoveryRunPhase {
    #[default]
    Idle = 0,
    RemoveList = 1,
    RemoveCurrent = 2,
    ClearList = 3,
    Retired = 4,
    DropList = 5,
    Prune = 6,
    Publish = 7,
    WriteKept = 8,
    WriteCurrent = 9,
    Commit = 10,
    Done = 11,
    Aborted = 12,
}

impl DiscoveryRunPhase {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::Idle,
            Self::RemoveList,
            Self::RemoveCurrent,
            Self::ClearList,
            Self::Retired,
            Self::DropList,
            Self::Prune,
            Self::Publish,
            Self::WriteKept,
            Self::WriteCurrent,
            Self::Commit,
            Self::Done,
            Self::Aborted,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// What a run did (C++ `DiscoveryRun::Stats`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiscoveryRunStats {
    pub configs: u16,
    pub deletes: u16,
    pub skipped: u16,
    pub list_written: bool,
}

/// One discovery run: phases in this order, each only when the plan asks:
///  RemoveList (every usable list line -> ""), RemoveCurrent (every current topic -> ""),
///  ClearList (empty list; a pure Delete ends here), Retired (diag_stm_uptime and the 2.0.0
///  forms of changed ids -> ""), DropList, Prune (Stale lines -> "", Foreign dropped,
///  KeptUnknown and, without publish, Current lines carried), Publish (every config), then
///  WriteKept / WriteCurrent / Commit when the list changed (missing, a Stale or Foreign line,
///  other CRC32 or line count): the carried lines and then every current topic into the tmp
///  file, renamed over the list.
/// A failed publish aborts the run (Aborted); a failed list write abandons only the list
/// (list_abort), the run still ends Done. A context without a valid station ends the run at its
/// first step (Done, nothing touched).
///
/// The run keeps no reference to the context: [`step`](Self::step) takes it, and it must be
/// the same context at every step of a run (C++ `start(ctx, plan)` binds it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiscoveryRun {
    plan: DiscoveryPlan,
    phase: DiscoveryRunPhase,
    entered: bool,
    /// the list is open for reading
    reading: bool,
    /// the tmp list is open for writing
    writing: bool,
    uptime_done: bool,
    changed: bool,
    crc_old: u32,
    crc_new: u32,
    lines_old: u16,
    lines_new: u16,
    it: DiscoveryIterator,
    drop: DropListIterator,
    stats: DiscoveryRunStats,
}

impl DiscoveryRun {
    /// At most one publish and at most LINES_PER_STEP list lines per step.
    pub const LINES_PER_STEP: u8 = 8;

    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a run with `plan`; every [`step`](Self::step) of it takes the same context.
    pub fn start(&mut self, plan: &DiscoveryPlan) {
        self.plan = *plan;
        self.it.restart();
        self.drop.restart();
        self.stats = DiscoveryRunStats::default();
        self.reading = false;
        self.writing = false;
        self.uptime_done = false;
        self.changed = false;
        self.crc_old = 0;
        self.crc_new = 0;
        self.lines_old = 0;
        self.lines_new = 0;
        self.phase = DiscoveryRunPhase::Idle;
        self.advance();
    }

    /// At most one publish and at most [`LINES_PER_STEP`](Self::LINES_PER_STEP) list lines per
    /// call. Returns the phase after the step.
    pub fn step(
        &mut self,
        ctx: &DiscoveryContext,
        port: &mut dyn DiscoveryPort,
        payload: &mut JsonWriter<'_>,
    ) -> DiscoveryRunPhase {
        use DiscoveryRunPhase as P;
        if !self.running() {
            return self.phase;
        }
        if node_of(ctx).is_none() {
            self.finish_list(port, false);
            self.enter(P::Done);
            return self.phase;
        }
        match self.phase {
            P::RemoveList | P::Prune => self.step_list(ctx, port),
            P::RemoveCurrent => {
                let mut msg = DiscoveryMessage::default();
                if self.it.next_topic(ctx, &mut msg) {
                    self.remove(port, &msg.topic);
                } else {
                    self.advance();
                }
            }
            P::ClearList => {
                if port.list_begin() {
                    self.writing = true;
                    self.finish_list(port, true);
                } else {
                    port.list_abort();
                }
                self.advance();
            }
            P::Retired => self.step_retired(ctx, port),
            P::DropList => {
                let mut msg = DiscoveryMessage::default();
                if self.drop.next(ctx, &mut msg) {
                    self.remove(port, &msg.topic);
                } else {
                    self.advance();
                }
            }
            P::Publish => self.step_publish(ctx, port, payload),
            P::WriteKept => self.step_write_kept(ctx, port),
            P::WriteCurrent => self.step_write_current(ctx, port),
            P::Commit => {
                self.finish_list(port, true);
                self.enter(P::Done);
            }
            P::Idle | P::Done | P::Aborted => {}
        }
        self.phase
    }

    /// Ends a running run (Aborted): the list being read is closed and the tmp list abandoned.
    pub fn abort(&mut self, port: &mut dyn DiscoveryPort) {
        if !self.running() {
            return;
        }
        self.finish_list(port, false);
        self.phase = DiscoveryRunPhase::Aborted;
    }

    pub fn phase(&self) -> DiscoveryRunPhase {
        self.phase
    }

    /// Phase not Idle, Done or Aborted.
    pub fn running(&self) -> bool {
        !matches!(
            self.phase,
            DiscoveryRunPhase::Idle | DiscoveryRunPhase::Done | DiscoveryRunPhase::Aborted
        )
    }

    pub fn stats(&self) -> &DiscoveryRunStats {
        &self.stats
    }

    pub fn plan(&self) -> &DiscoveryPlan {
        &self.plan
    }

    fn enter(&mut self, p: DiscoveryRunPhase) {
        self.phase = p;
        self.entered = false;
        self.it.restart();
    }

    /// The next phase the plan asks for after the current one (the C++ switch with its
    /// fallthroughs: an arm without `break` goes on with the next phase).
    fn advance(&mut self) {
        use DiscoveryRunPhase as P;
        let p = self.plan;
        let list_changed =
            self.changed || self.crc_new != self.crc_old || self.lines_new != self.lines_old;
        let mut at = self.phase;
        let next = loop {
            at = match at {
                P::Idle if p.remove_all => break P::RemoveList,
                P::Idle => P::RemoveList,
                P::RemoveList if p.remove_all => break P::RemoveCurrent,
                P::RemoveList => P::RemoveCurrent,
                P::RemoveCurrent if p.remove_all => break P::ClearList,
                P::RemoveCurrent => P::ClearList,
                P::ClearList if !p.publish && !p.prune => break P::Done,
                P::ClearList if p.retire20 && p.publish => break P::Retired,
                P::ClearList => P::Retired,
                P::Retired if p.drop_legacy => break P::DropList,
                P::Retired => P::DropList,
                P::DropList if p.prune => break P::Prune,
                P::DropList => P::Prune,
                P::Prune if p.publish => break P::Publish,
                P::Prune => P::Publish,
                P::Publish if list_changed => break P::WriteKept,
                P::WriteKept if p.publish => break P::WriteCurrent,
                P::WriteKept => break P::Commit,
                P::WriteCurrent => break P::Commit,
                _ => break P::Done,
            };
        };
        self.enter(next);
    }

    fn remove(&mut self, port: &mut dyn DiscoveryPort, topic: &[u8]) {
        if !port.publish(topic, b"") {
            self.abort(port);
            return;
        }
        self.stats.deletes = self.stats.deletes.wrapping_add(1);
    }

    fn finish_list(&mut self, port: &mut dyn DiscoveryPort, commit: bool) {
        if self.reading {
            port.list_close();
        }
        self.reading = false;
        if !self.writing {
            return;
        }
        self.writing = false;
        if commit && port.list_commit() {
            self.stats.list_written = true;
        } else {
            port.list_abort();
        }
    }

    /// RemoveList and Prune: up to LINES_PER_STEP lines, at most one delete.
    fn step_list(&mut self, ctx: &DiscoveryContext, port: &mut dyn DiscoveryPort) {
        let prune = self.phase == DiscoveryRunPhase::Prune;
        if !self.entered {
            self.entered = true;
            self.reading = port.list_open();
            if !self.reading {
                // a missing list is written after the run
                self.changed = self.changed || prune;
                self.advance();
                return;
            }
        }
        for _ in 0..Self::LINES_PER_STEP {
            let Some(line) = read_list_line(port) else {
                self.finish_list(port, false);
                self.advance();
                return;
            };
            let cls = classify_discovery_topic(ctx, &line);
            if !prune {
                if cls == TopicClass::Foreign {
                    continue;
                }
                self.remove(port, &line);
                return;
            }
            self.lines_old = self.lines_old.wrapping_add(1);
            self.crc_old = crc32(b"\n", crc32(&line, self.crc_old));
            match cls {
                TopicClass::Stale => {
                    self.changed = true;
                    self.remove(port, &line);
                    return;
                }
                TopicClass::Foreign => self.changed = true,
                TopicClass::KeptUnknown | TopicClass::Current => {}
            }
            if carried(cls, self.plan.publish) {
                self.lines_new = self.lines_new.wrapping_add(1);
                self.crc_new = crc32(b"\n", crc32(&line, self.crc_new));
            }
        }
    }

    fn step_retired(&mut self, ctx: &DiscoveryContext, port: &mut dyn DiscoveryPort) {
        let mut msg = DiscoveryMessage::default();
        if !self.uptime_done {
            self.uptime_done = true;
            // the station is valid: step() checked the node id
            let station = station_of(ctx).unwrap_or_default();
            if build_config_topic(
                LEGACY_PREFIX,
                station,
                HaComponent::Sensor,
                b"diag_stm_uptime",
                &mut msg,
            ) {
                self.remove(port, &msg.topic);
                return;
            }
        }
        // every next_topic() moves on by at least one entity
        for _ in 0..ENTITY_COUNT {
            if !self.it.next_topic(ctx, &mut msg) {
                break;
            }
            if self.it.v20_topic(ctx, &mut msg) {
                self.remove(port, &msg.topic);
                return;
            }
        }
        self.advance();
    }

    fn step_publish(
        &mut self,
        ctx: &DiscoveryContext,
        port: &mut dyn DiscoveryPort,
        payload: &mut JsonWriter<'_>,
    ) {
        let mut msg = DiscoveryMessage::default();
        let got = self.it.next(ctx, &mut msg, payload);
        if !got && payload.ok() {
            self.advance();
            return;
        }
        if !msg.topic.is_empty() {
            self.lines_new = self.lines_new.wrapping_add(1);
            self.crc_new = crc32(b"\n", crc32(&msg.topic, self.crc_new));
        }
        if !got {
            self.stats.skipped += 1;
        } else if port.publish(&msg.topic, payload.as_bytes()) {
            self.stats.configs += 1;
        } else {
            self.abort(port);
        }
    }

    fn step_write_kept(&mut self, ctx: &DiscoveryContext, port: &mut dyn DiscoveryPort) {
        if !self.entered {
            self.entered = true;
            if !port.list_begin() {
                port.list_abort();
                self.enter(DiscoveryRunPhase::Done);
                return;
            }
            self.writing = true;
            self.reading = port.list_open();
        }
        for _ in 0..Self::LINES_PER_STEP {
            let line = if self.reading {
                read_list_line(port)
            } else {
                None
            };
            let Some(line) = line else {
                if self.reading {
                    port.list_close();
                }
                self.reading = false;
                self.advance();
                return;
            };
            if !carried(classify_discovery_topic(ctx, &line), self.plan.publish) {
                continue;
            }
            if !port.list_write(&line) {
                self.finish_list(port, false);
                self.enter(DiscoveryRunPhase::Done);
                return;
            }
        }
    }

    fn step_write_current(&mut self, ctx: &DiscoveryContext, port: &mut dyn DiscoveryPort) {
        for _ in 0..Self::LINES_PER_STEP {
            let mut msg = DiscoveryMessage::default();
            if !self.it.next_topic(ctx, &mut msg) {
                self.advance();
                return;
            }
            if !port.list_write(&msg.topic) {
                self.finish_list(port, false);
                self.enter(DiscoveryRunPhase::Done);
                return;
            }
        }
    }
}

// ---------------------------------------------------------------- context

// The snapshot-dependent parts, one item at a time: build_discovery_context() fills a context
// with them, discovery_input_key() hashes them without building a context.

/// Valve i: the STM reports sensor 1 / sensor 2 assigned, and its assignments are settled.
fn valve_sensors(input: &DiscoveryInputs<'_>, i: usize) -> [bool; 3] {
    match input.valves.and_then(|vs| vs.get(i)) {
        Some(v) => [
            v.temp1 != TEMP_UNASSIGNED,
            v.temp2 != TEMP_UNASSIGNED,
            input.sensors_settled && v.known,
        ],
        None => [false; 3],
    }
}

fn sensor_published(input: &DiscoveryInputs<'_>, cfg: &Config, kind: ItemKind, i: u8) -> bool {
    if kind == ItemKind::Temp {
        temp_published(cfg, input.valves, i)
    } else {
        volt_announced(cfg, i)
    }
}

/// The published segment of sensor slot i; 0 when it is not known (unnamed and not on the
/// bus).
fn sensor_segment(
    input: &DiscoveryInputs<'_>,
    cfg: &Config,
    kind: ItemKind,
    i: u8,
    out: &mut [u8],
) -> usize {
    let bus = if kind == ItemKind::Temp {
        cfg.temps
            .get(usize::from(i))
            .and_then(|s| find_temp_bus(input.temps, &s.id))
    } else {
        cfg.volts
            .get(usize::from(i))
            .and_then(|s| find_volt_bus(input.volts, &s.id))
    };
    sensor_topic_segment(cfg, kind, i, bus, out)
}

/// The text a segment builder writes into the C++ `char[kSegmentMax + 1]`.
fn segment_text(build: impl FnOnce(&mut [u8]) -> usize) -> Text<SEGMENT_MAX> {
    let mut buf = [0u8; SEGMENT_MAX + 1];
    let n = build(&mut buf);
    let mut t = Text::new();
    copy_string(&mut t, buf.get(..n).unwrap_or_default());
    t
}

/// The 1-Wire id text of a slot, "" for an empty slot.
fn id_text(id: &OneWireId) -> Text<ONE_WIRE_ID_TEXT_LEN> {
    let mut t = Text::new();
    if !is_zero(id) {
        let mut buf = [0u8; ONE_WIRE_ID_TEXT_LEN + 1];
        let n = format_one_wire_id(id, &mut buf);
        copy_string(&mut t, buf.get(..n).unwrap_or_default());
    }
    t
}

/// Fills every field of `c` (false and an empty context without cfg).
pub fn build_discovery_context(input: &DiscoveryInputs<'_>, c: &mut DiscoveryContext) -> bool {
    *c = DiscoveryContext::default();
    let Some(cfg) = input.cfg else {
        return false;
    };
    copy_string(&mut c.topics.station, mqtt_root_topic(cfg));
    c.topics.path_as_root = cfg.mqtt.path_as_root;
    c.topics.separate = cfg.mqtt.separate;
    copy_string(&mut c.station, &cfg.station);
    copy_string(&mut c.discovery_prefix, &cfg.mqtt.discovery_prefix);
    c.plain_text = cfg.mqtt.plain_text;
    c.publish_diag = cfg.mqtt.diag;
    c.publish_uptime = cfg.mqtt.up_time;
    c.publish_all_temps = cfg.mqtt.all_temps;
    c.new_diag = cfg.mqtt.new_diag;
    c.events = cfg.mqtt.events;
    c.publish_interval_s = cfg.mqtt.publish_interval_s;
    c.stm_v3 = input.stm_proto >= 3;
    if input.ip != 0 {
        let mut ip = [0u8; IP_TEXT_MAX + 1];
        let n = format_ipv4(input.ip, &mut ip);
        copy_string(&mut c.ip, ip.get(..n).unwrap_or_default());
    }
    copy_string(&mut c.sw_version, input.sw_version);
    copy_string(&mut c.hw_version, input.stm_hw);
    for ((i, v), vc) in (0..VALVE_COUNT).zip(&mut c.valves).zip(&cfg.valves) {
        v.active = vc.active;
        v.segment = segment_text(|out| item_segment(cfg, ItemKind::Valve, i, out));
        copy_string(&mut v.name, &vc.name);
        [v.has_temp1, v.has_temp2, v.temps_known] = valve_sensors(input, usize::from(i));
    }
    for ((i, d), s) in (0..TEMP_SLOT_COUNT).zip(&mut c.temps).zip(&cfg.temps) {
        d.active = s.active && !is_zero(&s.id);
        d.published = sensor_published(input, cfg, ItemKind::Temp, i);
        d.segment = segment_text(|out| item_segment(cfg, ItemKind::Temp, i, out));
        d.topic_segment = segment_text(|out| sensor_segment(input, cfg, ItemKind::Temp, i, out));
        d.topic_known = !d.topic_segment.is_empty();
        copy_string(&mut d.name, &s.name);
        d.id = id_text(&s.id);
    }
    for ((i, d), s) in (0..VOLT_SLOT_COUNT).zip(&mut c.volts).zip(&cfg.volts) {
        d.active = volt_announced(cfg, i);
        d.published = sensor_published(input, cfg, ItemKind::Volt, i);
        d.segment = segment_text(|out| item_segment(cfg, ItemKind::Volt, i, out));
        d.topic_segment = segment_text(|out| sensor_segment(input, cfg, ItemKind::Volt, i, out));
        d.topic_known = !d.topic_segment.is_empty();
        copy_string(&mut d.name, &s.name);
        d.id = id_text(&s.id);
        copy_string(&mut d.unit, &s.unit);
    }
    true
}

/// CRC32 over the snapshot-dependent parts of the context (per valve has_temp1/has_temp2/
/// temps_known, per sensor slot published/topic_known/topic segment, hw, stm_v3): the glue
/// re-runs discovery when it changes. 0 without cfg.
pub fn discovery_input_key(input: &DiscoveryInputs<'_>) -> u32 {
    let Some(cfg) = input.cfg else {
        return 0;
    };
    // Whole arrays go into the CRC: zeroed first, like the fields of a new context.
    let mut crc = 0;
    for i in 0..usize::from(VALVE_COUNT) {
        let [t1, t2, known] = valve_sensors(input, i);
        crc = crc32(&[u8::from(t1), u8::from(t2), u8::from(known)], crc);
    }
    for (kind, n) in [
        (ItemKind::Temp, TEMP_SLOT_COUNT),
        (ItemKind::Volt, VOLT_SLOT_COUNT),
    ] {
        for i in 0..n {
            let mut seg = [0u8; SEGMENT_MAX + 1];
            let topic_known = sensor_segment(input, cfg, kind, i, &mut seg) > 0;
            let published = sensor_published(input, cfg, kind, i);
            crc = crc32(&[u8::from(published), u8::from(topic_known)], crc);
            crc = crc32(&seg, crc);
        }
    }
    let mut hw_text: Text<HW_VERSION_MAX> = Text::new();
    copy_string(&mut hw_text, input.stm_hw);
    let mut hw = [0u8; HW_VERSION_MAX + 1];
    for (d, s) in hw.iter_mut().zip(hw_text.iter()) {
        *d = *s;
    }
    crc = crc32(&hw, crc);
    crc32(&[u8::from(input.stm_proto >= 3)], crc)
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
