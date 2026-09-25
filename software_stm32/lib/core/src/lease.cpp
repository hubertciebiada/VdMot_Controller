#include "vdm/lease.h"

namespace vdm {

bool leaseTimeoutValid(uint32_t minutes) {
  return minutes == kLeaseTimeoutOff || (minutes >= kLeaseTimeoutMinMin && minutes <= kLeaseTimeoutMaxMin);
}

uint16_t sanitizeLeaseTimeout(uint16_t minutes) {
  return leaseTimeoutValid(minutes) ? minutes : kLeaseTimeoutDefaultMin;
}

}  // namespace vdm
