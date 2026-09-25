#include <stdint.h>
#include <string.h>

#include "doctest.h"
#include "vdm/reset_guard.h"

using vdm::BootReason;
using vdm::ResetGuardCell;

namespace {

// a valid cell after a power-on and two watchdog resets (count 2)
ResetGuardCell twoWatchdogs() {
  ResetGuardCell c;
  memset(&c, 0xA5, sizeof c);
  vdm::resetGuardOnBoot(c, BootReason::PowerOn);
  vdm::resetGuardOnBoot(c, BootReason::IndependentWatchdog);
  vdm::resetGuardOnBoot(c, BootReason::IndependentWatchdog);
  return c;
}

}  // namespace

TEST_CASE("reset guard: brown-out and unknown clear a valid cell") {
  const BootReason cold[] = {BootReason::BrownOut, BootReason::Unknown, BootReason::PowerOn};
  for (BootReason r : cold) {
    ResetGuardCell c = twoWatchdogs();
    REQUIRE(c.count == 2);
    vdm::resetGuardAlive(c, 30);
    CHECK_FALSE(vdm::resetGuardOnBoot(c, r));
    CHECK(c.count == 0);
    CHECK(c.windowS == 0);
  }
}

TEST_CASE("reset guard: a boot with no uptime keeps the window sum") {
  ResetGuardCell c = twoWatchdogs();
  vdm::resetGuardAlive(c, 100);
  vdm::resetGuardOnBoot(c, BootReason::Pin);
  CHECK(c.windowS == 100);
  vdm::resetGuardOnBoot(c, BootReason::Pin);  // lastUptimeS 0
  CHECK(c.windowS == 100);
  vdm::resetGuardAlive(c, UINT32_MAX - 100);
  vdm::resetGuardOnBoot(c, BootReason::Pin);
  CHECK(c.windowS == UINT32_MAX);
}

TEST_CASE("reset guard: the watchdog count saturates at 255") {
  ResetGuardCell c = twoWatchdogs();
  for (int i = 3; i <= 254; ++i) vdm::resetGuardOnBoot(c, BootReason::IndependentWatchdog);
  CHECK(c.count == 254);
  CHECK(vdm::resetGuardOnBoot(c, BootReason::IndependentWatchdog));
  CHECK(c.count == 255);
  for (int i = 0; i < 3; ++i) CHECK(vdm::resetGuardOnBoot(c, BootReason::WindowWatchdog));
  CHECK(c.count == 255);
  CHECK(c.windowS == 0);
}
