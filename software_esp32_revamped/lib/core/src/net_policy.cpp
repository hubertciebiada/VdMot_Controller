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

// ---------------------------------------------------------------- NetReachability

void NetReachability::update(bool ipUp, uint32_t gateway, uint32_t nowMs) {
  if (!started_ || ipUp != ipUp_) {
    started_ = true;
    ipUp_ = ipUp;
    upMs_ = nowMs;
    evidence_ = NetEvidence::None;
    probeSent_ = false;
    if (!ipUp) {
      lost_ = false;
      lostKnown_ = false;
    }
  }
  // While the IP is down the interface reports no gateway: that is no change.
  if (ipUp && gateway != gateway_) {
    gateway_ = gateway;
    armed_ = false;
    probeSent_ = false;
  }
}

void NetReachability::onEvidence(NetEvidence e, uint32_t nowMs) {
  if (!ipUp_ || e == NetEvidence::None) return;
  evidence_ = e;
  evidenceMs_ = nowMs;
  if (e == NetEvidence::GatewayPing) armed_ = true;
}

bool NetReachability::probeDue(uint32_t nowMs) const {
  if (!ipUp_ || gateway_ == 0) return false;
  return !probeSent_ || elapsedMs(nowMs, probeMs_) >= kProbeIntervalMs;
}

void NetReachability::onProbeSent(uint32_t nowMs) {
  probeSent_ = true;
  probeMs_ = nowMs;
}

bool NetReachability::reachable(uint32_t nowMs) const {
  if (!ipUp_) return false;
  if (!armed_) return true;
  const uint32_t since = evidence_ != NetEvidence::None ? evidenceMs_ : upMs_;
  return elapsedMs(nowMs, since) < kStaleMs;
}

uint32_t NetReachability::evidenceAgeMs(uint32_t nowMs) const {
  return evidence_ != NetEvidence::None ? elapsedMs(nowMs, evidenceMs_) : UINT32_MAX;
}

NetReachability::Change NetReachability::change(uint32_t nowMs) {
  const bool r = reachable(nowMs);
  Change c = Change::None;
  if (ipUp_) {
    if (wasReachable_ && !r) {
      lost_ = true;
      lostKnown_ = true;
      lostMs_ = nowMs;
      c = Change::Lost;
    } else if (!wasReachable_ && r && lost_) {
      lost_ = false;
      c = Change::Regained;
    }
  }
  wasReachable_ = r;
  return c;
}

uint32_t NetReachability::lostForMs(uint32_t nowMs) const {
  return lostKnown_ ? elapsedMs(nowMs, lostMs_) : 0;
}

// ---------------------------------------------------------------- NetWatchdog

void NetWatchdog::configure(uint8_t minutes) {
  minutes_ = minutes;
  ifaceFired_ = false;
  espFired_ = false;
}

uint32_t NetWatchdog::waitMs() const {
  uint32_t min = minutes_;  // <= 255, below the cap
  for (uint8_t i = 0; i < restarts_; ++i) min = std::min<uint32_t>(min * kGrowth, kMaxWaitMin);
  return min * 60000u;
}

NetWatchdog::Action NetWatchdog::update(bool reachable, uint32_t nowMs) {
  if (reachable) {
    down_ = false;
    ifaceFired_ = false;
    espFired_ = false;
    restarts_ = 0;
    return Action::None;
  }
  if (!started_ || !down_) {
    // Boot (the first call) or the moment the network was lost.
    started_ = true;
    down_ = true;
    downSinceMs_ = nowMs;
  }
  if (minutes_ == 0) return Action::None;
  const uint32_t outage = elapsedMs(nowMs, downSinceMs_);
  const uint32_t ifaceAt = minutes_ * 60000u;
  if (!espFired_ && outage >= ifaceAt + waitMs()) {
    espFired_ = true;
    ifaceFired_ = true;
    if (restarts_ < UINT8_MAX) ++restarts_;
    return Action::RestartEsp;
  }
  if (!ifaceFired_ && outage >= ifaceAt) {
    ifaceFired_ = true;
    if (ifaceRestarts_ < UINT16_MAX) ++ifaceRestarts_;
    return Action::RestartInterface;
  }
  return Action::None;
}

uint32_t NetWatchdog::outageMs(uint32_t nowMs) const {
  return started_ && down_ ? elapsedMs(nowMs, downSinceMs_) : 0;
}

}  // namespace vdm
