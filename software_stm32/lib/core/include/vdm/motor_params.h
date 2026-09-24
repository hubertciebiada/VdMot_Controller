// Motor parameters (smotc/gmotc), their one validated range table and the
// loader used for the values stored in the EEPROM. Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

struct ParamRange {
  uint16_t min;
  uint16_t max;
  uint16_t def;  // used when a stored value is out of range

  constexpr bool contains(uint32_t v) const { return v >= min && v <= max; }
};

// One table for smotc, the EEPROM load at start-up and gmotx.
//  - end-stop factors are tenths (17 = 1.7 x mean current), web page 0.5..5.0
//  - startOnPower is the position in % assumed and targeted after a start
//  - minCounts is the minimum number of pulses of a calibration stroke
//  - maxRetries is the number of calibration repetitions before BLOCKS
constexpr ParamRange kLowFacRange{5, 50, 17};
constexpr ParamRange kHighFacRange{5, 50, 17};
constexpr ParamRange kStartOnPowerRange{0, 100, 30};
constexpr ParamRange kMinCountsRange{0, 60000, 3000};
constexpr ParamRange kMaxRetriesRange{0, 2, 2};

struct MotorParams {
  uint8_t lowFac;
  uint8_t highFac;
  uint8_t startOnPower;
  uint16_t minCounts;
  uint8_t maxRetries;
};

constexpr MotorParams kMotorParamsDefault{
    static_cast<uint8_t>(kLowFacRange.def), static_cast<uint8_t>(kHighFacRange.def),
    static_cast<uint8_t>(kStartOnPowerRange.def), kMinCountsRange.def,
    static_cast<uint8_t>(kMaxRetriesRange.def)};

// True if every field is inside its range.
bool motorParamsValid(const MotorParams& p);

// Replaces every out-of-range field by its default (EEPROM load).
MotorParams sanitizeMotorParams(const MotorParams& p);

// A parsed smotc request: the first three values are mandatory, minCounts and
// maxRetries are optional (argc 3..5) and keep their current value if absent.
// Returns false, and leaves `inOut` untouched, if argc is outside 3..5 or any
// supplied value is outside its range.
bool applyMotorParamsRequest(MotorParams& inOut, uint8_t argc, const uint32_t (&values)[5]);

}  // namespace vdm
