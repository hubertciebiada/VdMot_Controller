// src/communication.cpp (glue_communication): the first and last valve and sensor index of every
// command that takes one, the separators of the gonec/gowvc lists, the 5-value smotc, svmov with one
// argument, the pause of reset.
#include <string.h>

#include <string>

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

bool startsWith(const std::string& s, const std::string& prefix) { return s.compare(0, prefix.size(), prefix) == 0; }

const uint8_t kAddrA[8] = {0x28, 1, 2, 3, 4, 5, 6, 0x77};
const uint8_t kAddrB[8] = {0x26, 9, 8, 7, 6, 5, 4, 0x33};

}  // namespace

TEST_CASE("stgtp/gtgtp: valve 11, positions 0 and 100; valve 12 is refused") {
  begin();
  CHECK(request("stgtp 11 100\n") == "stgtp\r\n");
  CHECK(+myvalvemots[11].target_position == 100);
  CHECK(request("stgtp 1 0\n") == "stgtp\r\n");
  CHECK(+myvalvemots[1].target_position == 0);
  CHECK(request("gtgtp 11\n") == "gtgtp 11 100 \r\n");
  CHECK(request("gtgtp 12\n").empty());
}

TEST_CASE("gvlvd, gvlvx, gprof, gvlvy: valves 0 and 11 answer, 12 does not") {
  begin();
  for (const char* cmd : {"gvlvd", "gvlvx", "gprof", "gvlvy"}) {
    CAPTURE(cmd);
    CHECK(startsWith(request(std::string(cmd) + " 0\n"), std::string(cmd) + " 0"));
    CHECK(startsWith(request(std::string(cmd) + " 11\n"), std::string(cmd) + " 11"));
  }
  CHECK(request("gvlvx 12\n").empty());
}

TEST_CASE("gonec 255: one space before the list; 254 and 256 are no list") {
  begin();
  noOfDS18Devices = 1;
  memcpy(tempsensors[0].address, kAddrA, 8);
  CHECK(request("gonec 255\n") == "gonec 1 28-01-02-03-04-05-06-77 \r\n");
  CHECK(request("gonec 254\n").empty());
  CHECK(request("gonec 256\n").empty());
  CHECK(request("gonec 0\n").empty());
}

TEST_CASE("gowvc 255: no list, a list of two; 254 and 256 are no list") {
  begin();
  CHECK(request("gowvc 255\n") == "gowvc 0 \r\n");
  noOfDS2438Devices = 2;
  memcpy(voltsensors[0].address, kAddrA, 8);
  memcpy(voltsensors[1].address, kAddrB, 8);
  CHECK(request("gowvc 255\n") == "gowvc 2 28-01-02-03-04-05-06-77,26-09-08-07-06-05-04-33 \r\n");
  CHECK(request("gowvc 254\n").empty());
  CHECK(request("gowvc 256\n").empty());
  CHECK(request("gowvc 0\n").empty());
}

TEST_CASE("goned and gowvd: the first and the last sensor index") {
  begin();
  memcpy(tempsensors[0].address, kAddrA, 8);
  memcpy(tempsensors[MAXONEWIRECNT - 1].address, kAddrB, 8);
  memcpy(voltsensors[MAXDS2438CNT - 1].address, kAddrB, 8);
  CHECK(startsWith(request("goned 0\n"), "goned 28-01-02-03-04-05-06-77 "));
  CHECK(startsWith(request("goned " + std::to_string(MAXONEWIRECNT - 1) + "\n"), "goned 26-09-"));
  CHECK(request("goned " + std::to_string(MAXONEWIRECNT) + "\n") == "goned 0 \r\n");
  CHECK(startsWith(request("gowvd " + std::to_string(MAXDS2438CNT - 1) + "\n"), "gowvd 26-09-"));
}

TEST_CASE("gvlon: valves 0 and 11, 255; 254 and 256 are errors") {
  begin();
  CHECK(startsWith(request("gvlon 0\n"), "gvlon 0 "));
  CHECK(startsWith(request("gvlon 11\n"), "gvlon 11 "));
  CHECK(startsWith(request("gvlon 255\n"), "gvlon 12 "));
  CHECK(request("gvlon 254\n") == "goned error \r\n");
  CHECK(request("gvlon 256\n") == "goned error \r\n");
}

TEST_CASE("stsnx/stvls: the first and last valve and sensor index") {
  begin();
  noOfDS18Devices = MAXONEWIRECNT;
  CHECK(request("stsnx 0 0\n") == "stsnx\r\n");
  CHECK(request("stsny 11 " + std::to_string(MAXONEWIRECNT - 1) + "\n") == "stsny\r\n");
  CHECK(request("stsnx 12 0\n").empty());
  CHECK(request("stsnx 0 " + std::to_string(MAXONEWIRECNT) + "\n").empty());
  CHECK(request("stvls 0 00-00-00-00-00-00-00-00 00-00-00-00-00-00-00-00\n") == "stvls 0\r\n");
  CHECK(request("stvls 11 00-00-00-00-00-00-00-00 00-00-00-00-00-00-00-00\n") == "stvls 11\r\n");
  CHECK(comm_set_valve_sensors(0, "", "") == 0);
  CHECK(comm_set_valve_sensors(12, "", "") == -1);
}

TEST_CASE("staop, staln, stdet: 0 and 65535 reach the handler") {
  begin();
  request("staop 0\n");
  request("staop 65535\n");
  request("staln 65535\n");
  CHECK(stub::callsOf("app_set_valveopen") == stub::Calls{"app_set_valveopen(0)", "app_set_valveopen(65535)"});
  CHECK(stub::callsOf("app_set_valvelearning") == stub::Calls{"app_set_valvelearning(65535)"});
  CHECK(request("stdet 0\n") == "stdet err\r\n");
  CHECK(request("stdet 65535\n") == "stdet err\r\n");
  CHECK(request("stdet 65536\n").empty());
}

TEST_CASE("stlnt 0 and stlnm 0 are taken") {
  begin();
  CHECK(request("stlnt 0\n") == "stlnt\r\n");
  CHECK(stub::callsOf("app_set_learntime") == stub::Calls{"app_set_learntime(0)"});
  request("stlnm 0\n");
  CHECK(stub::callsOf("app_set_learnmovements") == stub::Calls{"app_set_learnmovements(0)"});
}

TEST_CASE("smotc: five values, zeros included") {
  begin();
  CHECK(request("smotc 18 19 0 0 0\n") == "smotc\r\n");
  CHECK(request("smotc 18 19 0 0 0 0\n").empty());  // too many arguments: dropped
}

TEST_CASE("svmov: valves 0 and 11; one argument reports its valve") {
  begin();
  CHECK(request("svmov 0 1 100 20\n") == "svmov 0 ok\r\n");
  CHECK(request("svmov 11 1 100 20\n") == "svmov 11 ok\r\n");
  CHECK(request("svmov 3\n") == "svmov 3 err 1\r\n");
}

TEST_CASE("scalx: enable 0 and step 0 are taken") {
  begin();
  CHECK(request("scalx 0 0 40\n") == "scalx ok\r\n");
}

TEST_CASE("reset: the reply, 200 ms for it to leave, then the reset request") {
  begin();
  const uint32_t before = millis();
  CHECK(request("reset\n") == "reset \r\n");
  CHECK(millis() - before == 200);
  CHECK(stub::callsOf("reset_STM32") == stub::Calls{"reset_STM32()"});
}
