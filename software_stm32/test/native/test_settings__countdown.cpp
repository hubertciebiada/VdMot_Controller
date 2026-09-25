// vdm::countdown and vdm::effectiveLearnTime (S3, C-7).
#include <stdint.h>

#include "doctest.h"
#include "vdm/settings.h"

TEST_CASE("countdown: fires when the rest is not more than the elapsed time, subtracts otherwise") {
  uint32_t rest = 30;
  CHECK_FALSE(vdm::countdown(rest, 10));
  CHECK(rest == 20);
  CHECK_FALSE(vdm::countdown(rest, 19));
  CHECK(rest == 1);
  CHECK(vdm::countdown(rest, 1));
  rest = 5;
  CHECK(vdm::countdown(rest, 11));
  CHECK(rest == 5);  // the caller reloads
  rest = 0;
  CHECK(vdm::countdown(rest, 0));
  rest = 7;
  CHECK_FALSE(vdm::countdown(rest, 0));
  CHECK(rest == 7);
}

TEST_CASE("effectiveLearnTime: a stored 0 is honoured only with a recent lease client") {
  CHECK(vdm::kLearnTimeClientWindowS == 86400);
  CHECK(vdm::effectiveLearnTime(0, true) == 0);
  CHECK(vdm::effectiveLearnTime(0, false) == 604800);
  CHECK(vdm::effectiveLearnTime(3600, false) == 3600);
  CHECK(vdm::effectiveLearnTime(3600, true) == 3600);
  CHECK(vdm::effectiveLearnTime(1, false) == 1);
}
