#include "vdm/temp_refresh.h"

namespace vdm {

void TempRefresh::cycleDone(uint32_t nowMs) {
  lastCycleMs_ = nowMs;
  periodStartMs_ = nowMs;
  holding_ = false;
}

bool TempRefresh::due(uint32_t nowMs) const { return nowMs - periodStartMs_ >= kTempRefreshMs; }

bool TempRefresh::holdCommands(uint32_t nowMs) {
  if (!due(nowMs)) return false;
  if (!holding_) {
    holding_ = true;
    holdStartMs_ = nowMs;
  }
  if (nowMs - holdStartMs_ < kTempHoldMaxMs) return true;
  // the cycle did not complete in time: go on and try again one period later
  holdTimedOut(nowMs);
  return false;
}

void TempRefresh::holdTimedOut(uint32_t nowMs) {
  holding_ = false;
  periodStartMs_ = nowMs;
}

}  // namespace vdm
