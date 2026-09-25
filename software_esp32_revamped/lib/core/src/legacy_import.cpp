#include "vdm/legacy_import.h"

#include <stdio.h>
#include <string.h>

namespace vdm {

namespace {

constexpr int64_t kEpoch2020 = 1577836800;  // 2020-01-01T00:00:00Z
constexpr int64_t kEpoch2100 = 4102444800;  // sanity bound for a stored time_t

// Legacy blob element layouts (Xtensa GCC, specs/04 §5).
constexpr size_t kValveElem = 12;   // name[11]@0 active@11
constexpr size_t kTempElem = 44;    // name[11]@0 active@11 int offset@12 ID[25]@16
constexpr size_t kVoltElem = 56;    // name[11]@0 active@11 float offset@12 float factor@16
                                    // unit[9]@20 ID[25]@29
constexpr size_t kLegacyNameLen = 11;
constexpr size_t kLegacyUnitLen = 9;
constexpr size_t kLegacyIdLen = 25;

static_assert(kValveElem * kValveCount == kLegacyValvesBlob, "valves blob layout");
static_assert(kTempElem * kTempSlotCount == kLegacyTempsBlob, "temps blob layout");
static_assert((kVoltElem + 4) * kVoltSlotCount == kLegacyVoltsBlob, "volts blob layout");

// Largest legacy string (64 chars) plus one, so an over-long value is read
// completely and rejected by the field rule instead of being cut.
constexpr size_t kStrBuf = kSecretMax + 2;

// Keys the new firmware drops (DESIGN.md "Legacy import"). They are only
// counted, never read into the config. Types differ per key, so every
// accessor is tried.
struct DroppedKey {
  const char* ns;
  const char* key;
};
const DroppedKey kDropped[] = {
    {"sysCfg", "CF"},
    {"protCfg", "brokerInterval"},
    {"protCfg", "brokerMQTO"},
    {"protCfg", "brokerMQToPos"},
    {"valvesCfg", "movCalib"},
    {"valvesCtrlCfg", "valvesCtrl"},
    {"valvesCtrlCfg", "valvesCtrl1"},
    {"valvesCtrlCfg", "vCtrlInit"},
    {"valvesCtrlCfg", "vCtrlHeat"},
    {"valvesCtrlCfg", "vCtrlParkPos"},
    {"msgCfg", "msgFlags"},
    {"msgCfg", "msgReas"},
    {"msgCfg", "msgMQTO"},
    {"msgCfg", "POAppTk"},
    {"msgCfg", "POUserTk"},
    {"msgCfg", "POTitle"},
    {"msgCfg", "EUser"},
    {"msgCfg", "EPwd"},
    {"msgCfg", "EHost"},
    {"msgCfg", "EPort"},
    {"msgCfg", "ERep"},
    {"msgCfg", "ETitle"},
    {"motorCfg", "motorMinC"},
    {"motorCfg", "motorMaxC"},
};

class Importer {
 public:
  Importer(LegacyNvsReader& nvs, Config& c, ImportReport& r) : nvs_(nvs), c_(c), r_(r) {}

  void run() {
    importSys();
    importNet();
    importTz();
    importProt();
    importValves();
    importTemps();
    importVolts();
    importMisc();
    countDropped();
    fixCrossFieldRules();
  }

 private:
  // ------------------------------------------------------------ reading

  bool readInt(const char* ns, const char* key, int64_t& out) {
    if (!nvs_.readInt(ns, key, out)) return false;
    r_.anyLegacy = true;
    return true;
  }

  // Missing -> false. Present but longer than kStrBuf - 1 -> rejected, false.
  bool readString(const char* ns, const char* key, char* out) {
    bool truncated = false;
    if (!nvs_.readString(ns, key, out, kStrBuf, truncated)) return false;
    r_.anyLegacy = true;
    out[kStrBuf - 1] = '\0';
    if (truncated) {
      rejected(ns, key);
      return false;
    }
    return true;
  }

  // Missing -> false. Present with another size -> rejected, false.
  bool readBlob(const char* ns, const char* key, uint8_t* out, size_t size) {
    size_t stored = 0;
    if (!nvs_.readBlob(ns, key, out, size, stored)) return false;
    r_.anyLegacy = true;
    if (stored != size) {
      rejected(ns, key);
      return false;
    }
    return true;
  }

  // ------------------------------------------------------------ report

  void imported() { ++r_.imported; }

  // "<ns>/<key>", cut to fit the report field.
  void rejected(const char* ns, const char* key) {
    if (r_.rejected == 0) {
      char* out = r_.firstRejected;
      const size_t cap = sizeof r_.firstRejected;
      size_t n = 0;
      const char* const parts[] = {ns, "/", key};
      for (const char* part : parts) {
        while (*part != '\0' && n + 1 < cap) out[n++] = *part++;
      }
      out[n] = '\0';
    }
    ++r_.rejected;
  }

  void rejectedElem(const char* ns, const char* key, size_t index, const char* field) {
    char k[32];
    snprintf(k, sizeof k, "%s.%u.%s", key, static_cast<unsigned>(index + 1), field);
    rejected(ns, k);
  }

  // ------------------------------------------------------------ setters

  bool set(const char* path, const ConfigValue& v) {
    return setConfigValue(c_, path, v, true) == SetResult::Ok;
  }

  bool setInt(const char* path, int64_t v) {
    ConfigValue cv;
    cv.type = ConfigValue::Type::Int;
    cv.i = v;
    return set(path, cv);
  }

  bool setString(const char* path, const char* s) {
    ConfigValue cv;
    cv.type = ConfigValue::Type::String;
    cv.s = s;
    cv.len = strlen(s);
    return set(path, cv);
  }

  bool setFloat(const char* path, double v) {
    ConfigValue cv;
    cv.type = ConfigValue::Type::Float;
    cv.f = v;
    return set(path, cv);
  }

  // Legacy uint32 IPv4 -> dotted string -> setter (keeps the mask rule).
  bool setIp(const char* path, int64_t v) {
    if (v < 0 || v > 0xFFFFFFFFll) return false;
    char ip[16];
    formatIpv4(static_cast<uint32_t>(v), ip, sizeof ip);
    return setString(path, ip);
  }

  void intKey(const char* ns, const char* key, const char* path) {
    int64_t v;
    if (!readInt(ns, key, v)) return;
    setInt(path, v) ? imported() : rejected(ns, key);
  }

  void ipKey(const char* ns, const char* key, const char* path) {
    int64_t v;
    if (!readInt(ns, key, v)) return;
    setIp(path, v) ? imported() : rejected(ns, key);
  }

  void stringKey(const char* ns, const char* key, const char* path) {
    char s[kStrBuf];
    if (!readString(ns, key, s)) return;
    setString(path, s) ? imported() : rejected(ns, key);
  }

  // Integer with a legacy "0 means default" quirk.
  void portKey(const char* ns, const char* key, const char* path, int64_t zeroMeans) {
    int64_t v;
    if (!readInt(ns, key, v)) return;
    setInt(path, v == 0 ? zeroMeans : v) ? imported() : rejected(ns, key);
  }

  // ------------------------------------------------------------ namespaces

  void importSys() { stringKey("sysCfg", "stName", "station"); }

  void importNet() {
    intKey("netCfg", "ethwifi", "net.iface");
    intKey("netCfg", "dhcp", "net.dhcp");
    ipKey("netCfg", "staticIp", "net.ip");
    ipKey("netCfg", "mask", "net.mask");
    ipKey("netCfg", "gw", "net.gateway");
    ipKey("netCfg", "dnsIp", "net.dns");
    stringKey("netCfg", "ssid", "net.ssid");
    stringKey("netCfg", "pwd", "net.wifiPassword");
    intKey("netCfg", "netConnTO", "net.reconnectTimeoutMin");
    stringKey("netCfg", "userName", "web.user");
    stringKey("netCfg", "userPwd", "web.password");
    stringKey("netCfg", "timeServer", "time.ntpServer");
    intKey("netCfg", "syslogEnable", "syslog.level");
    ipKey("netCfg", "sysLogIp", "syslog.server");
    portKey("netCfg", "sysLogPort", "syslog.port", 514);
  }

  void importTz() {
    stringKey("tZCfg", "tZ", "time.tzName");
    stringKey("tZCfg", "tZCode", "time.tzPosix");
  }

  void importProt() {
    int64_t dataProt;
    const bool haveProt = readInt("protCfg", "dataProt", dataProt);
    if (haveProt) setInt("mqtt.mode", dataProt) ? imported() : rejected("protCfg", "dataProt");

    int64_t v;
    if (readInt("protCfg", "brokerIp", v)) {
      bool ok = v == 0 ? setString("mqtt.host", "") : setIp("mqtt.host", v);
      ok ? imported() : rejected("protCfg", "brokerIp");
    }
    portKey("protCfg", "brokerPort", "mqtt.port", 1883);
    if (readInt("protCfg", "publishInterval", v)) {
      // The legacy firmware forced >= 2 s at runtime; clamp instead of reject.
      setInt("mqtt.publishIntervalS", v < 2 ? 2 : (v > 3600 ? 3600 : v));
      imported();
    }
    stringKey("protCfg", "brokerUser", "mqtt.user");
    stringKey("protCfg", "brokerPwd", "mqtt.password");
    intKey("protCfg", "brokerKAT", "mqtt.keepAliveS");
    intKey("protCfg", "brokerMD", "mqtt.minDelayS");

    int64_t pf;
    bool havePf = readInt("protCfg", "brokerPF", pf);
    if (havePf && (pf < 0 || pf > 0xFF)) {
      rejected("protCfg", "brokerPF");
      havePf = false;
    } else if (havePf) {
      imported();
    } else if (haveProt) {
      pf = 7;  // legacy readConfig() fallback when the key is missing
      havePf = true;
    }
    if (havePf) {
      MqttConfig& m = c_.mqtt;
      m.separate = (pf & 0x01) != 0;
      m.allTemps = (pf & 0x02) != 0;
      m.pathAsRoot = (pf & 0x04) != 0;
      m.upTime = (pf & 0x08) != 0;
      m.onChange = (pf & 0x10) != 0;
      m.retained = (pf & 0x20) != 0;
      m.plainText = (pf & 0x40) != 0;
      m.diag = (pf & 0x80) != 0;
    }

    if (readInt("protCfg", "brokerMQF", v)) {
      if (v < 0 || v > 0xFF) {
        rejected("protCfg", "brokerMQF");
      } else {
        c_.mqtt.germanDecimal = (v & 0x04) != 0;
        imported();
        if ((v & 0x03) != 0) ++r_.ignored;  // tValue / DS18 failsafes are dropped
      }
    }
  }

  // Legacy char[n] field: the text up to the first NUL; false when the
  // field has no NUL (strncpy of a too-long value).
  static bool fixedString(const uint8_t* p, size_t n, char* out) {
    const void* nul = memchr(p, 0, n);
    if (nul == nullptr) return false;
    const size_t len = static_cast<size_t>(static_cast<const uint8_t*>(nul) - p);
    memcpy(out, p, len);
    out[len] = '\0';
    return true;
  }

  static int32_t loadI32(const uint8_t* p) {
    return static_cast<int32_t>(static_cast<uint32_t>(p[0]) | (static_cast<uint32_t>(p[1]) << 8) |
                                (static_cast<uint32_t>(p[2]) << 16) |
                                (static_cast<uint32_t>(p[3]) << 24));
  }

  static float loadF32(const uint8_t* p) {
    const uint32_t bits = static_cast<uint32_t>(loadI32(p));
    float f;
    memcpy(&f, &bits, sizeof f);
    return f;
  }

  // Common part of every blob element: name[11]@0, active@11.
  void elemNameActive(const char* ns, const char* key, const char* group, size_t i,
                      const uint8_t* e) {
    char path[24];
    char name[kLegacyNameLen];
    snprintf(path, sizeof path, "%s.%u.name", group, static_cast<unsigned>(i + 1));
    if (!fixedString(e, kLegacyNameLen, name) || !setString(path, name)) {
      rejectedElem(ns, key, i, "name");
    }
    snprintf(path, sizeof path, "%s.%u.active", group, static_cast<unsigned>(i + 1));
    if (!setInt(path, e[11])) rejectedElem(ns, key, i, "active");
  }

  // ID[25]: "" or "00-..-00" = empty slot, else "hh-hh-hh-hh-hh-hh-hh-hh".
  void elemId(const char* ns, const char* key, const char* group, size_t i, const uint8_t* p) {
    char path[24];
    char id[kLegacyIdLen];
    snprintf(path, sizeof path, "%s.%u.id", group, static_cast<unsigned>(i + 1));
    if (!fixedString(p, kLegacyIdLen, id) || !setString(path, id)) rejectedElem(ns, key, i, "id");
  }

  void importValves() {
    uint8_t blob[kLegacyValvesBlob];
    if (readBlob("valvesCfg", "valves", blob, sizeof blob)) {
      imported();
      for (size_t i = 0; i < kValveCount; ++i) {
        elemNameActive("valvesCfg", "valves", "valves", i, blob + i * kValveElem);
      }
    }
    importCalib();
  }

  // The legacy web UI offered hour 0..24, and the legacy firmware fired when
  // tm_hour == hourOfCalib: 24 (or any larger stored value) meant "never".
  // Here that is dayMask 0; dayOfCalib is then irrelevant.
  void importCalib() {
    int64_t hour;
    if (readInt("valvesCfg", "hourOfCalib", hour) && hour >= 24 && hour <= UINT8_MAX) {
      setInt("calib.dayMask", 0) ? imported() : rejected("valvesCfg", "hourOfCalib");
      int64_t day;
      if (readInt("valvesCfg", "dayOfCalib", day)) imported();
      return;
    }
    intKey("valvesCfg", "dayOfCalib", "calib.dayMask");
    intKey("valvesCfg", "hourOfCalib", "calib.hour");
  }

  void importTemps() {
    static uint8_t blob[kLegacyTempsBlob];  // 1.5 KB: keep it off the caller's stack
    if (!readBlob("tempsCfg", "temps", blob, sizeof blob)) return;
    imported();
    for (size_t i = 0; i < kTempSlotCount; ++i) {
      const uint8_t* e = blob + i * kTempElem;
      elemNameActive("tempsCfg", "temps", "temps", i, e);
      int32_t off = loadI32(e + 12);
      if (off < -100 || off > 100) {
        rejectedElem("tempsCfg", "temps", i, "offset");
        off = off < 0 ? -100 : 100;
      }
      c_.temps[i].offset = static_cast<int16_t>(off);
      elemId("tempsCfg", "temps", "temps", i, e + 16);
    }
  }

  void importVolts() {
    uint8_t blob[kLegacyVoltsBlob];
    if (!readBlob("voltsCfg", "volts", blob, sizeof blob)) return;
    imported();
    for (size_t i = 0; i < kVoltSlotCount; ++i) {
      const uint8_t* e = blob + i * kVoltElem;
      char path[24];
      elemNameActive("voltsCfg", "volts", "volts", i, e);
      snprintf(path, sizeof path, "volts.%u.offset", static_cast<unsigned>(i + 1));
      if (!setFloat(path, loadF32(e + 12))) rejectedElem("voltsCfg", "volts", i, "offset");
      snprintf(path, sizeof path, "volts.%u.factor", static_cast<unsigned>(i + 1));
      if (!setFloat(path, loadF32(e + 16))) rejectedElem("voltsCfg", "volts", i, "factor");
      char unit[kLegacyUnitLen];
      snprintf(path, sizeof path, "volts.%u.unit", static_cast<unsigned>(i + 1));
      if (!fixedString(e + 20, kLegacyUnitLen, unit) || !setString(path, unit)) {
        rejectedElem("voltsCfg", "volts", i, "unit");
      }
      elemId("voltsCfg", "volts", "volts", i, e + 29);
    }
  }

  void importMisc() {
    int64_t v;
    if (!readInt("Misc", "MiscLC", v)) return;
    // The legacy firmware also stored the unsynced clock (1970); drop that.
    if (v >= kEpoch2020 && v < kEpoch2100) {
      r_.lastCalibEpoch = v;
      imported();
    } else {
      rejected("Misc", "MiscLC");
    }
  }

  void countDropped() {
    for (const DroppedKey& k : kDropped) {
      int64_t i;
      char s[kStrBuf];
      bool truncated;
      uint8_t b[1];
      size_t stored;
      if (nvs_.readInt(k.ns, k.key, i) || nvs_.readString(k.ns, k.key, s, sizeof s, truncated) ||
          nvs_.readBlob(k.ns, k.key, b, sizeof b, stored)) {
        r_.anyLegacy = true;
        ++r_.ignored;
      }
    }
  }

  // ------------------------------------------------------------ cross-field rules

  // Clears the optional feature that breaks a rule, so the result always
  // passes validateConfig(). Order matches validateConfig().
  void fixCrossFieldRules() {
    NetConfig& n = c_.net;
    if (!n.dhcp && (n.ip == 0 || n.mask == 0 || n.gateway == 0)) {
      n.dhcp = true;  // an incomplete static setup would leave the device unreachable
      rejected("netCfg", "dhcp");
    }
    const size_t pwdLen = strlen(n.wifiPassword);
    if (n.ssid[0] != '\0' && pwdLen > 0 && pwdLen < 8) {  // "" = open network
      n.ssid[0] = '\0';
      n.wifiPassword[0] = '\0';
      rejected("netCfg", "pwd");
    }
    if (n.iface == NetInterface::Wifi && n.ssid[0] == '\0') {
      n.iface = NetInterface::Auto;
      rejected("netCfg", "ethwifi");
    }
    if (c_.syslog.level > 0 && c_.syslog.server == 0) {
      c_.syslog.level = 0;
      rejected("netCfg", "syslogEnable");
    }
    WebConfig& w = c_.web;
    if ((w.user[0] == '\0') != (w.password[0] == '\0')) {
      rejected("netCfg", w.user[0] == '\0' ? "userName" : "userPwd");
      w.user[0] = '\0';
      w.password[0] = '\0';
    }
    MqttConfig& m = c_.mqtt;
    if (m.mode != MqttMode::Off && m.host[0] == '\0') {
      m.mode = MqttMode::Off;
      rejected("protCfg", "brokerIp");
    }
    if (m.minDelayS > m.publishIntervalS) {
      m.minDelayS = m.publishIntervalS;
      rejected("protCfg", "brokerMD");
    }
    if (m.mode == MqttMode::MqttHa && !m.separate) {
      m.mode = MqttMode::Mqtt;
      rejected("protCfg", "dataProt");
    }
    // Home Assistant reads a decimal point (legacy HA mode allowed the comma).
    if (m.mode == MqttMode::MqttHa && m.germanDecimal) {
      m.germanDecimal = false;
      rejected("protCfg", "brokerMQF");
    }
    fixValveNames();
    fixSlots();

    // Safety net: never hand out a config that does not validate.
    char path[40];
    if (!validateConfig(c_, path, sizeof path)) {
      setDefaults(c_);
      rejected("validate", path);
    }
  }

  using HaId = char[kItemNameMax + 1];

  // buildHaId() of item i's MQTT segment.
  void haId(ItemKind kind, uint8_t i, HaId& out) const {
    HaId seg;
    buildHaId(seg, itemSegment(c_, kind, i, seg, sizeof seg), out, sizeof out);
  }

  // One HA id for two items (also true for one MQTT segment: "a b"/"a_b").
  bool sameHaId(ItemKind kind, uint8_t a, uint8_t b) const {
    HaId ida;
    HaId idb;
    haId(kind, a, ida);
    haId(kind, b, idb);
    return strcmp(ida, idb) == 0;
  }

  // "1".."12" without leading zero -> 1..12, else 0.
  static uint8_t numberSegment(const char* s) {
    uint32_t v;
    if (s[0] == '0' || !parseUint(s, strlen(s), kValveCount, v)) return 0;
    return static_cast<uint8_t>(v);
  }

  // Later duplicates (one MQTT segment or one HA id: "Bad 1"/"Bad.1") lose
  // their name; a name that equals the number segment of another unnamed
  // valve is cleared too. Clearing can create a new number collision, so
  // repeat until stable (at most 12 rounds).
  void fixValveNames() {
    for (bool changed = true; changed;) {
      changed = false;
      for (uint8_t i = 0; i < kValveCount && !changed; ++i) {
        char* name = c_.valves[i].name;
        if (name[0] == '\0') continue;
        bool clash = false;
        for (uint8_t j = 0; j < i && !clash; ++j) clash = sameHaId(ItemKind::Valve, i, j);
        const uint8_t num = numberSegment(name);
        if (num != 0 && c_.valves[num - 1].name[0] == '\0') clash = true;
        if (clash) {
          name[0] = '\0';
          rejectedElem("valvesCfg", "valves", i, "name");
          changed = true;
        }
      }
    }
  }

  template <typename Slot>
  void fixSlotArray(Slot* slots, uint8_t count, const char* ns, const char* key) {
    for (uint8_t i = 0; i < count; ++i) {
      Slot& s = slots[i];
      if (!isZero(s.id)) {
        for (uint8_t j = 0; j < i; ++j) {
          if (slots[j].id == s.id) {
            s.id = OneWireId{};
            rejectedElem(ns, key, i, "id");
            break;
          }
        }
      }
      if (s.active && isZero(s.id)) {
        s.active = false;
        rejectedElem(ns, key, i, "active");
      }
    }
  }

  // Active slots (each has an id by now) with one HA id: the later slot loses
  // its name; when it has none (its segment is its number), the earlier slot
  // named like that number does. Clearing can create a new clash, so repeat
  // until stable (every round clears one name).
  template <typename Slot>
  void fixSlotHaIds(Slot* slots, uint8_t count, ItemKind kind, const char* ns, const char* key) {
    for (bool changed = true; changed;) {
      changed = false;
      for (uint8_t i = 0; i < count && !changed; ++i) {
        for (uint8_t j = 0; j < i && !changed; ++j) {
          if (!slots[i].active || !slots[j].active || !sameHaId(kind, i, j)) continue;
          const uint8_t k = slots[i].name[0] != '\0' ? i : j;
          slots[k].name[0] = '\0';
          rejectedElem(ns, key, k, "name");
          changed = true;
        }
      }
    }
  }

  void fixSlots() {
    fixSlotArray(c_.temps, kTempSlotCount, "tempsCfg", "temps");
    fixSlotArray(c_.volts, kVoltSlotCount, "voltsCfg", "volts");
    fixSlotHaIds(c_.temps, kTempSlotCount, ItemKind::Temp, "tempsCfg", "temps");
    fixSlotHaIds(c_.volts, kVoltSlotCount, ItemKind::Volt, "voltsCfg", "volts");
  }

  LegacyNvsReader& nvs_;
  Config& c_;
  ImportReport& r_;
};

}  // namespace

ImportReport importLegacyConfig(LegacyNvsReader& nvs, Config& out) {
  ImportReport report;
  setDefaults(out);
  Importer(nvs, out, report).run();
  return report;
}

}  // namespace vdm
