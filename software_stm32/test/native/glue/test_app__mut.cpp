// src/app.cpp (glue_app): return values, the debug lines of two-digit valves, the 10 s backstop and
// countdowns, the setters for one and all valves, sensor matching, warm copies of the lease client
// and the retry, calibration requests of a shorted valve, the retry pause while a valve is busy.
#include <string>

#include "glue_test.h"
#include "stub_eeprom.h"
#include "stub_motor.h"
#include "stub_owDevices.h"
#include "stub_sysstat.h"
#include "stub_terminal.h"
#include "vdm/valve_codes.h"

extern unsigned int reset_request;

namespace {

// app_setup: every valve UNKNOWN
void beginCold() {
  glue::begin();
  app_setup();
  stub::calls.clear();
  fake::takeTx(Serial6);
}

// app_setup, then every valve idle, calibrated, connected and at its target 50
void begin() {
  beginCold();
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    myvalvemots[v].status = VLV_STATE_IDLE;
    myvalvemots[v].calibrated = 1;
    myvalvemots[v].connected = 1;
    myvalvemots[v].scaler = 36;
  }
}

valve_v3_info v3(uint16_t v) {
  valve_v3_info info = {0xFFFF, 0xFF, 0xFF, 0xFF, 0xFFFFFFFF, 0xFF};
  app_get_valve_v3(v, info);
  return info;
}

}  // namespace

TEST_CASE("app_loop and app_set_learnmovements return 0") {
  beginCold();
  CHECK(app_loop() == 0);
  stub::motor.idle = false;
  CHECK(app_loop() == 0);
  stub::motor.idle = true;
  stub::terminal.manualActive = true;
  CHECK(app_loop() == 0);
  CHECK(app_set_learnmovements(60) == 0);
}

TEST_CASE("app_setup: the learn time spread of the start values stays when the EEPROM holds the default") {
  glue::begin();
  eep_content.learnTimeS = LEARN_AFTER_TIME_DEFAULT;
  app_setup();
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    CAPTURE(v);
    CHECK(+myvalves[v].learn_time ==
          (unsigned int) (((long) LEARN_AFTER_TIME_DEFAULT * ((long) v + 1)) / (long) ACTUATOR_COUNT));
  }
}

TEST_CASE("app_loop debug: presence tests of valves 9 and 10 name them in decimal") {
  beginCold();
  for (int i = 0; i < 12; i++) app_loop();
  std::string want;
  for (unsigned v : {0u, 2u, 4u, 6u, 8u, 10u, 1u, 3u, 5u, 7u, 9u, 11u}) {
    want += "App: valve " + std::to_string(v) + " unknown, try to find out...\r\n";
  }
  CHECK(fake::takeTx(Serial6) == want);
}

TEST_CASE("app_loop debug: the calibration start of valve 10, the time trigger is cleared") {
  begin();
  CHECK(app_set_valvelearning(10) == 0);
  myvalves[10].timedLearn = 1;
  for (int i = 0; i < 3; i++) app_loop();
  CHECK(stub::callsOf("appsetaction") == stub::Calls{"appsetaction(l, 10, 0, 0)"});
  CHECK(fake::takeTx(Serial6) == "App: learning started for valve 10\r\n");
  CHECK(+myvalves[10].timedLearn == 0);
  CHECK(+myvalves[10].forcedLearn == 0);
}

TEST_CASE("app_10s_loop: the time trigger names valves 0..10 in decimal") {
  beginCold();
  app_set_learntime(120);
  app_10s_loop(110);
  std::string want;
  for (unsigned v = 0; v <= 10; v++) want += "App: Valve " + std::to_string(v) + " will be learned soon\r\n";
  CHECK(fake::takeTx(Serial6) == want);
  CHECK(+myvalves[10].timedLearn == 1);
  CHECK(+myvalves[11].timedLearn == 0);
}

TEST_CASE("app_10s_loop: a learn time of 1 s counts down") {
  beginCold();
  app_set_learntime(1);
  app_10s_loop(10);
  CHECK(+myvalves[0].timedLearn == 1);
}

TEST_CASE("app_warm_moving: an invalid warm record stays invalid") {
  begin();
  app_warm_moving(0);
  stub::sysstat.reason = vdm::BootReason::Software;
  myvalvemots[0].actual_position = 33;
  app_restore();
  CHECK(+myvalvemots[0].actual_position == 33);
  CHECK(app_lease_timeout() == 60);
}

TEST_CASE("app_10s_loop: a running calibration satisfies the time trigger") {
  begin();
  app_set_learntime(120);
  myvalvemots[0].calibActive = 1;
  app_10s_loop(10);
  CHECK(+myvalves[0].timedLearn == 0);
  CHECK(+myvalves[0].learn_time == 120);
}

TEST_CASE("app_10s_loop: the movement trigger of valves 0, 9 and 10") {
  beginCold();
  app_set_learnmovements(50);
  for (unsigned v : {0u, 9u, 10u}) {
    myvalves[v].learn_movements = 0;
    myvalves[v].movements = 5;
    myvalvemots[v].connected = 1;
  }
  app_10s_loop(10);
  CHECK(fake::takeTx(Serial6) == "App: Valve 0 will be learned soon\r\nApp: Valve 9 will be learned soon\r\n"
                                 "App: Valve 10 will be learned soon\r\n");
  for (unsigned v : {0u, 9u, 10u}) {
    CAPTURE(v);
    CHECK(+myvalvemots[v].calibration == 1);
    CHECK(+myvalvemots[v].calibTime == 10);
    CHECK(+myvalvemots[v].calibState == calibStarted);
    CHECK(+myvalves[v].movements == 0);
    CHECK(+myvalves[v].learn_movements == 50);
  }
}

TEST_CASE("app_10s_loop: movements 0 switch the trigger off, 1 is a trigger") {
  beginCold();
  app_set_learnmovements(0);
  myvalvemots[2].connected = 1;
  app_10s_loop(10);
  CHECK(+myvalvemots[2].calibration == 0);
  app_set_learnmovements(1);
  myvalves[2].learn_movements = 0;
  app_10s_loop(10);
  CHECK(+myvalvemots[2].calibration == 1);
}

TEST_CASE("app_10s_loop: service holds count down, the calibration backstop") {
  beginCold();
  myvalves[0].svcHold = 2;
  myvalves[11].svcHold = 2;
  myvalvemots[3].calibState = calibInProgress;
  myvalvemots[3].calibration = true;
  myvalvemots[3].calibActive = 0;
  myvalvemots[3].calibTime = 2;
  myvalvemots[4].calibState = calibInProgress;
  myvalvemots[4].calibration = true;
  myvalvemots[4].calibActive = 1;
  myvalvemots[4].calibTime = 0;
  myvalvemots[5].calibState = calibStarted;
  myvalvemots[5].calibration = true;
  myvalvemots[5].calibTime = 2;
  app_10s_loop(10);
  CHECK(+myvalves[0].svcHold == 1);
  CHECK(+myvalves[11].svcHold == 1);
  CHECK(+myvalvemots[3].calibTime == 1);
  CHECK(+myvalvemots[4].calibTime == CALIB_START_TICKS);
  CHECK(+myvalvemots[5].calibTime == 2);
  app_10s_loop(10);
  CHECK(+myvalves[0].svcHold == 0);
  CHECK(+myvalvemots[3].calibTime == 0);
  CHECK(+myvalvemots[3].calibration == 1);
  CHECK(+myvalvemots[3].calibState == calibInProgress);
  app_10s_loop(10);
  CHECK(+myvalves[0].svcHold == 0);
  CHECK(+myvalvemots[3].calibTime == 0);
  CHECK(+myvalvemots[3].calibration == 0);
  CHECK(+myvalvemots[3].calibState == calibIdle);
  CHECK(+myvalvemots[4].calibration == 1);
  CHECK(+myvalvemots[5].calibration == 1);
  CHECK(+myvalvemots[5].calibState == calibStarted);
}

TEST_CASE("app_set_valvelearning: one valve and all connected valves") {
  beginCold();
  for (unsigned v : {0u, 3u, 11u}) {
    myvalves[v].svcHold = 5;
    myvalves[v].movements = 7;
    myvalvemots[v].connected = 1;
  }
  app_set_learnmovements(60);
  CHECK(+myvalves[0].movements == 0);
  CHECK(+myvalves[11].movements == 0);
  myvalves[3].movements = 7;
  CHECK(app_set_valvelearning(3) == 0);
  CHECK(+myvalves[3].forcedLearn == 1);
  CHECK(+myvalves[3].svcHold == 0);
  CHECK(+myvalvemots[3].calibration == 1);
  CHECK(+myvalvemots[3].calibState == calibStarted);
  CHECK(+myvalvemots[3].calibTime == 10);
  CHECK(+myvalves[3].movements == 0);
  myvalves[0].movements = 7;
  myvalves[11].movements = 7;
  CHECK(app_set_valvelearning(255) == 0);
  for (unsigned v : {0u, 11u}) {
    CAPTURE(v);
    CHECK(+myvalves[v].forcedLearn == 1);
    CHECK(+myvalves[v].svcHold == 0);
    CHECK(+myvalvemots[v].calibration == 1);
    CHECK(+myvalvemots[v].calibState == calibStarted);
    CHECK(+myvalvemots[v].calibTime == 10);
    CHECK(+myvalves[v].movements == 0);
    CHECK(+myvalves[v].learn_movements == 60);
  }
  CHECK(+myvalves[1].forcedLearn == 0);  // not connected
}

TEST_CASE("app_set_valveopen 255: every valve gets the request, the service hold ends") {
  beginCold();
  myvalves[0].svcHold = 5;
  myvalves[11].svcHold = 5;
  CHECK(app_set_valveopen(255) == 0);
  for (unsigned v : {0u, 11u}) {
    CAPTURE(v);
    CHECK(+myvalves[v].openRequest == 1);
    CHECK(+myvalves[v].svcHold == 0);
    CHECK(+myvalves[v].assemblyHold == 1);
  }
}

TEST_CASE("app_match_sensors: every byte of the rom code counts, the debug lines") {
  glue::begin();
  const uint8_t exact[8] = {0x28, 1, 2, 3, 4, 5, 6, 0x77};
  const uint8_t last[8] = {0x28, 1, 2, 3, 4, 5, 9, 0x77};
  const uint8_t first[8] = {0x28, 9, 2, 3, 4, 5, 6, 0x77};
  const uint8_t second[8] = {0x28, 7, 7, 7, 7, 7, 7, 0x55};
  const uint8_t none[8] = {0x28, 1, 1, 1, 1, 1, 1, 0x99};
  noOfDS18Devices = 5;
  memcpy(tempsensors[0].address, exact, 8);
  memcpy(tempsensors[1].address, last, 8);
  memcpy(tempsensors[2].address, first, 8);
  memcpy(tempsensors[3].address, second, 8);
  memcpy(tempsensors[4].address, none, 8);
  eep_content.owsensors1[0].familycode = 0x28;
  for (int i = 0; i < 6; i++) eep_content.owsensors1[0].romcode[i] = static_cast<uint8_t>(i + 1);
  eep_content.owsensors1[0].crc = 0x77;
  eep_content.owsensors2[11].familycode = 0x28;
  for (int i = 0; i < 6; i++) eep_content.owsensors2[11].romcode[i] = 7;
  eep_content.owsensors2[11].crc = 0x55;
  myvalves[0].sensorindex2 = 4;
  fake::takeTx(Serial6);
  CHECK(app_match_sensors() == 0);
  CHECK(+myvalves[0].sensorindex1 == 0);
  CHECK(+myvalves[0].sensorindex2 == VALVE_SENSOR_UNKNOWN);
  CHECK(+myvalves[11].sensorindex2 == 3);
  CHECK(+myvalves[11].sensorindex1 == VALVE_SENSOR_UNKNOWN);
  CHECK(fake::takeTx(Serial6) ==
        "Read 1-wire sensor addresses from eeprom\r\n"
        " found as 1st sensor at valve: 0:0\r\n"
        " not found\r\n"
        " not found\r\n"
        " found as 2nd sensor at valve: 11\r\n"
        " not found\r\n");
}

TEST_CASE("reset_STM32: sets the request flag to 1") {
  beginCold();
  reset_STM32();
  CHECK(reset_request == 1);
}

TEST_CASE("app_restore: the scaler is the opening count / 100") {
  begin();
  eep_content.calib[0] = vdm::CalibRecord{9900, 9900, 20, vdm::kCalibValid};
  stub::sysstat.reason = vdm::BootReason::PowerOn;
  app_restore();
  CHECK(+myvalvemots[0].scaler == 99);
}

TEST_CASE("app_warm_save: the lease client and an unscheduled retry survive a warm reset") {
  begin();
  app_lease_command();
  REQUIRE(app_lease_client());
  app_warm_save();
  stub::sysstat.reason = vdm::BootReason::Software;
  app_restore();
  CHECK(app_lease_client());
  CHECK((v3(0).flags & vdm::kVlvFlagRetry) == 0);
}

TEST_CASE("app_warm_save: no lease client stays none after a warm reset") {
  begin();
  REQUIRE_FALSE(app_lease_client());
  app_warm_save();
  stub::sysstat.reason = vdm::BootReason::Software;
  app_restore();
  CHECK_FALSE(app_lease_client());
}

TEST_CASE("app_stop 255: requests of valve 0 end, the stopped valve gets the hold") {
  begin();
  myvalves[0].forcedLearn = 1;
  myvalves[11].forcedLearn = 1;
  stub::motor.stop = 0;
  CHECK(app_stop(255) == 0);
  CHECK(+myvalves[0].forcedLearn == 0);
  CHECK(+myvalves[11].forcedLearn == 0);
  CHECK(+myvalves[0].svcHold == SVMOV_HOLD_10S);
  CHECK(+myvalves[1].svcHold == 0);
}

TEST_CASE("short W10: a time, retry or early-stop request of a shorted valve becomes a test") {
  begin();
  for (unsigned v : {0u, 1u, 2u}) {
    myvalvemots[v].status = VLV_STATE_FAILED;
    myvalvemots[v].faultReason = (uint8_t) vdm::ValveFault::Short;
  }
  myvalves[0].timedLearn = 1;
  myvalves[1].retryLearn = 1;
  myvalves[2].earlyLearn = 1;
  app_loop();
  for (unsigned v : {0u, 1u, 2u}) {
    CAPTURE(v);
    CHECK(+myvalves[v].timedLearn == 0);
    CHECK(+myvalves[v].retryLearn == 0);
    CHECK(+myvalves[v].earlyLearn == 0);
    CHECK(+myvalves[v].retestRequest == 0);
    CHECK(+myvalvemots[v].status == VLV_STATE_UNKNOWN);
  }
}

TEST_CASE("calibration records: an uncalibrated result is stored without flags") {
  begin();
  myvalvemots[0].calibrated = 0;
  myvalvemots[0].calibFailed = 0;
  myvalvemots[0].calibSeq = (uint8_t) (myvalvemots[0].calibSeq + 1);
  app_loop();
  CHECK(stub::callsOf("eeprom_store_calib") == stub::Calls{"eeprom_store_calib(0)"});
  CHECK(stub::eeprom.lastCalib.flags == 0);
}

TEST_CASE("retry K2: no countdown while a test or calibration of the valve is requested") {
  begin();
  app_lease_configure(0);
  for (unsigned v : {0u, 1u, 2u, 3u}) myvalvemots[v].status = VLV_STATE_BLOCKS;
  myvalves[0].retestRequest = RETEST_TEST;
  myvalves[1].forcedLearn = 1;
  myvalvemots[2].calibration = true;
  app_1s_tick(1);
  app_1s_tick(3600);
  CHECK(+myvalves[3].retryLearn == 1);
  CHECK(+myvalves[0].retryLearn == 0);
  CHECK(+myvalves[1].retryLearn == 0);
  CHECK(+myvalves[2].retryLearn == 0);
}
