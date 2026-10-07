//! Structured event log: event codes (single registry for the whole firmware), severities, a
//! fixed ring buffer with sequence numbers and filtered reads, and text/JSON formatting (port of
//! `vdm/event_log.h`). Hardware-free, not thread-safe (the logger glue wraps it in a mutex).
//!
//! Codes, names, severities, MQTT classes, messages and the line/JSON formats are external API
//! (DESIGN.md section 13): never renumber, only append.

use core::fmt::Write;

use crate::common::{c_str, copy_string, Text, TextBuf, ALL_VALVES, NO_VALVE, VALVE_COUNT};
use crate::json_writer::JsonWriter;
use crate::net_policy::{net_evidence_name, NetEvidence};
use crate::stm_codec::{stop_reason_name, valve_fault_name, StopReason};
use crate::valve_model::{
    target_source_name, valve_status_key, TargetSource, HEALTH_STALE, HEALTH_TARGET_UNCONFIRMED,
};

/// Ordered: a filter "minimum severity" keeps everything >= it.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Debug = 0,
    Info = 1,
    Warning = 2,
    Error = 3,
    Critical = 4,
}

const SEVERITIES: [Severity; 5] = [
    Severity::Debug,
    Severity::Info,
    Severity::Warning,
    Severity::Error,
    Severity::Critical,
];
const SEVERITY_NAMES: [&str; 5] = ["debug", "info", "warning", "error", "critical"];
const SEVERITY_UPPER: [&str; 5] = ["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"];

impl Severity {
    pub fn from_raw(v: u8) -> Option<Self> {
        SEVERITIES.get(usize::from(v)).copied()
    }
}

/// "debug", "info", "warning", "error", "critical".
pub fn severity_name(s: Severity) -> &'static str {
    SEVERITY_NAMES[s as usize]
}

/// Exactly the bytes of `s`, ASCII case-insensitive ("WARNING", "Critical").
pub fn parse_severity(s: &[u8]) -> Option<Severity> {
    SEVERITIES
        .iter()
        .zip(SEVERITY_NAMES)
        .find(|(_, name)| {
            // names are lowercase letters only: `| 0x20` folds exactly 'A'..'Z'
            name.len() == s.len() && s.iter().zip(name.bytes()).all(|(&c, n)| c | 0x20 == n)
        })
        .map(|(&sev, _)| sev)
}

/// RFC 5424 severity number for syslog: 7, 6, 4, 3, 2.
pub fn syslog_severity(s: Severity) -> u8 {
    match s {
        Severity::Debug => 7,
        Severity::Info => 6,
        Severity::Warning => 4,
        Severity::Error => 3,
        Severity::Critical => 2,
    }
}

/// Event codes. Numbers are part of the external API (MQTT events, HTTP, log files): never
/// renumber, only append. arg1/arg2/text meaning per code is binding (DESIGN.md "Event codes");
/// "-" = unused (0 / "").
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventCode {
    // system 1xx
    /// arg1 reset reason (esp_reset_reason), arg2 boot count; text fw version
    Boot = 100,
    /// arg1 keys imported, arg2 keys rejected
    ConfigImported = 101,
    /// arg1 config revision; text source ("web", "import", "factory")
    ConfigSaved = 102,
    /// stored config unreadable -> defaults; arg1 reason code
    ConfigDefaults = 103,
    /// LittleFS mount failed and was formatted
    FsFormatted = 104,
    /// arg1 image size (0 unknown)
    EspOtaStarted = 105,
    /// arg1 image size; text new version if known
    EspOtaDone = 106,
    /// arg1 Update error code; -4 (boot guard of the Rust firmware): the previous image failed
    /// its trial, arg2 1 boot limit, 2 health
    EspOtaFailed = 107,
    /// OTA image confirmed; arg1 seconds after boot
    AppMarkedValid = 108,
    /// arg1 RebootReason, arg2 detail (net watchdog: outage minutes; rollback: missing OTA
    /// checks bit0 net, bit1 http, bit2 stm)
    RebootRequested = 109,
    /// arg1 free heap, arg2 min free heap
    LowHeap = 110,
    /// arg1 step in seconds (clamped to int32)
    TimeSynced = 111,
    /// scheduled calibration skipped: no valid time; arg1 slot key (yyyymmdd)
    CalibTimeMissing = 112,
    /// factory reset pin still set after a reset: settings kept
    FactoryResetSkipped = 113,
    /// arg1 min free bytes, arg2 stack bytes; text task name
    StackLow = 114,
    /// arg1 largest free block, arg2 free heap
    HeapFragmented = 115,
    /// arg1 step (1 open, 2 write, 3 rotate, 4 size limit), arg2 events lost
    LogWriteFailed = 116,
    /// arg1 PI valves, arg2 dropped mask (1 pi, 2 window, 4 messenger, 8 ds18Timeout,
    /// 16 legacyFailsafe); text "ignored <n> keys"
    ImportDropped = 117,
    /// arg1 primary DecodeResult (0 = NVS empty)
    ConfigRestored = 118,
    /// arg1 repair mask, arg2 repairs; text first key path
    ConfigRepaired = 119,
    /// arg1 base schema, arg2 unknown settings kept
    ConfigNewerSchema = 120,
    /// arg1 files, arg2 KiB; text file name or "legacy images"
    FilesRemoved = 121,
    /// heap guard restart; arg1 free heap, arg2 largest free block
    HeapCritical = 122,
    // network / MQTT / web 2xx
    /// arg1 interface (1 eth, 2 wifi); text IP
    NetUp = 200,
    /// arg1 interface
    NetDown = 201,
    MqttConnected = 202,
    /// arg1 PubSubClient state
    MqttDisconnected = 203,
    /// arg1 valve (1-based, 0 unknown), arg2 TargetPayload of a payload reject; text reason
    MqttCommandRejected = 204,
    /// arg1 configs published, arg2 deletes published
    HaDiscoverySent = 205,
    /// retired with the web login in 2.1.0: never raised, number reserved
    AuthFailed = 206,
    /// arg1 window s; text new address
    NetTrialStarted = 207,
    /// arg1 s since the network came up, arg2 1 = by a newer change
    NetTrialConfirmed = 208,
    /// arg1 reason (1 not confirmed, 2 no network, 3 interrupted, 4 user, 5 trial not stored),
    /// arg2 0 ok / -1 revert failed; text previous address
    NetTrialReverted = 209,
    /// arg1 s since the last evidence, arg2 NetEvidence
    NetUnreachable = 210,
    /// arg1 outage s
    NetReachable = 211,
    /// arg1 outage s, arg2 1 eth, 2 wifi, 3 both
    NetInterfaceRestart = 212,
    /// arg1 verdict (1 host, 2 origin, 3 header, 4 content type); text client IP
    RequestRefused = 213,
    /// retired with the web login in 2.1.0: never raised, number reserved
    AuthLocked = 214,
    // STM link 3xx
    LinkUp = 300,
    /// arg1 consecutive timeouts
    LinkDegraded = 301,
    /// arg1 consecutive timeouts
    LinkDown = 302,
    /// arg1 consecutive timeouts, arg2 span s
    StmResetByPolicy = 303,
    StmResetByUser = 304,
    /// arg1 cause (1 gstat uptime, 2 gstat resets, 3 v1 heuristic, 4 link recovered)
    StmRebootDetected = 305,
    /// arg1 protocol (1/2), arg2 hw id; text version
    StmVersion = 306,
    /// text version (below VDM_MIN_STM_VERSION)
    StmIncompatible = 307,
    /// arg1 new total (STM rxOverflow or ESP line overflows), arg2 side (0 esp, 1 stm)
    StmRxOverflow = 308,
    /// arg1 new total, arg2 side (0 esp, 1 stm)
    StmParseErrors = 309,
    /// arg1 command enum value
    StmQueueFull = 310,
    /// arg1 image size; text image name
    StmFlashStarted = 311,
    /// arg1 duration ms; text new STM version
    StmFlashDone = 312,
    /// arg1 FlashError, arg2 failing address; text phase
    StmFlashFailed = 313,
    /// arg1 valve mask, arg2 source (1 STM lease, 2 ESP emulation)
    FailsafeActive = 314,
    /// arg1 seconds, arg2 source
    FailsafeEnded = 315,
    /// arg1 1 broker down, 2 HA offline
    RegulatorLost = 316,
    /// arg1 seconds lost
    RegulatorBack = 317,
    /// arg1 1 no reply, 2 rejected, 3 read-back differs; arg2 attempts
    LeaseConfigFailed = 318,
    /// arg1 watchdog resets
    StmSafeMode = 319,
    StmSafeModeEnded = 320,
    /// arg1 cfgFlags, arg2 cfgEvents
    StmConfigRepaired = 321,
    /// arg1 ore + fe + ne, arg2 rxDropped
    StmUartErrors = 322,
    /// arg1 waited ms, arg2 1 STM reset, 2 flash, 3 ESP restart; text "" or "stm task silent"
    StmEepromWaitTimeout = 323,
    /// arg1 valves, arg2 1 RTC, 2 NVS
    TargetsRestored = 324,
    StmProtectionSuspended = 325,
    // valves 4xx (valve = 0-based index in Event::valve)
    /// arg1 new target, arg2 TargetSource
    TargetSet = 400,
    /// arg1 old status, arg2 new status (Debug unless noted in DESIGN)
    ValveStateChanged = 401,
    /// arg1 calibRetries, arg2 failsafe position (-1 none)
    ValveBlocked = 402,
    /// arg2 ValveFault (protocol 3), -1 unknown
    ValveFailed = 403,
    /// active valve reports open circuit
    ValveNoValve = 404,
    /// arg1 previous bad status
    ValveRecovered = 405,
    /// arg1 1 = scheduled, 2 = automatic retry, 0 = manual/STM
    CalibStarted = 406,
    /// arg1 openCount, arg2 closeCount
    CalibOk = 407,
    /// arg1 calibRetries
    CalibRetry = 408,
    /// arg1 calibRetries (ends in Blocked), arg2 failsafe position (-1 none)
    CalibFailed = 409,
    /// arg1 earlyStops total, arg2 stop reason
    EarlyStop = 410,
    /// arg1 cmdRejected total
    CmdRejected = 411,
    /// arg1 desired, arg2 attempts
    TargetNotConfirmed = 412,
    /// arg1 seconds since last data
    ValveStale = 413,
    /// arg1 counted counts, arg2 stop reason
    ServiceMoveDone = 414,
    /// arg1 min(openCount, closeCount), arg2 minCounts
    CalibStrokeShort = 415,
    // sensors 5xx (valve = NO_VALVE, arg1 config slot 1-based)
    /// arg2 raw value; text sensor id
    TempSensorFailed = 500,
    TempSensorRecovered = 501,
    /// arg1 new count, arg2 kind (0 temp, 1 volt)
    SensorCountChanged = 502,
    /// arg2 raw vad
    VoltSensorFailed = 503,
    // calibration schedule 6xx
    /// arg1 slot key (yyyymmdd), arg2 minutes late
    ScheduledCalibration = 600,
    /// arg1 slot key, arg2 1 no reply, 2 not sent, 3 no result, 4 STM unsupported
    ScheduledCalibrationFailed = 601,
    /// arg1 slot key, arg2 attempts
    ScheduledCalibrationMissed = 602,
}

impl EventCode {
    /// The code with this number; None for numbers the registry does not have.
    pub fn from_raw(v: u16) -> Option<Self> {
        CODES
            .iter()
            .map(|ci| ci.code)
            .find(|&code| code as u16 == v)
    }
}

/// Which events `<main>events` publishes: No, WarnPlus (when the logged severity is Warning or
/// above) or Always.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventMqtt {
    No = 0,
    WarnPlus = 1,
    Always = 2,
}

/// RebootRequested arg1 (the numbers are the existing contract, only appended).
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebootReason {
    User = 0,
    Ota = 1,
    NetWatchdog = 2,
    FactoryReset = 3,
    Rollback = 4,
    NetRevert = 5,
    HeapGuard = 6,
    /// `POST /api/system/ota/switch-back` (Rust firmware, GLUE-DESIGN-ESP §7 item 6; C++ from
    /// 2.1.8): restart into the fallback image, logged as Info
    SwitchBack = 7,
}

impl RebootReason {
    pub fn from_raw(v: u8) -> Option<Self> {
        [
            Self::User,
            Self::Ota,
            Self::NetWatchdog,
            Self::FactoryReset,
            Self::Rollback,
            Self::NetRevert,
            Self::HeapGuard,
            Self::SwitchBack,
        ]
        .get(usize::from(v))
        .copied()
    }
}

/// chars, without NUL
pub const EVENT_TEXT_MAX: usize = 23;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// assigned by [`EventLog::append`], 1-based, never 0 for stored events
    pub seq: u32,
    /// seconds since ESP boot
    pub uptime_s: u32,
    /// UTC seconds, 0 when the clock was not valid
    pub epoch: u32,
    pub code: EventCode,
    pub severity: Severity,
    /// 0..11, [`ALL_VALVES`] or [`NO_VALVE`]
    pub valve: u8,
    pub arg1: i32,
    pub arg2: i32,
    /// read up to its first NUL, like the C++ `char text[kEventTextMax + 1]`
    pub text: Text<EVENT_TEXT_MAX>,
}

/// The C++ member initialisers (also the empty slots of an [`EventLog`]).
const EMPTY_EVENT: Event = Event {
    seq: 0,
    uptime_s: 0,
    epoch: 0,
    code: EventCode::Boot,
    severity: Severity::Info,
    valve: NO_VALVE,
    arg1: 0,
    arg2: 0,
    text: Text::new(),
};

impl Default for Event {
    fn default() -> Self {
        EMPTY_EVENT
    }
}

/// Convenience constructor; `text` (a C string) is truncated to [`EVENT_TEXT_MAX`] (never
/// fails).
pub fn make_event(
    code: EventCode,
    sev: Severity,
    valve: u8,
    arg1: i32,
    arg2: i32,
    text: &[u8],
) -> Event {
    let mut e = Event {
        code,
        severity: sev,
        valve,
        arg1,
        arg2,
        ..Event::default()
    };
    copy_string(&mut e.text, text);
    e
}

// ---------------------------------------------------------------- registry

// Message templates: text plus these control bytes. The templates are constants of this file,
// always well formed (a table instead of one formatting call per code keeps the registry small
// in flash).
/// arg1, decimal
const ARG1: u8 = 0x01;
/// arg2, decimal
const ARG2: u8 = 0x02;
/// the event text
const TXT: u8 = 0x03;
/// up to ELSE_TXT or END_TXT only when the event has a text
const IF_TXT: u8 = 0x04;
const ELSE_TXT: u8 = 0x05;
const END_TXT: u8 = 0x06;
/// + a set letter: arg1 through add_name()
const NAME1: u8 = 0x07;
/// + a set letter: arg2 through add_name()
const NAME2: u8 = 0x08;

// The control bytes inside the template literals (concat! takes literals only).
macro_rules! arg1 {
    () => {
        "\x01"
    };
}
macro_rules! arg2 {
    () => {
        "\x02"
    };
}
macro_rules! txt {
    () => {
        "\x03"
    };
}
macro_rules! if_txt {
    () => {
        "\x04"
    };
}
macro_rules! else_txt {
    () => {
        "\x05"
    };
}
macro_rules! end_txt {
    () => {
        "\x06"
    };
}
macro_rules! name1 {
    ($set:literal) => {
        concat!("\x07", $set)
    };
}
macro_rules! name2 {
    ($set:literal) => {
        concat!("\x08", $set)
    };
}

/// A formatter of a message whose shape depends on the arguments: text, event, event text.
type SpecialFn = fn(&mut TextBuf<'_>, &Event, &[u8]);

enum Message {
    Template(&'static [u8]),
    Special(SpecialFn),
}

struct CodeInfo {
    code: EventCode,
    severity: Severity,
    mqtt: EventMqtt,
    name: &'static str,
    msg: Message,
}

const fn templated(
    code: EventCode,
    severity: Severity,
    mqtt: EventMqtt,
    name: &'static str,
    msg: &'static str,
) -> CodeInfo {
    CodeInfo {
        code,
        severity,
        mqtt,
        name,
        msg: Message::Template(msg.as_bytes()),
    }
}

const fn special(
    code: EventCode,
    severity: Severity,
    mqtt: EventMqtt,
    name: &'static str,
    f: SpecialFn,
) -> CodeInfo {
    CodeInfo {
        code,
        severity,
        mqtt,
        name,
        msg: Message::Special(f),
    }
}

/// Registry order = the order of [`event_mqtt_names`].
#[rustfmt::skip]
static CODES: [CodeInfo; 87] = {
    use EventCode as C;
    use EventMqtt::{Always, No, WarnPlus as Warn};
    use Severity::{Critical, Debug, Error, Info, Warning};
    [
        templated(C::Boot, Info, Warn, "boot",
            concat!("boot (reset ", name1!("r"), ", count ", arg2!(), if_txt!(), ", fw ", txt!(),
                    end_txt!(), ")")),
        templated(C::ConfigImported, Info, Warn, "config_imported",
            concat!("legacy config imported (", arg1!(), " keys, ", arg2!(), " rejected", if_txt!(),
                    ", first ", txt!(), end_txt!(), ")")),
        templated(C::ConfigSaved, Info, Warn, "config_saved",
            concat!("config saved (revision ", arg1!(), if_txt!(), ", ", txt!(), end_txt!(), ")")),
        templated(C::ConfigDefaults, Error, Warn, "config_defaults",
            concat!("stored config unusable, using defaults (reason ", arg1!(), ")")),
        templated(C::FsFormatted, Warning, Warn, "fs_formatted",
            concat!("file system", name1!("S"))),
        templated(C::EspOtaStarted, Info, Warn, "esp_ota_started",
            concat!("ESP update started (", arg1!(), " bytes)")),
        templated(C::EspOtaDone, Info, Warn, "esp_ota_done",
            concat!("ESP update done (", arg1!(), " bytes", if_txt!(), ", ", txt!(), end_txt!(),
                    ")")),
        templated(C::EspOtaFailed, Error, Warn, "esp_ota_failed",
            concat!("ESP update failed (error ", arg1!(), ")")),
        templated(C::AppMarkedValid, Info, Warn, "app_marked_valid",
            concat!("firmware marked valid after ", arg1!(), " s")),
        special(C::RebootRequested, Info, Warn, "reboot_requested", add_reboot_requested),
        templated(C::LowHeap, Warning, Warn, "low_heap",
            concat!("low heap (free ", arg1!(), ", min ", arg2!(), ")")),
        templated(C::TimeSynced, Info, Warn, "time_synced",
            concat!("time synced (step ", arg1!(), " s)")),
        templated(C::CalibTimeMissing, Warning, Warn, "calib_time_missing",
            concat!("scheduled calibration skipped, no valid time (slot ", arg1!(), ")")),
        templated(C::FactoryResetSkipped, Warning, Warn, "factory_reset_skipped",
            "factory reset pin still set: settings kept, remove the jumper"),
        templated(C::StackLow, Warning, Warn, "stack_low",
            concat!("task ", txt!(), ": stack low (", arg1!(), " of ", arg2!(), " bytes free)")),
        templated(C::HeapFragmented, Warning, Warn, "heap_fragmented",
            concat!("heap fragmented (largest block ", arg1!(), ", free ", arg2!(), ")")),
        templated(C::LogWriteFailed, Warning, Warn, "log_write_failed",
            concat!("log file write failed (", name1!("l"), ", ", arg2!(), " events lost)")),
        templated(C::ImportDropped, Warning, Warn, "import_dropped",
            concat!("legacy import dropped ", name2!("f"), " (", arg1!(), " PI valves", if_txt!(),
                    ", ", txt!(), end_txt!(), ")")),
        templated(C::ConfigRestored, Warning, Warn, "config_restored",
            concat!("configuration restored from the backup (", name1!("R"), ")")),
        templated(C::ConfigRepaired, Warning, Warn, "config_repaired",
            concat!("configuration repaired (", arg2!(), " fields", if_txt!(), ", first ", txt!(),
                    end_txt!(), ")")),
        templated(C::ConfigNewerSchema, Warning, Warn, "config_newer_schema",
            concat!("configuration written by a newer firmware (schema ", arg1!(), ", ", arg2!(),
                    " unknown settings kept)")),
        templated(C::FilesRemoved, Info, No, "files_removed",
            concat!("removed ", arg1!(), " files (", arg2!(), " KiB)", if_txt!(), ": ", txt!(),
                    end_txt!())),
        templated(C::HeapCritical, Error, Warn, "heap_critical",
            concat!("heap critical, restarting (free ", arg1!(), ", largest block ", arg2!(), ")")),
        templated(C::NetUp, Info, Warn, "net_up",
            concat!("network up (", name1!("i"), if_txt!(), ", ", txt!(), end_txt!(), ")")),
        templated(C::NetDown, Warning, Warn, "net_down",
            concat!("network down (", name1!("i"), ")")),
        templated(C::MqttConnected, Info, Warn, "mqtt_connected", "MQTT connected"),
        templated(C::MqttDisconnected, Warning, Warn, "mqtt_disconnected",
            concat!("MQTT disconnected (state ", arg1!(), ")")),
        templated(C::MqttCommandRejected, Warning, Warn, "mqtt_command_rejected",
            concat!("MQTT command rejected (", name1!("m"), if_txt!(), ", ", txt!(), end_txt!(),
                    ")")),
        templated(C::HaDiscoverySent, Info, Warn, "ha_discovery_sent",
            concat!("HA discovery sent (", arg1!(), " configs, ", arg2!(), " deletes)")),
        // Retired with the web login (2.1.0): never raised, kept so the number and name stay
        // reserved; not in the HA event types.
        templated(C::AuthFailed, Warning, No, "auth_failed",
            concat!("authentication failed (", arg1!(), " in window", if_txt!(), ", ", txt!(),
                    end_txt!(), ")")),
        templated(C::NetTrialStarted, Info, No, "net_trial_started",
            concat!("network settings on trial for ", arg1!(), " s", if_txt!(), " (", txt!(), ")",
                    end_txt!())),
        templated(C::NetTrialConfirmed, Info, No, "net_trial_confirmed",
            concat!("network settings confirmed after ", arg1!(), " s", name2!("n"))),
        special(C::NetTrialReverted, Warning, Warn, "net_trial_reverted", add_net_trial_reverted),
        templated(C::NetUnreachable, Warning, Warn, "net_unreachable",
            concat!("network unreachable (nothing for ", arg1!(), " s, last ", name2!("e"), ")")),
        templated(C::NetReachable, Info, No, "net_reachable",
            concat!("network reachable again after ", arg1!(), " s")),
        templated(C::NetInterfaceRestart, Warning, Warn, "net_interface_restart",
            concat!("network interface restarted (", name2!("j"), ", after ", arg1!(), " s)")),
        templated(C::RequestRefused, Warning, Warn, "request_refused",
            concat!("request refused (", name1!("q"), ")", if_txt!(), " from ", txt!(),
                    end_txt!())),
        // Retired like auth_failed.
        templated(C::AuthLocked, Warning, No, "auth_locked",
            concat!("login locked for ", arg1!(), " s (lockout ", arg2!(), ")", if_txt!(), " for ",
                    txt!(), end_txt!())),
        templated(C::LinkUp, Info, Warn, "link_up", "STM link up"),
        templated(C::LinkDegraded, Info, Warn, "link_degraded",
            concat!("STM link degraded (", arg1!(), " timeouts)")),
        templated(C::LinkDown, Error, Warn, "link_down",
            concat!("STM link down (", arg1!(), " timeouts)")),
        templated(C::StmResetByPolicy, Error, Warn, "stm_reset_by_policy",
            concat!("STM reset by link policy (", arg1!(), " timeouts in ", arg2!(), " s)")),
        templated(C::StmResetByUser, Info, Warn, "stm_reset_by_user", "STM reset by user"),
        templated(C::StmRebootDetected, Warning, Warn, "stm_reboot_detected",
            concat!("STM reboot detected (", name1!("c"), ")")),
        templated(C::StmVersion, Info, Warn, "stm_version",
            concat!("STM ", if_txt!(), txt!(), else_txt!(), "version unknown", end_txt!(),
                    " (protocol ", arg1!(), ", hw 0x", name2!("3"), ")")),
        templated(C::StmIncompatible, Error, Warn, "stm_incompatible",
            concat!("STM version ", if_txt!(), txt!(), else_txt!(), "unknown", end_txt!(),
                    " is not supported")),
        templated(C::StmRxOverflow, Warning, Warn, "stm_rx_overflow",
            concat!("UART receive overflow (", arg1!(), " total, ", name2!("s"), " side)")),
        templated(C::StmParseErrors, Warning, Warn, "stm_parse_errors",
            concat!("UART parse errors (", arg1!(), " total, ", name2!("s"), " side)")),
        templated(C::StmQueueFull, Warning, Warn, "stm_queue_full",
            concat!("STM request queue full (command ", arg1!(), ")")),
        templated(C::StmFlashStarted, Info, Warn, "stm_flash_started",
            concat!("STM flash started (", arg1!(), " bytes", if_txt!(), ", ", txt!(), end_txt!(),
                    ")")),
        templated(C::StmFlashDone, Info, Warn, "stm_flash_done",
            concat!("STM flash done in ", arg1!(), " ms", if_txt!(), " (", txt!(), ")",
                    end_txt!())),
        templated(C::StmFlashFailed, Critical, Warn, "stm_flash_failed",
            concat!("STM flash failed (error ", arg1!(), " at 0x", name2!("8"), if_txt!(), ", ",
                    txt!(), end_txt!(), ")")),
        templated(C::FailsafeActive, Warning, Always, "failsafe_active",
            concat!("failsafe active on ", name1!("p"), " valves (", name2!("o"), ")")),
        templated(C::FailsafeEnded, Info, Always, "failsafe_ended",
            concat!("failsafe ended after ", arg1!(), " s (", name2!("o"), ")")),
        templated(C::RegulatorLost, Warning, Warn, "regulator_lost",
            concat!("regulator lost (", name1!("g"), ")")),
        templated(C::RegulatorBack, Info, Always, "regulator_back",
            concat!("regulator back after ", arg1!(), " s")),
        templated(C::LeaseConfigFailed, Warning, Warn, "lease_config_failed",
            concat!("failsafe settings not accepted by the STM (", name1!("k"), ", ", arg2!(),
                    " attempts)")),
        templated(C::StmSafeMode, Critical, Always, "stm_safe_mode",
            concat!("STM in safe mode (", arg1!(), " watchdog resets)")),
        templated(C::StmSafeModeEnded, Info, Always, "stm_safe_mode_ended", "STM left safe mode"),
        templated(C::StmConfigRepaired, Warning, Warn, "stm_config_repaired",
            concat!("STM configuration repaired (flags 0x", name1!("2"), ", ", arg2!(),
                    " repairs)")),
        templated(C::StmUartErrors, Warning, Warn, "stm_uart_errors",
            concat!("STM UART errors (", arg1!(), " total, ", arg2!(), " bytes dropped)")),
        templated(C::StmEepromWaitTimeout, Warning, Warn, "stm_eeprom_wait_timeout",
            concat!("STM EEPROM write still pending after ", arg1!(), " ms (", name2!("w"), ")",
                    if_txt!(), ": ", txt!(), end_txt!())),
        templated(C::TargetsRestored, Info, No, "targets_restored",
            concat!("desired targets restored for ", arg1!(), " valves (", name2!("t"), ")")),
        templated(C::StmProtectionSuspended, Error, Warn, "stm_protection_suspended",
            "STM short-circuit and inrush limits suspended until the next STM start"),
        templated(C::TargetSet, Info, Warn, "target_set",
            concat!("target ", arg1!(), " % (", name2!("u"), ")")),
        templated(C::ValveStateChanged, Debug, Warn, "valve_state_changed",
            concat!("state ", name1!("v"), " -> ", name2!("v"))),
        templated(C::ValveBlocked, Error, Warn, "valve_blocked",
            concat!("blocked (calibration retries ", arg1!(), name2!("d"), ")")),
        templated(C::ValveFailed, Error, Warn, "valve_failed", concat!("failed", name2!("a"))),
        templated(C::ValveNoValve, Warning, Warn, "valve_no_valve", "no valve detected"),
        special(C::ValveRecovered, Info, Warn, "valve_recovered", add_valve_recovered),
        templated(C::CalibStarted, Info, Warn, "calib_started",
            concat!("calibration started", name1!("y"))),
        templated(C::CalibOk, Info, Always, "calib_ok",
            concat!("calibration ok (oc ", arg1!(), ", cc ", arg2!(), ")")),
        templated(C::CalibRetry, Warning, Always, "calib_retry",
            concat!("calibration retry ", arg1!())),
        templated(C::CalibFailed, Error, Always, "calib_failed",
            concat!("calibration failed after ", arg1!(), " retries", name2!("d"))),
        templated(C::EarlyStop, Warning, Warn, "early_stop",
            concat!("early stop (total ", arg1!(), ", ", name2!("x"), ")")),
        templated(C::CmdRejected, Warning, Warn, "cmd_rejected",
            concat!("command rejected by the STM (total ", arg1!(), ")")),
        templated(C::TargetNotConfirmed, Warning, Warn, "target_not_confirmed",
            concat!("target ", arg1!(), " % not confirmed after ", arg2!(), " attempts")),
        templated(C::ValveStale, Warning, Warn, "valve_stale",
            concat!("no data for ", arg1!(), " s")),
        templated(C::ServiceMoveDone, Info, Warn, "service_move_done",
            concat!("service move done (", arg1!(), " counts, ", name2!("x"), ")")),
        templated(C::CalibStrokeShort, Warning, Warn, "calib_stroke_short",
            concat!("calibration stroke ", arg1!(), " close to the minimum ", arg2!())),
        templated(C::TempSensorFailed, Warning, Warn, "temp_sensor_failed",
            concat!("temp sensor ", arg1!(), " failed (raw ", arg2!(), if_txt!(), ", ", txt!(),
                    end_txt!(), ")")),
        templated(C::TempSensorRecovered, Info, Warn, "temp_sensor_recovered",
            concat!("temp sensor ", arg1!(), " recovered")),
        templated(C::SensorCountChanged, Info, Warn, "sensor_count_changed",
            concat!(name2!("z"), " sensor count ", arg1!())),
        templated(C::VoltSensorFailed, Warning, Warn, "volt_sensor_failed",
            concat!("volt sensor ", arg1!(), " failed (raw ", arg2!(), ")")),
        templated(C::ScheduledCalibration, Info, Warn, "scheduled_calibration",
            concat!("scheduled calibration (slot ", arg1!(), ", ", arg2!(), " min late)")),
        templated(C::ScheduledCalibrationFailed, Warning, Warn, "scheduled_calibration_failed",
            concat!("scheduled calibration not confirmed (slot ", arg1!(), ", ", name2!("h"), ")")),
        templated(C::ScheduledCalibrationMissed, Error, Warn, "scheduled_calibration_missed",
            concat!("scheduled calibration missed (slot ", arg1!(), ", ", arg2!(), " attempts)")),
    ]
};

/// The registry entry of `c`. Every [`EventCode`] has one (test); the C++ fallbacks of a code
/// outside the registry ("unknown", Info, No, "event <n>") are kept where it is read.
fn find_code(c: EventCode) -> Option<&'static CodeInfo> {
    CODES.iter().find(|ci| ci.code == c)
}

/// "boot", "config_imported", ... snake_case.
pub fn event_code_name(c: EventCode) -> &'static str {
    find_code(c).map_or("unknown", |ci| ci.name)
}

/// Default severity per code (DESIGN.md table).
pub fn event_default_severity(c: EventCode) -> Severity {
    find_code(c).map_or(Severity::Info, |ci| ci.severity)
}

/// True for codes that are "calibration outcomes" (CalibOk/Retry/Failed): they go to MQTT even
/// though CalibOk is Info (architecture R3).
pub fn event_is_calibration_outcome(c: EventCode) -> bool {
    matches!(
        c,
        EventCode::CalibOk | EventCode::CalibRetry | EventCode::CalibFailed
    )
}

/// The MQTT class of `c`.
pub fn event_mqtt(c: EventCode) -> EventMqtt {
    find_code(c).map_or(EventMqtt::No, |ci| ci.mqtt)
}

/// [`event_mqtt`] is Always, or WarnPlus and the logged severity >= Warning.
pub fn event_reaches_mqtt(e: &Event) -> bool {
    match event_mqtt(e.code) {
        EventMqtt::Always => true,
        EventMqtt::WarnPlus => e.severity >= Severity::Warning,
        EventMqtt::No => false,
    }
}

/// Names of every code with [`event_mqtt`] != No, in registry order (the HA event entity's
/// event_types). Writes at most `out.len()` names, returns the total.
pub fn event_mqtt_names(out: &mut [&'static str]) -> usize {
    let mut n = 0;
    for ci in CODES.iter().filter(|ci| ci.mqtt != EventMqtt::No) {
        if let Some(slot) = out.get_mut(n) {
            *slot = ci.name;
        }
        n += 1;
    }
    n
}

/// Name `index` of that list (no table of all of them); None past its end.
pub fn event_mqtt_name(index: usize) -> Option<&'static str> {
    CODES
        .iter()
        .filter(|ci| ci.mqtt != EventMqtt::No)
        .nth(index)
        .map(|ci| ci.name)
}

// ---------------------------------------------------------------- EventLog

/// What [`EventLog::read`] returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventFilter {
    /// return events with seq > since_seq
    pub since_seq: u32,
    pub min_severity: Severity,
    /// NO_VALVE = all valves and system events; a valve index also matches ALL_VALVES events
    pub valve: u8,
}

impl Default for EventFilter {
    fn default() -> Self {
        Self {
            since_seq: 0,
            min_severity: Severity::Debug,
            valve: NO_VALVE,
        }
    }
}

/// Ring buffer of `N` events in its own storage (the C++ `EventLog(storage, N)`; N = 0 is the
/// C++ capacity 0 or null storage). When full the oldest event is overwritten; sequence numbers
/// keep increasing so readers detect the gap (`first_seq() > since_seq + 1`). The logger's
/// capacity (C++ `logger::kEventCapacity`, `VDM_EVENT_CAPACITY`: 32 in the firmware build, 512
/// in the native tests) is the glue's choice of N.
#[derive(Clone, Debug)]
pub struct EventLog<const N: usize> {
    buf: [Event; N],
    /// index of the oldest event
    head: usize,
    size: usize,
    next_seq: u32,
    dropped: u32,
}

impl<const N: usize> Default for EventLog<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> EventLog<N> {
    pub const fn new() -> Self {
        Self {
            buf: [EMPTY_EVENT; N],
            head: 0,
            size: 0,
            next_seq: 1,
            dropped: 0,
        }
    }

    /// Stores a copy, assigns and returns its seq (never 0). N = 0 -> the event is counted as
    /// dropped and 0 is returned.
    pub fn append(&mut self, e: &Event) -> u32 {
        if N == 0 {
            self.dropped = self.dropped.wrapping_add(1);
            return 0;
        }
        let idx = if self.size == N {
            let oldest = self.head;
            self.head = (self.head + 1) % N;
            self.dropped = self.dropped.wrapping_add(1);
            oldest
        } else {
            let idx = (self.head + self.size) % N;
            self.size += 1;
            idx
        };
        let seq = self.next_seq;
        // 0 is never a seq: the numbering goes on at 1 after 2^32 - 1 events
        self.next_seq = seq.checked_add(1).unwrap_or(1);
        let slot = &mut self.buf[idx];
        slot.clone_from(e);
        slot.seq = seq;
        seq
    }

    pub fn size(&self) -> usize {
        self.size
    }

    pub fn capacity(&self) -> usize {
        N
    }

    /// The stored events, oldest first.
    fn events(&self) -> impl Iterator<Item = &Event> {
        let (wrapped, oldest) = self.buf.split_at(self.head);
        oldest.iter().chain(wrapped).take(self.size)
    }

    /// seq of the oldest stored event, 0 if empty
    pub fn first_seq(&self) -> u32 {
        self.events().next().map_or(0, |e| e.seq)
    }

    /// seq of the newest stored event, 0 if empty
    pub fn last_seq(&self) -> u32 {
        self.events().last().map_or(0, |e| e.seq)
    }

    /// overwritten or rejected
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Copies up to `out.len()` matching events, oldest first. Returns the count. `next_since`
    /// receives the seq to pass as since_seq next time (the seq of the last event examined, so
    /// filtered-out events are not re-scanned).
    pub fn read(&self, f: &EventFilter, out: &mut [Event], next_since: &mut u32) -> usize {
        *next_since = f.since_seq;
        let mut n = 0;
        for e in self.events() {
            if e.seq <= f.since_seq {
                continue;
            }
            let Some(slot) = out.get_mut(n) else {
                break;
            };
            *next_since = e.seq;
            if e.severity < f.min_severity {
                continue;
            }
            if f.valve != NO_VALVE && e.valve != f.valve && e.valve != ALL_VALVES {
                continue;
            }
            slot.clone_from(e);
            n += 1;
        }
        n
    }

    /// Event with this seq if still stored (stored events never carry seq 0, so get(0) finds
    /// nothing).
    pub fn get(&self, seq: u32) -> Option<&Event> {
        self.events().find(|e| e.seq == seq)
    }

    /// Forgets the stored events; the numbering goes on. head may stay where it is: an empty
    /// ring starts wherever head points.
    pub fn clear(&mut self) {
        self.size = 0;
    }
}

// ---------------------------------------------------------------- formatting

/// ESP-IDF 4.4 `esp_reset_reason_t`
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
/// The names of [`RebootReason`] (C++ 2.1.7 has the first 7: reason 7 reads "unknown" there).
const REBOOT_REASONS: [&str; 8] = [
    "user",
    "ota",
    "net watchdog",
    "factory reset",
    "rollback",
    "network revert",
    "heap guard",
    "switch back",
];
const LOG_STEPS: [&str; 4] = ["open", "write", "rotate", "size limit"];
const IMPORT_FEATURES: [&str; 5] = ["pi", "window", "messenger", "ds18Timeout", "legacyFailsafe"];
const TRIAL_REVERTS: [&str; 5] = [
    "not confirmed",
    "no network",
    "interrupted",
    "user",
    "trial not stored",
];
const INTERFACES: [&str; 2] = ["eth", "wifi"];
const IFACE_SETS: [&str; 3] = ["eth", "wifi", "eth+wifi"];
const REFUSALS: [&str; 4] = ["host", "origin", "header", "content type"];
const LEASE_SOURCES: [&str; 2] = ["STM lease", "ESP"];
const REGULATOR_LOSS: [&str; 2] = ["MQTT broker disconnected", "Home Assistant offline"];
const LEASE_CONFIG_FAILURES: [&str; 3] = ["no reply", "rejected", "read-back differs"];
const EEPROM_WAIT_ACTIONS: [&str; 3] = ["STM reset", "flash", "ESP restart"];
const RESTORE_SOURCES: [&str; 2] = ["RTC", "NVS"];
const SCHEDULE_FAILURES: [&str; 4] = ["no reply", "not sent", "no result", "STM unsupported"];
const OTA_CHECKS: [&str; 3] = ["net", "http", "stm"];
const REBOOT_CAUSES: [&str; 4] = ["uptime", "resets", "v1 heuristic", "link recovered"];
const SIDES: [&str; 2] = ["esp", "stm"];

/// names[v] for v in 0..names.len(), else "unknown".
fn zero_based(names: &[&'static str], v: i32) -> &'static str {
    usize::try_from(v)
        .ok()
        .and_then(|i| names.get(i))
        .copied()
        .unwrap_or("unknown")
}

/// names[v - 1] for v in 1..=names.len(), else "unknown" (arguments that name a reason).
fn one_based(names: &[&'static str], v: i32) -> &'static str {
    zero_based(names, v.wrapping_sub(1))
}

/// Names of the set bits of `mask` (bit i = names[i]) joined by ", "; false when none of them is
/// set.
fn add_bit_names(t: &mut TextBuf<'_>, mask: i32, names: &[&str]) -> bool {
    let mut sep: &[u8] = b"";
    for (i, name) in names.iter().enumerate() {
        if mask & (1 << i) == 0 {
            continue;
        }
        t.push_bytes(sep);
        t.push_bytes(name.as_bytes());
        sep = b", ";
    }
    !sep.is_empty()
}

/// The name sets of NAME1/NAME2. Event args are int32; the enum/status names take a byte: values
/// that do not fit one never wrap onto a valid name.
fn add_name(t: &mut TextBuf<'_>, set: u8, v: i32) {
    let byte = u8::try_from(v).ok();
    let s = match set {
        b'r' => zero_based(&RESET_REASONS, v),
        b'i' => one_based(&INTERFACES, v),
        b'j' => one_based(&IFACE_SETS, v),
        b'l' => one_based(&LOG_STEPS, v),
        b'q' => one_based(&REFUSALS, v),
        b'c' => one_based(&REBOOT_CAUSES, v),
        b'o' => one_based(&LEASE_SOURCES, v),
        b'g' => one_based(&REGULATOR_LOSS, v),
        b'k' => one_based(&LEASE_CONFIG_FAILURES, v),
        b'w' => one_based(&EEPROM_WAIT_ACTIONS, v),
        b't' => one_based(&RESTORE_SOURCES, v),
        b'h' => one_based(&SCHEDULE_FAILURES, v),
        b's' => zero_based(&SIDES, v),
        b'z' => {
            if v == 1 {
                "volt"
            } else {
                "temp"
            }
        }
        b'S' => {
            if v == -1 {
                " format failed"
            } else {
                " formatted"
            }
        }
        b'n' => {
            if v == 1 {
                " (by a newer change)"
            } else {
                ""
            }
        }
        b'y' => match v {
            1 => " (scheduled)",
            2 => " (automatic retry)",
            _ => "",
        },
        b'e' => byte
            .and_then(NetEvidence::from_raw)
            .map_or("unknown", net_evidence_name),
        b'u' => byte
            .and_then(TargetSource::from_raw)
            .map_or("unknown", target_source_name),
        b'x' => byte
            .and_then(StopReason::from_raw)
            .map_or("unknown", stop_reason_name),
        b'v' => byte.map_or("invalid", valve_status_key),
        // " (<fault>)" when known
        b'a' => {
            if v < 0 {
                return;
            }
            t.push_bytes(b" (");
            t.push_bytes(byte.map_or("unknown", valve_fault_name).as_bytes());
            ")"
        }
        // ", failsafe <v> %" when set
        b'd' => {
            if v >= 0 {
                let _ = write!(t, ", failsafe {v} %");
            }
            return;
        }
        b'f' => {
            if !add_bit_names(t, v, &IMPORT_FEATURES) {
                t.push_bytes(b"nothing");
            }
            return;
        }
        b'm' => {
            if v > 0 {
                let _ = write!(t, "valve {v}");
                return;
            }
            "unknown valve"
        }
        b'R' => {
            if v != 0 {
                let _ = write!(t, "decode error {v}");
                return;
            }
            "NVS empty"
        }
        b'p' => {
            let _ = write!(t, "{}", v.count_ones());
            return;
        }
        // hex digits: the set letter is the width
        b'2' | b'3' | b'8' => {
            let width = usize::from(set - b'0');
            let _ = write!(t, "{:0width$x}", v as u32);
            return;
        }
        _ => "unknown",
    };
    t.push_bytes(s.as_bytes());
}

fn render(t: &mut TextBuf<'_>, tpl: &[u8], e: &Event, txt: &[u8]) {
    let mut p = tpl.iter().copied();
    while let Some(c) = p.next() {
        match c {
            ARG1 => {
                let _ = write!(t, "{}", e.arg1);
            }
            ARG2 => {
                let _ = write!(t, "{}", e.arg2);
            }
            TXT => {
                t.push_bytes(txt);
            }
            // Without a text continue after ELSE_TXT or END_TXT.
            IF_TXT if txt.is_empty() => {
                let _ = p.find(|&c| c == ELSE_TXT || c == END_TXT);
            }
            // Reached from the text branch: skip the other one.
            ELSE_TXT => {
                let _ = p.find(|&c| c == END_TXT);
            }
            IF_TXT | END_TXT => {}
            NAME1 => add_name(t, p.next().unwrap_or(0), e.arg1),
            NAME2 => add_name(t, p.next().unwrap_or(0), e.arg2),
            _ => {
                t.push(c);
            }
        }
    }
}

/// RebootRequested.
fn add_reboot_requested(t: &mut TextBuf<'_>, e: &Event, _txt: &[u8]) {
    let (a1, a2) = (e.arg1, e.arg2);
    t.push_bytes(b"restart requested (");
    t.push_bytes(zero_based(&REBOOT_REASONS, a1).as_bytes());
    if a1 == RebootReason::NetWatchdog as i32 && a2 > 0 {
        let _ = write!(t, ", after {a2} min");
    } else if a1 == RebootReason::Rollback as i32 && (a2 & 0x7) != 0 {
        t.push_bytes(b", missing ");
        add_bit_names(t, a2, &OTA_CHECKS);
    }
    t.push(b')');
}

/// NetTrialReverted.
fn add_net_trial_reverted(t: &mut TextBuf<'_>, e: &Event, txt: &[u8]) {
    let failed = e.arg2 == -1;
    t.push_bytes(if failed {
        b"network settings could not be reverted ("
    } else {
        b"network settings reverted ("
    });
    t.push_bytes(one_based(&TRIAL_REVERTS, e.arg1).as_bytes());
    if !failed && !txt.is_empty() {
        t.push_bytes(b", back to ");
        t.push_bytes(txt);
    }
    t.push(b')');
}

/// ValveRecovered.
fn add_valve_recovered(t: &mut TextBuf<'_>, e: &Event, _txt: &[u8]) {
    t.push_bytes(b"recovered (");
    if e.arg1 != 0 {
        t.push_bytes(b"was ");
        add_name(t, b'v', e.arg1);
    } else {
        let mut sep: &[u8] = b"";
        if e.arg2 & i32::from(HEALTH_STALE) != 0 {
            t.push_bytes(b"data again");
            sep = b", ";
        }
        if e.arg2 & i32::from(HEALTH_TARGET_UNCONFIRMED) != 0 {
            t.push_bytes(sep);
            t.push_bytes(b"target confirmed");
        }
    }
    t.push(b')');
}

/// Civil date from days since 1970-01-01 (H. Hinnant's algorithm, reduced to non-negative day
/// counts: a u32 epoch ends in 2106).
fn civil_from_days(days: u32) -> (u32, u32, u32) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + u32::from(m <= 2);
    (y, m, d)
}

/// "YYYY-MM-DDTHH:MM:SS" of a UTC epoch, then `zone`.
fn add_utc(t: &mut TextBuf<'_>, epoch: u32, zone: &[u8]) {
    let sod = epoch % 86_400;
    let (y, m, d) = civil_from_days(epoch / 86_400);
    let (hh, mm, ss) = (sod / 3600, sod / 60 % 60, sod % 60);
    let _ = write!(t, "{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}");
    t.push_bytes(zone);
}

/// The 12 valve bits of a mask.
const VALVE_MASK_ALL: u16 = (1 << VALVE_COUNT) - 1;

/// The valves of an aggregate: `valve_mask` when it has 2 or more of the 12 valve bits, else 0.
fn aggregate_mask(valve_mask: u16) -> u16 {
    let m = valve_mask & VALVE_MASK_ALL;
    if m.count_ones() >= 2 {
        m
    } else {
        0
    }
}

fn format_message(e: &Event, multi: u16, out: &mut [u8]) -> usize {
    let txt = c_str(&e.text);
    let mut t = TextBuf::new(out);
    if multi != 0 {
        let mut sep = "valves ";
        for v in 0..VALVE_COUNT {
            if multi & (1 << v) == 0 {
                continue;
            }
            let _ = write!(t, "{sep}{}", u32::from(v) + 1);
            sep = ", ";
        }
        t.push_bytes(b": ");
    } else if e.valve < VALVE_COUNT {
        let _ = write!(t, "valve {}: ", u32::from(e.valve) + 1);
    } else if e.valve == ALL_VALVES {
        t.push_bytes(b"all valves: ");
    }
    match find_code(e.code).map(|ci| &ci.msg) {
        Some(Message::Template(tpl)) => render(&mut t, tpl, e, txt),
        Some(Message::Special(f)) => f(&mut t, e, txt),
        None => {
            let _ = write!(t, "event {}", e.code as u16);
        }
    }
    t.len()
}

/// Human-readable message for an event without the prefix, e.g. "valve 3: calibration ok (oc
/// 3120, cc 3350)" ("all valves: " for ALL_VALVES). Valve numbers are 1-based in text. Returns
/// the length (truncated to fit).
pub fn format_event_message(e: &Event, out: &mut [u8]) -> usize {
    format_message(e, 0, out)
}

/// The message of one event standing for the same event on several valves: with 2 or more bits
/// in `valve_mask` (bit v = valve v) the prefix is "valves 1, 3, 12: " (ascending, bits above 11
/// ignored); otherwise the same as [`format_event_message`].
pub fn format_event_message_multi(e: &Event, valve_mask: u16, out: &mut [u8]) -> usize {
    format_message(e, aggregate_mask(valve_mask), out)
}

/// One log/syslog line: "#<seq> <iso8601 or +<uptime>s> <SEV> <code_name>[ v<n>] <message>".
/// Returns the length (truncated to fit).
pub fn format_event_line(e: &Event, out: &mut [u8]) -> usize {
    let mut t = TextBuf::new(out);
    let _ = write!(t, "#{} ", e.seq);
    if e.epoch != 0 {
        add_utc(&mut t, e.epoch, b"Z");
    } else {
        let _ = write!(t, "+{}s", e.uptime_s);
    }
    t.push(b' ');
    t.push_bytes(SEVERITY_UPPER[e.severity as usize].as_bytes());
    t.push(b' ');
    t.push_bytes(event_code_name(e.code).as_bytes());
    if e.valve < VALVE_COUNT {
        let _ = write!(t, " v{}", u32::from(e.valve) + 1);
    }
    t.push(b' ');
    // at most out.len() - 1, so at least the C++ NUL fits
    let used = t.len();
    used + out
        .get_mut(used..)
        .map_or(0, |rest| format_event_message(e, rest))
}

/// writeEventJson() and, with `mqtt`, writeMqttEventJson().
fn write_event_object(jw: &mut JsonWriter<'_>, e: &Event, mqtt: bool, multi: u16) {
    let mut msg = [0u8; 160];
    let n = format_message(e, multi, &mut msg);
    jw.begin_object();
    jw.kv("seq", e.seq);
    jw.kv("t", (e.epoch != 0).then_some(e.epoch));
    jw.kv("up", e.uptime_s);
    jw.kv("sev", severity_name(e.severity));
    jw.kv("code", e.code as u16);
    jw.kv("name", event_code_name(e.code));
    if mqtt {
        jw.kv("event_type", event_code_name(e.code));
    }
    let valve = multi == 0 && e.valve < VALVE_COUNT;
    jw.kv("valve", valve.then(|| u32::from(e.valve) + 1));
    if multi != 0 {
        jw.key("valves");
        jw.begin_array();
        for v in 0..VALVE_COUNT {
            if multi & (1 << v) != 0 {
                jw.value(u32::from(v) + 1);
            }
        }
        jw.end_array();
    }
    jw.kv("a1", e.arg1);
    jw.kv("a2", e.arg2);
    jw.kv("text", c_str(&e.text));
    jw.kv("msg", msg.get(..n).unwrap_or_default());
    jw.end_object();
}

/// JSON object: {"seq":..,"t":<epoch|null>,"up":..,"sev":"warning","code":410,
/// "name":"early_stop","valve":3|null,"a1":..,"a2":..,"text":"..","msg":".."} (valve 1-based;
/// null for system and all-valves events). Returns jw.ok().
pub fn write_event_json(jw: &mut JsonWriter<'_>, e: &Event) -> bool {
    write_event_object(jw, e, false, 0);
    jw.ok()
}

/// The `<main>events` payload: [`write_event_json`]'s keys plus "event_type" (= name, HA event
/// entity) after "name"; with 2 or more bits in `valve_mask` "valve":null,"valves":[1-based
/// ascending] and the message of [`format_event_message_multi`]. Returns jw.ok().
pub fn write_mqtt_event_json(jw: &mut JsonWriter<'_>, e: &Event, valve_mask: u16) -> bool {
    write_event_object(jw, e, true, aggregate_mask(valve_mask));
    jw.ok()
}

/// UTC "2026-09-25T08:13:40+00:00" (HA timestamp sensors). Returns 25; 0 when `out` is shorter
/// than 26 bytes.
pub fn format_utc_timestamp(epoch: u32, out: &mut [u8]) -> usize {
    const LEN: usize = 25; // "2026-09-25T08:13:40+00:00"
    if out.len() < LEN + 1 {
        return 0;
    }
    let mut t = TextBuf::new(out);
    add_utc(&mut t, epoch, b"+00:00");
    t.len()
}

#[cfg(test)]
mod tests;
