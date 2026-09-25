#include <stdint.h>

#include "doctest.h"
#include "vdm/manual_enable.h"

TEST_CASE("manualEnableExpired: the time limit of 2000 ms") {
  CHECK_FALSE(vdm::manualEnableExpired(1000, 1000, 0));
  CHECK_FALSE(vdm::manualEnableExpired(1000, 2999, 0));
  CHECK(vdm::manualEnableExpired(1000, 3000, 0));
  CHECK(vdm::manualEnableExpired(1000, 3001, 0));
}

TEST_CASE("manualEnableExpired: more than 60 mA either way switches off at once") {
  CHECK_FALSE(vdm::manualEnableExpired(0, 0, 600));
  CHECK(vdm::manualEnableExpired(0, 0, 601));
  CHECK_FALSE(vdm::manualEnableExpired(0, 0, -600));
  CHECK(vdm::manualEnableExpired(0, 0, -601));
  CHECK(vdm::manualEnableExpired(0, 1999, 601));
}

TEST_CASE("manualEnableExpired: across the millis() wrap") {
  CHECK_FALSE(vdm::manualEnableExpired(0xFFFFFC18u, 0x000003E7u, 0));  // 1999 ms
  CHECK(vdm::manualEnableExpired(0xFFFFFC18u, 0x000003E8u, 0));        // 2000 ms
}

TEST_CASE("manual_enable: the limits") {
  CHECK(vdm::kManualEnableMaxMs == 2000);
  CHECK(vdm::kManualEnableLimit == 600);
}
