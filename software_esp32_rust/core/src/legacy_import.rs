//! One-shot import of the legacy (1.4.x) NVS configuration into the new Config (DESIGN.md
//! "Legacy import"; port of `vdm/legacy_import.h`). Hardware-free: the glue implements
//! [`LegacyNvsReader`] on top of NVS (read-only), tests use a map. The import never writes or
//! deletes legacy keys, so it is idempotent and a downgrade to the legacy firmware still finds
//! its config.

use crate::common::{
    c_str, copy_string, fmt_trunc, format_ipv4, is_printable_text, Text, TextBuf, TextView,
    TEMP_SLOT_COUNT, VALVE_COUNT, VOLT_SLOT_COUNT,
};
use crate::config::{
    sanitize_config, set_config_value, set_defaults, Config, ConfigValue, ItemKind, Repairs,
    SetResult, REPAIR_HA_DECIMAL, REPAIR_HA_SEPARATE, REPAIR_MIN_DELAY, REPAIR_MQTT_HOST,
    REPAIR_STATIC_IP, REPAIR_SYSLOG, REPAIR_WIFI_IFACE, REPAIR_WIFI_PASSWORD, SECRET_MAX,
};
use crate::json_writer::JsonWriter;

/// Read-only access to one legacy NVS key. Namespaces: "sysCfg", "netCfg", "tZCfg", "protCfg",
/// "valvesCfg", "tempsCfg", "voltsCfg", "Misc"; the dropped keys (the web login of "netCfg",
/// "valvesCtrlCfg", "msgCfg", "motorCfg", ...) are counted in [`ImportReport::ignored`]
/// (valvesCtrl and msgFlags are also read for the report). Every method returns None when the
/// namespace or key does not exist or the stored type differs; outputs are untouched then.
pub trait LegacyNvsReader {
    /// Integer keys of any width (UChar, UShort, ULong, Long, Short). Signed types are
    /// sign-extended, unsigned zero-extended.
    fn read_int(&mut self, ns: &str, key: &str) -> Option<i64>;
    /// String keys: the stored text into `out`, at most its capacity (the C++ cap - 1); the
    /// result is true when the stored string was longer (the importer then rejects the key).
    fn read_string(&mut self, ns: &str, key: &str, out: &mut TextView) -> Option<bool>;
    /// Blob keys: copies min(stored, out.len()) bytes and returns the stored length, so the
    /// importer can reject unexpected sizes.
    fn read_blob(&mut self, ns: &str, key: &str, out: &mut [u8]) -> Option<usize>;
}

// Legacy blob sizes (Xtensa GCC layout, DESIGN.md "Legacy import").
/// 12 x {char name[11]; bool active;}
pub const LEGACY_VALVES_BLOB: usize = 144;
/// 34 x {name[11], active, int offset@12, ID[25]@16, pad}
pub const LEGACY_TEMPS_BLOB: usize = 1496;
/// 8 x 56 {name, active, float offset@12, float factor@16, unit[9]@20, ID[25]@29} + 8 x 4
pub const LEGACY_VOLTS_BLOB: usize = 480;
/// 1.4.0: the same 8 x 56 elements without the 8 x 4 tail
pub const LEGACY_VOLTS_BLOB_140: usize = 448;
/// valvesCtrl: 12 elements of at most 64 bytes, byte 0 = controlFlags
pub const LEGACY_VALVES_CTRL_MAX: usize = 768;

// ImportReport::dropped: legacy features the new firmware does not have and that were in use
// (event import_dropped, report "dropped").
/// PI control active on a valve
pub const DROPPED_PI: u8 = 0x01;
/// window contact installed on a valve
pub const DROPPED_WINDOW: u8 = 0x02;
/// PushOver or e-mail messages
pub const DROPPED_MESSENGER: u8 = 0x04;
/// brokerMQF bit1: DS18 value timeout
pub const DROPPED_DS18_TIMEOUT: u8 = 0x08;
/// brokerMQF bit0: tValue timeout failsafe
pub const DROPPED_LEGACY_FAILSAFE: u8 = 0x10;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// at least one legacy namespace had a key
    pub any_legacy: bool,
    /// keys applied (a blob counts once)
    pub imported: u16,
    /// keys/blob fields present but invalid, plus repairs
    pub rejected: u16,
    /// DROP keys present (PI, messenger, ...), not imported
    pub ignored: u16,
    /// "<ns>/<key>" of the first rejected key; blob fields as "<ns>/<key>.<n>.<field>" (n
    /// 1-based, e.g. "tempsCfg/temps.3.id"); C++ char[40]
    pub first_rejected: Text<39>,
    /// Misc/MiscLC when valid (>= 2020-01-01), else 0
    pub last_calib_epoch: i64,
    /// valves with PI control (valvesCtrl)
    pub pi_valves: u8,
    /// valves with a window contact (valvesCtrl)
    pub window_valves: u8,
    /// DROPPED_* bits
    pub dropped: u8,
    /// bit i: item i got a new name (characters MQTT cannot carry replaced, or cleared as a
    /// duplicate); the config holds the result.
    pub renamed_valves: u16,
    pub renamed_temps: u64,
    pub renamed_volts: u8,
    // Legacy failsafe keys (not mapped: the new config keeps 60 min / 50 %).
    /// brokerMQTO or brokerMQToPos stored
    pub legacy_failsafe_valid: bool,
    /// brokerMQF bit0
    pub legacy_failsafe_enabled: bool,
    pub legacy_failsafe_timeout_min: i32,
    pub legacy_failsafe_pct: i32,
    /// syslogEnable 1..3 (debug verbosity) imported as level 3
    pub syslog_debug: bool,
    /// the volts blob of 1.4.0 (448 bytes)
    pub volts_blob_448: bool,
}

/// 2020-01-01T00:00:00Z
const EPOCH_2020: i64 = 1_577_836_800;
/// sanity bound for a stored time_t
const EPOCH_2100: i64 = 4_102_444_800;

// Legacy blob element layouts (Xtensa GCC, DESIGN.md "Legacy import").
/// name[11]@0 active@11
const VALVE_ELEM: usize = 12;
/// name[11]@0 active@11 int offset@12 ID[25]@16
const TEMP_ELEM: usize = 44;
/// name[11]@0 active@11 float offset@12 float factor@16 unit[9]@20 ID[25]@29
const VOLT_ELEM: usize = 56;
const LEGACY_NAME_LEN: usize = 11;
const LEGACY_UNIT_LEN: usize = 9;
const LEGACY_ID_LEN: usize = 25;

/// Largest legacy string (64 chars) plus one, so an over-long value is read completely and
/// rejected by the field rule instead of being cut (the C++ buffer of 66 bytes with its NUL).
const STR_MAX: usize = SECRET_MAX + 1;
type LegacyStr = Text<STR_MAX>;

/// Keys the new firmware drops (DESIGN.md "Legacy import"). They are only counted, never read
/// into the config. Types differ per key, so every accessor is tried.
const DROPPED: [(&str, &str); 26] = [
    ("sysCfg", "CF"),
    ("netCfg", "userName"), // the web login (removed in 2.1.0)
    ("netCfg", "userPwd"),
    ("protCfg", "brokerInterval"),
    ("protCfg", "brokerMQTO"),
    ("protCfg", "brokerMQToPos"),
    ("valvesCfg", "movCalib"),
    ("valvesCtrlCfg", "valvesCtrl"),
    ("valvesCtrlCfg", "valvesCtrl1"),
    ("valvesCtrlCfg", "vCtrlInit"),
    ("valvesCtrlCfg", "vCtrlHeat"),
    ("valvesCtrlCfg", "vCtrlParkPos"),
    ("msgCfg", "msgFlags"),
    ("msgCfg", "msgReas"),
    ("msgCfg", "msgMQTO"),
    ("msgCfg", "POAppTk"),
    ("msgCfg", "POUserTk"),
    ("msgCfg", "POTitle"),
    ("msgCfg", "EUser"),
    ("msgCfg", "EPwd"),
    ("msgCfg", "EHost"),
    ("msgCfg", "EPort"),
    ("msgCfg", "ERep"),
    ("msgCfg", "ETitle"),
    ("motorCfg", "motorMinC"),
    ("motorCfg", "motorMaxC"),
];

/// A key path of the setter ("valves.3.name"); C++ char[24].
type Path = [u8; 24];

/// Legacy char[n] field: the text up to the first NUL; None when the field has no NUL (strncpy
/// of a too-long value).
fn fixed_string(p: &[u8]) -> Option<&[u8]> {
    let nul = p.iter().position(|&b| b == 0)?;
    p.get(..nul)
}

fn load_i32(p: &[u8]) -> i32 {
    p.get(..4)
        .and_then(|b| b.try_into().ok())
        .map_or(0, i32::from_le_bytes)
}

fn load_f32(p: &[u8]) -> f32 {
    f32::from_bits(load_i32(p) as u32)
}

/// Bit i: item i has a non-empty name (`topic` false) or topic override. The repairs only clear
/// them.
fn set_bits(c: &Config, kind: ItemKind, topic: bool) -> u64 {
    let texts: &mut dyn Iterator<Item = (&[u8], &[u8])> = match kind {
        ItemKind::Valve => &mut c
            .valves
            .iter()
            .map(|v| (v.name.as_slice(), v.topic.as_slice())),
        ItemKind::Temp => &mut c
            .temps
            .iter()
            .map(|t| (t.name.as_slice(), t.topic.as_slice())),
        ItemKind::Volt => &mut c
            .volts
            .iter()
            .map(|v| (v.name.as_slice(), v.topic.as_slice())),
    };
    let mut bits = 0;
    for (i, (name, top)) in texts.enumerate() {
        let text = if topic { top } else { name };
        if !text.is_empty() {
            bits += 1u64 << i;
        }
    }
    bits
}

struct Importer<'a> {
    nvs: &'a mut dyn LegacyNvsReader,
    c: &'a mut Config,
    r: ImportReport,
    scratch: &'a mut [u8],
    /// the item repair reported first (Repairs::first), skipped in the item list
    lead: Text<39>,
}

impl Importer<'_> {
    fn run(&mut self) {
        self.import_sys();
        self.import_net();
        self.import_tz();
        self.import_prot();
        self.import_valves();
        self.import_temps();
        self.import_volts();
        self.import_misc();
        self.import_valves_ctrl();
        self.import_messenger();
        self.count_dropped();
        self.repair();
    }

    // ------------------------------------------------------------ reading

    fn read_int(&mut self, ns: &str, key: &str) -> Option<i64> {
        let v = self.nvs.read_int(ns, key)?;
        self.r.any_legacy = true;
        Some(v)
    }

    /// Missing -> None. Present but longer than the read buffer -> rejected, None.
    fn read_string(&mut self, ns: &str, key: &str, out: &mut LegacyStr) -> Option<()> {
        let truncated = self.nvs.read_string(ns, key, out)?;
        self.r.any_legacy = true;
        if truncated {
            self.rejected(ns, key.as_bytes());
            return None;
        }
        Some(())
    }

    /// Missing -> false. Present with another size -> rejected, false.
    fn read_blob(&mut self, ns: &str, key: &str, out: &mut [u8]) -> bool {
        let Some(stored) = self.nvs.read_blob(ns, key, out) else {
            return false;
        };
        self.r.any_legacy = true;
        if stored != out.len() {
            self.rejected(ns, key.as_bytes());
            return false;
        }
        true
    }

    // ------------------------------------------------------------ report

    fn imported(&mut self) {
        self.r.imported += 1;
    }

    /// "<ns>/<key>", cut to fit the report field.
    fn rejected(&mut self, ns: &str, key: &[u8]) {
        if self.r.rejected == 0 {
            let mut buf = [0u8; 40];
            let mut w = TextBuf::new(&mut buf);
            w.push_bytes(ns.as_bytes());
            w.push(b'/');
            w.push_bytes(c_str(key));
            let n = w.len();
            copy_string(&mut self.r.first_rejected, buf.get(..n).unwrap_or_default());
        }
        self.r.rejected += 1;
    }

    fn rejected_elem(&mut self, ns: &str, key: &str, index: usize, field: &str) {
        let mut k = [0u8; 32];
        let n = fmt_trunc(&mut k, format_args!("{key}.{}.{field}", index + 1));
        self.rejected(ns, k.get(..n).unwrap_or_default());
    }

    // ------------------------------------------------------------ setters

    fn set(&mut self, path: &[u8], v: ConfigValue<'_>) -> bool {
        set_config_value(self.c, path, &v, true) == SetResult::Ok
    }

    fn set_int(&mut self, path: &str, v: i64) -> bool {
        self.set(path.as_bytes(), ConfigValue::Int(v))
    }

    fn set_string(&mut self, path: &[u8], s: &[u8]) -> bool {
        self.set(path, ConfigValue::Str(c_str(s)))
    }

    fn set_float(&mut self, path: &[u8], v: f64) -> bool {
        self.set(path, ConfigValue::Float(v))
    }

    /// Legacy u32 IPv4 -> dotted string -> setter (keeps the mask rule).
    fn set_ip(&mut self, path: &str, v: i64) -> bool {
        let Ok(ip) = u32::try_from(v) else {
            return false;
        };
        let mut text = [0u8; 16];
        let n = format_ipv4(ip, &mut text);
        self.set_string(path.as_bytes(), text.get(..n).unwrap_or_default())
    }

    fn int_key(&mut self, ns: &str, key: &str, path: &str) {
        let Some(v) = self.read_int(ns, key) else {
            return;
        };
        if self.set_int(path, v) {
            self.imported();
        } else {
            self.rejected(ns, key.as_bytes());
        }
    }

    fn ip_key(&mut self, ns: &str, key: &str, path: &str) {
        let Some(v) = self.read_int(ns, key) else {
            return;
        };
        if self.set_ip(path, v) {
            self.imported();
        } else {
            self.rejected(ns, key.as_bytes());
        }
    }

    fn string_key(&mut self, ns: &str, key: &str, path: &str) {
        let mut s = LegacyStr::new();
        if self.read_string(ns, key, &mut s).is_none() {
            return;
        }
        if self.set_string(path.as_bytes(), &s) {
            self.imported();
        } else {
            self.rejected(ns, key.as_bytes());
        }
    }

    /// Integer with a legacy "0 means default" quirk.
    fn port_key(&mut self, ns: &str, key: &str, path: &str, zero_means: i64) {
        let Some(v) = self.read_int(ns, key) else {
            return;
        };
        if self.set_int(path, if v == 0 { zero_means } else { v }) {
            self.imported();
        } else {
            self.rejected(ns, key.as_bytes());
        }
    }

    // ------------------------------------------------------------ namespaces

    fn import_sys(&mut self) {
        let mut s = LegacyStr::new();
        if self.read_string("sysCfg", "stName", &mut s).is_none() {
            return;
        }
        if c_str(&s).is_empty() {
            // The legacy firmware published under "VdMotFBH/" without a name.
            self.set_string(b"mqtt.rootTopic", b"VdMotFBH");
            self.imported();
            return;
        }
        if self.set_string(b"station", &s) {
            self.imported();
        } else {
            self.rejected("sysCfg", b"stName");
        }
    }

    fn import_net(&mut self) {
        self.int_key("netCfg", "ethwifi", "net.iface");
        self.int_key("netCfg", "dhcp", "net.dhcp");
        self.ip_key("netCfg", "staticIp", "net.ip");
        self.ip_key("netCfg", "mask", "net.mask");
        self.ip_key("netCfg", "gw", "net.gateway");
        self.ip_key("netCfg", "dnsIp", "net.dns");
        self.string_key("netCfg", "ssid", "net.ssid");
        self.string_key("netCfg", "pwd", "net.wifiPassword");
        self.int_key("netCfg", "netConnTO", "net.reconnectTimeoutMin");
        self.string_key("netCfg", "timeServer", "time.ntpServer");
        if let Some(level) = self.read_int("netCfg", "syslogEnable") {
            // 1..3 were debug verbosity levels: everything is sent.
            self.r.syslog_debug = (1..=3).contains(&level);
            let ok = (level == 0 || self.r.syslog_debug)
                && self.set_int("syslog.level", if level == 0 { 0 } else { 3 });
            if ok {
                self.imported();
            } else {
                self.rejected("netCfg", b"syslogEnable");
            }
        }
        self.ip_key("netCfg", "sysLogIp", "syslog.server");
        self.port_key("netCfg", "sysLogPort", "syslog.port", 514);
    }

    fn import_tz(&mut self) {
        self.string_key("tZCfg", "tZ", "time.tzName");
        self.string_key("tZCfg", "tZCode", "time.tzPosix");
    }

    fn import_prot(&mut self) {
        let data_prot = self.read_int("protCfg", "dataProt");
        if let Some(v) = data_prot {
            if self.set_int("mqtt.mode", v) {
                self.imported();
            } else {
                self.rejected("protCfg", b"dataProt");
            }
        }

        if let Some(v) = self.read_int("protCfg", "brokerIp") {
            let ok = if v == 0 {
                self.set_string(b"mqtt.host", b"")
            } else {
                self.set_ip("mqtt.host", v)
            };
            if ok {
                self.imported();
            } else {
                self.rejected("protCfg", b"brokerIp");
            }
        }
        self.port_key("protCfg", "brokerPort", "mqtt.port", 1883);
        if let Some(v) = self.read_int("protCfg", "publishInterval") {
            // The legacy firmware forced >= 2 s at runtime; clamp instead of reject.
            self.set_int("mqtt.publishIntervalS", v.clamp(2, 3600));
            self.imported();
        }
        self.string_key("protCfg", "brokerUser", "mqtt.user");
        self.string_key("protCfg", "brokerPwd", "mqtt.password");
        self.int_key("protCfg", "brokerKAT", "mqtt.keepAliveS");
        self.int_key("protCfg", "brokerMD", "mqtt.minDelayS");

        let mut pf = self.read_int("protCfg", "brokerPF");
        match pf {
            Some(v) if !(0..=0xFF).contains(&v) => {
                self.rejected("protCfg", b"brokerPF");
                pf = None;
            }
            Some(_) => self.imported(),
            // legacy readConfig() fallback when the key is missing
            None if data_prot.is_some() => pf = Some(7),
            None => {}
        }
        if let Some(pf) = pf {
            let bit = |b: i64| pf & b != 0;
            let m = &mut self.c.mqtt;
            m.separate = bit(0x01);
            m.all_temps = bit(0x02);
            m.path_as_root = bit(0x04);
            m.up_time = bit(0x08);
            m.on_change = bit(0x10);
            m.retained = bit(0x20);
            m.plain_text = bit(0x40);
            m.diag = bit(0x80);
        }

        if let Some(v) = self.read_int("protCfg", "brokerMQF") {
            if !(0..=0xFF).contains(&v) {
                self.rejected("protCfg", b"brokerMQF");
            } else {
                self.c.mqtt.german_decimal = v & 0x04 != 0;
                self.imported();
                if v & 0x03 != 0 {
                    self.r.ignored += 1; // tValue / DS18 failsafes are dropped
                }
                if v & 0x01 != 0 {
                    self.r.dropped |= DROPPED_LEGACY_FAILSAFE;
                }
                if v & 0x02 != 0 {
                    self.r.dropped |= DROPPED_DS18_TIMEOUT;
                }
                self.r.legacy_failsafe_enabled = v & 0x01 != 0;
            }
        }
        // Reported only: the new failsafe keeps its defaults (60 min / 50 %).
        let timeout = self.nvs.read_int("protCfg", "brokerMQTO");
        let pct = self.nvs.read_int("protCfg", "brokerMQToPos");
        self.r.legacy_failsafe_valid = timeout.is_some() || pct.is_some();
        if let Some(t) = timeout {
            self.r.legacy_failsafe_timeout_min = t as i32;
        }
        if let Some(p) = pct {
            self.r.legacy_failsafe_pct = p as i32;
        }
    }

    fn mark_renamed(&mut self, kind: ItemKind, i: usize) {
        match kind {
            ItemKind::Valve => self.r.renamed_valves |= 1 << i,
            ItemKind::Temp => self.r.renamed_temps |= 1 << i,
            ItemKind::Volt => self.r.renamed_volts |= 1 << i,
        }
    }

    /// A printable legacy name with characters MQTT topics cannot carry: they become '_'; a name
    /// with '/', '"' or '\' (and no wildcard) keeps its legacy topic segment as the item's
    /// override. False when even the replaced name is not valid.
    fn rename_item(&mut self, group: &str, kind: ItemKind, i: usize, name: &[u8]) -> bool {
        if !is_printable_text(name) {
            return false;
        }
        let mut safe = [0u8; LEGACY_NAME_LEN];
        let mut topic = [0u8; LEGACY_NAME_LEN];
        let mut separator = false;
        for ((&ch, s), t) in name.iter().zip(&mut safe).zip(&mut topic) {
            let w = ch == b'+' || ch == b'#';
            let sep = ch == b'/' || ch == b'"' || ch == b'\\';
            separator = separator || sep;
            *s = if w || sep { b'_' } else { ch };
            *t = if ch == b' ' { b'_' } else { ch };
        }
        let mut path: Path = [0; 24];
        let n = fmt_trunc(&mut path, format_args!("{group}.{}.name", i + 1));
        if !self.set_string(path.get(..n).unwrap_or_default(), &safe) {
            return false;
        }
        self.mark_renamed(kind, i);
        if separator {
            let n = fmt_trunc(&mut path, format_args!("{group}.{}.topic", i + 1));
            // Not a valid segment ("/Bad"): no override. The C++ skips names with a wildcard
            // ('+', '#') here; the topic rule rejects those too, so the result is the same.
            self.set_string(path.get(..n).unwrap_or_default(), &topic);
        }
        true
    }

    /// Common part of every blob element: name[11]@0, active@11.
    fn elem_name_active(
        &mut self,
        ns: &str,
        key: &str,
        group: &str,
        kind: ItemKind,
        i: usize,
        e: &[u8],
    ) {
        let mut path: Path = [0; 24];
        let n = fmt_trunc(&mut path, format_args!("{group}.{}.name", i + 1));
        let name_ok = match fixed_string(e.get(..LEGACY_NAME_LEN).unwrap_or_default()) {
            Some(name) => {
                self.set_string(path.get(..n).unwrap_or_default(), name)
                    || self.rename_item(group, kind, i, name)
            }
            None => false,
        };
        if !name_ok {
            self.rejected_elem(ns, key, i, "name");
        }
        let n = fmt_trunc(&mut path, format_args!("{group}.{}.active", i + 1));
        let active = e.get(11).copied().unwrap_or_default();
        if !self.set(
            path.get(..n).unwrap_or_default(),
            ConfigValue::Int(i64::from(active)),
        ) {
            self.rejected_elem(ns, key, i, "active");
        }
    }

    /// ID[25]: "" or "00-..-00" = empty slot, else "hh-hh-hh-hh-hh-hh-hh-hh".
    fn elem_id(&mut self, ns: &str, key: &str, group: &str, i: usize, p: &[u8]) {
        let mut path: Path = [0; 24];
        let n = fmt_trunc(&mut path, format_args!("{group}.{}.id", i + 1));
        let ok = fixed_string(p.get(..LEGACY_ID_LEN).unwrap_or_default())
            .is_some_and(|id| self.set_string(path.get(..n).unwrap_or_default(), id));
        if !ok {
            self.rejected_elem(ns, key, i, "id");
        }
    }

    fn import_valves(&mut self) {
        let mut blob = [0u8; LEGACY_VALVES_BLOB];
        if self.read_blob("valvesCfg", "valves", &mut blob) {
            self.imported();
            for (i, e) in blob.chunks(VALVE_ELEM).enumerate() {
                self.elem_name_active("valvesCfg", "valves", "valves", ItemKind::Valve, i, e);
            }
        }
        self.import_calib();
    }

    /// The legacy web UI offered hour 0..24, and the legacy firmware fired when tm_hour ==
    /// hourOfCalib: 24 (or any larger stored value) meant "never". Here that is day mask 0;
    /// dayOfCalib is then irrelevant.
    fn import_calib(&mut self) {
        let hour = self.read_int("valvesCfg", "hourOfCalib");
        if hour.is_some_and(|h| (24..=255).contains(&h)) {
            if self.set_int("calib.dayMask", 0) {
                self.imported();
            } else {
                self.rejected("valvesCfg", b"hourOfCalib");
            }
            if self.read_int("valvesCfg", "dayOfCalib").is_some() {
                self.imported();
            }
            return;
        }
        self.int_key("valvesCfg", "dayOfCalib", "calib.dayMask");
        self.int_key("valvesCfg", "hourOfCalib", "calib.hour");
    }

    fn import_temps(&mut self) {
        // lent by the caller ([`import_legacy_config`]); without room the blob is not read
        let Some(blob) = self.scratch.get_mut(..LEGACY_TEMPS_BLOB) else {
            return;
        };
        let Some(stored) = self.nvs.read_blob("tempsCfg", "temps", blob) else {
            return;
        };
        self.r.any_legacy = true;
        if stored != LEGACY_TEMPS_BLOB {
            self.rejected("tempsCfg", b"temps");
            return;
        }
        self.imported();
        let mut elem = [0u8; TEMP_ELEM];
        for i in 0..usize::from(TEMP_SLOT_COUNT) {
            let at = i * TEMP_ELEM;
            if let Some(e) = self.scratch.get(at..at + TEMP_ELEM) {
                elem.copy_from_slice(e);
            }
            self.elem_name_active("tempsCfg", "temps", "temps", ItemKind::Temp, i, &elem);
            let off = load_i32(&elem[12..]);
            if !(-100..=100).contains(&off) {
                self.rejected_elem("tempsCfg", "temps", i, "offset");
            }
            if let Some(t) = self.c.temps.get_mut(i) {
                // C++: out of range -> off < 0 ? -100 : 100
                t.offset = off.clamp(-100, 100) as i16;
            }
            self.elem_id("tempsCfg", "temps", "temps", i, &elem[16..]);
        }
    }

    fn import_volts(&mut self) {
        let mut blob = [0u8; LEGACY_VOLTS_BLOB];
        let Some(stored) = self.nvs.read_blob("voltsCfg", "volts", &mut blob) else {
            return;
        };
        self.r.any_legacy = true;
        // 1.4.0 wrote the 8 elements without the tail of 1.4.1.
        if stored != LEGACY_VOLTS_BLOB && stored != LEGACY_VOLTS_BLOB_140 {
            self.rejected("voltsCfg", b"volts");
            return;
        }
        self.r.volts_blob_448 = stored == LEGACY_VOLTS_BLOB_140;
        self.imported();
        for (i, e) in blob
            .chunks(VOLT_ELEM)
            .take(usize::from(VOLT_SLOT_COUNT))
            .enumerate()
        {
            let mut path: Path = [0; 24];
            self.elem_name_active("voltsCfg", "volts", "volts", ItemKind::Volt, i, e);
            let n = fmt_trunc(&mut path, format_args!("volts.{}.offset", i + 1));
            if !self.set_float(
                path.get(..n).unwrap_or_default(),
                f64::from(load_f32(&e[12..])),
            ) {
                self.rejected_elem("voltsCfg", "volts", i, "offset");
            }
            let n = fmt_trunc(&mut path, format_args!("volts.{}.factor", i + 1));
            if !self.set_float(
                path.get(..n).unwrap_or_default(),
                f64::from(load_f32(&e[16..])),
            ) {
                self.rejected_elem("voltsCfg", "volts", i, "factor");
            }
            let n = fmt_trunc(&mut path, format_args!("volts.{}.unit", i + 1));
            let ok = fixed_string(e.get(20..20 + LEGACY_UNIT_LEN).unwrap_or_default())
                .is_some_and(|unit| self.set_string(path.get(..n).unwrap_or_default(), unit));
            if !ok {
                self.rejected_elem("voltsCfg", "volts", i, "unit");
            }
            self.elem_id("voltsCfg", "volts", "volts", i, &e[29..]);
        }
    }

    fn import_misc(&mut self) {
        let Some(v) = self.read_int("Misc", "MiscLC") else {
            return;
        };
        // The legacy firmware also stored the unsynced clock (1970); drop that.
        if (EPOCH_2020..EPOCH_2100).contains(&v) {
            self.r.last_calib_epoch = v;
            self.imported();
        } else {
            self.rejected("Misc", b"MiscLC");
        }
    }

    /// valvesCtrl (PI control, window contacts): only counted for the report. Every legacy
    /// version starts its element with controlFlags; the element size grew over the versions,
    /// the count is always 12.
    fn import_valves_ctrl(&mut self) {
        let mut blob = [0u8; LEGACY_VALVES_CTRL_MAX];
        let Some(stored) = self.nvs.read_blob("valvesCtrlCfg", "valvesCtrl", &mut blob) else {
            return;
        };
        let count = usize::from(VALVE_COUNT);
        if stored % count != 0 || stored < count || stored > blob.len() {
            self.rejected("valvesCtrlCfg", b"valvesCtrl");
            return;
        }
        for flags in blob.iter().step_by(stored / count).take(count) {
            if flags & 0x01 != 0 {
                self.r.pi_valves += 1;
            }
            if flags & 0x10 != 0 {
                self.r.window_valves += 1;
            }
        }
        if self.r.pi_valves != 0 {
            self.r.dropped |= DROPPED_PI;
        }
        if self.r.window_valves != 0 {
            self.r.dropped |= DROPPED_WINDOW;
        }
    }

    fn import_messenger(&mut self) {
        if self
            .nvs
            .read_int("msgCfg", "msgFlags")
            .is_some_and(|flags| flags & 0x03 != 0)
        {
            self.r.dropped |= DROPPED_MESSENGER; // bit0 PushOver, bit1 e-mail
        }
    }

    fn count_dropped(&mut self) {
        for (ns, key) in DROPPED {
            let mut s = LegacyStr::new();
            let mut b = [0u8; 1];
            if self.nvs.read_int(ns, key).is_some()
                || self.nvs.read_string(ns, key, &mut s).is_some()
                || self.nvs.read_blob(ns, key, &mut b).is_some()
            {
                self.r.any_legacy = true;
                self.r.ignored += 1;
            }
        }
    }

    // ------------------------------------------------------------ cross-field rules

    /// sanitize_config() makes the result valid; every repair is reported under the legacy key
    /// it came from, in the order of the rules. The item repair sanitize_config() made first
    /// (Repairs::first) is reported before the other item repairs, so first_rejected names it;
    /// the rest follow by number.
    fn repair(&mut self) {
        let temp_names = set_bits(self.c, ItemKind::Temp, false);
        let temp_topics = set_bits(self.c, ItemKind::Temp, true);
        let volt_names = set_bits(self.c, ItemKind::Volt, false);
        let volt_topics = set_bits(self.c, ItemKind::Volt, true);
        let mut r = Repairs::default();
        sanitize_config(self.c, Some(&mut r));
        const KEYS: [(u32, &str, &str); 8] = [
            (REPAIR_STATIC_IP, "netCfg", "dhcp"),
            (REPAIR_WIFI_PASSWORD, "netCfg", "pwd"),
            (REPAIR_WIFI_IFACE, "netCfg", "ethwifi"),
            (REPAIR_SYSLOG, "netCfg", "syslogEnable"),
            (REPAIR_MQTT_HOST, "protCfg", "brokerIp"),
            (REPAIR_MIN_DELAY, "protCfg", "brokerMD"),
            (REPAIR_HA_SEPARATE, "protCfg", "dataProt"),
            (REPAIR_HA_DECIMAL, "protCfg", "brokerMQF"),
        ];
        for (bit, ns, key) in KEYS {
            if r.mask & bit != 0 {
                self.rejected(ns, key.as_bytes());
            }
        }
        const ITEM_NS: [(&[u8], &str); 3] = [
            (b"valves.", "valvesCfg"),
            (b"temps.", "tempsCfg"),
            (b"volts.", "voltsCfg"),
        ];
        self.lead.clear();
        for (prefix, ns) in ITEM_NS {
            if !r.first.starts_with(prefix) {
                continue;
            }
            self.rejected(ns, &r.first);
            self.lead = r.first.clone();
        }
        for i in 0..usize::from(VALVE_COUNT) {
            let name = r.valve_names >> i & 1 != 0;
            let topic = r.valve_topics >> i & 1 != 0;
            if name {
                self.item("valvesCfg", "valves", i, "name");
            }
            if topic {
                self.item("valvesCfg", "valves", i, "topic");
            }
            if name || topic {
                self.mark_renamed(ItemKind::Valve, i);
            }
        }
        self.slot_repairs(
            ItemKind::Temp,
            (temp_names, temp_topics),
            (r.temp_ids, r.temp_active),
            "tempsCfg",
            "temps",
        );
        self.slot_repairs(
            ItemKind::Volt,
            (volt_names, volt_topics),
            (u64::from(r.volt_ids), u64::from(r.volt_active)),
            "voltsCfg",
            "volts",
        );
    }

    /// One item repair, unless it is the lead already reported.
    fn item(&mut self, ns: &str, key: &str, i: usize, field: &str) {
        let mut k = [0u8; 32];
        let n = fmt_trunc(&mut k, format_args!("{key}.{}.{field}", i + 1));
        let k = k.get(..n).unwrap_or_default();
        if k != self.lead.as_slice() {
            self.rejected(ns, k);
        }
    }

    /// Ids and active flags per slot, then the names and topic overrides cleared for one HA id.
    fn slot_repairs(
        &mut self,
        kind: ItemKind,
        (names, topics): (u64, u64),
        (ids, active): (u64, u64),
        ns: &str,
        key: &str,
    ) {
        let count = match kind {
            ItemKind::Temp => usize::from(TEMP_SLOT_COUNT),
            _ => usize::from(VOLT_SLOT_COUNT),
        };
        for i in 0..count {
            if ids >> i & 1 != 0 {
                self.item(ns, key, i, "id");
            }
            if active >> i & 1 != 0 {
                self.item(ns, key, i, "active");
            }
        }
        let names2 = set_bits(self.c, kind, false);
        let topics2 = set_bits(self.c, kind, true);
        for i in 0..count {
            let name = (names & !names2) >> i & 1 != 0;
            let topic = (topics & !topics2) >> i & 1 != 0;
            if name {
                self.item(ns, key, i, "name");
            }
            if topic {
                self.item(ns, key, i, "topic");
            }
            if name || topic {
                self.mark_renamed(kind, i);
            }
        }
    }
}

/// Builds `out` from defaults plus every legacy key that validates. Mapping (binding, DESIGN.md
/// "Legacy import"):
///  - sysCfg/stName -> station (is_safe_name; invalid -> default "VdMot"); "" (legacy topics
///    under "VdMotFBH/") -> station "VdMot", mqtt.rootTopic "VdMotFBH", counted as imported
///  - sysCfg/CF -> ignored (°C only)
///  - netCfg/ethwifi, dhcp, staticIp, mask, gw, dnsIp, ssid, pwd, timeServer, sysLogIp,
///    sysLogPort (0 -> 514), netConnTO -> net.*, time.ntpServer, syslog.*; syslogEnable 0 -> 0,
///    1..3 (the legacy debug verbosity) -> 3 and report.syslog_debug, others rejected;
///    userName, userPwd (the web login) -> ignored
///  - tZCfg/tZ, tZCode -> time.tzName, time.tzPosix
///  - protCfg/dataProt, brokerIp (u32 -> dotted host), brokerPort (0 -> 1883), publishInterval
///    (clamped 2..3600), brokerUser, brokerPwd, brokerPF (missing -> 7 like the legacy read
///    fallback, applied only when protCfg/dataProt exists, so a device without legacy MQTT
///    settings keeps the new defaults), brokerKAT, brokerMD, brokerMQF bit2 ->
///    mqtt.germanDecimal; bit0 (tValue failsafe) -> dropped DROPPED_LEGACY_FAILSAFE, bit1 ->
///    DROPPED_DS18_TIMEOUT, one ignored count for either. brokerInterval, brokerMQTO and
///    brokerMQToPos are ignored; MQTO/ToPos and bit0 are copied into report.legacy_failsafe_*.
///    dataProt 2 with publishSeparate 0: imported as mode Mqtt (HA needs separate topics) and
///    reported as rejected "protCfg/dataProt".
///  - valvesCfg/valves (blob 144), dayOfCalib, hourOfCalib (24..255 meant "never" in the legacy
///    firmware -> calib.dayMask 0, both keys imported)
///  - tempsCfg/temps (blob 1496): name, active, offset (clamped to +-10.0 C -> rejected if
///    outside), ID (parse_one_wire_id; "" or all-zero -> empty slot)
///  - voltsCfg/volts (blob 480, or 448 from 1.4.0 -> report.volts_blob_448): name, active,
///    offset, factor, unit, ID
///  - valvesCtrlCfg/valvesCtrl (ignored; 12 elements of L / 12 bytes, 12..768 bytes, byte 0 bit0
///    PI active, bit4 window contact) -> report.pi_valves, window_valves, dropped; other lengths
///    rejected
///  - msgCfg/msgFlags (ignored) bit0 PushOver or bit1 e-mail -> dropped DROPPED_MESSENGER
///  - Misc/MiscLC -> report.last_calib_epoch (2020-01-01 <= t < 2100-01-01; the legacy firmware
///    also stored the unsynced 1970 clock -> rejected)
///
/// Blob names that are printable text but contain '/', '+', '#', '"' or '\' get those replaced
/// by '_' (report.renamed_*); when the original contains '/', '"' or '\' and no '+' or '#', the
/// original with ' ' -> '_' becomes the item's MQTT topic override (the segment the legacy
/// firmware used), when it is a valid segment. Strings longer than their field and blob strings
/// without a NUL inside their char[] (legacy strncpy) are rejected. Per-key validation uses
/// set_config_value itself; a key that fails keeps the default. Afterwards sanitize_config()
/// makes the result valid, each repair counted as rejected under its legacy key: incomplete
/// static IP ("netCfg/dhcp"), a 1..7 char WiFi password ("netCfg/pwd"), WiFi-only without ssid
/// ("netCfg/ethwifi"), syslog without server ("netCfg/syslogEnable"), MQTT without broker
/// ("protCfg/brokerIp"), minDelay above the publish interval ("protCfg/brokerMD"), HA without
/// separate topics ("protCfg/dataProt"), HA with the decimal comma ("protCfg/brokerMQF"),
/// cleared valve names and overrides ("valvesCfg/valves.<n>.name|topic"), sensor ids, active
/// flags and names ("tempsCfg/temps.<n>.id|active|name", volts alike). The result always passes
/// validate_config(). The caller lends `scratch` for the temps blob (LEGACY_TEMPS_BLOB bytes or
/// more; the glue passes the config blob buffer of storage, unused while it imports under the
/// storage lock): 1.5 KB for a function that runs once at boot is too much for a static and for
/// the caller's stack. Without that room the temps blob is not read. The valvesCtrl blob (up to
/// 768 B) is read on the stack.
pub fn import_legacy_config(
    nvs: &mut dyn LegacyNvsReader,
    out: &mut Config,
    scratch: &mut [u8],
) -> ImportReport {
    set_defaults(out);
    let mut imp = Importer {
        nvs,
        c: out,
        r: ImportReport::default(),
        scratch,
        lead: Text::new(),
    };
    imp.run();
    imp.r
}

const DROPPED_NAMES: [&str; 5] = ["pi", "window", "messenger", "ds18Timeout", "legacyFailsafe"];

fn write_renamed(jw: &mut JsonWriter<'_>, bits: u64, c: &Config, kind: ItemKind) {
    let (name, items): (&str, &mut dyn Iterator<Item = (&[u8], &[u8])>) = match kind {
        ItemKind::Valve => (
            "valve",
            &mut c
                .valves
                .iter()
                .map(|v| (v.name.as_slice(), v.topic.as_slice())),
        ),
        ItemKind::Temp => (
            "temp",
            &mut c
                .temps
                .iter()
                .map(|t| (t.name.as_slice(), t.topic.as_slice())),
        ),
        ItemKind::Volt => (
            "volt",
            &mut c
                .volts
                .iter()
                .map(|v| (v.name.as_slice(), v.topic.as_slice())),
        ),
    };
    for (i, (item_name, topic)) in items.enumerate() {
        if bits >> i & 1 == 0 {
            continue;
        }
        jw.begin_object();
        jw.kv("kind", name);
        jw.kv("n", i as u32 + 1);
        jw.kv("name", item_name);
        jw.kv("topic", topic);
        jw.end_object();
    }
}

/// The import report document (/sys/import.json):
/// `{"imported":57,"rejected":2,"ignored":14,"firstRejected":"valvesCfg/valves.5.name",
///  "piValves":3,"windowValves":1,"dropped":["pi","window","messenger","ds18Timeout"],
///  "legacyFailsafe":{"enabled":true,"timeoutMin":120,"pct":10},"rootTopic":"VdMotFBH",
///  "renamed":[{"kind":"valve","n":3,"name":"Bad_WC","topic":"Bad/WC"}],
///  "syslogDebug":true,"voltsBlob448":false}`
/// "legacyFailsafe" and "rootTopic" are null when not set; "renamed" lists valves, temps, volts
/// (kind "valve", "temp", "volt") by the renamed bits with the names and overrides of `c`.
/// Returns `jw.ok()`.
pub fn write_import_report_json(jw: &mut JsonWriter<'_>, r: &ImportReport, c: &Config) -> bool {
    jw.begin_object();
    jw.kv("imported", r.imported);
    jw.kv("rejected", r.rejected);
    jw.kv("ignored", r.ignored);
    jw.kv("firstRejected", &r.first_rejected);
    jw.kv("piValves", r.pi_valves);
    jw.kv("windowValves", r.window_valves);
    jw.key("dropped");
    jw.begin_array();
    for (i, name) in DROPPED_NAMES.iter().enumerate() {
        if r.dropped >> i & 1 != 0 {
            jw.value(*name);
        }
    }
    jw.end_array();
    jw.key("legacyFailsafe");
    if r.legacy_failsafe_valid {
        jw.begin_object();
        jw.kv("enabled", r.legacy_failsafe_enabled);
        jw.kv("timeoutMin", r.legacy_failsafe_timeout_min);
        jw.kv("pct", r.legacy_failsafe_pct);
        jw.end_object();
    } else {
        jw.null_value();
    }
    jw.key("rootTopic");
    if c.mqtt.root_topic.is_empty() {
        jw.null_value();
    } else {
        jw.value(&c.mqtt.root_topic);
    }
    jw.key("renamed");
    jw.begin_array();
    write_renamed(jw, u64::from(r.renamed_valves), c, ItemKind::Valve);
    write_renamed(jw, r.renamed_temps, c, ItemKind::Temp);
    write_renamed(jw, u64::from(r.renamed_volts), c, ItemKind::Volt);
    jw.end_array();
    jw.kv("syslogDebug", r.syslog_debug);
    jw.kv("voltsBlob448", r.volts_blob_448);
    jw.end_object();
    jw.ok()
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_mut;
