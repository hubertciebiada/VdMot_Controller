// Temperatures of the DS18B20 sensors as the firmware reports them (goned, gvlvd). Hardware-free:
// the glue reads the raw register value (1/128 degC, DallasTemperature::getTemp(), retried once
// when the read failed) and reports what filterTemperature() makes of it:
// - a good read in 0.1 degC, rounded like round(getTempC() * 10) of firmware 2.0.0;
// - a failed read keeps the last good value for kTempHoldCycles cycles, then kTempFailedTenths;
// - 85.0 degC is the power-on value of the DS18B20 register (a sensor that lost its supply during
//   the conversion): it counts as a read only after a reading of at least 75.0 degC.
#pragma once

#include <stdint.h>

namespace vdm {

constexpr int16_t kTempRawDisconnected = -7040;  // DallasTemperature DEVICE_DISCONNECTED_RAW
constexpr int16_t kTempRaw85C = 10880;           // 85.0 degC in 1/128 degC
constexpr int32_t kTemp85MinPrevious = 750;      // 0.1 degC
constexpr uint8_t kTempHoldCycles = 2;
constexpr int32_t kTempFailedTenths = -1270;     // a failed read (round(-127.0 * 10))

// 1/128 degC -> 0.1 degC, rounded half away from zero
int32_t rawToTenths(int16_t raw128);

// one sensor; a new sensor at the index starts with a new track
struct TempTrack {
  int32_t last = 0;      // 0.1 degC, the last good read
  bool haveLast = false;
  uint8_t missed = 0;    // failed reads since the last good one
  uint16_t errors = 0;   // failed reads, saturating
};

// the value to report for raw128 (after the retry of a failed read)
int32_t filterTemperature(TempTrack& t, int16_t raw128);

}  // namespace vdm
