#include <stdint.h>

#include "doctest.h"
#include "vdm/stall_detector.h"

using vdm::StallDetector;

TEST_CASE("StallDetector: a busy state is stalled after exactly limit further ticks") {
  StallDetector d(3);
  d.tick(5, false);  // enters state 5
  CHECK_FALSE(d.stalled());
  d.tick(5, false);
  d.tick(5, false);
  CHECK_FALSE(d.stalled());
  d.tick(5, false);
  CHECK(d.stalled());
  d.tick(5, false);  // stays stalled, the count saturates
  CHECK(d.stalled());
}

TEST_CASE("StallDetector: a state change restarts the count") {
  StallDetector d(2);
  d.tick(5, false);
  d.tick(5, false);
  d.tick(6, false);
  d.tick(6, false);
  CHECK_FALSE(d.stalled());
  d.tick(6, false);
  CHECK(d.stalled());
  d.tick(5, false);  // progress after a stall clears it
  CHECK_FALSE(d.stalled());
}

TEST_CASE("StallDetector: idle is never stalled") {
  StallDetector d(2);
  for (int i = 0; i < 10; ++i) d.tick(1, true);
  CHECK_FALSE(d.stalled());
  d.tick(1, false);  // same value, but now busy: counting starts after idle
  d.tick(1, false);
  CHECK_FALSE(d.stalled());
  d.tick(1, false);
  CHECK(d.stalled());
  d.tick(1, true);
  CHECK_FALSE(d.stalled());
}

TEST_CASE("StallDetector: the first busy tick enters the state, whatever its value") {
  StallDetector d(1);
  CHECK_FALSE(d.stalled());
  d.tick(0, false);
  CHECK_FALSE(d.stalled());
  d.tick(0, false);
  CHECK(d.stalled());
}

TEST_CASE("StallDetector: limit 0 means stalled while busy") {
  StallDetector d(0);
  d.tick(7, false);
  CHECK(d.stalled());
  d.tick(7, true);
  CHECK_FALSE(d.stalled());
}

TEST_CASE("StallDetector: large limits do not overflow") {
  StallDetector d(UINT32_MAX);
  d.tick(3, false);
  for (int i = 0; i < 1000; ++i) d.tick(3, false);
  CHECK_FALSE(d.stalled());
}
