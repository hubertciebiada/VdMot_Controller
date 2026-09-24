// config: defaults, per-key setter (every key, both sides of every range,
// every type conversion), validation (every per-field and cross-field
// rule with its path), JSON export (golden), JSON patch reader (syntax,
// paths, secrets, fuzz), binary NVS encoding (round trip, every error),
// CRC-32.
#include <math.h>
#include <stdint.h>
#include <string.h>

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

bool sameConfig(const Config& a, const Config& b) {
  uint8_t ba[kConfigBlobMax], bb[kConfigBlobMax];
  const size_t na = encodeConfig(a, ba, sizeof ba);
  const size_t nb = encodeConfig(b, bb, sizeof bb);
  return na > 0 && na == nb && memcmp(ba, bb, na) == 0;
}

// Config with every field away from its default, still valid.
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
  REQUIRE(set(c, "mqtt.mode", I(2)) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.host", S("broker.lan")) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.port", I(8883)) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.user", S("mq")) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.password", S("mqpw")) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.keepAliveS", I(30)) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.publishIntervalS", I(120)) == SetResult::Ok);
  REQUIRE(set(c, "mqtt.minDelayS", I(7)) == SetResult::Ok);
  // Every MQTT flag away from its default.
  const char* flipToFalse[] = {"allTemps", "upTime",  "onChange", "retained", "plainText",
                               "diag",     "newDiag", "events",   "haDiscoveryOnConnect"};
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
  CHECK(c.schema == 1);
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

TEST_CASE("config: schema is read-only except for the current version") {
  Config c;
  CHECK(set(c, "schema", I(1)) == SetResult::Ok);
  CHECK(set(c, "schema", S("1")) == SetResult::Ok);
  CHECK(set(c, "schema", F(1.0)) == SetResult::Ok);
  CHECK(set(c, "schema", I(2)) == SetResult::ReadOnly);
  CHECK(set(c, "schema", I(0)) == SetResult::ReadOnly);
  CHECK(set(c, "schema", S("x")) == SetResult::ReadOnly);
  CHECK(set(c, "schema", B(true)) == SetResult::ReadOnly);
  CHECK(set(c, "schema.x", I(1)) == SetResult::UnknownKey);
  CHECK(c.schema == 1);
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
  CHECK(set(c, "station", S(" ab")) == SetResult::OutOfRange);
  CHECK(set(c, "station", S("ab ")) == SetResult::OutOfRange);
  CHECK(set(c, "station", S("a\tb")) == SetResult::OutOfRange);
  CHECK(set(c, "station", S("a\xc3\xa4")) == SetResult::OutOfRange);
  CHECK(set(c, "station", SL("ab\0c", 4)) == SetResult::OutOfRange);
  CHECK(set(c, "station", I(5)) == SetResult::WrongType);
  CHECK(set(c, "station", B(true)) == SetResult::WrongType);
  CHECK(std::string(c.station) == "12345678901234567890");
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
  CHECK(set(c, "valves.2.name", S(" x")) == SetResult::OutOfRange);
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
    c.schema = 2;
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
      "{\"schema\":1,\"station\":\"VdMot\","
      "\"net\":{\"iface\":0,\"dhcp\":true,\"ip\":\"0.0.0.0\",\"mask\":\"0.0.0.0\","
      "\"gateway\":\"0.0.0.0\",\"dns\":\"0.0.0.0\",\"ssid\":\"\",\"wifiPasswordSet\":false,"
      "\"reconnectTimeoutMin\":5},"
      "\"time\":{\"ntpServer\":\"pool.ntp.org\",\"tzName\":\"Europe/Berlin\","
      "\"tzPosix\":\"CET-1CEST,M3.5.0,M10.5.0/3\"},"
      "\"syslog\":{\"level\":0,\"server\":\"0.0.0.0\",\"port\":514},"
      "\"web\":{\"user\":\"\",\"passwordSet\":false,\"protectRead\":false},"
      "\"mqtt\":{\"mode\":0,\"host\":\"\",\"port\":1883,\"user\":\"\",\"passwordSet\":false,"
      "\"keepAliveS\":60,\"publishIntervalS\":10,\"minDelayS\":5,\"separate\":true,"
      "\"allTemps\":true,\"pathAsRoot\":false,\"upTime\":true,\"onChange\":true,"
      "\"retained\":true,\"plainText\":true,\"diag\":true,\"germanDecimal\":false,"
      "\"newDiag\":true,\"events\":true,\"haDiscoveryOnConnect\":true},\"valves\":[";
  for (int i = 0; i < 12; ++i) {
    expected += std::string(i ? "," : "") + "{\"name\":\"\",\"active\":false}";
  }
  expected += "],\"temps\":[";
  for (int i = 0; i < 34; ++i) {
    expected += std::string(i ? "," : "") +
                "{\"name\":\"\",\"active\":false,\"offset\":0.0,\"id\":\"\"}";
  }
  expected += "],\"volts\":[";
  for (int i = 0; i < 8; ++i) {
    expected += std::string(i ? "," : "") +
                "{\"name\":\"\",\"active\":false,\"offset\":0,\"factor\":1,\"unit\":\"\","
                "\"id\":\"\"}";
  }
  expected += "],\"calib\":{\"dayMask\":9,\"hour\":0,\"minute\":0},\"persistLog\":true}";
  CHECK(exportJson(Config{}) == expected);
}

TEST_CASE("config: JSON export of set values") {
  const Config c = fullConfig();
  const std::string j = exportJson(c);
  CHECK(j.find("\"station\":\"Heizung OG\"") != std::string::npos);
  CHECK(j.find("\"iface\":2,\"dhcp\":false,\"ip\":\"192.168.1.50\",\"mask\":\"255.255.255.0\","
               "\"gateway\":\"192.168.1.1\",\"dns\":\"8.8.8.8\",\"ssid\":\"My Wifi\","
               "\"wifiPasswordSet\":true,\"reconnectTimeoutMin\":17") != std::string::npos);
  CHECK(j.find("\"web\":{\"user\":\"admin\",\"passwordSet\":true,\"protectRead\":true}") !=
        std::string::npos);
  CHECK(j.find("\"mode\":2,\"host\":\"broker.lan\",\"port\":8883,\"user\":\"mq\","
               "\"passwordSet\":true,\"keepAliveS\":30,\"publishIntervalS\":120,\"minDelayS\":7,"
               "\"separate\":true,\"allTemps\":false,\"pathAsRoot\":true,\"upTime\":false,"
               "\"onChange\":false,\"retained\":false,\"plainText\":false,\"diag\":false,"
               "\"germanDecimal\":true,\"newDiag\":false,\"events\":false,"
               "\"haDiscoveryOnConnect\":false}") != std::string::npos);
  CHECK(j.find("{\"name\":\"t1\",\"active\":true,\"offset\":-1.5,"
               "\"id\":\"28-84-37-94-97-ff-03-23\"}") !=
        std::string::npos);
  CHECK(j.find("{\"name\":\"\",\"active\":false,\"offset\":10.0,"
               "\"id\":\"28-aa-bb-cc-dd-ee-01-67\"}]") !=
        std::string::npos);
  CHECK(j.find("{\"name\":\"bat\",\"active\":true,\"offset\":-0.25,\"factor\":0.01,\"unit\":\"V\","
               "\"id\":\"26-11-22-33-44-55-66-29\"}]") != std::string::npos);
  CHECK(j.find("\"calib\":{\"dayMask\":127,\"hour\":23,\"minute\":59},\"persistLog\":false}") !=
        std::string::npos);
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
  // (wifi ssid without password).
  std::string path;
  CHECK(patch(c, j, &path) == PatchResult::Invalid);
  CHECK(path == "net.wifiPassword");

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
  CHECK(patch(c, "{\"schema\":2}", &path) == PatchResult::ReadOnly);
  CHECK(path == "schema");
  CHECK(patch(c, "{\"schema\":1}", &path) == PatchResult::Ok);
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
  // Control characters and non-ASCII decode fine but fail the field rules.
  const char* rejected[] = {"\\n", "\\r", "\\t", "\\b", "\\f", "\\u0000", "\\u00e4",
                            "\\u20ac", "\\ud83d\\ude00", "\xc3\xa4"};
  for (const char* r : rejected) {
    CAPTURE(r);
    CHECK(patch(c, std::string("{\"web\":{\"user\":\"a") + r + "\"}}", &path) ==
          PatchResult::OutOfRange);
    CHECK(path == "web.user");
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
