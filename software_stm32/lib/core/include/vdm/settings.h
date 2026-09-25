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

// learn-after-time (`stlnt`) in seconds, 0 disables the time trigger;
// the default is one week (== LEARN_AFTER_TIME_DEFAULT, static_assert in app.cpp)
constexpr uint32_t kLearnTimeDefaultS = 7 * 24 * 3600;
// a stored learn time 0 (the ESP runs its own schedule) is only honoured while
// a lease client was seen within this time: a rolled-back ESP without a
// schedule never leaves the valves without the time trigger
constexpr uint32_t kLearnTimeClientWindowS = 24 * 3600;

// learn time the countdown runs with: stored, or the default for a stored 0
// without a recent lease client
uint32_t effectiveLearnTime(uint32_t storedS, bool clientSeen);

// One countdown step: true when remaining <= elapsedS (the caller reloads),
// otherwise remaining -= elapsedS.
bool countdown(uint32_t& remaining, uint32_t elapsedS);

// `stlnm` takes any 32-bit count (the ESP stores it as uint32) and 1.x always
// replied, so a value outside the range is moved to its nearest bound
// instead of being rejected: 1..49 -> 50, above 65534 -> 65534 (never wrapped).
uint16_t learnMovementsFromRequest(uint32_t movements);

// Stored value at start-up: anything `stlnm` can store is kept, anything else
// (erased EEPROM, 1..49 written by 1.x) loads the default.
uint16_t sanitizeLearnMovements(uint16_t stored);

}  // namespace vdm
