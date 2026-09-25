#include <stdint.h>

#include "doctest.h"
#include "vdm/protection_guard.h"

using vdm::ProtectionGuard;

TEST_CASE("ProtectionGuard: an out-of-range valve reports the current state without counting") {
  ProtectionGuard g;
  CHECK_FALSE(g.onTrip(vdm::kValveCount, 100));
  CHECK_FALSE(g.suspended());
  CHECK_FALSE(g.onTrip(0, 100));
  CHECK_FALSE(g.onTrip(1, 100));
  CHECK(g.onTrip(2, 100));
  CHECK(g.onTrip(vdm::kValveCount, 200));
  CHECK(g.onTrip(255, 200));
  CHECK(g.suspended());
}
