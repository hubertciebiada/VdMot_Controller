#include "vdm/protection_guard.h"

namespace vdm {

bool ProtectionGuard::onTrip(uint8_t valve, uint32_t uptimeS) {
  if (valve >= kValveCount) return suspended_;
  lastTripS_[valve] = uptimeS;
  tripped_ = static_cast<uint16_t>(tripped_ | (1u << valve));
  uint8_t recent = 0;
  for (uint8_t v = 0; v < kValveCount; v++) {
    if ((tripped_ & (1u << v)) != 0 && uptimeS - lastTripS_[v] < kProtectWindowS) recent++;
  }
  if (recent >= kProtectTripValves) suspended_ = true;
  return suspended_;
}

}  // namespace vdm
