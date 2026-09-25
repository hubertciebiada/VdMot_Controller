#include "vdm/ha_discovery.h"

#include <stdio.h>
#include <string.h>

#include <initializer_list>

#include "vdm/event_log.h"
#include "vdm/json_writer.h"
#include "vdm/mqtt_values.h"

namespace vdm {

namespace {

// ---------------------------------------------------------------- layout

constexpr uint16_t kCommonCount = 4;
constexpr uint16_t kValveKinds = 20;
constexpr uint16_t kValvesFirst = kCommonCount;
constexpr uint16_t kTempsFirst = kValvesFirst + kValveCount * kValveKinds;
constexpr uint16_t kVoltsFirst = kTempsFirst + kTempSlotCount;
constexpr uint16_t kTailFirst = kVoltsFirst + kVoltSlotCount;
constexpr uint16_t kTailCount = 23;
constexpr uint16_t kEntityCount = kTailFirst + kTailCount;
static_assert(kEntityCount == 309, "entity count (DESIGN.md)");

constexpr uint16_t kDropGlobal = 2;
constexpr uint16_t kDropKinds = 7;
constexpr uint16_t kDropCount = kDropGlobal + kValveCount * 2 * kDropKinds;

constexpr size_t kIdMax = 47;
constexpr size_t kEventTypesMax = 128;
constexpr char kLegacyPrefix[] = "homeassistant";

enum class Kind : uint8_t {
  Text,       // legacy read-only value exposed as text: command "<base>/set"
  Valve,      // target: HA valve entity
  Temp,       // temperature sensor (template, expire_after)
  Actual,     // position sensor, %
  Counter,    // v2 diag counter
  LastStop,   // v2 last move stop reason (enum)
  Plain,      // plain sensor
  Problem,    // binary sensor "1"/"0", device class problem
  Failsafe,   // enum off/lease/blocked
  Sync,       // enum of targetSyncName()
  Button,     // command only
};

enum class Gate : uint8_t { Always, Temp1, Temp2, Diag, NewDiag };

enum class Avail : uint8_t { None, Esp, EspStm };

// Object-id / name / unique_id style of per-valve entities.
enum class Style : uint8_t {
  Legacy,  // objectId valves_<key>_<Rid>, name = uid = valves.<R>.<dotted>
  Diag,    // objectId diag_<key>_<Rid>, uid diag.<R>.<key>, name "<V> <label>"
  New,     // objectId valves_<key>_<Rid>, uid valves.<R>.<key>, name "<V> <label>"
};

struct ValveDef {
  HaComponent comp;
  Kind kind;
  Gate gate;
  Style style;
  Avail avail;
  const char* key;     // object id part
  const char* dotted;  // legacy name / unique_id part, or the readable label
  Topic topic;         // state topic (command topic for buttons)
  const char* icon;
};

// Order is the discovery order (DESIGN.md "HA discovery").
const ValveDef kValveDefs[kValveKinds] = {
    {HaComponent::Text, Kind::Text, Gate::Always, Style::Legacy, Avail::None, "state", "state", Topic::ValveState, "mdi:state-machine"},
    {HaComponent::Valve, Kind::Valve, Gate::Always, Style::Legacy, Avail::None, "target", "target", Topic::ValveTarget, "mdi:valve"},
    {HaComponent::Sensor, Kind::Actual, Gate::Always, Style::New, Avail::EspStm, "actual", "position", Topic::ValveActual, "mdi:valve"},
    {HaComponent::Sensor, Kind::Temp, Gate::Temp1, Style::Legacy, Avail::None, "temp1", "temp1", Topic::ValveTemp1, "mdi:thermometer"},
    {HaComponent::Sensor, Kind::Temp, Gate::Temp2, Style::Legacy, Avail::None, "temp2", "temp2", Topic::ValveTemp2, "mdi:thermometer"},
    {HaComponent::Text, Kind::Text, Gate::Always, Style::Legacy, Avail::None, "calibration_date", "calibration.date", Topic::ValveCalibDate, "mdi:timelapse"},
    {HaComponent::Text, Kind::Text, Gate::Always, Style::Legacy, Avail::None, "calibration_repetitions", "calibration.repetitions", Topic::ValveCalibRepetitions, "mdi:valve"},
    {HaComponent::Text, Kind::Text, Gate::Diag, Style::Legacy, Avail::None, "diag_openCount", "diag.openCount", Topic::ValveOpenCount, "mdi:valve"},
    {HaComponent::Text, Kind::Text, Gate::Diag, Style::Legacy, Avail::None, "diag_closeCount", "diag.closeCount", Topic::ValveCloseCount, "mdi:valve"},
    {HaComponent::Text, Kind::Text, Gate::Diag, Style::Legacy, Avail::None, "diag_deadZoneCount", "diag.deadZoneCount", Topic::ValveDeadZoneCount, "mdi:valve"},
    {HaComponent::Text, Kind::Text, Gate::Diag, Style::Legacy, Avail::None, "diag_moves", "diag.moves", Topic::ValveMoves, "mdi:valve"},
    {HaComponent::Text, Kind::Text, Gate::Diag, Style::Legacy, Avail::None, "diag_meanCurrrent", "diag.meanCurrrent", Topic::ValveMeanCurrent, "mdi:valve"},
    {HaComponent::Sensor, Kind::Counter, Gate::NewDiag, Style::Diag, Avail::EspStm, "earlyStops", "early stops", Topic::DiagValveEarlyStops, "mdi:alert-outline"},
    {HaComponent::Sensor, Kind::Counter, Gate::NewDiag, Style::Diag, Avail::EspStm, "cmdRejected", "rejected commands", Topic::DiagValveCmdRejected, "mdi:alert-outline"},
    {HaComponent::Sensor, Kind::LastStop, Gate::NewDiag, Style::Diag, Avail::EspStm, "lastStop", "last stop", Topic::DiagValveLastMove, "mdi:stop-circle-outline"},
    {HaComponent::Sensor, Kind::Plain, Gate::NewDiag, Style::Diag, Avail::EspStm, "calState", "calibration state", Topic::DiagValveCalState, "mdi:progress-wrench"},
    {HaComponent::BinarySensor, Kind::Problem, Gate::Always, Style::New, Avail::Esp, "problem", "problem", Topic::ValveProblem, nullptr},
    {HaComponent::Sensor, Kind::Failsafe, Gate::Always, Style::New, Avail::EspStm, "failsafe", "failsafe", Topic::ValveFailsafe, "mdi:shield-alert-outline"},
    {HaComponent::Sensor, Kind::Sync, Gate::Always, Style::New, Avail::Esp, "sync", "target delivery", Topic::ValveSync, nullptr},
    {HaComponent::Button, Kind::Button, Gate::Always, Style::New, Avail::EspStm, "calibrate", "calibrate", Topic::CmdValveCalibrate, "mdi:tune-vertical"},
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

enum class TailGate : uint8_t { Always, NewDiag, Events, StmV3 };
enum class Category : uint8_t { None, Diagnostic, Config };
enum class Payloads : uint8_t { None, OneZero, OnlineOffline };

const char* const kLeaseOptions[] = {"off", "running", "expired"};
const char* const kLinkOptions[] = {"unknown", "up", "degraded", "down", "booting", "suspended"};
const char* const kStopOptions[] = {"none", "target", "endstop", "early_endstop", "timeout",
                                    "undercurrent", "safety_overcurrent", "aborted"};
const char* const kFailsafeOptions[] = {"off", "lease", "blocked"};
const char* const kSyncOptions[] = {"unknown", "synced", "pending", "await_ack", "await_verify",
                                    "failed"};

struct Options {
  const char* const* list;
  uint8_t count;
};

struct TailDef {
  HaComponent comp;
  const char* objectId;
  const char* name;
  const char* uid;
  Topic topic;             // state topic, command topic for buttons
  Avail avail;
  TailGate gate;
  const char* deviceClass;
  const char* stateClass;
  const char* icon;
  Category category;
  Payloads payloads;
  Options options;
};

constexpr Options kNoOptions = {nullptr, 0};

const TailDef kTailDefs[kTailCount] = {
    {HaComponent::BinarySensor, "diag_esp_online", "ESP online", "diag.esp.online", Topic::Status, Avail::None, TailGate::Always, "connectivity", nullptr, nullptr, Category::Diagnostic, Payloads::OnlineOffline, kNoOptions},
    {HaComponent::BinarySensor, "diag_stm_online", "STM link", "diag.stm.online", Topic::StmStatus, Avail::Esp, TailGate::Always, "connectivity", nullptr, nullptr, Category::Diagnostic, Payloads::OnlineOffline, kNoOptions},
    {HaComponent::BinarySensor, "diag_failsafe", "Failsafe active", "diag.failsafe", Topic::Failsafe, Avail::EspStm, TailGate::Always, "problem", nullptr, nullptr, Category::None, Payloads::OneZero, kNoOptions},
    {HaComponent::Sensor, "diag_stm_lease", "Lease", "diag.stm.lease", Topic::DiagStmLease, Avail::EspStm, TailGate::NewDiag, "enum", nullptr, nullptr, Category::Diagnostic, Payloads::None, {kLeaseOptions, 3}},
    {HaComponent::BinarySensor, "diag_stm_safeMode", "STM safe mode", "diag.stm.safeMode", Topic::DiagStmSafeMode, Avail::EspStm, TailGate::NewDiag, "problem", nullptr, nullptr, Category::Diagnostic, Payloads::OneZero, kNoOptions},
    {HaComponent::Sensor, "diag_stm_link", "STM link state", "diag.stm.link", Topic::DiagStmLink, Avail::Esp, TailGate::NewDiag, "enum", nullptr, "mdi:lan-connect", Category::Diagnostic, Payloads::None, {kLinkOptions, 6}},
    {HaComponent::Sensor, "diag_stm_proto", "STM protocol", "diag.stm.proto", Topic::DiagStmProto, Avail::EspStm, TailGate::NewDiag, nullptr, nullptr, nullptr, Category::Diagnostic, Payloads::None, kNoOptions},
    {HaComponent::Sensor, "diag_stm_version", "STM firmware", "diag.stm.version", Topic::DiagStmVersion, Avail::EspStm, TailGate::NewDiag, nullptr, nullptr, "mdi:chip", Category::Diagnostic, Payloads::None, kNoOptions},
    {HaComponent::Sensor, "diag_stm_started", "STM started", "diag.stm.started", Topic::DiagStmStarted, Avail::EspStm, TailGate::NewDiag, "timestamp", nullptr, nullptr, Category::Diagnostic, Payloads::None, kNoOptions},
    {HaComponent::Sensor, "diag_stm_resets", "STM resets", "diag.stm.resets", Topic::DiagStmResets, Avail::EspStm, TailGate::NewDiag, nullptr, "total_increasing", nullptr, Category::Diagnostic, Payloads::None, kNoOptions},
    {HaComponent::Sensor, "diag_stm_rxOverflow", "STM receive overflows", "diag.stm.rxOverflow", Topic::DiagStmRxOverflow, Avail::EspStm, TailGate::NewDiag, nullptr, "total_increasing", nullptr, Category::Diagnostic, Payloads::None, kNoOptions},
    {HaComponent::Sensor, "diag_stm_parseErr", "STM parse errors", "diag.stm.parseErr", Topic::DiagStmParseErr, Avail::EspStm, TailGate::NewDiag, nullptr, "total_increasing", nullptr, Category::Diagnostic, Payloads::None, kNoOptions},
    {HaComponent::BinarySensor, "diag_calibration_active", "Calibration running", "diag.calibration.active", Topic::DiagCalibrationActive, Avail::EspStm, TailGate::NewDiag, "running", nullptr, nullptr, Category::Diagnostic, Payloads::OneZero, kNoOptions},
    {HaComponent::Sensor, "diag_calibration_next", "Next calibration", "diag.calibration.next", Topic::DiagCalibrationNext, Avail::Esp, TailGate::NewDiag, "timestamp", nullptr, nullptr, Category::None, Payloads::None, kNoOptions},
    {HaComponent::Sensor, "diag_mqtt_eventsSuppressed", "Suppressed events", "diag.mqtt.eventsSuppressed", Topic::DiagMqttEventsSuppressed, Avail::Esp, TailGate::NewDiag, nullptr, "total_increasing", nullptr, Category::Diagnostic, Payloads::None, kNoOptions},
    {HaComponent::Sensor, "diag_mqtt_commandsRejected", "Rejected MQTT commands", "diag.mqtt.commandsRejected", Topic::DiagMqttCommandsRejected, Avail::Esp, TailGate::NewDiag, nullptr, "total_increasing", nullptr, Category::Diagnostic, Payloads::None, kNoOptions},
    {HaComponent::Event, "events", "Events", "events", Topic::Events, Avail::Esp, TailGate::Events, nullptr, nullptr, "mdi:bell-alert-outline", Category::None, Payloads::None, kNoOptions},
    {HaComponent::Button, "cmd_calibrate_all", "Calibrate all valves", "cmd.calibrate", Topic::CmdCalibrate, Avail::EspStm, TailGate::Always, nullptr, nullptr, nullptr, Category::Config, Payloads::None, kNoOptions},
    {HaComponent::Button, "cmd_detect", "Detect valves", "cmd.detect", Topic::CmdDetect, Avail::EspStm, TailGate::Always, nullptr, nullptr, nullptr, Category::Config, Payloads::None, kNoOptions},
    {HaComponent::Button, "cmd_stop", "Stop valves", "cmd.stop", Topic::CmdStop, Avail::EspStm, TailGate::StmV3, nullptr, nullptr, nullptr, Category::Config, Payloads::None, kNoOptions},
    {HaComponent::Button, "cmd_stm_reset", "Reset STM", "cmd.stmReset", Topic::CmdStmReset, Avail::Esp, TailGate::Always, "restart", nullptr, nullptr, Category::Config, Payloads::None, kNoOptions},
    {HaComponent::Button, "cmd_esp_restart", "Restart ESP", "cmd.restart", Topic::CmdRestart, Avail::Esp, TailGate::Always, "restart", nullptr, nullptr, Category::Config, Payloads::None, kNoOptions},
    {HaComponent::Button, "cmd_stm_safe_exit", "Leave STM safe mode", "cmd.stmSafeExit", Topic::CmdStmSafeExit, Avail::EspStm, TailGate::StmV3, nullptr, nullptr, nullptr, Category::Config, Payloads::None, kNoOptions},
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

constexpr char kTempTemplate[] = "{{ value | replace(',', '.') | float(None) }}";

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

// The station as it appears in names, unique_ids and the device block:
// non-empty and safe (isSafeName also bounds its length), else discovery is off.
bool stationOf(const DiscoveryContext& ctx, char (&out)[kStationNameMax + 1]) {
  const char* s = ctx.station[0] != '\0' ? ctx.station : ctx.topics.station;
  if (!isSafeName(s, kStationNameMax, false)) return false;
  copyField(out, s);
  return true;
}

// Node id: buildHaId(station).
bool nodeOf(const DiscoveryContext& ctx, char (&out)[kStationNameMax + 1]) {
  char station[kStationNameMax + 1];
  if (!stationOf(ctx, station)) return false;
  return buildHaId(station, kStationNameMax, out, sizeof out) > 0;
}

// Levels of [A-Za-z0-9_-] separated by single '/' (config TopicPath).
bool topicPath(const char* s) {
  if (s[0] == '\0') return false;
  bool levelEmpty = true;
  for (const char* p = s; *p; ++p) {
    const char c = *p;
    if (c == '/') {
      if (levelEmpty) return false;
      levelEmpty = true;
      continue;
    }
    const bool ok = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') ||
                    c == '_' || c == '-';
    if (!ok) return false;
    levelEmpty = false;
  }
  return !levelEmpty;
}

// Discovery prefix: a TopicPath of 1..32 chars, else the legacy prefix.
void prefixOf(const DiscoveryContext& ctx, char (&out)[kTopicPrefixMax + 1]) {
  copyField(out, ctx.discoveryPrefix);
  if (!topicPath(out)) copyField(out, kLegacyPrefix);
}

// A valve/sensor segment as filled by glue: must be a valid topic segment
// without space, '"' and '\\'.
bool segmentOf(const char* src, char (&out)[kSegmentMax + 1]) {
  if (boundedLength(src, kSegmentMax + 1) > kSegmentMax) return false;
  copyField(out, src);
  if (!topicSegmentValid(out, strlen(out))) return false;
  for (const char* p = out; *p; ++p) {
    if (*p == ' ' || *p == '"' || *p == '\\') return false;
  }
  return true;
}

bool buildConfigTopic(const char* prefix, const char* node, HaComponent comp, const char* objectId,
                      DiscoveryMessage& msg) {
  Str s(msg.topic, sizeof msg.topic);
  s.add(prefix).add("/").add(haComponentName(comp)).add("/").add(node).add("/");
  s.add(objectId).add("/config");
  return s.ok();
}

// ---------------------------------------------------------------- entities

struct Entity {
  HaComponent comp = HaComponent::Sensor;
  char objectId[kIdMax + 1] = {};     // with buildHaId() segments
  char objectIdRaw[kIdMax + 1] = {};  // the same with raw segments (2.0.0 form)
  char name[kIdMax + 1] = {};
  char uid[kIdMax + 1] = {};  // without the "<station>." prefix
  char state[kTopicMax + 1] = {};
  char command[kTopicMax + 1] = {};
  char unit[kUnitMax + 1] = {};
  const char* icon = nullptr;
  const char* deviceClass = nullptr;
  const char* stateClass = nullptr;
  const char* valueTemplate = nullptr;
  Options options = kNoOptions;
  uint32_t expireAfterS = 0;
  bool reportsPosition = false;
  bool qos1 = false;
  bool payloadStop = false;
  Payloads payloads = Payloads::None;
  Category category = Category::None;
  bool eventTypes = false;
  Avail avail = Avail::None;
  bool kept = false;  // described only to keep its config (KeptUnknown)
};

enum class Describe : uint8_t { Skip, Ok, Error };

uint32_t expireAfter(const DiscoveryContext& ctx) {
  const uint32_t e = 3u * ctx.publishIntervalS;
  return e < 60 ? 60 : e;
}

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
  Str(e.objectIdRaw, sizeof e.objectIdRaw).add(d.key);
  Str(e.name, sizeof e.name).add(d.key);
  Str(e.uid, sizeof e.uid).add("common.").add(d.key);
  if (!stateTopic(ctx, d.topic, nullptr, e) || !textCommand(ctx, d.topic, nullptr, e)) {
    return Describe::Error;
  }
  return Describe::Ok;
}

// `keep`: also describe the temp entities of a valve whose sensors are not
// known yet (classification only; never published on a guess).
Describe describeValve(const DiscoveryContext& ctx, uint8_t v, uint16_t k, Entity& e, bool keep) {
  const DiscoveryContext::Valve& valve = ctx.valves[v];
  const ValveDef& d = kValveDefs[k];
  if (!valve.active) return Describe::Skip;
  switch (d.gate) {
    case Gate::Always: break;
    case Gate::Temp1:
    case Gate::Temp2: {
      const bool has = d.gate == Gate::Temp1 ? valve.hasTemp1 : valve.hasTemp2;
      if (has) break;
      if (!keep || valve.tempsKnown) return Describe::Skip;
      e.kept = true;
      break;
    }
    case Gate::Diag: if (!ctx.publishDiag) return Describe::Skip; break;
    case Gate::NewDiag: if (!ctx.newDiag) return Describe::Skip; break;
  }
  char seg[kSegmentMax + 1];
  if (!segmentOf(valve.segment, seg)) return Describe::Error;

  e.comp = d.comp;
  e.icon = d.icon;
  e.avail = d.avail;
  Str name(e.name, sizeof e.name);
  Str uid(e.uid, sizeof e.uid);
  if (d.style == Style::Legacy) {
    name.add("valves.").add(seg).add(".").add(d.dotted);
    uid.add(e.name);
  } else {
    char display[kItemNameMax + 7];  // "Valve 12"
    char vname[kItemNameMax + 1];
    copyField(vname, valve.name);
    valveDisplayName(vname, v, display, sizeof display);
    name.add(display).add(" ").add(d.dotted);
    uid.add(d.style == Style::Diag ? "diag." : "valves.").add(seg).add(".").add(d.key);
  }
  // Object ids "<head><key>_<id>": the id from buildHaId(), the raw segment
  // in the 2.0.0 form.
  {
    char id[kSegmentMax + 1];
    if (buildHaId(seg, kSegmentMax, id, sizeof id) == 0) return Describe::Error;
    const char* head = d.style == Style::Diag ? "diag_" : "valves_";
    Str oid(e.objectId, sizeof e.objectId);
    oid.add(head).add(d.key).add("_").add(id);
    Str raw(e.objectIdRaw, sizeof e.objectIdRaw);
    raw.add(head).add(d.key).add("_").add(seg);
    if (!oid.ok() || !raw.ok()) return Describe::Error;
  }
  if (!name.ok() || !uid.ok()) return Describe::Error;
  if (d.kind == Kind::Button) {
    if (buildTopic(ctx.topics, d.topic, seg, e.command, sizeof e.command) == 0) {
      return Describe::Error;
    }
    e.category = Category::Config;
    return Describe::Ok;
  }
  if (!stateTopic(ctx, d.topic, seg, e)) return Describe::Error;

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
      e.qos1 = true;
      e.payloadStop = ctx.stmV3;
      break;
    case Kind::Temp:
      e.valueTemplate = kTempTemplate;
      e.deviceClass = "temperature";
      e.stateClass = "measurement";
      e.expireAfterS = expireAfter(ctx);
      Str(e.unit, sizeof e.unit).add("\xC2\xB0" "C");
      break;
    case Kind::Actual:
      e.stateClass = "measurement";
      Str(e.unit, sizeof e.unit).add("%");
      break;
    case Kind::Counter:
      e.stateClass = "total_increasing";
      e.category = Category::Diagnostic;
      break;
    case Kind::LastStop:
      e.valueTemplate = "{{ value_json.stop }}";
      e.deviceClass = "enum";
      e.options = {kStopOptions, 8};
      e.category = Category::Diagnostic;
      break;
    case Kind::Plain:
      e.category = Category::Diagnostic;
      break;
    case Kind::Problem:
      e.deviceClass = "problem";
      e.payloads = Payloads::OneZero;
      break;
    case Kind::Failsafe:
      e.deviceClass = "enum";
      e.options = {kFailsafeOptions, 3};
      break;
    case Kind::Sync:
      e.deviceClass = "enum";
      e.options = {kSyncOptions, 6};
      e.category = Category::Diagnostic;
      break;
    case Kind::Button:
      break;
  }
  return Describe::Ok;
}

// Temp and volt sensors: object id from the slot segment (legacy), state
// topic from the published segment (E22).
Describe describeSensor(const DiscoveryContext& ctx, const DiscoveryContext::Sensor& s, bool temp,
                        Entity& e, bool keep) {
  if (!s.active || (temp && !s.published) || s.id[0] == '\0') return Describe::Skip;
  if (!s.topicKnown) {
    if (!keep) return Describe::Skip;
    e.kept = true;
  }
  char seg[kSegmentMax + 1];
  if (!segmentOf(s.segment, seg)) return Describe::Error;
  e.comp = HaComponent::Sensor;
  e.stateClass = "measurement";
  e.valueTemplate = kTempTemplate;
  e.expireAfterS = expireAfter(ctx);
  Str name(e.name, sizeof e.name);
  if (temp) {
    e.icon = "mdi:thermometer";
    e.deviceClass = "temperature";
    Str(e.unit, sizeof e.unit).add("\xC2\xB0" "C");
    name.add("temps.").add(seg);
  } else {
    copyField(e.unit, s.unit);
    if (strcmp(e.unit, "V") == 0 || strcmp(e.unit, "mV") == 0) e.deviceClass = "voltage";
    // Legacy: the raw configured name (spaces kept, "volts." when unnamed).
    name.add("volts.").add(s.name, kItemNameMax);
  }
  char id[kSegmentMax + 1];
  if (buildHaId(seg, kSegmentMax, id, sizeof id) == 0) return Describe::Error;
  const char* head = temp ? "temps_" : "volts_";
  Str oid(e.objectId, sizeof e.objectId);
  oid.add(head).add(id);
  Str raw(e.objectIdRaw, sizeof e.objectIdRaw);
  raw.add(head).add(seg);
  Str uid(e.uid, sizeof e.uid);
  uid.add(s.id, kOneWireIdTextLen);
  if (!oid.ok() || !raw.ok() || !name.ok() || !uid.ok()) return Describe::Error;
  if (e.kept) return Describe::Ok;
  char pub[kSegmentMax + 1];
  if (!segmentOf(s.topicSegment, pub)) return Describe::Error;
  if (!stateTopic(ctx, temp ? Topic::TempValue : Topic::VoltValue, pub, e)) return Describe::Error;
  return Describe::Ok;
}

Describe describeTail(const DiscoveryContext& ctx, uint16_t k, Entity& e) {
  const TailDef& d = kTailDefs[k];
  switch (d.gate) {
    case TailGate::Always: break;
    case TailGate::NewDiag: if (!ctx.newDiag) return Describe::Skip; break;
    case TailGate::Events: if (!ctx.events) return Describe::Skip; break;
    case TailGate::StmV3: if (!ctx.stmV3) return Describe::Skip; break;
  }
  e.comp = d.comp;
  e.icon = d.icon;
  e.deviceClass = d.deviceClass;
  e.stateClass = d.stateClass;
  e.category = d.category;
  e.payloads = d.payloads;
  e.options = d.options;
  e.avail = d.avail;
  e.eventTypes = d.comp == HaComponent::Event;
  Str(e.objectId, sizeof e.objectId).add(d.objectId);
  Str(e.objectIdRaw, sizeof e.objectIdRaw).add(d.objectId);
  Str(e.name, sizeof e.name).add(d.name);
  Str(e.uid, sizeof e.uid).add(d.uid);
  const bool ok = d.comp == HaComponent::Button
                      ? buildTopic(ctx.topics, d.topic, nullptr, e.command, sizeof e.command) > 0
                      : stateTopic(ctx, d.topic, nullptr, e);
  return ok ? Describe::Ok : Describe::Error;
}

Describe describe(const DiscoveryContext& ctx, uint16_t pos, Entity& e, bool keep = false) {
  if (pos < kValvesFirst) return describeCommon(ctx, pos, e);
  if (pos < kTempsFirst) {
    const uint16_t r = pos - kValvesFirst;
    return describeValve(ctx, static_cast<uint8_t>(r / kValveKinds), r % kValveKinds, e, keep);
  }
  if (pos < kVoltsFirst) return describeSensor(ctx, ctx.temps[pos - kTempsFirst], true, e, keep);
  if (pos < kTailFirst) return describeSensor(ctx, ctx.volts[pos - kVoltsFirst], false, e, keep);
  return describeTail(ctx, pos - kTailFirst, e);
}

// Makes jw report failure (ok() false) for a message that cannot be built.
void poison(JsonWriter& jw) {
  jw.reset();
  jw.endObject();
}

void writeAvailability(const DiscoveryContext& ctx, Avail a, JsonWriter& jw) {
  if (a == Avail::None) return;
  char topic[kTopicMax + 1];
  jw.key("availability");
  jw.beginArray();
  buildTopic(ctx.topics, Topic::Status, nullptr, topic, sizeof topic);
  jw.beginObject();
  jw.kv("topic", static_cast<const char*>(topic));
  jw.endObject();
  if (a == Avail::EspStm) {
    buildTopic(ctx.topics, Topic::StmStatus, nullptr, topic, sizeof topic);
    jw.beginObject();
    jw.kv("topic", static_cast<const char*>(topic));
    jw.endObject();
  }
  jw.endArray();
  if (a == Avail::EspStm) jw.kv("availability_mode", "all");
}

void writeEntity(const DiscoveryContext& ctx, const char* station, const Entity& e,
                 JsonWriter& jw) {
  char uid[kStationNameMax + 1 + kIdMax + 1];
  Str(uid, sizeof uid).add(station).add(".").add(e.uid);
  for (char* p = uid; *p; ++p) {
    if (*p == ' ') *p = '_';
  }
  char ip[sizeof ctx.ip];
  copyField(ip, ctx.ip);
  char sw[sizeof ctx.swVersion];
  copyField(sw, ctx.swVersion);
  char hw[sizeof ctx.hwVersion];
  copyField(hw, ctx.hwVersion);

  jw.beginObject();
  jw.kv("name", static_cast<const char*>(e.name));
  jw.kv("unique_id", static_cast<const char*>(uid));
  if (e.state[0] != '\0') jw.kv("state_topic", static_cast<const char*>(e.state));
  if (e.command[0] != '\0') jw.kv("command_topic", static_cast<const char*>(e.command));
  if (e.valueTemplate != nullptr) jw.kv("value_template", e.valueTemplate);
  if (e.icon != nullptr) jw.kv("icon", e.icon);
  if (e.deviceClass != nullptr) jw.kv("device_class", e.deviceClass);
  if (e.stateClass != nullptr) jw.kv("state_class", e.stateClass);
  if (e.unit[0] != '\0') jw.kv("unit_of_measurement", static_cast<const char*>(e.unit));
  if (e.options.count > 0) {
    jw.key("options");
    jw.beginArray();
    for (uint8_t i = 0; i < e.options.count; ++i) jw.value(e.options.list[i]);
    jw.endArray();
  }
  if (e.expireAfterS > 0) jw.kv("expire_after", e.expireAfterS);
  if (e.reportsPosition) jw.kv("reports_position", true);
  if (e.qos1) jw.kv("qos", static_cast<uint32_t>(1));
  if (e.payloadStop) jw.kv("payload_stop", "STOP");
  if (e.payloads == Payloads::OneZero) {
    jw.kv("payload_on", "1");
    jw.kv("payload_off", "0");
  } else if (e.payloads == Payloads::OnlineOffline) {
    jw.kv("payload_on", "online");
    jw.kv("payload_off", "offline");
  }
  if (e.category == Category::Diagnostic) jw.kv("entity_category", "diagnostic");
  if (e.category == Category::Config) jw.kv("entity_category", "config");
  if (e.eventTypes) {
    static const char* names[kEventTypesMax];  // the MQTT task only: kept off its stack
    const size_t n = eventMqttNames(names, kEventTypesMax);
    jw.key("event_types");
    jw.beginArray();
    for (size_t i = 0; i < n && i < kEventTypesMax; ++i) jw.value(names[i]);
    jw.endArray();
    if (n > kEventTypesMax) poison(jw);  // never a partial list
  }
  writeAvailability(ctx, e.avail, jw);
  jw.key("device");
  jw.beginObject();
  jw.kv("identifiers", station);
  jw.kv("name", station);
  jw.kv("sw_version", static_cast<const char*>(sw));
  if (hw[0] != '\0') jw.kv("hw_version", static_cast<const char*>(hw));
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

bool isControl(char c) { return static_cast<unsigned char>(c) < 0x20 || c == 0x7F; }

// "<p>/<component>/<id>/<id>/config" with p = the legacy or the configured
// prefix, no wildcards or control characters.
bool configTopicShape(const DiscoveryContext& ctx, const char* t, size_t len) {
  if (len == 0 || len > kDiscoveryTopicMax) return false;
  for (size_t i = 0; i < len; ++i) {
    if (t[i] == '+' || t[i] == '#' || isControl(t[i])) return false;
  }
  char prefix[kTopicPrefixMax + 1];
  prefixOf(ctx, prefix);
  size_t pl = 0;
  for (const char* p : {kLegacyPrefix, static_cast<const char*>(prefix)}) {
    const size_t n = strlen(p);
    if (len > n && memcmp(t, p, n) == 0 && t[n] == '/') pl = n + 1;
  }
  if (pl == 0) return false;
  // Exactly three non-empty levels, then "config".
  size_t levels = 0;
  size_t start = pl;
  for (size_t i = pl; i <= len; ++i) {
    if (i < len && t[i] != '/') continue;
    if (i == start) return false;  // empty level
    ++levels;
    if (levels == 4) return i == len && i - start == 6 && memcmp(t + start, "config", 6) == 0;
    start = i + 1;
  }
  return false;
}

const DiscoveryContext& emptyContext() {
  static const DiscoveryContext kEmpty;
  return kEmpty;
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
    case HaComponent::Button: return "button";
    case HaComponent::Event: return "event";
  }
  return "";
}

// ---------------------------------------------------------------- DiscoveryIterator

DiscoveryIterator::DiscoveryIterator(const DiscoveryContext& ctx) : ctx_(&ctx) {}

bool DiscoveryIterator::next(DiscoveryMessage& msg, JsonWriter& jw) {
  msg = DiscoveryMessage{};
  jw.reset();
  char station[kStationNameMax + 1];
  char node[kStationNameMax + 1];
  if (!stationOf(*ctx_, station) || !nodeOf(*ctx_, node)) {
    pos_ = kEntityCount;
    return false;
  }
  char prefix[kTopicPrefixMax + 1];
  prefixOf(*ctx_, prefix);
  while (pos_ < kEntityCount) {
    Entity e;
    const Describe d = describe(*ctx_, pos_++, e);
    if (d == Describe::Skip) continue;
    if (d == Describe::Error || !buildConfigTopic(prefix, node, e.comp, e.objectId, msg)) {
      msg = DiscoveryMessage{};
      poison(jw);
      return false;
    }
    writeEntity(*ctx_, station, e, jw);
    if (!jw.complete() || jw.length() > kDiscoveryPayloadMax) {
      poison(jw);
      return false;
    }
    return true;
  }
  return false;
}

bool DiscoveryIterator::nextTopic(DiscoveryMessage& msg) {
  msg = DiscoveryMessage{};
  char node[kStationNameMax + 1];
  if (!nodeOf(*ctx_, node)) {
    pos_ = kEntityCount;
    return false;
  }
  char prefix[kTopicPrefixMax + 1];
  prefixOf(*ctx_, prefix);
  while (pos_ < kEntityCount) {
    Entity e;
    if (describe(*ctx_, pos_++, e) != Describe::Ok) continue;
    if (buildConfigTopic(prefix, node, e.comp, e.objectId, msg)) return true;
  }
  msg = DiscoveryMessage{};
  return false;
}

bool DiscoveryIterator::v20Topic(DiscoveryMessage& msg) const {
  msg = DiscoveryMessage{};
  if (pos_ == 0) return false;
  char station[kStationNameMax + 1];
  char node[kStationNameMax + 1];
  if (!stationOf(*ctx_, station) || !nodeOf(*ctx_, node)) return false;
  Entity e;
  if (describe(*ctx_, static_cast<uint16_t>(pos_ - 1), e) != Describe::Ok) return false;
  char prefix[kTopicPrefixMax + 1];
  prefixOf(*ctx_, prefix);
  DiscoveryMessage cur;
  if (!buildConfigTopic(prefix, node, e.comp, e.objectId, cur) ||
      !buildConfigTopic(kLegacyPrefix, station, e.comp, e.objectIdRaw, msg)) {
    msg = DiscoveryMessage{};
    return false;
  }
  if (strcmp(cur.topic, msg.topic) != 0) return true;
  msg = DiscoveryMessage{};
  return false;
}

void DiscoveryIterator::restart() { pos_ = 0; }

void DiscoveryIterator::reset(const DiscoveryContext& ctx) {
  ctx_ = &ctx;
  pos_ = 0;
}

// ---------------------------------------------------------------- DropListIterator

DropListIterator::DropListIterator(const DiscoveryContext& ctx) : ctx_(&ctx) {}

bool DropListIterator::next(DiscoveryMessage& msg) {
  msg = DiscoveryMessage{};
  char station[kStationNameMax + 1];
  if (!stationOf(*ctx_, station)) {
    pos_ = kDropCount;
    return false;
  }
  while (pos_ < kDropCount) {
    const uint16_t pos = pos_++;
    if (pos < kDropGlobal) {
      const DropDef& d = kDropGlobals[pos];
      if (!buildConfigTopic(kLegacyPrefix, station, d.comp, d.prefix, msg)) continue;
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
    const bool named = segmentOf(ctx_->valves[valve].segment, seg) && strchr(seg, '/') == nullptr;
    if (indexForm) {
      if (named && strcmp(seg, number) == 0) continue;  // already sent in the name form
      copyField(seg, number);
    } else if (!named) {
      continue;
    }
    char objectId[kIdMax + 1];
    Str oid(objectId, sizeof objectId);
    oid.add(d.prefix).add(seg);
    if (!oid.ok() || !buildConfigTopic(kLegacyPrefix, station, d.comp, objectId, msg)) continue;
    msg.remove = true;
    return true;
  }
  return false;
}

void DropListIterator::restart() { pos_ = 0; }

void DropListIterator::reset(const DiscoveryContext& ctx) {
  ctx_ = &ctx;
  pos_ = 0;
}

// ---------------------------------------------------------------- classification

TopicClass classifyDiscoveryTopic(const DiscoveryContext& ctx, const char* topic, size_t len) {
  if (topic == nullptr) return TopicClass::Foreign;
  while (len > 0 && (topic[len - 1] == '\r' || topic[len - 1] == '\n' || topic[len - 1] == ' ' ||
                     topic[len - 1] == '\t')) {
    --len;
  }
  if (!configTopicShape(ctx, topic, len)) return TopicClass::Foreign;
  char node[kStationNameMax + 1];
  if (!nodeOf(ctx, node)) return TopicClass::Stale;
  char prefix[kTopicPrefixMax + 1];
  prefixOf(ctx, prefix);
  DiscoveryMessage msg;
  for (uint16_t pos = 0; pos < kEntityCount; ++pos) {
    Entity e;
    if (describe(ctx, pos, e, true) != Describe::Ok) continue;
    if (!buildConfigTopic(prefix, node, e.comp, e.objectId, msg)) continue;
    if (strlen(msg.topic) == len && memcmp(msg.topic, topic, len) == 0) {
      return e.kept ? TopicClass::KeptUnknown : TopicClass::Current;
    }
  }
  return TopicClass::Stale;
}

// ---------------------------------------------------------------- context

bool buildDiscoveryContext(const DiscoveryInputs& in, DiscoveryContext& c) {
  c = DiscoveryContext{};
  if (in.cfg == nullptr) return false;
  const Config& cfg = *in.cfg;
  copyString(c.topics.station, sizeof c.topics.station, mqttRootTopic(cfg));
  c.topics.pathAsRoot = cfg.mqtt.pathAsRoot;
  c.topics.separate = cfg.mqtt.separate;
  copyString(c.station, sizeof c.station, cfg.station);
  copyString(c.discoveryPrefix, sizeof c.discoveryPrefix, cfg.mqtt.discoveryPrefix);
  c.plainText = cfg.mqtt.plainText;
  c.publishDiag = cfg.mqtt.diag;
  c.publishUptime = cfg.mqtt.upTime;
  c.publishAllTemps = cfg.mqtt.allTemps;
  c.newDiag = cfg.mqtt.newDiag;
  c.events = cfg.mqtt.events;
  c.publishIntervalS = cfg.mqtt.publishIntervalS;
  c.stmV3 = in.stmProto >= 3;
  if (in.ip != 0) formatIpv4(in.ip, c.ip, sizeof c.ip);
  copyString(c.swVersion, sizeof c.swVersion, in.swVersion);
  copyString(c.hwVersion, sizeof c.hwVersion, in.stmHw);
  for (uint8_t i = 0; i < kValveCount; ++i) {
    DiscoveryContext::Valve& v = c.valves[i];
    v.active = cfg.valves[i].active;
    itemSegment(cfg, ItemKind::Valve, i, v.segment, sizeof v.segment);
    copyString(v.name, sizeof v.name, cfg.valves[i].name);
    if (in.valves != nullptr) {
      v.hasTemp1 = in.valves[i].temp1 != kTempUnassigned;
      v.hasTemp2 = in.valves[i].temp2 != kTempUnassigned;
      v.tempsKnown = in.sensorsSettled && in.valves[i].known;
    } else {
      v.tempsKnown = false;
    }
  }
  for (uint8_t i = 0; i < kTempSlotCount; ++i) {
    const TempSlotConfig& s = cfg.temps[i];
    DiscoveryContext::Sensor& d = c.temps[i];
    d.active = s.active && !isZero(s.id);
    d.published = tempPublished(cfg, in.valves, i);
    itemSegment(cfg, ItemKind::Temp, i, d.segment, sizeof d.segment);
    d.topicKnown = sensorTopicSegment(cfg, ItemKind::Temp, i, findTempBus(in.temps, in.tempCount, s.id),
                                      d.topicSegment, sizeof d.topicSegment) > 0;
    copyString(d.name, sizeof d.name, s.name);
    if (!isZero(s.id)) formatOneWireId(s.id, d.id, sizeof d.id);
  }
  for (uint8_t i = 0; i < kVoltSlotCount; ++i) {
    const VoltSlotConfig& s = cfg.volts[i];
    DiscoveryContext::Sensor& d = c.volts[i];
    d.active = voltAnnounced(cfg, i);
    d.published = d.active;
    itemSegment(cfg, ItemKind::Volt, i, d.segment, sizeof d.segment);
    d.topicKnown = sensorTopicSegment(cfg, ItemKind::Volt, i, findVoltBus(in.volts, in.voltCount, s.id),
                                      d.topicSegment, sizeof d.topicSegment) > 0;
    copyString(d.name, sizeof d.name, s.name);
    if (!isZero(s.id)) formatOneWireId(s.id, d.id, sizeof d.id);
    copyString(d.unit, sizeof d.unit, s.unit);
  }
  return true;
}

uint32_t discoveryInputKey(const DiscoveryInputs& in) {
  static DiscoveryContext c;  // task-local use only (the MQTT task): too large for its stack
  if (!buildDiscoveryContext(in, c)) return 0;
  uint32_t crc = 0;
  for (const DiscoveryContext::Valve& v : c.valves) {
    const uint8_t b[3] = {v.hasTemp1, v.hasTemp2, v.tempsKnown};
    crc = crc32(b, sizeof b, crc);
  }
  for (const DiscoveryContext::Sensor* arr : {c.temps, c.volts}) {
    const uint8_t n = arr == c.temps ? kTempSlotCount : kVoltSlotCount;
    for (uint8_t i = 0; i < n; ++i) {
      const uint8_t b[2] = {arr[i].published, arr[i].topicKnown};
      crc = crc32(b, sizeof b, crc);
      crc = crc32(reinterpret_cast<const uint8_t*>(arr[i].topicSegment), sizeof arr[i].topicSegment, crc);
    }
  }
  crc = crc32(reinterpret_cast<const uint8_t*>(c.hwVersion), sizeof c.hwVersion, crc);
  const uint8_t v3 = c.stmV3;
  return crc32(&v3, 1, crc);
}

// ---------------------------------------------------------------- list file

bool readListLine(DiscoveryPort& port, char (&out)[kDiscoveryTopicMax + 1]) {
  size_t n = 0;
  bool overlong = false;
  for (;;) {
    const int c = port.listRead();
    if (c < 0 || c == '\n' || c == '\r') {
      if (n > 0 && !overlong) {
        out[n] = '\0';
        return true;
      }
      if (c < 0) {
        out[0] = '\0';
        return false;
      }
      n = 0;  // empty line, CRLF or the end of an overlong line
      overlong = false;
      continue;
    }
    if (n < kDiscoveryTopicMax) {
      out[n++] = static_cast<char>(c);
    } else {
      overlong = true;
    }
  }
}

// ---------------------------------------------------------------- DiscoveryRun

DiscoveryRun::DiscoveryRun() : it_(emptyContext()), drop_(emptyContext()) {}

void DiscoveryRun::start(const DiscoveryContext& ctx, const DiscoveryPlan& plan) {
  ctx_ = &ctx;
  plan_ = plan;
  it_.reset(ctx);
  drop_.reset(ctx);
  stats_ = Stats{};
  reading_ = writing_ = uptimeDone_ = changed_ = false;
  crcOld_ = crcNew_ = 0;
  linesOld_ = linesNew_ = 0;
  phase_ = Phase::Idle;
  advance();
}

bool DiscoveryRun::running() const {
  return phase_ != Phase::Idle && phase_ != Phase::Done && phase_ != Phase::Aborted;
}

void DiscoveryRun::enter(Phase p) {
  phase_ = p;
  entered_ = false;
  it_.restart();
}

// The next phase the plan asks for after the current one.
void DiscoveryRun::advance() {
  switch (phase_) {
    case Phase::Idle:
      if (plan_.removeAll) return enter(Phase::RemoveList);
      [[fallthrough]];
    case Phase::RemoveList:
      if (plan_.removeAll) return enter(Phase::RemoveCurrent);
      [[fallthrough]];
    case Phase::RemoveCurrent:
      if (plan_.removeAll) return enter(Phase::ClearList);
      [[fallthrough]];
    case Phase::ClearList:
      if (!plan_.publish && !plan_.prune) return enter(Phase::Done);
      if (plan_.retire20 && plan_.publish) return enter(Phase::Retired);
      [[fallthrough]];
    case Phase::Retired:
      if (plan_.dropLegacy) return enter(Phase::DropList);
      [[fallthrough]];
    case Phase::DropList:
      if (plan_.prune) return enter(Phase::Prune);
      [[fallthrough]];
    case Phase::Prune:
      if (plan_.publish) return enter(Phase::Publish);
      [[fallthrough]];
    case Phase::Publish:
      if (changed_ || crcNew_ != crcOld_ || linesNew_ != linesOld_) return enter(Phase::WriteKept);
      return enter(Phase::Done);
    case Phase::WriteKept:
      if (plan_.publish) return enter(Phase::WriteCurrent);
      return enter(Phase::Commit);
    case Phase::WriteCurrent:
      return enter(Phase::Commit);
    default:
      return enter(Phase::Done);
  }
}

bool DiscoveryRun::remove(DiscoveryPort& port, const char* topic) {
  if (!port.publish(topic, "")) {
    abort(port);
    return false;
  }
  ++stats_.deletes;
  return true;
}

void DiscoveryRun::finishList(DiscoveryPort& port, bool commit) {
  if (reading_) port.listClose();
  reading_ = false;
  if (!writing_) return;
  writing_ = false;
  if (commit && port.listCommit()) {
    stats_.listWritten = true;
  } else {
    port.listAbort();
  }
}

void DiscoveryRun::abort(DiscoveryPort& port) {
  if (!running()) return;
  finishList(port, false);
  phase_ = Phase::Aborted;
}

DiscoveryRun::Phase DiscoveryRun::step(DiscoveryPort& port, JsonWriter& payload) {
  if (!running()) return phase_;
  char node[kStationNameMax + 1];
  if (!nodeOf(*ctx_, node)) {
    finishList(port, false);
    enter(Phase::Done);
    return phase_;
  }
  char line[kDiscoveryTopicMax + 1];
  DiscoveryMessage msg;
  switch (phase_) {
    case Phase::RemoveList:
    case Phase::Prune: {
      const bool prune = phase_ == Phase::Prune;
      if (!entered_) {
        entered_ = true;
        reading_ = port.listOpen();
        if (!reading_) {
          changed_ = changed_ || prune;  // a missing list is written after the run
          advance();
          return phase_;
        }
      }
      for (uint8_t k = 0; k < kLinesPerStep; ++k) {
        if (!readListLine(port, line)) {
          finishList(port, false);
          advance();
          return phase_;
        }
        const size_t len = strlen(line);
        const TopicClass cls = classifyDiscoveryTopic(*ctx_, line, len);
        if (!prune) {
          if (cls == TopicClass::Foreign) continue;
          remove(port, line);
          return phase_;
        }
        ++linesOld_;
        crcOld_ = crc32(reinterpret_cast<const uint8_t*>(line), len, crcOld_);
        crcOld_ = crc32(reinterpret_cast<const uint8_t*>("\n"), 1, crcOld_);
        if (cls == TopicClass::Foreign || cls == TopicClass::Stale) changed_ = true;
        if (cls == TopicClass::Stale) {
          remove(port, line);
          return phase_;
        }
        if (cls == TopicClass::KeptUnknown || (cls == TopicClass::Current && !plan_.publish)) {
          ++linesNew_;
          crcNew_ = crc32(reinterpret_cast<const uint8_t*>(line), len, crcNew_);
          crcNew_ = crc32(reinterpret_cast<const uint8_t*>("\n"), 1, crcNew_);
        }
      }
      return phase_;
    }
    case Phase::RemoveCurrent:
      if (it_.nextTopic(msg)) {
        remove(port, msg.topic);
      } else {
        advance();
      }
      return phase_;
    case Phase::ClearList:
      if (port.listBegin()) {
        writing_ = true;
        finishList(port, true);
      } else {
        port.listAbort();
      }
      advance();
      return phase_;
    case Phase::Retired: {
      if (!uptimeDone_) {
        uptimeDone_ = true;
        char station[kStationNameMax + 1];
        stationOf(*ctx_, station);
        if (buildConfigTopic(kLegacyPrefix, station, HaComponent::Sensor, "diag_stm_uptime", msg)) {
          remove(port, msg.topic);
          return phase_;
        }
      }
      while (it_.nextTopic(msg)) {
        if (it_.v20Topic(msg)) {
          remove(port, msg.topic);
          return phase_;
        }
      }
      advance();
      return phase_;
    }
    case Phase::DropList:
      if (drop_.next(msg)) {
        remove(port, msg.topic);
      } else {
        advance();
      }
      return phase_;
    case Phase::Publish: {
      const bool got = it_.next(msg, payload);
      if (got || !payload.ok()) {
        const size_t len = strlen(msg.topic);
        if (len > 0) {
          ++linesNew_;
          crcNew_ = crc32(reinterpret_cast<const uint8_t*>(msg.topic), len, crcNew_);
          crcNew_ = crc32(reinterpret_cast<const uint8_t*>("\n"), 1, crcNew_);
        }
        if (!got) {
          ++stats_.skipped;
        } else if (port.publish(msg.topic, payload.c_str())) {
          ++stats_.configs;
        } else {
          abort(port);
        }
        return phase_;
      }
      advance();
      return phase_;
    }
    case Phase::WriteKept: {
      if (!entered_) {
        entered_ = true;
        if (!port.listBegin()) {
          port.listAbort();
          enter(Phase::Done);
          return phase_;
        }
        writing_ = true;
        reading_ = port.listOpen();
      }
      for (uint8_t k = 0; k < kLinesPerStep; ++k) {
        if (!reading_ || !readListLine(port, line)) {
          if (reading_) port.listClose();
          reading_ = false;
          advance();
          return phase_;
        }
        const TopicClass cls = classifyDiscoveryTopic(*ctx_, line, strlen(line));
        if (cls != TopicClass::KeptUnknown && (cls != TopicClass::Current || plan_.publish)) continue;
        if (!port.listWrite(line)) {
          finishList(port, false);
          enter(Phase::Done);
          return phase_;
        }
      }
      return phase_;
    }
    case Phase::WriteCurrent:
      for (uint8_t k = 0; k < kLinesPerStep; ++k) {
        if (!it_.nextTopic(msg)) {
          advance();
          return phase_;
        }
        if (!port.listWrite(msg.topic)) {
          finishList(port, false);
          enter(Phase::Done);
          return phase_;
        }
      }
      return phase_;
    case Phase::Commit:
      finishList(port, true);
      enter(Phase::Done);
      return phase_;
    default:
      return phase_;
  }
}

}  // namespace vdm
