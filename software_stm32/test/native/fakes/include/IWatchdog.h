// Fake of the core's IWatchdog library: begin() starts the watchdog (it cannot be stopped), reload()
// feeds it (fake::Ev::Reload). When the fake time passes timeoutUs without a reload,
// fake::advanceUs() throws fake::WatchdogReset (glue::run() turns it into a watchdog reset).
// Glue-facing: no STL.
#pragma once

#include <stdint.h>

#define IWDG_TIMEOUT_MIN 125
#define IWDG_TIMEOUT_MAX 32768000

class IWatchdogClass {
 public:
  void begin(uint32_t timeout, uint32_t window = IWDG_TIMEOUT_MAX);
  void set(uint32_t timeout, uint32_t window = IWDG_TIMEOUT_MAX);
  void reload();
  bool isEnabled() const { return enabled; }
  bool isReset(bool clear = false);
  void clearReset();

  // ---- fake state
  bool enabled = false;
  uint32_t timeoutUs = 0;
  unsigned begins = 0;
  unsigned reloads = 0;
  uint32_t lastReloadMs = 0;
  uint64_t lastReloadUs = 0;
};

extern IWatchdogClass IWatchdog;
