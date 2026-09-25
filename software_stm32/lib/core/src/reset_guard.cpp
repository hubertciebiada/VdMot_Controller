#include "vdm/reset_guard.h"

#include <stddef.h>

#include "vdm/legacy_layout.h"

namespace vdm {

namespace {

uint16_t guardCrc(const ResetGuardCell& c) {
  return crc16Ccitt(reinterpret_cast<const uint8_t*>(&c), offsetof(ResetGuardCell, crc));
}

void seal(ResetGuardCell& c) {
  c.magic = kResetGuardMagic;
  c.pad = 0;
  c.crc = guardCrc(c);
}

void clearWindow(ResetGuardCell& c) {
  c.count = 0;
  c.safe = 0;
  c.windowS = 0;
}

bool isWatchdog(BootReason r) { return r == BootReason::IndependentWatchdog || r == BootReason::WindowWatchdog; }

bool isCold(BootReason r) { return r == BootReason::PowerOn || r == BootReason::BrownOut || r == BootReason::Unknown; }

}  // namespace

bool resetGuardOnBoot(ResetGuardCell& c, BootReason reason) {
  if (isCold(reason) || c.magic != kResetGuardMagic || c.crc != guardCrc(c)) {
    clearWindow(c);
  } else {
    const uint32_t sum = c.windowS + c.lastUptimeS;  // wraps on overflow
    c.windowS = sum < c.windowS ? UINT32_MAX : sum;
    if (isWatchdog(reason)) {
      if (c.count == 0 || c.windowS > kSafeModeWindowS) {
        c.count = 1;
        c.windowS = 0;
      } else if (c.count < 255) {
        c.count++;
      }
    }
    if (c.count >= kSafeModeResets) c.safe = 1;
  }
  c.lastUptimeS = 0;
  seal(c);
  return c.safe != 0;
}

bool resetGuardAlive(ResetGuardCell& c, uint32_t uptimeS) {
  c.lastUptimeS = uptimeS;
  if (c.safe != 0 && uptimeS >= kSafeModeExitS) clearWindow(c);
  seal(c);
  return c.safe != 0;
}

void resetGuardClear(ResetGuardCell& c) {
  clearWindow(c);
  seal(c);
}

}  // namespace vdm
