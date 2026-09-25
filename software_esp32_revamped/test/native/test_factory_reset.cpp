// Factory reset pin: boot decision, run-time latch clear, hold detector.
#include "doctest.h"
#include "vdm/factory_reset.h"

using namespace vdm;

using F = FactoryPinDecision;
using S = PinHold::State;

TEST_CASE("factoryPinAtBoot: the 8 combinations") {
  CHECK(factoryPinAtBoot(false, false, false) == F::Idle);
  CHECK(factoryPinAtBoot(false, true, false) == F::Idle);
  CHECK(factoryPinAtBoot(false, false, true) == F::ClearLatch);
  CHECK(factoryPinAtBoot(false, true, true) == F::ClearLatch);
  CHECK(factoryPinAtBoot(true, false, true) == F::KeepLatched);
  CHECK(factoryPinAtBoot(true, true, true) == F::KeepLatched);
  CHECK(factoryPinAtBoot(true, false, false) == F::Idle);
  CHECK(factoryPinAtBoot(true, true, false) == F::Reset);
}

TEST_CASE("factoryPinRuntimeClear") {
  CHECK_FALSE(factoryPinRuntimeClear(false, false));
  CHECK(factoryPinRuntimeClear(false, true));
  CHECK_FALSE(factoryPinRuntimeClear(true, false));
  CHECK_FALSE(factoryPinRuntimeClear(true, true));
}

TEST_CASE("PinHold: held after 5 s of LOW") {
  PinHold h(5000);
  h.begin(100);
  CHECK(h.sample(true, 100) == S::Holding);
  CHECK(h.sample(true, 5099) == S::Holding);
  CHECK(h.sample(true, 5100) == S::Held);
  CHECK(h.sample(false, 5150) == S::Held);  // final
  h.begin(10000);
  CHECK(h.sample(true, 10000) == S::Holding);
}

TEST_CASE("PinHold: a HIGH sample releases for good") {
  PinHold h(5000);
  h.begin(100);
  CHECK(h.sample(true, 1000) == S::Holding);
  CHECK(h.sample(false, 2000) == S::Released);
  CHECK(h.sample(true, 3000) == S::Released);
  CHECK(h.sample(true, 9000) == S::Released);
}

TEST_CASE("PinHold: begun near the 32-bit wrap") {
  PinHold h(5000);
  const uint32_t s = 0xFFFFF000u;
  h.begin(s);
  CHECK(h.sample(true, s + 4999u) == S::Holding);
  CHECK(h.sample(true, s + 5000u) == S::Held);
}
