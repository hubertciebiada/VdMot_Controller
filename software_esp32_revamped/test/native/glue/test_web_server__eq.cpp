// Tests of src/web_server.cpp for what real clients and the real heap can do: bytes pipelined
// after a body, multipart framing the library parses only for a handler that is not trivial, a
// login lock that ends between the guard and the login check, and a request marked busy for one
// file that owns the upload with the next, also when the heap hands its address out again.
#include <memory>
#include <string>

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

Request apiPost(const std::string& url, const std::string& body,
                const std::string& contentType = "application/json") {
  Request r = fakes::http::post(url, body, contentType);
  r.header("X-VdMot", "1");
  return r;
}

Request apiUpload(const std::string& url, const std::string& name, const std::string& data) {
  Request r = fakes::http::upload(url, name, data);
  r.header("X-VdMot", "1");
  return r;
}

void setLogin(const char* user, const char* password) {
  vdm::Config& c = sib::storage().active;
  vdm::copyString(c.web.user, sizeof c.web.user, user);
  vdm::copyString(c.web.password, sizeof c.web.password, password);
  ++sib::storage().revision;
}

Request withAuth(Request r, bool right = true) {
  // admin:secret12 / admin:wrong
  r.header("Authorization", right ? "Basic YWRtaW46c2VjcmV0MTI=" : "Basic YWRtaW46d3Jvbmc=");
  return r;
}

const char* const kTarget1 = "/api/valves/1/target";

// A sends two files: the first while B uploads (A is marked busy), the second after B (A owns the
// upload). Returns A, whose body is not complete yet.
std::unique_ptr<Exchange> markedThenOwner() {
  fakes::http::server().recycleRequests = true;
  Request a = apiUpload("/api/stm/images", "a.bin", std::string(100, 'a'));
  a.parts.push_back({"file2", "b.bin", std::string(100, 'b'), "application/octet-stream"});
  const std::string wire = a.wireBody();
  const size_t secondName = wire.find("b.bin");
  const size_t secondData = wire.find(std::string(100, 'b'));
  REQUIRE(secondData != std::string::npos);
  std::unique_ptr<Exchange> ea(new Exchange(a));
  Exchange eb(apiUpload("/api/ota/esp", "fw.bin", std::string(3000, 'i')));
  eb.sendBody(2000);
  REQUIRE(sib::ota().uploadBegins.size() == 1);
  ea->sendBody(secondName);  // the first file of A while B owns the upload
  CHECK(eb.finish().code == 200);
  ea->sendBody(secondData + 10 - secondName);  // the second file of A: A owns the upload
  REQUIRE(sib::storage().uploadBegins.size() == 1);
  CHECK(sib::storage().uploadBegins[0].name == "b.bin");
  return ea;
}

// markedThenOwner(), then A goes away before the end of its body: the upload is aborted. Returns
// the address of A, where the next request is built.
AsyncWebServerRequest* markedUploadGone() {
  std::unique_ptr<Exchange> ea = markedThenOwner();
  AsyncWebServerRequest* const address = ea->request();
  ea->disconnect();
  CHECK(sib::storage().uploadAborts == 1);
  return address;
}

}  // namespace

// ---------------------------------------------------------------- body

TEST_CASE("web body: bytes pipelined after an 8192-byte body never go past the body buffer") {
  glue::begin();
  start();
  // the last segment carries the rest of the body and the start of the next request
  Request r = apiPost("/api/config", std::string(8192, ' '));
  r.pipelined = "GET /api/status HTTP/1.1\r\n";
  const Response res = fakes::http::perform(r);
  CHECK(res.sends == 0);  // the library never ends the request: it read past Content-Length
  // the client is gone: the buffer takes the next body
  CHECK(fakes::http::perform(apiPost(kTarget1, "{\"target\":5}")).code == 202);
  REQUIRE(sib::app().submitted.size() == 1);
  CHECK(sib::app().submitted[0].pos == 5);
}

TEST_CASE("web body: a mark left by an upload that went away does not answer a bodyless request") {
  glue::begin();
  start();
  AsyncWebServerRequest* const addressOfA = markedUploadGone();
  // C, without a body, is built where A was
  Exchange ec(fakes::http::get("/api/status"));
  REQUIRE(ec.request() == addressOfA);
  const Response& r = ec.finish();
  CHECK(r.code == 200);
  CHECK(r.body.rfind("{\"station\":\"VdMot\",", 0) == 0);
}

TEST_CASE("web body: a mark left by an upload that went away does not answer form fields") {
  glue::begin();
  start();
  AsyncWebServerRequest* const addressOfA = markedUploadGone();
  // C, built where A was, sends form fields: the library parses them, the handler gets no body
  Request c = fakes::http::get("/api/status");
  c.contentType = "application/x-www-form-urlencoded";
  c.body = "a=1";
  Exchange ec(c);
  REQUIRE(ec.request() == addressOfA);
  const Response& r = ec.finish();
  CHECK(r.code == 200);
  CHECK(r.body.rfind("{\"station\":\"VdMot\",", 0) == 0);
}

// ---------------------------------------------------------------- upload

TEST_CASE("web upload: a mark left by an upload that went away does not answer the next upload") {
  glue::begin();
  start();
  storage::ImageEntry& e = sib::storage().uploadEndInfo;
  vdm::copyString(e.name, sizeof e.name, "c");
  e.size = 100;
  AsyncWebServerRequest* const addressOfA = markedUploadGone();
  // U, built where A was, uploads an image
  Exchange eu(apiUpload("/api/stm/images", "c.bin", std::string(100, 'c')));
  REQUIRE(eu.request() == addressOfA);
  const Response& r = eu.finish();
  CHECK(r.code == 201);
  CHECK(r.body ==
        "{\"name\":\"c\",\"size\":100,\"crc32\":\"0x00000000\",\"version\":null,\"check\":null,"
        "\"hw\":null}");
  REQUIRE(sib::storage().uploadBegins.size() == 2);
  CHECK(sib::storage().uploadBegins[1].name == "c.bin");
  CHECK(sib::storage().uploadData == std::string(100, 'c'));
  CHECK(sib::storage().uploadEnds == 1);
}

TEST_CASE("web upload: a request that owns the upload after a refused file is answered for it") {
  glue::begin();
  start();
  storage::ImageEntry& e = sib::storage().uploadEndInfo;
  vdm::copyString(e.name, sizeof e.name, "b");
  e.size = 100;
  const Response r = markedThenOwner()->finish();
  // the file it stored decides, not the file refused while B uploaded
  CHECK(r.code == 201);
  CHECK(r.body ==
        "{\"name\":\"b\",\"size\":100,\"crc32\":\"0x00000000\",\"version\":null,\"check\":null,"
        "\"hw\":null}");
  CHECK(sib::storage().uploadData == std::string(100, 'b'));
  CHECK(sib::storage().uploadEnds == 1);
  CHECK(sib::storage().uploadAborts == 0);
}

// ---------------------------------------------------------------- guard

TEST_CASE("web guard: a refused multipart body is skipped, whatever follows its last boundary") {
  glue::begin();
  start();
  // to a JSON route, ending right after "--b--" without the CRLF
  const std::string form = "--b\r\nContent-Disposition: form-data; name=\"x\"\r\n\r\n1\r\n--b--";
  Response r =
      fakes::http::perform(apiPost("/api/config", form, "multipart/form-data; boundary=b"));
  CHECK(r.sends == 1);
  CHECK(r.code == 415);
  CHECK(r.body == errorBody("unsupported_media_type", "application/json required"));
  // an upload refused while another one runs, with an epilogue after the closing boundary
  sib::ota().uploadActive = true;
  Request u = apiUpload("/api/stm/images", "fw.bin", "abc");
  u.body = u.wireBody() + "epilogue";
  u.parts.clear();
  r = fakes::http::perform(u);
  CHECK(r.sends == 1);
  CHECK(r.code == 409);
  CHECK(r.body == errorBody("busy", "upload or flash running"));
  CHECK(sib::storage().uploadBegins.empty());
}

// ---------------------------------------------------------------- login

TEST_CASE("web auth: a lock that ends between the guard and the login check answers retry") {
  glue::begin();
  setLogin("admin", "secret12");
  start();
  const Request wrong = withAuth(apiPost(kTarget1, "{\"target\":5}"), false);
  for (int i = 0; i < 10; ++i) REQUIRE(fakes::http::perform(wrong).code == 401);  // 60 s lock
  fakes::advanceMs(59999);
  // The guard reads the clock at the headers and again when the body is in: locked both times.
  // The login check right after reads it 1 ms later, when the lock is over.
  int reads = 0;
  sib::app().onNowMs = [&reads] {
    if (++reads == 3) fakes::advanceMs(1);
  };
  const Response r =
      fakes::http::perform(withAuth(apiUpload("/api/stm/images", "fw.bin", "abc")));
  sib::app().onNowMs = nullptr;
  CHECK(reads == 4);  // the fourth books the login
  CHECK(r.code == 503);
  CHECK(r.body == errorBody("retry", "state changed"));
  CHECK(sib::storage().uploadBegins.empty());
  CHECK(fakes::http::perform(withAuth(apiPost(kTarget1, "{\"target\":6}"))).code == 202);
}
