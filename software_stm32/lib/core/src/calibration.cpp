#include "vdm/calibration.h"

namespace vdm {

int32_t endStopBound(uint16_t meanCurrent_mA, uint8_t factor) {
  const uint16_t mean = meanCurrent_mA < kMeanCurrentFloor_mA ? kMeanCurrentFloor_mA : meanCurrent_mA;
  return static_cast<int32_t>(mean) * factor;
}

bool escalationValid(const EscalationConfig& c) {
  return c.enable <= 1 && c.stepPct <= kEscalationStepMax && c.maxmA >= kEscalationMaxmAMin &&
         c.maxmA <= kEscalationMaxmAMax;
}

EscalationConfig sanitizeEscalation(const EscalationConfig& c) {
  return escalationValid(c) ? c : kEscalationDefault;
}

int32_t escalatedBound(int32_t bound, uint8_t repetition, const EscalationConfig& c) {
  if (!c.enable || repetition == 0 || c.stepPct == 0 || bound <= 0) return bound;

  const int32_t cap = static_cast<int32_t>(c.maxmA > kSafetyLimit_mA ? kSafetyLimit_mA : c.maxmA) * 10;
  if (bound >= cap) return bound;

  const int64_t grown = static_cast<int64_t>(bound) * (100 + static_cast<int32_t>(c.stepPct) * repetition) / 100;
  return grown > cap ? cap : static_cast<int32_t>(grown);
}

CalibrationVerdict evaluateCalibration(uint32_t openingCount, uint32_t closingCount,
                                       uint16_t minCounts, uint8_t failedBefore,
                                       uint8_t maxRetries) {
  const uint32_t required = minCounts > kMinTravelCounts ? minCounts : kMinTravelCounts;
  if (openingCount >= required && closingCount >= required) return CalibrationVerdict::Accept;
  // failedBefore + 1 failed passes now; blocked once they exceed the repetitions
  return failedBefore >= maxRetries ? CalibrationVerdict::Blocked : CalibrationVerdict::Retry;
}

uint16_t strokeMeanCurrent(int32_t sum, uint16_t samples) {
  if (samples == 0) return 0;
  const int64_t magnitude = sum < 0 ? -static_cast<int64_t>(sum) : sum;
  const int64_t mean = magnitude / samples / 10;
  return mean > 0xFFFF ? 0xFFFF : static_cast<uint16_t>(mean);
}

uint16_t learnMeanCurrent(uint16_t previous_mA, uint16_t openMean_mA, uint16_t openSamples,
                          uint16_t closeMean_mA, uint16_t closeSamples) {
  const bool openValid = openSamples >= kMinMeanSamples && openMean_mA > 0;
  const bool closeValid = closeSamples >= kMinMeanSamples && closeMean_mA > 0;

  if (openValid && closeValid) {
    return static_cast<uint16_t>((static_cast<uint32_t>(openMean_mA) + closeMean_mA) / 2);
  }
  if (openValid) return openMean_mA;
  if (closeValid) return closeMean_mA;
  return previous_mA;
}

}  // namespace vdm
