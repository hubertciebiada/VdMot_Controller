#include "vdm/warm_state.h"

#include "vdm/valve_codes.h"

namespace vdm {

namespace {

uint16_t warmCrc(const WarmState& s) {
  return crc16Ccitt(reinterpret_cast<const uint8_t*>(&s), offsetof(WarmState, crc));
}

bool warmValveValid(const WarmValve& w) {
  return w.actual <= 100 && w.target <= 100 && w.status >= kStIdle && w.status <= kStBlocked &&
         (w.flags & kWarmFlagsUnused) == 0 && w.retryScheduled <= 1 && w.retryRemainingS <= kWarmRetryMaxS;
}

}  // namespace

void warmStateSeal(WarmState& s) {
  s.magic = kWarmStateMagic;
  s.version = kWarmStateVersion;
  s.count = kValveCount;
  s.reserved = 0;
  s.crc = warmCrc(s);
}

bool warmStateValid(const WarmState& s) {
  return s.magic == kWarmStateMagic && s.version == kWarmStateVersion && s.count == kValveCount &&
         s.crc == warmCrc(s);
}

bool isWarmBoot(BootReason r) {
  switch (r) {
    case BootReason::Pin:
    case BootReason::Software:
    case BootReason::IndependentWatchdog:
    case BootReason::WindowWatchdog:
    case BootReason::LowPower:
      return true;
    default:
      return false;
  }
}

RestoredValve restoreValve(const WarmValve& w, bool calibrated) {
  RestoredValve r{};
  if (!warmValveValid(w)) return r;
  r.valid = true;
  r.actual = w.actual;
  r.target = w.target;
  r.status = w.status;
  r.needsReference = (w.flags & kWarmNeedsReference) != 0;
  r.recal = (w.flags & kWarmRecal) != 0;
  r.assemblyHold = (w.flags & kWarmAssemblyHold) != 0;
  const bool moving = w.status == kStOpening || w.status == kStClosing;
  if (moving || (w.flags & kWarmPosValid) == 0) {
    r.status = calibrated ? kStIdle : kStUnknown;
    r.needsReference = r.needsReference || calibrated;
  }
  return r;
}

}  // namespace vdm
