// HealthMonitor events, systemState, EventRateLimiter, OtaValidator,
// NetWatchdog.
#include <initializer_list>

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

Event event(EventCode c, uint8_t valve = 0, Severity sev = Severity::Warning) {
  return makeEvent(c, sev, valve, 0, 0, "");
}

}  // namespace

TEST_CASE("systemState") {
  CHECK(systemState(LinkState::Up, nullptr, 0, 0) == 0);
  CHECK(systemState(LinkState::Unknown, nullptr, 0, 0) == 1);
  CHECK(systemState(LinkState::Degraded, nullptr, 0, 0) == 1);
  CHECK(systemState(LinkState::Booting, nullptr, 0, 0) == 1);
  CHECK(systemState(LinkState::Suspended, nullptr, 0, 0) == 1);
  CHECK(systemState(LinkState::Down, nullptr, 0, 0) == 2);

  ValveState v[kValveCount];
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF) == 0);
  v[4].health = kHealthBlocked;
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF) == 2);
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FEF) == 0);  // inactive
  CHECK(systemState(LinkState::Up, v, 4, 0x0FFF) == 0);            // beyond count
  CHECK(systemState(LinkState::Up, v, 5, 0x0FFF) == 2);
  v[4].health = kHealthFailed;
  CHECK(systemState(LinkState::Degraded, v, kValveCount, 0x0010) == 2);
  const uint16_t others[] = {kHealthNoValve,    kHealthCalibRetries,      kHealthEarlyStop,
                             kHealthCmdRejected, kHealthStale,            kHealthTargetUnconfirmed,
                             kHealthTempFailed};
  for (uint16_t f : others) {
    v[4].health = f;
    CHECK(systemState(LinkState::Up, v, kValveCount, 0x0010) == 1);
    CHECK(systemState(LinkState::Up, v, kValveCount, 0x0000) == 0);
    CHECK(systemState(LinkState::Down, v, kValveCount, 0x0010) == 2);
  }
  // Blocked on a later valve wins over an info flag on an earlier one.
  v[0].health = kHealthStale;
  v[11].health = kHealthBlocked;
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF) == 2);
  // The first valve counts as well.
  v[11].health = 0;
  v[0].health = kHealthBlocked;
  CHECK(systemState(LinkState::Up, v, 1, 0x0001) == 2);
  // count above 12 is clamped (no read past the array).
  v[11].health = 0;
  v[0].health = 0;
  v[4].health = 0;
  CHECK(systemState(LinkState::Up, v, 255, 0xFFFF) == 0);
}

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
  checkEvent(r.e[0], EventCode::ValveBlocked, 3, 2);
  r = onValve(hm, unknown, known(4));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveFailed, 3);
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
  checkEvent(r.e[0], EventCode::ValveBlocked, 3, 1);
  CHECK(r.e[0].severity == Severity::Error);
  r = onValve(hm, known(9), known(1));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveRecovered, 3, 9, 0);
  r = onValve(hm, known(4), known(9));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveBlocked, 3, 0);
  r = onValve(hm, known(1), known(4));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveFailed, 3);
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
  checkEvent(r.e[0], EventCode::CalibFailed, 3, 2);

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

TEST_CASE("EventRateLimiter: severity gate and per-key limit") {
  EventRateLimiter rl;
  CHECK_FALSE(rl.allow(event(EventCode::TargetSet, 0, Severity::Info), 0));
  CHECK_FALSE(rl.allow(event(EventCode::ValveStateChanged, 0, Severity::Debug), 0));
  CHECK(rl.suppressed() == 0);
  CHECK(rl.allow(event(EventCode::CalibOk, 0, Severity::Info), 0));
  CHECK(rl.allow(event(EventCode::EarlyStop, 0), 0));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 0), 1));
  CHECK(rl.suppressed() == 1);
  CHECK(rl.allow(event(EventCode::EarlyStop, 1), 1));             // other valve
  CHECK(rl.allow(event(EventCode::CmdRejected, 0), 1));           // other code
  CHECK(rl.allow(event(EventCode::LinkDown, kNoValve, Severity::Error), 1));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 0), 599999));
  CHECK(rl.suppressed() == 2);
  CHECK(rl.allow(event(EventCode::EarlyStop, 0), 600000));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 0), 600001));
  CHECK(rl.allow(event(EventCode::StmFlashFailed, kNoValve, Severity::Critical), 600001));
}

TEST_CASE("EventRateLimiter: hourly token bucket") {
  EventRateLimiter rl(600000, 30);
  for (uint8_t i = 0; i < 30; ++i) {
    CHECK(rl.allow(event(EventCode::EarlyStop, i), 1000));
  }
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 100), 1000));
  CHECK(rl.suppressed() == 1);
  // One token per 120 s.
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 101), 1000 + 119999));
  CHECK(rl.allow(event(EventCode::EarlyStop, 102), 1000 + 120000));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 103), 1000 + 120000));
  // Frequent calls must not lose the fractional refill.
  EventRateLimiter f(600000, 30);
  for (uint8_t i = 0; i < 30; ++i) REQUIRE(f.allow(event(EventCode::EarlyStop, i), 0));
  uint32_t t = 0;
  while (t < 119980) {
    t += 20;
    CHECK_FALSE(f.allow(event(EventCode::TargetSet, 0, Severity::Info), t));
  }
  CHECK_FALSE(f.allow(event(EventCode::EarlyStop, 200), 119999));
  CHECK(f.allow(event(EventCode::EarlyStop, 201), 120000));
  // The bucket never exceeds its capacity.
  EventRateLimiter c(1, 2);
  CHECK(c.allow(event(EventCode::EarlyStop, 0), 0));
  CHECK(c.allow(event(EventCode::EarlyStop, 1), 0));
  CHECK_FALSE(c.allow(event(EventCode::EarlyStop, 2), 0));
  CHECK(c.allow(event(EventCode::EarlyStop, 3), 36000000));
  CHECK(c.allow(event(EventCode::EarlyStop, 4), 36000000));
  CHECK_FALSE(c.allow(event(EventCode::EarlyStop, 5), 36000000));
  // Refill precision at one token per hour, and the remainder is dropped
  // when the bucket is full.
  EventRateLimiter h(1, 1);
  CHECK(h.allow(event(EventCode::EarlyStop, 0), 0));
  CHECK_FALSE(h.allow(event(EventCode::EarlyStop, 1), 3599999));
  CHECK(h.allow(event(EventCode::EarlyStop, 2), 3600000));
  EventRateLimiter q(1, 1);
  CHECK(q.allow(event(EventCode::EarlyStop, 0), 0));
  CHECK(q.allow(event(EventCode::EarlyStop, 1), 3601800));
  CHECK_FALSE(q.allow(event(EventCode::EarlyStop, 2), 7200000));
  CHECK(q.allow(event(EventCode::EarlyStop, 3), 7201800));
  // No budget at all.
  EventRateLimiter z(600000, 0);
  CHECK_FALSE(z.allow(event(EventCode::EarlyStop, 0), 0));
  CHECK_FALSE(z.allow(event(EventCode::EarlyStop, 0), 3600000));
  CHECK(z.suppressed() == 2);
}

TEST_CASE("EventRateLimiter: key table recycles the least recently used entry") {
  EventRateLimiter rl(1000000000u, 1000);
  for (uint8_t i = 0; i < EventRateLimiter::kKeys; ++i) {
    REQUIRE(rl.allow(event(EventCode::EarlyStop, i), 10u + i));
  }
  CHECK(rl.allow(event(EventCode::EarlyStop, 200), 100));   // evicts valve 0
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 5), 101));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 200), 101));
  CHECK(rl.allow(event(EventCode::EarlyStop, 0), 102));     // evicts valve 1
  CHECK(rl.allow(event(EventCode::EarlyStop, 1), 103));     // evicts valve 2
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 3), 104));
  CHECK(rl.allow(event(EventCode::EarlyStop, 2), 105));
}

TEST_CASE("EventRateLimiter: entries expire across a millis() wrap") {
  EventRateLimiter rl(600000, 30);
  const uint32_t t0 = 0xFFFFFF00u;
  CHECK(rl.allow(event(EventCode::EarlyStop, 0), t0));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 0), t0 + 599999u));
  CHECK(rl.allow(event(EventCode::EarlyStop, 0), t0 + 600000u));
}

TEST_CASE("OtaValidator") {
  OtaValidator none;
  CHECK(none.update(true, true, 0) == OtaValidator::Decision::NotPending);
  none.begin(false, 0);
  CHECK(none.update(true, true, 999999) == OtaValidator::Decision::NotPending);

  OtaValidator v;
  v.begin(true, 5000);
  CHECK(v.update(true, true, 5000) == OtaValidator::Decision::Wait);
  CHECK(v.update(true, true, 5000 + 119999) == OtaValidator::Decision::Wait);
  CHECK(v.update(true, true, 5000 + 120000) == OtaValidator::Decision::MarkValid);
  CHECK(v.update(true, true, 5000 + 120001) == OtaValidator::Decision::NotPending);

  OtaValidator gap;
  gap.begin(true, 0);
  CHECK(gap.update(true, true, 0) == OtaValidator::Decision::Wait);
  CHECK(gap.update(true, false, 60000) == OtaValidator::Decision::Wait);
  CHECK(gap.update(true, true, 61000) == OtaValidator::Decision::Wait);
  CHECK(gap.update(true, true, 180999) == OtaValidator::Decision::Wait);
  CHECK(gap.update(true, true, 181000) == OtaValidator::Decision::MarkValid);

  OtaValidator netOnly;
  netOnly.begin(true, 0);
  CHECK(netOnly.update(false, true, 0) == OtaValidator::Decision::Wait);
  CHECK(netOnly.update(true, false, 599999) == OtaValidator::Decision::Wait);
  CHECK(netOnly.update(false, false, 600000) == OtaValidator::Decision::Wait);
  CHECK(netOnly.update(true, false, 600001) == OtaValidator::Decision::MarkValid);
  OtaValidator netExact;
  netExact.begin(true, 0);
  CHECK(netExact.update(true, false, 600000) == OtaValidator::Decision::MarkValid);

  // The healthy period starts with the first healthy observation.
  OtaValidator late;
  late.begin(true, 0);
  CHECK(late.update(true, true, 50) == OtaValidator::Decision::Wait);
  CHECK(late.update(true, true, 120000) == OtaValidator::Decision::Wait);
  CHECK(late.update(true, true, 120050) == OtaValidator::Decision::MarkValid);
  // begin() forgets an earlier healthy period.
  late.begin(true, 200000);
  CHECK(late.update(true, true, 200100) == OtaValidator::Decision::Wait);
  CHECK(late.update(true, true, 320000) == OtaValidator::Decision::Wait);
  CHECK(late.update(true, true, 320100) == OtaValidator::Decision::MarkValid);

  OtaValidator rb;
  rb.begin(true, 0xFFFFF000u);  // wrap-safe
  CHECK(rb.update(false, true, 0xFFFFF000u + 899999u) == OtaValidator::Decision::Wait);
  CHECK(rb.update(false, true, 0xFFFFF000u + 900000u) == OtaValidator::Decision::Rollback);
  CHECK(rb.update(true, true, 0xFFFFF000u + 900001u) == OtaValidator::Decision::NotPending);

  OtaValidator custom(10, 20, 30);
  custom.begin(true, 0);
  CHECK(custom.update(true, true, 0) == OtaValidator::Decision::Wait);
  CHECK(custom.update(true, true, 9) == OtaValidator::Decision::Wait);
  CHECK(custom.update(true, true, 10) == OtaValidator::Decision::MarkValid);
  custom.begin(true, 100);  // a second begin re-arms
  CHECK(custom.update(false, false, 129) == OtaValidator::Decision::Wait);
  CHECK(custom.update(false, false, 130) == OtaValidator::Decision::Rollback);
}

TEST_CASE("NetWatchdog") {
  NetWatchdog off;
  CHECK_FALSE(off.update(false, 0));
  CHECK_FALSE(off.update(false, 100000000));

  NetWatchdog w;
  w.configure(5);
  CHECK_FALSE(w.update(false, 1000));  // boot counts as the start of the outage
  CHECK_FALSE(w.update(false, 1000 + 299999));
  CHECK(w.update(false, 1000 + 300000));
  CHECK_FALSE(w.update(false, 1000 + 300001));  // once
  CHECK_FALSE(w.update(true, 400000));
  CHECK_FALSE(w.update(false, 500000));
  CHECK_FALSE(w.update(false, 799999));
  CHECK(w.update(false, 800000));

  // Up at boot, lost later; up again resets the timer.
  NetWatchdog u;
  u.configure(1);
  CHECK_FALSE(u.update(true, 0));
  CHECK_FALSE(u.update(false, 10000));
  CHECK_FALSE(u.update(true, 69000));
  CHECK_FALSE(u.update(false, 70000));
  CHECK_FALSE(u.update(false, 129999));
  CHECK(u.update(false, 130000));

  // Enabled later: the outage is measured from when it started.
  NetWatchdog late;
  CHECK_FALSE(late.update(false, 0));
  CHECK_FALSE(late.update(false, 200000));
  late.configure(2);
  CHECK(late.update(false, 200001));
  // configure() re-arms after firing.
  late.configure(2);
  CHECK(late.update(false, 200002));
  late.configure(0);
  CHECK_FALSE(late.update(false, 999999999));

  // Maximum setting.
  NetWatchdog max;
  max.configure(255);
  CHECK_FALSE(max.update(false, 0));
  CHECK_FALSE(max.update(false, 255u * 60000u - 1));
  CHECK(max.update(false, 255u * 60000u));
}
