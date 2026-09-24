// Detects a state machine that stays in one busy state for too long. Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

class StallDetector {
 public:
  explicit constexpr StallDetector(uint32_t limitTicks) : limit_(limitTicks) {}

  // Call once per state machine tick with the current state. Being idle or
  // entering a different state counts as progress and restarts the count.
  void tick(uint8_t state, bool idle) {
    if (idle || !busy_ || state != state_) {
      busy_ = !idle;
      state_ = state;
      age_ = 0;
    } else if (age_ < limit_) {
      ++age_;
    }
  }

  // True once the same busy state was seen on `limitTicks` further ticks.
  bool stalled() const { return busy_ && age_ >= limit_; }

 private:
  uint32_t limit_;
  uint32_t age_ = 0;
  uint8_t state_ = 0;
  bool busy_ = false;
};

}  // namespace vdm
