// Tests of the idle release of src/web_server.cpp: 30 s after the last callback the working set
// and the response slot buffers go back to the heap (web::service(), called by the app task), the
// next request allocates them again, nothing in flight loses a buffer, and a request that gets no
// memory is answered 503. web::service() runs as the app task, the request driver as AsyncTCP.
#include <string>

#include "glue_test.h"
#include "logger.h"
#include "web_server.h"

namespace {

using fakes::http::Exchange;
using fakes::http::Request;
using fakes::http::Response;

// The working set: body, snapshot, config, patch, JSON document, valve/temp/volt views, events,
// images, files, health snapshot, status snapshot, health text, guard detail.
constexpr size_t kWorkParts = 15;
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

Request apiUpload(const std::string& url, const std::string& name, const std::string& data) {
  Request r = fakes::http::upload(url, name, data);
  r.header("X-VdMot", "1");
  return r;
}

Response get(const std::string& url) { return fakes::http::perform(fakes::http::get(url)); }

bool isStatus(const Response& r) {
  return r.code == 200 && r.body.rfind("{\"station\":\"Boiler\",", 0) == 0;
}

// web::service() as the app task calls it, with the time `ms`.
void serviceAt(uint32_t ms) {
  fakes::rtos().current = reinterpret_cast<TaskHandle_t>(0xA11);
  web::service(ms);
  fakes::rtos().current = nullptr;
}

void service() { serviceAt(static_cast<uint32_t>(fakes::nowMs())); }

// The app task sees the last callback now and looks again kIdleReleaseMs later.
void idleRelease() {
  service();
  fakes::advanceMs(web::kIdleReleaseMs);
  service();
}

size_t allocations() { return fakes::heap().allocated.size(); }

// The next new (std::nothrow) calls: `ok` of them succeed, then one fails.
void failAfter(size_t ok) {
  fakes::heap().next.assign(ok, true);
  fakes::heap().next.push_back(false);
}

}  // namespace

// ---------------------------------------------------------------- release and allocation

TEST_CASE("web idle: 30 s after the last request the buffers go back to the heap") {
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
  service();  // sees the request
  fakes::advanceMs(web::kIdleReleaseMs - 1);
  service();
  CHECK(isStatus(get("/api/status")));
  CHECK(allocations() == kWorkParts + 1);  // kept
  service();
  fakes::advanceMs(web::kIdleReleaseMs);
  service();  // released
  CHECK(isStatus(get("/api/status")));  // the config is loaded again
  CHECK(allocations() == 2 * (kWorkParts + 1));
}

TEST_CASE("web idle: every callback starts the 30 s again") {
  glue::begin();
  start();
  CHECK(isStatus(get("/api/status")));
  service();
  fakes::advanceMs(20000);
  CHECK(get("/api/valves").code == 200);
  fakes::advanceMs(10000);
  service();  // 30 s after the first request: sees the second one
  fakes::advanceMs(web::kIdleReleaseMs - 1);
  service();
  CHECK(isStatus(get("/api/status")));
  CHECK(allocations() == kWorkParts + 1);
}

TEST_CASE("web idle: a response being sent keeps its slot, the release follows its disconnect") {
  glue::begin();
  start();
  Exchange a(fakes::http::get("/api/status"));  // answered, not yet transmitted
  service();
  fakes::advanceMs(web::kIdleReleaseMs + 1000);
  service();
  const Response& r = a.finish();  // the library reads the slot now
  CHECK(isStatus(r));
  service();  // sees the disconnect
  fakes::advanceMs(web::kIdleReleaseMs - 1);
  service();
  CHECK(isStatus(get("/api/status")));
  CHECK(allocations() == kWorkParts + 1);
  idleRelease();
  CHECK(isStatus(get("/api/status")));
  CHECK(allocations() == 2 * (kWorkParts + 1));
}

TEST_CASE("web idle: a body being collected keeps the working set") {
  glue::begin();
  start();
  Exchange a(apiPost(kTarget1, "{\"target\":50}"));
  a.sendBody(5);
  idleRelease();
  fakes::advanceMs(1000);
  service();
  const Response& r = a.finish();
  CHECK(r.code == 202);
  CHECK(r.body == "{\"valve\":1,\"target\":50}");
  CHECK(allocations() == kWorkParts);
}

TEST_CASE("web idle: an upload keeps the working set") {
  glue::begin();
  start();
  sib::storage().uploadEndInfo.size = 3000;
  Exchange u(apiUpload("/api/stm/images", "fw.bin", std::string(3000, 'x')));
  u.sendBody(1000);
  REQUIRE(sib::storage().uploadBegins.size() == 1);
  idleRelease();
  fakes::advanceMs(1000);
  service();
  CHECK(u.finish().code == 201);
  CHECK(sib::storage().uploadData == std::string(3000, 'x'));
  CHECK(allocations() == kWorkParts);
}

TEST_CASE("web idle: a log download keeps the working set") {
  glue::begin();
  fakes::fs().mounted = true;
  fakes::fs().put(logger::kLogFile, "#1 line\n");
  start();
  Exchange l(fakes::http::get("/api/log"));
  idleRelease();
  fakes::advanceMs(1000);
  service();
  CHECK(isStatus(get("/api/status")));
  CHECK(allocations() == kWorkParts + 1);  // the slot is new, the working set is not
  CHECK(l.finish().body == "#1 line\n");
}

TEST_CASE("web idle: service() while a request runs frees nothing and does not wait") {
  glue::begin();
  start();
  CHECK(isStatus(get("/api/status")));
  int calls = 0;
  uint64_t before = 0;
  uint64_t after = 0;
  sib::app().onNowMs = [&] {  // inside the valve handler, after its views are built
    if (++calls != 1) return;
    before = fakes::nowMs();
    const uint32_t now = static_cast<uint32_t>(before);
    serviceAt(now);
    serviceAt(now + web::kIdleReleaseMs + 1000);
    after = fakes::nowMs();
  };
  const Response r = get("/api/valves");
  sib::app().onNowMs = nullptr;
  CHECK(calls >= 1);
  CHECK(r.code == 200);
  CHECK(r.body.find("{\"idx\":12,") != std::string::npos);  // the views are still there
  CHECK(after == before);
  CHECK(allocations() == kWorkParts + 1);
}

TEST_CASE("web idle: service() before the server started does nothing") {
  glue::begin();
  serviceAt(web::kIdleReleaseMs + 5000);
  CHECK(fakes::rtos().violations == 0);
  start();
  CHECK(isStatus(get("/api/status")));
}

// ---------------------------------------------------------------- no memory

TEST_CASE("web idle: without memory after a release the request is answered 503") {
  glue::begin();
  start();
  CHECK(isStatus(get("/api/status")));
  idleRelease();
  fakes::heap().failAll = true;
  const Response r = get("/api/status");
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "out of memory"));
  fakes::heap().failAll = false;
  CHECK(isStatus(get("/api/status")));
}

TEST_CASE("web idle: each part of the working set that cannot be allocated answers 503") {
  glue::begin();
  start();
  for (size_t k = 0; k < kWorkParts; ++k) {
    CAPTURE(k);
    // the guard tries every part at the headers, then the missing one again
    failAfter(k);
    for (size_t i = k + 1; i < kWorkParts; ++i) fakes::heap().next.push_back(true);
    fakes::heap().next.push_back(false);
    const size_t before = allocations();
    Response r = get("/api/status");
    CHECK(r.code == 503);
    CHECK(r.body == errorBody("busy", "out of memory"));
    CHECK(allocations() == before + kWorkParts - 1);
    r = get("/api/status");  // the missing part and a slot
    CHECK(isStatus(r));
    CHECK(allocations() == before + kWorkParts + 1);
    idleRelease();
  }
}

TEST_CASE("web idle: slot buffers that cannot be allocated") {
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

TEST_CASE("web idle: a body after a release without memory is refused, the buffer stays free") {
  glue::begin();
  start();
  Request post = apiPost(kTarget1, "{\"target\":50}");
  post.segment = 5;
  Exchange a(post);  // the guard allocates at the headers
  idleRelease();     // the request holds nothing yet
  fakes::heap().failAll = true;
  a.sendBody(5);
  fakes::heap().failAll = false;
  const Response& r = a.finish();
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "body"));
  CHECK(sib::app().submitted.empty());
  // still without memory when the body is in: the request handler answers
  Exchange b(post);
  idleRelease();
  fakes::heap().failAll = true;
  const Response& rb = b.finish();
  CHECK(rb.code == 503);
  CHECK(rb.body == errorBody("busy", "out of memory"));
  fakes::heap().failAll = false;
  CHECK(fakes::http::perform(apiPost(kTarget1, "{\"target\":7}")).code == 202);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].pos == 7);
}

TEST_CASE("web idle: an upload after a release without memory is refused, nothing stored") {
  glue::begin();
  start();
  Exchange u(apiUpload("/api/stm/images", "fw.bin", std::string(3000, 'x')));
  idleRelease();
  fakes::heap().failAll = true;
  u.sendBody(1460);  // the first piece of the file
  fakes::heap().failAll = false;
  const Response& r = u.finish();
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "upload"));
  CHECK(sib::storage().uploadBegins.empty());
  // the upload state is free
  sib::storage().uploadEndInfo.size = 3;
  CHECK(fakes::http::perform(apiUpload("/api/stm/images", "fw.bin", "abc")).code == 201);
}
