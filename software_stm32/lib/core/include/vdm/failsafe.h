// Failsafe positions: where a valve goes when the lease of its target expired
// (vdm/lease.h) or when it is blocked. Set by sfspo and stored in the EEPROM.
// Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

constexpr uint8_t kFailsafeHold = 255;  // the valve keeps its target
constexpr uint8_t kFailsafeDefaultPct = 50;

// 0..100 % or kFailsafeHold
bool failsafePctValid(uint32_t v);

// Stored value at start-up: anything sfspo accepts is kept, anything else
// loads kFailsafeDefaultPct.
uint8_t sanitizeFailsafePct(uint8_t v);

}  // namespace vdm
