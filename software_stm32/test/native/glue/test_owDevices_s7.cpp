// Tests of src/owDevices.cpp (glue_owDevices), S7: a failed DS18B20 read is repeated once, a failure
// after it keeps the last good value for 2 cycles (then -1270), 85.0 degC counts only after a
// reading of at least 75.0 degC, a new sensor at an index starts without history, and the bus is
// enumerated again 24 h after the last enumeration while the temperature machine is not locked.
#include "glue_test.h"
#include "hardware.h"
#include "owDevices.h"
#include "stub_app.h"
#include "stub_ds2438.h"

namespace {

// one complete temperature cycle (at most 400 calls); true if it ended
bool cycle() {
  stub::calls.clear();
  for (unsigned i = 0; i < 400; i++) {
    temperature_loop();
    if (!stub::callsOf("app_temp_cycle_done").empty()) return true;
  }
  return false;
}

void setupOne(fake::OneWireDevice*& dev, int16_t raw) {
  dev = &fake::addOneWire(0x28, 1, raw);
  temperature_setup();
}

void advanceS(uint32_t s) { fake::board.nowUs += static_cast<uint64_t>(s) * 1000000u; }

}  // namespace

TEST_CASE("a failed read is repeated once in the same cycle") {
  glue::begin();
  fake::OneWireDevice* dev;
  setupOne(dev, 2752);
  dev->failReads = 1;
  REQUIRE(cycle());
  CHECK(tempsensors[0].temperature == 215);
  CHECK(sensors.reads == 2);
  REQUIRE(cycle());
  CHECK(sensors.reads == 3);
}

TEST_CASE("a sensor that fails: the last good value for 2 cycles, then -1270; a good read ends it") {
  glue::begin();
  fake::OneWireDevice* dev;
  setupOne(dev, 2752);
  REQUIRE(cycle());
  CHECK(tempsensors[0].temperature == 215);
  const int expected[] = {215, 215, -1270, -1270};
  for (int e : expected) {
    dev->failReads = 2;
    REQUIRE(cycle());
    CHECK(tempsensors[0].temperature == e);
  }
  dev->raw = -672;
  REQUIRE(cycle());
  CHECK(tempsensors[0].temperature == -53);
  dev->failReads = 2;
  REQUIRE(cycle());
  CHECK(tempsensors[0].temperature == -53);
}

TEST_CASE("a sensor without a good read reports -1270 at once, 85.0 degC only after 75.0 degC") {
  glue::begin();
  fake::OneWireDevice* dev;
  setupOne(dev, 10880);  // 85.0 degC: power-on value
  REQUIRE(cycle());
  CHECK(tempsensors[0].temperature == -1270);
  dev->raw = 9728;  // 76.0 degC
  REQUIRE(cycle());
  CHECK(tempsensors[0].temperature == 760);
  dev->raw = 10880;
  REQUIRE(cycle());
  CHECK(tempsensors[0].temperature == 850);
}

TEST_CASE("a different sensor at an index starts without the history of the old one") {
  glue::begin();
  fake::OneWireDevice* dev;
  setupOne(dev, 2752);
  REQUIRE(cycle());
  CHECK(tempsensors[0].temperature == 215);
  fake::oneWireBus.devices.clear();
  fake::OneWireDevice& other = fake::addOneWire(0x28, 9, 0);
  setDeviceAddress();
  other.failReads = 2;
  REQUIRE(cycle());
  CHECK(tempsensors[0].temperature == -1270);
}

TEST_CASE("the bus is enumerated again 24 h after the last enumeration, not while locked") {
  glue::begin();
  fake::OneWireDevice* dev;
  setupOne(dev, 2752);
  CHECK(ow_scan_age_s() == 0);
  advanceS(86399);
  CHECK(ow_scan_age_s() == 86399);
  REQUIRE(cycle());
  CHECK(sensors.begins == 1);
  fake::board.nowUs += 999999;
  CHECK(ow_scan_age_s() == 86399);
  fake::board.nowUs += 1;
  CHECK(ow_scan_age_s() == 86400);
  fake::addOneWire(0x28, 2, 1280);
  temp_command(TEMP_CMD_LOCK);
  for (int i = 0; i < 20; i++) temperature_loop();
  CHECK(sensors.begins == 1);
  CHECK(noOfDS18Devices == 1);
  temp_command(TEMP_CMD_UNLOCK);
  stub::calls.clear();
  for (int i = 0; i < 4; i++) temperature_loop();
  CHECK(sensors.begins == 2);
  CHECK(noOfDS18Devices == 2);
  CHECK(stub::callsOf("app_match_sensors").size() == 1);
  CHECK(ow_scan_age_s() == 0);
  // the next one 24 h later; the time counts on in whole seconds
  advanceS(3600);
  fake::board.nowUs += 1500000;
  CHECK(ow_scan_age_s() == 3601);
  fake::board.nowUs += 500000;
  CHECK(ow_scan_age_s() == 3602);
}

TEST_CASE("stons searches at once, also while the temperature machine is locked") {
  glue::begin();
  fake::OneWireDevice* dev;
  setupOne(dev, 2752);
  advanceS(90000);
  temp_command(TEMP_CMD_NEWSEARCH);
  temp_command(TEMP_CMD_LOCK);
  CHECK(temp_locked());
  for (int i = 0; i < 5; i++) temperature_loop();
  CHECK(sensors.begins == 2);
  CHECK(ow_scan_age_s() == 0);
  for (int i = 0; i < 20; i++) temperature_loop();
  CHECK(sensors.begins == 2);
}
