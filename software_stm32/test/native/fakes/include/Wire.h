// Fake of the core's Wire (TwoWire): records the pin setup and begin/end (fake::Ev::WireBegin,
// WireEnd). The EEPROM transfers are in I2C_eeprom.h. Glue-facing: no STL.
#pragma once

#include <stddef.h>
#include <stdint.h>

class TwoWire {
 public:
  void setSDA(uint32_t pin) { sda = pin; }
  void setSCL(uint32_t pin) { scl = pin; }
  void begin();
  void end();
  void beginTransmission(uint8_t address) { lastAddress = address; }
  size_t write(uint8_t) { return 1; }
  uint8_t endTransmission() { return 0; }

  // ---- fake state
  uint32_t sda = 0xFFFFFFFF;
  uint32_t scl = 0xFFFFFFFF;
  unsigned begins = 0;
  unsigned ends = 0;
  uint8_t lastAddress = 0;
  bool running = false;
};

extern TwoWire Wire;
