#include <stdint.h>

#include "doctest.h"
#include "vdm/target_rejection.h"

TEST_CASE("rejectTarget: the target the fault left behind is recorded, not counted") {
  uint8_t rejected = vdm::kNoRejectedTarget;
  uint16_t count = 0;
  CHECK_FALSE(vdm::rejectTarget(rejected, count, 30));
  CHECK(rejected == 30);
  CHECK(count == 0);
  // app_loop visits the valve again and again: nothing changes
  for (int i = 0; i < 100; i++) CHECK_FALSE(vdm::rejectTarget(rejected, count, 30));
  CHECK(count == 0);
}

TEST_CASE("rejectTarget: every later target change is counted once") {
  uint8_t rejected = vdm::kNoRejectedTarget;
  uint16_t count = 0;
  vdm::rejectTarget(rejected, count, 30);
  CHECK(vdm::rejectTarget(rejected, count, 100));   // e.g. staop on a blocked valve
  CHECK(count == 1);
  CHECK_FALSE(vdm::rejectTarget(rejected, count, 100));
  CHECK(vdm::rejectTarget(rejected, count, 0));
  CHECK(vdm::rejectTarget(rejected, count, 30));    // back to the first value is a change too
  CHECK(count == 3);
  CHECK(rejected == 30);
}

TEST_CASE("rejectTarget: the counter saturates") {
  uint8_t rejected = 10;
  uint16_t count = 0xFFFE;
  CHECK(vdm::rejectTarget(rejected, count, 11));
  CHECK(count == 0xFFFF);
  CHECK(vdm::rejectTarget(rejected, count, 12));
  CHECK(count == 0xFFFF);
}

TEST_CASE("rejectTarget: 0 and 100 are ordinary targets") {
  uint8_t rejected = vdm::kNoRejectedTarget;
  uint16_t count = 0;
  CHECK_FALSE(vdm::rejectTarget(rejected, count, 0));
  CHECK(rejected == 0);
  CHECK(vdm::rejectTarget(rejected, count, 100));
  CHECK(count == 1);
}
