#include "web_server.h"

#include <Arduino.h>
#include <AsyncTCP.h>
#include <AsyncWebServer_WT32_ETH01.h>
#include <esp_system.h>
#include <new>
#include <string.h>

#include <vdm/auth.h>
#include <vdm/config.h>
#include <vdm/json_api.h>
#include <vdm/json_writer.h>

#include "app.h"
#include "generated/web_assets.h"
#include "logger.h"
#include "mqtt_client.h"
#include "net.h"
#include "ota.h"
#include "storage.h"

namespace web {

namespace {

AsyncWebServer gServer(80);
bool gStarted = false;

// All handlers run in the single AsyncTCP task, so the statics below need no
// locking among themselves. Slot buffers are allocated once in begin()
// (heap, never freed).
struct Slot {
  char* buf;
  bool busy;
};
Slot gSlots[kResponseSlots];

char gBody[kMaxBodySize + 1];
size_t gBodyLen = 0;
bool gBodyOverflow = false;
AsyncWebServerRequest* gBodyOwner = nullptr;

app::StmSnapshot gSnap;
vdm::Config gCfg;
vdm::AuthLimiter gAuthLimiter;

int acquireSlot() {
  for (size_t i = 0; i < kResponseSlots; ++i) {
    if (gSlots[i].buf != nullptr && !gSlots[i].busy) {
      gSlots[i].busy = true;
      return static_cast<int>(i);
    }
  }
  return -1;
}

// Sends slot content without copying; the slot is released when the client
// is gone (response sent or connection dropped).
void sendSlot(AsyncWebServerRequest* req, int code, int slot, size_t len) {
  AsyncWebServerResponse* res = req->beginResponse_P(
      code, "application/json", reinterpret_cast<const uint8_t*>(gSlots[slot].buf), len);
  res->addHeader("Cache-Control", "no-store");
  req->onDisconnect([slot]() { gSlots[slot].busy = false; });
  req->send(res);
}

void sendError(AsyncWebServerRequest* req, int code, const char* error, const char* detail) {
  char buf[160];
  vdm::JsonWriter jw(buf, sizeof buf);
  vdm::writeErrorJson(jw, error, detail);
  req->send(code, "application/json", jw.c_str());
}

vdm::HttpMethod methodOf(AsyncWebServerRequest* req) {
  switch (req->method()) {
    case HTTP_GET: return vdm::HttpMethod::Get;
    case HTTP_POST: return vdm::HttpMethod::Post;
    case HTTP_DELETE: return vdm::HttpMethod::Delete;
    default: return vdm::HttpMethod::Other;
  }
}

bool authorised(AsyncWebServerRequest* req, bool needsAuth) {
  const bool enabled = gCfg.web.user[0] != '\0' && gCfg.web.password[0] != '\0';
  if (!enabled || !needsAuth) return true;
  const uint32_t now = app::nowMs();
  if (gAuthLimiter.locked(now)) {
    sendError(req, 429, "locked", "too many failed logins");
    return false;
  }
  AsyncWebHeader* h = req->getHeader("Authorization");
  const bool ok = h != nullptr && vdm::checkBasicAuth(h->value().c_str(), h->value().length(),
                                                      gCfg.web.user, gCfg.web.password);
  gAuthLimiter.onResult(ok, now);
  if (!ok) {
    if (h != nullptr) {
      logger::log(vdm::EventCode::AuthFailed, vdm::kNoValve,
                  static_cast<int32_t>(gAuthLimiter.failuresInWindow()), 0,
                  req->client()->remoteIP().toString().c_str());
    }
    AsyncWebServerResponse* res = req->beginResponse(401, "application/json",
                                                     "{\"error\":\"unauthorized\"}");
    res->addHeader("WWW-Authenticate", "Basic realm=\"VdMot\"");
    req->send(res);
  }
  return ok;
}

void handleStatus(AsyncWebServerRequest* req) {
  const int slot = acquireSlot();
  if (slot < 0) return sendError(req, 503, "busy", "response buffers in use");
  app::readStmSnapshot(gSnap);
  vdm::StatusSnapshot s;
  s.espVersion = vdm::firmwareVersion();
  s.uptimeS = app::uptimeS();
  s.resetReason = static_cast<uint8_t>(esp_reset_reason());
  s.freeHeap = ESP.getFreeHeap();
  s.minFreeHeap = ESP.getMinFreeHeap();
  s.largestFreeBlock = ESP.getMaxAllocHeap();
  s.sketchSize = ESP.getSketchSize();
  s.sketchSpace = ESP.getFreeSketchSpace();
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
  vdm::copyString(s.hostname, sizeof s.hostname, gCfg.station);
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
  s.authEnabled = gCfg.web.user[0] != '\0' && gCfg.web.password[0] != '\0';
  s.lastEventSeq = logger::lastSeq();
  vdm::JsonWriter jw(gSlots[slot].buf, kResponseSlotSize);
  if (!vdm::writeStatusJson(jw, s)) {
    gSlots[slot].busy = false;
    return sendError(req, 500, "internal", "status document");
  }
  sendSlot(req, 200, slot, jw.length());
}

void handleApi(AsyncWebServerRequest* req) {
  storage::getConfig(gCfg);
  const String& url = req->url();
  const vdm::RouteMatch m =
      vdm::matchApiRoute(methodOf(req), url.c_str(), url.length(), gCfg.web.protectRead);
  if (m.route == vdm::ApiRoute::NotFound) return sendError(req, 404, "not_found", url.c_str());
  if (m.route == vdm::ApiRoute::MethodNotAllowed) {
    return sendError(req, 405, "method_not_allowed", url.c_str());
  }
  if (!authorised(req, m.needsAuth)) return;
  if (gBodyOverflow && gBodyOwner == req) return sendError(req, 413, "too_large", "body");

  switch (m.route) {
    case vdm::ApiRoute::Status:
      return handleStatus(req);
    case vdm::ApiRoute::Reboot:
      ota::requestRestart(0, 1000);
      return req->send(202, "application/json", "{\"result\":\"restarting\"}");
    case vdm::ApiRoute::MqttReconnect:
      mqtt::requestReconnect();
      return req->send(202, "application/json", "{\"result\":\"ok\"}");
    default:
      // Remaining endpoints: implemented per DESIGN.md "HTTP API".
      return sendError(req, 501, "not_implemented", url.c_str());
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
    res->addHeader("Content-Encoding", "gzip");
    res->addHeader("ETag", a.etag);
    res->addHeader("Cache-Control", "no-cache");
    return req->send(res);
  }
  sendError(req, 404, "not_found", url.c_str());
}

void onRequest(AsyncWebServerRequest* req) {
  if (req->url().startsWith("/api/")) {
    handleApi(req);
  } else if (req->method() == HTTP_GET) {
    handleStatic(req);
  } else {
    sendError(req, 405, "method_not_allowed", req->url().c_str());
  }
  if (gBodyOwner == req) gBodyOwner = nullptr;
}

void onBody(AsyncWebServerRequest* req, uint8_t* data, size_t len, size_t index, size_t total) {
  if (index == 0) {
    gBodyOwner = req;
    gBodyLen = 0;
    gBodyOverflow = total > kMaxBodySize;
  }
  if (gBodyOwner != req || gBodyOverflow) return;
  if (gBodyLen + len > kMaxBodySize) {
    gBodyOverflow = true;
    return;
  }
  memcpy(gBody + gBodyLen, data, len);
  gBodyLen += len;
  gBody[gBodyLen] = '\0';
}

}  // namespace

void begin() {
  if (gStarted) return;
  for (Slot& s : gSlots) s.buf = new (std::nothrow) char[kResponseSlotSize];
  gServer.onNotFound(onRequest);
  gServer.onRequestBody(onBody);
  gServer.begin();
  gStarted = true;
}

bool started() { return gStarted; }

}  // namespace web
