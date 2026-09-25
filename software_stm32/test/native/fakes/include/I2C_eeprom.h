// Fake of robtillaart/I2C_EEPROM 1.9.4 on the 24LC64 of the controller: fake::eeprom (fake_board.h)
// holds the 8 KB (erased = 0xFF), the transfer log and the fault injection. writeBlock() returns 0
// or a Wire error code, readBlock() the number of bytes read (0 when the transfer fails).
// Glue-facing: no STL.
#pragma once

#include <stdint.h>

#include "Wire.h"

class I2C_eeprom {
 public:
  I2C_eeprom(const uint8_t address, TwoWire* wire = &Wire);
  I2C_eeprom(const uint8_t address, const uint32_t size, TwoWire* wire = &Wire);
  bool begin(int8_t writeProtectPin = -1);
  bool isConnected();
  int writeBlock(const uint16_t memoryAddress, const uint8_t* buffer, const uint16_t length);
  uint16_t readBlock(const uint16_t memoryAddress, uint8_t* buffer, const uint16_t length);
  int writeByte(const uint16_t memoryAddress, const uint8_t value);
  uint8_t readByte(const uint16_t memoryAddress);
  uint32_t getDeviceSize() const { return deviceSize; }

  // ---- fake state
  uint8_t deviceAddress;
  uint32_t deviceSize;
  TwoWire* bus;
};
