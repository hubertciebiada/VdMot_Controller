// Tests of src/web_server.cpp, request side: response slots and the error buffer, the marks of
// refused bodies, the request guard, JSON input and the upload limits.
#include <sdkconfig.h>

#include <memory>
#include <string>
#include <vector>

#include <vdm/json_api.h>
#include <vdm/json_writer.h>

#include "glue_test.h"
#include "web_server.h"

namespace {

using fakes::http::Exchange;
using fakes::http::Request;
using fakes::http::Response;

std::string errorBody(const std::string& code, const std::string& detail) {
  return "{\"error\":\"" + code + "\",\"detail\":\"" + detail + "\"}";
}

void start() {
  sib::storage().active.valves[0].active = true;
  ++sib::storage().revision;
  web::begin();
}

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

const char* const kTarget1 = "/api/valves/1/target";

}  // namespace

// ---------------------------------------------------------------- slots and error buffer

TEST_CASE("web slots: a document whose response cannot be allocated answers 500, slot freed") {
  glue::begin();
  start();
  fakes::http::server().failNextResponse = true;
  const Response r = fakes::http::perform(fakes::http::get("/api/status"));
  CHECK(r.code == 500);
  CHECK(r.sends == 1);
  Exchange a(fakes::http::get("/api/status"));
  Exchange b(fakes::http::get("/api/status"));
  CHECK(a.finish().code == 200);
  CHECK(b.finish().code == 200);
}

TEST_CASE("web slots: the second slot carries its own document") {
  glue::begin();
  start();
  Exchange a(fakes::http::get("/api/status"));
  const Response v = fakes::http::perform(fakes::http::get("/api/valves"));
  CHECK(v.code == 200);
  CHECK(v.body.rfind("{\"valves\":[{\"idx\":1,", 0) == 0);
  const Response& s = a.finish();
  CHECK(s.code == 200);
  CHECK(s.body.rfind("{\"station\":\"VdMot\",", 0) == 0);
}

TEST_CASE("web: an error answer longer than its 200-byte buffer becomes internal") {
  glue::begin();
  start();
  // the longest path whose not_found answer fits 199 characters and the terminator
  size_t n = 0;
  for (size_t len = 1; len < 300; ++len) {
    char buf[200];
    vdm::JsonWriter jw(buf, sizeof buf);
    const std::string url = "/api/" + std::string(len, 'a');
    if (vdm::writeErrorJson(jw, "not_found", url.c_str()) && jw.complete()) n = len;
  }
  const std::string fits = "/api/" + std::string(n, 'a');
  Response r = fakes::http::perform(fakes::http::get(fits));
  CHECK(r.code == 404);
  CHECK(r.body == errorBody("not_found", fits));
  CHECK(r.body.size() == 199);
  r = fakes::http::perform(fakes::http::get(fits + "a"));
  CHECK(r.code == 404);
  CHECK(r.body == "{\"error\":\"internal\"}");
}

// ---------------------------------------------------------------- marks of refused bodies

TEST_CASE("web body: bodies arriving while one is buffered are answered 409, each its own") {
  glue::begin();
  start();
  Exchange a(apiPost(kTarget1, "{\"target\":5}"));
  a.sendBody(4);
  std::vector<std::unique_ptr<Exchange>> others;
  for (int i = 0; i < 4; ++i) {
    others.emplace_back(new Exchange(apiPost(kTarget1, "{\"target\":6}")));
    others.back()->sendBody(4);
  }
  for (std::unique_ptr<Exchange>& e : others) {
    const Response& r = e->finish();
    CHECK(r.code == 409);
    CHECK(r.body == errorBody("busy", "body"));
  }
  CHECK(a.finish().code == 202);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].pos == 5);
}

TEST_CASE("web body: one mark per connection the server holds, none gives way") {
  glue::begin();
  start();
  Exchange a(apiPost(kTarget1, "{\"target\":5}"));
  a.sendBody(4);
  // as many as the server holds (the listen backlog), and the body owner next to them
  std::vector<std::unique_ptr<Exchange>> others;
  for (size_t i = 0; i < web::kMaxConnections; ++i) {
    others.emplace_back(new Exchange(apiPost(kTarget1, "{\"target\":6}")));
    others.back()->sendBody(4);
  }
  for (std::unique_ptr<Exchange>& e : others) {
    const Response& r = e->finish();
    CHECK(r.code == 409);
    CHECK(r.body == errorBody("busy", "body"));
  }
  CHECK(a.finish().code == 202);
  CHECK(sib::app().submitted.size() == 1);
}

TEST_CASE("web body: a body to a path outside /api/ is refused, also while one is buffered") {
  glue::begin();
  start();
  Exchange a(apiPost(kTarget1, "{\"target\":5}"));
  a.sendBody(4);
  Response r = fakes::http::perform(fakes::http::post("/nope", "{\"a\":1}"));
  CHECK(r.code == 405);
  CHECK(r.body == errorBody("method_not_allowed", "/nope"));
  Request getBody = fakes::http::get("/nope");
  getBody.body = "x";
  r = fakes::http::perform(getBody);
  CHECK(r.code == 404);
  CHECK(r.body == errorBody("not_found", "/nope"));
  CHECK(a.finish().code == 202);
}

// ---------------------------------------------------------------- guard

TEST_CASE("web guard: the longest client address is logged in full") {
  glue::begin();
  start();
  Request refused = fakes::http::post(kTarget1, "{\"target\":5}");
  refused.remoteIp = 0xFFFFFFFF;
  CHECK(fakes::http::perform(refused).code == 403);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::RequestRefused);
  REQUIRE(ev.size() == 1);
  CHECK(std::string(ev[0].text) == "255.255.255.255");
  CHECK(ev[0].arg1 == 3);
  CHECK(ev[0].arg2 == 0);
}

TEST_CASE("web guard: a POST without a body needs no content type, a one-byte body does") {
  glue::begin();
  start();
  Request bare = fakes::http::post("/api/system/reboot", "", "");
  bare.header("X-VdMot", "1");
  Response r = fakes::http::perform(bare);
  CHECK(r.code == 202);
  Request one = fakes::http::post("/api/system/reboot", "x", "text/plain");
  one.header("X-VdMot", "1");
  r = fakes::http::perform(one);
  CHECK(r.code == 415);
  CHECK(r.body == errorBody("unsupported_media_type", "application/json required"));
  CHECK(sib::ota().restartRequests.size() == 1);
}

TEST_CASE("web guard: an oversized body is refused, legacy alias or API") {
  glue::begin();
  start();
  Response r = fakes::http::perform(fakes::http::post("/setvalve", std::string(8193, ' ')));
  CHECK(r.code == 413);
  CHECK(r.body == errorBody("too_large", "body"));
  r = fakes::http::perform(apiPost("/api/config", std::string(8193, ' ')));
  CHECK(r.code == 413);
  CHECK(r.body == errorBody("too_large", "body"));
  CHECK(sib::app().submitted.empty());
}

TEST_CASE("web guard: an oversized body never takes the body buffer") {
  glue::begin();
  start();
  const Request big[] = {fakes::http::post("/setvalve", std::string(8193, ' ')),
                         apiPost("/api/config", std::string(8193, ' '))};
  for (const Request& b : big) {
    CAPTURE(b.url);
    Exchange a(b);
    a.sendBody(4000);  // the refused body is still arriving
    // another request's body gets the buffer meanwhile
    CHECK(fakes::http::perform(apiPost(kTarget1, "{\"target\":5}")).code == 202);
    const Response& r = a.finish();
    CHECK(r.code == 413);
    CHECK(r.body == errorBody("too_large", "body"));
  }
  CHECK(sib::app().submitted.size() == 2);
}

TEST_CASE("web guard: /setvalve reads a body of 8192 bytes") {
  glue::begin();
  start();
  const Response r = fakes::http::perform(fakes::http::post("/setvalve", std::string(8192, ' ')));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "EmptyInput"));
}

TEST_CASE("web guard: a multipart body outside the upload routes is refused") {
  glue::begin();
  start();
  Request d = apiDel("/api/files?path=/x.bin");
  d.contentType = "multipart/form-data; boundary=b";
  d.body = "x";
  Response r = fakes::http::perform(d);
  CHECK(r.code == 415);
  CHECK(r.body == errorBody("unsupported_media_type", "application/json expected"));
  CHECK(sib::storage().deletedFiles.empty());
  Request empty = apiDel("/api/files?path=/x.bin");
  empty.contentType = "multipart/form-data; boundary=b";
  r = fakes::http::perform(empty);
  CHECK(r.code == 404);
  CHECK(r.body == errorBody("not_found", "/x.bin"));
  CHECK(sib::storage().deletedFiles == std::vector<std::string>{"/x.bin"});
}

TEST_CASE("web guard: an STM image may be 512 KiB plus 8 KiB of framing") {
  glue::begin();
  start();
  const size_t limit = web::kMaxStmImageSize + web::kMultipartSlack;
  Request u = apiUpload("/api/stm/images", "fw.bin", "abc");
  u.contentLength = limit;
  Response r = fakes::http::perform(u);
  CHECK(r.code == 201);
  REQUIRE(sib::storage().uploadBegins.size() == 1);
  CHECK(sib::storage().uploadBegins[0].announcedBytes == limit);
  CHECK(sib::storage().uploadData == "abc");
  u.contentLength = limit + 1;
  r = fakes::http::perform(u);
  CHECK(r.code == 413);
  CHECK(r.body == errorBody("too_large", "file"));
  CHECK(sib::storage().uploadBegins.size() == 1);
}

TEST_CASE("web guard: an ESP image may fill the next OTA partition plus 8 KiB of framing") {
  glue::begin();
  start();
  const size_t limit = fakes::ota().parts[1].size + web::kMultipartSlack;
  Request u = apiUpload("/api/ota/esp", "fw.bin", "img");
  u.contentLength = limit;
  Response r = fakes::http::perform(u);
  CHECK(r.code == 200);
  REQUIRE(sib::ota().uploadBegins.size() == 1);
  u.contentLength = limit + 1;
  r = fakes::http::perform(u);
  CHECK(r.code == 413);
  CHECK(r.body == errorBody("too_large", "file"));
  // without a partition to write only the framing fits
  fakes::ota().hasNext = false;
  u.contentLength = web::kMultipartSlack + 1;
  r = fakes::http::perform(u);
  CHECK(r.code == 413);
  u.contentLength = web::kMultipartSlack;
  r = fakes::http::perform(u);
  CHECK(r.code == 200);
  CHECK(sib::ota().uploadBegins.size() == 2);
}

TEST_CASE("web guard: an upload waits for every other upload and the flash") {
  glue::begin();
  start();
  const Request u = apiUpload("/api/stm/images", "fw.bin", "abc");
  const std::string busy = errorBody("busy", "upload or flash running");
  sib::ota().uploadActive = true;
  Response r = fakes::http::perform(u);
  CHECK(r.code == 409);
  CHECK(r.body == busy);
  sib::ota().uploadActive = false;
  sib::storage().imageUploadActive = true;
  r = fakes::http::perform(u);
  CHECK(r.code == 409);
  CHECK(r.body == busy);
  sib::storage().imageUploadActive = false;
  sib::app().flashActive = true;
  r = fakes::http::perform(u);
  CHECK(r.code == 409);
  CHECK(r.body == busy);
  sib::app().flashActive = false;
  CHECK(sib::storage().uploadBegins.empty());
  // an upload of this server in progress
  Exchange a(apiUpload("/api/ota/esp", "fw.bin", std::string(3000, 'i')));
  a.sendBody(2000);
  REQUIRE(sib::ota().uploadBegins.size() == 1);
  sib::ota().uploadActive = false;  // only the upload state of the server says busy
  r = fakes::http::perform(u);
  CHECK(r.code == 409);
  CHECK(r.body == busy);
  sib::ota().uploadActive = true;
  CHECK(a.finish().code == 200);
  CHECK(fakes::http::perform(u).code == 201);
}

TEST_CASE("web guard: a refusal that no longer holds when the body ends answers retry") {
  glue::begin();
  start();
  sib::ota().uploadActive = true;
  Exchange e(apiUpload("/api/stm/images", "fw.bin", "abc"));
  sib::ota().uploadActive = false;
  const Response& r = e.finish();
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("retry", "state changed"));
  CHECK(sib::storage().uploadBegins.empty());
}

// ---------------------------------------------------------------- uploads

TEST_CASE("web upload: the error of a failed upload is kept up to 47 characters") {
  glue::begin();
  start();
  sib::ota().uploadBeginResult = false;
  sib::ota().uploadError = std::string(47, 'e');
  Response r = fakes::http::perform(apiUpload("/api/ota/esp", "fw.bin", "img"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("upload_failed", std::string(47, 'e')));
  sib::ota().uploadError = std::string(48, 'f');
  r = fakes::http::perform(apiUpload("/api/ota/esp", "fw.bin", "img"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("upload_failed", std::string(47, 'f')));
}

TEST_CASE("web upload: a file part without its end is an incomplete file") {
  glue::begin();
  start();
  Request u = apiUpload("/api/stm/images", "fw.bin", std::string(2000, 'd'));
  u.contentLength = u.wireBody().size() - 12;  // the closing delimiter never arrives
  const Response r = fakes::http::perform(u);
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("upload_failed", "incomplete file"));
  CHECK_FALSE(sib::storage().uploadData.empty());  // what arrived before the end was missed
  CHECK(sib::storage().uploadData.find_first_not_of('d') == std::string::npos);
  CHECK(sib::storage().uploadAborts == 1);
  CHECK(sib::storage().uploadEnds == 0);
}

// ---------------------------------------------------------------- JSON input

TEST_CASE("web JSON: a request without a body never sees the previous one") {
  glue::begin();
  start();
  CHECK(fakes::http::perform(apiPost(kTarget1, "{\"target\":5}")).code == 202);
  Response r = fakes::http::perform(apiPost(kTarget1, ""));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "JSON body required"));
  r = fakes::http::perform(apiPost(kTarget1, "7"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "object expected"));
  r = fakes::http::perform(apiPost(kTarget1, "{"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "IncompleteInput"));
  CHECK(sib::app().submitted.size() == 1);
}

TEST_CASE("web JSON: optional members, strict types and the upper bound") {
  glue::begin();
  sib::app().proto = 2;
  vdm::StmSnapshot& s = sib::app().snapshot;
  s.haveBreakaway = true;
  s.breakaway.stepPct = 10;
  s.breakaway.maxmA = 30;
  start();
  const char* const motor = "/api/stm/motor";
  Response r = fakes::http::perform(apiPost(motor, "{\"breakaway\":{\"enable\":true}}"));
  CHECK(r.code == 202);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].hasBreakaway);
  CHECK(sib::app().submitted[0].breakaway.enable);
  CHECK(sib::app().submitted[0].breakaway.stepPct == 10);
  CHECK(sib::app().submitted[0].breakaway.maxmA == 30);
  r = fakes::http::perform(apiPost(motor, "{\"breakaway\":{\"enable\":1}}"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("out_of_range", "breakaway"));
  const std::string learn = errorBody("out_of_range", "learnMovements 0 or 50..65534");
  r = fakes::http::perform(apiPost(motor, "{\"learnMovements\":\"100\"}"));
  CHECK(r.code == 400);
  CHECK(r.body == learn);
  r = fakes::http::perform(apiPost(motor, "{\"learnMovements\":65535}"));
  CHECK(r.code == 400);
  CHECK(r.body == learn);
  r = fakes::http::perform(apiPost(motor, "{\"learnMovements\":65534}"));
  CHECK(r.code == 202);
  REQUIRE(sib::app().submitted.size() == 2);
  CHECK(sib::app().submitted[1].hasLearnMovements);
  CHECK(sib::app().submitted[1].learnMovements == 65534);
}

// ---------------------------------------------------------------- hostname

TEST_CASE("web guard: a station name of 20 characters is the whole hostname") {
  glue::begin();
  vdm::copyString(sib::storage().active.station, sizeof sib::storage().active.station,
                  "Abcdefghijklmnopqrst");
  start();
  Request full = fakes::http::get("/api/status");
  full.host = "abcdefghijklmnopqrst.local";
  CHECK(fakes::http::perform(full).code == 200);
  Request cut = fakes::http::get("/api/status");
  cut.host = "abcdefghijklmnopqrs.local";
  CHECK(fakes::http::perform(cut).code == 403);
}
