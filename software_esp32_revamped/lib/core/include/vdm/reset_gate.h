// Wait for pending STM EEPROM writes before the ESP pulses NRST (user reset,
// flash start) or restarts itself (jumper X20 resets the STM with it): the
// STM gets up to kMaxWaitMs; the ESP lets its queued User/Config requests go
// out first, then polls `eepst` every kPollMs until "eepst 1". Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

class ResetGate {
 public:
  static constexpr uint32_t kPollMs = 500;
  static constexpr uint32_t kMaxWaitMs = 10000;
  enum class State : uint8_t { Idle, Waiting, Ready, TimedOut };

  // stmAnswers false (link not Up/Degraded): Ready at once.
  void begin(uint32_t nowMs, bool stmAnswers);
  // Waiting, no eepst in flight, no queued User/Config requests
  // (requestsPending false) and kPollMs since the last poll completed; the
  // first poll goes out at once.
  bool pollDue(uint32_t nowMs, bool requestsPending) const;
  void onPollSent(uint32_t nowMs);
  // Result of the eepst request: answered && idle -> Ready; otherwise the
  // next poll kPollMs later.
  void onEepst(bool answered, bool idle, uint32_t nowMs);
  // Waiting -> TimedOut once kMaxWaitMs passed since begin(); returns the state.
  State update(uint32_t nowMs);
  State state() const { return state_; }
  uint32_t waitedMs(uint32_t nowMs) const;  // since begin(), 0 while Idle
  void reset();                             // -> Idle

 private:
  State state_ = State::Idle;
  uint32_t beganMs_ = 0;
  bool inFlight_ = false;
  bool polledOnce_ = false;
  uint32_t lastPollMs_ = 0;  // completion of the last poll
};

}  // namespace vdm
