#include "vdm/motor_params.h"

#include "vdm/calibration.h"
#include "vdm/end_stop_detector.h"

namespace vdm {

// a factor above the table maximum cannot change where a move stops (kFacRequestMax)
static_assert(static_cast<int32_t>(kMeanCurrentFloor_mA) * kLowFacRange.max >= EndStopDetector::kSafetyLimit &&
                  static_cast<int32_t>(kMeanCurrentFloor_mA) * kHighFacRange.max >= EndStopDetector::kSafetyLimit,
              "the largest end-stop factor must reach the safety limit");
static_assert(kFacRequestMax >= kLowFacRange.max && kFacRequestMax >= kHighFacRange.max && kFacRequestMax <= 0xFF,
              "factor request limit");

namespace {

template <typename T>
T pick(const ParamRange& r, T v) {
  return r.contains(v) ? v : static_cast<T>(r.def);
}

// stores v in field if it is inside r
template <typename T>
bool take(const ParamRange& r, uint32_t v, T& field) {
  if (!r.contains(v)) return false;
  field = static_cast<T>(v);
  return true;
}

// an end-stop factor: like take(), a value in (max, kFacRequestMax] is stored as max
bool takeFactor(const ParamRange& r, uint32_t v, uint8_t& field) {
  if (v > r.max && v <= kFacRequestMax) v = r.max;
  return take(r, v, field);
}

}  // namespace

bool motorParamsValid(const MotorParams& p) {
  return kLowFacRange.contains(p.lowFac) && kHighFacRange.contains(p.highFac) &&
         kStartOnPowerRange.contains(p.startOnPower) && kMinCountsRange.contains(p.minCounts) &&
         kMaxRetriesRange.contains(p.maxRetries);
}

bool sameMotorParams(const MotorParams& a, const MotorParams& b) {
  return a.lowFac == b.lowFac && a.highFac == b.highFac && a.startOnPower == b.startOnPower &&
         a.minCounts == b.minCounts && a.maxRetries == b.maxRetries;
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

ParamsRequest applyMotorParamsRequest(MotorParams& inOut, uint8_t argc, const uint32_t (&values)[5]) {
  if (argc < 3 || argc > 5) return ParamsRequest::Rejected;

  bool all = takeFactor(kLowFacRange, values[0], inOut.lowFac);
  all = takeFactor(kHighFacRange, values[1], inOut.highFac) && all;
  all = take(kStartOnPowerRange, values[2], inOut.startOnPower) && all;
  if (argc >= 4) all = take(kMinCountsRange, values[3], inOut.minCounts) && all;
  if (argc == 5) all = take(kMaxRetriesRange, values[4], inOut.maxRetries) && all;
  return all ? ParamsRequest::Applied : ParamsRequest::Partial;
}

}  // namespace vdm
