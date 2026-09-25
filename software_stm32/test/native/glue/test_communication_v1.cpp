// Tests of src/communication.cpp (glue_communication): the v1 and v2 replies keep the bytes of
// firmware 2.0.0 (58632d6) for every command, valid and invalid arguments, against stub values.
#include <string.h>

#include "communication.h"
#include "glue_test.h"
#include "stub_app.h"
#include "stub_eeprom.h"
#include "stub_motor.h"
#include "stub_owDevices.h"
#include "stub_sysstat.h"

namespace {

void begin() {
  glue::begin();
  Serial1.begin(115200, SERIAL_8E1);
  communication_setup();
  fake::takeTx(Serial1);
  fake::takeTx(Serial6);
  stub::calls.clear();
}

std::string request(const std::string& line) {
  fake::inject(Serial1, line);
  communication_loop();
  return fake::takeTx(Serial1);
}

const uint8_t kRomA[8] = {0x28, 0x84, 0x37, 0x94, 0x97, 0xFF, 0x03, 0x23};
const uint8_t kRomB[8] = {0x28, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07};
const char* kTextA = "28-84-37-94-97-ff-03-23";
const char* kTextB = "28-01-02-03-04-05-06-07";

void twoSensors() {
  memcpy(tempsensors[0].address, kRomA, 8);
  memcpy(tempsensors[1].address, kRomB, 8);
  tempsensors[0].temperature = 215;
  tempsensors[1].temperature = -53;
  noOfDS18Devices = 2;
}

}  // namespace

TEST_CASE("v1: gonec and goned") {
  begin();
  CHECK(request("gonec\n") == "gonec 0 \r\n");
  CHECK(request("gonec 255\n") == "gonec 0 \r\n");
  twoSensors();
  CHECK(request("gonec\n") == "gonec 2 \r\n");
  CHECK(request("gonec 255\n") == std::string("gonec 2 ") + kTextA + "," + kTextB + " \r\n");
  CHECK(request("gonec 3\n").empty());
  CHECK(request("goned 1\n") == std::string("goned ") + kTextB + " -53 \r\n");
  CHECK(request("goned 34\n") == "goned 0 \r\n");
  CHECK(request("goned\n") == "goned 0 \r\n");
}

TEST_CASE("v1: gowvc and gowvd") {
  begin();
  memcpy(voltsensors[0].address, kRomA, 8);
  voltsensors[0].vad = 450;
  noOfDS2438Devices = 1;
  CHECK(request("gowvc\n") == "gowvc 1 \r\n");
  CHECK(request("gowvc 255\n") == std::string("gowvc 1 ") + kTextA + " \r\n");
  CHECK(request("gowvd 0\n") == std::string("gowvd ") + kTextA + " 450 \r\n");
  CHECK(request("gowvd 8\n") == "gowvd 0 \r\n");
}

TEST_CASE("v1: gvlon for one valve, all valves and an invalid index") {
  begin();
  twoSensors();
  myvalves[1].sensorindex1 = 0;
  myvalves[1].sensorindex2 = VALVE_SENSOR_UNKNOWN;
  CHECK(request("gvlon 1\n") == std::string("gvlon 1 ") + kTextA + " 00-00-00-00-00-00-00-00 \r\n");
  const std::string all = request("gvlon 255\n");
  CHECK(all.rfind("gvlon 12 ", 0) == 0);
  CHECK(all.size() == 9 + 12 * 47 + 11 + 3);
  CHECK(request("gvlon 12\n") == "goned error \r\n");
}

TEST_CASE("v1: stons, stlnt, stlnm, gtlnm, staop, staln") {
  begin();
  CHECK(request("stons\n") == "stons\r\n");
  CHECK(request("stlnt 100\n") == "stlnt\r\n");
  CHECK(request("stlnm 100\n") == "stlnm\r\n");
  CHECK(request("gtlnm\n") == "gtlnm 2000 \r\n");
  CHECK(request("staop 255\n") == "staop \r\n");
  CHECK(request("staln 3\n") == "staln\r\n");
  stub::app.setValveOpen = -1;
  stub::app.setValveLearning = -1;
  stub::app.setLearnMovements = -1;
  CHECK(request("staop 20\n").empty());
  CHECK(request("staln 20\n").empty());
  CHECK(request("stlnm 100\n").empty());
  CHECK(request("stlnt\n").empty());
  CHECK(stub::calls == stub::Calls{"temp_command(1)", "app_set_learntime(100)", "eeprom_changed(0x0010)",
                                   "app_set_learnmovements(100)",
                                   "eeprom_changed(0x0002)", "app_set_valveopen(255)", "app_set_valvelearning(3)",
                                   "app_set_valveopen(20)", "app_set_valvelearning(20)", "app_set_learnmovements(100)"});
}

TEST_CASE("v1: smotc and gmotc") {
  begin();
  CHECK(request("gmotc\n") == "gmotc 17 17 50 3000 0 \r\n");
  CHECK(request("smotc 18 19 40\n") == "smotc\r\n");
  CHECK(request("smotc 18 19 40\n") == "smotc\r\n");
  CHECK(stub::callsOf("eeprom_changed") == stub::Calls{"eeprom_changed(0x0004)"});
  CHECK(request("smotc 18 19\n") == "smotc err\r\n");
  CHECK(request("smotc 18 99 40\n") == "smotc err\r\n");
}

TEST_CASE("v1: ghwin, masns, eepst, gvlst, reset") {
  begin();
  CHECK(request("ghwin\n") == "ghwin 1059 \r\n");
  CHECK(request("masns\n") == "masns \r\n");
  CHECK(request("eepst\n") == "eepst 1 \r\n");
  stub::eeprom.state = vdm::kEepStatePending;
  CHECK(request("eepst\n") == "eepst 0 \r\n");
  myvalvemots[0].status = 8;
  myvalvemots[11].status = 6;
  CHECK(request("gvlst\n") == "gvlst 12 8,0,0,0,0,0,0,0,0,0,0,6 \r\n");
  CHECK(request("reset\n") == "reset \r\n");
  CHECK(stub::callsOf("reset_STM32") == stub::Calls{"reset_STM32()"});
}

TEST_CASE("v2: gprof, svmov, scalx, gcalx, gmotx") {
  begin();
  CHECK(request("gprof 0\n") == "gprof 0 0\r\n");
  CHECK(request("gprof 12\n").empty());
  CHECK(request("svmov 2 1 100 20\n") == "svmov 2 ok\r\n");
  stub::app.serviceMove = -3;
  CHECK(request("svmov 2 1 100 20\n") == "svmov 2 err 3\r\n");
  stub::app.serviceMove = -2;
  CHECK(request("svmov 2 1 100 20\n") == "svmov 2 err 2\r\n");
  CHECK(request("svmov 2 1 0 20\n") == "svmov 2 err 1\r\n");
  CHECK(request("svmov 12 1 100 20\n") == "svmov -1 err 1\r\n");
  CHECK(request("scalx 1 30 40\n") == "scalx ok\r\n");
  CHECK(request("scalx 2 30 40\n") == "scalx err\r\n");
  CHECK(request("gcalx\n") == "gcalx 1 30 40\r\n");
  CHECK(request("gmotx\n").rfind("gmotx ", 0) == 0);
  CHECK(request("ESPalive\n").empty());
}
