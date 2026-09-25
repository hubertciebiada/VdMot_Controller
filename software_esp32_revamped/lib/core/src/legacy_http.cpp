#include "vdm/legacy_http.h"

#include <string.h>

namespace vdm {

namespace {

struct GonePath {
  const char* path;
  const char* replacement;
};

const char kCmdReplacement[] =
    "/api/system/reboot, /api/valves/calibrate, /api/valves/assembly, /api/valves/detect, "
    "/api/sensors/scan, /api/mqtt/reconnect, /api/mqtt/discovery";

const GonePath kGone[] = {
    {"/netinfo", "/api/status"},
    {"/sysinfo", "/api/status"},
    {"/sysdyninfo", "/api/status"},
    {"/update/identity", "/api/status"},
    {"/netconfig", "/api/config"},
    {"/protconfig", "/api/config"},
    {"/valvesconfig", "/api/config"},
    {"/tempsconfig", "/api/config"},
    {"/voltsconfig", "/api/config"},
    {"/sysconfig", "/api/config"},
    {"/sysLogCfg", "/api/config"},
    {"/motorconfig", "/api/stm/motor"},
    {"/tempsensorsid", "/api/sensors"},
    {"/voltsensorsid", "/api/sensors"},
    {"/fsdir", "/api/files"},
    {"/fupload", "/api/stm/images"},
    {"/stmupdate", "/#maintenance"},
    {"/stmupdstatus", "/api/stm/flash"},
    {"/stmdoupdate", "/api/stm/flash"},
    {"/update", "/api/ota/esp"},
    {"/cmd", kCmdReplacement},
    {"/valvesctrlconfig", "removed: PI control"},
    {"/msgconfig", "removed: messenger"},
    {"/testPO", "removed: messenger"},
    {"/testEmail", "removed: messenger"},
    {"/ssidinfo", "removed: WiFi scan"},
    {"/auth", "removed: HTTP Basic auth is used"},
};

bool pathIs(const char* path, size_t len, const char* want) {
  return strlen(want) == len && memcmp(path, want, len) == 0;
}

void writeTemp(JsonWriter& jw, bool valid, int32_t tenths) {
  if (valid) {
    jw.fixed(tenths, 1);
  } else {
    jw.value("failed");
  }
}

}  // namespace

LegacyMatch matchLegacyRoute(HttpMethod m, const char* path, size_t len, bool protectRead) {
  LegacyMatch r;
  if (path == nullptr) return r;
  struct Alias {
    const char* path;
    LegacyRoute route;
    HttpMethod method;
  };
  static const Alias kAliases[] = {
      {"/valves", LegacyRoute::Valves, HttpMethod::Get},
      {"/temps", LegacyRoute::Temps, HttpMethod::Get},
      {"/volts", LegacyRoute::Volts, HttpMethod::Get},
      {"/setvalve", LegacyRoute::SetValve, HttpMethod::Post},
  };
  for (const Alias& a : kAliases) {
    if (!pathIs(path, len, a.path)) continue;
    if (m != a.method) {
      r.route = LegacyRoute::MethodNotAllowed;
      return r;
    }
    r.route = a.route;
    r.needsAuth = a.route == LegacyRoute::SetValve || protectRead;
    return r;
  }
  for (const GonePath& g : kGone) {
    if (!pathIs(path, len, g.path)) continue;
    r.route = LegacyRoute::Gone;
    r.replacement = g.replacement;
    return r;
  }
  return r;
}

bool writeLegacyValvesJson(JsonWriter& jw, const ValveView* views, uint8_t count) {
  if (views == nullptr) count = 0;
  jw.beginObject();
  jw.key("valves");
  jw.beginArray();
  for (uint8_t i = 0; i < count; ++i) {
    const ValveView& v = views[i];
    if (v.state == nullptr || v.config == nullptr) continue;
    const ValveState& st = *v.state;
    if (st.status == 0 || st.status == static_cast<uint8_t>(ValveStatus::NoValve)) continue;
    jw.beginObject();
    jw.kv("idx", static_cast<uint32_t>(i) + 1u);
    jw.key("name");
    jw.value(v.config->name, boundedLength(v.config->name, sizeof v.config->name - 1));
    jw.kv("state", static_cast<uint32_t>(st.status));
    jw.kv("pos", static_cast<uint32_t>(st.position));
    jw.kv("meanCur", static_cast<uint32_t>(st.meanCurrent));
    const uint8_t target = st.desiredValid     ? st.desired
                           : st.stmTargetKnown ? st.stmTarget
                                               : st.position;
    jw.kv("targetPos", static_cast<uint32_t>(target));
    jw.kv("link", static_cast<uint32_t>(0));
    jw.kv("moves", st.moves);
    jw.kv("oc", st.openCount);
    jw.kv("cc", st.closeCount);
    jw.kv("dc", st.deadZone);
    jw.kv("cr", static_cast<uint32_t>(st.calibRetries));
    static const char* const kNameKeys[2] = {"tIdxName1", "tIdxName2"};
    static const char* const kTempKeys[2] = {"temp1", "temp2"};
    for (uint8_t k = 0; k < 2; ++k) {
      if (v.sensorSlot[k] == 0) continue;
      jw.kv(kNameKeys[k], v.sensorName[k] ? v.sensorName[k] : "");
      jw.key(kTempKeys[k]);
      writeTemp(jw, v.sensorValid[k], v.sensorTenths[k]);
    }
    jw.kv("controlActive", static_cast<uint32_t>(0));
    if (st.calibrating) jw.kv("calibration", static_cast<uint32_t>(1));
    jw.endObject();
  }
  jw.endArray();
  jw.endObject();
  return jw.ok();
}

namespace {

void idValue(JsonWriter& jw, const OneWireId& id) {
  char tmp[kOneWireIdTextLen + 1] = {0};
  formatOneWireId(id, tmp, sizeof tmp);
  jw.kv("id", tmp);
}

bool listed(const SensorView& s) { return s.slot != 0 && s.active && !isZero(s.id); }

}  // namespace

bool writeLegacyTempsJson(JsonWriter& jw, const SensorView* temps, uint8_t count, bool allTemps) {
  if (temps == nullptr) count = 0;
  jw.beginArray();
  for (uint8_t i = 0; i < count; ++i) {
    const SensorView& s = temps[i];
    if (!listed(s) || (s.valve < kValveCount && !allTemps)) continue;
    jw.beginObject();
    idValue(jw, s.id);
    jw.kv("name", s.name ? s.name : "");
    jw.key("temp");
    writeTemp(jw, s.valid, s.value);
    jw.endObject();
  }
  jw.endArray();
  return jw.ok();
}

bool writeLegacyVoltsJson(JsonWriter& jw, const SensorView* volts, uint8_t count) {
  if (volts == nullptr) count = 0;
  jw.beginArray();
  for (uint8_t i = 0; i < count; ++i) {
    const SensorView& s = volts[i];
    if (!listed(s)) continue;
    jw.beginObject();
    idValue(jw, s.id);
    jw.kv("name", s.name ? s.name : "");
    jw.kv("unit", s.unit ? s.unit : "");
    jw.key("value");
    if (s.valid) {
      jw.fixed(s.value, 3);
    } else {
      jw.value("failed");
    }
    jw.endObject();
  }
  jw.endArray();
  return jw.ok();
}

}  // namespace vdm
