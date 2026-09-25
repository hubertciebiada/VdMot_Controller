#include "vdm/factory_reset.h"

#include "vdm/common.h"

namespace vdm {

FactoryPinDecision factoryPinAtBoot(bool pinLow, bool held, bool latched) {
  if (!pinLow) return latched ? FactoryPinDecision::ClearLatch : FactoryPinDecision::Idle;
  if (latched) return FactoryPinDecision::KeepLatched;
  return held ? FactoryPinDecision::Reset : FactoryPinDecision::Idle;
}

bool factoryPinRuntimeClear(bool pinLow, bool latched) { return latched && !pinLow; }

void PinHold::begin(uint32_t nowMs) {
  startMs_ = nowMs;
  state_ = State::Holding;
}

PinHold::State PinHold::sample(bool low, uint32_t nowMs) {
  if (state_ != State::Holding) return state_;
  if (!low) {
    state_ = State::Released;
  } else if (elapsedMs(nowMs, startMs_) >= holdMs_) {
    state_ = State::Held;
  }
  return state_;
}

}  // namespace vdm
