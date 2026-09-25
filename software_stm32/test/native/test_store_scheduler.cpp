#include <stdint.h>

#include "doctest.h"
#include "vdm/config_store.h"
#include "vdm/replies_v2.h"
#include "vdm/store_scheduler.h"

using vdm::StoreScheduler;
using Step = vdm::StoreScheduler::Step;

namespace {

// ticks until tick() returns `step`, at most limit (0: never); every other result must be None
uint32_t ticksUntil(StoreScheduler& s, Step step, uint32_t limit) {
  for (uint32_t n = 1; n <= limit; n++) {
    const Step got = s.tick();
    if (got == step) return n;
    REQUIRE(got == Step::None);
  }
  return 0;
}

// a change written and the write failed kWriteAttempts times: writeFailed
void giveUp(StoreScheduler& s) {
  s.changed(vdm::kChangedMotor);
  REQUIRE(ticksUntil(s, Step::Write, 10) == 3);
  s.writeResult(false);
  REQUIRE(s.tick() == Step::Write);
  s.writeResult(false);
  REQUIRE(s.tick() == Step::Write);
  s.writeResult(false);
  REQUIRE(s.writeFailed());
}

}  // namespace

TEST_CASE("StoreScheduler: nothing to do without a change") {
  StoreScheduler s;
  CHECK(ticksUntil(s, Step::Write, 5000) == 0);
  CHECK(s.dirty() == 0);
  CHECK(s.eepState() == vdm::kEepStateOk);
  CHECK(s.free());
  CHECK_FALSE(s.retrying());
  CHECK_FALSE(s.readFailed());
  CHECK_FALSE(s.writeFailed());
}

TEST_CASE("StoreScheduler: one change is written 3 ticks after it, not as a retry") {
  StoreScheduler s;
  s.changed(vdm::kChangedMovements);
  CHECK(s.eepState() == vdm::kEepStatePending);
  CHECK_FALSE(s.free());
  CHECK(s.tick() == Step::None);
  CHECK(s.tick() == Step::None);
  CHECK(s.tick() == Step::Write);
  CHECK_FALSE(s.retrying());
  CHECK(s.dirty() == vdm::kChangedMovements);
}

TEST_CASE("StoreScheduler: changes at ticks 0, 2 and 4 are written at tick 7") {
  StoreScheduler s;
  s.changed(vdm::kChangedMotor);
  CHECK(s.tick() == Step::None);
  CHECK(s.tick() == Step::None);
  s.changed(vdm::kChangedLease);
  CHECK(s.tick() == Step::None);
  CHECK(s.tick() == Step::None);
  s.changed(vdm::kChangedFailsafe);
  CHECK(ticksUntil(s, Step::Write, 10) == 3);
  CHECK(s.dirty() == (vdm::kChangedMotor | vdm::kChangedLease | vdm::kChangedFailsafe));
}

TEST_CASE("StoreScheduler: a change every tick is written 30 ticks after the first one") {
  StoreScheduler s;
  // an earlier write, so the counters do not start from their initial values
  s.changed(vdm::kChangedMotor);
  REQUIRE(ticksUntil(s, Step::Write, 10) == 3);
  s.writeResult(true);
  s.changed(vdm::kChangedSensors);
  uint32_t n = 0;
  for (n = 1; n <= 40; n++) {
    if (s.tick() == Step::Write) break;
    s.changed(vdm::kChangedSensors);
  }
  CHECK(n == 30);
}

TEST_CASE("StoreScheduler: a successful write ends the change") {
  StoreScheduler s;
  s.changed(vdm::kChangedEscalation);
  REQUIRE(ticksUntil(s, Step::Write, 10) == 3);
  s.writeResult(true);
  CHECK(s.dirty() == 0);
  CHECK(s.eepState() == vdm::kEepStateOk);
  CHECK(s.free());
  CHECK_FALSE(s.retrying());
  CHECK(ticksUntil(s, Step::Write, 5000) == 0);
}

TEST_CASE("StoreScheduler: a failed write is repeated at the next ticks, the third failure gives up") {
  StoreScheduler s;
  s.changed(vdm::kChangedLearnTime);
  REQUIRE(ticksUntil(s, Step::Write, 10) == 3);
  s.writeResult(false);
  CHECK(s.eepState() == vdm::kEepStatePending);
  CHECK_FALSE(s.free());
  CHECK_FALSE(s.writeFailed());
  CHECK(s.retrying());
  CHECK(s.tick() == Step::Write);
  s.writeResult(false);
  CHECK(s.tick() == Step::Write);
  CHECK(s.retrying());
  s.writeResult(false);
  CHECK(s.writeFailed());
  CHECK(s.eepState() == vdm::kEepStateWriteFailed);
  CHECK(s.free());
  CHECK(s.dirty() == vdm::kChangedLearnTime);
}

TEST_CASE("StoreScheduler: a change after a failed attempt restarts the debounce of the repetition") {
  StoreScheduler s;
  s.changed(vdm::kChangedLearnTime);
  REQUIRE(ticksUntil(s, Step::Write, 10) == 3);
  s.writeResult(false);
  s.changed(vdm::kChangedLease);
  CHECK(ticksUntil(s, Step::Write, 10) == 3);
  CHECK(s.retrying());
  s.writeResult(false);
  CHECK(s.tick() == Step::Write);
  s.writeResult(false);
  CHECK(s.writeFailed());
}

TEST_CASE("StoreScheduler: after the attempts the write is retried after 30, 60, ... 3600 ticks") {
  StoreScheduler s;
  giveUp(s);
  const uint32_t expected[] = {30, 60, 120, 240, 480, 960, 1920, 3600, 3600};
  for (uint32_t interval : expected) {
    CAPTURE(interval);
    CHECK(ticksUntil(s, Step::Write, 5000) == interval);
    CHECK(s.retrying());
    s.writeResult(false);  // a failed retry gives up at once
    CHECK(s.eepState() == vdm::kEepStateWriteFailed);
  }
}

TEST_CASE("StoreScheduler: a retry that succeeds clears the failure, the next change has 3 attempts") {
  StoreScheduler s;
  giveUp(s);
  REQUIRE(ticksUntil(s, Step::Write, 100) == 30);
  s.writeResult(true);
  CHECK_FALSE(s.writeFailed());
  CHECK(s.eepState() == vdm::kEepStateOk);
  CHECK(s.dirty() == 0);
  CHECK_FALSE(s.retrying());
  CHECK(ticksUntil(s, Step::Write, 5000) == 0);
  giveUp(s);  // three attempts again
  CHECK(ticksUntil(s, Step::Write, 100) == 30);  // the backoff starts again with the first interval
}

TEST_CASE("StoreScheduler: a change while the write failed gets one attempt after the debounce") {
  StoreScheduler s;
  giveUp(s);
  s.changed(vdm::kChangedCalib);
  CHECK(s.eepState() == vdm::kEepStatePending);
  CHECK_FALSE(s.free());
  CHECK(ticksUntil(s, Step::Write, 10) == 3);
  CHECK(s.retrying());
  CHECK(s.dirty() == (vdm::kChangedMotor | vdm::kChangedCalib));
  s.writeResult(false);
  CHECK(s.eepState() == vdm::kEepStateWriteFailed);
  CHECK(ticksUntil(s, Step::Write, 5000) == 60);
}

TEST_CASE("StoreScheduler: after a failed read nothing is written, the read is repeated") {
  StoreScheduler s;
  s.readResult(false);
  CHECK(s.readFailed());
  CHECK(s.eepState() == vdm::kEepStateReadFailed);
  CHECK(s.free());
  s.changed(vdm::kChangedSensors);
  CHECK(s.eepState() == vdm::kEepStateReadFailed);
  CHECK(s.free());
  CHECK(ticksUntil(s, Step::Reread, 100) == 30);
  CHECK(s.retrying());
  s.readResult(false);
  CHECK(ticksUntil(s, Step::Reread, 100) == 60);
  s.readResult(true);
  CHECK_FALSE(s.readFailed());
  CHECK(s.eepState() == vdm::kEepStatePending);
  CHECK_FALSE(s.free());
  // the change made meanwhile is written after the debounce, as a first attempt
  CHECK(ticksUntil(s, Step::Write, 10) == 3);
  CHECK_FALSE(s.retrying());
  CHECK(s.dirty() == vdm::kChangedSensors);
}

TEST_CASE("StoreScheduler: a successful read schedules nothing") {
  StoreScheduler s;
  s.readResult(true);
  CHECK_FALSE(s.readFailed());
  CHECK(s.eepState() == vdm::kEepStateOk);
  CHECK(ticksUntil(s, Step::Reread, 5000) == 0);
  CHECK(ticksUntil(s, Step::Write, 5000) == 0);
}

TEST_CASE("StoreScheduler: the retry intervals of the constructor") {
  StoreScheduler s(5, 12);
  s.readResult(false);
  CHECK(ticksUntil(s, Step::Reread, 100) == 5);
  s.readResult(false);
  CHECK(ticksUntil(s, Step::Reread, 100) == 10);
  s.readResult(false);
  CHECK(ticksUntil(s, Step::Reread, 100) == 12);
}
