// Smoke tests of src/ota.cpp: upload steps, restart scheduling, image validation.
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

}  // namespace

TEST_CASE("ota uploadBegin: the announced size may be the partition plus 16 KiB framing") {
  glue::begin();
  CHECK(ota::uploadBegin(kNextSize + kSlack, nullptr));
  CHECK(ota::uploadActive());
  CHECK(sib::logger().withCode(vdm::EventCode::EspOtaStarted).at(0).arg1 ==
        static_cast<int32_t>(kNextSize + kSlack));
  CHECK(fakes::ota().update.beginSize == UPDATE_SIZE_UNKNOWN);
  CHECK(fakes::ota().update.beginCommand == U_FLASH);
}

TEST_CASE("ota uploadBegin: one byte more is refused before Update.begin") {
  glue::begin();
  CHECK_FALSE(ota::uploadBegin(kNextSize + kSlack + 1, nullptr));
  CHECK(std::string(ota::uploadError()) == "image too large");
  CHECK(fakes::ota().update.begins == 0);
}

TEST_CASE("ota uploadBegin: the md5 goes to Update in lowercase") {
  glue::begin();
  CHECK(beginWithMd5(10, "09afAF0123456789abcdefABCDEF0123"));
  CHECK(fakes::ota().update.md5 == "09afaf0123456789abcdefabcdef0123");
}

TEST_CASE("ota uploadBegin: an md5 that is not 32 hex digits is refused") {
  glue::begin();
  CHECK_FALSE(beginWithMd5(10, "09afAF0123456789abcdefABCDEF012g"));
  CHECK(std::string(ota::uploadError()) == "md5 invalid");
  CHECK_FALSE(beginWithMd5(10, "09afAF0123456789abcdefABCDEF012"));
  CHECK(fakes::ota().update.begins == 0);
}

TEST_CASE("ota uploadBegin: refused while an STM flash runs, busy while an upload runs") {
  glue::begin();
  sib::app().flashActive = true;
  CHECK_FALSE(ota::uploadBegin(10, nullptr));
  CHECK(std::string(ota::uploadError()) == "stm flash or upload running");
  sib::app().flashActive = false;
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
  CHECK(ota::uploadEnd(true));
  CHECK_FALSE(ota::uploadActive());
  CHECK(fakes::ota().update.endEvenIfRemaining);
  CHECK(fakes::ota().update.written == std::vector<uint8_t>{0xE9, 1, 2});
  CHECK(sib::logger().withCode(vdm::EventCode::EspOtaDone).at(0).arg1 == 3);
  CHECK(ota::restartPending());
  const vdm::Event reboot = sib::logger().withCode(vdm::EventCode::RebootRequested).at(0);
  CHECK(reboot.arg1 == 1);
  CHECK(reboot.severity == vdm::Severity::Info);
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

TEST_CASE("ota requestRestart: the first request wins; net watchdog and rollback are warnings") {
  glue::begin();
  fakes::setMs(5000);
  ota::requestRestart(2, 1000, 7);
  ota::requestRestart(0, 10);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::RebootRequested);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 2);
  CHECK(ev[0].arg2 == 7);
  CHECK(ev[0].severity == vdm::Severity::Warning);
  ota::serviceRestart(5999, true, true);
  CHECK(fakes::esp().restarts == 0);
  CHECK_THROWS_AS(ota::serviceRestart(6000, true, true), fakes::Restarted);
  CHECK(fakes::find("logger.service 0") < fakes::find("esp_restart"));
}

TEST_CASE("ota serviceRestart: never in the middle of an STM flash") {
  glue::begin();
  ota::requestRestart(3, 0);
  sib::app().flashActive = true;
  ota::serviceRestart(100, true, true);
  CHECK(fakes::esp().restarts == 0);
  sib::app().flashActive = false;
  CHECK_THROWS_AS(ota::serviceRestart(100, true, true), fakes::Restarted);
}

TEST_CASE("ota serviceRestart: a user restart confirms a pending image first") {
  glue::begin();
  fakes::ota().state[0] = ESP_OTA_IMG_PENDING_VERIFY;
  ota::begin();
  ota::requestRestart(0, 0);
  CHECK_THROWS_AS(ota::serviceRestart(0, true, true), fakes::Restarted);
  CHECK(fakes::ota().state[0] == ESP_OTA_IMG_VALID);
  CHECK(sib::logger().has(vdm::EventCode::AppMarkedValid));
}

TEST_CASE("ota service: a pending image healthy for 120 s is marked valid") {
  glue::begin();
  fakes::ota().state[0] = ESP_OTA_IMG_PENDING_VERIFY;
  ota::begin();
  CHECK(ota::health(0).pending);
  ota::service(1000, true, true, true);
  ota::service(120999, true, true, true);
  CHECK(fakes::ota().markValid == 0);
  ota::service(121000, true, true, true);
  CHECK(fakes::ota().markValid == 1);
  CHECK(fakes::ota().state[0] == ESP_OTA_IMG_VALID);
  CHECK_FALSE(ota::health(0).pending);
}

TEST_CASE("ota service: a pending image that never gets healthy is rolled back after 15 min") {
  glue::begin();
  fakes::ota().state[0] = ESP_OTA_IMG_PENDING_VERIFY;
  fakes::ota().state[1] = ESP_OTA_IMG_VALID;
  ota::begin();
  ota::service(899999, false, false, true);
  CHECK(fakes::ota().markInvalid == 0);
  CHECK_THROWS_AS(ota::service(900000, false, false, true), fakes::Restarted);
  CHECK(fakes::find("logger.service 0") < fakes::find("delay 200"));
  CHECK(fakes::find("delay 200") < fakes::find("esp_ota_mark_app_invalid_rollback_and_reboot"));
  CHECK(sib::logger().withCode(vdm::EventCode::RebootRequested).at(0).arg1 == 4);
}
