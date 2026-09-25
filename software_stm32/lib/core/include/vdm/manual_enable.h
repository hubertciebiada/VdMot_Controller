// Limits of a motor output switched on from the debug terminal (sena): it is switched off after
// kManualEnableMaxMs or as soon as the filtered motor current exceeds the safety limit.
// Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

constexpr uint32_t kManualEnableMaxMs = 2000;
constexpr int32_t kManualEnableLimit = 600;  // 0.1 mA: the 60 mA safety limit

// true when the output enabled at startMs must be switched off; filteredCurrent in 0.1 mA, either
// sign; millis() differences, so the wrap does not matter
bool manualEnableExpired(uint32_t startMs, uint32_t nowMs, int32_t filteredCurrent);

}  // namespace vdm
