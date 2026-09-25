#include "web_server.h"

#include <Arduino.h>
#include <ArduinoJson.h>
#include <AsyncTCP.h>
#include <AsyncWebServer_WT32_ETH01.h>
#include <LittleFS.h>
#include <esp_ota_ops.h>
#include <esp_system.h>
#include <new>
#include <string.h>

#include <vdm/auth.h>
#include <vdm/config.h>
#include <vdm/json_api.h>
#include <vdm/json_writer.h>

#include "app.h"
#include "boot_alloc.h"
#include "generated/web_assets.h"
#include "logger.h"
#include "mqtt_client.h"
#include "net.h"
#include "ota.h"
#include "storage.h"

namespace web {

namespace {

// Everything below is only touched by the AsyncTCP task (all handlers,
// body/upload callbacks and disconnect callbacks run there), so the statics
// need no locking among themselves.

constexpr const char* kJson = "application/json";
constexpr uint32_t kSensorStaleMs = 60000;
constexpr size_t kMaxEventsPerResponse = 50;

AsyncWebServer gServer(80);
bool gStarted = false;

struct Slot {
  char* buf;
  bool busy;
};
Slot gSlots[kResponseSlots];

// JSON body (one at a time).
using BodyBuf = ObjArray<char, kMaxBodySize + 1>;
char* const gBody = bootAlloc<BodyBuf>().data();
size_t gBodyLen = 0;
bool gBodyOverflow = false;
AsyncWebServerRequest* gBodyOwner = nullptr;

// Requests whose body was refused while it arrived (answered in
// handleRequest). A mark is cleared when its request starts a new body and
// when it disconnects (every request ends that way, also one that never
// reached handleRequest), so a recycled request address never inherits a
// stale mark.
struct Mark {
  AsyncWebServerRequest* req;
  uint16_t code;
  const char* error;
};
Mark gMarks[4];

// Upload in progress (STM image or ESP firmware).
enum class UploadKind : uint8_t { None, StmImage, EspOta };
struct Upload {
  AsyncWebServerRequest* owner = nullptr;
  UploadKind kind = UploadKind::None;
  bool started = false;
  bool done = false;
  uint16_t failCode = 0;
  char error[48] = {0};
  storage::ImageEntry info;
} gUpload;

// /api/log download (one at a time).
struct LogStream {
  AsyncWebServerRequest* owner = nullptr;
  fs::File file;
  uint8_t part = 0;  // 0 = events.1.log, 1 = events.log, 2 = done
} gLog;

app::StmSnapshot& gSnap = bootAlloc<app::StmSnapshot>();
vdm::Config& gCfg = bootAlloc<vdm::Config>();
uint32_t gCfgRevision = UINT32_MAX;
vdm::Config& gPatch = bootAlloc<vdm::Config>();
vdm::AuthLimiter gAuthLimiter;
StaticJsonDocument<512>& gDoc = bootAlloc<StaticJsonDocument<512>>();
using ValveViews = ObjArray<vdm::ValveView, vdm::kValveCount>;
using TempViews = ObjArray<vdm::SensorView, 2 * vdm::kTempSlotCount>;  // slots + unconfigured
using VoltViews = ObjArray<vdm::SensorView, 2 * vdm::kVoltSlotCount>;
using Events = ObjArray<vdm::Event, kMaxEventsPerResponse>;
using Images = ObjArray<storage::ImageEntry, storage::kImageSlots>;
ValveViews& gValveViews = bootAlloc<ValveViews>();
TempViews& gTempViews = bootAlloc<TempViews>();
VoltViews& gVoltViews = bootAlloc<VoltViews>();
Events& gEvents = bootAlloc<Events>();
Images& gImages = bootAlloc<Images>();

void refreshConfig() {
  const uint32_t rev = storage::configRevision();
  if (rev == gCfgRevision) return;
  gCfgRevision = rev;
  storage::getConfig(gCfg);
}

bool authEnabled() { return gCfg.web.user[0] != '\0' && gCfg.web.password[0] != '\0'; }

// ---------------------------------------------------------------- responses

int acquireSlot() {
  for (size_t i = 0; i < kResponseSlots; ++i) {
    if (gSlots[i].buf != nullptr && !gSlots[i].busy) {
      gSlots[i].busy = true;
      return static_cast<int>(i);
    }
  }
  return -1;
}

void releaseSlot(int slot) {
  if (slot >= 0 && static_cast<size_t>(slot) < kResponseSlots) gSlots[slot].busy = false;
}

// Sends slot content without copying; the slot is released when the client
// is gone (response sent or connection dropped).
void sendSlot(AsyncWebServerRequest* req, int code, int slot, size_t len,
              const char* attachment = nullptr) {
  AsyncWebServerResponse* res = req->beginResponse_P(
      code, kJson, reinterpret_cast<const uint8_t*>(gSlots[slot].buf), len);
  if (res == nullptr) {
    releaseSlot(slot);
    req->send(500);
    return;
  }
  res->addHeader("Cache-Control", "no-store");
  if (attachment != nullptr) res->addHeader("Content-Disposition", attachment);
  req->onDisconnect([slot]() { releaseSlot(slot); });
  req->send(res);
}

void sendError(AsyncWebServerRequest* req, int code, const char* error, const char* detail) {
  char buf[200];
  vdm::JsonWriter jw(buf, sizeof buf);
  vdm::writeErrorJson(jw, error, detail ? detail : "");
  req->send(code, kJson, jw.complete() ? jw.c_str() : "{\"error\":\"internal\"}");
}

void sendAccepted(AsyncWebServerRequest* req) { req->send(202, kJson, "{\"result\":\"queued\"}"); }

// Builds a document into a response slot and sends it. `build` returns
// false when the document did not fit or failed.
template <typename F>
void sendDocument(AsyncWebServerRequest* req, int code, F build,
                  const char* attachment = nullptr) {
  const int slot = acquireSlot();
  if (slot < 0) return sendError(req, 503, "busy", "response buffers in use");
  vdm::JsonWriter jw(gSlots[slot].buf, kResponseSlotSize);
  if (!build(jw) || !jw.complete()) {
    releaseSlot(slot);
    return sendError(req, 500, "internal", "document too large");
  }
  sendSlot(req, code, slot, jw.length(), attachment);
}

void clearMark(AsyncWebServerRequest* req) {
  for (Mark& m : gMarks) {
    if (m.req == req) m = Mark{nullptr, 0, nullptr};
  }
}

// Only for a request that owns nothing else (its onDisconnect is free).
void mark(AsyncWebServerRequest* req, uint16_t code, const char* error) {
  req->onDisconnect([req]() { clearMark(req); });
  for (Mark& m : gMarks) {
    if (m.req == nullptr || m.req == req) {
      m = Mark{req, code, error};
      return;
    }
  }
  gMarks[0] = Mark{req, code, error};  // table full: the oldest mark is stale
}

bool takeMark(AsyncWebServerRequest* req, Mark& out) {
  for (Mark& m : gMarks) {
    if (m.req == req && req != nullptr) {
      out = m;
      m = Mark{nullptr, 0, nullptr};
      return true;
    }
  }
  return false;
}

// ---------------------------------------------------------------- auth

vdm::HttpMethod methodOf(AsyncWebServerRequest* req) {
  switch (req->method()) {
    case HTTP_GET: return vdm::HttpMethod::Get;
    case HTTP_POST: return vdm::HttpMethod::Post;
    case HTTP_DELETE: return vdm::HttpMethod::Delete;
    default: return vdm::HttpMethod::Other;
  }
}

vdm::RouteMatch route(AsyncWebServerRequest* req) {
  const String& url = req->url();
  return vdm::matchApiRoute(methodOf(req), url.c_str(), url.length(), gCfg.web.protectRead);
}

enum class AuthResult : uint8_t { Ok, Locked, Missing, Wrong };

AuthResult checkAuth(AsyncWebServerRequest* req, bool needsAuth) {
  if (!authEnabled() || !needsAuth) return AuthResult::Ok;
  if (gAuthLimiter.locked(app::nowMs())) return AuthResult::Locked;
  AsyncWebHeader* h = req->getHeader("Authorization");
  if (h == nullptr) return AuthResult::Missing;
  return vdm::checkBasicAuth(h->value().c_str(), h->value().length(), gCfg.web.user,
                             gCfg.web.password)
             ? AuthResult::Ok
             : AuthResult::Wrong;
}

// Books the result, logs failures and answers 401/429. True = go on.
bool authorised(AsyncWebServerRequest* req, bool needsAuth) {
  const AuthResult r = checkAuth(req, needsAuth);
  if (r == AuthResult::Ok) {
    if (authEnabled() && needsAuth) gAuthLimiter.onResult(true, app::nowMs());
    return true;
  }
  if (r == AuthResult::Locked) {
    sendError(req, 429, "locked", "too many failed logins");
    return false;
  }
  if (r == AuthResult::Wrong) {
    // A missing header is the browser's first attempt, not a failure.
    gAuthLimiter.onResult(false, app::nowMs());
    char ip[16];
    vdm::formatIpv4(static_cast<uint32_t>(req->client()->remoteIP()), ip, sizeof ip);
    logger::log(vdm::EventCode::AuthFailed, vdm::kNoValve,
                static_cast<int32_t>(gAuthLimiter.failuresInWindow()), 0, ip);
  }
  AsyncWebServerResponse* res =
      req->beginResponse(401, kJson, "{\"error\":\"unauthorized\",\"detail\":\"\"}");
  if (res == nullptr) {
    req->send(500);  // out of memory
    return false;
  }
  res->addHeader("WWW-Authenticate", "Basic realm=\"VdMot\"");
  req->send(res);
  return false;
}

// ---------------------------------------------------------------- guard

bool isUploadRoute(vdm::ApiRoute r) {
  return r == vdm::ApiRoute::StmImageUpload || r == vdm::ApiRoute::EspOta;
}

size_t uploadLimit(vdm::ApiRoute r) {
  if (r == vdm::ApiRoute::StmImageUpload) return kMaxStmImageSize + kMultipartSlack;
  const esp_partition_t* p = esp_ota_get_next_update_partition(nullptr);
  return (p != nullptr ? p->size : 0) + kMultipartSlack;
}

bool uploadBusy() {
  return gUpload.owner != nullptr || ota::uploadActive() || storage::imageUploadActive() ||
         app::stmFlashActive();
}

struct Refusal {
  uint16_t code = 0;
  const char* error = nullptr;
  const char* detail = nullptr;
  AuthResult auth = AuthResult::Ok;
};

// Requests that must be answered without buffering their body.
bool refusal(AsyncWebServerRequest* req, Refusal& out) {
  refreshConfig();
  const size_t len = req->contentLength();
  const bool multipart = req->contentType().startsWith("multipart/");
  if (!req->url().startsWith("/api/")) {
    if (len > 0) out = Refusal{413, "too_large", "no body expected"};
    return len > 0;
  }
  const vdm::RouteMatch m = route(req);
  if (m.route != vdm::ApiRoute::NotFound && m.route != vdm::ApiRoute::MethodNotAllowed &&
      isUploadRoute(m.route)) {
    if (!multipart) {
      out = Refusal{415, "unsupported_media_type", "multipart/form-data required"};
    } else if (len == 0) {
      out = Refusal{411, "length_required", "Content-Length"};
    } else if (len > uploadLimit(m.route)) {
      out = Refusal{413, "too_large", "file"};
    } else {
      out.auth = checkAuth(req, m.needsAuth);
      if (out.auth != AuthResult::Ok) {
        out.code = 401;
      } else if (uploadBusy()) {
        out = Refusal{409, "busy", "upload or flash running"};
      }
    }
    return out.code != 0;
  }
  if (multipart && len > 0) {
    out = Refusal{415, "unsupported_media_type", "application/json expected"};
  } else if (len > kMaxBodySize) {
    out = Refusal{413, "too_large", "body"};
  }
  return out.code != 0;
}

class GuardHandler : public AsyncWebHandler {
 public:
  bool canHandle(AsyncWebServerRequest* req) override {
    Refusal r;
    if (!refusal(req, r)) return false;
    req->addInterestingHeader("Authorization");
    return true;
  }
  void handleRequest(AsyncWebServerRequest* req) override {
    Refusal r;
    if (!refusal(req, r)) return sendError(req, 503, "retry", "state changed");
    if (r.code == 401) {
      // Books the failure and answers 401/429; credentials that became
      // valid meanwhile (config change) get a retry answer.
      if (authorised(req, true)) sendError(req, 503, "retry", "state changed");
      return;
    }
    sendError(req, r.code, r.error, r.detail);
  }
  // The body of a refused request is skipped, never parsed or buffered.
  bool isRequestHandlerTrivial() override { return true; }
};

// ---------------------------------------------------------------- JSON input

bool parseBody(AsyncWebServerRequest* req, bool hasBody) {
  if (!hasBody || gBodyLen == 0) {
    sendError(req, 400, "bad_request", "JSON body required");
    return false;
  }
  gDoc.clear();
  const DeserializationError e = deserializeJson(gDoc, gBody, gBodyLen);
  if (e || !gDoc.is<JsonObject>()) {
    sendError(req, 400, "bad_request", e ? e.c_str() : "object expected");
    return false;
  }
  return true;
}

// Strict integer member: present, an integer (not bool/float/string), in
// range. `required` false lets a missing member keep `out`.
bool intField(JsonObjectConst o, const char* key, int64_t min, int64_t max, int64_t& out,
              bool required) {
  JsonVariantConst v = o[key];
  if (v.isNull()) return !required;
  if (!v.is<long long>() || v.is<bool>()) return false;
  const long long x = v.as<long long>();
  if (x < min || x > max) return false;
  out = x;
  return true;
}

bool boolField(JsonObjectConst o, const char* key, bool& out, bool required) {
  JsonVariantConst v = o[key];
  if (v.isNull()) return !required;
  if (!v.is<bool>()) return false;
  out = v.as<bool>();
  return true;
}

// Rejects members other than the listed ones.
bool onlyKeys(JsonObjectConst o, const char* const* keys, size_t n) {
  for (JsonPairConst kv : o) {
    bool known = false;
    for (size_t i = 0; i < n && !known; ++i) known = strcmp(kv.key().c_str(), keys[i]) == 0;
    if (!known) return false;
  }
  return true;
}

bool submitOr503(AsyncWebServerRequest* req, const app::Command& c) {
  if (app::submit(c)) return true;
  sendError(req, 503, "queue_full", "STM command queue full");
  return false;
}

bool refuseWhileFlashing(AsyncWebServerRequest* req) {
  if (!app::stmFlashActive()) return false;
  sendError(req, 409, "flashing", "STM flash in progress");
  return true;
}

// ---------------------------------------------------------------- GET handlers

void handleStatus(AsyncWebServerRequest* req) {
  app::readStmSnapshot(gSnap);
  static vdm::StatusSnapshot s;
  s = vdm::StatusSnapshot{};
  s.espVersion = vdm::firmwareVersion();
#ifdef VDM_BUILD_EPOCH
  s.buildEpoch = static_cast<uint32_t>(VDM_BUILD_EPOCH);
#endif
  s.uptimeS = app::uptimeS();
  s.resetReason = static_cast<uint8_t>(esp_reset_reason());
  s.bootCount = storage::bootCount();
  s.freeHeap = ESP.getFreeHeap();
  s.minFreeHeap = ESP.getMinFreeHeap();
  s.largestFreeBlock = ESP.getMaxAllocHeap();
  s.sketchSize = ESP.getSketchSize();
  const esp_partition_t* running = esp_ota_get_running_partition();
  s.sketchSpace = running != nullptr ? running->size : 0;
  s.local = net::localTime();
  s.timeValid = s.local.valid;
  s.epoch = s.local.epoch;
  s.lastSyncEpoch = net::lastSyncEpoch();
  const net::Info ni = net::info();
  s.net = ni.state;
  s.ip = ni.ip;
  s.mask = ni.mask;
  s.gateway = ni.gateway;
  s.dns = ni.dns;
  memcpy(s.mac, ni.mac, sizeof s.mac);
  s.wifiRssi = ni.rssi;
  vdm::buildHostname(gCfg.station, s.hostname, sizeof s.hostname);
  const mqtt::Status ms = mqtt::status();
  s.mqtt = ms.state;
  s.mqttRc = ms.rc;
  s.mqttReconnects = ms.reconnects;
  s.mqttPublishFailures = ms.publishFailures;
  s.link = gSnap.link;
  s.linkStats = gSnap.linkStats;
  s.stmProto = gSnap.proto;
  s.stmVersion = gSnap.version;
  s.stmBuild = gSnap.build;
  s.stmHwId = gSnap.hwId;
  s.stmCompatible = gSnap.compatible;
  s.haveStmStatus = gSnap.haveStatus;
  s.stmStatus = gSnap.status;
  s.espLineOverflows = gSnap.lineOverflows;
  s.espLineMalformed = gSnap.lineMalformed;
  for (const vdm::ValveState& v : gSnap.valves)
    s.calibrationActive = s.calibrationActive || v.calibrating;
  const app::CalibInfo ci = app::calibInfo();
  s.lastScheduledCalibEpoch = ci.lastScheduledEpoch;
  s.nextCalibSlot = ci.nextSlot;
  s.authEnabled = authEnabled();
  s.lastEventSeq = logger::lastSeq();
  sendDocument(req, 200, [](vdm::JsonWriter& jw) { return vdm::writeStatusJson(jw, s); });
}

// Config slot (1-based) whose id is `id`, 0 if none.
uint8_t tempSlotOf(const vdm::OneWireId& id) {
  if (vdm::isZero(id)) return 0;
  for (uint8_t i = 0; i < vdm::kTempSlotCount; ++i) {
    if (gCfg.temps[i].id == id) return static_cast<uint8_t>(i + 1);
  }
  return 0;
}

void handleValves(AsyncWebServerRequest* req) {
  app::readStmSnapshot(gSnap);
  for (uint8_t i = 0; i < vdm::kValveCount; ++i) {
    vdm::ValveView& v = gValveViews[i];
    v = vdm::ValveView{};
    const vdm::ValveState& st = gSnap.valves[i];
    v.state = &st;
    v.config = &gCfg.valves[i];
    const int16_t raw[2] = {st.temp1, st.temp2};
    for (uint8_t k = 0; k < 2; ++k) {
      const uint8_t slot = st.sensorSlot[k];
      if (slot == 0 || slot > vdm::kTempSlotCount) continue;
      v.sensorSlot[k] = slot;
      v.sensorName[k] = gCfg.temps[slot - 1].name;
      v.sensorValid[k] = vdm::tempRawValid(raw[k]);
      v.sensorTenths[k] = static_cast<int32_t>(raw[k]) + gCfg.temps[slot - 1].offset;
    }
  }
  const uint32_t now = app::nowMs();
  sendDocument(req, 200, [now](vdm::JsonWriter& jw) {
    return vdm::writeValvesJson(jw, gValveViews.data(), vdm::kValveCount, now);
  });
}

void handleSensors(AsyncWebServerRequest* req) {
  app::readStmSnapshot(gSnap);
  const uint32_t now = app::nowMs();
  uint8_t nt = 0;
  // Configured temperature slots.
  for (uint8_t i = 0; i < vdm::kTempSlotCount; ++i) {
    const vdm::TempSlotConfig& c = gCfg.temps[i];
    if (vdm::isZero(c.id) && !c.active && c.name[0] == '\0') continue;
    vdm::SensorView& v = gTempViews[nt++];
    v = vdm::SensorView{};
    v.slot = static_cast<uint8_t>(i + 1);
    v.name = c.name;
    v.active = c.active;
    v.id = c.id;
    for (uint8_t b = 0; b < gSnap.tempCount && b < vdm::kTempSlotCount; ++b) {
      const vdm::TempReading& r = gSnap.temps[b];
      if (vdm::isZero(c.id) || r.id != c.id) continue;
      v.onBus = true;
      v.raw = r.raw;
      v.value = static_cast<int32_t>(r.raw) + c.offset;
      v.valid = r.seen && vdm::tempRawValid(r.raw) &&
                vdm::elapsedMs(now, r.lastSeenMs) <= kSensorStaleMs;
      v.ageS = r.seen ? vdm::elapsedMs(now, r.lastSeenMs) / 1000 : 0;
      break;
    }
    for (uint8_t k = 0; k < vdm::kValveCount; ++k) {
      if (gSnap.valves[k].sensorSlot[0] == v.slot || gSnap.valves[k].sensorSlot[1] == v.slot) {
        v.valve = k;
        break;
      }
    }
  }
  // Bus sensors without a config slot.
  for (uint8_t b = 0; b < gSnap.tempCount && b < vdm::kTempSlotCount; ++b) {
    const vdm::TempReading& r = gSnap.temps[b];
    if (vdm::isZero(r.id) || tempSlotOf(r.id) != 0) continue;
    vdm::SensorView& v = gTempViews[nt++];
    v = vdm::SensorView{};
    v.onBus = true;
    v.id = r.id;
    v.raw = r.raw;
    v.value = r.raw;
    v.valid = r.seen && vdm::tempRawValid(r.raw) &&
              vdm::elapsedMs(now, r.lastSeenMs) <= kSensorStaleMs;
    v.ageS = r.seen ? vdm::elapsedMs(now, r.lastSeenMs) / 1000 : 0;
  }
  uint8_t nv = 0;
  for (uint8_t i = 0; i < vdm::kVoltSlotCount; ++i) {
    const vdm::VoltSlotConfig& c = gCfg.volts[i];
    if (vdm::isZero(c.id) && !c.active && c.name[0] == '\0') continue;
    vdm::SensorView& v = gVoltViews[nv++];
    v = vdm::SensorView{};
    v.slot = static_cast<uint8_t>(i + 1);
    v.name = c.name;
    v.active = c.active;
    v.id = c.id;
    v.unit = c.unit;
    for (uint8_t b = 0; b < gSnap.voltCount && b < vdm::kVoltSlotCount; ++b) {
      const vdm::VoltReading& r = gSnap.volts[b];
      if (vdm::isZero(c.id) || r.id != c.id) continue;
      v.onBus = true;
      v.raw = r.vad;
      double milli = (static_cast<double>(r.vad) / 100.0 + c.offset) * c.factor * 1000.0;
      if (milli > 2e9) milli = 2e9;
      if (milli < -2e9) milli = -2e9;
      v.value = static_cast<int32_t>(milli);
      v.valid = r.seen && vdm::vadValid(r.vad) &&
                vdm::elapsedMs(now, r.lastSeenMs) <= kSensorStaleMs;
      v.ageS = r.seen ? vdm::elapsedMs(now, r.lastSeenMs) / 1000 : 0;
      break;
    }
  }
  for (uint8_t b = 0; b < gSnap.voltCount && b < vdm::kVoltSlotCount; ++b) {
    const vdm::VoltReading& r = gSnap.volts[b];
    if (vdm::isZero(r.id)) continue;
    bool configured = false;
    for (const vdm::VoltSlotConfig& c : gCfg.volts) configured = configured || c.id == r.id;
    if (configured) continue;
    vdm::SensorView& v = gVoltViews[nv++];
    v = vdm::SensorView{};
    v.onBus = true;
    v.id = r.id;
    v.raw = r.vad;
    v.valid = false;  // no offset/factor/unit without a slot
    v.ageS = r.seen ? vdm::elapsedMs(now, r.lastSeenMs) / 1000 : 0;
  }
  sendDocument(req, 200, [nt, nv](vdm::JsonWriter& jw) {
    return vdm::writeSensorsJson(jw, gTempViews.data(), nt, gVoltViews.data(), nv);
  });
}

// Strict uint query parameter; missing keeps `out`.
bool queryUint(AsyncWebServerRequest* req, const char* name, uint32_t max, uint32_t& out) {
  AsyncWebParameter* p = req->getParam(name);
  if (p == nullptr) return true;
  const String& v = p->value();
  return vdm::parseUint(v.c_str(), v.length(), max, out);
}

void handleEvents(AsyncWebServerRequest* req) {
  vdm::EventFilter f;
  uint32_t limit = kMaxEventsPerResponse;
  uint32_t valve = 0;
  if (!queryUint(req, "since", UINT32_MAX, f.sinceSeq) ||
      !queryUint(req, "limit", kMaxEventsPerResponse, limit) || limit == 0 ||
      !queryUint(req, "valve", vdm::kValveCount, valve)) {
    return sendError(req, 400, "bad_request", "since/limit/valve");
  }
  if (req->hasParam("valve")) {
    if (valve == 0) return sendError(req, 400, "bad_request", "valve");
    f.valve = static_cast<uint8_t>(valve - 1);
  }
  if (AsyncWebParameter* p = req->getParam("minSeverity")) {
    const String& v = p->value();
    if (!vdm::parseSeverity(v.c_str(), v.length(), f.minSeverity)) {
      return sendError(req, 400, "bad_request", "minSeverity");
    }
  }
  const int slot = acquireSlot();
  if (slot < 0) return sendError(req, 503, "busy", "response buffers in use");
  // The ring may hold more text than one slot: halve the count until it fits.
  for (size_t max = limit; max > 0; max /= 2) {
    uint32_t next = f.sinceSeq, first = 0, last = 0, dropped = 0;
    const size_t n = logger::read(f, gEvents.data(), max, next, first, last, dropped);
    vdm::JsonWriter jw(gSlots[slot].buf, kResponseSlotSize);
    if (vdm::writeEventsJson(jw, gEvents.data(), n, first, last, next, dropped) && jw.complete()) {
      return sendSlot(req, 200, slot, jw.length());
    }
  }
  releaseSlot(slot);
  sendError(req, 500, "internal", "events");
}

void handleProfileGet(AsyncWebServerRequest* req, uint8_t valve) {
  app::readStmSnapshot(gSnap);
  const vdm::Profile& p = gSnap.profiles[valve];
  if (p.count == 0) return sendError(req, 404, "not_found", "no profile");
  sendDocument(req, 200, [&p](vdm::JsonWriter& jw) { return vdm::writeProfileJson(jw, p); });
}

void handleMotorGet(AsyncWebServerRequest* req) {
  app::readStmSnapshot(gSnap);
  sendDocument(req, 200, [](vdm::JsonWriter& jw) {
    return vdm::writeMotorJson(jw, gSnap.motor, gSnap.learnMovements,
                               gSnap.haveBreakaway ? &gSnap.breakaway : nullptr, gSnap.haveMotor);
  });
}

void handleFlashStatus(AsyncWebServerRequest* req) {
  app::readStmSnapshot(gSnap);
  sendDocument(req, 200, [](vdm::JsonWriter& jw) {
    return vdm::writeFlashStatusJson(jw, gSnap.flash,
                                     gSnap.flashImage[0] ? gSnap.flashImage : nullptr);
  });
}

void writeImage(vdm::JsonWriter& jw, const storage::ImageEntry& e, bool crcKnown) {
  jw.beginObject();
  jw.kv("name", e.name);
  jw.kv("size", e.size);
  char buf[12];
  jw.key("crc32");
  if (crcKnown) {
    snprintf(buf, sizeof buf, "0x%08lx", static_cast<unsigned long>(e.crc));
    jw.value(buf);
  } else {
    jw.nullValue();
  }
  jw.key("version");
  if (e.scanned && e.version[0]) {
    jw.value(e.version);
  } else {
    jw.nullValue();
  }
  jw.key("check");
  if (e.scanned) {
    jw.value(vdm::flashErrorName(e.check));
  } else {
    jw.nullValue();
  }
  jw.endObject();
}

void handleImages(AsyncWebServerRequest* req) {
  const size_t n = storage::listImages(gImages.data(), storage::kImageSlots);
  sendDocument(req, 200, [n](vdm::JsonWriter& jw) {
    jw.beginArray();
    for (size_t i = 0; i < n; ++i) writeImage(jw, gImages[i], gImages[i].scanned);
    jw.endArray();
    return jw.ok();
  });
}

void handleConfigGet(AsyncWebServerRequest* req, bool attachment) {
  sendDocument(req, 200, [](vdm::JsonWriter& jw) { return vdm::writeConfigJson(jw, gCfg); },
               attachment ? "attachment; filename=\"vdmot-config.json\"" : nullptr);
}

// ---------------------------------------------------------------- log download

void closeLogStream() {
  if (gLog.file) gLog.file.close();
  gLog.owner = nullptr;
  gLog.part = 2;
}

size_t fillLog(uint8_t* buf, size_t maxLen) {
  while (gLog.part < 2) {
    if (!gLog.file) {
      gLog.file = LittleFS.open(gLog.part == 0 ? logger::kLogFileOld : logger::kLogFile, FILE_READ);
      if (!gLog.file) {
        ++gLog.part;
        continue;
      }
    }
    const size_t n = gLog.file.read(buf, maxLen);
    if (n > 0) return n;
    gLog.file.close();
    ++gLog.part;
  }
  return 0;  // end of the chunked response
}

void handleLog(AsyncWebServerRequest* req) {
  if (!storage::fsReady()) return sendError(req, 503, "unavailable", "no file system");
  if (gLog.owner != nullptr) return sendError(req, 409, "busy", "log download running");
  gLog.owner = req;
  gLog.part = 0;
  AsyncWebServerResponse* res = req->beginChunkedResponse(
      "text/plain; charset=utf-8",
      [](uint8_t* buf, size_t maxLen, size_t) -> size_t { return fillLog(buf, maxLen); });
  if (res == nullptr) {
    closeLogStream();
    return sendError(req, 500, "internal", "log");
  }
  res->addHeader("Content-Disposition", "attachment; filename=\"vdmot-events.log\"");
  req->onDisconnect([req]() {
    if (gLog.owner == req) closeLogStream();
  });
  req->send(res);
}

// ---------------------------------------------------------------- POST handlers

void handleTarget(AsyncWebServerRequest* req, uint8_t valve, bool hasBody) {
  if (!parseBody(req, hasBody)) return;
  JsonObjectConst o = gDoc.as<JsonObjectConst>();
  static const char* const kKeys[] = {"target"};
  int64_t target = 0;
  if (!onlyKeys(o, kKeys, 1) || !intField(o, "target", 0, 100, target, true)) {
    return sendError(req, 400, "out_of_range", "target 0..100");
  }
  if (!gCfg.valves[valve].active) return sendError(req, 409, "inactive", "valve not active");
  app::Command c;
  c.type = app::CommandType::SetTarget;
  c.valve = valve;
  c.pos = static_cast<uint8_t>(target);
  c.source = vdm::TargetSource::Web;
  if (!submitOr503(req, c)) return;
  char buf[48];
  snprintf(buf, sizeof buf, "{\"valve\":%u,\"target\":%u}", valve + 1u,
           static_cast<unsigned>(target));
  req->send(202, kJson, buf);
}

void handleSimple(AsyncWebServerRequest* req, app::CommandType type, uint8_t valve) {
  if (refuseWhileFlashing(req)) return;
  app::Command c;
  c.type = type;
  c.valve = valve;
  if (submitOr503(req, c)) sendAccepted(req);
}

void handleServiceMove(AsyncWebServerRequest* req, uint8_t valve, bool hasBody) {
  if (refuseWhileFlashing(req)) return;
  if (app::stmProtocol() < 2) return sendError(req, 409, "unsupported", "STM protocol v2 required");
  if (!parseBody(req, hasBody)) return;
  JsonObjectConst o = gDoc.as<JsonObjectConst>();
  static const char* const kKeys[] = {"dir", "counts", "maxmA"};
  const char* dir = o["dir"].is<const char*>() ? o["dir"].as<const char*>() : nullptr;
  int64_t counts = 0, maxmA = 0;
  if (!onlyKeys(o, kKeys, 3) || dir == nullptr ||
      (strcmp(dir, "open") != 0 && strcmp(dir, "close") != 0) ||
      !intField(o, "counts", 1, 10000, counts, true) || !intField(o, "maxmA", 5, 60, maxmA, true)) {
    return sendError(req, 400, "out_of_range", "dir open|close, counts 1..10000, maxmA 5..60");
  }
  app::Command c;
  c.type = app::CommandType::ServiceMove;
  c.valve = valve;
  c.dir = strcmp(dir, "open") == 0 ? vdm::MoveDir::Open : vdm::MoveDir::Close;
  c.counts = static_cast<uint16_t>(counts);
  c.maxmA = static_cast<uint8_t>(maxmA);
  if (submitOr503(req, c)) sendAccepted(req);
}

void handleValveSensors(AsyncWebServerRequest* req, uint8_t valve, bool hasBody) {
  if (refuseWhileFlashing(req)) return;
  if (!parseBody(req, hasBody)) return;
  JsonObjectConst o = gDoc.as<JsonObjectConst>();
  static const char* const kKeys[] = {"slot1", "slot2"};
  int64_t slot[2] = {0, 0};
  if (!onlyKeys(o, kKeys, 2) || !intField(o, "slot1", 0, vdm::kTempSlotCount, slot[0], true) ||
      !intField(o, "slot2", 0, vdm::kTempSlotCount, slot[1], true) ||
      (slot[0] != 0 && slot[0] == slot[1])) {
    return sendError(req, 400, "out_of_range", "slot1/slot2 0..34, distinct");
  }
  app::Command c;
  c.type = app::CommandType::SetValveSensors;
  c.valve = valve;
  for (uint8_t k = 0; k < 2; ++k) {
    if (slot[k] == 0) continue;
    const vdm::OneWireId& id = gCfg.temps[slot[k] - 1].id;
    if (vdm::isZero(id) || !vdm::crcValid(id)) {
      return sendError(req, 400, "invalid", k == 0 ? "slot1 has no valid sensor id"
                                                     : "slot2 has no valid sensor id");
    }
    c.ids[k] = id;
  }
  if (submitOr503(req, c)) sendAccepted(req);
}

void handleProfileRefresh(AsyncWebServerRequest* req, uint8_t valve) {
  if (app::stmProtocol() < 2) return sendError(req, 409, "unsupported", "STM protocol v2 required");
  handleSimple(req, app::CommandType::RequestProfile, valve);
}

void handleConfigPatch(AsyncWebServerRequest* req, bool hasBody) {
  if (!hasBody || gBodyLen == 0) return sendError(req, 400, "bad_request", "JSON body required");
  gPatch = gCfg;
  char path[72];
  const vdm::PatchResult r = vdm::applyConfigJson(gPatch, gBody, gBodyLen, path, sizeof path);
  if (r != vdm::PatchResult::Ok) return sendError(req, 400, "invalid", path);
  if (!storage::applyConfig(gPatch, path, sizeof path)) {
    return sendError(req, strcmp(path, "nvs") == 0 ? 500 : 400, "invalid", path);
  }
  refreshConfig();
  logger::log(vdm::EventCode::ConfigSaved, vdm::kNoValve,
              static_cast<int32_t>(storage::configRevision()), 0, "web");
  handleConfigGet(req, false);
}

void handleMotorSet(AsyncWebServerRequest* req, bool hasBody) {
  if (refuseWhileFlashing(req)) return;
  if (!parseBody(req, hasBody)) return;
  app::readStmSnapshot(gSnap);
  JsonObjectConst o = gDoc.as<JsonObjectConst>();
  static const char* const kRoot[] = {"motor", "learnMovements", "breakaway"};
  if (!onlyKeys(o, kRoot, 3))
    return sendError(req, 400, "unknown_key", "motor/learnMovements/breakaway");
  app::Command cmd;
  cmd.type = app::CommandType::SetMotorSettings;

  JsonVariantConst mv = o["motor"];
  if (!mv.isNull()) {
    static const char* const kKeys[] = {"lowC", "highC", "startOnPower", "noOfMinCount",
                                        "maxCalReps"};
    if (!mv.is<JsonObjectConst>() || !onlyKeys(mv.as<JsonObjectConst>(), kKeys, 5)) {
      return sendError(req, 400, "invalid", "motor");
    }
    JsonObjectConst m = mv.as<JsonObjectConst>();
    const bool complete = m.size() == 5;
    if (!complete && !gSnap.haveMotor) {
      return sendError(req, 409, "unknown", "motor parameters not read yet: send all five");
    }
    vdm::MotorChars mc = gSnap.motor;
    int64_t low = mc.lowFactor, high = mc.highFactor, sop = mc.startOnPower,
            minCnt = mc.minCounts, reps = mc.maxCalibRetries;
    if (!intField(m, "lowC", 10, 40, low, false) || !intField(m, "highC", 10, 40, high, false) ||
        !intField(m, "startOnPower", 0, 100, sop, false) ||
        !intField(m, "noOfMinCount", 0, 60000, minCnt, false) ||
        !intField(m, "maxCalReps", 0, 2, reps, false)) {
      return sendError(req, 400, "out_of_range", "motor");
    }
    mc.lowFactor = static_cast<uint8_t>(low);
    mc.highFactor = static_cast<uint8_t>(high);
    mc.startOnPower = static_cast<uint8_t>(sop);
    mc.minCounts = static_cast<uint16_t>(minCnt);
    mc.maxCalibRetries = static_cast<uint8_t>(reps);
    mc.fieldCount = 5;
    if (!vdm::motorCharsValid(mc)) return sendError(req, 400, "out_of_range", "motor");
    cmd.hasMotor = true;
    cmd.motor = mc;
  }
  int64_t learn = 0;
  if (!intField(o, "learnMovements", 0, 65534, learn, false) ||
      (!o["learnMovements"].isNull() && !vdm::learnMovementsValid(static_cast<uint32_t>(learn)))) {
    return sendError(req, 400, "out_of_range", "learnMovements 0 or 50..65534");
  }
  if (!o["learnMovements"].isNull()) {
    cmd.hasLearnMovements = true;
    cmd.learnMovements = static_cast<uint16_t>(learn);
  }
  JsonVariantConst bv = o["breakaway"];
  if (!bv.isNull()) {
    static const char* const kKeys[] = {"enable", "stepPct", "maxmA"};
    if (!bv.is<JsonObjectConst>() || !onlyKeys(bv.as<JsonObjectConst>(), kKeys, 3)) {
      return sendError(req, 400, "invalid", "breakaway");
    }
    if (app::stmProtocol() < 2)
      return sendError(req, 409, "unsupported", "STM protocol v2 required");
    JsonObjectConst b = bv.as<JsonObjectConst>();
    if (b.size() != 3 && !gSnap.haveBreakaway) {
      return sendError(req, 409, "unknown", "breakaway not read yet: send all three");
    }
    vdm::Breakaway ba = gSnap.breakaway;
    int64_t step = ba.stepPct, maxmA = ba.maxmA;
    if (!boolField(b, "enable", ba.enable, false) || !intField(b, "stepPct", 0, 100, step, false) ||
        !intField(b, "maxmA", 20, 60, maxmA, false)) {
      return sendError(req, 400, "out_of_range", "breakaway");
    }
    ba.stepPct = static_cast<uint8_t>(step);
    ba.maxmA = static_cast<uint8_t>(maxmA);
    if (!vdm::breakawayValid(ba)) return sendError(req, 400, "out_of_range", "breakaway");
    cmd.hasBreakaway = true;
    cmd.breakaway = ba;
  }
  if (!cmd.hasMotor && !cmd.hasLearnMovements && !cmd.hasBreakaway) {
    return sendError(req, 400, "bad_request", "nothing to set");
  }
  if (!app::submit(cmd)) return sendError(req, 503, "queue_full", "STM command queue full");
  sendAccepted(req);
}

void handleStmReset(AsyncWebServerRequest* req, bool hasBody) {
  if (refuseWhileFlashing(req)) return;
  if (!parseBody(req, hasBody)) return;
  JsonObjectConst o = gDoc.as<JsonObjectConst>();
  bool confirm = false;
  if (!boolField(o, "confirm", confirm, true) || !confirm) {
    return sendError(req, 400, "confirm_required", "{\"confirm\":true}");
  }
  handleSimple(req, app::CommandType::ResetStm, vdm::kNoValve);
}

void handleImageDelete(AsyncWebServerRequest* req, const char* rawName) {
  char name[storage::kImageNameMax + 1];
  if (!storage::normalizeImageName(rawName, strlen(rawName), name, sizeof name)) {
    return sendError(req, 400, "bad_name", rawName);
  }
  if (app::stmFlashActive()) return sendError(req, 409, "flashing", "STM flash in progress");
  switch (storage::deleteImage(name)) {
    case storage::ImageResult::Ok: return req->send(204);
    case storage::ImageResult::NotFound: return sendError(req, 404, "not_found", name);
    case storage::ImageResult::Busy: return sendError(req, 409, "busy", name);
    default: return sendError(req, 500, "io_error", name);
  }
}

void handleFlash(AsyncWebServerRequest* req, bool hasBody) {
  if (!parseBody(req, hasBody)) return;
  JsonObjectConst o = gDoc.as<JsonObjectConst>();
  static const char* const kKeys[] = {"image", "mode", "force"};
  const char* image = o["image"].is<const char*>() ? o["image"].as<const char*>() : nullptr;
  const char* mode = o["mode"].isNull() ? "normal"
                                        : (o["mode"].is<const char*>() ? o["mode"].as<const char*>()
                                                                       : "");
  bool force = false;
  char name[storage::kImageNameMax + 1];
  if (!onlyKeys(o, kKeys, 3) || image == nullptr ||
      !storage::normalizeImageName(image, strlen(image), name, sizeof name) ||
      (strcmp(mode, "normal") != 0 && strcmp(mode, "blank") != 0) ||
      !boolField(o, "force", force, false)) {
    return sendError(req, 400, "bad_request", "image, mode normal|blank, force");
  }
  if (uploadBusy()) return sendError(req, 409, "busy", "upload or flash running");
  if (ota::restartPending()) return sendError(req, 409, "restarting", "ESP restart pending");
  storage::ImageEntry e;
  if (!storage::findImage(name, e)) return sendError(req, 404, "not_found", name);
  if (!e.scanned) return sendError(req, 409, "validating", "image check pending, retry");
  if (e.check != vdm::FlashError::None &&
      !(force && e.check == vdm::FlashError::ImageNoHandshake)) {
    return sendError(req, 400, "invalid_image", vdm::flashErrorName(e.check));
  }
  app::Command c;
  c.type = app::CommandType::StartFlash;
  vdm::copyString(c.image, sizeof c.image, name);
  c.blank = strcmp(mode, "blank") == 0;
  c.force = force;
  if (submitOr503(req, c)) sendAccepted(req);
}

void handleFlashAbort(AsyncWebServerRequest* req) {
  if (!app::stmFlashActive()) return sendError(req, 409, "idle", "no flash running");
  app::Command c;
  c.type = app::CommandType::AbortFlash;
  if (submitOr503(req, c)) sendAccepted(req);
}

void handleFactoryReset(AsyncWebServerRequest* req, bool hasBody) {
  if (!parseBody(req, hasBody)) return;
  JsonObjectConst o = gDoc.as<JsonObjectConst>();
  const char* confirm = o["confirm"].is<const char*>() ? o["confirm"].as<const char*>() : "";
  if (strcmp(confirm, "factory-reset") != 0) {
    return sendError(req, 400, "confirm_required", "{\"confirm\":\"factory-reset\"}");
  }
  if (app::stmFlashActive()) return sendError(req, 409, "flashing", "STM flash in progress");
  if (!storage::factoryReset()) return sendError(req, 500, "nvs", "erase failed");
  ota::requestRestart(3, 1000);
  req->send(202, kJson, "{\"result\":\"restarting\"}");
}

void handleDiscovery(AsyncWebServerRequest* req, bool hasBody) {
  if (!parseBody(req, hasBody)) return;
  if (gCfg.mqtt.mode != vdm::MqttMode::MqttHa) {
    return sendError(req, 409, "disabled", "MQTT + HA mode required");
  }
  JsonObjectConst o = gDoc.as<JsonObjectConst>();
  const char* action = o["action"].is<const char*>() ? o["action"].as<const char*>() : "";
  mqtt::DiscoveryAction a;
  if (strcmp(action, "publish") == 0) {
    a = mqtt::DiscoveryAction::Publish;
  } else if (strcmp(action, "delete") == 0) {
    a = mqtt::DiscoveryAction::Delete;
  } else if (strcmp(action, "republish") == 0) {
    a = mqtt::DiscoveryAction::DeleteAndPublish;
  } else {
    return sendError(req, 400, "bad_request", "action publish|delete|republish");
  }
  mqtt::requestDiscovery(a);
  sendAccepted(req);
}

// ---------------------------------------------------------------- uploads

void resetUpload() { gUpload = Upload{}; }

void failUpload(uint16_t code, const char* error) {
  if (gUpload.failCode != 0) return;
  gUpload.failCode = code;
  vdm::copyString(gUpload.error, sizeof gUpload.error, error);
  if (gUpload.kind == UploadKind::StmImage) storage::imageUploadAbort();
  if (gUpload.kind == UploadKind::EspOta) ota::uploadEnd(false);
}

uint16_t imageHttpCode(storage::ImageResult r) {
  switch (r) {
    case storage::ImageResult::BadName: return 400;
    case storage::ImageResult::Empty: return 400;
    case storage::ImageResult::TooLarge: return 413;
    case storage::ImageResult::NoSpace:
    case storage::ImageResult::TooMany: return 507;
    case storage::ImageResult::Busy: return 409;
    default: return 500;
  }
}

// Expected MD5 of an ESP image: query "md5" (what the dashboard sends), a
// form field "md5"/"MD5" before the file part, or the X-Update-MD5 / X-MD5
// header. "" when none was given. The pointer lives as long as `req`.
const char* uploadMd5(AsyncWebServerRequest* req) {
  static const char* const kParams[] = {"md5", "MD5"};
  for (const char* name : kParams) {
    if (AsyncWebParameter* p = req->getParam(name)) return p->value().c_str();
    if (AsyncWebParameter* p = req->getParam(name, true)) return p->value().c_str();
  }
  static const char* const kHeaders[] = {"X-Update-MD5", "X-MD5"};
  for (const char* name : kHeaders) {
    if (AsyncWebHeader* h = req->getHeader(name)) return h->value().c_str();
  }
  return "";
}

void beginUpload(AsyncWebServerRequest* req, UploadKind kind, const String& filename) {
  resetUpload();
  gUpload.owner = req;
  gUpload.kind = kind;
  gUpload.started = true;
  // Client gone before the request completed: abort and free the state.
  req->onDisconnect([req]() {
    if (gUpload.owner != req) return;
    failUpload(499, "client disconnected");
    resetUpload();
  });
  if (kind == UploadKind::StmImage) {
    const storage::ImageResult r =
        storage::imageUploadBegin(filename.c_str(), req->contentLength());
    if (r != storage::ImageResult::Ok) {
      gUpload.kind = UploadKind::None;  // nothing to abort
      failUpload(imageHttpCode(r), storage::imageResultName(r));
    }
    return;
  }
  if (!ota::uploadBegin(req->contentLength(), uploadMd5(req))) {
    gUpload.kind = UploadKind::None;
    failUpload(strcmp(ota::uploadError(), "busy") == 0 ? 409 : 400, ota::uploadError());
  }
}

void onUpload(AsyncWebServerRequest* req, const String& filename, size_t index, uint8_t* data,
              size_t len, bool final) {
  refreshConfig();
  const vdm::RouteMatch m = route(req);
  const UploadKind kind = m.route == vdm::ApiRoute::StmImageUpload ? UploadKind::StmImage
                          : m.route == vdm::ApiRoute::EspOta       ? UploadKind::EspOta
                                                                   : UploadKind::None;
  if (kind == UploadKind::None) return;  // the guard answers everything else
  if (index == 0) {
    if (gUpload.owner != nullptr && gUpload.owner != req) {
      mark(req, 409, "busy");
      return;
    }
    if (gUpload.owner == req && gUpload.started) {
      failUpload(400, "one file per request");
      return;
    }
    beginUpload(req, kind, filename);
  }
  if (gUpload.owner != req || gUpload.failCode != 0) return;
  if (gUpload.kind == UploadKind::StmImage) {
    const storage::ImageResult r = storage::imageUploadWrite(data, len);
    if (r != storage::ImageResult::Ok) {
      gUpload.kind = UploadKind::None;  // storage already removed the part file
      failUpload(imageHttpCode(r), storage::imageResultName(r));
      return;
    }
    if (final) {
      const storage::ImageResult e = storage::imageUploadEnd(gUpload.info);
      gUpload.kind = UploadKind::None;
      if (e != storage::ImageResult::Ok)
        return failUpload(imageHttpCode(e), storage::imageResultName(e));
      gUpload.done = true;
    }
  } else {
    if (!ota::uploadWrite(data, len)) {
      gUpload.kind = UploadKind::None;  // ota aborted itself
      failUpload(500, ota::uploadError());
      return;
    }
    if (final) {
      gUpload.kind = UploadKind::None;
      if (!ota::uploadEnd(true)) return failUpload(500, ota::uploadError());
      gUpload.done = true;
    }
  }
}

void finishUpload(AsyncWebServerRequest* req, vdm::ApiRoute r) {
  Mark mk;
  if (takeMark(req, mk)) return sendError(req, mk.code, mk.error, "upload");
  if (gUpload.owner != req) return sendError(req, 400, "bad_request", "no file in request");
  if (!gUpload.done && gUpload.failCode == 0) failUpload(400, "incomplete file");
  const Upload u = gUpload;
  resetUpload();
  if (u.failCode != 0) return sendError(req, u.failCode, "upload_failed", u.error);
  if (r == vdm::ApiRoute::EspOta) {
    req->send(200, kJson, "{\"result\":\"ok\",\"restart\":true}");
    return;
  }
  char buf[160];
  vdm::JsonWriter jw(buf, sizeof buf);
  writeImage(jw, u.info, true);
  req->send(201, kJson, jw.complete() ? jw.c_str() : "{}");
}

// ---------------------------------------------------------------- dispatch

void handleApi(AsyncWebServerRequest* req, bool hasBody) {
  const vdm::RouteMatch m = route(req);
  const String& url = req->url();
  if (m.route == vdm::ApiRoute::NotFound) return sendError(req, 404, "not_found", url.c_str());
  if (m.route == vdm::ApiRoute::MethodNotAllowed) {
    return sendError(req, 405, "method_not_allowed", url.c_str());
  }
  if (isUploadRoute(m.route)) return finishUpload(req, m.route);
  if (!authorised(req, m.needsAuth)) return;
  if (hasBody && gBodyOverflow) return sendError(req, 413, "too_large", "body");

  const uint8_t v = m.valve;
  using R = vdm::ApiRoute;
  using C = app::CommandType;
  switch (m.route) {
    case R::Status: return handleStatus(req);
    case R::Valves: return handleValves(req);
    case R::ValveTarget: return handleTarget(req, v, hasBody);
    case R::ValveCalibrate: return handleSimple(req, C::Calibrate, v);
    case R::ValveAssembly: return handleSimple(req, C::Assembly, v);
    case R::ValveServiceMove: return handleServiceMove(req, v, hasBody);
    case R::ValveSensors: return handleValveSensors(req, v, hasBody);
    case R::ValveProfile: return handleProfileGet(req, v);
    case R::ValveProfileRefresh: return handleProfileRefresh(req, v);
    case R::CalibrateAll: return handleSimple(req, C::Calibrate, vdm::kAllValves);
    case R::AssemblyAll: return handleSimple(req, C::Assembly, vdm::kAllValves);
    case R::Detect: return handleSimple(req, C::Detect, vdm::kAllValves);
    case R::Sensors: return handleSensors(req);
    case R::SensorsScan: return handleSimple(req, C::ScanSensors, vdm::kNoValve);
    case R::Events: return handleEvents(req);
    case R::ConfigGet: return handleConfigGet(req, false);
    case R::ConfigPatch: return handleConfigPatch(req, hasBody);
    case R::ConfigExport: return handleConfigGet(req, true);
    case R::Motor: return handleMotorGet(req);
    case R::MotorSet: return handleMotorSet(req, hasBody);
    case R::StmReset: return handleStmReset(req, hasBody);
    case R::StmImages: return handleImages(req);
    case R::StmImageDelete: return handleImageDelete(req, m.name);
    case R::StmFlash: return handleFlash(req, hasBody);
    case R::StmFlashStatus: return handleFlashStatus(req);
    case R::StmFlashAbort: return handleFlashAbort(req);
    case R::Reboot:
      ota::requestRestart(0, 1000);
      return req->send(202, kJson, "{\"result\":\"restarting\"}");
    case R::FactoryReset: return handleFactoryReset(req, hasBody);
    case R::MqttReconnect:
      mqtt::requestReconnect();
      return sendAccepted(req);
    case R::MqttDiscovery: return handleDiscovery(req, hasBody);
    case R::LogDownload: return handleLog(req);
    // Routes of 2.1 whose handlers are not there yet.
    default: return sendError(req, 501, "not_implemented", url.c_str());
  }
}

void handleStatic(AsyncWebServerRequest* req) {
  const String& url = req->url();
  const char* path = url == "/" ? "/index.html" : url.c_str();
  for (size_t i = 0; i < web_assets::kAssetCount; ++i) {
    const web_assets::Asset& a = web_assets::kAssets[i];
    if (strcmp(a.path, path) != 0) continue;
    AsyncWebHeader* inm = req->getHeader("If-None-Match");
    if (inm != nullptr && inm->value() == a.etag) return req->send(304);
    AsyncWebServerResponse* res = req->beginResponse_P(200, a.contentType, a.data, a.length);
    if (res == nullptr) return req->send(500);  // out of memory
    res->addHeader("Content-Encoding", "gzip");
    res->addHeader("ETag", a.etag);
    res->addHeader("Cache-Control", "no-cache");
    return req->send(res);
  }
  sendError(req, 404, "not_found", url.c_str());
}

class ApiHandler : public AsyncWebHandler {
 public:
  bool canHandle(AsyncWebServerRequest* req) override {
    req->addInterestingHeader("Authorization");
    req->addInterestingHeader("If-None-Match");
    req->addInterestingHeader("X-Update-MD5");
    req->addInterestingHeader("X-MD5");
    return true;
  }

  void handleRequest(AsyncWebServerRequest* req) override {
    refreshConfig();
    const bool hasBody = gBodyOwner == req;
    Mark mk;
    if (req->contentLength() > 0 && !isUploadRoute(route(req).route) && takeMark(req, mk)) {
      sendError(req, mk.code, mk.error, "body");
    } else if (req->url().startsWith("/api/")) {
      handleApi(req, hasBody);
    } else if (req->method() == HTTP_GET) {
      handleStatic(req);
    } else {
      sendError(req, 405, "method_not_allowed", req->url().c_str());
    }
    if (gBodyOwner == req) gBodyOwner = nullptr;
  }

  void handleBody(AsyncWebServerRequest* req, uint8_t* data, size_t len, size_t index,
                  size_t total) override {
    if (index == 0) {
      clearMark(req);
      if (gBodyOwner != nullptr && gBodyOwner != req) {
        mark(req, 409, "busy");
        return;
      }
      gBodyOwner = req;
      gBodyLen = 0;
      gBodyOverflow = total > kMaxBodySize;
      gBody[0] = '\0';
      // Client gone mid-body: free the buffer for the next request.
      req->onDisconnect([req]() {
        if (gBodyOwner == req) gBodyOwner = nullptr;
      });
    }
    if (gBodyOwner != req || gBodyOverflow) return;
    if (len > kMaxBodySize - gBodyLen) {
      gBodyOverflow = true;
      return;
    }
    memcpy(gBody + gBodyLen, data, len);
    gBodyLen += len;
    gBody[gBodyLen] = '\0';
  }

  void handleUpload(AsyncWebServerRequest* req, const String& filename, size_t index,
                    uint8_t* data, size_t len, bool final) override {
    onUpload(req, filename, index, data, len, final);
  }

  bool isRequestHandlerTrivial() override { return false; }
};

GuardHandler gGuard;
ApiHandler gApi;

}  // namespace

void begin() {
  if (gStarted) return;
  for (Slot& s : gSlots) {
    s.buf = new (std::nothrow) char[kResponseSlotSize];
    s.busy = false;
  }
  refreshConfig();
  gServer.addHandler(&gGuard);
  gServer.addHandler(&gApi);
  gServer.begin();
  gStarted = true;
}

bool started() { return gStarted; }

}  // namespace web
