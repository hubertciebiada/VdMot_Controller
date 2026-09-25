#include <stdint.h>

#include "doctest.h"
#include "vdm/fault_retry.h"

using vdm::FaultRetry;

TEST_CASE("faultRetryInterval: 1 h, 6 h, then every 24 h") {
  CHECK(vdm::faultRetryInterval(0) == 3600);
  CHECK(vdm::faultRetryInterval(1) == 21600);
  CHECK(vdm::faultRetryInterval(2) == 86400);
  CHECK(vdm::faultRetryInterval(3) == 86400);
  CHECK(vdm::faultRetryInterval(255) == 86400);
}

TEST_CASE("FaultRetry: schedules on the first faulted call and fires after exactly 3600 s") {
  FaultRetry r;
  CHECK_FALSE(r.scheduled());
  CHECK(r.remainingS() == 0);
  CHECK_FALSE(r.update(true, false, 50));  // the elapsed time of the scheduling call does not count
  CHECK(r.scheduled());
  CHECK(r.remainingS() == 3600);
  CHECK(r.attempts() == 0);
  CHECK_FALSE(r.update(true, false, 3590));
  CHECK(r.remainingS() == 10);
  CHECK(r.update(true, false, 10));
  CHECK_FALSE(r.scheduled());
  CHECK(r.remainingS() == 0);
  CHECK(r.attempts() == 1);
}

TEST_CASE("FaultRetry: a late call fires once, the retry runs (busy), a new fault waits 6 h") {
  FaultRetry r;
  r.update(true, false, 0);
  CHECK(r.update(true, false, 4000));
  CHECK_FALSE(r.update(true, false, 1));  // not busy yet: schedules the next interval
  CHECK(r.remainingS() == 21600);
  CHECK_FALSE(r.update(false, true, 1));   // the retry calibration runs
  CHECK_FALSE(r.scheduled());
  CHECK(r.attempts() == 1);
  CHECK_FALSE(r.update(true, true, 1));    // still requested
  CHECK_FALSE(r.scheduled());
  CHECK_FALSE(r.update(true, false, 1));   // failed again
  CHECK(r.remainingS() == 21600);
  CHECK(r.update(true, false, 21600));
  CHECK(r.attempts() == 2);
  r.update(true, false, 0);
  CHECK(r.remainingS() == 86400);
}

TEST_CASE("FaultRetry: a valve that is fine again resets the attempts") {
  FaultRetry r;
  r.update(true, false, 0);
  CHECK(r.update(true, false, 3600));
  CHECK(r.attempts() == 1);
  CHECK_FALSE(r.update(false, false, 1));
  CHECK(r.attempts() == 0);
  CHECK_FALSE(r.scheduled());
  r.update(true, false, 0);
  CHECK(r.remainingS() == 3600);
}

TEST_CASE("FaultRetry: busy while scheduled drops the schedule and keeps the attempts") {
  FaultRetry r;
  r.update(true, false, 0);
  CHECK(r.update(true, false, 3600));
  r.update(true, false, 0);
  r.update(true, false, 100);
  CHECK(r.remainingS() == 21500);
  CHECK_FALSE(r.update(true, true, 5));
  CHECK_FALSE(r.scheduled());
  CHECK(r.attempts() == 1);
  r.update(true, false, 7);
  CHECK(r.remainingS() == 21600);
}

TEST_CASE("FaultRetry: the attempts saturate at 255") {
  FaultRetry r;
  for (int i = 0; i < 300; i++) {
    r.update(true, false, 0);
    CHECK(r.update(true, false, 86400));
  }
  CHECK(r.attempts() == 255);
}

TEST_CASE("FaultRetry: snapshot and restore") {
  FaultRetry a;
  a.update(true, false, 0);
  a.update(true, false, 600);
  const FaultRetry::Snapshot s = a.snapshot();
  CHECK(s.attempts == 0);
  CHECK(s.scheduled);
  CHECK(s.remainingS == 3000);
  FaultRetry b;
  b.restore(s);
  CHECK(b.scheduled());
  CHECK(b.remainingS() == 3000);
  CHECK(b.update(true, false, 3000));
  CHECK(b.attempts() == 1);
  b.restore(FaultRetry::Snapshot{7, false, 123});
  CHECK(b.attempts() == 7);
  CHECK_FALSE(b.scheduled());
  CHECK(b.remainingS() == 0);
  CHECK(b.snapshot().remainingS == 0);
  b.restore(FaultRetry::Snapshot{2, true, 5});
  CHECK_FALSE(b.update(true, false, 4));
  CHECK(b.remainingS() == 1);
}
