// config: defaults, per-key setter (every key, both sides of every range,
// every type conversion), validation (every per-field and cross-field
// rule with its path), JSON export (golden), JSON patch reader (syntax,
// paths, secrets, fuzz), binary NVS encoding (round trip, every error),
// CRC-32.
#include <math.h>
#include <stdint.h>
#include <string.h>

#include <algorithm>
#include <random>
#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/config.h"
#include "vdm/json_writer.h"

using namespace vdm;

namespace {

const char* const kIdA = "28-84-37-94-97-ff-03-23";
const char* const kIdB = "28-aa-bb-cc-dd-ee-01-67";
const char* const kIdV = "26-11-22-33-44-55-66-29";

ConfigValue S(const char* s) {
  ConfigValue v;
  v.type = ConfigValue::Type::String;
  v.s = s;
  v.len = strlen(s);
  return v;
}
ConfigValue SL(const char* s, size_t len) {
  ConfigValue v = S(s);
  v.len = len;
  return v;
}
ConfigValue I(int64_t i) {
  ConfigValue v;
  v.type = ConfigValue::Type::Int;
  v.i = i;
  return v;
}
ConfigValue F(double f) {
  ConfigValue v;
  v.type = ConfigValue::Type::Float;
  v.f = f;
  return v;
}
ConfigValue B(bool b) {
  ConfigValue v;
  v.type = ConfigValue::Type::Bool;
  v.b = b;
  return v;
}
ConfigValue N() { return ConfigValue{}; }

SetResult set(Config& c, const char* path, const ConfigValue& v, bool clear = false) {
  return setConfigValue(c, path, v, clear);
}

OneWireId oid(const char* s) {
  OneWireId v;
  REQUIRE(parseOneWireId(s, strlen(s), v));
  return v;
}

std::string validatePath(const Config& c) {
  char path[64];
  memset(path, 'x', sizeof path);
  const bool ok = validateConfig(c, path, sizeof path);
  return ok ? std::string("OK") + path : std::string(path);
}

std::string exportJson(const Config& c) {
  static char buf[8192];
  JsonWriter jw(buf, sizeof buf);
  REQUIRE(writeConfigJson(jw, c));
  REQUIRE(jw.complete());
  return std::string(buf, jw.length());
}

PatchResult patch(Config& c, const std::string& json, std::string* pathOut = nullptr) {
  char path[80];
  memset(path, 'x', sizeof path);
  const PatchResult r = applyConfigJson(c, json.data(), json.size(), path, sizeof path);
  if (pathOut) *pathOut = path;
  return r;
}

// Equal stored configs: the `cfg` and the `cfgx` blob.
bool sameConfig(const Config& a, const Config& b) {
  uint8_t ba[kConfigBlobMax], bb[kConfigBlobMax];
  const size_t na = encodeConfig(a, ba, sizeof ba);
  const size_t nb = encodeConfig(b, bb, sizeof bb);
  uint8_t xa[kConfigExtBlobMax], xb[kConfigExtBlobMax];
  const size_t ma = encodeConfigExt(a, xa, sizeof xa);
  const size_t mb = encodeConfigExt(b, xb, sizeof xb);
  return na > 0 && na == nb && memcmp(ba, bb, na) == 0 && ma > 0 && ma == mb &&
         memcmp(xa, xb, ma) == 0;
}

// Config with every field of the 2.0.0 blob away from its default, still
// valid (the keys added later keep their defaults: fullConfigExt()).
Config fullConfig() {
  Config c;
  REQUIRE(set(c, "station", S("Heizung OG")) == SetResult::Ok);
  REQUIRE(set(c, "net.iface", I(2)) == SetResult::Ok);
  REQUIRE(set(c, "net.dhcp", B(false)) == SetResult::Ok);
  REQUIRE(set(c, "net.ip", S("192.168.1.50")) == SetResult::Ok);
  REQUIRE(set(c, "net.mask", S("255.255.255.0")) == SetResult::Ok);
  REQUIRE(set(c, "net.gateway", S("192.168.1.1")) == SetResult::Ok);
  REQUIRE(set(c, "net.dns", S("8.8.8.8")) == SetResult::Ok);
  REQUIRE(set(c, "net.ssid", S("My Wifi")) == SetResult::Ok);
  REQUIRE(set(c, "net.wifiPassword", S("secret123")) == SetResult::Ok);
  REQUIRE(set(c, "net.reconnectTimeoutMin", I(17)) == SetResult::Ok);
  REQUIRE(set(c, "time.ntpServer", S("192.168.1.1")) == SetResult::Ok);
  REQUIRE(set(c, "time.tzName", S("Europe/Warsaw")) == SetResult::Ok);
  REQUIRE(set(c, "time.tzPosix", S("CET-1CEST,M3.5.0,M10.5.0/3x")) == SetResult::Ok);
  REQUIRE(set(c, "syslog.level", I(3)) == SetResult::Ok);
  REQUIRE(set(c, "syslog.server", S("10.0.0.9")) == SetResult::Ok);
  REQUIRE(set(c, "syslog.port", I(1514)) == SetResult::Ok);
  REQUIRE(set(c, "web.user", S("admin")) == SetResult::Ok);
  REQUIRE(set(c, "web.password", S("pa ss\"w")) == SetResult::Ok);
  REQUIRE(set(c, "web.protectRead", B(true)) == SetResult::Ok);
  // Plain MQTT: HA mode needs separate topics and the decimal point.
  REQUIRE(set(c, "mqtt.mode", I(1)) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.host", S("broker.lan")) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.port", I(8883)) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.user", S("mq")) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.password", S("mqpw")) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.keepAliveS", I(30)) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.publishIntervalS", I(120)) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.minDelayS", I(7)) == SetResult::Ok);
  // Every MQTT flag away from its default.
  const char* flipToFalse[] = {"separate",  "allTemps", "upTime",  "onChange", "retained",
                               "plainText", "diag",     "newDiag", "events",
                               "haDiscoveryOnConnect"};
  for (const char* b : flipToFalse) {
    REQUIRE(set(c, (std::string("mqtt.") + b).c_str(), B(false)) == SetResult::Ok);
  }
  REQUIRE(set(c, "mqtt.pathAsRoot", B(true)) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.germanDecimal", B(true)) == SetResult::Ok);
  REQUIRE(set(c, "valves.1.name", S("Bad")) == SetResult::Ok);
  REQUIRE(set(c, "valves.1.active", B(true)) == SetResult::Ok);
  REQUIRE(set(c, "valves.12.name", S("Kitchen")) == SetResult::Ok);
  REQUIRE(set(c, "temps.1.name", S("t1")) == SetResult::Ok);
  REQUIRE(set(c, "temps.1.id", S(kIdA)) == SetResult::Ok);
  REQUIRE(set(c, "temps.1.active", B(true)) == SetResult::Ok);
  REQUIRE(set(c, "temps.1.offset", F(-1.5)) == SetResult::Ok);
  REQUIRE(set(c, "temps.34.id", S(kIdB)) == SetResult::Ok);
  REQUIRE(set(c, "temps.34.offset", F(10.0)) == SetResult::Ok);
  REQUIRE(set(c, "volts.8.name", S("bat")) == SetResult::Ok);
  REQUIRE(set(c, "volts.8.id", S(kIdV)) == SetResult::Ok);
  REQUIRE(set(c, "volts.8.active", B(true)) == SetResult::Ok);
  REQUIRE(set(c, "volts.8.offset", F(-0.25)) == SetResult::Ok);
  REQUIRE(set(c, "volts.8.factor", F(0.01)) == SetResult::Ok);
  REQUIRE(set(c, "volts.8.unit", S("V")) == SetResult::Ok);
  REQUIRE(set(c, "calib.dayMask", I(127)) == SetResult::Ok);
  REQUIRE(set(c, "calib.hour", I(23)) == SetResult::Ok);
  REQUIRE(set(c, "calib.minute", I(59)) == SetResult::Ok);
  REQUIRE(set(c, "persistLog", B(false)) == SetResult::Ok);
  REQUIRE(validatePath(c) == "OK");
  return c;
}

// fullConfig() with every key of the `cfgx` blob away from its default too.
Config fullConfigExt() {
  Config c = fullConfig();
  REQUIRE(set(c, "web.allowedHosts", S("vdmot.lan, 192.168.1.9")) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.rootTopic", S("VdMotFBH")) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.clientId", S("VdMot-east-6c1e51")) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.discoveryPrefix", S("ha/discovery")) == SetResult::Ok);
  REQUIRE(set(c, "failsafe.timeoutMin", I(1440)) == SetResult::Ok);
  REQUIRE(set(c, "valves.1.failsafePct", I(kFailsafeHold)) == SetResult::Ok);
  REQUIRE(set(c, "valves.12.failsafePct", I(0)) == SetResult::Ok);
  REQUIRE(set(c, "valves.2.topic", S("Bad/WC")) == SetResult::Ok);
  REQUIRE(set(c, "valves.12.topic", S("x")) == SetResult::Ok);
  REQUIRE(set(c, "temps.1.topic", S("t/1")) == SetResult::Ok);
  REQUIRE(set(c, "temps.34.topic", S("t34")) == SetResult::Ok);
  REQUIRE(set(c, "volts.8.topic", S("battery")) == SetResult::Ok);
  REQUIRE(validatePath(c) == "OK");
  return c;
}

void fixCrc(std::vector<uint8_t>& b) {
  const uint32_t crc = crc32(b.data(), b.size() - 4);
  for (int i = 0; i < 4; ++i) b[b.size() - 4 + i] = static_cast<uint8_t>(crc >> (8 * i));
}

std::vector<uint8_t> encode(const Config& c) {
  std::vector<uint8_t> b(kConfigBlobMax);
  const size_t n = encodeConfig(c, b.data(), b.size());
  REQUIRE(n > 0);
  b.resize(n);
  return b;
}

// Offset of the first payload byte of a field, found by encoding two
// configs that differ only there.
size_t diffOffset(const Config& a, const Config& b) {
  const std::vector<uint8_t> ea = encode(a), eb = encode(b);
  REQUIRE(ea.size() == eb.size());
  for (size_t i = 8; i + 4 < ea.size(); ++i) {
    if (ea[i] != eb[i]) return i;
  }
  FAIL("no difference");
  return 0;
}

}  // namespace

// ---------------------------------------------------------------- defaults

TEST_CASE("config: defaults are the documented values and validate") {
  Config c;
  c.station[0] = 'X';
  c.mqtt.port = 1;
  setDefaults(c);
  CHECK(c.schema == 2);
  CHECK(kConfigBaseSchema == 1);
  CHECK(kConfigJsonSchema == 2);
  CHECK(std::string(c.station) == "VdMot");
  CHECK(c.net.iface == NetInterface::Auto);
  CHECK(c.net.dhcp);
  CHECK(c.net.ip == 0);
  CHECK(c.net.reconnectTimeoutMin == 5);
  CHECK(std::string(c.time.ntpServer) == "pool.ntp.org");
  CHECK(std::string(c.time.tzName) == "Europe/Berlin");
  CHECK(c.syslog.level == 0);
  CHECK(c.syslog.port == 514);
  CHECK(c.mqtt.mode == MqttMode::Off);
  CHECK(c.mqtt.port == 1883);
  CHECK(c.mqtt.keepAliveS == 60);
  CHECK(c.mqtt.publishIntervalS == 10);
  CHECK(c.mqtt.minDelayS == 5);
  CHECK(c.mqtt.separate);
  CHECK(c.mqtt.allTemps);
  CHECK_FALSE(c.mqtt.pathAsRoot);
  CHECK(c.mqtt.upTime);
  CHECK(c.mqtt.onChange);
  CHECK(c.mqtt.retained);
  CHECK(c.mqtt.plainText);
  CHECK(c.mqtt.diag);
  CHECK_FALSE(c.mqtt.germanDecimal);
  CHECK(c.mqtt.newDiag);
  CHECK(c.mqtt.events);
  CHECK(c.mqtt.haDiscoveryOnConnect);
  CHECK(c.volts[7].factor == 1.0f);
  CHECK(c.calib.dayMask == 9);
  CHECK(c.calib.hour == 0);
  CHECK(c.calib.minute == 0);
  CHECK(c.persistLog);
  // Keys of the cfgx blob.
  CHECK(std::string(c.web.allowedHosts).empty());
  CHECK(std::string(c.mqtt.rootTopic).empty());
  CHECK(std::string(c.mqtt.clientId).empty());
  CHECK(std::string(c.mqtt.discoveryPrefix) == "homeassistant");
  CHECK(c.failsafe.timeoutMin == 60);
  for (const ValveConfig& v : c.valves) {
    CHECK(v.failsafePct == 50);
    CHECK(std::string(v.topic).empty());
  }
  CHECK(std::string(c.temps[33].topic).empty());
  CHECK(std::string(c.volts[7].topic).empty());
  CHECK(validatePath(c) == "OK");
}

TEST_CASE("config: result names") {
  CHECK(std::string(setResultName(SetResult::Ok)) == "ok");
  CHECK(std::string(setResultName(SetResult::UnknownKey)) == "unknown_key");
  CHECK(std::string(setResultName(SetResult::WrongType)) == "wrong_type");
  CHECK(std::string(setResultName(SetResult::OutOfRange)) == "out_of_range");
  CHECK(std::string(setResultName(SetResult::ReadOnly)) == "read_only");
  CHECK(std::string(setResultName(static_cast<SetResult>(99))) == "unknown");
  CHECK(std::string(patchResultName(PatchResult::Ok)) == "ok");
  CHECK(std::string(patchResultName(PatchResult::Malformed)) == "malformed");
  CHECK(std::string(patchResultName(PatchResult::UnknownKey)) == "unknown_key");
  CHECK(std::string(patchResultName(PatchResult::WrongType)) == "wrong_type");
  CHECK(std::string(patchResultName(PatchResult::OutOfRange)) == "out_of_range");
  CHECK(std::string(patchResultName(PatchResult::ReadOnly)) == "read_only");
  CHECK(std::string(patchResultName(PatchResult::Invalid)) == "invalid");
  CHECK(std::string(patchResultName(static_cast<PatchResult>(99))) == "unknown");
}

// ---------------------------------------------------------------- setter: paths

TEST_CASE("config: setter path parsing") {
  Config c;
  CHECK(set(c, nullptr, I(1)) == SetResult::UnknownKey);
  CHECK(set(c, "", I(1)) == SetResult::UnknownKey);
  CHECK(set(c, "nope", I(1)) == SetResult::UnknownKey);
  CHECK(set(c, "net", I(1)) == SetResult::UnknownKey);
  CHECK(set(c, "net.", I(1)) == SetResult::UnknownKey);
  CHECK(set(c, ".net.iface", I(1)) == SetResult::UnknownKey);
  CHECK(set(c, "net..iface", I(1)) == SetResult::UnknownKey);
  CHECK(set(c, "net.iface.x", I(1)) == SetResult::UnknownKey);
  CHECK(set(c, "net.nope", I(1)) == SetResult::UnknownKey);
  CHECK(set(c, "Net.iface", I(1)) == SetResult::UnknownKey);
  CHECK(set(c, "net.ifac", I(1)) == SetResult::UnknownKey);
  CHECK(set(c, "net.ifacee", I(1)) == SetResult::UnknownKey);
  CHECK(set(c, "station.x", S("a")) == SetResult::UnknownKey);
  CHECK(set(c, "valves", S("a")) == SetResult::UnknownKey);
  CHECK(set(c, "valves.name", S("a")) == SetResult::UnknownKey);
  CHECK(set(c, "valves.1", S("a")) == SetResult::UnknownKey);
  CHECK(set(c, "valves.0.name", S("a")) == SetResult::UnknownKey);
  CHECK(set(c, "valves.01.name", S("a")) == SetResult::UnknownKey);
  CHECK(set(c, "valves.13.name", S("a")) == SetResult::UnknownKey);
  CHECK(set(c, "valves.-1.name", S("a")) == SetResult::UnknownKey);
  CHECK(set(c, "valves.1.name.x", S("a")) == SetResult::UnknownKey);
  CHECK(set(c, "valves.1.nope", S("a")) == SetResult::UnknownKey);
  CHECK(set(c, "temps.35.name", S("a")) == SetResult::UnknownKey);
  CHECK(set(c, "volts.9.name", S("a")) == SetResult::UnknownKey);
  CHECK(set(c, "calib.1.hour", I(1)) == SetResult::UnknownKey);
  CHECK(set(c, "persistLog.x", B(true)) == SetResult::UnknownKey);
  CHECK(set(c, "clearSecrets", B(true)) == SetResult::UnknownKey);
  CHECK(set(c, "stationSet", B(true)) == SetResult::UnknownKey);
  CHECK(set(c, "net.ssidSet", B(true)) == SetResult::UnknownKey);
  // Path length limit: 64 chars.
  std::string longPath = "valves.1.name";
  longPath += std::string(51, 'x');
  CHECK(longPath.size() == 64);
  CHECK(set(c, longPath.c_str(), S("a")) == SetResult::UnknownKey);
  longPath += "x";
  CHECK(set(c, longPath.c_str(), S("a")) == SetResult::UnknownKey);
  // Array bounds: 1 and N work.
  CHECK(set(c, "valves.1.name", S("a")) == SetResult::Ok);
  CHECK(set(c, "valves.12.name", S("b")) == SetResult::Ok);
  CHECK(set(c, "temps.34.name", S("t")) == SetResult::Ok);
  CHECK(set(c, "volts.8.name", S("v")) == SetResult::Ok);
  CHECK(std::string(c.valves[0].name) == "a");
  CHECK(std::string(c.valves[11].name) == "b");
  CHECK(std::string(c.temps[33].name) == "t");
  CHECK(std::string(c.volts[7].name) == "v");
  // Nothing else was touched.
  Config d;
  d.valves[0] = c.valves[0];
  d.valves[11] = c.valves[11];
  d.temps[33] = c.temps[33];
  d.volts[7] = c.volts[7];
  CHECK(sameConfig(c, d));
}

TEST_CASE("config: schema is read-only except for the known versions") {
  Config c;
  CHECK(set(c, "schema", I(1)) == SetResult::Ok);  // a 2.0.0 export
  CHECK(set(c, "schema", S("1")) == SetResult::Ok);
  CHECK(set(c, "schema", F(1.0)) == SetResult::Ok);
  CHECK(set(c, "schema", I(2)) == SetResult::Ok);
  CHECK(set(c, "schema", I(3)) == SetResult::ReadOnly);
  CHECK(set(c, "schema", I(0)) == SetResult::ReadOnly);
  CHECK(set(c, "schema", I(-1)) == SetResult::ReadOnly);
  CHECK(set(c, "schema", S("x")) == SetResult::ReadOnly);
  CHECK(set(c, "schema", B(true)) == SetResult::ReadOnly);
  CHECK(set(c, "schema.x", I(1)) == SetResult::UnknownKey);
  CHECK(c.schema == 2);
  // No-op: the posted value is not stored.
  CHECK(set(c, "schema", I(1)) == SetResult::Ok);
  CHECK(c.schema == 2);
}

// ---------------------------------------------------------------- setter: numbers

namespace {

struct IntKey {
  const char* path;
  int64_t min, max;
};

int64_t readIntKey(const Config& c, const std::string& path) {
  if (path == "net.iface") return static_cast<int64_t>(c.net.iface);
  if (path == "net.reconnectTimeoutMin") return c.net.reconnectTimeoutMin;
  if (path == "syslog.level") return c.syslog.level;
  if (path == "syslog.port") return c.syslog.port;
  if (path == "mqtt.mode") return static_cast<int64_t>(c.mqtt.mode);
  if (path == "mqtt.port") return c.mqtt.port;
  if (path == "mqtt.keepAliveS") return c.mqtt.keepAliveS;
  if (path == "mqtt.publishIntervalS") return c.mqtt.publishIntervalS;
  if (path == "mqtt.minDelayS") return c.mqtt.minDelayS;
  if (path == "calib.dayMask") return c.calib.dayMask;
  if (path == "calib.hour") return c.calib.hour;
  if (path == "calib.minute") return c.calib.minute;
  FAIL("unknown key");
  return -1;
}

}  // namespace

TEST_CASE("config: every integer key accepts exactly its range") {
  const IntKey keys[] = {
      {"net.iface", 0, 2},          {"net.reconnectTimeoutMin", 0, 240},
      {"syslog.level", 0, 3},       {"syslog.port", 1, 65535},
      {"mqtt.mode", 0, 2},          {"mqtt.port", 1, 65535},
      {"mqtt.keepAliveS", 5, 300},  {"mqtt.publishIntervalS", 2, 3600},
      {"mqtt.minDelayS", 0, 3600},  {"calib.dayMask", 0, 127},
      {"calib.hour", 0, 23},        {"calib.minute", 0, 59},
  };
  for (const IntKey& k : keys) {
    CAPTURE(k.path);
    Config c;
    const int64_t before = readIntKey(c, k.path);
    CHECK(set(c, k.path, I(k.min - 1)) == SetResult::OutOfRange);
    CHECK(set(c, k.path, I(k.max + 1)) == SetResult::OutOfRange);
    CHECK(set(c, k.path, I(-4294967296ll)) == SetResult::OutOfRange);
    CHECK(set(c, k.path, I(INT64_MAX)) == SetResult::OutOfRange);
    CHECK(readIntKey(c, k.path) == before);
    CHECK(set(c, k.path, I(k.min)) == SetResult::Ok);
    CHECK(readIntKey(c, k.path) == k.min);
    CHECK(set(c, k.path, I(k.max)) == SetResult::Ok);
    CHECK(readIntKey(c, k.path) == k.max);
    // Numbers as strings (legacy UI) and integral floats.
    const std::string minText = std::to_string(k.min);
    CHECK(set(c, k.path, S(minText.c_str())) == SetResult::Ok);
    CHECK(readIntKey(c, k.path) == k.min);
    CHECK(set(c, k.path, F(static_cast<double>(k.max))) == SetResult::Ok);
    CHECK(readIntKey(c, k.path) == k.max);
    const std::string over = std::to_string(k.max + 1);
    CHECK(set(c, k.path, S(over.c_str())) == SetResult::OutOfRange);
    CHECK(set(c, k.path, F(k.max + 1.0)) == SetResult::OutOfRange);
    // Wrong types.
    CHECK(set(c, k.path, F(k.min + 0.5)) == SetResult::WrongType);
    CHECK(set(c, k.path, B(true)) == SetResult::WrongType);
    CHECK(set(c, k.path, N()) == SetResult::WrongType);
    CHECK(set(c, k.path, S("")) == SetResult::WrongType);
    CHECK(set(c, k.path, S(" 3")) == SetResult::WrongType);
    CHECK(set(c, k.path, S("0x3")) == SetResult::WrongType);
    CHECK(set(c, k.path, S("+3")) == SetResult::WrongType);
    CHECK(set(c, k.path, S("abc")) == SetResult::WrongType);
    CHECK(set(c, k.path, F(NAN)) == SetResult::OutOfRange);
    CHECK(set(c, k.path, F(INFINITY)) == SetResult::OutOfRange);
    CHECK(set(c, k.path, F(-INFINITY)) == SetResult::OutOfRange);
    CHECK(readIntKey(c, k.path) == k.max);
  }
}

TEST_CASE("config: integer conversion corner cases") {
  Config c;
  CHECK(set(c, "calib.hour", S("7.0")) == SetResult::Ok);
  CHECK(c.calib.hour == 7);
  CHECK(set(c, "calib.hour", S("1e1")) == SetResult::Ok);
  CHECK(c.calib.hour == 10);
  CHECK(set(c, "calib.hour", S("-0")) == SetResult::Ok);
  CHECK(c.calib.hour == 0);
  CHECK(set(c, "calib.hour", S("7.5")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S("07")) == SetResult::WrongType);  // JSON number grammar
  CHECK(set(c, "calib.hour", S("7.")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S(".5")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S("1e")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S("-")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", SL("5\0", 2)) == SetResult::WrongType);
  // Only `len` bytes are parsed, in every part of the grammar.
  CHECK(set(c, "calib.hour", SL("123", 2)) == SetResult::Ok);
  CHECK(c.calib.hour == 12);
  CHECK(set(c, "calib.hour", SL("1e12", 3)) == SetResult::Ok);
  CHECK(c.calib.hour == 10);
  CHECK(set(c, "temps.1.offset", SL("1.55", 3)) == SetResult::Ok);
  CHECK(c.temps[0].offset == 15);
  CHECK(set(c, "temps.1.offset", S("0.9")) == SetResult::Ok);
  CHECK(c.temps[0].offset == 9);
  CHECK(set(c, "temps.1.offset", S("1.09e0")) == SetResult::Ok);
  CHECK(c.temps[0].offset == 11);
  CHECK(set(c, "calib.hour", S("9e0")) == SetResult::Ok);
  CHECK(c.calib.hour == 9);
  CHECK(set(c, "calib.hour", S("19")) == SetResult::Ok);
  CHECK(c.calib.hour == 19);
  CHECK(set(c, "calib.hour", S("-5")) == SetResult::OutOfRange);
  CHECK(set(c, "calib.hour", S("--5")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S("1e+1")) == SetResult::Ok);
  CHECK(set(c, "calib.hour", S("1E1")) == SetResult::Ok);
  CHECK(set(c, "calib.hour", S("1e/")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S("1./")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S("1:")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S("/1")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S(":1")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S("e5")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S(".5e1")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S("-e5")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S("1.0:")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S("1.0/")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S("1e1:")) == SetResult::WrongType);
  CHECK(set(c, "calib.hour", S("1e1/")) == SetResult::WrongType);
  CHECK(set(c, "mqtt.port", F(4294967295.0)) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.port", F(4294967296.0)) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.port", F(-2147483648.0)) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.port", F(-2147483649.0)) == SetResult::OutOfRange);
  // 41+ char number text -> infinite -> out of range, not a crash.
  const std::string longNum(50, '9');
  CHECK(set(c, "mqtt.port", S(longNum.c_str())) == SetResult::OutOfRange);
  // Exactly 40 chars are converted, 41 count as infinite.
  const std::string tiny40 = "0." + std::string(37, '0') + "1";
  REQUIRE(tiny40.size() == 40);
  CHECK(set(c, "volts.1.offset", S(tiny40.c_str())) == SetResult::Ok);
  CHECK(c.volts[0].offset == 1e-38f);
  const std::string tiny41 = "0." + std::string(38, '0') + "1";
  CHECK(set(c, "volts.1.offset", S(tiny41.c_str())) == SetResult::OutOfRange);
  const std::string neg41 = "-" + std::string(40, '1');
  CHECK(set(c, "volts.1.offset", S(neg41.c_str())) == SetResult::OutOfRange);
  ConfigValue nullStr;
  nullStr.type = ConfigValue::Type::String;
  CHECK(set(c, "mqtt.port", nullStr) == SetResult::WrongType);
  CHECK(set(c, "mqtt.separate", nullStr) == SetResult::WrongType);
  CHECK(set(c, "station", nullStr) == SetResult::WrongType);
  CHECK(set(c, "net.ip", nullStr) == SetResult::WrongType);
  CHECK(set(c, "temps.1.id", nullStr) == SetResult::WrongType);
  CHECK(set(c, "temps.1.offset", nullStr) == SetResult::WrongType);
  CHECK(c.mqtt.port == 1883);
}

TEST_CASE("config: every bool key and its conversions") {
  const char* keys[] = {"net.dhcp",         "web.protectRead",     "mqtt.separate",
                        "mqtt.allTemps",    "mqtt.pathAsRoot",     "mqtt.upTime",
                        "mqtt.onChange",    "mqtt.retained",       "mqtt.plainText",
                        "mqtt.diag",        "mqtt.germanDecimal",  "mqtt.newDiag",
                        "mqtt.events",      "mqtt.haDiscoveryOnConnect",
                        "valves.5.active",  "temps.7.active",      "volts.3.active",
                        "persistLog"};
  for (const char* k : keys) {
    CAPTURE(k);
    Config c;
    const std::string needleTrue = "true";
    CHECK(set(c, k, B(true)) == SetResult::Ok);
    Config t = c;
    CHECK(set(c, k, B(false)) == SetResult::Ok);
    Config f = c;
    CHECK_FALSE(sameConfig(t, f));
    CHECK(set(c, k, I(1)) == SetResult::Ok);
    CHECK(sameConfig(c, t));
    CHECK(set(c, k, I(0)) == SetResult::Ok);
    CHECK(sameConfig(c, f));
    CHECK(set(c, k, S("true")) == SetResult::Ok);
    CHECK(sameConfig(c, t));
    CHECK(set(c, k, S("false")) == SetResult::Ok);
    CHECK(sameConfig(c, f));
    CHECK(set(c, k, S("1")) == SetResult::Ok);
    CHECK(sameConfig(c, t));
    CHECK(set(c, k, S("0")) == SetResult::Ok);
    CHECK(sameConfig(c, f));
    CHECK(set(c, k, F(1.0)) == SetResult::Ok);
    CHECK(sameConfig(c, t));
    CHECK(set(c, k, I(2)) == SetResult::OutOfRange);
    CHECK(set(c, k, I(-1)) == SetResult::OutOfRange);
    CHECK(set(c, k, F(0.5)) == SetResult::WrongType);
    CHECK(set(c, k, S("yes")) == SetResult::WrongType);
    CHECK(set(c, k, S("True")) == SetResult::WrongType);
    CHECK(set(c, k, S("truex")) == SetResult::WrongType);
    CHECK(set(c, k, S("trux")) == SetResult::WrongType);
    CHECK(set(c, k, SL("truex", 4)) == SetResult::Ok);  // only len bytes count
    CHECK(set(c, k, SL("falsex", 5)) == SetResult::Ok);
    CHECK(sameConfig(c, f));
    CHECK(set(c, k, B(true)) == SetResult::Ok);
    CHECK(set(c, k, S("falsx")) == SetResult::WrongType);
    CHECK(set(c, k, S("fals")) == SetResult::WrongType);
    CHECK(set(c, k, S("11")) == SetResult::WrongType);
    CHECK(set(c, k, S("00")) == SetResult::WrongType);
    CHECK(set(c, k, S("2")) == SetResult::WrongType);
    CHECK(set(c, k, S("")) == SetResult::WrongType);
    CHECK(set(c, k, N()) == SetResult::WrongType);
    CHECK(sameConfig(c, t));
  }
}

// ---------------------------------------------------------------- setter: strings

TEST_CASE("config: station name") {
  Config c;
  CHECK(set(c, "station", S("")) == SetResult::OutOfRange);
  CHECK(set(c, "station", S("a")) == SetResult::Ok);
  CHECK(std::string(c.station) == "a");
  CHECK(set(c, "station", S("12345678901234567890")) == SetResult::Ok);
  CHECK(std::string(c.station) == "12345678901234567890");
  CHECK(set(c, "station", S("123456789012345678901")) == SetResult::OutOfRange);
  CHECK(set(c, "station", S("a/b")) == SetResult::OutOfRange);
  CHECK(set(c, "station", S("a+b")) == SetResult::OutOfRange);
  CHECK(set(c, "station", S("a#b")) == SetResult::OutOfRange);
  CHECK(set(c, "station", S("a\"b")) == SetResult::OutOfRange);
  CHECK(set(c, "station", S("a\\b")) == SetResult::OutOfRange);
  CHECK(set(c, "station", S("a\tb")) == SetResult::OutOfRange);
  CHECK(set(c, "station", S("a\x7f")) == SetResult::OutOfRange);
  CHECK(set(c, "station", S("a\xc2\x85")) == SetResult::OutOfRange);  // C1 control
  CHECK(set(c, "station", S("a\xc3")) == SetResult::OutOfRange);      // truncated UTF-8
  CHECK(set(c, "station", S("a\xe4")) == SetResult::OutOfRange);      // Latin-1, not UTF-8
  CHECK(set(c, "station", SL("ab\0c", 4)) == SetResult::OutOfRange);
  CHECK(set(c, "station", I(5)) == SetResult::WrongType);
  CHECK(set(c, "station", B(true)) == SetResult::WrongType);
  CHECK(std::string(c.station) == "12345678901234567890");
  // Legacy names are kept as they are: UTF-8 and spaces at either end.
  CHECK(set(c, "station", S("Fu\xc3\x9f" "boden")) == SetResult::Ok);
  CHECK(std::string(c.station) == "Fu\xc3\x9f" "boden");
  CHECK(set(c, "station", S(" ab")) == SetResult::Ok);
  CHECK(set(c, "station", S("ab ")) == SetResult::Ok);
  CHECK(std::string(c.station) == "ab ");
  // 20 bytes: 10 two-byte characters fit, the 21st byte does not.
  CHECK(set(c, "station", S("\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4"
                            "\xc3\xa4\xc3\xa4")) == SetResult::Ok);
  CHECK(set(c, "station", S("x\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4\xc3\xa4"
                            "\xc3\xa4\xc3\xa4")) == SetResult::OutOfRange);
  CHECK(set(c, "station", S("12345678901234567890")) == SetResult::Ok);
  // Only `len` bytes are used.
  CHECK(set(c, "station", SL("abcdef", 3)) == SetResult::Ok);
  CHECK(std::string(c.station) == "abc");
  CHECK(set(c, "station", S("a b")) == SetResult::Ok);
}

TEST_CASE("config: item names and units") {
  Config c;
  CHECK(set(c, "valves.1.name", S("")) == SetResult::Ok);
  CHECK(set(c, "valves.1.name", S("1234567890")) == SetResult::Ok);
  CHECK(set(c, "valves.1.name", S("12345678901")) == SetResult::OutOfRange);
  CHECK(set(c, "temps.1.name", S("1234567890")) == SetResult::Ok);
  CHECK(set(c, "temps.1.name", S("12345678901")) == SetResult::OutOfRange);
  CHECK(set(c, "volts.1.name", S("1234567890")) == SetResult::Ok);
  CHECK(set(c, "volts.1.name", S("12345678901")) == SetResult::OutOfRange);
  CHECK(set(c, "volts.1.name", S("a/b")) == SetResult::OutOfRange);
  CHECK(set(c, "volts.1.unit", S("12345678")) == SetResult::Ok);
  CHECK(std::string(c.volts[0].unit) == "12345678");
  CHECK(set(c, "volts.1.unit", S("123456789")) == SetResult::OutOfRange);
  CHECK(set(c, "volts.1.unit", S("")) == SetResult::Ok);
  CHECK(set(c, "volts.1.unit", S("m#")) == SetResult::OutOfRange);
  CHECK(set(c, "valves.2.name", S(" x")) == SetResult::Ok);  // legacy: segment "_x"
  CHECK(set(c, "valves.2.name", S("K\xc3\xbc" "che")) == SetResult::Ok);
  CHECK(std::string(c.valves[1].name) == "K\xc3\xbc" "che");
  CHECK(set(c, "volts.1.unit", S("\xc2\xb0" "C")) == SetResult::Ok);
  CHECK(set(c, "volts.1.unit", S("\xb0" "C")) == SetResult::OutOfRange);
}

TEST_CASE("config: network strings, hosts and time zone") {
  Config c;
  const std::string ssid32(32, 's');
  CHECK(set(c, "net.ssid", S(ssid32.c_str())) == SetResult::Ok);
  CHECK(set(c, "net.ssid", S((ssid32 + "s").c_str())) == SetResult::OutOfRange);
  CHECK(set(c, "net.ssid", S("with space")) == SetResult::Ok);
  CHECK(set(c, "net.ssid", S("tab\t")) == SetResult::OutOfRange);
  CHECK(set(c, "net.ssid", S("del\x7f")) == SetResult::OutOfRange);
  CHECK(set(c, "net.ssid", S("~!")) == SetResult::Ok);
  CHECK(set(c, "net.ssid", S("")) == SetResult::Ok);

  const std::string h64(64, 'h');
  CHECK(set(c, "mqtt.host", S(h64.c_str())) == SetResult::Ok);
  CHECK(set(c, "mqtt.host", S((h64 + "h").c_str())) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.host", S("broker.local")) == SetResult::Ok);
  CHECK(set(c, "mqtt.host", S("192.168.1.2")) == SetResult::Ok);
  CHECK(set(c, "mqtt.host", S("192.168.1.300")) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.host", S("192.168.1")) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.host", S("1")) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.host", S("1host")) == SetResult::Ok);
  CHECK(set(c, "mqtt.host", S("-host")) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.host", S("host.")) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.host", S("ho st")) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.host", S("ho_st")) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.host", S("")) == SetResult::Ok);
  CHECK(std::string(c.mqtt.host).empty());
  CHECK(set(c, "time.ntpServer", S("")) == SetResult::Ok);
  CHECK(set(c, "time.ntpServer", S("de.pool.ntp.org")) == SetResult::Ok);
  CHECK(set(c, "time.ntpServer", S("pool ntp")) == SetResult::OutOfRange);

  const std::string tz49(49, 'z');
  CHECK(set(c, "time.tzName", S("")) == SetResult::Ok);
  CHECK(set(c, "time.tzName", S("America/Argentina/Buenos Aires")) == SetResult::Ok);
  CHECK(set(c, "time.tzName", S(tz49.c_str())) == SetResult::Ok);
  CHECK(set(c, "time.tzName", S((tz49 + "z").c_str())) == SetResult::OutOfRange);
  CHECK(set(c, "time.tzName", S("x\n")) == SetResult::OutOfRange);
  CHECK(set(c, "time.tzPosix", S("")) == SetResult::OutOfRange);
  CHECK(set(c, "time.tzPosix", S("U")) == SetResult::Ok);
  CHECK(set(c, "time.tzPosix", S(tz49.c_str())) == SetResult::Ok);
  CHECK(set(c, "time.tzPosix", S((tz49 + "z").c_str())) == SetResult::OutOfRange);
  CHECK(set(c, "time.tzPosix", S("CET -1")) == SetResult::OutOfRange);
  CHECK(set(c, "time.tzPosix", S("<+0330>-3:30")) == SetResult::Ok);

  const std::string u64(64, 'u');
  CHECK(set(c, "web.user", S(u64.c_str())) == SetResult::Ok);
  CHECK(set(c, "web.user", S((u64 + "u").c_str())) == SetResult::OutOfRange);
  CHECK(set(c, "web.user", S("a:b")) == SetResult::OutOfRange);
  CHECK(set(c, "web.user", S("a b")) == SetResult::Ok);
  CHECK(set(c, "mqtt.user", S(u64.c_str())) == SetResult::Ok);
  CHECK(set(c, "mqtt.user", S((u64 + "u").c_str())) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.user", S("a:b")) == SetResult::Ok);
  CHECK(set(c, "mqtt.user", S("\x01")) == SetResult::OutOfRange);
}

TEST_CASE("config: secrets are write-only and cleared only on request") {
  Config c;
  CHECK(set(c, "web.password", S("pw1")) == SetResult::Ok);
  CHECK(std::string(c.web.password) == "pw1");
  CHECK(set(c, "web.password", S("")) == SetResult::Ok);
  CHECK(std::string(c.web.password) == "pw1");
  CHECK(set(c, "web.password", S(""), true) == SetResult::Ok);
  CHECK(std::string(c.web.password).empty());

  const std::string p63(63, 'p');
  CHECK(set(c, "net.wifiPassword", S(p63.c_str())) == SetResult::Ok);
  CHECK(set(c, "net.wifiPassword", S((p63 + "p").c_str())) == SetResult::OutOfRange);
  CHECK(std::string(c.net.wifiPassword) == p63);
  CHECK(set(c, "net.wifiPassword", S(""), true) == SetResult::Ok);
  CHECK(std::string(c.net.wifiPassword).empty());

  const std::string p64(64, 'p');
  CHECK(set(c, "mqtt.password", S(p64.c_str())) == SetResult::Ok);
  CHECK(set(c, "mqtt.password", S((p64 + "p").c_str())) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.password", S("\x7f")) == SetResult::OutOfRange);
  CHECK(set(c, "mqtt.password", S("a:b \"c\"")) == SetResult::Ok);
  CHECK(set(c, "mqtt.password", I(3)) == SetResult::WrongType);
  CHECK(set(c, "web.password", S((p64 + "p").c_str())) == SetResult::OutOfRange);

  // Export flags are accepted as no-ops (bool only).
  const Config before = c;
  CHECK(set(c, "web.passwordSet", B(false)) == SetResult::Ok);
  CHECK(set(c, "net.wifiPasswordSet", B(true)) == SetResult::Ok);
  CHECK(set(c, "mqtt.passwordSet", B(true)) == SetResult::Ok);
  CHECK(sameConfig(c, before));
  CHECK(set(c, "mqtt.passwordSet", I(1)) == SetResult::WrongType);
  CHECK(set(c, "mqtt.passwordSe", B(true)) == SetResult::UnknownKey);
  CHECK(set(c, "mqtt.passwordSett", B(true)) == SetResult::UnknownKey);
  CHECK(set(c, "mqtt.hostSet", B(true)) == SetResult::UnknownKey);
  CHECK(set(c, "mqtt.passwordXet", B(true)) == SetResult::UnknownKey);
  CHECK(set(c, "mqtt.passwordSex", B(true)) == SetResult::UnknownKey);
  CHECK(set(c, "mqtt.passwordSxt", B(true)) == SetResult::UnknownKey);
  // Neighbouring fields survive a maximum-length write.
  Config n;
  CHECK(set(n, "net.wifiPassword", S("12345678")) == SetResult::Ok);
  CHECK(set(n, "net.ssid", S(std::string(32, 's').c_str())) == SetResult::Ok);
  CHECK(std::string(n.net.wifiPassword) == "12345678");
  CHECK(set(n, "web.user", S(std::string(64, 'u').c_str())) == SetResult::Ok);
  CHECK(set(n, "web.password", S(std::string(64, 'p').c_str())) == SetResult::Ok);
  CHECK(std::string(n.web.user) == std::string(64, 'u'));
  CHECK_FALSE(n.web.protectRead);
  // A one-char secret counts as set in the export.
  Config o;
  CHECK(set(o, "mqtt.password", S("x")) == SetResult::Ok);
  CHECK(exportJson(o).find("\"passwordSet\":true,\"keepAliveS\"") != std::string::npos);
}

// ---------------------------------------------------------------- setter: addresses and numbers

TEST_CASE("config: IPv4 keys") {
  Config c;
  CHECK(set(c, "net.ip", S("192.168.1.2")) == SetResult::Ok);
  CHECK(c.net.ip == 0x0201A8C0u);
  CHECK(set(c, "net.gateway", S("255.255.255.255")) == SetResult::Ok);
  CHECK(c.net.gateway == 0xFFFFFFFFu);
  CHECK(set(c, "net.dns", S("1.2.3.4")) == SetResult::Ok);
  CHECK(c.net.dns == 0x04030201u);
  CHECK(set(c, "syslog.server", S("10.0.0.1")) == SetResult::Ok);
  CHECK(c.syslog.server == 0x0100000Au);
  CHECK(set(c, "net.ip", S("")) == SetResult::Ok);
  CHECK(c.net.ip == 0);
  CHECK(set(c, "net.ip", S("1.2.3")) == SetResult::OutOfRange);
  CHECK(set(c, "net.ip", S("1.2.3.256")) == SetResult::OutOfRange);
  CHECK(set(c, "net.ip", S("host")) == SetResult::OutOfRange);
  CHECK(set(c, "net.ip", I(5)) == SetResult::WrongType);
  CHECK(c.net.ip == 0);

  const char* goodMasks[] = {"0.0.0.0",       "128.0.0.0",       "255.0.0.0",
                             "255.255.0.0",   "255.255.255.0",   "255.255.255.128",
                             "255.255.255.254", "255.255.255.255", "255.255.240.0"};
  for (const char* m : goodMasks) {
    CAPTURE(m);
    CHECK(set(c, "net.mask", S(m)) == SetResult::Ok);
  }
  CHECK(c.net.mask == 0x00F0FFFFu);
  const char* badMasks[] = {"0.0.0.255", "255.0.255.0", "255.255.255.253", "254.255.255.0",
                            "0.255.255.255", "1.0.0.0"};
  for (const char* m : badMasks) {
    CAPTURE(m);
    CHECK(set(c, "net.mask", S(m)) == SetResult::OutOfRange);
  }
  CHECK(c.net.mask == 0x00F0FFFFu);
}

TEST_CASE("config: temperature offset in tenths, rounded half away from zero") {
  Config c;
  struct Case {
    double in;
    int16_t out;
  };
  const Case cases[] = {{0.0, 0},    {0.04, 0},   {0.05, 1},   {0.15, 2},  {-0.15, -2},
                        {0.7, 7},    {-0.7, -7},  {0.24, 2},   {0.25, 3},  {-0.25, -3},
                        {9.94, 99},  {10.0, 100}, {10.04, 100}, {-10.0, -100}, {-10.04, -100},
                        {-0.04, 0}};
  for (const Case& k : cases) {
    CAPTURE(k.in);
    CHECK(set(c, "temps.2.offset", F(k.in)) == SetResult::Ok);
    CHECK(c.temps[1].offset == k.out);
  }
  CHECK(set(c, "temps.2.offset", I(3)) == SetResult::Ok);
  CHECK(c.temps[1].offset == 30);
  CHECK(set(c, "temps.2.offset", S("-2.5")) == SetResult::Ok);
  CHECK(c.temps[1].offset == -25);
  CHECK(set(c, "temps.2.offset", F(10.05)) == SetResult::OutOfRange);
  CHECK(set(c, "temps.2.offset", F(-10.05)) == SetResult::OutOfRange);
  CHECK(set(c, "temps.2.offset", I(11)) == SetResult::OutOfRange);
  CHECK(set(c, "temps.2.offset", I(-11)) == SetResult::OutOfRange);
  CHECK(set(c, "temps.2.offset", F(3000.0)) == SetResult::OutOfRange);
  CHECK(set(c, "temps.2.offset", F(3000.1)) == SetResult::OutOfRange);
  CHECK(set(c, "temps.2.offset", F(1e300)) == SetResult::OutOfRange);
  CHECK(set(c, "temps.2.offset", F(NAN)) == SetResult::OutOfRange);
  CHECK(set(c, "temps.2.offset", F(-INFINITY)) == SetResult::OutOfRange);
  CHECK(set(c, "temps.2.offset", S("x")) == SetResult::WrongType);
  CHECK(set(c, "temps.2.offset", B(true)) == SetResult::WrongType);
  CHECK(set(c, "temps.2.offset", N()) == SetResult::WrongType);
  CHECK(c.temps[1].offset == -25);
}

TEST_CASE("config: volt offset and factor") {
  Config c;
  CHECK(set(c, "volts.1.offset", F(1000.0)) == SetResult::Ok);
  CHECK(c.volts[0].offset == 1000.0f);
  CHECK(set(c, "volts.1.offset", F(-1000.0)) == SetResult::Ok);
  CHECK(set(c, "volts.1.offset", F(1000.001)) == SetResult::OutOfRange);
  CHECK(set(c, "volts.1.offset", F(-1000.001)) == SetResult::OutOfRange);
  CHECK(set(c, "volts.1.offset", F(0.0)) == SetResult::Ok);
  CHECK(set(c, "volts.1.offset", I(-3)) == SetResult::Ok);
  CHECK(c.volts[0].offset == -3.0f);
  CHECK(set(c, "volts.1.offset", S("0.125")) == SetResult::Ok);
  CHECK(c.volts[0].offset == 0.125f);
  CHECK(set(c, "volts.1.offset", F(NAN)) == SetResult::OutOfRange);
  CHECK(set(c, "volts.1.offset", F(INFINITY)) == SetResult::OutOfRange);
  CHECK(set(c, "volts.1.offset", B(false)) == SetResult::WrongType);
  CHECK(c.volts[0].offset == 0.125f);

  CHECK(set(c, "volts.1.factor", F(0.0)) == SetResult::OutOfRange);
  CHECK(set(c, "volts.1.factor", F(1e-50)) == SetResult::OutOfRange);  // becomes 0.0f
  CHECK(set(c, "volts.1.factor", I(0)) == SetResult::OutOfRange);
  CHECK(set(c, "volts.1.factor", F(-0.001)) == SetResult::Ok);
  CHECK(c.volts[0].factor == -0.001f);
  CHECK(set(c, "volts.1.factor", F(1000.0)) == SetResult::Ok);
  CHECK(set(c, "volts.1.factor", F(-1000.0)) == SetResult::Ok);
  CHECK(set(c, "volts.1.factor", F(1001.0)) == SetResult::OutOfRange);
  CHECK(set(c, "volts.1.factor", F(1000.001)) == SetResult::OutOfRange);
  CHECK(set(c, "volts.1.factor", F(-1000.001)) == SetResult::OutOfRange);
  CHECK(c.volts[0].factor == -1000.0f);
}

TEST_CASE("config: 1-Wire id keys") {
  Config c;
  CHECK(set(c, "temps.3.id", S("28-84-37-94-97-FF-03-23")) == SetResult::Ok);
  CHECK(c.temps[2].id == oid(kIdA));
  CHECK(set(c, "volts.3.id", S(kIdV)) == SetResult::Ok);
  CHECK(c.volts[2].id == oid(kIdV));
  CHECK(set(c, "temps.3.id", S("28-84-37-94-97-ff-03")) == SetResult::OutOfRange);
  CHECK(set(c, "temps.3.id", S("28-84-37-94-97-ff-03-2g")) == SetResult::OutOfRange);
  CHECK(set(c, "temps.3.id", S("28:84:37:94:97:ff:03:23")) == SetResult::OutOfRange);
  CHECK(set(c, "temps.3.id", I(1)) == SetResult::WrongType);
  CHECK(c.temps[2].id == oid(kIdA));
  CHECK(set(c, "temps.3.id", S("00-00-00-00-00-00-00-00")) == SetResult::Ok);
  CHECK(isZero(c.temps[2].id));
  CHECK(set(c, "temps.3.id", S(kIdB)) == SetResult::Ok);
  CHECK(set(c, "temps.3.id", S("")) == SetResult::Ok);
  CHECK(isZero(c.temps[2].id));
}

// ---------------------------------------------------------------- validation

TEST_CASE("config: validation reports every cross-field rule with its path") {
  Config c;
  c.net.dhcp = false;
  CHECK(validatePath(c) == "net.ip");
  c.net.ip = 1;
  CHECK(validatePath(c) == "net.mask");
  c.net.mask = 0x00FFFFFF;
  CHECK(validatePath(c) == "net.gateway");
  c.net.gateway = 1;
  CHECK(validatePath(c) == "OK");
  c.net.dhcp = true;
  c.net.ip = 0;
  c.net.mask = 0;
  c.net.gateway = 0;
  CHECK(validatePath(c) == "OK");

  strcpy(c.net.ssid, "w");
  CHECK(validatePath(c) == "OK");  // open network
  strcpy(c.net.wifiPassword, "1");
  CHECK(validatePath(c) == "net.wifiPassword");
  strcpy(c.net.ssid, "wlan");
  strcpy(c.net.wifiPassword, "1234567");
  CHECK(validatePath(c) == "net.wifiPassword");
  strcpy(c.net.wifiPassword, "12345678");
  CHECK(validatePath(c) == "OK");
  c.net.iface = NetInterface::Wifi;
  CHECK(validatePath(c) == "OK");
  c.net.ssid[0] = '\0';
  CHECK(validatePath(c) == "net.ssid");
  c.net.iface = NetInterface::Ethernet;
  CHECK(validatePath(c) == "OK");  // password without ssid is harmless

  c.syslog.level = 1;
  CHECK(validatePath(c) == "syslog.server");
  c.net.iface = NetInterface::Wifi;
  strcpy(c.net.ssid, "w");
  c.syslog.level = 0;
  CHECK(validatePath(c) == "OK");  // one-char ssid counts as set
  c.net.iface = NetInterface::Ethernet;
  c.syslog.level = 1;
  c.syslog.server = 5;
  CHECK(validatePath(c) == "OK");
  c.syslog.level = 0;
  c.syslog.server = 0;

  strcpy(c.web.user, "u");
  CHECK(validatePath(c) == "web.password");
  strcpy(c.web.password, "p");
  CHECK(validatePath(c) == "OK");
  c.web.user[0] = '\0';
  CHECK(validatePath(c) == "web.user");
  c.web.password[0] = '\0';
  CHECK(validatePath(c) == "OK");

  c.mqtt.mode = MqttMode::Mqtt;
  CHECK(validatePath(c) == "mqtt.host");
  strcpy(c.mqtt.host, "b");
  CHECK(validatePath(c) == "OK");
  c.mqtt.minDelayS = 11;
  CHECK(validatePath(c) == "mqtt.minDelayS");
  c.mqtt.minDelayS = 10;
  CHECK(validatePath(c) == "OK");
  c.mqtt.separate = false;
  CHECK(validatePath(c) == "OK");
  c.mqtt.mode = MqttMode::MqttHa;
  CHECK(validatePath(c) == "mqtt.mode");
  c.mqtt.separate = true;
  CHECK(validatePath(c) == "OK");
  c.mqtt.mode = MqttMode::Off;
  c.mqtt.host[0] = '\0';
  CHECK(validatePath(c) == "OK");
}

TEST_CASE("config: valve names must give unique MQTT segments") {
  Config c;
  strcpy(c.valves[0].name, "Bad");
  strcpy(c.valves[4].name, "Bad");
  CHECK(validatePath(c) == "valves.5.name");
  strcpy(c.valves[4].name, "bad");
  CHECK(validatePath(c) == "OK");
  strcpy(c.valves[0].name, "a b");
  strcpy(c.valves[4].name, "a_b");
  CHECK(validatePath(c) == "valves.5.name");
  strcpy(c.valves[4].name, "a_c");
  CHECK(validatePath(c) == "OK");

  Config n;
  strcpy(n.valves[4].name, "3");  // valve 3 is unnamed -> "3" is its segment
  CHECK(validatePath(n) == "valves.5.name");
  strcpy(n.valves[2].name, "x");  // now valve 3 publishes as "x"
  CHECK(validatePath(n) == "OK");
  n.valves[2].name[0] = '\0';
  strcpy(n.valves[4].name, "5");  // own number
  CHECK(validatePath(n) == "OK");
  strcpy(n.valves[4].name, "12");
  CHECK(validatePath(n) == "valves.5.name");
  strcpy(n.valves[4].name, "1");
  CHECK(validatePath(n) == "valves.5.name");
  strcpy(n.valves[4].name, "123");
  CHECK(validatePath(n) == "OK");
  strcpy(n.valves[4].name, "13");
  CHECK(validatePath(n) == "OK");
  strcpy(n.valves[4].name, "03");
  CHECK(validatePath(n) == "OK");
  strcpy(n.valves[4].name, "0");
  CHECK(validatePath(n) == "OK");
  strcpy(n.valves[11].name, "12");
  strcpy(n.valves[4].name, "x");
  CHECK(validatePath(n) == "OK");
  strcpy(n.valves[0].name, "12");  // valve 12 is named "12" -> duplicate of it
  CHECK(validatePath(n) == "valves.12.name");
}

TEST_CASE("config: uniqueness checks cover every index pair") {
  Config c;
  strcpy(c.valves[2].name, "dup");
  strcpy(c.valves[5].name, "dup");
  CHECK(validatePath(c) == "valves.6.name");
  Config d;
  strcpy(d.valves[0].name, "5");  // valve 1 named like unnamed valve 5
  CHECK(validatePath(d) == "valves.1.name");
  strcpy(d.valves[11].name, "12x");
  CHECK(validatePath(d) == "valves.1.name");
  Config m;  // item names of other tables never clash with valve names
  strcpy(m.valves[0].name, "dup");
  strcpy(m.temps[0].name, "dup");
  strcpy(m.volts[0].name, "dup");
  CHECK(validatePath(m) == "OK");
  Config z;
  z.temps[0].id = oid(kIdA);
  z.temps[5].id = oid(kIdA);
  CHECK(validatePath(z) == "temps.6.id");
  z.temps[5].id = oid(kIdB);
  z.volts[0].id = oid(kIdV);
  z.volts[4].id = oid(kIdV);
  CHECK(validatePath(z) == "volts.5.id");
  Config t;
  t.temps[0].active = true;
  CHECK(validatePath(t) == "temps.1.active");
  t.temps[0].active = false;
  t.temps[2].id = oid(kIdA);
  t.temps[33].id = oid(kIdA);
  CHECK(validatePath(t) == "temps.34.id");
  Config v;
  v.volts[0].active = true;
  CHECK(validatePath(v) == "volts.1.active");
  v.volts[0].active = false;
  v.volts[3].id = oid(kIdV);
  v.volts[7].id = oid(kIdV);
  CHECK(validatePath(v) == "volts.8.id");
}

TEST_CASE("config: sensor slots need ids and unique ids") {
  Config c;
  c.temps[3].active = true;
  CHECK(validatePath(c) == "temps.4.active");
  c.temps[3].id = oid(kIdA);
  CHECK(validatePath(c) == "OK");
  c.temps[9].id = oid(kIdA);
  CHECK(validatePath(c) == "temps.10.id");
  c.temps[9].id = oid(kIdB);
  CHECK(validatePath(c) == "OK");
  c.volts[0].id = oid(kIdA);  // the same id in the volt table is fine
  CHECK(validatePath(c) == "OK");
  c.volts[7].active = true;
  CHECK(validatePath(c) == "volts.8.active");
  c.volts[7].id = oid(kIdA);
  CHECK(validatePath(c) == "volts.8.id");
  c.volts[7].id = oid(kIdV);
  CHECK(validatePath(c) == "OK");
}

TEST_CASE("config: validation rejects out-of-range stored fields") {
  {
    Config c;
    c.schema = 3;
    CHECK(validatePath(c) == "schema");
    c.schema = 1;  // the JSON schema of a config in RAM is always the current one
    CHECK(validatePath(c) == "schema");
  }
  {
    Config c;
    c.station[0] = '\0';
    CHECK(validatePath(c) == "station");
  }
  {
    Config c;
    memset(c.station, 'a', sizeof c.station);  // not terminated
    CHECK(validatePath(c) == "station");
  }
  {
    Config c;
    memset(c.valves[2].name, 'a', sizeof c.valves[2].name);
    CHECK(validatePath(c) == "valves.3.name");
  }
  {
    Config c;
    c.net.iface = static_cast<NetInterface>(3);
    CHECK(validatePath(c) == "net.iface");
  }
  {
    Config c;
    uint8_t raw = 2;
    memcpy(&c.net.dhcp, &raw, 1);
    CHECK(validatePath(c) == "net.dhcp");
  }
  {
    Config c;
    c.net.mask = 0x00FF00FF;
    CHECK(validatePath(c) == "net.mask");
  }
  {
    Config c;
    c.net.reconnectTimeoutMin = 241;
    CHECK(validatePath(c) == "net.reconnectTimeoutMin");
  }
  {
    Config c;
    strcpy(c.time.tzPosix, "a b");
    CHECK(validatePath(c) == "time.tzPosix");
  }
  {
    Config c;
    strcpy(c.time.ntpServer, "300.1.1.1");
    CHECK(validatePath(c) == "time.ntpServer");
  }
  {
    Config c;
    c.syslog.port = 0;
    CHECK(validatePath(c) == "syslog.port");
  }
  {
    Config c;
    strcpy(c.web.user, "a:b");
    CHECK(validatePath(c) == "web.user");
  }
  {
    Config c;
    c.mqtt.keepAliveS = 4;
    CHECK(validatePath(c) == "mqtt.keepAliveS");
  }
  {
    Config c;
    c.mqtt.keepAliveS = 301;
    CHECK(validatePath(c) == "mqtt.keepAliveS");
  }
  {
    Config c;
    c.mqtt.publishIntervalS = 1;
    CHECK(validatePath(c) == "mqtt.publishIntervalS");
  }
  {
    Config c;
    c.mqtt.mode = static_cast<MqttMode>(3);
    CHECK(validatePath(c) == "mqtt.mode");
  }
  {
    Config c;
    c.temps[33].offset = 101;
    CHECK(validatePath(c) == "temps.34.offset");
  }
  {
    Config c;
    c.temps[33].offset = -101;
    CHECK(validatePath(c) == "temps.34.offset");
  }
  {
    Config c;
    c.volts[1].factor = 0.0f;
    CHECK(validatePath(c) == "volts.2.factor");
  }
  {
    Config c;
    c.volts[1].offset = NAN;
    CHECK(validatePath(c) == "volts.2.offset");
  }
  {
    Config c;
    c.volts[1].offset = 1000.5f;
    CHECK(validatePath(c) == "volts.2.offset");
  }
  {
    Config c;
    c.calib.dayMask = 128;
    CHECK(validatePath(c) == "calib.dayMask");
  }
  {
    Config c;
    c.calib.hour = 24;
    CHECK(validatePath(c) == "calib.hour");
  }
  {
    Config c;
    c.calib.minute = 60;
    CHECK(validatePath(c) == "calib.minute");
  }
  {
    Config c;
    uint8_t raw = 7;
    memcpy(&c.persistLog, &raw, 1);
    CHECK(validatePath(c) == "persistLog");
  }
}

TEST_CASE("config: validation accepts every stored field at both range ends") {
  Config lo, hi;
  lo.mqtt.port = 1;
  lo.mqtt.keepAliveS = 5;
  lo.mqtt.publishIntervalS = 2;
  lo.mqtt.minDelayS = 0;
  lo.syslog.port = 1;
  lo.temps[0].offset = -100;
  lo.volts[0].offset = -1000.0f;
  lo.volts[0].factor = -1000.0f;
  lo.calib.dayMask = 0;
  lo.net.reconnectTimeoutMin = 0;
  strcpy(lo.time.tzPosix, "U");
  strcpy(lo.station, "a");
  CHECK(validatePath(lo) == "OK");
  hi.mqtt.port = 65535;
  hi.mqtt.keepAliveS = 300;
  hi.mqtt.publishIntervalS = 3600;
  hi.mqtt.minDelayS = 3600;
  hi.syslog.port = 65535;
  hi.syslog.level = 3;
  hi.syslog.server = 1;
  hi.net.iface = NetInterface::Wifi;
  strcpy(hi.net.ssid, "12345678901234567890123456789012");
  memset(hi.net.wifiPassword, 'p', 63);
  hi.mqtt.mode = MqttMode::MqttHa;
  strcpy(hi.mqtt.host, "h");
  hi.temps[0].offset = 100;
  hi.volts[0].offset = 1000.0f;
  hi.volts[0].factor = 1000.0f;
  hi.calib.dayMask = 127;
  hi.calib.hour = 23;
  hi.calib.minute = 59;
  hi.net.reconnectTimeoutMin = 240;
  memset(hi.station, 's', 20);
  memset(hi.valves[11].name, 'v', 10);
  memset(hi.volts[7].unit, 'u', 8);
  CHECK(validatePath(hi) == "OK");
  hi.mqtt.keepAliveS = 301;
  CHECK(validatePath(hi) == "mqtt.keepAliveS");
  lo.mqtt.keepAliveS = 4;
  CHECK(validatePath(lo) == "mqtt.keepAliveS");
  hi.mqtt.keepAliveS = 300;
  hi.mqtt.publishIntervalS = 3601;
  CHECK(validatePath(hi) == "mqtt.publishIntervalS");
}

TEST_CASE("config: validation path buffer handling") {
  Config c;
  c.calib.hour = 24;
  CHECK_FALSE(validateConfig(c, nullptr, 0));
  char small[6];
  CHECK_FALSE(validateConfig(c, small, sizeof small));
  CHECK(std::string(small) == "calib");
  char one[1] = {'x'};
  CHECK_FALSE(validateConfig(c, one, 0));
  CHECK(one[0] == 'x');
  CHECK_FALSE(validateConfig(c, one, 1));
  CHECK(one[0] == '\0');
  Config ok;
  char path[8] = "junk";
  CHECK(validateConfig(ok, path, sizeof path));
  CHECK(path[0] == '\0');
}

// ---------------------------------------------------------------- JSON export

TEST_CASE("config: JSON export of the defaults (golden)") {
  std::string expected =
      "{\"schema\":2,\"station\":\"VdMot\","
      "\"net\":{\"iface\":0,\"dhcp\":true,\"ip\":\"0.0.0.0\",\"mask\":\"0.0.0.0\","
      "\"gateway\":\"0.0.0.0\",\"dns\":\"0.0.0.0\",\"ssid\":\"\",\"wifiPasswordSet\":false,"
      "\"reconnectTimeoutMin\":5},"
      "\"time\":{\"ntpServer\":\"pool.ntp.org\",\"tzName\":\"Europe/Berlin\","
      "\"tzPosix\":\"CET-1CEST,M3.5.0,M10.5.0/3\"},"
      "\"syslog\":{\"level\":0,\"server\":\"0.0.0.0\",\"port\":514},"
      "\"web\":{\"user\":\"\",\"passwordSet\":false,\"protectRead\":false,\"allowedHosts\":\"\"},"
      "\"mqtt\":{\"mode\":0,\"host\":\"\",\"port\":1883,\"user\":\"\",\"passwordSet\":false,"
      "\"keepAliveS\":60,\"publishIntervalS\":10,\"minDelayS\":5,\"separate\":true,"
      "\"allTemps\":true,\"pathAsRoot\":false,\"upTime\":true,\"onChange\":true,"
      "\"retained\":true,\"plainText\":true,\"diag\":true,\"germanDecimal\":false,"
      "\"newDiag\":true,\"events\":true,\"haDiscoveryOnConnect\":true,\"rootTopic\":\"\","
      "\"clientId\":\"\",\"discoveryPrefix\":\"homeassistant\"},\"valves\":[";
  for (int i = 0; i < 12; ++i) {
    expected += std::string(i ? "," : "") +
                "{\"name\":\"\",\"active\":false,\"failsafePct\":50,\"topic\":\"\"}";
  }
  expected += "],\"temps\":[";
  for (int i = 0; i < 34; ++i) {
    expected += std::string(i ? "," : "") +
                "{\"name\":\"\",\"active\":false,\"offset\":0.0,\"id\":\"\",\"topic\":\"\"}";
  }
  expected += "],\"volts\":[";
  for (int i = 0; i < 8; ++i) {
    expected += std::string(i ? "," : "") +
                "{\"name\":\"\",\"active\":false,\"offset\":0,\"factor\":1,\"unit\":\"\","
                "\"id\":\"\",\"topic\":\"\"}";
  }
  expected +=
      "],\"calib\":{\"dayMask\":9,\"hour\":0,\"minute\":0},\"failsafe\":{\"timeoutMin\":60},"
      "\"persistLog\":true}";
  CHECK(exportJson(Config{}) == expected);
}

TEST_CASE("config: JSON export of set values") {
  const Config c = fullConfig();
  const std::string j = exportJson(c);
  CHECK(j.find("\"station\":\"Heizung OG\"") != std::string::npos);
  CHECK(j.find("\"iface\":2,\"dhcp\":false,\"ip\":\"192.168.1.50\",\"mask\":\"255.255.255.0\","
               "\"gateway\":\"192.168.1.1\",\"dns\":\"8.8.8.8\",\"ssid\":\"My Wifi\","
               "\"wifiPasswordSet\":true,\"reconnectTimeoutMin\":17") != std::string::npos);
  CHECK(j.find("\"web\":{\"user\":\"admin\",\"passwordSet\":true,\"protectRead\":true,"
               "\"allowedHosts\":\"\"}") != std::string::npos);
  CHECK(j.find("\"mode\":1,\"host\":\"broker.lan\",\"port\":8883,\"user\":\"mq\","
               "\"passwordSet\":true,\"keepAliveS\":30,\"publishIntervalS\":120,\"minDelayS\":7,"
               "\"separate\":false,\"allTemps\":false,\"pathAsRoot\":true,\"upTime\":false,"
               "\"onChange\":false,\"retained\":false,\"plainText\":false,\"diag\":false,"
               "\"germanDecimal\":true,\"newDiag\":false,\"events\":false,"
               "\"haDiscoveryOnConnect\":false,") != std::string::npos);
  CHECK(j.find("{\"name\":\"t1\",\"active\":true,\"offset\":-1.5,"
               "\"id\":\"28-84-37-94-97-ff-03-23\",\"topic\":\"\"}") != std::string::npos);
  CHECK(j.find("{\"name\":\"\",\"active\":false,\"offset\":10.0,"
               "\"id\":\"28-aa-bb-cc-dd-ee-01-67\",\"topic\":\"\"}]") != std::string::npos);
  CHECK(j.find("{\"name\":\"bat\",\"active\":true,\"offset\":-0.25,\"factor\":0.01,\"unit\":\"V\","
               "\"id\":\"26-11-22-33-44-55-66-29\",\"topic\":\"\"}]") != std::string::npos);
  CHECK(j.find("\"calib\":{\"dayMask\":127,\"hour\":23,\"minute\":59},"
               "\"failsafe\":{\"timeoutMin\":60},\"persistLog\":false}") != std::string::npos);
  // The keys of the cfgx blob.
  const std::string x = exportJson(fullConfigExt());
  CHECK(x.find("\"protectRead\":true,\"allowedHosts\":\"vdmot.lan, 192.168.1.9\"}") !=
        std::string::npos);
  CHECK(x.find("\"haDiscoveryOnConnect\":false,\"rootTopic\":\"VdMotFBH\","
               "\"clientId\":\"VdMot-east-6c1e51\",\"discoveryPrefix\":\"ha/discovery\"}") !=
        std::string::npos);
  CHECK(x.find("\"valves\":[{\"name\":\"Bad\",\"active\":true,\"failsafePct\":255,\"topic\":\"\"},"
               "{\"name\":\"\",\"active\":false,\"failsafePct\":50,\"topic\":\"Bad/WC\"},") !=
        std::string::npos);
  CHECK(x.find("{\"name\":\"Kitchen\",\"active\":false,\"failsafePct\":0,\"topic\":\"x\"}]") !=
        std::string::npos);
  CHECK(x.find("\"id\":\"28-84-37-94-97-ff-03-23\",\"topic\":\"t/1\"}") != std::string::npos);
  CHECK(x.find("\"id\":\"26-11-22-33-44-55-66-29\",\"topic\":\"battery\"}]") !=
        std::string::npos);
  CHECK(x.find("\"failsafe\":{\"timeoutMin\":1440}") != std::string::npos);
  // Secrets never appear.
  CHECK(j.find("secret123") == std::string::npos);
  CHECK(j.find("pa ss") == std::string::npos);
  CHECK(j.find("mqpw") == std::string::npos);
}

TEST_CASE("config: JSON float formatting is the shortest exact form") {
  Config c;
  struct Case {
    float v;
    const char* text;
  };
  const Case cases[] = {{1.0f, "1"},        {-0.0f, "0"},       {0.1f, "0.1"},
                        {-2.5f, "-2.5"},    {0.001f, "0.001"},  {1000.0f, "1000"},
                        {123.456f, "123.456"}, {1e-7f, "0.000000"}, {0.333333f, "0.333333"}};
  for (const Case& k : cases) {
    CAPTURE(k.text);
    c.volts[0].offset = k.v;
    const std::string j = exportJson(c);
    CHECK(j.find(std::string("\"offset\":") + k.text + ",\"factor\"") != std::string::npos);
  }
  c.volts[0].offset = NAN;
  CHECK(exportJson(c).find("\"offset\":null,\"factor\"") != std::string::npos);
  c.volts[0].offset = 1e30f;  // invalid, but must still export something bounded
  CHECK(exportJson(c).find("\"offset\":1000000015047466219876688855040,\"factor\"") !=
        std::string::npos);
}

TEST_CASE("config: JSON export of maximum-length and unterminated strings") {
  Config c;
  memset(c.station, 's', 20);
  strcpy(c.valves[0].name, "1234567890");
  strcpy(c.volts[0].unit, "12345678");
  std::string j = exportJson(c);
  CHECK(j.find("\"station\":\"" + std::string(20, 's') + "\"") != std::string::npos);
  CHECK(j.find("\"name\":\"1234567890\"") != std::string::npos);
  CHECK(j.find("\"unit\":\"12345678\"") != std::string::npos);
  memset(c.station, 't', sizeof c.station);  // no NUL: cut at 20
  memset(c.valves[0].name, 'v', sizeof c.valves[0].name);
  j = exportJson(c);
  CHECK(j.find("\"station\":\"" + std::string(20, 't') + "\"") != std::string::npos);
  CHECK(j.find("\"name\":\"" + std::string(10, 'v') + "\"") != std::string::npos);
}

TEST_CASE("config: JSON export fails cleanly on a small buffer") {
  const std::string full = exportJson(fullConfig());
  const Config c = fullConfig();
  std::vector<char> buf(full.size() + 1);
  for (size_t cap : {size_t{0}, size_t{1}, size_t{50}, full.size() / 2, full.size()}) {
    CAPTURE(cap);
    JsonWriter jw(buf.data(), cap);
    CHECK_FALSE(writeConfigJson(jw, c));
  }
  JsonWriter jw(buf.data(), full.size() + 1);
  CHECK(writeConfigJson(jw, c));
  CHECK(std::string(buf.data()) == full);
}

// ---------------------------------------------------------------- JSON patch

TEST_CASE("config: patch round trip of an export") {
  const Config full = fullConfig();
  const std::string j = exportJson(full);
  Config c;
  // Secrets are not exported: without them the import fails validation
  // (web user without password; an ssid without password is an open network).
  std::string path;
  CHECK(patch(c, j, &path) == PatchResult::Invalid);
  CHECK(path == "web.password");

  Config d = full;  // same secrets present -> exact round trip
  CHECK(patch(d, j, &path) == PatchResult::Ok);
  CHECK(path.empty());
  CHECK(sameConfig(d, full));

  Config e;
  std::string withSecrets = j;
  withSecrets.insert(1,
                     "\"net\":{\"wifiPassword\":\"secret123\"},"
                     "\"web\":{\"password\":\"pa ss\\\"w\"},"
                     "\"mqtt\":{\"password\":\"mqpw\"},");
  CHECK(patch(e, withSecrets, &path) == PatchResult::Ok);
  CHECK(sameConfig(e, full));
}

TEST_CASE("config: patch paths, nesting forms and clearSecrets") {
  Config c;
  std::string path;
  CHECK(patch(c, "{}") == PatchResult::Ok);
  CHECK(sameConfig(c, Config{}));
  CHECK(patch(c, " \t\r\n{ \"calib\" : { \"hour\" : 5 } } \n") == PatchResult::Ok);
  CHECK(c.calib.hour == 5);
  CHECK(patch(c, "{\"valves\":{\"3\":{\"name\":\"x\"}}}") == PatchResult::Ok);
  CHECK(std::string(c.valves[2].name) == "x");
  CHECK(patch(c, "{\"valves.4.name\":\"y\"}") == PatchResult::Ok);
  CHECK(std::string(c.valves[3].name) == "y");
  CHECK(patch(c, "{\"valves\":[{},{\"active\":true},{\"name\":\"\"}]}") == PatchResult::Ok);
  CHECK(c.valves[1].active);
  CHECK(std::string(c.valves[2].name).empty());
  CHECK(patch(c, "{\"temps\":[{\"offset\":\"1.25\"}]}") == PatchResult::Ok);
  CHECK(c.temps[0].offset == 13);
  // Array elements beyond the table and scalar arrays.
  std::string tooMany = "{\"valves\":[";
  for (int i = 0; i < 13; ++i) tooMany += std::string(i ? "," : "") + "{\"active\":false}";
  tooMany += "]}";
  CHECK(patch(c, tooMany, &path) == PatchResult::UnknownKey);
  CHECK(path == "valves.13.active");
  CHECK(patch(c, "{\"valves\":[1]}", &path) == PatchResult::UnknownKey);
  CHECK(path == "valves.1");
  CHECK(patch(c, "{\"valves\":[[{\"name\":\"a\"}]]}", &path) == PatchResult::UnknownKey);
  CHECK(path == "valves.1.1.name");
  CHECK(patch(c, "{\"calib\":{\"hour\":\"x\"}}", &path) == PatchResult::WrongType);
  CHECK(path == "calib.hour");
  CHECK(patch(c, "{\"calib\":{\"hour\":24}}", &path) == PatchResult::OutOfRange);
  CHECK(path == "calib.hour");
  CHECK(patch(c, "{\"schema\":3}", &path) == PatchResult::ReadOnly);
  CHECK(path == "schema");
  CHECK(patch(c, "{\"schema\":1}", &path) == PatchResult::Ok);
  CHECK(patch(c, "{\"schema\":2}", &path) == PatchResult::Ok);
  CHECK(patch(c, "{\"calib\":{\"hour\":null}}", &path) == PatchResult::WrongType);
  CHECK(patch(c, "{\"nope\":{}}", &path) == PatchResult::Ok);  // empty container: no leaf
  CHECK(patch(c, "{\"nope\":1}", &path) == PatchResult::UnknownKey);
  CHECK(path == "nope");
  CHECK(patch(c, "{\"\":1}", &path) == PatchResult::UnknownKey);
  CHECK(path.empty());

  // The first failure stops the walk; earlier keys were applied to `c`.
  Config d;
  CHECK(patch(d, "{\"calib\":{\"hour\":3,\"minute\":60,\"dayMask\":1}}", &path) ==
        PatchResult::OutOfRange);
  CHECK(path == "calib.minute");
  CHECK(d.calib.hour == 3);
  CHECK(d.calib.dayMask == 9);

  // Invalid after all keys: validateConfig path.
  Config e;
  CHECK(patch(e, "{\"mqtt\":{\"mode\":1}}", &path) == PatchResult::Invalid);
  CHECK(path == "mqtt.host");
  CHECK(patch(e, "{\"mqtt\":{\"mode\":1,\"host\":\"b\"}}", &path) == PatchResult::Ok);

  // clearSecrets anywhere at the root, applied before the secrets.
  Config s;
  strcpy(s.web.user, "u");
  strcpy(s.web.password, "p");
  CHECK(patch(s, "{\"web\":{\"password\":\"\"}}") == PatchResult::Ok);
  CHECK(std::string(s.web.password) == "p");
  CHECK(patch(s, "{\"web\":{\"password\":\"\",\"user\":\"\"},\"clearSecrets\":true}") ==
        PatchResult::Ok);
  CHECK(std::string(s.web.password).empty());
  strcpy(s.web.user, "u");
  strcpy(s.web.password, "p");
  CHECK(patch(s, "{\"clearSecrets\":false,\"web\":{\"password\":\"\"}}") == PatchResult::Ok);
  CHECK(std::string(s.web.password) == "p");
  CHECK(patch(s, "{\"clearSecrets\":1}", &path) == PatchResult::WrongType);
  CHECK(path == "clearSecrets");
  CHECK(patch(s, "{\"clearSecrets\":\"true\"}", &path) == PatchResult::WrongType);
  // Only the root key is special.
  CHECK(patch(s, "{\"web\":{\"clearSecrets\":true}}", &path) == PatchResult::UnknownKey);
  CHECK(path == "web.clearSecrets");
  // Last root clearSecrets wins.
  CHECK(patch(s, "{\"clearSecrets\":true,\"clearSecrets\":false,\"web\":{\"password\":\"\"}}") ==
        PatchResult::Ok);
  CHECK(std::string(s.web.password) == "p");
}

TEST_CASE("config: patch string decoding") {
  Config c;
  std::string path;
  CHECK(patch(c, "{\"station\":\"A\\u0042\\/\\\\x\"}", &path) == PatchResult::OutOfRange);
  CHECK(patch(c, "{\"station\":\"A\\u0042_x\"}") == PatchResult::Ok);
  CHECK(std::string(c.station) == "AB_x");
  CHECK(patch(c, "{\"time\":{\"tzName\":\"a\\/b\"}}") == PatchResult::Ok);
  CHECK(std::string(c.time.tzName) == "a/b");
  CHECK(patch(c, "{\"web\":{\"user\":\"q\\\"\",\"password\":\"\\\\\"}}") == PatchResult::Ok);
  CHECK(std::string(c.web.user) == "q\"");
  CHECK(std::string(c.web.password) == "\\");
  // Control characters decode fine but fail the field rules.
  const char* rejected[] = {"\\n", "\\r", "\\t", "\\b", "\\f", "\\u0000", "\\u007f",
                            "\\u0085", "\xc2\x85", "\xc3", ":"};
  for (const char* r : rejected) {
    CAPTURE(r);
    CHECK(patch(c, std::string("{\"web\":{\"user\":\"a") + r + "\"}}", &path) ==
          PatchResult::OutOfRange);
    CHECK(path == "web.user");
  }
  // UTF-8, raw or escaped, is text.
  const char* accepted[][2] = {{"\\u00e4", "\xc3\xa4"},
                               {"\\u20ac", "\xe2\x82\xac"},
                               {"\\ud83d\\ude00", "\xf0\x9f\x98\x80"},
                               {"\xc3\xa4", "\xc3\xa4"}};
  for (const auto& a : accepted) {
    CAPTURE(a[0]);
    CHECK(patch(c, std::string("{\"web\":{\"user\":\"a") + a[0] + "\"}}", &path) ==
          PatchResult::Ok);
    CHECK(std::string(c.web.user) == std::string("a") + a[1]);
  }
  // Keys are decoded too.
  CHECK(patch(c, "{\"c\\u0061lib\":{\"\\u0068our\":7}}") == PatchResult::Ok);
  CHECK(c.calib.hour == 7);
  CHECK(patch(c, "{\"cal\\u0000ib\":{\"hour\":7}}", &path) == PatchResult::UnknownKey);
  // A 96+ byte string is cut, and then too long for every field.
  const std::string longValue(200, 'a');
  CHECK(patch(c, "{\"station\":\"" + longValue + "\"}", &path) == PatchResult::OutOfRange);
  CHECK(path == "station");
  CHECK(patch(c, "{\"nope\":\"" + longValue + "\"}", &path) == PatchResult::UnknownKey);
  // A long key or path is an unknown key.
  CHECK(patch(c, "{\"" + longValue + "\":1}", &path) == PatchResult::UnknownKey);
  std::string deepKey = "{\"calib\":{\"" + std::string(60, 'k') + "\":1}}";
  CHECK(patch(c, deepKey, &path) == PatchResult::UnknownKey);
  CHECK(path == "calib");
  // 65 chars of path are an overflow (the path stops at the parent).
  CHECK(patch(c, "{\"calib\":{\"" + std::string(59, 'k') + "\":1}}", &path) ==
        PatchResult::UnknownKey);
  CHECK(path == "calib");
  // Exactly 64 chars of path are still passed on.
  std::string key58(58, 'k');  // "calib." + 58 = 64
  CHECK(patch(c, "{\"calib\":{\"" + key58 + "\":1}}", &path) == PatchResult::UnknownKey);
  CHECK(path == "calib." + key58);
}

TEST_CASE("config: patch hex escapes and invalid strings before a closing brace") {
  Config c;
  std::string path;
  CHECK(patch(c, "{\"time\":{\"tzName\":\"\\u0030\\u0039\\u0041\\u0046\\u0061\\u0066"
                 "\\u004a\\u006B\"}}") == PatchResult::Ok);
  CHECK(std::string(c.time.tzName) == "09AFafJk");
  // Every hex digit class at both ends: 0 9 a f A F.
  CHECK(patch(c, "{\"time\":{\"tzName\":\"\\u004F\\u006f\\u004A\\u006a\\u0030\\u0039\"}}") ==
        PatchResult::Ok);
  CHECK(std::string(c.time.tzName) == "OoJj09");
  // Surrogate ranges: syntax only (the text itself is rejected by the field).
  const char* okSyntax[] = {"\\ud7ff", "\\ue000", "\\udbff\\udfff", "\\ud800\\udc00",
                            "\\uD800\\uDC00"};
  for (const char* e : okSyntax) {
    CAPTURE(e);
    CHECK(patch(c, std::string("{\"x\":\"") + e + "\"}", &path) == PatchResult::UnknownKey);
  }
  const char* badSyntax[] = {"\\udfff", "\\udc00", "\\udbff\\ue000", "\\udbff\\udbff",
                             "\\ud800\\ud7ff", "\\ud800\\u", "\\ud800\\"};
  for (const char* e : badSyntax) {
    CAPTURE(e);
    CHECK(patch(c, std::string("{\"x\":\"") + e + "\"}", &path) == PatchResult::Malformed);
  }
  // An overlong key below a valid path never falls back to that path.
  CHECK(patch(c, "{\"calib.hour\":{\"" + std::string(60, 'k') + "\":5}}", &path) ==
        PatchResult::UnknownKey);
  CHECK(c.calib.hour == 0);
  const char* badHex[] = {"G", "g", "/", ":", "@", "`"};
  for (const char* h : badHex) {
    CAPTURE(h);
    CHECK(patch(c, std::string("{\"time\":{\"tzName\":\"\\u004") + h + "\"}}", &path) ==
          PatchResult::Malformed);
  }
  // Each of these is malformed; a parser that gave up on the string without
  // failing would see a complete document and report an unknown key.
  const char* bad[] = {"{\"x\":\"\n}",          "{\"x\":\"\\q}",
                       "{\"x\":\"\\ud800}",     "{\"x\":\"\\ud800\\u0041}",
                       "{\"x\":\"\\udc00}",     "{\"x\":\"\\u12}",
                       "{\"x\":\"\x01}"};
  for (const char* b : bad) {
    CAPTURE(b);
    CHECK(patch(c, b, &path) == PatchResult::Malformed);
  }
  // Empty keys never fall back to the parent path.
  CHECK(patch(c, "{\"calib.hour\":{\"\":5}}", &path) == PatchResult::UnknownKey);
  CHECK(c.calib.hour == 0);
  CHECK(patch(c, "{\"calib.hour\":{\"a\\u0000\":5}}", &path) == PatchResult::UnknownKey);
  CHECK(c.calib.hour == 0);
  // clearSecrets first, secrets after it.
  strcpy(c.web.user, "u");
  strcpy(c.web.password, "p");
  CHECK(patch(c, "{\"clearSecrets\":true,\"web\":{\"user\":\"\",\"password\":\"\"}}") ==
        PatchResult::Ok);
  CHECK(std::string(c.web.password).empty());
}

TEST_CASE("config: patch number forms") {
  Config c;
  std::string path;
  CHECK(patch(c, "{\"calib\":{\"hour\":-0}}") == PatchResult::Ok);
  CHECK(c.calib.hour == 0);
  CHECK(patch(c, "{\"calib\":{\"hour\":1e1}}") == PatchResult::Ok);
  CHECK(c.calib.hour == 10);
  CHECK(patch(c, "{\"calib\":{\"hour\":2.0E+0}}") == PatchResult::Ok);
  CHECK(c.calib.hour == 2);
  CHECK(patch(c, "{\"calib\":{\"hour\":2.5}}", &path) == PatchResult::WrongType);
  CHECK(patch(c, "{\"calib\":{\"hour\":-1}}", &path) == PatchResult::OutOfRange);
  CHECK(patch(c, "{\"calib\":{\"hour\":999999999999999999}}", &path) == PatchResult::OutOfRange);
  CHECK(patch(c, "{\"calib\":{\"hour\":9999999999999999999}}", &path) == PatchResult::OutOfRange);
  CHECK(patch(c, "{\"calib\":{\"hour\":-9999999999999999999}}", &path) == PatchResult::OutOfRange);
  CHECK(patch(c, "{\"calib\":{\"hour\":" + std::string(60, '1') + "}}", &path) ==
        PatchResult::OutOfRange);
  CHECK(patch(c, "{\"temps\":[{\"offset\":-0.05}]}") == PatchResult::Ok);
  CHECK(c.temps[0].offset == -1);
  CHECK(patch(c, "{\"volts\":[{\"factor\":1e-3}]}") == PatchResult::Ok);
  CHECK(c.volts[0].factor == 0.001f);
}

TEST_CASE("config: patch syntax errors report the byte offset") {
  struct Case {
    const char* json;
    const char* at;
  };
  const Case cases[] = {
      {"", "@0"},
      {"   ", "@3"},
      {"[]", "@0"},
      {"1", "@0"},
      {"\"x\"", "@0"},
      {"{", "@1"},
      {"{\"a\"", "@4"},
      {"{\"a\":", "@5"},
      {"{\"a\":1", "@6"},
      {"{\"a\":1,}", "@7"},
      {"{\"a\" 1}", "@5"},
      {"{a:1}", "@1"},
      {"{\"a\":1}}", "@7"},
      {"{\"a\":1} x", "@8"},
      {"{\"a\":01}", "@5"},
      {"{\"a\":1.}", "@5"},
      {"{\"a\":.5}", "@5"},
      {"{\"a\":-}", "@5"},
      {"{\"a\":+1}", "@5"},
      {"{\"a\":1e}", "@5"},
      {"{\"a\":0x10}", "@6"},
      {"{\"a\":tru}", "@5"},
      {"{\"a\":nul}", "@5"},
      {"{\"a\":True}", "@5"},
      {"{\"a\":'x'}", "@5"},
      {"{\"a\":\"x}", "@8"},
      {"{\"a\":\"\\x\"}", "@8"},
      {"{\"a\":\"\\u12\"}", "@8"},
      {"{\"a\":\"\\u12g4\"}", "@8"},
      {"{\"a\":\"\\udc00\"}", "@12"},
      {"{\"a\":\"\\ud800\"}", "@12"},
      {"{\"a\":\"\\ud800\\u0041\"}", "@18"},
      {"{\"a\":\"\\ud800x\"}", "@12"},
      {"{\"a\":\"a\nb\"}", "@8"},
      {"{\"a\":[1,]}", "@8"},
      {"{\"a\":[1 2]}", "@8"},
      {"{\"a\":{}", "@7"},
      {"{\"a\":1 \"b\":2}", "@7"},
      {"{,\"a\":1}", "@1"},
      {"{\"a\":1,,\"b\":1}", "@7"},
      {"{\"a\":[}", "@6"},
  };
  for (const Case& k : cases) {
    CAPTURE(k.json);
    Config c;
    std::string path;
    CHECK(patch(c, k.json, &path) == PatchResult::Malformed);
    CHECK(path == k.at);
    CHECK(sameConfig(c, Config{}));  // nothing applied on a syntax error
  }
  // A syntax error after a valid key: nothing is applied either.
  Config c;
  CHECK(patch(c, "{\"calib\":{\"hour\":3},\"x\":}") == PatchResult::Malformed);
  CHECK(c.calib.hour == 0);
  // Embedded NUL.
  const std::string withNul("{\"calib\":{\"hour\":3}}\0", 21);
  std::string path;
  CHECK(patch(c, withNul, &path) == PatchResult::Malformed);
  CHECK(path == "@20");
  // Null pointer and tiny path buffers.
  char p[4];
  CHECK(applyConfigJson(c, nullptr, 5, p, sizeof p) == PatchResult::Malformed);
  CHECK(std::string(p) == "@0");
  CHECK(applyConfigJson(c, "[", 1, nullptr, 0) == PatchResult::Malformed);
  CHECK(applyConfigJson(c, "{\"calib\":{\"hour\":99}}", 21, p, sizeof p) == PatchResult::OutOfRange);
  CHECK(std::string(p) == "cal");
}

TEST_CASE("config: patch nesting depth is bounded") {
  Config c;
  std::string path;
  // 8 levels are parsed (then the path is simply unknown).
  std::string ok8 = "{\"a\":[[[[[[[1]]]]]]]}";
  CHECK(patch(c, ok8, &path) == PatchResult::UnknownKey);
  CHECK(path == "a.1.1.1.1.1.1.1");
  std::string deep9 = "{\"a\":[[[[[[[[1]]]]]]]]}";
  CHECK(patch(c, deep9, &path) == PatchResult::Malformed);
  CHECK(path == "@12");
  std::string veryDeep = "{\"a\":" + std::string(5000, '[') + std::string(5000, ']') + "}";
  CHECK(patch(c, veryDeep, &path) == PatchResult::Malformed);
}

TEST_CASE("config: patch fuzz with random bytes and mutated documents") {
  std::mt19937 rng(20260924);
  const std::string base = exportJson(fullConfig());
  const char alphabet[] = "{}[]\":,\\/ -0123456789.eEtrufalsn\"abcxyz\x01\xff";
  for (int iter = 0; iter < 3000; ++iter) {
    std::string doc;
    if (iter % 2 == 0) {
      const size_t n = rng() % 64;
      for (size_t i = 0; i < n; ++i) doc += alphabet[rng() % (sizeof alphabet - 1)];
      if (iter % 4 == 0) doc = "{" + doc;
    } else {
      doc = base;
      const int edits = 1 + static_cast<int>(rng() % 4);
      for (int e = 0; e < edits; ++e) {
        const size_t at = rng() % doc.size();
        switch (rng() % 3) {
          case 0: doc[at] = alphabet[rng() % (sizeof alphabet - 1)]; break;
          case 1: doc.erase(at, 1 + rng() % 8); break;
          default: doc.insert(at, 1, alphabet[rng() % (sizeof alphabet - 1)]); break;
        }
        if (doc.empty()) doc = "{";
      }
    }
    Config c = fullConfig();
    char path[80];
    const PatchResult r = applyConfigJson(c, doc.data(), doc.size(), path, sizeof path);
    CHECK(static_cast<int>(r) <= static_cast<int>(PatchResult::Invalid));
    CHECK(strlen(path) < sizeof path);
    if (r == PatchResult::Ok) CHECK(validatePath(c) == "OK");
    if (r == PatchResult::Malformed) CHECK(path[0] == '@');
  }
}

// ---------------------------------------------------------------- binary

TEST_CASE("config: CRC-32 matches zlib") {
  const uint8_t check[] = {'1', '2', '3', '4', '5', '6', '7', '8', '9'};
  CHECK(crc32(check, sizeof check) == 0xCBF43926u);
  CHECK(crc32(check, 0) == 0u);
  CHECK(crc32(nullptr, 5) == 0u);
  CHECK(crc32(nullptr, 5, 0x1234u) == 0x1234u);
  CHECK(crc32(check + 4, 5, crc32(check, 4)) == 0xCBF43926u);
  const uint8_t zero = 0;
  CHECK(crc32(&zero, 1) == 0xD202EF8Du);
  const uint8_t ff[4] = {0xFF, 0xFF, 0xFF, 0xFF};
  CHECK(crc32(ff, 4) == 0xFFFFFFFFu);
  const char* fox = "The quick brown fox jumps over the lazy dog";
  CHECK(crc32(reinterpret_cast<const uint8_t*>(fox), strlen(fox)) == 0x414FA339u);
}

TEST_CASE("config: binary encoding layout and round trip") {
  const std::vector<uint8_t> d = encode(Config{});
  REQUIRE(d.size() > 12);
  CHECK(d[0] == 'V');
  CHECK(d[1] == 'D');
  CHECK(d[2] == 'M');
  CHECK(d[3] == 'C');
  CHECK(d[4] == 1);
  CHECK(d[5] == 0);
  CHECK(static_cast<size_t>(d[6] | (d[7] << 8)) == d.size() - 12);
  const uint32_t crc = crc32(d.data(), d.size() - 4);
  CHECK(d[d.size() - 4] == static_cast<uint8_t>(crc));
  CHECK(d[d.size() - 1] == static_cast<uint8_t>(crc >> 24));
  // The payload starts with the station as u8 length + bytes.
  CHECK(d[8] == 5);
  CHECK(memcmp(&d[9], "VdMot", 5) == 0);
  // net.iface u8, dhcp u8, ip u32 LE.
  CHECK(d[14] == 0);
  CHECK(d[15] == 1);

  Config back;
  back.calib.hour = 7;
  CHECK(decodeConfig(d.data(), d.size(), back) == DecodeResult::Ok);
  CHECK(sameConfig(back, Config{}));

  const Config full = fullConfig();
  const std::vector<uint8_t> f = encode(full);
  CHECK(f.size() <= kConfigBlobMax);
  Config fb;
  CHECK(decodeConfig(f.data(), f.size(), fb) == DecodeResult::Ok);
  CHECK(sameConfig(fb, full));
  CHECK(std::string(fb.web.password) == "pa ss\"w");  // secrets are persisted
  CHECK(std::string(fb.net.wifiPassword) == "secret123");
  CHECK(fb.volts[7].factor == 0.01f);
  CHECK(fb.temps[0].offset == -15);
  CHECK(fb.net.ip == full.net.ip);

  // Little-endian u16: mqtt.port 8883 = 0x22B3 somewhere in the payload.
  Config p1, p2;
  p2.mqtt.port = 0x1234;
  const size_t at = diffOffset(p1, p2);
  CHECK(encode(p2)[at] == 0x34);
  CHECK(encode(p2)[at + 1] == 0x12);
}

TEST_CASE("config: encode needs the full capacity") {
  const Config full = fullConfig();
  const std::vector<uint8_t> f = encode(full);
  std::vector<uint8_t> buf(f.size());
  for (size_t cap = 0; cap < f.size(); ++cap) {
    CAPTURE(cap);
    CHECK(encodeConfig(full, buf.data(), cap) == 0);
  }
  CHECK(encodeConfig(full, buf.data(), f.size()) == f.size());
  CHECK(buf == f);
  CHECK(encodeConfig(full, nullptr, 4096) == 0);
}

TEST_CASE("config: decode rejects every kind of damage and keeps defaults") {
  const Config full = fullConfig();
  const std::vector<uint8_t> good = encode(full);
  Config out;

  auto expect = [&](const std::vector<uint8_t>& b, DecodeResult r) {
    Config o = full;
    CHECK(decodeConfig(b.data(), b.size(), o) == r);
    if (r != DecodeResult::Ok) CHECK(sameConfig(o, Config{}));
  };

  CHECK(decodeConfig(nullptr, 100, out) == DecodeResult::TooShort);
  for (size_t n = 0; n < 12; ++n) {
    std::vector<uint8_t> b(good.begin(), good.begin() + static_cast<long>(n));
    expect(b, DecodeResult::TooShort);
  }
  {
    std::vector<uint8_t> b(good.begin(), good.end() - 1);
    expect(b, DecodeResult::TooShort);
  }
  {
    // Header + CRC only (12 bytes, empty payload): structurally complete,
    // but the fields are missing.
    std::vector<uint8_t> b = {'V', 'D', 'M', 'C', 1, 0, 0, 0, 0, 0, 0, 0};
    fixCrc(b);
    expect(b, DecodeResult::Invalid);
  }
  {
    std::vector<uint8_t> b = good;
    b.push_back(0);
    expect(b, DecodeResult::Invalid);
  }
  for (int i = 0; i < 4; ++i) {
    std::vector<uint8_t> b = good;
    b[static_cast<size_t>(i)] ^= 0x20;
    expect(b, DecodeResult::BadMagic);
  }
  {
    std::vector<uint8_t> b = good;
    b[20] ^= 1;
    expect(b, DecodeResult::BadCrc);
    b = good;
    b.back() ^= 0x80;
    expect(b, DecodeResult::BadCrc);
  }
  {
    std::vector<uint8_t> b = good;
    b[4] = 2;
    fixCrc(b);
    expect(b, DecodeResult::UnsupportedSchema);
    b[4] = 0;
    fixCrc(b);
    expect(b, DecodeResult::UnsupportedSchema);
    b[4] = 1;
    b[5] = 1;  // 257
    fixCrc(b);
    expect(b, DecodeResult::UnsupportedSchema);
  }
  {
    // Payload length one short: last field incomplete -> too short a
    // remaining payload vs. the declared blob length.
    std::vector<uint8_t> b = good;
    const size_t payload = good.size() - 12;
    b[6] = static_cast<uint8_t>(payload - 1);
    b[7] = static_cast<uint8_t>((payload - 1) >> 8);
    b.erase(b.end() - 5);
    fixCrc(b);
    expect(b, DecodeResult::Invalid);
    // Payload with one extra byte.
    b = good;
    b[6] = static_cast<uint8_t>(payload + 1);
    b[7] = static_cast<uint8_t>((payload + 1) >> 8);
    b.insert(b.end() - 4, 0);
    fixCrc(b);
    expect(b, DecodeResult::Invalid);
  }
  {
    // Station length byte larger than its field.
    std::vector<uint8_t> b = good;
    b[8] = 21;
    fixCrc(b);
    expect(b, DecodeResult::Invalid);
    // Station with an embedded NUL ("H\0izung OG" would validate as "H").
    b = good;
    b[10] = 0;
    fixCrc(b);
    expect(b, DecodeResult::Invalid);
    // Station of length 0: structurally fine, fails validation.
    Config e;
    Config z;
    const std::vector<uint8_t> zeroLen = [&] {
      std::vector<uint8_t> v = encode(e);
      v.erase(v.begin() + 9, v.begin() + 14);
      v[8] = 0;
      const size_t pl = v.size() - 12;
      v[6] = static_cast<uint8_t>(pl);
      v[7] = static_cast<uint8_t>(pl >> 8);
      fixCrc(v);
      return v;
    }();
    CHECK(decodeConfig(zeroLen.data(), zeroLen.size(), z) == DecodeResult::Invalid);
  }
  {
    // Bool byte 2 and out-of-range values.
    Config a, b2;
    b2.persistLog = false;
    const size_t at = diffOffset(a, b2);
    std::vector<uint8_t> b = encode(a);
    b[at] = 2;
    fixCrc(b);
    expect(b, DecodeResult::Invalid);
    Config h;
    h.calib.hour = 5;
    const size_t hourAt = diffOffset(a, h);
    b = encode(a);
    b[hourAt] = 24;
    fixCrc(b);
    expect(b, DecodeResult::Invalid);
    b[hourAt] = 23;
    fixCrc(b);
    expect(b, DecodeResult::Ok);
  }
}

TEST_CASE("config: decode fuzz") {
  std::mt19937 rng(424242);
  const std::vector<uint8_t> good = encode(fullConfig());
  for (int iter = 0; iter < 4000; ++iter) {
    std::vector<uint8_t> b;
    if (iter % 3 == 0) {
      b.resize(rng() % 64);
      for (auto& x : b) x = static_cast<uint8_t>(rng());
    } else {
      b = good;
      const int flips = 1 + static_cast<int>(rng() % 3);
      for (int k = 0; k < flips; ++k) b[8 + rng() % (b.size() - 12)] = static_cast<uint8_t>(rng());
      if (iter % 3 == 1) fixCrc(b);
    }
    Config c = fullConfig();
    const DecodeResult r = decodeConfig(b.data(), b.size(), c);
    if (r == DecodeResult::Ok) {
      CHECK(validatePath(c) == "OK");
    } else {
      CHECK(sameConfig(c, Config{}));
    }
  }
}

// ---- mutation-driven cases -------------------------------------------------

TEST_CASE("config: tzPosix NoSpace rule checks every byte, '~' allowed") {
  Config c;
  setDefaults(c);
  CHECK(set(c, "time.tzPosix", S(" CET-1")) == SetResult::OutOfRange);
  CHECK(set(c, "time.tzPosix", S("\x7f" "CET")) == SetResult::OutOfRange);
  CHECK(set(c, "time.tzPosix", S("CET-1 ")) == SetResult::OutOfRange);
  CHECK(set(c, "time.tzPosix", S("~")) == SetResult::Ok);
  CHECK(std::string(c.time.tzPosix) == "~");
  CHECK(set(c, "time.tzPosix", S("!A~")) == SetResult::Ok);
  CHECK(validateConfig(c, nullptr, 0));
  c.time.tzPosix[0] = ' ';
  char path[40];
  CHECK_FALSE(validateConfig(c, path, sizeof path));
  CHECK(std::string(path) == "time.tzPosix");
}

TEST_CASE("config: numeric strings are parsed within their length only") {
  Config c;
  setDefaults(c);
  // "-5" cut to "-": not a number, even though a digit follows in memory.
  CHECK(set(c, "mqtt.minDelayS", SL("-5", 1)) == SetResult::WrongType);
  // "1." cut to "1": a number, the '.' outside the length does not count.
  CHECK(set(c, "mqtt.minDelayS", SL("1.", 1)) == SetResult::Ok);
  CHECK(c.mqtt.minDelayS == 1);
  CHECK(set(c, "mqtt.minDelayS", SL("2e", 1)) == SetResult::Ok);
  CHECK(c.mqtt.minDelayS == 2);
  CHECK(set(c, "mqtt.minDelayS", SL("30", 1)) == SetResult::Ok);
  CHECK(c.mqtt.minDelayS == 3);
  CHECK(set(c, "mqtt.minDelayS", SL("4.5", 3)) == SetResult::WrongType);
  CHECK(set(c, "mqtt.minDelayS", SL("4.0", 3)) == SetResult::Ok);
  CHECK(c.mqtt.minDelayS == 4);
  CHECK(set(c, "mqtt.minDelayS", SL("5e0", 2)) == SetResult::WrongType);
  CHECK(set(c, "mqtt.minDelayS", SL("6e+", 3)) == SetResult::WrongType);
  CHECK(set(c, "mqtt.minDelayS", SL("7e+0", 4)) == SetResult::Ok);
  CHECK(c.mqtt.minDelayS == 7);
}

namespace {
// Applies only the first `len` bytes of `full`; the rest stays readable in
// memory, so reading past `len` would change the result.
PatchResult patchCut(Config& c, const std::string& full, size_t len, std::string* pathOut) {
  char path[80];
  memset(path, 'x', sizeof path);
  const PatchResult r = applyConfigJson(c, full.data(), len, path, sizeof path);
  *pathOut = path;
  return r;
}
std::string at(size_t n) { return "@" + std::to_string(n); }
}  // namespace

TEST_CASE("config patch: nothing after `len` is read") {
  Config c;
  setDefaults(c);
  std::string p;
  // Trailing whitespace beyond len.
  CHECK(patchCut(c, "{} ", 2, &p) == PatchResult::Ok);
  CHECK(patchCut(c, "{}\t\n", 3, &p) == PatchResult::Ok);
  // A literal cut by len.
  const std::string lit = "{\"persistLog\":true}";
  const size_t t = lit.find("true");
  CHECK(patchCut(c, lit, t + 3, &p) == PatchResult::Malformed);
  CHECK(p == at(t));
  // The literal ends exactly at len: accepted, the object is unterminated.
  CHECK(patchCut(c, lit, t + 4, &p) == PatchResult::Malformed);
  CHECK(p == at(t + 4));
  const std::string lf = "{\"persistLog\":false}";
  const size_t f = lf.find("false");
  CHECK(patchCut(c, lf, f + 4, &p) == PatchResult::Malformed);
  CHECK(p == at(f));
  CHECK(patchCut(c, lf, f + 5, &p) == PatchResult::Malformed);
  CHECK(p == at(f + 5));
  const std::string ln = "{\"persistLog\":null}";
  const size_t n = ln.find("null");
  CHECK(patchCut(c, ln, n + 3, &p) == PatchResult::Malformed);
  CHECK(p == at(n));
}

TEST_CASE("config patch: \\u escapes cut by len") {
  Config c;
  setDefaults(c);
  std::string p;
  const std::string s = "{\"station\":\"\\u0041\"}";
  const size_t h = s.find("0041");
  for (size_t k = 0; k < 4; ++k) {
    CAPTURE(k);
    CHECK(patchCut(c, s, h + k, &p) == PatchResult::Malformed);
    CHECK(p == at(h));
  }
  // All four digits inside len: the string is unterminated at len.
  CHECK(patchCut(c, s, h + 4, &p) == PatchResult::Malformed);
  CHECK(p == at(h + 4));
  CHECK(patch(c, s, &p) == PatchResult::Ok);
  CHECK(std::string(c.station) == "A");

  // Surrogate pair: the low half must be inside len.
  const std::string sp = "{\"station\":\"\\uD83D\\uDE00\"}";
  const size_t lo = sp.find("\\uDE00");
  CHECK(patchCut(c, sp, lo, &p) == PatchResult::Malformed);
  CHECK(p == at(lo));
  CHECK(patchCut(c, sp, lo + 1, &p) == PatchResult::Malformed);
  CHECK(p == at(lo));
  CHECK(patchCut(c, sp, lo + 2, &p) == PatchResult::Malformed);
  CHECK(p == at(lo + 2));
  CHECK(patchCut(c, sp, lo + 6, &p) == PatchResult::Malformed);
  CHECK(p == at(lo + 6));
}

TEST_CASE("config patch: \\u escapes encode exact UTF-8") {
  struct Case {
    const char* esc;
    const char* utf8;
  };
  const Case cases[] = {
      {"\\u00a0", "\xc2\xa0"},
      {"\\u07ff", "\xdf\xbf"},
      {"\\u0800", "\xe0\xa0\x80"},
      {"\\u20ac", "\xe2\x82\xac"},
      {"\\uffff", "\xef\xbf\xbf"},
      {"\\uD800\\uDC00", "\xf0\x90\x80\x80"},
      {"\\uD83D\\uDE00", "\xf0\x9f\x98\x80"},
      {"\\uDBFF\\uDFFF", "\xf4\x8f\xbf\xbf"},
      {"\\uD8C0\\uDC01", "\xf1\x80\x80\x81"},
      {"\\u0041\\u007e", "A~"},
  };
  for (const Case& k : cases) {
    CAPTURE(k.esc);
    Config c;
    setDefaults(c);
    std::string p;
    CHECK(patch(c, std::string("{\"station\":\"") + k.esc + "\"}", &p) == PatchResult::Ok);
    CHECK(std::string(c.station) == k.utf8);
  }
}

TEST_CASE("config export: all-ones addresses are written in full") {
  Config c;
  setDefaults(c);
  REQUIRE(set(c, "net.mask", S("255.255.255.255")) == SetResult::Ok);
  REQUIRE(set(c, "syslog.server", S("255.255.255.254")) == SetResult::Ok);
  const std::string j = exportJson(c);
  CHECK(j.find("\"mask\":\"255.255.255.255\"") != std::string::npos);
  CHECK(j.find("\"server\":\"255.255.255.254\"") != std::string::npos);
}

TEST_CASE("config patch: syntax errors at len report the exact offset") {
  Config c;
  setDefaults(c);
  std::string p;
  // Escape backslash is the last byte inside len.
  const std::string e = "{\"station\":\"a\\n\"}";
  const size_t bs = e.find('\\');
  CHECK(patchCut(c, e, bs + 1, &p) == PatchResult::Malformed);
  CHECK(p == at(bs + 1));
  // A number cut by len: digits after len are not part of it.
  const std::string num = "{\"persistLog\":12}";
  const size_t d = num.find('1');
  CHECK(patchCut(c, num, d + 1, &p) == PatchResult::Malformed);
  CHECK(p == at(d + 1));
  // Value missing: the string after len is not read.
  const std::string st = "{\"station\":\"x\"}";
  const size_t colon = st.find(':');
  CHECK(patchCut(c, st, colon + 1, &p) == PatchResult::Malformed);
  CHECK(p == at(colon + 1));
  // Object cut after '{', after a key, before ':'.
  CHECK(patchCut(c, "{}", 1, &p) == PatchResult::Malformed);
  CHECK(p == at(1));
  CHECK(patchCut(c, "{\"a\":1}", 1, &p) == PatchResult::Malformed);
  CHECK(p == at(1));
  CHECK(patchCut(c, "{\"a\":1}", 4, &p) == PatchResult::Malformed);
  CHECK(p == at(4));
  CHECK(patchCut(c, "{\"a\" :1}", 5, &p) == PatchResult::Malformed);
  CHECK(p == at(5));
  CHECK(patchCut(c, "[]", 1, &p) == PatchResult::Malformed);
  CHECK(p == at(0));
  CHECK(patchCut(c, "{\"valves\":[]}", 11, &p) == PatchResult::Malformed);
  CHECK(p == at(11));
}

TEST_CASE("config patch: empty keys and keys with NUL name nothing") {
  Config c;
  setDefaults(c);
  std::string p;
  CHECK(patch(c, "{\"net\":{\"\":1}}", &p) == PatchResult::UnknownKey);
  CHECK(p == "net");
  CHECK(patch(c, "{\"net\":{\"dhcp\\u0000\":true}}", &p) == PatchResult::UnknownKey);
  CHECK(p == "net");
  CHECK(patch(c, "{\"\":{\"station\":\"x\"}}", &p) == PatchResult::UnknownKey);
  CHECK(p == "");
  CHECK(patch(c, "{\"\":1}", &p) == PatchResult::UnknownKey);
  CHECK(p == "");
}

TEST_CASE("config binary: a string length past the payload is not read") {
  // Header + a 1-byte payload that announces a 20-char station name. The
  // buffer is exactly 13 bytes (ASan catches any read past it).
  std::vector<uint8_t> b = {'V', 'D', 'M', 'C', 1, 0, 1, 0, 20, 0, 0, 0, 0};
  fixCrc(b);
  const std::vector<uint8_t> exact(b.begin(), b.end());
  Config out;
  CHECK(decodeConfig(exact.data(), exact.size(), out) == DecodeResult::Invalid);
  // Same with a 4-byte field read past a 2-byte payload (station "" + iface).
  Config c;
  setDefaults(c);
  const std::vector<uint8_t> full = encode(c);
  std::vector<uint8_t> cut(full.begin(), full.begin() + 8 + 2);
  cut[6] = 2;
  cut[7] = 0;
  cut.resize(cut.size() + 4);
  fixCrc(cut);
  const std::vector<uint8_t> exact2(cut.begin(), cut.end());
  CHECK(decodeConfig(exact2.data(), exact2.size(), out) == DecodeResult::Invalid);
  // Payload cut right after the length byte of a 12-char string, deep in the
  // blob: the 12 bytes must not be read from beyond the payload.
  Config other = c;
  REQUIRE(std::string(c.time.ntpServer) == "pool.ntp.org");
  other.time.ntpServer[0] = 'x';
  const size_t off = diffOffset(c, other) - 1;  // ntpServer length byte
  REQUIRE(full[off] == strlen(c.time.ntpServer));
  std::vector<uint8_t> deep(full.begin(), full.begin() + static_cast<long>(off) + 1);
  const size_t payload = deep.size() - 8;
  deep[6] = static_cast<uint8_t>(payload);
  deep[7] = static_cast<uint8_t>(payload >> 8);
  deep.resize(deep.size() + 4);
  fixCrc(deep);
  const std::vector<uint8_t> exact3(deep.begin(), deep.end());
  CHECK(decodeConfig(exact3.data(), exact3.size(), out) == DecodeResult::Invalid);
}

TEST_CASE("config binary: an unterminated string is encoded at most cap-1 bytes") {
  Config c;
  setDefaults(c);
  memset(c.station, 'x', sizeof c.station);  // no NUL inside the array
  std::vector<uint8_t> b(kConfigBlobMax);
  const size_t n = encodeConfig(c, b.data(), b.size());
  REQUIRE(n > 0);
  CHECK(b[8] == sizeof c.station - 1);
  Config out;
  CHECK(decodeConfig(b.data(), n, out) == DecodeResult::Ok);
  CHECK(std::string(out.station) == std::string(sizeof c.station - 1, 'x'));
}

// ---------------------------------------------------------------- keys after 2.0.0

TEST_CASE("config: failsafe keys accept exactly their range (C-1)") {
  Config c;
  for (int64_t ok : {0, 1, 99, 100, 255}) {
    CAPTURE(ok);
    CHECK(set(c, "valves.3.failsafePct", I(ok)) == SetResult::Ok);
    CHECK(c.valves[2].failsafePct == ok);
  }
  for (int64_t bad : {-1, 101, 102, 200, 254, 256, 355, 65535, 65536}) {
    CAPTURE(bad);
    CHECK(set(c, "valves.3.failsafePct", I(bad)) == SetResult::OutOfRange);
    CHECK(c.valves[2].failsafePct == 255);
  }
  CHECK(set(c, "valves.12.failsafePct", S("42")) == SetResult::Ok);
  CHECK(c.valves[11].failsafePct == 42);
  CHECK(set(c, "valves.12.failsafePct", F(43.0)) == SetResult::Ok);
  CHECK(c.valves[11].failsafePct == 43);
  CHECK(set(c, "valves.12.failsafePct", F(43.5)) == SetResult::WrongType);
  CHECK(set(c, "valves.12.failsafePct", B(true)) == SetResult::WrongType);
  CHECK(set(c, "valves.12.failsafePct", S("x")) == SetResult::WrongType);
  CHECK(c.valves[11].failsafePct == 43);
  // The other valves keep their default.
  CHECK(c.valves[0].failsafePct == 50);

  for (int64_t ok : {0, 5, 6, 60, 1439, 1440}) {
    CAPTURE(ok);
    CHECK(set(c, "failsafe.timeoutMin", I(ok)) == SetResult::Ok);
    CHECK(c.failsafe.timeoutMin == ok);
  }
  for (int64_t bad : {-1, 1, 4, 1441, 65535, 65536, 65541}) {
    CAPTURE(bad);
    CHECK(set(c, "failsafe.timeoutMin", I(bad)) == SetResult::OutOfRange);
    CHECK(c.failsafe.timeoutMin == 1440);
  }
  CHECK(set(c, "failsafe.timeoutMin", S("300")) == SetResult::Ok);
  CHECK(c.failsafe.timeoutMin == 300);
  CHECK(set(c, "failsafe.timeoutMin", S("")) == SetResult::WrongType);
  CHECK(set(c, "failsafe.timeoutMin", B(false)) == SetResult::WrongType);
  CHECK(set(c, "failsafe.x", I(5)) == SetResult::UnknownKey);
  CHECK(set(c, "failsafe.1.timeoutMin", I(5)) == SetResult::UnknownKey);
  CHECK(c.failsafe.timeoutMin == 300);
  CHECK(validatePath(c) == "OK");
}

TEST_CASE("config: string keys after 2.0.0 and their rules (C-1)") {
  Config c;
  struct Row {
    const char* path;
    const char* value;
    SetResult r;
  };
  const std::string h79 = std::string(19, 'a') + "," + std::string(19, 'b') + "," +
                          std::string(19, 'c') + "," + std::string(19, 'd');
  const std::string h80 = h79 + "d";
  const std::string h81 = h80 + "d";
  const std::string label65 = std::string(65, 'a');
  const std::string c64(64, 'c'), c65(65, 'c');
  const std::string p32 = "ha/" + std::string(29, 'p'), p33 = p32 + "p";
  const Row rows[] = {
      {"web.allowedHosts", "", SetResult::Ok},
      {"web.allowedHosts", "vdmot.lan", SetResult::Ok},
      {"web.allowedHosts", "vdmot.lan, 192.168.1.9", SetResult::Ok},
      {"web.allowedHosts", "  a  ,  b  ", SetResult::Ok},
      {"web.allowedHosts", "a,b,c,d", SetResult::Ok},
      {"web.allowedHosts", h79.c_str(), SetResult::Ok},
      {"web.allowedHosts", h80.c_str(), SetResult::Ok},
      {"web.allowedHosts", h81.c_str(), SetResult::OutOfRange},
      {"web.allowedHosts", "a,b,c,d,e", SetResult::OutOfRange},
      {"web.allowedHosts", "a,,b", SetResult::OutOfRange},
      {"web.allowedHosts", "a,", SetResult::OutOfRange},
      {"web.allowedHosts", ",a", SetResult::OutOfRange},
      {"web.allowedHosts", " , ", SetResult::OutOfRange},
      {"web.allowedHosts", " ", SetResult::OutOfRange},
      {"web.allowedHosts", "bad_host!", SetResult::OutOfRange},
      {"web.allowedHosts", "a b", SetResult::OutOfRange},
      {"web.allowedHosts", "a,-b", SetResult::OutOfRange},
      {"web.allowedHosts", label65.c_str(), SetResult::OutOfRange},
      {"mqtt.rootTopic", "", SetResult::Ok},
      {"mqtt.rootTopic", "VdMotFBH", SetResult::Ok},
      {"mqtt.rootTopic", "Dom 1", SetResult::Ok},
      {"mqtt.rootTopic", "12345678901234567890", SetResult::Ok},
      {"mqtt.rootTopic", "123456789012345678901", SetResult::OutOfRange},
      {"mqtt.rootTopic", "a/b", SetResult::OutOfRange},
      {"mqtt.rootTopic", "a+b", SetResult::OutOfRange},
      {"mqtt.clientId", "", SetResult::Ok},
      {"mqtt.clientId", "VdMot-east-6c1e51", SetResult::Ok},
      {"mqtt.clientId", "a.b_c-D9", SetResult::Ok},
      {"mqtt.clientId", c64.c_str(), SetResult::Ok},
      {"mqtt.clientId", c65.c_str(), SetResult::OutOfRange},
      {"mqtt.clientId", "a b", SetResult::OutOfRange},
      {"mqtt.clientId", "a/b", SetResult::OutOfRange},
      {"mqtt.clientId", "a:b", SetResult::OutOfRange},
      {"mqtt.clientId", "a@b", SetResult::OutOfRange},
      {"mqtt.clientId", "K\xc3\xbc" "che", SetResult::OutOfRange},
      {"mqtt.discoveryPrefix", "ha/discovery", SetResult::Ok},
      {"mqtt.discoveryPrefix", "a", SetResult::Ok},
      {"mqtt.discoveryPrefix", "a-b_c/D9/x", SetResult::Ok},
      {"mqtt.discoveryPrefix", p32.c_str(), SetResult::Ok},
      {"mqtt.discoveryPrefix", p33.c_str(), SetResult::OutOfRange},
      {"mqtt.discoveryPrefix", "", SetResult::OutOfRange},
      {"mqtt.discoveryPrefix", "/ha", SetResult::OutOfRange},
      {"mqtt.discoveryPrefix", "ha/", SetResult::OutOfRange},
      {"mqtt.discoveryPrefix", "/", SetResult::OutOfRange},
      {"mqtt.discoveryPrefix", "a//b", SetResult::OutOfRange},
      {"mqtt.discoveryPrefix", "a b", SetResult::OutOfRange},
      {"mqtt.discoveryPrefix", "a.b", SetResult::OutOfRange},
      {"mqtt.discoveryPrefix", "a+", SetResult::OutOfRange},
      {"valves.1.topic", "", SetResult::Ok},
      {"valves.1.topic", "Bad/WC", SetResult::Ok},
      {"valves.1.topic", "a\"b", SetResult::Ok},
      {"valves.1.topic", "a\\b", SetResult::Ok},
      {"valves.1.topic", "\xc3\xa4/\xc3\xb6", SetResult::Ok},
      {"valves.1.topic", "1234567890", SetResult::Ok},
      {"valves.1.topic", "12345678901", SetResult::OutOfRange},
      {"valves.1.topic", "/a", SetResult::OutOfRange},
      {"valves.1.topic", "a/", SetResult::OutOfRange},
      {"valves.1.topic", "/", SetResult::OutOfRange},
      {"valves.1.topic", "a//b", SetResult::OutOfRange},
      {"valves.1.topic", "a b", SetResult::OutOfRange},
      {"valves.1.topic", "a+b", SetResult::OutOfRange},
      {"valves.1.topic", "a#b", SetResult::OutOfRange},
      {"valves.1.topic", "a\x01", SetResult::OutOfRange},
      {"valves.1.topic", "a\xc3", SetResult::OutOfRange},
      {"temps.34.topic", "t/1", SetResult::Ok},
      {"temps.34.topic", "t 1", SetResult::OutOfRange},
      {"volts.8.topic", "v/1", SetResult::Ok},
      {"volts.8.topic", "v/", SetResult::OutOfRange},
  };
  for (const Row& r : rows) {
    CAPTURE(r.path);
    CAPTURE(r.value);
    Config k;
    CHECK(set(k, r.path, S(r.value)) == r.r);
  }
  // Stored exactly; a rejected value leaves the field unchanged.
  CHECK(set(c, "web.allowedHosts", S("  a  ,  b  ")) == SetResult::Ok);
  CHECK(std::string(c.web.allowedHosts) == "  a  ,  b  ");
  CHECK(set(c, "web.allowedHosts", S("a,,b")) == SetResult::OutOfRange);
  CHECK(std::string(c.web.allowedHosts) == "  a  ,  b  ");
  CHECK(set(c, "temps.34.topic", S("t/1")) == SetResult::Ok);
  CHECK(std::string(c.temps[33].topic) == "t/1");
  CHECK(set(c, "volts.8.topic", S("v/1")) == SetResult::Ok);
  CHECK(std::string(c.volts[7].topic) == "v/1");
  CHECK(set(c, "mqtt.discoveryPrefix", S("ha")) == SetResult::Ok);
  CHECK(std::string(c.mqtt.discoveryPrefix) == "ha");
  CHECK(set(c, "mqtt.clientId", I(1)) == SetResult::WrongType);
  CHECK(set(c, "valves.13.topic", S("x")) == SetResult::UnknownKey);
}

TEST_CASE("config: stored values of the new keys are validated") {
  struct Case {
    void (*edit)(Config&);
    const char* path;
  };
  const Case cases[] = {
      {[](Config& c) { c.valves[3].failsafePct = 101; }, "valves.4.failsafePct"},
      {[](Config& c) { c.valves[3].failsafePct = 254; }, "valves.4.failsafePct"},
      {[](Config& c) { c.failsafe.timeoutMin = 4; }, "failsafe.timeoutMin"},
      {[](Config& c) { c.failsafe.timeoutMin = 1; }, "failsafe.timeoutMin"},
      {[](Config& c) { c.failsafe.timeoutMin = 1441; }, "failsafe.timeoutMin"},
      {[](Config& c) { strcpy(c.web.allowedHosts, "a,,b"); }, "web.allowedHosts"},
      {[](Config& c) { c.mqtt.discoveryPrefix[0] = '\0'; }, "mqtt.discoveryPrefix"},
      {[](Config& c) { strcpy(c.mqtt.clientId, "a b"); }, "mqtt.clientId"},
      {[](Config& c) { strcpy(c.mqtt.rootTopic, "a/b"); }, "mqtt.rootTopic"},
      {[](Config& c) { strcpy(c.valves[0].topic, "a b"); }, "valves.1.topic"},
      {[](Config& c) { strcpy(c.temps[1].topic, "/t"); }, "temps.2.topic"},
      {[](Config& c) { strcpy(c.volts[2].topic, "v/"); }, "volts.3.topic"},
      {[](Config& c) { memset(c.valves[0].topic, 'x', sizeof c.valves[0].topic); },
       "valves.1.topic"},
  };
  for (const Case& k : cases) {
    CAPTURE(k.path);
    Config c;
    k.edit(c);
    CHECK(validatePath(c) == k.path);
  }
  Config ok;
  ok.valves[3].failsafePct = kFailsafeHold;
  ok.valves[4].failsafePct = 0;
  ok.valves[5].failsafePct = 100;
  ok.failsafe.timeoutMin = 0;
  CHECK(validatePath(ok) == "OK");
  ok.failsafe.timeoutMin = 5;
  CHECK(validatePath(ok) == "OK");
  ok.failsafe.timeoutMin = 1440;
  CHECK(validatePath(ok) == "OK");
}

TEST_CASE("config: V1 Home Assistant needs the decimal point") {
  Config c;
  c.mqtt.mode = MqttMode::MqttHa;
  strcpy(c.mqtt.host, "b");
  c.mqtt.germanDecimal = true;
  CHECK(validatePath(c) == "mqtt.germanDecimal");
  c.mqtt.mode = MqttMode::Mqtt;
  CHECK(validatePath(c) == "OK");
  c.mqtt.mode = MqttMode::MqttHa;
  c.mqtt.germanDecimal = false;
  CHECK(validatePath(c) == "OK");
  // Checked after every older rule.
  c.mqtt.germanDecimal = true;
  c.calib.hour = 24;
  CHECK(validatePath(c) == "calib.hour");
  std::string path;
  Config p;
  CHECK(patch(p, "{\"mqtt\":{\"mode\":2,\"host\":\"b\",\"germanDecimal\":true}}", &path) ==
        PatchResult::Invalid);
  CHECK(path == "mqtt.germanDecimal");
}

TEST_CASE("config: V2 every valve gets its own MQTT segment") {
  Config c;
  strcpy(c.valves[0].topic, "x");
  strcpy(c.valves[1].name, "x");
  CHECK(validatePath(c) == "valves.2.name");
  Config d;
  strcpy(d.valves[0].name, "x");
  strcpy(d.valves[1].topic, "x");
  CHECK(validatePath(d) == "valves.2.topic");
  Config e;  // an override equal to an unnamed valve's number
  strcpy(e.valves[0].topic, "3");
  CHECK(validatePath(e) == "valves.3.name");
  strcpy(e.valves[2].name, "three");
  CHECK(validatePath(e) == "OK");
  Config f;  // names map ' ' to '_'
  strcpy(f.valves[4].name, "a b");
  strcpy(f.valves[9].topic, "a_b");
  CHECK(validatePath(f) == "valves.10.topic");
  Config g;
  strcpy(g.valves[10].topic, "a/b");
  strcpy(g.valves[11].topic, "a/b");
  CHECK(validatePath(g) == "valves.12.topic");
  strcpy(g.valves[11].topic, "a/c");
  CHECK(validatePath(g) == "OK");
  Config h;  // the override replaces the name
  strcpy(h.valves[0].name, "same");
  strcpy(h.valves[0].topic, "one");
  strcpy(h.valves[1].topic, "same");
  CHECK(validatePath(h) == "OK");
  // V2 is reported before V3: valves 2/3 only share an HA id, valves 5/6 a
  // segment.
  Config k;
  strcpy(k.valves[1].name, "Bad 1");
  strcpy(k.valves[2].name, "Bad.1");
  strcpy(k.valves[4].topic, "x");
  strcpy(k.valves[5].name, "x");
  CHECK(validatePath(k) == "valves.6.name");
  k.valves[5].name[0] = '\0';
  CHECK(validatePath(k) == "valves.3.name");
}

TEST_CASE("config: V3 HA ids are unique among valves and active slots (C-1b)") {
  Config c;
  strcpy(c.valves[0].name, "Bad 1");
  strcpy(c.valves[1].name, "Bad.1");
  CHECK(validatePath(c) == "valves.2.name");
  strcpy(c.valves[1].name, "Bad-1");
  CHECK(validatePath(c) == "OK");
  Config u;  // UTF-8 letters map to their base letter
  strcpy(u.valves[3].name, "\xc5\x81" "azienka");
  strcpy(u.valves[7].topic, "Lazienka");
  CHECK(validatePath(u) == "valves.8.topic");
  Config o;
  strcpy(o.valves[0].topic, "a/b");
  strcpy(o.valves[1].topic, "a.b");
  CHECK(validatePath(o) == "valves.2.topic");

  Config t;
  t.temps[0].id = oid(kIdA);
  t.temps[0].active = true;
  strcpy(t.temps[0].name, "Bad 1");
  t.temps[5].id = oid(kIdB);
  t.temps[5].active = true;
  strcpy(t.temps[5].name, "Bad.1");
  CHECK(validatePath(t) == "temps.6.name");
  strcpy(t.temps[5].topic, "Bad/1");
  CHECK(validatePath(t) == "temps.6.topic");
  t.temps[5].active = false;  // only active slots count
  CHECK(validatePath(t) == "OK");
  t.temps[5].active = true;
  t.temps[0].active = false;
  CHECK(validatePath(t) == "OK");
  // Unnamed slots use their number.
  Config n;
  n.temps[0].id = oid(kIdA);
  n.temps[0].active = true;
  strcpy(n.temps[0].name, "2");
  n.temps[1].id = oid(kIdB);
  n.temps[1].active = true;
  CHECK(validatePath(n) == "temps.2.name");

  Config v;
  v.volts[2].id = oid(kIdA);
  v.volts[2].active = true;
  strcpy(v.volts[2].name, "a b");
  v.volts[7].id = oid(kIdV);
  v.volts[7].active = true;
  strcpy(v.volts[7].name, "a.b");
  CHECK(validatePath(v) == "volts.8.name");
  strcpy(v.volts[7].topic, "a/b");
  CHECK(validatePath(v) == "volts.8.topic");
  v.volts[2].active = false;
  CHECK(validatePath(v) == "OK");
  // Different tables never clash.
  Config m;
  strcpy(m.valves[0].name, "x");
  m.temps[0].id = oid(kIdA);
  m.temps[0].active = true;
  strcpy(m.temps[0].name, "x");
  m.volts[0].id = oid(kIdV);
  m.volts[0].active = true;
  strcpy(m.volts[0].name, "x");
  CHECK(validatePath(m) == "OK");
}

TEST_CASE("config: the cfg blob stays the 2.0.0 blob (C-2)") {
  // fullConfig() encoded by the 2.0.0 encoder (58632d6).
  static const uint8_t kGolden200[] = {
      0x56, 0x44, 0x4d, 0x43, 0x01, 0x00, 0x00, 0x03, 0x0a, 0x48, 0x65, 0x69, 0x7a, 0x75,
      0x6e, 0x67, 0x20, 0x4f, 0x47, 0x02, 0x00, 0xc0, 0xa8, 0x01, 0x32, 0xff, 0xff, 0xff,
      0x00, 0xc0, 0xa8, 0x01, 0x01, 0x08, 0x08, 0x08, 0x08, 0x07, 0x4d, 0x79, 0x20, 0x57,
      0x69, 0x66, 0x69, 0x09, 0x73, 0x65, 0x63, 0x72, 0x65, 0x74, 0x31, 0x32, 0x33, 0x11,
      0x0b, 0x31, 0x39, 0x32, 0x2e, 0x31, 0x36, 0x38, 0x2e, 0x31, 0x2e, 0x31, 0x0d, 0x45,
      0x75, 0x72, 0x6f, 0x70, 0x65, 0x2f, 0x57, 0x61, 0x72, 0x73, 0x61, 0x77, 0x1b, 0x43,
      0x45, 0x54, 0x2d, 0x31, 0x43, 0x45, 0x53, 0x54, 0x2c, 0x4d, 0x33, 0x2e, 0x35, 0x2e,
      0x30, 0x2c, 0x4d, 0x31, 0x30, 0x2e, 0x35, 0x2e, 0x30, 0x2f, 0x33, 0x78, 0x03, 0x0a,
      0x00, 0x00, 0x09, 0xea, 0x05, 0x05, 0x61, 0x64, 0x6d, 0x69, 0x6e, 0x07, 0x70, 0x61,
      0x20, 0x73, 0x73, 0x22, 0x77, 0x01, 0x01, 0x0a, 0x62, 0x72, 0x6f, 0x6b, 0x65, 0x72,
      0x2e, 0x6c, 0x61, 0x6e, 0xb3, 0x22, 0x02, 0x6d, 0x71, 0x04, 0x6d, 0x71, 0x70, 0x77,
      0x1e, 0x00, 0x78, 0x00, 0x07, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x01, 0x00, 0x00, 0x00, 0x03, 0x42, 0x61, 0x64, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x07, 0x4b, 0x69, 0x74, 0x63, 0x68, 0x65, 0x6e, 0x00, 0x02, 0x74, 0x31, 0x01,
      0xf1, 0xff, 0x28, 0x84, 0x37, 0x94, 0x97, 0xff, 0x03, 0x23, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x64, 0x00, 0x28, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x01, 0x67,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80,
      0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x80, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00,
      0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0x62, 0x61, 0x74, 0x01, 0x00, 0x00,
      0x80, 0xbe, 0x0a, 0xd7, 0x23, 0x3c, 0x01, 0x56, 0x26, 0x11, 0x22, 0x33, 0x44, 0x55,
      0x66, 0x29, 0x7f, 0x17, 0x3b, 0x00, 0xc0, 0xc3, 0x24, 0xb0,
  };
  const std::vector<uint8_t> golden(kGolden200, kGolden200 + sizeof kGolden200);
  CHECK(encode(fullConfig()) == golden);
  // The keys added later do not touch it.
  CHECK(encode(fullConfigExt()) == golden);
  Config back;
  CHECK(decodeConfig(golden.data(), golden.size(), back) == DecodeResult::Ok);
  CHECK(sameConfig(back, fullConfig()));
}

namespace {

std::vector<uint8_t> encodeExt(const Config& c, const uint8_t* keep = nullptr, size_t n = 0) {
  std::vector<uint8_t> b(kConfigExtBlobMax);
  const size_t len = encodeConfigExt(c, b.data(), b.size(), keep, n);
  REQUIRE(len > 0);
  b.resize(len);
  return b;
}

// A cfgx blob from raw records.
std::vector<uint8_t> extBlob(const std::vector<uint8_t>& records, uint8_t version = 1) {
  std::vector<uint8_t> b = {'V', 'D', 'M', 'X', version, static_cast<uint8_t>(records.size()),
                            static_cast<uint8_t>(records.size() >> 8)};
  b.insert(b.end(), records.begin(), records.end());
  b.resize(b.size() + 4);
  fixCrc(b);
  return b;
}

ExtResult decodeExt(const std::vector<uint8_t>& b, Config& c, ExtInfo* info = nullptr,
                    uint8_t* keep = nullptr, size_t keepCap = 0) {
  return decodeConfigExt(b.data(), b.size(), c, info, keep, keepCap);
}

}  // namespace

TEST_CASE("config: cfgx layout and round trip (C-4)") {
  const Config full = fullConfigExt();
  const std::vector<uint8_t> x = encodeExt(full);
  REQUIRE(x.size() > 11);
  CHECK(x[0] == 'V');
  CHECK(x[1] == 'D');
  CHECK(x[2] == 'M');
  CHECK(x[3] == 'X');
  CHECK(x[4] == 1);
  CHECK(static_cast<size_t>(x[5] | (x[6] << 8)) == x.size() - 11);
  const uint32_t crc = crc32(x.data(), x.size() - 4);
  CHECK(x[x.size() - 4] == static_cast<uint8_t>(crc));
  CHECK(x[x.size() - 3] == static_cast<uint8_t>(crc >> 8));
  CHECK(x[x.size() - 2] == static_cast<uint8_t>(crc >> 16));
  CHECK(x[x.size() - 1] == static_cast<uint8_t>(crc >> 24));
  // Table order: web.allowedHosts first, as tag, element, length, bytes.
  const std::string hosts = "vdmot.lan, 192.168.1.9";
  CHECK(x[7] == 1);
  CHECK(x[8] == 0);
  CHECK(x[9] == hosts.size());
  CHECK(std::string(x.begin() + 10, x.begin() + 10 + static_cast<long>(hosts.size())) == hosts);
  // Then mqtt.rootTopic.
  const size_t root = 10 + hosts.size();
  CHECK(x[root] == 2);
  CHECK(x[root + 1] == 0);
  CHECK(x[root + 2] == 8);
  CHECK(memcmp(&x[root + 3], "VdMotFBH", 8) == 0);
  // failsafe.timeoutMin 1440 = 0x05A0: tag 5, element 0, length 2, u16 LE.
  const uint8_t timeout[] = {5, 0, 2, 0xA0, 0x05};
  CHECK(std::search(x.begin(), x.end(), timeout, timeout + 5) != x.end());
  // valves.12.failsafePct 0 and its topic "x": tags 6 and 7, element 11.
  const uint8_t valve12[] = {6, 11, 1, 0, 7, 11, 1, 'x'};
  CHECK(std::search(x.begin(), x.end(), valve12, valve12 + 8) != x.end());

  // Round trip on top of the decoded base blob.
  Config back;
  REQUIRE(decodeConfig(encode(full).data(), encode(full).size(), back) == DecodeResult::Ok);
  CHECK_FALSE(sameConfig(back, full));
  ExtInfo info;
  info.bad = 9;
  CHECK(decodeExt(x, back, &info) == ExtResult::Ok);
  CHECK(sameConfig(back, full));
  CHECK(info.applied == 1 + 3 + 12 * 2 + 34 + 8 + 1);
  CHECK(info.unknown == 0);
  CHECK(info.bad == 0);
  CHECK(info.keepLen == 0);
  // The defaults: every record present, strings also when empty.
  const std::vector<uint8_t> d = encodeExt(Config{});
  Config z = full;
  CHECK(decodeExt(d, z) == ExtResult::Ok);
  Config expect = full;
  const Config defaults;
  memcpy(expect.web.allowedHosts, defaults.web.allowedHosts, sizeof expect.web.allowedHosts);
  memcpy(expect.mqtt.rootTopic, defaults.mqtt.rootTopic, sizeof expect.mqtt.rootTopic);
  memcpy(expect.mqtt.clientId, defaults.mqtt.clientId, sizeof expect.mqtt.clientId);
  memcpy(expect.mqtt.discoveryPrefix, defaults.mqtt.discoveryPrefix,
         sizeof expect.mqtt.discoveryPrefix);
  expect.failsafe = defaults.failsafe;
  for (ValveConfig& v : expect.valves) {
    v.failsafePct = kFailsafePctDefault;
    v.topic[0] = '\0';
  }
  for (TempSlotConfig& t : expect.temps) t.topic[0] = '\0';
  for (VoltSlotConfig& v : expect.volts) v.topic[0] = '\0';
  CHECK(sameConfig(z, expect));
}

TEST_CASE("config: cfgx encode needs the full capacity") {
  const Config full = fullConfigExt();
  const std::vector<uint8_t> x = encodeExt(full);
  std::vector<uint8_t> buf(x.size());
  for (size_t cap = 0; cap < x.size(); ++cap) {
    CAPTURE(cap);
    CHECK(encodeConfigExt(full, buf.data(), cap) == 0);
  }
  CHECK(encodeConfigExt(full, buf.data(), x.size()) == x.size());
  CHECK(buf == x);
  CHECK(encodeConfigExt(full, nullptr, kConfigExtBlobMax) == 0);
  // The largest possible blob plus the kept records fits kConfigExtBlobMax.
  Config big;
  memset(big.web.allowedHosts, 'a', kAllowedHostsMax);
  memset(big.mqtt.rootTopic, 'r', kStationNameMax);
  memset(big.mqtt.clientId, 'c', kClientIdMax);
  memset(big.mqtt.discoveryPrefix, 'p', kTopicPrefixMax);
  for (ValveConfig& v : big.valves) memset(v.topic, 't', kItemNameMax);
  for (TempSlotConfig& t : big.temps) memset(t.topic, 't', kItemNameMax);
  for (VoltSlotConfig& v : big.volts) memset(v.topic, 't', kItemNameMax);
  const std::vector<uint8_t> keep(kConfigExtKeepMax, 0);
  CHECK(encodeExt(big, keep.data(), keep.size()).size() <= kConfigExtBlobMax);
}

TEST_CASE("config: cfgx unknown records are kept and written back (C-4)") {
  // Known record, unknown tag 200, known record.
  const std::vector<uint8_t> recs = {5, 0, 2, 10, 0,           // failsafe.timeoutMin 10
                                     200, 0, 3, 'a', 'b', 'c',  // unknown
                                     6, 2, 1, 70};              // valves.3.failsafePct 70
  const std::vector<uint8_t> b = extBlob(recs);
  Config c;
  ExtInfo info;
  uint8_t keep[kConfigExtKeepMax];
  CHECK(decodeExt(b, c, &info, keep, sizeof keep) == ExtResult::Ok);
  CHECK(info.applied == 2);
  CHECK(info.unknown == 1);
  CHECK(info.bad == 0);
  REQUIRE(info.keepLen == 6);
  CHECK(memcmp(keep, &recs[5], 6) == 0);
  CHECK(c.failsafe.timeoutMin == 10);
  CHECK(c.valves[2].failsafePct == 70);
  // Re-emitted verbatim after the own records.
  const std::vector<uint8_t> x = encodeExt(c, keep, info.keepLen);
  CHECK(std::equal(recs.begin() + 5, recs.begin() + 11, x.end() - 10));
  Config again;
  ExtInfo i2;
  uint8_t keep2[kConfigExtKeepMax];
  CHECK(decodeExt(x, again, &i2, keep2, sizeof keep2) == ExtResult::Ok);
  CHECK(i2.unknown == 1);
  CHECK(i2.keepLen == 6);
  CHECK(sameConfig(again, c));
  // Without a keep buffer, or one that is too small, the record is only counted.
  ExtInfo i3;
  CHECK(decodeExt(b, again, &i3) == ExtResult::Ok);
  CHECK(i3.unknown == 1);
  CHECK(i3.keepLen == 0);
  uint8_t tiny[5];
  CHECK(decodeExt(b, again, &i3, tiny, sizeof tiny) == ExtResult::Ok);
  CHECK(i3.keepLen == 0);
  uint8_t exact[6];
  CHECK(decodeExt(b, again, &i3, exact, sizeof exact) == ExtResult::Ok);
  CHECK(i3.keepLen == 6);
  // At most kConfigExtKeepMax bytes are kept, whole records only.
  std::vector<uint8_t> many;
  for (int i = 0; i < 70; ++i) {
    const uint8_t r[] = {201, 0, 5, 1, 2, 3, 4, static_cast<uint8_t>(i)};
    many.insert(many.end(), r, r + 8);
  }
  std::vector<uint8_t> big(1024);
  ExtInfo i4;
  CHECK(decodeExt(extBlob(many), again, &i4, big.data(), big.size()) == ExtResult::Ok);
  CHECK(i4.unknown == 70);
  CHECK(i4.keepLen == 512);
  CHECK(big[511] == 63);
  CHECK(big[512] == 0);
  ExtInfo i5;
  CHECK(decodeExt(extBlob(many), again, &i5, big.data(), 511) == ExtResult::Ok);
  CHECK(i5.keepLen == 504);
}

TEST_CASE("config: cfgx bad records change nothing (C-4)") {
  struct Case {
    std::vector<uint8_t> rec;
    const char* why;
  };
  const Case cases[] = {
      {{6, 12, 1, 40}, "valve element 12"},
      {{6, 255, 1, 40}, "valve element 255"},
      {{5, 1, 2, 10, 0}, "object element 1"},
      {{6, 0, 2, 40, 0}, "pct with 2 bytes"},
      {{6, 0, 0}, "pct with 0 bytes"},
      {{6, 0, 1, 101}, "pct 101"},
      {{6, 0, 1, 254}, "pct 254"},
      {{5, 0, 1, 10}, "timeout with 1 byte"},
      {{5, 0, 3, 10, 0, 0}, "timeout with 3 bytes"},
      {{5, 0, 2, 4, 0}, "timeout 4"},
      {{5, 0, 2, 0xA1, 0x05}, "timeout 1441"},
      {{7, 0, 11, 'a', 'a', 'a', 'a', 'a', 'a', 'a', 'a', 'a', 'a', 'a'}, "topic of 11"},
      {{7, 0, 3, 'a', 0, 'b'}, "NUL inside"},
      {{7, 0, 3, 'a', ' ', 'b'}, "topic rule"},
      {{4, 0, 0}, "empty discovery prefix"},
      {{1, 0, 4, 'a', ',', ',', 'b'}, "host list rule"},
      {{8, 34, 1, 't'}, "temp element 34"},
      {{9, 8, 1, 'v'}, "volt element 8"},
  };
  for (const Case& k : cases) {
    CAPTURE(k.why);
    Config c;
    ExtInfo info;
    CHECK(decodeExt(extBlob(k.rec), c, &info) == ExtResult::Ok);
    CHECK(info.bad == 1);
    CHECK(info.applied == 0);
    CHECK(info.unknown == 0);
    CHECK(sameConfig(c, Config{}));
  }
  // The next record still applies; the last record for a field wins.
  Config c;
  ExtInfo info;
  CHECK(decodeExt(extBlob({6, 12, 1, 40, 6, 11, 1, 41, 6, 11, 1, 42, 5, 0, 2, 0, 0}), c, &info) ==
        ExtResult::Ok);
  CHECK(info.bad == 1);
  CHECK(info.applied == 3);
  CHECK(c.valves[11].failsafePct == 42);
  CHECK(c.failsafe.timeoutMin == 0);
  // Longest valid string and the edge values.
  Config e;
  CHECK(decodeExt(extBlob({7, 0, 10, 'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 6, 0, 1,
                           255, 5, 0, 2, 0xA0, 0x05, 8, 33, 1, 't', 9, 7, 1, 'v'}),
                  e, &info) == ExtResult::Ok);
  CHECK(info.applied == 5);
  CHECK(std::string(e.valves[0].topic) == "abcdefghij");
  CHECK(e.valves[0].failsafePct == kFailsafeHold);
  CHECK(e.failsafe.timeoutMin == 1440);
  CHECK(std::string(e.temps[33].topic) == "t");
  CHECK(std::string(e.volts[7].topic) == "v");
  // A shorter string replaces a longer one completely.
  CHECK(decodeExt(extBlob({7, 0, 2, 'x', 'y'}), e, &info) == ExtResult::Ok);
  CHECK(std::string(e.valves[0].topic) == "xy");
}

TEST_CASE("config: cfgx damage applies nothing (C-4)") {
  const Config full = fullConfigExt();
  const std::vector<uint8_t> good = encodeExt(full);
  auto expect = [&](const std::vector<uint8_t>& b, ExtResult r) {
    Config c;
    ExtInfo info;
    info.applied = 7;
    CHECK(decodeExt(b, c, &info) == r);
    CHECK(sameConfig(c, Config{}));
    CHECK(info.applied == 0);
  };
  Config c;
  ExtInfo info;
  info.unknown = 3;
  CHECK(decodeConfigExt(nullptr, 20, c, &info) == ExtResult::Absent);
  CHECK(info.unknown == 0);
  CHECK(decodeConfigExt(good.data(), 0, c, &info) == ExtResult::Absent);
  CHECK(decodeConfigExt(good.data(), good.size(), c) == ExtResult::Ok);  // info optional
  for (size_t n = 1; n < 11; ++n) {
    CAPTURE(n);
    expect(std::vector<uint8_t>(good.begin(), good.begin() + static_cast<long>(n)),
           ExtResult::TooShort);
  }
  expect(std::vector<uint8_t>(good.begin(), good.end() - 1), ExtResult::TooShort);
  {
    std::vector<uint8_t> b = good;  // L one too large
    const size_t l = good.size() - 11 + 1;
    b[5] = static_cast<uint8_t>(l);
    b[6] = static_cast<uint8_t>(l >> 8);
    expect(b, ExtResult::TooShort);
  }
  for (int i = 0; i < 4; ++i) {
    std::vector<uint8_t> b = good;
    b[static_cast<size_t>(i)] ^= 0x20;
    expect(b, ExtResult::BadMagic);
  }
  {
    std::vector<uint8_t> b = good;
    b[20] ^= 1;
    expect(b, ExtResult::BadCrc);
    b = good;
    b.back() ^= 0x80;
    expect(b, ExtResult::BadCrc);
    b = good;
    b[good.size() - 4] ^= 0x01;
    expect(b, ExtResult::BadCrc);
  }
  // Empty payload: header and CRC only.
  CHECK(decodeExt(extBlob({}), c, &info) == ExtResult::Ok);
  CHECK(info.applied == 0);
  // A newer format version is read like version 1; bytes after the CRC are
  // ignored.
  Config v2;
  CHECK(decodeExt(extBlob({5, 0, 2, 30, 0}, 2), v2) == ExtResult::Ok);
  CHECK(v2.failsafe.timeoutMin == 30);
  std::vector<uint8_t> tail = extBlob({5, 0, 2, 31, 0});
  tail.push_back(0xEE);
  CHECK(decodeExt(tail, v2) == ExtResult::Ok);
  CHECK(v2.failsafe.timeoutMin == 31);
  // A record cut by the payload end: counted as bad, the ones before it apply.
  for (const std::vector<uint8_t>& cut :
       {std::vector<uint8_t>{5, 0, 2, 32, 0, 6}, std::vector<uint8_t>{5, 0, 2, 32, 0, 6, 0},
        std::vector<uint8_t>{5, 0, 2, 32, 0, 6, 0, 1}, std::vector<uint8_t>{5, 0, 2, 32, 0, 7, 0, 3,
                                                                           'a', 'b'}}) {
    CAPTURE(cut.size());
    Config k;
    ExtInfo ki;
    const std::vector<uint8_t> blob = extBlob(cut);
    const std::vector<uint8_t> exact(blob.begin(), blob.end());  // ASan: nothing past it is read
    CHECK(decodeExt(exact, k, &ki) == ExtResult::Ok);
    CHECK(ki.applied == 1);
    CHECK(ki.bad == 1);
    CHECK(k.failsafe.timeoutMin == 32);
  }
}

TEST_CASE("config: restart reasons (C-7)") {
  struct Row {
    const char* what;
    void (*edit)(Config&);
    uint8_t reasons;
  };
  const Row rows[] = {
      {"reconnectTimeoutMin", [](Config& c) { c.net.reconnectTimeoutMin = 0; }, 0},
      {"dns with DHCP", [](Config& c) { c.net.dns = 0x08080808; }, 0},
      {"ip with DHCP", [](Config& c) { c.net.ip = 0x0101A8C0; }, 0},
      {"mask with DHCP", [](Config& c) { c.net.mask = 0x00FFFFFF; }, 0},
      {"gateway with DHCP", [](Config& c) { c.net.gateway = 0x0501A8C0; }, 0},
      {"iface", [](Config& c) { c.net.iface = NetInterface::Wifi; }, kRestartNetwork},
      {"dhcp", [](Config& c) { c.net.dhcp = false; }, kRestartNetwork},
      {"ssid with Auto", [](Config& c) { strcpy(c.net.ssid, "w"); }, kRestartNetwork},
      {"wifiPassword with Auto", [](Config& c) { strcpy(c.net.wifiPassword, "12345678"); },
       kRestartNetwork},
      {"station Dom 1 -> Dom  1", [](Config& c) { strcpy(c.station, "Dom  1"); }, 0},
      {"station Dom 1 -> Dom_1", [](Config& c) { strcpy(c.station, "Dom_1"); }, kRestartHostname},
      {"mqtt.host", [](Config& c) { strcpy(c.mqtt.host, "b"); }, 0},
      {"calib", [](Config& c) { c.calib.hour = 3; }, 0},
      {"failsafe", [](Config& c) { c.failsafe.timeoutMin = 0; }, 0},
      {"both",
       [](Config& c) {
         c.net.dhcp = false;
         strcpy(c.station, "Dom_1");
       },
       kRestartNetwork | kRestartHostname},
  };
  Config before;
  strcpy(before.station, "Dom 1");  // host name "Dom-1"
  for (const Row& r : rows) {
    CAPTURE(r.what);
    Config after = before;
    r.edit(after);
    CHECK(configRestartReasons(before, after) == r.reasons);
    CHECK(configRestartReasons(after, before) == r.reasons);
    CHECK(configRestartReasons(after, after) == 0);
  }
  // Static addresses: every field counts, a dns of 0.0.0.0 means the gateway.
  Config st;
  st.net.dhcp = false;
  st.net.ip = 0x3201A8C0;
  st.net.mask = 0x00FFFFFF;
  st.net.gateway = 0x0101A8C0;
  const Row statics[] = {
      {"dns with static", [](Config& c) { c.net.dns = 0x08080808; }, kRestartNetwork},
      {"dns = gateway with static", [](Config& c) { c.net.dns = 0x0101A8C0; }, 0},
      {"gateway with static and dns 0.0.0.0",
       [](Config& c) { c.net.gateway = 0xFE01A8C0; }, kRestartNetwork},
      {"ip with static", [](Config& c) { c.net.ip = 0x3301A8C0; }, kRestartNetwork},
  };
  for (const Row& r : statics) {
    CAPTURE(r.what);
    Config after = st;
    r.edit(after);
    CHECK(configRestartReasons(st, after) == r.reasons);
  }
  // WiFi credentials do not matter with Ethernet before and after.
  Config eth = st;
  eth.net.iface = NetInterface::Ethernet;
  Config ethWifi = eth;
  strcpy(ethWifi.net.ssid, "w");
  strcpy(ethWifi.net.wifiPassword, "12345678");
  CHECK(configRestartReasons(eth, ethWifi) == 0);
  // Host names "K-che" and "Kuche".
  Config k1, k2;
  strcpy(k1.station, "K\xc3\xbc" "che");
  strcpy(k2.station, "Kuche");
  CHECK(configRestartReasons(k1, k2) == kRestartHostname);
}

TEST_CASE("config: network trial rule (C-7)") {
  NetConfig dhcp;
  NetConfig st;
  st.dhcp = false;
  st.ip = 0x3201A8C0;
  st.mask = 0x00FFFFFF;
  st.gateway = 0x0101A8C0;
  CHECK_FALSE(netTrialRequired(st, st));
  CHECK_FALSE(netTrialRequired(dhcp, dhcp));
  CHECK(netTrialRequired(dhcp, st));
  CHECK(netTrialRequired(st, dhcp));
  struct Row {
    const char* what;
    void (*edit)(NetConfig&);
    bool trial;
  };
  const Row rows[] = {
      {"ip", [](NetConfig& n) { n.ip = 0x3301A8C0; }, true},
      {"mask", [](NetConfig& n) { n.mask = 0x0000FFFF; }, true},
      {"gateway", [](NetConfig& n) { n.gateway = 0xFE01A8C0; }, true},
      {"dns", [](NetConfig& n) { n.dns = 0x08080808; }, true},
      {"dns = gateway", [](NetConfig& n) { n.dns = 0x0101A8C0; }, false},
      {"reconnect", [](NetConfig& n) { n.reconnectTimeoutMin = 9; }, false},
      {"ssid", [](NetConfig& n) { strcpy(n.ssid, "w"); }, true},
      {"password", [](NetConfig& n) { strcpy(n.wifiPassword, "12345678"); }, true},
  };
  for (const Row& r : rows) {
    CAPTURE(r.what);
    NetConfig after = st;
    r.edit(after);
    CHECK(netTrialRequired(st, after) == r.trial);
    CHECK(netTrialRequired(after, st) == r.trial);
  }
  // Static fields do not matter with DHCP after the change.
  NetConfig d2 = dhcp;
  d2.ip = 5;
  d2.dns = 7;
  CHECK_FALSE(netTrialRequired(dhcp, d2));
  // WiFi credentials do not matter when Ethernet is used before and after.
  NetConfig e1;
  e1.iface = NetInterface::Ethernet;
  NetConfig e2 = e1;
  strcpy(e2.ssid, "w");
  strcpy(e2.wifiPassword, "12345678");
  CHECK_FALSE(netTrialRequired(e1, e2));
  e1.iface = NetInterface::Wifi;
  e2.iface = NetInterface::Wifi;
  CHECK(netTrialRequired(e1, e2));
  strcpy(e1.ssid, "w");
  CHECK(netTrialRequired(e1, e2));
  strcpy(e1.wifiPassword, "12345678");
  CHECK_FALSE(netTrialRequired(e1, e2));
  // Bytes after the NUL are not part of the text.
  e2.ssid[3] = 'z';
  e2.wifiPassword[20] = 'z';
  CHECK_FALSE(netTrialRequired(e1, e2));
}

TEST_CASE("config: effectiveDns, mqttRootTopic and itemSegment (C-8)") {
  NetConfig n;
  n.gateway = 0x0101A8C0;
  CHECK(effectiveDns(n) == 0);  // DHCP: the configured dns
  n.dns = 0x08080808;
  CHECK(effectiveDns(n) == 0x08080808u);
  n.dhcp = false;
  CHECK(effectiveDns(n) == 0x08080808u);
  n.dns = 0;
  CHECK(effectiveDns(n) == 0x0101A8C0u);  // static without dns: the gateway

  Config c;
  CHECK(mqttRootTopic(c) == c.station);
  strcpy(c.mqtt.rootTopic, "VdMotFBH");
  CHECK(std::string(mqttRootTopic(c)) == "VdMotFBH");

  char out[11];
  memset(out, 'x', sizeof out);
  CHECK(itemSegment(c, ItemKind::Valve, 2, out, sizeof out) == 1);
  CHECK(std::string(out) == "3");
  CHECK(itemSegment(c, ItemKind::Valve, 11, out, sizeof out) == 2);
  CHECK(std::string(out) == "12");
  CHECK(itemSegment(c, ItemKind::Temp, 33, out, sizeof out) == 2);
  CHECK(std::string(out) == "34");
  CHECK(itemSegment(c, ItemKind::Volt, 7, out, sizeof out) == 1);
  CHECK(std::string(out) == "8");
  strcpy(c.valves[2].name, "Bad OG 1");
  CHECK(itemSegment(c, ItemKind::Valve, 2, out, sizeof out) == 8);
  CHECK(std::string(out) == "Bad_OG_1");
  strcpy(c.valves[2].topic, "Bad/WC");
  CHECK(itemSegment(c, ItemKind::Valve, 2, out, sizeof out) == 6);
  CHECK(std::string(out) == "Bad/WC");
  strcpy(c.temps[0].name, "t 1");
  CHECK(itemSegment(c, ItemKind::Temp, 0, out, sizeof out) == 3);
  CHECK(std::string(out) == "t_1");
  strcpy(c.temps[0].topic, "t/1");
  CHECK(itemSegment(c, ItemKind::Temp, 0, out, sizeof out) == 3);
  CHECK(std::string(out) == "t/1");
  strcpy(c.volts[7].name, "bat");
  CHECK(itemSegment(c, ItemKind::Volt, 7, out, sizeof out) == 3);
  CHECK(std::string(out) == "bat");
  strcpy(c.volts[7].topic, "v/8");
  CHECK(itemSegment(c, ItemKind::Volt, 7, out, sizeof out) == 3);
  CHECK(std::string(out) == "v/8");
  memcpy(c.valves[0].name, "abcdefghij", 11);
  CHECK(itemSegment(c, ItemKind::Valve, 0, out, sizeof out) == 10);  // exactly fits
  CHECK(std::string(out) == "abcdefghij");
  // Too small, out of range, no buffer.
  memset(out, 'x', sizeof out);
  CHECK(itemSegment(c, ItemKind::Valve, 0, out, 10) == 0);
  CHECK(out[0] == '\0');
  memset(out, 'x', sizeof out);
  CHECK(itemSegment(c, ItemKind::Valve, 11, out, 2) == 0);
  CHECK(out[0] == '\0');
  CHECK(itemSegment(c, ItemKind::Valve, 11, out, 3) == 2);
  for (const auto& bad : {std::make_pair(ItemKind::Valve, 12), std::make_pair(ItemKind::Temp, 34),
                          std::make_pair(ItemKind::Volt, 8),
                          std::make_pair(static_cast<ItemKind>(3), 0)}) {
    memset(out, 'x', sizeof out);
    CHECK(itemSegment(c, bad.first, static_cast<uint8_t>(bad.second), out, sizeof out) == 0);
    CHECK(out[0] == '\0');
  }
  out[0] = 'x';
  CHECK(itemSegment(c, ItemKind::Valve, 0, out, 0) == 0);
  CHECK(out[0] == 'x');
  CHECK(itemSegment(c, ItemKind::Valve, 0, nullptr, 8) == 0);
}

TEST_CASE("config: what changes the MQTT topics") {
  const Config base = fullConfigExt();
  CHECK_FALSE(mqttTopicConfigChanged(base, base));
  struct Row {
    const char* what;
    void (*edit)(Config&);
    bool changed;
  };
  const Row rows[] = {
      {"station", [](Config& c) { strcpy(c.station, "Other"); }, true},
      {"mqtt.mode", [](Config& c) { c.mqtt.mode = MqttMode::Off; }, true},
      {"mqtt.host", [](Config& c) { strcpy(c.mqtt.host, "other"); }, true},
      {"mqtt.port", [](Config& c) { c.mqtt.port = 1; }, true},
      {"mqtt.password", [](Config& c) { strcpy(c.mqtt.password, "x"); }, true},
      {"mqtt.keepAliveS", [](Config& c) { c.mqtt.keepAliveS = 60; }, true},
      {"mqtt.separate", [](Config& c) { c.mqtt.separate = true; }, true},
      {"mqtt.haDiscoveryOnConnect", [](Config& c) { c.mqtt.haDiscoveryOnConnect = true; }, true},
      {"mqtt.rootTopic", [](Config& c) { strcpy(c.mqtt.rootTopic, "R"); }, true},
      {"mqtt.clientId", [](Config& c) { strcpy(c.mqtt.clientId, "c"); }, true},
      {"mqtt.discoveryPrefix", [](Config& c) { strcpy(c.mqtt.discoveryPrefix, "hb"); }, true},
      {"valve name", [](Config& c) { strcpy(c.valves[11].name, "K"); }, true},
      {"valve active", [](Config& c) { c.valves[11].active = true; }, true},
      {"valve topic", [](Config& c) { strcpy(c.valves[11].topic, "y"); }, true},
      {"temp name", [](Config& c) { strcpy(c.temps[33].name, "n"); }, true},
      {"temp active", [](Config& c) { c.temps[33].active = true; }, true},
      {"temp topic", [](Config& c) { strcpy(c.temps[33].topic, "z"); }, true},
      {"temp id", [](Config& c) { c.temps[33].id.b[7] ^= 1; }, true},
      {"volt name", [](Config& c) { strcpy(c.volts[0].name, "n"); }, true},
      {"volt active", [](Config& c) { c.volts[7].active = false; }, true},
      {"volt topic", [](Config& c) { strcpy(c.volts[7].topic, "b2"); }, true},
      {"volt id", [](Config& c) { c.volts[7].id.b[0] ^= 1; }, true},
      {"failsafe.timeoutMin", [](Config& c) { c.failsafe.timeoutMin = 5; }, false},
      {"valve failsafePct", [](Config& c) { c.valves[0].failsafePct = 7; }, false},
      {"temp offset", [](Config& c) { c.temps[0].offset = 3; }, false},
      {"volt offset", [](Config& c) { c.volts[7].offset = 3.0f; }, false},
      {"volt factor", [](Config& c) { c.volts[7].factor = 3.0f; }, false},
      {"volt unit", [](Config& c) { strcpy(c.volts[7].unit, "A"); }, false},
      {"web", [](Config& c) { c.web.protectRead = false; }, false},
      {"net", [](Config& c) { c.net.reconnectTimeoutMin = 1; }, false},
      {"calib", [](Config& c) { c.calib.hour = 1; }, false},
      {"persistLog", [](Config& c) { c.persistLog = true; }, false},
      // Bytes after the NUL are not part of a name.
      {"after the NUL", [](Config& c) { c.mqtt.host[20] = 'q'; }, false},
  };
  for (const Row& r : rows) {
    CAPTURE(r.what);
    Config after = base;
    r.edit(after);
    CHECK(mqttTopicConfigChanged(base, after) == r.changed);
    CHECK(mqttTopicConfigChanged(after, base) == r.changed);
  }
}

TEST_CASE("config: JSON export with secrets and the apply members (C-9)") {
  Config c;
  strcpy(c.net.wifiPassword, "w\"1");
  strcpy(c.web.user, "u");
  strcpy(c.web.password, "pw");
  const std::string flags = exportJson(c);
  CHECK(flags.find("\"wifiPasswordSet\":true") != std::string::npos);
  CHECK(flags.find("pw") == std::string::npos);
  static char buf[8192];
  JsonWriter jw(buf, sizeof buf);
  REQUIRE(writeConfigJson(jw, c, SecretMode::Clear));
  const std::string clear(buf, jw.length());
  CHECK(clear.find("Set\"") == std::string::npos);
  CHECK(clear.find("\"ssid\":\"\",\"wifiPassword\":\"w\\\"1\",\"reconnectTimeoutMin\":5") !=
        std::string::npos);
  CHECK(clear.find("\"web\":{\"user\":\"u\",\"password\":\"pw\",\"protectRead\":false,") !=
        std::string::npos);
  CHECK(clear.find("\"user\":\"\",\"password\":\"\",\"keepAliveS\":60") != std::string::npos);
  CHECK(clear.compare(clear.size() - 18, 18, "\"persistLog\":true}") == 0);
  // Golden: the defaults with SecretMode::Clear are the Flags document (golden
  // above) with the secrets in place of the flags.
  std::string expect = exportJson(Config{});
  auto replaceAll = [](std::string& s, const std::string& from, const std::string& to) {
    for (size_t at = s.find(from); at != std::string::npos; at = s.find(from, at + to.size())) {
      s.replace(at, from.size(), to);
    }
  };
  replaceAll(expect, "\"wifiPasswordSet\":false", "\"wifiPassword\":\"\"");
  replaceAll(expect, "\"passwordSet\":false", "\"password\":\"\"");
  JsonWriter jd(buf, sizeof buf);
  REQUIRE(writeConfigJson(jd, Config{}, SecretMode::Clear));
  CHECK(std::string(buf, jd.length()) == expect);
  // A cleartext export posts back to the same config.
  Config back;
  std::string path;
  CHECK(patch(back, clear, &path) == PatchResult::Ok);
  CHECK(sameConfig(back, c));

  auto endsWith = [](const std::string& s, const std::string& t) {
    return s.size() >= t.size() && s.compare(s.size() - t.size(), t.size(), t) == 0;
  };
  ApplyInfo apply;
  apply.restartRequired = true;
  JsonWriter ja(buf, sizeof buf);
  REQUIRE(writeConfigJson(ja, c, SecretMode::Flags, &apply));
  const std::string a(buf, ja.length());
  CHECK(endsWith(a, "\"persistLog\":true,\"restartRequired\":true,\"netTrial\":false}"));
  apply.restartRequired = false;
  apply.netTrial = true;
  JsonWriter jb(buf, sizeof buf);
  REQUIRE(writeConfigJson(jb, c, SecretMode::Flags, &apply));
  const std::string b(buf, jb.length());
  CHECK(endsWith(b, "\"persistLog\":true,\"restartRequired\":false,\"netTrial\":true}"));
  // The apply members are not config keys.
  CHECK(patch(back, a, &path) == PatchResult::UnknownKey);
  CHECK(path == "restartRequired");
}

TEST_CASE("config: export and patch round trip with every key") {
  const Config full = fullConfigExt();
  static char buf[8192];
  JsonWriter jw(buf, sizeof buf);
  REQUIRE(writeConfigJson(jw, full, SecretMode::Clear));
  Config c;
  std::string path;
  CHECK(patch(c, std::string(buf, jw.length()), &path) == PatchResult::Ok);
  CHECK(path.empty());
  CHECK(sameConfig(c, full));
}

TEST_CASE("config: load and repair API before the repairs") {
  Config c = fullConfigExt();
  Repairs r;
  r.mask = 5;
  r.count = 2;
  strcpy(r.first, "x");
  CHECK(sanitizeConfig(c, &r) == 0);
  CHECK(r.mask == 0);
  CHECK(r.count == 0);
  CHECK(r.first[0] == '\0');
  CHECK(sanitizeConfig(c, nullptr) == 0);
  CHECK(sameConfig(c, fullConfigExt()));

  // decodeConfig reports the header schema.
  const std::vector<uint8_t> base = encode(fullConfigExt());
  DecodeInfo info;
  info.newerSchema = true;
  info.repairs.count = 3;
  Config d;
  CHECK(decodeConfig(base.data(), base.size(), d, &info) == DecodeResult::Ok);
  CHECK(info.schema == 1);
  CHECK_FALSE(info.newerSchema);
  CHECK(info.repairs.count == 0);
  std::vector<uint8_t> newer = base;
  newer[4] = 2;
  fixCrc(newer);
  CHECK(decodeConfig(newer.data(), newer.size(), d, &info) == DecodeResult::UnsupportedSchema);
  CHECK(info.schema == 2);
  std::vector<uint8_t> magic = base;
  magic[0] = 'X';
  CHECK(decodeConfig(magic.data(), magic.size(), d, &info) == DecodeResult::BadMagic);
  CHECK(info.schema == 0);
  CHECK(decodeConfig(nullptr, 0, d, &info) == DecodeResult::TooShort);

  // loadConfigBlobs: base, then the ext records.
  const std::vector<uint8_t> ext = encodeExt(fullConfigExt());
  StoredBlobs b;
  b.base = base.data();
  b.baseLen = base.size();
  b.ext = ext.data();
  b.extLen = ext.size();
  LoadInfo li;
  Config out;
  CHECK(loadConfigBlobs(b, out, li));
  CHECK(li.base == DecodeResult::Ok);
  CHECK(li.ext == ExtResult::Ok);
  CHECK(li.extInfo.applied > 0);
  CHECK(li.decode.schema == 1);
  CHECK(li.repairs.mask == 0);
  CHECK(sameConfig(out, fullConfigExt()));
  // A damaged ext blob leaves the defaults of its keys.
  std::vector<uint8_t> badExt = ext;
  badExt[9] ^= 1;
  b.ext = badExt.data();
  CHECK(loadConfigBlobs(b, out, li));
  CHECK(li.ext == ExtResult::BadCrc);
  CHECK(sameConfig(out, fullConfig()));
  // Unknown records go to `keep`.
  const std::vector<uint8_t> unknown = extBlob({210, 0, 1, 9});
  b.ext = unknown.data();
  b.extLen = unknown.size();
  uint8_t keep[16];
  CHECK(loadConfigBlobs(b, out, li, keep, sizeof keep));
  CHECK(li.extInfo.unknown == 1);
  CHECK(li.extInfo.keepLen == 4);
  CHECK(keep[0] == 210);
  // An unusable base blob: false, defaults.
  std::vector<uint8_t> badBase = base;
  badBase[9] ^= 1;
  b.base = badBase.data();
  CHECK_FALSE(loadConfigBlobs(b, out, li));
  CHECK(li.base == DecodeResult::BadCrc);
  CHECK(li.ext == ExtResult::Absent);
  CHECK(sameConfig(out, Config{}));
}

TEST_CASE("config: a stored config that breaks only V1-V3 is still loaded") {
  Config ha;
  ha.mqtt.mode = MqttMode::MqttHa;
  strcpy(ha.mqtt.host, "b");
  ha.mqtt.germanDecimal = true;
  strcpy(ha.valves[0].name, "Bad 1");
  strcpy(ha.valves[1].name, "Bad.1");
  CHECK(validatePath(ha) == "mqtt.germanDecimal");
  const std::vector<uint8_t> blob = encode(ha);
  Config back;
  CHECK(decodeConfig(blob.data(), blob.size(), back) == DecodeResult::Ok);
  CHECK(sameConfig(back, ha));
  ha.mqtt.germanDecimal = false;
  CHECK(validatePath(ha) == "valves.2.name");
  // A 2.0.0 rule still drops it.
  ha.mqtt.separate = false;
  const std::vector<uint8_t> broken = encode(ha);
  CHECK(decodeConfig(broken.data(), broken.size(), back) == DecodeResult::Invalid);
  CHECK(sameConfig(back, Config{}));
}
