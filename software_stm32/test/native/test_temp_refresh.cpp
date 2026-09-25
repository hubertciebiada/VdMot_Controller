#include <stdint.h>

#include "doctest.h"
#include "vdm/temp_refresh.h"

using vdm::TempRefresh;

TEST_CASE("TempRefresh: constants") {
  CHECK(vdm::kTempRefreshMs == 60000);
  CHECK(vdm::kTempHoldMaxMs == 3000);
}

TEST_CASE("TempRefresh: due 60 s after start-up, the hold ends with the cycle") {
  TempRefresh t;
  CHECK_FALSE(t.due(59999));
  CHECK_FALSE(t.holdCommands(59999));
  CHECK(t.due(60000));
  CHECK(t.holdCommands(60000));
  CHECK(t.holdCommands(61000));
  t.cycleDone(61500);
  CHECK_FALSE(t.due(61500));
  CHECK_FALSE(t.holdCommands(61500));
  CHECK_FALSE(t.due(121499));
  CHECK(t.due(121500));
}

TEST_CASE("TempRefresh: a hold ends after 3000 ms and comes again 60 s later") {
  TempRefresh t;
  CHECK(t.holdCommands(60000));
  CHECK(t.holdCommands(62999));
  CHECK_FALSE(t.holdCommands(63000));
  CHECK_FALSE(t.due(63000));
  CHECK_FALSE(t.holdCommands(64000));
  CHECK_FALSE(t.holdCommands(122999));
  CHECK(t.holdCommands(123000));
  CHECK(t.holdCommands(125999));
  CHECK_FALSE(t.holdCommands(126000));
}

TEST_CASE("TempRefresh: the hold time starts with the first hold, not with the due time") {
  TempRefresh t;
  CHECK(t.due(70000));
  CHECK(t.holdCommands(70000));
  CHECK(t.holdCommands(72999));
  CHECK_FALSE(t.holdCommands(73000));
}

TEST_CASE("TempRefresh: age since start-up and since the last cycle") {
  TempRefresh t;
  CHECK(t.ageS(0) == 0);
  CHECK(t.ageS(999) == 0);
  CHECK(t.ageS(1000) == 1);
  CHECK(t.ageS(65000) == 65);
  t.cycleDone(65000);
  CHECK(t.ageS(65999) == 0);
  CHECK(t.ageS(67000) == 2);
  // a timed-out hold does not reset the age
  t.holdCommands(125000);
  t.holdCommands(128000);
  CHECK(t.ageS(128000) == 63);
}

TEST_CASE("TempRefresh: millis() wrap") {
  TempRefresh t;
  t.cycleDone(0xFFFFF000u);
  CHECK_FALSE(t.due(0xFFFFFFFFu));
  CHECK(t.due(0x00010000u));
  CHECK(t.ageS(0x00010000u) == 69);
  CHECK(t.holdCommands(0x00010000u));
  CHECK_FALSE(t.holdCommands(0x00010000u + 3000));
}
