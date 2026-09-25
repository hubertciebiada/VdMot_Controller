// Restart gate: before an ESP restart (which also resets the STM when jumper
// X20 is fitted) the STM gets the chance to finish a pending EEPROM write.
// The stm task answers through app::setStmSaveState(). Hardware-free.
#pragma once

#include <stdint.h>

#include "vdm/stm_types.h"

namespace vdm {

class RestartGate {
 public:
  // The stm task gives up after 10 s (TimedOut); this guard covers an stm
  // task that does not answer at all.
  static constexpr uint32_t kGuardMs = 12000;
  enum class Step : uint8_t { RequestStmSave, Wait, Proceed };
  // Every pass once a restart is due. First call -> RequestStmSave (the timer
  // starts); then Saved, Unavailable or TimedOut -> Proceed; >= kGuardMs ->
  // Proceed with guardExpired(); else Wait. Proceed is final until reset().
  Step update(StmSaveState s, uint32_t nowMs);
  bool guardExpired() const { return guardExpired_; }
  uint32_t waitedMs(uint32_t nowMs) const;  // since RequestStmSave, 0 before
  void reset();

 private:
  bool started_ = false;
  bool done_ = false;
  bool guardExpired_ = false;
  uint32_t startMs_ = 0;
};

}  // namespace vdm
