#include "vdm/net_policy.h"

#include <algorithm>

#include "vdm/common.h"

namespace vdm {

const char* netEvidenceName(NetEvidence e) {
  switch (e) {
    case NetEvidence::None: return "none";
    case NetEvidence::GatewayPing: return "ping";
    case NetEvidence::Mqtt: return "mqtt";
    case NetEvidence::TimeSync: return "ntp";
    case NetEvidence::InboundHttp: return "http";
    case NetEvidence::DhcpLease: return "dhcp";
  }
  return "unknown";
}

void NetWatchdog::configure(uint8_t minutes) {
  minutes_ = minutes;
  fired_ = false;
}

uint32_t NetWatchdog::waitMs() const {
  uint32_t min = minutes_;  // <= 255, below the cap
  for (uint8_t i = 0; i < restarts_; ++i) min = std::min<uint32_t>(min * kGrowth, kMaxWaitMin);
  return min * 60000u;
}

bool NetWatchdog::update(bool netUp, uint32_t nowMs) {
  if (netUp) {
    down_ = false;
    fired_ = false;
    restarts_ = 0;
    return false;
  }
  if (!started_ || !down_) {
    // Boot (the first call) or the moment the network was lost.
    started_ = true;
    down_ = true;
    downSinceMs_ = nowMs;
  }
  if (minutes_ == 0 || fired_) return false;
  if (elapsedMs(nowMs, downSinceMs_) < waitMs()) return false;
  fired_ = true;
  if (restarts_ < UINT8_MAX) ++restarts_;
  return true;
}

}  // namespace vdm
