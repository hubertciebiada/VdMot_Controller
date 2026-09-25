// Tests of src/app.cpp (glue_app): start values, configuration load, the valve walk of app_loop,
// the 10 s countdowns, the setters and the soft reset. Protocol 3 (lease, failsafe, retries, warm
// restore, stop, protection guard) in test_app_v3.cpp.
#include "glue_test.h"
#include "stub_eeprom.h"
#include "stub_motor.h"
#include "stub_owDevices.h"
#include "stub_terminal.h"

namespace {

void begin() {
  glue::begin();
  app_setup();
  stub::calls.clear();
  fake::takeTx(Serial6);
}

}  // namespace

TEST_CASE("app_setup: start values of all 12 valves, the stored configuration loaded") {
  glue::begin();
  CHECK(app_setup() == 0);
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    CAPTURE(v);
    CHECK(+myvalvemots[v].target_position == 50);
    CHECK(+myvalvemots[v].actual_position == 50);
    CHECK(+myvalvemots[v].status == VLV_STATE_UNKNOWN);
    CHECK(+myvalvemots[v].calibration == 0);
    CHECK(+myvalves[v].sensorindex1 == VALVE_SENSOR_UNKNOWN);
    CHECK(+myvalves[v].sensorindex2 == VALVE_SENSOR_UNKNOWN);
    CHECK(+myvalves[v].rejectedTarget == VALVE_NO_TARGET);
    // the learn time is spread over the valves; the stored learn movements 0 switch the trigger off
    CHECK(+myvalves[v].learn_time == LEARN_AFTER_TIME_DEFAULT / ACTUATOR_COUNT * (v + 1));
    CHECK(+myvalves[v].learn_movements == 0);
  }
  CHECK(learning_movements == 0);
  // an all-zero EEPROM mirror: the factors are out of range and load their defaults
  CHECK(stub::calls == stub::Calls{"motor_set_params(17, 17, 0, 0, 0)", "motor_set_escalation(0, 25, 50)",
                                   "eeprom_lease_source()"});
  CHECK(fake::takeTx(Serial6) == "Read 1-wire sensor addresses from eeprom\r\nlearning_movements: 0\r\n");
}

TEST_CASE("app_match_sensors: stored sensor addresses select the index of the found sensor") {
  glue::begin();
  const uint8_t other[8] = {0x28, 9, 9, 9, 9, 9, 9, 0x11};
  const uint8_t a[8] = {0x28, 1, 2, 3, 4, 5, 6, 0x77};
  noOfDS18Devices = 2;
  memcpy(tempsensors[0].address, other, 8);
  memcpy(tempsensors[1].address, a, 8);
  eep_content.owsensors2[4].familycode = 0x28;
  for (int i = 0; i < 6; i++) eep_content.owsensors2[4].romcode[i] = static_cast<uint8_t>(i + 1);
  eep_content.owsensors2[4].crc = 0x77;
  CHECK(app_match_sensors() == 0);
  CHECK(+myvalves[4].sensorindex2 == 1);
  CHECK(+myvalves[4].sensorindex1 == VALVE_SENSOR_UNKNOWN);
  CHECK(+myvalves[3].sensorindex2 == VALVE_SENSOR_UNKNOWN);
}

TEST_CASE("app_loop: tests the unknown valves in the order 0 2 4 .. 10 1 3 .. 11 0") {
  begin();
  for (int i = 0; i < 13; i++) app_loop();
  CHECK(stub::callsOf("appsetaction") ==
        stub::Calls{"appsetaction(x, 0, 0, 0)", "appsetaction(x, 2, 0, 0)", "appsetaction(x, 4, 0, 0)",
                    "appsetaction(x, 6, 0, 0)", "appsetaction(x, 8, 0, 0)", "appsetaction(x, 10, 0, 0)",
                    "appsetaction(x, 1, 0, 0)", "appsetaction(x, 3, 0, 0)", "appsetaction(x, 5, 0, 0)",
                    "appsetaction(x, 7, 0, 0)", "appsetaction(x, 9, 0, 0)", "appsetaction(x, 11, 0, 0)",
                    "appsetaction(x, 0, 0, 0)"});
  CHECK(+myvalves[0].rejectedTarget == 50);
}

TEST_CASE("app_loop: nothing while the valve machine is busy or the terminal drives a motor") {
  begin();
  stub::motor.idle = false;
  app_loop();
  stub::motor.idle = true;
  stub::terminal.manualActive = true;
  app_loop();
  CHECK(stub::callsOf("appsetaction").empty());
  CHECK(stub::calls == stub::Calls{"terminal_manual_active()", "valve_idle()", "terminal_manual_active()"});
}

TEST_CASE("app_loop: a known valve away from its target moves by the difference") {
  begin();
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    myvalvemots[v].status = VLV_STATE_IDLE;
    myvalvemots[v].calibrated = 1;
  }
  myvalvemots[0].target_position = 70;
  myvalvemots[1].target_position = 100;
  myvalvemots[2].target_position = 20;
  myvalvemots[3].target_position = 0;
  for (int i = 0; i < 4; i++) app_loop();
  CHECK(stub::callsOf("appsetaction") == stub::Calls{"appsetaction(o, 0, 20, 0)", "appsetaction(p, 1, 0, 0)",
                                                     "appsetaction(c, 2, 30, 0)", "appsetaction(v, 3, 0, 0)"});
}

TEST_CASE("app_10s_loop: the learn time counts down 10 s per call and triggers at <= 10") {
  begin();
  app_set_learntime(120);
  CHECK(+myvalves[0].learn_time == 10);
  CHECK(+myvalves[11].learn_time == 120);
  app_10s_loop(10);
  CHECK(+myvalves[0].timedLearn == 1);
  CHECK(+myvalves[0].learn_time == 120);
  CHECK(+myvalves[11].learn_time == 110);
  CHECK(+myvalves[11].timedLearn == 0);
}

TEST_CASE("app_10s_loop: the movement trigger starts a calibration of a connected valve only") {
  begin();
  app_set_learnmovements(50);
  myvalves[2].learn_movements = 0;
  myvalves[3].learn_movements = 0;
  myvalvemots[2].connected = 1;
  app_10s_loop(10);
  CHECK(+myvalvemots[2].calibration == 1);
  CHECK(+myvalvemots[2].calibState == calibStarted);
  CHECK(+myvalves[2].learn_movements == 50);
  CHECK(+myvalvemots[3].calibration == 0);
}

TEST_CASE("setters: valve 11 and 255 are taken, 12 is refused") {
  begin();
  CHECK(app_set_valveopen(11) == 0);
  CHECK(+myvalvemots[11].target_position == 100);
  CHECK(+myvalves[11].openRequest == 1);
  CHECK(app_set_valveopen(12) == -1);
  CHECK(app_set_valveopen(255) == 0);
  CHECK(+myvalvemots[0].target_position == 100);
  CHECK(app_set_valvelearning(12) == -1);
  CHECK(app_set_valvelearning(11) == 0);
  CHECK(+myvalves[11].forcedLearn == 1);
  CHECK(app_service_move(12, 0, 100, 20) == -1);
  CHECK(app_service_move(11, 0, 100, 20) == -3);  // the learn request of valve 11 is pending
  CHECK(app_service_move(10, 0, 100, 20) == 0);
  CHECK(stub::callsOf("appsetservice") == stub::Calls{"appsetservice(10, 0, 100, 20)"});
}

TEST_CASE("reset_check: the soft reset waits for the EEPROM, then resets the controller") {
  begin();
  reset_STM32();
  stub::eeprom.free = false;
  app_loop();
  stub::eeprom.free = true;
  CHECK_THROWS_AS(app_loop(), fake::SystemReset);
  CHECK(fake::takeTx(Serial6) ==
        "prepare for soft reset\r\nApp: valve 0 unknown, try to find out...\r\nsoft reset now\r\n");
}
