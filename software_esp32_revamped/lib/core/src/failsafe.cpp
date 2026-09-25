#include "vdm/failsafe.h"

#include "vdm/config.h"

namespace vdm {

bool leaseTimeoutValid(uint32_t minutes) {
  return minutes == 0 || (minutes >= kFailsafeTimeoutMinMin && minutes <= kFailsafeTimeoutMaxMin);
}

bool failsafePctValid(uint32_t pct) { return pct <= 100 || pct == kFailsafeHold; }

const char* leaseStateName(LeaseState s) {
  switch (s) {
    case LeaseState::Off: return "off";
    case LeaseState::Running: return "running";
    case LeaseState::Expired: return "expired";
  }
  return "unknown";
}

const char* leaseModeName(LeaseMode m) {
  switch (m) {
    case LeaseMode::None: return "none";
    case LeaseMode::Stm: return "stm";
    case LeaseMode::Emulated: return "esp";
  }
  return "unknown";
}

const char* haStatusName(HaStatus s) {
  switch (s) {
    case HaStatus::Unknown: return "unknown";
    case HaStatus::Online: return "online";
    case HaStatus::Offline: return "offline";
  }
  return "unknown";
}

const char* regulatorCauseName(RegulatorCause c) {
  switch (c) {
    case RegulatorCause::Alive: return "alive";
    case RegulatorCause::BrokerDown: return "broker_down";
    case RegulatorCause::HaOffline: return "ha_offline";
  }
  return "unknown";
}

RegulatorCause regulatorCause(const RegulatorInput& in) {
  if (in.mode == MqttMode::Off) return RegulatorCause::Alive;
  if (!in.brokerConnected) return RegulatorCause::BrokerDown;
  if (in.mode == MqttMode::MqttHa && in.ha == HaStatus::Offline) return RegulatorCause::HaOffline;
  return RegulatorCause::Alive;
}

const char* failsafeKindName(FailsafeKind k) {
  switch (k) {
    case FailsafeKind::None: return "off";
    case FailsafeKind::Lease: return "lease";
    case FailsafeKind::Blocked: return "blocked";
  }
  return "unknown";
}

}  // namespace vdm
