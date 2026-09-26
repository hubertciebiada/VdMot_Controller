// Edge cases of src/ota.cpp: the timing and the partial reads of the loopback self-check, the
// validator seconds in /api/health, the restart delay after an upload, the restart reasons that
// confirm a pending image, the first upload error of a boot.
#include <IPAddress.h>
#include <Update.h>
#include <esp_ota_ops.h>

#include <string>
#include <vector>

#include "glue_test.h"
#include "ota.h"

namespace {

std::vector<std::string> gRequests;

// A pending image (no STM link required), the network up at `ip`; the loopback server answers
// `first` at once and whatever fakes::net().tcpLater holds later.
void pendingImage(const std::string& first, IPAddress ip = IPAddress(192, 168, 1, 20)) {
  fakes::ota().state[0] = ESP_OTA_IMG_PENDING_VERIFY;
  fakes::ota().state[1] = ESP_OTA_IMG_VALID;
  sib::net().up = true;
  sib::net().info.ip = ip;
  gRequests.clear();
  fakes::net().tcpResponder = [first](const fakes::TcpConnect&, const std::string& req) {
    gRequests.push_back(req);
    return first;
  };
  ota::begin();
}

// The self-check of ota::service() at fake time 0; its result.
bool selfCheckAtZero() {
  ota::service(0, true, true, true);
  return ota::health(0).httpOk;
}

// A restart through the STM save gate: requested, saved, esp_restart().
void restartThroughGate(bool netUp, bool linkUp) {
  ota::serviceRestart(fakes::nowMs(), netUp, linkUp);
  sib::app().saveState = vdm::StmSaveState::Saved;
  CHECK_THROWS_AS(ota::serviceRestart(fakes::nowMs(), netUp, linkUp), fakes::Restarted);
}

}  // namespace

// ---------------------------------------------------------------- self-check timing

TEST_CASE("ota self-check: a silent server is polled every 10 ms for 3 s") {
  glue::begin();
  pendingImage("");
  fakes::net().tcpLater.push_back({100000, "HTTP/1.1 200 OK\r\n"});
  CHECK_FALSE(selfCheckAtZero());
  CHECK(fakes::rtos().delays == std::vector<uint32_t>(300, 10));
  CHECK(fakes::nowMs() == 3000);
  CHECK(gRequests.size() == 1);
}

TEST_CASE("ota self-check: an answer at 3000 ms is too late") {
  glue::begin();
  pendingImage("");
  fakes::net().tcpLater.push_back({3000, "HTTP/1.1 200 OK\r\n"});
  CHECK_FALSE(selfCheckAtZero());
  CHECK(fakes::nowMs() == 3000);
}

TEST_CASE("ota self-check: an answer at 2999 ms still counts") {
  glue::begin();
  pendingImage("");
  fakes::net().tcpLater.push_back({2999, "HTTP/1.1 200 OK\r\n"});
  // one extra 9 ms on the first wait: the loop looks at 0, 19, 29, ..., 2999 ms
  bool shifted = false;
  fakes::rtos().onDelay = [&shifted](uint32_t) {
    if (!shifted) fakes::advanceMs(9);
    shifted = true;
  };
  CHECK(selfCheckAtZero());
  CHECK(fakes::nowMs() == 2999);
}

// ---------------------------------------------------------------- self-check reads

TEST_CASE("ota self-check: a status line in two parts, the last one a single byte") {
  glue::begin();
  pendingImage("HTTP/1.1 20");
  fakes::net().tcpLater.push_back({10, "0"});
  CHECK(selfCheckAtZero());
  CHECK(fakes::rtos().delays == std::vector<uint32_t>{10});
}

TEST_CASE("ota self-check: the second part is read only up to the 12 status bytes") {
  glue::begin();
  pendingImage("HTTP/1.1 20");
  fakes::net().tcpLater.push_back({10, "0 OK\r\nContent-Length: 2\r\n\r\n{}"});
  CHECK(selfCheckAtZero());
}

TEST_CASE("ota self-check: only the first 12 bytes of the status line are looked at") {
  glue::begin();
  pendingImage("HTTP/1.1 200OK\r\n");
  CHECK(selfCheckAtZero());
}

TEST_CASE("ota self-check: the Host header carries a 15-character address whole") {
  glue::begin();
  pendingImage("HTTP/1.1 200 OK\r\n", IPAddress(192, 168, 100, 200));
  CHECK(selfCheckAtZero());
  REQUIRE(gRequests.size() == 1);
  CHECK(gRequests[0] ==
        "GET /api/health HTTP/1.1\r\nHost: 192.168.100.200\r\nConnection: close\r\n\r\n");
}

// ---------------------------------------------------------------- health seconds

TEST_CASE("ota health: healthy and remaining time are whole seconds, rounded down") {
  glue::begin();
  pendingImage("HTTP/1.1 200 OK\r\n");
  ota::service(0, true, true, true);
  ota::service(1998, true, true, true);
  CHECK(ota::health(1998).healthyForS == 1);
  ota::service(2500, true, true, true);
  CHECK(ota::health(2500).remainingS == 897);  // 897.5 s left
}

// ---------------------------------------------------------------- restart

TEST_CASE("ota upload: the restart after a committed image is due at exactly 1000 ms") {
  glue::begin();
  REQUIRE(ota::uploadBegin(1, nullptr));
  uint8_t b = 0xE9;
  REQUIRE(ota::uploadWrite(&b, 1));
  REQUIRE(ota::uploadEnd(true));
  ota::serviceRestart(999, true, true);
  CHECK(sib::app().saveRequests == 0);
  ota::serviceRestart(1000, true, true);
  CHECK(sib::app().saveRequests == 1);
}

TEST_CASE("ota serviceRestart: an upload restart without an upload of this boot stores 0") {
  glue::begin();
  ota::requestRestart(1, 0);
  restartThroughGate(true, true);
  CHECK(sib::storage().otaStmSets == std::vector<bool>{false});
}

TEST_CASE("ota serviceRestart: a factory reset confirms a pending image first") {
  glue::begin();
  pendingImage("");
  ota::requestRestart(3, 0);
  restartThroughGate(true, true);
  CHECK(fakes::ota().markValid == 1);
  CHECK(fakes::ota().state[0] == ESP_OTA_IMG_VALID);
}

TEST_CASE("ota serviceRestart: a requested rollback restart is a warning and confirms nothing") {
  glue::begin();
  pendingImage("");
  ota::requestRestart(4, 0);
  CHECK(sib::logger().withCode(vdm::EventCode::RebootRequested).at(0).severity ==
        vdm::Severity::Warning);
  restartThroughGate(true, true);
  CHECK(fakes::ota().markValid == 0);
}

// ---------------------------------------------------------------- upload errors

TEST_CASE("ota uploadEnd: without any upload in this boot the error is \"no upload\"") {
  glue::begin();
  CHECK_FALSE(ota::uploadEnd(true));
  CHECK(std::string(ota::uploadError()) == "no upload");
}

TEST_CASE("ota uploadError: a long library error is cut to 47 characters") {
  glue::begin();
  const std::string text(60, 'e');
  fakes::ota().update.errorText = text;
  fakes::ota().update.beginResult = false;
  CHECK_FALSE(ota::uploadBegin(10, nullptr));
  CHECK(std::string(ota::uploadError()) == text.substr(0, 47));
}
