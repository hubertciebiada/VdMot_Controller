#include "vdm/fault_retry.h"

namespace vdm {

uint32_t faultRetryInterval(uint8_t attempts) {
  if (attempts == 0) return kFaultRetryFirstS;
  if (attempts == 1) return kFaultRetrySecondS;
  return kFaultRetryRepeatS;
}

bool FaultRetry::update(bool faulted, bool busy, uint32_t elapsedS) {
  if (busy) {
    scheduled_ = false;
    return false;
  }
  if (!faulted) {
    attempts_ = 0;
    scheduled_ = false;
    return false;
  }
  if (!scheduled_) {
    scheduled_ = true;
    remainingS_ = faultRetryInterval(attempts_);
    return false;
  }
  if (elapsedS < remainingS_) {
    remainingS_ -= elapsedS;
    return false;
  }
  scheduled_ = false;
  if (attempts_ < 255) attempts_++;
  return true;
}

FaultRetry::Snapshot FaultRetry::snapshot() const { return Snapshot{attempts_, scheduled_, remainingS()}; }

void FaultRetry::restore(const Snapshot& s) {
  attempts_ = s.attempts;
  scheduled_ = s.scheduled;
  remainingS_ = s.remainingS;
}

}  // namespace vdm
