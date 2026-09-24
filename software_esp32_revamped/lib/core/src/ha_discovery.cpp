#include "vdm/ha_discovery.h"

#include <stdio.h>
#include <string.h>

#include "vdm/json_writer.h"

namespace vdm {

namespace {

// ---------------------------------------------------------------- layout

constexpr uint16_t kCommonCount = 4;
constexpr uint16_t kValveKinds = 15;
constexpr uint16_t kValvesFirst = kCommonCount;
constexpr uint16_t kTempsFirst = kValvesFirst + kValveCount * kValveKinds;
constexpr uint16_t kVoltsFirst = kTempsFirst + kTempSlotCount;
constexpr uint16_t kTailFirst = kVoltsFirst + kVoltSlotCount;
constexpr uint16_t kTailCount = 3;
constexpr uint16_t kEntityCount = kTailFirst + kTailCount;

constexpr uint16_t kDropGlobal = 2;
constexpr uint16_t kDropKinds = 7;
constexpr uint16_t kDropCount = kDropGlobal + kValveCount * 2 * kDropKinds;

constexpr size_t kIdMax = 47;

enum class Kind : uint8_t {
  Text,       // legacy read-only value exposed as text: command "<base>/set"
  Valve,      // target: HA valve entity
  Temp,       // temperature sensor
  Actual,     // position sensor, %
  Counter,    // v2 diag counter
  LastStop,   // v2 last move stop reason
};

enum class Gate : uint8_t { Always, Temp1, Temp2, Diag, NewDiag };

// Object-id / name / unique_id style of per-valve entities.
enum class Style : uint8_t {
  Legacy,  // objectId valves_<key>_<R>, name/uid valves.<R>.<dotted>
  Diag,    // objectId diag_<key>_<R>, name/uid diag.<R>.<key>
};

struct ValveDef {
  HaComponent comp;
  Kind kind;
  Gate gate;
  Style style;
  const char* key;     // object id part
  const char* dotted;  // name / unique_id part
  Topic topic;
  const char* icon;
};

// Order is the discovery order (DESIGN.md "HA discovery").
const ValveDef kValveDefs[kValveKinds] = {
    {HaComponent::Text, Kind::Text, Gate::Always, Style::Legacy, "state", "state", Topic::ValveState, "mdi:state-machine"},
    {HaComponent::Valve, Kind::Valve, Gate::Always, Style::Legacy, "target", "target", Topic::ValveTarget, "mdi:valve"},
    {HaComponent::Sensor, Kind::Actual, Gate::NewDiag, Style::Legacy, "actual", "actual", Topic::ValveActual, "mdi:valve"},
    {HaComponent::Sensor, Kind::Temp, Gate::Temp1, Style::Legacy, "temp1", "temp1", Topic::ValveTemp1, "mdi:thermometer"},
    {HaComponent::Sensor, Kind::Temp, Gate::Temp2, Style::Legacy, "temp2", "temp2", Topic::ValveTemp2, "mdi:thermometer"},
    {HaComponent::Text, Kind::Text, Gate::Always, Style::Legacy, "calibration_date", "calibration.date", Topic::ValveCalibDate, "mdi:timelapse"},
    {HaComponent::Text, Kind::Text, Gate::Always, Style::Legacy, "calibration_repetitions", "calibration.repetitions", Topic::ValveCalibRepetitions, "mdi:valve"},
    {HaComponent::Text, Kind::Text, Gate::Diag, Style::Legacy, "diag_openCount", "diag.openCount", Topic::ValveOpenCount, "mdi:valve"},
    {HaComponent::Text, Kind::Text, Gate::Diag, Style::Legacy, "diag_closeCount", "diag.closeCount", Topic::ValveCloseCount, "mdi:valve"},
    {HaComponent::Text, Kind::Text, Gate::Diag, Style::Legacy, "diag_deadZoneCount", "diag.deadZoneCount", Topic::ValveDeadZoneCount, "mdi:valve"},
    {HaComponent::Text, Kind::Text, Gate::Diag, Style::Legacy, "diag_moves", "diag.moves", Topic::ValveMoves, "mdi:valve"},
    {HaComponent::Text, Kind::Text, Gate::Diag, Style::Legacy, "diag_meanCurrrent", "diag.meanCurrrent", Topic::ValveMeanCurrent, "mdi:valve"},
    {HaComponent::Sensor, Kind::Counter, Gate::NewDiag, Style::Diag, "earlyStops", "earlyStops", Topic::DiagValveEarlyStops, "mdi:alert-outline"},
    {HaComponent::Sensor, Kind::Counter, Gate::NewDiag, Style::Diag, "cmdRejected", "cmdRejected", Topic::DiagValveCmdRejected, "mdi:alert-outline"},
    {HaComponent::Sensor, Kind::LastStop, Gate::NewDiag, Style::Diag, "lastStop", "lastStop", Topic::DiagValveLastMove, "mdi:stop-circle-outline"},
};

struct CommonDef {
  const char* key;
  Topic topic;
  const char* icon;
};

const CommonDef kCommonDefs[kCommonCount] = {
    {"state", Topic::CommonState, "mdi:state-machine"},
    {"message", Topic::CommonMessage, "mdi:message"},
    {"uptime", Topic::CommonUptime, "mdi:timelapse"},
    {"ip", Topic::CommonIp, "mdi:message"},
};

struct DropDef {
  HaComponent comp;
  const char* prefix;
};

const DropDef kDropGlobals[kDropGlobal] = {
    {HaComponent::Select, "heatControl"},
    {HaComponent::Number, "parkPosition"},
};

const DropDef kDropValve[kDropKinds] = {
    {HaComponent::Climate, "climate_"},
    {HaComponent::Number, "valves_control_dynOffs_"},
    {HaComponent::Number, "valves_control_min_"},
    {HaComponent::Number, "valves_control_max_"},
    {HaComponent::Select, "valves_window_state_"},
    {HaComponent::Switch, "valves_window_state_"},
    {HaComponent::Number, "valves_window_target_"},
};

// ---------------------------------------------------------------- helpers

// Bounded string builder; ok() false after an overflow.
class Str {
 public:
  Str(char* out, size_t cap) : out_(out), cap_(cap) { out_[0] = '\0'; }
  Str& add(const char* s, size_t max = SIZE_MAX) {
    const size_t n = boundedLength(s, max);
    if (!ok_ || n >= cap_ - len_) {
      ok_ = false;
      return *this;
    }
    memcpy(out_ + len_, s, n);
    len_ += n;
    out_[len_] = '\0';
    return *this;
  }
  bool ok() const { return ok_; }

 private:
  char* out_;
  size_t cap_;
  size_t len_ = 0;
  bool ok_ = true;
};

// NUL-terminated copy of at most N - 1 chars of a context field (which may
// lack its NUL when the glue filled all of it).
template <size_t N>
void copyField(char (&dst)[N], const char* src) {
  const size_t n = boundedLength(src, N - 1);
  memcpy(dst, src, n);
  dst[n] = '\0';
}

// The station as it appears in discovery topics and ids: non-empty and safe
// (isSafeName also bounds its length), else discovery is off.
bool stationOf(const DiscoveryContext& ctx, char (&out)[kStationNameMax + 1]) {
  if (!isSafeName(ctx.topics.station, kStationNameMax, false)) return false;
  copyField(out, ctx.topics.station);
  return true;
}

// A valve/sensor segment as filled by glue: must be a valid topic segment.
bool segmentOf(const char* src, char (&out)[kSegmentMax + 1]) {
  if (boundedLength(src, kSegmentMax + 1) > kSegmentMax) return false;
  copyField(out, src);
  if (out[0] == '\0') return false;
  for (const char* p = out; *p; ++p) {
    if (*p == '/' || *p == '+' || *p == '#' || *p == ' ' || *p == '"' || *p == '\\') return false;
  }
  return true;
}

bool buildConfigTopic(const char* station, HaComponent comp, const char* objectId,
                      DiscoveryMessage& msg) {
  Str s(msg.topic, sizeof msg.topic);
  s.add("homeassistant/").add(haComponentName(comp)).add("/").add(station).add("/");
  s.add(objectId).add("/config");
  return s.ok();
}

// ---------------------------------------------------------------- entities

struct Entity {
  HaComponent comp = HaComponent::Sensor;
  char objectId[kIdMax + 1] = {};
  char name[kIdMax + 1] = {};
  char uid[kIdMax + 1] = {};  // without the "<station>." prefix
  char state[kTopicMax + 1] = {};
  char command[kTopicMax + 1] = {};
  char unit[kUnitMax + 1] = {};
  const char* icon = nullptr;
  const char* deviceClass = nullptr;
  const char* stateClass = nullptr;
  const char* valueTemplate = nullptr;
  bool reportsPosition = false;
  bool binary = false;
  bool diagnostic = false;
};

enum class Describe : uint8_t { Skip, Ok, Error };

// "<base>/set": the legacy command topic of a text entity (never subscribed).
bool textCommand(const DiscoveryContext& ctx, Topic t, const char* segment, Entity& e) {
  TopicContext plain = ctx.topics;
  plain.separate = false;
  const size_t n = buildTopic(plain, t, segment, e.command, sizeof e.command);
  if (n == 0 || n + 4 >= sizeof e.command) return false;
  memcpy(e.command + n, "/set", 5);
  return true;
}

bool stateTopic(const DiscoveryContext& ctx, Topic t, const char* segment, Entity& e) {
  return buildTopic(ctx.topics, t, segment, e.state, sizeof e.state) > 0;
}

Describe describeCommon(const DiscoveryContext& ctx, uint16_t k, Entity& e) {
  const CommonDef& d = kCommonDefs[k];
  if (d.topic == Topic::CommonUptime && !ctx.publishUptime) return Describe::Skip;
  e.comp = HaComponent::Text;
  e.icon = d.icon;
  Str(e.objectId, sizeof e.objectId).add(d.key);
  Str(e.name, sizeof e.name).add(d.key);
  Str(e.uid, sizeof e.uid).add("common.").add(d.key);
  if (!stateTopic(ctx, d.topic, nullptr, e) || !textCommand(ctx, d.topic, nullptr, e)) {
    return Describe::Error;
  }
  return Describe::Ok;
}

// `keepUnknown`: describe the temp entities of a valve whose sensors are not
// known yet (stale check only; never published on a guess).
Describe describeValve(const DiscoveryContext& ctx, uint8_t v, uint16_t k, Entity& e,
                       bool keepUnknown) {
  const DiscoveryContext::Valve& valve = ctx.valves[v];
  const ValveDef& d = kValveDefs[k];
  if (!valve.active) return Describe::Skip;
  const bool unknown = keepUnknown && !valve.tempsKnown;
  switch (d.gate) {
    case Gate::Always: break;
    case Gate::Temp1: if (!valve.hasTemp1 && !unknown) return Describe::Skip; break;
    case Gate::Temp2: if (!valve.hasTemp2 && !unknown) return Describe::Skip; break;
    case Gate::Diag: if (!ctx.publishDiag) return Describe::Skip; break;
    case Gate::NewDiag: if (!ctx.newDiag) return Describe::Skip; break;
  }
  char seg[kSegmentMax + 1];
  if (!segmentOf(valve.segment, seg)) return Describe::Error;

  e.comp = d.comp;
  e.icon = d.icon;
  Str oid(e.objectId, sizeof e.objectId);
  Str name(e.name, sizeof e.name);
  if (d.style == Style::Legacy) {
    oid.add("valves_").add(d.key).add("_").add(seg);
    name.add("valves.").add(seg).add(".").add(d.dotted);
  } else {
    oid.add("diag_").add(d.key).add("_").add(seg);
    name.add("diag.").add(seg).add(".").add(d.dotted);
  }
  Str(e.uid, sizeof e.uid).add(e.name);
  if (!oid.ok() || !name.ok() || !stateTopic(ctx, d.topic, seg, e)) return Describe::Error;

  switch (d.kind) {
    case Kind::Text:
      if (!textCommand(ctx, d.topic, seg, e)) return Describe::Error;
      break;
    case Kind::Valve:
      if (buildTargetCommandTopic(ctx.topics, seg, e.command, sizeof e.command) == 0) {
        return Describe::Error;
      }
      e.deviceClass = "water";
      e.reportsPosition = true;
      break;
    case Kind::Temp:
      e.deviceClass = "temperature";
      e.stateClass = "measurement";
      Str(e.unit, sizeof e.unit).add("\xC2\xB0" "C");
      break;
    case Kind::Actual:
      e.stateClass = "measurement";
      Str(e.unit, sizeof e.unit).add("%");
      break;
    case Kind::Counter:
      e.stateClass = "total_increasing";
      e.diagnostic = true;
      break;
    case Kind::LastStop:
      e.valueTemplate = "{{ value_json.stop }}";
      e.diagnostic = true;
      break;
  }
  return Describe::Ok;
}

Describe describeTemp(const DiscoveryContext& ctx, uint8_t i, Entity& e) {
  const DiscoveryContext::Sensor& s = ctx.temps[i];
  if (!s.active || !s.published || s.id[0] == '\0') return Describe::Skip;
  char seg[kSegmentMax + 1];
  if (!segmentOf(s.segment, seg)) return Describe::Error;
  e.comp = HaComponent::Sensor;
  e.icon = "mdi:thermometer";
  e.deviceClass = "temperature";
  e.stateClass = "measurement";
  Str(e.unit, sizeof e.unit).add("\xC2\xB0" "C");
  Str oid(e.objectId, sizeof e.objectId);
  oid.add("temps_").add(seg);
  Str name(e.name, sizeof e.name);
  name.add("temps.").add(seg);
  Str uid(e.uid, sizeof e.uid);
  uid.add(s.id, kOneWireIdTextLen);
  if (!oid.ok() || !name.ok() || !uid.ok() || !stateTopic(ctx, Topic::TempValue, seg, e)) {
    return Describe::Error;
  }
  return Describe::Ok;
}

Describe describeVolt(const DiscoveryContext& ctx, uint8_t i, Entity& e) {
  const DiscoveryContext::Sensor& s = ctx.volts[i];
  if (!s.active || s.id[0] == '\0') return Describe::Skip;
  char seg[kSegmentMax + 1];
  if (!segmentOf(s.segment, seg)) return Describe::Error;
  e.comp = HaComponent::Sensor;
  e.stateClass = "measurement";
  copyField(e.unit, s.unit);
  if (strcmp(e.unit, "V") == 0 || strcmp(e.unit, "mV") == 0) e.deviceClass = "voltage";
  Str oid(e.objectId, sizeof e.objectId);
  oid.add("volts_").add(seg);
  // Legacy: the raw configured name (spaces kept, "volts." when unnamed).
  Str name(e.name, sizeof e.name);
  name.add("volts.").add(s.name, kItemNameMax);
  Str uid(e.uid, sizeof e.uid);
  uid.add(s.id, kOneWireIdTextLen);
  if (!oid.ok() || !name.ok() || !uid.ok() || !stateTopic(ctx, Topic::VoltValue, seg, e)) {
    return Describe::Error;
  }
  return Describe::Ok;
}

Describe describeTail(const DiscoveryContext& ctx, uint16_t k, Entity& e) {
  if (!ctx.newDiag) return Describe::Skip;
  e.diagnostic = true;
  Topic t;
  const char* id;
  switch (k) {
    case 0:
      id = "stm.uptime";
      t = Topic::DiagStmUptime;
      e.deviceClass = "duration";
      e.stateClass = "measurement";
      Str(e.unit, sizeof e.unit).add("s");
      break;
    case 1:
      id = "calibration.active";
      t = Topic::DiagCalibrationActive;
      e.comp = HaComponent::BinarySensor;
      e.deviceClass = "running";
      e.binary = true;
      break;
    default:
      id = "stm.link";
      t = Topic::DiagStmLink;
      e.icon = "mdi:lan-connect";
      break;
  }
  Str oid(e.objectId, sizeof e.objectId);
  oid.add("diag_").add(id);
  for (char* p = e.objectId; *p; ++p) {
    if (*p == '.') *p = '_';
  }
  Str(e.name, sizeof e.name).add("diag.").add(id);
  Str(e.uid, sizeof e.uid).add(e.name);
  return stateTopic(ctx, t, nullptr, e) ? Describe::Ok : Describe::Error;
}

Describe describe(const DiscoveryContext& ctx, uint16_t pos, Entity& e,
                  bool keepUnknown = false) {
  if (pos < kValvesFirst) return describeCommon(ctx, pos, e);
  if (pos < kTempsFirst) {
    const uint16_t r = pos - kValvesFirst;
    return describeValve(ctx, static_cast<uint8_t>(r / kValveKinds), r % kValveKinds, e,
                         keepUnknown);
  }
  if (pos < kVoltsFirst) return describeTemp(ctx, static_cast<uint8_t>(pos - kTempsFirst), e);
  if (pos < kTailFirst) return describeVolt(ctx, static_cast<uint8_t>(pos - kVoltsFirst), e);
  return describeTail(ctx, pos - kTailFirst, e);
}

void writeEntity(const DiscoveryContext& ctx, const char* station, const Entity& e,
                 JsonWriter& jw) {
  char uid[kStationNameMax + 1 + kIdMax + 1];
  Str(uid, sizeof uid).add(station).add(".").add(e.uid);
  for (char* p = uid; *p; ++p) {
    if (*p == ' ') *p = '_';
  }
  char status[kTopicMax + 1];
  buildTopic(ctx.topics, Topic::Status, nullptr, status, sizeof status);
  char ip[sizeof ctx.ip];
  copyField(ip, ctx.ip);
  char sw[sizeof ctx.swVersion];
  copyField(sw, ctx.swVersion);

  jw.beginObject();
  jw.kv("name", static_cast<const char*>(e.name));
  jw.kv("unique_id", static_cast<const char*>(uid));
  jw.kv("state_topic", static_cast<const char*>(e.state));
  if (e.command[0] != '\0') jw.kv("command_topic", static_cast<const char*>(e.command));
  if (e.valueTemplate != nullptr) jw.kv("value_template", e.valueTemplate);
  if (e.icon != nullptr) jw.kv("icon", e.icon);
  if (e.deviceClass != nullptr) jw.kv("device_class", e.deviceClass);
  if (e.stateClass != nullptr) jw.kv("state_class", e.stateClass);
  if (e.unit[0] != '\0') jw.kv("unit_of_measurement", static_cast<const char*>(e.unit));
  if (e.reportsPosition) jw.kv("reports_position", true);
  if (e.binary) {
    jw.kv("payload_on", "1");
    jw.kv("payload_off", "0");
  }
  if (e.diagnostic) jw.kv("entity_category", "diagnostic");
  jw.kv("availability_topic", static_cast<const char*>(status));
  jw.kv("payload_available", "online");
  jw.kv("payload_not_available", "offline");
  jw.key("device");
  jw.beginObject();
  jw.kv("identifiers", station);
  jw.kv("name", station);
  jw.kv("sw_version", static_cast<const char*>(sw));
  jw.kv("hw_version", "2.0");
  jw.kv("model", "VdMot Revamped");
  jw.kv("manufacturer", "Lenti84/Surfgargano");
  if (ip[0] != '\0') {
    char url[sizeof ip + 8];  // "http://" + ip + "/" + NUL
    Str(url, sizeof url).add("http://").add(ip).add("/");
    jw.kv("configuration_url", static_cast<const char*>(url));
  }
  jw.endObject();
  jw.endObject();
}

// Makes jw report failure (ok() false) for a message that cannot be built.
void poison(JsonWriter& jw) {
  jw.reset();
  jw.endObject();
}

}  // namespace

const char* haComponentName(HaComponent c) {
  switch (c) {
    case HaComponent::Sensor: return "sensor";
    case HaComponent::BinarySensor: return "binary_sensor";
    case HaComponent::Text: return "text";
    case HaComponent::Valve: return "valve";
    case HaComponent::Number: return "number";
    case HaComponent::Select: return "select";
    case HaComponent::Switch: return "switch";
    case HaComponent::Climate: return "climate";
  }
  return "";
}

// ---------------------------------------------------------------- DiscoveryIterator

DiscoveryIterator::DiscoveryIterator(const DiscoveryContext& ctx) : ctx_(ctx) {}

bool DiscoveryIterator::next(DiscoveryMessage& msg, JsonWriter& jw) {
  msg = DiscoveryMessage{};
  jw.reset();
  char station[kStationNameMax + 1];
  if (!stationOf(ctx_, station)) {
    pos_ = kEntityCount;
    return false;
  }
  while (pos_ < kEntityCount) {
    Entity e;
    const Describe d = describe(ctx_, pos_++, e);
    if (d == Describe::Skip) continue;
    if (d == Describe::Error || !buildConfigTopic(station, e.comp, e.objectId, msg)) {
      poison(jw);
      return false;
    }
    writeEntity(ctx_, station, e, jw);
    if (!jw.complete() || jw.length() > kDiscoveryPayloadMax) {
      poison(jw);
      return false;
    }
    return true;
  }
  return false;
}

void DiscoveryIterator::restart() { pos_ = 0; }

// ---------------------------------------------------------------- DropListIterator

DropListIterator::DropListIterator(const DiscoveryContext& ctx) : ctx_(ctx) {}

bool DropListIterator::next(DiscoveryMessage& msg) {
  msg = DiscoveryMessage{};
  char station[kStationNameMax + 1];
  if (!stationOf(ctx_, station)) {
    pos_ = kDropCount;
    return false;
  }
  while (pos_ < kDropCount) {
    const uint16_t pos = pos_++;
    if (pos < kDropGlobal) {
      const DropDef& d = kDropGlobals[pos];
      if (!buildConfigTopic(station, d.comp, d.prefix, msg)) continue;
      msg.remove = true;
      return true;
    }
    const uint16_t r = pos - kDropGlobal;
    const uint8_t valve = static_cast<uint8_t>(r / (2 * kDropKinds));
    const bool indexForm = (r / kDropKinds) % 2 == 1;
    const DropDef& d = kDropValve[r % kDropKinds];
    char number[4];
    snprintf(number, sizeof number, "%u", static_cast<unsigned>(valve) + 1u);
    char seg[kSegmentMax + 1];
    const bool named = segmentOf(ctx_.valves[valve].segment, seg);
    if (indexForm) {
      if (named && strcmp(seg, number) == 0) continue;  // already sent in the name form
      copyField(seg, number);
    } else if (!named) {
      continue;
    }
    char objectId[kIdMax + 1];
    Str oid(objectId, sizeof objectId);
    oid.add(d.prefix).add(seg);
    if (!oid.ok() || !buildConfigTopic(station, d.comp, objectId, msg)) continue;
    msg.remove = true;
    return true;
  }
  return false;
}

void DropListIterator::restart() { pos_ = 0; }

// ---------------------------------------------------------------- stale check

bool discoveryTopicIsCurrent(const DiscoveryContext& ctx, const char* topic, size_t len) {
  if (topic == nullptr) return false;
  while (len > 0 && (topic[len - 1] == '\r' || topic[len - 1] == '\n' || topic[len - 1] == ' ' ||
                     topic[len - 1] == '\t')) {
    --len;
  }
  char station[kStationNameMax + 1];
  if (!stationOf(ctx, station)) return false;
  DiscoveryMessage msg;
  for (uint16_t pos = 0; pos < kEntityCount; ++pos) {
    Entity e;
    if (describe(ctx, pos, e, true) != Describe::Ok) continue;
    if (!buildConfigTopic(station, e.comp, e.objectId, msg)) continue;
    if (strlen(msg.topic) == len && memcmp(msg.topic, topic, len) == 0) return true;
  }
  return false;
}

}  // namespace vdm
