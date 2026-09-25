// ValveModel / SensorModel behaviour of the 2.1 link: assembly hold, restored
// targets, failsafe emulation, protocol 3 fields, forced pushes after an STM
// reboot, sensor debounce and id-matched late replies.
#include <string.h>

#include <initializer_list>

#include "doctest.h"
#include "vdm/valve_model.h"

using namespace vdm;

namespace {

ValveData data(uint8_t valve, uint8_t status = 1) {
  ValveData d;
  d.valve = valve;
  d.position = 40;
  d.status = status;
  d.openCount = 3000;
  d.closeCount = 3100;
  return d;
}

ValveEx exV3(uint8_t valve, uint8_t target, uint16_t flags = 0) {
  ValveEx d;
  d.valve = valve;
  d.status = 1;
  d.target = target;
  d.openCount = 5000;
  d.closeCount = 5200;
  d.v3 = true;
  d.flags = flags;
  d.fsPct = 50;
  return d;
}

TargetReply target(uint8_t valve, uint8_t pos) {
  TargetReply t;
  t.valve = valve;
  t.target = pos;
  return t;
}

OneWireId idWithCrc(uint8_t seed) {
  OneWireId id;
  id.b[0] = 0x28;
  for (int i = 1; i < 7; ++i) id.b[i] = static_cast<uint8_t>(seed * 7 + i);
  uint8_t crc = 0;
  for (int i = 0; i < 7; ++i) {
    uint8_t in = id.b[i];
    for (int bit = 0; bit < 8; ++bit) {
      const uint8_t mix = (crc ^ in) & 1;
      crc >>= 1;
      if (mix) crc ^= 0x8C;
      in >>= 1;
    }
  }
  id.b[7] = crc;
  return id;
}

// Valves in `mask` active and known, desired `pos` delivered and verified.
ValveModel synced(uint16_t mask, uint8_t pos) {
  ValveModel m;
  m.setActiveMask(mask);
  for (uint8_t v = 0; v < kValveCount; ++v) {
    if (!((mask >> v) & 1u)) continue;
    m.applyValveData(data(v), 0);
    m.applyTarget(target(v, pos), 0);
    REQUIRE(m.valve(v).sync == TargetSync::Synced);
  }
  return m;
}

const uint8_t kPct50[kValveCount] = {50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50};

}  // namespace

// ================================================================ helpers

TEST_CASE("strokeNearMinimum: below 1.2 x minCounts with every count known") {
  CHECK(strokeNearMinimum(3599, 5000, 3000));
  CHECK_FALSE(strokeNearMinimum(3600, 5000, 3000));
  CHECK(strokeNearMinimum(5000, 3599, 3000));  // the smaller of the two counts
  CHECK_FALSE(strokeNearMinimum(5000, 3600, 3000));
  CHECK_FALSE(strokeNearMinimum(100, 100, 0));
  CHECK_FALSE(strokeNearMinimum(0, 100, 3000));
  CHECK_FALSE(strokeNearMinimum(100, 0, 3000));
  CHECK(strokeNearMinimum(1, 1, 1));
  CHECK(strokeNearMinimum(0xFFFFFFFFu, 78641, 65535));  // no overflow: 78641 * 5 < 65535 * 6
  CHECK_FALSE(strokeNearMinimum(78642, 0xFFFFFFFFu, 65535));
  CHECK_FALSE(strokeNearMinimum(0xFFFFFFFFu, 0xFFFFFFFFu, 65535));
}

TEST_CASE("expectSensor: goned/gowvd expect the id at their bus index, others untouched") {
  SensorModel s;
  OneWireList l;
  l.count = 2;
  l.hasList = true;
  l.ids[0] = idWithCrc(1);
  l.ids[1] = idWithCrc(2);
  s.applyTempList(l, 0);
  l.ids[1] = idWithCrc(5);
  s.applyVoltList(l, 0);
  RequestLine r;
  REQUIRE(buildTempData(1, r));
  expectSensor(r, s);
  CHECK(r.expect == idWithCrc(2));
  REQUIRE(buildTempData(5, r));
  expectSensor(r, s);
  CHECK(isZero(r.expect));  // unknown index: any id
  REQUIRE(buildVoltData(1, r));
  expectSensor(r, s);
  CHECK(r.expect == idWithCrc(5));
  REQUIRE(buildValveData(1, r));
  r.expect = idWithCrc(9);
  expectSensor(r, s);
  CHECK(r.expect == idWithCrc(9));
}

// ================================================================ protocol 3 fields

TEST_CASE("gvlvy fields enter the state; gvlvx and gvlvd clear them") {
  ValveModel m;
  m.setActiveMask(0x001);
  ValveEx d = exV3(0, 30, kStmFlagRetry | kStmFlagFsBlocked);
  d.fault = 4;
  d.fsPct = 60;
  d.drive = 60;
  d.retryS = 3540;
  d.retries = 2;
  m.applyValveEx(d, 0);
  ValveState v = m.valve(0);
  CHECK(v.hasV3);
  CHECK(v.stmFlags == (kStmFlagRetry | kStmFlagFsBlocked));
  CHECK(v.fault == 4);
  CHECK(v.fsPct == 60);
  CHECK(v.drive == 60);
  CHECK(v.retryS == 3540);
  CHECK(v.retries == 2);
  CHECK(v.autoRetry);
  const uint32_t rev = v.revision;
  d.retryS = 3530;  // counts down on every poll: not a change
  m.applyValveEx(d, 10);
  CHECK(m.valve(0).revision == rev);
  CHECK(m.valve(0).retryS == 3530);
  ValveEx x = d;
  x.v3 = false;
  m.applyValveEx(x, 20);
  v = m.valve(0);
  CHECK_FALSE(v.hasV3);
  CHECK(v.stmFlags == 0);
  CHECK(v.fault == 0);
  CHECK(v.drive == 0);
  CHECK(v.retryS == 0);
  CHECK(v.retries == 0);
  CHECK_FALSE(v.autoRetry);
  CHECK(v.fsPct == 60);  // fsPct stays (the ESP config applies on 1/2)
  m.applyValveEx(d, 30);
  REQUIRE(m.valve(0).hasV3);
  m.applyValveData(data(0), 40);
  CHECK_FALSE(m.valve(0).hasV3);
  CHECK(m.valve(0).stmFlags == 0);
  CHECK(m.valve(0).fault == 0);
}

TEST_CASE("autoRetry: set when retries rise, cleared at the calibration end or with retries 0") {
  ValveModel m;
  m.setActiveMask(0x001);
  ValveEx d = exV3(0, 30);
  m.applyValveEx(d, 0);
  CHECK_FALSE(m.valve(0).autoRetry);
  d.retries = 1;
  m.applyValveEx(d, 10);
  CHECK(m.valve(0).autoRetry);
  d.calibrating = true;
  m.applyValveEx(d, 20);
  CHECK(m.valve(0).autoRetry);  // the retry calibration runs
  m.applyValveEx(d, 25);        // equal retries keep it
  CHECK(m.valve(0).autoRetry);
  d.calibrating = false;
  m.applyValveEx(d, 30);
  CHECK_FALSE(m.valve(0).autoRetry);
  m.applyValveEx(d, 35);  // equal retries do not set it again
  CHECK_FALSE(m.valve(0).autoRetry);
  d.retries = 2;
  m.applyValveEx(d, 40);
  CHECK(m.valve(0).autoRetry);
  d.retries = 0;
  m.applyValveEx(d, 50);
  CHECK_FALSE(m.valve(0).autoRetry);
  d.retries = 3;
  d.calibrating = false;
  m.applyValveEx(d, 60);
  CHECK(m.valve(0).autoRetry);
}

TEST_CASE("health: the STM lease flag sets kHealthFailsafe and nothing else, active valves only") {
  ValveModel m;
  m.setActiveMask(0x001);
  m.applyValveEx(exV3(0, 30, kStmFlagFsLease), 0);
  CHECK(m.valve(0).health == kHealthFailsafe);
  m.applyValveEx(exV3(0, 30, kStmFlagFsBlocked), 0);
  CHECK(m.valve(0).health == 0);
  m.applyValveEx(exV3(1, 30, kStmFlagFsLease), 0);
  CHECK(m.valve(1).health == 0);  // inactive
}

TEST_CASE("health: kHealthStrokeShort from setMinCounts and the counts") {
  ValveModel m;
  m.setActiveMask(0x003);
  ValveData d = data(0);
  d.openCount = 3599;
  d.closeCount = 5000;
  m.applyValveData(d, 0);
  d.valve = 1;
  m.applyValveData(d, 0);
  m.setActiveMask(0x001);
  CHECK(m.valve(0).health == 0);  // minCounts unknown
  const uint32_t rev = m.valve(0).revision;
  m.setMinCounts(3000);
  CHECK(m.minCounts() == 3000);
  CHECK(m.valve(0).health == kHealthStrokeShort);
  CHECK(m.valve(0).revision == rev + 1);
  CHECK(m.valve(1).health == 0);  // inactive
  d.valve = 0;
  d.openCount = 3600;
  m.applyValveData(d, 0);
  CHECK(m.valve(0).health == 0);
  m.setMinCounts(3001);
  CHECK(m.valve(0).health == kHealthStrokeShort);
  m.setMinCounts(0);
  CHECK(m.valve(0).health == 0);
}

TEST_CASE("diffValve: the failsafe group covers every protocol 3 and emulation field") {
  const ValveState a;
  ValveState b;
  CHECK(diffValve(a, b) == 0);
  b.hasV3 = true;
  CHECK(diffValve(a, b) == kChangeFailsafe);
  b = a;
  b.stmFlags = 1;
  CHECK(diffValve(a, b) == kChangeFailsafe);
  b = a;
  b.fault = 1;
  CHECK(diffValve(a, b) == kChangeFailsafe);
  b = a;
  b.fsPct = 1;
  CHECK(diffValve(a, b) == kChangeFailsafe);
  b = a;
  b.drive = 1;
  CHECK(diffValve(a, b) == kChangeFailsafe);
  b = a;
  b.retries = 1;
  CHECK(diffValve(a, b) == kChangeFailsafe);
  b = a;
  b.autoRetry = true;
  CHECK(diffValve(a, b) == kChangeFailsafe);
  b = a;
  b.fsOverride = true;
  CHECK(diffValve(a, b) == kChangeFailsafe);
  b = a;
  b.fsTarget = 1;
  CHECK(diffValve(a, b) == kChangeFailsafe);
  b = a;
  b.retryS = 1;
  b.forcePush = true;
  CHECK(diffValve(a, b) == 0);
}

// ================================================================ assembly (W1)

TEST_CASE("assembly: desired 100 held without stgtp until the staop result, then verified") {
  ValveModel m = synced(0x008, 20);
  const uint32_t rev = m.desiredRevision();
  m.setAssembly(3, 100);
  const ValveState& v = m.valve(3);
  CHECK(v.desired == 100);
  CHECK(v.source == TargetSource::Assembly);
  CHECK(v.sync == TargetSync::AwaitAck);
  CHECK(m.desiredRevision() == rev + 1);
  uint8_t valve, pos;
  CHECK_FALSE(m.nextTargetPush(100000, valve, pos));
  m.applyTarget(target(3, 20), 200);  // read-back while waiting changes nothing
  CHECK(m.valve(3).sync == TargetSync::AwaitAck);
  m.onTargetAck(3, 300);  // a stgtp ack is not the staop's
  CHECK(m.valve(3).sync == TargetSync::AwaitAck);
  m.onTargetTimeout(3, 300);
  CHECK(m.valve(3).sync == TargetSync::AwaitAck);
  m.onAssemblyAck(3, 400);
  CHECK(m.valve(3).sync == TargetSync::AwaitVerify);
  m.onAssemblyAck(3, 450);  // only once
  CHECK(m.valve(3).sync == TargetSync::AwaitVerify);
  m.applyTarget(target(3, 100), 500);
  CHECK(m.valve(3).sync == TargetSync::Synced);
  CHECK(m.valve(3).desired == 100);
  CHECK(m.valve(3).source == TargetSource::Assembly);
}

TEST_CASE("assembly: a differing read-back after the ack pushes stgtp 100") {
  ValveModel m = synced(0x008, 20);
  m.setAssembly(3, 100);
  m.onAssemblyAck(3, 400);
  m.applyTarget(target(3, 20), 500);
  CHECK(m.valve(3).sync == TargetSync::Pending);
  uint8_t valve, pos;
  REQUIRE(m.nextTargetPush(600, valve, pos));
  CHECK(valve == 3);
  CHECK(pos == 100);
}

TEST_CASE("assembly: all valves addresses the active ones; a failure affects the named valve only") {
  ValveModel m = synced(0x005, 20);
  m.setAssembly(kAllValves, 100);
  CHECK(m.valve(0).source == TargetSource::Assembly);
  CHECK(m.valve(2).source == TargetSource::Assembly);
  CHECK(m.valve(1).source == TargetSource::None);
  CHECK_FALSE(m.valve(1).desiredValid);
  m.onAssemblyFailed(2, 200);
  CHECK(m.valve(2).sync == TargetSync::Pending);
  CHECK(m.valve(2).pushAttempts == 0);
  CHECK(m.valve(0).sync == TargetSync::AwaitAck);
  m.onAssemblyAck(kAllValves, 300);
  CHECK(m.valve(0).sync == TargetSync::AwaitVerify);
  CHECK(m.valve(2).sync == TargetSync::Pending);  // no longer waiting for the staop
  uint8_t valve, pos;
  REQUIRE(m.nextTargetPush(400, valve, pos));
  CHECK(valve == 2);
  CHECK(pos == 100);
}

TEST_CASE("assembly: failures after maxPushAttempts staop deliveries give Failed") {
  ValveModelParams p;
  p.maxPushAttempts = 2;
  ValveModel m(p);
  m.setActiveMask(0x001);
  m.applyValveData(data(0), 0);
  m.setAssemblyViaStaop(true);
  m.setAssembly(0, 0);
  m.onAssemblyFailed(0, 0);
  uint8_t valve = 99;
  REQUIRE(m.nextAssemblyPush(10000, valve));
  CHECK(valve == 0);
  m.onAssemblyFailed(0, 10000);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  REQUIRE(m.nextAssemblyPush(20000, valve));
  m.onAssemblyFailed(0, 20000);
  CHECK(m.valve(0).sync == TargetSync::Failed);
}

TEST_CASE("assembly: a web target ends it: source Web, Pending, push 40") {
  ValveModel m = synced(0x008, 20);
  m.setAssembly(3, 100);
  m.onAssemblyAck(3, 400);
  m.applyTarget(target(3, 100), 500);
  REQUIRE(m.setDesiredTarget(3, 40, TargetSource::Web, 600));
  CHECK(m.valve(3).source == TargetSource::Web);
  CHECK(m.valve(3).sync == TargetSync::Pending);
  uint8_t valve, pos;
  REQUIRE(m.nextTargetPush(700, valve, pos));
  CHECK(pos == 40);
}

TEST_CASE("assembly: a web target of 100 still ends it with a stgtp 100") {
  ValveModel m = synced(0x008, 20);
  m.setAssembly(3, 100);
  m.onAssemblyAck(3, 400);
  m.applyTarget(target(3, 100), 500);
  REQUIRE(m.valve(3).sync == TargetSync::Synced);
  REQUIRE(m.setDesiredTarget(3, 100, TargetSource::Mqtt, 600));
  CHECK(m.valve(3).source == TargetSource::Mqtt);
  CHECK(m.valve(3).sync == TargetSync::Pending);  // the STM holds 100, the stgtp ends its hold
  CHECK(m.valve(3).forcePush);
  uint8_t valve, pos;
  REQUIRE(m.nextTargetPush(650, valve, pos));
  CHECK(pos == 100);
  m.onTargetAck(3, 660);
  m.applyTarget(target(3, 100), 670);
  CHECK(m.valve(3).sync == TargetSync::Synced);
  REQUIRE(m.setDesiredTarget(3, 100, TargetSource::Web, 680));  // same value, not an assembly
  CHECK(m.valve(3).sync == TargetSync::Synced);
  // While the staop is outstanding a web target cancels the wait.
  m.setAssembly(3, 700);
  REQUIRE(m.setDesiredTarget(3, 100, TargetSource::Web, 800));
  CHECK(m.valve(3).sync == TargetSync::Pending);
  m.onAssemblyAck(3, 900);  // a late staop ack changes nothing
  CHECK(m.valve(3).sync == TargetSync::Pending);
}

TEST_CASE("assembly via staop: an Assembly valve is delivered by staop, never stgtp") {
  ValveModel m = synced(0x003, 20);
  m.setAssemblyViaStaop(true);
  m.setAssembly(1, 0);
  m.onAssemblyAck(1, 10);
  m.applyTarget(target(1, 100), 20);
  REQUIRE(m.valve(1).sync == TargetSync::Synced);
  m.onStmRebooted(1000);
  CHECK(m.valve(1).sync == TargetSync::Pending);
  uint8_t valve = 99, pos = 0;
  m.applyTarget(target(0, 20), 1100);
  m.applyTarget(target(1, 100), 1100);
  // valve 0: forced stgtp; valve 1: staop only.
  REQUIRE(m.nextTargetPush(1200, valve, pos));
  CHECK(valve == 0);
  CHECK_FALSE(m.nextTargetPush(1200, valve, pos));
  REQUIRE(m.nextAssemblyPush(1200, valve));
  CHECK(valve == 1);
  CHECK(m.valve(1).sync == TargetSync::AwaitAck);
  CHECK(m.valve(1).pushAttempts == 1);
  CHECK_FALSE(m.valve(1).forcePush);
  CHECK_FALSE(m.nextAssemblyPush(1200, valve));
  m.onTargetAck(1, 1300);  // not the staop result
  CHECK(m.valve(1).sync == TargetSync::AwaitAck);
  m.onAssemblyAck(1, 1300);
  CHECK(m.valve(1).sync == TargetSync::AwaitVerify);
  // protocol 3: 100 without the Assembly flag is not the hold.
  m.applyValveEx(exV3(1, 100), 1400);
  CHECK(m.valve(1).sync == TargetSync::Pending);
  CHECK_FALSE(m.nextAssemblyPush(1200 + 1999, valve));  // pushRetryMs since the staop
  REQUIRE(m.nextAssemblyPush(1200 + 2000, valve));
  m.onAssemblyAck(1, 3500);
  m.applyValveEx(exV3(1, 100, kStmFlagAssembly), 3600);
  CHECK(m.valve(1).sync == TargetSync::Synced);
  // Without staop delivery (protocol 1) the same valve gets stgtp 100.
  m.setAssemblyViaStaop(false);
  m.onStmRebooted(4000);
  m.applyTarget(target(1, 30), 4100);
  CHECK_FALSE(m.nextAssemblyPush(10000, valve));
  REQUIRE(m.nextTargetPush(10000, valve, pos));
  CHECK(valve == 0);
  REQUIRE(m.nextTargetPush(10000, valve, pos));
  CHECK(valve == 1);
  CHECK(pos == 100);
}

TEST_CASE("assembly via staop: a staop that could not be queued goes back to Pending uncounted") {
  ValveModel m = synced(0x001, 20);
  m.setAssemblyViaStaop(true);
  m.setAssembly(0, 0);
  m.onAssemblyFailed(0, 0);
  uint8_t valve;
  REQUIRE(m.nextAssemblyPush(5000, valve));
  REQUIRE(m.valve(0).pushAttempts == 1);
  m.onTargetPushDropped(0, 5000);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK(m.valve(0).pushAttempts == 0);
  m.onAssemblyAck(0, 5100);  // no staop outstanding any more
  CHECK(m.valve(0).sync == TargetSync::Pending);
}

// ================================================================ forced push after a reboot (L18)

TEST_CASE("reboot: one stgtp per valve even when the read-back equals, then none") {
  ValveModel m = synced(0x005, 30);
  m.onStmRebooted(1000);
  CHECK(m.valve(0).forcePush);
  CHECK(m.valve(2).forcePush);
  CHECK_FALSE(m.valve(1).forcePush);  // no desired target
  m.applyTarget(target(0, 30), 1100);
  m.applyTarget(target(2, 30), 1100);
  uint8_t valve, pos;
  REQUIRE(m.nextTargetPush(1200, valve, pos));
  CHECK(valve == 0);
  CHECK(pos == 30);
  CHECK_FALSE(m.valve(0).forcePush);
  REQUIRE(m.nextTargetPush(1200, valve, pos));
  CHECK(valve == 2);
  CHECK_FALSE(m.nextTargetPush(1200, valve, pos));
  m.onTargetAck(0, 1300);
  m.onTargetAck(2, 1300);
  m.applyTarget(target(0, 30), 1400);
  m.applyTarget(target(2, 30), 1400);
  CHECK(m.valve(0).sync == TargetSync::Synced);
  CHECK(m.valve(2).sync == TargetSync::Synced);
  CHECK_FALSE(m.nextTargetPush(100000, valve, pos));
}

// ================================================================ restored targets (W2)

TEST_CASE("restoreDesired: active valves only, Restored or Assembly, Pending") {
  ValveModel m;
  m.setActiveMask(0x00B);
  const uint32_t rev = m.desiredRevision();
  CHECK(m.restoreDesired(0, 42, TargetSource::Mqtt));
  CHECK(m.desiredRevision() == rev + 1);
  CHECK(m.valve(0).desired == 42);
  CHECK(m.valve(0).desiredValid);
  CHECK(m.valve(0).source == TargetSource::Restored);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK(m.restoreDesired(1, 100, TargetSource::Assembly));
  CHECK(m.valve(1).source == TargetSource::Assembly);
  CHECK(m.restoreDesired(3, 7, TargetSource::Web));
  CHECK(m.valve(3).source == TargetSource::Restored);
  CHECK(m.restoreDesired(3, 8, TargetSource::Stm));
  CHECK(m.valve(3).source == TargetSource::Restored);
  CHECK_FALSE(m.restoreDesired(2, 42, TargetSource::Mqtt));  // inactive
  CHECK_FALSE(m.valve(2).desiredValid);
  CHECK_FALSE(m.restoreDesired(0, 101, TargetSource::Mqtt));
  CHECK_FALSE(m.restoreDesired(12, 1, TargetSource::Mqtt));
  CHECK(m.valve(0).desired == 42);
  CHECK(m.restoreDesired(0, 100, TargetSource::Mqtt));
  CHECK(m.restoreDesired(0, 0, TargetSource::Mqtt));
  CHECK(m.valve(0).desired == 0);
}

TEST_CASE("restoreDesired: no push before the read-back; equal read-back is Synced without one") {
  ValveModel m;
  m.setActiveMask(0x003);
  REQUIRE(m.restoreDesired(0, 42, TargetSource::Mqtt));
  REQUIRE(m.restoreDesired(1, 42, TargetSource::Mqtt));
  m.applyValveData(data(0), 100);  // known, but the STM target is not read yet
  m.applyValveData(data(1), 100);
  uint8_t valve, pos;
  CHECK_FALSE(m.nextTargetPush(1000, valve, pos));
  const uint32_t rev = m.desiredRevision();
  m.applyTarget(target(0, 42), 1100);
  CHECK(m.valve(0).sync == TargetSync::Synced);
  CHECK(m.valve(0).pushAttempts == 0);
  CHECK(m.valve(0).source == TargetSource::Restored);
  m.applyTarget(target(1, 50), 1100);
  CHECK(m.valve(1).sync == TargetSync::Pending);
  CHECK(m.desiredRevision() == rev);  // read-backs of restored valves change no desired value
  REQUIRE(m.nextTargetPush(1200, valve, pos));
  CHECK(valve == 1);
  CHECK(pos == 42);
}

TEST_CASE("restoreDesired: a web target before the read-back wins") {
  ValveModel m;
  m.setActiveMask(0x001);
  REQUIRE(m.restoreDesired(0, 42, TargetSource::Mqtt));
  m.applyValveData(data(0), 100);
  REQUIRE(m.setDesiredTarget(0, 60, TargetSource::Web, 200));
  uint8_t valve, pos;
  REQUIRE(m.nextTargetPush(300, valve, pos));
  CHECK(pos == 60);
}

TEST_CASE("desiredRevision: web targets, assembly and adoption count; read-backs, pushes and overrides do not") {
  ValveModel m;
  m.setActiveMask(0x003);
  uint32_t rev = m.desiredRevision();
  m.applyTarget(target(1, 33), 0);  // adoption
  CHECK(m.desiredRevision() == ++rev);
  m.applyTarget(target(1, 34), 0);  // later read-back
  CHECK(m.desiredRevision() == rev);
  m.applyValveData(data(0), 0);
  REQUIRE(m.setDesiredTarget(0, 20, TargetSource::Web, 0));
  CHECK(m.desiredRevision() == ++rev);
  REQUIRE(m.setDesiredTarget(0, 20, TargetSource::Web, 0));  // same value
  CHECK(m.desiredRevision() == rev);
  uint8_t valve, pos;
  REQUIRE(m.nextTargetPush(0, valve, pos));
  m.onTargetAck(valve, 10);
  m.applyTarget(target(0, 20), 20);
  CHECK(m.desiredRevision() == rev);
  m.setFailsafeDrive(0x001, kPct50);
  CHECK(m.desiredRevision() == rev);
  m.setFailsafeDrive(0, kPct50);
  CHECK(m.desiredRevision() == rev);
  m.setAssembly(0, 30);
  CHECK(m.desiredRevision() == ++rev);
  REQUIRE(m.setDesiredTarget(0, 20, TargetSource::Mqtt, 40));
  CHECK(m.desiredRevision() == ++rev);
}

// ================================================================ failsafe emulation (K1)

TEST_CASE("failsafe drive: the override pushes pct, desired stays; its end pushes desired") {
  ValveModel m = synced(0x001, 30);
  const uint8_t pct[kValveCount] = {50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50};
  m.setFailsafeDrive(0x001, pct);
  const ValveState& v = m.valve(0);
  CHECK(v.fsOverride);
  CHECK(v.fsTarget == 50);
  CHECK(v.desired == 30);
  CHECK(v.sync == TargetSync::Pending);
  CHECK(m.pushTarget(0) == 50);
  CHECK((v.health & kHealthFailsafe) != 0);
  CHECK(valveAtFailsafe(v));
  uint8_t valve, pos;
  REQUIRE(m.nextTargetPush(1000, valve, pos));
  CHECK(pos == 50);
  m.onTargetAck(0, 1100);
  m.applyTarget(target(0, 50), 1200);
  CHECK(m.valve(0).sync == TargetSync::Synced);
  CHECK(m.valve(0).desired == 30);
  // A target set during the failsafe is stored and waits.
  REQUIRE(m.setDesiredTarget(0, 40, TargetSource::Web, 1300));
  CHECK(m.valve(0).desired == 40);
  CHECK(m.valve(0).sync == TargetSync::Synced);
  CHECK_FALSE(m.nextTargetPush(100000, valve, pos));
  // The same mask again changes nothing.
  m.setFailsafeDrive(0x001, pct);
  CHECK(m.valve(0).sync == TargetSync::Synced);
  m.setFailsafeDrive(0, pct);
  CHECK_FALSE(m.valve(0).fsOverride);
  CHECK(m.valve(0).fsTarget == 0);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK((m.valve(0).health & kHealthFailsafe) == 0);
  REQUIRE(m.nextTargetPush(200000, valve, pos));
  CHECK(pos == 40);
}

TEST_CASE("failsafe drive: a known STM target equal to pct is Synced at once") {
  ValveModel m = synced(0x001, 50);
  m.setFailsafeDrive(0x001, kPct50);
  CHECK(m.valve(0).fsOverride);
  CHECK(m.valve(0).sync == TargetSync::Synced);
  uint8_t pct[kValveCount] = {60, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50};
  m.setFailsafeDrive(0x001, pct);  // another failsafe position: Pending, attempts reset
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK(m.valve(0).pushAttempts == 0);
  CHECK(m.pushTarget(0) == 60);
}

TEST_CASE("failsafe drive: assembly, inactive, no desired target and hold are never overridden") {
  ValveModel m = synced(0x007, 30);
  m.setAssembly(1, 0);
  m.setActiveMask(0x00B);  // valve 2 inactive, valve 3 active without a desired target
  uint8_t pct[kValveCount] = {kFailsafeHold, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50};
  m.setFailsafeDrive(0x0FFF, pct);
  CHECK_FALSE(m.valve(0).fsOverride);  // hold
  CHECK_FALSE(m.valve(1).fsOverride);  // assembly
  CHECK_FALSE(m.valve(2).fsOverride);  // inactive
  CHECK_FALSE(m.valve(3).fsOverride);  // no desired target
  CHECK_FALSE(m.valve(4).fsOverride);
  CHECK(m.valve(0).fsPct == kFailsafeHold);  // fsPct follows the ESP config without v3 data
  CHECK(m.valve(1).fsPct == 50);
  CHECK(m.pushTarget(12) == 0);
}

TEST_CASE("failsafe drive: protocol 3 valves keep the STM's fsPct") {
  ValveModel m;
  m.setActiveMask(0x001);
  ValveEx d = exV3(0, 30);
  d.fsPct = 70;
  m.applyValveEx(d, 0);
  m.setFailsafeDrive(0, kPct50);
  CHECK(m.valve(0).fsPct == 70);
}

TEST_CASE("setAssembly ends a running failsafe override of that valve") {
  ValveModel m = synced(0x001, 30);
  m.setFailsafeDrive(0x001, kPct50);
  REQUIRE(m.valve(0).fsOverride);
  m.setAssembly(0, 0);
  CHECK_FALSE(m.valve(0).fsOverride);
  CHECK(m.pushTarget(0) == 100);
}

// ================================================================ unsupported STM (W13)

TEST_CASE("forgetStmData: STM data back to defaults, desired and the override kept") {
  ValveModel m = synced(0x003, 30);
  m.applyValveEx(exV3(1, 30, kStmFlagFsLease), 0);
  m.setFailsafeDrive(0x001, kPct50);
  const uint32_t rev = m.valve(0).revision;
  const uint32_t drev = m.desiredRevision();
  m.forgetStmData();
  const ValveState& v = m.valve(0);
  CHECK_FALSE(v.known);
  CHECK(v.status == 0);
  CHECK(v.position == 0);
  CHECK(v.openCount == 0);
  CHECK_FALSE(v.stmTargetKnown);
  CHECK(v.desiredValid);
  CHECK(v.desired == 30);
  CHECK(v.source == TargetSource::Stm);
  CHECK(v.sync == TargetSync::Pending);
  CHECK(v.fsOverride);
  CHECK(v.fsTarget == 50);
  CHECK(v.revision == rev + 1);
  CHECK(m.desiredRevision() == drev);
  CHECK_FALSE(m.valve(1).hasV3);
  CHECK(m.valve(1).stmFlags == 0);
  // No push before the STM target is read back again; a gtgtp enables it.
  uint8_t valve, pos;
  CHECK_FALSE(m.nextTargetPush(100000, valve, pos));
  m.applyTarget(target(0, 20), 100000);
  REQUIRE(m.nextTargetPush(100000, valve, pos));
  CHECK(valve == 0);
  CHECK(pos == 50);
}

TEST_CASE("forgetStmData: a valve without a desired target is Unknown") {
  ValveModel m;
  m.setActiveMask(0x001);
  m.applyValveData(data(0), 0);
  m.forgetStmData();
  CHECK(m.valve(0).sync == TargetSync::Unknown);
  CHECK_FALSE(m.valve(0).desiredValid);
}

// ================================================================ sensors (E9, S1)

TEST_CASE("SensorModel: one failed temperature is held, the second applies; good resets") {
  SensorModel s;
  TempData good;
  good.valid = true;
  good.id = idWithCrc(1);
  good.value = 215;
  s.applyTempData(0, good, 100);
  TempData bad = good;
  bad.value = kTempReadError;
  s.applyTempData(0, bad, 200);
  CHECK(s.temp(0).raw == 215);
  CHECK(s.temp(0).failStreak == 1);
  CHECK(s.temp(0).lastSeenMs == 100);
  CHECK(s.tempFresh(0, 200, 60000));
  s.applyTempData(0, bad, 300);
  CHECK(s.temp(0).raw == kTempReadError);
  CHECK(s.temp(0).failStreak == 2);
  CHECK(s.temp(0).lastSeenMs == 300);
  CHECK(s.temp(0).seen);
  s.applyTempData(0, good, 400);
  CHECK(s.temp(0).failStreak == 0);
  CHECK(s.temp(0).raw == 215);
  TempData none;  // "goned 0" counts like a failure
  s.applyTempData(0, none, 500);
  CHECK(s.temp(0).raw == 215);
  CHECK(s.temp(0).seen);
  s.applyTempData(0, none, 600);
  CHECK_FALSE(s.temp(0).seen);
  CHECK(s.temp(0).raw == kTempUnassigned);
  for (int i = 0; i < 300; ++i) s.applyTempData(0, bad, 700);
  CHECK(s.temp(0).failStreak == 255);
  for (int16_t raw : {kTempPowerOn, static_cast<int16_t>(1251), kTempUnassigned}) {
    s.applyTempData(0, good, 800);
    bad.value = raw;
    s.applyTempData(0, bad, 900);
    CHECK(s.temp(0).raw == 215);
  }
}

TEST_CASE("SensorModel: one failed volt reading is held, the second applies") {
  SensorModel s;
  VoltData good;
  good.valid = true;
  good.id = idWithCrc(1);
  good.vad = 330;
  s.applyVoltData(0, good, 100);
  VoltData bad = good;
  bad.vad = kVadFailed;
  s.applyVoltData(0, bad, 200);
  CHECK(s.volt(0).vad == 330);
  CHECK(s.volt(0).failStreak == 1);
  s.applyVoltData(0, bad, 300);
  CHECK(s.volt(0).vad == kVadFailed);
  CHECK(s.volt(0).seen);
  CHECK(s.volt(0).lastSeenMs == 300);
  s.applyVoltData(0, good, 400);
  CHECK(s.volt(0).failStreak == 0);
  for (int i = 0; i < 300; ++i) s.applyVoltData(0, bad, 500);
  CHECK(s.volt(0).failStreak == 255);
}

TEST_CASE("SensorModel: a stray reading lands on the bus index of its id") {
  SensorModel s;
  OneWireList l;
  l.count = 5;
  l.hasList = true;
  for (uint8_t i = 0; i < 5; ++i) l.ids[i] = idWithCrc(static_cast<uint8_t>(i + 1));
  s.applyTempList(l, 0);
  s.applyVoltList(l, 0);
  TempData d;
  d.valid = true;
  d.id = idWithCrc(5);
  d.value = 222;
  CHECK(s.applyStrayTempData(d, 100));
  CHECK(s.temp(4).raw == 222);
  CHECK(s.temp(4).lastSeenMs == 100);
  d.id = idWithCrc(9);
  CHECK_FALSE(s.applyStrayTempData(d, 200));
  for (uint8_t i = 0; i < 4; ++i) CHECK_FALSE(s.temp(i).seen);
  TempData none;
  CHECK_FALSE(s.applyStrayTempData(none, 300));
  VoltData v;
  v.valid = true;
  v.id = idWithCrc(2);
  v.vad = 55;
  CHECK(s.applyStrayVoltData(v, 400));
  CHECK(s.volt(1).vad == 55);
  v.id = idWithCrc(9);
  CHECK_FALSE(s.applyStrayVoltData(v, 500));
  VoltData vnone;
  CHECK_FALSE(s.applyStrayVoltData(vnone, 600));
  CHECK_FALSE(s.volt(0).seen);
}

TEST_CASE("SensorModel: the STM temperature age makes readings stale above 200 s") {
  SensorModel s;
  TempData d;
  d.valid = true;
  d.id = idWithCrc(1);
  d.value = 215;
  s.applyTempData(0, d, 100000);
  s.setStmTempAge(180, 100000);
  CHECK(s.tempFresh(0, 100000, 60000));
  CHECK(s.tempFresh(0, 120999, 60000));  // 180 + 20 = 200
  CHECK_FALSE(s.tempFresh(0, 121000, 60000));
  s.setStmTempAge(201, 121000);
  CHECK_FALSE(s.tempFresh(0, 121000, 60000));
  s.setStmTempAge(200, 121000);
  CHECK(s.tempFresh(0, 121000, 60000));
  s.setStmTempAge(0xFFFFFFFFu, 121000);  // no overflow
  CHECK_FALSE(s.tempFresh(0, 121000, 60000));
}
