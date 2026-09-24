// Calibration decisions: end-stop thresholds, breakaway escalation, the
// verdict on the counts of a calibration pass and mean current learning.
// Hardware-free. Currents: mean currents in mA, bounds in 0.1 mA.
#pragma once

#include <stdint.h>

namespace vdm {

// Floor for the mean current an end-stop threshold is derived from.
constexpr uint16_t kMeanCurrentFloor_mA = 15;
// Mean current assumed for a valve that was never calibrated.
constexpr uint16_t kMeanCurrentDefault_mA = 20;
// A stroke must have at least this many mean current samples (one every
// ~0.5 s) before its mean is learned.
constexpr uint16_t kMinMeanSamples = 4;
// A stroke shorter than this is never accepted: scaler = counts / 100 must not be 0.
constexpr uint32_t kMinTravelCounts = 100;
// Absolute end-stop limit of the motor current detection (the safety limit).
constexpr uint8_t kSafetyLimit_mA = 60;

// End-stop threshold in 0.1 mA for a mean current and a factor in tenths:
// max(mean, floor) * factor.
int32_t endStopBound(uint16_t meanCurrent_mA, uint8_t factor);

// Breakaway escalation: on calibration repetition n (n >= 1) the threshold
// grows by n * stepPct percent, but never beyond maxmA (which itself is capped
// by the 60 mA safety limit). A threshold that already is above the cap is
// left as it is.
struct EscalationConfig {
  uint8_t enable;  // 0 / 1
  uint8_t stepPct;  // 0..100
  uint8_t maxmA;   // 20..60
};

constexpr uint8_t kEscalationStepMax = 100;
constexpr uint8_t kEscalationMaxmAMin = 20;
constexpr uint8_t kEscalationMaxmAMax = kSafetyLimit_mA;
constexpr EscalationConfig kEscalationDefault{0, 25, 50};

bool escalationValid(const EscalationConfig& c);
// Out-of-range config (e.g. from an EEPROM image) -> default.
EscalationConfig sanitizeEscalation(const EscalationConfig& c);

int32_t escalatedBound(int32_t bound, uint8_t repetition, const EscalationConfig& c);

enum class CalibrationVerdict : uint8_t {
  Accept,   // store counts, scaler and mean current
  Retry,    // repeat the calibration, keep the previous values
  Blocked,  // repetitions exhausted: BLOCKS, keep the previous values
};

// failedBefore: failed passes of this calibration before this one.
// A pass fails if either stroke has fewer than max(minCounts, kMinTravelCounts)
// pulses; after maxRetries repetitions the valve is blocked.
CalibrationVerdict evaluateCalibration(uint32_t openingCount, uint32_t closingCount,
                                       uint16_t minCounts, uint8_t failedBefore,
                                       uint8_t maxRetries);

// Mean current of one stroke: average of `samples` values in 0.1 mA whose sum
// is `sum`, in mA. 0 without samples.
uint16_t strokeMeanCurrent(int32_t sum, uint16_t samples);

// Mean current after a successful pass: strokes with fewer than
// kMinMeanSamples samples or a mean of 0 are ignored; the remaining are
// averaged; without any the previous value stays.
uint16_t learnMeanCurrent(uint16_t previous_mA, uint16_t openMean_mA, uint16_t openSamples,
                          uint16_t closeMean_mA, uint16_t closeSamples);

}  // namespace vdm
