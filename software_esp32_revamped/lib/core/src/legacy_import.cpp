#include "vdm/legacy_import.h"

#include <stdio.h>
#include <string.h>

#include "vdm/json_writer.h"

namespace vdm {

namespace {

constexpr int64_t kEpoch2020 = 1577836800;  // 2020-01-01T00:00:00Z
constexpr int64_t kEpoch2100 = 4102444800;  // sanity bound for a stored time_t

// Legacy blob element layouts (Xtensa GCC, DESIGN.md "Legacy import").
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
static_assert(kVoltElem * kVoltSlotCount == kLegacyVoltsBlob140, "1.4.0 volts blob layout");

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
    importValvesCtrl();
    importMessenger();
    countDropped();
    repair();
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

  void importSys() {
    char s[kStrBuf];
    if (!readString("sysCfg", "stName", s)) return;
    if (s[0] == '\0') {
      // The legacy firmware published under "VdMotFBH/" without a name.
      setString("mqtt.rootTopic", "VdMotFBH");
      imported();
      return;
    }
    setString("station", s) ? imported() : rejected("sysCfg", "stName");
  }

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
    int64_t level;
    if (readInt("netCfg", "syslogEnable", level)) {
      // 1..3 were debug verbosity levels: everything is sent.
      r_.syslogDebug = level >= 1 && level <= 3;
      const bool ok = (level == 0 || r_.syslogDebug) && setInt("syslog.level", level == 0 ? 0 : 3);
      ok ? imported() : rejected("netCfg", "syslogEnable");
    }
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
        if ((v & 0x01) != 0) r_.dropped |= kDroppedLegacyFailsafe;
        if ((v & 0x02) != 0) r_.dropped |= kDroppedDs18Timeout;
        r_.legacyFailsafeEnabled = (v & 0x01) != 0;
      }
    }
    // Reported only: the new failsafe keeps its defaults (60 min / 50 %).
    int64_t timeout;
    int64_t pct;
    const bool haveTimeout = nvs_.readInt("protCfg", "brokerMQTO", timeout);
    const bool havePct = nvs_.readInt("protCfg", "brokerMQToPos", pct);
    r_.legacyFailsafeValid = haveTimeout || havePct;
    if (haveTimeout) r_.legacyFailsafeTimeoutMin = static_cast<int32_t>(timeout);
    if (havePct) r_.legacyFailsafePct = static_cast<int32_t>(pct);
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

  void markRenamed(ItemKind kind, size_t i) {
    if (kind == ItemKind::Valve) {
      r_.renamedValves = static_cast<uint16_t>(r_.renamedValves | (1u << i));
    } else if (kind == ItemKind::Temp) {
      r_.renamedTemps |= 1ull << i;
    } else {
      r_.renamedVolts = static_cast<uint8_t>(r_.renamedVolts | (1u << i));
    }
  }

  // A printable legacy name with characters MQTT topics cannot carry: they
  // become '_'; a name with '/', '"' or '\' (and no wildcard) keeps its
  // legacy topic segment as the item's override. False when even the
  // replaced name is not valid.
  bool renameItem(const char* group, ItemKind kind, size_t i, const char* name) {
    const size_t len = strlen(name);
    if (!isPrintableText(name, len)) return false;
    char safe[kLegacyNameLen];
    char topic[kLegacyNameLen];
    bool keepTopic = true;
    for (size_t k = 0; k <= len; ++k) {
      const char ch = name[k];
      const bool wildcard = ch == '+' || ch == '#';
      keepTopic = keepTopic && !wildcard;
      safe[k] = wildcard || ch == '/' || ch == '"' || ch == '\\' ? '_' : ch;
      topic[k] = ch == ' ' ? '_' : ch;
    }
    char path[24];
    snprintf(path, sizeof path, "%s.%u.name", group, static_cast<unsigned>(i + 1));
    if (!setString(path, safe)) return false;
    markRenamed(kind, i);
    if (keepTopic && strpbrk(name, "/\"\\") != nullptr) {
      snprintf(path, sizeof path, "%s.%u.topic", group, static_cast<unsigned>(i + 1));
      setString(path, topic);  // not a valid segment ("/Bad"): no override
    }
    return true;
  }

  // Common part of every blob element: name[11]@0, active@11.
  void elemNameActive(const char* ns, const char* key, const char* group, ItemKind kind, size_t i,
                      const uint8_t* e) {
    char path[24];
    char name[kLegacyNameLen];
    snprintf(path, sizeof path, "%s.%u.name", group, static_cast<unsigned>(i + 1));
    if (!fixedString(e, kLegacyNameLen, name) ||
        (!setString(path, name) && !renameItem(group, kind, i, name))) {
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
        elemNameActive("valvesCfg", "valves", "valves", ItemKind::Valve, i, blob + i * kValveElem);
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
      elemNameActive("tempsCfg", "temps", "temps", ItemKind::Temp, i, e);
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
    size_t stored = 0;
    if (!nvs_.readBlob("voltsCfg", "volts", blob, sizeof blob, stored)) return;
    r_.anyLegacy = true;
    // 1.4.0 wrote the 8 elements without the tail of 1.4.1.
    if (stored != kLegacyVoltsBlob && stored != kLegacyVoltsBlob140) {
      rejected("voltsCfg", "volts");
      return;
    }
    r_.voltsBlob448 = stored == kLegacyVoltsBlob140;
    imported();
    for (size_t i = 0; i < kVoltSlotCount; ++i) {
      const uint8_t* e = blob + i * kVoltElem;
      char path[24];
      elemNameActive("voltsCfg", "volts", "volts", ItemKind::Volt, i, e);
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

  // valvesCtrl (PI control, window contacts): only counted for the report.
  // Every legacy version starts its element with controlFlags; the element
  // size grew over the versions, the count is always 12.
  void importValvesCtrl() {
    static uint8_t blob[kLegacyValvesCtrlMax];
    size_t stored = 0;
    if (!nvs_.readBlob("valvesCtrlCfg", "valvesCtrl", blob, sizeof blob, stored)) return;
    if (stored % kValveCount != 0 || stored < kValveCount || stored > sizeof blob) {
      rejected("valvesCtrlCfg", "valvesCtrl");
      return;
    }
    const size_t elem = stored / kValveCount;
    for (size_t i = 0; i < kValveCount; ++i) {
      const uint8_t flags = blob[i * elem];
      if ((flags & 0x01) != 0) ++r_.piValves;
      if ((flags & 0x10) != 0) ++r_.windowValves;
    }
    if (r_.piValves != 0) r_.dropped |= kDroppedPi;
    if (r_.windowValves != 0) r_.dropped |= kDroppedWindow;
  }

  void importMessenger() {
    int64_t flags;
    if (nvs_.readInt("msgCfg", "msgFlags", flags) && (flags & 0x03) != 0) {
      r_.dropped |= kDroppedMessenger;  // bit0 PushOver, bit1 e-mail
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

  using Name = char[kItemNameMax + 1];

  // sanitizeConfig() makes the result valid; every repair is reported under
  // the legacy key it came from, in the order of the rules.
  void repair() {
    static Name temps[kTempSlotCount];  // 374 bytes: off the caller's stack
    Name volts[kVoltSlotCount];
    for (uint8_t i = 0; i < kTempSlotCount; ++i) memcpy(temps[i], c_.temps[i].name, sizeof temps[i]);
    for (uint8_t i = 0; i < kVoltSlotCount; ++i) memcpy(volts[i], c_.volts[i].name, sizeof volts[i]);
    Repairs r;
    sanitizeConfig(c_, &r);
    struct Bit {
      uint32_t bit;
      const char* ns;
      const char* key;
    };
    static const Bit kKeys[] = {
        {kRepairStaticIp, "netCfg", "dhcp"},         {kRepairWifiPassword, "netCfg", "pwd"},
        {kRepairWifiIface, "netCfg", "ethwifi"},     {kRepairSyslog, "netCfg", "syslogEnable"},
        {kRepairWebNoPassword, "netCfg", "userPwd"}, {kRepairWebNoUser, "netCfg", "userName"},
        {kRepairMqttHost, "protCfg", "brokerIp"},    {kRepairMinDelay, "protCfg", "brokerMD"},
        {kRepairHaSeparate, "protCfg", "dataProt"},  {kRepairHaDecimal, "protCfg", "brokerMQF"},
    };
    for (const Bit& b : kKeys) {
      if ((r.mask & b.bit) != 0) rejected(b.ns, b.key);
    }
    for (uint8_t i = 0; i < kValveCount; ++i) {
      if ((r.valveNames >> i) & 1u) rejectedElem("valvesCfg", "valves", i, "name");
      if ((r.valveTopics >> i) & 1u) rejectedElem("valvesCfg", "valves", i, "topic");
      if (((r.valveNames | r.valveTopics) >> i) & 1u) markRenamed(ItemKind::Valve, i);
    }
    slotRepairs(c_.temps, kTempSlotCount, temps, r.tempIds, r.tempActive, ItemKind::Temp,
                "tempsCfg", "temps");
    slotRepairs(c_.volts, kVoltSlotCount, volts, r.voltIds, r.voltActive, ItemKind::Volt,
                "voltsCfg", "volts");
  }

  // Ids and active flags per slot, then the names cleared for one HA id.
  template <typename Slot, typename Bits>
  void slotRepairs(const Slot* slots, uint8_t count, const Name* before, Bits ids, Bits active,
                   ItemKind kind, const char* ns, const char* key) {
    for (uint8_t i = 0; i < count; ++i) {
      if ((ids >> i) & 1u) rejectedElem(ns, key, i, "id");
      if ((active >> i) & 1u) rejectedElem(ns, key, i, "active");
    }
    for (uint8_t i = 0; i < count; ++i) {
      if (strcmp(before[i], slots[i].name) == 0) continue;
      rejectedElem(ns, key, i, "name");
      markRenamed(kind, i);
    }
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

namespace {

const char* const kDroppedNames[] = {"pi", "window", "messenger", "ds18Timeout", "legacyFailsafe"};

template <typename Bits, typename Item>
void writeRenamed(JsonWriter& jw, Bits bits, const Item* items, uint8_t count, const char* kind) {
  for (uint8_t i = 0; i < count; ++i) {
    if (((bits >> i) & 1u) == 0) continue;
    jw.beginObject();
    jw.kv("kind", kind);
    jw.kv("n", static_cast<uint32_t>(i + 1u));
    jw.kv("name", items[i].name);
    jw.kv("topic", items[i].topic);
    jw.endObject();
  }
}

}  // namespace

bool writeImportReportJson(JsonWriter& jw, const ImportReport& r, const Config& c) {
  jw.beginObject();
  jw.kv("imported", static_cast<uint32_t>(r.imported));
  jw.kv("rejected", static_cast<uint32_t>(r.rejected));
  jw.kv("ignored", static_cast<uint32_t>(r.ignored));
  jw.kv("firstRejected", r.firstRejected);
  jw.kv("piValves", static_cast<uint32_t>(r.piValves));
  jw.kv("windowValves", static_cast<uint32_t>(r.windowValves));
  jw.key("dropped");
  jw.beginArray();
  for (size_t i = 0; i < sizeof kDroppedNames / sizeof *kDroppedNames; ++i) {
    if ((r.dropped >> i) & 1u) jw.value(kDroppedNames[i]);
  }
  jw.endArray();
  jw.key("legacyFailsafe");
  if (r.legacyFailsafeValid) {
    jw.beginObject();
    jw.kv("enabled", r.legacyFailsafeEnabled);
    jw.kv("timeoutMin", r.legacyFailsafeTimeoutMin);
    jw.kv("pct", r.legacyFailsafePct);
    jw.endObject();
  } else {
    jw.nullValue();
  }
  jw.key("rootTopic");
  if (c.mqtt.rootTopic[0] != '\0') {
    jw.value(c.mqtt.rootTopic);
  } else {
    jw.nullValue();
  }
  jw.key("renamed");
  jw.beginArray();
  writeRenamed(jw, r.renamedValves, c.valves, kValveCount, "valve");
  writeRenamed(jw, r.renamedTemps, c.temps, kTempSlotCount, "temp");
  writeRenamed(jw, r.renamedVolts, c.volts, kVoltSlotCount, "volt");
  jw.endArray();
  jw.kv("syslogDebug", r.syslogDebug);
  jw.kv("voltsBlob448", r.voltsBlob448);
  jw.endObject();
  return jw.ok();
}

}  // namespace vdm
