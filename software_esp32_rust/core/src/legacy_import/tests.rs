//! Port of test/native/test_legacy_import.cpp: every legacy NVS key of DESIGN.md "Legacy import"
//! with its quirks, blob layouts, per-key rejection, cross-field repair, dropped keys,
//! idempotence, and a fuzz run that the result always validates. A doctest SUBCASE is a block
//! of its own here (each starts from fresh state, as in doctest). The C++ null scratch pointer
//! has no Rust form; the empty scratch slice is tested instead.

use super::*;
use crate::common::{is_zero, parse_one_wire_id, OneWireId};
use crate::config::{encode_config, validate_config, MqttMode, NetInterface, CONFIG_BLOB_MAX};
use crate::test_support::{assert_text, Rng};
use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;
use std::{format, vec};

#[derive(Clone)]
enum Entry {
    Int(i64),
    Str(Vec<u8>),
    Blob(Vec<u8>),
}

/// In-memory NVS with typed entries (a key has exactly one type).
#[derive(Clone, Default)]
pub(super) struct FakeNvs {
    map: BTreeMap<String, Entry>,
    pub(super) reads: u32,
}

fn k(ns: &str, key: &str) -> String {
    format!("{ns}/{key}")
}

impl FakeNvs {
    pub(super) fn put_int(&mut self, ns: &str, key: &str, v: i64) {
        self.map.insert(k(ns, key), Entry::Int(v));
    }
    pub(super) fn put_str(&mut self, ns: &str, key: &str, v: impl AsRef<[u8]>) {
        self.map.insert(k(ns, key), Entry::Str(v.as_ref().to_vec()));
    }
    pub(super) fn put_blob(&mut self, ns: &str, key: &str, v: &[u8]) {
        self.map.insert(k(ns, key), Entry::Blob(v.to_vec()));
    }
}

impl LegacyNvsReader for FakeNvs {
    fn read_int(&mut self, ns: &str, key: &str) -> Option<i64> {
        self.reads += 1;
        match self.map.get(&k(ns, key)) {
            Some(Entry::Int(v)) => Some(*v),
            _ => None,
        }
    }
    fn read_string(&mut self, ns: &str, key: &str, out: &mut TextView) -> Option<bool> {
        self.reads += 1;
        let Some(Entry::Str(s)) = self.map.get(&k(ns, key)) else {
            return None;
        };
        let n = s.len().min(out.capacity());
        out.clear();
        out.extend_from_slice(&s[..n]).expect("fits");
        Some(s.len() > out.capacity())
    }
    fn read_blob(&mut self, ns: &str, key: &str, out: &mut [u8]) -> Option<usize> {
        self.reads += 1;
        let Some(Entry::Blob(b)) = self.map.get(&k(ns, key)) else {
            return None;
        };
        let n = b.len().min(out.len());
        out[..n].copy_from_slice(&b[..n]);
        Some(b.len())
    }
}

/// The import with the scratch the glue lends for the temps blob (exactly the size needed).
pub(super) fn import(n: &mut FakeNvs, c: &mut Config) -> ImportReport {
    let mut scratch = vec![0u8; LEGACY_TEMPS_BLOB];
    import_legacy_config(n, c, &mut scratch)
}

fn put_str_field(b: &mut [u8], at: usize, n: usize, s: &[u8]) {
    b[at..at + n].fill(0);
    let m = s.len().min(n);
    b[at..at + m].copy_from_slice(&s[..m]);
}

fn put_i32(b: &mut [u8], at: usize, v: i32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_f32(b: &mut [u8], at: usize, f: f32) {
    put_i32(b, at, f.to_bits() as i32);
}

fn valves_blob() -> Vec<u8> {
    vec![0u8; LEGACY_VALVES_BLOB]
}

fn set_valve(b: &mut [u8], i: usize, name: &[u8], active: u8) {
    put_str_field(b, i * 12, 11, name);
    b[i * 12 + 11] = active;
}

fn temps_blob() -> Vec<u8> {
    vec![0u8; LEGACY_TEMPS_BLOB]
}

fn set_temp(b: &mut [u8], i: usize, name: &[u8], active: u8, off: i32, id: &str) {
    put_str_field(b, i * 44, 11, name);
    b[i * 44 + 11] = active;
    put_i32(b, i * 44 + 12, off);
    put_str_field(b, i * 44 + 16, 25, id.as_bytes());
}

fn volts_blob() -> Vec<u8> {
    let mut b = vec![0u8; LEGACY_VOLTS_BLOB];
    for i in 0..8 {
        put_f32(&mut b, i * 56 + 16, 1.0);
    }
    b
}

#[allow(clippy::too_many_arguments)]
fn set_volt(
    b: &mut [u8],
    i: usize,
    name: &[u8],
    active: u8,
    off: f32,
    factor: f32,
    unit: &[u8],
    id: &str,
) {
    put_str_field(b, i * 56, 11, name);
    b[i * 56 + 11] = active;
    put_f32(b, i * 56 + 12, off);
    put_f32(b, i * 56 + 16, factor);
    put_str_field(b, i * 56 + 20, 9, unit);
    put_str_field(b, i * 56 + 29, 25, id.as_bytes());
}

const ID_A: &str = "28-84-37-94-97-ff-03-23";
const ID_B: &str = "28-aa-bb-cc-dd-ee-01-67";
const ID_V: &str = "26-11-22-33-44-55-66-29";

fn oid(s: &str) -> OneWireId {
    parse_one_wire_id(s.as_bytes()).expect("a valid id")
}

pub(super) fn valid(c: &Config) -> bool {
    let mut path = [0u8; 64];
    match validate_config(c, &mut path) {
        Ok(()) => true,
        Err(n) => {
            std::println!("invalid at {}", String::from_utf8_lossy(&path[..n]));
            false
        }
    }
}

/// Equal `cfg` blobs.
fn same_config(a: &Config, b: &Config) -> bool {
    let (mut ba, mut bb) = (vec![0u8; CONFIG_BLOB_MAX], vec![0u8; CONFIG_BLOB_MAX]);
    let na = encode_config(a, &mut ba);
    let nb = encode_config(b, &mut bb);
    na > 0 && na == nb && ba[..na] == bb[..nb]
}

pub(super) fn first(r: &ImportReport) -> String {
    String::from_utf8_lossy(&r.first_rejected).into_owned()
}

/// A typical 1.4.x device: Ethernet + DHCP, MQTT + HA, three valves, two sensors, one volt
/// sensor.
fn typical_device() -> FakeNvs {
    let mut n = FakeNvs::default();
    n.put_int("sysCfg", "CF", 0);
    n.put_str("sysCfg", "stName", "VdMotFBH");
    n.put_int("netCfg", "ethwifi", 1);
    n.put_int("netCfg", "dhcp", 1);
    n.put_int("netCfg", "staticIp", 0);
    n.put_int("netCfg", "mask", 0);
    n.put_int("netCfg", "gw", 0);
    n.put_int("netCfg", "dnsIp", 0);
    n.put_str("netCfg", "ssid", "");
    n.put_str("netCfg", "pwd", "");
    n.put_str("netCfg", "userName", "");
    n.put_str("netCfg", "userPwd", "");
    n.put_str("netCfg", "timeServer", "pool.ntp.org");
    n.put_int("netCfg", "syslogEnable", 0);
    n.put_int("netCfg", "sysLogIp", 0);
    n.put_int("netCfg", "sysLogPort", 0);
    n.put_int("netCfg", "netConnTO", 5);
    n.put_str("tZCfg", "tZ", "Europe/Warsaw");
    n.put_str("tZCfg", "tZCode", "CET-1CEST,M3.5.0,M10.5.0/3");
    n.put_int("protCfg", "dataProt", 2);
    n.put_int("protCfg", "brokerIp", 0x6401_A8C0); // 192.168.1.100
    n.put_int("protCfg", "brokerPort", 1883);
    n.put_int("protCfg", "brokerInterval", 1000);
    n.put_int("protCfg", "publishInterval", 30);
    n.put_str("protCfg", "brokerUser", "mqtt");
    n.put_str("protCfg", "brokerPwd", "secret");
    // separate, allTemps, upTime, onChange, retained, plainText
    n.put_int("protCfg", "brokerPF", 0x7B);
    n.put_int("protCfg", "brokerKAT", 60);
    n.put_int("protCfg", "brokerMD", 5);
    n.put_int("protCfg", "brokerMQF", 0);
    n.put_int("protCfg", "brokerMQTO", 120);
    n.put_int("protCfg", "brokerMQToPos", 10);
    let mut v = valves_blob();
    set_valve(&mut v, 0, b"Bad", 1);
    set_valve(&mut v, 1, b"Kueche", 1);
    set_valve(&mut v, 2, b"", 1);
    n.put_blob("valvesCfg", "valves", &v);
    n.put_int("valvesCfg", "dayOfCalib", 9);
    n.put_int("valvesCfg", "hourOfCalib", 3);
    let mut t = temps_blob();
    set_temp(&mut t, 0, b"Flur", 1, -7, ID_A);
    set_temp(&mut t, 5, b"Aussen", 1, 12, "28-AA-BB-CC-DD-EE-01-67");
    set_temp(&mut t, 6, b"", 0, 0, "00-00-00-00-00-00-00-00");
    n.put_blob("tempsCfg", "temps", &t);
    let mut w = volts_blob();
    set_volt(&mut w, 0, b"Akku", 1, 0.5, 0.01, b"V", ID_V);
    n.put_blob("voltsCfg", "volts", &w);
    n.put_int("Misc", "MiscLC", 1_760_000_000);
    n
}

/// typical_device() with a static address, which a reset to the defaults would lose.
fn static_device() -> FakeNvs {
    let mut n = typical_device();
    n.put_int("netCfg", "dhcp", 0);
    n.put_int("netCfg", "staticIp", 0x3201_A8C0); // 192.168.1.50
    n.put_int("netCfg", "mask", 0x00FF_FFFF);
    n.put_int("netCfg", "gw", 0x0101_A8C0);
    n
}

/// Station, network and MQTT of static_device() came through.
fn check_kept(c: &Config) {
    assert_text(&c.station, "VdMotFBH");
    assert!(!c.net.dhcp);
    assert_eq!(c.net.ip, 0x3201_A8C0);
    assert_eq!(c.net.gateway, 0x0101_A8C0);
    assert_eq!(c.mqtt.mode, MqttMode::MqttHa);
    assert_text(&c.mqtt.host, "192.168.1.100");
    assert_text(&c.mqtt.password, "secret");
    assert_text(&c.time.tz_name, "Europe/Warsaw");
    assert!(valid(c));
}

#[test]
fn empty_nvs_gives_the_defaults_and_no_legacy_flag() {
    let mut n = FakeNvs::default();
    let mut c = Config::default();
    c.calib.hour = 9;
    let r = import(&mut n, &mut c);
    assert!(!r.any_legacy);
    assert_eq!(r.imported, 0);
    assert_eq!(r.rejected, 0);
    assert_eq!(r.ignored, 0);
    assert!(first(&r).is_empty());
    assert_eq!(r.last_calib_epoch, 0);
    assert!(same_config(&c, &Config::default()));
    assert!(n.reads > 0);
}

#[test]
fn a_typical_device_imports_completely() {
    let mut n = typical_device();
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert!(r.any_legacy);
    assert_eq!(r.rejected, 0);
    assert!(first(&r).is_empty());
    // stName + 13 netCfg + 2 tZCfg + dataProt brokerIp brokerPort publishInterval brokerUser
    // brokerPwd brokerPF brokerKAT brokerMD brokerMQF + valves dayOfCalib hourOfCalib + temps
    // + volts + MiscLC
    assert_eq!(r.imported, 1 + 13 + 2 + 10 + 3 + 1 + 1 + 1);
    // CF, userName, userPwd, brokerInterval, brokerMQTO, brokerMQToPos
    assert_eq!(r.ignored, 6);
    assert_eq!(r.last_calib_epoch, 1_760_000_000);
    assert!(valid(&c));

    assert_text(&c.station, "VdMotFBH");
    assert_eq!(c.net.iface, NetInterface::Ethernet);
    assert!(c.net.dhcp);
    assert_eq!(c.net.reconnect_timeout_min, 5);
    assert_text(&c.time.ntp_server, "pool.ntp.org");
    assert_text(&c.time.tz_name, "Europe/Warsaw");
    assert_text(&c.time.tz_posix, "CET-1CEST,M3.5.0,M10.5.0/3");
    assert_eq!(c.syslog.level, 0);
    assert_eq!(c.syslog.port, 514); // 0 -> 514
    assert_eq!(c.mqtt.mode, MqttMode::MqttHa);
    assert_text(&c.mqtt.host, "192.168.1.100");
    assert_eq!(c.mqtt.port, 1883);
    assert_eq!(c.mqtt.publish_interval_s, 30);
    assert_text(&c.mqtt.user, "mqtt");
    assert_text(&c.mqtt.password, "secret");
    assert_eq!(c.mqtt.keep_alive_s, 60);
    assert_eq!(c.mqtt.min_delay_s, 5);
    assert!(c.mqtt.separate);
    assert!(c.mqtt.all_temps);
    assert!(!c.mqtt.path_as_root);
    assert!(c.mqtt.up_time);
    assert!(c.mqtt.on_change);
    assert!(c.mqtt.retained);
    assert!(c.mqtt.plain_text);
    assert!(!c.mqtt.diag);
    assert!(!c.mqtt.german_decimal);
    assert!(c.mqtt.new_diag); // new keys keep their defaults
    assert!(c.mqtt.events);
    assert_text(&c.valves[0].name, "Bad");
    assert!(c.valves[0].active);
    assert_text(&c.valves[1].name, "Kueche");
    assert!(c.valves[2].active);
    assert!(!c.valves[3].active);
    assert_eq!(c.calib.day_mask, 9);
    assert_eq!(c.calib.hour, 3);
    assert_eq!(c.calib.minute, 0);
    assert_text(&c.temps[0].name, "Flur");
    assert!(c.temps[0].active);
    assert_eq!(c.temps[0].offset, -7);
    assert_eq!(c.temps[0].id, oid(ID_A));
    assert_eq!(c.temps[5].id, oid(ID_B));
    assert_eq!(c.temps[5].offset, 12);
    assert!(is_zero(&c.temps[6].id));
    assert_text(&c.volts[0].name, "Akku");
    assert!(c.volts[0].active);
    assert_eq!(c.volts[0].offset, 0.5);
    assert_eq!(c.volts[0].factor, 0.01f32);
    assert_text(&c.volts[0].unit, "V");
    assert_eq!(c.volts[0].id, oid(ID_V));
    assert_eq!(c.volts[1].factor, 1.0);
}

#[test]
fn import_is_idempotent_and_read_only() {
    let mut n = typical_device();
    let mut a = Config::default();
    let mut b = Config::default();
    let ra = import(&mut n, &mut a);
    let rb = import(&mut n, &mut b);
    assert!(same_config(&a, &b));
    assert_eq!(ra.imported, rb.imported);
    assert_eq!(ra.rejected, rb.rejected);
    assert_eq!(ra.ignored, rb.ignored);
    // The output is rebuilt from defaults, not merged into what was there.
    let mut dirty = Config::default();
    copy_string(&mut dirty.valves[9].name, b"junk");
    dirty.calib.minute = 30;
    import(&mut n, &mut dirty);
    assert!(same_config(&dirty, &a));
}

#[test]
fn network_keys_and_their_quirks() {
    {
        // static IP
        let mut n = FakeNvs::default();
        n.put_int("netCfg", "dhcp", 0);
        n.put_int("netCfg", "staticIp", 0x3201_A8C0);
        n.put_int("netCfg", "mask", 0x00FF_FFFF);
        n.put_int("netCfg", "gw", 0x0101_A8C0);
        n.put_int("netCfg", "dnsIp", 0x0808_0808);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.imported, 5);
        assert_eq!(r.rejected, 0);
        assert!(!c.net.dhcp);
        assert_eq!(c.net.ip, 0x3201_A8C0);
        assert_eq!(c.net.mask, 0x00FF_FFFF);
        assert_eq!(c.net.gateway, 0x0101_A8C0);
        assert_eq!(c.net.dns, 0x0808_0808);
        assert!(valid(&c));
    }
    {
        // incomplete static IP falls back to DHCP
        let mut n = FakeNvs::default();
        n.put_int("netCfg", "dhcp", 0);
        n.put_int("netCfg", "staticIp", 0x3201_A8C0);
        n.put_int("netCfg", "mask", 0x00FF_FFFF);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert!(c.net.dhcp);
        assert_eq!(c.net.ip, 0x3201_A8C0);
        assert_eq!(r.rejected, 1);
        assert_eq!(first(&r), "netCfg/dhcp");
        assert!(valid(&c));
    }
    {
        // non-contiguous mask and bad integers are rejected
        let mut n = FakeNvs::default();
        n.put_int("netCfg", "mask", 0x00FF_00FF);
        n.put_int("netCfg", "staticIp", -1);
        n.put_int("netCfg", "gw", 0x1_0000_0000);
        n.put_int("netCfg", "dhcp", 2);
        n.put_int("netCfg", "ethwifi", 3);
        n.put_int("netCfg", "netConnTO", 241);
        n.put_int("netCfg", "syslogEnable", 4);
        n.put_int("netCfg", "sysLogPort", 65536);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.imported, 0);
        assert_eq!(r.rejected, 8);
        assert_eq!(first(&r), "netCfg/ethwifi");
        assert!(same_config(&c, &Config::default()));
    }
    {
        // boundaries that are accepted
        let mut n = FakeNvs::default();
        n.put_int("netCfg", "ethwifi", 2);
        n.put_str("netCfg", "ssid", "s".repeat(32));
        n.put_str("netCfg", "pwd", "p".repeat(63));
        n.put_int("netCfg", "netConnTO", 240);
        n.put_int("netCfg", "syslogEnable", 3);
        n.put_int("netCfg", "sysLogIp", 0x0A00_000A);
        n.put_int("netCfg", "sysLogPort", 65535);
        n.put_int("netCfg", "staticIp", 0xFFFF_FFFF);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.rejected, 0);
        assert_eq!(r.imported, 8);
        assert_eq!(c.net.iface, NetInterface::Wifi);
        assert_eq!(c.net.reconnect_timeout_min, 240);
        assert_eq!(c.syslog.level, 3);
        assert_eq!(c.syslog.server, 0x0A00_000A);
        assert_eq!(c.syslog.port, 65535);
        assert_eq!(c.net.ip, 0xFFFF_FFFF);
        assert!(valid(&c));
    }
    {
        // WiFi without a usable password is disabled
        let mut n = FakeNvs::default();
        n.put_int("netCfg", "ethwifi", 2);
        n.put_str("netCfg", "ssid", "home");
        n.put_str("netCfg", "pwd", "short");
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert!(c.net.ssid.is_empty());
        assert!(c.net.wifi_password.is_empty());
        assert_eq!(c.net.iface, NetInterface::Auto);
        assert_eq!(r.imported, 3);
        assert_eq!(r.rejected, 2);
        assert_eq!(first(&r), "netCfg/pwd");
        assert!(valid(&c));
    }
    {
        // WiFi-only without ssid becomes auto
        let mut n = FakeNvs::default();
        n.put_int("netCfg", "ethwifi", 2);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(c.net.iface, NetInterface::Auto);
        assert_eq!(first(&r), "netCfg/ethwifi");
    }
    {
        // over-long strings (strncpy without NUL in the legacy UI)
        let mut n = FakeNvs::default();
        n.put_str("netCfg", "ssid", "s".repeat(33));
        n.put_str("netCfg", "pwd", "p".repeat(64));
        n.put_str("netCfg", "userName", "u".repeat(65)); // the web login: ignored
        n.put_str("netCfg", "userPwd", "p".repeat(200));
        n.put_str("netCfg", "timeServer", "bad host");
        n.put_str("sysCfg", "stName", "x".repeat(21));
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.rejected, 4);
        assert_eq!(r.ignored, 2);
        assert_eq!(r.imported, 0);
        assert_eq!(first(&r), "sysCfg/stName");
        assert!(same_config(&c, &Config::default()));
    }
    {
        // a string longer than the read buffer is rejected once
        let mut n = FakeNvs::default();
        n.put_str("netCfg", "timeServer", "h".repeat(200));
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.rejected, 1);
        assert_eq!(first(&r), "netCfg/timeServer");
        assert!(same_config(&c, &Config::default()));
    }
    {
        // time server may be empty or an address
        let mut n = FakeNvs::default();
        n.put_str("netCfg", "timeServer", "");
        let mut c = Config::default();
        import(&mut n, &mut c);
        assert!(c.time.ntp_server.is_empty());
        n.put_str("netCfg", "timeServer", "192.168.1.1");
        import(&mut n, &mut c);
        assert_text(&c.time.ntp_server, "192.168.1.1");
    }
    {
        // syslog without server is switched off
        let mut n = FakeNvs::default();
        n.put_int("netCfg", "syslogEnable", 2);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(c.syslog.level, 0);
        assert_eq!(first(&r), "netCfg/syslogEnable");
        assert_eq!(r.imported, 1);
        assert_eq!(r.rejected, 1);
    }
}

#[test]
fn cross_field_repairs_one_condition_at_a_time() {
    let st: [(i64, i64, i64, bool); 4] = [
        (0, 0x00FF_FFFF, 1, true),
        (1, 0, 1, true),
        (1, 0x00FF_FFFF, 0, true),
        (1, 0x00FF_FFFF, 1, false),
    ];
    for (ip, mask, gw, dhcp_after) in st {
        let mut n = FakeNvs::default();
        n.put_int("netCfg", "dhcp", 0);
        n.put_int("netCfg", "staticIp", ip);
        n.put_int("netCfg", "mask", mask);
        n.put_int("netCfg", "gw", gw);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(c.net.dhcp, dhcp_after);
        assert_eq!(r.rejected, u16::from(dhcp_after));
        assert_eq!(i64::from(c.net.ip), ip); // repaired, not reset to defaults
        assert_eq!(i64::from(c.net.gateway), gw);
    }
    {
        let mut n = FakeNvs::default(); // one-char ssid, short password
        n.put_str("netCfg", "ssid", "x");
        n.put_str("netCfg", "pwd", "1234567");
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert!(c.net.ssid.is_empty());
        assert_eq!(first(&r), "netCfg/pwd");
        assert_eq!(r.rejected, 1);
    }
    {
        let mut n = FakeNvs::default(); // one-char ssid, WiFi only, good password: kept
        n.put_int("netCfg", "ethwifi", 2);
        n.put_str("netCfg", "ssid", "x");
        n.put_str("netCfg", "pwd", "12345678");
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_text(&c.net.ssid, "x");
        assert_eq!(c.net.iface, NetInterface::Wifi);
        assert_eq!(r.rejected, 0);
    }
    {
        let mut n = FakeNvs::default(); // syslog level 1 without server
        n.put_int("netCfg", "syslogEnable", 1);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(c.syslog.level, 0);
        assert_eq!(first(&r), "netCfg/syslogEnable");
    }
    {
        let mut n = FakeNvs::default(); // one-char broker host is a host
        n.put_int("protCfg", "dataProt", 1);
        n.put_int("protCfg", "brokerIp", 0x0403_0201);
        n.put_int("protCfg", "brokerMD", 10);
        n.put_int("protCfg", "publishInterval", 10); // min delay == interval: fine
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(c.mqtt.mode, MqttMode::Mqtt);
        assert_eq!(c.mqtt.min_delay_s, 10);
        assert_eq!(r.rejected, 0);
    }
}

#[test]
fn the_web_login_is_counted_as_ignored_whatever_it_holds() {
    // A whole login, half ones the 1.4.x firmware accepted, one with a ':'.
    let logins: [(Option<&str>, Option<&str>, u16); 5] = [
        (Some("admin"), Some("pw"), 2),
        (Some("u"), None, 1),
        (None, Some("p"), 1),
        (Some("ad:min"), Some("pw"), 2),
        (Some(""), Some(""), 2),
    ];
    for (user, password, ignored) in logins {
        let mut n = FakeNvs::default();
        n.put_str("sysCfg", "stName", "keep");
        if let Some(u) = user {
            n.put_str("netCfg", "userName", u);
        }
        if let Some(p) = password {
            n.put_str("netCfg", "userPwd", p);
        }
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.ignored, ignored);
        assert_eq!(r.imported, 1);
        assert_eq!(r.rejected, 0);
        assert!(first(&r).is_empty());
        let mut expect = Config::default();
        copy_string(&mut expect.station, b"keep");
        assert!(same_config(&c, &expect));
    }
}

#[test]
fn station_name() {
    let mut n = FakeNvs::default();
    n.put_str("sysCfg", "stName", "a/b");
    let mut c = Config::default();
    let mut r = import(&mut n, &mut c);
    assert_text(&c.station, "VdMot");
    assert_eq!(first(&r), "sysCfg/stName");
    assert!(r.any_legacy); // a string key alone counts
    assert!(c.mqtt.root_topic.is_empty());
    // Empty: the legacy topics were under "VdMotFBH/" (W18-1).
    n.put_str("sysCfg", "stName", "");
    r = import(&mut n, &mut c);
    assert_text(&c.station, "VdMot");
    assert_text(&c.mqtt.root_topic, "VdMotFBH");
    assert_eq!(r.rejected, 0);
    assert_eq!(r.imported, 1);
    n.put_str("sysCfg", "stName", "Dom 1");
    import(&mut n, &mut c);
    assert_text(&c.station, "Dom 1");
    assert!(c.mqtt.root_topic.is_empty());
    n.put_str("sysCfg", "stName", "x".repeat(20));
    r = import(&mut n, &mut c);
    assert_text(&c.station, "x".repeat(20));
    assert_eq!(r.imported, 1);
    // Wrong stored type: the key is treated as missing.
    n.put_int("sysCfg", "stName", 5);
    r = import(&mut n, &mut c);
    assert_text(&c.station, "VdMot");
    assert_eq!(r.rejected, 0);
    assert_eq!(r.imported, 0);
    assert!(!r.any_legacy);
}

#[test]
fn a_one_character_station_name_is_a_name_not_the_unnamed_station() {
    let mut n = FakeNvs::default();
    n.put_str("sysCfg", "stName", "A");
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_text(&c.station, "A");
    assert!(c.mqtt.root_topic.is_empty()); // "VdMotFBH" only for the empty name
    assert_eq!(r.imported, 1);
    assert_eq!(r.rejected, 0);
}

#[test]
fn mqtt_keys_and_their_quirks() {
    {
        // zeros mean defaults, interval is clamped
        let mut n = FakeNvs::default();
        n.put_int("protCfg", "dataProt", 1);
        n.put_int("protCfg", "brokerIp", 0x0100_000A);
        n.put_int("protCfg", "brokerPort", 0);
        n.put_int("protCfg", "publishInterval", 0);
        let mut c = Config::default();
        let mut r = import(&mut n, &mut c);
        assert_eq!(c.mqtt.mode, MqttMode::Mqtt);
        assert_text(&c.mqtt.host, "10.0.0.1");
        assert_eq!(c.mqtt.port, 1883);
        assert_eq!(c.mqtt.publish_interval_s, 2);
        assert_eq!(c.mqtt.min_delay_s, 2); // default 5 > 2 -> clamped, reported
        assert_eq!(first(&r), "protCfg/brokerMD");
        assert_eq!(r.imported, 4);
        n.put_int("protCfg", "publishInterval", 1);
        import(&mut n, &mut c);
        assert_eq!(c.mqtt.publish_interval_s, 2);
        n.put_int("protCfg", "publishInterval", 2);
        import(&mut n, &mut c);
        assert_eq!(c.mqtt.publish_interval_s, 2);
        n.put_int("protCfg", "publishInterval", 3600);
        r = import(&mut n, &mut c);
        assert_eq!(c.mqtt.publish_interval_s, 3600);
        assert_eq!(c.mqtt.min_delay_s, 5);
        assert_eq!(r.rejected, 0);
        n.put_int("protCfg", "publishInterval", 3601);
        import(&mut n, &mut c);
        assert_eq!(c.mqtt.publish_interval_s, 3600);
        n.put_int("protCfg", "publishInterval", 4_294_967_295);
        import(&mut n, &mut c);
        assert_eq!(c.mqtt.publish_interval_s, 3600);
        n.put_int("protCfg", "publishInterval", -5);
        import(&mut n, &mut c);
        assert_eq!(c.mqtt.publish_interval_s, 2);
    }
    {
        // broker address 0 means no host
        let mut n = FakeNvs::default();
        n.put_int("protCfg", "dataProt", 1);
        n.put_int("protCfg", "brokerIp", 0);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(c.mqtt.mode, MqttMode::Off); // MQTT without a broker is switched off
        assert!(c.mqtt.host.is_empty());
        assert_eq!(r.imported, 2);
        assert_eq!(first(&r), "protCfg/brokerIp");
        assert!(valid(&c));
    }
    {
        // broker address out of range
        let mut n = FakeNvs::default();
        n.put_int("protCfg", "brokerIp", -1);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(first(&r), "protCfg/brokerIp");
        assert_eq!(r.imported, 0);
    }
    {
        // flags: missing brokerPF falls back to 7 when protCfg exists
        let mut n = FakeNvs::default();
        n.put_int("protCfg", "dataProt", 0);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert!(c.mqtt.separate);
        assert!(c.mqtt.all_temps);
        assert!(c.mqtt.path_as_root);
        assert!(!c.mqtt.up_time);
        assert!(!c.mqtt.on_change);
        assert!(!c.mqtt.retained);
        assert!(!c.mqtt.plain_text);
        assert!(!c.mqtt.diag);
        assert_eq!(r.imported, 1);
        assert!(valid(&c));
    }
    {
        // flags: no protCfg at all keeps the new defaults
        let mut n = FakeNvs::default();
        n.put_str("sysCfg", "stName", "x");
        let mut c = Config::default();
        import(&mut n, &mut c);
        assert!(!c.mqtt.path_as_root);
        assert!(c.mqtt.up_time);
        assert!(c.mqtt.diag);
    }
    {
        // flags: every bit maps to its field
        let names = [
            "separate",
            "allTemps",
            "pathAsRoot",
            "upTime",
            "onChange",
            "retained",
            "plainText",
            "diag",
        ];
        for (bit, name) in names.iter().enumerate() {
            let mut n = FakeNvs::default();
            n.put_int("protCfg", "brokerPF", 1 << bit);
            let mut c = Config::default();
            import(&mut n, &mut c);
            let m = &c.mqtt;
            let got = [
                m.separate,
                m.all_temps,
                m.path_as_root,
                m.up_time,
                m.on_change,
                m.retained,
                m.plain_text,
                m.diag,
            ];
            for (kk, g) in got.iter().enumerate() {
                assert_eq!(*g, kk == bit, "{name} {kk}");
            }
        }
        {
            let mut z = FakeNvs::default();
            z.put_int("protCfg", "brokerPF", 0);
            let mut c0 = Config::default();
            let r0 = import(&mut z, &mut c0);
            assert_eq!(r0.rejected, 0);
            assert_eq!(r0.imported, 1);
            assert!(!c0.mqtt.separate);
            assert!(!c0.mqtt.up_time);
            assert!(!c0.mqtt.diag);
        }
        let mut n = FakeNvs::default();
        n.put_int("protCfg", "brokerPF", 0xFF);
        let mut c = Config::default();
        import(&mut n, &mut c);
        assert!(c.mqtt.diag);
        assert!(c.mqtt.path_as_root);
        n.put_int("protCfg", "brokerPF", 256);
        let r = import(&mut n, &mut c);
        assert_eq!(first(&r), "protCfg/brokerPF");
        assert!(c.mqtt.up_time); // default kept
        n.put_int("protCfg", "brokerPF", -1);
        assert_eq!(first(&import(&mut n, &mut c)), "protCfg/brokerPF");
    }
    {
        // HA mode needs separate topics
        let mut n = FakeNvs::default();
        n.put_int("protCfg", "dataProt", 2);
        n.put_int("protCfg", "brokerIp", 0x0100_000A);
        n.put_int("protCfg", "brokerPF", 0xFE);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(c.mqtt.mode, MqttMode::Mqtt);
        assert!(!c.mqtt.separate);
        assert_eq!(first(&r), "protCfg/dataProt");
        assert_eq!(r.rejected, 1);
        assert!(valid(&c));
    }
    {
        // dataProt out of range
        let mut n = FakeNvs::default();
        n.put_int("protCfg", "dataProt", 3);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(c.mqtt.mode, MqttMode::Off);
        assert_eq!(first(&r), "protCfg/dataProt");
        assert!(c.mqtt.path_as_root); // namespace present -> brokerPF fallback 7
    }
    {
        // keepalive, min delay and number format
        let mut n = FakeNvs::default();
        n.put_int("protCfg", "brokerKAT", 4);
        n.put_int("protCfg", "brokerMD", 3601);
        n.put_int("protCfg", "brokerMQF", 4);
        let mut c = Config::default();
        let mut r = import(&mut n, &mut c);
        assert_eq!(c.mqtt.keep_alive_s, 60);
        assert_eq!(c.mqtt.min_delay_s, 5);
        assert!(c.mqtt.german_decimal);
        assert_eq!(r.rejected, 2);
        assert_eq!(r.ignored, 0);
        assert_eq!(first(&r), "protCfg/brokerKAT");
        n.put_int("protCfg", "brokerKAT", 300);
        n.put_int("protCfg", "brokerMD", 0);
        n.put_int("protCfg", "brokerMQF", 3); // failsafe bits only: dropped
        r = import(&mut n, &mut c);
        assert_eq!(c.mqtt.keep_alive_s, 300);
        assert_eq!(c.mqtt.min_delay_s, 0);
        assert!(!c.mqtt.german_decimal);
        assert_eq!(r.ignored, 1);
        assert_eq!(r.rejected, 0);
        n.put_int("protCfg", "brokerKAT", 5);
        n.put_int("protCfg", "brokerMQF", 1);
        r = import(&mut n, &mut c);
        assert_eq!(c.mqtt.keep_alive_s, 5);
        assert_eq!(r.ignored, 1);
        n.put_int("protCfg", "brokerKAT", 301);
        n.put_int("protCfg", "brokerMQF", 0x104);
        r = import(&mut n, &mut c);
        assert_eq!(r.rejected, 2);
        assert!(!c.mqtt.german_decimal);
        n.put_int("protCfg", "brokerMQF", 0xFF);
        n.put_int("protCfg", "brokerKAT", 60);
        r = import(&mut n, &mut c);
        assert_eq!(r.rejected, 0);
        assert!(c.mqtt.german_decimal);
        assert_eq!(r.ignored, 1);
        n.put_int("protCfg", "brokerMQF", -4);
        assert_eq!(import(&mut n, &mut c).rejected, 1);
    }
    {
        // credentials
        let mut n = FakeNvs::default();
        n.put_str("protCfg", "brokerUser", "u".repeat(64));
        n.put_str("protCfg", "brokerPwd", "p".repeat(65));
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_text(&c.mqtt.user, "u".repeat(64));
        assert!(c.mqtt.password.is_empty());
        assert_eq!(first(&r), "protCfg/brokerPwd");
    }
}

#[test]
fn time_zone_keys() {
    let mut n = FakeNvs::default();
    n.put_str("tZCfg", "tZ", "");
    n.put_str("tZCfg", "tZCode", ""); // setDefault() stores empty strings
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert!(c.time.tz_name.is_empty());
    assert_text(&c.time.tz_posix, "CET-1CEST,M3.5.0,M10.5.0/3");
    assert_eq!(first(&r), "tZCfg/tZCode");
    assert_eq!(r.imported, 1);
    n.put_str("tZCfg", "tZCode", "EST5EDT");
    n.put_str("tZCfg", "tZ", "z".repeat(50));
    let r2 = import(&mut n, &mut c);
    assert_text(&c.time.tz_posix, "EST5EDT");
    assert_text(&c.time.tz_name, "Europe/Berlin");
    assert_eq!(first(&r2), "tZCfg/tZ");
}

#[test]
fn calibration_schedule_keys() {
    let mut n = FakeNvs::default();
    n.put_int("valvesCfg", "dayOfCalib", 127);
    n.put_int("valvesCfg", "hourOfCalib", 23);
    let mut c = Config::default();
    let mut r = import(&mut n, &mut c);
    assert_eq!(c.calib.day_mask, 127);
    assert_eq!(c.calib.hour, 23);
    assert_eq!(r.imported, 2);
    n.put_int("valvesCfg", "dayOfCalib", 0);
    n.put_int("valvesCfg", "hourOfCalib", 0);
    import(&mut n, &mut c);
    assert_eq!(c.calib.day_mask, 0);
    assert_eq!(c.calib.hour, 0);
    n.put_int("valvesCfg", "dayOfCalib", 128);
    n.put_int("valvesCfg", "hourOfCalib", 23);
    r = import(&mut n, &mut c);
    assert_eq!(c.calib.day_mask, 9);
    assert_eq!(c.calib.hour, 23);
    assert_eq!(r.rejected, 1);
    assert_eq!(first(&r), "valvesCfg/dayOfCalib");
    n.put_int("valvesCfg", "hourOfCalib", -1);
    r = import(&mut n, &mut c);
    assert_eq!(r.rejected, 2);
    assert_eq!(c.calib.day_mask, 9);
    assert_eq!(c.calib.hour, 0);

    // Hour 24 (the legacy UI allowed it; tm_hour never matches) meant "never": no scheduled
    // calibration, whatever the days say.
    for hour in [24, 25, 255] {
        for days in [9, 128] {
            let mut h = FakeNvs::default();
            h.put_int("valvesCfg", "dayOfCalib", days);
            h.put_int("valvesCfg", "hourOfCalib", hour);
            let mut d = Config::default();
            r = import(&mut h, &mut d);
            assert_eq!(d.calib.day_mask, 0, "{hour} {days}");
            assert_eq!(d.calib.hour, 0, "{hour} {days}");
            assert_eq!(r.imported, 2, "{hour} {days}");
            assert_eq!(r.rejected, 0, "{hour} {days}");
        }
    }
    let mut h = FakeNvs::default();
    h.put_int("valvesCfg", "hourOfCalib", 24); // days missing
    let mut d = Config::default();
    r = import(&mut h, &mut d);
    assert_eq!(d.calib.day_mask, 0);
    assert_eq!(r.imported, 1);
    h.put_int("valvesCfg", "hourOfCalib", 256); // not a legacy value
    d = Config::default();
    r = import(&mut h, &mut d);
    assert_eq!(d.calib.day_mask, 9);
    assert_eq!(r.rejected, 1);
}

#[test]
fn names_and_texts_are_kept_byte_for_byte_utf8_edge_spaces() {
    let mut n = FakeNvs::default();
    n.put_str("sysCfg", "stName", "Fu\u{df}boden");
    let mut v = valves_blob();
    set_valve(&mut v, 0, "K\u{fc}che".as_bytes(), 1); // 6 bytes
    set_valve(&mut v, 1, b"Bad ", 1); // trailing space: segment "Bad_"
    set_valve(&mut v, 2, "\u{e4}\u{f6}\u{fc}\u{df}\u{e4}".as_bytes(), 1); // 10 bytes
    n.put_blob("valvesCfg", "valves", &v);
    let mut vb = volts_blob();
    set_volt(
        &mut vb,
        0,
        b"Vorlauf",
        0,
        0.0,
        1.0,
        "\u{b0}C".as_bytes(),
        "",
    );
    n.put_blob("voltsCfg", "volts", &vb);
    n.put_str("netCfg", "ssid", "G\u{e4}ste");
    n.put_str("netCfg", "pwd", "p\u{e4}sswort");
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_eq!(r.rejected, 0);
    assert_text(&c.station, "Fu\u{df}boden");
    assert_text(&c.valves[0].name, "K\u{fc}che");
    assert_text(&c.valves[1].name, "Bad ");
    assert_text(&c.valves[2].name, "\u{e4}\u{f6}\u{fc}\u{df}\u{e4}");
    assert_text(&c.volts[0].unit, "\u{b0}C");
    assert_text(&c.net.ssid, "G\u{e4}ste");
    assert_text(&c.net.wifi_password, "p\u{e4}sswort");
    let mut path = [0u8; 48];
    assert!(validate_config(&c, &mut path).is_ok());

    // Bytes that are not UTF-8 (a truncated sequence) are still rejected.
    let mut bad = FakeNvs::default();
    let mut bv = valves_blob();
    set_valve(&mut bv, 0, b"Kueche\xc3", 1);
    bad.put_blob("valvesCfg", "valves", &bv);
    bad.put_str("sysCfg", "stName", b"St\xe4tion"); // Latin-1
    let mut b = Config::default();
    let rb = import(&mut bad, &mut b);
    assert_eq!(rb.rejected, 2);
    assert!(b.valves[0].name.is_empty());
    assert_text(&b.station, "VdMot");
}

#[test]
fn an_open_wifi_network_empty_password_is_kept() {
    let mut n = FakeNvs::default();
    n.put_int("netCfg", "ethwifi", 2);
    n.put_str("netCfg", "ssid", "Guest");
    n.put_str("netCfg", "pwd", "");
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_eq!(r.rejected, 0);
    assert_text(&c.net.ssid, "Guest");
    assert!(c.net.wifi_password.is_empty());
    assert_eq!(c.net.iface, NetInterface::Wifi);
    let mut path = [0u8; 48];
    assert!(validate_config(&c, &mut path).is_ok());
}

#[test]
fn valves_blob_checks() {
    {
        // wrong size rejects the whole blob
        for size in [0, 143, 145, 288] {
            let mut n = FakeNvs::default();
            let mut b = vec![0u8; size];
            if size >= 12 {
                set_valve(&mut b, 0, b"x", 1);
            }
            n.put_blob("valvesCfg", "valves", &b);
            let mut c = Config::default();
            let r = import(&mut n, &mut c);
            assert!(r.any_legacy, "{size}");
            assert_eq!(r.imported, 0, "{size}");
            assert_eq!(r.rejected, 1, "{size}");
            assert_eq!(first(&r), "valvesCfg/valves");
            assert!(c.valves[0].name.is_empty());
            assert!(!c.valves[0].active);
        }
    }
    {
        // per-element checks
        let mut n = FakeNvs::default();
        let mut b = valves_blob();
        set_valve(&mut b, 0, b"ok", 1);
        b[12..23].fill(b'x'); // valve 2: name without NUL
        b[23] = 1;
        set_valve(&mut b, 2, b"bad\x01", 0); // not printable
        set_valve(&mut b, 3, b"x", 2); // active byte not a bool
        set_valve(&mut b, 11, b"1234567890", 1);
        n.put_blob("valvesCfg", "valves", &b);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.imported, 1);
        assert_eq!(r.rejected, 3);
        assert_eq!(r.renamed_valves, 0);
        assert_eq!(first(&r), "valvesCfg/valves.2.name");
        assert_text(&c.valves[0].name, "ok");
        assert!(c.valves[0].active);
        assert!(c.valves[1].name.is_empty());
        assert!(c.valves[1].active);
        assert!(c.valves[2].name.is_empty());
        assert_text(&c.valves[3].name, "x");
        assert!(!c.valves[3].active);
        assert_text(&c.valves[11].name, "1234567890");
        assert!(valid(&c));
    }
    {
        // duplicate names: later ones are cleared
        let mut n = FakeNvs::default();
        let mut b = valves_blob();
        set_valve(&mut b, 0, b"Bad", 1);
        set_valve(&mut b, 4, b"Bad", 1);
        set_valve(&mut b, 5, b"a b", 1);
        set_valve(&mut b, 6, b"a_b", 1);
        n.put_blob("valvesCfg", "valves", &b);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_text(&c.valves[0].name, "Bad");
        assert!(c.valves[4].name.is_empty());
        assert_text(&c.valves[5].name, "a b");
        assert!(c.valves[6].name.is_empty());
        assert_eq!(r.rejected, 2);
        assert_eq!(first(&r), "valvesCfg/valves.5.name");
        assert!(valid(&c));
    }
    {
        // number collisions, also the ones created by clearing
        let mut n = FakeNvs::default();
        let mut b = valves_blob();
        set_valve(&mut b, 0, b"4", 1); // valve 4 is named "x" ... until it is cleared
        set_valve(&mut b, 1, b"x", 1);
        set_valve(&mut b, 3, b"x", 1); // duplicate of valve 2 -> cleared -> "4" collides
        set_valve(&mut b, 7, b"8", 1); // own number: fine
        n.put_blob("valvesCfg", "valves", &b);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert!(c.valves[3].name.is_empty());
        assert!(c.valves[0].name.is_empty());
        assert_text(&c.valves[1].name, "x");
        assert_text(&c.valves[7].name, "8");
        assert_eq!(r.rejected, 2);
        assert_eq!(first(&r), "valvesCfg/valves.4.name");
        assert_eq!(r.renamed_valves, (1 << 0) | (1 << 3));
        assert!(valid(&c));
    }
}

#[test]
fn valve_name_rules_keep_everything_else() {
    let mut n = FakeNvs::default();
    let mut b = valves_blob();
    set_valve(&mut b, 1, b"ab", 1);
    set_valve(&mut b, 2, b"ac", 1); // same first char: no clash
    set_valve(&mut b, 3, b"05", 1); // leading zero: not a number
    set_valve(&mut b, 4, b"10", 1); // valve 10 is unnamed: clash
    set_valve(&mut b, 5, b"1", 1); // valve 1 is unnamed: clash
    n.put_blob("valvesCfg", "valves", &b);
    let mut t = temps_blob();
    set_temp(&mut t, 0, b"ab", 0, 0, ""); // temp names never clash with valves
    n.put_blob("tempsCfg", "temps", &t);
    n.put_str("sysCfg", "stName", "keep");
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_text(&c.valves[1].name, "ab");
    assert_text(&c.valves[2].name, "ac");
    assert_text(&c.valves[3].name, "05");
    assert!(c.valves[4].name.is_empty());
    assert!(c.valves[5].name.is_empty());
    assert_text(&c.temps[0].name, "ab");
    assert_text(&c.station, "keep");
    assert_eq!(r.rejected, 2);
    assert_eq!(first(&r), "valvesCfg/valves.5.name");
}

#[test]
fn slot_repairs_keep_everything_else() {
    let mut n = FakeNvs::default();
    let mut t = temps_blob();
    set_temp(&mut t, 0, b"a", 1, 0, ""); // active without id
    set_temp(&mut t, 2, b"c", 1, 0, ID_B);
    set_temp(&mut t, 5, b"f", 1, 0, ID_B); // duplicate of slot 3
    set_temp(&mut t, 33, b"z", 1, 0, ID_A);
    n.put_blob("tempsCfg", "temps", &t);
    let mut v = volts_blob();
    set_volt(&mut v, 0, b"v", 1, 0.0, 1.0, b"", "");
    set_volt(&mut v, 3, b"w", 1, 0.0, 1.0, b"", ID_V);
    set_volt(&mut v, 7, b"x", 1, 0.0, 1.0, b"", ID_V);
    n.put_blob("voltsCfg", "volts", &v);
    n.put_str("sysCfg", "stName", "keep");
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert!(!c.temps[0].active);
    assert!(c.temps[2].active);
    assert_eq!(c.temps[2].id, oid(ID_B));
    assert!(!c.temps[5].active);
    assert!(is_zero(&c.temps[5].id));
    assert!(c.temps[33].active);
    assert!(!c.volts[0].active);
    assert!(c.volts[3].active);
    assert!(!c.volts[7].active);
    assert!(is_zero(&c.volts[7].id));
    assert_text(&c.station, "keep");
    // temps.1.active, temps.6.id, temps.6.active, volts.1.active, volts.8.id, volts.8.active
    assert_eq!(r.rejected, 6);
    assert_eq!(first(&r), "tempsCfg/temps.1.active");
}

#[test]
fn ha_mode_with_the_decimal_comma_switches_to_the_dot() {
    let mut n = static_device();
    n.put_int("protCfg", "brokerMQF", 4);
    let mut c = Config::default();
    let mut r = import(&mut n, &mut c);
    assert!(!c.mqtt.german_decimal);
    assert_eq!(r.rejected, 1);
    assert_eq!(first(&r), "protCfg/brokerMQF");
    check_kept(&c);
    // HA without separate topics becomes plain MQTT first, which keeps the comma.
    n.put_int("protCfg", "brokerPF", 0x7A);
    r = import(&mut n, &mut c);
    assert_eq!(c.mqtt.mode, MqttMode::Mqtt);
    assert!(c.mqtt.german_decimal);
    assert_eq!(r.rejected, 1);
    assert_eq!(first(&r), "protCfg/dataProt");
    n.put_int("protCfg", "brokerPF", 0x7B);
    n.put_int("protCfg", "dataProt", 1);
    r = import(&mut n, &mut c);
    assert!(c.mqtt.german_decimal);
    assert_eq!(r.rejected, 0);
}

#[test]
fn valve_names_with_one_ha_id_keep_the_first_name() {
    let mut n = static_device();
    let mut v = valves_blob();
    set_valve(&mut v, 0, b"Bad 1", 1);
    set_valve(&mut v, 1, b"Bad.1", 1); // HA id "Bad_1" like valve 1
    set_valve(&mut v, 2, "K\u{fc}che".as_bytes(), 1);
    set_valve(&mut v, 3, b"Kuche", 0); // HA id "Kuche" like valve 3, also when inactive
    set_valve(&mut v, 4, b"Bad-1", 1); // '-' stays: another id
    n.put_blob("valvesCfg", "valves", &v);
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_text(&c.valves[0].name, "Bad 1");
    assert!(c.valves[1].name.is_empty());
    assert!(c.valves[1].active);
    assert_text(&c.valves[2].name, "K\u{fc}che");
    assert!(c.valves[3].name.is_empty());
    assert_text(&c.valves[4].name, "Bad-1");
    assert_eq!(r.rejected, 2);
    assert_eq!(first(&r), "valvesCfg/valves.2.name");
    check_kept(&c);
}

#[test]
fn active_sensors_with_one_ha_id_keep_the_first_name() {
    let mut n = static_device();
    let mut t = temps_blob();
    set_temp(&mut t, 0, b"Flur", 1, -7, ID_A);
    set_temp(&mut t, 5, b"Flur", 1, 12, ID_B); // later: name cleared, the rest kept
    set_temp(&mut t, 6, b"Bad", 0, 0, ""); // inactive: no HA entity, no clash
    set_temp(&mut t, 7, b"Bad", 1, 0, "28-00-00-00-00-00-00-01");
    set_temp(&mut t, 8, b"Flur", 0, 0, "28-00-00-00-00-00-00-02");
    n.put_blob("tempsCfg", "temps", &t);
    let mut w = volts_blob();
    set_volt(&mut w, 0, b"U", 1, 0.5, 0.01, b"V", ID_V);
    // neighbours, one char
    set_volt(
        &mut w,
        1,
        b"U",
        1,
        0.0,
        1.0,
        b"V",
        "26-00-00-00-00-00-00-01",
    );
    n.put_blob("voltsCfg", "volts", &w);
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_text(&c.temps[0].name, "Flur");
    assert!(c.temps[5].name.is_empty());
    assert!(c.temps[5].active);
    assert_eq!(c.temps[5].id, oid(ID_B));
    assert_eq!(c.temps[5].offset, 12);
    assert_text(&c.temps[6].name, "Bad");
    assert_text(&c.temps[7].name, "Bad");
    assert_text(&c.temps[8].name, "Flur");
    assert_text(&c.volts[0].name, "U");
    assert!(c.volts[1].name.is_empty());
    assert!(c.volts[1].active);
    assert_eq!(r.rejected, 2);
    assert_eq!(first(&r), "tempsCfg/temps.6.name");
    check_kept(&c);
}

#[test]
fn a_sensor_named_like_the_number_of_a_later_unnamed_one_loses_its_name() {
    let mut n = FakeNvs::default();
    let mut t = temps_blob();
    set_temp(&mut t, 0, b"3", 1, 0, ID_A); // HA id "3" like slot 3 once that one is cleared
    set_temp(&mut t, 2, b"7", 1, 0, ID_B); // HA id "7" like the unnamed slot 7
    set_temp(&mut t, 6, b"", 1, 0, "28-00-00-00-00-00-00-01");
    n.put_blob("tempsCfg", "temps", &t);
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert!(c.temps[0].name.is_empty());
    assert!(c.temps[2].name.is_empty());
    assert!(c.temps[0].active);
    assert!(c.temps[2].active);
    assert!(c.temps[6].active);
    assert_eq!(r.rejected, 2);
    assert_eq!(first(&r), "tempsCfg/temps.3.name");
    assert_eq!(r.renamed_temps, (1 << 0) | (1 << 2));
    assert!(valid(&c));
}

#[test]
fn a_sensor_name_cleared_for_one_clash_is_checked_again() {
    let mut n = FakeNvs::default();
    let mut t = temps_blob();
    set_temp(&mut t, 0, b"x", 1, 0, ID_A);
    set_temp(&mut t, 1, b"3", 1, 0, ID_B); // clashes with slot 3 once its "x" is cleared
    set_temp(&mut t, 2, b"x", 1, 0, "28-00-00-00-00-00-00-01");
    n.put_blob("tempsCfg", "temps", &t);
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_text(&c.temps[0].name, "x");
    assert!(c.temps[1].name.is_empty());
    assert!(c.temps[2].name.is_empty());
    assert_eq!(r.rejected, 2);
    assert_eq!(first(&r), "tempsCfg/temps.3.name");
    assert!(valid(&c));
}

#[test]
fn a_sensor_topic_override_cleared_for_one_clash_is_reported() {
    let mut n = FakeNvs::default();
    let mut t = temps_blob();
    set_temp(&mut t, 0, b"Bad/WC", 1, 0, ID_A);
    set_temp(&mut t, 1, b"Bad/WC", 1, 0, ID_B);
    n.put_blob("tempsCfg", "temps", &t);
    let mut w = volts_blob();
    set_volt(&mut w, 0, b"Bad/WC", 1, 0.0, 1.0, b"", ID_V);
    set_volt(
        &mut w,
        1,
        b"Bad/WC",
        1,
        0.0,
        1.0,
        b"",
        "26-11-22-33-44-55-66-01",
    );
    n.put_blob("voltsCfg", "volts", &w);
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_text(&c.temps[0].topic, "Bad/WC");
    assert!(c.temps[1].topic.is_empty());
    assert_text(&c.volts[0].topic, "Bad/WC");
    assert!(c.volts[1].topic.is_empty());
    assert_eq!(first(&r), "tempsCfg/temps.2.topic");
    assert!(r.rejected >= 2);
    assert_eq!(r.renamed_temps, 3);
    assert_eq!(r.renamed_volts, 3);
    assert!(valid(&c));
}

#[test]
fn temps_blob_checks() {
    {
        // wrong size
        let mut n = FakeNvs::default();
        n.put_blob("tempsCfg", "temps", &[0u8; 1495]);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(first(&r), "tempsCfg/temps");
        assert_eq!(r.imported, 0);
    }
    {
        // offsets are clamped to +-10.0 C
        let mut n = FakeNvs::default();
        let mut b = temps_blob();
        set_temp(&mut b, 0, b"", 0, 100, "");
        set_temp(&mut b, 1, b"", 0, -100, "");
        set_temp(&mut b, 2, b"", 0, 101, "");
        set_temp(&mut b, 3, b"", 0, -101, "");
        set_temp(&mut b, 4, b"", 0, i32::MAX, "");
        set_temp(&mut b, 5, b"", 0, i32::MIN, "");
        n.put_blob("tempsCfg", "temps", &b);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(c.temps[0].offset, 100);
        assert_eq!(c.temps[1].offset, -100);
        assert_eq!(c.temps[2].offset, 100);
        assert_eq!(c.temps[3].offset, -100);
        assert_eq!(c.temps[4].offset, 100);
        assert_eq!(c.temps[5].offset, -100);
        assert_eq!(r.rejected, 4);
        assert_eq!(first(&r), "tempsCfg/temps.3.offset");
        assert!(valid(&c));
    }
    {
        // ids, activity and duplicates
        let mut n = FakeNvs::default();
        let mut b = temps_blob();
        set_temp(&mut b, 0, b"a", 1, 0, ID_A);
        set_temp(&mut b, 1, b"b", 1, 0, ""); // active without id
        set_temp(&mut b, 2, b"c", 1, 0, "00-00-00-00-00-00-00-00"); // undefined id
        set_temp(&mut b, 3, b"d", 1, 0, "28-84-37-94-97-FF-03-23"); // duplicate of slot 1
        set_temp(&mut b, 4, b"e", 0, 0, "28-84-37"); // malformed
        b[5 * 44 + 16..5 * 44 + 16 + 25].fill(b'2'); // no NUL
        set_temp(&mut b, 33, b"z", 1, 5, ID_B);
        n.put_blob("tempsCfg", "temps", &b);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert!(c.temps[0].active);
        assert_eq!(c.temps[0].id, oid(ID_A));
        assert!(!c.temps[1].active);
        assert!(!c.temps[2].active);
        assert!(is_zero(&c.temps[2].id));
        assert!(!c.temps[3].active);
        assert!(is_zero(&c.temps[3].id));
        assert_text(&c.temps[3].name, "d");
        assert!(is_zero(&c.temps[4].id));
        assert!(is_zero(&c.temps[5].id));
        assert!(c.temps[33].active);
        assert_eq!(c.temps[33].offset, 5);
        // rejected: 5.id, 6.id (import) then 2.active, 3.active, 4.id, 4.active (repair)
        assert_eq!(r.rejected, 6);
        assert_eq!(first(&r), "tempsCfg/temps.5.id");
        assert!(valid(&c));
    }
    {
        // names
        let mut n = FakeNvs::default();
        let mut b = temps_blob();
        set_temp(&mut b, 0, b"Wohnzimmer", 0, 0, "");
        set_temp(&mut b, 1, b"x#", 0, 0, "");
        b[2 * 44..2 * 44 + 11].fill(b'n');
        n.put_blob("tempsCfg", "temps", &b);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_text(&c.temps[0].name, "Wohnzimmer");
        assert_text(&c.temps[1].name, "x_"); // renamed (W18)
        assert!(c.temps[1].topic.is_empty());
        assert_eq!(r.renamed_temps, 2);
        assert!(c.temps[2].name.is_empty());
        assert_eq!(r.rejected, 1);
        assert_eq!(first(&r), "tempsCfg/temps.3.name");
    }
}

#[test]
fn the_temps_blob_goes_into_the_lent_scratch_without_room_it_is_not_read() {
    let mut n = FakeNvs::default();
    let mut b = temps_blob();
    set_temp(&mut b, 2, b"Flur", 1, 25, ID_A);
    n.put_blob("tempsCfg", "temps", &b);
    let mut c = Config::default();
    let mut scratch = vec![0xA5u8; LEGACY_TEMPS_BLOB];
    let mut r = import_legacy_config(&mut n, &mut c, &mut scratch);
    assert_eq!(r.imported, 1);
    assert_text(&c.temps[2].name, "Flur");
    assert_eq!(c.temps[2].offset, 25);
    assert_eq!(scratch, b);
    // One byte short, or no buffer at all: the blob stays unread (one NVS read less), nothing
    // is counted.
    let reads_full = n.reads;
    r = import_legacy_config(&mut n, &mut c, &mut scratch[..LEGACY_TEMPS_BLOB - 1]);
    assert_eq!(n.reads - reads_full, reads_full - 1);
    assert_eq!(r.imported, 0);
    assert_eq!(r.rejected, 0);
    assert!(!r.any_legacy);
    assert!(c.temps[2].name.is_empty());
    assert_eq!(c.temps[2].offset, 0);
    // C++ a null scratch with a capacity: the empty slice.
    let reads_short = n.reads;
    r = import_legacy_config(&mut n, &mut c, &mut []);
    assert_eq!(n.reads - reads_short, reads_full - 1);
    assert_eq!(r.imported, 0);
    assert!(!r.any_legacy);
    assert_eq!(c.temps[2].offset, 0);
}

#[test]
fn volts_blob_checks() {
    {
        // wrong size
        for size in [447, 449, 479, 481] {
            let mut n = FakeNvs::default();
            n.put_blob("voltsCfg", "volts", &vec![0u8; size]);
            let mut c = Config::default();
            let r = import(&mut n, &mut c);
            assert_eq!(first(&r), "voltsCfg/volts", "{size}");
            assert_eq!(r.imported, 0, "{size}");
            assert!(r.any_legacy, "{size}");
            assert!(!r.volts_blob_448, "{size}");
        }
    }
    {
        // fields
        let mut n = FakeNvs::default();
        let mut b = volts_blob();
        set_volt(&mut b, 0, b"v1", 1, -1000.0, 1000.0, b"12345678", ID_V);
        set_volt(&mut b, 1, b"v2", 0, f32::NAN, 0.0, b"", "");
        set_volt(&mut b, 2, b"v3", 0, 1000.5, -1000.5, b"", "");
        set_volt(&mut b, 3, b"v4", 0, 0.0, f32::INFINITY, b"", "");
        b[4 * 56 + 20..4 * 56 + 29].fill(b'u'); // unit without NUL
        set_volt(&mut b, 5, b"v6", 1, 0.0, 1.0, b"V", ID_V); // duplicate id
        set_volt(&mut b, 6, b"v7", 0, 0.0, 1.0, b"a/b", "");
        b[7 * 56 + 11] = 3;
        n.put_blob("voltsCfg", "volts", &b);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(c.volts[0].offset, -1000.0);
        assert_eq!(c.volts[0].factor, 1000.0);
        assert_text(&c.volts[0].unit, "12345678");
        assert!(c.volts[0].active);
        assert_eq!(c.volts[1].offset, 0.0);
        assert_eq!(c.volts[1].factor, 1.0);
        assert_eq!(c.volts[2].offset, 0.0);
        assert_eq!(c.volts[2].factor, 1.0);
        assert_eq!(c.volts[3].factor, 1.0);
        assert!(c.volts[4].unit.is_empty());
        assert!(is_zero(&c.volts[5].id));
        assert!(!c.volts[5].active);
        assert!(c.volts[6].unit.is_empty());
        assert!(!c.volts[7].active);
        // 2.offset 2.factor 3.offset 3.factor 4.factor 5.unit 7.unit 8.active, then repair
        // 6.id 6.active
        assert_eq!(r.rejected, 10);
        assert_eq!(first(&r), "voltsCfg/volts.2.offset");
        assert!(valid(&c));
    }
}

#[test]
fn last_calibration_time() {
    let cases: [(i64, i64, u16); 6] = [
        (1_577_836_800, 1_577_836_800, 0),
        (1_577_836_799, 0, 1),
        (0, 0, 1),
        (-1, 0, 1),
        (4_102_444_799, 4_102_444_799, 0),
        (4_102_444_800, 0, 1),
    ];
    for (v, epoch, rejected) in cases {
        let mut n = FakeNvs::default();
        n.put_int("Misc", "MiscLC", v);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.last_calib_epoch, epoch, "{v}");
        assert_eq!(r.rejected, rejected, "{v}");
        assert_eq!(r.imported, 1 - rejected, "{v}");
        if rejected != 0 {
            assert_eq!(first(&r), "Misc/MiscLC");
        }
        assert!(r.any_legacy, "{v}");
    }
}

#[test]
fn dropped_keys_are_counted_never_imported() {
    let mut n = FakeNvs::default();
    n.put_int("sysCfg", "CF", 1);
    n.put_str("netCfg", "userName", "admin");
    n.put_str("netCfg", "userPwd", "p".repeat(300));
    n.put_int("protCfg", "brokerInterval", 1000);
    n.put_int("protCfg", "brokerMQTO", 120);
    n.put_int("protCfg", "brokerMQToPos", 10);
    n.put_int("valvesCfg", "movCalib", 1);
    n.put_blob("valvesCtrlCfg", "valvesCtrl", &[0u8; 240]);
    n.put_blob("valvesCtrlCfg", "valvesCtrl1", &[1u8; 10]);
    n.put_blob("valvesCtrlCfg", "vCtrlInit", &[1u8; 48]);
    n.put_int("valvesCtrlCfg", "vCtrlHeat", 1);
    n.put_int("valvesCtrlCfg", "vCtrlParkPos", 10);
    n.put_int("msgCfg", "msgFlags", 3);
    n.put_int("msgCfg", "msgReas", 127);
    n.put_int("msgCfg", "msgMQTO", 10);
    n.put_str("msgCfg", "POAppTk", "tok");
    n.put_str("msgCfg", "POUserTk", "tok");
    n.put_str("msgCfg", "POTitle", "t");
    n.put_str("msgCfg", "EUser", "u");
    n.put_str("msgCfg", "EPwd", "p".repeat(300)); // long values are fine to skip
    n.put_str("msgCfg", "EHost", "h");
    n.put_int("msgCfg", "EPort", 465);
    n.put_str("msgCfg", "ERep", "r");
    n.put_str("msgCfg", "ETitle", "t");
    n.put_int("motorCfg", "motorMinC", 1);
    n.put_int("motorCfg", "motorMaxC", 2);
    n.put_int("msgCfg", "unknownKey", 1); // not on the list: not counted
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_eq!(r.ignored, 26);
    assert_eq!(r.imported, 0);
    assert_eq!(r.rejected, 0);
    assert_eq!(r.dropped, DROPPED_MESSENGER);
    assert_eq!(r.pi_valves, 0);
    assert!(r.legacy_failsafe_valid);
    assert!(!r.legacy_failsafe_enabled);
    assert!(r.any_legacy);
    assert!(same_config(&c, &Config::default()));

    let mut only = FakeNvs::default();
    only.put_int("valvesCtrlCfg", "vCtrlHeat", 1);
    let mut d = Config::default();
    let r2 = import(&mut only, &mut d);
    assert!(r2.any_legacy);
    assert_eq!(r2.ignored, 1);
}

#[test]
fn first_rejected_key_is_kept_counting_continues() {
    let mut n = FakeNvs::default();
    n.put_str("sysCfg", "stName", "");
    n.put_int("netCfg", "ethwifi", 9);
    n.put_int("valvesCfg", "hourOfCalib", 256);
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_eq!(r.rejected, 2);
    assert_eq!(first(&r), "netCfg/ethwifi");
}

#[test]
fn fuzz_the_result_always_validates() {
    let mut rng = Rng::new(97531);
    let int_keys: [(&str, &str); 21] = [
        ("netCfg", "ethwifi"),
        ("netCfg", "dhcp"),
        ("netCfg", "staticIp"),
        ("netCfg", "mask"),
        ("netCfg", "gw"),
        ("netCfg", "dnsIp"),
        ("netCfg", "netConnTO"),
        ("netCfg", "syslogEnable"),
        ("netCfg", "sysLogIp"),
        ("netCfg", "sysLogPort"),
        ("protCfg", "dataProt"),
        ("protCfg", "brokerIp"),
        ("protCfg", "brokerPort"),
        ("protCfg", "publishInterval"),
        ("protCfg", "brokerPF"),
        ("protCfg", "brokerKAT"),
        ("protCfg", "brokerMD"),
        ("protCfg", "brokerMQF"),
        ("valvesCfg", "dayOfCalib"),
        ("valvesCfg", "hourOfCalib"),
        ("Misc", "MiscLC"),
    ];
    // tZCfg/tZ is always "Europe/Warsaw": nothing resets the whole config.
    let str_keys: [(&str, &str); 7] = [
        ("sysCfg", "stName"),
        ("netCfg", "ssid"),
        ("netCfg", "pwd"),
        ("netCfg", "timeServer"),
        ("tZCfg", "tZCode"),
        ("protCfg", "brokerUser"),
        ("protCfg", "brokerPwd"),
    ];
    // The first 10 are names, the last 3 ids.
    let pool: [&str; 18] = [
        "",
        "a",
        "1",
        "3",
        "x y",
        "a b",
        "a_b",
        "a.b",
        "Bad",
        "12345678",
        "pool.ntp.org",
        "1.2.3.4",
        "a:b",
        "bad/x",
        "EST5EDT",
        ID_A,
        ID_B,
        "00-00-00-00-00-00-00-00",
    ];
    let ints: [i64; 20] = [
        0,
        1,
        2,
        3,
        4,
        7,
        23,
        24,
        127,
        128,
        255,
        300,
        3600,
        65535,
        65536,
        -1,
        0x00FF_FFFF,
        0x0101_A8C0,
        1_760_000_000,
        0xFFFF_FFFF,
    ];
    for _ in 0..1500 {
        let mut n = FakeNvs::default();
        n.put_str("tZCfg", "tZ", "Europe/Warsaw");
        for (ns, key) in int_keys {
            if rng.below(2) != 0 {
                let v = ints[rng.below(ints.len() as u32) as usize];
                n.put_int(ns, key, v);
            }
        }
        for (ns, key) in str_keys {
            if rng.below(2) != 0 {
                let mut s = pool[rng.below(pool.len() as u32) as usize]
                    .as_bytes()
                    .to_vec();
                if rng.below(8) == 0 {
                    // C++ std::string(count, ch): the arguments are drawn right to left (GCC)
                    let ch = rng.next_u32() as u8;
                    let count = rng.below(80) as usize;
                    s = vec![ch; count];
                }
                n.put_str(ns, key, &s);
            }
        }
        if rng.below(2) != 0 {
            let mut b = valves_blob();
            for i in 0..12 {
                if rng.below(2) != 0 {
                    // C++ setValve(b, i, pool[rng() % 10], rng() % 3): right to left (GCC)
                    let active = rng.below(3) as u8;
                    let name = pool[rng.below(10) as usize];
                    set_valve(&mut b, i, name.as_bytes(), active);
                }
            }
            if rng.below(6) == 0 {
                for x in &mut b {
                    *x = rng.next_u32() as u8;
                }
            }
            n.put_blob("valvesCfg", "valves", &b);
        }
        if rng.below(2) != 0 {
            let mut b = temps_blob();
            for i in 0..34 {
                if rng.below(3) == 0 {
                    // C++ setTemp(b, i, pool[..], rng() % 3, rng() % 400 - 200, pool[15 + ..]):
                    // the arguments are drawn right to left (GCC)
                    let id = pool[15 + rng.below(3) as usize];
                    let off = rng.below(400) as i32 - 200;
                    let active = rng.below(3) as u8;
                    let name = pool[rng.below(10) as usize];
                    set_temp(&mut b, i, name.as_bytes(), active, off, id);
                }
            }
            if rng.below(6) == 0 {
                for x in &mut b {
                    *x = rng.next_u32() as u8;
                }
            }
            n.put_blob("tempsCfg", "temps", &b);
        }
        if rng.below(2) != 0 {
            let mut b = volts_blob();
            for x in &mut b {
                if rng.below(4) == 0 {
                    *x = rng.next_u32() as u8;
                }
            }
            n.put_blob("voltsCfg", "volts", &b);
        }
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert!(valid(&c));
        assert_text(&c.time.tz_name, "Europe/Warsaw");
        assert!(r.first_rejected.len() < 40);
        assert_eq!(r.rejected == 0, r.first_rejected.is_empty());
    }
}

#[test]
fn a_one_character_wifi_password_is_repaired_not_reset_to_defaults() {
    let mut n = FakeNvs::default();
    n.put_str("netCfg", "ssid", "home");
    n.put_str("netCfg", "pwd", "1");
    n.put_str("sysCfg", "stName", "keep");
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert!(c.net.ssid.is_empty());
    assert!(c.net.wifi_password.is_empty());
    assert_text(&c.station, "keep");
    assert_eq!(first(&r), "netCfg/pwd");
    assert_eq!(r.rejected, 1);
}

#[test]
fn names_mqtt_cannot_carry_are_renamed_the_legacy_segment_kept() {
    let cases: [(&[u8], &str, &str); 7] = [
        (b"Bad/WC", "Bad_WC", "Bad/WC"),
        (b"Ba d\"x", "Ba d_x", "Ba_d\"x"),
        (b"A+B", "A_B", ""),
        (b"/Bad", "_Bad", ""),
        (b"a\\b", "a_b", "a\\b"),
        (b"x#/y", "x__y", ""),
        (b"a//b", "a__b", ""),
    ];
    for (legacy, name, topic) in cases {
        let mut n = FakeNvs::default();
        let mut v = valves_blob();
        set_valve(&mut v, 2, legacy, 1);
        n.put_blob("valvesCfg", "valves", &v);
        let mut t = temps_blob();
        set_temp(&mut t, 33, legacy, 0, 0, "");
        n.put_blob("tempsCfg", "temps", &t);
        let mut w = volts_blob();
        set_volt(&mut w, 7, legacy, 0, 0.0, 1.0, b"", "");
        n.put_blob("voltsCfg", "volts", &w);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.rejected, 0, "{name}");
        assert_eq!(r.imported, 3, "{name}");
        assert_text(&c.valves[2].name, name);
        assert_text(&c.valves[2].topic, topic);
        assert_text(&c.temps[33].name, name);
        assert_text(&c.temps[33].topic, topic);
        assert_text(&c.volts[7].name, name);
        assert_text(&c.volts[7].topic, topic);
        assert_eq!(r.renamed_valves, 1 << 2);
        assert_eq!(r.renamed_temps, 1 << 33);
        assert_eq!(r.renamed_volts, 1 << 7);
        assert!(valid(&c));
    }
    // A valid name is not renamed, a name that stays invalid is rejected.
    let mut n = FakeNvs::default();
    let mut v = valves_blob();
    set_valve(&mut v, 0, b"Bad WC", 1);
    set_valve(&mut v, 1, b"a/\x01", 1);
    n.put_blob("valvesCfg", "valves", &v);
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_eq!(r.renamed_valves, 0);
    assert_eq!(r.rejected, 1);
    assert_eq!(first(&r), "valvesCfg/valves.2.name");
    assert_text(&c.valves[0].name, "Bad WC");
    assert!(c.valves[0].topic.is_empty());
    assert!(c.valves[1].name.is_empty());
}

#[test]
fn a_renamed_valve_that_clashes_loses_its_name_and_its_override() {
    let mut n = FakeNvs::default();
    let mut v = valves_blob();
    set_valve(&mut v, 0, b"Bad/WC", 1);
    set_valve(&mut v, 4, b"Bad/WC", 1);
    n.put_blob("valvesCfg", "valves", &v);
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_text(&c.valves[0].name, "Bad_WC");
    assert_text(&c.valves[0].topic, "Bad/WC");
    assert!(c.valves[4].name.is_empty());
    assert!(c.valves[4].topic.is_empty());
    assert_eq!(r.rejected, 2);
    assert_eq!(first(&r), "valvesCfg/valves.5.name");
    assert_eq!(r.renamed_valves, (1 << 0) | (1 << 4));
    assert!(valid(&c));
}

#[test]
fn the_volts_blob_of_1_4_0_448_bytes_is_imported() {
    for size in [448, 480] {
        let mut n = FakeNvs::default();
        let mut w = volts_blob();
        for i in 0..8 {
            let name = format!("v{}", i + 1);
            let id = format!("26-00-00-00-00-00-00-0{}", i + 1);
            set_volt(
                &mut w,
                i,
                name.as_bytes(),
                1,
                0.5 * i as f32,
                2.0,
                b"V",
                &id,
            );
        }
        w.truncate(size);
        n.put_blob("voltsCfg", "volts", &w);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.volts_blob_448, size == 448);
        assert_eq!(r.imported, 1, "{size}");
        assert_eq!(r.rejected, 0, "{size}");
        for (i, v) in c.volts.iter().enumerate() {
            assert_text(&v.name, format!("v{}", i + 1));
            assert!(v.active);
            assert_eq!(v.offset, 0.5 * i as f32);
            assert_eq!(v.factor, 2.0);
            assert!(!is_zero(&v.id));
        }
    }
}

#[test]
fn valves_ctrl_counts_pi_valves_and_window_contacts() {
    let ctrl = |elem: usize| {
        let mut b = vec![0xEEu8; 12 * elem];
        for i in 0..12 {
            b[i * elem] = 0x20; // other flag bits
        }
        b[0] = 0x21; // valve 1 PI
        b[2 * elem] = 0x11; // valve 3 PI + window
        b
    };
    for elem in [20, 16, 1, 64] {
        let mut n = FakeNvs::default();
        n.put_blob("valvesCtrlCfg", "valvesCtrl", &ctrl(elem));
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.pi_valves, 2, "{elem}");
        assert_eq!(r.window_valves, 1, "{elem}");
        assert_eq!(r.dropped, DROPPED_PI | DROPPED_WINDOW, "{elem}");
        assert_eq!(r.ignored, 1, "{elem}");
        assert_eq!(r.rejected, 0, "{elem}");
    }
    // Window only.
    {
        let mut n = FakeNvs::default();
        let mut b = vec![0u8; 12 * 20];
        b[5 * 20] = 0x10;
        n.put_blob("valvesCtrlCfg", "valvesCtrl", &b);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.pi_valves, 0);
        assert_eq!(r.window_valves, 1);
        assert_eq!(r.dropped, DROPPED_WINDOW);
    }
    for size in [100, 0, 780, 11] {
        let mut n = FakeNvs::default();
        n.put_blob("valvesCtrlCfg", "valvesCtrl", &vec![0x11u8; size]);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.rejected, 1, "{size}");
        assert_eq!(first(&r), "valvesCtrlCfg/valvesCtrl");
        assert_eq!(r.pi_valves, 0, "{size}");
        assert_eq!(r.dropped, 0, "{size}");
        assert_eq!(r.ignored, 1, "{size}");
    }
}

#[test]
fn messenger_ds18_timeout_and_the_legacy_failsafe_are_reported() {
    for flags in [1, 2, 3, 0, 4] {
        let mut n = FakeNvs::default();
        n.put_int("msgCfg", "msgFlags", flags);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        let want = if flags & 3 != 0 { DROPPED_MESSENGER } else { 0 };
        assert_eq!(r.dropped, want, "{flags}");
        assert_eq!(r.ignored, 1, "{flags}");
    }
    {
        let mut n = FakeNvs::default();
        n.put_int("protCfg", "brokerMQF", 0x02);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(r.dropped, DROPPED_DS18_TIMEOUT);
        assert!(!r.legacy_failsafe_enabled);
        assert!(!r.legacy_failsafe_valid);
        assert_eq!(r.ignored, 1);
    }
    {
        // bit0 with the legacy timeout: not mapped, only reported.
        let mut n = FakeNvs::default();
        n.put_int("protCfg", "brokerMQF", 0x01);
        n.put_int("protCfg", "brokerMQTO", 120);
        n.put_int("protCfg", "brokerMQToPos", 10);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(c.failsafe.timeout_min, 60);
        for v in &c.valves {
            assert_eq!(v.failsafe_pct, 50);
        }
        assert!(r.legacy_failsafe_valid);
        assert!(r.legacy_failsafe_enabled);
        assert_eq!(r.legacy_failsafe_timeout_min, 120);
        assert_eq!(r.legacy_failsafe_pct, 10);
        assert_eq!(r.dropped, DROPPED_LEGACY_FAILSAFE);
        assert_eq!(r.ignored, 3); // bit0 once, MQTO and ToPos
        assert_eq!(r.imported, 1);
    }
    {
        // Only one of the keys: still reported, the other stays 0.
        let mut n = FakeNvs::default();
        n.put_int("protCfg", "brokerMQToPos", 30);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert!(r.legacy_failsafe_valid);
        assert!(!r.legacy_failsafe_enabled);
        assert_eq!(r.legacy_failsafe_timeout_min, 0);
        assert_eq!(r.legacy_failsafe_pct, 30);
        assert_eq!(r.dropped, 0);
        let mut m = FakeNvs::default();
        m.put_int("protCfg", "brokerMQTO", 45);
        let r2 = import(&mut m, &mut c);
        assert!(r2.legacy_failsafe_valid);
        assert_eq!(r2.legacy_failsafe_timeout_min, 45);
        assert_eq!(r2.legacy_failsafe_pct, 0);
    }
}

#[test]
fn syslog_levels_1_to_3_were_debug_verbosity_and_become_3() {
    let cases: [(i64, bool, u8, bool); 6] = [
        (0, true, 0, false),
        (1, true, 3, true),
        (2, true, 3, true),
        (3, true, 3, true),
        (4, false, 0, false),
        (-1, false, 0, false),
    ];
    for (legacy, ok, level, debug) in cases {
        let mut n = FakeNvs::default();
        n.put_int("netCfg", "syslogEnable", legacy);
        n.put_int("netCfg", "sysLogIp", 0x0901_A8C0);
        let mut c = Config::default();
        let r = import(&mut n, &mut c);
        assert_eq!(c.syslog.level, level, "{legacy}");
        assert_eq!(r.syslog_debug, debug, "{legacy}");
        assert_eq!(r.rejected, u16::from(!ok), "{legacy}");
        assert_eq!(r.imported, if ok { 2 } else { 1 }, "{legacy}");
        if !ok {
            assert_eq!(first(&r), "netCfg/syslogEnable");
        }
    }
}

#[test]
fn the_import_report_document() {
    let mut r = ImportReport {
        imported: 57,
        rejected: 2,
        ignored: 14,
        pi_valves: 3,
        window_valves: 1,
        dropped: DROPPED_PI
            | DROPPED_WINDOW
            | DROPPED_MESSENGER
            | DROPPED_DS18_TIMEOUT
            | DROPPED_LEGACY_FAILSAFE,
        legacy_failsafe_valid: true,
        legacy_failsafe_enabled: true,
        legacy_failsafe_timeout_min: 120,
        legacy_failsafe_pct: 10,
        renamed_valves: (1 << 2) | (1 << 4),
        renamed_temps: 1 << 33,
        renamed_volts: 1 << 7,
        syslog_debug: true,
        volts_blob_448: false,
        ..ImportReport::default()
    };
    copy_string(&mut r.first_rejected, b"valvesCfg/valves.5.name");
    let mut c = Config::default();
    copy_string(&mut c.mqtt.root_topic, b"VdMotFBH");
    copy_string(&mut c.valves[2].name, b"Bad_WC");
    copy_string(&mut c.valves[2].topic, b"Bad/WC");
    copy_string(&mut c.temps[33].name, b"a\"b");
    copy_string(&mut c.volts[7].name, b"v");
    let mut buf = [0u8; 1024];
    let mut jw = JsonWriter::new(&mut buf);
    assert!(write_import_report_json(&mut jw, &r, &c));
    assert_text(
        jw.as_bytes(),
        "{\"imported\":57,\"rejected\":2,\"ignored\":14,\
\"firstRejected\":\"valvesCfg/valves.5.name\",\"piValves\":3,\"windowValves\":1,\
\"dropped\":[\"pi\",\"window\",\"messenger\",\"ds18Timeout\",\"legacyFailsafe\"],\
\"legacyFailsafe\":{\"enabled\":true,\"timeoutMin\":120,\"pct\":10},\
\"rootTopic\":\"VdMotFBH\",\
\"renamed\":[{\"kind\":\"valve\",\"n\":3,\"name\":\"Bad_WC\",\"topic\":\"Bad/WC\"},\
{\"kind\":\"valve\",\"n\":5,\"name\":\"\",\"topic\":\"\"},\
{\"kind\":\"temp\",\"n\":34,\"name\":\"a\\\"b\",\"topic\":\"\"},\
{\"kind\":\"volt\",\"n\":8,\"name\":\"v\",\"topic\":\"\"}],\
\"syslogDebug\":true,\"voltsBlob448\":false}",
    );
    // Empty report: nulls and empty arrays.
    let e = ImportReport {
        dropped: DROPPED_MESSENGER,
        volts_blob_448: true,
        ..ImportReport::default()
    };
    let mut je = JsonWriter::new(&mut buf);
    assert!(write_import_report_json(&mut je, &e, &Config::default()));
    assert_text(
        je.as_bytes(),
        "{\"imported\":0,\"rejected\":0,\"ignored\":0,\"firstRejected\":\"\",\"piValves\":0,\
\"windowValves\":0,\"dropped\":[\"messenger\"],\"legacyFailsafe\":null,\
\"rootTopic\":null,\"renamed\":[],\"syslogDebug\":false,\"voltsBlob448\":true}",
    );
    // Too small a buffer: false.
    let mut small = JsonWriter::new(&mut buf[..40]);
    assert!(!write_import_report_json(&mut small, &r, &c));
}

#[test]
fn a_typical_device_reports_its_dropped_features_in_one_import() {
    let mut n = typical_device();
    n.put_str("sysCfg", "stName", "");
    n.put_int("protCfg", "brokerMQF", 0x03);
    let mut ctrl = vec![0u8; 12 * 20];
    ctrl[0] = 0x01;
    n.put_blob("valvesCtrlCfg", "valvesCtrl", &ctrl);
    n.put_int("msgCfg", "msgFlags", 1);
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_eq!(
        r.dropped,
        DROPPED_PI | DROPPED_MESSENGER | DROPPED_DS18_TIMEOUT | DROPPED_LEGACY_FAILSAFE
    );
    assert_eq!(r.pi_valves, 1);
    assert_text(&c.mqtt.root_topic, "VdMotFBH");
    assert_text(&c.station, "VdMot");
    assert_eq!(r.legacy_failsafe_timeout_min, 120);
    assert_eq!(r.legacy_failsafe_pct, 10);
    assert!(valid(&c));
}

// ---------------------------------------------------------------- Rust-only cases
// (expected values checked against the C++ on the differential drivers)

#[test]
fn a_65_byte_string_is_read_whole_a_66_byte_one_is_cut() {
    // The C++ reads a string into char[66]: 65 bytes come back whole and are then a C string,
    // a longer value comes back cut and is rejected. A NUL inside makes the two visible.
    let mut n = FakeNvs::default();
    let mut station = b"abc\0".to_vec();
    station.resize(65, b'x');
    let mut ntp = b"pool\0".to_vec();
    ntp.resize(65, b'y');
    n.put_str("sysCfg", "stName", &station);
    n.put_str("netCfg", "timeServer", &ntp);
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert_eq!((r.imported, r.rejected), (2, 0));
    assert_text(&c.station, "abc");
    assert_text(&c.time.ntp_server, "pool");

    station.push(b'x');
    n.put_str("sysCfg", "stName", &station);
    let r = import(&mut n, &mut c);
    assert_eq!((r.imported, r.rejected), (1, 1));
    assert_eq!(first(&r), "sysCfg/stName");
    assert_text(&c.station, "VdMot");
}

#[test]
fn temp_slot_repairs_are_reported_past_the_8_volt_slots() {
    let mut n = FakeNvs::default();
    let mut t = temps_blob();
    set_temp(&mut t, 0, b"a", 1, 0, ""); // active without id: the lead
    set_temp(&mut t, 9, b"j", 1, 0, "");
    n.put_blob("tempsCfg", "temps", &t);
    let mut c = Config::default();
    let r = import(&mut n, &mut c);
    assert!(!c.temps[0].active);
    assert!(!c.temps[9].active);
    assert_eq!(r.rejected, 2); // temps.1.active, temps.10.active
    assert_eq!(first(&r), "tempsCfg/temps.1.active");
    assert!(valid(&c));
}

#[test]
fn a_cleared_slot_topic_after_the_lead_is_reported() {
    // Slot 3 repeats the legacy segment of slot 2: its override and then its name (the same
    // HA id) are cleared, both reported after the lead (slot 1 active without id).
    let mut n = FakeNvs::default();
    let mut t = temps_blob();
    set_temp(&mut t, 0, b"a", 1, 0, "");
    set_temp(&mut t, 1, b"Bad/WC", 1, 0, ID_A);
    set_temp(&mut t, 2, b"Bad/WC", 1, 0, ID_B);
    n.put_blob("tempsCfg", "temps", &t);
    let mut w = volts_blob();
    set_volt(&mut w, 0, b"v", 1, 0.0, 1.0, b"", "");
    set_volt(&mut w, 1, b"Bad/WC", 1, 0.0, 1.0, b"", ID_V);
    set_volt(
        &mut w,
        2,
        b"Bad/WC",
        1,
        0.0,
        1.0,
        b"",
        "26-11-22-33-44-55-66-01",
    );
    let mut nv = FakeNvs::default();
    nv.put_blob("voltsCfg", "volts", &w);
    for (nvs, lead) in [
        (&mut n, "tempsCfg/temps.1.active"),
        (&mut nv, "voltsCfg/volts.1.active"),
    ] {
        let mut c = Config::default();
        let r = import(nvs, &mut c);
        assert_eq!(r.rejected, 3, "{lead}"); // the lead, <slot 3>.topic, <slot 3>.name
        assert_eq!(first(&r), lead);
        assert!(valid(&c));
        if lead.starts_with("temps") {
            assert_text(&c.temps[1].topic, "Bad/WC");
            assert!(c.temps[2].topic.is_empty() && c.temps[2].name.is_empty());
            assert_eq!(r.renamed_temps, 6);
        } else {
            assert_text(&c.volts[1].topic, "Bad/WC");
            assert!(c.volts[2].topic.is_empty() && c.volts[2].name.is_empty());
            assert_eq!(r.renamed_volts, 6);
        }
    }
}

#[test]
fn constants() {
    assert_eq!(LEGACY_VALVES_BLOB, 144);
    assert_eq!(LEGACY_TEMPS_BLOB, 1496);
    assert_eq!(LEGACY_VOLTS_BLOB, 480);
    assert_eq!(LEGACY_VOLTS_BLOB_140, 448);
    assert_eq!(LEGACY_VALVES_CTRL_MAX, 768);
    assert_eq!(
        [
            DROPPED_PI,
            DROPPED_WINDOW,
            DROPPED_MESSENGER,
            DROPPED_DS18_TIMEOUT,
            DROPPED_LEGACY_FAILSAFE
        ],
        [1, 2, 4, 8, 16]
    );
}
