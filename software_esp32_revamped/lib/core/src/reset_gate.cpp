#include "vdm/reset_gate.h"

#include "vdm/common.h"

namespace vdm {

void ResetGate::begin(uint32_t nowMs, bool stmAnswers) {
  state_ = stmAnswers ? State::Waiting : State::Ready;
  beganMs_ = nowMs;
  inFlight_ = false;
  polledOnce_ = false;
  lastPollMs_ = nowMs;
}

bool ResetGate::pollDue(uint32_t nowMs, bool requestsPending) const {
  if (state_ != State::Waiting || inFlight_ || requestsPending) return false;
  return !polledOnce_ || elapsedMs(nowMs, lastPollMs_) >= kPollMs;
}

void ResetGate::onPollSent(uint32_t nowMs) {
  (void)nowMs;
  inFlight_ = true;
  polledOnce_ = true;
}

void ResetGate::onEepst(bool answered, bool idle, uint32_t nowMs) {
  inFlight_ = false;
  lastPollMs_ = nowMs;
  if (state_ == State::Waiting && answered && idle) state_ = State::Ready;
}

ResetGate::State ResetGate::update(uint32_t nowMs) {
  if (state_ == State::Waiting && elapsedMs(nowMs, beganMs_) >= kMaxWaitMs) {
    state_ = State::TimedOut;
  }
  return state_;
}

uint32_t ResetGate::waitedMs(uint32_t nowMs) const {
  return state_ == State::Idle ? 0 : elapsedMs(nowMs, beganMs_);
}

// Idle: pollDue() is false whatever the poll flags say; begin() sets them.
void ResetGate::reset() { state_ = State::Idle; }

}  // namespace vdm
