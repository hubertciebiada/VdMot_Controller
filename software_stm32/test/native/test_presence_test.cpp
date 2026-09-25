#include <stdint.h>

#include <vector>

#include "doctest.h"
#include "vdm/presence_test.h"

using Result = vdm::PresenceTest::Result;

namespace {

// sample index (1-based) of the first result != Pending, 0 if none within n samples
int firstResult(vdm::PresenceTest& t, int32_t current, int n, Result& r) {
  for (int k = 1; k <= n; k++) {
    r = t.sample(current);
    if (r != Result::Pending) return k;
  }
  return 0;
}

int firstResult(bool shortCheck, bool enforce, int32_t current, Result& r) {
  vdm::PresenceTest t;
  t.start(shortCheck, enforce);
  return firstResult(t, current, 200, r);
}

}  // namespace

TEST_CASE("PresenceTest: constants") {
  CHECK(vdm::PresenceTest::kSettleTicks == 7);
  CHECK(vdm::PresenceTest::kNoCurrent == 20);
  CHECK(vdm::PresenceTest::kAbsentTicks == 80);
  CHECK(vdm::PresenceTest::kPresentTicks == 5);
  CHECK(vdm::PresenceTest::kShortLimit == 2000);
  CHECK(vdm::PresenceTest::kShortTicks == 3);
}

TEST_CASE("PresenceTest: no current is absent at sample 88, pending before") {
  Result r = Result::Pending;
  CHECK(firstResult(true, true, 0, r) == 88);
  CHECK(r == Result::Absent);
  CHECK(firstResult(true, true, 19, r) == 88);
  CHECK(firstResult(true, true, -19, r) == 88);
  CHECK(r == Result::Absent);
}

TEST_CASE("PresenceTest: a motor current is present at sample 13") {
  Result r = Result::Pending;
  CHECK(firstResult(true, true, 300, r) == 13);
  CHECK(r == Result::Present);
  CHECK(firstResult(true, true, 20, r) == 13);
  CHECK(r == Result::Present);
  CHECK(firstResult(true, true, -20, r) == 13);
  CHECK(firstResult(true, true, 2000, r) == 13);  // the short limit itself is not a short
  CHECK(r == Result::Present);
}

TEST_CASE("PresenceTest: a short is found at sample 10 when enforced") {
  Result r = Result::Pending;
  CHECK(firstResult(true, true, 2500, r) == 10);
  CHECK(r == Result::Short);
  CHECK(firstResult(true, true, -2001, r) == 10);
  CHECK(r == Result::Short);
  vdm::PresenceTest t;
  t.start(true, true);
  firstResult(t, 2500, 200, r);
  CHECK(t.shortSeen());
}

TEST_CASE("PresenceTest: only three consecutive samples above the limit are a short") {
  vdm::PresenceTest t;
  t.start(true, true);
  for (int k = 1; k <= 7; k++) CHECK(t.sample(2500) == Result::Pending);
  CHECK(t.sample(2500) == Result::Pending);
  CHECK(t.sample(2500) == Result::Pending);
  CHECK(t.sample(1500) == Result::Pending);
  CHECK(t.sample(2500) == Result::Pending);
  CHECK(t.sample(2500) == Result::Pending);
  CHECK(t.sample(2500) == Result::Short);  // sample 13: the 6th normal sample would be present
}

TEST_CASE("PresenceTest: report only records the short and goes on like 1.x") {
  vdm::PresenceTest t;
  t.start(true, false);
  Result r = Result::Pending;
  CHECK(firstResult(t, 2500, 200, r) == 13);
  CHECK(r == Result::Present);
  CHECK(t.shortSeen());
  t.start(true, false);
  CHECK_FALSE(t.shortSeen());
  CHECK(firstResult(t, 300, 200, r) == 13);
  CHECK_FALSE(t.shortSeen());
}

TEST_CASE("PresenceTest: with the short check off a short is never seen") {
  vdm::PresenceTest t;
  t.start(false, true);
  Result r = Result::Pending;
  CHECK(firstResult(t, 2800, 200, r) == 13);
  CHECK(r == Result::Present);
  CHECK_FALSE(t.shortSeen());
}

TEST_CASE("PresenceTest: start() begins a new test") {
  vdm::PresenceTest t;
  t.start(true, true);
  for (int k = 0; k < 50; k++) t.sample(0);
  t.start(true, true);
  Result r = Result::Pending;
  CHECK(firstResult(t, 0, 200, r) == 88);
  t.start(true, true);
  for (int k = 0; k < 9; k++) t.sample(2500);
  t.start(true, true);
  CHECK(firstResult(t, 300, 200, r) == 13);
  CHECK(r == Result::Present);
}

TEST_CASE("presenceOutcome: status, reference and fault per result") {
  vdm::PresenceOutcome o = vdm::presenceOutcome(Result::Present, true, false);
  CHECK(+o.status == 1);
  CHECK(o.needsReference);
  CHECK(+o.fault == 0);
  o = vdm::presenceOutcome(Result::Present, true, true);
  CHECK(+o.status == 8);
  CHECK_FALSE(o.needsReference);
  o = vdm::presenceOutcome(Result::Present, false, false);
  CHECK(+o.status == 8);
  CHECK_FALSE(o.needsReference);
  o = vdm::presenceOutcome(Result::Present, false, true);
  CHECK(+o.status == 8);
  o = vdm::presenceOutcome(Result::Absent, true, false);
  CHECK(+o.status == 6);
  CHECK_FALSE(o.needsReference);
  CHECK(+o.fault == 0);
  o = vdm::presenceOutcome(Result::Short, true, false);
  CHECK(+o.status == 4);
  CHECK(+o.fault == 3);
  CHECK_FALSE(o.needsReference);
  o = vdm::presenceOutcome(Result::Pending, true, false);
  CHECK(+o.status == 5);
  CHECK_FALSE(o.needsReference);
  CHECK(+o.fault == 0);
}
