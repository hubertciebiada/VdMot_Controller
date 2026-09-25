#include "vdm/ota_policy.h"

#include "vdm/common.h"

namespace vdm {

OtaValidator::OtaValidator(uint32_t confirmMs, uint32_t giveUpMs)
    : confirmMs_(confirmMs), giveUpMs_(giveUpMs) {}

void OtaValidator::begin(bool pendingVerify, bool stmRequired, uint32_t nowMs) {
  pending_ = pendingVerify;
  stmRequired_ = stmRequired;
  startMs_ = nowMs;
  healthy_ = false;
  healthySinceMs_ = nowMs;
  checked_ = false;
  checkOk_ = false;
  checkMs_ = nowMs;
  missing_ = 0;
}

bool OtaValidator::selfCheckDue(bool webStarted, uint32_t nowMs) const {
  if (!pending_ || !webStarted) return false;
  return !checked_ || elapsedMs(nowMs, checkMs_) >= kSelfCheckIntervalMs;
}

void OtaValidator::onSelfCheck(bool ok, uint32_t nowMs) {
  checked_ = true;
  checkOk_ = ok;
  checkMs_ = nowMs;
  // A failed check ends the healthy period at once.
  if (!ok) healthy_ = false;
}

bool OtaValidator::httpOk(uint32_t nowMs) const {
  return checked_ && checkOk_ && elapsedMs(nowMs, checkMs_) < kHttpFreshMs;
}

OtaValidator::Decision OtaValidator::update(bool netOk, bool linkUp, uint32_t nowMs) {
  if (!pending_) return Decision::NotPending;
  missing_ = static_cast<uint8_t>((netOk ? 0 : kCheckNet) | (httpOk(nowMs) ? 0 : kCheckHttp) |
                                  (stmRequired_ && !linkUp ? kCheckStm : 0));
  const bool healthy = missing_ == 0;
  if (healthy && !healthy_) healthySinceMs_ = nowMs;
  healthy_ = healthy;
  if (healthy_ && elapsedMs(nowMs, healthySinceMs_) >= confirmMs_) {
    pending_ = false;
    return Decision::MarkValid;
  }
  if (elapsedMs(nowMs, startMs_) >= giveUpMs_) {
    pending_ = false;
    return Decision::Rollback;
  }
  return Decision::Wait;
}

bool OtaValidator::confirmBeforeRestart(bool userRequested, bool netUp, bool linkUp) {
  if (!pending_ || !userRequested || !netUp || (stmRequired_ && !linkUp)) return false;
  pending_ = false;
  return true;
}

uint32_t OtaValidator::healthyForMs(uint32_t nowMs) const {
  return pending_ && healthy_ ? elapsedMs(nowMs, healthySinceMs_) : 0;
}

uint32_t OtaValidator::remainingMs(uint32_t nowMs) const {
  if (!pending_) return 0;
  const uint32_t up = elapsedMs(nowMs, startMs_);
  return up >= giveUpMs_ ? 0 : giveUpMs_ - up;
}

bool normalizeMd5(const char* in, char* out) {
  if (in == nullptr) return false;
  char tmp[33];
  for (size_t i = 0; i < 32; ++i) {
    const char c = in[i];
    if (c >= '0' && c <= '9') {
      tmp[i] = c;
    } else if (c >= 'a' && c <= 'f') {
      tmp[i] = c;
    } else if (c >= 'A' && c <= 'F') {
      tmp[i] = static_cast<char>(c - 'A' + 'a');
    } else {
      return false;  // also the terminator of a shorter string
    }
  }
  if (in[32] != '\0') return false;
  tmp[32] = '\0';
  for (size_t i = 0; i < 33; ++i) out[i] = tmp[i];
  return true;
}

}  // namespace vdm
