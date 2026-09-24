#include <stdint.h>

#include "doctest.h"
#include "vdm/settings.h"

TEST_CASE("capLearnMovements: values in range are kept") {
  CHECK(vdm::capLearnMovements(0) == 0);
  CHECK(vdm::capLearnMovements(50) == 50);
  CHECK(vdm::capLearnMovements(65533) == 65533);
  CHECK(vdm::capLearnMovements(65534) == 65534);
}

TEST_CASE("capLearnMovements: larger values are capped, never wrapped") {
  CHECK(vdm::kMaxLearnMovements == 65534);
  CHECK(vdm::capLearnMovements(65535) == 65534);
  CHECK(vdm::capLearnMovements(65536) == 65534);  // would wrap to 0
  CHECK(vdm::capLearnMovements(70000) == 65534);  // would wrap to 4464
  CHECK(vdm::capLearnMovements(UINT32_MAX) == 65534);
}
