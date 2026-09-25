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

// gvlvy field 23 and gstax failsafeMask: where a valve is driven to and why
enum class DriveSource : uint8_t { Target = 0, LeaseFailsafe = 1, BlockedFailsafe = 2 };
struct Drive {
  uint8_t position;
  DriveSource source;
};

// Hold (255) -> target; blocked (9) -> failsafe; failed (4) or open circuit (6)
// -> target (not driven); lease expired and no assembly hold -> failsafe;
// otherwise the target.
Drive driveTarget(uint8_t target, uint8_t failsafePct, uint8_t status, bool leaseExpired, bool assemblyHold);

}  // namespace vdm
