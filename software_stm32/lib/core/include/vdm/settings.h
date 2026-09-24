// Validation of settings received over the UART protocol. Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

// learn-after-movements: 0 disables the movement trigger, otherwise
// kMinLearnMovements..kMaxLearnMovements. The same range applies to `stlnm`
// and to the value loaded from the EEPROM (16 bit field, 0xFFFF = erased).
constexpr uint16_t kMinLearnMovements = 50;
constexpr uint16_t kMaxLearnMovements = 65534;
constexpr uint16_t kLearnMovementsDefault = 2000;

// `stlnm` takes any 32-bit count (the ESP stores it as uint32) and 1.x always
// replied, so a value outside the range is moved to its nearest bound
// instead of being rejected: 1..49 -> 50, above 65534 -> 65534 (never wrapped).
constexpr uint16_t learnMovementsFromRequest(uint32_t movements) {
  return movements == 0 ? 0
         : movements < kMinLearnMovements ? kMinLearnMovements
         : movements > kMaxLearnMovements ? kMaxLearnMovements
                                          : static_cast<uint16_t>(movements);
}

// Stored value at start-up: anything `stlnm` can store is kept, anything else
// (erased EEPROM, 1..49 written by 1.x) loads the default.
constexpr uint16_t sanitizeLearnMovements(uint16_t stored) {
  return (stored == 0 || (stored >= kMinLearnMovements && stored <= kMaxLearnMovements))
             ? stored
             : kLearnMovementsDefault;
}

}  // namespace vdm
