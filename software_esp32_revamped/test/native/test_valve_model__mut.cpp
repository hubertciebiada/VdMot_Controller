// ValveModel / SensorModel: what the resets of assembly, restore, failsafe,
// reboot and forgetStmData leave behind; bus index 0; desired revisions.
#include "doctest.h"
#include "vdm/stm_types.h"
#include "vdm/valve_model.h"

using namespace vdm;

namespace {

ValveData data(uint8_t valve) {
  ValveData d;
  d.valve = valve;
  d.position = 40;
  d.meanCurrent = 12;
  d.status = 1;
  d.temp1 = kTempUnassigned;
  d.temp2 = kTempUnassigned;
  d.moves = 7;
  d.openCount = 3000;
  d.closeCount = 3100;
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

// Valve 0 active and known, desired 50 (web).
ValveModel webModel() {
  ValveModel m;
  m.setActiveMask(0x001);
  m.applyValveData(data(0), 0);
  CHECK(m.setDesiredTarget(0, 50, TargetSource::Web, 0));
  return m;
}

// webModel with a push in flight and the target marked unconfirmed (a retried Failed delivery).
ValveModel unconfirmedModel() {
  ValveModel m = webModel();
  uint8_t v = 0, pos = 0;
  uint32_t t = 0;
  for (int i = 0; i < 5; ++i, t += 3000) {
    REQUIRE(m.nextTargetPush(t, v, pos));
    m.onTargetTimeout(0, t);
  }
  REQUIRE(m.valve(0).sync == TargetSync::Failed);
  REQUIRE(m.nextTargetPush(t + 400000, v, pos));  // re-armed after failedRetryMs
  REQUIRE((m.valve(0).health & kHealthTargetUnconfirmed) != 0);
  return m;
}

const uint8_t kNoPct[kValveCount] = {};

}  // namespace

TEST_CASE("desiredRevision: a new desired value with the same source counts") {
  ValveModel m = webModel();
  const uint32_t rev = m.desiredRevision();
  CHECK(m.setDesiredTarget(0, 60, TargetSource::Web, 0));
  CHECK(m.desiredRevision() == rev + 1);
}

TEST_CASE("setAssembly: forgets the STM target, the failsafe target and the unconfirmed flag") {
  ValveModel m = unconfirmedModel();
  m.applyTarget(target(0, 50), 0);
  uint8_t pct[kValveCount] = {};
  pct[0] = 30;
  m.setFailsafeDrive(0x001, pct);
  REQUIRE(m.valve(0).fsOverride);
  REQUIRE(m.valve(0).fsTarget == 30);
  REQUIRE(m.valve(0).stmTargetKnown);
  m.setAssembly(0, 0);
  CHECK_FALSE(m.valve(0).stmTargetKnown);
  CHECK_FALSE(m.valve(0).fsOverride);
  CHECK(m.valve(0).fsTarget == 0);
  CHECK(m.valve(0).sync == TargetSync::AwaitAck);
  CHECK((m.valve(0).health & kHealthTargetUnconfirmed) == 0);
}

TEST_CASE("restoreDesired: attempts and the unconfirmed flag start over") {
  ValveModel m = unconfirmedModel();
  REQUIRE(m.valve(0).pushAttempts > 0);
  CHECK(m.restoreDesired(0, 60, TargetSource::Web));
  CHECK(m.valve(0).pushAttempts == 0);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK((m.valve(0).health & kHealthTargetUnconfirmed) == 0);
}

TEST_CASE("setFailsafeDrive: pct 100 is a failsafe position, 101 is not") {
  ValveModel m = unconfirmedModel();
  uint8_t pct[kValveCount] = {};
  pct[0] = 101;
  m.setFailsafeDrive(0x001, pct);
  CHECK_FALSE(m.valve(0).fsOverride);
  CHECK(m.pushTarget(0) == 50);
  pct[0] = 100;
  m.setFailsafeDrive(0x001, pct);
  CHECK(m.valve(0).fsOverride);
  CHECK(m.pushTarget(0) == 100);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK(m.valve(0).pushAttempts == 0);
  CHECK((m.valve(0).health & kHealthTargetUnconfirmed) == 0);
}

TEST_CASE("setFailsafeDrive: an unchanged pushed value leaves a delivery in flight alone") {
  ValveModel m = webModel();
  uint8_t v = 0, pos = 0;
  REQUIRE(m.nextTargetPush(0, v, pos));
  REQUIRE(m.valve(0).sync == TargetSync::AwaitAck);
  m.setFailsafeDrive(0, kNoPct);
  CHECK(m.valve(0).sync == TargetSync::AwaitAck);
  CHECK(m.valve(0).pushAttempts == 1);
}

TEST_CASE("setMinCounts: every active valve gets the stroke check") {
  ValveModel m;
  m.setActiveMask(0x002);
  m.applyValveData(data(1), 0);
  m.setMinCounts(3000);
  CHECK(m.valve(1).health == kHealthStrokeShort);
}

TEST_CASE("forgetStmData: staleness starts over, no flag and no assembly result left") {
  ValveModel m = unconfirmedModel();
  m.tick(0);
  m.forgetStmData();
  CHECK((m.valve(0).health & kHealthStale) == 0);
  CHECK((m.valve(0).health & kHealthTargetUnconfirmed) == 0);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  m.onAssemblyAck(kAllValves, 0);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  m.tick(60000);  // measured from this tick, not from the tick before forgetStmData
  CHECK((m.valve(0).health & kHealthStale) == 0);
  m.tick(120000);
  CHECK((m.valve(0).health & kHealthStale) != 0);
}

TEST_CASE("onStmRebooted: no assembly result is pending afterwards") {
  ValveModel m = webModel();
  m.onStmRebooted(0);
  REQUIRE(m.valve(0).sync == TargetSync::Pending);
  m.onAssemblyAck(kAllValves, 0);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  m.onAssemblyFailed(kAllValves, 0);
  CHECK(m.valve(0).sync == TargetSync::Pending);
}

TEST_CASE("SensorModel: bus index 0 for stray data and stale readings") {
  SensorModel s;
  OneWireList l;
  l.count = 1;
  l.hasList = true;
  l.ids[0] = idWithCrc(1);
  s.applyTempList(l, 0);
  s.applyVoltList(l, 0);
  TempData td;
  td.valid = true;
  td.id = l.ids[0];
  td.value = 215;
  CHECK(s.applyStrayTempData(td, 1000));
  CHECK(s.temp(0).raw == 215);
  VoltData vd;
  vd.valid = true;
  vd.id = l.ids[0];
  vd.vad = 500;
  CHECK(s.applyStrayVoltData(vd, 1000));
  CHECK(s.volt(0).vad == 500);

  ValveModel m;
  m.setActiveMask(0x001);
  ValveSensors vs;
  vs.isList = true;
  vs.ids[0][0] = l.ids[0];
  m.applyValveSensors(vs, nullptr, 0);
  m.applySensorTemps(s, 2000, 60000);
  CHECK(m.valve(0).temp1 == 215);
  m.applySensorTemps(s, 100000, 60000);  // seen before, too old now
  CHECK(m.valve(0).temp1 == kTempReadError);
}
