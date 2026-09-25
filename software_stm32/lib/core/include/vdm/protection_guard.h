// Common-mode guard of the short and inrush limits: when the presence-test
// short check or the inrush limit trips on several different valves within a
// short time, the limits are more likely wrong for this installation (supply,
// actuator type) than all the valves shorted. Then both limits stay off until
// the next start. Hardware-free; time in uptime seconds.
#pragma once

#include <stdint.h>

#include "vdm/legacy_layout.h"

namespace vdm {

constexpr uint8_t kProtectTripValves = 3;   // different valves ...
constexpr uint32_t kProtectWindowS = 600;   // ... with a trip less than this apart

// Build switch of the short and inrush limits: false = report only (the trip is
// detected and reported as fault 3 / 5, but the status of the valve does not
// change and the move or test goes on as without the limit). Set to true only
// after the limits were measured on the hardware (spec-stm C-4).
constexpr bool kProtectEnforce = false;

class ProtectionGuard {
 public:
  // A short verdict or an inrush trip of the valve. Returns suspended().
  bool onTrip(uint8_t valve, uint32_t uptimeS);
  bool suspended() const { return suspended_; }

 private:
  uint32_t lastTripS_[kValveCount] = {};
  uint16_t tripped_ = 0;  // bit v: lastTripS_[v] is valid
  bool suspended_ = false;
};

}  // namespace vdm
