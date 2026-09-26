// Lease settings, failsafe positions and learn time of the whole STM glue across resets and power
// cycles, over the UART with the real eeprom.cpp (glue_system): C-5.
#include "eeprom.h"
#include "system_uart.h"
#include "vdm/valve_codes.h"

using namespace sysuart;

TEST_CASE("system C-5: a new chip and a failed EEPROM read give 60 min and 50 %, the re-read the stored values") {
  sim::Rig rig;
  bootController(rig, 0, [] {
    if (testkit::boot() == 1) fake::eeprom.failReadsFrom = 1;  // every read of this boot fails
  });
  if (testkit::boot() == 0) {
    CHECK(exchange("glcfg\n") == glcfg(60, std::vector<uint8_t>(ACTUATOR_COUNT, 50)));
    runMain(5000);
    CHECK(exchange("slcfg 90\n") == "slcfg ok\r\n");
    CHECK(exchange("sfspo 255 20\n") == "sfspo 255 ok\r\n");
    runMain(5000);
    testkit::reboot(testkit::Reset::PowerOn);
  }
  // never 0 %: the defaults while the EEPROM cannot be read
  CHECK(gstax(kCfgFlags) == vdm::kCfgReadFailed);
  CHECK(exchange("glcfg\n") == glcfg(60, std::vector<uint8_t>(ACTUATOR_COUNT, 50)));
  fake::eeprom.failReadsFrom = 0;
  runMain(31000);  // the re-read 30 s later
  CHECK(gstax(kCfgFlags) == 0);
  CHECK(exchange("glcfg\n") == glcfg(90, std::vector<uint8_t>(ACTUATOR_COUNT, 20)));
  CHECK(gstax(kLeaseTimeout) == 90);
}

TEST_CASE("system C-5: a warm reset with the lease expired and the EEPROM read failing: lease 2 at once") {
  sim::Rig rig;
  bootController(rig, 0, [] {
    if (testkit::boot() == 1) fake::eeprom.failReadsFrom = 1;
  });
  if (testkit::boot() == 0) {
    runMain(5000);
    CHECK(exchange("sfspo 255 255\n") == "sfspo 255 ok\r\n");  // the failsafe holds every valve
    CHECK(exchange("sfspo 4 30\n") == "sfspo 4 ok\r\n");
    CHECK(exchange("slcfg 5\n") == "slcfg ok\r\n");
    runMain(5000);
    jumpS(300);
    REQUIRE(gstax(kLease) == 2);
    resetController();
  }
  CHECK(testkit::lastReset() == testkit::Reset::Software);
  CHECK(gstax(kCfgFlags) == vdm::kCfgReadFailed);
  CHECK(gstax(kLease) == 2);
  CHECK(gstax(kLeaseTimeout) == 5);
  std::vector<uint8_t> fs(ACTUATOR_COUNT, 255);
  fs[4] = 30;
  CHECK(exchange("glcfg\n") == glcfg(5, fs));
}
