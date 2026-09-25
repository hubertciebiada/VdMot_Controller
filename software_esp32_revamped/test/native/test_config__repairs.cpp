// config: sanitizeConfig (every per-field reset kind, every cross-field
// repair alone with its bit, path and item mask, the V2/V3 clash
// resolution, fuzzed configs end valid), the repairing decodeConfig (newer
// schema prefix) and loadConfigBlobs (repairs after the ext records).
#include <math.h>
#include <stdint.h>
#include <string.h>

#include <functional>
#include <random>
#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/config.h"

using namespace vdm;

namespace {

const char* const kIdA = "28-84-37-94-97-ff-03-23";
const char* const kIdB = "28-aa-bb-cc-dd-ee-01-67";

OneWireId oid(const char* s) {
  OneWireId v;
  REQUIRE(parseOneWireId(s, strlen(s), v));
  return v;
}

std::string validatePath(const Config& c) {
  char path[64];
  return validateConfig(c, path, sizeof path) ? std::string("OK") : std::string(path);
}

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

void fixCrc(std::vector<uint8_t>& b) {
  const uint32_t crc = crc32(b.data(), b.size() - 4);
  for (int i = 0; i < 4; ++i) b[b.size() - 4 + i] = static_cast<uint8_t>(crc >> (8 * i));
}

std::vector<uint8_t> encode(const Config& c) {
  std::vector<uint8_t> b(kConfigBlobMax);
  b.resize(encodeConfig(c, b.data(), b.size()));
  REQUIRE_FALSE(b.empty());
  return b;
}

std::vector<uint8_t> encodeExt(const Config& c) {
  std::vector<uint8_t> b(kConfigExtBlobMax);
  b.resize(encodeConfigExt(c, b.data(), b.size()));
  REQUIRE_FALSE(b.empty());
  return b;
}

// One repair: the config after it, the mask, the count, the first path.
struct Outcome {
  uint32_t mask;
  uint16_t count;
  std::string first;
  Repairs r;
};
Outcome repair(Config& c) {
  Outcome o{};
  o.mask = sanitizeConfig(c, &o.r);
  o.count = o.r.count;
  o.first = o.r.first;
  CHECK(o.mask == o.r.mask);
  CHECK(validatePath(c) == "OK");
  return o;
}

}  // namespace

TEST_CASE("sanitize: a valid config is left alone, out may be null") {
  Config c;
  Repairs r;
  r.mask = 7;
  r.count = 3;
  r.valveNames = 1;
  r.tempIds = 1;
  strcpy(r.first, "x");
  CHECK(sanitizeConfig(c, &r) == 0);
  CHECK(r.mask == 0);
  CHECK(r.count == 0);
  CHECK(r.valveNames == 0);
  CHECK(r.tempIds == 0);
  CHECK(r.first[0] == '\0');
  CHECK(sameConfig(c, Config{}));
  c.calib.hour = 30;
  CHECK(sanitizeConfig(c, nullptr) == kRepairField);
  CHECK(c.calib.hour == 0);
}

TEST_CASE("sanitize: every field kind outside its rule gets its default") {
  struct Case {
    const char* path;
    std::function<void(Config&)> breakIt;
    std::function<bool(const Config&)> isDefault;
  };
  const Case cases[] = {
      {"schema", [](Config& c) { c.schema = 1; },
       [](const Config& c) { return c.schema == kConfigJsonSchema; }},
      {"station", [](Config& c) { c.station[0] = '\0'; },
       [](const Config& c) { return std::string(c.station) == "VdMot"; }},
      {"net.iface", [](Config& c) { c.net.iface = static_cast<NetInterface>(3); },
       [](const Config& c) { return c.net.iface == NetInterface::Auto; }},
      {"net.mask", [](Config& c) { c.net.mask = 0x00FF00FFu; },
       [](const Config& c) { return c.net.mask == 0; }},
      {"net.wifiPassword", [](Config& c) { strcpy(c.net.wifiPassword, "a\x01"); },
       [](const Config& c) { return c.net.wifiPassword[0] == '\0'; }},
      {"net.reconnectTimeoutMin", [](Config& c) { c.net.reconnectTimeoutMin = 241; },
       [](const Config& c) { return c.net.reconnectTimeoutMin == 5; }},
      {"time.ntpServer", [](Config& c) { strcpy(c.time.ntpServer, "a b"); },
       [](const Config& c) { return std::string(c.time.ntpServer) == "pool.ntp.org"; }},
      {"time.tzPosix", [](Config& c) { c.time.tzPosix[0] = '\0'; },
       [](const Config& c) { return std::string(c.time.tzPosix) == "CET-1CEST,M3.5.0,M10.5.0/3"; }},
      {"syslog.port", [](Config& c) { c.syslog.port = 0; },
       [](const Config& c) { return c.syslog.port == 514; }},
      {"web.allowedHosts", [](Config& c) { strcpy(c.web.allowedHosts, "a,,b"); },
       [](const Config& c) { return c.web.allowedHosts[0] == '\0'; }},
      {"mqtt.keepAliveS", [](Config& c) { c.mqtt.keepAliveS = 4; },
       [](const Config& c) { return c.mqtt.keepAliveS == 60; }},
      {"mqtt.discoveryPrefix", [](Config& c) { c.mqtt.discoveryPrefix[0] = '\0'; },
       [](const Config& c) { return std::string(c.mqtt.discoveryPrefix) == "homeassistant"; }},
      {"valves.4.failsafePct", [](Config& c) { c.valves[3].failsafePct = 101; },
       [](const Config& c) { return c.valves[3].failsafePct == 50; }},
      {"valves.12.topic", [](Config& c) { strcpy(c.valves[11].topic, "a b"); },
       [](const Config& c) { return c.valves[11].topic[0] == '\0'; }},
      {"temps.34.offset", [](Config& c) { c.temps[33].offset = -101; },
       [](const Config& c) { return c.temps[33].offset == 0; }},
      {"volts.2.offset", [](Config& c) { c.volts[1].offset = NAN; },
       [](const Config& c) { return c.volts[1].offset == 0.0f; }},
      {"volts.8.factor", [](Config& c) { c.volts[7].factor = 0.0f; },
       [](const Config& c) { return c.volts[7].factor == 1.0f; }},
      {"calib.minute", [](Config& c) { c.calib.minute = 60; },
       [](const Config& c) { return c.calib.minute == 0; }},
      {"failsafe.timeoutMin", [](Config& c) { c.failsafe.timeoutMin = 4; },
       [](const Config& c) { return c.failsafe.timeoutMin == 60; }},
  };
  for (const Case& k : cases) {
    CAPTURE(k.path);
    Config c;
    k.breakIt(c);
    const Outcome o = repair(c);
    CHECK(o.mask == kRepairField);
    CHECK(o.count == 1);
    CHECK(o.first == k.path);
    CHECK(k.isDefault(c));
  }
  // A string without a NUL inside its array is reset as a whole.
  Config c;
  memset(c.station, 'A', sizeof c.station);
  const Outcome o = repair(c);
  CHECK(o.first == "station");
  CHECK(std::string(c.station) == "VdMot");
  // Several fields: every one counted, the first path in table order.
  Config d;
  d.calib.hour = 24;
  d.mqtt.port = 0;
  d.temps[2].offset = 500;
  const Outcome od = repair(d);
  CHECK(od.mask == kRepairField);
  CHECK(od.count == 3);
  CHECK(od.first == "mqtt.port");
  CHECK(d.mqtt.port == 1883);
}

TEST_CASE("sanitize: each cross-field repair alone sets exactly its bit") {
  struct Case {
    uint32_t bit;
    const char* first;
    std::function<void(Config&)> breakIt;
    std::function<bool(const Config&)> repaired;
  };
  const Case cases[] = {
      {kRepairStaticIp, "net.dhcp",
       [](Config& c) {
         c.net.dhcp = false;
         c.net.ip = 1;
         c.net.mask = 0x00FFFFFFu;
         c.net.gateway = 0;
       },
       [](const Config& c) { return c.net.dhcp && c.net.ip == 1; }},
      {kRepairWifiPassword, "net.wifiPassword",
       [](Config& c) {
         strcpy(c.net.ssid, "w");
         strcpy(c.net.wifiPassword, "1234567");
       },
       [](const Config& c) { return c.net.ssid[0] == '\0' && c.net.wifiPassword[0] == '\0'; }},
      {kRepairWifiIface, "net.iface", [](Config& c) { c.net.iface = NetInterface::Wifi; },
       [](const Config& c) { return c.net.iface == NetInterface::Auto; }},
      {kRepairSyslog, "syslog.level", [](Config& c) { c.syslog.level = 1; },
       [](const Config& c) { return c.syslog.level == 0; }},
      {kRepairWebNoPassword, "web.password", [](Config& c) { strcpy(c.web.user, "u"); },
       [](const Config& c) { return c.web.user[0] == '\0' && c.web.password[0] == '\0'; }},
      {kRepairWebNoUser, "web.user", [](Config& c) { strcpy(c.web.password, "p"); },
       [](const Config& c) { return c.web.user[0] == '\0' && c.web.password[0] == '\0'; }},
      {kRepairMqttHost, "mqtt.mode", [](Config& c) { c.mqtt.mode = MqttMode::Mqtt; },
       [](const Config& c) { return c.mqtt.mode == MqttMode::Off; }},
      {kRepairMinDelay, "mqtt.minDelayS",
       [](Config& c) {
         c.mqtt.publishIntervalS = 20;
         c.mqtt.minDelayS = 21;
       },
       [](const Config& c) { return c.mqtt.minDelayS == 20; }},
      {kRepairHaSeparate, "mqtt.mode",
       [](Config& c) {
         c.mqtt.mode = MqttMode::MqttHa;
         strcpy(c.mqtt.host, "b");
         c.mqtt.separate = false;
       },
       [](const Config& c) { return c.mqtt.mode == MqttMode::Mqtt; }},
      {kRepairHaDecimal, "mqtt.germanDecimal",
       [](Config& c) {
         c.mqtt.mode = MqttMode::MqttHa;
         strcpy(c.mqtt.host, "b");
         c.mqtt.germanDecimal = true;
       },
       [](const Config& c) { return !c.mqtt.germanDecimal && c.mqtt.mode == MqttMode::MqttHa; }},
      {kRepairValveNames, "valves.3.name",
       [](Config& c) {
         strcpy(c.valves[0].name, "Dom");
         strcpy(c.valves[2].name, "Dom");
       },
       [](const Config& c) { return c.valves[2].name[0] == '\0' && c.valves[0].name[0] == 'D'; }},
      {kRepairSlotIds, "temps.2.id",
       [](Config& c) {
         c.temps[0].id = oid(kIdA);
         c.temps[1].id = oid(kIdA);
       },
       [](const Config& c) { return isZero(c.temps[1].id) && !isZero(c.temps[0].id); }},
      {kRepairSlotActive, "volts.8.active", [](Config& c) { c.volts[7].active = true; },
       [](const Config& c) { return !c.volts[7].active; }},
      {kRepairTopics, "valves.2.topic",
       [](Config& c) {
         strcpy(c.valves[0].name, "Bad");
         strcpy(c.valves[1].topic, "Bad");
       },
       [](const Config& c) { return c.valves[1].topic[0] == '\0'; }},
      {kRepairHaIds, "valves.2.name",
       [](Config& c) {
         strcpy(c.valves[0].name, "Bad 1");
         strcpy(c.valves[1].name, "Bad.1");
       },
       [](const Config& c) { return c.valves[1].name[0] == '\0'; }},
  };
  for (const Case& k : cases) {
    CAPTURE(k.first);
    Config c;
    k.breakIt(c);
    const Outcome o = repair(c);
    CHECK(o.mask == k.bit);
    CHECK(o.count == 1);
    CHECK(o.first == k.first);
    CHECK(k.repaired(c));
  }
}

TEST_CASE("sanitize: the cross-field rules keep what is valid (boundaries)") {
  Config c;
  c.net.dhcp = false;
  c.net.ip = 1;
  c.net.mask = 0x00FFFFFFu;
  c.net.gateway = 2;
  strcpy(c.net.ssid, "w");
  strcpy(c.net.wifiPassword, "12345678");
  c.net.iface = NetInterface::Wifi;
  c.syslog.level = 3;
  c.syslog.server = 9;
  strcpy(c.web.user, "u");
  strcpy(c.web.password, "p");
  c.mqtt.mode = MqttMode::MqttHa;
  strcpy(c.mqtt.host, "b");
  c.mqtt.publishIntervalS = 20;
  c.mqtt.minDelayS = 20;
  c.temps[0].id = oid(kIdA);
  c.temps[0].active = true;
  c.temps[1].id = oid(kIdB);
  c.temps[1].active = true;
  const Config before = c;
  const Outcome o = repair(c);
  CHECK(o.mask == 0);
  CHECK(sameConfig(c, before));
  // An open network (no password) is kept.
  c.net.wifiPassword[0] = '\0';
  CHECK(repair(c).mask == 0);
  CHECK(std::string(c.net.ssid) == "w");
  // Every part of an incomplete static address counts.
  for (int part = 0; part < 3; ++part) {
    CAPTURE(part);
    Config s = before;
    (part == 0 ? s.net.ip : part == 1 ? s.net.mask : s.net.gateway) = 0;
    CHECK(repair(s).mask == kRepairStaticIp);
    CHECK(s.net.dhcp);
  }
}

TEST_CASE("sanitize: V2 and V3 clashes clear the later item, else the earlier one") {
  {
    // V2: the later valve's name equals an earlier override.
    Config c;
    strcpy(c.valves[0].topic, "x");
    strcpy(c.valves[1].name, "x");
    const Outcome o = repair(c);
    CHECK(o.mask == kRepairValveNames);
    CHECK(o.first == "valves.2.name");
    CHECK(o.r.valveNames == 2);
    CHECK(o.r.valveTopics == 0);
    CHECK(std::string(c.valves[0].topic) == "x");
  }
  {
    // V2: the unnamed later valve uses its number, an earlier override takes it.
    Config c;
    strcpy(c.valves[0].topic, "2");
    const Outcome o = repair(c);
    CHECK(o.mask == kRepairTopics);
    CHECK(o.first == "valves.1.topic");
    CHECK(o.r.valveTopics == 1);
    CHECK(o.r.valveNames == 0);
    CHECK(c.valves[0].topic[0] == '\0');
  }
  {
    // V3 on temp slots: the later name goes; inactive slots do not count.
    Config c;
    c.temps[0].id = oid(kIdA);
    c.temps[0].active = true;
    strcpy(c.temps[0].name, "a b");
    c.temps[3].id = oid(kIdB);
    strcpy(c.temps[3].name, "a.b");
    CHECK(repair(c).mask == 0);
    c.temps[3].active = true;
    const Outcome o = repair(c);
    CHECK(o.mask == kRepairHaIds);
    CHECK(o.first == "temps.4.name");
    CHECK(o.r.valveNames == 0);
    CHECK(c.temps[3].name[0] == '\0');
    CHECK(std::string(c.temps[0].name) == "a b");
  }
  {
    // V3: the unnamed later slot keeps its number, the earlier slot named
    // like it loses its name.
    Config c;
    c.temps[0].id = oid(kIdA);
    c.temps[0].active = true;
    strcpy(c.temps[0].name, "2");
    c.temps[1].id = oid(kIdB);
    c.temps[1].active = true;
    const Outcome o = repair(c);
    CHECK(o.mask == kRepairHaIds);
    CHECK(o.first == "temps.1.name");
    CHECK(c.temps[0].name[0] == '\0');
  }
  {
    // V3 on volt slots: the later override goes before its name.
    Config c;
    c.volts[0].id = oid(kIdA);
    c.volts[0].active = true;
    strcpy(c.volts[0].name, "bat");
    c.volts[5].id = oid(kIdB);
    c.volts[5].active = true;
    strcpy(c.volts[5].name, "other");
    strcpy(c.volts[5].topic, "bat");
    const Outcome o = repair(c);
    CHECK(o.mask == kRepairHaIds);
    CHECK(o.first == "volts.6.topic");
    CHECK(c.volts[5].topic[0] == '\0');
    CHECK(std::string(c.volts[5].name) == "other");
  }
  {
    // V3 on valves with an override: the later override goes.
    Config c;
    strcpy(c.valves[0].name, "Bad 1");
    strcpy(c.valves[4].topic, "Bad.1");
    const Outcome o = repair(c);
    CHECK(o.mask == kRepairHaIds);
    CHECK(o.first == "valves.5.topic");
    CHECK(o.r.valveTopics == (1u << 4));
  }
}

TEST_CASE("sanitize: clearing a name can make a new clash, repeated until stable") {
  // Valve 5 is named "3": fine while valve 3 has a name. Valve 3 is a
  // duplicate of valve 1 and loses its name, then valve 5 clashes with the
  // number of the unnamed valve 3.
  Config c;
  strcpy(c.valves[0].name, "Dom");
  strcpy(c.valves[2].name, "Dom");
  strcpy(c.valves[4].name, "3");
  const Outcome o = repair(c);
  CHECK(o.mask == kRepairValveNames);
  CHECK(o.count == 2);
  CHECK(o.first == "valves.3.name");
  CHECK(o.r.valveNames == ((1u << 2) | (1u << 4)));
  CHECK(std::string(c.valves[0].name) == "Dom");
}

TEST_CASE("sanitize: duplicate slot ids, the active flag follows the id") {
  Config c;
  c.temps[0].id = oid(kIdA);
  c.temps[5].id = oid(kIdA);
  c.temps[5].active = true;
  c.temps[33].id = oid(kIdA);
  c.volts[0].id = oid(kIdB);
  c.volts[7].id = oid(kIdB);
  const Outcome o = repair(c);
  CHECK(o.mask == (kRepairSlotIds | kRepairSlotActive));
  CHECK(o.count == 4);
  CHECK(o.first == "temps.6.id");
  CHECK(o.r.tempIds == ((1ull << 5) | (1ull << 33)));
  CHECK(o.r.tempActive == (1ull << 5));
  CHECK(o.r.voltIds == (1u << 7));
  CHECK(o.r.voltActive == 0);
  CHECK(!isZero(c.temps[0].id));
  CHECK(!c.temps[5].active);
  Config v;
  v.volts[2].active = true;
  const Outcome ov = repair(v);
  CHECK(ov.r.voltActive == (1u << 2));
  CHECK(ov.r.tempActive == 0);
}

TEST_CASE("sanitize: fuzzed configs always end valid and stay put on a second pass" *
          doctest::test_suite("fuzz")) {
  std::mt19937 rng(20260925);
  auto pick = [&](const std::vector<const char*>& v) { return v[rng() % v.size()]; };
  const std::vector<const char*> strs = {"",  "a",   "a b", "2",     "3",   "Dom", "Dom",
                                         "/x", "x/y", "a+b", "\x01", "ha/x", "a,,b", "Bad.1",
                                         "Bad 1"};
  const std::vector<const char*> ids = {kIdA, kIdB, ""};
  for (int iter = 0; iter < 4000; ++iter) {
    Config c;
    const int edits = 1 + static_cast<int>(rng() % 12);
    for (int k = 0; k < edits; ++k) {
      const uint8_t v = static_cast<uint8_t>(rng() % kValveCount);
      const uint8_t t = static_cast<uint8_t>(rng() % kTempSlotCount);
      const uint8_t u = static_cast<uint8_t>(rng() % kVoltSlotCount);
      switch (rng() % 20) {
        case 0: strcpy(c.station, pick(strs)); break;
        case 1: c.net.dhcp = rng() % 2; c.net.ip = rng() % 3; break;
        case 2: strcpy(c.net.ssid, pick(strs)); strcpy(c.net.wifiPassword, pick(strs)); break;
        case 3: c.net.iface = static_cast<NetInterface>(rng() % 4); break;
        case 4: c.syslog.level = static_cast<uint8_t>(rng() % 5); c.syslog.server = rng() % 2; break;
        case 5: strcpy(c.web.user, pick(strs)); strcpy(c.web.password, pick(strs)); break;
        case 6: c.mqtt.mode = static_cast<MqttMode>(rng() % 3); strcpy(c.mqtt.host, pick(strs)); break;
        case 7: c.mqtt.minDelayS = static_cast<uint16_t>(rng() % 30); c.mqtt.publishIntervalS = static_cast<uint16_t>(rng() % 30); break;
        case 8: c.mqtt.separate = rng() % 2; c.mqtt.germanDecimal = rng() % 2; break;
        case 9: strcpy(c.valves[v].name, pick(strs)); break;
        case 10: strcpy(c.valves[v].topic, pick(strs)); break;
        case 11: c.valves[v].failsafePct = static_cast<uint8_t>(rng()); break;
        case 12: strcpy(c.temps[t].name, pick(strs)); c.temps[t].active = rng() % 2; break;
        case 13: {
          const char* id = pick(ids);
          c.temps[t].id = id[0] ? oid(id) : OneWireId{};
          break;
        }
        case 14: strcpy(c.temps[t].topic, pick(strs)); break;
        case 15: strcpy(c.volts[u].name, pick(strs)); c.volts[u].active = rng() % 2; break;
        case 16: {
          const char* id = pick(ids);
          c.volts[u].id = id[0] ? oid(id) : OneWireId{};
          break;
        }
        case 17: strcpy(c.volts[u].topic, pick(strs)); c.volts[u].factor = static_cast<float>(rng() % 3); break;
        case 18: strcpy(c.mqtt.discoveryPrefix, pick(strs)); strcpy(c.web.allowedHosts, pick(strs)); break;
        default: c.failsafe.timeoutMin = static_cast<uint16_t>(rng() % 2000); break;
      }
    }
    const bool valid = validateConfig(c, nullptr, 0);
    Repairs r;
    const uint32_t mask = sanitizeConfig(c, &r);
    CHECK(validatePath(c) == "OK");
    CHECK((mask == 0) == valid);
    CHECK((r.count == 0) == (mask == 0));
    CHECK((r.first[0] == '\0') == (mask == 0));
    const Config once = c;
    CHECK(sanitizeConfig(c, nullptr) == 0);
    CHECK(sameConfig(c, once));
  }
}

TEST_CASE("decode: a newer base schema is read by its schema-1 prefix (C-5)") {
  const Config c = [] {
    Config x;
    strcpy(x.station, "Newer");
    x.calib.hour = 7;
    return x;
  }();
  std::vector<uint8_t> b = encode(c);
  // Schema 2 with 5 more payload bytes (a newer firmware's fields).
  b[4] = 2;
  b.insert(b.end() - 4, {1, 2, 3, 4, 5});
  const size_t payload = b.size() - 12;
  b[6] = static_cast<uint8_t>(payload);
  b[7] = static_cast<uint8_t>(payload >> 8);
  fixCrc(b);
  Config out;
  DecodeInfo info;
  CHECK(decodeConfig(b.data(), b.size(), out, &info) == DecodeResult::Ok);
  CHECK(info.schema == 2);
  CHECK(info.newerSchema);
  CHECK(info.repairs.mask == 0);
  CHECK(sameConfig(out, c));
  // Schema 2 exactly as long as schema 1: still Ok.
  std::vector<uint8_t> same = encode(c);
  same[4] = 2;
  fixCrc(same);
  CHECK(decodeConfig(same.data(), same.size(), out, &info) == DecodeResult::Ok);
  CHECK(info.newerSchema);
  // Schema 2 shorter than the schema-1 fields: Invalid, defaults, not newer.
  std::vector<uint8_t> shortB = encode(c);
  shortB.erase(shortB.end() - 5);
  shortB[4] = 2;
  const size_t pl = shortB.size() - 12;
  shortB[6] = static_cast<uint8_t>(pl);
  shortB[7] = static_cast<uint8_t>(pl >> 8);
  fixCrc(shortB);
  CHECK(decodeConfig(shortB.data(), shortB.size(), out, &info) == DecodeResult::Invalid);
  CHECK_FALSE(info.newerSchema);
  CHECK(sameConfig(out, Config{}));
  // Schema 1 with the same extra bytes: Invalid (the length rule of 2.0.0).
  b[4] = 1;
  fixCrc(b);
  CHECK(decodeConfig(b.data(), b.size(), out, &info) == DecodeResult::Invalid);
  CHECK_FALSE(info.newerSchema);
  CHECK(decodeConfig(b.data(), b.size(), out) == DecodeResult::Invalid);
}

TEST_CASE("decode: value damage is repaired and reported, the blob itself stays Ok") {
  Config c;
  c.net.dhcp = false;  // static without an address: repaired to DHCP
  strcpy(c.web.user, "admin");
  const std::vector<uint8_t> b = encode(c);
  Config out;
  DecodeInfo info;
  CHECK(decodeConfig(b.data(), b.size(), out, &info) == DecodeResult::Ok);
  CHECK(info.repairs.mask == (kRepairStaticIp | kRepairWebNoPassword));
  CHECK(info.repairs.count == 2);
  CHECK(std::string(info.repairs.first) == "net.dhcp");
  CHECK(out.net.dhcp);
  CHECK(out.web.user[0] == '\0');
}

TEST_CASE("loadConfigBlobs: ext records that break V2 are repaired after the ext (C-10)") {
  Config base;
  strcpy(base.valves[0].name, "Bad");
  Config ext = base;
  strcpy(ext.valves[3].topic, "Bad");
  const std::vector<uint8_t> b = encode(base);
  const std::vector<uint8_t> x = encodeExt(ext);
  StoredBlobs blobs;
  blobs.base = b.data();
  blobs.baseLen = b.size();
  blobs.ext = x.data();
  blobs.extLen = x.size();
  Config out;
  LoadInfo li;
  CHECK(loadConfigBlobs(blobs, out, li));
  CHECK(li.decode.repairs.mask == 0);
  CHECK(li.repairs.mask == kRepairTopics);
  CHECK(li.repairs.valveTopics == (1u << 3));
  CHECK(std::string(li.repairs.first) == "valves.4.topic");
  CHECK(out.valves[3].topic[0] == '\0');
  CHECK(std::string(out.valves[0].name) == "Bad");
  CHECK(validatePath(out) == "OK");
  // Base repairs are in `decode`, the final pass finds nothing more.
  Config bad = base;
  bad.calib.hour = 24;
  const std::vector<uint8_t> bb = encode(bad);
  blobs.base = bb.data();
  blobs.baseLen = bb.size();
  blobs.ext = nullptr;
  blobs.extLen = 0;
  CHECK(loadConfigBlobs(blobs, out, li));
  CHECK(li.ext == ExtResult::Absent);
  CHECK(li.decode.repairs.mask == kRepairField);
  CHECK(li.repairs.mask == 0);
  CHECK(li.repairs.first[0] == '\0');
}
