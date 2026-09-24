// Targets that a failed or blocked valve does not execute (S04). Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

// rejectedTarget value: no target known (the valve was not driven since its
// status was reset).
constexpr uint8_t kNoRejectedTarget = 255;

// rejectedTarget holds the last target that is not a new request: the target
// of the last move or calibration handed to the valve state machine (the
// caller records it at the handover), the target a failed or blocked valve
// already stands at, or the last target counted here.
//
// Called for a failed or blocked valve (whose motor is not driven):
//  - target == actual: the valve is where it is asked to be; nothing is
//    rejected, and the target is recorded, so a later different target
//    counts even if it is the one the fault left behind.
//  - target == rejectedTarget: already known, nothing changes.
//  - otherwise the target is a change the valve does not execute: it is
//    recorded and counted in cmdRejected (saturating). Only when no target is
//    known (kNoRejectedTarget) it is taken as the one the fault left behind and
//    only recorded.
// Returns true for a counted change.
bool rejectTarget(uint8_t& rejectedTarget, uint16_t& cmdRejected, uint8_t target, uint8_t actual);

}  // namespace vdm
