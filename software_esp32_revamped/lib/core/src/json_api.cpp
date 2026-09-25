#include "vdm/json_api.h"

#include <stdio.h>
#include <string.h>

namespace vdm {

const char* netStateName(NetState s) {
  switch (s) {
    case NetState::Down: return "down";
    case NetState::Ethernet: return "ethernet";
    case NetState::Wifi: return "wifi";
  }
  return "down";
}

const char* mqttStateName(MqttState s) {
  switch (s) {
    case MqttState::Disabled: return "disabled";
    case MqttState::Connecting: return "connecting";
    case MqttState::Connected: return "connected";
    case MqttState::Error: return "error";
  }
  return "error";
}

namespace {

// esp_reset_reason_t names (ESP-IDF 4.4).
const char* resetReasonName(uint8_t r) {
  static const char* const kNames[] = {"unknown", "poweron", "ext",       "sw",
                                       "panic",   "int_wdt", "task_wdt",  "wdt",
                                       "deepsleep", "brownout", "sdio"};
  return r < sizeof kNames / sizeof kNames[0] ? kNames[r] : "unknown";
}

// HealthFlag bit order.
const char* const kHealthNames[] = {"blocked",     "failed",      "noValve",
                                    "calibRetries", "earlyStop",   "cmdRejected",
                                    "stale",       "targetUnconfirmed", "tempFailed",
                                    "failsafe",    "strokeShort"};
constexpr uint8_t kHealthCount = sizeof kHealthNames / sizeof kHealthNames[0];

void ipValue(JsonWriter& jw, const char* k, uint32_t ip) {
  char tmp[16];
  formatIpv4(ip, tmp, sizeof tmp);
  jw.kv(k, tmp);
}

void hexValue(JsonWriter& jw, const char* k, uint32_t v, int digits) {
  char tmp[16];
  snprintf(tmp, sizeof tmp, "0x%0*lx", digits, static_cast<unsigned long>(v));
  jw.kv(k, tmp);
}

// Number or null when zero ("0 = none").
void u32OrNull(JsonWriter& jw, const char* k, uint32_t v) {
  jw.key(k);
  if (v) {
    jw.value(v);
  } else {
    jw.nullValue();
  }
}

void versionValue(JsonWriter& jw, const char* k, const Version& v) {
  char tmp[40];
  jw.key(k);
  if (formatVersion(v, tmp, sizeof tmp) > 0) {
    jw.value(tmp);
  } else {
    jw.nullValue();
  }
}

void chipValues(JsonWriter& jw, const char* idKey, const char* nameKey, uint16_t pid) {
  if (pid == 0) {
    jw.key(idKey);
    jw.nullValue();
    jw.key(nameKey);
    jw.nullValue();
    return;
  }
  hexValue(jw, idKey, pid, 3);
  jw.kv(nameKey, stmChipName(pid));
}

void oneWireIdValue(JsonWriter& jw, const char* k, const OneWireId& id) {
  char tmp[kOneWireIdTextLen + 1] = {0};
  if (!isZero(id)) formatOneWireId(id, tmp, sizeof tmp);
  jw.kv(k, tmp);
}

void stringOrEmpty(JsonWriter& jw, const char* k, const char* s) { jw.kv(k, s ? s : ""); }

void writeLinkStats(JsonWriter& jw, const LinkStats& st) {
  jw.beginObject();
  jw.kv("sent", st.sent);
  jw.kv("answered", st.answered);
  jw.kv("timeouts", st.timeouts);
  jw.kv("failedRequests", st.failedRequests);
  jw.kv("strayLines", st.strayLines);
  jw.kv("parseErrors", st.parseErrors);
  jw.kv("queueFull", st.queueFull);
  jw.kv("evictions", st.evictions);
  jw.kv("policyResets", st.policyResets);
  jw.kv("userResets", st.userResets);
  jw.kv("consecutiveTimeouts", static_cast<uint32_t>(st.consecutiveTimeouts));
  jw.kv("lastReplyMs", st.lastReplyMs);
  jw.endObject();
}

void writeStmStatus(JsonWriter& jw, const StmStatus& st) {
  jw.beginObject();
  jw.kv("uptime", st.uptimeS);
  jw.kv("resets", st.resets);
  jw.kv("bootReason", st.bootReason);
  jw.kv("rxOverflow", st.rxOverflow);
  jw.kv("parseErr", st.parseErrors);
  jw.kv("eepState", static_cast<uint32_t>(st.eepState));
  jw.endObject();
}

}  // namespace

bool writeStatusJson(JsonWriter& jw, const StatusSnapshot& s) {
  jw.beginObject();

  jw.key("esp");
  jw.beginObject();
  jw.kv("version", s.espVersion);
  u32OrNull(jw, "build", s.buildEpoch);
  jw.kv("uptime", s.uptimeS);
  jw.kv("resetReason", resetReasonName(s.resetReason));
  jw.kv("boots", s.bootCount);
  jw.key("heap");
  jw.beginObject();
  jw.kv("free", s.freeHeap);
  jw.kv("min", s.minFreeHeap);
  jw.kv("largest", s.largestFreeBlock);
  jw.endObject();
  jw.key("flash");
  jw.beginObject();
  jw.kv("used", s.sketchSize);
  jw.kv("size", s.sketchSpace);
  jw.endObject();
  jw.endObject();

  jw.key("time");
  jw.beginObject();
  jw.kv("valid", s.timeValid);
  jw.key("epoch");
  if (s.timeValid) {
    jw.value(s.epoch);
  } else {
    jw.nullValue();
  }
  jw.key("local");
  const LocalTime& t = s.local;
  if (s.timeValid && t.valid) {
    char tmp[32];
    snprintf(tmp, sizeof tmp, "%04u-%02u-%02uT%02u:%02u:%02u", static_cast<unsigned>(t.year),
             static_cast<unsigned>(t.month), static_cast<unsigned>(t.mday),
             static_cast<unsigned>(t.hour), static_cast<unsigned>(t.minute),
             static_cast<unsigned>(t.second));
    jw.value(tmp);
  } else {
    jw.nullValue();
  }
  u32OrNull(jw, "lastSync", s.lastSyncEpoch);
  jw.endObject();

  jw.key("net");
  jw.beginObject();
  jw.kv("state", netStateName(s.net));
  ipValue(jw, "ip", s.ip);
  ipValue(jw, "mask", s.mask);
  ipValue(jw, "gw", s.gateway);
  ipValue(jw, "dns", s.dns);
  jw.key("mac");
  jw.value(s.mac, boundedLength(s.mac, sizeof s.mac - 1));
  jw.key("rssi");
  if (s.net == NetState::Wifi) {
    jw.value(static_cast<int32_t>(s.wifiRssi));
  } else {
    jw.nullValue();
  }
  jw.key("hostname");
  jw.value(s.hostname, boundedLength(s.hostname, sizeof s.hostname - 1));
  jw.endObject();

  jw.key("mqtt");
  jw.beginObject();
  jw.kv("state", mqttStateName(s.mqtt));
  jw.kv("rc", static_cast<int32_t>(s.mqttRc));
  jw.kv("reconnects", s.mqttReconnects);
  jw.kv("publishFailures", s.mqttPublishFailures);
  jw.endObject();

  jw.key("stm");
  jw.beginObject();
  jw.kv("link", linkStateName(s.link));
  jw.key("proto");
  if (s.stmProto) {
    jw.value(static_cast<uint32_t>(s.stmProto));
  } else {
    jw.nullValue();
  }
  versionValue(jw, "version", s.stmVersion);
  u32OrNull(jw, "build", s.stmBuild);
  chipValues(jw, "hwId", "chip", s.stmHwId);
  jw.kv("compatible", s.stmCompatible);
  jw.kv("minVersion", minStmVersion());
  jw.key("stats");
  writeLinkStats(jw, s.linkStats);
  jw.key("status");
  if (s.haveStmStatus) {
    writeStmStatus(jw, s.stmStatus);
  } else {
    jw.nullValue();
  }
  jw.key("espRx");
  jw.beginObject();
  jw.kv("overflow", s.espLineOverflows);
  jw.kv("malformed", s.espLineMalformed);
  jw.endObject();
  jw.endObject();

  jw.key("calibration");
  jw.beginObject();
  jw.kv("active", s.calibrationActive);
  jw.key("lastScheduled");
  if (s.lastScheduledCalibEpoch > 0) {
    jw.value(s.lastScheduledCalibEpoch);
  } else {
    jw.nullValue();
  }
  u32OrNull(jw, "nextSlot", s.nextCalibSlot);
  jw.endObject();

  jw.kv("auth", s.authEnabled);
  jw.kv("lastEventSeq", s.lastEventSeq);
  jw.endObject();
  return jw.ok();
}

namespace {

void writeLastMove(JsonWriter& jw, const MoveResult& m) {
  jw.beginObject();
  jw.kv("dir", m.dir == MoveDir::Close ? "close" : "open");
  jw.kv("req", m.requestedCounts);
  jw.kv("cnt", m.countedCounts);
  jw.kv("stop", stopReasonName(m.stop));
  jw.key("peak");
  jw.fixed(m.peakCurrent, 1);  // 0.1 mA -> mA
  jw.kv("ms", m.durationMs);
  jw.endObject();
}

void writeValve(JsonWriter& jw, uint8_t index, const ValveView& v, uint32_t nowMs) {
  static const ValveState kNoState{};
  static const ValveConfig kNoConfig{};
  const ValveState& st = v.state ? *v.state : kNoState;
  const ValveConfig& cfg = v.config ? *v.config : kNoConfig;

  jw.beginObject();
  jw.kv("idx", static_cast<uint32_t>(index) + 1u);
  jw.key("name");
  jw.value(cfg.name, boundedLength(cfg.name, sizeof cfg.name - 1));
  jw.kv("active", cfg.active);
  jw.kv("known", st.known);
  jw.kv("state", static_cast<uint32_t>(st.status));
  jw.kv("stateKey", valveStatusKey(st.status));
  jw.kv("calibrating", st.calibrating);
  jw.kv("pos", static_cast<uint32_t>(st.position));
  jw.key("target");
  if (st.desiredValid) {
    jw.value(static_cast<uint32_t>(st.desired));
  } else {
    jw.nullValue();
  }
  jw.kv("targetSource", targetSourceName(st.source));
  jw.kv("sync", targetSyncName(st.sync));
  jw.key("stmTarget");
  if (st.stmTargetKnown) {
    jw.value(static_cast<uint32_t>(st.stmTarget));
  } else {
    jw.nullValue();
  }
  jw.kv("meanCur", static_cast<uint32_t>(st.meanCurrent));
  jw.kv("moves", st.moves);
  jw.kv("oc", st.openCount);
  jw.kv("cc", st.closeCount);
  jw.kv("dc", st.deadZone);
  jw.kv("cr", static_cast<uint32_t>(st.calibRetries));
  jw.key("health");
  jw.beginArray();
  for (uint8_t b = 0; b < kHealthCount; ++b) {
    if (st.health & (1u << b)) jw.value(kHealthNames[b]);
  }
  jw.endArray();
  jw.key("age");
  if (st.known) {
    jw.value(elapsedMs(nowMs, st.lastSeenMs) / 1000u);
  } else {
    jw.nullValue();
  }
  jw.key("sensors");
  jw.beginArray();
  for (uint8_t k = 0; k < 2; ++k) {
    if (v.sensorSlot[k] == 0) continue;
    jw.beginObject();
    jw.kv("slot", static_cast<uint32_t>(v.sensorSlot[k]));
    stringOrEmpty(jw, "name", v.sensorName[k]);
    jw.key("temp");
    if (v.sensorValid[k]) {
      jw.fixed(v.sensorTenths[k], 1);
    } else {
      jw.nullValue();
    }
    jw.endObject();
  }
  jw.endArray();
  jw.key("ext");
  if (st.hasExtended) {
    jw.beginObject();
    jw.kv("calState", static_cast<uint32_t>(st.calState));
    jw.kv("calEarlyStop", (st.calFlags & kCalFlagEarlyStop) != 0);
    jw.kv("calLastFailed", (st.calFlags & kCalFlagLastFailed) != 0);
    jw.kv("earlyStops", st.earlyStops);
    jw.kv("cmdRejected", st.cmdRejected);
    jw.key("lastMove");
    writeLastMove(jw, st.lastMove);
    jw.kv("moveSeq", st.moveSeq);
    jw.endObject();
  } else {
    jw.nullValue();
  }
  jw.endObject();
}

}  // namespace

bool writeValvesJson(JsonWriter& jw, const ValveView* views, uint8_t count, uint32_t nowMs) {
  if (views == nullptr) count = 0;
  jw.beginObject();
  jw.key("valves");
  jw.beginArray();
  for (uint8_t i = 0; i < count; ++i) writeValve(jw, i, views[i], nowMs);
  jw.endArray();
  jw.endObject();
  return jw.ok();
}

bool writeProfileJson(JsonWriter& jw, const Profile& p) {
  const uint8_t n = p.count <= kProfileMaxSamples ? p.count : kProfileMaxSamples;
  jw.beginObject();
  jw.kv("valve", static_cast<uint32_t>(p.valve) + 1u);
  jw.kv("count", static_cast<uint32_t>(n));
  jw.key("samples");
  jw.beginArray();
  for (uint8_t i = 0; i < n; ++i) {
    jw.beginArray();
    jw.value(p.samples[i].count);
    jw.value(static_cast<uint32_t>(p.samples[i].current));
    jw.endArray();
  }
  jw.endArray();
  jw.endObject();
  return jw.ok();
}

namespace {

void sensorCommon(JsonWriter& jw, const SensorView& s) {
  jw.key("slot");
  if (s.slot) {
    jw.value(static_cast<uint32_t>(s.slot));
  } else {
    jw.nullValue();
  }
  stringOrEmpty(jw, "name", s.name);
  oneWireIdValue(jw, "id", s.id);
  jw.kv("active", s.active);
  jw.kv("onBus", s.onBus);
}

}  // namespace

bool writeSensorsJson(JsonWriter& jw, const SensorView* temps, uint8_t tempCount,
                      const SensorView* volts, uint8_t voltCount) {
  if (temps == nullptr) tempCount = 0;
  if (volts == nullptr) voltCount = 0;
  jw.beginObject();
  jw.key("temps");
  jw.beginArray();
  for (uint8_t i = 0; i < tempCount; ++i) {
    const SensorView& s = temps[i];
    jw.beginObject();
    sensorCommon(jw, s);
    jw.key("temp");
    if (s.valid) {
      jw.fixed(s.value, 1);
    } else {
      jw.nullValue();
    }
    jw.kv("raw", s.raw);
    jw.kv("age", s.ageS);
    jw.key("valve");
    if (s.valve < kValveCount) {
      jw.value(static_cast<uint32_t>(s.valve) + 1u);
    } else {
      jw.nullValue();
    }
    jw.endObject();
  }
  jw.endArray();
  jw.key("volts");
  jw.beginArray();
  for (uint8_t i = 0; i < voltCount; ++i) {
    const SensorView& s = volts[i];
    jw.beginObject();
    sensorCommon(jw, s);
    jw.key("value");
    if (s.valid) {
      jw.fixed(s.value, 3);
    } else {
      jw.nullValue();
    }
    stringOrEmpty(jw, "unit", s.unit);
    jw.kv("raw", s.raw);
    jw.kv("age", s.ageS);
    jw.endObject();
  }
  jw.endArray();
  jw.endObject();
  return jw.ok();
}

bool writeEventsJson(JsonWriter& jw, const Event* events, size_t count, uint32_t firstSeq,
                     uint32_t lastSeq, uint32_t nextSince, uint32_t dropped) {
  if (events == nullptr) count = 0;
  jw.beginObject();
  jw.kv("first", firstSeq);
  jw.kv("last", lastSeq);
  jw.kv("next", nextSince);
  jw.kv("dropped", dropped);
  jw.key("events");
  jw.beginArray();
  for (size_t i = 0; i < count && jw.ok(); ++i) writeEventJson(jw, events[i]);
  jw.endArray();
  jw.endObject();
  return jw.ok();
}

bool writeFlashStatusJson(JsonWriter& jw, const FlashStatus& s, const char* imageName) {
  jw.beginObject();
  jw.kv("phase", flashPhaseName(s.phase));
  jw.kv("status", static_cast<uint32_t>(legacyFlashStatus(s.phase)));
  jw.kv("percent", static_cast<uint32_t>(s.percent));
  jw.kv("bytesDone", s.bytesDone);
  jw.kv("bytesTotal", s.bytesTotal);
  chipValues(jw, "chipId", "chipName", s.chipPid);
  jw.key("bootloaderVersion");
  if (s.bootloaderVersion) {
    char tmp[8];
    snprintf(tmp, sizeof tmp, "%u.%u", static_cast<unsigned>(s.bootloaderVersion >> 4),
             static_cast<unsigned>(s.bootloaderVersion & 0x0F));
    jw.value(tmp);
  } else {
    jw.nullValue();
  }
  jw.kv("attempt", static_cast<uint32_t>(s.attempt));
  jw.key("error");
  if (s.error != FlashError::None) {
    jw.beginObject();
    jw.kv("code", flashErrorName(s.error));
    jw.kv("phase", flashPhaseName(s.errorPhase));
    hexValue(jw, "addr", s.errorAddress, 8);
    jw.endObject();
  } else {
    jw.nullValue();
  }
  jw.kv("startedMs", s.startedMs);
  jw.kv("finishedMs", s.finishedMs);
  jw.key("image");
  if (imageName != nullptr || s.image.size != 0) {
    jw.beginObject();
    jw.key("name");
    jw.value(imageName);  // null pointer -> null
    jw.kv("size", s.image.size);
    hexValue(jw, "crc32", s.image.crc, 8);
    jw.key("version");
    const size_t vlen = boundedLength(s.image.version, sizeof s.image.version - 1);
    if (vlen) {
      jw.value(s.image.version, vlen);
    } else {
      jw.nullValue();
    }
    jw.endObject();
  } else {
    jw.nullValue();
  }
  versionValue(jw, "appVersion", s.appVersion);
  jw.endObject();
  return jw.ok();
}

bool writeMotorJson(JsonWriter& jw, const MotorChars& m, uint16_t learnMovements,
                    const Breakaway* breakaway, bool known) {
  jw.beginObject();
  jw.key("motor");
  jw.beginObject();
  jw.kv("lowC", static_cast<uint32_t>(m.lowFactor));
  jw.kv("highC", static_cast<uint32_t>(m.highFactor));
  jw.kv("startOnPower", static_cast<uint32_t>(m.startOnPower));
  jw.kv("noOfMinCount", static_cast<uint32_t>(m.minCounts));
  jw.kv("maxCalReps", static_cast<uint32_t>(m.maxCalibRetries));
  jw.endObject();
  jw.kv("learnMovements", static_cast<uint32_t>(learnMovements));
  jw.key("breakaway");
  if (breakaway != nullptr) {
    jw.beginObject();
    jw.kv("enable", breakaway->enable);
    jw.kv("stepPct", static_cast<uint32_t>(breakaway->stepPct));
    jw.kv("maxmA", static_cast<uint32_t>(breakaway->maxmA));
    jw.endObject();
  } else {
    jw.nullValue();
  }
  jw.kv("known", known);
  jw.endObject();
  return jw.ok();
}

bool writeErrorJson(JsonWriter& jw, const char* code, const char* detail) {
  jw.beginObject();
  jw.kv("error", code);
  jw.kv("detail", detail);
  jw.endObject();
  return jw.complete();
}

// ---------------------------------------------------------------- routing

namespace {

// Pattern segments: '#' = valve number {n}, '*' = file name {name}.
struct RouteDef {
  const char* pattern;  // below "/api/"
  HttpMethod method;
  ApiRoute route;
};

const RouteDef kRoutes[] = {
    {"status", HttpMethod::Get, ApiRoute::Status},
    {"valves", HttpMethod::Get, ApiRoute::Valves},
    {"valves/#/target", HttpMethod::Post, ApiRoute::ValveTarget},
    {"valves/#/calibrate", HttpMethod::Post, ApiRoute::ValveCalibrate},
    {"valves/#/assembly", HttpMethod::Post, ApiRoute::ValveAssembly},
    {"valves/#/service-move", HttpMethod::Post, ApiRoute::ValveServiceMove},
    {"valves/#/sensors", HttpMethod::Post, ApiRoute::ValveSensors},
    {"valves/#/profile", HttpMethod::Get, ApiRoute::ValveProfile},
    {"valves/#/profile", HttpMethod::Post, ApiRoute::ValveProfileRefresh},
    {"valves/calibrate", HttpMethod::Post, ApiRoute::CalibrateAll},
    {"valves/assembly", HttpMethod::Post, ApiRoute::AssemblyAll},
    {"valves/detect", HttpMethod::Post, ApiRoute::Detect},
    {"sensors", HttpMethod::Get, ApiRoute::Sensors},
    {"sensors/scan", HttpMethod::Post, ApiRoute::SensorsScan},
    {"events", HttpMethod::Get, ApiRoute::Events},
    {"config", HttpMethod::Get, ApiRoute::ConfigGet},
    {"config", HttpMethod::Post, ApiRoute::ConfigPatch},
    {"config/export", HttpMethod::Get, ApiRoute::ConfigExport},
    {"stm/motor", HttpMethod::Get, ApiRoute::Motor},
    {"stm/motor", HttpMethod::Post, ApiRoute::MotorSet},
    {"stm/reset", HttpMethod::Post, ApiRoute::StmReset},
    {"stm/images", HttpMethod::Get, ApiRoute::StmImages},
    {"stm/images", HttpMethod::Post, ApiRoute::StmImageUpload},
    {"stm/images/*", HttpMethod::Delete, ApiRoute::StmImageDelete},
    {"stm/flash", HttpMethod::Post, ApiRoute::StmFlash},
    {"stm/flash", HttpMethod::Get, ApiRoute::StmFlashStatus},
    {"stm/flash/abort", HttpMethod::Post, ApiRoute::StmFlashAbort},
    {"ota/esp", HttpMethod::Post, ApiRoute::EspOta},
    {"system/reboot", HttpMethod::Post, ApiRoute::Reboot},
    {"system/factory-reset", HttpMethod::Post, ApiRoute::FactoryReset},
    {"mqtt/reconnect", HttpMethod::Post, ApiRoute::MqttReconnect},
    {"mqtt/discovery", HttpMethod::Post, ApiRoute::MqttDiscovery},
    {"log", HttpMethod::Get, ApiRoute::LogDownload},
};

constexpr size_t kNameMax = sizeof(RouteMatch::name) - 1;

bool isReadRoute(ApiRoute r) {
  switch (r) {
    case ApiRoute::Status:
    case ApiRoute::Valves:
    case ApiRoute::ValveProfile:
    case ApiRoute::Sensors:
    case ApiRoute::Events:
    case ApiRoute::Motor:
    case ApiRoute::StmFlashStatus:
      return true;
    default:
      return false;
  }
}

// "1".."12" without leading zeros -> 0-based valve.
bool valveSegment(const char* s, size_t len, uint8_t& valve) {
  uint32_t v = 0;
  if (len == 0 || s[0] == '0' || !parseUint(s, len, kValveCount, v)) return false;
  valve = static_cast<uint8_t>(v - 1);
  return true;
}

// [A-Za-z0-9._-]{1,31}, no leading '.'.
bool nameSegment(const char* s, size_t len) {
  if (len == 0 || len > kNameMax || s[0] == '.') return false;
  for (size_t i = 0; i < len; ++i) {
    const char c = s[i];
    const bool ok = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') ||
                    c == '.' || c == '_' || c == '-';
    if (!ok) return false;
  }
  return true;
}

// Matches `rest` (the path after "/api/") against one pattern.
bool matchPattern(const char* pattern, const char* rest, size_t len, RouteMatch& m) {
  size_t pos = 0;
  const char* p = pattern;
  for (;;) {
    const char* pEnd = strchr(p, '/');
    const size_t pLen = pEnd ? static_cast<size_t>(pEnd - p) : strlen(p);
    size_t end = pos;
    while (end < len && rest[end] != '/') ++end;
    const char* seg = rest + pos;
    const size_t segLen = end - pos;
    if (pLen == 1 && p[0] == '#') {
      if (!valveSegment(seg, segLen, m.valve)) return false;
    } else if (pLen == 1 && p[0] == '*') {
      if (!nameSegment(seg, segLen)) return false;
      memcpy(m.name, seg, segLen);
      m.name[segLen] = '\0';
    } else if (segLen != pLen || memcmp(seg, p, pLen) != 0) {
      return false;
    }
    const bool patternDone = pEnd == nullptr;
    const bool pathDone = end == len;
    if (patternDone || pathDone) return patternDone && pathDone;
    p = pEnd + 1;
    pos = end + 1;
  }
}

}  // namespace

RouteMatch matchApiRoute(HttpMethod method, const char* path, size_t len, bool protectRead) {
  RouteMatch result;
  static const char kPrefix[] = "/api/";
  constexpr size_t kPrefixLen = sizeof kPrefix - 1;
  if (path == nullptr || len <= kPrefixLen || memcmp(path, kPrefix, kPrefixLen) != 0 ||
      memchr(path, '\0', len) != nullptr) {
    return result;
  }
  const char* rest = path + kPrefixLen;
  const size_t restLen = len - kPrefixLen;
  bool pathKnown = false;
  for (const RouteDef& r : kRoutes) {
    RouteMatch m;
    if (!matchPattern(r.pattern, rest, restLen, m)) continue;
    pathKnown = true;
    if (r.method != method) continue;
    m.route = r.route;
    m.needsAuth = !(isReadRoute(r.route) && !protectRead);
    return m;
  }
  if (pathKnown) result.route = ApiRoute::MethodNotAllowed;
  return result;
}

}  // namespace vdm
