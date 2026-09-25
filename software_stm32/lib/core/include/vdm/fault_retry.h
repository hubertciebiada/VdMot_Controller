// Automatic retry of a failed (4) or blocked (9) valve: a calibration (or a
// presence test after a short) 1 h after the fault, then 6 h, then every
// 24 h. Hardware-free; time comes in as elapsed seconds.
#pragma once

#include <stdint.h>

namespace vdm {

constexpr uint32_t kFaultRetryFirstS = 3600;
constexpr uint32_t kFaultRetrySecondS = 21600;
constexpr uint32_t kFaultRetryRepeatS = 86400;

// 0 -> kFaultRetryFirstS, 1 -> kFaultRetrySecondS, >= 2 -> kFaultRetryRepeatS
uint32_t faultRetryInterval(uint8_t attempts);

class FaultRetry {
 public:
  // faulted: status 4 or 9. busy: a calibration or presence test of the valve
  // is requested or running. Returns true once when a retry is due.
  //   busy                       -> nothing scheduled (the retry or another
  //                                 calibration runs), attempts kept, false
  //   !faulted                   -> reset (attempts 0), false
  //   faulted, nothing scheduled -> schedule faultRetryInterval(attempts), false
  //   scheduled, elapsed >= rest -> unschedule, attempts + 1 (saturating), true
  //   scheduled                  -> rest - elapsed, false
  bool update(bool faulted, bool busy, uint32_t elapsedS);

  bool scheduled() const { return scheduled_; }
  uint32_t remainingS() const { return scheduled_ ? remainingS_ : 0; }
  uint8_t attempts() const { return attempts_; }

  struct Snapshot {
    uint8_t attempts;
    bool scheduled;
    uint32_t remainingS;
  };
  Snapshot snapshot() const;
  void restore(const Snapshot& s);

 private:
  uint8_t attempts_ = 0;
  bool scheduled_ = false;
  uint32_t remainingS_ = 0;
};

}  // namespace vdm
