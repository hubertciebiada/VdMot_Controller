// Smoke tests of src/i2c_bus.cpp (glue_i2c_bus): the bus recovery clocks SCL while a slave holds
// SDA low (at most 9 clocks), then sends a STOP; i2c_bus_restart() is empty in wave 0.
#include "glue_test.h"
#include "hardware.h"
#include "i2c_bus.h"

using fake::Ev;

namespace {

// SDA is held low by a slave until SCL has been pulled low `clocks` times.
void holdSdaFor(unsigned clocks) {
  fake::board.input = [clocks](uint32_t pin) {
    if (pin != I2C_SDA_PIN) return -1;
    unsigned lows = 0;
    for (const fake::Event& e : fake::board.events) {
      if (e.kind == Ev::Write && e.pin == I2C_SCL_PIN && e.value == LOW) lows++;
    }
    return lows < clocks ? 0 : 1;
  };
}

unsigned sclClocks() {
  unsigned highs = 0;
  for (const fake::Event& e : fake::eventsOf(Ev::Write, I2C_SCL_PIN)) highs += e.value == HIGH;
  // the first HIGH presets the latch, the last one is the STOP
  return highs - 2;
}

fake::Event ev(Ev kind, uint32_t pin, uint32_t value) { return {0, kind, pin, value, 0}; }

}  // namespace

TEST_CASE("i2c_bus_recover: two clocks free the bus, then a STOP; whole pin sequence with 5 us half clocks") {
  glue::begin();
  holdSdaFor(2);
  i2c_bus_recover();
  const std::vector<fake::Event> expected = {
      ev(Ev::Mode, I2C_SDA_PIN, INPUT),
      ev(Ev::Write, I2C_SCL_PIN, HIGH),
      ev(Ev::Mode, I2C_SCL_PIN, OUTPUT_OPEN_DRAIN),
      ev(Ev::DelayUs, 0, 5),
      ev(Ev::Write, I2C_SCL_PIN, LOW),
      ev(Ev::DelayUs, 0, 5),
      ev(Ev::Write, I2C_SCL_PIN, HIGH),
      ev(Ev::DelayUs, 0, 5),
      ev(Ev::Write, I2C_SCL_PIN, LOW),
      ev(Ev::DelayUs, 0, 5),
      ev(Ev::Write, I2C_SCL_PIN, HIGH),
      ev(Ev::DelayUs, 0, 5),
      // STOP: SDA rises while SCL is high
      ev(Ev::Write, I2C_SCL_PIN, LOW),
      ev(Ev::Write, I2C_SDA_PIN, LOW),
      ev(Ev::Mode, I2C_SDA_PIN, OUTPUT_OPEN_DRAIN),
      ev(Ev::DelayUs, 0, 5),
      ev(Ev::Write, I2C_SCL_PIN, HIGH),
      ev(Ev::DelayUs, 0, 5),
      ev(Ev::Write, I2C_SDA_PIN, HIGH),
      ev(Ev::DelayUs, 0, 5),
      ev(Ev::Mode, I2C_SDA_PIN, INPUT),
      ev(Ev::Mode, I2C_SCL_PIN, INPUT),
  };
  CHECK(fake::board.events == expected);
  CHECK(fake::board.nowUs == 40);
}

TEST_CASE("i2c_bus_recover: a free bus gets no clock, a stuck one at most 9") {
  const unsigned held[] = {0, 1, 8, 9, 10, 20};
  const unsigned clocks[] = {0, 1, 8, 9, 9, 9};
  for (size_t i = 0; i < sizeof held / sizeof held[0]; i++) {
    glue::begin();
    CAPTURE(held[i]);
    holdSdaFor(held[i]);
    i2c_bus_recover();
    CHECK(sclClocks() == clocks[i]);
    CHECK(fake::board.mode[I2C_SDA_PIN] == INPUT);
    CHECK(fake::board.mode[I2C_SCL_PIN] == INPUT);
  }
}

TEST_CASE("i2c_bus_restart: does nothing in wave 0") {
  glue::begin();
  i2c_bus_restart();
  CHECK(fake::board.events.empty());
  CHECK(Wire.begins == 0);
}
