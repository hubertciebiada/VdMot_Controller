#include "vdm/target_rejection.h"

namespace vdm {

bool rejectTarget(uint8_t& rejectedTarget, uint16_t& cmdRejected, uint8_t target) {
  if (rejectedTarget == target) return false;

  const bool change = rejectedTarget != kNoRejectedTarget;
  rejectedTarget = target;
  if (change && cmdRejected < 0xFFFF) cmdRejected++;
  return change;
}

}  // namespace vdm
