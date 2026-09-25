// Fake of the OneWire library (lib/OneWire): the devices are fake::oneWireBus (fake_board.h);
// search() returns the present devices in the order of the list, crc8() is the Dallas CRC of
// lib/core (vdm::crc8). Glue-facing: no STL.
#pragma once

#include <stdint.h>

class OneWire {
 public:
  OneWire() {}
  explicit OneWire(uint8_t p) { begin(p); }
  void begin(uint8_t p) { pin = p; }
  uint8_t reset();
  void select(const uint8_t rom[8]);
  void skip() {}
  void write(uint8_t, uint8_t = 0) {}
  uint8_t read() { return 0xFF; }
  void reset_search() { searchPos = 0; }
  bool search(uint8_t* newAddr, bool searchMode = true);
  static uint8_t crc8(const uint8_t* addr, uint8_t len);

  // ---- fake state
  uint8_t pin = 0;
  unsigned searchPos = 0;
  unsigned searches = 0;  // search() calls
  uint8_t selected[8] = {};
};
