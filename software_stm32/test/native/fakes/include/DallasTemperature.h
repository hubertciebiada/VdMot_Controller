// Fake of the DallasTemperature library (lib/Arduino-Temperature-Control-Library) on the devices of
// fake::oneWireBus: begin() enumerates like the library (every valid address counts, the DS18
// family sets the resolution), getTemp() returns the raw value (1/128 degC) of a present device or
// DEVICE_DISCONNECTED_RAW (absent, or one of its failReads). Glue-facing: no STL.
#pragma once

#include <stdint.h>

#include "OneWire.h"

#define DEVICE_DISCONNECTED_C -127
#define DEVICE_DISCONNECTED_F -196.6
#define DEVICE_DISCONNECTED_RAW -7040

typedef uint8_t DeviceAddress[8];

class DallasTemperature {
 public:
  explicit DallasTemperature(OneWire* w) : wire(w) {}
  void begin(void);
  uint8_t getDeviceCount(void) { return devices; }
  bool validAddress(const uint8_t* address);
  bool validFamily(const uint8_t* address);
  uint8_t getResolution() { return bitResolution; }
  void setWaitForConversion(bool wait) { waitForConversion = wait; }
  void requestTemperatures(void);
  int16_t getTemp(const uint8_t* address);
  float getTempC(const uint8_t* address);
  static uint16_t millisToWaitForConversion(uint8_t resolution);

  // ---- fake state
  OneWire* wire;
  uint8_t devices = 0;
  uint8_t bitResolution = 9;
  bool waitForConversion = true;
  unsigned begins = 0;
  unsigned requests = 0;
  unsigned reads = 0;  // getTemp() calls
};
