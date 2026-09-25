#include "vdm/ota_policy.h"

#include "vdm/common.h"

namespace vdm {

OtaValidator::OtaValidator(uint32_t confirmMs, uint32_t networkOnlyMs, uint32_t giveUpMs)
    : confirmMs_(confirmMs), networkOnlyMs_(networkOnlyMs), giveUpMs_(giveUpMs) {}

void OtaValidator::begin(bool pendingVerify, uint32_t nowMs) {
  pending_ = pendingVerify;
  startMs_ = nowMs;
  healthy_ = false;
  healthySinceMs_ = nowMs;
}

OtaValidator::Decision OtaValidator::update(bool netUp, bool linkUp, uint32_t nowMs) {
  if (!pending_) return Decision::NotPending;
  const bool healthy = netUp && linkUp;
  if (healthy && !healthy_) healthySinceMs_ = nowMs;
  healthy_ = healthy;
  const uint32_t uptime = elapsedMs(nowMs, startMs_);
  if ((healthy_ && elapsedMs(nowMs, healthySinceMs_) >= confirmMs_) ||
      (netUp && uptime >= networkOnlyMs_)) {
    pending_ = false;
    return Decision::MarkValid;
  }
  if (uptime >= giveUpMs_) {
    pending_ = false;
    return Decision::Rollback;
  }
  return Decision::Wait;
}

bool OtaValidator::confirmBeforeRestart(bool userRequested, bool netUp) {
  if (!pending_ || !userRequested || !netUp) return false;
  pending_ = false;
  return true;
}

}  // namespace vdm
