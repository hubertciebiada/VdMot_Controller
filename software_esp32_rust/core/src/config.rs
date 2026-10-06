//! Configuration schema of the ESP firmware (port of `vdm/config.h`): one plain struct, defaults,
//! validation, a key-path setter used by HTTP/JSON import, JSON export, a JSON patch reader and
//! versioned binary encodings for NVS. Hardware-free.
//!
//! DESIGN.md "Config schema" is the binding table (key, type, range, default, legacy NVS source).
//! Every change to this struct must update that table, the schema numbers below and the tests.
//!
//! One table drives the key-path setter, validation, JSON export and the NVS encodings, so they
//! can never disagree about a field. Table order is the JSON order; the fields without an ext tag
//! in table order are the `cfg` blob (exactly the 2.0.0 layout), the others go to `cfgx` by tag.
//! The C++ reaches a field through its offset in the group struct; here the group's accessors map
//! the field index to the member.

use core::fmt::Write as _;

use crate::common::{
    build_ha_id, build_hostname, c_str, contains_bytes, copy_string, fmt_trunc, format_f64_fixed,
    format_ipv4, format_one_wire_id, is_host_name, is_printable_text, is_safe_name, is_zero,
    parse_ipv4, parse_one_wire_id, parse_uint, OneWireId, Text, TextBuf, ITEM_NAME_MAX,
    STATION_NAME_MAX, TEMP_SLOT_COUNT, UNIT_MAX, VALVE_COUNT, VOLT_SLOT_COUNT,
};
use crate::failsafe::{
    FAILSAFE_HOLD, FAILSAFE_PCT_DEFAULT, FAILSAFE_TIMEOUT_DEFAULT_MIN, FAILSAFE_TIMEOUT_MAX_MIN,
    FAILSAFE_TIMEOUT_MIN_MIN,
};
use crate::json_writer::JsonWriter;

/// Header of the NVS `cfg` blob: frozen forever (ESP 2.0.0 reads it). New keys go to the `cfgx`
/// blob with a new, never reused tag.
pub const CONFIG_BASE_SCHEMA: u16 = 1;
/// "schema" of JSON documents and `Config::schema`.
pub const CONFIG_JSON_SCHEMA: u16 = 2;
/// passwords (legacy char[65])
pub const SECRET_MAX: usize = 64;
/// broker host, NTP server
pub const HOST_MAX: usize = 64;
/// legacy char[50]
pub const TZ_NAME_MAX: usize = 49;
pub const TZ_POSIX_MAX: usize = 49;
/// below the patch reader's 96: a cut string stays invalid
pub const ALLOWED_HOSTS_MAX: usize = 80;
pub const CLIENT_ID_MAX: usize = 64;
pub const TOPIC_PREFIX_MAX: usize = 32;
/// WiFi SSID, the 802.11 limit (C++ `sizeof NetConfig::ssid - 1`)
pub const SSID_MAX: usize = 32;

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetInterface {
    #[default]
    Auto = 0,
    Ethernet = 1,
    Wifi = 2,
}

impl NetInterface {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Auto),
            1 => Some(Self::Ethernet),
            2 => Some(Self::Wifi),
            _ => None,
        }
    }
}

/// MQTT mode of the configuration (C++ `enum class MqttMode : uint8_t`).
#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MqttMode {
    #[default]
    Off = 0,
    Mqtt = 1,
    MqttHa = 2,
}

impl MqttMode {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Off),
            1 => Some(Self::Mqtt),
            2 => Some(Self::MqttHa),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetConfig {
    pub iface: NetInterface,
    pub dhcp: bool,
    /// legacy u32 layout (parse_ipv4)
    pub ip: u32,
    pub mask: u32,
    pub gateway: u32,
    pub dns: u32,
    /// 1..32 bytes (802.11 limit) or "" = WiFi off
    pub ssid: Text<SSID_MAX>,
    /// "" (open network) or 8..63 bytes (WPA2)
    pub wifi_password: Text<SECRET_MAX>,
    /// legacy netConnTO; 0 = never restart the ESP
    pub reconnect_timeout_min: u8,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            iface: NetInterface::Auto,
            dhcp: true,
            ip: 0,
            mask: 0,
            gateway: 0,
            dns: 0,
            ssid: Text::new(),
            wifi_password: Text::new(),
            reconnect_timeout_min: 5,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimeConfig {
    /// "" disables SNTP
    pub ntp_server: Text<HOST_MAX>,
    pub tz_name: Text<TZ_NAME_MAX>,
    pub tz_posix: Text<TZ_POSIX_MAX>,
}

impl Default for TimeConfig {
    fn default() -> Self {
        Self {
            ntp_server: text(b"pool.ntp.org"),
            tz_name: text(b"Europe/Berlin"),
            tz_posix: text(b"CET-1CEST,M3.5.0,M10.5.0/3"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyslogConfig {
    /// 0 off, 1 errors+warnings, 2 +info, 3 +debug
    pub level: u8,
    /// IPv4; required non-zero when level > 0
    pub server: u32,
    pub port: u16,
}

impl Default for SyslogConfig {
    fn default() -> Self {
        Self {
            level: 0,
            server: 0,
            port: 514,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WebConfig {
    /// Host header values the request guard accepts besides the interface IP and the host name:
    /// "" or 1..4 entries separated by ','.
    pub allowed_hosts: Text<ALLOWED_HOSTS_MAX>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MqttConfig {
    pub mode: MqttMode,
    /// hostname or dotted IPv4
    pub host: Text<HOST_MAX>,
    pub port: u16,
    /// used only when user AND password non-empty (legacy)
    pub user: Text<SECRET_MAX>,
    pub password: Text<SECRET_MAX>,
    /// 5..300
    pub keep_alive_s: u16,
    /// 2..3600
    pub publish_interval_s: u16,
    /// 0..publish_interval_s
    pub min_delay_s: u16,
    // legacy protocolFlags (brokerPF bit order)
    /// bit0
    pub separate: bool,
    /// bit1
    pub all_temps: bool,
    /// bit2
    pub path_as_root: bool,
    /// bit3
    pub up_time: bool,
    /// bit4
    pub on_change: bool,
    /// bit5
    pub retained: bool,
    /// bit6
    pub plain_text: bool,
    /// bit7
    pub diag: bool,
    /// legacy brokerMQF bit2 numFormat
    pub german_decimal: bool,
    // new
    /// diag/* topics + HA entities
    pub new_diag: bool,
    /// <main>events
    pub events: bool,
    /// re-send discovery on connect and HA birth
    pub ha_discovery_on_connect: bool,
    /// "" = the station name ([`mqtt_root_topic`])
    pub root_topic: Text<STATION_NAME_MAX>,
    /// "" = automatic
    pub client_id: Text<CLIENT_ID_MAX>,
    pub discovery_prefix: Text<TOPIC_PREFIX_MAX>,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            mode: MqttMode::Off,
            host: Text::new(),
            port: 1883,
            user: Text::new(),
            password: Text::new(),
            keep_alive_s: 60,
            publish_interval_s: 10,
            min_delay_s: 5,
            separate: true,
            all_temps: true,
            path_as_root: false,
            up_time: true,
            on_change: true,
            retained: true,
            plain_text: true,
            diag: true,
            german_decimal: false,
            new_diag: true,
            events: true,
            ha_discovery_on_connect: true,
            root_topic: Text::new(),
            client_id: Text::new(),
            discovery_prefix: text(b"homeassistant"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValveConfig {
    pub name: Text<ITEM_NAME_MAX>,
    pub active: bool,
    /// 0..100 or FAILSAFE_HOLD
    pub failsafe_pct: u8,
    /// MQTT segment override, "" = from the name
    pub topic: Text<ITEM_NAME_MAX>,
}

impl Default for ValveConfig {
    fn default() -> Self {
        Self {
            name: Text::new(),
            active: false,
            failsafe_pct: FAILSAFE_PCT_DEFAULT,
            topic: Text::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TempSlotConfig {
    pub name: Text<ITEM_NAME_MAX>,
    pub active: bool,
    /// 0.1 C, -100..100 (+-10.0 C)
    pub offset: i16,
    /// zero = empty slot
    pub id: OneWireId,
    pub topic: Text<ITEM_NAME_MAX>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VoltSlotConfig {
    pub name: Text<ITEM_NAME_MAX>,
    pub active: bool,
    /// finite, -1000..1000
    pub offset: f32,
    /// finite, -1000..1000, != 0
    pub factor: f32,
    pub unit: Text<UNIT_MAX>,
    pub id: OneWireId,
    pub topic: Text<ITEM_NAME_MAX>,
}

impl Default for VoltSlotConfig {
    fn default() -> Self {
        Self {
            name: Text::new(),
            active: false,
            offset: 0.0,
            factor: 1.0,
            unit: Text::new(),
            id: OneWireId::default(),
            topic: Text::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CalibScheduleConfig {
    /// bit i = tm_wday i (bit0 Sunday); 0 disables; default Sun+Wed
    pub day_mask: u8,
    /// 0..23 local time
    pub hour: u8,
    /// 0..59 (new; legacy always :00)
    pub minute: u8,
}

impl Default for CalibScheduleConfig {
    fn default() -> Self {
        Self {
            day_mask: 9,
            hour: 0,
            minute: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailsafeConfig {
    /// 0 (off) or 5..1440
    pub timeout_min: u16,
}

impl Default for FailsafeConfig {
    fn default() -> Self {
        Self {
            timeout_min: FAILSAFE_TIMEOUT_DEFAULT_MIN,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub schema: u16,
    /// hostname, MQTT client id/root, HA device
    pub station: Text<STATION_NAME_MAX>,
    pub net: NetConfig,
    pub time: TimeConfig,
    pub syslog: SyslogConfig,
    pub web: WebConfig,
    pub mqtt: MqttConfig,
    pub valves: [ValveConfig; VALVE_COUNT as usize],
    pub temps: [TempSlotConfig; TEMP_SLOT_COUNT as usize],
    pub volts: [VoltSlotConfig; VOLT_SLOT_COUNT as usize],
    pub calib: CalibScheduleConfig,
    pub failsafe: FailsafeConfig,
    /// write the event log to LittleFS (2 x 64 KB rotation)
    pub persist_log: bool,
}

/// Default station name.
const STATION_DEFAULT: &[u8] = b"VdMot";

impl Default for Config {
    fn default() -> Self {
        Self {
            schema: CONFIG_JSON_SCHEMA,
            station: text(STATION_DEFAULT),
            net: NetConfig::default(),
            time: TimeConfig::default(),
            syslog: SyslogConfig::default(),
            web: WebConfig::default(),
            mqtt: MqttConfig::default(),
            valves: core::array::from_fn(|_| ValveConfig::default()),
            temps: core::array::from_fn(|_| TempSlotConfig::default()),
            volts: core::array::from_fn(|_| VoltSlotConfig::default()),
            calib: CalibScheduleConfig::default(),
            failsafe: FailsafeConfig::default(),
            persist_log: true,
        }
    }
}

/// A text member holding `s`.
fn text<const N: usize>(s: &[u8]) -> Text<N> {
    let mut t = Text::new();
    copy_string(&mut t, s);
    t
}

/// Factory defaults (the `Default` values).
pub fn set_defaults(c: &mut Config) {
    *c = Config::default();
}

// ---------------------------------------------------------------- schema table

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// bool, stored 0/1
    Bool,
    /// u8 or u8-based enum, [min, max]
    U8,
    /// u16, [min, max]
    U16,
    /// text, length [min, max], rule
    Str,
    /// like Str, write-only
    Secret,
    /// u32 legacy IPv4 layout, any value
    Ip,
    /// u32 legacy IPv4 layout, contiguous
    Mask,
    /// i16 tenths, [min, max]
    Tenths,
    /// f32, finite, [min, max]
    Float,
    /// OneWireId, zero = empty
    Id,
    /// u8, [min, max] or FAILSAFE_HOLD
    PctHold,
    /// u16, 0 or [min, max]
    OffOrRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Rule {
    None,
    /// is_printable_text: ASCII 0x20..0x7E and UTF-8 (SSIDs, secrets, like legacy)
    Printable,
    /// 0x21..0x7E
    NoSpace,
    /// is_safe_name()
    SafeName,
    /// is_host_name() or dotted IPv4
    Host,
    /// "" or 1..4 is_host_name() entries separated by ',', spaces around them ignored
    HostList,
    /// [A-Za-z0-9._-]
    ClientId,
    /// levels of [A-Za-z0-9_-] separated by single '/'
    TopicPath,
    /// Printable without ' ', '+', '#'; '/' only between two non-empty parts
    TopicSegment,
}

#[derive(Clone, Copy, Debug)]
struct Field {
    name: &'static str,
    /// numbers: value range; strings: length range
    min: i32,
    max: i32,
    kind: Kind,
    /// Str/Secret: the C++ array size incl. NUL (a stored length must be below it)
    cap: u8,
    rule: Rule,
    /// Float: 0 is not allowed
    non_zero: bool,
    /// 0: `cfg` blob; else the `cfgx` record tag (never reused)
    ext: u8,
    /// gone from Config ([`retired`])
    retired: bool,
}

/// Members a kind does not use stay zero.
const fn field(name: &'static str, kind: Kind) -> Field {
    Field {
        name,
        min: 0,
        max: 0,
        kind,
        cap: 0,
        rule: Rule::None,
        non_zero: false,
        ext: 0,
        retired: false,
    }
}

const fn int_field(name: &'static str, kind: Kind, min: i32, max: i32) -> Field {
    Field {
        min,
        max,
        ..field(name, kind)
    }
}

const fn str_field(
    name: &'static str,
    kind: Kind,
    cap: u8,
    min: i32,
    max: i32,
    rule: Rule,
) -> Field {
    Field {
        cap,
        rule,
        ..int_field(name, kind, min, max)
    }
}

const fn float_field(name: &'static str, min: i32, max: i32, non_zero: bool) -> Field {
    Field {
        non_zero,
        ..int_field(name, Kind::Float, min, max)
    }
}

/// A field stored in the `cfgx` blob under `tag`.
const fn ext(f: Field, tag: u8) -> Field {
    Field { ext: tag, ..f }
}

/// A setting an older firmware had: it keeps its place in the `cfg` blob, written as its neutral
/// value (0, "" or false) and skipped when read, so the older firmware still reads the blob after
/// a rollback; the setter accepts and ignores its key, so an older export still imports. `kind`
/// is its old kind and `cap` the size of its old array (strings): they give the bytes skipped.
const fn retired(name: &'static str, kind: Kind, cap: u8) -> Field {
    Field {
        cap,
        retired: true,
        ..field(name, kind)
    }
}

const ROOT_HEAD_FIELDS: [Field; 1] = [str_field(
    "station",
    Kind::Str,
    21,
    1,
    STATION_NAME_MAX as i32,
    Rule::SafeName,
)];

const NET_FIELDS: [Field; 9] = [
    int_field("iface", Kind::U8, 0, 2),
    field("dhcp", Kind::Bool),
    field("ip", Kind::Ip),
    field("mask", Kind::Mask),
    field("gateway", Kind::Ip),
    field("dns", Kind::Ip),
    str_field("ssid", Kind::Str, 33, 0, SSID_MAX as i32, Rule::Printable),
    str_field("wifiPassword", Kind::Secret, 65, 0, 63, Rule::Printable),
    int_field("reconnectTimeoutMin", Kind::U8, 0, 240),
];

const TIME_FIELDS: [Field; 3] = [
    str_field("ntpServer", Kind::Str, 65, 0, HOST_MAX as i32, Rule::Host),
    str_field(
        "tzName",
        Kind::Str,
        50,
        0,
        TZ_NAME_MAX as i32,
        Rule::Printable,
    ),
    str_field(
        "tzPosix",
        Kind::Str,
        50,
        1,
        TZ_POSIX_MAX as i32,
        Rule::NoSpace,
    ),
];

const SYSLOG_FIELDS: [Field; 3] = [
    int_field("level", Kind::U8, 0, 3),
    field("server", Kind::Ip),
    int_field("port", Kind::U16, 1, 65535),
];

/// The web login went in 2.1.0.
const WEB_FIELDS: [Field; 4] = [
    retired("user", Kind::Str, 65),
    retired("password", Kind::Secret, 65),
    retired("protectRead", Kind::Bool, 0),
    ext(
        str_field(
            "allowedHosts",
            Kind::Str,
            81,
            0,
            ALLOWED_HOSTS_MAX as i32,
            Rule::HostList,
        ),
        1,
    ),
];

const MQTT_FIELDS: [Field; 23] = [
    int_field("mode", Kind::U8, 0, 2),
    str_field("host", Kind::Str, 65, 0, HOST_MAX as i32, Rule::Host),
    int_field("port", Kind::U16, 1, 65535),
    str_field("user", Kind::Str, 65, 0, SECRET_MAX as i32, Rule::Printable),
    str_field(
        "password",
        Kind::Secret,
        65,
        0,
        SECRET_MAX as i32,
        Rule::Printable,
    ),
    int_field("keepAliveS", Kind::U16, 5, 300),
    int_field("publishIntervalS", Kind::U16, 2, 3600),
    int_field("minDelayS", Kind::U16, 0, 3600),
    field("separate", Kind::Bool),
    field("allTemps", Kind::Bool),
    field("pathAsRoot", Kind::Bool),
    field("upTime", Kind::Bool),
    field("onChange", Kind::Bool),
    field("retained", Kind::Bool),
    field("plainText", Kind::Bool),
    field("diag", Kind::Bool),
    field("germanDecimal", Kind::Bool),
    field("newDiag", Kind::Bool),
    field("events", Kind::Bool),
    field("haDiscoveryOnConnect", Kind::Bool),
    ext(
        str_field(
            "rootTopic",
            Kind::Str,
            21,
            0,
            STATION_NAME_MAX as i32,
            Rule::SafeName,
        ),
        2,
    ),
    ext(
        str_field(
            "clientId",
            Kind::Str,
            65,
            0,
            CLIENT_ID_MAX as i32,
            Rule::ClientId,
        ),
        3,
    ),
    ext(
        str_field(
            "discoveryPrefix",
            Kind::Str,
            33,
            1,
            TOPIC_PREFIX_MAX as i32,
            Rule::TopicPath,
        ),
        4,
    ),
];

const VALVE_FIELDS: [Field; 4] = [
    str_field(
        "name",
        Kind::Str,
        11,
        0,
        ITEM_NAME_MAX as i32,
        Rule::SafeName,
    ),
    field("active", Kind::Bool),
    ext(int_field("failsafePct", Kind::PctHold, 0, 100), 6),
    ext(
        str_field(
            "topic",
            Kind::Str,
            11,
            0,
            ITEM_NAME_MAX as i32,
            Rule::TopicSegment,
        ),
        7,
    ),
];

const TEMP_FIELDS: [Field; 5] = [
    str_field(
        "name",
        Kind::Str,
        11,
        0,
        ITEM_NAME_MAX as i32,
        Rule::SafeName,
    ),
    field("active", Kind::Bool),
    int_field("offset", Kind::Tenths, -100, 100),
    field("id", Kind::Id),
    ext(
        str_field(
            "topic",
            Kind::Str,
            11,
            0,
            ITEM_NAME_MAX as i32,
            Rule::TopicSegment,
        ),
        8,
    ),
];

const VOLT_FIELDS: [Field; 7] = [
    str_field(
        "name",
        Kind::Str,
        11,
        0,
        ITEM_NAME_MAX as i32,
        Rule::SafeName,
    ),
    field("active", Kind::Bool),
    float_field("offset", -1000, 1000, false),
    float_field("factor", -1000, 1000, true),
    str_field("unit", Kind::Str, 9, 0, UNIT_MAX as i32, Rule::SafeName),
    field("id", Kind::Id),
    ext(
        str_field(
            "topic",
            Kind::Str,
            11,
            0,
            ITEM_NAME_MAX as i32,
            Rule::TopicSegment,
        ),
        9,
    ),
];

const CALIB_FIELDS: [Field; 3] = [
    int_field("dayMask", Kind::U8, 0, 127),
    int_field("hour", Kind::U8, 0, 23),
    int_field("minute", Kind::U8, 0, 59),
];

const FAILSAFE_FIELDS: [Field; 1] = [ext(
    int_field(
        "timeoutMin",
        Kind::OffOrRange,
        FAILSAFE_TIMEOUT_MIN_MIN as i32,
        FAILSAFE_TIMEOUT_MAX_MIN as i32,
    ),
    5,
)];

const ROOT_TAIL_FIELDS: [Field; 1] = [field("persistLog", Kind::Bool)];

/// A stored field value: the integer kinds (enums, bools) as i64, a float by its bits (stored
/// values compare bit by bit, like their encodings), an id, a text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Val<'a> {
    Int(i64),
    F32(u32),
    Id(OneWireId),
    Text(&'a [u8]),
}

/// A member of a group struct as a [`Val`] and back.
trait Stored {
    fn val(&self) -> Val<'_>;
    /// Writes `v`; false (member unchanged) when the member cannot hold it: an enum byte out of
    /// range, which a C++ enum keeps until the repair resets it.
    fn store(&mut self, v: Val<'_>) -> bool;
}

impl Stored for bool {
    fn val(&self) -> Val<'_> {
        Val::Int(i64::from(*self))
    }
    fn store(&mut self, v: Val<'_>) -> bool {
        if let Val::Int(i) = v {
            *self = i != 0;
        }
        true
    }
}

impl Stored for u8 {
    fn val(&self) -> Val<'_> {
        Val::Int(i64::from(*self))
    }
    fn store(&mut self, v: Val<'_>) -> bool {
        if let Val::Int(i) = v {
            *self = i as u8;
        }
        true
    }
}

impl Stored for u16 {
    fn val(&self) -> Val<'_> {
        Val::Int(i64::from(*self))
    }
    fn store(&mut self, v: Val<'_>) -> bool {
        if let Val::Int(i) = v {
            *self = i as u16;
        }
        true
    }
}

impl Stored for u32 {
    fn val(&self) -> Val<'_> {
        Val::Int(i64::from(*self))
    }
    fn store(&mut self, v: Val<'_>) -> bool {
        if let Val::Int(i) = v {
            *self = i as u32;
        }
        true
    }
}

impl Stored for i16 {
    fn val(&self) -> Val<'_> {
        Val::Int(i64::from(*self))
    }
    fn store(&mut self, v: Val<'_>) -> bool {
        if let Val::Int(i) = v {
            *self = i as i16;
        }
        true
    }
}

impl Stored for f32 {
    fn val(&self) -> Val<'_> {
        Val::F32(self.to_bits())
    }
    fn store(&mut self, v: Val<'_>) -> bool {
        if let Val::F32(bits) = v {
            *self = f32::from_bits(bits);
        }
        true
    }
}

impl Stored for OneWireId {
    fn val(&self) -> Val<'_> {
        Val::Id(*self)
    }
    fn store(&mut self, v: Val<'_>) -> bool {
        if let Val::Id(id) = v {
            *self = id;
        }
        true
    }
}

impl<const N: usize> Stored for Text<N> {
    fn val(&self) -> Val<'_> {
        Val::Text(self)
    }
    fn store(&mut self, v: Val<'_>) -> bool {
        if let Val::Text(s) = v {
            copy_string(self, s);
        }
        true
    }
}

/// An enum member from its raw byte.
fn store_enum<T>(dst: &mut T, v: Val<'_>, from_raw: fn(u8) -> Option<T>) -> bool {
    let raw = match v {
        Val::Int(i) => u8::try_from(i).ok().and_then(from_raw),
        _ => None,
    };
    match raw {
        Some(e) => {
            *dst = e;
            true
        }
        None => false,
    }
}

impl Stored for NetInterface {
    fn val(&self) -> Val<'_> {
        Val::Int(i64::from(*self as u8))
    }
    fn store(&mut self, v: Val<'_>) -> bool {
        store_enum(self, v, Self::from_raw)
    }
}

impl Stored for MqttMode {
    fn val(&self) -> Val<'_> {
        Val::Int(i64::from(*self as u8))
    }
    fn store(&mut self, v: Val<'_>) -> bool {
        store_enum(self, v, Self::from_raw)
    }
}

/// The members of a group struct by field index, in table order: one list gives the shared and
/// the mutable accessor, so they cannot disagree.
macro_rules! members {
    ($get:ident, $get_mut:ident, $ty:ty: $($fi:pat => $m:ident),+ $(,)?) => {
        fn $get(s: &$ty, fi: usize) -> &dyn Stored {
            match fi {
                $($fi => &s.$m,)+
            }
        }
        fn $get_mut(s: &mut $ty, fi: usize) -> &mut dyn Stored {
            match fi {
                $($fi => &mut s.$m,)+
            }
        }
    };
}

members!(net_member, net_member_mut, NetConfig:
    0 => iface, 1 => dhcp, 2 => ip, 3 => mask, 4 => gateway, 5 => dns, 6 => ssid,
    7 => wifi_password, _ => reconnect_timeout_min);
members!(time_member, time_member_mut, TimeConfig:
    0 => ntp_server, 1 => tz_name, _ => tz_posix);
members!(syslog_member, syslog_member_mut, SyslogConfig:
    0 => level, 1 => server, _ => port);
// 0..2: the retired web login, never read or written
members!(web_member, web_member_mut, WebConfig: _ => allowed_hosts);
members!(mqtt_member, mqtt_member_mut, MqttConfig:
    0 => mode, 1 => host, 2 => port, 3 => user, 4 => password, 5 => keep_alive_s,
    6 => publish_interval_s, 7 => min_delay_s, 8 => separate, 9 => all_temps,
    10 => path_as_root, 11 => up_time, 12 => on_change, 13 => retained, 14 => plain_text,
    15 => diag, 16 => german_decimal, 17 => new_diag, 18 => events,
    19 => ha_discovery_on_connect, 20 => root_topic, 21 => client_id, _ => discovery_prefix);
members!(valve_member, valve_member_mut, ValveConfig:
    0 => name, 1 => active, 2 => failsafe_pct, _ => topic);
members!(temp_member, temp_member_mut, TempSlotConfig:
    0 => name, 1 => active, 2 => offset, 3 => id, _ => topic);
members!(volt_member, volt_member_mut, VoltSlotConfig:
    0 => name, 1 => active, 2 => offset, 3 => factor, 4 => unit, 5 => id, _ => topic);
members!(calib_member, calib_member_mut, CalibScheduleConfig:
    0 => day_mask, 1 => hour, _ => minute);
members!(failsafe_member, failsafe_member_mut, FailsafeConfig: _ => timeout_min);

struct Group {
    /// "" for the fields directly on Config (C++ nullptr)
    name: &'static str,
    fields: &'static [Field],
    /// array length, 0 = object
    count: u8,
    /// field `fi` of element `e` (None: no such element)
    get: fn(&Config, usize, usize) -> Option<Val<'_>>,
    /// writes field `fi` of element `e` ([`Stored::store`])
    set: fn(&mut Config, usize, usize, Val<'_>) -> bool,
    /// field `fi` of element `e` back to its default (only the group's struct is built: a whole
    /// default Config would be a big stack object)
    reset: fn(&mut Config, usize, usize),
}

/// The members of an object group struct.
macro_rules! object_group {
    ($name:literal, $fields:expr, $member:ident, $member_mut:ident, $part:ident, $ty:ty) => {
        Group {
            name: $name,
            fields: &$fields,
            count: 0,
            get: |c, _, fi| Some($member(&c.$part, fi).val()),
            set: |c, _, fi, v| $member_mut(&mut c.$part, fi).store(v),
            reset: |c, _, fi| {
                $member_mut(&mut c.$part, fi).store($member(&<$ty>::default(), fi).val());
            },
        }
    };
}

/// The elements of an array group.
macro_rules! array_group {
    ($name:literal, $fields:expr, $count:expr, $member:ident, $member_mut:ident, $part:ident,
     $ty:ty) => {
        Group {
            name: $name,
            fields: &$fields,
            count: $count,
            get: |c, e, fi| c.$part.get(e).map(|s| $member(s, fi).val()),
            set: |c, e, fi, v| {
                c.$part
                    .get_mut(e)
                    .is_none_or(|s| $member_mut(s, fi).store(v))
            },
            reset: |c, e, fi| {
                if let Some(s) = c.$part.get_mut(e) {
                    $member_mut(s, fi).store($member(&<$ty>::default(), fi).val());
                }
            },
        }
    };
}

static ROOT_HEAD: Group = Group {
    name: "",
    fields: &ROOT_HEAD_FIELDS,
    count: 0,
    get: |c, _, _| Some(c.station.val()),
    set: |c, _, _, v| c.station.store(v),
    reset: |c, _, _| {
        c.station.store(Val::Text(STATION_DEFAULT));
    },
};
static NET: Group = object_group!(
    "net",
    NET_FIELDS,
    net_member,
    net_member_mut,
    net,
    NetConfig
);
static TIME: Group = object_group!(
    "time",
    TIME_FIELDS,
    time_member,
    time_member_mut,
    time,
    TimeConfig
);
static SYSLOG: Group = object_group!(
    "syslog",
    SYSLOG_FIELDS,
    syslog_member,
    syslog_member_mut,
    syslog,
    SyslogConfig
);
static WEB: Group = object_group!(
    "web",
    WEB_FIELDS,
    web_member,
    web_member_mut,
    web,
    WebConfig
);
static MQTT: Group = object_group!(
    "mqtt",
    MQTT_FIELDS,
    mqtt_member,
    mqtt_member_mut,
    mqtt,
    MqttConfig
);
static VALVES: Group = array_group!(
    "valves",
    VALVE_FIELDS,
    VALVE_COUNT,
    valve_member,
    valve_member_mut,
    valves,
    ValveConfig
);
static TEMPS: Group = array_group!(
    "temps",
    TEMP_FIELDS,
    TEMP_SLOT_COUNT,
    temp_member,
    temp_member_mut,
    temps,
    TempSlotConfig
);
static VOLTS: Group = array_group!(
    "volts",
    VOLT_FIELDS,
    VOLT_SLOT_COUNT,
    volt_member,
    volt_member_mut,
    volts,
    VoltSlotConfig
);
static CALIB: Group = object_group!(
    "calib",
    CALIB_FIELDS,
    calib_member,
    calib_member_mut,
    calib,
    CalibScheduleConfig
);
static FAILSAFE: Group = object_group!(
    "failsafe",
    FAILSAFE_FIELDS,
    failsafe_member,
    failsafe_member_mut,
    failsafe,
    FailsafeConfig
);
// A root group has no index: count 0 gives one element.
static ROOT_TAIL: Group = Group {
    name: "",
    fields: &ROOT_TAIL_FIELDS,
    count: 0,
    get: |c, _, _| Some(c.persist_log.val()),
    set: |c, _, _, v| c.persist_log.store(v),
    reset: |c, _, _| c.persist_log = true,
};

const GROUP_COUNT: usize = 12;
static GROUPS: [&Group; GROUP_COUNT] = [
    &ROOT_HEAD, &NET, &TIME, &SYSLOG, &WEB, &MQTT, &VALVES, &TEMPS, &VOLTS, &CALIB, &FAILSAFE,
    &ROOT_TAIL,
];

/// Longest path of the setter.
const PATH_MAX: usize = 64;

fn element_count(g: &Group) -> usize {
    usize::from(g.count).max(1)
}

// ---------------------------------------------------------------- field rules

fn is_digits_and_dots(s: &[u8]) -> bool {
    s.iter().all(|&c| c.is_ascii_digit() || c == b'.')
}

/// Host names that look numeric must be a valid dotted IPv4 ("192.168.1.300" is a typo, not a
/// host name).
fn host_valid(s: &[u8]) -> bool {
    if s.is_empty() {
        return true;
    }
    if is_digits_and_dots(s) {
        return parse_ipv4(s).is_some();
    }
    is_host_name(s, HOST_MAX)
}

/// "" or 1..4 host names separated by ','; spaces around an entry are ignored.
fn host_list_valid(s: &[u8]) -> bool {
    if s.is_empty() {
        return true;
    }
    let mut entries = 0;
    for part in s.split(|&c| c == b',') {
        let start = part.iter().take_while(|&&c| c == b' ').count();
        let end = part
            .iter()
            .rposition(|&c| c != b' ')
            .map_or(start, |last| last + 1);
        entries += 1;
        if entries > 4 || !is_host_name(part.get(start..end).unwrap_or_default(), HOST_MAX) {
            return false;
        }
    }
    true
}

fn is_id_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

/// '/' only between two non-empty parts.
fn slashes_inside(s: &[u8]) -> bool {
    !matches!(s, [b'/', ..] | [.., b'/']) && !contains_bytes(s, b"//")
}

/// The length and rule of a string field; `s` holds no NUL.
fn string_rule_ok(f: &Field, s: &[u8]) -> bool {
    let len = s.len() as i64;
    if len < i64::from(f.min) || len > i64::from(f.max) {
        return false;
    }
    match f.rule {
        Rule::SafeName => return is_safe_name(s, f.max as usize, f.min == 0),
        Rule::Host => return host_valid(s),
        Rule::HostList => return host_list_valid(s),
        Rule::NoSpace => return s.iter().all(|&c| c > 0x20 && c <= 0x7E),
        Rule::ClientId | Rule::TopicPath => {
            let extra = if f.rule == Rule::ClientId { b'.' } else { b'/' };
            return s.iter().all(|&c| is_id_char(c) || c == extra) && slashes_inside(s);
        }
        Rule::TopicSegment => {
            if s.iter().any(|&c| matches!(c, b' ' | b'+' | b'#')) || !slashes_inside(s) {
                return false;
            }
        }
        Rule::None | Rule::Printable => {}
    }
    is_printable_text(s)
}

/// Mask in the legacy layout (first octet in the low byte) is a run of ones followed by zeros in
/// network order. 0 counts as contiguous.
fn mask_contiguous(legacy: u32) -> bool {
    let inv = !legacy.swap_bytes();
    inv & inv.wrapping_add(1) == 0
}

fn float_ok(f: &Field, v: f64) -> bool {
    if !v.is_finite() || v < f64::from(f.min) || v > f64::from(f.max) {
        return false;
    }
    !(f.non_zero && v as f32 == 0.0)
}

fn in_range(f: &Field, v: i64) -> bool {
    v >= i64::from(f.min) && v <= i64::from(f.max)
}

/// Per-field check of the stored value (a bool, an IPv4 address and an id are always valid).
fn field_valid(f: &Field, v: Val<'_>) -> bool {
    match v {
        Val::Int(i) => match f.kind {
            Kind::U8 | Kind::U16 | Kind::Tenths => in_range(f, i),
            Kind::Mask => mask_contiguous(i as u32),
            Kind::PctHold => i == i64::from(FAILSAFE_HOLD) || in_range(f, i),
            Kind::OffOrRange => i == 0 || in_range(f, i),
            _ => true,
        },
        Val::F32(bits) => float_ok(f, f64::from(f32::from_bits(bits))),
        Val::Text(s) => string_rule_ok(f, s),
        Val::Id(_) => true,
    }
}

// ---------------------------------------------------------------- paths

/// Writes "<group>[.<n>].<field>" (n 1-based) into `out`, cut to fit like `snprintf`; returns
/// the length.
fn write_path(out: &mut [u8], g: &Group, e: usize, field: &str) -> usize {
    if g.name.is_empty() {
        fmt_trunc(out, format_args!("{field}"))
    } else if g.count != 0 {
        fmt_trunc(out, format_args!("{}.{}.{}", g.name, e + 1, field))
    } else {
        fmt_trunc(out, format_args!("{}.{}", g.name, field))
    }
}

/// The text `s` into `out`, cut to fit; returns the length.
fn put_text(out: &mut [u8], s: &[u8]) -> usize {
    let mut w = TextBuf::new(out);
    w.push_bytes(s);
    w.len()
}

/// Where a rule failed: group, element, field name (the C++ `failAt` arguments).
#[derive(Clone, Copy)]
struct At {
    g: &'static Group,
    e: usize,
    field: &'static str,
}

fn at(g: &'static Group, e: usize, field: &'static str) -> At {
    At { g, e, field }
}

/// Equal MQTT segments: names compare with ' ' == '_' (mqtt_topics rule).
fn same_segment(a: &[u8], b: &[u8]) -> bool {
    let seg = |c: u8| if c == b' ' { b'_' } else { c };
    a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| seg(x) == seg(y))
}

/// "1".."12" without leading zeros -> 1..12, else 0.
fn valve_number_segment(s: &[u8]) -> usize {
    if s.first() == Some(&b'0') {
        return 0;
    }
    parse_uint(s, u32::from(VALVE_COUNT)).map_or(0, |v| v as usize)
}

/// Valve i has a name that gives the MQTT segment of an earlier valve (names compare with
/// ' ' == '_') or the number segment of an unnamed valve ("3" while valve 3 has no name).
fn valve_name_clash(c: &Config, i: usize) -> bool {
    let Some(name) = c.valves.get(i).map(|v| &v.name) else {
        return false;
    };
    if name.is_empty() {
        return false;
    }
    if c.valves.iter().take(i).any(|w| same_segment(name, &w.name)) {
        return true;
    }
    // (A valve named after its own number finds itself named: no clash.)
    let num = valve_number_segment(name);
    c.valves
        .get(num.wrapping_sub(1))
        .is_some_and(|w| w.name.is_empty())
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Valve = 0,
    Temp = 1,
    Volt = 2,
}

impl ItemKind {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Valve),
            1 => Some(Self::Temp),
            2 => Some(Self::Volt),
            _ => None,
        }
    }
}

fn item_group(kind: ItemKind) -> &'static Group {
    match kind {
        ItemKind::Valve => &VALVES,
        ItemKind::Temp => &TEMPS,
        ItemKind::Volt => &VOLTS,
    }
}

type ItemText = Text<ITEM_NAME_MAX>;

/// Name and topic override of item i (None: no such item).
fn item_texts(c: &Config, kind: ItemKind, i: usize) -> Option<(&ItemText, &ItemText)> {
    match kind {
        ItemKind::Valve => c.valves.get(i).map(|v| (&v.name, &v.topic)),
        ItemKind::Temp => c.temps.get(i).map(|t| (&t.name, &t.topic)),
        ItemKind::Volt => c.volts.get(i).map(|v| (&v.name, &v.topic)),
    }
}

fn item_texts_mut(
    c: &mut Config,
    kind: ItemKind,
    i: usize,
) -> Option<(&mut ItemText, &mut ItemText)> {
    match kind {
        ItemKind::Valve => c.valves.get_mut(i).map(|v| (&mut v.name, &mut v.topic)),
        ItemKind::Temp => c.temps.get_mut(i).map(|t| (&mut t.name, &mut t.topic)),
        ItemKind::Volt => c.volts.get_mut(i).map(|v| (&mut v.name, &mut v.topic)),
    }
}

/// A temp or volt slot that is active; every valve counts.
fn item_active(c: &Config, kind: ItemKind, i: usize) -> bool {
    match kind {
        ItemKind::Valve => true,
        ItemKind::Temp => c.temps.get(i).is_some_and(|t| t.active),
        ItemKind::Volt => c.volts.get(i).is_some_and(|v| v.active),
    }
}

/// An item's MQTT segment or HA id (never longer than the segment), C++ `char[11]`.
const ITEM_KEY_CAP: usize = ITEM_NAME_MAX + 1;

/// MQTT segment (or with `ha_id` its build_ha_id()) of item i into `out`; its length. None for a
/// temp or volt slot that is not active (validate_config has already made sure active slots have
/// an id).
fn item_key(
    c: &Config,
    kind: ItemKind,
    i: usize,
    ha_id: bool,
    out: &mut [u8; ITEM_KEY_CAP],
) -> Option<usize> {
    if !item_active(c, kind, i) {
        return None;
    }
    let idx = u8::try_from(i).ok()?;
    if !ha_id {
        return Some(item_segment(c, kind, idx, out));
    }
    let mut seg = [0u8; ITEM_KEY_CAP];
    let n = item_segment(c, kind, idx, &mut seg);
    Some(build_ha_id(seg.get(..n).unwrap_or_default(), out))
}

/// The first item whose key equals the key of an earlier one: (later, earlier).
fn duplicate_item(c: &Config, kind: ItemKind, ha_id: bool) -> Option<(usize, usize)> {
    let count = usize::from(item_group(kind).count);
    let mut a = [0u8; ITEM_KEY_CAP];
    let mut b = [0u8; ITEM_KEY_CAP];
    for i in 0..count {
        let Some(na) = item_key(c, kind, i, ha_id, &mut a) else {
            continue;
        };
        for j in 0..i {
            if item_key(c, kind, j, ha_id, &mut b).is_some_and(|nb| a.get(..na) == b.get(..nb)) {
                return Some((i, j));
            }
        }
    }
    None
}

/// A temp or volt slot: its id and its active flag.
trait SensorSlot {
    fn id_active(&self) -> (OneWireId, bool);
    fn id_active_mut(&mut self) -> (&mut OneWireId, &mut bool);
}

impl SensorSlot for TempSlotConfig {
    fn id_active(&self) -> (OneWireId, bool) {
        (self.id, self.active)
    }
    fn id_active_mut(&mut self) -> (&mut OneWireId, &mut bool) {
        (&mut self.id, &mut self.active)
    }
}

impl SensorSlot for VoltSlotConfig {
    fn id_active(&self) -> (OneWireId, bool) {
        (self.id, self.active)
    }
    fn id_active_mut(&mut self) -> (&mut OneWireId, &mut bool) {
        (&mut self.id, &mut self.active)
    }
}

// ---------------------------------------------------------------- validation

/// The rules of 2.0.0: a stored config that passes them can be used as it is.
fn stored_rules_ok(c: &Config) -> Result<(), At> {
    if c.schema != CONFIG_JSON_SCHEMA {
        return Err(at(&ROOT_HEAD, 0, "schema"));
    }
    for &g in &GROUPS {
        for e in 0..element_count(g) {
            for (fi, f) in g.fields.iter().enumerate() {
                if !f.retired && !(g.get)(c, e, fi).is_some_and(|v| field_valid(f, v)) {
                    return Err(at(g, e, f.name));
                }
            }
        }
    }

    let n = &c.net;
    if !n.dhcp {
        if n.ip == 0 {
            return Err(at(&NET, 0, "ip"));
        }
        if n.mask == 0 {
            return Err(at(&NET, 0, "mask"));
        }
        if n.gateway == 0 {
            return Err(at(&NET, 0, "gateway"));
        }
    }
    // Empty = open network (legacy WiFi.begin(ssid, "")); WPA2 needs 8..63.
    let pwd_len = n.wifi_password.len();
    if !n.ssid.is_empty() && pwd_len > 0 && pwd_len < 8 {
        return Err(at(&NET, 0, "wifiPassword"));
    }
    if n.iface == NetInterface::Wifi && n.ssid.is_empty() {
        return Err(at(&NET, 0, "ssid"));
    }

    if c.syslog.level > 0 && c.syslog.server == 0 {
        return Err(at(&SYSLOG, 0, "server"));
    }

    let m = &c.mqtt;
    if m.mode != MqttMode::Off && m.host.is_empty() {
        return Err(at(&MQTT, 0, "host"));
    }
    if m.min_delay_s > m.publish_interval_s {
        return Err(at(&MQTT, 0, "minDelayS"));
    }
    if m.mode == MqttMode::MqttHa && !m.separate {
        return Err(at(&MQTT, 0, "mode"));
    }

    for i in 0..c.valves.len() {
        if valve_name_clash(c, i) {
            return Err(at(&VALVES, i, "name"));
        }
    }
    slot_rules(&c.temps, &TEMPS)?;
    slot_rules(&c.volts, &VOLTS)
}

/// An active slot needs an id; two slots must not share a non-zero id.
fn slot_rules<T: SensorSlot>(slots: &[T], g: &'static Group) -> Result<(), At> {
    for (i, s) in slots.iter().enumerate() {
        let (id, active) = s.id_active();
        if is_zero(&id) {
            if active {
                return Err(at(g, i, "active"));
            }
            continue;
        }
        if slots.iter().take(i).any(|t| t.id_active().0 == id) {
            return Err(at(g, i, "id"));
        }
    }
    Ok(())
}

/// An item that clashes with an earlier one: its topic override, else its name.
fn fail_item(c: &Config, kind: ItemKind, i: usize) -> At {
    let topic = item_texts(c, kind, i).is_some_and(|(_, topic)| !topic.is_empty());
    at(item_group(kind), i, if topic { "topic" } else { "name" })
}

/// V1-V3, the rules added after 2.0.0: checked on save and import; a stored config that breaks
/// them is repaired, not dropped.
fn new_rules_ok(c: &Config) -> Result<(), At> {
    // V1: Home Assistant reads a decimal point.
    let m = &c.mqtt;
    if m.mode == MqttMode::MqttHa && m.german_decimal {
        return Err(at(&MQTT, 0, "germanDecimal"));
    }
    // V2: one MQTT segment per valve; V3: one HA id per valve, per active slot.
    let valves = duplicate_item(c, ItemKind::Valve, false)
        .or_else(|| duplicate_item(c, ItemKind::Valve, true));
    if let Some((later, _)) = valves {
        return Err(fail_item(c, ItemKind::Valve, later));
    }
    for kind in [ItemKind::Temp, ItemKind::Volt] {
        if let Some((later, _)) = duplicate_item(c, kind, true) {
            return Err(fail_item(c, kind, later));
        }
    }
    Ok(())
}

/// Validation of a whole config: Ok when every field is within its documented range and the
/// cross-field rules hold:
///  - static IP: when !dhcp, ip/mask/gateway non-zero and mask contiguous;
///  - ssid set -> wifi_password "" (open network) or 8..63 bytes; iface Wifi -> ssid set;
///  - syslog level > 0 -> server != 0 and port != 0;
///  - ssid and secrets printable text (ASCII or UTF-8);
///  - mqtt mode != Off -> host valid (is_host_name or IPv4), port != 0; min_delay_s <=
///    publish_interval_s; mode MqttHa -> separate == true;
///  - names: is_safe_name (station 1..20, others 0..10); duplicate non-empty valve names are
///    rejected (MQTT segment collision), also a valve named like the number segment of another
///    valve ("3" vs unnamed valve 3);
///  - temp/volt slots: two slots must not share a non-zero id; active slot needs an id;
///  - V1: mode MqttHa -> german_decimal false (HA reads a decimal point);
///  - V2: the valves' MQTT segments ([`item_segment`]) are pairwise different (path: the later
///    valve's topic when it has one, else its name);
///  - V3: build_ha_id() of the segments is unique among the valves, among the active temp slots
///    with an id and among the active volt slots with an id (path as V2).
///
/// On failure `path` receives the first offending key path ("mqtt.port", "valves.3.name";
/// indices 1-based), cut to fit, and the error is its length.
pub fn validate_config(c: &Config, path: &mut [u8]) -> Result<(), usize> {
    stored_rules_ok(c)
        .and_then(|()| new_rules_ok(c))
        .map_err(|a| write_path(path, a.g, a.e, a.field))
}

// ---------------------------------------------------------------- helpers

/// MQTT topic segment of one item: the `topic` override when set, else the name with ' ' -> '_',
/// else the 1-based index ("3"). Returns the length; 0 when it does not fit or idx0 is out of
/// range.
pub fn item_segment(c: &Config, kind: ItemKind, idx0: u8, out: &mut [u8]) -> usize {
    let Some((name, topic)) = item_texts(c, kind, usize::from(idx0)) else {
        return 0;
    };
    let src = if topic.is_empty() { name } else { topic };
    let mut w = TextBuf::new(out);
    if src.is_empty() {
        let _ = write!(w, "{}", u32::from(idx0) + 1);
    } else {
        for &ch in src {
            w.push(if ch == b' ' { b'_' } else { ch });
        }
    }
    w.fit()
}

/// MQTT root: mqtt.root_topic when set, else the station name.
pub fn mqtt_root_topic(c: &Config) -> &[u8] {
    if c.mqtt.root_topic.is_empty() {
        &c.station
    } else {
        &c.mqtt.root_topic
    }
}

/// DNS server of the network settings: with a static IP net.dns, or the gateway when dns is 0;
/// with DHCP net.dns.
pub fn effective_dns(n: &NetConfig) -> u32 {
    if !n.dhcp && n.dns == 0 {
        n.gateway
    } else {
        n.dns
    }
}

/// A change that can make the device unreachable (network trial): iface or dhcp differ; with
/// !after.dhcp ip, mask, gateway or effective_dns differ; ssid or wifi_password differ unless
/// iface is Ethernet in both.
pub fn net_trial_required(before: &NetConfig, after: &NetConfig) -> bool {
    if before.iface != after.iface || before.dhcp != after.dhcp {
        return true;
    }
    if !after.dhcp
        && (before.ip != after.ip
            || before.mask != after.mask
            || before.gateway != after.gateway
            || effective_dns(before) != effective_dns(after))
    {
        return true;
    }
    if after.iface == NetInterface::Ethernet {
        return false; // the same iface in both
    }
    before.ssid != after.ssid || before.wifi_password != after.wifi_password
}

/// [`net_trial_required`]
pub const RESTART_NETWORK: u8 = 0x01;
/// build_hostname() of the stations differs
pub const RESTART_HOSTNAME: u8 = 0x02;

/// Why applying `after` over `before` needs an ESP restart: [`RESTART_NETWORK`] =
/// [`net_trial_required`], [`RESTART_HOSTNAME`] = build_hostname() of the stations differs. The
/// single rule for net reconfigure (restart iff != 0) and the POST /api/config answer.
pub fn config_restart_reasons(before: &Config, after: &Config) -> u8 {
    config_restart_reasons_from(&before.net, &before.station, after)
}

/// [`config_restart_reasons`] from the parts of `before` it reads (net keeps only these; the
/// C++ overload).
pub fn config_restart_reasons_from(
    before_net: &NetConfig,
    before_station: &[u8],
    after: &Config,
) -> u8 {
    let mut r = 0;
    if net_trial_required(before_net, &after.net) {
        r += RESTART_NETWORK;
    }
    let mut a = [0u8; STATION_NAME_MAX + 1];
    let mut b = [0u8; STATION_NAME_MAX + 1];
    let na = build_hostname(before_station, &mut a);
    let nb = build_hostname(&after.station, &mut b);
    if a.get(..na) != b.get(..nb) {
        r += RESTART_HOSTNAME;
    }
    r
}

/// The fields of a valve or sensor slot that shape its MQTT topics.
fn topic_field(f: &Field) -> bool {
    matches!(f.name, "name" | "active" | "topic" | "id")
}

/// The fields of group g, element e (with `items_only` the [`topic_field`] ones) hold equal
/// stored values (equal encodings: a string ends at its end, a float compares by its bits).
fn same_fields(a: &Config, b: &Config, g: &Group, e: usize, items_only: bool) -> bool {
    g.fields
        .iter()
        .enumerate()
        .all(|(fi, f)| (items_only && !topic_field(f)) || (g.get)(a, e, fi) == (g.get)(b, e, fi))
}

/// True when the MQTT topics or the session differ: station, every mqtt.* field, and per valve
/// name/active/topic, per temp and volt slot name/active/topic/id. Not failsafe.timeout_min, not
/// valves.N.failsafe_pct (they apply live). True -> reconnect with a clean session.
pub fn mqtt_topic_config_changed(a: &Config, b: &Config) -> bool {
    !same_fields(a, b, &ROOT_HEAD, 0, false)
        || !same_fields(a, b, &MQTT, 0, false)
        || [&VALVES, &TEMPS, &VOLTS]
            .iter()
            .any(|g| (0..usize::from(g.count)).any(|e| !same_fields(a, b, g, e, true)))
}

// ---------------------------------------------------------------- key paths

/// A JSON scalar handed over by glue (the parsed request value).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ConfigValue<'a> {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// the exact bytes (they may hold a NUL; the C++ `s, len`)
    Str(&'a [u8]),
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetResult {
    Ok = 0,
    UnknownKey = 1,
    /// e.g. string for a bool; numbers given as strings are accepted for numeric keys (legacy UI
    /// sends "3"), bools accept 0/1
    WrongType = 2,
    /// violates the per-field range (cross-field rules are checked by validate_config after the
    /// whole patch)
    OutOfRange = 3,
    /// "schema" with a value other than 1..CONFIG_JSON_SCHEMA
    ReadOnly = 4,
}

impl SetResult {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Ok),
            1 => Some(Self::UnknownKey),
            2 => Some(Self::WrongType),
            3 => Some(Self::OutOfRange),
            4 => Some(Self::ReadOnly),
            _ => None,
        }
    }
}

/// "ok", "unknown_key", "wrong_type", "out_of_range", "read_only".
pub fn set_result_name(r: SetResult) -> &'static str {
    match r {
        SetResult::Ok => "ok",
        SetResult::UnknownKey => "unknown_key",
        SetResult::WrongType => "wrong_type",
        SetResult::OutOfRange => "out_of_range",
        SetResult::ReadOnly => "read_only",
    }
}

/// Strict JSON number grammar over all of `s` (>= 1 byte):
/// `-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?`
fn is_json_number(s: &[u8]) -> bool {
    let digits = |s: &[u8]| s.iter().take_while(|c| c.is_ascii_digit()).count();
    let mut i = usize::from(s.first() == Some(&b'-'));
    match s.get(i) {
        Some(b'0') => i += 1,
        Some(b'1'..=b'9') => i += digits(s.get(i..).unwrap_or_default()),
        _ => return false,
    }
    if s.get(i) == Some(&b'.') {
        let n = digits(s.get(i + 1..).unwrap_or_default());
        if n == 0 {
            return false;
        }
        i += 1 + n;
    }
    if matches!(s.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(s.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let n = digits(s.get(i..).unwrap_or_default());
        if n == 0 {
            return false;
        }
        i += n;
    }
    i == s.len()
}

/// Longest number text that is converted; longer ones count as infinite.
const NUMBER_TEXT_MAX: usize = 40;

/// JSON number text -> double (correctly rounded, as glibc strtod). Texts longer than
/// [`NUMBER_TEXT_MAX`] count as +infinity (every numeric key rejects them); None when not a JSON
/// number.
fn number_from_text(s: &[u8]) -> Option<f64> {
    if !is_json_number(s) {
        return None;
    }
    if s.len() > NUMBER_TEXT_MAX {
        return Some(f64::INFINITY);
    }
    core::str::from_utf8(s).ok()?.parse().ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConvError {
    WrongType,
    OutOfRange,
}

impl From<ConvError> for SetResult {
    fn from(e: ConvError) -> Self {
        match e {
            ConvError::WrongType => SetResult::WrongType,
            ConvError::OutOfRange => SetResult::OutOfRange,
        }
    }
}

/// `x` has no fraction (|x| < 2^63; finite).
fn is_integral(x: f64) -> bool {
    x as i64 as f64 == x
}

/// Integer keys: Int, integral Float, or a numeric Str ("3").
fn to_integer(v: &ConfigValue<'_>) -> Result<i64, ConvError> {
    let d = match *v {
        ConfigValue::Int(i) => return Ok(i),
        ConfigValue::Float(f) => f,
        ConfigValue::Str(s) => number_from_text(s).ok_or(ConvError::WrongType)?,
        _ => return Err(ConvError::WrongType),
    };
    if !d.is_finite() || d < -2147483648.0 || d > 4294967295.0 {
        return Err(ConvError::OutOfRange);
    }
    if !is_integral(d) {
        return Err(ConvError::WrongType);
    }
    Ok(d as i64)
}

/// Bool keys: Bool, 0/1 as number, or "true"/"false"/"0"/"1" as string.
fn to_bool(v: &ConfigValue<'_>) -> Result<bool, ConvError> {
    match *v {
        ConfigValue::Bool(b) => Ok(b),
        ConfigValue::Str(b"true" | b"1") => Ok(true),
        ConfigValue::Str(b"false" | b"0") => Ok(false),
        ConfigValue::Str(_) => Err(ConvError::WrongType),
        _ => match to_integer(v)? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(ConvError::OutOfRange),
        },
    }
}

fn to_double(v: &ConfigValue<'_>) -> Result<f64, ConvError> {
    match *v {
        ConfigValue::Int(i) => Ok(i as f64),
        ConfigValue::Float(f) => Ok(f),
        ConfigValue::Str(s) => number_from_text(s).ok_or(ConvError::WrongType),
        _ => Err(ConvError::WrongType),
    }
}

/// C -> tenths, rounded half away from zero, within [min, max]. The epsilon absorbs the binary
/// representation error of decimal inputs (0.15 * 10 = 1.4999999...), so 0.15 -> 2 and
/// -0.15 -> -2. NaN and +-inf are out of range (in C++ every comparison with NaN fails), so
/// nothing out of range is cast.
fn celsius_to_tenths(c: f64, min: i32, max: i32) -> Option<i16> {
    let t = c * 10.0;
    if !t.is_finite() {
        return None;
    }
    const EPS: f64 = 1e-9;
    // floor() of the non-negative rounding argument is its truncation (beyond 2^63 it saturates,
    // which is out of range as the exact value is)
    let floor = |x: f64| x as i64 as f64;
    let r = if t >= 0.0 {
        floor(t + 0.5 + EPS)
    } else {
        -floor(-t + 0.5 + EPS)
    };
    if !(r >= f64::from(min) && r <= f64::from(max)) {
        return None;
    }
    Some(r as i16)
}

/// The text value of a Str-like key; None for another type.
fn str_value<'a>(v: &ConfigValue<'a>) -> Option<&'a [u8]> {
    match *v {
        ConfigValue::Str(s) => Some(s),
        _ => None,
    }
}

/// The checked value of field `f` from `v`, or the set result (Ok without a value: an empty
/// secret that keeps the stored one).
fn field_value<'a>(
    f: &Field,
    v: &ConfigValue<'a>,
    clear_secrets: bool,
) -> Result<Val<'a>, SetResult> {
    match f.kind {
        Kind::Bool => Ok(Val::Int(i64::from(to_bool(v)?))),
        Kind::U8 | Kind::U16 | Kind::PctHold | Kind::OffOrRange => {
            let i = to_integer(v)?;
            // [min, max], or the special value of the kind (hold, off).
            let special = (f.kind == Kind::PctHold && i == i64::from(FAILSAFE_HOLD))
                || (f.kind == Kind::OffOrRange && i == 0);
            if !special && !in_range(f, i) {
                return Err(SetResult::OutOfRange);
            }
            Ok(Val::Int(i))
        }
        Kind::Str | Kind::Secret => {
            let s = str_value(v).ok_or(SetResult::WrongType)?;
            if f.kind == Kind::Secret && s.is_empty() && !clear_secrets {
                return Err(SetResult::Ok);
            }
            if s.contains(&0) || !string_rule_ok(f, s) {
                return Err(SetResult::OutOfRange);
            }
            Ok(Val::Text(s))
        }
        Kind::Ip | Kind::Mask => {
            let s = str_value(v).ok_or(SetResult::WrongType)?;
            let ip = if s.is_empty() {
                0
            } else {
                parse_ipv4(s).ok_or(SetResult::OutOfRange)?
            };
            if f.kind == Kind::Mask && !mask_contiguous(ip) {
                return Err(SetResult::OutOfRange);
            }
            Ok(Val::Int(i64::from(ip)))
        }
        Kind::Tenths => {
            let t = celsius_to_tenths(to_double(v)?, f.min, f.max).ok_or(SetResult::OutOfRange)?;
            Ok(Val::Int(i64::from(t)))
        }
        Kind::Float => {
            let d = to_double(v)?;
            if !float_ok(f, d) {
                return Err(SetResult::OutOfRange);
            }
            Ok(Val::F32((d as f32).to_bits()))
        }
        Kind::Id => {
            let s = str_value(v).ok_or(SetResult::WrongType)?;
            if s.is_empty() {
                return Ok(Val::Id(OneWireId::default()));
            }
            parse_one_wire_id(s)
                .map(Val::Id)
                .ok_or(SetResult::OutOfRange)
        }
    }
}

/// "<secret>Set" pseudo keys emitted by write_config_json.
fn is_secret_flag(f: &Field, seg: &[u8]) -> bool {
    f.kind == Kind::Secret && seg.strip_prefix(f.name.as_bytes()) == Some(b"Set".as_slice())
}

fn set_in_group(
    c: &mut Config,
    g: &Group,
    element: usize,
    seg: &[u8],
    v: &ConfigValue<'_>,
    clear_secrets: bool,
) -> SetResult {
    for (fi, f) in g.fields.iter().enumerate() {
        let flag = is_secret_flag(f, seg);
        if !flag && seg != f.name.as_bytes() {
            continue;
        }
        if f.retired {
            return SetResult::Ok; // whatever its value
        }
        if flag {
            return if matches!(v, ConfigValue::Bool(_)) {
                SetResult::Ok
            } else {
                SetResult::WrongType
            };
        }
        return match field_value(f, v, clear_secrets) {
            Ok(val) => {
                (g.set)(c, element, fi, val);
                SetResult::Ok
            }
            Err(r) => r,
        };
    }
    SetResult::UnknownKey
}

/// The text before the first '.' and the rest after it (None: no '.').
fn split_segment(s: &[u8]) -> (&[u8], Option<&[u8]>) {
    let mut parts = s.splitn(2, |&c| c == b'.');
    (parts.next().unwrap_or_default(), parts.next())
}

/// Sets one field by dotted path (a C string), 1-based array indices: "station", "net.dhcp",
/// "net.ip" (dotted string), "mqtt.port", "valves.3.name", "temps.12.offset" (float C, rounded to
/// 0.1), "temps.12.id" ("hh-..." or "" to clear), "calib.dayMask", ... The complete key list is
/// DESIGN.md "Config schema". Secrets ("mqtt.password", "net.wifiPassword") are write-only; an
/// empty string for a secret means "unchanged" unless `clear_secrets` is true. So that an
/// exported document can be posted back unchanged, "schema" 1..CONFIG_JSON_SCHEMA (a 2.0.0
/// export says 1) and the export's "<secret>Set" booleans ("net.wifiPasswordSet",
/// "mqtt.passwordSet") are accepted as no-ops. The keys of removed settings ("web.user",
/// "web.password", "web.passwordSet", "web.protectRead") are accepted with any value and
/// ignored, so an export of an older firmware still imports. Paths are at most 64 bytes; array
/// indices are 1..N without leading zeros.
pub fn set_config_value(
    c: &mut Config,
    path: &[u8],
    v: &ConfigValue<'_>,
    clear_secrets: bool,
) -> SetResult {
    let path = c_str(path);
    if path.is_empty() || path.len() > PATH_MAX {
        return SetResult::UnknownKey;
    }
    let (seg, rest) = split_segment(path);
    if seg.is_empty() {
        return SetResult::UnknownKey;
    }
    let Some(rest) = rest else {
        if seg == b"schema" {
            // A 2.0.0 export says 1; both post back unchanged.
            return match to_integer(v) {
                Ok(i) if i >= 1 && i <= i64::from(CONFIG_JSON_SCHEMA) => SetResult::Ok,
                _ => SetResult::ReadOnly,
            };
        }
        for g in [&ROOT_HEAD, &ROOT_TAIL] {
            let r = set_in_group(c, g, 0, seg, v, clear_secrets);
            if r != SetResult::UnknownKey {
                return r;
            }
        }
        return SetResult::UnknownKey;
    };
    let Some(&g) = GROUPS
        .iter()
        .find(|g| !g.name.is_empty() && seg == g.name.as_bytes())
    else {
        return SetResult::UnknownKey;
    };
    let mut field = rest;
    let mut element = 0;
    if g.count != 0 {
        let (idx, after) = split_segment(rest);
        let Some(after) = after else {
            return SetResult::UnknownKey;
        };
        if idx.first() == Some(&b'0') {
            return SetResult::UnknownKey;
        }
        let Some(n) = parse_uint(idx, u32::from(g.count)) else {
            return SetResult::UnknownKey;
        };
        element = n as usize - 1;
        field = after;
    }
    // the field must end the path (an empty one names nothing)
    match split_segment(field) {
        (name, None) => set_in_group(c, g, element, name, v, clear_secrets),
        _ => SetResult::UnknownKey,
    }
}

// ---------------------------------------------------------------- JSON export

/// What POST /api/config reports about a saved document.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ApplyInfo {
    /// config_restart_reasons() != 0
    pub restart_required: bool,
    /// net_trial_required()
    pub net_trial: bool,
}

fn parse_f32(s: &[u8]) -> Option<f32> {
    core::str::from_utf8(s).ok()?.parse().ok()
}

/// Shortest decimal (0..6 places) that reads back as the same float.
fn write_float(jw: &mut JsonWriter<'_>, v: f32) {
    if !v.is_finite() {
        jw.null_value();
        return;
    }
    let v = if v == 0.0 { 0.0 } else { v }; // no "-0"
                                            // |v| <= FLT_MAX (39 digits) plus sign, point and 6 decimals: fits.
    let mut tmp = [0u8; 64];
    let mut n = 0;
    for d in 0..=6 {
        n = format_f64_fixed(f64::from(v), d, &mut tmp);
        if parse_f32(tmp.get(..n).unwrap_or_default()) == Some(v) {
            break;
        }
    }
    jw.raw(tmp.get(..n).unwrap_or_default());
}

fn write_field(jw: &mut JsonWriter<'_>, f: &Field, v: Val<'_>) {
    if f.kind == Kind::Secret {
        let mut name = [0u8; 32];
        let n = fmt_trunc(&mut name, format_args!("{}Set", f.name));
        jw.key(name.get(..n).unwrap_or_default());
        jw.value(v != Val::Text(&[]));
        return;
    }
    jw.key(f.name);
    match v {
        Val::Int(i) => match f.kind {
            Kind::Bool => jw.value(i != 0),
            Kind::Ip | Kind::Mask => {
                let mut tmp = [0u8; 16];
                let n = format_ipv4(i as u32, &mut tmp);
                jw.value(tmp.get(..n).unwrap_or_default());
            }
            Kind::Tenths => jw.fixed(i as i32, 1),
            _ => jw.value(i),
        },
        Val::F32(bits) => write_float(jw, f32::from_bits(bits)),
        Val::Id(id) => {
            let mut tmp = [0u8; 24];
            let n = if is_zero(&id) {
                0
            } else {
                format_one_wire_id(&id, &mut tmp)
            };
            jw.value(tmp.get(..n).unwrap_or_default());
        }
        Val::Text(s) => jw.value(s),
    }
}

fn write_fields(jw: &mut JsonWriter<'_>, c: &Config, g: &Group, e: usize) {
    for (fi, f) in g.fields.iter().enumerate() {
        if f.retired {
            continue;
        }
        if let Some(v) = (g.get)(c, e, fi) {
            write_field(jw, f, v);
        }
    }
}

/// JSON export in the same key structure as [`set_config_value`], e.g.
/// `{"schema":2,"station":"VdMot","net":{...},"valves":[{"name":..},...],...}`. Never a secret:
/// `"<key>Set":true/false` stands in for its value (e.g. "passwordSet"), and the keys of removed
/// settings are left out. `apply` appends "restartRequired" and "netTrial" as the last two
/// members of the root object. Returns `jw.ok()`.
pub fn write_config_json(jw: &mut JsonWriter<'_>, c: &Config, apply: Option<&ApplyInfo>) -> bool {
    jw.begin_object();
    jw.kv("schema", c.schema);
    for &g in &GROUPS {
        if g.name.is_empty() {
            write_fields(jw, c, g, 0);
        } else if g.count == 0 {
            jw.key(g.name);
            jw.begin_object();
            write_fields(jw, c, g, 0);
            jw.end_object();
        } else {
            jw.key(g.name);
            jw.begin_array();
            for e in 0..usize::from(g.count) {
                jw.begin_object();
                write_fields(jw, c, g, e);
                jw.end_object();
            }
            jw.end_array();
        }
    }
    if let Some(a) = apply {
        jw.kv("restartRequired", a.restart_required);
        jw.kv("netTrial", a.net_trial);
    }
    jw.end_object();
    jw.ok()
}

// ---------------------------------------------------------------- JSON patch

/// Nesting limit of a patch document.
pub const CONFIG_JSON_MAX_DEPTH: u8 = 8;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PatchResult {
    Ok = 0,
    /// not a JSON object / syntax error / nested deeper than CONFIG_JSON_MAX_DEPTH
    Malformed = 1,
    /// set_config_value results for the first failing key
    UnknownKey = 2,
    WrongType = 3,
    OutOfRange = 4,
    ReadOnly = 5,
    /// every key applied, but validate_config failed
    Invalid = 6,
}

impl PatchResult {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Ok),
            1 => Some(Self::Malformed),
            2 => Some(Self::UnknownKey),
            3 => Some(Self::WrongType),
            4 => Some(Self::OutOfRange),
            5 => Some(Self::ReadOnly),
            6 => Some(Self::Invalid),
            _ => None,
        }
    }
}

/// "ok", "malformed", "unknown_key", "wrong_type", "out_of_range", "read_only", "invalid".
pub fn patch_result_name(r: PatchResult) -> &'static str {
    match r {
        PatchResult::Ok => "ok",
        PatchResult::Malformed => "malformed",
        PatchResult::UnknownKey => "unknown_key",
        PatchResult::WrongType => "wrong_type",
        PatchResult::OutOfRange => "out_of_range",
        PatchResult::ReadOnly => "read_only",
        PatchResult::Invalid => "invalid",
    }
}

fn from_set(r: SetResult) -> PatchResult {
    match r {
        SetResult::Ok => PatchResult::Ok,
        SetResult::UnknownKey => PatchResult::UnknownKey,
        SetResult::WrongType => PatchResult::WrongType,
        SetResult::OutOfRange => PatchResult::OutOfRange,
        SetResult::ReadOnly => PatchResult::ReadOnly,
    }
}

/// Longest decoded string kept; every string field is shorter (test `string_field_limits`), so a
/// cut value is still rejected as out of range.
const PATCH_STR_MAX: usize = 96;

/// Appends byte `x` to `out[..*n]` when it fits.
fn put(out: &mut [u8], n: &mut usize, x: u8) {
    if let Some(slot) = out.get_mut(*n) {
        *slot = x;
        *n += 1;
    }
}

/// Appends code point `ch` as UTF-8, as far as it fits (a cut string is longer than every field
/// anyway).
fn put_utf8(ch: u32, out: &mut [u8], n: &mut usize) {
    let mut b = [0u8; 4];
    let bytes = char::from_u32(ch).map_or(&[][..], |c| c.encode_utf8(&mut b).as_bytes());
    for &x in bytes {
        put(out, n, x);
    }
}

/// A JSON text being read.
struct Cursor<'a> {
    s: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn literal(&mut self, word: &[u8]) -> bool {
        if !self
            .s
            .get(self.pos..)
            .is_some_and(|rest| rest.starts_with(word))
        {
            return false;
        }
        self.pos += word.len();
        true
    }

    /// Four hex digits; the read position stays on a failure.
    fn hex4(&mut self) -> Option<u32> {
        let digits = self.s.get(self.pos..self.pos + 4)?;
        let mut v = 0;
        for &c in digits {
            v = v * 16 + char::from(c).to_digit(16)?;
        }
        self.pos += 4;
        Some(v)
    }

    /// \uXXXX after the "\u" (pos at the first hex digit), incl. a following low surrogate for a
    /// high one: the code point (names and secrets may be UTF-8, the field rules decide).
    fn unicode_escape(&mut self) -> Option<u32> {
        let cp = self.hex4()?;
        if (0xDC00..=0xDFFF).contains(&cp) {
            return None;
        }
        if !(0xD800..=0xDBFF).contains(&cp) {
            return Some(cp);
        }
        // a high surrogate: a low one must follow
        if self.s.get(self.pos..self.pos + 2) != Some(b"\\u".as_slice()) {
            return None;
        }
        self.pos += 2;
        let lo = self.hex4()?;
        if !(0xDC00..=0xDFFF).contains(&lo) {
            return None;
        }
        Some(0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00))
    }

    /// A string at pos (which is '"'): decodes at most `out.len()` bytes into `out` and returns
    /// how many were kept (a longer string is cut: every field and path is far shorter). None on
    /// a syntax error; pos then is where the C++ reports it.
    fn string(&mut self, out: &mut [u8]) -> Option<usize> {
        let mut n = 0;
        self.pos += 1;
        while let Some(c) = self.peek() {
            self.pos += 1;
            if c == b'"' {
                return Some(n);
            }
            if c == b'\\' {
                let e = self.peek()?;
                self.pos += 1;
                let ch = match e {
                    b'"' | b'\\' | b'/' => u32::from(e),
                    b'b' => 0x08,
                    b'f' => 0x0C,
                    b'n' => 0x0A,
                    b'r' => 0x0D,
                    b't' => 0x09,
                    b'u' => self.unicode_escape()?,
                    _ => return None,
                };
                put_utf8(ch, out, &mut n);
            } else if c < 0x20 {
                return None; // raw control characters are not allowed in JSON strings
            } else {
                put(out, &mut n, c); // raw bytes (UTF-8) as they are
            }
        }
        None // unterminated
    }

    /// Every number becomes a double: all integer keys are far below 2^53, and larger values are
    /// out of range for every key anyway. None (pos back at its start) when not a number.
    fn number(&mut self) -> Option<f64> {
        let start = self.pos;
        while matches!(
            self.peek(),
            Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
        ) {
            self.pos += 1;
        }
        let v = self.s.get(start..self.pos).and_then(number_from_text);
        if v.is_none() {
            self.pos = start; // report the offset of the bad number
        }
        v
    }
}

/// The dotted path of the current value.
struct PatchPath {
    buf: [u8; PATH_MAX],
    len: usize,
    /// a segment did not fit (or named nothing): reported as an unknown key at a leaf below
    overflow: bool,
}

impl PatchPath {
    fn text(&self) -> &[u8] {
        self.buf.get(..self.len).unwrap_or_default()
    }

    /// Appends one path segment. An overlong path is remembered and reported as an unknown key
    /// when a leaf is reached below it.
    fn push(&mut self, seg: &[u8]) {
        let dot = usize::from(self.len != 0);
        let end = self.len + dot + seg.len();
        if self.overflow || end > PATH_MAX {
            self.overflow = true;
            return;
        }
        if dot == 1 {
            put(&mut self.buf, &mut self.len, b'.');
        }
        for &c in seg {
            put(&mut self.buf, &mut self.len, c);
        }
    }
}

/// A scalar leaf; a string by its length in the walker's string buffer.
#[derive(Clone, Copy)]
enum Scalar {
    Null,
    Bool(bool),
    Num(f64),
    Str(usize),
}

/// Recursive-descent walker over a bounded JSON text. Pass 1 (`cfg` None) checks the syntax and
/// reads the root "clearSecrets"; pass 2 applies every scalar leaf through set_config_value with
/// its dotted path (array elements are 1-based path segments). No heap; nesting <=
/// CONFIG_JSON_MAX_DEPTH.
struct PatchWalker<'a, 'c> {
    cur: Cursor<'a>,
    cfg: Option<&'c mut Config>,
    clear_secrets: bool,
    clear_secrets_value: bool,
    result: PatchResult,
    path: PatchPath,
    /// current string value
    str_buf: [u8; PATCH_STR_MAX],
    /// current object key (copied into the path at once)
    key_buf: [u8; PATCH_STR_MAX],
}

impl<'a, 'c> PatchWalker<'a, 'c> {
    fn new(s: &'a [u8], cfg: Option<&'c mut Config>, clear_secrets: bool) -> Self {
        Self {
            cur: Cursor { s, pos: 0 },
            cfg,
            clear_secrets,
            clear_secrets_value: false,
            result: PatchResult::Ok,
            path: PatchPath {
                buf: [0; PATH_MAX],
                len: 0,
                overflow: false,
            },
            str_buf: [0; PATCH_STR_MAX],
            key_buf: [0; PATCH_STR_MAX],
        }
    }

    /// The whole document: one object, then only whitespace.
    fn document(&mut self) -> bool {
        self.cur.ws();
        if self.cur.peek() != Some(b'{') {
            return self.syntax_error();
        }
        if !self.value(0) {
            return false;
        }
        self.cur.ws();
        self.cur.pos == self.cur.s.len() || self.syntax_error()
    }

    fn syntax_error(&mut self) -> bool {
        self.result = PatchResult::Malformed;
        false
    }

    fn fail(&mut self, r: SetResult) -> bool {
        self.result = from_set(r);
        false
    }

    fn leaf(&mut self, depth: u8, v: Scalar) -> bool {
        let is_clear = depth == 1 && !self.path.overflow && self.path.text() == b"clearSecrets";
        let r = if let Some(cfg) = self.cfg.as_deref_mut() {
            if is_clear {
                SetResult::Ok
            } else if self.path.overflow {
                SetResult::UnknownKey
            } else {
                let value = match v {
                    Scalar::Null => ConfigValue::Null,
                    Scalar::Bool(b) => ConfigValue::Bool(b),
                    Scalar::Num(f) => ConfigValue::Float(f),
                    Scalar::Str(n) => ConfigValue::Str(self.str_buf.get(..n).unwrap_or_default()),
                };
                set_config_value(cfg, self.path.text(), &value, self.clear_secrets)
            }
        } else if !is_clear {
            SetResult::Ok
        } else if let Scalar::Bool(b) = v {
            self.clear_secrets_value = b;
            SetResult::Ok
        } else {
            SetResult::WrongType
        };
        r == SetResult::Ok || self.fail(r)
    }

    fn value(&mut self, depth: u8) -> bool {
        self.cur.ws();
        let Some(c) = self.cur.peek() else {
            return self.syntax_error();
        };
        if c == b'{' || c == b'[' {
            return self.container(depth, c == b'{');
        }
        let v = match c {
            b'"' => match self.cur.string(&mut self.str_buf) {
                Some(n) => Scalar::Str(n),
                None => return self.syntax_error(),
            },
            b't' | b'f' => {
                let word: &[u8] = if c == b't' { b"true" } else { b"false" };
                if !self.cur.literal(word) {
                    return self.syntax_error();
                }
                Scalar::Bool(c == b't')
            }
            b'n' => {
                if !self.cur.literal(b"null") {
                    return self.syntax_error();
                }
                Scalar::Null
            }
            b'-' | b'0'..=b'9' => match self.cur.number() {
                Some(f) => Scalar::Num(f),
                None => return self.syntax_error(),
            },
            _ => return self.syntax_error(),
        };
        self.leaf(depth, v)
    }

    fn container(&mut self, depth: u8, is_object: bool) -> bool {
        if depth >= CONFIG_JSON_MAX_DEPTH {
            return self.syntax_error();
        }
        self.cur.pos += 1;
        self.cur.ws();
        let close = if is_object { b'}' } else { b']' };
        if self.cur.peek() == Some(close) {
            self.cur.pos += 1;
            return true;
        }
        let mut index: u32 = 1;
        loop {
            self.cur.ws();
            let saved_len = self.path.len;
            let saved_overflow = self.path.overflow;
            if is_object {
                if self.cur.peek() != Some(b'"') {
                    return self.syntax_error();
                }
                let Some(n) = self.cur.string(&mut self.key_buf) else {
                    return self.syntax_error();
                };
                self.cur.ws();
                if self.cur.peek() != Some(b':') {
                    return self.syntax_error();
                }
                self.cur.pos += 1;
                // Empty keys and keys with NUL name nothing; a cut (96+) key overflows the
                // 64-byte path in push().
                let key = self.key_buf.get(..n).unwrap_or_default();
                if key.is_empty() || key.contains(&0) {
                    self.path.overflow = true;
                } else {
                    self.path.push(key);
                }
            } else {
                let mut num = [0u8; 12];
                let n = fmt_trunc(&mut num, format_args!("{index}"));
                self.path.push(num.get(..n).unwrap_or_default());
            }
            if !self.value(depth + 1) {
                return false;
            }
            self.path.len = saved_len;
            self.path.overflow = saved_overflow;
            self.cur.ws();
            match self.cur.peek() {
                Some(b) if b == close => {
                    self.cur.pos += 1;
                    return true;
                }
                Some(b',') => self.cur.pos += 1,
                _ => return self.syntax_error(),
            }
            index += 1;
        }
    }
}

/// POST /api/config: applies a partial config document to `c` in place, so the caller passes a
/// copy of the active config and commits it only on Ok. Structure: the one write_config_json
/// emits (nested objects, arrays with element i -> path index i+1), nested objects keyed by index
/// (`{"valves":{"3":{"name":"x"}}}`) or dotted keys (`{"valves.3.name":"x"}`), in any mix. A
/// root "clearSecrets": true (bool only) makes empty secrets clear the stored ones. Pass 1 checks
/// the whole syntax (strict RFC 8259: escapes incl. surrogate pairs, number grammar, no trailing
/// data) before anything is changed; pass 2 applies every scalar with set_config_value and stops
/// at the first failure; then validate_config runs. Returns the result and the length of `path`:
/// the offending key path on failure, or "@<byte offset>" for Malformed, cut to fit; "" on Ok.
/// No heap, bounded recursion, strings longer than 96 bytes are truncated (and then rejected as
/// out of range by every string field). \uXXXX escapes (surrogate pairs included) are decoded to
/// UTF-8 like raw UTF-8 bytes; the field rules decide (names, SSIDs and secrets accept UTF-8
/// text, control characters and invalid sequences are out of range).
pub fn apply_config_json(c: &mut Config, json: &[u8], path: &mut [u8]) -> (PatchResult, usize) {
    let mut check = PatchWalker::new(json, None, false);
    if !check.document() {
        let n = if check.result == PatchResult::Malformed {
            fmt_trunc(path, format_args!("@{}", check.cur.pos))
        } else {
            put_text(path, check.path.text())
        };
        return (check.result, n);
    }
    let mut apply = PatchWalker::new(json, Some(&mut *c), check.clear_secrets_value);
    if !apply.document() {
        return (apply.result, put_text(path, apply.path.text()));
    }
    match validate_config(c, path) {
        Ok(()) => (PatchResult::Ok, 0),
        Err(n) => (PatchResult::Invalid, n),
    }
}

// ---------------------------------------------------------------- binary

/// The largest `cfg` blob.
pub const CONFIG_BLOB_MAX: usize = 4096;
/// The largest `cfgx` blob.
pub const CONFIG_EXT_BLOB_MAX: usize = 1536;
/// unknown records carried over (a newer firmware's keys)
pub const CONFIG_EXT_KEEP_MAX: usize = 512;

const MAGIC: &[u8; 4] = b"VDMC";
/// magic + schema + payload length
const HEADER_SIZE: usize = 8;
const CRC_SIZE: usize = 4;
const EXT_MAGIC: &[u8; 4] = b"VDMX";
const EXT_VERSION: u8 = 1;
/// magic + version + payload length
const EXT_HEADER_SIZE: usize = 7;
/// tag, element, length
const EXT_RECORD_HEAD: usize = 3;

/// Writes into a caller buffer; a write that does not fit fails this and every later one.
struct ByteOut<'a> {
    out: &'a mut [u8],
    len: usize,
    ok: bool,
}

impl<'a> ByteOut<'a> {
    fn new(out: &'a mut [u8]) -> Self {
        Self {
            out,
            len: 0,
            ok: true,
        }
    }

    fn bytes(&mut self, p: &[u8]) {
        if !self.ok {
            return;
        }
        let end = self.len + p.len();
        match self.out.get_mut(self.len..end) {
            Some(dst) => {
                dst.copy_from_slice(p);
                self.len = end;
            }
            None => self.ok = false,
        }
    }

    fn u8(&mut self, v: u8) {
        self.bytes(&[v]);
    }

    fn u16(&mut self, v: u16) {
        self.bytes(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    /// Patches the u16 payload length at `at` (after the header), appends the CRC-32 of
    /// everything before it; the blob length, 0 when it does not fit.
    fn finish(mut self, header: usize, at: usize) -> usize {
        if !self.ok {
            return 0;
        }
        let payload = (self.len - header) as u16; // < 2 KiB by the schema
        if let Some(dst) = self.out.get_mut(at..at + 2) {
            dst.copy_from_slice(&payload.to_le_bytes());
        }
        let crc = crc32(self.out.get(..self.len).unwrap_or_default(), 0);
        self.u32(crc);
        if self.ok {
            self.len
        } else {
            0
        }
    }
}

/// Reads from a blob; a read past its end fails this and every later one (reads then give 0).
struct ByteIn<'a> {
    p: &'a [u8],
    pos: usize,
    ok: bool,
}

impl<'a> ByteIn<'a> {
    fn new(p: &'a [u8]) -> Self {
        Self {
            p,
            pos: 0,
            ok: true,
        }
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if !self.ok {
            return None;
        }
        let s = self.p.get(self.pos..self.pos + n);
        match s {
            Some(_) => self.pos += n,
            None => self.ok = false,
        }
        s
    }

    fn u8(&mut self) -> u8 {
        self.take(1).and_then(|b| b.first().copied()).unwrap_or(0)
    }

    fn u16(&mut self) -> u16 {
        self.take(2)
            .and_then(|b| b.try_into().ok())
            .map_or(0, u16::from_le_bytes)
    }

    fn u32(&mut self) -> u32 {
        self.take(4)
            .and_then(|b| b.try_into().ok())
            .map_or(0, u32::from_le_bytes)
    }

    fn at_end(&self) -> bool {
        self.pos == self.p.len()
    }
}

/// The `cfg` blob encoding of one field (strings as u8 length + bytes, numbers little endian).
fn encode_val(bo: &mut ByteOut<'_>, f: &Field, v: Val<'_>) {
    match v {
        Val::Int(i) => match f.kind {
            Kind::Bool | Kind::U8 | Kind::PctHold => bo.u8(i as u8),
            Kind::U16 | Kind::Tenths | Kind::OffOrRange => bo.u16(i as u16),
            _ => bo.u32(i as u32),
        },
        Val::F32(bits) => bo.u32(bits),
        Val::Id(id) => bo.bytes(&id.b),
        Val::Text(s) => {
            bo.u8(s.len() as u8);
            bo.bytes(s);
        }
    }
}

/// Structural decoding of one field (value ranges are checked by the repair): a bool byte above
/// 1, a string length >= its array size or a NUL inside a string fail `inp`.
fn decode_val<'a>(inp: &mut ByteIn<'a>, f: &Field) -> Val<'a> {
    match f.kind {
        Kind::Bool | Kind::U8 | Kind::PctHold => {
            let b = inp.u8();
            if f.kind == Kind::Bool && b > 1 {
                inp.ok = false;
            }
            Val::Int(i64::from(b))
        }
        Kind::U16 | Kind::OffOrRange => Val::Int(i64::from(inp.u16())),
        Kind::Tenths => Val::Int(i64::from(inp.u16() as i16)),
        Kind::Str | Kind::Secret => {
            let n = inp.u8();
            if n >= f.cap {
                inp.ok = false;
            }
            let s = inp.take(usize::from(n)).unwrap_or_default();
            if s.contains(&0) {
                inp.ok = false;
            }
            Val::Text(s)
        }
        Kind::Ip | Kind::Mask => Val::Int(i64::from(inp.u32())),
        Kind::Float => Val::F32(inp.u32()),
        Kind::Id => {
            let mut id = OneWireId::default();
            if let Some(b) = inp.take(id.b.len()) {
                id.b.copy_from_slice(b);
            }
            Val::Id(id)
        }
    }
}

/// NVS `cfg` blob: magic "VDMC", u16 CONFIG_BASE_SCHEMA, u16 payload length, payload (explicit
/// little-endian field-by-field encoding, strings as u8 length + bytes), u32 CRC32 of everything
/// before it. Never a raw struct dump. Holds exactly the fields of ESP 2.0.0 in their order; a
/// setting removed since keeps its place with its neutral value (web.user "", web.password "",
/// web.protectRead false) and is skipped when decoded. Returns the bytes written, 0 when `out` is
/// too small.
pub fn encode_config(c: &Config, out: &mut [u8]) -> usize {
    let mut bo = ByteOut::new(out);
    bo.bytes(MAGIC);
    bo.u16(CONFIG_BASE_SCHEMA);
    bo.u16(0); // payload length, patched by finish()
    for &g in &GROUPS {
        for e in 0..element_count(g) {
            for (fi, f) in g.fields.iter().enumerate() {
                if f.ext != 0 {
                    continue;
                }
                if f.retired {
                    bo.u8(0); // the neutral value of every kind: 0, "" (length 0) or false
                } else if let Some(v) = (g.get)(c, e, fi) {
                    encode_val(&mut bo, f, v);
                }
            }
        }
    }
    bo.finish(HEADER_SIZE, 6)
}

/// Repairs that make a loaded config valid ([`sanitize_config`]). The mask is external
/// (/api/status config.repairs, event config_repaired): the bit of a rule that is gone stays
/// unused (0x0020: web user without password, 0x4000: web password without user, the web login
/// removed in 2.1.0).
/// a field outside its per-field rule was reset to its default
pub const REPAIR_FIELD: u32 = 0x0001;
/// incomplete static IP -> dhcp
pub const REPAIR_STATIC_IP: u32 = 0x0002;
/// ssid with a 1..7 byte password -> WiFi credentials cleared
pub const REPAIR_WIFI_PASSWORD: u32 = 0x0004;
/// iface WiFi without ssid -> auto
pub const REPAIR_WIFI_IFACE: u32 = 0x0008;
/// level > 0 without server -> 0
pub const REPAIR_SYSLOG: u32 = 0x0010;
/// mode != off without host -> off
pub const REPAIR_MQTT_HOST: u32 = 0x0040;
/// min_delay_s > publish_interval_s -> publish_interval_s
pub const REPAIR_MIN_DELAY: u32 = 0x0080;
/// HA mode without separate -> mode MQTT
pub const REPAIR_HA_SEPARATE: u32 = 0x0100;
/// HA mode with german_decimal -> german_decimal false (V1)
pub const REPAIR_HA_DECIMAL: u32 = 0x0200;
/// duplicate / number-clashing valve names -> later name cleared
pub const REPAIR_VALVE_NAMES: u32 = 0x0400;
/// duplicate temp/volt ids -> later id cleared
pub const REPAIR_SLOT_IDS: u32 = 0x0800;
/// active slot without id -> inactive
pub const REPAIR_SLOT_ACTIVE: u32 = 0x1000;
/// duplicate valve segment by an override -> later override cleared
pub const REPAIR_TOPICS: u32 = 0x2000;
/// equal HA ids (V3) -> the later name (or override) cleared
pub const REPAIR_HA_IDS: u32 = 0x8000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Repairs {
    /// REPAIR_* values applied
    pub mask: u32,
    /// repairs applied
    pub count: u16,
    /// bit i: valve i name cleared
    pub valve_names: u16,
    /// bit i: valve i topic override cleared
    pub valve_topics: u16,
    /// bit i: temp slot i id cleared
    pub temp_ids: u64,
    /// bit i: temp slot i set inactive
    pub temp_active: u64,
    pub volt_ids: u8,
    pub volt_active: u8,
    /// key path of the first repair ("calib.hour", "valves.3.name"; C++ char[40])
    pub first: Text<39>,
}

/// The fields whose stored byte their member cannot hold (an enum out of range), per group
/// index: the decoder reports them, the repair resets them like any other bad field.
#[derive(Clone, Copy, Default)]
struct RawBad([Option<usize>; GROUP_COUNT]);

impl RawBad {
    fn mark(&mut self, gi: usize, fi: usize) {
        if let Some(slot) = self.0.get_mut(gi) {
            *slot = Some(fi);
        }
    }

    fn is(&self, gi: usize, fi: usize) -> bool {
        self.0.get(gi) == Some(&Some(fi))
    }
}

fn note(r: &mut Repairs, bit: u32, g: &Group, e: usize, field: &str) {
    r.mask |= bit;
    r.count += 1; // at most one repair per field and item: never near u16::MAX
    if r.first.is_empty() {
        let mut buf = [0u8; 40];
        let n = write_path(&mut buf, g, e, field);
        copy_string(&mut r.first, buf.get(..n).unwrap_or_default());
    }
}

/// Every field outside its per-field rule gets its default.
fn repair_fields(c: &mut Config, r: &mut Repairs, raw: &RawBad) {
    if c.schema != CONFIG_JSON_SCHEMA {
        c.schema = CONFIG_JSON_SCHEMA;
        note(r, REPAIR_FIELD, &ROOT_HEAD, 0, "schema");
    }
    for (gi, &g) in GROUPS.iter().enumerate() {
        for e in 0..element_count(g) {
            for (fi, f) in g.fields.iter().enumerate() {
                if f.retired {
                    continue;
                }
                if !raw.is(gi, fi) && (g.get)(c, e, fi).is_some_and(|v| field_valid(f, v)) {
                    continue;
                }
                (g.reset)(c, e, fi);
                note(r, REPAIR_FIELD, g, e, f.name);
            }
        }
    }
}

fn repair_network(c: &mut Config, r: &mut Repairs) {
    let n = &mut c.net;
    if !n.dhcp && (n.ip == 0 || n.mask == 0 || n.gateway == 0) {
        n.dhcp = true; // an incomplete static setup would leave the device unreachable
        note(r, REPAIR_STATIC_IP, &NET, 0, "dhcp");
    }
    let pwd_len = n.wifi_password.len();
    if !n.ssid.is_empty() && pwd_len > 0 && pwd_len < 8 {
        // "" = open network
        n.ssid.clear();
        n.wifi_password.clear();
        note(r, REPAIR_WIFI_PASSWORD, &NET, 0, "wifiPassword");
    }
    if n.iface == NetInterface::Wifi && n.ssid.is_empty() {
        n.iface = NetInterface::Auto;
        note(r, REPAIR_WIFI_IFACE, &NET, 0, "iface");
    }
    if c.syslog.level > 0 && c.syslog.server == 0 {
        c.syslog.level = 0;
        note(r, REPAIR_SYSLOG, &SYSLOG, 0, "level");
    }
}

fn repair_mqtt(c: &mut Config, r: &mut Repairs) {
    let m = &mut c.mqtt;
    if m.mode != MqttMode::Off && m.host.is_empty() {
        m.mode = MqttMode::Off;
        note(r, REPAIR_MQTT_HOST, &MQTT, 0, "mode");
    }
    if m.min_delay_s > m.publish_interval_s {
        m.min_delay_s = m.publish_interval_s;
        note(r, REPAIR_MIN_DELAY, &MQTT, 0, "minDelayS");
    }
    if m.mode == MqttMode::MqttHa && !m.separate {
        m.mode = MqttMode::Mqtt;
        note(r, REPAIR_HA_SEPARATE, &MQTT, 0, "mode");
    }
    if m.mode == MqttMode::MqttHa && m.german_decimal {
        m.german_decimal = false; // V1: Home Assistant reads a decimal point
        note(r, REPAIR_HA_DECIMAL, &MQTT, 0, "germanDecimal");
    }
}

/// Two items with one key: the later item's override is cleared, else its name; an unnamed later
/// item uses its number, so then the earlier item's override or name goes. V2 names the cleared
/// string in the bit, V3 not.
fn resolve(
    c: &mut Config,
    r: &mut Repairs,
    kind: ItemKind,
    later: usize,
    earlier: usize,
    ha_id: bool,
) {
    let later_set = item_texts(c, kind, later)
        .is_some_and(|(name, topic)| !topic.is_empty() || !name.is_empty());
    let k = if later_set { later } else { earlier };
    let Some((name, topic)) = item_texts_mut(c, kind, k) else {
        return;
    };
    let is_topic = !topic.is_empty();
    if is_topic {
        topic.clear();
    } else {
        name.clear();
    }
    if kind == ItemKind::Valve {
        let bits = if is_topic {
            &mut r.valve_topics
        } else {
            &mut r.valve_names
        };
        *bits += 1 << k; // a string is cleared once: its bit is not set yet
    }
    let bit = if ha_id {
        REPAIR_HA_IDS
    } else if is_topic {
        REPAIR_TOPICS
    } else {
        REPAIR_VALVE_NAMES
    };
    note(
        r,
        bit,
        item_group(kind),
        k,
        if is_topic { "topic" } else { "name" },
    );
}

/// One valve repair; false when the valves are valid. Clearing a name can make a new clash (a
/// name equal to the number of a now unnamed valve), so the caller repeats until stable.
fn valve_step(c: &mut Config, r: &mut Repairs) -> bool {
    for i in 0..c.valves.len() {
        if valve_name_clash(c, i) {
            if let Some(v) = c.valves.get_mut(i) {
                v.name.clear();
            }
            r.valve_names += 1 << i; // a name is cleared once: its bit is not set yet
            note(r, REPAIR_VALVE_NAMES, &VALVES, i, "name");
            return true;
        }
    }
    // V2: one MQTT segment per valve.
    if let Some((later, earlier)) = duplicate_item(c, ItemKind::Valve, false) {
        resolve(c, r, ItemKind::Valve, later, earlier, false);
        return true;
    }
    ha_id_step(c, r, ItemKind::Valve)
}

/// V3: one HA id per valve and per active slot.
fn ha_id_step(c: &mut Config, r: &mut Repairs, kind: ItemKind) -> bool {
    let Some((later, earlier)) = duplicate_item(c, kind, true) else {
        return false;
    };
    resolve(c, r, kind, later, earlier, true);
    true
}

/// Duplicate ids: the later id is cleared; an active slot without id is switched off. Returns
/// the bits of the cleared ids and of the switched off slots.
fn repair_slots<T: SensorSlot>(slots: &mut [T], g: &Group, r: &mut Repairs) -> (u64, u64) {
    let mut ids = 0;
    let mut active = 0;
    for i in 0..slots.len() {
        let id = slots
            .get(i)
            .map_or(OneWireId::default(), |s| s.id_active().0);
        let dup = !is_zero(&id) && slots.iter().take(i).any(|s| s.id_active().0 == id);
        let Some(s) = slots.get_mut(i) else {
            continue;
        };
        let (id, on) = s.id_active_mut();
        if dup {
            *id = OneWireId::default();
            ids += 1u64 << i;
            note(r, REPAIR_SLOT_IDS, g, i, "id");
        }
        if *on && is_zero(id) {
            *on = false;
            active += 1u64 << i;
            note(r, REPAIR_SLOT_ACTIVE, g, i, "active");
        }
    }
    (ids, active)
}

/// Makes a config valid step by step and records every step in `r`.
fn repair(c: &mut Config, r: &mut Repairs, raw: &RawBad) {
    repair_fields(c, r, raw);
    repair_network(c, r);
    repair_mqtt(c, r);
    while valve_step(c, r) {}
    (r.temp_ids, r.temp_active) = repair_slots(&mut c.temps, &TEMPS, r);
    let (ids, active) = repair_slots(&mut c.volts, &VOLTS, r);
    (r.volt_ids, r.volt_active) = (ids as u8, active as u8);
    while ha_id_step(c, r, ItemKind::Temp) {}
    while ha_id_step(c, r, ItemKind::Volt) {}
}

/// Resets every field that fails its per-field rule to its default, then applies the cross-field
/// repairs in validate_config order. Postcondition: validate_config(c) is Ok. `out` (optional)
/// receives the details. Returns the repair mask.
pub fn sanitize_config(c: &mut Config, out: Option<&mut Repairs>) -> u32 {
    let mut local = Repairs::default();
    let r = out.unwrap_or(&mut local);
    *r = Repairs::default();
    repair(c, r, &RawBad::default());
    r.mask
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeResult {
    Ok = 0,
    TooShort = 1,
    BadMagic = 2,
    BadCrc = 3,
    UnsupportedSchema = 4,
    Invalid = 5,
}

impl DecodeResult {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Ok),
            1 => Some(Self::TooShort),
            2 => Some(Self::BadMagic),
            3 => Some(Self::BadCrc),
            4 => Some(Self::UnsupportedSchema),
            5 => Some(Self::Invalid),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DecodeInfo {
    /// header schema
    pub schema: u16,
    /// schema > CONFIG_BASE_SCHEMA: its known prefix was decoded
    pub newer_schema: bool,
    /// sanitize_config() result
    pub repairs: Repairs,
}

fn le32(p: &[u8]) -> u32 {
    p.get(..4)
        .and_then(|b| b.try_into().ok())
        .map_or(0, u32::from_le_bytes)
}

/// Decodes a `cfg` blob into `out` (defaults first, then the fields), then makes it valid with
/// the repair of [`sanitize_config`] (`info.repairs`):
///  - schema 0 -> UnsupportedSchema, defaults;
///  - schema 1 -> the payload must end exactly after the last field;
///  - schema > 1 (a newer firmware) -> the schema-1 fields are read from the payload start, the
///    rest is ignored; newer_schema; Ok;
///  - structural damage (bool byte > 1, string length >= its array size, NUL inside a string,
///    payload too short) -> Invalid, defaults;
///  - value damage -> Ok with repairs (station "" -> "VdMot", an enum byte out of range -> its
///    default, ...).
pub fn decode_config(data: &[u8], out: &mut Config, info: Option<&mut DecodeInfo>) -> DecodeResult {
    set_defaults(out);
    let mut local = DecodeInfo::default();
    let di = info.unwrap_or(&mut local);
    *di = DecodeInfo::default();
    let (Some(head), Some(body)) = (data.get(..HEADER_SIZE), data.get(HEADER_SIZE..)) else {
        return DecodeResult::TooShort;
    };
    if body.len() < CRC_SIZE {
        return DecodeResult::TooShort;
    }
    if head.get(..4) != Some(MAGIC.as_slice()) {
        return DecodeResult::BadMagic;
    }
    di.schema = u16::from(head[4]) + (u16::from(head[5]) << 8);
    let payload = usize::from(head[6]) + (usize::from(head[7]) << 8);
    if body.len() < payload + CRC_SIZE {
        return DecodeResult::TooShort;
    }
    if body.len() > payload + CRC_SIZE {
        return DecodeResult::Invalid;
    }
    let crc_at = HEADER_SIZE + payload;
    if crc32(data.get(..crc_at).unwrap_or_default(), 0)
        != le32(data.get(crc_at..).unwrap_or_default())
    {
        return DecodeResult::BadCrc;
    }
    // Schema 1 is the first one; older blobs do not exist.
    if di.schema == 0 {
        return DecodeResult::UnsupportedSchema;
    }

    // Decoded in place; structural damage puts the defaults back. A newer schema starts with the
    // schema-1 fields: that prefix is read, the rest is left alone.
    let mut inp = ByteIn::new(body.get(..payload).unwrap_or_default());
    let mut raw = RawBad::default();
    for (gi, &g) in GROUPS.iter().enumerate() {
        for e in 0..element_count(g) {
            for (fi, f) in g.fields.iter().enumerate() {
                if f.ext != 0 || !inp.ok {
                    continue;
                }
                // A retired field is read by its old rules and dropped.
                let v = decode_val(&mut inp, f);
                if !f.retired && !(g.set)(out, e, fi, v) {
                    raw.mark(gi, fi);
                }
            }
        }
    }
    let newer = di.schema > CONFIG_BASE_SCHEMA;
    if !inp.ok || (!newer && !inp.at_end()) {
        set_defaults(out);
        return DecodeResult::Invalid;
    }
    di.newer_schema = newer;
    // Values are repaired field by field; a stored config is never dropped because a rule got
    // stricter.
    repair(out, &mut di.repairs, &raw);
    DecodeResult::Ok
}

/// NVS `cfgx` blob (the keys added after 2.0.0): magic "VDMX", u8 format version 1 (a higher one
/// is read like 1), u16 payload length L, records {u8 tag, u8 element (0-based array index, 0 for
/// objects), u8 len, len bytes} (L bytes), u32 CRC-32 over bytes 0..7+L; bytes after the CRC are
/// ignored. Values: PctHold 1 byte, OffOrRange u16, strings without NUL. The encoder writes every
/// ext field in table order (strings also when empty), so a missing record means "default", and
/// appends `keep` (unknown records read from the stored cfgx) verbatim after its own records, so
/// a save does not destroy keys of a newer firmware. Returns the bytes written, 0 when `out` is
/// too small.
pub fn encode_config_ext(c: &Config, out: &mut [u8], keep: &[u8]) -> usize {
    let mut bo = ByteOut::new(out);
    bo.bytes(EXT_MAGIC);
    bo.u8(EXT_VERSION);
    bo.u16(0); // payload length, patched by finish()
    for &g in &GROUPS {
        for e in 0..element_count(g) {
            for (fi, f) in g.fields.iter().enumerate() {
                if f.ext == 0 {
                    continue;
                }
                let Some(v) = (g.get)(c, e, fi) else {
                    continue;
                };
                bo.u8(f.ext);
                bo.u8(e as u8);
                // Strings carry their length byte already.
                if f.kind != Kind::Str {
                    bo.u8(if f.kind == Kind::PctHold { 1 } else { 2 });
                }
                encode_val(&mut bo, f, v);
            }
        }
    }
    bo.bytes(keep);
    bo.finish(EXT_HEADER_SIZE, 5)
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExtResult {
    #[default]
    Absent = 0,
    Ok = 1,
    TooShort = 2,
    BadMagic = 3,
    BadCrc = 4,
}

impl ExtResult {
    pub fn from_raw(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::Absent),
            1 => Some(Self::Ok),
            2 => Some(Self::TooShort),
            3 => Some(Self::BadMagic),
            4 => Some(Self::BadCrc),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtInfo {
    pub applied: u16,
    pub unknown: u16,
    pub bad: u16,
    /// bytes of unknown records copied to `keep`
    pub keep_len: usize,
}

/// The `cfgx` field with this tag: its group, index and table entry.
fn find_ext(tag: u8) -> Option<(&'static Group, usize, &'static Field)> {
    GROUPS.iter().find_map(|&g| {
        g.fields
            .iter()
            .enumerate()
            .find(|(_, f)| f.ext == tag)
            .map(|(fi, f)| (g, fi, f))
    })
}

/// One record's value into its field, when it passes the field rule.
fn apply_ext(c: &mut Config, g: &Group, fi: usize, f: &Field, element: u8, v: &[u8]) -> bool {
    if usize::from(element) >= element_count(g) {
        return false;
    }
    let val = match (f.kind, v) {
        (Kind::Str, _) if !v.contains(&0) => Val::Text(v),
        (Kind::PctHold, &[b]) => Val::Int(i64::from(b)),
        (Kind::OffOrRange, &[lo, hi]) => Val::Int(i64::from(u16::from_le_bytes([lo, hi]))),
        _ => return false,
    };
    if !field_valid(f, val) {
        return false;
    }
    (g.set)(c, usize::from(element), fi, val);
    true
}

/// Applies the records of a `cfgx` blob to `inout` (defaults stay for missing ones). Unknown tag
/// -> unknown + 1 and the record is copied to `keep` while it fits (at most
/// CONFIG_EXT_KEEP_MAX bytes); known tag with element >= count, a wrong length or a value that
/// fails its field rule -> bad + 1, field unchanged; a later record for the same field wins. An
/// empty `data` -> Absent. Damaged header or CRC -> nothing applied. `info` (optional) receives
/// the counts.
pub fn decode_config_ext(
    data: &[u8],
    inout: &mut Config,
    info: Option<&mut ExtInfo>,
    keep: &mut [u8],
) -> ExtResult {
    let mut local = ExtInfo::default();
    let r = info.unwrap_or(&mut local);
    *r = ExtInfo::default();
    if data.is_empty() {
        return ExtResult::Absent;
    }
    if data.len() < EXT_HEADER_SIZE + CRC_SIZE {
        return ExtResult::TooShort;
    }
    if data.get(..4) != Some(EXT_MAGIC.as_slice()) {
        return ExtResult::BadMagic;
    }
    let end = EXT_HEADER_SIZE + usize::from(data[5]) + (usize::from(data[6]) << 8);
    if data.len() < end + CRC_SIZE {
        return ExtResult::TooShort;
    }
    if crc32(data.get(..end).unwrap_or_default(), 0) != le32(data.get(end..).unwrap_or_default()) {
        return ExtResult::BadCrc;
    }
    let keep_max = keep.len().min(CONFIG_EXT_KEEP_MAX);
    let mut records = data.get(EXT_HEADER_SIZE..end).unwrap_or_default();
    while !records.is_empty() {
        // A record cut by the payload end (only a broken writer does that).
        let Some(&[tag, element, n]) = records.get(..EXT_RECORD_HEAD) else {
            r.bad += 1;
            break;
        };
        let rec_len = EXT_RECORD_HEAD + usize::from(n);
        let Some(record) = records.get(..rec_len) else {
            r.bad += 1;
            break;
        };
        let value = record.get(EXT_RECORD_HEAD..).unwrap_or_default();
        match find_ext(tag) {
            None => {
                r.unknown += 1;
                if rec_len <= keep_max - r.keep_len {
                    if let Some(dst) = keep.get_mut(r.keep_len..r.keep_len + rec_len) {
                        dst.copy_from_slice(record);
                    }
                    r.keep_len += rec_len;
                }
            }
            Some((g, fi, f)) => {
                if apply_ext(inout, g, fi, f, element, value) {
                    r.applied += 1;
                } else {
                    r.bad += 1;
                }
            }
        }
        records = records.get(rec_len..).unwrap_or_default();
    }
    ExtResult::Ok
}

/// The stored blobs of a config (an empty slice for a missing one).
#[derive(Clone, Copy, Debug, Default)]
pub struct StoredBlobs<'a> {
    pub base: &'a [u8],
    pub ext: &'a [u8],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadInfo {
    pub base: DecodeResult,
    pub ext: ExtResult,
    /// base decode incl. its repairs
    pub decode: DecodeInfo,
    pub ext_info: ExtInfo,
    /// final sanitize_config() after the ext records
    pub repairs: Repairs,
}

impl Default for LoadInfo {
    fn default() -> Self {
        Self {
            base: DecodeResult::TooShort,
            ext: ExtResult::Absent,
            decode: DecodeInfo::default(),
            ext_info: ExtInfo::default(),
            repairs: Repairs::default(),
        }
    }
}

/// The base blob must be usable (decode_config Ok); then the ext records (unknown ones go to
/// `keep`); then sanitize_config. True when `out` holds a usable config (never false because of
/// the ext blob).
pub fn load_config_blobs(
    b: &StoredBlobs<'_>,
    out: &mut Config,
    info: &mut LoadInfo,
    keep: &mut [u8],
) -> bool {
    *info = LoadInfo::default();
    info.base = decode_config(b.base, out, Some(&mut info.decode));
    if info.base != DecodeResult::Ok {
        return false;
    }
    info.ext = decode_config_ext(b.ext, out, Some(&mut info.ext_info), keep);
    // The ext records can break a cross-field rule (an override equal to another valve's
    // segment).
    sanitize_config(out, Some(&mut info.repairs));
    true
}

/// CRC-32 (IEEE 802.3, reflected, init/xorout 0xFFFFFFFF; zlib crc32() semantics: pass the
/// previous result as `crc` to continue over more data, 0 to start). Also used by the flasher.
pub fn crc32(data: &[u8], crc: u32) -> u32 {
    // Nibble table: 64 bytes of flash, fast enough for 512 KiB images.
    const TABLE: [u32; 16] = [
        0x0000_0000,
        0x1DB7_1064,
        0x3B6E_20C8,
        0x26D9_30AC,
        0x76DC_4190,
        0x6B6B_51F4,
        0x4DB2_6158,
        0x5005_713C,
        0xEDB8_8320,
        0xF00F_9344,
        0xD6D6_A3E8,
        0xCB61_B38C,
        0x9B64_C2B0,
        0x86D3_D2D4,
        0xA00A_E278,
        0xBDBD_F21C,
    ];
    let nibble = |crc: u32| (crc >> 4) ^ TABLE[(crc & 0x0F) as usize];
    let mut crc = !crc;
    for &b in data {
        crc = nibble(nibble(crc ^ u32::from(b)));
    }
    !crc
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_repairs;
