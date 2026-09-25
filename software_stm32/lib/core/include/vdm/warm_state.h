// Valve positions, lease and retry schedule kept across a warm reset (pin,
// software, watchdog) in RAM the start-up code does not clear (.noinit).
// Layout of contracts.md section 4.1. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/legacy_layout.h"
#include "vdm/system_stats.h"

namespace vdm {

constexpr uint32_t kWarmStateMagic = 0x56445753u;  // "VDWS"
constexpr uint8_t kWarmStateVersion = 1;
// WarmValve.flags
constexpr uint8_t kWarmPosValid = 0x01;  // actual is where the valve stands (no move handed over)
constexpr uint8_t kWarmAssemblyHold = 0x02;
constexpr uint8_t kWarmNeedsReference = 0x04;
constexpr uint8_t kWarmRecal = 0x08;
constexpr uint8_t kWarmFlagsUnused = 0xF0;
constexpr uint32_t kWarmRetryMaxS = 86400;

struct WarmValve {
  uint8_t actual;
  uint8_t target;  // stored target, not the drive target
  uint8_t status;
  uint8_t flags;
  uint8_t retryAttempts;
  uint8_t retryScheduled;  // 0/1
  uint8_t pad[2];
  uint32_t retryRemainingS;
};

struct WarmState {
  uint32_t magic;
  uint8_t version;
  uint8_t count;
  uint16_t reserved;
  WarmValve valves[kValveCount];
  uint32_t leaseSinceRenewalS;
  uint32_t leaseSinceClientS;
  uint8_t leaseClient;
  uint8_t pad;
  uint16_t leaseTimeoutMin;
  uint8_t failsafePct[kValveCount];
  uint16_t crc;  // CRC-16/CCITT-FALSE over all bytes before it
};

static_assert(sizeof(WarmValve) == 12, "WarmValve layout");
static_assert(offsetof(WarmState, valves) == 8, "WarmState layout");
static_assert(offsetof(WarmState, leaseSinceRenewalS) == 152, "WarmState layout");
static_assert(offsetof(WarmState, failsafePct) == 164, "WarmState layout");
static_assert(offsetof(WarmState, crc) == 176, "WarmState layout");

// magic, version, count, reserved and the CRC (last)
void warmStateSeal(WarmState& s);
// magic, version, count and CRC
bool warmStateValid(const WarmState& s);
// Pin, Software, IndependentWatchdog, WindowWatchdog, LowPower
bool isWarmBoot(BootReason r);

struct RestoredValve {
  bool valid;  // false: this valve takes the cold path (presence test)
  uint8_t status;
  uint8_t actual;
  uint8_t target;
  bool needsReference;
  bool recal;
  bool assemblyHold;
};

// Field checks: actual and target <= 100, status 1..9, flags bits 4..7 clear,
// retryScheduled <= 1, retryRemainingS <= kWarmRetryMaxS; any failure ->
// !valid. Status: a valve that was moving (2, 3) or whose position is not
// valid -> 1 with needsReference when calibrated, else 5 (tested again);
// 1, 4..9 kept. needsReference, recal, assemblyHold from the flags.
RestoredValve restoreValve(const WarmValve& w, bool calibrated);

}  // namespace vdm
