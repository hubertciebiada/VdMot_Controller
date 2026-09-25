// mqtt_values edges: the first and last temperature slot, a sensor on bus index 0,
// one-character names and topics, calibration ends of valve 0 and out-of-range valves.
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/mqtt_values.h"

using namespace vdm;

namespace {

OneWireId sensorId(uint8_t last) {
  OneWireId o;
  o.b[0] = 0x28;
  o.b[7] = last;
  return o;
}

std::string segment(const Config& c, ItemKind k, uint8_t i, int bus) {
  char out[16];
  memset(out, 'X', sizeof out);
  const size_t n = sensorTopicSegment(c, k, i, bus, out, sizeof out);
  CHECK(n == strlen(out));
  return out;
}

}  // namespace

TEST_CASE("slotTempTenths: the first and the last slot, the first bus entry") {
  static Config c;
  c = Config{};
  c.temps[0].id = sensorId(1);
  c.temps[kTempSlotCount - 1].id = sensorId(2);
  c.temps[kTempSlotCount - 1].offset = 3;
  TempReading t[2];
  t[0].id = sensorId(1);
  t[0].seen = true;
  t[0].raw = 200;
  t[1].id = sensorId(2);
  t[1].seen = true;
  t[1].raw = 150;
  int32_t tenths = 0;
  CHECK(slotTempTenths(c, t, 2, 1, 0, 60000, tenths));
  CHECK(tenths == 200);
  CHECK(slotTempTenths(c, t, 2, kTempSlotCount, 0, 60000, tenths));
  CHECK(tenths == 153);
}

TEST_CASE("slotVoltValue: a sensor on the first bus entry") {
  static Config c;
  c = Config{};
  c.volts[0].id = sensorId(4);
  VoltReading v[1];
  v[0].id = sensorId(4);
  v[0].seen = true;
  v[0].vad = 500;
  double value = 0;
  CHECK(slotVoltValue(c, v, 1, 0, 0, 60000, value));
  CHECK(value == doctest::Approx(5.0));
}

TEST_CASE("findTempBus: a match on bus index 0") {
  TempReading t[2];
  t[0].id = sensorId(7);
  CHECK(findTempBus(t, 2, sensorId(7)) == 0);
}

TEST_CASE("sensorTopicSegment: a one-character name or topic wins over the bus index") {
  static Config c;
  c = Config{};
  copyString(c.temps[0].name, sizeof c.temps[0].name, "A");
  CHECK(segment(c, ItemKind::Temp, 0, 4) == "A");
  copyString(c.temps[1].topic, sizeof c.temps[1].topic, "B");
  CHECK(segment(c, ItemKind::Temp, 1, 4) == "B");
}

TEST_CASE("CalibEndTracker: the end of valve 0, out-of-range valves read valve 0") {
  CalibEndTracker t;
  ValveState v[kValveCount];
  LocalTime a;
  a.valid = true;
  a.epoch = 100;
  LocalTime b = a;
  b.epoch = 200;
  v[0].calibrating = true;
  CHECK(t.observe(v, a) == 0);
  v[0].calibrating = false;
  CHECK(t.observe(v, b) == 0x0001);
  CHECK(t.end(0).epoch == 200);
  CHECK(t.end(1).epoch == 0);
  CHECK(t.end(kValveCount).epoch == 200);
  CHECK(t.end(255).epoch == 200);
}
