#include "vdm/manual_enable.h"

namespace vdm {

bool manualEnableExpired(uint32_t startMs, uint32_t nowMs, int32_t filteredCurrent) {
  return nowMs - startMs >= kManualEnableMaxMs || filteredCurrent > kManualEnableLimit ||
         filteredCurrent < -kManualEnableLimit;
}

}  // namespace vdm
