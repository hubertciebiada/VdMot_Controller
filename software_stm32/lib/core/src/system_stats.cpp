#include "vdm/system_stats.h"

namespace vdm {

void UptimeCounter::update(uint32_t nowMs) {
  // unsigned difference stays right across the wrap of the millisecond clock
  const uint32_t elapsed = nowMs - lastMs_;
  lastMs_ = nowMs;
  const uint64_t total = static_cast<uint64_t>(remainderMs_) + elapsed;
  seconds_ += static_cast<uint32_t>(total / 1000);
  remainderMs_ = static_cast<uint32_t>(total % 1000);
}

BootReason classifyReset(const ResetFlags& f) {
  if (f.independentWatchdog) return BootReason::IndependentWatchdog;
  if (f.windowWatchdog) return BootReason::WindowWatchdog;
  if (f.lowPower) return BootReason::LowPower;
  if (f.software) return BootReason::Software;
  if (f.powerOn) return BootReason::PowerOn;
  if (f.brownOut) return BootReason::BrownOut;
  if (f.pin) return BootReason::Pin;
  return BootReason::Unknown;
}

uint32_t countReset(ResetCounterCell& cell, BootReason reason) {
  const bool coldStart = reason == BootReason::PowerOn || reason == BootReason::BrownOut;
  const bool valid = cell.magic == kResetCounterMagic && cell.check == ~cell.count;

  if (coldStart || !valid) {
    cell.count = 0;
  } else if (cell.count < 0xFFFFFFFFu) {
    cell.count++;
  }
  cell.magic = kResetCounterMagic;
  cell.check = ~cell.count;
  return cell.count;
}

}  // namespace vdm
