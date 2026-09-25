// Safe mode after a loop of watchdog resets: 3 watchdog resets within 10 min
// stop every valve movement until the safe mode is left (ssafe 0, 30 min of
// uptime, power-on). The cell lives in RAM the start-up code does not clear
// (.noinit, src/sysstat.cpp). Hardware-free.
#pragma once

#include <stdint.h>

#include "vdm/system_stats.h"

namespace vdm {

constexpr uint8_t kSafeModeResets = 3;
constexpr uint32_t kSafeModeWindowS = 600;
constexpr uint32_t kSafeModeExitS = 1800;
constexpr uint32_t kResetGuardMagic = 0x56445247u;  // "VDRG"

struct ResetGuardCell {
  uint32_t magic;
  uint8_t count;   // watchdog resets in the current window
  uint8_t safe;    // 0/1
  uint16_t pad;
  uint32_t windowS;       // uptime summed over the boots since the first watchdog reset of the window
  uint32_t lastUptimeS;   // uptime of this boot, updated every second
  uint16_t crc;           // CRC-16/CCITT-FALSE over the bytes before it
};

// Once per start after classifyReset(). Cold (PowerOn, BrownOut, Unknown) or
// an invalid cell -> all 0. Warm: windowS += lastUptimeS (saturating); a
// watchdog reason (IndependentWatchdog, WindowWatchdog) starts a new window
// (count 1, windowS 0) when count == 0 or windowS > kSafeModeWindowS, else
// count + 1; safe |= count >= kSafeModeResets. Returns safe.
bool resetGuardOnBoot(ResetGuardCell& c, BootReason reason);
// Every second: lastUptimeS = uptimeS; safe && uptimeS >= kSafeModeExitS
// leaves safe mode (count 0, windowS 0). Returns safe.
bool resetGuardAlive(ResetGuardCell& c, uint32_t uptimeS);
// ssafe 0: safe mode off, count 0, windowS 0
void resetGuardClear(ResetGuardCell& c);

}  // namespace vdm
