#include <stdint.h>

#include "doctest.h"
#include "vdm/valve_scheduler.h"

using vdm::ActionKind;
using vdm::Decision;
using vdm::StopReason;
using vdm::ValveScheduler;
using vdm::ValveView;

namespace {

using Views = ValveView[vdm::kValveCount];

// every valve idle, calibrated and at its target 50
void idleAll(Views& v) {
  for (uint8_t i = 0; i < vdm::kValveCount; i++) {
    v[i] = ValveView{};
    v[i].status = vdm::kStIdle;
    v[i].actual = 50;
    v[i].target = 50;
    v[i].drive = 50;
    v[i].calibrated = true;
  }
}

const vdm::SchedulerInputs kRun{false, false};

bool is(const Decision& d, ActionKind k, uint8_t valve) { return d.kind == k && d.valve == valve; }

}  // namespace

TEST_CASE("ValveScheduler: a presence test clears the end-stop latch of the valve") {
  Views v;
  idleAll(v);
  v[2].drive = 70;
  ValveScheduler s;
  CHECK(is(s.next(v, kRun), ActionKind::Open, 2));  // test index 0
  s.moveEnded(2, StopReason::EndStop, false, vdm::kStIdle);
  REQUIRE(s.latched(2));
  v[2].status = vdm::kStUnknown;
  CHECK(is(s.next(v, kRun), ActionKind::Test, 2));  // test index 2
  CHECK_FALSE(s.latched(2));
}

TEST_CASE("ValveScheduler: a move the other way keeps the retry of an early end stop") {
  Views v;
  idleAll(v);
  v[3].drive = 60;
  v[3].actual = 20;
  ValveScheduler s;
  CHECK(is(s.next(v, kRun), ActionKind::Open, 3));
  s.moveEnded(3, StopReason::EarlyEndStop, true, vdm::kStIdle);
  REQUIRE(s.latched(3));
  // the valve is above its drive value: a close move, not reported as ended
  v[3].actual = 90;
  CHECK(is(s.next(v, kRun), ActionKind::Close, 3));
  // below again: the one retry in the open direction is still there
  v[3].actual = 20;
  CHECK(is(s.next(v, kRun), ActionKind::Open, 3));
}

TEST_CASE("ValveScheduler: a full-open move that ends at the end stop latches, early with one retry") {
  Views v;
  idleAll(v);
  v[4].status = vdm::kStFullOpen;
  v[4].drive = 100;
  v[4].actual = 40;
  ValveScheduler s;
  CHECK(is(s.next(v, kRun), ActionKind::OpenEnd, 4));
  s.moveEnded(4, StopReason::EarlyEndStop, true, vdm::kStIdle);
  CHECK(s.latched(4));
  v[4].status = vdm::kStIdle;
  v[4].actual = 90;
  // the retry of the early end stop
  CHECK(is(s.next(v, kRun), ActionKind::OpenEnd, 4));
  s.moveEnded(4, StopReason::EndStop, false, vdm::kStIdle);
  CHECK(s.latched(4));
  CHECK(s.next(v, kRun).kind == ActionKind::None);
}

TEST_CASE("ValveScheduler: a reference move clears the latch when it is decided") {
  Views v;
  idleAll(v);
  v[5].drive = 70;
  ValveScheduler s;
  CHECK(is(s.next(v, kRun), ActionKind::Open, 5));
  s.moveEnded(5, StopReason::EndStop, false, vdm::kStIdle);
  REQUIRE(s.latched(5));
  v[5].needsReference = true;
  const Decision d = s.next(v, kRun);
  CHECK(is(d, ActionKind::OpenEnd, 5));
  CHECK(d.reference);
  CHECK_FALSE(s.latched(5));
}

TEST_CASE("ValveScheduler: a calibration drops the pending move of the valve") {
  Views v;
  idleAll(v);
  v[6].drive = 70;
  ValveScheduler s;
  CHECK(is(s.next(v, kRun), ActionKind::Open, 6));
  s.moveEnded(6, StopReason::Target, false, vdm::kStIdle);
  CHECK_FALSE(s.latched(6));
  v[6].actual = 70;
  v[6].status = vdm::kStPresent;
  CHECK(is(s.next(v, kRun), ActionKind::Learn, 6));
  s.moveEnded(6, StopReason::EndStop, false, vdm::kStIdle);
  CHECK_FALSE(s.latched(6));
}

TEST_CASE("ValveScheduler: the same move at an end stop latches again after the latch was cleared") {
  Views v;
  idleAll(v);
  v[7].drive = 70;
  ValveScheduler s;
  CHECK(is(s.next(v, kRun), ActionKind::Open, 7));
  s.moveEnded(7, StopReason::EndStop, false, vdm::kStIdle);
  REQUIRE(s.latched(7));
  s.clearLatch(7);
  CHECK(is(s.next(v, kRun), ActionKind::Open, 7));
  s.moveEnded(7, StopReason::EndStop, false, vdm::kStIdle);
  CHECK(s.latched(7));
}

TEST_CASE("ValveScheduler: an end stop the other way at the same drive value latches that way") {
  Views v;
  idleAll(v);
  v[8].drive = 50;
  v[8].actual = 20;
  ValveScheduler s;
  CHECK(is(s.next(v, kRun), ActionKind::Open, 8));
  s.moveEnded(8, StopReason::EndStop, false, vdm::kStIdle);
  REQUIRE(s.latched(8));
  v[8].actual = 80;
  CHECK(is(s.next(v, kRun), ActionKind::Close, 8));
  s.moveEnded(8, StopReason::EndStop, false, vdm::kStIdle);
  // latched closing now: no further close move, an open move is allowed
  CHECK(s.next(v, kRun).kind == ActionKind::None);
  v[8].actual = 20;
  CHECK(is(s.next(v, kRun), ActionKind::Open, 8));
}

TEST_CASE("ValveScheduler: clearLatch of an invalid valve touches no other valve") {
  Views v;
  idleAll(v);
  v[0].drive = 70;
  ValveScheduler s;
  CHECK(is(s.next(v, kRun), ActionKind::Open, 0));
  s.clearLatch(vdm::kValveCount);
  s.clearLatch(255);
  s.moveEnded(0, StopReason::EndStop, false, vdm::kStIdle);
  CHECK(s.latched(0));
}
