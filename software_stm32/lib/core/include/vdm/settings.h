// Validation of settings received over the UART protocol. Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

// Largest learn-after-movements value that survives a reboot: the EEPROM
// field is 16 bit and the start-up check accepts 50..65534.
constexpr uint16_t kMaxLearnMovements = 65534;

// `stlnm` takes any 32-bit count (the ESP stores it as uint32); larger
// values are capped instead of wrapping or being rejected.
constexpr uint16_t capLearnMovements(uint32_t movements) {
  return movements > kMaxLearnMovements ? kMaxLearnMovements : static_cast<uint16_t>(movements);
}

}  // namespace vdm
