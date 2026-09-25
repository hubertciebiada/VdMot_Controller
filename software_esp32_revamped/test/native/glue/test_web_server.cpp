// Tests of src/web_server.cpp through the request driver: routing, the request guard, per-address
// auth, response slots, static assets, uploads, the legacy aliases and the 2.1 routes.
#include <string>

#include <vdm/stm_codec.h>

#include "generated/web_assets.h"
#include "glue_test.h"
#include "web_server.h"

namespace {

using fakes::http::Request;
using fakes::http::Response;

std::string errorBody(const char* code, const char* detail) {
  return std::string("{\"error\":\"") + code + "\",\"detail\":\"" + detail + "\"}";
}

void start() {
  sib::storage().active.valves[0].active = true;
  ++sib::storage().revision;
  web::begin();
}

// POST/DELETE on /api/* as the dashboard sends them.
Request apiPost(const std::string& url, const std::string& body) {
  Request r = fakes::http::post(url, body);
  r.header("X-VdMot", "1");
  return r;
}

Request apiDel(const std::string& url) {
  Request r = fakes::http::del(url);
  r.header("X-VdMot", "1");
  return r;
}

Request apiUpload(const std::string& url, const std::string& name, const std::string& data) {
  Request r = fakes::http::upload(url, name, data);
  r.header("X-VdMot", "1");
  return r;
}

void enableAuth() {
  vdm::Config& c = sib::storage().active;
  vdm::copyString(c.web.user, sizeof c.web.user, "admin");
  vdm::copyString(c.web.password, sizeof c.web.password, "secret12");
}

Request withAuth(Request r, bool right = true) {
  // admin:secret12 / admin:wrong
  r.header("Authorization", right ? "Basic YWRtaW46c2VjcmV0MTI=" : "Basic YWRtaW46d3Jvbmc=");
  return r;
}

size_t countOf(const std::string& s, const std::string& what) {
  size_t n = 0;
  for (size_t at = s.find(what); at != std::string::npos; at = s.find(what, at + 1)) ++n;
  return n;
}

}  // namespace

TEST_CASE("web begin: the server starts once, on port 80") {
  glue::begin();
  CHECK_FALSE(web::started());
  web::begin();
  web::begin();
  CHECK(web::started());
  CHECK(fakes::http::server().begins == 1);
  CHECK(fakes::http::server().port == 80);
  REQUIRE(fakes::http::server().instance != nullptr);
  CHECK(fakes::http::server().instance->handlers().size() == 2);
}

TEST_CASE("web: GET /api/status answers JSON that is not cached") {
  glue::begin();
  start();
  const Response r = fakes::http::perform(fakes::http::get("/api/status"));
  CHECK(r.code == 200);
  CHECK(r.contentType == "application/json");
  CHECK(r.header("Cache-Control") == "no-store");
  REQUIRE_FALSE(r.body.empty());
  CHECK(r.body.rfind("{\"station\":\"VdMot\",", 0) == 0);
  CHECK(r.body.back() == '}');
}

TEST_CASE("web: the dashboard is served gzip with its ETag, 304 when unchanged") {
  glue::begin();
  start();
  const web_assets::Asset* index = nullptr;
  for (size_t i = 0; i < web_assets::kAssetCount; ++i) {
    if (std::string(web_assets::kAssets[i].path) == "/index.html") index = &web_assets::kAssets[i];
  }
  REQUIRE(index != nullptr);
  const Response r = fakes::http::perform(fakes::http::get("/"));
  CHECK(r.code == 200);
  CHECK(r.contentType == index->contentType);
  CHECK(r.header("Content-Encoding") == "gzip");
  CHECK(r.header("ETag") == index->etag);
  CHECK(r.header("Cache-Control") == "no-cache");
  CHECK(r.body == std::string(reinterpret_cast<const char*>(index->data), index->length));
  fakes::http::Request again = fakes::http::get("/index.html");
  again.header("If-None-Match", index->etag);
  CHECK(fakes::http::perform(again).code == 304);
}

TEST_CASE("web: unknown paths and methods") {
  glue::begin();
  start();
  Response r = fakes::http::perform(fakes::http::get("/api/nothing"));
  CHECK(r.code == 404);
  CHECK(r.body == errorBody("not_found", "/api/nothing"));
  r = fakes::http::perform(apiDel("/api/status"));
  CHECK(r.code == 405);
  CHECK(r.body == errorBody("method_not_allowed", "/api/status"));
  r = fakes::http::perform(fakes::http::get("/missing.html"));
  CHECK(r.code == 404);
  r = fakes::http::perform(fakes::http::post("/", ""));
  CHECK(r.code == 405);
  r = fakes::http::perform(fakes::http::post("/", "{}"));  // the guard: never buffered, never 413
  CHECK(r.code == 405);
  CHECK(r.body == errorBody("method_not_allowed", "/"));
}

TEST_CASE("web guard: a JSON body over 8192 bytes is refused without being buffered") {
  glue::begin();
  start();
  Response r = fakes::http::perform(apiPost("/api/config", std::string(8193, ' ')));
  CHECK(r.code == 413);
  CHECK(r.body == errorBody("too_large", "body"));
  CHECK(sib::storage().applied.empty());
  // 8192 bytes are read (and fail to parse as a patch, not as too large)
  r = fakes::http::perform(apiPost("/api/config", std::string(8192, ' ')));
  CHECK(r.code == 400);
  CHECK(r.body.find("\"error\":\"invalid\"") != std::string::npos);
}

TEST_CASE("web guard: uploads need multipart and a length") {
  glue::begin();
  start();
  Response r = fakes::http::perform(apiPost("/api/stm/images", "x"));
  CHECK(r.code == 415);
  CHECK(r.body == errorBody("unsupported_media_type", "multipart/form-data required"));
  fakes::http::Request u = apiUpload("/api/stm/images", "fw.bin", "abc");
  u.contentLength = 0;
  r = fakes::http::perform(u);
  CHECK(r.code == 411);
}

TEST_CASE("web: a valve target is submitted and answered 202") {
  glue::begin();
  start();
  const Response r = fakes::http::perform(apiPost("/api/valves/1/target", "{\"target\":50}"));
  CHECK(r.code == 202);
  CHECK(r.body == "{\"valve\":1,\"target\":50}");
  REQUIRE(sib::app().submitted.size() == 1);
  const app::Command& c = sib::app().submitted[0];
  CHECK(c.type == app::CommandType::SetTarget);
  CHECK(c.valve == 0);
  CHECK(c.pos == 50);
  CHECK(c.source == vdm::TargetSource::Web);
}

TEST_CASE("web: a target out of range, an inactive valve, a full queue") {
  glue::begin();
  start();
  Response r = fakes::http::perform(apiPost("/api/valves/1/target", "{\"target\":101}"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("out_of_range", "target 0..100"));
  r = fakes::http::perform(apiPost("/api/valves/2/target", "{\"target\":1}"));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("inactive", "valve not active"));
  sib::app().submitResult = false;
  r = fakes::http::perform(apiPost("/api/valves/1/target", "{\"target\":1}"));
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("queue_full", "STM command queue full"));
}

TEST_CASE("web auth: without credentials a protected route answers 401 with a challenge") {
  glue::begin();
  enableAuth();
  start();
  Response r = fakes::http::perform(apiPost("/api/valves/1/target", "{\"target\":5}"));
  CHECK(r.code == 401);
  CHECK(r.header("WWW-Authenticate") == "Basic realm=\"VdMot\"");
  CHECK(sib::app().submitted.empty());
  r = fakes::http::perform(withAuth(apiPost("/api/valves/1/target", "{\"target\":5}")));
  CHECK(r.code == 202);
  r = fakes::http::perform(withAuth(apiPost("/api/valves/1/target", "{\"target\":5}"), false));
  CHECK(r.code == 401);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::AuthFailed).at(0);
  CHECK(std::string(e.text) == "192.168.1.50");
  CHECK(e.arg1 == 1);
}

TEST_CASE("web: reboot and MQTT reconnect are handed to their modules") {
  glue::begin();
  start();
  Response r = fakes::http::perform(apiPost("/api/system/reboot", ""));
  CHECK(r.code == 202);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(sib::ota().restartRequests[0].reason == 0);
  CHECK(sib::ota().restartRequests[0].delayMs == 1000);
  r = fakes::http::perform(apiPost("/api/mqtt/reconnect", ""));
  CHECK(r.code == 202);
  CHECK(sib::mqtt().reconnectRequests == 1);
}

TEST_CASE("web: two responses in flight use both slots, a third request gets 503") {
  glue::begin();
  start();
  fakes::http::Exchange a(fakes::http::get("/api/status"));
  fakes::http::Exchange b(fakes::http::get("/api/status"));
  const Response c = fakes::http::perform(fakes::http::get("/api/status"));
  CHECK(c.code == 503);
  CHECK(c.body == errorBody("busy", "response buffers in use"));
  CHECK(a.finish().code == 200);
  CHECK(fakes::http::perform(fakes::http::get("/api/status")).code == 200);
  CHECK(b.finish().code == 200);
}

TEST_CASE("web upload: an STM image is stored and described") {
  glue::begin();
  start();
  sib::storage().uploadEndInfo.size = 3;
  vdm::copyString(sib::storage().uploadEndInfo.name, sizeof sib::storage().uploadEndInfo.name,
                  "fw");
  vdm::copyString(sib::storage().uploadEndInfo.hwTag, sizeof sib::storage().uploadEndInfo.hwTag,
                  "C2");
  const Response r = fakes::http::perform(apiUpload("/api/stm/images", "fw.bin", "abc"));
  CHECK(r.code == 201);
  CHECK(sib::storage().uploadData == "abc");
  REQUIRE(sib::storage().uploadBegins.size() == 1);
  CHECK(sib::storage().uploadBegins[0].name == "fw.bin");
  CHECK(r.body.find("\"name\":\"fw\"") != std::string::npos);
  CHECK(r.body.find("\"hw\":\"C2\"}") != std::string::npos);
}

TEST_CASE("web upload: an ESP image goes to ota and asks for the restart") {
  glue::begin();
  start();
  const Response r = fakes::http::perform(
      apiUpload("/api/ota/esp?md5=0123456789abcdef0123456789abcdef", "fw.bin", "img"));
  CHECK(r.code == 200);
  CHECK(r.body == "{\"result\":\"ok\",\"restart\":true}");
  REQUIRE(sib::ota().uploadBegins.size() == 1);
  CHECK(sib::ota().uploadBegins[0].second == "0123456789abcdef0123456789abcdef");
  CHECK(sib::ota().uploadData == "img");
  CHECK(sib::ota().uploadEnds == std::vector<bool>{true});
}

// ---------------------------------------------------------------- guard (WG-1..WG-6, WG-16)

TEST_CASE("WG-1: an API write without X-VdMot is refused, nothing submitted") {
  glue::begin();
  start();
  Response r =
      fakes::http::perform(fakes::http::post("/api/valves/1/target", "{\"target\":50}"));
  CHECK(r.code == 403);
  CHECK(r.body == errorBody("header_required", "X-VdMot: 1"));
  CHECK(sib::app().submitted.empty());
  Request zero = fakes::http::post("/api/valves/1/target", "{\"target\":50}");
  zero.header("X-VdMot", "0");
  CHECK(fakes::http::perform(zero).code == 403);
  r = fakes::http::perform(apiPost("/api/valves/1/target", "{\"target\":50}"));
  CHECK(r.code == 202);
  CHECK(sib::app().submitted.size() == 1);
  // DELETE needs it too, GET does not
  CHECK(fakes::http::perform(fakes::http::del("/api/stm/images/fw")).code == 403);
  CHECK(sib::storage().deletedImages.empty());
  CHECK(fakes::http::perform(fakes::http::get("/api/valves")).code == 200);
}

TEST_CASE("WG-2: a body must be JSON; parameters of the media type are fine") {
  glue::begin();
  start();
  Request plain = apiPost("/api/valves/1/target", "{\"target\":50}");
  plain.contentType = "text/plain";
  Response r = fakes::http::perform(plain);
  CHECK(r.code == 415);
  CHECK(r.body == errorBody("unsupported_media_type", "application/json required"));
  CHECK(sib::app().submitted.empty());
  Request form = apiPost("/api/valves/1/target", "target=50");
  form.contentType = "application/x-www-form-urlencoded";
  CHECK(fakes::http::perform(form).code == 415);
  Request charset = apiPost("/api/valves/1/target", "{\"target\":50}");
  charset.contentType = "application/json; charset=utf-8";
  r = fakes::http::perform(charset);
  CHECK(r.code == 202);
  CHECK(sib::app().submitted.size() == 1);
}

TEST_CASE("WG-3: the Host header must name this device; assets are not checked") {
  glue::begin();
  sib::net().info.ip = 0x3301A8C0;  // 192.168.1.51
  vdm::copyString(sib::storage().active.web.allowedHosts,
                  sizeof sib::storage().active.web.allowedHosts, "heating.lan");
  start();
  Request evil = fakes::http::get("/api/status");
  evil.host = "evil.com";
  Response r = fakes::http::perform(evil);
  CHECK(r.code == 403);
  CHECK(r.body ==
        errorBody("host_not_allowed",
                  "evil.com: use 192.168.1.51 or add the name to web.allowedHosts"));
  evil.localIp = 0x3401A8C0;  // the connection's own address is named first
  r = fakes::http::perform(evil);
  CHECK(r.body ==
        errorBody("host_not_allowed",
                  "evil.com: use 192.168.1.52 or add the name to web.allowedHosts"));
  for (const char* host : {"192.168.1.51", "192.168.1.51:80", "vdmot.local", "VdMot",
                           "heating.lan"}) {
    Request ok = fakes::http::get("/api/status");
    ok.host = host;
    CAPTURE(host);
    CHECK(fakes::http::perform(ok).code == 200);
  }
  Request local = fakes::http::get("/api/status");
  local.host = "10.1.1.1";
  local.localIp = 0x0101010A;  // 10.1.1.1: the address the client connected to
  CHECK(fakes::http::perform(local).code == 200);
  Request asset = fakes::http::get("/");
  asset.host = "evil.com";
  CHECK(fakes::http::perform(asset).code == 200);
  // a station change renames the device: the old name is refused
  vdm::copyString(sib::storage().active.station, sizeof sib::storage().active.station, "Dom 1");
  ++sib::storage().revision;
  CHECK(fakes::http::perform(fakes::http::get("/api/status")).code == 403);
  Request renamed = fakes::http::get("/api/status");
  renamed.host = "dom-1.local";
  CHECK(fakes::http::perform(renamed).code == 200);
}

TEST_CASE("WG-4: a foreign Origin is refused, the own one accepted") {
  glue::begin();
  sib::net().info.ip = 0x3301A8C0;  // 192.168.1.51
  start();
  Request evil = apiPost("/api/valves/1/target", "{\"target\":50}");
  evil.header("Origin", "http://evil.com");
  Response r = fakes::http::perform(evil);
  CHECK(r.code == 403);
  CHECK(r.body == errorBody("origin_not_allowed", "http://evil.com"));
  CHECK(sib::app().submitted.empty());
  Request own = apiPost("/api/valves/1/target", "{\"target\":50}");
  own.header("Origin", "http://192.168.1.51");
  CHECK(fakes::http::perform(own).code == 202);
  // a GET with a foreign Origin is refused too
  Request get = fakes::http::get("/api/status");
  get.header("Origin", "null");
  CHECK(fakes::http::perform(get).code == 403);
}

TEST_CASE("WG-5: an upload without X-VdMot never reaches storage") {
  glue::begin();
  start();
  const Response r =
      fakes::http::perform(fakes::http::upload("/api/stm/images", "fw.bin", "abc"));
  CHECK(r.code == 403);
  CHECK(r.body == errorBody("header_required", "X-VdMot: 1"));
  CHECK(sib::storage().uploadBegins.empty());
  CHECK(sib::storage().uploadData.empty());
  const Response o = fakes::http::perform(fakes::http::upload("/api/ota/esp", "fw.bin", "img"));
  CHECK(o.code == 403);
  CHECK(sib::ota().uploadBegins.empty());
}

TEST_CASE("WG-6: the refusal is recomputed after the header filter with the same answer") {
  glue::begin();
  start();
  // a refused request keeps Origin and X-VdMot: the answer names the origin, not the marker
  Request r = apiPost("/api/valves/1/target", "{\"target\":50}");
  r.header("Origin", "http://evil.com");
  r.header("X-Other", "dropped");
  fakes::http::Exchange e(r);
  const Response& res = e.finish();
  CHECK(res.code == 403);
  CHECK(res.body == errorBody("origin_not_allowed", "http://evil.com"));
}

TEST_CASE("WG-16: RequestRefused is logged once per verdict and minute") {
  glue::begin();
  start();
  const Request noMarker = fakes::http::post("/api/valves/1/target", "{\"target\":50}");
  Request badHost = fakes::http::get("/api/status");
  badHost.host = "evil.com";
  CHECK(fakes::http::perform(noMarker).code == 403);
  fakes::advanceMs(1000);
  CHECK(fakes::http::perform(noMarker).code == 403);
  CHECK(fakes::http::perform(badHost).code == 403);
  std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::RequestRefused);
  REQUIRE(ev.size() == 2);
  CHECK(ev[0].arg1 == 3);
  CHECK(std::string(ev[0].text) == "192.168.1.50");
  CHECK(ev[1].arg1 == 1);
  fakes::advanceMs(58999);  // 59999 ms after the first
  CHECK(fakes::http::perform(noMarker).code == 403);
  CHECK(sib::logger().withCode(vdm::EventCode::RequestRefused).size() == 2);
  fakes::advanceMs(1);
  CHECK(fakes::http::perform(noMarker).code == 403);
  ev = sib::logger().withCode(vdm::EventCode::RequestRefused);
  REQUIRE(ev.size() == 3);
  CHECK(ev[2].arg1 == 3);
  // a content type refusal is verdict 4
  Request plain = apiPost("/api/valves/1/target", "{}");
  plain.contentType = "text/plain";
  CHECK(fakes::http::perform(plain).code == 415);
  CHECK(sib::logger().withCode(vdm::EventCode::RequestRefused).back().arg1 == 4);
}

// ---------------------------------------------------------------- auth per address (WG-7)

TEST_CASE("WG-7: ten wrong logins lock that address only, with Retry-After") {
  glue::begin();
  enableAuth();
  start();
  Request wrong = withAuth(apiPost("/api/valves/1/target", "{\"target\":5}"), false);
  wrong.remoteIp = 0x0200000A;  // 10.0.0.2
  for (int i = 0; i < 9; ++i) CHECK(fakes::http::perform(wrong).code == 401);
  CHECK(sib::logger().withCode(vdm::EventCode::AuthLocked).empty());
  Response r = fakes::http::perform(wrong);
  CHECK(r.code == 401);  // the 10th is still checked
  std::vector<vdm::Event> locked = sib::logger().withCode(vdm::EventCode::AuthLocked);
  REQUIRE(locked.size() == 1);
  CHECK(locked[0].arg1 == 60);
  CHECK(locked[0].arg2 == 1);
  CHECK(std::string(locked[0].text) == "10.0.0.2");
  CHECK(sib::logger().withCode(vdm::EventCode::AuthFailed).back().arg1 == 10);
  r = fakes::http::perform(wrong);
  CHECK(r.code == 429);
  CHECK(r.header("Retry-After") == "60");
  CHECK(r.body ==
        errorBody("locked", "too many failed logins from this address, retry in 60 s"));
  // even the right password is locked out from that address
  Request right = withAuth(apiPost("/api/valves/1/target", "{\"target\":5}"));
  right.remoteIp = 0x0200000A;
  fakes::advanceMs(30500);
  r = fakes::http::perform(right);
  CHECK(r.code == 429);
  CHECK(r.header("Retry-After") == "30");
  // another address is not affected
  right.remoteIp = 0x0300000A;  // 10.0.0.3
  CHECK(fakes::http::perform(right).code == 202);
  CHECK(sib::logger().withCode(vdm::EventCode::AuthLocked).size() == 1);
  CHECK(sib::logger().withCode(vdm::EventCode::AuthFailed).size() == 10);
  // after the lock the address may log in again
  fakes::advanceMs(29500);
  right.remoteIp = 0x0200000A;
  CHECK(fakes::http::perform(right).code == 202);
}

TEST_CASE("WG-7: an upload from a locked address answers 429") {
  glue::begin();
  enableAuth();
  start();
  Request wrong = withAuth(apiPost("/api/valves/1/target", "{\"target\":5}"), false);
  for (int i = 0; i < 10; ++i) fakes::http::perform(wrong);
  const Response r =
      fakes::http::perform(withAuth(apiUpload("/api/stm/images", "fw.bin", "abc")));
  CHECK(r.code == 429);
  CHECK(r.header("Retry-After") == "60");
  CHECK(sib::storage().uploadBegins.empty());
}

// ---------------------------------------------------------------- legacy (WG-8)

TEST_CASE("WG-8: GET /valves answers the legacy document") {
  glue::begin();
  vdm::copyString(sib::storage().active.valves[0].name, sizeof sib::storage().active.valves[0].name,
                  "Bad");
  vdm::ValveState& v = sib::app().snapshot.valves[0];
  v.status = 1;
  v.position = 40;
  v.desiredValid = true;
  v.desired = 55;
  sib::app().snapshot.valves[1].status = 6;  // no valve: skipped
  start();
  const Response r = fakes::http::perform(fakes::http::get("/valves"));
  CHECK(r.code == 200);
  CHECK(r.body ==
        "{\"valves\":[{\"idx\":1,\"name\":\"Bad\",\"state\":1,\"pos\":40,\"meanCur\":0,"
        "\"targetPos\":55,\"link\":0,\"moves\":0,\"oc\":0,\"cc\":0,\"dc\":0,\"cr\":0,"
        "\"controlActive\":0}]}");
}

TEST_CASE("WG-8: /temps and /volts answer the legacy documents") {
  glue::begin();
  vdm::Config& c = sib::storage().active;
  vdm::TempSlotConfig& t = c.temps[0];
  vdm::copyString(t.name, sizeof t.name, "Floor");
  t.active = true;
  REQUIRE(vdm::parseOneWireId("28-84-37-94-97-ff-03-23", 23, t.id));
  t.offset = 5;
  vdm::VoltSlotConfig& u = c.volts[0];
  vdm::copyString(u.name, sizeof u.name, "Supply");
  vdm::copyString(u.unit, sizeof u.unit, "V");
  u.active = true;
  u.factor = 1;
  u.offset = 0;
  REQUIRE(vdm::parseOneWireId("26-11-22-33-44-55-66-29", 23, u.id));
  vdm::StmSnapshot& s = sib::app().snapshot;
  s.tempCount = 1;
  s.temps[0].id = t.id;
  s.temps[0].raw = 210;
  s.temps[0].seen = true;
  s.temps[0].lastSeenMs = 0;
  s.voltCount = 1;
  s.volts[0].id = u.id;
  s.volts[0].vad = 1208;
  s.volts[0].seen = true;
  start();
  Response r = fakes::http::perform(fakes::http::get("/temps"));
  CHECK(r.code == 200);
  CHECK(r.body == "[{\"id\":\"28-84-37-94-97-ff-03-23\",\"name\":\"Floor\",\"temp\":21.5}]");
  r = fakes::http::perform(fakes::http::get("/volts"));
  CHECK(r.code == 200);
  CHECK(r.body ==
        "[{\"id\":\"26-11-22-33-44-55-66-29\",\"name\":\"Supply\",\"unit\":\"V\","
        "\"value\":12.080}]");
  // a temperature used by a valve is listed only with allTemps
  s.valves[2].sensorSlot[0] = 1;
  sib::storage().active.mqtt.allTemps = false;
  ++sib::storage().revision;
  CHECK(fakes::http::perform(fakes::http::get("/temps")).body == "[]");
}

TEST_CASE("WG-8: POST /setvalve rounds the value and answers the legacy body") {
  glue::begin();
  start();
  Response r = fakes::http::perform(
      fakes::http::post("/setvalve", "{\"valve\":1,\"value\":43.7,\"ctrlValue\":3}"));
  CHECK(r.code == 200);
  CHECK(r.body == "{\"res\":\"ok\"}");
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].type == app::CommandType::SetTarget);
  CHECK(sib::app().submitted[0].valve == 0);
  CHECK(sib::app().submitted[0].pos == 44);
  CHECK(sib::app().submitted[0].source == vdm::TargetSource::Web);
  r = fakes::http::perform(fakes::http::post("/setvalve", "{\"valve\":2,\"value\":10}"));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("inactive", "valve not active"));
  for (const char* bad : {"{\"valve\":0,\"value\":1}", "{\"valve\":13,\"value\":1}",
                          "{\"valve\":1,\"value\":100.5}", "{\"valve\":1,\"value\":\"5\"}",
                          "{\"valve\":1}", "{\"value\":1}"}) {
    CAPTURE(bad);
    r = fakes::http::perform(fakes::http::post("/setvalve", bad));
    CHECK(r.code == 400);
    CHECK(r.body == errorBody("out_of_range", "valve 1..12, value 0..100"));
  }
  sib::app().submitResult = false;
  r = fakes::http::perform(fakes::http::post("/setvalve", "{\"valve\":1,\"value\":1}"));
  CHECK(r.code == 503);
  Request plain = fakes::http::post("/setvalve", "{\"valve\":1,\"value\":1}", "text/plain");
  CHECK(fakes::http::perform(plain).code == 415);
  CHECK(sib::app().submitted.size() == 1);
  CHECK(fakes::http::perform(fakes::http::get("/setvalve")).code == 405);
  CHECK(fakes::http::perform(fakes::http::post("/valves", "")).code == 405);
}

TEST_CASE("WG-8: /setvalve needs the login when auth is on; /valves only with protectRead") {
  glue::begin();
  enableAuth();
  start();
  CHECK(fakes::http::perform(fakes::http::post("/setvalve", "{\"valve\":1,\"value\":1}")).code ==
        401);
  CHECK(fakes::http::perform(fakes::http::get("/valves")).code == 200);
  sib::storage().active.web.protectRead = true;
  ++sib::storage().revision;
  CHECK(fakes::http::perform(fakes::http::get("/valves")).code == 401);
  CHECK(fakes::http::perform(withAuth(fakes::http::get("/valves"))).code == 200);
}

TEST_CASE("WG-8: legacy paths answer 410 without buffering, others 404/405, never 413") {
  glue::begin();
  start();
  Response r = fakes::http::perform(fakes::http::get("/netinfo"));
  CHECK(r.code == 410);
  CHECK(r.body == errorBody("gone", "/api/status"));
  r = fakes::http::perform(fakes::http::post("/netconfig", std::string(2048, 'x')));
  CHECK(r.code == 410);
  CHECK(r.body == errorBody("gone", "/api/config"));
  r = fakes::http::perform(fakes::http::post("/nope", "{\"a\":1}"));
  CHECK(r.code == 405);
  CHECK(r.body == errorBody("method_not_allowed", "/nope"));
  r = fakes::http::perform(fakes::http::get("/nope"));
  CHECK(r.code == 404);
  CHECK(r.body == errorBody("not_found", "/nope"));
  Request getBody = fakes::http::get("/nope");
  getBody.body = "x";
  CHECK(fakes::http::perform(getBody).code == 404);
  // the legacy aliases go through the guard
  Request evil = fakes::http::get("/valves");
  evil.host = "evil.com";
  CHECK(fakes::http::perform(evil).code == 403);
  // 410 answers are never guarded
  Request gone = fakes::http::get("/cmd");
  gone.host = "evil.com";
  CHECK(fakes::http::perform(gone).code == 410);
}

// ---------------------------------------------------------------- targets (WG-9)

TEST_CASE("WG-9: fractional targets are rounded, other values refused") {
  glue::begin();
  start();
  Response r = fakes::http::perform(apiPost("/api/valves/1/target", "{\"target\":55.0}"));
  CHECK(r.code == 202);
  CHECK(r.body == "{\"valve\":1,\"target\":55}");
  r = fakes::http::perform(apiPost("/api/valves/1/target", "{\"target\":43.5}"));
  CHECK(r.body == "{\"valve\":1,\"target\":44}");
  r = fakes::http::perform(apiPost("/api/valves/1/target", "{\"target\":100.0}"));
  CHECK(r.body == "{\"valve\":1,\"target\":100}");
  REQUIRE(sib::app().submitted.size() == 3);
  CHECK(sib::app().submitted[0].pos == 55);
  CHECK(sib::app().submitted[1].pos == 44);
  CHECK(sib::app().submitted[2].pos == 100);
  for (const char* bad : {"{\"target\":100.4}", "{\"target\":-0.4}", "{\"target\":\"5\"}",
                          "{\"target\":true}", "{\"target\":null}", "{}",
                          "{\"target\":5,\"x\":1}"}) {
    CAPTURE(bad);
    r = fakes::http::perform(apiPost("/api/valves/1/target", bad));
    CHECK(r.code == 400);
    CHECK(r.body == errorBody("out_of_range", "target 0..100"));
  }
  CHECK(sib::app().submitted.size() == 3);
}

// ---------------------------------------------------------------- config (WG-10, WG-11)

TEST_CASE("WG-10: a config save with both slots held answers 503 and applies nothing") {
  glue::begin();
  start();
  fakes::http::Exchange a(fakes::http::get("/api/status"));
  fakes::http::Exchange b(fakes::http::get("/api/status"));
  Response r = fakes::http::perform(apiPost("/api/config", "{\"calib\":{\"hour\":4}}"));
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "response buffers in use"));
  CHECK(sib::storage().applied.empty());
  CHECK(a.finish().code == 200);
  r = fakes::http::perform(apiPost("/api/config", "{\"calib\":{\"hour\":4}}"));
  CHECK(r.code == 200);
  CHECK(sib::storage().applied.size() == 1);
  CHECK(b.finish().code == 200);
}

TEST_CASE("WG-10: the save answer tells whether a restart and a trial follow") {
  glue::begin();
  start();
  Response r = fakes::http::perform(apiPost("/api/config", "{\"calib\":{\"hour\":4}}"));
  CHECK(r.code == 200);
  REQUIRE(sib::storage().applied.size() == 1);
  CHECK(sib::storage().applied[0].calib.hour == 4);
  const std::string tail = ",\"restartRequired\":false,\"netTrial\":false}";
  CHECK(r.body.substr(r.body.size() - tail.size()) == tail);
  CHECK(sib::logger().has(vdm::EventCode::ConfigSaved));
  r = fakes::http::perform(apiPost("/api/config", "{\"net\":{\"reconnectTimeoutMin\":9}}"));
  CHECK(r.body.substr(r.body.size() - tail.size()) == tail);
  r = fakes::http::perform(
      apiPost("/api/config", "{\"net\":{\"dhcp\":false,\"ip\":\"192.168.1.60\","
                             "\"mask\":\"255.255.255.0\",\"gateway\":\"192.168.1.1\"}}"));
  CHECK(r.code == 200);
  const std::string trial = ",\"restartRequired\":true,\"netTrial\":true}";
  CHECK(r.body.substr(r.body.size() - trial.size()) == trial);
  CHECK(sib::storage().applied.size() == 3);
  // a station rename restarts without a trial
  r = fakes::http::perform(apiPost("/api/config", "{\"station\":\"Other\"}"));
  const std::string host = ",\"restartRequired\":true,\"netTrial\":false}";
  CHECK(r.body.substr(r.body.size() - host.size()) == host);
}

TEST_CASE("WG-10: a dry run validates and answers without storing") {
  glue::begin();
  start();
  Response r = fakes::http::perform(
      apiPost("/api/config?dryRun=1", "{\"net\":{\"dhcp\":false,\"ip\":\"192.168.1.60\","
                                      "\"mask\":\"255.255.255.0\",\"gateway\":\"192.168.1.1\"}}"));
  CHECK(r.code == 200);
  CHECK(r.body == "{\"restartRequired\":true,\"netTrial\":true}");
  r = fakes::http::perform(apiPost("/api/config?dryRun=1", "{\"calib\":{\"hour\":4}}"));
  CHECK(r.body == "{\"restartRequired\":false,\"netTrial\":false}");
  r = fakes::http::perform(apiPost("/api/config?dryRun=1", "{\"calib\":{\"hour\":24}}"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("invalid", "calib.hour"));
  // a combination only the whole-config rules reject
  r = fakes::http::perform(apiPost("/api/config?dryRun=1", "{\"net\":{\"dhcp\":false}}"));
  CHECK(r.code == 400);
  CHECK(r.body.find("\"error\":\"invalid\"") != std::string::npos);
  r = fakes::http::perform(apiPost("/api/config?dryRun=2", "{\"calib\":{\"hour\":4}}"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "dryRun=1"));
  CHECK(sib::storage().applied.empty());
  // a dry run needs no response slot
  fakes::http::Exchange a(fakes::http::get("/api/status"));
  fakes::http::Exchange b(fakes::http::get("/api/status"));
  r = fakes::http::perform(apiPost("/api/config?dryRun=1", "{\"calib\":{\"hour\":4}}"));
  CHECK(r.code == 200);
  a.finish();
  b.finish();
}

TEST_CASE("WG-10: a failed save releases its slot") {
  glue::begin();
  start();
  sib::storage().applyResult = false;
  sib::storage().applyPath = "nvs";
  Response r = fakes::http::perform(apiPost("/api/config", "{\"calib\":{\"hour\":4}}"));
  CHECK(r.code == 500);
  CHECK(r.body == errorBody("invalid", "nvs"));
  sib::storage().applyPath = "net.ip";
  r = fakes::http::perform(apiPost("/api/config", "{\"calib\":{\"hour\":4}}"));
  CHECK(r.code == 400);
  r = fakes::http::perform(apiPost("/api/config", "{\"calib\":{\"hour\":99}}"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("invalid", "calib.hour"));
  // both slots are still free
  fakes::http::Exchange a(fakes::http::get("/api/status"));
  fakes::http::Exchange b(fakes::http::get("/api/status"));
  CHECK(a.finish().code == 200);
  CHECK(b.finish().code == 200);
}

TEST_CASE("WG-11: export with secrets needs the web login") {
  glue::begin();
  vdm::copyString(sib::storage().active.mqtt.password, sizeof sib::storage().active.mqtt.password,
                  "brokerpw");
  start();
  Response r = fakes::http::perform(fakes::http::get("/api/config/export?secrets=1"));
  CHECK(r.code == 403);
  CHECK(r.body == errorBody("auth_required", "enable web login to export passwords"));
  r = fakes::http::perform(fakes::http::get("/api/config/export"));
  CHECK(r.code == 200);
  CHECK(r.header("Content-Disposition") == "attachment; filename=\"vdmot-config.json\"");
  CHECK(r.body.find("brokerpw") == std::string::npos);
  r = fakes::http::perform(fakes::http::get("/api/config/export?secrets=0"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "secrets=1"));
  enableAuth();
  ++sib::storage().revision;
  r = fakes::http::perform(withAuth(fakes::http::get("/api/config/export?secrets=1")));
  CHECK(r.code == 200);
  CHECK(r.header("Content-Disposition") ==
        "attachment; filename=\"vdmot-config-secrets.json\"");
  CHECK(r.body.find("\"brokerpw\"") != std::string::npos);
  CHECK(r.body.find("\"secret12\"") != std::string::npos);
}

// ---------------------------------------------------------------- network trial (WG-12)

TEST_CASE("WG-12: confirm and revert of a network trial") {
  glue::begin();
  start();
  Response r = fakes::http::perform(apiPost("/api/system/network/confirm", ""));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("no_trial", "no network trial running"));
  r = fakes::http::perform(apiPost("/api/system/network/revert", ""));
  CHECK(r.code == 409);
  CHECK(sib::net().trialConfirms == 1);
  CHECK(sib::net().trialReverts == 1);
  sib::net().trialConfirmResult = true;
  sib::net().trialRevertResult = true;
  r = fakes::http::perform(apiPost("/api/system/network/confirm", ""));
  CHECK(r.code == 202);
  CHECK(r.body == "{\"result\":\"queued\"}");
  r = fakes::http::perform(apiPost("/api/system/network/revert", ""));
  CHECK(r.code == 202);
  CHECK(sib::net().trialConfirms == 2);
  CHECK(sib::net().trialReverts == 2);
}

// ---------------------------------------------------------------- files (WG-13, WG-14)

TEST_CASE("WG-13: the file list and file removal") {
  glue::begin();
  vdm::FileEntry a{};
  vdm::copyString(a.path, sizeof a.path, "/x.bin");
  a.size = 1024;
  vdm::FileEntry b{};
  vdm::copyString(b.path, sizeof b.path, "/sys/cfg.bak");
  b.size = 300;
  sib::storage().files = {a, b};
  sib::storage().fsTotal = 1000000;
  sib::storage().fsUsed = 5000;
  start();
  Response r = fakes::http::perform(fakes::http::get("/api/files"));
  CHECK(r.code == 200);
  CHECK(r.body ==
        "{\"total\":1000000,\"used\":5000,\"truncated\":false,\"files\":["
        "{\"path\":\"/x.bin\",\"size\":1024,\"kind\":\"legacy_image\",\"deletable\":true},"
        "{\"path\":\"/sys/cfg.bak\",\"size\":300,\"kind\":\"internal\",\"deletable\":false}]}");
  sib::storage().filesTruncated = true;
  CHECK(fakes::http::perform(fakes::http::get("/api/files")).body.find("\"truncated\":true") !=
        std::string::npos);

  sib::storage().deleteFileResult = storage::FileResult::Ok;
  r = fakes::http::perform(apiDel("/api/files?path=/x.bin"));
  CHECK(r.code == 204);
  CHECK(sib::storage().deletedFiles == std::vector<std::string>{"/x.bin"});
  sib::storage().deleteFileResult = storage::FileResult::Protected;
  r = fakes::http::perform(apiDel("/api/files?path=/sys/cfg.bak"));
  CHECK(r.code == 403);
  CHECK(r.body == errorBody("protected", vdm::fileProtectReason(vdm::FileKind::Internal)));
  r = fakes::http::perform(apiDel("/api/files?path=//x"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_path", "//x"));
  CHECK(sib::storage().deletedFiles.size() == 2);  // an invalid path never reaches storage
  r = fakes::http::perform(apiDel("/api/files"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_path", "path"));
  sib::storage().deleteFileResult = storage::FileResult::NotFound;
  r = fakes::http::perform(apiDel("/api/files?path=/y.bin"));
  CHECK(r.code == 404);
  CHECK(r.body == errorBody("not_found", "/y.bin"));
  sib::storage().deleteFileResult = storage::FileResult::BadPath;
  CHECK(fakes::http::perform(apiDel("/api/files?path=/y.bin")).code == 400);
  sib::storage().deleteFileResult = storage::FileResult::Io;
  r = fakes::http::perform(apiDel("/api/files?path=/y.bin"));
  CHECK(r.code == 500);
  CHECK(r.body == errorBody("io_error", "/y.bin"));
}

TEST_CASE("WG-14: the import report is streamed and dismissed") {
  glue::begin();
  start();
  Response r = fakes::http::perform(fakes::http::get("/api/import-report"));
  CHECK(r.code == 404);
  CHECK(r.body == errorBody("not_found", "no import report"));
  const std::string report = "{\"imported\":57,\"rejected\":2}";
  fakes::fs().mounted = true;
  fakes::fs().put(storage::kImportReportFile, report);
  sib::storage().hasImportReport = true;
  r = fakes::http::perform(fakes::http::get("/api/import-report"));
  CHECK(r.code == 200);
  CHECK(r.contentType == "application/json");
  CHECK(r.body == report);
  r = fakes::http::perform(apiDel("/api/import-report"));
  CHECK(r.code == 204);
  CHECK(sib::storage().importReportDismissals == 1);
  r = fakes::http::perform(fakes::http::get("/api/import-report"));
  CHECK(r.code == 404);
  r = fakes::http::perform(apiDel("/api/import-report"));
  CHECK(r.code == 404);
  // flag set but the file is gone
  sib::storage().hasImportReport = true;
  fakes::fs().nodes.erase(storage::kImportReportFile);
  CHECK(fakes::http::perform(fakes::http::get("/api/import-report")).code == 404);
}

// ---------------------------------------------------------------- status (WG-15)

TEST_CASE("WG-15: status members of 2.1") {
  glue::begin();
  vdm::copyString(sib::storage().active.station, sizeof sib::storage().active.station,
                  "Dom P\xc3\xb3\xc5\x82noc");
  sib::net().trial.active = true;
  sib::net().trial.remainS = 97;
  vdm::copyString(sib::mqtt().status.clientId, sizeof sib::mqtt().status.clientId, "vdm-1");
  sib::mqtt().status.haStatus = vdm::HaStatus::Online;
  sib::app().snapshot.support = vdm::StmSupport::Supported;
  sib::app().snapshot.haveLearnTime = true;
  sib::app().snapshot.learnTimeS = 600;
  sib::storage().loadSource = storage::LoadSource::Backup;
  sib::storage().loadDetails.info.repairs.mask = 6;
  sib::storage().loadDetails.info.decode.newerSchema = true;
  sib::storage().hasImportReport = true;
  start();
  Request host = fakes::http::get("/api/status");
  host.host = "dom-p-noc.local";
  const Response r = fakes::http::perform(host);
  CHECK(r.code == 200);
  CHECK(r.body.rfind("{\"station\":\"Dom P\xc3\xb3\xc5\x82noc\",\"esp\":", 0) == 0);
  CHECK(r.body.find("\"hostname\":\"Dom-P-noc\",\"trial\":{\"remainS\":97}}") !=
        std::string::npos);
  CHECK(r.body.find("\"clientId\":\"vdm-1\",\"haStatus\":\"online\"}") != std::string::npos);
  CHECK(r.body.find("\"support\":\"ok\",\"lease\":null,\"learnTime\":600}") != std::string::npos);
  CHECK(r.body.find("\"config\":{\"source\":\"backup\",\"repairs\":6,\"newerSchema\":true},"
                    "\"importReport\":true}") != std::string::npos);
  const char* const sources[] = {"stored", "imported", "defaults", "defaults_after_error"};
  for (uint8_t i = 0; i < 4; ++i) {
    sib::storage().loadSource = static_cast<storage::LoadSource>(i);
    const Response s = fakes::http::perform(host);
    CHECK(s.body.find(std::string("\"config\":{\"source\":\"") + sources[i]) != std::string::npos);
  }
  sib::net().trial.active = false;
  sib::storage().hasImportReport = false;
  const Response n = fakes::http::perform(host);
  CHECK(n.body.find("\"trial\":null}") != std::string::npos);
  CHECK(n.body.find("\"importReport\":false}") != std::string::npos);
}

TEST_CASE("WG-15: calibration.next is the local time of the next slot") {
  glue::begin();
  sib::app().calib.nextEpoch = 1790000000;  // 2026-09-21 14:13:20 UTC
  start();
  Response r = fakes::http::perform(fakes::http::get("/api/status"));
  const time_t t = 1790000000;
  struct tm tm;
  REQUIRE(localtime_r(&t, &tm) != nullptr);
  char want[64];
  snprintf(want, sizeof want, "\"next\":\"%04d-%02d-%02dT%02d:%02d:%02d\"}", tm.tm_year + 1900,
           tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec);
  CHECK(r.body.find(want) != std::string::npos);
  sib::app().calib.nextEpoch = 0;
  r = fakes::http::perform(fakes::http::get("/api/status"));
  CHECK(r.body.find("\"next\":null}") != std::string::npos);
}

TEST_CASE("web: valves carry the sensor position and the calibration end") {
  glue::begin();
  vdm::Config& c = sib::storage().active;
  vdm::copyString(c.temps[6].name, sizeof c.temps[6].name, "Wall");
  c.temps[6].offset = -3;
  vdm::ValveState& v = sib::app().snapshot.valves[0];
  v.sensorSlot[1] = 7;
  v.temp2 = 210;
  sib::mqtt().calibEnded[0] = true;
  vdm::LocalTime& e = sib::mqtt().calibEnd[0];
  e.valid = true;
  e.year = 2026;
  e.month = 9;
  e.mday = 20;
  e.hour = 3;
  e.minute = 4;
  e.second = 5;
  start();
  const Response r = fakes::http::perform(fakes::http::get("/api/valves"));
  CHECK(r.code == 200);
  CHECK(r.body.find("\"sensors\":[{\"sensor\":2,\"slot\":7,\"name\":\"Wall\",\"temp\":20.7}]") !=
        std::string::npos);
  CHECK(r.body.find("\"calibrationEnd\":\"2026-09-20T03:04:05\"},{\"idx\":2,") !=
        std::string::npos);
  CHECK(countOf(r.body, "\"calibrationEnd\":null") == 11);
}

// ---------------------------------------------------------------- health (WG-17)

TEST_CASE("WG-17: /api/health is public and needs no response slot") {
  glue::begin();
  enableAuth();
  sib::storage().active.web.protectRead = true;
  sib::net().info.ip = 0x3301A8C0;
  sib::app().health.version = "2.1.0-revamped";
  sib::app().health.uptimeS = 77;
  start();
  fakes::http::Exchange a(withAuth(fakes::http::get("/api/status")));
  fakes::http::Exchange b(withAuth(fakes::http::get("/api/status")));
  Request h = fakes::http::get("/api/health");
  h.host = "192.168.1.51";
  Response r = fakes::http::perform(h);
  CHECK(r.code == 200);
  CHECK(r.contentType == "application/json");
  CHECK(r.header("Cache-Control") == "no-store");
  CHECK(r.body.rfind("{\"ok\":true,\"version\":\"2.1.0-revamped\",\"uptime\":77,", 0) == 0);
  CHECK(r.body.back() == '}');
  CHECK(sib::app().healthReads == 1);
  h.host = "evil.com";
  CHECK(fakes::http::perform(h).code == 403);
  CHECK(sib::app().healthReads == 1);
  a.finish();
  b.finish();
}

// ---------------------------------------------------------------- sys hooks (WG-18)

TEST_CASE("WG-18: every served request reports its client; /api/log flushes first") {
  glue::begin();
  start();
  Request g = fakes::http::get("/api/status");
  g.remoteIp = 0x1401A8C0;  // 192.168.1.20
  fakes::http::perform(g);
  CHECK(sib::net().inboundHttp == std::vector<uint32_t>{0x1401A8C0u});
  Request refused = fakes::http::post("/api/valves/1/target", "{}");
  refused.remoteIp = 0x1501A8C0;
  fakes::http::perform(refused);
  Request asset = fakes::http::get("/");
  asset.remoteIp = 0x1601A8C0;
  fakes::http::perform(asset);
  CHECK(sib::net().inboundHttp ==
        std::vector<uint32_t>{0x1401A8C0u, 0x1501A8C0u, 0x1601A8C0u});
  fakes::fs().mounted = true;
  fakes::fs().put("/log/events.log", "#1 line\n");
  fakes::http::Exchange log(fakes::http::get("/api/log"));
  CHECK(sib::logger().flushRequests == 1);
  const Response& lr = log.finish();
  CHECK(lr.code == 200);
  CHECK(lr.body == "#1 line\n");
}

// ---------------------------------------------------------------- STM support and v3 routes

TEST_CASE("web: STM actions are refused while the STM firmware is too old") {
  glue::begin();
  sib::app().support = vdm::StmSupport::TooOld;
  REQUIRE(vdm::parseVersion("1.3.9", 5, sib::app().snapshot.version));
  sib::app().proto = 2;
  start();
  const std::string detail = "STM firmware 1.3.9 is older than 1.4.0: update the STM";
  for (const char* url : {"/api/valves/1/calibrate", "/api/valves/1/assembly",
                          "/api/valves/calibrate", "/api/valves/assembly", "/api/valves/detect",
                          "/api/sensors/scan", "/api/valves/1/profile", "/api/valves/1/stop",
                          "/api/valves/stop", "/api/stm/safe-mode/leave"}) {
    CAPTURE(url);
    const Response r = fakes::http::perform(apiPost(url, ""));
    CHECK(r.code == 409);
    CHECK(r.body == errorBody("stm_unsupported", detail.c_str()));
  }
  Response r = fakes::http::perform(
      apiPost("/api/valves/1/service-move", "{\"dir\":\"open\",\"counts\":10,\"maxmA\":20}"));
  CHECK(r.code == 409);
  r = fakes::http::perform(apiPost("/api/valves/1/sensors", "{\"slot1\":0,\"slot2\":0}"));
  CHECK(r.code == 409);
  r = fakes::http::perform(apiPost("/api/stm/motor", "{\"learnMovements\":0}"));
  CHECK(r.code == 409);
  CHECK(sib::app().submitted.empty());
  // target and reset still work
  CHECK(fakes::http::perform(apiPost("/api/valves/1/target", "{\"target\":5}")).code == 202);
  CHECK(fakes::http::perform(apiPost("/api/stm/reset", "{\"confirm\":true}")).code == 202);
  CHECK(sib::app().submitted.size() == 2);
  // an unreadable version is shown as "?"
  sib::app().snapshot.version = vdm::Version{};
  r = fakes::http::perform(apiPost("/api/valves/detect", ""));
  CHECK(r.body ==
        errorBody("stm_unsupported", "STM firmware ? is older than 1.4.0: update the STM"));
}

TEST_CASE("web: stop and safe-mode leave need protocol 3") {
  glue::begin();
  sib::app().proto = 2;
  start();
  Response r = fakes::http::perform(apiPost("/api/valves/3/stop", ""));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("stm_unsupported", "STM protocol 3 required"));
  CHECK(fakes::http::perform(apiPost("/api/stm/safe-mode/leave", "")).code == 409);
  sib::app().proto = 3;
  r = fakes::http::perform(apiPost("/api/valves/3/stop", ""));
  CHECK(r.code == 202);
  CHECK(r.body == "{\"result\":\"queued\"}");
  r = fakes::http::perform(apiPost("/api/valves/stop", ""));
  CHECK(r.code == 202);
  r = fakes::http::perform(apiPost("/api/stm/safe-mode/leave", ""));
  CHECK(r.code == 202);
  REQUIRE(sib::app().submitted.size() == 3);
  CHECK(sib::app().submitted[0].type == app::CommandType::StopValve);
  CHECK(sib::app().submitted[0].valve == 2);
  CHECK(sib::app().submitted[1].type == app::CommandType::StopValve);
  CHECK(sib::app().submitted[1].valve == vdm::kAllValves);
  CHECK(sib::app().submitted[2].type == app::CommandType::LeaveSafeMode);
  sib::app().flashActive = true;
  CHECK(fakes::http::perform(apiPost("/api/valves/stop", "")).code == 409);
  sib::app().flashActive = false;
  sib::app().submitResult = false;
  CHECK(fakes::http::perform(apiPost("/api/stm/safe-mode/leave", "")).code == 503);
}

TEST_CASE("web: MQTT discovery rules per mode") {
  glue::begin();
  vdm::Config& c = sib::storage().active;
  c.mqtt.mode = vdm::MqttMode::Off;
  start();
  Response r = fakes::http::perform(apiPost("/api/mqtt/discovery", "{\"action\":\"publish\"}"));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("disabled", "MQTT is off"));
  c.mqtt.mode = vdm::MqttMode::Mqtt;
  c.mqtt.separate = false;
  ++sib::storage().revision;
  r = fakes::http::perform(apiPost("/api/mqtt/discovery", "{\"action\":\"publish\"}"));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("separate_required", "enable mqtt.separate first"));
  r = fakes::http::perform(apiPost("/api/mqtt/discovery", "{\"action\":\"republish\"}"));
  CHECK(r.code == 409);
  r = fakes::http::perform(apiPost("/api/mqtt/discovery", "{\"action\":\"delete\"}"));
  CHECK(r.code == 202);
  c.mqtt.separate = true;
  ++sib::storage().revision;
  r = fakes::http::perform(apiPost("/api/mqtt/discovery", "{\"action\":\"publish\"}"));
  CHECK(r.code == 202);
  c.mqtt.mode = vdm::MqttMode::MqttHa;
  ++sib::storage().revision;
  r = fakes::http::perform(apiPost("/api/mqtt/discovery", "{\"action\":\"republish\"}"));
  CHECK(r.code == 202);
  CHECK(sib::mqtt().discoveryRequests ==
        std::vector<mqtt::DiscoveryAction>{mqtt::DiscoveryAction::Delete,
                                           mqtt::DiscoveryAction::Publish,
                                           mqtt::DiscoveryAction::DeleteAndPublish});
}

TEST_CASE("web: a flash checks the board revision of the image") {
  glue::begin();
  storage::ImageEntry e;
  vdm::copyString(e.name, sizeof e.name, "fw");
  e.scanned = true;
  e.check = vdm::FlashError::None;
  vdm::copyString(e.hwTag, sizeof e.hwTag, "C1");
  sib::storage().images = {e};
  start();
  // the running STM is unknown: blank mode without a board choice is refused
  Response r = fakes::http::perform(apiPost("/api/stm/flash", "{\"image\":\"fw\",\"mode\":\"blank\"}"));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("board_required", "choose the board: C1 or C2"));
  r = fakes::http::perform(
      apiPost("/api/stm/flash", "{\"image\":\"fw\",\"mode\":\"blank\",\"board\":\"C2\"}"));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("board_mismatch", "image C1, board C2"));
  CHECK(sib::app().submitted.empty());
  r = fakes::http::perform(
      apiPost("/api/stm/flash", "{\"image\":\"fw\",\"mode\":\"blank\",\"board\":\"C3\"}"));
  CHECK(r.code == 400);
  r = fakes::http::perform(
      apiPost("/api/stm/flash", "{\"image\":\"fw\",\"mode\":\"blank\",\"board\":\"C1\"}"));
  CHECK(r.code == 202);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(std::string(sib::app().submitted[0].board) == "C1");
  CHECK(sib::app().submitted[0].blank);
  // the running STM's tag wins over the choice; force skips the check
  REQUIRE(vdm::parseVersion("2.1.0-revamped_C2", 17, sib::app().snapshot.version));
  r = fakes::http::perform(
      apiPost("/api/stm/flash", "{\"image\":\"fw\",\"board\":\"C1\"}"));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("board_mismatch", "image C1, board C2"));
  r = fakes::http::perform(
      apiPost("/api/stm/flash", "{\"image\":\"fw\",\"force\":true}"));
  CHECK(r.code == 202);
  CHECK(std::string(sib::app().submitted[1].board) == "");
  // an untagged image flashes on any board
  sib::storage().images[0].hwTag[0] = '\0';
  r = fakes::http::perform(apiPost("/api/stm/flash", "{\"image\":\"fw\"}"));
  CHECK(r.code == 202);
}

TEST_CASE("web: images and the flash status show the board revision") {
  glue::begin();
  storage::ImageEntry e;
  vdm::copyString(e.name, sizeof e.name, "fw");
  e.scanned = true;
  vdm::copyString(e.hwTag, sizeof e.hwTag, "C2");
  storage::ImageEntry f;
  vdm::copyString(f.name, sizeof f.name, "old");
  sib::storage().images = {e, f};
  sib::app().snapshot.flashPending = true;
  start();
  Response r = fakes::http::perform(fakes::http::get("/api/stm/images"));
  CHECK(r.body.find("\"name\":\"fw\"") != std::string::npos);
  CHECK(r.body.find("\"hw\":\"C2\"}") != std::string::npos);
  CHECK(r.body.find("\"check\":null,\"hw\":null}") != std::string::npos);
  r = fakes::http::perform(fakes::http::get("/api/stm/flash"));
  CHECK(r.body.find("\"pending\":true}") != std::string::npos);
}
