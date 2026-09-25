#include "vdm/temp_filter.h"

namespace vdm {

int32_t rawToTenths(int16_t raw128) {
  // raw128 * 10 / 128 = raw128 * 5 / 64
  const int32_t x = static_cast<int32_t>(raw128) * 5;
  return x >= 0 ? (x + 32) / 64 : -((32 - x) / 64);
}

int32_t filterTemperature(TempTrack& t, int16_t raw128) {
  const bool good = raw128 > kTempRawDisconnected && (raw128 != kTempRaw85C || t.last >= kTemp85MinPrevious);
  if (good) {
    t.last = rawToTenths(raw128);
    t.haveLast = true;
    t.missed = 0;
    return t.last;
  }
  if (t.errors < UINT16_MAX) ++t.errors;
  if (t.haveLast && t.missed < kTempHoldCycles) {
    ++t.missed;
    return t.last;
  }
  return kTempFailedTenths;
}

}  // namespace vdm
