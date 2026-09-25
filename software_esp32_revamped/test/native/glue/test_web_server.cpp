// Smoke tests of src/web_server.cpp through the request driver: routing, guard, auth, response
// slots, static assets, uploads.
#include <string>

#include "generated/web_assets.h"
#include "glue_test.h"
#include "web_server.h"

namespace {

using fakes::http::Response;

std::string errorBody(const char* code, const char* detail) {
  return std::string("{\"error\":\"") + code + "\",\"detail\":\"" + detail + "\"}";
}

void start() {
  sib::storage().active.valves[0].active = true;
  ++sib::storage().revision;
  web::begin();
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
  CHECK(r.body.front() == '{');
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
  r = fakes::http::perform(fakes::http::del("/api/status"));
  CHECK(r.code == 405);
  r = fakes::http::perform(fakes::http::get("/missing.html"));
  CHECK(r.code == 404);
  r = fakes::http::perform(fakes::http::post("/", ""));
  CHECK(r.code == 405);
  r = fakes::http::perform(fakes::http::post("/", "{}"));  // the guard: no body expected
  CHECK(r.code == 413);
  CHECK(r.body == errorBody("too_large", "no body expected"));
}

TEST_CASE("web guard: a JSON body over 4096 bytes is refused without being buffered") {
  glue::begin();
  start();
  const Response r = fakes::http::perform(fakes::http::post("/api/config", std::string(4097, ' ')));
  CHECK(r.code == 413);
  CHECK(r.body == errorBody("too_large", "body"));
  CHECK(sib::storage().applied.empty());
}

TEST_CASE("web guard: uploads need multipart and a length") {
  glue::begin();
  start();
  Response r = fakes::http::perform(fakes::http::post("/api/stm/images", "x", "application/json"));
  CHECK(r.code == 415);
  fakes::http::Request u = fakes::http::upload("/api/stm/images", "fw.bin", "abc");
  u.contentLength = 0;
  r = fakes::http::perform(u);
  CHECK(r.code == 411);
}

TEST_CASE("web: a valve target is submitted and answered 202") {
  glue::begin();
  start();
  const Response r =
      fakes::http::perform(fakes::http::post("/api/valves/1/target", "{\"target\":50}"));
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
  Response r = fakes::http::perform(fakes::http::post("/api/valves/1/target", "{\"target\":101}"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("out_of_range", "target 0..100"));
  r = fakes::http::perform(fakes::http::post("/api/valves/2/target", "{\"target\":1}"));
  CHECK(r.code == 409);
  sib::app().submitResult = false;
  r = fakes::http::perform(fakes::http::post("/api/valves/1/target", "{\"target\":1}"));
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("queue_full", "STM command queue full"));
}

TEST_CASE("web auth: without credentials a protected route answers 401 with a challenge") {
  glue::begin();
  vdm::Config& c = sib::storage().active;
  vdm::copyString(c.web.user, sizeof c.web.user, "admin");
  vdm::copyString(c.web.password, sizeof c.web.password, "secret12");
  start();
  Response r = fakes::http::perform(fakes::http::post("/api/valves/1/target", "{\"target\":5}"));
  CHECK(r.code == 401);
  CHECK(r.header("WWW-Authenticate") == "Basic realm=\"VdMot\"");
  CHECK(sib::app().submitted.empty());
  fakes::http::Request ok = fakes::http::post("/api/valves/1/target", "{\"target\":5}");
  ok.header("Authorization", "Basic YWRtaW46c2VjcmV0MTI=");  // admin:secret12
  r = fakes::http::perform(ok);
  CHECK(r.code == 202);
  fakes::http::Request wrong = fakes::http::post("/api/valves/1/target", "{\"target\":5}");
  wrong.header("Authorization", "Basic YWRtaW46d3Jvbmc=");  // admin:wrong
  r = fakes::http::perform(wrong);
  CHECK(r.code == 401);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::AuthFailed).at(0);
  CHECK(std::string(e.text) == "192.168.1.50");
}

TEST_CASE("web: reboot and MQTT reconnect are handed to their modules") {
  glue::begin();
  start();
  Response r = fakes::http::perform(fakes::http::post("/api/system/reboot", ""));
  CHECK(r.code == 202);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(sib::ota().restartRequests[0].reason == 0);
  CHECK(sib::ota().restartRequests[0].delayMs == 1000);
  r = fakes::http::perform(fakes::http::post("/api/mqtt/reconnect", ""));
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
  const Response r = fakes::http::perform(fakes::http::upload("/api/stm/images", "fw.bin", "abc"));
  CHECK(r.code == 201);
  CHECK(sib::storage().uploadData == "abc");
  REQUIRE(sib::storage().uploadBegins.size() == 1);
  CHECK(sib::storage().uploadBegins[0].name == "fw.bin");
  CHECK(r.body.find("\"name\":\"fw\"") != std::string::npos);
}

TEST_CASE("web upload: an ESP image goes to ota and asks for the restart") {
  glue::begin();
  start();
  const Response r = fakes::http::perform(
      fakes::http::upload("/api/ota/esp?md5=0123456789abcdef0123456789abcdef", "fw.bin", "img"));
  CHECK(r.code == 200);
  CHECK(r.body == "{\"result\":\"ok\",\"restart\":true}");
  REQUIRE(sib::ota().uploadBegins.size() == 1);
  CHECK(sib::ota().uploadBegins[0].second == "0123456789abcdef0123456789abcdef");
  CHECK(sib::ota().uploadData == "img");
  CHECK(sib::ota().uploadEnds == std::vector<bool>{true});
}
