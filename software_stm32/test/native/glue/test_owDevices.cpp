// Smoke tests of src/owDevices.cpp (glue_owDevices) on the fake 1-Wire bus: enumeration, one
// temperature cycle, the lock and search commands, the JSON of the terminal.
#include <stdio.h>

#include "glue_test.h"
#include "hardware.h"
#include "owDevices.h"
#include "stub_app.h"
#include "stub_ds2438.h"

extern OneWire oneWire;

namespace {

std::string joinedAddress(const uint8_t* rom, char separator) {
  std::string out;
  char b[3];
  for (int i = 0; i < 8; i++) {
    snprintf(b, sizeof b, "%02x", rom[i]);
    if (i > 0) out += separator;
    out += b;
  }
  return out;
}

std::string hexAddress(const uint8_t* rom) { return joinedAddress(rom, ' '); }
std::string dashAddress(const uint8_t* rom) { return joinedAddress(rom, '-'); }

// runs temperature_loop() until the cycle reports its end, at most n calls; the number of calls
unsigned cycle(unsigned n) {
  for (unsigned i = 1; i <= n; i++) {
    temperature_loop();
    if (!stub::callsOf("app_temp_cycle_done").empty()) return i;
  }
  return 0;
}

}  // namespace

TEST_CASE("temperature_setup: DS18 sensors and DS2438 monitors in search order, others counted only") {
  glue::begin();
  fake::OneWireDevice& t0 = fake::addOneWire(0x28, 1);
  fake::OneWireDevice& bad = fake::addOneWire(0x28, 2);
  bad.rom[7] ^= 0xFF;
  fake::addOneWire(0x01, 3);
  fake::OneWireDevice& v0 = fake::addOneWire(0x26, 4);
  fake::OneWireDevice& t1 = fake::addOneWire(0x10, 5);
  temperature_setup();
  CHECK(noOfDevices == 4);
  CHECK(noOfDS18Devices == 2);
  CHECK(noOfDS2438Devices == 1);
  CHECK(memcmp(tempsensors[0].address, t0.rom, 8) == 0);
  CHECK(memcmp(tempsensors[1].address, t1.rom, 8) == 0);
  CHECK(memcmp(voltsensors[0].address, v0.rom, 8) == 0);
  CHECK(tempsensors[0].temperature == -500);
  CHECK(tempsensors[2].address[0] == 0);
  CHECK(sensors.begins == 1);
  CHECK_FALSE(sensors.waitForConversion);
  CHECK(stub::calls == stub::Calls{"DS2438::begin(3)"});
}

TEST_CASE("temperature_loop: request, 95 waits, one sensor per call, the DS2438, then the cycle is done") {
  glue::begin();
  fake::OneWireDevice& t0 = fake::addOneWire(0x28, 1, 2752);   // 21.5 degC
  fake::OneWireDevice& t1 = fake::addOneWire(0x28, 2, -672);   // -5.25 degC
  fake::OneWireDevice& v0 = fake::addOneWire(0x26, 3);
  temperature_setup();
  stub::ds2438.vad[stub::ds2438Key(v0.rom)] = 4.5f;
  stub::calls.clear();
  CHECK(cycle(200) == 104);
  CHECK(sensors.requests == 1);
  CHECK(tempsensors[0].temperature == 215);
  CHECK(tempsensors[1].temperature == -53);
  CHECK(voltsensors[0].vad == 450);
  CHECK(stub::calls == stub::Calls{"DS2438::setAddress(" + dashAddress(v0.rom) + ")", "DS2438::readVAD()",
                                   "app_temp_cycle_done()"});
  CHECK(sensors.reads == 2);
  (void)t0;
  (void)t1;
}

TEST_CASE("temperature_loop: no request while locked, a search re-enumerates and matches the sensors") {
  glue::begin();
  fake::addOneWire(0x28, 1);
  temperature_setup();
  temp_command(TEMP_CMD_LOCK);
  CHECK(temp_locked());
  for (int i = 0; i < 10; i++) temperature_loop();
  CHECK(sensors.requests == 0);
  temp_command(TEMP_CMD_UNLOCK);
  CHECK_FALSE(temp_locked());
  temperature_loop();  // T_IDLE -> T_REQUEST
  CHECK(sensors.requests == 0);
  temperature_loop();
  CHECK(sensors.requests == 1);
  // a search waits for the end of the cycle
  fake::addOneWire(0x28, 2);
  temp_command(TEMP_CMD_NEWSEARCH);
  stub::calls.clear();
  CHECK(cycle(200) != 0);
  for (int i = 0; i < 4; i++) temperature_loop();
  CHECK(noOfDS18Devices == 2);
  CHECK(stub::callsOf("app_match_sensors").size() == 1);
  CHECK(ow_scan_age_s() == 0);
}

TEST_CASE("setDeviceAddress: at most 64 search passes, at most 34 DS18 sensors, stale entries cleared") {
  glue::begin();
  for (int i = 0; i < 70; i++) fake::addOneWire(0x28, static_cast<uint8_t>(i));
  setDeviceAddress();
  CHECK(oneWire.searches == 64);
  CHECK(noOfDevices == 64);
  CHECK(noOfDS18Devices == MAXONEWIRECNT);
  fake::oneWireBus.devices.resize(1);
  tempsensors[0].temperature = 200;
  setDeviceAddress();
  CHECK(noOfDS18Devices == 1);
  CHECK(tempsensors[0].temperature == 200);  // the same sensor keeps its value
  CHECK(tempsensors[1].address[0] == 0);
  CHECK(tempsensors[1].temperature == -500);
}

TEST_CASE("print_sensordata: the JSON of the terminal command getone") {
  glue::begin();
  fake::OneWireDevice& a = fake::addOneWire(0x28, 0xAB, 2752);
  fake::OneWireDevice& b = fake::addOneWire(0x28, 0x01, -672);
  temperature_setup();
  tempsensors[0].temperature = 215;
  tempsensors[1].temperature = -53;
  print_sensordata(Serial6);
  CHECK(fake::takeTx(Serial6) == "{\"cnt\":2,\"sns\":[{\"temp\":215,\"add\":\"" + hexAddress(a.rom) +
                                     "\"},{\"temp\":-53,\"add\":\"" + hexAddress(b.rom) + "\"}]}\r\n");
  fake::oneWireBus.devices.clear();
  setDeviceAddress();
  print_sensordata(Serial6);
  CHECK(fake::takeTx(Serial6) == "{\"cnt\":0}\r\n");
}
