// Clamp edges of positionAfterEndStop.
#include <stdint.h>

#include "doctest.h"
#include "vdm/move_classifier.h"

TEST_CASE("positionAfterEndStop: an open move one past 100 is capped to 100") {
  CHECK(vdm::positionAfterEndStop(95, vdm::kDirOpen, 40 * 89, 6 * 89, 89) == 100);
  CHECK(vdm::positionAfterEndStop(0, vdm::kDirOpen, 40 * 89, 101 * 89, 89) == 100);
}

TEST_CASE("positionAfterEndStop: the counted travel is capped to 100 before a close") {
  CHECK(vdm::positionAfterEndStop(150, vdm::kDirClose, 40 * 89, 101 * 89, 89) == 50);
  CHECK(vdm::positionAfterEndStop(150, vdm::kDirClose, 40 * 89, 100 * 89, 89) == 50);
  CHECK(vdm::positionAfterEndStop(150, vdm::kDirClose, 40 * 89, 99 * 89, 89) == 51);
}
