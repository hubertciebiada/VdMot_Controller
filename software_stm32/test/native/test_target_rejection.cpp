#include <stdint.h>

#include "doctest.h"
#include "vdm/target_rejection.h"

namespace {

// position the failed or blocked valve is stuck at in these tests
constexpr uint8_t kStuck = 70;

}  // namespace

TEST_CASE("rejectTarget: without a known target the first one is recorded, not counted") {
  uint8_t rejected = vdm::kNoRejectedTarget;
  uint16_t count = 0;
  CHECK_FALSE(vdm::rejectTarget(rejected, count, 30, kStuck));
  CHECK(rejected == 30);
  CHECK(count == 0);
  // app_loop visits the valve again and again: nothing changes
  for (int i = 0; i < 100; i++) CHECK_FALSE(vdm::rejectTarget(rejected, count, 30, kStuck));
  CHECK(count == 0);
}

TEST_CASE("rejectTarget: the target recorded at the handover is not counted") {
  // app_loop recorded 50 when it handed the move that then timed out
  uint8_t rejected = 50;
  uint16_t count = 0;
  CHECK_FALSE(vdm::rejectTarget(rejected, count, 50, kStuck));
  CHECK(count == 0);
}

TEST_CASE("rejectTarget: a new target before the first visit after the fault is counted") {
  // handed with target 50, the move timed out, the ESP sent 60 before app_loop saw the fault
  uint8_t rejected = 50;
  uint16_t count = 0;
  CHECK(vdm::rejectTarget(rejected, count, 60, kStuck));
  CHECK(count == 1);
  CHECK(rejected == 60);
}

TEST_CASE("rejectTarget: every later target change is counted once") {
  uint8_t rejected = vdm::kNoRejectedTarget;
  uint16_t count = 0;
  vdm::rejectTarget(rejected, count, 30, kStuck);
  CHECK(vdm::rejectTarget(rejected, count, 100, kStuck));   // e.g. staop on a blocked valve
  CHECK(count == 1);
  CHECK_FALSE(vdm::rejectTarget(rejected, count, 100, kStuck));
  CHECK(vdm::rejectTarget(rejected, count, 0, kStuck));
  CHECK(vdm::rejectTarget(rejected, count, 30, kStuck));    // back to the first value is a change too
  CHECK(count == 3);
  CHECK(rejected == 30);
}

TEST_CASE("rejectTarget: a target equal to the position is not rejected, the next one counts") {
  // review finding: stuck at 30 with the left-behind target 50; the ESP sets 30, then 50 again
  uint8_t rejected = 50;
  uint16_t count = 0;
  CHECK_FALSE(vdm::rejectTarget(rejected, count, 50, 30));
  CHECK_FALSE(vdm::rejectTarget(rejected, count, 30, 30));
  CHECK(count == 0);
  CHECK(rejected == 30);
  CHECK(vdm::rejectTarget(rejected, count, 50, 30));
  CHECK(count == 1);
  // also without a known target
  rejected = vdm::kNoRejectedTarget;
  CHECK_FALSE(vdm::rejectTarget(rejected, count, 30, 30));
  CHECK(rejected == 30);
  CHECK(vdm::rejectTarget(rejected, count, 31, 30));
  CHECK(count == 2);
}

TEST_CASE("rejectTarget: the counter saturates") {
  uint8_t rejected = 10;
  uint16_t count = 0xFFFE;
  CHECK(vdm::rejectTarget(rejected, count, 11, kStuck));
  CHECK(count == 0xFFFF);
  CHECK(vdm::rejectTarget(rejected, count, 12, kStuck));
  CHECK(count == 0xFFFF);
}

TEST_CASE("rejectTarget: 0 and 100 are ordinary targets") {
  uint8_t rejected = vdm::kNoRejectedTarget;
  uint16_t count = 0;
  CHECK_FALSE(vdm::rejectTarget(rejected, count, 0, kStuck));
  CHECK(rejected == 0);
  CHECK(vdm::rejectTarget(rejected, count, 100, kStuck));
  CHECK(count == 1);
  CHECK_FALSE(vdm::rejectTarget(rejected, count, 100, 100));
  CHECK(count == 1);
}
