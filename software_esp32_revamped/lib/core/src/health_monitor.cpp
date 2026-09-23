// Stub: contract in vdm/health_monitor.h; implemented by the core implementer.
#include "vdm/health_monitor.h"

namespace vdm {

uint8_t systemState(LinkState, const ValveState*, uint8_t, uint16_t) { return 0; }

size_t HealthMonitor::onValve(uint8_t, const ValveState&, const ValveState&, bool, Event*, size_t) {
  return 0;
}
size_t HealthMonitor::onLink(LinkState, LinkState, uint8_t, Event*, size_t) { return 0; }
size_t HealthMonitor::onStmCounters(uint32_t, uint32_t, uint8_t, uint32_t, Event*, size_t) {
  return 0;
}
size_t HealthMonitor::onTempSensor(uint8_t, bool, bool, bool, int16_t, Event*, size_t) {
  return 0;
}

EventRateLimiter::EventRateLimiter(uint32_t perKeyMs, uint16_t maxPerHour)
    : perKeyMs_(perKeyMs),
      maxPerHour_(maxPerHour),
      tokensMilli_(static_cast<uint32_t>(maxPerHour) * 1000u),
      lastRefillMs_(0),
      keys_() {}
bool EventRateLimiter::allow(const Event&, uint32_t) { return false; }

OtaValidator::OtaValidator(uint32_t confirmMs, uint32_t networkOnlyMs, uint32_t giveUpMs)
    : confirmMs_(confirmMs), networkOnlyMs_(networkOnlyMs), giveUpMs_(giveUpMs) {}
void OtaValidator::begin(bool, uint32_t) {}
OtaValidator::Decision OtaValidator::update(bool, bool, uint32_t) { return Decision::NotPending; }

void NetWatchdog::configure(uint8_t) {}
bool NetWatchdog::update(bool, uint32_t) { return false; }

}  // namespace vdm
