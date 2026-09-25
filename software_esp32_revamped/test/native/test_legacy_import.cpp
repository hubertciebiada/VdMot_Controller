// legacy_import: every legacy NVS key of specs/04 §5 with its quirks,
// blob layouts, per-key rejection, cross-field repair, dropped keys,
// idempotence, and a fuzz run that the result always validates.
#include <math.h>
#include <stdint.h>
#include <string.h>

#include <map>
#include <random>
#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/config.h"
#include "vdm/legacy_import.h"

using namespace vdm;

namespace {

// In-memory NVS with typed entries (a key has exactly one type).
class FakeNvs : public LegacyNvsReader {
 public:
  struct Entry {
    enum Kind { Int, Str, Blob } kind = Int;
    int64_t i = 0;
    std::string s;
    std::vector<uint8_t> b;
  };

  void putInt(const char* ns, const char* key, int64_t v) {
    Entry e;
    e.kind = Entry::Int;
    e.i = v;
    map_[k(ns, key)] = e;
  }
  void putStr(const char* ns, const char* key, const std::string& v) {
    Entry e;
    e.kind = Entry::Str;
    e.s = v;
    map_[k(ns, key)] = e;
  }
  void putBlob(const char* ns, const char* key, const std::vector<uint8_t>& v) {
    Entry e;
    e.kind = Entry::Blob;
    e.b = v;
    map_[k(ns, key)] = e;
  }
  void erase(const char* ns, const char* key) { map_.erase(k(ns, key)); }

  bool readInt(const char* ns, const char* key, int64_t& out) override {
    ++reads;
    const Entry* e = find(ns, key, Entry::Int);
    if (!e) return false;
    out = e->i;
    return true;
  }
  bool readString(const char* ns, const char* key, char* out, size_t cap,
                  bool& truncated) override {
    ++reads;
    const Entry* e = find(ns, key, Entry::Str);
    if (!e || cap == 0) return false;
    const size_t n = e->s.size() < cap - 1 ? e->s.size() : cap - 1;
    if (n) memcpy(out, e->s.data(), n);
    out[n] = '\0';
    truncated = e->s.size() > cap - 1;
    return true;
  }
  bool readBlob(const char* ns, const char* key, uint8_t* out, size_t cap,
                size_t& storedLen) override {
    ++reads;
    const Entry* e = find(ns, key, Entry::Blob);
    if (!e) return false;
    storedLen = e->b.size();
    const size_t n = e->b.size() < cap ? e->b.size() : cap;
    if (n) memcpy(out, e->b.data(), n);
    return true;
  }

  int reads = 0;

 private:
  static std::string k(const char* ns, const char* key) { return std::string(ns) + "/" + key; }
  const Entry* find(const char* ns, const char* key, Entry::Kind kind) const {
    auto it = map_.find(k(ns, key));
    return (it == map_.end() || it->second.kind != kind) ? nullptr : &it->second;
  }
  std::map<std::string, Entry> map_;
};

void putStrField(std::vector<uint8_t>& b, size_t at, size_t n, const char* s) {
  memset(&b[at], 0, n);
  memcpy(&b[at], s, strlen(s) < n ? strlen(s) : n);
}
void putI32(std::vector<uint8_t>& b, size_t at, int32_t v) {
  for (int i = 0; i < 4; ++i) b[at + i] = static_cast<uint8_t>(static_cast<uint32_t>(v) >> (8 * i));
}
void putF32(std::vector<uint8_t>& b, size_t at, float f) {
  uint32_t u;
  memcpy(&u, &f, 4);
  putI32(b, at, static_cast<int32_t>(u));
}

std::vector<uint8_t> valvesBlob() { return std::vector<uint8_t>(kLegacyValvesBlob, 0); }
void setValve(std::vector<uint8_t>& b, size_t i, const char* name, uint8_t active) {
  putStrField(b, i * 12, 11, name);
  b[i * 12 + 11] = active;
}

std::vector<uint8_t> tempsBlob() { return std::vector<uint8_t>(kLegacyTempsBlob, 0); }
void setTemp(std::vector<uint8_t>& b, size_t i, const char* name, uint8_t active, int32_t off,
             const char* id) {
  putStrField(b, i * 44, 11, name);
  b[i * 44 + 11] = active;
  putI32(b, i * 44 + 12, off);
  putStrField(b, i * 44 + 16, 25, id);
}

std::vector<uint8_t> voltsBlob() {
  std::vector<uint8_t> b(kLegacyVoltsBlob, 0);
  for (size_t i = 0; i < 8; ++i) putF32(b, i * 56 + 16, 1.0f);
  return b;
}
void setVolt(std::vector<uint8_t>& b, size_t i, const char* name, uint8_t active, float off,
             float factor, const char* unit, const char* id) {
  putStrField(b, i * 56, 11, name);
  b[i * 56 + 11] = active;
  putF32(b, i * 56 + 12, off);
  putF32(b, i * 56 + 16, factor);
  putStrField(b, i * 56 + 20, 9, unit);
  putStrField(b, i * 56 + 29, 25, id);
}

const char* const kIdA = "28-84-37-94-97-ff-03-23";
const char* const kIdB = "28-aa-bb-cc-dd-ee-01-67";
const char* const kIdV = "26-11-22-33-44-55-66-29";

OneWireId oid(const char* s) {
  OneWireId v;
  REQUIRE(parseOneWireId(s, strlen(s), v));
  return v;
}

bool valid(const Config& c) {
  char path[64];
  const bool ok = validateConfig(c, path, sizeof path);
  if (!ok) MESSAGE("invalid at ", path);
  return ok;
}

bool sameConfig(const Config& a, const Config& b) {
  uint8_t ba[kConfigBlobMax], bb[kConfigBlobMax];
  const size_t na = encodeConfig(a, ba, sizeof ba);
  const size_t nb = encodeConfig(b, bb, sizeof bb);
  return na > 0 && na == nb && memcmp(ba, bb, na) == 0;
}

std::string first(const ImportReport& r) { return r.firstRejected; }

// A typical 1.4.x device: Ethernet + DHCP, MQTT + HA, three valves, two
// sensors, one volt sensor.
FakeNvs typicalDevice() {
  FakeNvs n;
  n.putInt("sysCfg", "CF", 0);
  n.putStr("sysCfg", "stName", "VdMotFBH");
  n.putInt("netCfg", "ethwifi", 1);
  n.putInt("netCfg", "dhcp", 1);
  n.putInt("netCfg", "staticIp", 0);
  n.putInt("netCfg", "mask", 0);
  n.putInt("netCfg", "gw", 0);
  n.putInt("netCfg", "dnsIp", 0);
  n.putStr("netCfg", "ssid", "");
  n.putStr("netCfg", "pwd", "");
  n.putStr("netCfg", "userName", "");
  n.putStr("netCfg", "userPwd", "");
  n.putStr("netCfg", "timeServer", "pool.ntp.org");
  n.putInt("netCfg", "syslogEnable", 0);
  n.putInt("netCfg", "sysLogIp", 0);
  n.putInt("netCfg", "sysLogPort", 0);
  n.putInt("netCfg", "netConnTO", 5);
  n.putStr("tZCfg", "tZ", "Europe/Warsaw");
  n.putStr("tZCfg", "tZCode", "CET-1CEST,M3.5.0,M10.5.0/3");
  n.putInt("protCfg", "dataProt", 2);
  n.putInt("protCfg", "brokerIp", 0x6401A8C0);  // 192.168.1.100
  n.putInt("protCfg", "brokerPort", 1883);
  n.putInt("protCfg", "brokerInterval", 1000);
  n.putInt("protCfg", "publishInterval", 30);
  n.putStr("protCfg", "brokerUser", "mqtt");
  n.putStr("protCfg", "brokerPwd", "secret");
  n.putInt("protCfg", "brokerPF", 0x7B);  // separate, allTemps, upTime, onChange, retained, plainText
  n.putInt("protCfg", "brokerKAT", 60);
  n.putInt("protCfg", "brokerMD", 5);
  n.putInt("protCfg", "brokerMQF", 0);
  n.putInt("protCfg", "brokerMQTO", 120);
  n.putInt("protCfg", "brokerMQToPos", 10);
  auto v = valvesBlob();
  setValve(v, 0, "Bad", 1);
  setValve(v, 1, "Kueche", 1);
  setValve(v, 2, "", 1);
  n.putBlob("valvesCfg", "valves", v);
  n.putInt("valvesCfg", "dayOfCalib", 9);
  n.putInt("valvesCfg", "hourOfCalib", 3);
  auto t = tempsBlob();
  setTemp(t, 0, "Flur", 1, -7, kIdA);
  setTemp(t, 5, "Aussen", 1, 12, "28-AA-BB-CC-DD-EE-01-67");
  setTemp(t, 6, "", 0, 0, "00-00-00-00-00-00-00-00");
  n.putBlob("tempsCfg", "temps", t);
  auto w = voltsBlob();
  setVolt(w, 0, "Akku", 1, 0.5f, 0.01f, "V", kIdV);
  n.putBlob("voltsCfg", "volts", w);
  n.putInt("Misc", "MiscLC", 1760000000);
  return n;
}

// typicalDevice() with a static address, which a reset to the defaults
// would lose.
FakeNvs staticDevice() {
  FakeNvs n = typicalDevice();
  n.putInt("netCfg", "dhcp", 0);
  n.putInt("netCfg", "staticIp", 0x3201A8C0);  // 192.168.1.50
  n.putInt("netCfg", "mask", 0x00FFFFFF);
  n.putInt("netCfg", "gw", 0x0101A8C0);
  return n;
}

// Station, network and MQTT of staticDevice() came through.
void checkKept(const Config& c) {
  CHECK(std::string(c.station) == "VdMotFBH");
  CHECK_FALSE(c.net.dhcp);
  CHECK(c.net.ip == 0x3201A8C0u);
  CHECK(c.net.gateway == 0x0101A8C0u);
  CHECK(c.mqtt.mode == MqttMode::MqttHa);
  CHECK(std::string(c.mqtt.host) == "192.168.1.100");
  CHECK(std::string(c.mqtt.password) == "secret");
  CHECK(std::string(c.time.tzName) == "Europe/Warsaw");
  CHECK(valid(c));
}

}  // namespace

TEST_CASE("legacy: empty NVS gives the defaults and no legacy flag") {
  FakeNvs n;
  Config c;
  c.calib.hour = 9;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK_FALSE(r.anyLegacy);
  CHECK(r.imported == 0);
  CHECK(r.rejected == 0);
  CHECK(r.ignored == 0);
  CHECK(first(r).empty());
  CHECK(r.lastCalibEpoch == 0);
  CHECK(sameConfig(c, Config{}));
  CHECK(n.reads > 0);
}

TEST_CASE("legacy: a typical device imports completely") {
  FakeNvs n = typicalDevice();
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK(r.anyLegacy);
  CHECK(r.rejected == 0);
  CHECK(first(r).empty());
  // stName + 16 netCfg + 2 tZCfg + dataProt brokerIp brokerPort
  // publishInterval brokerUser brokerPwd brokerPF brokerKAT brokerMD brokerMQF
  // + valves dayOfCalib hourOfCalib + temps + volts + MiscLC
  CHECK(r.imported == 1 + 15 + 2 + 10 + 3 + 1 + 1 + 1);
  CHECK(r.ignored == 4);  // CF, brokerInterval, brokerMQTO, brokerMQToPos
  CHECK(r.lastCalibEpoch == 1760000000);
  CHECK(valid(c));

  CHECK(std::string(c.station) == "VdMotFBH");
  CHECK(c.net.iface == NetInterface::Ethernet);
  CHECK(c.net.dhcp);
  CHECK(c.net.reconnectTimeoutMin == 5);
  CHECK(std::string(c.time.ntpServer) == "pool.ntp.org");
  CHECK(std::string(c.time.tzName) == "Europe/Warsaw");
  CHECK(std::string(c.time.tzPosix) == "CET-1CEST,M3.5.0,M10.5.0/3");
  CHECK(c.syslog.level == 0);
  CHECK(c.syslog.port == 514);  // 0 -> 514
  CHECK(c.mqtt.mode == MqttMode::MqttHa);
  CHECK(std::string(c.mqtt.host) == "192.168.1.100");
  CHECK(c.mqtt.port == 1883);
  CHECK(c.mqtt.publishIntervalS == 30);
  CHECK(std::string(c.mqtt.user) == "mqtt");
  CHECK(std::string(c.mqtt.password) == "secret");
  CHECK(c.mqtt.keepAliveS == 60);
  CHECK(c.mqtt.minDelayS == 5);
  CHECK(c.mqtt.separate);
  CHECK(c.mqtt.allTemps);
  CHECK_FALSE(c.mqtt.pathAsRoot);
  CHECK(c.mqtt.upTime);
  CHECK(c.mqtt.onChange);
  CHECK(c.mqtt.retained);
  CHECK(c.mqtt.plainText);
  CHECK_FALSE(c.mqtt.diag);
  CHECK_FALSE(c.mqtt.germanDecimal);
  CHECK(c.mqtt.newDiag);  // new keys keep their defaults
  CHECK(c.mqtt.events);
  CHECK(std::string(c.valves[0].name) == "Bad");
  CHECK(c.valves[0].active);
  CHECK(std::string(c.valves[1].name) == "Kueche");
  CHECK(c.valves[2].active);
  CHECK_FALSE(c.valves[3].active);
  CHECK(c.calib.dayMask == 9);
  CHECK(c.calib.hour == 3);
  CHECK(c.calib.minute == 0);
  CHECK(std::string(c.temps[0].name) == "Flur");
  CHECK(c.temps[0].active);
  CHECK(c.temps[0].offset == -7);
  CHECK(c.temps[0].id == oid(kIdA));
  CHECK(c.temps[5].id == oid(kIdB));
  CHECK(c.temps[5].offset == 12);
  CHECK(isZero(c.temps[6].id));
  CHECK(std::string(c.volts[0].name) == "Akku");
  CHECK(c.volts[0].active);
  CHECK(c.volts[0].offset == 0.5f);
  CHECK(c.volts[0].factor == 0.01f);
  CHECK(std::string(c.volts[0].unit) == "V");
  CHECK(c.volts[0].id == oid(kIdV));
  CHECK(c.volts[1].factor == 1.0f);
}

TEST_CASE("legacy: import is idempotent and read-only") {
  FakeNvs n = typicalDevice();
  Config a, b;
  const ImportReport ra = importLegacyConfig(n, a);
  const ImportReport rb = importLegacyConfig(n, b);
  CHECK(sameConfig(a, b));
  CHECK(ra.imported == rb.imported);
  CHECK(ra.rejected == rb.rejected);
  CHECK(ra.ignored == rb.ignored);
  // The output is rebuilt from defaults, not merged into what was there.
  Config dirty;
  strcpy(dirty.valves[9].name, "junk");
  dirty.calib.minute = 30;
  importLegacyConfig(n, dirty);
  CHECK(sameConfig(dirty, a));
}

TEST_CASE("legacy: network keys and their quirks") {
  SUBCASE("static IP") {
    FakeNvs n;
    n.putInt("netCfg", "dhcp", 0);
    n.putInt("netCfg", "staticIp", 0x3201A8C0);
    n.putInt("netCfg", "mask", 0x00FFFFFF);
    n.putInt("netCfg", "gw", 0x0101A8C0);
    n.putInt("netCfg", "dnsIp", 0x08080808);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(r.imported == 5);
    CHECK(r.rejected == 0);
    CHECK_FALSE(c.net.dhcp);
    CHECK(c.net.ip == 0x3201A8C0u);
    CHECK(c.net.mask == 0x00FFFFFFu);
    CHECK(c.net.gateway == 0x0101A8C0u);
    CHECK(c.net.dns == 0x08080808u);
    CHECK(valid(c));
  }
  SUBCASE("incomplete static IP falls back to DHCP") {
    FakeNvs n;
    n.putInt("netCfg", "dhcp", 0);
    n.putInt("netCfg", "staticIp", 0x3201A8C0);
    n.putInt("netCfg", "mask", 0x00FFFFFF);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(c.net.dhcp);
    CHECK(c.net.ip == 0x3201A8C0u);
    CHECK(r.rejected == 1);
    CHECK(first(r) == "netCfg/dhcp");
    CHECK(valid(c));
  }
  SUBCASE("non-contiguous mask and bad integers are rejected") {
    FakeNvs n;
    n.putInt("netCfg", "mask", 0x00FF00FF);
    n.putInt("netCfg", "staticIp", -1);
    n.putInt("netCfg", "gw", 0x100000000ll);
    n.putInt("netCfg", "dhcp", 2);
    n.putInt("netCfg", "ethwifi", 3);
    n.putInt("netCfg", "netConnTO", 241);
    n.putInt("netCfg", "syslogEnable", 4);
    n.putInt("netCfg", "sysLogPort", 65536);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(r.imported == 0);
    CHECK(r.rejected == 8);
    CHECK(first(r) == "netCfg/ethwifi");
    CHECK(sameConfig(c, Config{}));
  }
  SUBCASE("boundaries that are accepted") {
    FakeNvs n;
    n.putInt("netCfg", "ethwifi", 2);
    n.putStr("netCfg", "ssid", std::string(32, 's'));
    n.putStr("netCfg", "pwd", std::string(63, 'p'));
    n.putInt("netCfg", "netConnTO", 240);
    n.putInt("netCfg", "syslogEnable", 3);
    n.putInt("netCfg", "sysLogIp", 0x0A00000A);
    n.putInt("netCfg", "sysLogPort", 65535);
    n.putInt("netCfg", "staticIp", 0xFFFFFFFFll);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(r.rejected == 0);
    CHECK(r.imported == 8);
    CHECK(c.net.iface == NetInterface::Wifi);
    CHECK(c.net.reconnectTimeoutMin == 240);
    CHECK(c.syslog.level == 3);
    CHECK(c.syslog.server == 0x0A00000Au);
    CHECK(c.syslog.port == 65535);
    CHECK(c.net.ip == 0xFFFFFFFFu);
    CHECK(valid(c));
  }
  SUBCASE("WiFi without a usable password is disabled") {
    FakeNvs n;
    n.putInt("netCfg", "ethwifi", 2);
    n.putStr("netCfg", "ssid", "home");
    n.putStr("netCfg", "pwd", "short");
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(std::string(c.net.ssid).empty());
    CHECK(std::string(c.net.wifiPassword).empty());
    CHECK(c.net.iface == NetInterface::Auto);
    CHECK(r.imported == 3);
    CHECK(r.rejected == 2);
    CHECK(first(r) == "netCfg/pwd");
    CHECK(valid(c));
  }
  SUBCASE("WiFi-only without ssid becomes auto") {
    FakeNvs n;
    n.putInt("netCfg", "ethwifi", 2);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(c.net.iface == NetInterface::Auto);
    CHECK(first(r) == "netCfg/ethwifi");
  }
  SUBCASE("over-long strings (strncpy without NUL in the legacy UI)") {
    FakeNvs n;
    n.putStr("netCfg", "ssid", std::string(33, 's'));
    n.putStr("netCfg", "pwd", std::string(64, 'p'));
    n.putStr("netCfg", "userName", std::string(65, 'u'));
    n.putStr("netCfg", "userPwd", std::string(200, 'p'));
    n.putStr("netCfg", "timeServer", "bad host");
    n.putStr("sysCfg", "stName", std::string(21, 'x'));
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(r.rejected == 6);
    CHECK(r.imported == 0);
    CHECK(first(r) == "sysCfg/stName");
    CHECK(sameConfig(c, Config{}));
  }
  SUBCASE("time server may be empty or an address") {
    FakeNvs n;
    n.putStr("netCfg", "timeServer", "");
    Config c;
    importLegacyConfig(n, c);
    CHECK(std::string(c.time.ntpServer).empty());
    n.putStr("netCfg", "timeServer", "192.168.1.1");
    importLegacyConfig(n, c);
    CHECK(std::string(c.time.ntpServer) == "192.168.1.1");
  }
  SUBCASE("syslog without server is switched off") {
    FakeNvs n;
    n.putInt("netCfg", "syslogEnable", 2);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(c.syslog.level == 0);
    CHECK(first(r) == "netCfg/syslogEnable");
    CHECK(r.imported == 1);
    CHECK(r.rejected == 1);
  }
}

TEST_CASE("legacy: cross-field repairs, one condition at a time") {
  struct Static {
    int64_t ip, mask, gw;
    bool dhcpAfter;
  };
  const Static st[] = {{0, 0x00FFFFFF, 1, true},
                       {1, 0, 1, true},
                       {1, 0x00FFFFFF, 0, true},
                       {1, 0x00FFFFFF, 1, false}};
  for (const Static& k : st) {
    FakeNvs n;
    n.putInt("netCfg", "dhcp", 0);
    n.putInt("netCfg", "staticIp", k.ip);
    n.putInt("netCfg", "mask", k.mask);
    n.putInt("netCfg", "gw", k.gw);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(c.net.dhcp == k.dhcpAfter);
    CHECK(r.rejected == (k.dhcpAfter ? 1 : 0));
    CHECK(c.net.ip == static_cast<uint32_t>(k.ip));  // repaired, not reset to defaults
    CHECK(c.net.gateway == static_cast<uint32_t>(k.gw));
  }
  {
    FakeNvs n;  // one-char ssid, short password
    n.putStr("netCfg", "ssid", "x");
    n.putStr("netCfg", "pwd", "1234567");
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(std::string(c.net.ssid).empty());
    CHECK(first(r) == "netCfg/pwd");
    CHECK(r.rejected == 1);
  }
  {
    FakeNvs n;  // one-char ssid, WiFi only, good password: kept
    n.putInt("netCfg", "ethwifi", 2);
    n.putStr("netCfg", "ssid", "x");
    n.putStr("netCfg", "pwd", "12345678");
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(std::string(c.net.ssid) == "x");
    CHECK(c.net.iface == NetInterface::Wifi);
    CHECK(r.rejected == 0);
  }
  {
    FakeNvs n;  // syslog level 1 without server
    n.putInt("netCfg", "syslogEnable", 1);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(c.syslog.level == 0);
    CHECK(first(r) == "netCfg/syslogEnable");
  }
  {
    FakeNvs n;  // one-char credentials are a complete login
    n.putStr("netCfg", "userName", "u");
    n.putStr("netCfg", "userPwd", "p");
    Config c;
    CHECK(importLegacyConfig(n, c).rejected == 0);
    CHECK(std::string(c.web.user) == "u");
  }
  {
    FakeNvs n;  // one-char user alone
    n.putStr("netCfg", "userName", "u");
    Config c;
    n.putStr("sysCfg", "stName", "keep");
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(first(r) == "netCfg/userPwd");
    CHECK(r.rejected == 1);
    CHECK(std::string(c.web.user).empty());
    CHECK(std::string(c.station) == "keep");
  }
  {
    FakeNvs n;  // one-char broker host is a host
    n.putInt("protCfg", "dataProt", 1);
    n.putInt("protCfg", "brokerIp", 0x04030201);
    n.putInt("protCfg", "brokerMD", 10);
    n.putInt("protCfg", "publishInterval", 10);  // min delay == interval: fine
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(c.mqtt.mode == MqttMode::Mqtt);
    CHECK(c.mqtt.minDelayS == 10);
    CHECK(r.rejected == 0);
  }
}

TEST_CASE("legacy: web login needs both user and password") {
  {
    FakeNvs n;
    n.putStr("netCfg", "userName", "admin");
    n.putStr("netCfg", "userPwd", "pw");
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(std::string(c.web.user) == "admin");
    CHECK(std::string(c.web.password) == "pw");
    CHECK(r.rejected == 0);
  }
  {
    FakeNvs n;
    n.putStr("netCfg", "userName", "admin");
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(std::string(c.web.user).empty());
    CHECK(first(r) == "netCfg/userPwd");
  }
  {
    FakeNvs n;
    n.putStr("netCfg", "userPwd", "pw");
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(std::string(c.web.password).empty());
    CHECK(first(r) == "netCfg/userName");
    CHECK(valid(c));
  }
  {
    FakeNvs n;
    n.putStr("netCfg", "userName", "ad:min");
    n.putStr("netCfg", "userPwd", "pw");
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(r.rejected == 2);
    CHECK(first(r) == "netCfg/userName");
    CHECK(std::string(c.web.password).empty());
    CHECK(valid(c));
  }
}

TEST_CASE("legacy: station name") {
  FakeNvs n;
  n.putStr("sysCfg", "stName", "a/b");
  Config c;
  ImportReport r = importLegacyConfig(n, c);
  CHECK(std::string(c.station) == "VdMot");
  CHECK(first(r) == "sysCfg/stName");
  CHECK(r.anyLegacy);  // a string key alone counts
  n.putStr("sysCfg", "stName", "");
  r = importLegacyConfig(n, c);
  CHECK(std::string(c.station) == "VdMot");
  CHECK(r.rejected == 1);
  n.putStr("sysCfg", "stName", std::string(20, 'x'));
  r = importLegacyConfig(n, c);
  CHECK(std::string(c.station) == std::string(20, 'x'));
  CHECK(r.imported == 1);
  // Wrong stored type: the key is treated as missing.
  n.putInt("sysCfg", "stName", 5);
  r = importLegacyConfig(n, c);
  CHECK(std::string(c.station) == "VdMot");
  CHECK(r.rejected == 0);
  CHECK(r.imported == 0);
  CHECK_FALSE(r.anyLegacy);
}

TEST_CASE("legacy: MQTT keys and their quirks") {
  SUBCASE("zeros mean defaults, interval is clamped") {
    FakeNvs n;
    n.putInt("protCfg", "dataProt", 1);
    n.putInt("protCfg", "brokerIp", 0x0100000A);
    n.putInt("protCfg", "brokerPort", 0);
    n.putInt("protCfg", "publishInterval", 0);
    Config c;
    ImportReport r = importLegacyConfig(n, c);
    CHECK(c.mqtt.mode == MqttMode::Mqtt);
    CHECK(std::string(c.mqtt.host) == "10.0.0.1");
    CHECK(c.mqtt.port == 1883);
    CHECK(c.mqtt.publishIntervalS == 2);
    CHECK(c.mqtt.minDelayS == 2);  // default 5 > 2 -> clamped, reported
    CHECK(first(r) == "protCfg/brokerMD");
    CHECK(r.imported == 4);
    n.putInt("protCfg", "publishInterval", 1);
    r = importLegacyConfig(n, c);
    CHECK(c.mqtt.publishIntervalS == 2);
    n.putInt("protCfg", "publishInterval", 2);
    importLegacyConfig(n, c);
    CHECK(c.mqtt.publishIntervalS == 2);
    n.putInt("protCfg", "publishInterval", 3600);
    r = importLegacyConfig(n, c);
    CHECK(c.mqtt.publishIntervalS == 3600);
    CHECK(c.mqtt.minDelayS == 5);
    CHECK(r.rejected == 0);
    n.putInt("protCfg", "publishInterval", 3601);
    importLegacyConfig(n, c);
    CHECK(c.mqtt.publishIntervalS == 3600);
    n.putInt("protCfg", "publishInterval", 4294967295ll);
    importLegacyConfig(n, c);
    CHECK(c.mqtt.publishIntervalS == 3600);
    n.putInt("protCfg", "publishInterval", -5);
    importLegacyConfig(n, c);
    CHECK(c.mqtt.publishIntervalS == 2);
  }
  SUBCASE("broker address 0 means no host") {
    FakeNvs n;
    n.putInt("protCfg", "dataProt", 1);
    n.putInt("protCfg", "brokerIp", 0);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(c.mqtt.mode == MqttMode::Off);  // MQTT without a broker is switched off
    CHECK(std::string(c.mqtt.host).empty());
    CHECK(r.imported == 2);
    CHECK(first(r) == "protCfg/brokerIp");
    CHECK(valid(c));
  }
  SUBCASE("broker address out of range") {
    FakeNvs n;
    n.putInt("protCfg", "brokerIp", -1);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(first(r) == "protCfg/brokerIp");
    CHECK(r.imported == 0);
  }
  SUBCASE("flags: missing brokerPF falls back to 7 when protCfg exists") {
    FakeNvs n;
    n.putInt("protCfg", "dataProt", 0);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(c.mqtt.separate);
    CHECK(c.mqtt.allTemps);
    CHECK(c.mqtt.pathAsRoot);
    CHECK_FALSE(c.mqtt.upTime);
    CHECK_FALSE(c.mqtt.onChange);
    CHECK_FALSE(c.mqtt.retained);
    CHECK_FALSE(c.mqtt.plainText);
    CHECK_FALSE(c.mqtt.diag);
    CHECK(r.imported == 1);
    CHECK(valid(c));
  }
  SUBCASE("flags: no protCfg at all keeps the new defaults") {
    FakeNvs n;
    n.putStr("sysCfg", "stName", "x");
    Config c;
    importLegacyConfig(n, c);
    CHECK_FALSE(c.mqtt.pathAsRoot);
    CHECK(c.mqtt.upTime);
    CHECK(c.mqtt.diag);
  }
  SUBCASE("flags: every bit maps to its field") {
    const char* names[] = {"separate", "allTemps", "pathAsRoot", "upTime",
                           "onChange", "retained", "plainText",  "diag"};
    for (int bit = 0; bit < 8; ++bit) {
      CAPTURE(names[bit]);
      FakeNvs n;
      n.putInt("protCfg", "brokerPF", 1 << bit);
      Config c;
      importLegacyConfig(n, c);
      const MqttConfig& m = c.mqtt;
      const bool got[] = {m.separate, m.allTemps, m.pathAsRoot, m.upTime,
                          m.onChange, m.retained, m.plainText,  m.diag};
      for (int k = 0; k < 8; ++k) CHECK(got[k] == (k == bit));
    }
    {
      FakeNvs z;
      z.putInt("protCfg", "brokerPF", 0);
      Config c0;
      const ImportReport r0 = importLegacyConfig(z, c0);
      CHECK(r0.rejected == 0);
      CHECK(r0.imported == 1);
      CHECK_FALSE(c0.mqtt.separate);
      CHECK_FALSE(c0.mqtt.upTime);
      CHECK_FALSE(c0.mqtt.diag);
    }
    FakeNvs n;
    n.putInt("protCfg", "brokerPF", 0xFF);
    Config c;
    importLegacyConfig(n, c);
    CHECK(c.mqtt.diag);
    CHECK(c.mqtt.pathAsRoot);
    n.putInt("protCfg", "brokerPF", 256);
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(first(r) == "protCfg/brokerPF");
    CHECK(c.mqtt.upTime);  // default kept
    n.putInt("protCfg", "brokerPF", -1);
    CHECK(first(importLegacyConfig(n, c)) == "protCfg/brokerPF");
  }
  SUBCASE("HA mode needs separate topics") {
    FakeNvs n;
    n.putInt("protCfg", "dataProt", 2);
    n.putInt("protCfg", "brokerIp", 0x0100000A);
    n.putInt("protCfg", "brokerPF", 0xFE);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(c.mqtt.mode == MqttMode::Mqtt);
    CHECK_FALSE(c.mqtt.separate);
    CHECK(first(r) == "protCfg/dataProt");
    CHECK(r.rejected == 1);
    CHECK(valid(c));
  }
  SUBCASE("dataProt out of range") {
    FakeNvs n;
    n.putInt("protCfg", "dataProt", 3);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(c.mqtt.mode == MqttMode::Off);
    CHECK(first(r) == "protCfg/dataProt");
    CHECK(c.mqtt.pathAsRoot);  // namespace present -> brokerPF fallback 7
  }
  SUBCASE("keepalive, min delay and number format") {
    FakeNvs n;
    n.putInt("protCfg", "brokerKAT", 4);
    n.putInt("protCfg", "brokerMD", 3601);
    n.putInt("protCfg", "brokerMQF", 4);
    Config c;
    ImportReport r = importLegacyConfig(n, c);
    CHECK(c.mqtt.keepAliveS == 60);
    CHECK(c.mqtt.minDelayS == 5);
    CHECK(c.mqtt.germanDecimal);
    CHECK(r.rejected == 2);
    CHECK(r.ignored == 0);
    CHECK(first(r) == "protCfg/brokerKAT");
    n.putInt("protCfg", "brokerKAT", 300);
    n.putInt("protCfg", "brokerMD", 0);
    n.putInt("protCfg", "brokerMQF", 3);  // failsafe bits only: dropped
    r = importLegacyConfig(n, c);
    CHECK(c.mqtt.keepAliveS == 300);
    CHECK(c.mqtt.minDelayS == 0);
    CHECK_FALSE(c.mqtt.germanDecimal);
    CHECK(r.ignored == 1);
    CHECK(r.rejected == 0);
    n.putInt("protCfg", "brokerKAT", 5);
    n.putInt("protCfg", "brokerMQF", 1);
    r = importLegacyConfig(n, c);
    CHECK(c.mqtt.keepAliveS == 5);
    CHECK(r.ignored == 1);
    n.putInt("protCfg", "brokerKAT", 301);
    n.putInt("protCfg", "brokerMQF", 0x104);
    r = importLegacyConfig(n, c);
    CHECK(r.rejected == 2);
    CHECK_FALSE(c.mqtt.germanDecimal);
    n.putInt("protCfg", "brokerMQF", 0xFF);
    n.putInt("protCfg", "brokerKAT", 60);
    r = importLegacyConfig(n, c);
    CHECK(r.rejected == 0);
    CHECK(c.mqtt.germanDecimal);
    CHECK(r.ignored == 1);
    n.putInt("protCfg", "brokerMQF", -4);
    CHECK(importLegacyConfig(n, c).rejected == 1);
  }
  SUBCASE("credentials") {
    FakeNvs n;
    n.putStr("protCfg", "brokerUser", std::string(64, 'u'));
    n.putStr("protCfg", "brokerPwd", std::string(65, 'p'));
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(std::string(c.mqtt.user) == std::string(64, 'u'));
    CHECK(std::string(c.mqtt.password).empty());
    CHECK(first(r) == "protCfg/brokerPwd");
  }
}

TEST_CASE("legacy: time zone keys") {
  FakeNvs n;
  n.putStr("tZCfg", "tZ", "");
  n.putStr("tZCfg", "tZCode", "");  // setDefault() stores empty strings
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK(std::string(c.time.tzName).empty());
  CHECK(std::string(c.time.tzPosix) == "CET-1CEST,M3.5.0,M10.5.0/3");
  CHECK(first(r) == "tZCfg/tZCode");
  CHECK(r.imported == 1);
  n.putStr("tZCfg", "tZCode", "EST5EDT");
  n.putStr("tZCfg", "tZ", std::string(50, 'z'));
  const ImportReport r2 = importLegacyConfig(n, c);
  CHECK(std::string(c.time.tzPosix) == "EST5EDT");
  CHECK(std::string(c.time.tzName) == "Europe/Berlin");
  CHECK(first(r2) == "tZCfg/tZ");
}

TEST_CASE("legacy: calibration schedule keys") {
  FakeNvs n;
  n.putInt("valvesCfg", "dayOfCalib", 127);
  n.putInt("valvesCfg", "hourOfCalib", 23);
  Config c;
  ImportReport r = importLegacyConfig(n, c);
  CHECK(c.calib.dayMask == 127);
  CHECK(c.calib.hour == 23);
  CHECK(r.imported == 2);
  n.putInt("valvesCfg", "dayOfCalib", 0);
  n.putInt("valvesCfg", "hourOfCalib", 0);
  importLegacyConfig(n, c);
  CHECK(c.calib.dayMask == 0);
  CHECK(c.calib.hour == 0);
  n.putInt("valvesCfg", "dayOfCalib", 128);
  n.putInt("valvesCfg", "hourOfCalib", 23);
  r = importLegacyConfig(n, c);
  CHECK(c.calib.dayMask == 9);
  CHECK(c.calib.hour == 23);
  CHECK(r.rejected == 1);
  CHECK(first(r) == "valvesCfg/dayOfCalib");
  n.putInt("valvesCfg", "hourOfCalib", -1);
  r = importLegacyConfig(n, c);
  CHECK(r.rejected == 2);
  CHECK(c.calib.dayMask == 9);
  CHECK(c.calib.hour == 0);

  // Hour 24 (the legacy UI allowed it; tm_hour never matches) meant "never":
  // no scheduled calibration, whatever the days say.
  for (int64_t hour : {int64_t{24}, int64_t{25}, int64_t{255}}) {
    CAPTURE(hour);
    for (int64_t days : {int64_t{9}, int64_t{128}}) {
      FakeNvs h;
      h.putInt("valvesCfg", "dayOfCalib", days);
      h.putInt("valvesCfg", "hourOfCalib", hour);
      Config d;
      r = importLegacyConfig(h, d);
      CHECK(d.calib.dayMask == 0);
      CHECK(d.calib.hour == 0);
      CHECK(r.imported == 2);
      CHECK(r.rejected == 0);
    }
  }
  FakeNvs h;
  h.putInt("valvesCfg", "hourOfCalib", 24);  // days missing
  Config d;
  r = importLegacyConfig(h, d);
  CHECK(d.calib.dayMask == 0);
  CHECK(r.imported == 1);
  h.putInt("valvesCfg", "hourOfCalib", 256);  // not a legacy value
  d = Config{};
  r = importLegacyConfig(h, d);
  CHECK(d.calib.dayMask == 9);
  CHECK(r.rejected == 1);
}

TEST_CASE("legacy: names and texts are kept byte for byte (UTF-8, edge spaces)") {
  FakeNvs n;
  n.putStr("sysCfg", "stName", "Fu\xc3\x9f" "boden");
  auto v = valvesBlob();
  setValve(v, 0, "K\xc3\xbc" "che", 1);      // 6 bytes
  setValve(v, 1, "Bad ", 1);               // trailing space: segment "Bad_"
  setValve(v, 2, "\xc3\xa4\xc3\xb6\xc3\xbc\xc3\x9f\xc3\xa4", 1);  // 10 bytes
  n.putBlob("valvesCfg", "valves", v);
  auto vb = voltsBlob();
  setVolt(vb, 0, "Vorlauf", 0, 0.0f, 1.0f, "\xc2\xb0" "C", "");
  n.putBlob("voltsCfg", "volts", vb);
  n.putStr("netCfg", "ssid", "G\xc3\xa4ste");
  n.putStr("netCfg", "pwd", "p\xc3\xa4sswort");
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK(r.rejected == 0);
  CHECK(std::string(c.station) == "Fu\xc3\x9f" "boden");
  CHECK(std::string(c.valves[0].name) == "K\xc3\xbc" "che");
  CHECK(std::string(c.valves[1].name) == "Bad ");
  CHECK(std::string(c.valves[2].name) == "\xc3\xa4\xc3\xb6\xc3\xbc\xc3\x9f\xc3\xa4");
  CHECK(std::string(c.volts[0].unit) == "\xc2\xb0" "C");
  CHECK(std::string(c.net.ssid) == "G\xc3\xa4ste");
  CHECK(std::string(c.net.wifiPassword) == "p\xc3\xa4sswort");
  char path[48];
  CHECK(validateConfig(c, path, sizeof path));

  // Bytes that are not UTF-8 (a truncated sequence) are still rejected.
  FakeNvs bad;
  auto bv = valvesBlob();
  setValve(bv, 0, "Kueche\xc3", 1);
  bad.putBlob("valvesCfg", "valves", bv);
  bad.putStr("sysCfg", "stName", "St\xe4tion");  // Latin-1
  Config b;
  const ImportReport rb = importLegacyConfig(bad, b);
  CHECK(rb.rejected == 2);
  CHECK(std::string(b.valves[0].name).empty());
  CHECK(std::string(b.station) == "VdMot");
}

TEST_CASE("legacy: an open WiFi network (empty password) is kept") {
  FakeNvs n;
  n.putInt("netCfg", "ethwifi", 2);
  n.putStr("netCfg", "ssid", "Guest");
  n.putStr("netCfg", "pwd", "");
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK(r.rejected == 0);
  CHECK(std::string(c.net.ssid) == "Guest");
  CHECK(c.net.wifiPassword[0] == '\0');
  CHECK(c.net.iface == NetInterface::Wifi);
  char path[48];
  CHECK(validateConfig(c, path, sizeof path));
}

TEST_CASE("legacy: valves blob") {
  SUBCASE("wrong size rejects the whole blob") {
    for (size_t size : {size_t{0}, size_t{143}, size_t{145}, size_t{288}}) {
      CAPTURE(size);
      FakeNvs n;
      std::vector<uint8_t> b(size, 0);
      if (size >= 12) setValve(b, 0, "x", 1);
      n.putBlob("valvesCfg", "valves", b);
      Config c;
      const ImportReport r = importLegacyConfig(n, c);
      CHECK(r.anyLegacy);
      CHECK(r.imported == 0);
      CHECK(r.rejected == 1);
      CHECK(first(r) == "valvesCfg/valves");
      CHECK(std::string(c.valves[0].name).empty());
      CHECK_FALSE(c.valves[0].active);
    }
  }
  SUBCASE("per-element checks") {
    FakeNvs n;
    auto b = valvesBlob();
    setValve(b, 0, "ok", 1);
    memset(&b[12], 'x', 11);  // valve 2: name without NUL
    b[23] = 1;
    setValve(b, 2, "bad/name", 0);
    setValve(b, 3, "x", 2);  // active byte not a bool
    setValve(b, 11, "1234567890", 1);
    n.putBlob("valvesCfg", "valves", b);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(r.imported == 1);
    CHECK(r.rejected == 3);
    CHECK(first(r) == "valvesCfg/valves.2.name");
    CHECK(std::string(c.valves[0].name) == "ok");
    CHECK(c.valves[0].active);
    CHECK(std::string(c.valves[1].name).empty());
    CHECK(c.valves[1].active);
    CHECK(std::string(c.valves[2].name).empty());
    CHECK(std::string(c.valves[3].name) == "x");
    CHECK_FALSE(c.valves[3].active);
    CHECK(std::string(c.valves[11].name) == "1234567890");
    CHECK(valid(c));
  }
  SUBCASE("duplicate names: later ones are cleared") {
    FakeNvs n;
    auto b = valvesBlob();
    setValve(b, 0, "Bad", 1);
    setValve(b, 4, "Bad", 1);
    setValve(b, 5, "a b", 1);
    setValve(b, 6, "a_b", 1);
    n.putBlob("valvesCfg", "valves", b);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(std::string(c.valves[0].name) == "Bad");
    CHECK(std::string(c.valves[4].name).empty());
    CHECK(std::string(c.valves[5].name) == "a b");
    CHECK(std::string(c.valves[6].name).empty());
    CHECK(r.rejected == 2);
    CHECK(first(r) == "valvesCfg/valves.5.name");
    CHECK(valid(c));
  }
  SUBCASE("number collisions, also the ones created by clearing") {
    FakeNvs n;
    auto b = valvesBlob();
    setValve(b, 0, "4", 1);   // valve 4 is named "x" ... until it is cleared
    setValve(b, 1, "x", 1);
    setValve(b, 3, "x", 1);   // duplicate of valve 2 -> cleared -> "4" collides
    setValve(b, 7, "8", 1);   // own number: fine
    n.putBlob("valvesCfg", "valves", b);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(std::string(c.valves[3].name).empty());
    CHECK(std::string(c.valves[0].name).empty());
    CHECK(std::string(c.valves[1].name) == "x");
    CHECK(std::string(c.valves[7].name) == "8");
    CHECK(r.rejected == 2);
    CHECK(first(r) == "valvesCfg/valves.4.name");
    CHECK(valid(c));
  }
}

TEST_CASE("legacy: valve name rules keep everything else") {
  FakeNvs n;
  auto b = valvesBlob();
  setValve(b, 1, "ab", 1);
  setValve(b, 2, "ac", 1);   // same first char: no clash
  setValve(b, 3, "05", 1);   // leading zero: not a number
  setValve(b, 4, "10", 1);   // valve 10 is unnamed: clash
  setValve(b, 5, "1", 1);    // valve 1 is unnamed: clash
  n.putBlob("valvesCfg", "valves", b);
  auto t = tempsBlob();
  setTemp(t, 0, "ab", 0, 0, "");  // temp names never clash with valves
  n.putBlob("tempsCfg", "temps", t);
  n.putStr("sysCfg", "stName", "keep");
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK(std::string(c.valves[1].name) == "ab");
  CHECK(std::string(c.valves[2].name) == "ac");
  CHECK(std::string(c.valves[3].name) == "05");
  CHECK(std::string(c.valves[4].name).empty());
  CHECK(std::string(c.valves[5].name).empty());
  CHECK(std::string(c.temps[0].name) == "ab");
  CHECK(std::string(c.station) == "keep");
  CHECK(r.rejected == 2);
  CHECK(first(r) == "valvesCfg/valves.5.name");
}

TEST_CASE("legacy: slot repairs keep everything else") {
  FakeNvs n;
  auto t = tempsBlob();
  setTemp(t, 0, "a", 1, 0, "");      // active without id
  setTemp(t, 2, "c", 1, 0, kIdB);
  setTemp(t, 5, "f", 1, 0, kIdB);    // duplicate of slot 3
  setTemp(t, 33, "z", 1, 0, kIdA);
  n.putBlob("tempsCfg", "temps", t);
  auto v = voltsBlob();
  setVolt(v, 0, "v", 1, 0.0f, 1.0f, "", "");
  setVolt(v, 3, "w", 1, 0.0f, 1.0f, "", kIdV);
  setVolt(v, 7, "x", 1, 0.0f, 1.0f, "", kIdV);
  n.putBlob("voltsCfg", "volts", v);
  n.putStr("sysCfg", "stName", "keep");
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK_FALSE(c.temps[0].active);
  CHECK(c.temps[2].active);
  CHECK(c.temps[2].id == oid(kIdB));
  CHECK_FALSE(c.temps[5].active);
  CHECK(isZero(c.temps[5].id));
  CHECK(c.temps[33].active);
  CHECK_FALSE(c.volts[0].active);
  CHECK(c.volts[3].active);
  CHECK_FALSE(c.volts[7].active);
  CHECK(isZero(c.volts[7].id));
  CHECK(std::string(c.station) == "keep");
  // temps.1.active, temps.6.id, temps.6.active, volts.1.active, volts.8.id, volts.8.active
  CHECK(r.rejected == 6);
  CHECK(first(r) == "tempsCfg/temps.1.active");
}

TEST_CASE("legacy: HA mode with the decimal comma switches to the dot") {
  FakeNvs n = staticDevice();
  n.putInt("protCfg", "brokerMQF", 4);
  Config c;
  ImportReport r = importLegacyConfig(n, c);
  CHECK_FALSE(c.mqtt.germanDecimal);
  CHECK(r.rejected == 1);
  CHECK(first(r) == "protCfg/brokerMQF");
  checkKept(c);
  // HA without separate topics becomes plain MQTT first, which keeps the comma.
  n.putInt("protCfg", "brokerPF", 0x7A);
  r = importLegacyConfig(n, c);
  CHECK(c.mqtt.mode == MqttMode::Mqtt);
  CHECK(c.mqtt.germanDecimal);
  CHECK(r.rejected == 1);
  CHECK(first(r) == "protCfg/dataProt");
  n.putInt("protCfg", "brokerPF", 0x7B);
  n.putInt("protCfg", "dataProt", 1);
  r = importLegacyConfig(n, c);
  CHECK(c.mqtt.germanDecimal);
  CHECK(r.rejected == 0);
}

TEST_CASE("legacy: valve names with one HA id keep the first name") {
  FakeNvs n = staticDevice();
  auto v = valvesBlob();
  setValve(v, 0, "Bad 1", 1);
  setValve(v, 1, "Bad.1", 1);          // HA id "Bad_1" like valve 1
  setValve(v, 2, "K\xC3\xBC" "che", 1);
  setValve(v, 3, "Kuche", 0);          // HA id "Kuche" like valve 3, also when inactive
  setValve(v, 4, "Bad-1", 1);          // '-' stays: another id
  n.putBlob("valvesCfg", "valves", v);
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK(std::string(c.valves[0].name) == "Bad 1");
  CHECK(std::string(c.valves[1].name).empty());
  CHECK(c.valves[1].active);
  CHECK(std::string(c.valves[2].name) == "K\xC3\xBC" "che");
  CHECK(std::string(c.valves[3].name).empty());
  CHECK(std::string(c.valves[4].name) == "Bad-1");
  CHECK(r.rejected == 2);
  CHECK(first(r) == "valvesCfg/valves.2.name");
  checkKept(c);
}

TEST_CASE("legacy: active sensors with one HA id keep the first name") {
  FakeNvs n = staticDevice();
  auto t = tempsBlob();
  setTemp(t, 0, "Flur", 1, -7, kIdA);
  setTemp(t, 5, "Flur", 1, 12, kIdB);  // later: name cleared, the rest kept
  setTemp(t, 6, "Bad", 0, 0, "");      // inactive: no HA entity, no clash
  setTemp(t, 7, "Bad", 1, 0, "28-00-00-00-00-00-00-01");
  setTemp(t, 8, "Flur", 0, 0, "28-00-00-00-00-00-00-02");
  n.putBlob("tempsCfg", "temps", t);
  auto w = voltsBlob();
  setVolt(w, 0, "U", 1, 0.5f, 0.01f, "V", kIdV);
  setVolt(w, 1, "U", 1, 0.0f, 1.0f, "V", "26-00-00-00-00-00-00-01");  // neighbours, one char
  n.putBlob("voltsCfg", "volts", w);
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK(std::string(c.temps[0].name) == "Flur");
  CHECK(std::string(c.temps[5].name).empty());
  CHECK(c.temps[5].active);
  CHECK(c.temps[5].id == oid(kIdB));
  CHECK(c.temps[5].offset == 12);
  CHECK(std::string(c.temps[6].name) == "Bad");
  CHECK(std::string(c.temps[7].name) == "Bad");
  CHECK(std::string(c.temps[8].name) == "Flur");
  CHECK(std::string(c.volts[0].name) == "U");
  CHECK(std::string(c.volts[1].name).empty());
  CHECK(c.volts[1].active);
  CHECK(r.rejected == 2);
  CHECK(first(r) == "tempsCfg/temps.6.name");
  checkKept(c);
}

TEST_CASE("legacy: a sensor named like the number of a later unnamed one loses its name") {
  FakeNvs n;
  auto t = tempsBlob();
  setTemp(t, 0, "3", 1, 0, kIdA);  // HA id "3" like slot 3 once that one is cleared
  setTemp(t, 2, "7", 1, 0, kIdB);  // HA id "7" like the unnamed slot 7
  setTemp(t, 6, "", 1, 0, "28-00-00-00-00-00-00-01");
  n.putBlob("tempsCfg", "temps", t);
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK(std::string(c.temps[0].name).empty());
  CHECK(std::string(c.temps[2].name).empty());
  CHECK(c.temps[0].active);
  CHECK(c.temps[2].active);
  CHECK(c.temps[6].active);
  CHECK(r.rejected == 2);
  CHECK(first(r) == "tempsCfg/temps.3.name");
  CHECK(valid(c));
}

TEST_CASE("legacy: a sensor name cleared for one clash is checked again") {
  FakeNvs n;
  auto t = tempsBlob();
  setTemp(t, 0, "x", 1, 0, kIdA);
  setTemp(t, 1, "3", 1, 0, kIdB);  // clashes with slot 3 once its "x" is cleared
  setTemp(t, 2, "x", 1, 0, "28-00-00-00-00-00-00-01");
  n.putBlob("tempsCfg", "temps", t);
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK(std::string(c.temps[0].name) == "x");
  CHECK(std::string(c.temps[1].name).empty());
  CHECK(std::string(c.temps[2].name).empty());
  CHECK(r.rejected == 2);
  CHECK(first(r) == "tempsCfg/temps.3.name");
  CHECK(valid(c));
}

TEST_CASE("legacy: temps blob") {
  SUBCASE("wrong size") {
    FakeNvs n;
    n.putBlob("tempsCfg", "temps", std::vector<uint8_t>(1495, 0));
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(first(r) == "tempsCfg/temps");
    CHECK(r.imported == 0);
  }
  SUBCASE("offsets are clamped to +-10.0 C") {
    FakeNvs n;
    auto b = tempsBlob();
    setTemp(b, 0, "", 0, 100, "");
    setTemp(b, 1, "", 0, -100, "");
    setTemp(b, 2, "", 0, 101, "");
    setTemp(b, 3, "", 0, -101, "");
    setTemp(b, 4, "", 0, INT32_MAX, "");
    setTemp(b, 5, "", 0, INT32_MIN, "");
    n.putBlob("tempsCfg", "temps", b);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(c.temps[0].offset == 100);
    CHECK(c.temps[1].offset == -100);
    CHECK(c.temps[2].offset == 100);
    CHECK(c.temps[3].offset == -100);
    CHECK(c.temps[4].offset == 100);
    CHECK(c.temps[5].offset == -100);
    CHECK(r.rejected == 4);
    CHECK(first(r) == "tempsCfg/temps.3.offset");
    CHECK(valid(c));
  }
  SUBCASE("ids, activity and duplicates") {
    FakeNvs n;
    auto b = tempsBlob();
    setTemp(b, 0, "a", 1, 0, kIdA);
    setTemp(b, 1, "b", 1, 0, "");                          // active without id
    setTemp(b, 2, "c", 1, 0, "00-00-00-00-00-00-00-00");   // undefined id
    setTemp(b, 3, "d", 1, 0, "28-84-37-94-97-FF-03-23");   // duplicate of slot 1
    setTemp(b, 4, "e", 0, 0, "28-84-37");                  // malformed
    memset(&b[5 * 44 + 16], '2', 25);                      // no NUL
    setTemp(b, 33, "z", 1, 5, kIdB);
    n.putBlob("tempsCfg", "temps", b);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(c.temps[0].active);
    CHECK(c.temps[0].id == oid(kIdA));
    CHECK_FALSE(c.temps[1].active);
    CHECK_FALSE(c.temps[2].active);
    CHECK(isZero(c.temps[2].id));
    CHECK_FALSE(c.temps[3].active);
    CHECK(isZero(c.temps[3].id));
    CHECK(std::string(c.temps[3].name) == "d");
    CHECK(isZero(c.temps[4].id));
    CHECK(isZero(c.temps[5].id));
    CHECK(c.temps[33].active);
    CHECK(c.temps[33].offset == 5);
    // rejected: 5.id, 6.id (import) then 2.active, 3.active, 4.id, 4.active (repair)
    CHECK(r.rejected == 6);
    CHECK(first(r) == "tempsCfg/temps.5.id");
    CHECK(valid(c));
  }
  SUBCASE("names") {
    FakeNvs n;
    auto b = tempsBlob();
    setTemp(b, 0, "Wohnzimmer", 0, 0, "");
    setTemp(b, 1, "x#", 0, 0, "");
    memset(&b[2 * 44], 'n', 11);
    n.putBlob("tempsCfg", "temps", b);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(std::string(c.temps[0].name) == "Wohnzimmer");
    CHECK(std::string(c.temps[1].name).empty());
    CHECK(std::string(c.temps[2].name).empty());
    CHECK(r.rejected == 2);
    CHECK(first(r) == "tempsCfg/temps.2.name");
  }
}

TEST_CASE("legacy: volts blob") {
  SUBCASE("wrong size") {
    FakeNvs n;
    n.putBlob("voltsCfg", "volts", std::vector<uint8_t>(448, 0));  // without the AV table
    Config c;
    CHECK(first(importLegacyConfig(n, c)) == "voltsCfg/volts");
  }
  SUBCASE("fields") {
    FakeNvs n;
    auto b = voltsBlob();
    setVolt(b, 0, "v1", 1, -1000.0f, 1000.0f, "12345678", kIdV);
    setVolt(b, 1, "v2", 0, NAN, 0.0f, "", "");
    setVolt(b, 2, "v3", 0, 1000.5f, -1000.5f, "", "");
    setVolt(b, 3, "v4", 0, 0.0f, INFINITY, "", "");
    memset(&b[4 * 56 + 20], 'u', 9);  // unit without NUL
    setVolt(b, 5, "v6", 1, 0.0f, 1.0f, "V", kIdV);  // duplicate id
    setVolt(b, 6, "v7", 0, 0.0f, 1.0f, "a/b", "");
    b[7 * 56 + 11] = 3;
    n.putBlob("voltsCfg", "volts", b);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(c.volts[0].offset == -1000.0f);
    CHECK(c.volts[0].factor == 1000.0f);
    CHECK(std::string(c.volts[0].unit) == "12345678");
    CHECK(c.volts[0].active);
    CHECK(c.volts[1].offset == 0.0f);
    CHECK(c.volts[1].factor == 1.0f);
    CHECK(c.volts[2].offset == 0.0f);
    CHECK(c.volts[2].factor == 1.0f);
    CHECK(c.volts[3].factor == 1.0f);
    CHECK(std::string(c.volts[4].unit).empty());
    CHECK(isZero(c.volts[5].id));
    CHECK_FALSE(c.volts[5].active);
    CHECK(std::string(c.volts[6].unit).empty());
    CHECK_FALSE(c.volts[7].active);
    // 2.offset 2.factor 3.offset 3.factor 4.factor 5.unit 7.unit 8.active,
    // then repair 6.id 6.active
    CHECK(r.rejected == 10);
    CHECK(first(r) == "voltsCfg/volts.2.offset");
    CHECK(valid(c));
  }
}

TEST_CASE("legacy: last calibration time") {
  struct Case {
    int64_t v;
    int64_t epoch;
    uint16_t rejected;
  };
  const Case cases[] = {{1577836800, 1577836800, 0}, {1577836799, 0, 1}, {0, 0, 1},
                        {-1, 0, 1},                  {4102444799, 4102444799, 0},
                        {4102444800, 0, 1}};
  for (const Case& k : cases) {
    CAPTURE(k.v);
    FakeNvs n;
    n.putInt("Misc", "MiscLC", k.v);
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(r.lastCalibEpoch == k.epoch);
    CHECK(r.rejected == k.rejected);
    CHECK(r.imported == (k.rejected ? 0 : 1));
    if (k.rejected) CHECK(first(r) == "Misc/MiscLC");
    CHECK(r.anyLegacy);
  }
}

TEST_CASE("legacy: dropped keys are counted, never imported") {
  FakeNvs n;
  n.putInt("sysCfg", "CF", 1);
  n.putInt("protCfg", "brokerInterval", 1000);
  n.putInt("protCfg", "brokerMQTO", 120);
  n.putInt("protCfg", "brokerMQToPos", 10);
  n.putInt("valvesCfg", "movCalib", 1);
  n.putBlob("valvesCtrlCfg", "valvesCtrl", std::vector<uint8_t>(400, 1));
  n.putBlob("valvesCtrlCfg", "valvesCtrl1", std::vector<uint8_t>(10, 1));
  n.putBlob("valvesCtrlCfg", "vCtrlInit", std::vector<uint8_t>(48, 1));
  n.putInt("valvesCtrlCfg", "vCtrlHeat", 1);
  n.putInt("valvesCtrlCfg", "vCtrlParkPos", 10);
  n.putInt("msgCfg", "msgFlags", 3);
  n.putInt("msgCfg", "msgReas", 127);
  n.putInt("msgCfg", "msgMQTO", 10);
  n.putStr("msgCfg", "POAppTk", "tok");
  n.putStr("msgCfg", "POUserTk", "tok");
  n.putStr("msgCfg", "POTitle", "t");
  n.putStr("msgCfg", "EUser", "u");
  n.putStr("msgCfg", "EPwd", std::string(300, 'p'));  // long values are fine to skip
  n.putStr("msgCfg", "EHost", "h");
  n.putInt("msgCfg", "EPort", 465);
  n.putStr("msgCfg", "ERep", "r");
  n.putStr("msgCfg", "ETitle", "t");
  n.putInt("motorCfg", "motorMinC", 1);
  n.putInt("motorCfg", "motorMaxC", 2);
  n.putInt("msgCfg", "unknownKey", 1);  // not on the list: not counted
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK(r.ignored == 24);
  CHECK(r.imported == 0);
  CHECK(r.rejected == 0);
  CHECK(r.anyLegacy);
  CHECK(sameConfig(c, Config{}));

  FakeNvs only;
  only.putInt("valvesCtrlCfg", "vCtrlHeat", 1);
  Config d;
  const ImportReport r2 = importLegacyConfig(only, d);
  CHECK(r2.anyLegacy);
  CHECK(r2.ignored == 1);
}

TEST_CASE("legacy: first rejected key is kept, counting continues") {
  FakeNvs n;
  n.putStr("sysCfg", "stName", "");
  n.putInt("netCfg", "ethwifi", 9);
  n.putInt("valvesCfg", "hourOfCalib", 256);
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK(r.rejected == 3);
  CHECK(first(r) == "sysCfg/stName");
}

TEST_CASE("legacy: fuzz - the result always validates" * doctest::test_suite("fuzz")) {
  std::mt19937 rng(97531);
  const char* intKeys[][2] = {
      {"netCfg", "ethwifi"},       {"netCfg", "dhcp"},         {"netCfg", "staticIp"},
      {"netCfg", "mask"},          {"netCfg", "gw"},           {"netCfg", "dnsIp"},
      {"netCfg", "netConnTO"},     {"netCfg", "syslogEnable"}, {"netCfg", "sysLogIp"},
      {"netCfg", "sysLogPort"},    {"protCfg", "dataProt"},    {"protCfg", "brokerIp"},
      {"protCfg", "brokerPort"},   {"protCfg", "publishInterval"}, {"protCfg", "brokerPF"},
      {"protCfg", "brokerKAT"},    {"protCfg", "brokerMD"},    {"protCfg", "brokerMQF"},
      {"valvesCfg", "dayOfCalib"}, {"valvesCfg", "hourOfCalib"}, {"Misc", "MiscLC"}};
  // tZCfg/tZ is always "Europe/Warsaw": nothing resets the whole config.
  const char* strKeys[][2] = {{"sysCfg", "stName"},     {"netCfg", "ssid"},
                              {"netCfg", "pwd"},        {"netCfg", "userName"},
                              {"netCfg", "userPwd"},    {"netCfg", "timeServer"},
                              {"tZCfg", "tZCode"},
                              {"protCfg", "brokerUser"}, {"protCfg", "brokerPwd"}};
  // The first 10 are names, the last 3 ids.
  const char* pool[] = {"", "a", "1", "3", "x y", "a b", "a_b", "a.b", "Bad", "12345678",
                        "pool.ntp.org", "1.2.3.4", "a:b", "bad/x", "EST5EDT", kIdA, kIdB,
                        "00-00-00-00-00-00-00-00"};
  const int64_t ints[] = {0, 1, 2, 3, 4, 7, 23, 24, 127, 128, 255, 300, 3600, 65535, 65536,
                          -1, 0x00FFFFFF, 0x0101A8C0, 1760000000, 0xFFFFFFFFll};
  for (int iter = 0; iter < 1500; ++iter) {
    FakeNvs n;
    n.putStr("tZCfg", "tZ", "Europe/Warsaw");
    for (auto& k : intKeys) {
      if (rng() % 2) n.putInt(k[0], k[1], ints[rng() % (sizeof ints / sizeof ints[0])]);
    }
    for (auto& k : strKeys) {
      if (rng() % 2) {
        std::string s = pool[rng() % (sizeof pool / sizeof pool[0])];
        if (rng() % 8 == 0) s = std::string(rng() % 80, static_cast<char>(rng() % 256));
        n.putStr(k[0], k[1], s);
      }
    }
    if (rng() % 2) {
      auto b = valvesBlob();
      for (size_t i = 0; i < 12; ++i) {
        if (rng() % 2) setValve(b, i, pool[rng() % 10], static_cast<uint8_t>(rng() % 3));
      }
      if (rng() % 6 == 0) {
        for (auto& x : b) x = static_cast<uint8_t>(rng());
      }
      n.putBlob("valvesCfg", "valves", b);
    }
    if (rng() % 2) {
      auto b = tempsBlob();
      for (size_t i = 0; i < 34; ++i) {
        if (rng() % 3 == 0) {
          setTemp(b, i, pool[rng() % 10], static_cast<uint8_t>(rng() % 3),
                  static_cast<int32_t>(rng() % 400) - 200, pool[15 + rng() % 3]);
        }
      }
      if (rng() % 6 == 0) {
        for (auto& x : b) x = static_cast<uint8_t>(rng());
      }
      n.putBlob("tempsCfg", "temps", b);
    }
    if (rng() % 2) {
      auto b = voltsBlob();
      for (auto& x : b) {
        if (rng() % 4 == 0) x = static_cast<uint8_t>(rng());
      }
      n.putBlob("voltsCfg", "volts", b);
    }
    Config c;
    const ImportReport r = importLegacyConfig(n, c);
    CHECK(valid(c));
    CHECK(std::string(c.time.tzName) == "Europe/Warsaw");
    CHECK(strlen(r.firstRejected) < sizeof r.firstRejected);
    CHECK((r.rejected == 0) == (r.firstRejected[0] == '\0'));
  }
}

TEST_CASE("legacy: a one-character WiFi password is repaired, not reset to defaults") {
  FakeNvs n;
  n.putStr("netCfg", "ssid", "home");
  n.putStr("netCfg", "pwd", "1");
  n.putStr("sysCfg", "stName", "keep");
  Config c;
  const ImportReport r = importLegacyConfig(n, c);
  CHECK(std::string(c.net.ssid).empty());
  CHECK(std::string(c.net.wifiPassword).empty());
  CHECK(std::string(c.station) == "keep");
  CHECK(first(r) == "netCfg/pwd");
  CHECK(r.rejected == 1);
}
