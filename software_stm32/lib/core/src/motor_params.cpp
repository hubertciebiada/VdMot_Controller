#include "vdm/motor_params.h"

namespace vdm {

namespace {

template <typename T>
T pick(const ParamRange& r, T v) {
  return r.contains(v) ? v : static_cast<T>(r.def);
}

}  // namespace

bool motorParamsValid(const MotorParams& p) {
  return kLowFacRange.contains(p.lowFac) && kHighFacRange.contains(p.highFac) &&
         kStartOnPowerRange.contains(p.startOnPower) && kMinCountsRange.contains(p.minCounts) &&
         kMaxRetriesRange.contains(p.maxRetries);
}

MotorParams sanitizeMotorParams(const MotorParams& p) {
  MotorParams r;
  r.lowFac = pick(kLowFacRange, p.lowFac);
  r.highFac = pick(kHighFacRange, p.highFac);
  r.startOnPower = pick(kStartOnPowerRange, p.startOnPower);
  r.minCounts = pick(kMinCountsRange, p.minCounts);
  r.maxRetries = pick(kMaxRetriesRange, p.maxRetries);
  return r;
}

bool applyMotorParamsRequest(MotorParams& inOut, uint8_t argc, const uint32_t (&values)[5]) {
  if (argc < 3 || argc > 5) return false;
  if (!kLowFacRange.contains(values[0]) || !kHighFacRange.contains(values[1]) ||
      !kStartOnPowerRange.contains(values[2])) {
    return false;
  }
  if (argc >= 4 && !kMinCountsRange.contains(values[3])) return false;
  if (argc == 5 && !kMaxRetriesRange.contains(values[4])) return false;

  inOut.lowFac = static_cast<uint8_t>(values[0]);
  inOut.highFac = static_cast<uint8_t>(values[1]);
  inOut.startOnPower = static_cast<uint8_t>(values[2]);
  if (argc >= 4) inOut.minCounts = static_cast<uint16_t>(values[3]);
  if (argc == 5) inOut.maxRetries = static_cast<uint8_t>(values[4]);
  return true;
}

}  // namespace vdm
