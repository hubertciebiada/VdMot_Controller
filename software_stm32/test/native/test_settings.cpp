#include <stdint.h>

#include "doctest.h"
#include "vdm/settings.h"

TEST_CASE("learnMovementsFromRequest: values in range are kept, 0 disables") {
  CHECK(vdm::learnMovementsFromRequest(0) == 0);
  CHECK(vdm::learnMovementsFromRequest(50) == 50);
  CHECK(vdm::learnMovementsFromRequest(2000) == 2000);
  CHECK(vdm::learnMovementsFromRequest(65533) == 65533);
  CHECK(vdm::learnMovementsFromRequest(65534) == 65534);
}

TEST_CASE("learnMovementsFromRequest: 1..49 are raised to 50") {
  CHECK(vdm::kMinLearnMovements == 50);
  CHECK(vdm::learnMovementsFromRequest(1) == 50);
  CHECK(vdm::learnMovementsFromRequest(49) == 50);
}

TEST_CASE("learnMovementsFromRequest: larger values are capped, never wrapped") {
  CHECK(vdm::kMaxLearnMovements == 65534);
  CHECK(vdm::learnMovementsFromRequest(65535) == 65534);
  CHECK(vdm::learnMovementsFromRequest(65536) == 65534);  // would wrap to 0
  CHECK(vdm::learnMovementsFromRequest(70000) == 65534);  // would wrap to 4464
  CHECK(vdm::learnMovementsFromRequest(UINT32_MAX) == 65534);
}

TEST_CASE("sanitizeLearnMovements: 0 and 50..65534 load, anything else is the default") {
  CHECK(vdm::kLearnMovementsDefault == 2000);
  CHECK(vdm::sanitizeLearnMovements(0) == 0);
  CHECK(vdm::sanitizeLearnMovements(1) == 2000);
  CHECK(vdm::sanitizeLearnMovements(49) == 2000);
  CHECK(vdm::sanitizeLearnMovements(50) == 50);
  CHECK(vdm::sanitizeLearnMovements(65534) == 65534);
  CHECK(vdm::sanitizeLearnMovements(65535) == 2000);  // erased EEPROM
}

TEST_CASE("every value stlnm can store survives a restart unchanged") {
  // the "works until restart" class of bug: the runtime and the start-up range must match
  const uint32_t requests[] = {0, 1, 2, 49, 50, 51, 1000, 2000, 65533, 65534, 65535, 65536, 100000, UINT32_MAX};
  for (uint32_t r : requests) {
    const uint16_t stored = vdm::learnMovementsFromRequest(r);
    CHECK(vdm::sanitizeLearnMovements(stored) == stored);
  }
  for (uint32_t v = 0; v <= 0xFFFF; v++) {
    const uint16_t stored = vdm::learnMovementsFromRequest(v);
    REQUIRE(vdm::sanitizeLearnMovements(stored) == stored);
  }
}
