// Factory reset by the GPIO2 jumper: held LOW for the hold time at boot,
// once per fitting of the jumper (a latch in NVS). Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

enum class FactoryPinDecision : uint8_t { Idle, Reset, KeepLatched, ClearLatch };
// pinLow: first sample at boot; held: LOW for the whole hold time (measured
// only when pinLow && !latched); latched: a pin reset happened and the pin
// was not seen HIGH since.
//  !pinLow -> latched ? ClearLatch : Idle;  pinLow && latched -> KeepLatched (no wait);
//  pinLow && !latched -> held ? Reset : Idle.
FactoryPinDecision factoryPinAtBoot(bool pinLow, bool held, bool latched);
// Run time (every second): true when the latch is to be cleared (latched && !pinLow).
bool factoryPinRuntimeClear(bool pinLow, bool latched);

// Debounced hold detector.
class PinHold {
 public:
  enum class State : uint8_t { Holding, Held, Released };
  explicit PinHold(uint32_t holdMs) : holdMs_(holdMs) {}
  void begin(uint32_t nowMs);
  // Released on the first HIGH sample (final); Held once LOW for holdMs (final).
  State sample(bool low, uint32_t nowMs);

 private:
  uint32_t holdMs_;
  uint32_t startMs_ = 0;
  State state_ = State::Holding;
};

}  // namespace vdm
