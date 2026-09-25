// Edge cases of src/owDevices.cpp (glue_owDevices): the state before temperature_setup(), the debug
// address format, a repeated setup, too many DS2438, the cycle and search lengths, one sensor in JSON.
#include <stdio.h>

#include "glue_test.h"
#include "hardware.h"
#include "owDevices.h"
#include "stub_app.h"
#include "stub_ds2438.h"

namespace {

unsigned cycleCalls(unsigned n) {
  for (unsigned i = 1; i <= n; i++) {
    temperature_loop();
    if (!stub::callsOf("app_temp_cycle_done").empty()) return i;
  }
  return 0;
}

}  // namespace

TEST_CASE("owDevices: before temperature_setup no device, no scan age") {
  glue::begin();
  CHECK(noOfDevices == 0);
  CHECK(noOfDS18Devices == 0);
  CHECK(noOfDS2438Devices == 0);
  CHECK(ow_scan_age_s() == 0);
  fake::advanceMs(1000);
  CHECK(ow_scan_age_s() == 1);
  print_sensordata(Serial6);
  CHECK(fake::takeTx(Serial6) == "{\"cnt\":0}\r\n");
}

TEST_CASE("owDevices: a cycle without temperature_setup reads no DS2438") {
  glue::begin();
  CHECK(cycleCalls(200) != 0);
  CHECK(stub::calls == stub::Calls{"app_temp_cycle_done()"});
}

TEST_CASE("printAddress: the eight bytes in hex, one leading zero below 0x10") {
  glue::begin();
  Serial6.begin(115200);
  DeviceAddress a = {0x28, 0x0F, 0x10, 0x00, 0xAB, 0x01, 0xFF, 0x05};
  printAddress(a);
  CHECK(fake::takeTx(Serial6) == "{ 28,  0F,  10,  00,  AB,  01,  FF,  05 }");
}

TEST_CASE("temperature_setup again: the known sensors start over without a temperature") {
  glue::begin();
  fake::addOneWire(0x28, 1, 2752);
  fake::addOneWire(0x28, 2, 2752);
  temperature_setup();
  tempsensors[0].temperature = 215;
  tempsensors[1].temperature = 215;
  temperature_setup();
  CHECK(tempsensors[0].temperature == -500);
  CHECK(tempsensors[1].temperature == -500);
}

TEST_CASE("setDeviceAddress: at most 8 DS2438, a vanished one is cleared") {
  glue::begin();
  for (int i = 0; i < MAXDS2438CNT + 1; i++) fake::addOneWire(0x26, static_cast<uint8_t>(i + 1));
  setDeviceAddress();
  CHECK(noOfDS2438Devices == MAXDS2438CNT);
  CHECK(noOfDevices == MAXDS2438CNT + 1);
  voltsensors[0].vad = 450;
  fake::oneWireBus.devices.clear();
  setDeviceAddress();
  CHECK(noOfDS2438Devices == 0);
  for (int k = 0; k < 8; k++) CHECK(voltsensors[0].address[k] == 0);
  CHECK(voltsensors[0].vad == 0);
}

TEST_CASE("temperature_loop: a cycle without DS2438 ends after the last temperature") {
  glue::begin();
  fake::addOneWire(0x28, 1, 2752);
  temperature_setup();
  stub::calls.clear();
  CHECK(cycleCalls(200) == 101);
  CHECK(stub::calls == stub::Calls{"app_temp_cycle_done()"});
}

TEST_CASE("temperature_loop: a search takes three calls after the idle one, counts are 0 meanwhile") {
  glue::begin();
  fake::addOneWire(0x28, 1);
  fake::addOneWire(0x26, 2);
  temperature_setup();
  REQUIRE(noOfDS18Devices == 1);
  REQUIRE(noOfDS2438Devices == 1);
  temperature_loop();  // T_INIT
  temp_command(TEMP_CMD_NEWSEARCH);
  stub::calls.clear();
  temperature_loop();  // T_IDLE -> T_SEARCH
  temperature_loop();  // begin
  CHECK(noOfDS18Devices == 0);
  CHECK(noOfDS2438Devices == 0);
  temperature_loop();  // device count
  CHECK(stub::callsOf("app_match_sensors").empty());
  temperature_loop();  // enumeration
  CHECK(stub::callsOf("app_match_sensors").size() == 1);
  CHECK(noOfDS18Devices == 1);
}

TEST_CASE("print_sensordata: one sensor is a list of one") {
  glue::begin();
  fake::OneWireDevice& a = fake::addOneWire(0x28, 0x01);
  temperature_setup();
  tempsensors[0].temperature = 215;
  print_sensordata(Serial6);
  char add[3 * 8];
  snprintf(add, sizeof add, "%02x %02x %02x %02x %02x %02x %02x %02x", a.rom[0], a.rom[1], a.rom[2], a.rom[3],
           a.rom[4], a.rom[5], a.rom[6], a.rom[7]);
  CHECK(fake::takeTx(Serial6) == std::string("{\"cnt\":1,\"sns\":[{\"temp\":215,\"add\":\"") + add + "\"}]}\r\n");
}
