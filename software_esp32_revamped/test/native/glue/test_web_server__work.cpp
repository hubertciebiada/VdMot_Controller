// Tests of the working set of src/web_server.cpp: the first request allocates the working set and a
// response slot buffer and both are kept (no per-request heap growth; the idle release of 2.1.0 was
// withdrawn in 2.1.1), a request that gets no memory is answered 503 and the next one allocates
// what is missing.
#include <ArduinoJson.h>

#include <string>

#include "glue_test.h"
#include "web_server.h"

namespace {

using fakes::http::Exchange;
using fakes::http::Request;
using fakes::http::Response;

// The working set: body, snapshot, config, patch, JSON document, valve/temp/volt views, events,
// images, files, health snapshot, status snapshot, health text, guard detail, profile.
constexpr size_t kWorkParts = 16;
const char* const kTarget1 = "/api/valves/1/target";

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

bool isStatus(const Response& r) {
  return r.code == 200 && r.body.rfind("{\"station\":\"Boiler\",", 0) == 0;
}

size_t allocations() { return fakes::heap().allocated.size(); }

// The next new (std::nothrow) calls: `ok` of them succeed, then one fails.
void failAfter(size_t ok) {
  fakes::heap().next.assign(ok, true);
  fakes::heap().next.push_back(false);
}

// Boot b of the case: part first + b cannot be allocated in the first request. The guard tries
// every part at the headers, then the missing one again, and answers 503; the next request
// allocates the missing part and a slot. A complete working set is kept, so every part needs a
// boot of its own.
void failEachPart(size_t first, size_t last) {
  glue::begin();
  start();
  const size_t k = first + testkit::boot();
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
  if (k < last) testkit::reboot(testkit::Reset::Software);
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
    CHECK(fakes::heap().allocated[i] <= web::kMaxBodySize + 1);  // none larger than the body
  }
  CHECK(fakes::heap().allocated.back() == web::kResponseSlotSize);
  fakes::advanceMs(86400000);  // a day without a web client
  CHECK(isStatus(get("/api/status")));
  CHECK(get("/api/valves").code == 200);
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
  fakes::heap().failAll = true;  // no body buffer either
  const Response r = fakes::http::perform(apiPost(kTarget1, "{\"target\":50}"));
  fakes::heap().failAll = false;
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "out of memory"));
  CHECK(sib::app().submitted.empty());
  CHECK(fakes::http::perform(apiPost(kTarget1, "{\"target\":7}")).code == 202);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].pos == 7);
}

TEST_CASE("web working set: each of the parts 0-4 that cannot be allocated answers 503") {
  failEachPart(0, 4);
}

TEST_CASE("web working set: each of the parts 5-9 that cannot be allocated answers 503") {
  failEachPart(5, 9);
}

TEST_CASE("web working set: each of the parts 10-15 that cannot be allocated answers 503") {
  failEachPart(10, kWorkParts - 1);
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
  Response r = fakes::http::perform(apiPost(kTarget1, object(fits)));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("out_of_range", "target 0..100"));
  r = fakes::http::perform(apiPost(kTarget1, object(fits + 1)));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "NoMemory"));
}
