// HealthMonitor events of the 2.1 link: protocol 3 arguments, stroke warning,
// assembly targets and the gstax system events.
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

Events onValve(HealthMonitor& hm, const ValveState& a, const ValveState& b, bool active = true) {
  Events r;
  r.n = hm.onValve(3, a, b, active, r.e, kMaxEventsPerUpdate);
  return r;
}

Events onStatus(HealthMonitor& hm, const StmStatus* before, const StmStatus& after,
                uint32_t now = 0) {
  Events r;
  r.n = hm.onStmStatus(before, after, now, r.e, kMaxEventsPerUpdate);
  return r;
}

void checkEvent(const Event& e, EventCode code, uint8_t valve, int32_t a1 = 0, int32_t a2 = 0) {
  CHECK(e.code == code);
  CHECK(e.severity == eventDefaultSeverity(code));
  CHECK(e.valve == valve);
  CHECK(e.arg1 == a1);
  CHECK(e.arg2 == a2);
}

ValveState v3(uint8_t status, uint16_t flags, uint8_t fsPct, uint8_t fault = 0) {
  ValveState v = known(status);
  v.hasV3 = true;
  v.stmFlags = flags;
  v.fsPct = fsPct;
  v.fault = fault;
  v.calibRetries = 2;
  return v;
}

StmStatus v3Status() {
  StmStatus s;
  s.v3 = true;
  s.uptimeS = 100;
  return s;
}

}  // namespace

TEST_CASE("onValve: ValveBlocked names the failsafe position of a blocked protocol 3 valve") {
  HealthMonitor hm;
  Events r = onValve(hm, known(1), v3(9, kStmFlagFsBlocked, 50));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveBlocked, 3, 2, 50);
  r = onValve(hm, known(1), v3(9, kStmFlagFsBlocked, 0));
  checkEvent(r.e[0], EventCode::ValveBlocked, 3, 2, 0);
  r = onValve(hm, known(1), v3(9, kStmFlagFsBlocked, kFailsafeHold));  // hold: stays at 0 %
  checkEvent(r.e[0], EventCode::ValveBlocked, 3, 2, -1);
  r = onValve(hm, known(1), v3(9, kStmFlagFsLease, 50));  // not the blocked flag
  checkEvent(r.e[0], EventCode::ValveBlocked, 3, 2, -1);
  ValveState noV3 = v3(9, kStmFlagFsBlocked, 50);
  noV3.hasV3 = false;
  r = onValve(hm, known(1), noV3);
  checkEvent(r.e[0], EventCode::ValveBlocked, 3, 2, -1);
  r = onValve(hm, ValveState{}, v3(9, kStmFlagFsBlocked, 40));  // first snapshot
  checkEvent(r.e[0], EventCode::ValveBlocked, 3, 2, 40);
}

TEST_CASE("onValve: ValveFailed carries the protocol 3 fault, -1 without") {
  HealthMonitor hm;
  Events r = onValve(hm, known(1), v3(4, 0, 50, 3));
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::ValveFailed, 3, 2, 3);
  r = onValve(hm, known(1), v3(4, 0, 50, 0));
  checkEvent(r.e[0], EventCode::ValveFailed, 3, 2, 0);
  ValveState noV3 = v3(4, 0, 50, 3);
  noV3.hasV3 = false;
  r = onValve(hm, known(1), noV3);
  checkEvent(r.e[0], EventCode::ValveFailed, 3, 2, -1);
}

TEST_CASE("onValve: CalibFailed carries the failsafe position like ValveBlocked") {
  HealthMonitor hm;
  ValveState a = v3(1, 0, 50);
  a.calibrating = true;
  const ValveState b = v3(9, kStmFlagFsBlocked, 60);
  Events r = onValve(hm, a, b);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::CalibFailed, 3, 2, 60);
}

TEST_CASE("onValve: CalibStarted arg1 2 for an automatic retry") {
  HealthMonitor hm;
  ValveState a = known(1);
  ValveState b = known(1);
  b.calibrating = true;
  b.autoRetry = true;
  Events r = onValve(hm, a, b);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::CalibStarted, 3, 2);
  b.autoRetry = false;
  r = onValve(hm, a, b);
  checkEvent(r.e[0], EventCode::CalibStarted, 3, 0);
}

TEST_CASE("onValve: CalibStrokeShort once when the flag rises") {
  HealthMonitor hm;
  hm.setMinCounts(3000);
  ValveState a = known(1);
  a.openCount = 3599;
  a.closeCount = 5000;
  ValveState b = a;
  b.health = kHealthStrokeShort;
  Events r = onValve(hm, a, b);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::CalibStrokeShort, 3, 3599, 3000);
  CHECK(onValve(hm, b, b).n == 0);
  b.openCount = 6000;
  b.closeCount = 3100;
  r = onValve(hm, a, b);
  checkEvent(r.e[0], EventCode::CalibStrokeShort, 3, 3100, 3000);
  CHECK(onValve(hm, b, a).n == 0);  // falling: silent
}

TEST_CASE("onValve: TargetSet for an assembly, none for restored or adopted targets") {
  HealthMonitor hm;
  ValveState a = known(1);
  a.desiredValid = true;
  a.desired = 30;
  a.source = TargetSource::Web;
  ValveState b = a;
  b.desired = 100;
  b.source = TargetSource::Assembly;
  Events r = onValve(hm, a, b);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::TargetSet, 3, 100, 5);
  b.source = TargetSource::Restored;
  CHECK(onValve(hm, a, b).n == 0);
  b.source = TargetSource::Stm;
  CHECK(onValve(hm, a, b).n == 0);
}

// ================================================================ gstax

TEST_CASE("onStmStatus: nothing without protocol 3 data") {
  HealthMonitor hm;
  StmStatus s = v3Status();
  s.v3 = false;
  s.safeMode = true;
  s.cfgEvents = 1;
  s.sysFlags = kStmSysProtectSuspended;
  CHECK(onStatus(hm, nullptr, s).n == 0);
  Event out[1];
  StmStatus t = v3Status();
  t.safeMode = true;
  CHECK(hm.onStmStatus(nullptr, t, 0, nullptr, 1) == 0);
  CHECK(hm.onStmStatus(nullptr, t, 0, out, 0) == 0);
}

TEST_CASE("onStmStatus: safe mode rise and fall; safe mode on the first status reports") {
  HealthMonitor hm;
  StmStatus a = v3Status();
  StmStatus b = a;
  b.safeMode = true;
  b.wdgResets = 3;
  Events r = onStatus(hm, &a, b);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::StmSafeMode, kNoValve, 3);
  CHECK(onStatus(hm, &b, b).n == 0);
  r = onStatus(hm, &b, a);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::StmSafeModeEnded, kNoValve);
  r = onStatus(hm, nullptr, b);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::StmSafeMode, kNoValve, 3);
  StmStatus old = a;
  old.v3 = false;
  old.safeMode = true;
  r = onStatus(hm, &old, b);  // a status without protocol 3 data counts as none
  REQUIRE(r.n == 1);
  CHECK(r.e[0].code == EventCode::StmSafeMode);
  CHECK(onStatus(hm, nullptr, a).n == 0);
}

TEST_CASE("onStmStatus: config repairs on the first status of a young STM and on increases") {
  HealthMonitor hm;
  StmStatus s = v3Status();
  s.cfgEvents = 1;
  s.cfgFlags = 0x24;
  s.uptimeS = 599;
  Events r = onStatus(hm, nullptr, s);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::StmConfigRepaired, kNoValve, 0x24, 1);
  s.uptimeS = 600;
  CHECK(onStatus(hm, nullptr, s).n == 0);  // an ESP restart does not report an old repair
  StmStatus zero = s;
  zero.cfgEvents = 0;
  zero.uptimeS = 1;
  CHECK(onStatus(hm, nullptr, zero).n == 0);
  CHECK(onStatus(hm, &s, s).n == 0);  // 1 -> 1
  StmStatus t = s;
  t.cfgEvents = 2;
  r = onStatus(hm, &s, t);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::StmConfigRepaired, kNoValve, 0x24, 2);
  CHECK(onStatus(hm, &t, s).n == 0);  // lower: nothing
}

TEST_CASE("onStmStatus: UART error totals, baseline first, one event per 10 min") {
  HealthMonitor hm;
  StmStatus a = v3Status();
  a.uartOre = 1;
  a.uartFe = 2;
  a.uartNe = 3;
  a.rxDropped = 4;
  CHECK(onStatus(hm, nullptr, a, 1000).n == 0);  // baseline
  StmStatus b = a;
  b.uartFe = 3;
  Events r = onStatus(hm, &a, b, 2000);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::StmUartErrors, kNoValve, 7, 4);
  StmStatus c = b;
  c.rxDropped = 10;
  CHECK(onStatus(hm, &b, c, 2000 + 599999).n == 0);
  c.uartOre = 5;
  r = onStatus(hm, &b, c, 2000 + 600000);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::StmUartErrors, kNoValve, 11, 10);
  // A lower total (the STM restarted) re-baselines silently.
  StmStatus d = a;
  d.uartOre = 0;
  CHECK(onStatus(hm, &c, d, 3000000).n == 0);
  StmStatus e = d;
  e.uartNe = 4;
  r = onStatus(hm, &d, e, 3300000);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::StmUartErrors, kNoValve, 6, 4);
  // The first status after a reboot is a new baseline.
  StmStatus f = e;
  f.uartOre = 50;
  CHECK(onStatus(hm, nullptr, f, 4000000).n == 0);
  f.rxDropped = 5;
  r = onStatus(hm, &f, f, 4700000);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::StmUartErrors, kNoValve, 56, 5);
}

TEST_CASE("onStmStatus: protection suspended once per STM boot") {
  HealthMonitor hm;
  StmStatus a = v3Status();
  StmStatus b = a;
  b.sysFlags = kStmSysProtectSuspended;
  Events r = onStatus(hm, &a, b);
  REQUIRE(r.n == 1);
  checkEvent(r.e[0], EventCode::StmProtectionSuspended, kNoValve);
  CHECK(onStatus(hm, &b, b).n == 0);
  r = onStatus(hm, nullptr, b);
  REQUIRE(r.n == 1);
  CHECK(r.e[0].code == EventCode::StmProtectionSuspended);
  StmStatus other = a;
  other.sysFlags = 0x02;  // reserved bit only
  CHECK(onStatus(hm, &a, other).n == 0);
}
