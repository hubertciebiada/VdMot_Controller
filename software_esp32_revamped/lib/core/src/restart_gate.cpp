#include "vdm/restart_gate.h"

#include "vdm/common.h"

namespace vdm {

RestartGate::Step RestartGate::update(StmSaveState s, uint32_t nowMs) {
  if (done_) return Step::Proceed;
  if (!started_) {
    started_ = true;
    startMs_ = nowMs;
    return Step::RequestStmSave;
  }
  if (s == StmSaveState::Saved || s == StmSaveState::Unavailable || s == StmSaveState::TimedOut) {
    done_ = true;
    return Step::Proceed;
  }
  if (elapsedMs(nowMs, startMs_) < kGuardMs) return Step::Wait;
  done_ = true;
  guardExpired_ = true;
  return Step::Proceed;
}

uint32_t RestartGate::waitedMs(uint32_t nowMs) const {
  return started_ ? elapsedMs(nowMs, startMs_) : 0;
}

void RestartGate::reset() { *this = RestartGate(); }

}  // namespace vdm
