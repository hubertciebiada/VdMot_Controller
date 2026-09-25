#include "vdm/failsafe.h"

namespace vdm {

bool failsafePctValid(uint32_t v) { return v <= 100 || v == kFailsafeHold; }

uint8_t sanitizeFailsafePct(uint8_t v) { return failsafePctValid(v) ? v : kFailsafeDefaultPct; }

}  // namespace vdm
