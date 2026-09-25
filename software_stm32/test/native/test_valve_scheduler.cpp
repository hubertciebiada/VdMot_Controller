#include <stdint.h>

#include <string>

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

std::string str(const Decision& d) {
  static const char* const kNames[] = {"None", "Test", "OpenEnd", "CloseEnd", "Open", "Close", "Learn", "MarkPresent"};
  if (d.kind == ActionKind::None) return "None";
  std::string s = std::string(kNames[static_cast<int>(d.kind)]) + " " + std::to_string(d.valve);
  if (d.kind == ActionKind::Open || d.kind == ActionKind::Close) s += " " + std::to_string(d.delta);
  if (d.keepStatus) s += " keep";
  if (d.reference) s += " ref";
  return s;
}

std::string next(ValveScheduler& s, const Views& v) { return str(s.next(v, kRun)); }

// the valve state machine did what the decision asked, without an end stop
void apply(Views& v, const Decision& d) {
  ValveView& x = v[d.valve];
  switch (d.kind) {
    case ActionKind::OpenEnd:
      x.actual = 100;
      break;
    case ActionKind::CloseEnd:
      x.actual = 0;
      break;
    case ActionKind::Open:
      x.actual = static_cast<uint8_t>(x.actual + d.delta);
      break;
    case ActionKind::Close:
      x.actual = static_cast<uint8_t>(x.actual - d.delta);
      break;
    default:
      break;
  }
}

}  // namespace

TEST_CASE("ValveScheduler: nothing in safe mode or while a temperature cycle holds the motors") {
  Views v;
  idleAll(v);
  v[0].status = vdm::kStUnknown;
  v[3].drive = 70;
  ValveScheduler s;
  CHECK(str(s.next(v, vdm::SchedulerInputs{true, false})) == "None");
  CHECK(str(s.next(v, vdm::SchedulerInputs{false, true})) == "None");
  CHECK(str(s.next(v, vdm::SchedulerInputs{true, true})) == "None");
  // the test index did not move
  CHECK(next(s, v) == "Test 0");
}

TEST_CASE("ValveScheduler: presence tests in the order 0 2 4 .. 10 1 3 .. 11 0") {
  Views v;
  idleAll(v);
  for (auto& x : v) x.status = vdm::kStUnknown;
  ValveScheduler s;
  std::string order;
  for (int i = 0; i < 13; i++) order += next(s, v) + ",";
  CHECK(order ==
        "Test 0,Test 2,Test 4,Test 6,Test 8,Test 10,Test 1,Test 3,Test 5,Test 7,Test 9,Test 11,Test 0,");
}

TEST_CASE("ValveScheduler: the test index advances also when its valve is known") {
  Views v;
  idleAll(v);
  v[4].status = vdm::kStUnknown;
  ValveScheduler s;
  CHECK(next(s, v) == "None");  // index 0
  CHECK(next(s, v) == "None");  // index 2
  CHECK(next(s, v) == "Test 4");
}

TEST_CASE("ValveScheduler: plain moves by the difference, to the end stops for 0 and 100") {
  Views v;
  idleAll(v);
  v[0].drive = 70;
  v[1].drive = 100;
  v[2].drive = 20;
  v[3].drive = 0;
  ValveScheduler s;
  CHECK_FALSE(s.firstChange());
  Decision d = s.next(v, kRun);
  CHECK(str(d) == "Open 0 20");
  CHECK(s.firstChange());
  apply(v, d);
  d = s.next(v, kRun);
  CHECK(str(d) == "OpenEnd 1");
  apply(v, d);
  d = s.next(v, kRun);
  CHECK(str(d) == "Close 2 30");
  apply(v, d);
  d = s.next(v, kRun);
  CHECK(str(d) == "CloseEnd 3");
  apply(v, d);
  CHECK(next(s, v) == "None");
}

TEST_CASE("ValveScheduler: a service hold, a request or a pending calibration keeps a valve from step 2") {
  Views v;
  idleAll(v);
  v[0].drive = 70;
  v[0].svcHold = true;
  v[1].drive = 70;
  v[1].timedLearn = true;
  v[2].drive = 70;
  v[2].recal = true;
  v[2].svcHold = true;
  v[3].drive = 70;
  v[3].calibrated = false;
  v[3].svcHold = true;
  ValveScheduler s;
  // valve 1: marked present (step 3c); valves 2, 3 pending but held
  CHECK(next(s, v) == "MarkPresent 1");
  v[1].status = vdm::kStPresent;
  CHECK(next(s, v) == "Learn 1");
  v[1] = v[4];
  CHECK(next(s, v) == "None");
  v[3].svcHold = false;
  CHECK(next(s, v) == "Learn 3");
}

TEST_CASE("ValveScheduler: full open, open circuit retest, known failed valves") {
  Views v;
  idleAll(v);
  v[2].status = vdm::kStFullOpen;
  v[2].actual = 100;
  v[2].target = 100;
  v[2].drive = 100;
  v[5].status = vdm::kStOpenCircuit;
  v[5].drive = 40;
  v[5].actual = 40;
  v[6].status = vdm::kStFailed;
  v[6].drive = 80;
  ValveScheduler s;
  CHECK(next(s, v) == "OpenEnd 2");
  v[2].status = vdm::kStIdle;
  CHECK(next(s, v) == "None");
  v[5].drive = 60;
  v[5].target = 60;
  CHECK(next(s, v) == "Test 5");
}

TEST_CASE("ValveScheduler: an open-circuit valve is tested once per target change (C-3)") {
  Views v;
  idleAll(v);
  v[0].status = vdm::kStOpenCircuit;
  v[0].actual = 20;
  v[0].target = 20;
  v[0].drive = 20;
  ValveScheduler s;
  CHECK(next(s, v) == "None");
  v[0].target = 30;
  v[0].drive = 30;
  CHECK(next(s, v) == "Test 0");
  v[0].actual = 30;  // A_TEST "absent" takes the drive target
  for (int i = 0; i < 20; i++) CHECK(next(s, v) == "None");
}

TEST_CASE("ValveScheduler: blocked valve to its failsafe position with the status kept (K2-2)") {
  Views v;
  idleAll(v);
  v[3].status = vdm::kStBlocked;
  v[3].actual = 0;
  v[3].target = 20;
  v[3].drive = 50;
  v[3].blockedFailsafe = true;
  ValveScheduler s;
  Decision d = s.next(v, kRun);
  CHECK(str(d) == "Open 3 50 keep");
  CHECK_FALSE(s.firstChange());
  apply(v, d);
  CHECK(next(s, v) == "None");
  // hold (255): the drive is the target, the source not BlockedFailsafe
  v[3].actual = 0;
  v[3].drive = 20;
  v[3].blockedFailsafe = false;
  CHECK(next(s, v) == "None");
  v[3].drive = 50;
  v[3].blockedFailsafe = true;
  v[3].svcHold = true;
  CHECK(next(s, v) == "None");
  v[3].svcHold = false;
  v[3].drive = 100;
  CHECK(next(s, v) == "OpenEnd 3 keep");
  v[3].drive = 0;
  v[3].actual = 30;
  CHECK(next(s, v) == "CloseEnd 3 keep");
  v[3].drive = 10;
  CHECK(next(s, v) == "Close 3 20 keep");
}

TEST_CASE("ValveScheduler: a failed or blocked valve is calibrated only on staln or its retry (C-1)") {
  Views v;
  idleAll(v);
  v[1].status = vdm::kStBlocked;
  v[1].blockedFailsafe = true;
  v[1].earlyLearn = true;
  v[1].timedLearn = true;
  v[1].calibFlag = true;
  v[2].status = vdm::kStFailed;
  v[2].earlyLearn = true;
  v[2].timedLearn = true;
  ValveScheduler s;
  CHECK(next(s, v) == "None");
  v[1].retryLearn = true;
  CHECK(next(s, v) == "MarkPresent 1");
  v[2].forcedLearn = true;
  CHECK(next(s, v) == "MarkPresent 2");
  // a request keeps the blocked valve from its failsafe move
  v[1].status = vdm::kStBlocked;
  v[1].drive = 60;
  CHECK(next(s, v) == "MarkPresent 1");
}

TEST_CASE("ValveScheduler: needsReference goes to the nearer end stop first (W2-4)") {
  Views v;
  idleAll(v);
  v[0].needsReference = true;
  v[0].actual = 30;
  v[0].drive = 70;
  ValveScheduler s;
  Decision d = s.next(v, kRun);
  CHECK(str(d) == "OpenEnd 0 ref");
  CHECK(s.firstChange());
  apply(v, d);
  s.moveEnded(0, StopReason::EndStop, false, vdm::kStIdle);
  v[0].needsReference = false;
  CHECK(next(s, v) == "Close 0 30");

  Views w;
  idleAll(w);
  w[4].needsReference = true;
  w[4].actual = 30;
  w[4].drive = 20;
  ValveScheduler t;
  CHECK(next(t, w) == "CloseEnd 4 ref");
  w[4].drive = 50;
  CHECK(next(t, w) == "OpenEnd 4 ref");
  w[4].drive = 49;
  CHECK(next(t, w) == "CloseEnd 4 ref");
}

TEST_CASE("ValveScheduler: a reference move also when touched or lease forced (C-2)") {
  Views v;
  idleAll(v);
  v[0].needsReference = true;
  ValveScheduler s;
  CHECK(next(s, v) == "None");
  v[0].leaseForced = true;
  CHECK(next(s, v) == "OpenEnd 0 ref");
  v[0].leaseForced = false;
  v[0].touched = true;
  CHECK(next(s, v) == "OpenEnd 0 ref");
  // after the reference: needsReference and touched clear, nothing more
  v[0].touched = false;
  v[0].needsReference = false;
  CHECK(next(s, v) == "None");
  // touched alone does nothing for a referenced valve
  v[0].touched = true;
  CHECK(next(s, v) == "None");
  v[0].svcHold = true;
  v[0].needsReference = true;
  CHECK(next(s, v) == "None");
}

TEST_CASE("ValveScheduler: a present valve calibrates after a change, a request or when due (C-2)") {
  Views v;
  idleAll(v);
  v[5].status = vdm::kStPresent;
  v[5].calibrated = false;
  ValveScheduler s;
  CHECK(next(s, v) == "None");
  v[5].leaseForced = true;
  CHECK(next(s, v) == "Learn 5");
  CHECK_FALSE(s.firstChange());
  v[5].leaseForced = false;
  v[5].touched = true;
  CHECK(next(s, v) == "Learn 5");
  v[5].touched = false;
  v[5].forcedLearn = true;
  CHECK(next(s, v) == "Learn 5");
  v[5].forcedLearn = false;
  v[5].calibFlag = true;
  CHECK(next(s, v) == "Learn 5");
  v[5].calibFlag = false;
  v[5].retryLearn = true;
  CHECK(next(s, v) == "Learn 5");
  v[5].retryLearn = false;
  v[5].earlyLearn = true;
  CHECK(next(s, v) == "Learn 5");
  v[5].earlyLearn = false;
  v[5].timedLearn = true;
  CHECK(next(s, v) == "None");
  v[5].drive = 60;
  CHECK(next(s, v) == "Learn 5");
  CHECK(s.firstChange());
  v[5].drive = 50;
  CHECK(next(s, v) == "Learn 5");  // after the first change every present valve calibrates
}

TEST_CASE("ValveScheduler: a counted rejection is a change for the present valves") {
  Views v;
  idleAll(v);
  v[5].status = vdm::kStPresent;
  ValveScheduler s;
  CHECK(next(s, v) == "None");
  s.noteChange();
  CHECK(s.firstChange());
  CHECK(next(s, v) == "Learn 5");
}

TEST_CASE("ValveScheduler: calibration pending at the valve's own target change (W12-1)") {
  Views v;
  idleAll(v);
  v[0].calibrated = false;
  v[0].actual = 100;
  v[0].target = 100;
  v[0].drive = 100;
  ValveScheduler s;
  CHECK(next(s, v) == "None");
  CHECK_FALSE(s.firstChange());
  v[0].drive = 40;
  CHECK(next(s, v) == "Learn 0");
  CHECK(s.firstChange());
  Views w;
  idleAll(w);
  w[7].recal = true;
  w[7].touched = true;
  ValveScheduler t;
  CHECK(next(t, w) == "Learn 7");
  w[7].touched = false;
  w[7].leaseForced = true;
  CHECK(next(t, w) == "Learn 7");
}

TEST_CASE("ValveScheduler: a leased-out valve status 8 after power-on calibrates with drive == actual (C-2)") {
  Views v;
  idleAll(v);
  v[2].status = vdm::kStPresent;
  v[2].calibrated = false;
  v[2].leaseForced = true;
  ValveScheduler s;
  CHECK(next(s, v) == "Learn 2");
}

TEST_CASE("ValveScheduler: requests of known valves mark them present, not unknown or present ones") {
  Views v;
  idleAll(v);
  v[0].status = vdm::kStUnknown;
  v[0].forcedLearn = true;
  v[1].status = vdm::kStOpenCircuit;
  v[1].forcedLearn = true;
  v[2].calibFlag = true;
  v[3].earlyLearn = true;
  ValveScheduler s;
  CHECK(next(s, v) == "Test 0");
  CHECK(next(s, v) == "MarkPresent 1");
  CHECK(next(s, v) == "MarkPresent 2");
  CHECK(next(s, v) == "MarkPresent 3");
  v[1].status = vdm::kStPresent;
  v[2].status = vdm::kStPresent;
  v[3].status = vdm::kStPresent;
  v[1].forcedLearn = false;
  v[2].calibFlag = false;
  v[3].earlyLearn = false;
  CHECK(next(s, v) == "None");
}

TEST_CASE("ValveScheduler: moves of every valve before the next calibration (S8-1)") {
  Views v;
  idleAll(v);
  v[3].status = vdm::kStPresent;
  v[3].forcedLearn = true;
  v[5].drive = 80;
  ValveScheduler s;
  Decision d = s.next(v, kRun);
  CHECK(str(d) == "Open 5 30");
  apply(v, d);
  CHECK(next(s, v) == "Learn 3");

  Views w;
  idleAll(w);
  w[0].status = vdm::kStPresent;
  w[0].forcedLearn = true;
  w[1].status = vdm::kStPresent;
  w[1].forcedLearn = true;
  w[11].drive = 10;
  ValveScheduler t;
  CHECK(next(t, w) == "Close 11 40");
}

TEST_CASE("ValveScheduler: round robin continues after the valve of the last decision") {
  Views v;
  idleAll(v);
  v[2].drive = 60;
  v[8].drive = 60;
  ValveScheduler s;
  CHECK(next(s, v) == "Open 2 10");
  CHECK(next(s, v) == "Open 8 10");
  CHECK(next(s, v) == "Open 2 10");
  v[8].status = vdm::kStPresent;
  v[1].status = vdm::kStPresent;
  v[2].drive = 50;
  // step 3 scans from valve 3 as well
  CHECK(next(s, v) == "Learn 8");
  CHECK(next(s, v) == "Learn 1");
}

TEST_CASE("ValveScheduler: after 12 step-2 decisions in a row a calibration goes first (C-3)") {
  Views v;
  idleAll(v);
  v[0].drive = 70;  // never reaches it: a permanent step-2 item
  v[6].status = vdm::kStPresent;
  v[6].forcedLearn = true;
  ValveScheduler s;
  int learnAt = 0;
  for (int i = 1; i <= 100 && learnAt == 0; i++) {
    if (next(s, v) == "Learn 6") learnAt = i;
  }
  CHECK(learnAt == 13);
  // the run starts again
  v[6].status = vdm::kStIdle;
  v[6].forcedLearn = false;
  v[9].status = vdm::kStPresent;
  v[9].forcedLearn = true;
  learnAt = 0;
  for (int i = 1; i <= 100 && learnAt == 0; i++) {
    if (next(s, v) == "Learn 9") learnAt = i;
  }
  CHECK(learnAt == 13);
}

TEST_CASE("ValveScheduler: fairness looks at step 3 first only while it has something") {
  Views v;
  idleAll(v);
  v[0].drive = 70;
  ValveScheduler s;
  for (int i = 0; i < 20; i++) CHECK(next(s, v) == "Open 0 20");
  v[4].status = vdm::kStPresent;
  v[4].forcedLearn = true;
  CHECK(next(s, v) == "Learn 4");
  for (int i = 0; i < 12; i++) CHECK(next(s, v) == "Open 0 20");
  CHECK(next(s, v) == "Learn 4");
}

TEST_CASE("ValveScheduler: blocked valve at 28 % after its failsafe move met the end stop (C-1)") {
  Views v;
  idleAll(v);
  v[3].status = vdm::kStBlocked;
  v[3].actual = 0;
  v[3].drive = 50;
  v[3].blockedFailsafe = true;
  ValveScheduler s;
  CHECK(next(s, v) == "Open 3 50 keep");
  v[3].actual = 28;
  s.moveEnded(3, StopReason::EarlyEndStop, true, vdm::kStBlocked);
  CHECK(s.latched(3));
  for (int i = 0; i < 1000; i++) {
    if (next(s, v) != "None") {
      FAIL("moved again at call " << i);
      break;
    }
  }
  // a new failsafe position clears the latch
  v[3].drive = 60;
  CHECK(next(s, v) == "Open 3 32 keep");
  CHECK_FALSE(s.latched(3));
}

TEST_CASE("ValveScheduler: a non-early end stop is not repeated (drive 95, actual 94)") {
  Views v;
  idleAll(v);
  v[0].actual = 60;
  v[0].drive = 95;
  ValveScheduler s;
  CHECK(next(s, v) == "Open 0 35");
  v[0].actual = 94;
  s.moveEnded(0, StopReason::EndStop, false, vdm::kStIdle);
  for (int i = 0; i < 50; i++) CHECK(next(s, v) == "None");
  // the other direction is allowed and clears the latch at its end
  v[0].drive = 90;
  CHECK(next(s, v) == "Close 0 4");
  v[0].actual = 90;
  s.moveEnded(0, StopReason::Target, false, vdm::kStIdle);
  CHECK_FALSE(s.latched(0));
  v[0].drive = 95;
  CHECK(next(s, v) == "Open 0 5");
}

TEST_CASE("ValveScheduler: an early end stop gets exactly one retry") {
  Views v;
  idleAll(v);
  v[2].actual = 20;
  v[2].drive = 60;
  ValveScheduler s;
  CHECK(next(s, v) == "Open 2 40");
  v[2].actual = 25;
  s.moveEnded(2, StopReason::EarlyEndStop, true, vdm::kStIdle);
  CHECK(next(s, v) == "Open 2 35");
  s.moveEnded(2, StopReason::EarlyEndStop, true, vdm::kStIdle);
  CHECK(s.latched(2));
  for (int i = 0; i < 20; i++) CHECK(next(s, v) == "None");
  // an early safety stop counts like an early end stop
  Views w;
  idleAll(w);
  w[1].drive = 80;
  ValveScheduler t;
  CHECK(next(t, w) == "Open 1 30");
  w[1].actual = 55;
  t.moveEnded(1, StopReason::SafetyOvercurrent, true, vdm::kStIdle);
  CHECK(next(t, w) == "Open 1 25");
  t.moveEnded(1, StopReason::SafetyOvercurrent, false, vdm::kStIdle);
  CHECK(next(t, w) == "None");
}

TEST_CASE("ValveScheduler: a failsafe move gets no retry after an early end stop") {
  Views v;
  idleAll(v);
  v[3].status = vdm::kStBlocked;
  v[3].actual = 0;
  v[3].drive = 50;
  v[3].blockedFailsafe = true;
  ValveScheduler s;
  CHECK(next(s, v) == "Open 3 50 keep");
  v[3].actual = 10;
  s.moveEnded(3, StopReason::SafetyOvercurrent, true, vdm::kStBlocked);
  CHECK(next(s, v) == "None");
}

TEST_CASE("ValveScheduler: the latch clears with the status, a calibration, a test or clearLatch") {
  Views v;
  idleAll(v);
  v[3].status = vdm::kStBlocked;
  v[3].actual = 0;
  v[3].drive = 50;
  v[3].blockedFailsafe = true;
  ValveScheduler s;
  CHECK(next(s, v) == "Open 3 50 keep");
  v[3].actual = 28;
  s.moveEnded(3, StopReason::EndStop, false, vdm::kStBlocked);
  CHECK(next(s, v) == "None");
  // the retry calibration: marked present, calibrated, blocked again at 0
  v[3].retryLearn = true;
  CHECK(next(s, v) == "MarkPresent 3");
  CHECK_FALSE(s.latched(3));
  v[3].retryLearn = false;
  v[3].actual = 0;
  CHECK(next(s, v) == "Open 3 50 keep");
  v[3].actual = 28;
  s.moveEnded(3, StopReason::EndStop, false, vdm::kStBlocked);
  s.clearLatch(3);
  CHECK_FALSE(s.latched(3));
  s.clearLatch(12);
  CHECK_FALSE(s.latched(12));
  // a status change clears it too
  v[3].actual = 0;
  CHECK(next(s, v) == "Open 3 50 keep");
  v[3].actual = 28;
  s.moveEnded(3, StopReason::EndStop, false, vdm::kStBlocked);
  v[3].status = vdm::kStIdle;
  v[3].blockedFailsafe = false;
  v[3].drive = 50;
  CHECK(next(s, v) == "Open 3 22");
}

TEST_CASE("ValveScheduler: a reference move and other stop reasons set no latch") {
  Views v;
  idleAll(v);
  v[0].needsReference = true;
  v[0].drive = 60;
  ValveScheduler s;
  CHECK(next(s, v) == "OpenEnd 0 ref");
  s.moveEnded(0, StopReason::EndStop, false, vdm::kStIdle);
  CHECK_FALSE(s.latched(0));
  v[0].needsReference = false;
  v[0].actual = 40;
  CHECK(next(s, v) == "Open 0 20");
  s.moveEnded(0, StopReason::Timeout, false, vdm::kStFailed);
  CHECK_FALSE(s.latched(0));
  s.moveEnded(0, StopReason::EndStop, false, vdm::kStIdle);  // no move pending
  CHECK_FALSE(s.latched(0));
  s.moveEnded(12, StopReason::EndStop, false, vdm::kStIdle);
  const StopReason others[] = {StopReason::Target, StopReason::Undercurrent, StopReason::Aborted, StopReason::None};
  for (StopReason r : others) {
    CHECK(next(s, v) == "Open 0 20");
    s.moveEnded(0, r, true, vdm::kStIdle);
    CHECK_FALSE(s.latched(0));
  }
  CHECK(next(s, v) == "Open 0 20");
  s.moveEnded(0, StopReason::EndStop, false, vdm::kStIdle);
  CHECK(s.latched(0));
  // the next test of the valve clears it
  v[0].status = vdm::kStOpenCircuit;
  v[0].drive = 30;
  CHECK(next(s, v) == "Test 0");
  CHECK_FALSE(s.latched(0));
}

TEST_CASE("ValveScheduler: a full-open move ends at the end stop without blocking later moves") {
  Views v;
  idleAll(v);
  v[0].status = vdm::kStFullOpen;
  v[0].drive = 100;
  ValveScheduler s;
  CHECK(next(s, v) == "OpenEnd 0");
  v[0].status = vdm::kStIdle;
  v[0].actual = 100;
  s.moveEnded(0, StopReason::EndStop, false, vdm::kStIdle);
  v[0].drive = 30;
  CHECK(next(s, v) == "Close 0 70");
}
