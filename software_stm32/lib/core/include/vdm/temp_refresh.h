// Temperature conversions during a long series of valve moves: the valve
// state machine locks the temperature state machine while a motor runs (ADC
// interference), so a calibration series of all valves would leave the
// temperatures unchanged for tens of minutes. At least every
// kTempRefreshMs the motors pause (at most kTempHoldMaxMs) for one complete
// conversion cycle. Hardware-free; times are millis() values (wrap-safe).
#pragma once

#include <stdint.h>

namespace vdm {

constexpr uint32_t kTempRefreshMs = 60000;
constexpr uint32_t kTempHoldMaxMs = 3000;

class TempRefresh {
 public:
  // the temperature state machine completed a cycle
  void cycleDone(uint32_t nowMs);
  // the last cycle is kTempRefreshMs or more ago (and no hold of this period timed out)
  bool due(uint32_t nowMs) const;
  // while due: true until a cycle completes or kTempHoldMaxMs passed since the
  // first call of this hold; a hold that timed out comes again kTempRefreshMs later
  bool holdCommands(uint32_t nowMs);
  // a hold kept by someone else (the pause between two calibration strokes) timed out: the next
  // one comes kTempRefreshMs later
  void holdTimedOut(uint32_t nowMs);
  // seconds since the last complete cycle (since start-up if none)
  uint32_t ageS(uint32_t nowMs) const { return (nowMs - lastCycleMs_) / 1000; }

 private:
  uint32_t lastCycleMs_ = 0;
  uint32_t periodStartMs_ = 0;
  uint32_t holdStartMs_ = 0;
  bool holding_ = false;
};

}  // namespace vdm
