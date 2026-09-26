// Tests of src/web_server.cpp through the request driver: config save and dry run, the import
// report, health, the log download, static assets, the JSON body buffer shared by the requests,
// and uploads (failures per storage/ota result, several files, disconnects, concurrency).
#include <string>

#include <vdm/json_writer.h>
#include <vdm/stm_flasher.h>

#include "generated/web_assets.h"
#include "glue_test.h"
#include "logger.h"
#include "web_server.h"

namespace {

using fakes::http::Exchange;
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

Request apiPost(const std::string& url, const std::string& body) {
  Request r = fakes::http::post(url, body);
  r.header("X-VdMot", "1");
  return r;
}

Request apiUpload(const std::string& url, const std::string& name, const std::string& data,
                  const std::vector<std::pair<std::string, std::string>>& fields = {}) {
  Request r = fakes::http::upload(url, name, data, fields);
  r.header("X-VdMot", "1");
  return r;
}

Response get(const std::string& url) { return fakes::http::perform(fakes::http::get(url)); }

const char* kStmUpload = "/api/stm/images";
const char* kEspUpload = "/api/ota/esp";

}  // namespace

// ---------------------------------------------------------------- config

TEST_CASE("web config: GET is inline, no download") {
  glue::begin();
  start();
  const Response r = get("/api/config");
  CHECK(r.code == 200);
  CHECK(r.header("Content-Disposition").empty());
  CHECK(r.body.rfind("{\"schema\":", 0) == 0);
}

TEST_CASE("web config: a save needs a body, also after an earlier body") {
  glue::begin();
  start();
  Response r = fakes::http::perform(apiPost("/api/config", ""));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "JSON body required"));
  CHECK(fakes::http::perform(apiPost("/api/config", "{\"calib\":{\"hour\":4}}")).code == 200);
  r = fakes::http::perform(apiPost("/api/config", ""));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "JSON body required"));
  r = fakes::http::perform(apiPost("/api/config?dryRun=1", ""));
  CHECK(r.body == errorBody("bad_request", "JSON body required"));
  // a one-byte body is parsed (and malformed)
  r = fakes::http::perform(apiPost("/api/config", "{"));
  CHECK(r.code == 400);
  CHECK(r.body.rfind("{\"error\":\"invalid\",\"detail\":\"@", 0) == 0);
  CHECK(sib::storage().applied.size() == 1);
}

TEST_CASE("web config: a failed dry run holds no response slot") {
  glue::begin();
  start();
  Exchange a(fakes::http::get("/api/status"));
  Response r = fakes::http::perform(apiPost("/api/config?dryRun=1", "{\"calib\":{\"hour\":24}}"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("invalid", "calib.hour"));
  Exchange b(fakes::http::get("/api/status"));
  r = get("/api/status");
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("busy", "response buffers in use"));
  CHECK(a.finish().code == 200);
  CHECK(b.finish().code == 200);
}

TEST_CASE("web config: a save logs the new revision") {
  glue::begin();
  start();
  sib::storage().revision = 41;
  CHECK(fakes::http::perform(apiPost("/api/config", "{\"calib\":{\"hour\":4}}")).code == 200);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::ConfigSaved);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].valve == vdm::kNoValve);
  CHECK(ev[0].arg1 == 42);
  CHECK(ev[0].arg2 == 0);
  CHECK(std::string(ev[0].text) == "web");
}

// ---------------------------------------------------------------- import report

TEST_CASE("web import report: busy slots, an empty and an oversized file") {
  glue::begin();
  fakes::fs().mounted = true;
  fakes::fs().put(storage::kImportReportFile, "{\"imported\":1}");
  sib::storage().hasImportReport = true;
  start();
  {
    Exchange a(fakes::http::get("/api/status"));
    Exchange b(fakes::http::get("/api/status"));
    const Response r = get("/api/import-report");
    CHECK(r.code == 503);
    CHECK(r.body == errorBody("busy", "response buffers in use"));
    CHECK(fakes::fs().openHandles == 0);
  }
  fakes::fs().put(storage::kImportReportFile, "x");
  Response r = get("/api/import-report");
  CHECK(r.code == 200);
  CHECK(r.body == "x");
  fakes::fs().put(storage::kImportReportFile, "");
  r = get("/api/import-report");
  CHECK(r.code == 500);
  CHECK(r.body == errorBody("io_error", storage::kImportReportFile));
  fakes::fs().put(storage::kImportReportFile, std::string(web::kResponseSlotSize, 'a'));
  r = get("/api/import-report");
  CHECK(r.code == 200);
  CHECK(r.body.size() == web::kResponseSlotSize);
  fakes::fs().put(storage::kImportReportFile, std::string(web::kResponseSlotSize + 1, 'a'));
  r = get("/api/import-report");
  CHECK(r.code == 500);
  CHECK(r.body == errorBody("io_error", storage::kImportReportFile));
  CHECK(fakes::fs().openHandles == 0);
  // the slots are free again
  Exchange a(fakes::http::get("/api/status"));
  Exchange b(fakes::http::get("/api/status"));
  CHECK(a.finish().code == 200);
  CHECK(b.finish().code == 200);
}

// ---------------------------------------------------------------- health

TEST_CASE("web health: a document too large and a response out of memory answer 500") {
  glue::begin();
  static const std::string version(1100, 'v');
  sib::app().health.version = version.c_str();
  start();
  Response r = get("/api/health");
  CHECK(r.code == 500);
  CHECK(r.body == errorBody("internal", "health"));
  sib::app().health.version = "2.1.0";
  fakes::http::server().failNextResponse = true;
  r = get("/api/health");
  CHECK(r.code == 500);
  CHECK(r.body.empty());
  CHECK(get("/api/health").code == 200);
}

// ---------------------------------------------------------------- log download

TEST_CASE("web log: the previous file first, then the current one") {
  glue::begin();
  fakes::fs().mounted = true;
  fakes::fs().put(logger::kLogFileOld, "#1 old\n");
  fakes::fs().put(logger::kLogFile, "#2 new\n");
  start();
  Response r = get("/api/log");
  CHECK(r.code == 200);
  CHECK(r.chunked);
  CHECK(r.contentType == "text/plain; charset=utf-8");
  CHECK(r.header("Content-Disposition") == "attachment; filename=\"vdmot-events.log\"");
  CHECK(r.body == "#1 old\n#2 new\n");
  // one-byte files are not lost
  fakes::fs().put(logger::kLogFileOld, "A");
  fakes::fs().put(logger::kLogFile, "B");
  r = get("/api/log");
  CHECK(r.code == 200);
  CHECK(r.body == "AB");
  // only the previous file
  fakes::fs().nodes.erase(logger::kLogFile);
  CHECK(get("/api/log").body == "A");
}

TEST_CASE("web log: one download at a time, none without a file system") {
  glue::begin();
  fakes::fs().mounted = true;
  fakes::fs().put(logger::kLogFile, "#1 line\n");
  start();
  {
    Exchange a(fakes::http::get("/api/log"));
    const Response r = get("/api/log");
    CHECK(r.code == 409);
    CHECK(r.body == errorBody("busy", "log download running"));
    CHECK(a.finish().body == "#1 line\n");
  }
  // the finished download frees the stream
  Response r = get("/api/log");
  CHECK(r.code == 200);
  CHECK(r.body == "#1 line\n");
  // a client gone before the transfer frees it too
  {
    Exchange a(fakes::http::get("/api/log"));
    a.disconnect();
  }
  CHECK(get("/api/log").code == 200);
  // out of memory: 500, and the stream is free again
  fakes::http::server().failNextResponse = true;
  r = get("/api/log");
  CHECK(r.code == 500);
  CHECK(r.body == errorBody("internal", "log"));
  CHECK(get("/api/log").body == "#1 line\n");
  sib::storage().fsReady = false;
  r = get("/api/log");
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("unavailable", "no file system"));
}

// ---------------------------------------------------------------- static assets

TEST_CASE("web static: every asset is served, out of memory answers 500") {
  glue::begin();
  start();
  REQUIRE(web_assets::kAssetCount > 0);
  for (size_t i = 0; i < web_assets::kAssetCount; ++i) {
    const web_assets::Asset& a = web_assets::kAssets[i];
    CAPTURE(a.path);
    const Response r = get(a.path);
    CHECK(r.code == 200);
    CHECK(r.header("ETag") == a.etag);
  }
  fakes::http::server().failNextResponse = true;
  const Response r = get("/");
  CHECK(r.code == 500);
  CHECK(r.body.empty());
}

// ---------------------------------------------------------------- JSON body buffer

TEST_CASE("web body: a second body while one arrives is refused and never mixed in") {
  glue::begin();
  start();
  Exchange a(apiPost("/api/valves/1/target", "{\"target\":50}"));
  a.sendBody(5);
  Response r = fakes::http::perform(apiPost("/api/valves/1/target", "{"));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("busy", "body"));
  Request seg = apiPost("/api/valves/1/target", "{\"target\":7}");
  seg.segment = 3;
  r = fakes::http::perform(seg);
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("busy", "body"));
  const Response& ra = a.finish();
  CHECK(ra.code == 202);
  CHECK(ra.body == "{\"valve\":1,\"target\":50}");
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].pos == 50);
  // the buffer is free again
  r = fakes::http::perform(apiPost("/api/valves/1/target", "{\"target\":8}"));
  CHECK(r.code == 202);
}

TEST_CASE("web body: a client gone mid-body frees the buffer") {
  glue::begin();
  start();
  {
    Exchange a(apiPost("/api/valves/1/target", "{\"target\":50}"));
    a.sendBody(4);
    a.disconnect();
  }
  const Response r = fakes::http::perform(apiPost("/api/valves/1/target", "{\"target\":9}"));
  CHECK(r.code == 202);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].pos == 9);
}

// ---------------------------------------------------------------- uploads

TEST_CASE("web upload: each storage result of the begin has its status") {
  glue::begin();
  start();
  const std::pair<storage::ImageResult, int> cases[] = {
      {storage::ImageResult::BadName, 400}, {storage::ImageResult::Empty, 400},
      {storage::ImageResult::TooLarge, 413}, {storage::ImageResult::NoSpace, 507},
      {storage::ImageResult::TooMany, 507},  {storage::ImageResult::Busy, 409},
      {storage::ImageResult::Io, 500},       {storage::ImageResult::NotFound, 500}};
  for (const auto& c : cases) {
    CAPTURE(static_cast<int>(c.first));
    sib::storage().uploadBeginResult = c.first;
    const Response r = fakes::http::perform(apiUpload(kStmUpload, "fw.bin", "abc"));
    CHECK(r.code == c.second);
    CHECK(r.body == errorBody("upload_failed", storage::imageResultName(c.first)));
  }
  CHECK(sib::storage().uploadData.empty());
  CHECK(sib::ota().uploadBegins.empty());
  CHECK(sib::ota().uploadEnds.empty());
}

TEST_CASE("web upload: a failed write or end of an STM image") {
  glue::begin();
  start();
  sib::storage().uploadWriteResult = storage::ImageResult::NoSpace;
  Response r = fakes::http::perform(apiUpload(kStmUpload, "fw.bin", "abc"));
  CHECK(r.code == 507);
  CHECK(r.body == errorBody("upload_failed", "no_space"));
  CHECK(sib::storage().uploadEnds == 0);
  sib::storage().uploadWriteResult = storage::ImageResult::Ok;
  sib::storage().uploadEndResult = storage::ImageResult::Io;
  r = fakes::http::perform(apiUpload(kStmUpload, "fw.bin", "abc"));
  CHECK(r.code == 500);
  CHECK(r.body == errorBody("upload_failed", storage::imageResultName(storage::ImageResult::Io)));
  CHECK(sib::storage().uploadEnds == 1);
  CHECK(sib::storage().uploadAborts == 0);
}

TEST_CASE("web upload: ESP image begin, write and end failures") {
  glue::begin();
  start();
  sib::ota().uploadBeginResult = false;
  sib::ota().uploadError = "busy";
  Response r = fakes::http::perform(apiUpload(kEspUpload, "fw.bin", "img"));
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("upload_failed", "busy"));
  sib::ota().uploadError = "bad_md5";
  r = fakes::http::perform(apiUpload(kEspUpload, "fw.bin", "img"));
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("upload_failed", "bad_md5"));
  CHECK(sib::ota().uploadEnds.empty());
  sib::ota().uploadBeginResult = true;
  sib::ota().uploadWriteResult = false;
  sib::ota().uploadError = "write";
  r = fakes::http::perform(apiUpload(kEspUpload, "fw.bin", "img"));
  CHECK(r.code == 500);
  CHECK(r.body == errorBody("upload_failed", "write"));
  CHECK(sib::ota().uploadEnds.empty());
  sib::ota().uploadWriteResult = true;
  sib::ota().uploadEndResult = false;
  sib::ota().uploadError = "verify";
  r = fakes::http::perform(apiUpload(kEspUpload, "fw.bin", "img"));
  CHECK(r.code == 500);
  CHECK(r.body == errorBody("upload_failed", "verify"));
  CHECK(sib::ota().uploadEnds == std::vector<bool>{true});
  CHECK(sib::storage().uploadBegins.empty());
}

TEST_CASE("web upload: the MD5 from a form field or a header") {
  glue::begin();
  start();
  const std::string md5 = "0123456789abcdef0123456789abcdef";
  CHECK(fakes::http::perform(apiUpload(kEspUpload, "fw.bin", "img", {{"md5", md5}})).code == 200);
  Request h = apiUpload(kEspUpload, "fw.bin", "img");
  h.header("X-Update-MD5", "fedcba9876543210fedcba9876543210");
  CHECK(fakes::http::perform(h).code == 200);
  CHECK(fakes::http::perform(apiUpload(kEspUpload, "fw.bin", "img")).code == 200);
  REQUIRE(sib::ota().uploadBegins.size() == 3);
  CHECK(sib::ota().uploadBegins[0].second == md5);
  CHECK(sib::ota().uploadBegins[1].second == "fedcba9876543210fedcba9876543210");
  CHECK(sib::ota().uploadBegins[2].second.empty());
}

TEST_CASE("web upload: one file per request") {
  glue::begin();
  start();
  Request two = apiUpload(kStmUpload, "fw.bin", "abc");
  two.parts.push_back({"file2", "fw2.bin", "def", "application/octet-stream"});
  Response r = fakes::http::perform(two);
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("upload_failed", "one file per request"));
  REQUIRE(sib::storage().uploadBegins.size() == 1);
  CHECK(sib::storage().uploadData == "abc");
  // a file whose begin failed stays the answer, a second file does not replace it
  sib::storage().uploadBeginResult = storage::ImageResult::TooLarge;
  r = fakes::http::perform(two);
  CHECK(r.code == 413);
  CHECK(r.body == errorBody("upload_failed", "too_large"));
  CHECK(sib::storage().uploadBegins.size() == 2);
}

TEST_CASE("web upload: a request without a file part") {
  glue::begin();
  start();
  Request none = apiUpload(kStmUpload, "fw.bin", "abc");
  none.parts = {{"md5", "", "x", ""}};
  const Response r = fakes::http::perform(none);
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("bad_request", "no file in request"));
  CHECK(sib::storage().uploadBegins.empty());
}

TEST_CASE("web upload: a file cut short is incomplete and aborted") {
  glue::begin();
  start();
  Request cut = apiUpload(kStmUpload, "fw.bin", std::string(100, 'x'));
  cut.contentLength = cut.wireBody().size() - 10;
  cut.segment = 50;  // a segment ends inside the file data: its first piece is delivered
  Response r = fakes::http::perform(cut);
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("upload_failed", "incomplete file"));
  CHECK(sib::storage().uploadAborts == 1);
  CHECK(sib::storage().uploadEnds == 0);
  Request esp = apiUpload(kEspUpload, "fw.bin", std::string(100, 'y'));
  esp.contentLength = esp.wireBody().size() - 10;
  esp.segment = 50;
  r = fakes::http::perform(esp);
  CHECK(r.code == 400);
  CHECK(r.body == errorBody("upload_failed", "incomplete file"));
  CHECK(sib::ota().uploadEnds == std::vector<bool>{false});
}

TEST_CASE("web upload: a client gone mid-upload aborts it") {
  glue::begin();
  start();
  {
    Exchange e(apiUpload(kStmUpload, "fw.bin", std::string(3000, 'x')));
    e.sendBody(1000);
    CHECK(sib::storage().imageUploadActive);
    e.disconnect();
  }
  CHECK(sib::storage().uploadAborts == 1);
  CHECK_FALSE(sib::storage().imageUploadActive);
  {
    Exchange e(apiUpload(kEspUpload, "fw.bin", std::string(3000, 'y')));
    e.sendBody(1000);
    e.disconnect();
  }
  CHECK(sib::ota().uploadEnds == std::vector<bool>{false});
  CHECK_FALSE(sib::ota().uploadActive);
  // the state is free for the next upload
  CHECK(fakes::http::perform(apiUpload(kStmUpload, "fw.bin", "abc")).code == 201);
}

TEST_CASE("web upload: a second upload while one runs is refused and never mixed in") {
  glue::begin();
  start();
  const std::string data(3000, 'a');
  Exchange a(apiUpload(kStmUpload, "fw.bin", data));
  Exchange b(apiUpload(kStmUpload, "other.bin", std::string(2000, 'b')));
  a.sendBody(1000);
  const Response& rb = b.finish();
  CHECK(rb.code == 409);
  CHECK(rb.body == errorBody("busy", "upload"));
  const Response& ra = a.finish();
  CHECK(ra.code == 201);
  REQUIRE(sib::storage().uploadBegins.size() == 1);
  CHECK(sib::storage().uploadBegins[0].name == "fw.bin");
  CHECK(sib::storage().uploadData == data);
}

TEST_CASE("web upload: a finished upload's disconnect leaves the next upload alone") {
  glue::begin();
  start();
  Exchange a(apiUpload(kStmUpload, "fw.bin", "abc"));
  a.sendBody(SIZE_MAX);
  CHECK(a.response().code == 201);
  Exchange b(apiUpload(kStmUpload, "next.bin", std::string(3000, 'n')));
  b.sendBody(1000);
  a.disconnect();
  CHECK(sib::storage().uploadAborts == 0);
  const Response& rb = b.finish();
  CHECK(rb.code == 201);
  CHECK(sib::storage().uploadData == std::string(3000, 'n'));
}

namespace {

// The upload answer of a scanned image entry with every member set.
std::string imageJson(const storage::ImageEntry& e) {
  char crc[12];
  snprintf(crc, sizeof crc, "0x%08lx", static_cast<unsigned long>(e.crc));
  return std::string("{\"name\":\"") + e.name + "\",\"size\":" + std::to_string(e.size) +
         ",\"crc32\":\"" + crc + "\",\"version\":\"" + e.version + "\",\"check\":\"" +
         vdm::flashErrorName(e.check) + "\",\"hw\":\"" + e.hwTag + "\"}";
}

// An upload whose answer would be `length` bytes long.
Response uploadDescribed(size_t length, std::string& full) {
  storage::ImageEntry& e = sib::storage().uploadEndInfo;
  e = storage::ImageEntry{};
  vdm::copyString(e.name, sizeof e.name, std::string(31, 'n').c_str());
  e.size = 4000000000u;
  e.crc = 0x1234abcd;
  e.scanned = true;
  e.check = vdm::FlashError::ImageNoHandshake;
  vdm::copyString(e.hwTag, sizeof e.hwTag, "C2");
  const size_t base = imageJson(e).size();
  REQUIRE(length > base);
  REQUIRE(length - base < sizeof e.version);
  vdm::copyString(e.version, sizeof e.version, std::string(length - base, 'v').c_str());
  full = imageJson(e);
  REQUIRE(full.size() == length);
  return fakes::http::perform(apiUpload(kStmUpload, "fw.bin", "abc"));
}

}  // namespace

TEST_CASE("web upload: the image answer fits 159 bytes, a longer one is {}") {
  glue::begin();
  start();
  std::string full;
  Response r = uploadDescribed(159, full);
  CHECK(r.code == 201);
  CHECK(r.body == full);
  r = uploadDescribed(160, full);
  CHECK(r.code == 201);
  CHECK(r.body == "{}");
}
