// Health figures reported by gstat: uptime, reset cause and reset counter.
// Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

// Seconds since start from a wrapping millisecond clock (millis() wraps
// after 49.7 days). update() must be called at least once per wrap period.
class UptimeCounter {
 public:
  void update(uint32_t nowMs);
  uint32_t seconds() const { return seconds_; }

 private:
  uint32_t lastMs_ = 0;
  uint32_t remainderMs_ = 0;
  uint32_t seconds_ = 0;
};

// Wire values of gstat bootReason.
enum class BootReason : uint8_t {
  Unknown = 0,
  PowerOn = 1,
  Pin = 2,       // NRST (e.g. reset by the ESP)
  Software = 3,  // soft reset (reset command, after flashing)
  IndependentWatchdog = 4,
  WindowWatchdog = 5,
  LowPower = 6,
  BrownOut = 7,
};

// Reset flags as found in RCC->CSR of the STM32F4.
struct ResetFlags {
  bool lowPower;
  bool windowWatchdog;
  bool independentWatchdog;
  bool software;
  bool powerOn;
  bool pin;
  bool brownOut;
};

// A power-on also sets the pin and brown-out flags, a software or watchdog
// reset also sets the pin flag, so the most specific flag wins.
BootReason classifyReset(const ResetFlags& f);

// Resets since the last power-on, kept in RAM that the start-up code does not
// clear. The magic word tells a warm start from random RAM content.
struct ResetCounterCell {
  uint32_t magic;
  uint32_t count;
  uint32_t check;  // ~count
};

constexpr uint32_t kResetCounterMagic = 0x5644524Du;  // "VDRM"

// Called once per start. Starts again at 0 after a power-on or brown-out and
// when the cell does not hold a valid value; otherwise counts this reset.
// Returns the new count.
uint32_t countReset(ResetCounterCell& cell, BootReason reason);

}  // namespace vdm
