#include "vdm/stall_detector.h"

namespace vdm {

void StallDetector::tick(uint8_t state, bool idle) {
  if (idle || !busy_ || state != state_) {
    busy_ = !idle;
    state_ = state;
    age_ = 0;
  } else if (age_ < limit_) {  // NOMUTATE: `<=` saturates at limit_ + 1, which stalled() (>=) cannot tell apart (the wrap at UINT32_MAX takes 2^32 ticks)
    ++age_;
  }
}

bool StallDetector::stalled() const { return busy_ && age_ >= limit_; }

}  // namespace vdm
