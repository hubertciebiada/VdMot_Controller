// HealthMonitor events.
#include <initializer_list>
#include <string>

#include "doctest.h"
#include "vdm/health_monitor.h"

using namespace vdm;

namespace {

ValveState known(uint8_t status = 1) {
  ValveState v;
  v.known = true;
  v.status = status;
  return v;
}

struct Events {
  Event e[kMaxEventsPerUpdate];
  size_t n = 0;
};

Events onValve(HealthMonitor& hm, const ValveState& a, const ValveState& b, bool active = true,
               uint8_t valve = 3) {
  Events r;
  r.n = hm.onValve(valve, a, b, active, r.e, kMaxEventsPerUpdate);
  return r;
}

void checkEvent(const Event& e, EventCode code, uint8_t valve, int32_t a1 = 0, int32_t a2 = 0) {
  CHECK(e.code == code);
  CHECK(e.severity == eventDefaultSeverity(code));
  CHECK(e.valve == valve);
  CHECK(e.arg1 == a1);
  CHECK(e.arg2 == a2);
  CHECK(e.text[0] == '\0');
}

}  // namespace

TEST_CASE("onValve: argument guards") {
  HealthMonitor hm;
  const ValveState a = known(1), b = known(9);
  Event out[kMaxEventsPerUpdate];
  CHECK(hm.onValve(12, a, b, true, out, kMaxEventsPerUpdate) == 0);
  CHECK(hm.onValve(0, a, b, true, nullptr, kMaxEventsPerUpdate) == 0);
  CHECK(hm.onValve(0, a, b, true, out, 0) == 0);
  CHECK(hm.onValve(11, a, b, true, out, 1) == 1);
  CHECK(out[0].valve == 11);
}

TEST_CASE("onValve: first snapshot is silent except bad states") {
  HealthMonitor hm;
  const ValveState unknown;
  CHECK(onValve(hm, unknown, known(1)).n == 0);
  CHECK(onValve(hm, unknown, known(5)).n == 0);
  ValveState blocked = known(9);
  blocked.calibRetries = 2;
  Events r = onValve(hm, unknown, blocked);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveBlocked, 3, 2, -1);
  r = onValve(hm, unknown, known(4));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveFailed, 3, 0, -1);
  r = onValve(hm, unknown, known(6));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveNoValve, 3);
  CHECK(onValve(hm, unknown, known(6), false).n == 0);
  // Calibrating, counters, flags: all silent on the first snapshot.
  ValveState busy = known(2);
  busy.calibrating = true;
  busy.calibRetries = 1;
  busy.health = kHealthStale | kHealthTargetUnconfirmed;
  CHECK(onValve(hm, unknown, busy).n == 0);
  // A user target set before the first data is still reported.
  ValveState t = known(1);
  t.desiredValid = true;
  t.desired = 20;
  t.source = TargetSource::Web;
  r = onValve(hm, unknown, t);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::TargetSet, 3, 20, static_cast<int32_t>(TargetSource::Web));
  ValveState u;
  u.desiredValid = true;
  u.desired = 5;
  u.source = TargetSource::Mqtt;
  r = onValve(hm, ValveState(), u);  // valve still unknown
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::TargetSet, 3, 5, static_cast<int32_t>(TargetSource::Mqtt));
  // Nothing at all for an unknown valve without a target change.
  ValveState w;
  w.status = 9;
  CHECK(onValve(hm, ValveState(), w).n == 0);
}

TEST_CASE("onValve: bad status transitions and recovery") {
  HealthMonitor hm;
  ValveState b = known(9);
  b.calibRetries = 1;
  Events r = onValve(hm, known(1), b);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveBlocked, 3, 1, -1);
  CHECK(r.e[0].severity == Severity::Error);
  r = onValve(hm, known(9), known(1));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveRecovered, 3, 9, 0);
  r = onValve(hm, known(4), known(9));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveBlocked, 3, 0, -1);
  r = onValve(hm, known(1), known(4));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveFailed, 3, 0, -1);
  r = onValve(hm, known(6), known(8));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveRecovered, 3, 6);
  r = onValve(hm, known(1), known(6));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveNoValve, 3);
  // Inactive NoValve is just a state change (Debug).
  r = onValve(hm, known(1), known(6), false);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveStateChanged, 3, 1, 6);
  CHECK(r.e[0].severity == Severity::Debug);
  // Staying bad: nothing.
  CHECK(onValve(hm, known(9), known(9)).n == 0);
  // Plain status change.
  r = onValve(hm, known(1), known(2));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveStateChanged, 3, 1, 2);
  CHECK(onValve(hm, known(2), known(2)).n == 0);
}

TEST_CASE("onValve: calibration outcomes") {
  HealthMonitor hm;
  ValveState idle = known(1);
  ValveState cal = known(1);
  cal.calibrating = true;
  Events r = onValve(hm, idle, cal);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::CalibStarted, 3, 0);

  ValveState ok = known(1);
  ok.openCount = 3120;
  ok.closeCount = 3350;
  r = onValve(hm, cal, ok);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::CalibOk, 3, 3120, 3350);

  // Ending Blocked: CalibFailed replaces ValveBlocked.
  ValveState failed = known(9);
  failed.calibRetries = 2;
  ValveState calRetry = cal;
  calRetry.calibRetries = 1;
  r = onValve(hm, calRetry, failed);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::CalibFailed, 3, 2, -1);

  // Blocked valve recalibrated fine: CalibOk replaces ValveRecovered.
  ValveState calFromBlocked = known(9);
  calFromBlocked.calibrating = true;
  r = onValve(hm, calFromBlocked, ok);
  REQUIRE(r.n == 1);
  CHECK(r.e[0].code == EventCode::CalibOk);
  // Blocked valve failing again (status stays 9): CalibFailed only.
  r = onValve(hm, calFromBlocked, failed);
  REQUIRE(r.n == 1);
  CHECK(r.e[0].code == EventCode::CalibFailed);

  // Ending in another state: no outcome; the status event describes it.
  r = onValve(hm, cal, known(6));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveNoValve, 3);
  r = onValve(hm, cal, known(5));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveStateChanged, 3, 1, 5);
  // Starting calibration from a bad state recovers it (status changed).
  ValveState calIdle = known(8);
  calIdle.calibrating = true;
  r = onValve(hm, known(9), calIdle);
  REQUIRE(r.n == 2);
  CHECK(r.e[0].code == EventCode::CalibStarted);
  checkEvent(r.e[1], EventCode::ValveRecovered, 3, 9);

  // Retries during calibration.
  r = onValve(hm, cal, calRetry);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::CalibRetry, 3, 1);
  CHECK(r.e[0].severity == Severity::Warning);
  // ... also when the calibration ends ok in the same snapshot.
  ValveState okRetry = ok;
  okRetry.calibRetries = 2;
  r = onValve(hm, calRetry, okRetry);
  REQUIRE(r.n == 2);
  CHECK(r.e[0].code == EventCode::CalibOk);
  checkEvent(r.e[1], EventCode::CalibRetry, 3, 2);
  // ... and when it starts with a retry count already raised.
  ValveState startRetry = cal;
  startRetry.calibRetries = 1;
  r = onValve(hm, idle, startRetry);
  REQUIRE(r.n == 2);
  CHECK(r.e[0].code == EventCode::CalibStarted);
  CHECK(r.e[1].code == EventCode::CalibRetry);
  // Retry count changes outside calibration are not events.
  ValveState idleRetry = idle;
  idleRetry.calibRetries = 1;
  CHECK(onValve(hm, idle, idleRetry).n == 0);
  // A drop is not a retry.
  CHECK(onValve(hm, calRetry, cal).n == 0);
}

TEST_CASE("onValve: blocked and failed messages claim no failsafe position or fault") {
  HealthMonitor hm;
  auto message = [](const Event& e) {
    char buf[160];
    formatEventMessage(e, buf, sizeof buf);
    return std::string(buf);
  };
  ValveState blocked = known(9);
  blocked.calibRetries = 3;
  Events r = onValve(hm, known(1), blocked, true, 2);
  REQUIRE(r.n == 1);
  CHECK(message(r.e[0]) == "valve 3: blocked (calibration retries 3)");
  r = onValve(hm, known(1), known(4), true, 2);
  REQUIRE(r.n == 1);
  CHECK(message(r.e[0]) == "valve 3: failed");
  ValveState cal = known(1);
  cal.calibrating = true;
  r = onValve(hm, cal, blocked, true, 2);
  REQUIRE(r.n == 1);
  CHECK(message(r.e[0]) == "valve 3: calibration failed after 3 retries");
}

TEST_CASE("onValve: v2 reports a failed calibration with calState bit 3") {
  HealthMonitor hm;
  ValveState cal = known(1);
  cal.hasExtended = true;
  cal.calibrating = true;
  cal.calState = kCalStateRunning;
  ValveState done = known(1);
  done.hasExtended = true;
  done.calFlags = kCalFlagLastFailed;
  Events r = onValve(hm, cal, done);
  REQUIRE(r.n == 1);
  CHECK(r.e[0].code == EventCode::CalibFailed);
  // The early-stop flag alone is not a failure.
  done.calFlags = kCalFlagEarlyStop;
  r = onValve(hm, cal, done);
  REQUIRE(r.n == 1);
  CHECK(r.e[0].code == EventCode::CalibOk);
  // Without extended data the flag is not looked at (1.x).
  ValveState v1 = known(1);
  v1.calFlags = kCalFlagLastFailed;
  ValveState v1cal = cal;
  v1cal.hasExtended = false;
  r = onValve(hm, v1cal, v1);
  REQUIRE(r.n == 1);
  CHECK(r.e[0].code == EventCode::CalibOk);
}

TEST_CASE("onValve: v2 counters, flags and target events") {
  HealthMonitor hm;
  ValveState a = known(1);
  a.hasExtended = true;
  a.earlyStops = 1;
  a.cmdRejected = 1;
  ValveState b = a;
  b.earlyStops = 2;
  b.lastMove.stop = StopReason::EndStop;
  Events r = onValve(hm, a, b);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::EarlyStop, 3, 2, 2);
  b = a;
  b.cmdRejected = 4;
  r = onValve(hm, a, b);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::CmdRejected, 3, 4);
  // First gvlvx (before without extended data): baseline, silent.
  ValveState noEx = known(1);
  CHECK(onValve(hm, noEx, b).n == 0);
  // Decrease (STM reboot): silent.
  CHECK(onValve(hm, b, a).n == 0);

  ValveState f = known(1);
  f.desiredValid = true;
  f.desired = 40;
  f.source = TargetSource::Web;
  ValveState g = f;
  g.health = kHealthTargetUnconfirmed;
  g.pushAttempts = 5;
  r = onValve(hm, f, g);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::TargetNotConfirmed, 3, 40, 5);
  CHECK(onValve(hm, g, g).n == 0);  // stays set: nothing
  r = onValve(hm, g, f);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveRecovered, 3, 0, kHealthTargetUnconfirmed);

  ValveState s = known(1);
  s.health = kHealthStale;
  r = onValve(hm, known(1), s);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveStale, 3, 60);
  r = onValve(hm, s, known(1));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveRecovered, 3, 0, kHealthStale);
  ValveState both = known(1);
  both.health = kHealthStale | kHealthTargetUnconfirmed | kHealthTempFailed;
  r = onValve(hm, both, known(1));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveRecovered, 3, 0, kHealthStale | kHealthTargetUnconfirmed);
  // Other flags have no transition events of their own.
  ValveState other = known(1);
  other.health = kHealthTempFailed | kHealthCalibRetries | kHealthEarlyStop;
  CHECK(onValve(hm, known(1), other).n == 0);
  CHECK(onValve(hm, other, known(1)).n == 0);

  // TargetSet: web/mqtt changes only.
  ValveState t0 = known(1);
  ValveState t1 = t0;
  t1.desiredValid = true;
  t1.desired = 30;
  t1.source = TargetSource::Mqtt;
  r = onValve(hm, t0, t1);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::TargetSet, 3, 30, 3);
  ValveState t2 = t1;
  t2.desired = 31;
  t2.source = TargetSource::Web;
  r = onValve(hm, t1, t2);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::TargetSet, 3, 31, 2);
  CHECK(onValve(hm, t2, t2).n == 0);
  ValveState t3 = t2;
  t3.source = TargetSource::Mqtt;  // same value, other source
  CHECK(onValve(hm, t2, t3).n == 0);
  ValveState stm = t2;
  stm.desired = 50;
  stm.source = TargetSource::Stm;
  CHECK(onValve(hm, t2, stm).n == 0);
  ValveState none = t2;
  none.desired = 50;
  none.source = TargetSource::None;
  CHECK(onValve(hm, t2, none).n == 0);
  ValveState invalid = t2;
  invalid.desiredValid = false;
  invalid.desired = 60;
  CHECK(onValve(hm, t2, invalid).n == 0);

  // TargetSet comes before the Debug state change.
  ValveState moving = t2;
  moving.status = 2;
  moving.desired = 90;
  r = onValve(hm, t2, moving);
  REQUIRE(r.n == 2);
  CHECK(r.e[0].code == EventCode::TargetSet);
  CHECK(r.e[1].code == EventCode::ValveStateChanged);
}

TEST_CASE("onValve: many events are ordered and bounded") {
  HealthMonitor hm;
  ValveState a = known(1);
  a.hasExtended = true;
  a.health = kHealthStale;
  ValveState b = a;
  b.status = 9;
  b.calibrating = true;
  b.calibRetries = 1;
  b.earlyStops = 1;
  b.cmdRejected = 1;
  b.health = kHealthTargetUnconfirmed;
  b.desiredValid = true;
  b.desired = 10;
  b.source = TargetSource::Web;
  Event out[kMaxEventsPerUpdate + 4];
  const size_t n = hm.onValve(0, a, b, true, out, kMaxEventsPerUpdate + 4);
  REQUIRE(n == 8);
  CHECK(out[0].code == EventCode::CalibStarted);
  CHECK(out[1].code == EventCode::CalibRetry);
  CHECK(out[2].code == EventCode::ValveBlocked);
  CHECK(out[3].code == EventCode::EarlyStop);
  CHECK(out[4].code == EventCode::CmdRejected);
  CHECK(out[5].code == EventCode::TargetNotConfirmed);
  CHECK(out[6].code == EventCode::ValveRecovered);
  CHECK(out[7].code == EventCode::TargetSet);
  Event few[3];
  REQUIRE(hm.onValve(0, a, b, true, few, 3) == 3);
  CHECK(few[2].code == EventCode::ValveBlocked);
}

TEST_CASE("onLink") {
  HealthMonitor hm;
  Event out[2];
  CHECK(hm.onLink(LinkState::Up, LinkState::Up, 0, out, 2) == 0);
  REQUIRE(hm.onLink(LinkState::Unknown, LinkState::Up, 0, out, 2) == 1);
  checkEvent(out[0], EventCode::LinkUp, kNoValve);
  REQUIRE(hm.onLink(LinkState::Up, LinkState::Degraded, 1, out, 2) == 1);
  checkEvent(out[0], EventCode::LinkDegraded, kNoValve, 1);
  REQUIRE(hm.onLink(LinkState::Degraded, LinkState::Down, 5, out, 2) == 1);
  checkEvent(out[0], EventCode::LinkDown, kNoValve, 5);
  CHECK(out[0].severity == Severity::Error);
  REQUIRE(hm.onLink(LinkState::Booting, LinkState::Up, 0, out, 2) == 1);
  CHECK(hm.onLink(LinkState::Up, LinkState::Booting, 0, out, 2) == 0);
  CHECK(hm.onLink(LinkState::Up, LinkState::Suspended, 0, out, 2) == 0);
  CHECK(hm.onLink(LinkState::Up, LinkState::Unknown, 0, out, 2) == 0);
  CHECK(hm.onLink(LinkState::Up, LinkState::Down, 5, nullptr, 2) == 0);
  CHECK(hm.onLink(LinkState::Up, LinkState::Down, 5, out, 0) == 0);
}

TEST_CASE("onStmCounters: increases, rate limit, baselines") {
  HealthMonitor hm;
  Event out[2];
  CHECK(hm.onStmCounters(1, 1, 2, 0, out, 2) == 0);
  // ESP side counts from 0.
  REQUIRE(hm.onStmCounters(3, 0, 0, 1000, out, 2) == 1);
  checkEvent(out[0], EventCode::StmRxOverflow, kNoValve, 3, 0);
  CHECK(hm.onStmCounters(3, 0, 0, 2000, out, 2) == 0);
  CHECK(hm.onStmCounters(4, 0, 0, 1000 + 599999, out, 2) == 0);  // within 10 min
  REQUIRE(hm.onStmCounters(5, 0, 0, 1000 + 600000, out, 2) == 1);
  checkEvent(out[0], EventCode::StmRxOverflow, kNoValve, 5, 0);
  // Pending increase reported at the next allowed time with the latest total.
  CHECK(hm.onStmCounters(6, 0, 0, 700000, out, 2) == 0);
  REQUIRE(hm.onStmCounters(6, 0, 0, 1201000, out, 2) == 1);
  CHECK(out[0].arg1 == 6);
  // Parse errors are tracked separately.
  REQUIRE(hm.onStmCounters(6, 2, 0, 1201500, out, 2) == 1);
  checkEvent(out[0], EventCode::StmParseErrors, kNoValve, 2, 0);

  // STM side: the first observation is only the baseline.
  CHECK(hm.onStmCounters(10, 20, 1, 0, out, 2) == 0);
  CHECK(hm.onStmCounters(10, 20, 1, 1, out, 2) == 0);
  REQUIRE(hm.onStmCounters(11, 21, 1, 2, out, 2) == 2);
  checkEvent(out[0], EventCode::StmRxOverflow, kNoValve, 11, 1);
  checkEvent(out[1], EventCode::StmParseErrors, kNoValve, 21, 1);
  REQUIRE(hm.onStmCounters(12, 22, 1, 600002, out, 1) == 1);  // bounded by maxOut
  CHECK(out[0].code == EventCode::StmRxOverflow);
  // Counters restart (STM reboot): silent re-baseline.
  CHECK(hm.onStmCounters(0, 0, 1, 700000, out, 2) == 0);
  CHECK(hm.onStmCounters(1, 0, 1, 1300000, out, 2) == 1);
  // Wrap-safe timing.
  HealthMonitor w;
  CHECK(w.onStmCounters(1, 0, 0, 0xFFFFFF00u, out, 2) == 1);
  CHECK(w.onStmCounters(2, 0, 0, 0xFFFFFF00u + 599999u, out, 2) == 0);
  CHECK(w.onStmCounters(2, 0, 0, 0xFFFFFF00u + 600000u, out, 2) == 1);
}

TEST_CASE("onTempSensor") {
  HealthMonitor hm;
  Event out[1];
  CHECK(hm.onTempSensor(0, true, true, false, -1270, out, 1) == 0);
  CHECK(hm.onTempSensor(35, true, true, false, -1270, out, 1) == 0);
  CHECK(hm.onTempSensor(4, false, true, false, -1270, out, 1) == 0);
  CHECK(hm.onTempSensor(4, true, true, true, 200, out, 1) == 0);
  CHECK(hm.onTempSensor(4, true, false, false, 850, out, 1) == 0);
  REQUIRE(hm.onTempSensor(4, true, true, false, -1270, out, 1) == 1);
  checkEvent(out[0], EventCode::TempSensorFailed, kNoValve, 4, -1270);
  REQUIRE(hm.onTempSensor(34, true, false, true, 215, out, 1) == 1);
  checkEvent(out[0], EventCode::TempSensorRecovered, kNoValve, 34, 0);
  REQUIRE(hm.onTempSensor(1, true, false, true, 215, out, 1) == 1);
  CHECK(hm.onTempSensor(1, true, false, true, 215, nullptr, 1) == 0);
}
