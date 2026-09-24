// Targets that a failed or blocked valve does not execute (S04). Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

// rejectedTarget value: no target recorded (the valve was driven since).
constexpr uint8_t kNoRejectedTarget = 255;

// Called for a failed or blocked valve whose position differs from its target.
// The first target seen after the valve was last driven is the one the fault
// left behind (the target of the move that timed out, the target kept through
// a blocked calibration): it is only recorded. Every later, different target
// is a change the ESP requested; it is recorded and counted in cmdRejected
// (saturating). Returns true for such a change.
bool rejectTarget(uint8_t& rejectedTarget, uint16_t& cmdRejected, uint8_t target);

}  // namespace vdm
