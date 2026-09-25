#include <stdint.h>

#include "doctest.h"
#include "vdm/failsafe.h"

TEST_CASE("failsafe: hold marker and default position") {
  CHECK(vdm::kFailsafeHold == 255);
  CHECK(vdm::kFailsafeDefaultPct == 50);
}

TEST_CASE("failsafePctValid: 0..100 % and 255 (hold)") {
  CHECK(vdm::failsafePctValid(0));
  CHECK(vdm::failsafePctValid(1));
  CHECK(vdm::failsafePctValid(50));
  CHECK(vdm::failsafePctValid(99));
  CHECK(vdm::failsafePctValid(100));
  CHECK_FALSE(vdm::failsafePctValid(101));
  CHECK_FALSE(vdm::failsafePctValid(200));
  CHECK_FALSE(vdm::failsafePctValid(254));
  CHECK(vdm::failsafePctValid(255));
  // the request value is 32 bit: no truncation to 8 bit
  CHECK_FALSE(vdm::failsafePctValid(256));
  CHECK_FALSE(vdm::failsafePctValid(256 + 50));
  CHECK_FALSE(vdm::failsafePctValid(256 + 255));
  CHECK_FALSE(vdm::failsafePctValid(UINT32_MAX));
}

TEST_CASE("sanitizeFailsafePct: what sfspo accepts is kept, anything else loads 50") {
  CHECK(vdm::sanitizeFailsafePct(0) == 0);
  CHECK(vdm::sanitizeFailsafePct(30) == 30);
  CHECK(vdm::sanitizeFailsafePct(100) == 100);
  CHECK(vdm::sanitizeFailsafePct(101) == 50);
  CHECK(vdm::sanitizeFailsafePct(254) == 50);
  CHECK(vdm::sanitizeFailsafePct(255) == 255);
}
