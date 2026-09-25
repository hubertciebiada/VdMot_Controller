// Tests of src/ota.cpp: upload steps, restart scheduling and the restart path (STM EEPROM gate,
// target flush, log flush), image validation with the loopback self-check, rollback.
#include <IPAddress.h>
#include <Update.h>
#include <esp_ota_ops.h>

#include <memory>
#include <string>

#include "glue_test.h"
#include "ota.h"

namespace {

// The md5 as an exact-size heap string: a read past the terminator is an ASan error.
bool beginWithMd5(size_t announced, const std::string& md5) {
  std::unique_ptr<char[]> s(new char[md5.size() + 1]);
  memcpy(s.get(), md5.c_str(), md5.size() + 1);
  return ota::uploadBegin(announced, s.get());
}

const uint32_t kNextSize = 0x140000;  // app1 of the fake partition table
const uint32_t kSlack = 16 * 1024;

std::vector<std::string> gRequests;

// A pending image, the network up with a device address, the loopback server answering `status`.
void pendingImage(const std::string& status, bool stmRequired) {
  fakes::ota().state[0] = ESP_OTA_IMG_PENDING_VERIFY;
  fakes::ota().state[1] = ESP_OTA_IMG_VALID;
  sib::storage().otaStmRequired = stmRequired;
  sib::net().up = true;
  sib::net().info.ip = IPAddress(192, 168, 1, 20);
  gRequests.clear();
  fakes::net().tcpResponder = [status](const fakes::TcpConnect& to, const std::string& req) {
    char ip[16];
    vdm::formatIpv4(to.ip, ip, sizeof ip);
    gRequests.push_back(std::string(ip) + ":" + std::to_string(to.port) + " " + req);
    return status;
  };
  ota::begin();
}

// ota::service() once a second from `from` to `to` (inclusive), fake clock in step.
void serviceSeconds(uint32_t from, uint32_t to, bool netOk, bool linkUp) {
  for (uint32_t t = from; t <= to; t += 1000) {
    fakes::setMs(t);
    ota::service(t, netOk, linkUp, true);
  }
}

}  // namespace

TEST_CASE("ota uploadBegin: the announced size may be the partition plus 16 KiB framing") {
  glue::begin();
  CHECK(ota::uploadBegin(kNextSize + kSlack, nullptr));
  CHECK(ota::uploadActive());
  CHECK(sib::logger().withCode(vdm::EventCode::EspOtaStarted).at(0).arg1 ==
        static_cast<int32_t>(kNextSize + kSlack));
  CHECK(fakes::ota().update.beginSize == UPDATE_SIZE_UNKNOWN);
  CHECK(fakes::ota().update.beginCommand == U_FLASH);
  CHECK(fakes::ota().update.md5.empty());
}

TEST_CASE("ota uploadBegin: one byte more is refused before Update.begin") {
  glue::begin();
  CHECK_FALSE(ota::uploadBegin(kNextSize + kSlack + 1, nullptr));
  CHECK(std::string(ota::uploadError()) == "image too large");
  CHECK(fakes::ota().update.begins == 0);
}

TEST_CASE("ota uploadBegin: no partition to write to") {
  glue::begin();
  fakes::ota().hasNext = false;
  CHECK_FALSE(ota::uploadBegin(10, nullptr));
  CHECK(std::string(ota::uploadError()) == "no ota partition");
}

TEST_CASE("ota uploadBegin: the md5 goes to Update in lowercase, \"\" means none") {
  glue::begin();
  CHECK(beginWithMd5(10, "09afAF0123456789abcdefABCDEF0123"));
  CHECK(fakes::ota().update.md5 == "09afaf0123456789abcdefabcdef0123");
  REQUIRE(ota::uploadEnd(false) == false);
  CHECK(beginWithMd5(10, ""));
  CHECK(fakes::ota().update.md5.empty());
}

TEST_CASE("ota uploadBegin: an md5 that is not 32 hex digits is refused") {
  glue::begin();
  CHECK_FALSE(beginWithMd5(10, "09afAF0123456789abcdefABCDEF012g"));
  CHECK(std::string(ota::uploadError()) == "md5 invalid");
  CHECK_FALSE(beginWithMd5(10, "09afAF0123456789abcdefABCDEF012"));
  CHECK(fakes::ota().update.begins == 0);
}

TEST_CASE("ota uploadBegin: Update refusing the md5 aborts the upload") {
  glue::begin();
  fakes::ota().update.setMd5Result = false;
  CHECK_FALSE(beginWithMd5(10, "09afAF0123456789abcdefABCDEF0123"));
  CHECK(std::string(ota::uploadError()) == "md5 invalid");
  CHECK(fakes::ota().update.aborts == 1);
  CHECK_FALSE(ota::uploadActive());
}

TEST_CASE("ota uploadBegin: Update.begin failing is logged with the library's error") {
  glue::begin();
  fakes::ota().update.beginResult = false;
  CHECK_FALSE(ota::uploadBegin(10, nullptr));
  CHECK(sib::logger().withCode(vdm::EventCode::EspOtaFailed).at(0).arg1 == UPDATE_ERROR_SPACE);
  CHECK_FALSE(ota::uploadActive());
}

TEST_CASE("ota uploadBegin: refused while an STM flash or image upload runs, busy while an upload runs") {
  glue::begin();
  sib::app().flashActive = true;
  CHECK_FALSE(ota::uploadBegin(10, nullptr));
  CHECK(std::string(ota::uploadError()) == "stm flash or upload running");
  sib::app().flashActive = false;
  sib::storage().imageUploadActive = true;
  CHECK_FALSE(ota::uploadBegin(10, nullptr));
  sib::storage().imageUploadActive = false;
  REQUIRE(ota::uploadBegin(10, nullptr));
  CHECK(std::string(ota::uploadError()) == "unknown");
  CHECK_FALSE(ota::uploadBegin(10, nullptr));
  CHECK(std::string(ota::uploadError()) == "busy");
}

TEST_CASE("ota upload: a committed image asks for a restart in 1 s") {
  glue::begin();
  REQUIRE(ota::uploadBegin(3, nullptr));
  uint8_t data[3] = {0xE9, 1, 2};
  CHECK(ota::uploadWrite(data, 3));
  CHECK(ota::uploadWrite(data, 0));
  CHECK(ota::uploadEnd(true));
  CHECK_FALSE(ota::uploadActive());
  CHECK(fakes::ota().update.endEvenIfRemaining);
  CHECK(fakes::ota().update.written == std::vector<uint8_t>{0xE9, 1, 2});
  CHECK(sib::logger().withCode(vdm::EventCode::EspOtaDone).at(0).arg1 == 3);
  CHECK(ota::restartPending());
  const vdm::Event reboot = sib::logger().withCode(vdm::EventCode::RebootRequested).at(0);
  CHECK(reboot.arg1 == 1);
  CHECK(reboot.severity == vdm::Severity::Info);
  CHECK_FALSE(ota::uploadWrite(data, 3));  // no upload any more
  CHECK_FALSE(ota::uploadEnd(true));
  CHECK(std::string(ota::uploadError()) == "no upload");
}

TEST_CASE("ota upload: an aborted or empty upload fails with -1 / -2") {
  glue::begin();
  REQUIRE(ota::uploadBegin(3, nullptr));
  CHECK_FALSE(ota::uploadEnd(false));
  CHECK(std::string(ota::uploadError()) == "aborted");
  REQUIRE(ota::uploadBegin(3, nullptr));
  CHECK_FALSE(ota::uploadEnd(true));
  CHECK(std::string(ota::uploadError()) == "empty image");
  const std::vector<vdm::Event> failed = sib::logger().withCode(vdm::EventCode::EspOtaFailed);
  REQUIRE(failed.size() == 2);
  CHECK(failed[0].arg1 == -1);
  CHECK(failed[1].arg1 == -2);
  CHECK_FALSE(ota::restartPending());
}

TEST_CASE("ota upload: Update.end failing keeps the image unselected") {
  glue::begin();
  fakes::ota().update.endResult = false;
  REQUIRE(ota::uploadBegin(3, nullptr));
  uint8_t data[1] = {0xE9};
  REQUIRE(ota::uploadWrite(data, 1));
  CHECK_FALSE(ota::uploadEnd(true));
  CHECK(sib::logger().withCode(vdm::EventCode::EspOtaFailed).at(0).arg1 == UPDATE_ERROR_MD5);
  CHECK_FALSE(ota::restartPending());
}

TEST_CASE("ota upload: a short write aborts the upload with the library's error") {
  glue::begin();
  fakes::ota().update.shortWriteAt = 2;
  REQUIRE(ota::uploadBegin(4, nullptr));
  uint8_t data[4] = {1, 2, 3, 4};
  CHECK_FALSE(ota::uploadWrite(data, 4));
  CHECK_FALSE(ota::uploadActive());
  CHECK(std::string(ota::uploadError()) == "Flash Write Failed");
  CHECK(sib::logger().withCode(vdm::EventCode::EspOtaFailed).at(0).arg1 == UPDATE_ERROR_WRITE);
  CHECK_FALSE(ota::uploadEnd(true));
  CHECK(std::string(ota::uploadError()) == "Flash Write Failed");
}

TEST_CASE("ota requestRestart: the first request wins; severity per reason") {
  glue::begin();
  fakes::setMs(5000);
  CHECK_FALSE(ota::restartPending());
  ota::requestRestart(2, 1000, 7);
  ota::requestRestart(0, 10);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::RebootRequested);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 2);
  CHECK(ev[0].arg2 == 7);
  CHECK(ev[0].severity == vdm::Severity::Warning);
  CHECK(ota::restartPending());
  ota::serviceRestart(5999, true, true);
  CHECK(sib::app().saveRequests == 0);
  ota::serviceRestart(6000, true, true);
  CHECK(sib::app().saveRequests == 1);
}

TEST_CASE("ota requestRestart: a network revert is a warning") {
  glue::begin();
  ota::requestRestart(5, 0);
  CHECK(sib::logger().withCode(vdm::EventCode::RebootRequested).at(0).severity ==
        vdm::Severity::Warning);
}

TEST_CASE("ota requestRestart: a factory reset is Info") {
  glue::begin();
  ota::requestRestart(3, 0);
  CHECK(sib::logger().withCode(vdm::EventCode::RebootRequested).at(0).severity ==
        vdm::Severity::Info);
}

TEST_CASE("ota requestRestart: a user restart is Info") {
  glue::begin();
  ota::requestRestart(0, 0);
  CHECK(sib::logger().withCode(vdm::EventCode::RebootRequested).at(0).severity ==
        vdm::Severity::Info);
}

TEST_CASE("ota serviceRestart: STM save, target flush, log flush, then esp_restart") {
  glue::begin();
  ota::requestRestart(0, 0);
  ota::serviceRestart(0, true, true);
  CHECK(sib::app().saveRequests == 1);
  CHECK(sib::stmService().restartFlushes == 1);
  ota::serviceRestart(5000, true, true);  // Waiting
  CHECK(sib::logger().flushes == 0);
  sib::app().saveState = vdm::StmSaveState::Saved;
  CHECK_THROWS_AS(ota::serviceRestart(5100, true, true), fakes::Restarted);
  const int save = fakes::find("app.requestStmSave");
  const int targets = fakes::find("stm_service.flushForRestart");
  const int flush = fakes::find("logger.flush");
  const int restart = fakes::find("esp_restart");
  CHECK(save >= 0);
  CHECK(save < targets);
  CHECK(targets < flush);
  CHECK(flush < restart);
  CHECK(sib::app().saveRequests == 1);
  CHECK(sib::stmService().restartFlushes == 1);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::StmEepromWaitTimeout));
}

TEST_CASE("ota serviceRestart: an unavailable STM does not hold the restart") {
  glue::begin();
  ota::requestRestart(2, 0);
  ota::serviceRestart(0, true, true);
  sib::app().saveState = vdm::StmSaveState::Unavailable;
  CHECK_THROWS_AS(ota::serviceRestart(100, true, true), fakes::Restarted);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::StmEepromWaitTimeout));
}

TEST_CASE("ota serviceRestart: an STM save timed out by the stm task does not hold the restart") {
  glue::begin();
  ota::requestRestart(2, 0);
  ota::serviceRestart(0, true, true);
  sib::app().saveState = vdm::StmSaveState::TimedOut;
  CHECK_THROWS_AS(ota::serviceRestart(100, true, true), fakes::Restarted);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::StmEepromWaitTimeout));
}

TEST_CASE("ota serviceRestart: a factory reset does not save the targets") {
  glue::begin();
  ota::requestRestart(3, 0);
  ota::serviceRestart(0, true, true);
  CHECK(sib::app().saveRequests == 1);
  CHECK(sib::stmService().restartFlushes == 0);
}

TEST_CASE("ota serviceRestart: a silent stm task -> restart at 12000 ms with event 323") {
  glue::begin();
  ota::requestRestart(0, 0);
  ota::serviceRestart(0, true, true);
  ota::serviceRestart(11999, true, true);
  CHECK(fakes::esp().restarts == 0);
  CHECK_THROWS_AS(ota::serviceRestart(12000, true, true), fakes::Restarted);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::StmEepromWaitTimeout);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 12000);
  CHECK(ev[0].arg2 == 3);
  CHECK(std::string(ev[0].text) == "stm task silent");
  CHECK(fakes::find("logger.log stm_eeprom_wait_timeout") < fakes::find("logger.flush"));
}

TEST_CASE("ota serviceRestart: never in the middle of an STM flash") {
  glue::begin();
  ota::requestRestart(3, 0);
  sib::app().flashActive = true;
  ota::serviceRestart(100, true, true);
  CHECK(sib::app().saveRequests == 0);
  sib::app().flashActive = false;
  ota::serviceRestart(100, true, true);
  sib::app().saveState = vdm::StmSaveState::Saved;
  CHECK_THROWS_AS(ota::serviceRestart(200, true, true), fakes::Restarted);
}

TEST_CASE("ota serviceRestart: an upload restart stores whether the STM link was up") {
  glue::begin();
  sib::app().link = vdm::LinkState::Up;
  REQUIRE(ota::uploadBegin(1, nullptr));
  uint8_t b = 0xE9;
  REQUIRE(ota::uploadWrite(&b, 1));
  REQUIRE(ota::uploadEnd(true));
  sib::app().link = vdm::LinkState::Down;  // the link state of the upload counts
  fakes::setMs(1000);
  ota::serviceRestart(1000, true, false);
  sib::app().saveState = vdm::StmSaveState::Saved;
  CHECK(sib::storage().otaStmSets.empty());
  CHECK_THROWS_AS(ota::serviceRestart(1100, true, false), fakes::Restarted);
  CHECK(sib::storage().otaStmSets == std::vector<bool>{true});
}

TEST_CASE("ota serviceRestart: an upload with the link down stores 0") {
  glue::begin();
  REQUIRE(ota::uploadBegin(1, nullptr));
  uint8_t b = 0xE9;
  REQUIRE(ota::uploadWrite(&b, 1));
  REQUIRE(ota::uploadEnd(true));
  fakes::setMs(1000);
  ota::serviceRestart(1000, true, true);
  sib::app().saveState = vdm::StmSaveState::Saved;
  CHECK_THROWS_AS(ota::serviceRestart(1100, true, true), fakes::Restarted);
  CHECK(sib::storage().otaStmSets == std::vector<bool>{false});
}

TEST_CASE("ota serviceRestart: other restarts do not touch otaStm") {
  glue::begin();
  ota::requestRestart(0, 0);
  ota::serviceRestart(0, true, true);
  sib::app().saveState = vdm::StmSaveState::Saved;
  CHECK_THROWS_AS(ota::serviceRestart(100, true, true), fakes::Restarted);
  CHECK(sib::storage().otaStmSets.empty());
}

TEST_CASE("ota begin: a pending image reads otaStm once and erases it") {
  glue::begin();
  fakes::ota().state[0] = ESP_OTA_IMG_PENDING_VERIFY;
  sib::storage().otaStmRequired = true;
  ota::begin();
  const vdm::OtaHealthInfo h = ota::health(0);
  CHECK(h.pending);
  CHECK(h.stmRequired);
  CHECK_FALSE(sib::storage().otaStmRequired);
  CHECK(sib::storage().otaStmSets == std::vector<bool>{false});
}

TEST_CASE("ota begin: an image not pending ignores and erases otaStm") {
  glue::begin();
  sib::storage().otaStmRequired = true;
  ota::begin();
  CHECK_FALSE(ota::health(0).pending);
  CHECK_FALSE(ota::health(0).stmRequired);
  CHECK_FALSE(sib::storage().otaStmRequired);
  fakes::ota().stateResult = ESP_FAIL;
  fakes::ota().state[0] = ESP_OTA_IMG_PENDING_VERIFY;
  ota::begin();
  CHECK_FALSE(ota::health(0).pending);
  fakes::ota().stateResult = ESP_OK;
  fakes::ota().hasRunning = false;
  ota::begin();
  CHECK_FALSE(ota::health(0).pending);
}

TEST_CASE("ota serviceRestart: a user restart confirms a pending image first") {
  glue::begin();
  pendingImage("", false);
  ota::requestRestart(0, 0);
  ota::serviceRestart(0, true, false);
  sib::app().saveState = vdm::StmSaveState::Saved;
  CHECK_THROWS_AS(ota::serviceRestart(0, true, false), fakes::Restarted);
  CHECK(fakes::ota().state[0] == ESP_OTA_IMG_VALID);
  CHECK(sib::logger().has(vdm::EventCode::AppMarkedValid));
}

TEST_CASE("ota serviceRestart: a user restart without the required STM link does not confirm") {
  glue::begin();
  pendingImage("", true);
  ota::requestRestart(3, 0);
  ota::serviceRestart(0, true, false);
  sib::app().saveState = vdm::StmSaveState::Saved;
  CHECK_THROWS_AS(ota::serviceRestart(0, true, false), fakes::Restarted);
  CHECK(fakes::ota().markValid == 0);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::AppMarkedValid));
}

TEST_CASE("ota serviceRestart: a watchdog restart does not confirm a pending image") {
  glue::begin();
  pendingImage("", false);
  ota::requestRestart(2, 0);
  ota::serviceRestart(0, true, true);
  sib::app().saveState = vdm::StmSaveState::Saved;
  CHECK_THROWS_AS(ota::serviceRestart(0, true, true), fakes::Restarted);
  CHECK(fakes::ota().markValid == 0);
}

TEST_CASE("ota service: loopback self-check 200, net and link -> valid 120 s after the first healthy second") {
  glue::begin();
  pendingImage("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{}", true);
  serviceSeconds(0, 119000, true, true);
  CHECK(fakes::ota().markValid == 0);
  vdm::OtaHealthInfo h = ota::health(119000);
  CHECK(h.pending);
  CHECK(h.stmRequired);
  CHECK(h.netOk);
  CHECK(h.httpOk);
  CHECK(h.stmOk);
  CHECK(h.healthyForS == 119);
  CHECK(h.remainingS == 781);
  serviceSeconds(120000, 120000, true, true);
  CHECK(fakes::ota().markValid == 1);
  CHECK(fakes::ota().state[0] == ESP_OTA_IMG_VALID);
  CHECK(sib::logger().has(vdm::EventCode::AppMarkedValid));
  CHECK_FALSE(ota::health(120000).pending);
  REQUIRE(gRequests.size() == 13);  // every 10 s while pending
  CHECK(gRequests[0] ==
        "127.0.0.1:80 GET /api/health HTTP/1.1\r\nHost: 192.168.1.20\r\nConnection: close\r\n\r\n");
  REQUIRE_FALSE(fakes::net().tcpConnects.empty());
  CHECK(fakes::net().tcpConnects[0].timeoutMs == 1000);
  serviceSeconds(121000, 140000, true, true);
  CHECK(gRequests.size() == 13);  // no self-check once valid
}

TEST_CASE("ota service: no self-check while the network is down or the web server is off") {
  glue::begin();
  pendingImage("HTTP/1.1 200 OK\r\n", false);
  sib::net().up = false;
  ota::service(0, true, true, true);
  CHECK(gRequests.empty());
  sib::net().up = true;
  ota::service(1000, true, true, false);
  CHECK(gRequests.empty());
  ota::service(2000, true, true, true);
  CHECK(gRequests.size() == 1);
}

TEST_CASE("ota service: an unreachable loopback server fails the check") {
  glue::begin();
  pendingImage("HTTP/1.1 200 OK\r\n", false);
  fakes::net().tcpConnectResult = 0;
  ota::service(0, true, true, true);
  CHECK_FALSE(ota::health(0).httpOk);
  CHECK(gRequests.empty());
  fakes::net().tcpConnectResult = 1;
  fakes::net().tcpResponder = [](const fakes::TcpConnect&, const std::string&) {
    return std::string("HTTP/1.1 20");
  };
  ota::service(10000, true, true, true);
  CHECK_FALSE(ota::health(10000).httpOk);
  pendingImage("HTTP/1.1 200", false);
  ota::service(20000, true, true, true);
  CHECK(ota::health(20000).httpOk);
}

TEST_CASE("ota service: a 503 self-check rolls back at 900 s through the restart path") {
  glue::begin();
  pendingImage("HTTP/1.1 503 Service Unavailable\r\n", false);
  serviceSeconds(0, 899000, true, true);
  CHECK_FALSE(ota::restartPending());
  fakes::setMs(900000);
  ota::service(900000, true, true, true);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::RebootRequested);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 4);
  CHECK(ev[0].arg2 == 2);
  CHECK(ev[0].severity == vdm::Severity::Warning);
  CHECK(ota::restartPending());
  ota::serviceRestart(900999, true, true);
  CHECK(sib::app().saveRequests == 0);
  ota::serviceRestart(901000, true, true);
  CHECK(sib::app().saveRequests == 1);
  CHECK(fakes::ota().markInvalid == 0);
  sib::app().saveState = vdm::StmSaveState::Saved;
  CHECK_THROWS_AS(ota::serviceRestart(901100, true, true), fakes::Restarted);
  CHECK(fakes::find("app.requestStmSave") < fakes::find("logger.flush"));
  CHECK(fakes::find("logger.flush") <
        fakes::find("esp_ota_mark_app_invalid_rollback_and_reboot"));
  CHECK(fakes::ota().boot == 1);
  CHECK(fakes::ota().markValid == 0);
}

TEST_CASE("ota service: a rollback without another image keeps running this one") {
  glue::begin();
  pendingImage("", false);
  fakes::ota().state[1] = ESP_OTA_IMG_INVALID;
  serviceSeconds(0, 900000, false, false);
  const vdm::Event r = sib::logger().withCode(vdm::EventCode::RebootRequested).at(0);
  CHECK(r.arg2 == 3);  // net and http missing
  ota::serviceRestart(901000, false, false);
  sib::app().saveState = vdm::StmSaveState::Saved;
  ota::serviceRestart(901100, false, false);
  CHECK(fakes::ota().markInvalid == 1);
  CHECK(sib::logger().withCode(vdm::EventCode::EspOtaFailed).at(0).arg1 == -3);
  CHECK_FALSE(ota::restartPending());
  // A later restart goes through the gate again and restarts normally.
  sib::app().saveState = vdm::StmSaveState::Idle;
  ota::requestRestart(0, 0);
  ota::serviceRestart(902000, false, false);
  CHECK(sib::app().saveRequests == 2);
  sib::app().saveState = vdm::StmSaveState::Saved;
  CHECK_THROWS_AS(ota::serviceRestart(902100, false, false), fakes::Restarted);
  CHECK(fakes::ota().markInvalid == 1);
}

TEST_CASE("ota service: a rollback while another restart is pending becomes that restart") {
  glue::begin();
  pendingImage("", true);
  serviceSeconds(0, 899000, true, false);
  fakes::setMs(899500);
  ota::requestRestart(0, 5000);
  fakes::setMs(900000);
  ota::service(900000, true, false, true);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::RebootRequested);
  REQUIRE(ev.size() == 2);
  CHECK(ev[1].arg1 == 4);
  CHECK(ev[1].arg2 == 6);  // http and stm missing
  ota::serviceRestart(904499, true, true);
  CHECK(sib::app().saveRequests == 0);  // the pending restart's time stands
  ota::serviceRestart(904500, true, true);
  sib::app().saveState = vdm::StmSaveState::Saved;
  CHECK_THROWS_AS(ota::serviceRestart(904600, true, true), fakes::Restarted);
  CHECK(fakes::ota().markInvalid == 1);
  CHECK(fakes::ota().markValid == 0);  // the user restart does not confirm a rolled-back image
  CHECK(sib::stmService().restartFlushes == 1);
}

TEST_CASE("ota service: MarkValid with a failing mark call logs nothing") {
  glue::begin();
  pendingImage("HTTP/1.1 200 OK\r\n", false);
  fakes::ota().markValidResult = ESP_FAIL;
  serviceSeconds(0, 120000, true, false);
  CHECK(fakes::ota().markValid == 1);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::AppMarkedValid));
}
