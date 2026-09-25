#include "vdm/failsafe.h"

#include "vdm/valve_codes.h"

namespace vdm {

bool failsafePctValid(uint32_t v) { return v <= 100 || v == kFailsafeHold; }

uint8_t sanitizeFailsafePct(uint8_t v) { return failsafePctValid(v) ? v : kFailsafeDefaultPct; }

Drive driveTarget(uint8_t target, uint8_t failsafePct, uint8_t status, bool leaseExpired, bool assemblyHold) {
  if (failsafePct == kFailsafeHold) return Drive{target, DriveSource::Target};
  if (status == kStBlocked) return Drive{failsafePct, DriveSource::BlockedFailsafe};
  if (status == kStFailed || status == kStOpenCircuit) return Drive{target, DriveSource::Target};
  if (leaseExpired && !assemblyHold) return Drive{failsafePct, DriveSource::LeaseFailsafe};
  return Drive{target, DriveSource::Target};
}

}  // namespace vdm
