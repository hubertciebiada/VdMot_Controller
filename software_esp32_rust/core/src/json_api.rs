//! HTTP API document builders (/api/*) and the API router (port of `vdm/json_api.h`). The
//! builders take plain snapshot structs so they are host-testable; the web glue fills the
//! snapshots under the respective locks and streams the result. Hardware-free.
//! DESIGN.md "HTTP API" and docs/revamped/API.md are the binding field lists.
//!
//! Text inputs that the C++ takes as `const char*` are `&[u8]` read up to their first NUL
//! ([`c_str`]); where the C++ writes `null` for a null pointer the Rust field is an `Option`
//! (None writes `null`), where it writes "" the empty slice gives the same. Text members are
//! [`Text<N>`](Text) and are read up to a NUL as well.

use crate::common::{
    c_str, elapsed_ms, fmt_fit, format_ipv4, format_one_wire_id, is_zero, parse_uint, LocalTime,
    OneWireId, Text, NO_VALVE, STATION_NAME_MAX, VALVE_COUNT,
};
use crate::config::{ValveConfig, CLIENT_ID_MAX};
use crate::event_log::{write_event_json, Event};
use crate::failsafe::{
    failsafe_kind_name, ha_status_name, lease_mode_name, lease_state_name, regulator_cause_name,
    HaStatus, LeaseMode, LeaseStatus,
};
use crate::json_writer::JsonWriter;
use crate::link_policy::{link_state_name, LinkState, LinkStats};
use crate::net_policy::{net_evidence_name, NetEvidence};
use crate::stm_codec::{
    stm_cfg_flag_name, stm_chip_name, stm_flag_name, stop_reason_name, valve_fault_name, Breakaway,
    MotorChars, MoveDir, MoveResult, Profile, StmStatus, CAL_FLAG_EARLY_STOP, CAL_FLAG_LAST_FAILED,
    PROFILE_MAX_SAMPLES, STM_SYS_PROTECT_SUSPENDED,
};
use crate::stm_flasher::{
    board_check_name, flash_error_name, flash_phase_name, legacy_flash_status, FlashError,
    FlashStatus,
};
use crate::valve_model::{
    failsafe_kind, target_source_name, target_sync_name, valve_status_key, ValveState,
};
use crate::version::{format_version, min_stm_version, stm_support_name, StmSupport, Version};

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetState {
    #[default]
    Down = 0,
    Ethernet = 1,
    Wifi = 2,
}

impl NetState {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Down),
            1 => Some(Self::Ethernet),
            2 => Some(Self::Wifi),
            _ => None,
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MqttState {
    #[default]
    Disabled = 0,
    Connecting = 1,
    Connected = 2,
    Error = 3,
}

impl MqttState {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Disabled),
            1 => Some(Self::Connecting),
            2 => Some(Self::Connected),
            3 => Some(Self::Error),
            _ => None,
        }
    }
}

/// "down", "ethernet", "wifi".
pub fn net_state_name(s: NetState) -> &'static str {
    match s {
        NetState::Down => "down",
        NetState::Ethernet => "ethernet",
        NetState::Wifi => "wifi",
    }
}

/// "disabled", "connecting", "connected", "error".
pub fn mqtt_state_name(s: MqttState) -> &'static str {
    match s {
        MqttState::Disabled => "disabled",
        MqttState::Connecting => "connecting",
        MqttState::Connected => "connected",
        MqttState::Error => "error",
    }
}

/// Everything /api/status shows. Filled by the web glue from the app, net, mqtt and stm_link
/// snapshots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusSnapshot<'a> {
    // ESP
    /// firmware_version(); None writes null (C++ a null pointer)
    pub esp_version: Option<&'a [u8]>,
    /// VDM_BUILD_EPOCH (dev builds), 0 = none
    pub build_epoch: u32,
    pub uptime_s: u32,
    /// esp_reset_reason()
    pub reset_reason: u8,
    pub boot_count: u32,
    pub free_heap: u32,
    pub min_free_heap: u32,
    pub largest_free_block: u32,
    /// app image size
    pub sketch_size: u32,
    /// partition size
    pub sketch_space: u32,
    // time
    pub time_valid: bool,
    pub epoch: i64,
    pub local: LocalTime,
    pub last_sync_epoch: u32,
    // network
    pub net: NetState,
    pub ip: u32,
    pub mask: u32,
    pub gateway: u32,
    pub dns: u32,
    /// C++ `char mac[18]`
    pub mac: Text<17>,
    pub wifi_rssi: i8,
    pub hostname: Text<STATION_NAME_MAX>,
    // mqtt
    pub mqtt: MqttState,
    /// PubSubClient state()
    pub mqtt_rc: i8,
    pub mqtt_reconnects: u32,
    pub mqtt_publish_failures: u32,
    // stm
    pub link: LinkState,
    pub link_stats: LinkStats,
    pub stm_proto: u8,
    pub stm_version: Version,
    pub stm_build: u32,
    pub stm_hw_id: u16,
    /// >= min_stm_version() or unknown
    pub stm_compatible: bool,
    pub have_stm_status: bool,
    /// v2 gstat
    pub stm_status: StmStatus,
    pub esp_line_overflows: u32,
    pub esp_line_malformed: u32,
    pub calibration_active: bool,
    pub last_scheduled_calib_epoch: i64,
    /// yyyymmdd, 0 = none
    pub next_calib_slot: u32,
    pub last_event_seq: u32,
    // Added in 2.1.
    /// config station name (root "station", first member); a C string
    pub station: &'a [u8],
    /// net.trial: {"remainS":n} or null
    pub net_trial_active: bool,
    pub net_trial_remain_s: u32,
    /// mqtt.clientId (mqtt::status())
    pub mqtt_client_id: Text<CLIENT_ID_MAX>,
    pub mqtt_ha_status: HaStatus,
    /// stm.support
    pub stm_support: StmSupport,
    /// stm.lease; null while lease.mode is None
    pub lease: LeaseStatus,
    /// stm.learnTime seconds, else null
    pub have_learn_time: bool,
    pub learn_time_s: u32,
    /// calibration.next, 0 = null
    pub next_calib_epoch: i64,
    /// next_calib_epoch as local time (glue: localtime_r)
    pub next_calib_local: LocalTime,
    /// config.source: stored, imported, defaults, defaults_after_error, backup (storage
    /// LoadSource)
    pub config_source: u8,
    /// config.repairs (RepairBit mask)
    pub config_repairs: u32,
    pub config_newer_schema: bool,
    /// root "importReport"
    pub import_report: bool,
}

impl Default for StatusSnapshot<'_> {
    fn default() -> Self {
        Self {
            esp_version: Some(b""),
            build_epoch: 0,
            uptime_s: 0,
            reset_reason: 0,
            boot_count: 0,
            free_heap: 0,
            min_free_heap: 0,
            largest_free_block: 0,
            sketch_size: 0,
            sketch_space: 0,
            time_valid: false,
            epoch: 0,
            local: LocalTime::default(),
            last_sync_epoch: 0,
            net: NetState::Down,
            ip: 0,
            mask: 0,
            gateway: 0,
            dns: 0,
            mac: Text::new(),
            wifi_rssi: 0,
            hostname: Text::new(),
            mqtt: MqttState::Disabled,
            mqtt_rc: 0,
            mqtt_reconnects: 0,
            mqtt_publish_failures: 0,
            link: LinkState::Unknown,
            link_stats: LinkStats::default(),
            stm_proto: 0,
            stm_version: Version::default(),
            stm_build: 0,
            stm_hw_id: 0,
            stm_compatible: true,
            have_stm_status: false,
            stm_status: StmStatus::default(),
            esp_line_overflows: 0,
            esp_line_malformed: 0,
            calibration_active: false,
            last_scheduled_calib_epoch: 0,
            next_calib_slot: 0,
            last_event_seq: 0,
            station: b"",
            net_trial_active: false,
            net_trial_remain_s: 0,
            mqtt_client_id: Text::new(),
            mqtt_ha_status: HaStatus::Unknown,
            stm_support: StmSupport::Unknown,
            lease: LeaseStatus::default(),
            have_learn_time: false,
            learn_time_s: 0,
            next_calib_epoch: 0,
            next_calib_local: LocalTime::default(),
            config_source: 0,
            config_repairs: 0,
            config_newer_schema: false,
            import_report: false,
        }
    }
}

/// esp_reset_reason_t names (ESP-IDF 4.4).
const RESET_REASONS: [&str; 11] = [
    "unknown",
    "poweron",
    "ext",
    "sw",
    "panic",
    "int_wdt",
    "task_wdt",
    "wdt",
    "deepsleep",
    "brownout",
    "sdio",
];

/// HealthFlag bit order.
const HEALTH_NAMES: [&str; 11] = [
    "blocked",
    "failed",
    "noValve",
    "calibRetries",
    "earlyStop",
    "cmdRejected",
    "stale",
    "targetUnconfirmed",
    "tempFailed",
    "failsafe",
    "strokeShort",
];

/// storage LoadSource names; out of range -> "stored".
const CONFIG_SOURCES: [&str; 5] = [
    "stored",
    "imported",
    "defaults",
    "defaults_after_error",
    "backup",
];

fn ip_value(jw: &mut JsonWriter<'_>, k: &str, ip: u32) {
    let mut tmp = [0u8; 16];
    let n = format_ipv4(ip, &mut tmp);
    jw.kv(k, tmp.get(..n).unwrap_or_default());
}

/// "0x" and `digits` (at least) lowercase hex digits.
fn hex_value(jw: &mut JsonWriter<'_>, k: &str, v: u32, digits: usize) {
    let mut tmp = [0u8; 16];
    let n = fmt_fit(&mut tmp, format_args!("0x{v:0digits$x}"));
    jw.kv(k, tmp.get(..n).unwrap_or_default());
}

/// Number or null when zero ("0 = none").
fn non_zero(v: u32) -> Option<u32> {
    (v != 0).then_some(v)
}

fn version_value(jw: &mut JsonWriter<'_>, k: &str, v: &Version) {
    let mut tmp = [0u8; 40];
    let n = format_version(v, &mut tmp);
    jw.kv(k, (n > 0).then(|| tmp.get(..n).unwrap_or_default()));
}

fn chip_values(jw: &mut JsonWriter<'_>, id_key: &str, name_key: &str, pid: u16) {
    if pid == 0 {
        jw.kv(id_key, None::<u32>);
        jw.kv(name_key, None::<u32>);
        return;
    }
    hex_value(jw, id_key, u32::from(pid), 3);
    jw.kv(name_key, stm_chip_name(pid));
}

fn one_wire_id_value(jw: &mut JsonWriter<'_>, k: &str, id: &OneWireId) {
    let mut tmp = [0u8; 24];
    let n = if is_zero(id) {
        0
    } else {
        format_one_wire_id(id, &mut tmp)
    };
    jw.kv(k, tmp.get(..n).unwrap_or_default());
}

/// "YYYY-MM-DDTHH:MM:SS" or null.
fn local_time_value(jw: &mut JsonWriter<'_>, k: &str, t: &LocalTime, known: bool) {
    jw.key(k);
    if !known || !t.valid {
        jw.null_value();
        return;
    }
    let mut tmp = [0u8; 32];
    let n = fmt_fit(
        &mut tmp,
        format_args!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            t.year, t.month, t.mday, t.hour, t.minute, t.second
        ),
    );
    jw.value(tmp.get(..n).unwrap_or_default());
}

/// Board revision tag ("C2") or null when untagged.
fn hw_tag_value(jw: &mut JsonWriter<'_>, k: &str, tag: &[u8]) {
    let tag = c_str(tag);
    jw.kv(k, (!tag.is_empty()).then_some(tag));
}

/// Names of the set bits of `mask`, in bit order: `names` yields the name of bit 0, 1, ...
fn flag_names<'n>(
    jw: &mut JsonWriter<'_>,
    k: &str,
    mask: u32,
    names: impl Iterator<Item = &'n str>,
) {
    jw.key(k);
    jw.begin_array();
    for (bit, name) in names.enumerate() {
        if (mask >> bit) & 1 != 0 {
            jw.value(name);
        }
    }
    jw.end_array();
}

fn config_source_name(s: u8) -> &'static str {
    CONFIG_SOURCES
        .get(usize::from(s))
        .copied()
        .unwrap_or("stored")
}

fn write_lease(jw: &mut JsonWriter<'_>, l: &LeaseStatus) {
    jw.key("lease");
    if l.mode == LeaseMode::None {
        jw.null_value();
        return;
    }
    jw.begin_object();
    jw.kv("mode", lease_mode_name(l.mode));
    jw.kv("state", lease_state_name(l.state));
    jw.kv("remainS", l.remain_s);
    jw.kv("timeoutMin", l.timeout_min);
    jw.kv("failsafeMask", l.failsafe_mask);
    jw.kv("regulator", regulator_cause_name(l.regulator));
    jw.kv("regulatorLostS", l.regulator_lost_s);
    jw.kv("configSynced", l.config_synced);
    jw.kv("configFailed", l.config_failed);
    jw.kv("configTrusted", l.config_trusted);
    jw.end_object();
}

fn write_link_stats(jw: &mut JsonWriter<'_>, st: &LinkStats) {
    jw.begin_object();
    jw.kv("sent", st.sent);
    jw.kv("answered", st.answered);
    jw.kv("timeouts", st.timeouts);
    jw.kv("failedRequests", st.failed_requests);
    jw.kv("strayLines", st.stray_lines);
    jw.kv("parseErrors", st.parse_errors);
    jw.kv("queueFull", st.queue_full);
    jw.kv("evictions", st.evictions);
    jw.kv("policyResets", st.policy_resets);
    jw.kv("userResets", st.user_resets);
    jw.kv("consecutiveTimeouts", st.consecutive_timeouts);
    jw.kv("lastReplyMs", st.last_reply_ms);
    jw.end_object();
}

fn write_stm_status(jw: &mut JsonWriter<'_>, st: &StmStatus) {
    jw.begin_object();
    jw.kv("uptime", st.uptime_s);
    jw.kv("resets", st.resets);
    jw.kv("bootReason", st.boot_reason);
    jw.kv("rxOverflow", st.rx_overflow);
    jw.kv("parseErr", st.parse_errors);
    jw.kv("eepState", st.eep_state);
    if st.v3 {
        jw.kv("lease", lease_state_name(st.lease));
        jw.kv("leaseRemainS", st.lease_remain_s);
        jw.kv("leaseClient", st.lease_client);
        jw.kv("leaseTimeoutMin", st.lease_timeout_min);
        jw.kv("failsafeMask", st.failsafe_mask);
        jw.kv("safeMode", st.safe_mode);
        jw.kv("wdgResets", st.wdg_resets);
        jw.kv("uartOre", st.uart_ore);
        jw.kv("uartFe", st.uart_fe);
        jw.kv("uartNe", st.uart_ne);
        jw.kv("rxDropped", st.rx_dropped);
        flag_names(
            jw,
            "cfgFlags",
            u32::from(st.cfg_flags),
            (0..8).map(stm_cfg_flag_name),
        );
        jw.kv("cfgEvents", st.cfg_events);
        jw.kv("eepWrites", st.eep_writes);
        jw.kv("tempAgeS", st.temp_age_s);
        jw.kv("owScanAgeS", st.ow_scan_age_s);
        jw.kv(
            "protectSuspended",
            (st.sys_flags & STM_SYS_PROTECT_SUSPENDED) != 0,
        );
    }
    jw.end_object();
}

/// `{"esp":{"version":..,"build":..,"uptime":..,"resetReason":"..","boots":..,
///  "heap":{"free":..,"min":..,"largest":..},"flash":{"used":..,"size":..}},
///  "time":{"valid":..,"epoch":..,"local":"2026-09-23T14:03:05","lastSync":..},
///  "net":{"state":"ethernet","ip":"..","mask":"..","gw":"..","dns":"..","mac":"..","rssi":..,
///  "hostname":".."},
///  "mqtt":{"state":"connected","rc":0,"reconnects":..,"publishFailures":..},
///  "stm":{"link":"up","proto":2,"version":"2.0.0-revamped_C2","build":..,"hwId":"0x431",
///         "chip":"STM32F411xx","compatible":true,"minVersion":"1.4.0",
///         "stats":{...LinkStats...},"status":{...gstat...}|null,
///         "espRx":{"overflow":..,"malformed":..}},
///  "calibration":{"active":..,"lastScheduled":..,"nextSlot":..},
///  "lastEventSeq":..}`
///
/// 2.1 members: root "station" first; net.trial null|{"remainS":n}; mqtt.clientId,
/// mqtt.haStatus; stm.support, stm.lease null (mode none)|{"mode","state","remainS",
/// "timeoutMin","failsafeMask","regulator","regulatorLostS","configSynced","configFailed",
/// "configTrusted"}, stm.learnTime s|null, stm.status v3 members (gstax) when status.v3;
/// calibration.next local ISO time|null (next_calib_epoch > 0 and next_calib_local.valid); root
/// "config":{"source":..,"repairs":..,"newerSchema":..} and "importReport" last.
///
/// Unknown values are null: esp.build and stm.build 0, time.epoch/local while !time_valid,
/// lastSync 0, net.rssi unless on WiFi, stm.proto 0, an invalid stm.version, hwId/chip for hwId
/// 0, lastScheduled <= 0, nextSlot 0. resetReason is the esp_reset_reason_t name ("poweron",
/// "task_wdt", ...). Returns jw.ok().
pub fn write_status_json(jw: &mut JsonWriter<'_>, s: &StatusSnapshot<'_>) -> bool {
    jw.begin_object();
    jw.kv("station", c_str(s.station));

    jw.key("esp");
    jw.begin_object();
    jw.kv("version", s.esp_version.map(c_str));
    jw.kv("build", non_zero(s.build_epoch));
    jw.kv("uptime", s.uptime_s);
    jw.kv(
        "resetReason",
        RESET_REASONS
            .get(usize::from(s.reset_reason))
            .copied()
            .unwrap_or("unknown"),
    );
    jw.kv("boots", s.boot_count);
    jw.key("heap");
    jw.begin_object();
    jw.kv("free", s.free_heap);
    jw.kv("min", s.min_free_heap);
    jw.kv("largest", s.largest_free_block);
    jw.end_object();
    jw.key("flash");
    jw.begin_object();
    jw.kv("used", s.sketch_size);
    jw.kv("size", s.sketch_space);
    jw.end_object();
    jw.end_object();

    jw.key("time");
    jw.begin_object();
    jw.kv("valid", s.time_valid);
    jw.kv("epoch", s.time_valid.then_some(s.epoch));
    local_time_value(jw, "local", &s.local, s.time_valid);
    jw.kv("lastSync", non_zero(s.last_sync_epoch));
    jw.end_object();

    jw.key("net");
    jw.begin_object();
    jw.kv("state", net_state_name(s.net));
    ip_value(jw, "ip", s.ip);
    ip_value(jw, "mask", s.mask);
    ip_value(jw, "gw", s.gateway);
    ip_value(jw, "dns", s.dns);
    jw.kv("mac", c_str(&s.mac));
    jw.kv("rssi", (s.net == NetState::Wifi).then_some(s.wifi_rssi));
    jw.kv("hostname", c_str(&s.hostname));
    jw.key("trial");
    if s.net_trial_active {
        jw.begin_object();
        jw.kv("remainS", s.net_trial_remain_s);
        jw.end_object();
    } else {
        jw.null_value();
    }
    jw.end_object();

    jw.key("mqtt");
    jw.begin_object();
    jw.kv("state", mqtt_state_name(s.mqtt));
    jw.kv("rc", s.mqtt_rc);
    jw.kv("reconnects", s.mqtt_reconnects);
    jw.kv("publishFailures", s.mqtt_publish_failures);
    jw.kv("clientId", c_str(&s.mqtt_client_id));
    jw.kv("haStatus", ha_status_name(s.mqtt_ha_status));
    jw.end_object();

    jw.key("stm");
    jw.begin_object();
    jw.kv("link", link_state_name(s.link));
    jw.kv("proto", (s.stm_proto != 0).then_some(s.stm_proto));
    version_value(jw, "version", &s.stm_version);
    jw.kv("build", non_zero(s.stm_build));
    chip_values(jw, "hwId", "chip", s.stm_hw_id);
    jw.kv("compatible", s.stm_compatible);
    jw.kv("minVersion", min_stm_version());
    jw.key("stats");
    write_link_stats(jw, &s.link_stats);
    jw.key("status");
    if s.have_stm_status {
        write_stm_status(jw, &s.stm_status);
    } else {
        jw.null_value();
    }
    jw.key("espRx");
    jw.begin_object();
    jw.kv("overflow", s.esp_line_overflows);
    jw.kv("malformed", s.esp_line_malformed);
    jw.end_object();
    jw.kv("support", stm_support_name(s.stm_support));
    write_lease(jw, &s.lease);
    jw.kv("learnTime", s.have_learn_time.then_some(s.learn_time_s));
    jw.end_object();

    jw.key("calibration");
    jw.begin_object();
    jw.kv("active", s.calibration_active);
    jw.kv(
        "lastScheduled",
        (s.last_scheduled_calib_epoch > 0).then_some(s.last_scheduled_calib_epoch),
    );
    jw.kv("nextSlot", non_zero(s.next_calib_slot));
    local_time_value(jw, "next", &s.next_calib_local, s.next_calib_epoch > 0);
    jw.end_object();

    jw.kv("lastEventSeq", s.last_event_seq);
    jw.key("config");
    jw.begin_object();
    jw.kv("source", config_source_name(s.config_source));
    jw.kv("repairs", s.config_repairs);
    jw.kv("newerSchema", s.config_newer_schema);
    jw.end_object();
    jw.kv("importReport", s.import_report);
    jw.end_object();
    jw.ok()
}

/// Per-valve view: model state + config + sensor names/values resolved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValveView<'a> {
    /// None renders as an empty state (C++ a null pointer)
    pub state: Option<&'a ValveState>,
    /// None renders as an empty config (C++ a null pointer)
    pub config: Option<&'a ValveConfig>,
    /// Resolved sensors: 1-based config slot, 0 = none
    pub sensor_slot: [u8; 2],
    /// C strings
    pub sensor_name: [&'a [u8]; 2],
    pub sensor_valid: [bool; 2],
    /// temp in tenths when valid: raw + slot offset
    pub sensor_tenths: [i32; 2],
    /// last calibration seen by MQTT; null when !valid
    pub calibration_end: LocalTime,
}

fn write_last_move(jw: &mut JsonWriter<'_>, m: &MoveResult) {
    jw.begin_object();
    jw.kv(
        "dir",
        if m.dir == MoveDir::Close {
            "close"
        } else {
            "open"
        },
    );
    jw.kv("req", m.requested_counts);
    jw.kv("cnt", m.counted_counts);
    jw.kv("stop", stop_reason_name(m.stop));
    jw.key("peak");
    jw.fixed(i32::from(m.peak_current), 1); // 0.1 mA -> mA
    jw.kv("ms", m.duration_ms);
    jw.end_object();
}

fn write_valve(jw: &mut JsonWriter<'_>, idx: u32, v: &ValveView<'_>, now_ms: u32) {
    let st = v.state.unwrap_or(&ValveState::EMPTY);
    let (name, active): (&[u8], bool) = match v.config {
        Some(cfg) => (c_str(&cfg.name), cfg.active),
        None => (b"", false),
    };

    jw.begin_object();
    jw.kv("idx", idx);
    jw.kv("name", name);
    jw.kv("active", active);
    jw.kv("known", st.known);
    jw.kv("state", st.status);
    jw.kv("stateKey", valve_status_key(st.status));
    jw.kv("calibrating", st.calibrating);
    jw.kv("pos", st.position);
    jw.kv("target", st.desired_valid.then_some(st.desired));
    jw.kv("targetSource", target_source_name(st.source));
    jw.kv("sync", target_sync_name(st.sync));
    jw.kv("stmTarget", st.stm_target_known.then_some(st.stm_target));
    jw.kv("meanCur", st.mean_current);
    jw.kv("moves", st.moves);
    jw.kv("oc", st.open_count);
    jw.kv("cc", st.close_count);
    jw.kv("dc", st.dead_zone);
    jw.kv("cr", st.calib_retries);
    flag_names(
        jw,
        "health",
        u32::from(st.health),
        HEALTH_NAMES.iter().copied(),
    );
    jw.kv(
        "age",
        st.known.then(|| elapsed_ms(now_ms, st.last_seen_ms) / 1000),
    );
    jw.key("sensors");
    jw.begin_array();
    for (k, sensor) in [(0, 1u8), (1, 2)] {
        if v.sensor_slot[k] == 0 {
            continue;
        }
        jw.begin_object();
        jw.kv("sensor", sensor);
        jw.kv("slot", v.sensor_slot[k]);
        jw.kv("name", c_str(v.sensor_name[k]));
        jw.key("temp");
        if v.sensor_valid[k] {
            jw.fixed(v.sensor_tenths[k], 1);
        } else {
            jw.null_value();
        }
        jw.end_object();
    }
    jw.end_array();
    jw.key("ext");
    if st.has_extended {
        jw.begin_object();
        jw.kv("calState", st.cal_state);
        jw.kv("calEarlyStop", (st.cal_flags & CAL_FLAG_EARLY_STOP) != 0);
        jw.kv("calLastFailed", (st.cal_flags & CAL_FLAG_LAST_FAILED) != 0);
        jw.kv("earlyStops", st.early_stops);
        jw.kv("cmdRejected", st.cmd_rejected);
        jw.key("lastMove");
        write_last_move(jw, &st.last_move);
        jw.kv("moveSeq", st.move_seq);
        jw.key("v3");
        if st.has_v3 {
            jw.begin_object();
            flag_names(
                jw,
                "flags",
                u32::from(st.stm_flags),
                (0..16).map(stm_flag_name),
            );
            jw.kv("fault", valve_fault_name(st.fault));
            jw.kv("drive", st.drive);
            jw.kv("retryS", st.retry_s);
            jw.kv("retries", st.retries);
            jw.end_object();
        } else {
            jw.null_value();
        }
        jw.end_object();
    } else {
        jw.null_value();
    }
    jw.key("failsafe");
    jw.begin_object();
    jw.kv("state", failsafe_kind_name(failsafe_kind(st)));
    jw.kv("pct", (st.fs_pct <= 100).then_some(st.fs_pct));
    jw.end_object();
    local_time_value(jw, "calibrationEnd", &v.calibration_end, true);
    jw.end_object();
}

/// `{"valves":[{"idx":1..12,"name":"..","active":..,"known":..,"state":<n>,
///  "stateKey":"idle","calibrating":..,"pos":..,"target":..|null,
///  "targetSource":"web","sync":"synced","stmTarget":..|null,"meanCur":..,
///  "moves":..,"oc":..,"cc":..,"dc":..,"cr":..,"health":[..flag names..],
///  "age":<s since last data>|null,
///  "sensors":[{"slot":..,"name":"..","temp":21.5|null}, ...],
///  "ext":null|{"calState":0..3,"calEarlyStop":..,"calLastFailed":..,
///        "earlyStops":..,"cmdRejected":..,
///        "lastMove":{"dir":"open","req":..,"cnt":..,"stop":"endstop","peak":..,"ms":..},
///        "moveSeq":..}}, ... 12 entries always]}`
///
/// One entry per view (the glue passes all 12), idx = position in `views` + 1. "health" lists
/// HealthFlag names in bit order: "blocked", "failed", "noValve", "calibRetries", "earlyStop",
/// "cmdRejected", "stale", "targetUnconfirmed", "tempFailed", "failsafe", "strokeShort".
/// "sensors" lists only assigned slots (slot != 0), each with "sensor":1|2 (its temp1/temp2
/// position) first. "peak" is in mA with one decimal.
///
/// 2.1 members: ext."v3" null|{"flags":[stm_flag_name..],"fault":"<valve_fault_name>",
/// "drive":..,"retryS":..,"retries":..} (state.has_v3), then after "ext": "failsafe":{"state":
/// failsafe_kind_name,"pct":fs_pct|null (hold)}, "calibrationEnd":"YYYY-MM-DDTHH:MM:SS"|null.
/// A view without state or config renders them as empty. Returns jw.ok().
pub fn write_valves_json(jw: &mut JsonWriter<'_>, views: &[ValveView<'_>], now_ms: u32) -> bool {
    jw.begin_object();
    jw.key("valves");
    jw.begin_array();
    for (idx, v) in (1..).zip(views) {
        write_valve(jw, idx, v, now_ms);
    }
    jw.end_array();
    jw.end_object();
    jw.ok()
}

/// `{"valve":n,"count":k,"samples":[[count,current_mA_x10],...]}`; at most the
/// PROFILE_MAX_SAMPLES stored samples. Returns jw.ok().
pub fn write_profile_json(jw: &mut JsonWriter<'_>, p: &Profile) -> bool {
    let n = p.count.min(PROFILE_MAX_SAMPLES);
    jw.begin_object();
    jw.kv("valve", u32::from(p.valve) + 1);
    jw.kv("count", n);
    jw.key("samples");
    jw.begin_array();
    for s in p.samples.iter().take(usize::from(n)) {
        jw.begin_array();
        jw.value(s.count);
        jw.value(s.current);
        jw.end_array();
    }
    jw.end_array();
    jw.end_object();
    jw.ok()
}

/// Sensors: every discovered bus sensor + every configured slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SensorView<'a> {
    /// 1-based config slot, 0 = not configured
    pub slot: u8,
    /// a C string
    pub name: &'a [u8],
    pub active: bool,
    /// present in the last gonec/gowvc list
    pub on_bus: bool,
    pub id: OneWireId,
    pub valid: bool,
    /// STM raw (tenths / 10 mV)
    pub raw: i32,
    /// temps: tenths incl. offset; volts: milli-units
    pub value: i32,
    pub age_s: u32,
    /// temps: valve using it (0-based), else NO_VALVE
    pub valve: u8,
    /// volts; a C string
    pub unit: &'a [u8],
}

impl Default for SensorView<'_> {
    fn default() -> Self {
        Self {
            slot: 0,
            name: b"",
            active: false,
            on_bus: false,
            id: OneWireId::default(),
            valid: false,
            raw: 0,
            value: 0,
            age_s: 0,
            valve: NO_VALVE,
            unit: b"",
        }
    }
}

fn sensor_common(jw: &mut JsonWriter<'_>, s: &SensorView<'_>) {
    jw.kv("slot", (s.slot != 0).then_some(s.slot));
    jw.kv("name", c_str(s.name));
    one_wire_id_value(jw, "id", &s.id);
    jw.kv("active", s.active);
    jw.kv("onBus", s.on_bus);
}

/// `{"temps":[{"slot":..,"name":..,"id":"28-..","active":..,"onBus":..,
///   "temp":21.5|null,"raw":..,"age":..,"valve":n|null}],
///  "volts":[{"slot":..,"name":..,"id":..,"active":..,"onBus":..,
///   "value":12.345|null,"unit":"V","raw":..,"age":..}]}`
///
/// "slot" is null for bus sensors without a config slot, "id" "" when zero. Returns jw.ok().
pub fn write_sensors_json(
    jw: &mut JsonWriter<'_>,
    temps: &[SensorView<'_>],
    volts: &[SensorView<'_>],
) -> bool {
    jw.begin_object();
    jw.key("temps");
    jw.begin_array();
    for s in temps {
        jw.begin_object();
        sensor_common(jw, s);
        jw.key("temp");
        if s.valid {
            jw.fixed(s.value, 1);
        } else {
            jw.null_value();
        }
        jw.kv("raw", s.raw);
        jw.kv("age", s.age_s);
        jw.kv(
            "valve",
            (s.valve < VALVE_COUNT).then(|| u32::from(s.valve) + 1),
        );
        jw.end_object();
    }
    jw.end_array();
    jw.key("volts");
    jw.begin_array();
    for s in volts {
        jw.begin_object();
        sensor_common(jw, s);
        jw.key("value");
        if s.valid {
            jw.fixed(s.value, 3);
        } else {
            jw.null_value();
        }
        jw.kv("unit", c_str(s.unit));
        jw.kv("raw", s.raw);
        jw.kv("age", s.age_s);
        jw.end_object();
    }
    jw.end_array();
    jw.end_object();
    jw.ok()
}

/// `{"first":..,"last":..,"next":..,"dropped":..,"events":[write_event_json...]}`; `events` are
/// already filtered (EventLog::read). Returns jw.ok().
pub fn write_events_json(
    jw: &mut JsonWriter<'_>,
    events: &[Event],
    first_seq: u32,
    last_seq: u32,
    next_since: u32,
    dropped: u32,
) -> bool {
    jw.begin_object();
    jw.kv("first", first_seq);
    jw.kv("last", last_seq);
    jw.kv("next", next_since);
    jw.kv("dropped", dropped);
    jw.key("events");
    jw.begin_array();
    for e in events {
        write_event_json(jw, e);
    }
    jw.end_array();
    jw.end_object();
    jw.ok()
}

/// `{"phase":"writing","status":4,"percent":..,"bytesDone":..,"bytesTotal":..,
///  "chipId":"0x431","chipName":"..","bootloaderVersion":"3.1","attempt":..,
///  "error":null|{"code":"nack","phase":"writing","addr":"0x08000100"},
///  "startedMs":..,"finishedMs":..,"image":{"name":..,"size":..,"crc32":"0x..","version":".."},
///  "appVersion":".."|null,"board":"ok","boardHw":"C2"|null,"manualReset":..,"baud":..,
///  "pending":..}`; image gains "hw":"C2"|null (image.hw_tag).
///
/// chipId/chipName null before GetId; bootloaderVersion "<hi>.<lo>" nibbles of the GET byte
/// (0x31 -> "3.1"), null when 0; "image" null when neither a name (`image_name` None: C++ a null
/// pointer) nor a validated image is known; image.name null without a name; image.version null
/// when empty. `pending`: a flash waits for the STM EEPROM (StmSnapshot::flashPending; C++
/// default for pending: false). Returns jw.ok().
pub fn write_flash_status_json(
    jw: &mut JsonWriter<'_>,
    s: &FlashStatus,
    image_name: Option<&[u8]>,
    pending: bool,
) -> bool {
    jw.begin_object();
    jw.kv("phase", flash_phase_name(s.phase));
    jw.kv("status", legacy_flash_status(s.phase));
    jw.kv("percent", s.percent);
    jw.kv("bytesDone", s.bytes_done);
    jw.kv("bytesTotal", s.bytes_total);
    chip_values(jw, "chipId", "chipName", s.chip_pid);
    jw.key("bootloaderVersion");
    if s.bootloader_version != 0 {
        let mut tmp = [0u8; 8];
        let n = fmt_fit(
            &mut tmp,
            format_args!(
                "{}.{}",
                s.bootloader_version >> 4,
                s.bootloader_version & 0x0F
            ),
        );
        jw.value(tmp.get(..n).unwrap_or_default());
    } else {
        jw.null_value();
    }
    jw.kv("attempt", s.attempt);
    jw.key("error");
    if s.error != FlashError::None {
        jw.begin_object();
        jw.kv("code", flash_error_name(s.error));
        jw.kv("phase", flash_phase_name(s.error_phase));
        hex_value(jw, "addr", s.error_address, 8);
        jw.end_object();
    } else {
        jw.null_value();
    }
    jw.kv("startedMs", s.started_ms);
    jw.kv("finishedMs", s.finished_ms);
    jw.key("image");
    if image_name.is_some() || s.image.size != 0 {
        jw.begin_object();
        jw.kv("name", image_name.map(c_str));
        jw.kv("size", s.image.size);
        hex_value(jw, "crc32", s.image.crc, 8);
        let version = c_str(&s.image.version);
        jw.kv("version", (!version.is_empty()).then_some(version));
        hw_tag_value(jw, "hw", &s.image.hw_tag);
        jw.end_object();
    } else {
        jw.null_value();
    }
    version_value(jw, "appVersion", &s.app_version);
    jw.kv("board", board_check_name(s.board));
    hw_tag_value(jw, "boardHw", &s.board_hw);
    jw.kv("manualReset", s.manual_reset);
    jw.kv("baud", s.baud);
    jw.kv("pending", pending);
    jw.end_object();
    jw.ok()
}

/// Motor/STM parameters: `{"motor":{"lowC":..,"highC":..,"startOnPower":..,
/// "noOfMinCount":..,"maxCalReps":..},"learnMovements":..,
/// "breakaway":null|{"enable":..,"stepPct":..,"maxmA":..},"known":..}`. Returns jw.ok().
pub fn write_motor_json(
    jw: &mut JsonWriter<'_>,
    m: &MotorChars,
    learn_movements: u16,
    breakaway: Option<&Breakaway>,
    known: bool,
) -> bool {
    jw.begin_object();
    jw.key("motor");
    jw.begin_object();
    jw.kv("lowC", m.low_factor);
    jw.kv("highC", m.high_factor);
    jw.kv("startOnPower", m.start_on_power);
    jw.kv("noOfMinCount", m.min_counts);
    jw.kv("maxCalReps", m.max_calib_retries);
    jw.end_object();
    jw.kv("learnMovements", learn_movements);
    jw.key("breakaway");
    if let Some(b) = breakaway {
        jw.begin_object();
        jw.kv("enable", b.enable);
        jw.kv("stepPct", b.step_pct);
        jw.kv("maxmA", b.max_ma);
        jw.end_object();
    } else {
        jw.null_value();
    }
    jw.kv("known", known);
    jw.end_object();
    jw.ok()
}

/// Uniform error body: `{"error":"<code>","detail":"<text>"}` (HTTP 4xx/5xx); `code` and
/// `detail` are C strings, a `detail` of None writes null (C++ a null pointer). Returns
/// jw.complete().
pub fn write_error_json(jw: &mut JsonWriter<'_>, code: &[u8], detail: Option<&[u8]>) -> bool {
    jw.begin_object();
    jw.kv("error", c_str(code));
    jw.kv("detail", detail.map(c_str));
    jw.end_object();
    jw.complete()
}

// ---------------------------------------------------------------- health

pub const HEALTH_TASK_MAX: u8 = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskStackInfo<'a> {
    /// a C string
    pub name: &'a [u8],
    pub stack_bytes: u32,
    pub min_free_bytes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetHealthInfo {
    pub ip_up: bool,
    pub reachable: bool,
    pub proven: bool,
    pub ping_armed: bool,
    pub evidence: NetEvidence,
    /// u32::MAX = none
    pub evidence_age_s: u32,
    pub iface_restarts: u16,
    pub trial_active: bool,
    pub trial_remaining_s: u32,
}

impl Default for NetHealthInfo {
    fn default() -> Self {
        Self {
            ip_up: false,
            reachable: false,
            proven: false,
            ping_armed: false,
            evidence: NetEvidence::None,
            evidence_age_s: u32::MAX,
            iface_restarts: 0,
            trial_active: false,
            trial_remaining_s: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OtaHealthInfo {
    pub pending: bool,
    pub stm_required: bool,
    pub net_ok: bool,
    pub http_ok: bool,
    pub stm_ok: bool,
    pub healthy_for_s: u32,
    pub remaining_s: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LogHealthInfo {
    pub persist: bool,
    pub backlog: u32,
    pub flushes: u32,
    pub lost: u32,
    pub failures: u32,
    /// a flush was attempted since boot
    pub flushed: bool,
    pub last_flush_age_s: u32,
}

/// Everything GET /api/health shows (app::readHealth()).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HealthSnapshot<'a> {
    /// a C string
    pub version: &'a [u8],
    pub uptime_s: u32,
    pub free_heap: u32,
    pub min_free_heap: u32,
    pub largest_free_block: u32,
    pub min_largest_free_block: u32,
    pub tasks: [TaskStackInfo<'a>; HEALTH_TASK_MAX as usize],
    /// entries of `tasks` in use (at most HEALTH_TASK_MAX are written)
    pub task_count: u8,
    pub net: NetHealthInfo,
    pub ota: OtaHealthInfo,
    pub log: LogHealthInfo,
}

/// `{"ok":true,"version":"..","uptime":..,
///  "heap":{"free":..,"min":..,"largest":..,"minLargest":..},
///  "tasks":[{"name":"stm","stack":..,"minFree":..},...],
///  "net":{"ip":..,"reachable":..,"proven":..,"pingArmed":..,"evidence":"ping"|null,
///         "evidenceAgeS":n|null,"ifaceRestarts":..,"trial":null|{"remainS":n}},
///  "ota":null|{"stmRequired":..,"checks":{"net":..,"http":..,"stm":..},
///              "healthyForS":..,"remainS":..},
///  "log":{"persist":..,"backlog":..,"flushes":..,"lastFlushAgeS":n|null,"lost":..,
///         "failures":..}}`
///
/// Returns jw.complete().
pub fn write_health_json(jw: &mut JsonWriter<'_>, s: &HealthSnapshot<'_>) -> bool {
    jw.begin_object();
    jw.kv("ok", true);
    jw.kv("version", c_str(s.version));
    jw.kv("uptime", s.uptime_s);
    jw.key("heap");
    jw.begin_object();
    jw.kv("free", s.free_heap);
    jw.kv("min", s.min_free_heap);
    jw.kv("largest", s.largest_free_block);
    jw.kv("minLargest", s.min_largest_free_block);
    jw.end_object();
    jw.key("tasks");
    jw.begin_array();
    for t in s.tasks.iter().take(usize::from(s.task_count)) {
        jw.begin_object();
        jw.kv("name", c_str(t.name));
        jw.kv("stack", t.stack_bytes);
        jw.kv("minFree", t.min_free_bytes);
        jw.end_object();
    }
    jw.end_array();
    let net = &s.net;
    let evidence = net.evidence != NetEvidence::None;
    jw.key("net");
    jw.begin_object();
    jw.kv("ip", net.ip_up);
    jw.kv("reachable", net.reachable);
    jw.kv("proven", net.proven);
    jw.kv("pingArmed", net.ping_armed);
    jw.kv(
        "evidence",
        evidence.then(|| net_evidence_name(net.evidence)),
    );
    jw.kv(
        "evidenceAgeS",
        (evidence && net.evidence_age_s != u32::MAX).then_some(net.evidence_age_s),
    );
    jw.kv("ifaceRestarts", net.iface_restarts);
    jw.key("trial");
    if net.trial_active {
        jw.begin_object();
        jw.kv("remainS", net.trial_remaining_s);
        jw.end_object();
    } else {
        jw.null_value();
    }
    jw.end_object();
    let ota = &s.ota;
    jw.key("ota");
    if ota.pending {
        jw.begin_object();
        jw.kv("stmRequired", ota.stm_required);
        jw.key("checks");
        jw.begin_object();
        jw.kv("net", ota.net_ok);
        jw.kv("http", ota.http_ok);
        jw.kv("stm", ota.stm_ok);
        jw.end_object();
        jw.kv("healthyForS", ota.healthy_for_s);
        jw.kv("remainS", ota.remaining_s);
        jw.end_object();
    } else {
        jw.null_value();
    }
    let log = &s.log;
    jw.key("log");
    jw.begin_object();
    jw.kv("persist", log.persist);
    jw.kv("backlog", log.backlog);
    jw.kv("flushes", log.flushes);
    jw.kv("lastFlushAgeS", log.flushed.then_some(log.last_flush_age_s));
    jw.kv("lost", log.lost);
    jw.kv("failures", log.failures);
    jw.end_object();
    jw.end_object();
    jw.complete()
}

// ---------------------------------------------------------------- routing

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HttpMethod {
    #[default]
    Get = 0,
    Post = 1,
    Delete = 2,
    Other = 3,
}

impl HttpMethod {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Get),
            1 => Some(Self::Post),
            2 => Some(Self::Delete),
            3 => Some(Self::Other),
            _ => None,
        }
    }
}

/// Every /api endpoint (docs/revamped/API.md is the binding table). The numbers are the C++
/// ones; `OtaSwitchBack` is the Rust firmware's addition (GLUE-DESIGN-ESP.md decision 7.6).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ApiRoute {
    /// 404
    #[default]
    NotFound = 0,
    /// 405 (path known, method not)
    MethodNotAllowed = 1,
    /// GET /api/status
    Status = 2,
    /// GET /api/valves
    Valves = 3,
    /// POST /api/valves/{n}/target {"target":0..100}
    ValveTarget = 4,
    /// POST /api/valves/{n}/calibrate
    ValveCalibrate = 5,
    /// POST /api/valves/{n}/assembly
    ValveAssembly = 6,
    /// POST /api/valves/{n}/service-move {"dir":"open|close","counts":1..10000,"maxmA":5..60}
    ValveServiceMove = 7,
    /// POST /api/valves/{n}/sensors {"slot1":0..34,"slot2":0..34}
    ValveSensors = 8,
    /// GET /api/valves/{n}/profile: last gprof (v2)
    ValveProfile = 9,
    /// POST /api/valves/{n}/profile: request a fresh gprof (v2)
    ValveProfileRefresh = 10,
    /// POST /api/valves/calibrate
    CalibrateAll = 11,
    /// POST /api/valves/assembly
    AssemblyAll = 12,
    /// POST /api/valves/detect
    Detect = 13,
    /// GET /api/sensors
    Sensors = 14,
    /// POST /api/sensors/scan
    SensorsScan = 15,
    /// GET /api/events?since=&minSeverity=&valve=&limit=
    Events = 16,
    /// GET /api/config
    ConfigGet = 17,
    /// POST /api/config: partial config JSON
    ConfigPatch = 18,
    /// GET /api/config/export: full config (no secrets) as download
    ConfigExport = 19,
    /// GET /api/stm/motor
    Motor = 20,
    /// POST /api/stm/motor
    MotorSet = 21,
    /// POST /api/stm/reset
    StmReset = 22,
    /// GET /api/stm/images
    StmImages = 23,
    /// POST /api/stm/images: multipart
    StmImageUpload = 24,
    /// DELETE /api/stm/images/{name}
    StmImageDelete = 25,
    /// POST /api/stm/flash {"image":"..","mode":"normal|blank","force":false}
    StmFlash = 26,
    /// GET /api/stm/flash
    StmFlashStatus = 27,
    /// POST /api/stm/flash/abort
    StmFlashAbort = 28,
    /// POST /api/ota/esp: multipart
    EspOta = 29,
    /// POST /api/system/reboot
    Reboot = 30,
    /// POST /api/system/factory-reset {"confirm":"factory-reset"}
    FactoryReset = 31,
    /// POST /api/mqtt/reconnect
    MqttReconnect = 32,
    /// POST /api/mqtt/discovery {"action":"publish|delete|republish"}
    MqttDiscovery = 33,
    /// GET /api/log: current + previous log file, text/plain
    LogDownload = 34,
    /// GET /api/health
    Health = 35,
    /// POST /api/valves/{n}/stop
    ValveStop = 36,
    /// POST /api/valves/stop
    StopAll = 37,
    /// POST /api/stm/safe-mode/leave
    StmSafeModeLeave = 38,
    /// POST /api/system/network/confirm: keep the settings on trial
    NetConfirm = 39,
    /// POST /api/system/network/revert
    NetRevert = 40,
    /// GET /api/files
    Files = 41,
    /// DELETE /api/files?path=
    FileDelete = 42,
    /// GET /api/import-report
    ImportReport = 43,
    /// DELETE /api/import-report
    ImportReportDismiss = 44,
    /// POST /api/system/ota/switch-back {"confirm":"switch-back"}: restart into the other OTA
    /// slot (Rust firmware only)
    OtaSwitchBack = 45,
}

/// {name}: at most 31 chars (C++ `char name[32]`).
pub const ROUTE_NAME_MAX: usize = 31;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RouteMatch {
    pub route: ApiRoute,
    /// 0-based, from {n} = 1..12; NO_VALVE without one
    pub valve: u8,
    /// {name}: [A-Za-z0-9._-]{1,31}, no leading '.'; "" without one
    pub name: Text<ROUTE_NAME_MAX>,
}

impl Default for RouteMatch {
    fn default() -> Self {
        Self {
            route: ApiRoute::NotFound,
            valve: NO_VALVE,
            name: Text::new(),
        }
    }
}

/// Pattern segments: '#' = valve number {n}, '*' = file name {name}; below "/api/".
#[rustfmt::skip]
const ROUTES: [(&[u8], HttpMethod, ApiRoute); 44] = [
    (b"status", HttpMethod::Get, ApiRoute::Status),
    (b"valves", HttpMethod::Get, ApiRoute::Valves),
    (b"valves/#/target", HttpMethod::Post, ApiRoute::ValveTarget),
    (b"valves/#/calibrate", HttpMethod::Post, ApiRoute::ValveCalibrate),
    (b"valves/#/assembly", HttpMethod::Post, ApiRoute::ValveAssembly),
    (b"valves/#/service-move", HttpMethod::Post, ApiRoute::ValveServiceMove),
    (b"valves/#/sensors", HttpMethod::Post, ApiRoute::ValveSensors),
    (b"valves/#/profile", HttpMethod::Get, ApiRoute::ValveProfile),
    (b"valves/#/profile", HttpMethod::Post, ApiRoute::ValveProfileRefresh),
    (b"valves/calibrate", HttpMethod::Post, ApiRoute::CalibrateAll),
    (b"valves/assembly", HttpMethod::Post, ApiRoute::AssemblyAll),
    (b"valves/detect", HttpMethod::Post, ApiRoute::Detect),
    (b"sensors", HttpMethod::Get, ApiRoute::Sensors),
    (b"sensors/scan", HttpMethod::Post, ApiRoute::SensorsScan),
    (b"events", HttpMethod::Get, ApiRoute::Events),
    (b"config", HttpMethod::Get, ApiRoute::ConfigGet),
    (b"config", HttpMethod::Post, ApiRoute::ConfigPatch),
    (b"config/export", HttpMethod::Get, ApiRoute::ConfigExport),
    (b"stm/motor", HttpMethod::Get, ApiRoute::Motor),
    (b"stm/motor", HttpMethod::Post, ApiRoute::MotorSet),
    (b"stm/reset", HttpMethod::Post, ApiRoute::StmReset),
    (b"stm/images", HttpMethod::Get, ApiRoute::StmImages),
    (b"stm/images", HttpMethod::Post, ApiRoute::StmImageUpload),
    (b"stm/images/*", HttpMethod::Delete, ApiRoute::StmImageDelete),
    (b"stm/flash", HttpMethod::Post, ApiRoute::StmFlash),
    (b"stm/flash", HttpMethod::Get, ApiRoute::StmFlashStatus),
    (b"stm/flash/abort", HttpMethod::Post, ApiRoute::StmFlashAbort),
    (b"ota/esp", HttpMethod::Post, ApiRoute::EspOta),
    (b"system/reboot", HttpMethod::Post, ApiRoute::Reboot),
    (b"system/factory-reset", HttpMethod::Post, ApiRoute::FactoryReset),
    (b"mqtt/reconnect", HttpMethod::Post, ApiRoute::MqttReconnect),
    (b"mqtt/discovery", HttpMethod::Post, ApiRoute::MqttDiscovery),
    (b"log", HttpMethod::Get, ApiRoute::LogDownload),
    (b"health", HttpMethod::Get, ApiRoute::Health),
    (b"valves/#/stop", HttpMethod::Post, ApiRoute::ValveStop),
    (b"valves/stop", HttpMethod::Post, ApiRoute::StopAll),
    (b"stm/safe-mode/leave", HttpMethod::Post, ApiRoute::StmSafeModeLeave),
    (b"system/network/confirm", HttpMethod::Post, ApiRoute::NetConfirm),
    (b"system/network/revert", HttpMethod::Post, ApiRoute::NetRevert),
    (b"files", HttpMethod::Get, ApiRoute::Files),
    (b"files", HttpMethod::Delete, ApiRoute::FileDelete),
    (b"import-report", HttpMethod::Get, ApiRoute::ImportReport),
    (b"import-report", HttpMethod::Delete, ApiRoute::ImportReportDismiss),
    (b"system/ota/switch-back", HttpMethod::Post, ApiRoute::OtaSwitchBack),
];

/// "1".."12" without leading zeros -> 0-based valve.
fn valve_segment(s: &[u8]) -> Option<u8> {
    if !matches!(s.first(), Some(b'1'..=b'9')) {
        return None;
    }
    let v = parse_uint(s, u32::from(VALVE_COUNT))?;
    u8::try_from(v).ok()?.checked_sub(1)
}

/// [A-Za-z0-9._-]{1,31}, no leading '.'.
fn name_segment(s: &[u8]) -> Option<Text<ROUTE_NAME_MAX>> {
    match s {
        [] | [b'.', ..] => None,
        _ if s
            .iter()
            .all(|&c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-')) =>
        {
            Text::from_slice(s).ok()
        }
        _ => None,
    }
}

/// Matches `rest` (the path after "/api/") against one pattern, segment by segment.
fn match_pattern(pattern: &[u8], rest: &[u8]) -> Option<RouteMatch> {
    let mut m = RouteMatch::default();
    let mut segments = rest.split(|&c| c == b'/');
    for p in pattern.split(|&c| c == b'/') {
        let seg = segments.next()?;
        match p {
            b"#" => m.valve = valve_segment(seg)?,
            b"*" => m.name = name_segment(seg)?,
            _ if seg == p => {}
            _ => return None,
        }
    }
    segments.next().is_none().then_some(m)
}

/// Matches "/api/..." (no query string; a trailing '/' is not accepted). {n} must be 1..12
/// without leading zeros, else NotFound. A path containing NUL bytes is NotFound.
pub fn match_api_route(method: HttpMethod, path: &[u8]) -> RouteMatch {
    let Some(rest) = path.strip_prefix(b"/api/") else {
        return RouteMatch::default();
    };
    if path.contains(&0) {
        return RouteMatch::default();
    }
    let mut path_known = false;
    for &(pattern, m, route) in &ROUTES {
        let Some(mut found) = match_pattern(pattern, rest) else {
            continue;
        };
        path_known = true;
        if m == method {
            found.route = route;
            return found;
        }
    }
    RouteMatch {
        route: if path_known {
            ApiRoute::MethodNotAllowed
        } else {
            ApiRoute::NotFound
        },
        ..RouteMatch::default()
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
