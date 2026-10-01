// Tests of the working set of src/web_server.cpp: the first request allocates the working set and a
// response slot buffer and both are kept (no per-request heap growth; the idle release of 2.1.0 was
// withdrawn in 2.1.1), a request that gets no memory is answered 503 and the next one allocates
// what is missing. The handlers share one scratch buffer, a JSON body is received into a response
// slot and a config patch takes its copy from the heap for the request.
#include <ArduinoJson.h>

#include <algorithm>
#include <string>

#include <vdm/config.h>

#include "glue_test.h"
#include "web_server.h"

namespace {

using fakes::http::Exchange;
using fakes::http::Request;
using fakes::http::Response;

// The working set: snapshot, config, JSON document, scratch, guard detail.
constexpr size_t kWorkParts = 5;
const char* const kTarget1 = "/api/valves/1/target";
const char* const kSave = "{\"calib\":{\"hour\":4}}";

std::string errorBody(const std::string& code, const std::string& detail) {
  return "{\"error\":\"" + code + "\",\"detail\":\"" + detail + "\"}";
}

// A station name the default config does not have: a status document shows that the config was
// loaded into the working set. The requests keep the host of the default name.
void start() {
  vdm::Config& c = sib::storage().active;
  c.valves[0].active = true;
  vdm::copyString(c.station, sizeof c.station, "Boiler");
  vdm::copyString(c.web.allowedHosts, sizeof c.web.allowedHosts, "vdmot.local");
  ++sib::storage().revision;
  web::begin();
}

Request apiPost(const std::string& url, const std::string& body) {
  Request r = fakes::http::post(url, body);
  r.header("X-VdMot", "1");
  return r;
}

Response get(const std::string& url) { return fakes::http::perform(fakes::http::get(url)); }

Response post(const std::string& url, const std::string& body) {
  return fakes::http::perform(apiPost(url, body));
}

bool isStatus(const Response& r) {
  return r.code == 200 && r.body.rfind("{\"station\":\"Boiler\",", 0) == 0;
}

size_t allocations() { return fakes::heap().allocated.size(); }

size_t allocationsOf(size_t size) {
  const std::vector<size_t>& a = fakes::heap().allocated;
  return static_cast<size_t>(std::count(a.begin(), a.end(), size));
}

// The next new (std::nothrow) calls: `ok` of them succeed, then one fails.
void failAfter(size_t ok) {
  fakes::heap().next.assign(ok, true);
  fakes::heap().next.push_back(false);
}

}  // namespace

// ---------------------------------------------------------------- allocation

TEST_CASE("web working set: the first request allocates it and a slot, both are kept") {
  glue::begin();
  start();
  CHECK(allocations() == 0);  // begin() allocates nothing
  CHECK(isStatus(get("/api/status")));
  REQUIRE(allocations() == kWorkParts + 1);  // the working set and one slot
  for (size_t i = 0; i < kWorkParts; ++i) {
    CAPTURE(i);
    CHECK(fakes::heap().allocated[i] < web::kResponseSlotSize);  // none larger than a slot
  }
  CHECK(fakes::heap().allocated.back() == web::kResponseSlotSize);
  fakes::advanceMs(86400000);  // a day without a web client
  CHECK(isStatus(get("/api/status")));
  // every handler builds in the one scratch buffer: nothing else is allocated
  for (const char* url : {"/api/valves", "/api/sensors", "/api/events", "/api/health",
                          "/api/stm/images", "/api/files", "/valves", "/temps", "/volts"}) {
    CAPTURE(url);
    CHECK(get(url).code == 200);
  }
  CHECK(isStatus(get("/api/status")));
  CHECK(allocations() == kWorkParts + 1);
}

TEST_CASE("web working set: two responses in flight get both slots, later ones reuse them") {
  glue::begin();
  start();
  Exchange a(fakes::http::get("/api/status"));  // answered, not yet transmitted
  CHECK(get("/api/valves").code == 200);         // the second slot
  CHECK(allocations() == kWorkParts + 2);
  CHECK(isStatus(a.finish()));
  CHECK(isStatus(get("/api/status")));
  CHECK(get("/api/valves").code == 200);
  CHECK(allocations() == kWorkParts + 2);
}

// ---------------------------------------------------------------- no memory

TEST_CASE("web working set: without memory the first request is answered 503, the next allocates") {
  glue::begin();
  start();
  fakes::heap().failAll = true;
  const Response r = get("/api/status");
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "out of memory"));
  fakes::heap().failAll = false;
  CHECK(isStatus(get("/api/status")));
  CHECK(allocations() == kWorkParts + 1);
}

TEST_CASE("web working set: without memory the guard answers a body 503 and never collects it") {
  glue::begin();
  start();
  fakes::heap().failAll = true;  // no working set, no slot either
  const Response r = post(kTarget1, "{\"target\":50}");
  fakes::heap().failAll = false;
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "out of memory"));
  CHECK(sib::app().submitted.empty());
  CHECK(post(kTarget1, "{\"target\":7}").code == 202);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].pos == 7);
}

// Boot b of the case: part b cannot be allocated in the first request. The guard tries every part
// at the headers, then the missing one again, and answers 503; the next request allocates the
// missing part and a slot. A complete working set is kept, so every part needs a boot of its own.
TEST_CASE("web working set: each of the parts that cannot be allocated answers 503") {
  glue::begin();
  start();
  const size_t k = testkit::boot();
  CAPTURE(k);
  failAfter(k);
  for (size_t i = k + 1; i < kWorkParts; ++i) fakes::heap().next.push_back(true);
  fakes::heap().next.push_back(false);
  Response r = get("/api/status");
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "out of memory"));
  CHECK(allocations() == kWorkParts - 1);
  r = get("/api/status");
  CHECK(isStatus(r));
  CHECK(allocations() == kWorkParts + 1);
  if (k + 1 < kWorkParts) testkit::reboot(testkit::Reset::Software);
}

TEST_CASE("web working set: a body refused for memory is never collected, also when memory returns") {
  glue::begin();
  start();
  // the first part fails at the headers; it and a slot would be there for the request itself
  failAfter(0);
  for (size_t i = 1; i < kWorkParts + 2; ++i) fakes::heap().next.push_back(true);
  const Response r = post(kTarget1, "{\"target\":50}");
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("retry", "state changed"));
  CHECK(sib::app().submitted.empty());
  CHECK(allocations() == kWorkParts);  // the working set, no slot: the body was skipped
}

TEST_CASE("web working set: slot buffers that cannot be allocated") {
  glue::begin();
  start();
  failAfter(kWorkParts);  // the first slot
  fakes::heap().next.push_back(false);  // the second
  Response r = get("/api/status");
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "response buffers in use"));
  failAfter(0);  // the first slot again: the second one answers
  CHECK(isStatus(get("/api/status")));
  CHECK(allocations() == kWorkParts + 1);
  CHECK(fakes::heap().allocated.back() == web::kResponseSlotSize);
}

// ---------------------------------------------------------------- bodies in response slots

TEST_CASE("web body: a JSON body is received into a slot, the answer of a save goes out of it") {
  glue::begin();
  start();
  Exchange a(fakes::http::get("/api/status"));  // the first slot
  const size_t configs = allocationsOf(sizeof(vdm::Config));
  const Response r = post("/api/config", kSave);  // the second slot: body, then the answer
  CHECK(r.code == 200);
  CHECK(r.body.find("\"station\":\"Boiler\"") != std::string::npos);
  CHECK(r.body.find(",\"restartRequired\":false,\"netTrial\":false}") != std::string::npos);
  REQUIRE(sib::storage().applied.size() == 1);
  CHECK(sib::storage().applied[0].calib.hour == 4);
  // both slots and, for this request, the patched copy
  CHECK(allocations() == kWorkParts + 3);
  CHECK(allocationsOf(sizeof(vdm::Config)) == configs + 1);
  // the end of the save gave back only its own slot: the next document goes to the second one
  Exchange v(fakes::http::get("/api/valves"));
  CHECK(fakes::http::perform(fakes::http::get("/api/status")).code == 503);
  CHECK(isStatus(a.finish()));
  CHECK(v.finish().body.rfind("{\"valves\":[", 0) == 0);
  // the slots are free again
  Exchange b(fakes::http::get("/api/status"));
  Exchange c(fakes::http::get("/api/status"));
  CHECK(isStatus(b.finish()));
  CHECK(isStatus(c.finish()));
  CHECK(allocations() == kWorkParts + 3);
}

TEST_CASE("web body: with both slots busy a body is refused 503 before anything runs") {
  glue::begin();
  start();
  Exchange a(fakes::http::get("/api/status"));
  Exchange b(fakes::http::get("/api/status"));
  Response r = post(kTarget1, "{\"target\":50}");
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "response buffers in use"));
  CHECK(sib::app().submitted.empty());
  CHECK(isStatus(a.finish()));
  r = post(kTarget1, "{\"target\":50}");
  CHECK(r.code == 202);
  CHECK(sib::app().submitted.size() == 1);
  CHECK(isStatus(b.finish()));
}

TEST_CASE("web body: a small answer gives the body's slot back with the request") {
  glue::begin();
  start();
  CHECK(post(kTarget1, "{\"target\":50}").code == 202);
  CHECK(post(kTarget1, "{\"target\":\"x\"}").code == 400);
  Exchange a(fakes::http::get("/api/status"));
  Exchange b(fakes::http::get("/api/status"));
  CHECK(isStatus(a.finish()));
  CHECK(isStatus(b.finish()));
}

TEST_CASE("web body: a client gone mid-body gives the slot back") {
  glue::begin();
  start();
  Exchange body(apiPost("/api/config", kSave));
  body.sendBody(4);
  Exchange a(fakes::http::get("/api/status"));  // the other slot
  Response r = get("/api/status");
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "response buffers in use"));
  body.disconnect();
  CHECK(isStatus(get("/api/status")));
  CHECK(sib::storage().applied.empty());
  CHECK(isStatus(a.finish()));
  // the next body is collected as usual
  CHECK(post("/api/config", kSave).code == 200);
}

TEST_CASE("web body: a GET with a body answers from the body's slot") {
  glue::begin();
  start();
  Exchange a(fakes::http::get("/api/status"));
  Request withBody = apiPost("/api/status", "{}");
  withBody.method = HTTP_GET;
  Exchange b(withBody);
  const Response& rb = b.finish();
  CHECK(isStatus(rb));
  CHECK(allocations() == kWorkParts + 2);  // a third slot was never needed
  CHECK(isStatus(a.finish()));
}

TEST_CASE("web config: without memory for the patched copy a save answers 503, nothing applied") {
  glue::begin();
  start();
  CHECK(isStatus(get("/api/status")));  // the working set and a slot
  fakes::heap().failAll = true;
  Response r = post("/api/config", kSave);
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "out of memory"));
  r = post("/api/config?dryRun=1", kSave);
  CHECK(r.code == 503);
  fakes::heap().failAll = false;
  CHECK(sib::storage().applied.empty());
  CHECK(sib::logger().withCode(vdm::EventCode::ConfigSaved).empty());
  r = post("/api/config", kSave);
  CHECK(r.code == 200);
  CHECK(sib::storage().applied.size() == 1);
}

// ---------------------------------------------------------------- working set

TEST_CASE("web working set: the JSON document holds the largest object of 512 bytes") {
  glue::begin();
  start();
  size_t fits = 0;
  while (JSON_OBJECT_SIZE(fits + 1) <= 512) ++fits;
  const auto object = [](size_t members) {
    std::string body = "{\"target\":5";
    for (size_t i = 1; i < members; ++i) body += ",\"k" + std::to_string(i) + "\":1";
    return body + "}";
  };
  // parsed: the extra members are refused one step later
  Response r = post(kTarget1, object(fits));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("out_of_range", "target 0..100"));
  r = post(kTarget1, object(fits + 1));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "NoMemory"));
}
