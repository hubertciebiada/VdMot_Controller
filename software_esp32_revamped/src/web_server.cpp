#include "web_server.h"

#include <Arduino.h>
#include <esp_heap_caps.h>
#include <ArduinoJson.h>
#include <AsyncTCP.h>
#include <AsyncWebServer_WT32_ETH01.h>
#include <LittleFS.h>
#include <algorithm>
#include <cstddef>
#include <esp_ota_ops.h>
#include <esp_system.h>
#include <new>
#include <string.h>
#include <time.h>
#include <type_traits>

#include <vdm/config.h>
#include <vdm/file_manager.h>
#include <vdm/json_api.h>
#include <vdm/json_writer.h>
#include <vdm/legacy_http.h>
#include <vdm/web_guard.h>

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
constexpr size_t kMaxEventsPerResponse = 50;  // ?limit= of GET /api/events
// The ring holds at most logger::kEventCapacity events (32 in the firmware),
// so no read fills more of the event buffer.
constexpr size_t kEventsBuffer = std::min(kMaxEventsPerResponse, logger::kEventCapacity);
constexpr size_t kMaxFilesPerResponse = 32;
constexpr size_t kHealthBufSize = 1024;

AsyncWebServer gServer(80);
bool gStarted = false;

struct Slot {
  char* buf;
  bool busy;
};
Slot gSlots[kResponseSlots];

// Working set of the request handlers, about 10 KB in 5 blocks. Each part gets
// its buffer in the first request (refreshConfig()) and keeps it, like the
// response slots (web_server.h): without a web client the heap keeps these
// bytes. JSON bodies go into a response slot (handleBody()), a config patch
// takes its Config copy from the heap for that request.
//
// What a handler builds for its response (views, event, image and file lists,
// the status and health snapshots, the health text, a profile) is dead when
// the handler returns, and handlers never overlap (they all run in the
// AsyncTCP task, one after another), so all of it shares one scratch buffer
// of the largest (scratch()).
using ValveViews = ObjArray<vdm::ValveView, vdm::kValveCount>;
struct SensorViews {
  vdm::SensorView temps[2 * vdm::kTempSlotCount];  // slots + unconfigured
  vdm::SensorView volts[2 * vdm::kVoltSlotCount];
};
using Events = ObjArray<vdm::Event, kEventsBuffer>;
using Images = ObjArray<storage::ImageEntry, storage::kImageSlots>;
using Files = ObjArray<vdm::FileEntry, kMaxFilesPerResponse>;
struct Health {  // GET /api/health, outside the slots
  vdm::HealthSnapshot snap;
  char text[kHealthBufSize];
};
constexpr size_t kScratchSize =
    std::max({sizeof(ValveViews), sizeof(SensorViews), sizeof(Events), sizeof(Images),
              sizeof(Files), sizeof(Health), sizeof(vdm::StatusSnapshot), sizeof(vdm::Profile)});
struct Scratch {
  alignas(std::max_align_t) unsigned char bytes[kScratchSize];
};
using GuardDetail = ObjArray<char, 160>;  // detail of the current guard refusal
app::StmSnapshot* gSnapPtr = nullptr;
vdm::Config* gCfgPtr = nullptr;
StaticJsonDocument<512>* gDocPtr = nullptr;
Scratch* gScratchPtr = nullptr;
GuardDetail* gGuardDetailPtr = nullptr;

template <typename T>
bool allocOnce(T*& p) {
  if (p == nullptr) p = new (std::nothrow) T();
  return p != nullptr;
}

// Every part, or false: the request is answered 503.
bool allocWork() {
  bool ok = allocOnce(gSnapPtr);
  ok = allocOnce(gCfgPtr) && ok;
  ok = allocOnce(gDocPtr) && ok;
  ok = allocOnce(gScratchPtr) && ok;
  return allocOnce(gGuardDetailPtr) && ok;
}

// The scratch buffer as a fresh T for the calling handler.
template <typename T>
T& scratch() {
  static_assert(sizeof(T) <= sizeof(Scratch::bytes), "scratch buffer too small");
  static_assert(std::is_trivially_destructible<T>::value, "the scratch is never destroyed");
  return *new (gScratchPtr->bytes) T();
}

// The JSON body of gBodyOwner (one body at a time) is received into a
// response slot, gBodySlot: a handler consumes the body (parseBody(),
// applyConfigJson()) before it writes its response, so the response takes
// that slot (acquireSlot()). Without a free slot the body is refused (503).
AsyncWebServerRequest* gBodyOwner = nullptr;
int gBodySlot = -1;  // -1 once the response took it
char* gBody = nullptr;
size_t gBodyLen = 0;
bool gBodyOverflow = false;

// Requests whose body was refused while it arrived (answered in
// handleRequest). A mark is cleared when its request starts a new body or
// takes over the upload (both replace the callback of mark()) and when it
// disconnects (every request ends that way, also one that never reached
// handleRequest), so a recycled request address never inherits a stale mark.
// One slot per connection the server holds (kMaxConnections, the listen
// backlog of AsyncTCP): every marked request has its own (AsyncTCP runs the
// disconnect of a closed connection before the data that arrives after it),
// so the table never runs full.
struct Mark {
  AsyncWebServerRequest* req;
  uint16_t code;
  const char* error;
  const char* detail;
};
Mark gMarks[kMaxConnections];

// Upload in progress (STM image or ESP firmware).
enum class UploadKind : uint8_t { None, StmImage, EspOta };
struct Upload {
  AsyncWebServerRequest* owner = nullptr;
  UploadKind kind = UploadKind::None;
  bool started = false;
  bool done = false;
  uint16_t failCode = 0;
  char error[48] = {};
  storage::ImageEntry info;
} gUpload;

// /api/log download (one at a time).
struct LogStream {
  AsyncWebServerRequest* owner = nullptr;
  fs::File file;
  uint8_t part = 0;  // 0 = events.1.log, 1 = events.log, 2 = done
} gLog;

uint32_t gCfgRevision = UINT32_MAX;
vdm::RepeatLimiter gRefusedLimiter;  // RequestRefused once per verdict per 60 s
char gHostname[vdm::kStationNameMax + 1] = {};   // buildHostname(gCfgPtr->station)

// First step of every request (GuardHandler::canHandle -> refusal()): the
// working set, then the config. false: no memory for the working set.
bool refreshConfig() {
  if (!allocWork()) return false;
  const uint32_t rev = storage::configRevision();
  if (rev == gCfgRevision) return true;
  gCfgRevision = rev;
  storage::getConfig(*gCfgPtr);
  vdm::buildHostname(gCfgPtr->station, gHostname, sizeof gHostname);
  return true;
}

uint32_t remoteIp(AsyncWebServerRequest* req) {
  return static_cast<uint32_t>(req->client()->remoteIP());
}

// ---------------------------------------------------------------- responses

// The slot for the response to `req`: the slot of its body when it has one
// (consumed by then), else a free one; -1 when every slot is busy.
int acquireSlot(AsyncWebServerRequest* req) {
  if (req == gBodyOwner && gBodySlot >= 0) {
    const int slot = gBodySlot;
    gBodySlot = -1;
    return slot;
  }
  for (size_t i = 0; i < kResponseSlots; ++i) {
    Slot& s = gSlots[i];
    if (s.busy) continue;
    // First use: the buffer is kept from now on (web_server.h).
    if (s.buf == nullptr) s.buf = new (std::nothrow) char[kResponseSlotSize];
    if (s.buf == nullptr) continue;
    s.busy = true;
    return static_cast<int>(i);
  }
  return -1;
}

void releaseSlot(int slot) {
  if (slot >= 0) gSlots[slot].busy = false;  // -1: a body slot the response took
}

// The body of `req` ends (request answered or client gone): its slot goes
// back to the pool unless the response took it.
void endBody(AsyncWebServerRequest* req) {
  if (gBodyOwner != req) return;
  releaseSlot(gBodySlot);
  gBodyOwner = nullptr;  // gBodySlot counts only for the owner
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
  const int slot = acquireSlot(req);
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
    if (m.req == req) m = Mark{};
  }
}

// Only for a request that owns nothing else (its onDisconnect is free).
void mark(AsyncWebServerRequest* req, uint16_t code, const char* error, const char* detail) {
  req->onDisconnect([req]() { clearMark(req); });
  for (Mark& m : gMarks) {
    if (m.req == nullptr || m.req == req) {
      m = Mark{req, code, error, detail};
      return;
    }
  }
}

bool takeMark(AsyncWebServerRequest* req, Mark& out) {
  for (Mark& m : gMarks) {
    if (m.req == req && req != nullptr) {
      out = m;
      m = Mark{};
      return true;
    }
  }
  return false;
}

// ---------------------------------------------------------------- routing

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
  return vdm::matchApiRoute(methodOf(req), url.c_str(), url.length());
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
  vdm::GuardVerdict verdict = vdm::GuardVerdict::Allow;
};

const char* headerValue(AsyncWebServerRequest* req, const char* name, size_t& len) {
  AsyncWebHeader* h = req->getHeader(name);
  if (h == nullptr) {
    len = 0;
    return nullptr;
  }
  len = h->value().length();
  return h->value().c_str();
}

// K5 guard of an /api/* request or a legacy alias; true = refused.
bool guardRefusal(AsyncWebServerRequest* req, vdm::GuardScope scope, bool upload,
                  Refusal& out) {
  vdm::GuardRequest g;
  g.method = methodOf(req);
  g.scope = scope;
  g.upload = upload;
  g.hasBody = req->contentLength() > 0;
  const String& host = req->host();
  g.host = host.c_str();
  g.hostLen = host.length();
  g.origin = headerValue(req, "Origin", g.originLen);
  g.marker = headerValue(req, "X-VdMot", g.markerLen);
  const String& type = req->contentType();
  g.contentType = type.c_str();
  g.contentTypeLen = type.length();
  vdm::HostPolicy p;
  p.localIp = static_cast<uint32_t>(req->client()->localIP());
  p.ifaceIp = net::info().ip;
  p.hostname = gHostname;
  p.allowed = gCfgPtr->web.allowedHosts;
  const vdm::GuardVerdict v = vdm::checkRequest(g, p);
  if (v == vdm::GuardVerdict::Allow) return false;
  vdm::guardDetail(v, g, p, gGuardDetailPtr->data(), sizeof gGuardDetailPtr->items);
  out = Refusal{vdm::guardHttpStatus(v), vdm::guardErrorCode(v), gGuardDetailPtr->data(), v};
  return true;
}

// Paths outside /api/: the 410 table, 405 for a legacy alias with another
// method, the alias guard, and every other request with a body (404/405,
// never buffered).
bool legacyRefusal(AsyncWebServerRequest* req, Refusal& out) {
  const String& url = req->url();
  const size_t len = req->contentLength();
  const vdm::LegacyMatch lm = vdm::matchLegacyRoute(methodOf(req), url.c_str(), url.length());
  switch (lm.route) {
    case vdm::LegacyRoute::Gone:
      out = Refusal{410, "gone", lm.replacement};
      return true;
    case vdm::LegacyRoute::MethodNotAllowed:
      out = Refusal{405, "method_not_allowed", url.c_str()};
      return true;
    case vdm::LegacyRoute::Valves:
    case vdm::LegacyRoute::Temps:
    case vdm::LegacyRoute::Volts:
      return guardRefusal(req, vdm::GuardScope::LegacyRead, false, out);
    case vdm::LegacyRoute::SetValve:
      if (guardRefusal(req, vdm::GuardScope::LegacyWrite, false, out)) return true;
      if (len > kMaxBodySize) out = Refusal{413, "too_large", "body"};
      return out.code != 0;
    default:
      break;
  }
  if (len == 0) return false;
  if (req->method() == HTTP_GET) {
    out = Refusal{404, "not_found", url.c_str()};
  } else {
    out = Refusal{405, "method_not_allowed", url.c_str()};
  }
  return true;
}

// Requests that must be answered without buffering their body.
bool refusal(AsyncWebServerRequest* req, Refusal& out) {
  if (!refreshConfig()) {
    out = Refusal{503, "busy", "out of memory"};
    return true;
  }
  const size_t len = req->contentLength();
  const bool multipart = req->contentType().startsWith("multipart/");
  if (!req->url().startsWith("/api/")) return legacyRefusal(req, out);
  const vdm::RouteMatch m = route(req);
  const bool upload = m.route != vdm::ApiRoute::NotFound &&
                      m.route != vdm::ApiRoute::MethodNotAllowed && isUploadRoute(m.route);
  if (guardRefusal(req, vdm::GuardScope::Api, upload, out)) return true;
  if (upload) {
    if (!multipart) {
      out = Refusal{415, "unsupported_media_type", "multipart/form-data required"};
    } else if (len == 0) {
      out = Refusal{411, "length_required", "Content-Length"};
    } else if (len > uploadLimit(m.route)) {
      out = Refusal{413, "too_large", "file"};
    } else if (uploadBusy()) {
      out = Refusal{409, "busy", "upload or flash running"};
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

void keepGuardHeaders(AsyncWebServerRequest* req) {
  req->addInterestingHeader("Origin");
  req->addInterestingHeader("X-VdMot");
}

class GuardHandler : public AsyncWebHandler {
 public:
  bool canHandle(AsyncWebServerRequest* req) override {
    Refusal r;
    if (!refusal(req, r)) return false;
    keepGuardHeaders(req);
    return true;
  }
  void handleRequest(AsyncWebServerRequest* req) override {
    net::noteInboundHttp(remoteIp(req));
    Refusal r;
    if (!refusal(req, r)) return sendError(req, 503, "retry", "state changed");
    if (r.verdict != vdm::GuardVerdict::Allow &&
        gRefusedLimiter.allow(static_cast<uint8_t>(r.verdict), app::nowMs())) {
      char ip[16];
      vdm::formatIpv4(remoteIp(req), ip, sizeof ip);
      logger::log(vdm::EventCode::RequestRefused, vdm::kNoValve,
                  static_cast<int32_t>(r.verdict), 0, ip);
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
  gDocPtr->clear();
  const DeserializationError e = deserializeJson(*gDocPtr, gBody, gBodyLen);
  if (e || !gDocPtr->is<JsonObject>()) {
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

// STM firmware older than 1.4.0: STM actions except target, reset and flash
// are refused.
bool refuseTooOld(AsyncWebServerRequest* req) {
  if (app::stmSupport() != vdm::StmSupport::TooOld) return false;
  app::readStmSnapshot(*gSnapPtr);
  char version[40];
  if (vdm::formatVersion(gSnapPtr->version, version, sizeof version) == 0) {
    vdm::copyString(version, sizeof version, "?");
  }
  char detail[96];
  snprintf(detail, sizeof detail, "STM firmware %s is older than 1.4.0: update the STM", version);
  sendError(req, 409, "stm_unsupported", detail);
  return true;
}

// Protocol 3 commands (stop, safe mode).
bool refuseBelowV3(AsyncWebServerRequest* req) {
  if (refuseTooOld(req)) return true;
  if (app::stmProtocol() >= 3) return false;
  sendError(req, 409, "stm_unsupported", "STM protocol 3 required");
  return true;
}

// ---------------------------------------------------------------- GET handlers

void handleStatus(AsyncWebServerRequest* req) {
  app::readStmSnapshot(*gSnapPtr);
  vdm::StatusSnapshot& s = scratch<vdm::StatusSnapshot>();
  s.espVersion = vdm::firmwareVersion();
#ifdef VDM_BUILD_EPOCH
  s.buildEpoch = static_cast<uint32_t>(VDM_BUILD_EPOCH);
#endif
  s.uptimeS = app::uptimeS();
  s.resetReason = static_cast<uint8_t>(esp_reset_reason());
  s.bootCount = storage::bootCount();
  s.freeHeap = heap_caps_get_free_size(MALLOC_CAP_8BIT);
  s.minFreeHeap = heap_caps_get_minimum_free_size(MALLOC_CAP_8BIT);
  s.largestFreeBlock = heap_caps_get_largest_free_block(MALLOC_CAP_8BIT);
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
  vdm::buildHostname(gCfgPtr->station, s.hostname, sizeof s.hostname);
  const mqtt::Status ms = mqtt::status();
  s.mqtt = ms.state;
  s.mqttRc = ms.rc;
  s.mqttReconnects = ms.reconnects;
  s.mqttPublishFailures = ms.publishFailures;
  s.link = gSnapPtr->link;
  s.linkStats = gSnapPtr->linkStats;
  s.stmProto = gSnapPtr->proto;
  s.stmVersion = gSnapPtr->version;
  s.stmBuild = gSnapPtr->build;
  s.stmHwId = gSnapPtr->hwId;
  s.stmCompatible = gSnapPtr->compatible;
  s.haveStmStatus = gSnapPtr->haveStatus;
  s.stmStatus = gSnapPtr->status;
  s.espLineOverflows = gSnapPtr->lineOverflows;
  s.espLineMalformed = gSnapPtr->lineMalformed;
  for (const vdm::ValveState& v : gSnapPtr->valves)
    s.calibrationActive = s.calibrationActive || v.calibrating;
  const app::CalibInfo ci = app::calibInfo();
  s.lastScheduledCalibEpoch = ci.lastScheduledEpoch;
  s.nextCalibSlot = ci.nextSlot;
  s.lastEventSeq = logger::lastSeq();
  s.station = gCfgPtr->station;
  const net::TrialInfo trial = net::trialInfo();
  s.netTrialActive = trial.active;
  s.netTrialRemainS = trial.remainS;
  vdm::copyString(s.mqttClientId, sizeof s.mqttClientId, ms.clientId);
  s.mqttHaStatus = ms.haStatus;
  s.stmSupport = gSnapPtr->support;
  s.lease = gSnapPtr->lease;
  s.haveLearnTime = gSnapPtr->haveLearnTime;
  s.learnTimeS = gSnapPtr->learnTimeS;
  s.nextCalibEpoch = ci.nextEpoch;
  if (ci.nextEpoch > 0) {
    const time_t t = static_cast<time_t>(ci.nextEpoch);
    struct tm tm;
    if (localtime_r(&t, &tm) != nullptr) {
      vdm::LocalTime& l = s.nextCalibLocal;
      l.valid = true;
      l.year = static_cast<uint16_t>(tm.tm_year + 1900);
      l.month = static_cast<uint8_t>(tm.tm_mon + 1);
      l.mday = static_cast<uint8_t>(tm.tm_mday);
      l.wday = static_cast<uint8_t>(tm.tm_wday);
      l.hour = static_cast<uint8_t>(tm.tm_hour);
      l.minute = static_cast<uint8_t>(tm.tm_min);
      l.second = static_cast<uint8_t>(tm.tm_sec);
      l.epoch = ci.nextEpoch;
    }
  }
  s.configSource = static_cast<uint8_t>(storage::bootLoadSource());
  s.configRepairs = storage::bootLoadDetails().info.repairs.mask;
  s.configNewerSchema = storage::bootLoadDetails().info.decode.newerSchema;
  s.importReport = storage::hasImportReport();
  sendDocument(req, 200, [&s](vdm::JsonWriter& jw) { return vdm::writeStatusJson(jw, s); });
}

// True when a temperature config slot has the (non-zero) id `id`.
bool tempConfigured(const vdm::OneWireId& id) {
  for (const vdm::TempSlotConfig& c : gCfgPtr->temps) {
    if (c.id == id) return true;
  }
  return false;
}

ValveViews& buildValveViews() {
  app::readStmSnapshot(*gSnapPtr);
  ValveViews& views = scratch<ValveViews>();
  for (vdm::ValveView& v : views.items) {
    const uint8_t i = static_cast<uint8_t>(&v - views.items);
    v = vdm::ValveView{};
    const vdm::ValveState& st = gSnapPtr->valves[i];
    v.state = &st;
    v.config = &gCfgPtr->valves[i];
    const int16_t raw[] = {st.temp1, st.temp2};
    for (uint8_t k = 0; k < 2; ++k) {
      const uint8_t slot = st.sensorSlot[k];
      if (slot == 0 || slot > vdm::kTempSlotCount) continue;
      v.sensorSlot[k] = slot;
      v.sensorName[k] = gCfgPtr->temps[slot - 1].name;
      v.sensorValid[k] = vdm::tempRawValid(raw[k]);
      v.sensorTenths[k] = static_cast<int32_t>(raw[k]) + gCfgPtr->temps[slot - 1].offset;
    }
    v.calibrationEnd.valid = mqtt::calibrationEnd(i, v.calibrationEnd);
  }
  return views;
}

void handleValves(AsyncWebServerRequest* req) {
  ValveViews& views = buildValveViews();
  const uint32_t now = app::nowMs();
  sendDocument(req, 200, [now, &views](vdm::JsonWriter& jw) {
    return vdm::writeValvesJson(jw, views.data(), vdm::kValveCount, now);
  });
}

// Fills the sensor views; returns the counts.
SensorViews& buildSensorViews(uint8_t& ntOut, uint8_t& nvOut) {
  app::readStmSnapshot(*gSnapPtr);
  SensorViews& views = scratch<SensorViews>();
  const uint32_t now = app::nowMs();
  const uint8_t busTemps = std::min(gSnapPtr->tempCount, vdm::kTempSlotCount);
  const uint8_t busVolts = std::min(gSnapPtr->voltCount, vdm::kVoltSlotCount);
  uint8_t nt = 0;
  // Configured temperature slots.
  for (uint8_t i = 0; i < vdm::kTempSlotCount; ++i) {
    const vdm::TempSlotConfig& c = gCfgPtr->temps[i];
    if (vdm::isZero(c.id) && !c.active && c.name[0] == '\0') continue;
    vdm::SensorView& v = views.temps[nt++];
    v = vdm::SensorView{};
    v.slot = static_cast<uint8_t>(i + 1);
    v.name = c.name;
    v.active = c.active;
    v.id = c.id;
    for (uint8_t b = 0; b < busTemps; ++b) {
      const vdm::TempReading& r = gSnapPtr->temps[b];
      if (vdm::isZero(c.id) || r.id != c.id) continue;
      v.onBus = true;
      v.raw = r.raw;
      v.value = static_cast<int32_t>(r.raw) + c.offset;
      v.valid = r.seen && vdm::tempRawValid(r.raw) &&
                vdm::elapsedMs(now, r.lastSeenMs) <= kSensorStaleMs;
      v.ageS = r.seen ? vdm::elapsedMs(now, r.lastSeenMs) / 1000 : 0;
      break;
    }
    uint8_t k = 0;
    for (const vdm::ValveState& st : gSnapPtr->valves) {
      if (st.sensorSlot[0] == v.slot || st.sensorSlot[1] == v.slot) {
        v.valve = k;
        break;
      }
      ++k;
    }
  }
  // Bus sensors without a config slot.
  for (uint8_t b = 0; b < busTemps; ++b) {
    const vdm::TempReading& r = gSnapPtr->temps[b];
    if (vdm::isZero(r.id) || tempConfigured(r.id)) continue;
    vdm::SensorView& v = views.temps[nt++];
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
    const vdm::VoltSlotConfig& c = gCfgPtr->volts[i];
    if (vdm::isZero(c.id) && !c.active && c.name[0] == '\0') continue;
    vdm::SensorView& v = views.volts[nv++];
    v = vdm::SensorView{};
    v.slot = static_cast<uint8_t>(i + 1);
    v.name = c.name;
    v.active = c.active;
    v.id = c.id;
    v.unit = c.unit;
    for (uint8_t b = 0; b < busVolts; ++b) {
      const vdm::VoltReading& r = gSnapPtr->volts[b];
      if (vdm::isZero(c.id) || r.id != c.id) continue;
      v.onBus = true;
      v.raw = r.vad;
      const double milli = (static_cast<double>(r.vad) / 100.0 + c.offset) * c.factor * 1000.0;
      v.value = static_cast<int32_t>(std::clamp(milli, -2e9, 2e9));
      v.valid = r.seen && vdm::vadValid(r.vad) &&
                vdm::elapsedMs(now, r.lastSeenMs) <= kSensorStaleMs;
      v.ageS = r.seen ? vdm::elapsedMs(now, r.lastSeenMs) / 1000 : 0;
      break;
    }
  }
  for (uint8_t b = 0; b < busVolts; ++b) {
    const vdm::VoltReading& r = gSnapPtr->volts[b];
    if (vdm::isZero(r.id)) continue;
    bool configured = false;
    for (const vdm::VoltSlotConfig& c : gCfgPtr->volts) configured = configured || c.id == r.id;
    if (configured) continue;
    vdm::SensorView& v = views.volts[nv++];
    v = vdm::SensorView{};
    v.onBus = true;
    v.id = r.id;
    v.raw = r.vad;
    v.valid = false;  // no offset/factor/unit without a slot
    v.ageS = r.seen ? vdm::elapsedMs(now, r.lastSeenMs) / 1000 : 0;
  }
  ntOut = nt;
  nvOut = nv;
  return views;
}

void handleSensors(AsyncWebServerRequest* req) {
  uint8_t nt = 0, nv = 0;
  const SensorViews& views = buildSensorViews(nt, nv);
  sendDocument(req, 200, [nt, nv, &views](vdm::JsonWriter& jw) {
    return vdm::writeSensorsJson(jw, views.temps, nt, views.volts, nv);
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
  const int slot = acquireSlot(req);
  if (slot < 0) return sendError(req, 503, "busy", "response buffers in use");
  Events& events = scratch<Events>();
  // The ring may hold more text than one slot: halve the count until it fits.
  for (size_t max = limit; max > 0; max /= 2) {
    uint32_t next = f.sinceSeq, first = 0, last = 0, dropped = 0;
    const size_t n = logger::read(f, events.data(), std::min(max, kEventsBuffer), next, first,
                                  last, dropped);
    vdm::JsonWriter jw(gSlots[slot].buf, kResponseSlotSize);
    if (vdm::writeEventsJson(jw, events.data(), n, first, last, next, dropped) && jw.complete()) {
      return sendSlot(req, 200, slot, jw.length());
    }
  }
  releaseSlot(slot);
  sendError(req, 500, "internal", "events");
}

void handleProfileGet(AsyncWebServerRequest* req, uint8_t valve) {
  vdm::Profile& p = scratch<vdm::Profile>();
  app::readProfile(valve, p);
  if (p.count == 0) return sendError(req, 404, "not_found", "no profile");
  sendDocument(req, 200, [&p](vdm::JsonWriter& jw) { return vdm::writeProfileJson(jw, p); });
}

void handleMotorGet(AsyncWebServerRequest* req) {
  app::readStmSnapshot(*gSnapPtr);
  sendDocument(req, 200, [](vdm::JsonWriter& jw) {
    return vdm::writeMotorJson(jw, gSnapPtr->motor, gSnapPtr->learnMovements,
                               gSnapPtr->haveBreakaway ? &gSnapPtr->breakaway : nullptr, gSnapPtr->haveMotor);
  });
}

void handleFlashStatus(AsyncWebServerRequest* req) {
  app::readStmSnapshot(*gSnapPtr);
  sendDocument(req, 200, [](vdm::JsonWriter& jw) {
    return vdm::writeFlashStatusJson(jw, gSnapPtr->flash,
                                     gSnapPtr->flashImage[0] ? gSnapPtr->flashImage : nullptr,
                                     gSnapPtr->flashPending);
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
  jw.key("hw");
  if (e.hwTag[0]) {
    jw.value(e.hwTag, strnlen(e.hwTag, sizeof e.hwTag));
  } else {
    jw.nullValue();
  }
  jw.endObject();
}

void handleImages(AsyncWebServerRequest* req) {
  Images& images = scratch<Images>();
  const size_t n = storage::listImages(images.data(), storage::kImageSlots);
  sendDocument(req, 200, [n, &images](vdm::JsonWriter& jw) {
    jw.beginArray();
    for (size_t i = 0; i < n; ++i) writeImage(jw, images[i], images[i].scanned);
    jw.endArray();
    return jw.ok();
  });
}

void handleConfigGet(AsyncWebServerRequest* req, bool attachment) {
  sendDocument(req, 200, [](vdm::JsonWriter& jw) { return vdm::writeConfigJson(jw, *gCfgPtr); },
               attachment ? "attachment; filename=\"vdmot-config.json\"" : nullptr);
}

// ---------------------------------------------------------------- files

void handleFiles(AsyncWebServerRequest* req) {
  bool truncated = false;
  Files& files = scratch<Files>();
  const size_t n = storage::listFiles(files.data(), kMaxFilesPerResponse, truncated);
  const uint32_t total = storage::fsTotal();
  const uint32_t used = storage::fsUsed();
  sendDocument(req, 200, [n, total, used, truncated, &files](vdm::JsonWriter& jw) {
    return vdm::writeFilesJson(jw, files.data(), n, total, used, truncated);
  });
}

void handleFileDelete(AsyncWebServerRequest* req) {
  AsyncWebParameter* p = req->getParam("path");
  if (p == nullptr) return sendError(req, 400, "bad_path", "path");
  const String& path = p->value();
  if (!vdm::fsPathValid(path.c_str(), path.length())) {
    return sendError(req, 400, "bad_path", path.c_str());
  }
  switch (storage::deleteFile(path.c_str())) {
    case storage::FileResult::Ok: return req->send(204);
    case storage::FileResult::BadPath: return sendError(req, 400, "bad_path", path.c_str());
    case storage::FileResult::Protected:
      return sendError(req, 403, "protected",
                       vdm::fileProtectReason(vdm::classifyFsPath(path.c_str(), path.length())));
    case storage::FileResult::NotFound: return sendError(req, 404, "not_found", path.c_str());
    default: return sendError(req, 500, "io_error", path.c_str());
  }
}

void handleImportReport(AsyncWebServerRequest* req) {
  if (!storage::hasImportReport()) return sendError(req, 404, "not_found", "no import report");
  fs::File f = LittleFS.open(storage::kImportReportFile, FILE_READ);
  if (!f) return sendError(req, 404, "not_found", "no import report");
  f.setBufferSize(storage::kFileBufferSize);
  const int slot = acquireSlot(req);
  if (slot < 0) {
    f.close();
    return sendError(req, 503, "busy", "response buffers in use");
  }
  const size_t n = f.read(reinterpret_cast<uint8_t*>(gSlots[slot].buf), kResponseSlotSize);
  const bool whole = f.available() == 0;
  f.close();
  if (n == 0 || !whole) {
    releaseSlot(slot);
    return sendError(req, 500, "io_error", storage::kImportReportFile);
  }
  sendSlot(req, 200, slot, n);
}

// ---------------------------------------------------------------- health

void handleHealth(AsyncWebServerRequest* req) {
  Health& h = scratch<Health>();
  app::readHealth(h.snap);
  vdm::JsonWriter jw(h.text, sizeof h.text);
  if (!vdm::writeHealthJson(jw, h.snap)) return sendError(req, 500, "internal", "health");
  // A copy: with a const char* the library keeps the pointer and reads the
  // buffer until the response is sent, which holds no slot, so the next
  // request could overwrite the text under it.
  AsyncWebServerResponse* res = req->beginResponse(200, kJson, String(h.text));
  if (res == nullptr) return req->send(500);
  res->addHeader("Cache-Control", "no-store");
  req->send(res);
}

// ---------------------------------------------------------------- log download

void closeLogStream() {
  gLog.file.close();  // no-op when not open
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
      gLog.file.setBufferSize(storage::kFileBufferSize);
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
  logger::requestFlush();
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

// JSON number (integer or fraction, never bool or string) rounded by
// vdm::roundTargetPercent.
bool targetField(JsonVariantConst v, uint8_t& out) {
  if (!v.is<double>()) return false;  // also null and bool
  return vdm::roundTargetPercent(v.as<double>(), out);
}

// Queues a web target; false when an error was answered.
bool submitTarget(AsyncWebServerRequest* req, uint8_t valve, uint8_t target) {
  if (!gCfgPtr->valves[valve].active) {
    sendError(req, 409, "inactive", "valve not active");
    return false;
  }
  app::Command c;
  c.type = app::CommandType::SetTarget;
  c.valve = valve;
  c.pos = target;
  c.source = vdm::TargetSource::Web;
  return submitOr503(req, c);
}

void handleTarget(AsyncWebServerRequest* req, uint8_t valve, bool hasBody) {
  if (!parseBody(req, hasBody)) return;
  JsonObjectConst o = gDocPtr->as<JsonObjectConst>();
  static const char* const kKeys[] = {"target"};
  uint8_t target = 0;
  if (!onlyKeys(o, kKeys, 1) || !targetField(o["target"], target)) {
    return sendError(req, 400, "out_of_range", "target 0..100");
  }
  if (!submitTarget(req, valve, target)) return;
  char buf[48];
  snprintf(buf, sizeof buf, "{\"valve\":%u,\"target\":%u}", valve + 1u,
           static_cast<unsigned>(target));
  req->send(202, kJson, buf);
}

void handleSimple(AsyncWebServerRequest* req, app::CommandType type, uint8_t valve) {
  if (refuseWhileFlashing(req)) return;
  if (type != app::CommandType::ResetStm && refuseTooOld(req)) return;
  app::Command c;
  c.type = type;
  c.valve = valve;
  if (submitOr503(req, c)) sendAccepted(req);
}

void handleServiceMove(AsyncWebServerRequest* req, uint8_t valve, bool hasBody) {
  if (refuseWhileFlashing(req)) return;
  if (refuseTooOld(req)) return;
  if (app::stmProtocol() < 2) return sendError(req, 409, "unsupported", "STM protocol v2 required");
  if (!parseBody(req, hasBody)) return;
  JsonObjectConst o = gDocPtr->as<JsonObjectConst>();
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
  if (refuseTooOld(req)) return;
  if (!parseBody(req, hasBody)) return;
  JsonObjectConst o = gDocPtr->as<JsonObjectConst>();
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
    const vdm::OneWireId& id = gCfgPtr->temps[slot[k] - 1].id;
    if (vdm::isZero(id) || !vdm::crcValid(id)) {
      return sendError(req, 400, "invalid", k == 0 ? "slot1 has no valid sensor id"
                                                     : "slot2 has no valid sensor id");
    }
    c.ids[k] = id;
  }
  if (submitOr503(req, c)) sendAccepted(req);
}

void handleProfileRefresh(AsyncWebServerRequest* req, uint8_t valve) {
  if (refuseTooOld(req)) return;
  if (app::stmProtocol() < 2) return sendError(req, 409, "unsupported", "STM protocol v2 required");
  handleSimple(req, app::CommandType::RequestProfile, valve);
}

// ?dryRun=1: validated like a save (applyConfigJson checks the whole
// config), nothing stored; the answer names what a save would do. The body
// holds a response slot, so the answer of a save always has one: a busy
// server refused the body (503) before anything was applied. The patched
// copy (2.5 KB) lives on the heap for this request only (no memory: 503,
// nothing applied).
void handleConfigPatch(AsyncWebServerRequest* req, bool hasBody) {
  bool dryRun = false;
  if (AsyncWebParameter* p = req->getParam("dryRun")) {
    if (p->value() != "1") return sendError(req, 400, "bad_request", "dryRun=1");
    dryRun = true;
  }
  if (!hasBody || gBodyLen == 0) return sendError(req, 400, "bad_request", "JSON body required");
  vdm::Config* patch = new (std::nothrow) vdm::Config;
  if (patch == nullptr) return sendError(req, 503, "busy", "out of memory");
  *patch = *gCfgPtr;
  char path[72];
  const vdm::PatchResult r = vdm::applyConfigJson(*patch, gBody, gBodyLen, path, sizeof path);
  if (r != vdm::PatchResult::Ok) {
    delete patch;
    return sendError(req, 400, "invalid", path);
  }
  vdm::ApplyInfo info;
  info.restartRequired = vdm::configRestartReasons(*gCfgPtr, *patch) != 0;
  info.netTrial = vdm::netTrialRequired(gCfgPtr->net, patch->net);
  const bool saved = dryRun || storage::applyConfig(*patch, path, sizeof path);
  delete patch;
  if (dryRun) {
    char buf[64];
    snprintf(buf, sizeof buf, "{\"restartRequired\":%s,\"netTrial\":%s}",
             info.restartRequired ? "true" : "false", info.netTrial ? "true" : "false");
    return req->send(200, kJson, buf);
  }
  if (!saved) return sendError(req, strcmp(path, "nvs") == 0 ? 500 : 400, "invalid", path);
  refreshConfig();
  logger::log(vdm::EventCode::ConfigSaved, vdm::kNoValve,
              static_cast<int32_t>(storage::configRevision()), 0, "web");
  sendDocument(req, 200,
               [&info](vdm::JsonWriter& jw) { return vdm::writeConfigJson(jw, *gCfgPtr, &info); });
}

void handleMotorSet(AsyncWebServerRequest* req, bool hasBody) {
  if (refuseWhileFlashing(req)) return;
  if (refuseTooOld(req)) return;
  if (!parseBody(req, hasBody)) return;
  app::readStmSnapshot(*gSnapPtr);
  JsonObjectConst o = gDocPtr->as<JsonObjectConst>();
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
    if (!complete && !gSnapPtr->haveMotor) {
      return sendError(req, 409, "unknown", "motor parameters not read yet: send all five");
    }
    vdm::MotorChars mc = gSnapPtr->motor;
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
    if (b.size() != 3 && !gSnapPtr->haveBreakaway) {
      return sendError(req, 409, "unknown", "breakaway not read yet: send all three");
    }
    vdm::Breakaway ba = gSnapPtr->breakaway;
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
  JsonObjectConst o = gDocPtr->as<JsonObjectConst>();
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
  JsonObjectConst o = gDocPtr->as<JsonObjectConst>();
  static const char* const kKeys[] = {"image", "mode", "force", "board"};
  const char* image = o["image"].is<const char*>() ? o["image"].as<const char*>() : nullptr;
  const char* board = o["board"].isNull() ? ""
                      : (o["board"].is<const char*>() ? o["board"].as<const char*>() : nullptr);
  const char* mode = o["mode"].isNull() ? "normal"
                                        : (o["mode"].is<const char*>() ? o["mode"].as<const char*>()
                                                                       : "");
  bool force = false;
  char name[storage::kImageNameMax + 1];
  if (!onlyKeys(o, kKeys, 4) || image == nullptr || board == nullptr ||
      (board[0] != '\0' && strcmp(board, "C1") != 0 && strcmp(board, "C2") != 0) ||
      !storage::normalizeImageName(image, strlen(image), name, sizeof name) ||
      (strcmp(mode, "normal") != 0 && strcmp(mode, "blank") != 0) ||
      !boolField(o, "force", force, false)) {
    return sendError(req, 400, "bad_request", "image, mode normal|blank, force, board C1|C2");
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
  // Board revision: the running STM's tag, else the user's choice.
  app::readStmSnapshot(*gSnapPtr);
  const char* boardHw = gSnapPtr->version.hw[0] ? gSnapPtr->version.hw : board;
  const vdm::BoardCheck bc = vdm::checkBoard(e.hwTag, boardHw);
  if (!force && bc == vdm::BoardCheck::Mismatch) {
    char detail[40];
    snprintf(detail, sizeof detail, "image %.3s, board %.3s", e.hwTag, boardHw);
    return sendError(req, 409, "board_mismatch", detail);
  }
  if (!force && bc == vdm::BoardCheck::BoardRequired) {
    return sendError(req, 409, "board_required", "choose the board: C1 or C2");
  }
  app::Command c;
  c.type = app::CommandType::StartFlash;
  vdm::copyString(c.image, sizeof c.image, name);
  vdm::copyString(c.board, sizeof c.board, board);
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
  JsonObjectConst o = gDocPtr->as<JsonObjectConst>();
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
  if (gCfgPtr->mqtt.mode == vdm::MqttMode::Off) return sendError(req, 409, "disabled", "MQTT is off");
  JsonObjectConst o = gDocPtr->as<JsonObjectConst>();
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
  if (a != mqtt::DiscoveryAction::Delete && !gCfgPtr->mqtt.separate) {
    return sendError(req, 409, "separate_required", "HA discovery needs separate topics");
  }
  mqtt::requestDiscovery(a);
  sendAccepted(req);
}

void handleStop(AsyncWebServerRequest* req, uint8_t valve) {
  if (refuseWhileFlashing(req)) return;
  if (refuseBelowV3(req)) return;
  app::Command c;
  c.type = app::CommandType::StopValve;
  c.valve = valve;
  if (submitOr503(req, c)) sendAccepted(req);
}

void handleSafeModeLeave(AsyncWebServerRequest* req) {
  if (refuseWhileFlashing(req)) return;
  if (refuseBelowV3(req)) return;
  app::Command c;
  c.type = app::CommandType::LeaveSafeMode;
  if (submitOr503(req, c)) sendAccepted(req);
}

void handleNetTrial(AsyncWebServerRequest* req, bool confirm) {
  const bool ok = confirm ? net::requestTrialConfirm() : net::requestTrialRevert();
  if (!ok) return sendError(req, 409, "no_trial", "no network trial running");
  sendAccepted(req);
}

void handleImportReportDismiss(AsyncWebServerRequest* req) {
  if (!storage::dismissImportReport()) return sendError(req, 404, "not_found", "no import report");
  req->send(204);
}

// ---------------------------------------------------------------- legacy aliases

void handleLegacyValves(AsyncWebServerRequest* req) {
  ValveViews& views = buildValveViews();
  sendDocument(req, 200, [&views](vdm::JsonWriter& jw) {
    return vdm::writeLegacyValvesJson(jw, views.data(), vdm::kValveCount);
  });
}

void handleLegacySensors(AsyncWebServerRequest* req, bool temps) {
  uint8_t nt = 0, nv = 0;
  const SensorViews& views = buildSensorViews(nt, nv);
  const bool all = gCfgPtr->mqtt.allTemps;
  sendDocument(req, 200, [temps, nt, nv, all, &views](vdm::JsonWriter& jw) {
    return temps ? vdm::writeLegacyTempsJson(jw, views.temps, nt, all)
                 : vdm::writeLegacyVoltsJson(jw, views.volts, nv);
  });
}

// {"valve":1..12,"value":<number>}; other members (legacy PI values) are
// ignored.
void handleSetValve(AsyncWebServerRequest* req, bool hasBody) {
  if (hasBody && gBodyOverflow) return sendError(req, 413, "too_large", "body");
  if (!parseBody(req, hasBody)) return;
  JsonObjectConst o = gDocPtr->as<JsonObjectConst>();
  int64_t valve = 0;
  uint8_t target = 0;
  if (!intField(o, "valve", 1, vdm::kValveCount, valve, true) || !targetField(o["value"], target)) {
    return sendError(req, 400, "out_of_range", "valve 1..12, value 0..100");
  }
  if (!submitTarget(req, static_cast<uint8_t>(valve - 1), target)) return;
  req->send(200, kJson, "{\"res\":\"ok\"}");
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
  // The request owns the upload now: a file of it refused while another upload ran no longer
  // decides the answer, and the callback that would clear its mark is replaced below.
  clearMark(req);
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
      mark(req, 409, "busy", "upload");
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
  if (takeMark(req, mk)) return sendError(req, mk.code, mk.error, mk.detail);
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
    case R::Health: return handleHealth(req);
    case R::ValveStop: return handleStop(req, v);
    case R::StopAll: return handleStop(req, vdm::kAllValves);
    case R::StmSafeModeLeave: return handleSafeModeLeave(req);
    case R::NetConfirm: return handleNetTrial(req, true);
    case R::NetRevert: return handleNetTrial(req, false);
    case R::Files: return handleFiles(req);
    case R::FileDelete: return handleFileDelete(req);
    case R::ImportReport: return handleImportReport(req);
    case R::ImportReportDismiss: return handleImportReportDismiss(req);
    default: return sendError(req, 404, "not_found", url.c_str());
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

// Non-API paths: the legacy aliases, else the dashboard files.
void handleNonApi(AsyncWebServerRequest* req, bool hasBody) {
  const String& url = req->url();
  const vdm::LegacyMatch lm = vdm::matchLegacyRoute(methodOf(req), url.c_str(), url.length());
  switch (lm.route) {
    case vdm::LegacyRoute::Valves:
      return handleLegacyValves(req);
    case vdm::LegacyRoute::Temps:
    case vdm::LegacyRoute::Volts:
      return handleLegacySensors(req, lm.route == vdm::LegacyRoute::Temps);
    case vdm::LegacyRoute::SetValve:
      return handleSetValve(req, hasBody);
    default:
      break;
  }
  if (req->method() == HTTP_GET) return handleStatic(req);
  sendError(req, 405, "method_not_allowed", url.c_str());
}

class ApiHandler : public AsyncWebHandler {
 public:
  bool canHandle(AsyncWebServerRequest* req) override {
    req->addInterestingHeader("Origin");
    req->addInterestingHeader("X-VdMot");
    req->addInterestingHeader("If-None-Match");
    req->addInterestingHeader("X-Update-MD5");
    req->addInterestingHeader("X-MD5");
    return true;
  }

  void handleRequest(AsyncWebServerRequest* req) override {
    net::noteInboundHttp(remoteIp(req));
    if (!refreshConfig()) {  // the guard answers first; kept for safety
      endBody(req);
      return sendError(req, 503, "busy", "out of memory");
    }
    const bool hasBody = gBodyOwner == req;
    Mark mk;
    if (req->contentLength() > 0 && !isUploadRoute(route(req).route) && takeMark(req, mk)) {
      sendError(req, mk.code, mk.error, mk.detail);
    } else if (req->url().startsWith("/api/")) {
      handleApi(req, hasBody);
    } else {
      handleNonApi(req, hasBody);
    }
    endBody(req);
  }

  void handleBody(AsyncWebServerRequest* req, uint8_t* data, size_t len, size_t index,
                  size_t total) override {
    if (index == 0) {
      clearMark(req);
      if (gBodyOwner != nullptr && gBodyOwner != req) {
        mark(req, 409, "busy", "body");
        return;
      }
      const int slot = acquireSlot(req);  // a restarted body keeps its slot
      if (slot < 0) {
        mark(req, 503, "busy", "response buffers in use");
        return;
      }
      gBodyOwner = req;
      gBodySlot = slot;
      gBody = gSlots[slot].buf;
      gBodyLen = 0;
      gBodyOverflow = total > kMaxBodySize;
      gBody[0] = '\0';
      // Client gone mid-body: the slot goes back to the pool.
      req->onDisconnect([req]() { endBody(req); });
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
  gServer.addHandler(&gGuard);  // the first request allocates the working set
  gServer.addHandler(&gApi);
  gServer.begin();
  gStarted = true;
}

bool started() { return gStarted; }

}  // namespace web
