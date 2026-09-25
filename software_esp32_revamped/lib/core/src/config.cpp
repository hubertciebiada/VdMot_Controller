#include "vdm/config.h"

#include <algorithm>
#include <math.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "vdm/json_writer.h"

namespace vdm {

namespace {

// ---------------------------------------------------------------- schema table
//
// One table drives the key-path setter, validation, JSON export and the NVS
// encodings, so they can never disagree about a field. Table order is the
// JSON order; the fields without an ext tag in table order are the `cfg`
// blob (exactly the 2.0.0 layout), the others go to `cfgx` by tag.

enum class Kind : uint8_t {
  Bool,        // bool, stored 0/1
  U8,          // uint8_t or uint8_t-based enum, [min,max]
  U16,         // uint16_t, [min,max]
  Str,         // char[cap], length [min,max], rule
  Secret,      // like Str, write-only
  Ip,          // uint32_t legacy IPv4 layout, any value
  Mask,        // uint32_t legacy IPv4 layout, contiguous
  Tenths,      // int16_t tenths, [min,max]
  Float,       // float, finite, [min,max]
  Id,          // OneWireId, zero = empty
  PctHold,     // uint8_t, [min,max] or kFailsafeHold
  OffOrRange,  // uint16_t, 0 or [min,max]
};

enum class Rule : uint8_t {
  None,
  Printable,     // isPrintableText: ASCII 0x20..0x7E and UTF-8 (SSIDs, secrets, like legacy)
  NoSpace,       // 0x21..0x7E
  NoColon,       // Printable without ':' (HTTP Basic user)
  SafeName,      // isSafeName()
  Host,          // isHostName() or dotted IPv4
  HostList,      // "" or 1..4 isHostName() entries separated by ',', spaces around them ignored
  ClientId,      // [A-Za-z0-9._-]
  TopicPath,     // levels of [A-Za-z0-9_-] separated by single '/'
  TopicSegment,  // Printable without ' ', '+', '#'; '/' only between two non-empty parts
};

// Member order without padding holes: 20 bytes per table entry on the ESP32.
struct Field {
  const char* name;
  int32_t min;      // numbers: value range; strings: length range
  int32_t max;
  uint16_t offset;  // inside the group struct
  Kind kind;
  uint8_t cap;      // Str/Secret: array size incl. NUL
  Rule rule;
  bool nonZero;     // Float: 0 is not allowed
  uint8_t ext;      // 0: `cfg` blob; else the `cfgx` record tag (never reused)
};

// Members a kind does not use stay zero.
constexpr Field makeField(const char* n, Kind k, size_t off) {
  Field f{};
  f.name = n;
  f.kind = k;
  f.offset = static_cast<uint16_t>(off);
  return f;
}
constexpr Field boolField(const char* n, size_t off) { return makeField(n, Kind::Bool, off); }
constexpr Field plainField(const char* n, Kind k, size_t off) { return makeField(n, k, off); }
constexpr Field intField(const char* n, Kind k, size_t off, int32_t mn, int32_t mx) {
  Field f = makeField(n, k, off);
  f.min = mn;
  f.max = mx;
  return f;
}
constexpr Field strField(const char* n, Kind k, size_t off, size_t cap, int32_t mn, int32_t mx,
                         Rule r) {
  Field f = intField(n, k, off, mn, mx);
  f.cap = static_cast<uint8_t>(cap);
  f.rule = r;
  return f;
}
constexpr Field floatField(const char* n, size_t off, int32_t mn, int32_t mx, bool nonZero) {
  Field f = intField(n, Kind::Float, off, mn, mx);
  f.nonZero = nonZero;
  return f;
}
// A field stored in the `cfgx` blob under `tag`.
constexpr Field extField(Field f, uint8_t tag) {
  f.ext = tag;
  return f;
}

constexpr int32_t kSecretMaxI = static_cast<int32_t>(kSecretMax);
constexpr int32_t kHostMaxI = static_cast<int32_t>(kHostMax);
constexpr int32_t kItemNameMaxI = static_cast<int32_t>(kItemNameMax);
// Longest string the patch reader keeps (PatchWalker).
constexpr size_t kPatchStrMax = 96;  // NOMUTATE: any size above the longest field is equivalent

constexpr Field kRootHead[] = {
    strField("station", Kind::Str, offsetof(Config, station), sizeof(Config::station), 1,
             static_cast<int32_t>(kStationNameMax), Rule::SafeName),
};

constexpr Field kNetFields[] = {
    intField("iface", Kind::U8, offsetof(NetConfig, iface), 0, 2),
    boolField("dhcp", offsetof(NetConfig, dhcp)),
    plainField("ip", Kind::Ip, offsetof(NetConfig, ip)),
    plainField("mask", Kind::Mask, offsetof(NetConfig, mask)),
    plainField("gateway", Kind::Ip, offsetof(NetConfig, gateway)),
    plainField("dns", Kind::Ip, offsetof(NetConfig, dns)),
    strField("ssid", Kind::Str, offsetof(NetConfig, ssid), sizeof(NetConfig::ssid), 0, 32,
             Rule::Printable),
    strField("wifiPassword", Kind::Secret, offsetof(NetConfig, wifiPassword),
             sizeof(NetConfig::wifiPassword), 0, 63, Rule::Printable),
    intField("reconnectTimeoutMin", Kind::U8, offsetof(NetConfig, reconnectTimeoutMin), 0, 240),
};

constexpr Field kTimeFields[] = {
    strField("ntpServer", Kind::Str, offsetof(TimeConfig, ntpServer), sizeof(TimeConfig::ntpServer),
             0, kHostMaxI, Rule::Host),
    strField("tzName", Kind::Str, offsetof(TimeConfig, tzName), sizeof(TimeConfig::tzName), 0,
             static_cast<int32_t>(kTzNameMax), Rule::Printable),
    strField("tzPosix", Kind::Str, offsetof(TimeConfig, tzPosix), sizeof(TimeConfig::tzPosix), 1,
             static_cast<int32_t>(kTzPosixMax), Rule::NoSpace),
};

constexpr Field kSyslogFields[] = {
    intField("level", Kind::U8, offsetof(SyslogConfig, level), 0, 3),
    plainField("server", Kind::Ip, offsetof(SyslogConfig, server)),
    intField("port", Kind::U16, offsetof(SyslogConfig, port), 1, 65535),
};

constexpr Field kWebFields[] = {
    strField("user", Kind::Str, offsetof(WebConfig, user), sizeof(WebConfig::user), 0, kSecretMaxI,
             Rule::NoColon),
    strField("password", Kind::Secret, offsetof(WebConfig, password), sizeof(WebConfig::password),
             0, kSecretMaxI, Rule::Printable),
    boolField("protectRead", offsetof(WebConfig, protectRead)),
    extField(strField("allowedHosts", Kind::Str, offsetof(WebConfig, allowedHosts),
                      sizeof(WebConfig::allowedHosts), 0, static_cast<int32_t>(kAllowedHostsMax),
                      Rule::HostList),
             1),
};

constexpr Field kMqttFields[] = {
    intField("mode", Kind::U8, offsetof(MqttConfig, mode), 0, 2),
    strField("host", Kind::Str, offsetof(MqttConfig, host), sizeof(MqttConfig::host), 0, kHostMaxI,
             Rule::Host),
    intField("port", Kind::U16, offsetof(MqttConfig, port), 1, 65535),
    strField("user", Kind::Str, offsetof(MqttConfig, user), sizeof(MqttConfig::user), 0,
             kSecretMaxI, Rule::Printable),
    strField("password", Kind::Secret, offsetof(MqttConfig, password), sizeof(MqttConfig::password),
             0, kSecretMaxI, Rule::Printable),
    intField("keepAliveS", Kind::U16, offsetof(MqttConfig, keepAliveS), 5, 300),
    intField("publishIntervalS", Kind::U16, offsetof(MqttConfig, publishIntervalS), 2, 3600),
    intField("minDelayS", Kind::U16, offsetof(MqttConfig, minDelayS), 0, 3600),
    boolField("separate", offsetof(MqttConfig, separate)),
    boolField("allTemps", offsetof(MqttConfig, allTemps)),
    boolField("pathAsRoot", offsetof(MqttConfig, pathAsRoot)),
    boolField("upTime", offsetof(MqttConfig, upTime)),
    boolField("onChange", offsetof(MqttConfig, onChange)),
    boolField("retained", offsetof(MqttConfig, retained)),
    boolField("plainText", offsetof(MqttConfig, plainText)),
    boolField("diag", offsetof(MqttConfig, diag)),
    boolField("germanDecimal", offsetof(MqttConfig, germanDecimal)),
    boolField("newDiag", offsetof(MqttConfig, newDiag)),
    boolField("events", offsetof(MqttConfig, events)),
    boolField("haDiscoveryOnConnect", offsetof(MqttConfig, haDiscoveryOnConnect)),
    extField(strField("rootTopic", Kind::Str, offsetof(MqttConfig, rootTopic),
                      sizeof(MqttConfig::rootTopic), 0, static_cast<int32_t>(kStationNameMax),
                      Rule::SafeName),
             2),
    extField(strField("clientId", Kind::Str, offsetof(MqttConfig, clientId),
                      sizeof(MqttConfig::clientId), 0, static_cast<int32_t>(kClientIdMax),
                      Rule::ClientId),
             3),
    extField(strField("discoveryPrefix", Kind::Str, offsetof(MqttConfig, discoveryPrefix),
                      sizeof(MqttConfig::discoveryPrefix), 1,
                      static_cast<int32_t>(kTopicPrefixMax), Rule::TopicPath),
             4),
};

constexpr Field kValveFields[] = {
    strField("name", Kind::Str, offsetof(ValveConfig, name), sizeof(ValveConfig::name), 0,
             kItemNameMaxI, Rule::SafeName),
    boolField("active", offsetof(ValveConfig, active)),
    extField(intField("failsafePct", Kind::PctHold, offsetof(ValveConfig, failsafePct), 0, 100), 6),
    extField(strField("topic", Kind::Str, offsetof(ValveConfig, topic), sizeof(ValveConfig::topic),
                      0, kItemNameMaxI, Rule::TopicSegment),
             7),
};

constexpr Field kTempFields[] = {
    strField("name", Kind::Str, offsetof(TempSlotConfig, name), sizeof(TempSlotConfig::name), 0,
             kItemNameMaxI, Rule::SafeName),
    boolField("active", offsetof(TempSlotConfig, active)),
    intField("offset", Kind::Tenths, offsetof(TempSlotConfig, offset), -100, 100),
    plainField("id", Kind::Id, offsetof(TempSlotConfig, id)),
    extField(strField("topic", Kind::Str, offsetof(TempSlotConfig, topic),
                      sizeof(TempSlotConfig::topic), 0, kItemNameMaxI, Rule::TopicSegment),
             8),
};

constexpr Field kVoltFields[] = {
    strField("name", Kind::Str, offsetof(VoltSlotConfig, name), sizeof(VoltSlotConfig::name), 0,
             kItemNameMaxI, Rule::SafeName),
    boolField("active", offsetof(VoltSlotConfig, active)),
    floatField("offset", offsetof(VoltSlotConfig, offset), -1000, 1000, false),
    floatField("factor", offsetof(VoltSlotConfig, factor), -1000, 1000, true),
    strField("unit", Kind::Str, offsetof(VoltSlotConfig, unit), sizeof(VoltSlotConfig::unit), 0,
             static_cast<int32_t>(kUnitMax), Rule::SafeName),
    plainField("id", Kind::Id, offsetof(VoltSlotConfig, id)),
    extField(strField("topic", Kind::Str, offsetof(VoltSlotConfig, topic),
                      sizeof(VoltSlotConfig::topic), 0, kItemNameMaxI, Rule::TopicSegment),
             9),
};

constexpr Field kCalibFields[] = {
    intField("dayMask", Kind::U8, offsetof(CalibScheduleConfig, dayMask), 0, 127),
    intField("hour", Kind::U8, offsetof(CalibScheduleConfig, hour), 0, 23),
    intField("minute", Kind::U8, offsetof(CalibScheduleConfig, minute), 0, 59),
};

constexpr Field kFailsafeFields[] = {
    extField(intField("timeoutMin", Kind::OffOrRange, offsetof(FailsafeConfig, timeoutMin),
                      kFailsafeTimeoutMinMin, kFailsafeTimeoutMaxMin),
             5),
};

constexpr Field kRootTail[] = {
    boolField("persistLog", offsetof(Config, persistLog)),
};

// String fields: max < cap (room for the NUL), cap within the scratch buffers
// (sized like the largest field, web.allowedHosts), and shorter than what the
// patch reader keeps (so a cut string is still too long). Checked at compile
// time for every table.
constexpr bool fits(const Field& f) {  // NOMUTATE: compile-time check
  return (f.kind != Kind::Str && f.kind != Kind::Secret) ||  // NOMUTATE: compile-time check
         (f.max < f.cap && f.cap <= sizeof(WebConfig::allowedHosts) &&  // NOMUTATE: compile-time check
          static_cast<size_t>(f.max) < kPatchStrMax);           // NOMUTATE: compile-time check
}
template <size_t N>
constexpr bool allFit(const Field (&a)[N]) {
  for (const Field& f : a) {     // NOMUTATE: compile-time check
    if (!fits(f)) return false;  // NOMUTATE: compile-time check
  }
  return true;  // NOMUTATE: compile-time check
}
static_assert(allFit(kRootHead) && allFit(kNetFields) &&          // NOMUTATE: compile-time check
                  allFit(kTimeFields) && allFit(kSyslogFields) &&  // NOMUTATE: compile-time check
                  allFit(kWebFields) && allFit(kMqttFields) &&     // NOMUTATE: compile-time check
                  allFit(kValveFields) && allFit(kTempFields) &&   // NOMUTATE: compile-time check
                  allFit(kVoltFields) && allFit(kCalibFields) &&   // NOMUTATE: compile-time check
                  allFit(kFailsafeFields) && allFit(kRootTail),    // NOMUTATE: compile-time check
              "string field limits");

struct Group {
  const char* name;  // nullptr: fields directly on Config
  const Field* fields;
  uint8_t fieldCount;
  uint16_t base;     // offsetof(Config, member)
  uint8_t count;     // array length, 0 = object
  uint16_t stride;   // element size (objects: their struct size, unused)
};

template <typename T, size_t N>
constexpr uint8_t countOf(const T (&)[N]) {
  return static_cast<uint8_t>(N);
}

// A root group (name nullptr) has no index: count 0 and 1 both give one element.
const Group kGroups[] = {
    {nullptr, kRootHead, countOf(kRootHead), 0, 0,  // NOMUTATE: root count 0 and 1 are equal
     sizeof(Config)},
    {"net", kNetFields, countOf(kNetFields), offsetof(Config, net), 0, sizeof(NetConfig)},
    {"time", kTimeFields, countOf(kTimeFields), offsetof(Config, time), 0, sizeof(TimeConfig)},
    {"syslog", kSyslogFields, countOf(kSyslogFields), offsetof(Config, syslog), 0,
     sizeof(SyslogConfig)},
    {"web", kWebFields, countOf(kWebFields), offsetof(Config, web), 0, sizeof(WebConfig)},
    {"mqtt", kMqttFields, countOf(kMqttFields), offsetof(Config, mqtt), 0, sizeof(MqttConfig)},
    {"valves", kValveFields, countOf(kValveFields), offsetof(Config, valves), kValveCount,
     sizeof(ValveConfig)},
    {"temps", kTempFields, countOf(kTempFields), offsetof(Config, temps), kTempSlotCount,
     sizeof(TempSlotConfig)},
    {"volts", kVoltFields, countOf(kVoltFields), offsetof(Config, volts), kVoltSlotCount,
     sizeof(VoltSlotConfig)},
    {"calib", kCalibFields, countOf(kCalibFields), offsetof(Config, calib), 0,
     sizeof(CalibScheduleConfig)},
    {"failsafe", kFailsafeFields, countOf(kFailsafeFields), offsetof(Config, failsafe), 0,
     sizeof(FailsafeConfig)},
    {nullptr, kRootTail, countOf(kRootTail), 0, 0,  // NOMUTATE: root count 0 and 1 are equal
     sizeof(Config)},
};

constexpr size_t kGroupCount = countOf(kGroups);
constexpr size_t kPathMax = 64;

uint8_t* fieldPtr(Config& c, const Group& g, uint8_t element, const Field& f) {
  return reinterpret_cast<uint8_t*>(&c) + g.base + static_cast<size_t>(element) * g.stride +
         f.offset;
}

const uint8_t* fieldPtr(const Config& c, const Group& g, uint8_t element, const Field& f) {
  return reinterpret_cast<const uint8_t*>(&c) + g.base + static_cast<size_t>(element) * g.stride +
         f.offset;
}

uint8_t elementCount(const Group& g) { return g.count ? g.count : 1; }

// ---------------------------------------------------------------- raw access

uint16_t loadU16(const uint8_t* p) {
  uint16_t v;
  memcpy(&v, p, sizeof v);
  return v;
}
uint32_t loadU32(const uint8_t* p) {
  uint32_t v;
  memcpy(&v, p, sizeof v);
  return v;
}
int16_t loadI16(const uint8_t* p) {
  int16_t v;
  memcpy(&v, p, sizeof v);
  return v;
}
float loadFloat(const uint8_t* p) {
  float v;
  memcpy(&v, p, sizeof v);
  return v;
}

// ---------------------------------------------------------------- field rules

bool isDigitsAndDots(const char* s, size_t len) {
  for (size_t i = 0; i < len; ++i) {
    if (!((s[i] >= '0' && s[i] <= '9') || s[i] == '.')) return false;
  }
  return true;
}

// Host names that look numeric must be a valid dotted IPv4 ("192.168.1.300"
// is a typo, not a host name).
bool hostValid(const char* s, size_t len) {
  if (len == 0) return true;
  if (isDigitsAndDots(s, len)) {
    uint32_t ip;
    return parseIpv4(s, len, ip);
  }
  return isHostName(s, kHostMax);
}

// "" or 1..4 host names separated by ','; spaces around an entry are ignored.
bool hostListValid(const char* s, size_t len) {
  size_t entries = 0;
  for (size_t pos = 0; pos < len;) {
    size_t end = pos;
    while (end < len && s[end] != ',') ++end;
    size_t b = pos, e = end;
    while (s[b] == ' ') ++b;  // s[end] is ',' or the NUL
    while (e > b && s[e - 1] == ' ') --e;
    char host[sizeof(WebConfig::allowedHosts)];
    memcpy(host, s + b, e - b);
    host[e - b] = '\0';
    if (++entries > 4 || !isHostName(host, kHostMax)) return false;
    if (end == len) return true;
    pos = end + 1;
    if (pos == len) return false;  // trailing ','
  }
  return true;
}

bool isIdChar(char c) {
  return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_' ||
         c == '-';
}

// '/' only between two non-empty parts (`s` ends with a NUL after len).
bool slashesInside(const char* s, size_t len) {
  if (len > 0 && (s[0] == '/' || s[len - 1] == '/')) return false;
  return strstr(s, "//") == nullptr;
}

// `s` holds exactly `len` chars, none of them NUL, followed by a NUL.
bool stringRuleOk(const Field& f, const char* s, size_t len) {
  if (len < static_cast<size_t>(f.min) || len > static_cast<size_t>(f.max)) return false;
  switch (f.rule) {
    case Rule::SafeName: return isSafeName(s, static_cast<size_t>(f.max), f.min == 0);
    case Rule::Host: return hostValid(s, len);
    case Rule::HostList: return hostListValid(s, len);
    case Rule::NoSpace:
      for (size_t i = 0; i < len; ++i) {
        if (s[i] <= 0x20 || s[i] > 0x7E) return false;
      }
      return true;
    case Rule::ClientId:
    case Rule::TopicPath: {
      const char extra = f.rule == Rule::ClientId ? '.' : '/';
      for (size_t i = 0; i < len; ++i) {
        if (!isIdChar(s[i]) && s[i] != extra) return false;
      }
      return slashesInside(s, len);
    }
    case Rule::TopicSegment:
      if (memchr(s, ' ', len) != nullptr || memchr(s, '+', len) != nullptr ||
          memchr(s, '#', len) != nullptr || !slashesInside(s, len)) {
        return false;
      }
      break;
    case Rule::NoColon:
      if (memchr(s, ':', len) != nullptr) return false;
      break;
    case Rule::None:
    case Rule::Printable:
      break;
  }
  return isPrintableText(s, len);
}

// Mask in the legacy layout (first octet in the low byte) is a run of ones
// followed by zeros in network order. 0 counts as contiguous.
bool maskContiguous(uint32_t legacy) {
  const uint32_t be = ((legacy & 0xFFu) << 24) | ((legacy & 0xFF00u) << 8) |
                      ((legacy >> 8) & 0xFF00u) | (legacy >> 24);
  const uint32_t inv = ~be;
  return (inv & (inv + 1u)) == 0;
}

bool floatOk(const Field& f, double v) {
  if (!isfinite(v) || v < f.min || v > f.max) return false;
  return !(f.nonZero && static_cast<float>(v) == 0.0f);
}

// Per-field check of the stored representation.
bool fieldValid(const Field& f, const uint8_t* p) {
  switch (f.kind) {
    case Kind::Bool:
      return *p <= 1;
    case Kind::U8:
      return *p >= f.min && *p <= f.max;
    case Kind::U16: {
      const uint16_t v = loadU16(p);
      return v >= f.min && v <= f.max;
    }
    case Kind::Str:
    case Kind::Secret: {
      const char* s = reinterpret_cast<const char*>(p);
      const size_t len = boundedLength(s, f.cap);
      return len < f.cap && stringRuleOk(f, s, len);
    }
    case Kind::Ip:
    case Kind::Id:
      return true;
    case Kind::Mask:
      return maskContiguous(loadU32(p));
    case Kind::Tenths: {
      const int16_t v = loadI16(p);
      return v >= f.min && v <= f.max;
    }
    case Kind::Float:
      return floatOk(f, loadFloat(p));
    case Kind::PctHold:
      return *p == kFailsafeHold || (*p >= f.min && *p <= f.max);
    case Kind::OffOrRange: {
      const uint16_t v = loadU16(p);
      return v == 0 || (v >= f.min && v <= f.max);
    }
  }
  return false;
}

// ---------------------------------------------------------------- paths

// Writes "<group>[.<n>].<field>" (n 1-based) into a caller buffer.
class PathOut {
 public:
  PathOut() : out_(nullptr), cap_(0) {}  // no path wanted
  PathOut(char* out, size_t cap) : out_(out), cap_(out ? cap : 0) {
    if (cap_) out_[0] = '\0';
  }
  void set(const Group& g, uint8_t element, const char* field) {
    if (!cap_) return;
    if (g.name == nullptr) {
      snprintf(out_, cap_, "%s", field);
    } else if (g.count) {
      snprintf(out_, cap_, "%s.%u.%s", g.name, static_cast<unsigned>(element) + 1u, field);
    } else {
      snprintf(out_, cap_, "%s.%s", g.name, field);
    }
  }
  void setText(const char* text) {
    if (cap_) snprintf(out_, cap_, "%s", text);
  }

 private:
  char* out_;
  size_t cap_;
};

// Equal MQTT segments: names compare with ' ' == '_' (mqtt_topics rule).
bool sameSegment(const char* a, const char* b) {
  for (;; ++a, ++b) {
    const char ca = *a == ' ' ? '_' : *a;
    const char cb = *b == ' ' ? '_' : *b;
    if (ca != cb) return false;
    if (ca == '\0') return true;
  }
}

// "1".."12" without leading zeros -> 1..12, else 0. `s` is NUL-terminated.
uint8_t valveNumberSegment(const char* s) {
  uint32_t v;
  if (s[0] == '0' || !parseUint(s, strlen(s), kValveCount, v)) return 0;
  return static_cast<uint8_t>(v);
}

const Group& kNet = kGroups[1];
const Group& kSyslog = kGroups[3];
const Group& kWeb = kGroups[4];
const Group& kMqtt = kGroups[5];
const Group& kValves = kGroups[6];
const Group& kTemps = kGroups[7];
const Group& kVolts = kGroups[8];

bool failAt(PathOut& po, const Group& g, uint8_t element, const char* field) {
  po.set(g, element, field);
  return false;
}

// Object groups and root fields have no element index.
bool failAt(PathOut& po, const Group& g, const char* field) {
  po.set(g, 0, field);
  return false;
}

// An item that clashes with an earlier one: its topic override, else its name.
bool failItem(PathOut& po, const Group& g, uint8_t i, const char* topic) {
  return failAt(po, g, i, topic[0] != '\0' ? "topic" : "name");
}

// An item's MQTT segment or HA id (never longer than the segment).
using ItemKey = char[sizeof(ValveConfig::name)];

// MQTT segment (or with `haId` its buildHaId()) of item i; false for a temp
// or volt slot that is not active (validateConfig has already made sure
// active slots have an id).
bool itemKey(const Config& c, ItemKind kind, uint8_t i, bool haId, ItemKey& out) {
  if (kind == ItemKind::Temp && !c.temps[i].active) return false;
  if (kind == ItemKind::Volt && !c.volts[i].active) return false;
  if (!haId) {
    itemSegment(c, kind, i, out, sizeof out);
    return true;
  }
  ItemKey seg;
  buildHaId(seg, itemSegment(c, kind, i, seg, sizeof seg), out, sizeof out);
  return true;
}

// The first item whose key equals the key of an earlier one: true and its
// index in `later`.
bool duplicateItem(const Config& c, ItemKind kind, uint8_t count, bool haId, uint8_t& later) {
  ItemKey a;
  ItemKey b;
  for (uint8_t i = 1; i < count; ++i) {  // NOMUTATE: i = 0 has no earlier item to compare
    if (!itemKey(c, kind, i, haId, a)) continue;
    for (uint8_t j = 0; j < i; ++j) {
      if (itemKey(c, kind, j, haId, b) && strcmp(a, b) == 0) {
        later = i;
        return true;
      }
    }
  }
  return false;
}

}  // namespace

// ---------------------------------------------------------------- defaults / validation

void setDefaults(Config& c) { c = Config{}; }

namespace {

// The rules of 2.0.0: a stored config that passes them can be used as it is.
bool storedRulesOk(const Config& c, PathOut& po) {
  if (c.schema != kConfigJsonSchema) return failAt(po, kGroups[0], "schema");
  for (size_t gi = 0; gi < kGroupCount; ++gi) {
    const Group& g = kGroups[gi];
    for (uint8_t e = 0; e < elementCount(g); ++e) {
      for (uint8_t fi = 0; fi < g.fieldCount; ++fi) {
        if (!fieldValid(g.fields[fi], fieldPtr(c, g, e, g.fields[fi]))) {
          return failAt(po, g, e, g.fields[fi].name);
        }
      }
    }
  }

  const NetConfig& n = c.net;
  if (!n.dhcp) {
    if (n.ip == 0) return failAt(po, kNet, "ip");
    if (n.mask == 0) return failAt(po, kNet, "mask");
    if (n.gateway == 0) return failAt(po, kNet, "gateway");
  }
  const size_t pwdLen = strlen(n.wifiPassword);
  // Empty = open network (legacy WiFi.begin(ssid, "")); WPA2 needs 8..63.
  if (n.ssid[0] != '\0' && pwdLen > 0 && pwdLen < 8) return failAt(po, kNet, "wifiPassword");
  if (n.iface == NetInterface::Wifi && n.ssid[0] == '\0') return failAt(po, kNet, "ssid");

  if (c.syslog.level > 0 && c.syslog.server == 0) return failAt(po, kSyslog, "server");

  const bool userSet = c.web.user[0] != '\0';
  const bool pwdSet = c.web.password[0] != '\0';
  if (userSet && !pwdSet) return failAt(po, kWeb, "password");
  if (pwdSet && !userSet) return failAt(po, kWeb, "user");

  const MqttConfig& m = c.mqtt;
  if (m.mode != MqttMode::Off && m.host[0] == '\0') return failAt(po, kMqtt, "host");
  if (m.minDelayS > m.publishIntervalS) return failAt(po, kMqtt, "minDelayS");
  if (m.mode == MqttMode::MqttHa && !m.separate) return failAt(po, kMqtt, "mode");

  for (uint8_t i = 0; i < kValveCount; ++i) {
    const char* name = c.valves[i].name;
    if (name[0] == '\0') continue;
    for (uint8_t j = 0; j < i; ++j) {
      if (sameSegment(name, c.valves[j].name)) return failAt(po, kValves, i, "name");
    }
    const uint8_t num = valveNumberSegment(name);
    // (A valve named after its own number finds itself named: no clash.)
    if (num != 0 && c.valves[num - 1].name[0] == '\0') {
      return failAt(po, kValves, i, "name");
    }
  }

  for (uint8_t i = 0; i < kTempSlotCount; ++i) {
    const TempSlotConfig& t = c.temps[i];
    if (isZero(t.id)) {
      if (t.active) return failAt(po, kTemps, i, "active");
      continue;
    }
    for (uint8_t j = 0; j < i; ++j) {
      if (c.temps[j].id == t.id) return failAt(po, kTemps, i, "id");
    }
  }
  for (uint8_t i = 0; i < kVoltSlotCount; ++i) {
    const VoltSlotConfig& v = c.volts[i];
    if (isZero(v.id)) {
      if (v.active) return failAt(po, kVolts, i, "active");
      continue;
    }
    for (uint8_t j = 0; j < i; ++j) {
      if (c.volts[j].id == v.id) return failAt(po, kVolts, i, "id");
    }
  }
  return true;
}

// V1-V3, the rules added after 2.0.0: checked on save and import; a stored
// config that breaks them is repaired, not dropped.
bool newRulesOk(const Config& c, PathOut& po) {
  // V1: Home Assistant reads a decimal point.
  const MqttConfig& m = c.mqtt;
  if (m.mode == MqttMode::MqttHa && m.germanDecimal) return failAt(po, kMqtt, "germanDecimal");
  // V2: one MQTT segment per valve; V3: one HA id per valve, per active slot.
  uint8_t i{};
  if (duplicateItem(c, ItemKind::Valve, kValveCount, false, i) ||
      duplicateItem(c, ItemKind::Valve, kValveCount, true, i)) {
    return failItem(po, kValves, i, c.valves[i].topic);
  }
  if (duplicateItem(c, ItemKind::Temp, kTempSlotCount, true, i)) {
    return failItem(po, kTemps, i, c.temps[i].topic);
  }
  if (duplicateItem(c, ItemKind::Volt, kVoltSlotCount, true, i)) {
    return failItem(po, kVolts, i, c.volts[i].topic);
  }
  return true;
}

}  // namespace

bool validateConfig(const Config& c, char* path, size_t pathCap) {
  PathOut po(path, pathCap);
  return storedRulesOk(c, po) && newRulesOk(c, po);
}

// ---------------------------------------------------------------- helpers

size_t itemSegment(const Config& c, ItemKind kind, uint8_t idx0, char* out, size_t cap) {
  if (out == nullptr || cap == 0) return 0;
  out[0] = '\0';
  const char* name = nullptr;
  const char* topic = nullptr;
  if (kind == ItemKind::Valve && idx0 < kValveCount) {
    name = c.valves[idx0].name;
    topic = c.valves[idx0].topic;
  } else if (kind == ItemKind::Temp && idx0 < kTempSlotCount) {
    name = c.temps[idx0].name;
    topic = c.temps[idx0].topic;
  } else if (kind == ItemKind::Volt && idx0 < kVoltSlotCount) {
    name = c.volts[idx0].name;
    topic = c.volts[idx0].topic;
  } else {
    return 0;
  }
  char seg[kItemNameMax];  // no NUL: `out` gets it
  const char* src = topic[0] != '\0' ? topic : name;
  size_t n = boundedLength(src, kItemNameMax);
  if (n == 0) {
    n = static_cast<size_t>(snprintf(seg, sizeof seg, "%u", idx0 + 1u));
  } else {
    for (size_t i = 0; i < n; ++i) seg[i] = src[i] == ' ' ? '_' : src[i];
  }
  if (n >= cap) return 0;
  memcpy(out, seg, n);
  out[n] = '\0';
  return n;
}

const char* mqttRootTopic(const Config& c) {
  return c.mqtt.rootTopic[0] != '\0' ? c.mqtt.rootTopic : c.station;
}

uint32_t effectiveDns(const NetConfig& n) { return !n.dhcp && n.dns == 0 ? n.gateway : n.dns; }

bool netTrialRequired(const NetConfig& before, const NetConfig& after) {
  if (before.iface != after.iface || before.dhcp != after.dhcp) return true;
  if (!after.dhcp && (before.ip != after.ip || before.mask != after.mask ||
                      before.gateway != after.gateway ||
                      effectiveDns(before) != effectiveDns(after))) {
    return true;
  }
  if (after.iface == NetInterface::Ethernet) return false;  // the same iface in both
  return strncmp(before.ssid, after.ssid, sizeof before.ssid) != 0 ||
         strncmp(before.wifiPassword, after.wifiPassword, sizeof before.wifiPassword) != 0;
}

uint8_t configRestartReasons(const Config& before, const Config& after) {
  uint8_t r = netTrialRequired(before.net, after.net) ? kRestartNetwork : 0;
  char a[sizeof(Config::station)];
  char b[sizeof(Config::station)];
  buildHostname(before.station, a, sizeof a);
  buildHostname(after.station, b, sizeof b);
  if (strcmp(a, b) != 0) r |= kRestartHostname;
  return r;
}

namespace {

// The `cfg` blob encoding of one field into `out` (codec below); its length.
size_t encodeOneField(const Field& f, const uint8_t* p, uint8_t* out, size_t cap);

// Equal stored values of one field: equal encodings (a string ends at its NUL).
bool sameField(const Field& f, const uint8_t* a, const uint8_t* b) {
  uint8_t ea[sizeof(WebConfig::allowedHosts)];  // the longest encoding: length byte + 80 chars
  uint8_t eb[sizeof ea];
  const size_t n = encodeOneField(f, a, ea, sizeof ea);
  return n == encodeOneField(f, b, eb, sizeof eb) && memcmp(ea, eb, n) == 0;
}

// The fields of a valve or sensor slot that shape its MQTT topics.
bool topicField(const Field& f) {
  return strcmp(f.name, "name") == 0 || strcmp(f.name, "active") == 0 ||
         strcmp(f.name, "topic") == 0 || strcmp(f.name, "id") == 0;
}

// The fields of group g, element e (with `itemsOnly` the topicField() ones).
bool sameFields(const Config& a, const Config& b, const Group& g, uint8_t e, bool itemsOnly) {
  for (uint8_t fi = 0; fi < g.fieldCount; ++fi) {
    const Field& f = g.fields[fi];
    if (itemsOnly && !topicField(f)) continue;
    if (!sameField(f, fieldPtr(a, g, e, f), fieldPtr(b, g, e, f))) return false;
  }
  return true;
}

}  // namespace

bool mqttTopicConfigChanged(const Config& a, const Config& b) {
  if (!sameFields(a, b, kGroups[0], 0, false) || !sameFields(a, b, kMqtt, 0, false)) return true;
  const Group* const items[] = {&kValves, &kTemps, &kVolts};
  for (const Group* g : items) {
    for (uint8_t e = 0; e < g->count; ++e) {
      if (!sameFields(a, b, *g, e, true)) return true;
    }
  }
  return false;
}

// ---------------------------------------------------------------- key paths

const char* setResultName(SetResult r) {
  switch (r) {
    case SetResult::Ok: return "ok";
    case SetResult::UnknownKey: return "unknown_key";
    case SetResult::WrongType: return "wrong_type";
    case SetResult::OutOfRange: return "out_of_range";
    case SetResult::ReadOnly: return "read_only";
  }
  return "unknown";
}

namespace {

// Strict JSON number grammar over exactly len (>= 1) bytes:
// -?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?
bool isDigit(char c) { return c >= '0' && c <= '9'; }

bool isJsonNumber(const char* s, size_t len) {
  size_t i = s[0] == '-' ? 1 : 0;
  if (i >= len) return false;
  if (s[i] == '0') {
    ++i;
  } else if (s[i] >= '1' && s[i] <= '9') {
    while (i < len && isDigit(s[i])) ++i;
  } else {
    return false;
  }
  if (i < len && s[i] == '.') {
    const size_t start = ++i;
    while (i < len && isDigit(s[i])) ++i;
    if (i == start) return false;
  }
  if (i < len && (s[i] == 'e' || s[i] == 'E')) {
    ++i;
    if (i < len && (s[i] == '+' || s[i] == '-')) ++i;
    const size_t start = i;
    while (i < len && isDigit(s[i])) ++i;
    if (i == start) return false;
  }
  return i == len;
}

constexpr size_t kNumberTextMax = 40;

// JSON number text -> double. Texts longer than kNumberTextMax count as
// infinite (every numeric key rejects them); false when not a JSON number.
bool numberFromText(const char* s, size_t len, double& out) {
  if (len == 0 || !isJsonNumber(s, len)) return false;
  if (len > kNumberTextMax) {
    out = HUGE_VAL;
    return true;
  }
  char tmp[kNumberTextMax + 1];
  memcpy(tmp, s, len);
  tmp[len] = '\0';
  out = strtod(tmp, nullptr);
  return true;
}

enum class Conv : uint8_t { Ok, WrongType, OutOfRange };

// Integer keys: Int, integral Float, or a numeric String ("3").
Conv toInteger(const ConfigValue& v, int64_t& out) {
  double d;
  switch (v.type) {
    case ConfigValue::Type::Int:
      out = v.i;
      return Conv::Ok;
    case ConfigValue::Type::Float:
      d = v.f;
      break;
    case ConfigValue::Type::String:
      if (v.s == nullptr || !numberFromText(v.s, v.len, d)) return Conv::WrongType;
      break;
    default:
      return Conv::WrongType;
  }
  if (!isfinite(d) || d < -2147483648.0 || d > 4294967295.0) return Conv::OutOfRange;
  if (d != floor(d)) return Conv::WrongType;
  out = static_cast<int64_t>(d);
  return Conv::Ok;
}

// Bool keys: Bool, 0/1 as number, or "true"/"false"/"0"/"1" as string.
Conv toBool(const ConfigValue& v, bool& out) {
  if (v.type == ConfigValue::Type::Bool) {
    out = v.b;
    return Conv::Ok;
  }
  if (v.type == ConfigValue::Type::String) {
    if (v.s == nullptr) return Conv::WrongType;
    if ((v.len == 4 && memcmp(v.s, "true", 4) == 0) || (v.len == 1 && v.s[0] == '1')) {
      out = true;
      return Conv::Ok;
    }
    if ((v.len == 5 && memcmp(v.s, "false", 5) == 0) || (v.len == 1 && v.s[0] == '0')) {
      out = false;
      return Conv::Ok;
    }
    return Conv::WrongType;
  }
  int64_t i;
  const Conv r = toInteger(v, i);
  if (r != Conv::Ok) return r;
  if (i != 0 && i != 1) return Conv::OutOfRange;
  out = i == 1;
  return Conv::Ok;
}

Conv toDouble(const ConfigValue& v, double& out) {
  switch (v.type) {
    case ConfigValue::Type::Int:
      out = static_cast<double>(v.i);
      return Conv::Ok;
    case ConfigValue::Type::Float:
      out = v.f;
      return Conv::Ok;
    case ConfigValue::Type::String:
      return (v.s != nullptr && numberFromText(v.s, v.len, out)) ? Conv::Ok : Conv::WrongType;
    default:
      return Conv::WrongType;
  }
}

// C -> tenths, rounded half away from zero, within [min, max]. The epsilon
// absorbs the binary representation error of decimal inputs (0.15 * 10 =
// 1.4999999...), so 0.15 -> 2 and -0.15 -> -2. NaN and +-inf fail the range
// test (every comparison with NaN is false), so nothing out of range is cast.
bool celsiusToTenths(double c, int32_t min, int32_t max, int16_t& out) {
  const double t = c * 10.0;
  const double eps = 1e-9;  // NOMUTATE: any tiny epsilon is equivalent
  const double r = t >= 0 ? floor(t + 0.5 + eps) : -floor(-t + 0.5 + eps);
  if (!(r >= min && r <= max)) return false;
  out = static_cast<int16_t>(r);
  return true;
}

SetResult fromConv(Conv c) {
  return c == Conv::WrongType ? SetResult::WrongType : SetResult::OutOfRange;
}

SetResult setField(Config& c, const Group& g, uint8_t element, const Field& f,
                   const ConfigValue& v, bool clearSecrets) {
  uint8_t* p = fieldPtr(c, g, element, f);
  switch (f.kind) {
    case Kind::Bool: {
      bool b;
      const Conv r = toBool(v, b);
      if (r != Conv::Ok) return fromConv(r);
      *p = b ? 1 : 0;
      return SetResult::Ok;
    }
    case Kind::U8:
    case Kind::U16:
    case Kind::PctHold:
    case Kind::OffOrRange: {
      int64_t i;
      const Conv r = toInteger(v, i);
      if (r != Conv::Ok) return fromConv(r);
      // [min, max], or the special value of the kind (hold, off).
      const bool special = (f.kind == Kind::PctHold && i == kFailsafeHold) ||
                           (f.kind == Kind::OffOrRange && i == 0);
      if (!special && (i < f.min || i > f.max)) return SetResult::OutOfRange;
      if (f.kind == Kind::U8 || f.kind == Kind::PctHold) {
        *p = static_cast<uint8_t>(i);
      } else {
        const uint16_t u = static_cast<uint16_t>(i);
        memcpy(p, &u, sizeof u);
      }
      return SetResult::Ok;
    }
    case Kind::Str:
    case Kind::Secret: {
      if (v.type != ConfigValue::Type::String || v.s == nullptr) return SetResult::WrongType;
      if (f.kind == Kind::Secret && v.len == 0 && !clearSecrets) return SetResult::Ok;
      // max < cap for every field, so the text and its NUL fit tmp and p.
      if (v.len > static_cast<size_t>(f.max) || memchr(v.s, '\0', v.len) != nullptr) {
        return SetResult::OutOfRange;
      }
      char tmp[sizeof(WebConfig::allowedHosts)];  // the largest field incl. NUL
      memcpy(tmp, v.s, v.len);
      tmp[v.len] = '\0';
      if (!stringRuleOk(f, tmp, v.len)) return SetResult::OutOfRange;
      memcpy(p, tmp, v.len + 1);
      return SetResult::Ok;
    }
    case Kind::Ip:
    case Kind::Mask: {
      if (v.type != ConfigValue::Type::String || v.s == nullptr) return SetResult::WrongType;
      uint32_t ip = 0;
      if (v.len != 0 && !parseIpv4(v.s, v.len, ip)) return SetResult::OutOfRange;
      if (f.kind == Kind::Mask && !maskContiguous(ip)) return SetResult::OutOfRange;
      memcpy(p, &ip, sizeof ip);
      return SetResult::Ok;
    }
    case Kind::Tenths: {
      double d;
      const Conv r = toDouble(v, d);
      if (r != Conv::Ok) return fromConv(r);
      int16_t t;
      if (!celsiusToTenths(d, f.min, f.max, t)) return SetResult::OutOfRange;
      memcpy(p, &t, sizeof t);
      return SetResult::Ok;
    }
    case Kind::Float: {
      double d;
      const Conv r = toDouble(v, d);
      if (r != Conv::Ok) return fromConv(r);
      if (!floatOk(f, d)) return SetResult::OutOfRange;
      const float fl = static_cast<float>(d);
      memcpy(p, &fl, sizeof fl);
      return SetResult::Ok;
    }
    case Kind::Id: {
      if (v.type != ConfigValue::Type::String || v.s == nullptr) return SetResult::WrongType;
      OneWireId id;
      if (v.len != 0 && !parseOneWireId(v.s, v.len, id)) return SetResult::OutOfRange;
      memcpy(p, id.b, sizeof id.b);
      return SetResult::Ok;
    }
  }
  return SetResult::UnknownKey;
}

// Next '.'-separated segment of path[0..len) starting at `pos`. `last` is
// true when the segment ends the path.
bool nextSegment(const char* path, size_t len, size_t& pos, const char*& seg, size_t& segLen,
                 bool& last) {
  if (pos > len) return false;
  seg = path + pos;
  size_t end = pos;
  while (end < len && path[end] != '.') ++end;
  segLen = end - pos;
  last = end == len;
  pos = end + 1;
  return segLen > 0;
}

bool segmentIs(const char* seg, size_t segLen, const char* name) {
  return strlen(name) == segLen && memcmp(seg, name, segLen) == 0;
}

// "<secret>Set" pseudo keys emitted by writeConfigJson.
bool isSecretFlag(const Field& f, const char* seg, size_t segLen) {
  const size_t n = strlen(f.name);
  return f.kind == Kind::Secret && segLen == n + 3 && memcmp(seg, f.name, n) == 0 &&
         memcmp(seg + n, "Set", 3) == 0;
}

SetResult setInGroup(Config& c, const Group& g, uint8_t element, const char* seg, size_t segLen,
                     const ConfigValue& v, bool clearSecrets) {
  for (uint8_t fi = 0; fi < g.fieldCount; ++fi) {
    const Field& f = g.fields[fi];
    if (segmentIs(seg, segLen, f.name)) return setField(c, g, element, f, v, clearSecrets);
    if (isSecretFlag(f, seg, segLen)) {
      return v.type == ConfigValue::Type::Bool ? SetResult::Ok : SetResult::WrongType;
    }
  }
  return SetResult::UnknownKey;
}

}  // namespace

SetResult setConfigValue(Config& c, const char* path, const ConfigValue& v, bool clearSecrets) {
  if (path == nullptr) return SetResult::UnknownKey;
  const size_t len = boundedLength(path, kPathMax + 1);
  if (len == 0 || len > kPathMax) return SetResult::UnknownKey;

  size_t pos = 0;
  const char* seg;
  size_t segLen;
  bool last;
  if (!nextSegment(path, len, pos, seg, segLen, last)) return SetResult::UnknownKey;

  if (last) {
    if (segmentIs(seg, segLen, "schema")) {
      // A 2.0.0 export says 1; both post back unchanged.
      int64_t i;
      return toInteger(v, i) == Conv::Ok && i >= 1 && i <= kConfigJsonSchema ? SetResult::Ok
                                                                              : SetResult::ReadOnly;
    }
    for (size_t gi = 0; gi < kGroupCount; ++gi) {
      if (kGroups[gi].name != nullptr) continue;
      const SetResult r = setInGroup(c, kGroups[gi], 0, seg, segLen, v, clearSecrets);
      if (r != SetResult::UnknownKey) return r;
    }
    return SetResult::UnknownKey;
  }

  for (size_t gi = 0; gi < kGroupCount; ++gi) {
    const Group& g = kGroups[gi];
    if (g.name == nullptr || !segmentIs(seg, segLen, g.name)) continue;
    uint8_t element = 0;
    if (g.count) {
      const char* idx;
      size_t idxLen;
      uint32_t n;
      if (!nextSegment(path, len, pos, idx, idxLen, last) || last || idx[0] == '0' ||
          !parseUint(idx, idxLen, g.count, n)) {
        return SetResult::UnknownKey;
      }
      element = static_cast<uint8_t>(n - 1);
    }
    const char* field;
    size_t fieldLen;
    if (!nextSegment(path, len, pos, field, fieldLen, last) || !last) return SetResult::UnknownKey;
    return setInGroup(c, g, element, field, fieldLen, v, clearSecrets);
  }
  return SetResult::UnknownKey;
}

// ---------------------------------------------------------------- JSON export

namespace {

// Shortest decimal (0..6 places) that reads back as the same float.
void writeFloat(JsonWriter& jw, float v) {
  if (!isfinite(v)) {
    jw.nullValue();
    return;
  }
  if (v == 0.0f) v = 0.0f;  // no "-0"
  // |v| <= FLT_MAX (39 digits) plus sign, point and 6 decimals: fits.
  char tmp[64];
  for (int d = 0;; ++d) {
    snprintf(tmp, sizeof tmp, "%.*f", d, static_cast<double>(v));
    if (d == 6 || strtof(tmp, nullptr) == v) break;
  }
  jw.raw(tmp);
}

void writeField(JsonWriter& jw, const Field& f, const uint8_t* p, SecretMode secrets) {
  if (f.kind == Kind::Secret && secrets == SecretMode::Flags) {
    char name[32];
    snprintf(name, sizeof name, "%sSet", f.name);
    jw.key(name);
    jw.value(p[0] != '\0');
    return;
  }
  jw.key(f.name);
  switch (f.kind) {
    case Kind::Bool:
      jw.value(*p != 0);
      break;
    case Kind::U8:
    case Kind::PctHold:
      jw.value(static_cast<uint32_t>(*p));
      break;
    case Kind::U16:
    case Kind::OffOrRange:
      jw.value(static_cast<uint32_t>(loadU16(p)));
      break;
    case Kind::Str:
    case Kind::Secret: {
      const char* s = reinterpret_cast<const char*>(p);
      jw.value(s, boundedLength(s, f.cap - 1u));
      break;
    }
    case Kind::Ip:
    case Kind::Mask: {
      char tmp[16];
      formatIpv4(loadU32(p), tmp, sizeof tmp);
      jw.value(tmp);
      break;
    }
    case Kind::Tenths:
      jw.fixed(loadI16(p), 1);
      break;
    case Kind::Float:
      writeFloat(jw, loadFloat(p));
      break;
    case Kind::Id: {
      OneWireId id;
      memcpy(id.b, p, sizeof id.b);
      char tmp[kOneWireIdTextLen + 1] = {0};
      if (!isZero(id)) formatOneWireId(id, tmp, sizeof tmp);
      jw.value(tmp);
      break;
    }
  }
}

void writeFields(JsonWriter& jw, const Config& c, const Group& g, uint8_t element,
                 SecretMode secrets) {
  for (uint8_t fi = 0; fi < g.fieldCount; ++fi) {
    writeField(jw, g.fields[fi], fieldPtr(c, g, element, g.fields[fi]), secrets);
  }
}

}  // namespace

bool writeConfigJson(JsonWriter& jw, const Config& c, SecretMode secrets, const ApplyInfo* apply) {
  jw.beginObject();
  jw.kv("schema", static_cast<uint32_t>(c.schema));
  for (size_t gi = 0; gi < kGroupCount; ++gi) {
    const Group& g = kGroups[gi];
    if (g.name == nullptr) {
      writeFields(jw, c, g, 0, secrets);
    } else if (g.count == 0) {
      jw.key(g.name);
      jw.beginObject();
      writeFields(jw, c, g, 0, secrets);
      jw.endObject();
    } else {
      jw.key(g.name);
      jw.beginArray();
      for (uint8_t e = 0; e < g.count; ++e) {
        jw.beginObject();
        writeFields(jw, c, g, e, secrets);
        jw.endObject();
      }
      jw.endArray();
    }
  }
  if (apply != nullptr) {
    jw.kv("restartRequired", apply->restartRequired);
    jw.kv("netTrial", apply->netTrial);
  }
  jw.endObject();
  return jw.ok();
}

// ---------------------------------------------------------------- JSON patch

const char* patchResultName(PatchResult r) {
  switch (r) {
    case PatchResult::Ok: return "ok";
    case PatchResult::Malformed: return "malformed";
    case PatchResult::UnknownKey: return "unknown_key";
    case PatchResult::WrongType: return "wrong_type";
    case PatchResult::OutOfRange: return "out_of_range";
    case PatchResult::ReadOnly: return "read_only";
    case PatchResult::Invalid: return "invalid";
  }
  return "unknown";
}

namespace {

PatchResult fromSet(SetResult r) {
  switch (r) {
    case SetResult::Ok: return PatchResult::Ok;
    case SetResult::UnknownKey: return PatchResult::UnknownKey;
    case SetResult::WrongType: return PatchResult::WrongType;
    case SetResult::OutOfRange: return PatchResult::OutOfRange;
    case SetResult::ReadOnly: return PatchResult::ReadOnly;
  }
  return PatchResult::UnknownKey;
}

// Recursive-descent walker over a bounded JSON text. Pass 1 (cfg == nullptr)
// checks the syntax and reads the root "clearSecrets"; pass 2 applies every
// scalar leaf through setConfigValue with its dotted path (array elements
// are 1-based path segments). No heap; nesting <= kConfigJsonMaxDepth.
class PatchWalker {
 public:
  PatchWalker(const char* s, size_t len, Config* cfg, bool clearSecrets)
      : s_(s), len_(len), cfg_(cfg), clearSecrets_(clearSecrets) {}

  // The whole document: one object, then only whitespace.
  bool document() {
    ws();
    if (pos_ >= len_ || s_[pos_] != '{') return syntaxError();
    if (!value(0)) return false;
    ws();
    return pos_ == len_ || syntaxError();
  }

  PatchResult result() const { return result_; }
  const char* path() const { return path_; }
  size_t offset() const { return pos_; }
  bool clearSecrets() const { return clearSecretsValue_; }

 private:
  // Longest decoded string kept; every string field is shorter (fits()), so
  // a truncated value is still rejected as out of range.
  static constexpr size_t kStrMax = kPatchStrMax;

  bool syntaxError() {
    result_ = PatchResult::Malformed;
    return false;
  }

  bool fail(SetResult r) {
    result_ = fromSet(r);
    return false;
  }

  void ws() {
    while (pos_ < len_ &&
           (s_[pos_] == ' ' || s_[pos_] == '\t' || s_[pos_] == '\n' || s_[pos_] == '\r')) {
      ++pos_;
    }
  }

  bool literal(const char* word) {
    const size_t n = strlen(word);
    if (len_ - pos_ < n || memcmp(s_ + pos_, word, n) != 0) return false;
    pos_ += n;
    return true;
  }

  static int hexVal(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
  }

  bool hex4(uint32_t& out) {
    if (len_ - pos_ < 4) return false;
    out = 0;
    for (size_t i = 0; i < 4; ++i) {
      const int h = hexVal(s_[pos_ + i]);
      if (h < 0) return false;
      out = (out << 4) | static_cast<uint32_t>(h);
    }
    pos_ += 4;
    return true;
  }

  // \uXXXX after the "\u" (pos_ at the first hex digit), incl. a following
  // low surrogate for a high one: the code point (names and secrets may be
  // UTF-8, the field rules decide).
  bool unicodeEscape(uint32_t& ch) {
    uint32_t cp;
    if (!hex4(cp) || (cp >= 0xDC00 && cp <= 0xDFFF)) return false;
    if (cp >= 0xD800 && cp <= 0xDBFF) {  // high surrogate: a low one must follow
      uint32_t lo;
      if (len_ - pos_ < 2 || s_[pos_] != '\\' || s_[pos_ + 1] != 'u') return false;
      pos_ += 2;
      if (!hex4(lo) || lo < 0xDC00 || lo > 0xDFFF) return false;
      cp = 0x10000u + ((cp - 0xD800u) << 10) + (lo - 0xDC00u);
    }
    ch = cp;
    return true;
  }

  // Appends code point `ch` as UTF-8, as far as it fits (a cut string is
  // longer than every field anyway).
  static void putUtf8(uint32_t ch, char* out, size_t cap, size_t& n) {
    uint8_t b[4];
    size_t k;
    if (ch < 0x80) {
      b[0] = static_cast<uint8_t>(ch);
      k = 1;
    } else if (ch < 0x800) {
      b[0] = static_cast<uint8_t>(0xC0 | (ch >> 6));
      b[1] = static_cast<uint8_t>(0x80 | (ch & 0x3F));
      k = 2;
    } else if (ch < 0x10000) {
      b[0] = static_cast<uint8_t>(0xE0 | (ch >> 12));
      b[1] = static_cast<uint8_t>(0x80 | ((ch >> 6) & 0x3F));
      b[2] = static_cast<uint8_t>(0x80 | (ch & 0x3F));
      k = 3;
    } else {
      b[0] = static_cast<uint8_t>(0xF0 | (ch >> 18));
      b[1] = static_cast<uint8_t>(0x80 | ((ch >> 12) & 0x3F));
      b[2] = static_cast<uint8_t>(0x80 | ((ch >> 6) & 0x3F));
      b[3] = static_cast<uint8_t>(0x80 | (ch & 0x3F));
      k = 4;
    }
    for (size_t i = 0; i < k && n < cap; ++i) out[n++] = static_cast<char>(b[i]);
  }

  // A string at pos_ (which is '"'). Decodes at most cap bytes into out and
  // NUL-terminates it (out has cap + 1 bytes); n = bytes kept. A longer
  // string is cut: every field and path is far shorter than cap.
  bool string(char* out, size_t cap, size_t& n) {
    n = 0;
    ++pos_;
    while (pos_ < len_) {
      const uint8_t c = static_cast<uint8_t>(s_[pos_++]);
      if (c == '"') {
        out[n] = '\0';
        return true;
      }
      uint32_t ch = c;
      bool escaped = false;
      if (c == '\\') {
        escaped = true;
        if (pos_ >= len_) return false;
        switch (s_[pos_++]) {
          case '"': ch = '"'; break;
          case '\\': ch = '\\'; break;
          case '/': ch = '/'; break;
          case 'b': ch = '\b'; break;
          case 'f': ch = '\f'; break;
          case 'n': ch = '\n'; break;
          case 'r': ch = '\r'; break;
          case 't': ch = '\t'; break;
          case 'u':
            if (!unicodeEscape(ch)) return false;
            break;
          default:
            return false;
        }
      } else if (c < 0x20) {
        return false;  // raw control characters are not allowed in JSON strings
      }
      if (escaped) {
        putUtf8(ch, out, cap, n);
      } else if (n < cap) {
        out[n++] = static_cast<char>(ch);  // raw bytes (UTF-8) as they are
      }
    }
    return false;  // unterminated
  }

  bool number(ConfigValue& v) {
    const size_t start = pos_;
    while (pos_ < len_) {
      const char c = s_[pos_];
      if (!((c >= '0' && c <= '9') || c == '-' || c == '+' || c == '.' || c == 'e' || c == 'E')) {
        break;
      }
      ++pos_;
    }
    // Every number becomes a double: all integer keys are far below 2^53,
    // and larger values are out of range for every key anyway.
    v.type = ConfigValue::Type::Float;
    if (!numberFromText(s_ + start, pos_ - start, v.f)) {
      pos_ = start;  // report the offset of the bad number
      return false;
    }
    return true;
  }

  // Appends one path segment. An overlong path is remembered and reported
  // as an unknown key when a leaf is reached below it.
  void push(const char* seg, size_t segLen) {
    const size_t need = segLen + (pathLen_ ? 1u : 0u);
    if (pathOverflow_ || pathLen_ + need > kPathMax) {
      pathOverflow_ = true;
      return;
    }
    if (pathLen_) path_[pathLen_++] = '.';
    memcpy(path_ + pathLen_, seg, segLen);
    pathLen_ += segLen;
    path_[pathLen_] = '\0';
  }

  bool leaf(uint8_t depth, const ConfigValue& v) {
    const bool isClear = depth == 1 && !pathOverflow_ && strcmp(path_, "clearSecrets") == 0;
    if (cfg_ == nullptr) {
      if (isClear) {
        if (v.type != ConfigValue::Type::Bool) return fail(SetResult::WrongType);
        clearSecretsValue_ = v.b;
      }
      return true;
    }
    if (isClear) return true;
    if (pathOverflow_) return fail(SetResult::UnknownKey);
    const SetResult r = setConfigValue(*cfg_, path_, v, clearSecrets_);
    return r == SetResult::Ok || fail(r);
  }

  bool value(uint8_t depth) {
    ws();
    if (pos_ >= len_) return syntaxError();
    const char c = s_[pos_];
    if (c == '{' || c == '[') return container(depth, c == '{');
    ConfigValue v;
    if (c == '"') {
      size_t n;
      if (!string(str_, kStrMax, n)) return syntaxError();
      v.type = ConfigValue::Type::String;
      v.s = str_;
      v.len = n;
    } else if (c == 't' || c == 'f') {
      if (!literal(c == 't' ? "true" : "false")) return syntaxError();
      v.type = ConfigValue::Type::Bool;
      v.b = c == 't';
    } else if (c == 'n') {
      if (!literal("null")) return syntaxError();
      v.type = ConfigValue::Type::Null;
    } else if (c == '-' || (c >= '0' && c <= '9')) {
      if (!number(v)) return syntaxError();
    } else {
      return syntaxError();
    }
    return leaf(depth, v);
  }

  bool container(uint8_t depth, bool isObject) {
    if (depth >= kConfigJsonMaxDepth) return syntaxError();
    ++pos_;
    ws();
    const char close = isObject ? '}' : ']';
    if (pos_ < len_ && s_[pos_] == close) {
      ++pos_;
      return true;
    }
    for (uint32_t index = 1;; ++index) {
      ws();
      const size_t savedLen = pathLen_;
      const bool savedOverflow = pathOverflow_;
      if (isObject) {
        size_t n;
        if (pos_ >= len_ || s_[pos_] != '"' || !string(key_, kStrMax, n)) {
          return syntaxError();
        }
        ws();
        if (pos_ >= len_ || s_[pos_] != ':') return syntaxError();
        ++pos_;
        // Empty keys and keys with NUL name nothing; a cut (96+) key
        // overflows the 64-char path in push().
        if (n == 0 || memchr(key_, '\0', n) != nullptr) {
          pathOverflow_ = true;
        } else {
          push(key_, n);
        }
      } else {
        char num[12];
        const int n = snprintf(num, sizeof num, "%u", static_cast<unsigned>(index));
        push(num, static_cast<size_t>(n));
      }
      if (!value(static_cast<uint8_t>(depth + 1))) return false;
      pathLen_ = savedLen;
      path_[pathLen_] = '\0';
      pathOverflow_ = savedOverflow;
      ws();
      if (pos_ >= len_) return syntaxError();
      if (s_[pos_] == close) {
        ++pos_;
        return true;
      }
      if (s_[pos_] != ',') return syntaxError();
      ++pos_;
    }
  }

  const char* s_;
  size_t len_;
  size_t pos_ = 0;
  Config* cfg_;
  bool clearSecrets_;
  bool clearSecretsValue_ = false;
  PatchResult result_ = PatchResult::Ok;
  char path_[kPathMax + 1] = {0};
  size_t pathLen_ = 0;
  bool pathOverflow_ = false;
  char str_[kStrMax + 1] = {0};  // current string value
  char key_[kStrMax + 1] = {0};  // current object key (copied into path_ at once)
};

}  // namespace

PatchResult applyConfigJson(Config& c, const char* json, size_t len, char* path, size_t pathCap) {
  PathOut po(path, pathCap);
  if (json == nullptr) {
    po.setText("@0");
    return PatchResult::Malformed;
  }
  PatchWalker check(json, len, nullptr, false);
  if (!check.document()) {
    if (check.result() == PatchResult::Malformed) {
      char at[24];
      snprintf(at, sizeof at, "@%u", static_cast<unsigned>(check.offset()));
      po.setText(at);
    } else {
      po.setText(check.path());
    }
    return check.result();
  }
  PatchWalker apply(json, len, &c, check.clearSecrets());
  if (!apply.document()) {
    po.setText(apply.path());
    return apply.result();
  }
  return validateConfig(c, path, pathCap) ? PatchResult::Ok : PatchResult::Invalid;
}

// ---------------------------------------------------------------- binary

namespace {

constexpr uint8_t kMagic[4] = {'V', 'D', 'M', 'C'};
constexpr size_t kHeaderSize = 8;  // magic + schema + payload length
constexpr size_t kCrcSize = 4;
constexpr uint8_t kExtMagic[4] = {'V', 'D', 'M', 'X'};
constexpr uint8_t kExtVersion = 1;
constexpr size_t kExtHeaderSize = 7;  // magic + version + payload length
constexpr size_t kExtRecordHead = 3;  // tag, element, length

class ByteOut {
 public:
  ByteOut(uint8_t* out, size_t cap) : out_(out), cap_(out ? cap : 0) {}
  void bytes(const void* p, size_t n) {
    if (!ok_ || n > cap_ - len_) {
      ok_ = false;
      return;
    }
    memcpy(out_ + len_, p, n);
    len_ += n;
  }
  void u8(uint8_t v) { bytes(&v, 1); }
  void u16(uint16_t v) {
    const uint8_t b[2] = {static_cast<uint8_t>(v), static_cast<uint8_t>(v >> 8)};
    bytes(b, 2);
  }
  void u32(uint32_t v) {
    const uint8_t b[4] = {static_cast<uint8_t>(v), static_cast<uint8_t>(v >> 8),
                          static_cast<uint8_t>(v >> 16), static_cast<uint8_t>(v >> 24)};
    bytes(b, 4);
  }
  bool ok() const { return ok_; }
  size_t length() const { return len_; }

 private:
  uint8_t* out_;
  size_t cap_;
  size_t len_ = 0;
  bool ok_ = true;
};

class ByteIn {
 public:
  ByteIn(const uint8_t* p, size_t len) : p_(p), len_(len) {}
  bool bytes(void* out, size_t n) {
    if (!ok_ || n > len_ - pos_) {
      ok_ = false;
      return false;
    }
    memcpy(out, p_ + pos_, n);
    pos_ += n;
    return true;
  }
  uint8_t u8() {
    uint8_t v = 0;
    bytes(&v, 1);
    return v;
  }
  uint16_t u16() {
    uint8_t b[2] = {0, 0};
    bytes(b, 2);
    return static_cast<uint16_t>(b[0] | (b[1] << 8));
  }
  uint32_t u32() {
    uint8_t b[4] = {0, 0, 0, 0};
    bytes(b, 4);
    return static_cast<uint32_t>(b[0]) | (static_cast<uint32_t>(b[1]) << 8) |
           (static_cast<uint32_t>(b[2]) << 16) | (static_cast<uint32_t>(b[3]) << 24);
  }
  bool ok() const { return ok_; }
  bool atEnd() const { return pos_ == len_; }
  void fail() { ok_ = false; }

 private:
  const uint8_t* p_;
  size_t len_;
  size_t pos_ = 0;
  bool ok_ = true;
};

void encodeField(ByteOut& out, const Field& f, const uint8_t* p) {
  switch (f.kind) {
    case Kind::Bool:
    case Kind::U8:
    case Kind::PctHold:
      out.u8(*p);
      break;
    case Kind::U16:
    case Kind::Tenths:
    case Kind::OffOrRange:
      out.u16(loadU16(p));
      break;
    case Kind::Str:
    case Kind::Secret: {
      const size_t n = boundedLength(reinterpret_cast<const char*>(p), f.cap - 1u);
      out.u8(static_cast<uint8_t>(n));
      out.bytes(p, n);
      break;
    }
    case Kind::Ip:
    case Kind::Mask:
    case Kind::Float:
      out.u32(loadU32(p));
      break;
    case Kind::Id:
      out.bytes(p, sizeof(OneWireId::b));
      break;
  }
}

size_t encodeOneField(const Field& f, const uint8_t* p, uint8_t* out, size_t cap) {
  ByteOut bo(out, cap);
  encodeField(bo, f, p);
  return bo.length();
}

// Structural decoding only; value ranges are checked by validateConfig().
void decodeField(ByteIn& in, const Field& f, uint8_t* p) {
  switch (f.kind) {
    case Kind::Bool:
    case Kind::U8:
    case Kind::PctHold:
      *p = in.u8();
      if (f.kind == Kind::Bool && *p > 1) in.fail();
      break;
    case Kind::U16:
    case Kind::Tenths:
    case Kind::OffOrRange: {
      const uint16_t v = in.u16();
      memcpy(p, &v, sizeof v);
      break;
    }
    case Kind::Str:
    case Kind::Secret: {
      const uint8_t n = in.u8();
      if (n >= f.cap) {
        in.fail();
        break;
      }
      if (in.bytes(p, n)) {
        p[n] = '\0';
        if (memchr(p, '\0', n) != nullptr) in.fail();
      }
      break;
    }
    case Kind::Ip:
    case Kind::Mask:
    case Kind::Float: {
      const uint32_t v = in.u32();
      memcpy(p, &v, sizeof v);
      break;
    }
    case Kind::Id:
      in.bytes(p, sizeof(OneWireId::b));
      break;
  }
}

uint32_t loadLe32(const uint8_t* p) {
  return static_cast<uint32_t>(p[0]) | (static_cast<uint32_t>(p[1]) << 8) |
         (static_cast<uint32_t>(p[2]) << 16) | (static_cast<uint32_t>(p[3]) << 24);
}

// The `cfgx` field with this tag.
bool findExt(uint8_t tag, const Group*& group, const Field*& field) {
  for (const Group& g : kGroups) {
    for (uint8_t fi = 0; fi < g.fieldCount; ++fi) {
      if (g.fields[fi].ext != tag) continue;
      group = &g;
      field = &g.fields[fi];
      return true;
    }
  }
  return false;
}

// One record's value into its field, when it passes the field rule.
bool applyExt(Config& c, const Group& g, const Field& f, uint8_t element, const uint8_t* v,
              uint8_t n) {
  if (element >= elementCount(g)) return false;
  uint8_t tmp[sizeof(WebConfig::allowedHosts)] = {0};  // the largest field incl. NUL
  size_t size = n;
  if (f.kind == Kind::Str) {
    if (n > f.max || memchr(v, '\0', n) != nullptr) return false;
    memcpy(tmp, v, n);
    ++size;  // with the NUL
  } else {
    if (n != (f.kind == Kind::PctHold ? 1 : 2)) return false;
    tmp[0] = v[0];
    if (n == 2) {
      const uint16_t u = static_cast<uint16_t>(v[0] | v[1] << 8);  // little endian on the wire
      memcpy(tmp, &u, sizeof u);
    }
  }
  if (!fieldValid(f, tmp)) return false;
  memcpy(fieldPtr(c, g, element, f), tmp, size);
  return true;
}

}  // namespace

size_t encodeConfig(const Config& c, uint8_t* out, size_t cap) {
  ByteOut bo(out, cap);
  bo.bytes(kMagic, sizeof kMagic);
  bo.u16(kConfigBaseSchema);
  bo.u16(0);  // NOMUTATE: payload length placeholder, patched below
  for (size_t gi = 0; gi < kGroupCount; ++gi) {
    const Group& g = kGroups[gi];
    for (uint8_t e = 0; e < elementCount(g); ++e) {
      for (uint8_t fi = 0; fi < g.fieldCount; ++fi) {
        if (g.fields[fi].ext != 0) continue;
        encodeField(bo, g.fields[fi], fieldPtr(c, g, e, g.fields[fi]));
      }
    }
  }
  if (!bo.ok()) return 0;
  const size_t payload = bo.length() - kHeaderSize;  // < 2 KiB by the schema, fits u16
  out[6] = static_cast<uint8_t>(payload);
  out[7] = static_cast<uint8_t>(payload >> 8);
  bo.u32(crc32(out, bo.length()));
  return bo.ok() ? bo.length() : 0;
}

uint32_t sanitizeConfig(Config& c, Repairs* out) {
  // Pass-through: the loader still rejects what a repair would fix.
  (void)c;
  if (out != nullptr) *out = Repairs{};
  return 0;
}

DecodeResult decodeConfig(const uint8_t* data, size_t len, Config& out, DecodeInfo* info) {
  setDefaults(out);
  if (info != nullptr) *info = DecodeInfo{};
  if (data == nullptr || len < kHeaderSize + kCrcSize) return DecodeResult::TooShort;
  if (memcmp(data, kMagic, sizeof kMagic) != 0) return DecodeResult::BadMagic;
  const uint16_t schema = static_cast<uint16_t>(data[4] | (data[5] << 8));
  if (info != nullptr) info->schema = schema;
  const size_t payload = static_cast<size_t>(data[6] | (data[7] << 8));
  if (len < kHeaderSize + payload + kCrcSize) return DecodeResult::TooShort;
  if (len > kHeaderSize + payload + kCrcSize) return DecodeResult::Invalid;
  const size_t crcAt = kHeaderSize + payload;
  if (crc32(data, crcAt) != loadLe32(data + crcAt)) return DecodeResult::BadCrc;
  // Schema 1 is the first one; older blobs do not exist. A newer schema is
  // left alone so a downgrade does not destroy it.
  if (schema != kConfigBaseSchema) return DecodeResult::UnsupportedSchema;

  // Decoded in place (Config is too big for a second copy on small stacks);
  // any failure puts the defaults back.
  ByteIn in(data + kHeaderSize, payload);
  for (size_t gi = 0; gi < kGroupCount && in.ok(); ++gi) {
    const Group& g = kGroups[gi];
    for (uint8_t e = 0; e < elementCount(g) && in.ok(); ++e) {
      for (uint8_t fi = 0; fi < g.fieldCount && in.ok(); ++fi) {
        if (g.fields[fi].ext != 0) continue;
        decodeField(in, g.fields[fi], fieldPtr(out, g, e, g.fields[fi]));
      }
    }
  }
  // The rules added after 2.0.0 never drop a stored config.
  PathOut none;
  if (!in.ok() || !in.atEnd() || !storedRulesOk(out, none)) {
    setDefaults(out);
    return DecodeResult::Invalid;
  }
  return DecodeResult::Ok;
}

size_t encodeConfigExt(const Config& c, uint8_t* out, size_t cap, const uint8_t* keep,
                       size_t keepLen) {
  ByteOut bo(out, cap);
  bo.bytes(kExtMagic, sizeof kExtMagic);
  bo.u8(kExtVersion);
  bo.u16(0);  // NOMUTATE: payload length placeholder, patched below
  for (const Group& g : kGroups) {
    for (uint8_t e = 0; e < elementCount(g); ++e) {
      for (uint8_t fi = 0; fi < g.fieldCount; ++fi) {
        const Field& f = g.fields[fi];
        if (f.ext == 0) continue;
        bo.u8(f.ext);
        bo.u8(e);
        // Strings carry their length byte already.
        if (f.kind != Kind::Str) bo.u8(f.kind == Kind::PctHold ? 1 : 2);
        encodeField(bo, f, fieldPtr(c, g, e, f));
      }
    }
  }
  if (keep != nullptr) bo.bytes(keep, keepLen);
  if (!bo.ok()) return 0;
  const size_t payload = bo.length() - kExtHeaderSize;  // < 2 KiB, fits u16
  out[5] = static_cast<uint8_t>(payload);
  out[6] = static_cast<uint8_t>(payload >> 8);
  bo.u32(crc32(out, bo.length()));
  return bo.ok() ? bo.length() : 0;
}

ExtResult decodeConfigExt(const uint8_t* data, size_t len, Config& inout, ExtInfo* info,
                          uint8_t* keep, size_t keepCap) {
  ExtInfo local;
  ExtInfo& r = info != nullptr ? *info : local;
  r = ExtInfo{};
  if (data == nullptr || len == 0) return ExtResult::Absent;
  if (len < kExtHeaderSize + kCrcSize) return ExtResult::TooShort;
  if (memcmp(data, kExtMagic, sizeof kExtMagic) != 0) return ExtResult::BadMagic;
  const size_t end = kExtHeaderSize + static_cast<size_t>(data[5] | (data[6] << 8));
  if (len < end + kCrcSize) return ExtResult::TooShort;
  if (crc32(data, end) != loadLe32(data + end)) return ExtResult::BadCrc;
  const size_t keepMax = std::min(keepCap, kConfigExtKeepMax);
  for (size_t pos = kExtHeaderSize; pos < end;) {
    // A record cut by the payload end (only a broken writer does that).
    if (end - pos < kExtRecordHead || end - pos - kExtRecordHead < data[pos + 2]) {
      ++r.bad;
      break;
    }
    const size_t recLen = kExtRecordHead + data[pos + 2];
    const Group* g = nullptr;
    const Field* f = nullptr;
    if (!findExt(data[pos], g, f)) {
      ++r.unknown;
      if (keep != nullptr && recLen <= keepMax - r.keepLen) {
        memcpy(keep + r.keepLen, data + pos, recLen);
        r.keepLen += recLen;
      }
    } else if (applyExt(inout, *g, *f, data[pos + 1], data + pos + kExtRecordHead,
                        data[pos + 2])) {
      ++r.applied;
    } else {
      ++r.bad;
    }
    pos += recLen;
  }
  return ExtResult::Ok;
}

bool loadConfigBlobs(const StoredBlobs& b, Config& out, LoadInfo& info, uint8_t* keep,
                     size_t keepCap) {
  info = LoadInfo{};
  info.base = decodeConfig(b.base, b.baseLen, out, &info.decode);
  if (info.base != DecodeResult::Ok) return false;
  info.ext = decodeConfigExt(b.ext, b.extLen, out, &info.extInfo, keep, keepCap);
  return true;
}

uint32_t crc32(const uint8_t* data, size_t len, uint32_t crc) {
  // Nibble table: 64 bytes of flash, fast enough for 512 KiB images.
  static const uint32_t kTable[16] = {
      0x00000000u, 0x1DB71064u, 0x3B6E20C8u, 0x26D930ACu, 0x76DC4190u, 0x6B6B51F4u,
      0x4DB26158u, 0x5005713Cu, 0xEDB88320u, 0xF00F9344u, 0xD6D6A3E8u, 0xCB61B38Cu,
      0x9B64C2B0u, 0x86D3D2D4u, 0xA00AE278u, 0xBDBDF21Cu};
  if (data == nullptr) return crc;
  crc = ~crc;
  for (size_t i = 0; i < len; ++i) {
    crc ^= data[i];
    crc = (crc >> 4) ^ kTable[crc & 0x0F];
    crc = (crc >> 4) ^ kTable[crc & 0x0F];
  }
  return ~crc;
}

}  // namespace vdm
