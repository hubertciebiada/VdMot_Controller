// ValveModel / SensorModel: target delivery state machine, health flags,
// change detection, sensor bookkeeping.
#include <string.h>

#include <initializer_list>

#include "doctest.h"
#include "vdm/stm_types.h"
#include "vdm/valve_model.h"

using namespace vdm;

namespace {

ValveData data(uint8_t valve, uint8_t status = 1) {
  ValveData d;
  d.valve = valve;
  d.position = 40;
  d.meanCurrent = 12;
  d.status = status;
  d.temp1 = 215;
  d.temp2 = kTempUnassigned;
  d.moves = 7;
  d.openCount = 3000;
  d.closeCount = 3100;
  d.deadZone = -4;
  d.calibRetries = 0;
  return d;
}

ValveEx ex(uint8_t valve, uint8_t target, uint32_t early = 0, uint32_t rejected = 0) {
  ValveEx d;
  d.valve = valve;
  d.status = 1;
  d.position = 33;
  d.target = target;
  d.meanCurrent = 14;
  d.openCount = 11;
  d.closeCount = 12;
  d.deadZone = -3;
  d.calibRetries = 1;
  d.moves = 99;
  d.calState = 2;
  d.calFlags = kCalFlagEarlyStop;
  d.earlyStops = early;
  d.cmdRejected = rejected;
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
  // Compute the Dallas CRC so the id is valid.
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

// Model with valve 0 active and known, desired 50 delivered and verified.
ValveModel syncedModel(uint32_t now = 1000) {
  ValveModel m;
  m.setActiveMask(0x001);
  m.applyValveData(data(0), now);
  m.applyTarget(target(0, 50), now);
  return m;
}

}  // namespace

TEST_CASE("valve status texts and keys cover every status") {
  const char* texts[] = {"",        "idle",     "opens",     "closes",    "failed",
                         "unknown", "no valve", "full open", "connected", "blocked"};
  const char* keys[] = {"nodata",  "idle",    "opening",  "closing",   "failed",
                        "unknown", "novalve", "fullopen", "connected", "blocked"};
  for (uint8_t s = 0; s < 10; ++s) {
    CHECK(strcmp(valveStatusText(s), texts[s]) == 0);
    CHECK(strcmp(valveStatusKey(s), keys[s]) == 0);
  }
  CHECK(strcmp(valveStatusText(10), "") == 0);
  CHECK(strcmp(valveStatusText(255), "") == 0);
  CHECK(strcmp(valveStatusKey(10), "invalid") == 0);
  CHECK(strcmp(valveStatusKey(255), "invalid") == 0);
}

TEST_CASE("target source and sync names") {
  CHECK(strcmp(targetSourceName(TargetSource::None), "none") == 0);
  CHECK(strcmp(targetSourceName(TargetSource::Stm), "stm") == 0);
  CHECK(strcmp(targetSourceName(TargetSource::Web), "web") == 0);
  CHECK(strcmp(targetSourceName(TargetSource::Mqtt), "mqtt") == 0);
  CHECK(strcmp(targetSourceName(TargetSource::Restored), "restored") == 0);
  CHECK(strcmp(targetSourceName(TargetSource::Assembly), "assembly") == 0);
  CHECK(static_cast<uint8_t>(TargetSource::Restored) == 4);
  CHECK(static_cast<uint8_t>(TargetSource::Assembly) == 5);
  CHECK(strcmp(targetSourceName(static_cast<TargetSource>(6)), "unknown") == 0);
  CHECK(strcmp(targetSyncName(TargetSync::Unknown), "unknown") == 0);
  CHECK(strcmp(targetSyncName(TargetSync::Synced), "synced") == 0);
  CHECK(strcmp(targetSyncName(TargetSync::Pending), "pending") == 0);
  CHECK(strcmp(targetSyncName(TargetSync::AwaitAck), "await_ack") == 0);
  CHECK(strcmp(targetSyncName(TargetSync::AwaitVerify), "await_verify") == 0);
  CHECK(strcmp(targetSyncName(TargetSync::Failed), "failed") == 0);
  CHECK(strcmp(targetSyncName(static_cast<TargetSync>(6)), "invalid") == 0);
}

TEST_CASE("failsafeKind and valveAtFailsafe") {
  struct Row {
    bool hasV3;
    uint16_t flags;
    bool fsOverride;
    FailsafeKind kind;
  };
  const uint16_t both = kStmFlagFsLease | kStmFlagFsBlocked;
  const uint16_t others = static_cast<uint16_t>(0xFFFF & ~both);
  const Row rows[] = {
      {false, 0, false, FailsafeKind::None},
      {false, kStmFlagFsLease, false, FailsafeKind::None},    // gvlvx: no v3 flags
      {false, kStmFlagFsBlocked, false, FailsafeKind::None},
      {false, both, false, FailsafeKind::None},
      {false, 0, true, FailsafeKind::Lease},                  // ESP emulation
      {false, kStmFlagFsBlocked, true, FailsafeKind::Lease},
      {true, 0, false, FailsafeKind::None},
      {true, others, false, FailsafeKind::None},
      {true, kStmFlagFsLease, false, FailsafeKind::Lease},
      {true, kStmFlagFsBlocked, false, FailsafeKind::Blocked},
      {true, both, false, FailsafeKind::Blocked},             // blocked wins
      {true, 0, true, FailsafeKind::Lease},
      {true, kStmFlagFsBlocked, true, FailsafeKind::Blocked},
  };
  for (const Row& r : rows) {
    CAPTURE(r.hasV3);
    CAPTURE(r.flags);
    CAPTURE(r.fsOverride);
    ValveState v;
    v.hasV3 = r.hasV3;
    v.stmFlags = r.flags;
    v.fsOverride = r.fsOverride;
    CHECK(failsafeKind(v) == r.kind);
    CHECK(valveAtFailsafe(v) == (r.kind == FailsafeKind::Lease));
  }
}

TEST_CASE("defaults of the protocol 3 and failsafe fields") {
  const ValveState v;
  CHECK_FALSE(v.hasV3);
  CHECK(v.stmFlags == 0);
  CHECK(v.fault == 0);
  CHECK(v.fsPct == kFailsafeHold);
  CHECK(v.drive == 0);
  CHECK(v.retryS == 0);
  CHECK(v.retries == 0);
  CHECK_FALSE(v.autoRetry);
  CHECK_FALSE(v.fsOverride);
  CHECK(v.fsTarget == 0);
  CHECK_FALSE(v.forcePush);
  CHECK(failsafeKind(v) == FailsafeKind::None);
  CHECK(kHealthFailsafe == (1u << 9));
  CHECK(kHealthStrokeShort == (1u << 10));
  CHECK(kChangeFailsafe == (1u << 14));
  CHECK(TempReading{}.failStreak == 0);
  CHECK(VoltReading{}.failStreak == 0);
}

TEST_CASE("stm types: command numbers and defaults") {
  // Command type numbers are external (StmQueueFull arg1): appended only.
  CHECK(static_cast<uint8_t>(StmCommandType::SetTarget) == 0);
  CHECK(static_cast<uint8_t>(StmCommandType::ConfigChanged) == 12);
  CHECK(static_cast<uint8_t>(StmCommandType::StopValve) == 13);
  CHECK(static_cast<uint8_t>(StmCommandType::LeaveSafeMode) == 14);
  const StmCommand c;
  CHECK(c.type == StmCommandType::ConfigChanged);
  CHECK(c.board[0] == '\0');
  CHECK(c.attempt == 0);
  CHECK(static_cast<uint8_t>(StmSaveState::Idle) == 0);
  CHECK(static_cast<uint8_t>(StmSaveState::Waiting) == 1);
  CHECK(static_cast<uint8_t>(StmSaveState::Saved) == 2);
  CHECK(static_cast<uint8_t>(StmSaveState::Unavailable) == 3);
  CHECK(static_cast<uint8_t>(StmSaveState::TimedOut) == 4);
  static StmSnapshot s;  // large: not on the stack
  CHECK(s.support == StmSupport::Unknown);
  CHECK(s.lease.mode == LeaseMode::None);
  CHECK_FALSE(s.haveLearnTime);
  CHECK(s.learnTimeS == 0);
  CHECK_FALSE(s.flashPending);
}

TEST_CASE("diffValve reports exactly the changed groups") {
  const ValveState a;
  CHECK(diffValve(a, a) == 0);
  struct Case {
    void (*mutate)(ValveState&);
    uint32_t bit;
  };
  const Case cases[] = {
      {[](ValveState& s) { s.status = 3; }, kChangeStatus},
      {[](ValveState& s) { s.calibrating = true; }, kChangeStatus},
      {[](ValveState& s) { s.position = 1; }, kChangePosition},
      {[](ValveState& s) { s.desired = 1; }, kChangeTarget},
      {[](ValveState& s) { s.desiredValid = true; }, kChangeTarget},
      {[](ValveState& s) { s.meanCurrent = 1; }, kChangeMeanCurrent},
      {[](ValveState& s) { s.temp1 = 1; }, kChangeTemp1},
      {[](ValveState& s) { s.temp2 = 1; }, kChangeTemp2},
      {[](ValveState& s) { s.moves = 1; }, kChangeCounters},
      {[](ValveState& s) { s.openCount = 1; }, kChangeCounters},
      {[](ValveState& s) { s.closeCount = 1; }, kChangeCounters},
      {[](ValveState& s) { s.deadZone = -1; }, kChangeCounters},
      {[](ValveState& s) { s.calibRetries = 1; }, kChangeCalibRetries},
      {[](ValveState& s) { s.calState = 1; }, kChangeExtended},
      {[](ValveState& s) { s.calFlags = kCalFlagLastFailed; }, kChangeExtended},
      {[](ValveState& s) { s.earlyStops = 1; }, kChangeExtended},
      {[](ValveState& s) { s.cmdRejected = 1; }, kChangeExtended},
      {[](ValveState& s) { s.moveSeq = 1; }, kChangeLastMove},
      {[](ValveState& s) { s.sync = TargetSync::Pending; }, kChangeSync},
      {[](ValveState& s) { s.stmTargetKnown = true; }, kChangeSync},
      {[](ValveState& s) { s.stmTarget = 1; }, kChangeSync},
      {[](ValveState& s) { s.sensorId[0].b[3] = 1; }, kChangeSensors},
      {[](ValveState& s) { s.sensorId[1].b[0] = 1; }, kChangeSensors},
      {[](ValveState& s) { s.sensorSlot[0] = 1; }, kChangeSensors},
      {[](ValveState& s) { s.sensorSlot[1] = 1; }, kChangeSensors},
      {[](ValveState& s) { s.health = kHealthStale; }, kChangeHealth},
      {[](ValveState& s) { s.known = true; }, kChangeKnown},
  };
  for (const Case& c : cases) {
    ValveState b;
    c.mutate(b);
    CHECK(diffValve(a, b) == c.bit);
    CHECK(diffValve(b, a) == c.bit);
  }
  // Bookkeeping fields are not change groups.
  ValveState b;
  b.lastSeenMs = 5;
  b.lastPushMs = 5;
  b.revision = 9;
  b.pushAttempts = 3;
  b.source = TargetSource::Web;
  CHECK(diffValve(a, b) == 0);
  ValveState c;
  c.status = 2;
  c.position = 9;
  CHECK(diffValve(a, c) == (kChangeStatus | kChangePosition));
}

TEST_CASE("valve() bounds and defaults") {
  ValveModel m;
  CHECK(m.activeMask() == 0);
  CHECK_FALSE(m.valve(0).known);
  CHECK(m.valve(11).sync == TargetSync::Unknown);
  CHECK(&m.valve(12) == &m.valve(255));
  CHECK_FALSE(m.valve(12).known);
  CHECK_FALSE(m.anyCalibrating());
  CHECK_FALSE(m.isBusy(0));
  CHECK_FALSE(m.isBusy(12));
  uint8_t v = 99;
  CHECK_FALSE(m.nextVerify(v));
  CHECK(v == 99);
  uint8_t pos = 77;
  CHECK_FALSE(m.nextTargetPush(0, v, pos));
  CHECK(v == 99);
  CHECK(pos == 77);
}

TEST_CASE("setActiveMask keeps only 12 bits") {
  ValveModel m;
  m.setActiveMask(0xFFFF);
  CHECK(m.activeMask() == 0x0FFF);
  m.setActiveMask(0x8001);
  CHECK(m.activeMask() == 0x0001);
  m.setActiveMask(0);
  CHECK(m.activeMask() == 0);
}

TEST_CASE("setDesiredTarget validates its input") {
  ValveModel m;
  m.setActiveMask(0x0FFF);
  CHECK_FALSE(m.setDesiredTarget(12, 10, TargetSource::Web, 0));
  CHECK_FALSE(m.setDesiredTarget(255, 10, TargetSource::Web, 0));
  CHECK_FALSE(m.setDesiredTarget(0, 101, TargetSource::Web, 0));
  CHECK_FALSE(m.setDesiredTarget(0, 255, TargetSource::Web, 0));
  CHECK(m.valve(0).revision == 0);
  CHECK_FALSE(m.valve(0).desiredValid);
  m.setActiveMask(0x0FFE);
  CHECK_FALSE(m.setDesiredTarget(0, 10, TargetSource::Web, 0));
  CHECK_FALSE(m.valve(0).desiredValid);

  CHECK(m.setDesiredTarget(11, 100, TargetSource::Mqtt, 0));
  CHECK(m.valve(11).desiredValid);
  CHECK(m.valve(11).desired == 100);
  CHECK(m.valve(11).source == TargetSource::Mqtt);
  CHECK(m.valve(11).sync == TargetSync::Pending);
  CHECK(m.valve(11).revision == 1);
  CHECK(m.setDesiredTarget(1, 0, TargetSource::Web, 0));
  CHECK(m.valve(1).desired == 0);
}

TEST_CASE("target push, ack and verify cycle") {
  ValveModel m;
  m.setActiveMask(0x001);
  REQUIRE(m.setDesiredTarget(0, 60, TargetSource::Web, 100));
  uint8_t valve = 99, pos = 99;
  // Unknown valve (no data yet): no push.
  CHECK_FALSE(m.nextTargetPush(200, valve, pos));
  CHECK(m.isBusy(0));  // Pending
  m.applyValveData(data(0), 300);
  REQUIRE(m.nextTargetPush(400, valve, pos));
  CHECK(valve == 0);
  CHECK(pos == 60);
  CHECK(m.valve(0).sync == TargetSync::AwaitAck);
  CHECK(m.valve(0).pushAttempts == 1);
  CHECK(m.valve(0).lastPushMs == 400);
  CHECK_FALSE(m.valve(0).stmTargetKnown);
  CHECK(m.isBusy(0));
  // Nothing else to push while awaiting the ack.
  CHECK_FALSE(m.nextTargetPush(10000, valve, pos));
  uint8_t verify = 99;
  CHECK_FALSE(m.nextVerify(verify));

  m.onTargetAck(0, 450);
  CHECK(m.valve(0).sync == TargetSync::AwaitVerify);
  CHECK(m.isBusy(0));
  REQUIRE(m.nextVerify(verify));
  CHECK(verify == 0);

  const uint32_t rev = m.valve(0).revision;
  m.applyTarget(target(0, 60), 500);
  CHECK(m.valve(0).sync == TargetSync::Synced);
  CHECK(m.valve(0).pushAttempts == 0);
  CHECK(m.valve(0).stmTargetKnown);
  CHECK(m.valve(0).stmTarget == 60);
  CHECK(m.valve(0).source == TargetSource::Web);
  CHECK(m.valve(0).revision == rev + 1);
  CHECK_FALSE(m.nextVerify(verify));
  CHECK_FALSE(m.isBusy(0));
  CHECK_FALSE(m.nextTargetPush(100000, valve, pos));
}

TEST_CASE("read-back mismatch after the ack re-pushes, spaced by pushRetryMs") {
  ValveModel m;
  m.setActiveMask(0x001);
  m.applyValveData(data(0), 0);
  m.setDesiredTarget(0, 70, TargetSource::Mqtt, 0);
  uint8_t v, p;
  REQUIRE(m.nextTargetPush(1000, v, p));
  m.onTargetAck(0, 1010);
  m.applyTarget(target(0, 30), 1020);  // STM dropped it (calibrating)
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK(m.valve(0).pushAttempts == 1);  // attempts keep counting
  CHECK(m.valve(0).stmTarget == 30);
  CHECK_FALSE(m.nextTargetPush(2999, v, p));
  REQUIRE(m.nextTargetPush(3000, v, p));
  CHECK(m.valve(0).pushAttempts == 2);
}

TEST_CASE("five timed-out pushes end in Failed, retried after failedRetryMs") {
  ValveModelParams params;
  ValveModel m(params);
  m.setActiveMask(0x001);
  m.applyValveData(data(0), 0);
  m.setDesiredTarget(0, 20, TargetSource::Web, 0);
  uint8_t v, p;
  uint32_t now = 10000;
  for (int i = 1; i <= 5; ++i) {
    REQUIRE(m.nextTargetPush(now, v, p));
    CHECK(m.valve(0).pushAttempts == i);
    m.onTargetTimeout(0, now + 400);
    if (i < 5) {
      CHECK(m.valve(0).sync == TargetSync::Pending);
      CHECK((m.valve(0).health & kHealthTargetUnconfirmed) == 0);
      CHECK_FALSE(m.nextTargetPush(now + params.pushRetryMs - 1, v, p));
    }
    now += params.pushRetryMs;
  }
  CHECK(m.valve(0).sync == TargetSync::Failed);
  CHECK((m.valve(0).health & kHealthTargetUnconfirmed) != 0);
  CHECK_FALSE(m.isBusy(0));
  const uint32_t lastPush = m.valve(0).lastPushMs;
  CHECK_FALSE(m.nextTargetPush(lastPush + params.failedRetryMs - 1, v, p));
  REQUIRE(m.nextTargetPush(lastPush + params.failedRetryMs, v, p));
  CHECK(m.valve(0).sync == TargetSync::AwaitAck);
  CHECK(m.valve(0).pushAttempts == 1);
  // Still unconfirmed while the retry is in progress: no flapping.
  CHECK((m.valve(0).health & kHealthTargetUnconfirmed) != 0);
  m.onTargetTimeout(0, lastPush + params.failedRetryMs + 400);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK((m.valve(0).health & kHealthTargetUnconfirmed) != 0);
  // A read-back that matches clears it.
  m.applyTarget(target(0, 20), lastPush + params.failedRetryMs + 500);
  CHECK(m.valve(0).sync == TargetSync::Synced);
  CHECK((m.valve(0).health & kHealthTargetUnconfirmed) == 0);
  // Once confirmed, a later mismatch is an ordinary re-push, not unconfirmed.
  m.applyTarget(target(0, 21), lastPush + params.failedRetryMs + 600);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK((m.valve(0).health & kHealthTargetUnconfirmed) == 0);
}

TEST_CASE("custom maxPushAttempts") {
  ValveModelParams params;
  params.maxPushAttempts = 1;
  ValveModel m(params);
  m.setActiveMask(0x001);
  m.applyValveData(data(0), 0);
  m.setDesiredTarget(0, 20, TargetSource::Web, 0);
  uint8_t v, p;
  REQUIRE(m.nextTargetPush(0, v, p));
  m.onTargetTimeout(0, 400);
  CHECK(m.valve(0).sync == TargetSync::Failed);
}

TEST_CASE("setDesiredTarget on a Failed valve re-arms, same value on Synced is a no-op") {
  ValveModelParams params;
  params.maxPushAttempts = 1;
  ValveModel m(params);
  m.setActiveMask(0x001);
  m.applyValveData(data(0), 0);
  m.setDesiredTarget(0, 20, TargetSource::Web, 0);
  uint8_t v, p;
  REQUIRE(m.nextTargetPush(0, v, p));
  m.onTargetTimeout(0, 400);
  REQUIRE(m.valve(0).sync == TargetSync::Failed);
  CHECK(m.setDesiredTarget(0, 20, TargetSource::Mqtt, 500));
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK(m.valve(0).pushAttempts == 0);
  CHECK(m.valve(0).source == TargetSource::Mqtt);
  CHECK((m.valve(0).health & kHealthTargetUnconfirmed) == 0);

  ValveModel s = syncedModel();
  const ValveState before = s.valve(0);
  CHECK(s.setDesiredTarget(0, 50, TargetSource::Mqtt, 2000));
  CHECK(s.valve(0).revision == before.revision);
  CHECK(s.valve(0).sync == TargetSync::Synced);
  CHECK(s.valve(0).source == TargetSource::Stm);
  // Same value while Pending stays Pending with its attempts.
  ValveModel q;
  q.setActiveMask(1);
  q.applyValveData(data(0), 0);
  q.setDesiredTarget(0, 5, TargetSource::Web, 0);
  REQUIRE(q.nextTargetPush(0, v, p));
  q.onTargetTimeout(0, 400);
  CHECK(q.setDesiredTarget(0, 5, TargetSource::Web, 500));
  CHECK(q.valve(0).sync == TargetSync::Pending);
  CHECK(q.valve(0).pushAttempts == 1);
}

TEST_CASE("a new desired value equal to the known STM target is synced at once") {
  ValveModel m = syncedModel();
  uint8_t v, p;
  REQUIRE(m.setDesiredTarget(0, 30, TargetSource::Web, 2000));
  CHECK(m.valve(0).sync == TargetSync::Pending);
  // Back to 50 before the push: the STM still has 50.
  REQUIRE(m.setDesiredTarget(0, 50, TargetSource::Web, 2100));
  CHECK(m.valve(0).sync == TargetSync::Synced);
  CHECK_FALSE(m.nextTargetPush(9000, v, p));

  // While a push is in flight the STM target is unknown: go Pending.
  REQUIRE(m.setDesiredTarget(0, 30, TargetSource::Web, 9000));
  REQUIRE(m.nextTargetPush(9000, v, p));
  REQUIRE(m.setDesiredTarget(0, 50, TargetSource::Web, 9100));
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK(m.valve(0).pushAttempts == 0);
  // The stale ack/timeout of the old push is ignored (sync is not AwaitAck).
  m.onTargetAck(0, 9200);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  m.onTargetTimeout(0, 9200);
  CHECK(m.valve(0).sync == TargetSync::Pending);

  // A read-back that arrives while the stgtp is in flight does not make the
  // old value current again.
  ValveModel r = syncedModel();
  REQUIRE(r.setDesiredTarget(0, 30, TargetSource::Web, 2000));
  REQUIRE(r.nextTargetPush(4000, v, p));
  r.applyTarget(target(0, 50), 4050);
  REQUIRE(r.valve(0).stmTargetKnown);
  REQUIRE(r.setDesiredTarget(0, 50, TargetSource::Web, 4060));
  CHECK(r.valve(0).sync == TargetSync::Pending);

  // In AwaitVerify as well.
  ValveModel n = syncedModel();
  REQUIRE(n.setDesiredTarget(0, 30, TargetSource::Web, 2000));
  REQUIRE(n.nextTargetPush(4000, v, p));
  n.onTargetAck(0, 4100);
  n.applyTarget(target(0, 50), 4150);  // STM still reports the old value
  CHECK(n.valve(0).sync == TargetSync::Pending);
  REQUIRE(n.nextTargetPush(6100, v, p));
  n.onTargetAck(0, 6200);
  REQUIRE(n.valve(0).sync == TargetSync::AwaitVerify);
  REQUIRE(n.setDesiredTarget(0, 50, TargetSource::Web, 6300));
  CHECK(n.valve(0).sync == TargetSync::Pending);
}

TEST_CASE("read-back handling per sync state") {
  // Adoption at ESP boot.
  ValveModel m;
  m.setActiveMask(0x003);
  m.applyTarget(target(1, 42), 10);
  CHECK(m.valve(1).desiredValid);
  CHECK(m.valve(1).desired == 42);
  CHECK(m.valve(1).source == TargetSource::Stm);
  CHECK(m.valve(1).sync == TargetSync::Synced);
  CHECK(m.valve(1).stmTargetKnown);
  CHECK(m.valve(1).revision == 1);
  // Synced + differing read-back -> Pending.
  m.applyTarget(target(1, 43), 20);
  CHECK(m.valve(1).sync == TargetSync::Pending);
  CHECK(m.valve(1).desired == 42);
  // Pending + equal read-back -> Synced.
  m.applyTarget(target(1, 42), 30);
  CHECK(m.valve(1).sync == TargetSync::Synced);
  // Boundary values are accepted.
  m.applyTarget(target(0, 100), 35);
  CHECK(m.valve(0).desired == 100);
  m.applyTarget(target(0, 0), 36);
  CHECK(m.valve(0).stmTarget == 0);
  // Out-of-range replies are ignored.
  m.applyTarget(target(12, 42), 40);
  m.applyTarget(target(1, 101), 40);
  CHECK(m.valve(1).stmTarget == 42);
  CHECK(m.valve(1).revision == 3);

  // AwaitAck: the read-back may predate the push; sync unchanged.
  m.applyTarget(target(0, 100), 45);  // valve 0 synced: the push below is valve 1's
  m.applyValveData(data(1), 50);
  m.setDesiredTarget(1, 10, TargetSource::Web, 60);
  uint8_t v, p;
  REQUIRE(m.nextTargetPush(70, v, p));
  m.applyTarget(target(1, 10), 80);
  CHECK(m.valve(1).sync == TargetSync::AwaitAck);
  CHECK(m.valve(1).stmTargetKnown);
  CHECK(m.valve(1).stmTarget == 10);

  // Failed + differing read-back stays Failed; equal -> Synced.
  ValveModelParams params;
  params.maxPushAttempts = 1;
  ValveModel f(params);
  f.setActiveMask(1);
  f.applyValveData(data(0), 0);
  f.setDesiredTarget(0, 90, TargetSource::Web, 0);
  REQUIRE(f.nextTargetPush(0, v, p));
  f.onTargetTimeout(0, 400);
  f.applyTarget(target(0, 80), 500);
  CHECK(f.valve(0).sync == TargetSync::Failed);
  f.applyTarget(target(0, 90), 600);
  CHECK(f.valve(0).sync == TargetSync::Synced);
  CHECK(f.valve(0).pushAttempts == 0);
}

TEST_CASE("pushes wait for calibration and skip inactive valves") {
  ValveModel m;
  m.setActiveMask(0x003);
  ValveData d0 = data(0);
  d0.calibrating = true;
  m.applyValveData(d0, 0);
  m.applyValveData(data(1), 0);
  m.setDesiredTarget(0, 10, TargetSource::Web, 0);
  m.setDesiredTarget(1, 20, TargetSource::Web, 0);
  m.setActiveMask(0x001);  // valve 1 inactive now
  uint8_t v, p;
  CHECK_FALSE(m.nextTargetPush(100, v, p));
  CHECK(m.anyCalibrating());
  CHECK(m.isBusy(0));
  d0.calibrating = false;
  m.applyValveData(d0, 200);
  CHECK_FALSE(m.anyCalibrating());
  REQUIRE(m.nextTargetPush(300, v, p));
  CHECK(v == 0);
  CHECK(p == 10);
  CHECK_FALSE(m.nextTargetPush(300, v, p));
  CHECK(m.valve(1).sync == TargetSync::Pending);
}

TEST_CASE("v2: targets are pushed to calibrating valves when not held") {
  ValveModel m;
  m.setActiveMask(0x001);
  ValveEx x = ex(0, 30);  // calState 2: running
  x.calibrating = true;
  m.applyValveEx(x, 0);
  REQUIRE(m.setDesiredTarget(0, 70, TargetSource::Mqtt, 0));
  uint8_t v = 99, p = 99;
  CHECK_FALSE(m.nextTargetPush(100, v, p));  // default: held like 1.x
  m.setHoldTargetsWhileCalibrating(false);
  REQUIRE(m.nextTargetPush(200, v, p));
  CHECK(v == 0);
  CHECK(p == 70);
  m.onTargetAck(0, 250);
  x.target = 70;
  m.applyValveEx(x, 300);  // the STM keeps the target while calibrating
  CHECK(m.valve(0).sync == TargetSync::Synced);
  CHECK(m.isBusy(0));      // still calibrating: fast polling
  m.setHoldTargetsWhileCalibrating(true);
  REQUIRE(m.setDesiredTarget(0, 71, TargetSource::Mqtt, 400));
  CHECK_FALSE(m.nextTargetPush(10000, v, p));
}

TEST_CASE("a push that could not be queued goes back to Pending") {
  ValveModel m = syncedModel();
  REQUIRE(m.setDesiredTarget(0, 80, TargetSource::Web, 2000));
  uint8_t v = 99, p = 99;
  REQUIRE(m.nextTargetPush(3000, v, p));
  CHECK(m.valve(0).pushAttempts == 1);
  const uint32_t rev = m.valve(0).revision;
  m.onTargetPushDropped(0, 3000);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK(m.valve(0).pushAttempts == 0);
  CHECK(m.valve(0).revision == rev + 1);
  CHECK(m.isBusy(0));
  // Retried after pushRetryMs, and dropping forever never reaches Failed.
  CHECK_FALSE(m.nextTargetPush(4999, v, p));
  uint32_t now = 5000;
  for (int i = 0; i < 20; ++i, now += 2000) {
    REQUIRE(m.nextTargetPush(now, v, p));
    CHECK(p == 80);
    m.onTargetPushDropped(0, now);
    CHECK(m.valve(0).sync == TargetSync::Pending);
  }
  CHECK(m.valve(0).health == 0);
  // Queued at last: the normal cycle continues.
  REQUIRE(m.nextTargetPush(now, v, p));
  CHECK(m.valve(0).pushAttempts == 1);
  m.onTargetAck(0, now);
  m.applyTarget(target(0, 80), now);
  CHECK(m.valve(0).sync == TargetSync::Synced);

  // Only a valve in AwaitAck is affected; bad indices are ignored.
  const uint32_t rev2 = m.valve(0).revision;
  m.onTargetPushDropped(0, now);
  m.onTargetPushDropped(12, now);
  CHECK(m.valve(0).sync == TargetSync::Synced);
  CHECK(m.valve(0).revision == rev2);
}

TEST_CASE("a dropped retry of a Failed delivery keeps the flag and attempt count at 0") {
  ValveModel m = syncedModel();
  REQUIRE(m.setDesiredTarget(0, 90, TargetSource::Web, 2000));
  uint8_t v, p;
  uint32_t now = 3000;
  for (int i = 0; i < 5; ++i, now += 2000) {
    REQUIRE(m.nextTargetPush(now, v, p));
    m.onTargetTimeout(0, now);
  }
  REQUIRE(m.valve(0).sync == TargetSync::Failed);
  now += 300000;
  REQUIRE(m.nextTargetPush(now, v, p));  // re-armed retry
  m.onTargetPushDropped(0, now);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK(m.valve(0).pushAttempts == 0);
  CHECK((m.valve(0).health & kHealthTargetUnconfirmed) != 0);
  REQUIRE(m.nextTargetPush(now + 2000, v, p));
  CHECK(m.valve(0).pushAttempts == 1);
}

TEST_CASE("target pushes rotate round robin over valves") {
  ValveModel m;
  m.setActiveMask(0x0FFF);
  for (uint8_t i = 0; i < kValveCount; ++i) {
    m.applyValveData(data(i), 0);
    m.setDesiredTarget(i, static_cast<uint8_t>(i + 1), TargetSource::Web, 0);
  }
  uint8_t v, p;
  for (uint8_t i = 0; i < kValveCount; ++i) {
    REQUIRE(m.nextTargetPush(0, v, p));
    CHECK(v == i);
    CHECK(p == i + 1);
    m.onTargetTimeout(i, 10);
  }
  // All just pushed: spacing blocks everything.
  CHECK_FALSE(m.nextTargetPush(1999, v, p));
  // Cursor wrapped to valve 0; valve 0 first again, then 1.
  REQUIRE(m.nextTargetPush(2000, v, p));
  CHECK(v == 0);
  REQUIRE(m.nextTargetPush(2000, v, p));
  CHECK(v == 1);
  // The cursor moves past the valve just pushed.
  ValveModel c;
  c.setActiveMask(0x003);
  for (uint8_t i = 0; i < 2; ++i) {
    c.applyValveData(data(i), 0);
    c.setDesiredTarget(i, 9, TargetSource::Web, 0);
  }
  REQUIRE(c.nextTargetPush(0, v, p));
  CHECK(v == 0);
  c.onTargetTimeout(0, 1);
  REQUIRE(c.nextTargetPush(5000, v, p));
  CHECK(v == 1);
  c.onTargetTimeout(1, 5001);
  REQUIRE(c.nextTargetPush(10000, v, p));
  CHECK(v == 0);
  // Starting after valve 11 wraps to 0.
  ValveModel w;
  w.setActiveMask(0x0801);
  w.applyValveData(data(0), 0);
  w.applyValveData(data(11), 0);
  w.setDesiredTarget(11, 5, TargetSource::Web, 0);
  REQUIRE(w.nextTargetPush(0, v, p));
  CHECK(v == 11);
  w.setDesiredTarget(0, 6, TargetSource::Web, 0);
  REQUIRE(w.nextTargetPush(0, v, p));
  CHECK(v == 0);
}

TEST_CASE("nextVerify returns the lowest valve awaiting verification") {
  ValveModel m;
  m.setActiveMask(0x0FFF);
  uint8_t v, p;
  for (uint8_t i : {3, 7}) {
    m.applyValveData(data(i), 0);
    m.setDesiredTarget(i, 1, TargetSource::Web, 0);
    REQUIRE(m.nextTargetPush(0, v, p));
    m.onTargetAck(i, 1);
  }
  uint8_t out = 0;
  REQUIRE(m.nextVerify(out));
  CHECK(out == 3);
  m.applyTarget(target(3, 1), 2);
  REQUIRE(m.nextVerify(out));
  CHECK(out == 7);
  // Out-of-range ack/timeout are ignored.
  m.onTargetAck(12, 0);
  m.onTargetTimeout(12, 0);
  // Ack/timeout for a valve that is not awaiting one are ignored.
  const uint32_t rev = m.valve(3).revision;
  REQUIRE(m.valve(3).sync == TargetSync::Synced);
  m.onTargetTimeout(3, 5);
  m.onTargetAck(3, 5);
  CHECK(m.valve(3).sync == TargetSync::Synced);
  CHECK(m.valve(3).revision == rev);
  REQUIRE(m.valve(7).sync == TargetSync::AwaitVerify);
  m.onTargetTimeout(7, 5);
  CHECK(m.valve(7).sync == TargetSync::AwaitVerify);
}

TEST_CASE("onStmRebooted re-pushes desired targets and forgets STM targets") {
  ValveModel m = syncedModel();
  m.setActiveMask(0x003);
  m.applyValveData(data(1), 0);
  m.applyValveEx(ex(0, 50, 5, 6), 1000);
  CHECK(m.valve(0).earlyStopsAtBoot == 5);
  const uint32_t rev0 = m.valve(0).revision;
  const uint32_t rev1 = m.valve(1).revision;
  m.onStmRebooted(2000);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK(m.valve(0).desired == 50);
  CHECK_FALSE(m.valve(0).stmTargetKnown);
  CHECK(m.valve(0).stmTarget == 0);
  CHECK(m.valve(0).revision == rev0 + 1);
  CHECK(m.valve(1).sync == TargetSync::Unknown);
  CHECK(m.valve(1).revision == rev1);  // nothing changed for valve 1
  // v2 counters re-baseline on the next gvlvx (STM counters restarted).
  m.applyValveEx(ex(0, 50, 1, 0), 3000);
  CHECK(m.valve(0).earlyStopsAtBoot == 1);
  CHECK(m.valve(0).cmdRejectedAtBoot == 0);
  CHECK((m.valve(0).health & kHealthEarlyStop) == 0);
  // Read-back equal, but the rebooted STM gets the target once more.
  CHECK(m.valve(0).sync == TargetSync::Pending);
  CHECK(m.valve(0).forcePush);
  // Valve without desired target adopts again.
  m.applyTarget(target(1, 77), 3100);
  CHECK(m.valve(1).desired == 77);
  CHECK(m.valve(1).source == TargetSource::Stm);
  // Every valve is handled, and baselines move up as well as down.
  m.applyValveEx(ex(0, 50, 9, 9), 3200);
  CHECK((m.valve(0).health & kHealthEarlyStop) != 0);
  m.onStmRebooted(3300);
  CHECK(m.valve(1).sync == TargetSync::Pending);
  CHECK(m.valve(0).sync == TargetSync::Pending);
  m.applyValveEx(ex(0, 50, 12, 11), 3400);
  CHECK(m.valve(0).earlyStopsAtBoot == 12);
  CHECK(m.valve(0).cmdRejectedAtBoot == 11);
  CHECK((m.valve(0).health & (kHealthEarlyStop | kHealthCmdRejected)) == 0);
}

TEST_CASE("applyValveData copies every field and ignores bad indices") {
  ValveModel m;
  m.setActiveMask(0x001);
  ValveData d = data(0, 2);
  d.calibrating = true;
  d.temp2 = 199;
  d.calibRetries = 2;
  m.applyValveData(d, 1234);
  const ValveState& v = m.valve(0);
  CHECK(v.known);
  CHECK(v.lastSeenMs == 1234);
  CHECK(v.status == 2);
  CHECK(v.calibrating);
  CHECK(v.position == 40);
  CHECK(v.meanCurrent == 12);
  CHECK(v.temp1 == 215);
  CHECK(v.temp2 == 199);
  CHECK(v.moves == 7);
  CHECK(v.openCount == 3000);
  CHECK(v.closeCount == 3100);
  CHECK(v.deadZone == -4);
  CHECK(v.calibRetries == 2);
  CHECK_FALSE(v.hasExtended);
  CHECK(v.revision == 1);
  // Identical data later: only the timestamp moves, no revision bump.
  m.applyValveData(d, 5000);
  CHECK(m.valve(0).lastSeenMs == 5000);
  CHECK(m.valve(0).revision == 1);
  ValveData bad = data(12);
  m.applyValveData(bad, 1);
  bad.valve = 255;
  m.applyValveData(bad, 1);
  CHECK_FALSE(m.valve(12).known);
}

TEST_CASE("applyValveEx: extended data, baselines, moveSeq and read-back") {
  ValveModel m;
  m.setActiveMask(0x001);
  ValveEx e = ex(0, 25, 3, 4);
  e.lastMove.dir = MoveDir::Close;
  e.lastMove.requestedCounts = 100;
  e.lastMove.countedCounts = 90;
  e.lastMove.stop = StopReason::EarlyEndStop;
  e.lastMove.peakCurrent = 321;
  e.lastMove.durationMs = 4000;
  m.applyValveEx(e, 77);
  const ValveState& v = m.valve(0);
  CHECK(v.known);
  CHECK(v.lastSeenMs == 77);
  CHECK(v.hasExtended);
  CHECK(v.status == 1);
  CHECK(v.position == 33);
  CHECK(v.meanCurrent == 14);
  CHECK(v.openCount == 11);
  CHECK(v.closeCount == 12);
  CHECK(v.deadZone == -3);
  CHECK(v.calibRetries == 1);
  CHECK(v.moves == 99);
  CHECK(v.calState == 2);
  CHECK(v.calFlags == kCalFlagEarlyStop);
  CHECK(v.earlyStops == 3);
  CHECK(v.cmdRejected == 4);
  CHECK(v.earlyStopsAtBoot == 3);
  CHECK(v.cmdRejectedAtBoot == 4);
  CHECK(v.moveSeq == 1);
  CHECK(v.lastMove.stop == StopReason::EarlyEndStop);
  CHECK(v.lastMove.peakCurrent == 321);
  // Read-back adopted the target.
  CHECK(v.desired == 25);
  CHECK(v.source == TargetSource::Stm);
  CHECK(v.stmTarget == 25);
  CHECK((v.health & (kHealthEarlyStop | kHealthCmdRejected)) == 0);
  CHECK((v.health & kHealthCalibRetries) != 0);

  // Same move again: no moveSeq change; each move field counts.
  m.applyValveEx(e, 78);
  CHECK(m.valve(0).moveSeq == 1);
  uint32_t seq = 1;
  for (int field = 0; field < 6; ++field) {
    ValveEx n = e;
    switch (field) {
      case 0: n.lastMove.dir = MoveDir::Open; break;
      case 1: n.lastMove.requestedCounts = 101; break;
      case 2: n.lastMove.countedCounts = 91; break;
      case 3: n.lastMove.stop = StopReason::Target; break;
      case 4: n.lastMove.peakCurrent = 1; break;
      default: n.lastMove.durationMs = 1; break;
    }
    m.applyValveEx(n, 79);
    CHECK(m.valve(0).moveSeq == ++seq);
    m.applyValveEx(e, 80);
    CHECK(m.valve(0).moveSeq == ++seq);
  }

  // Counter increases raise flags.
  m.applyValveEx(ex(0, 25, 4, 4), 90);
  CHECK((m.valve(0).health & kHealthEarlyStop) != 0);
  CHECK((m.valve(0).health & kHealthCmdRejected) == 0);
  m.applyValveEx(ex(0, 25, 4, 5), 91);
  CHECK((m.valve(0).health & kHealthCmdRejected) != 0);
  // A counter below its baseline re-baselines (unnoticed STM reboot).
  m.applyValveEx(ex(0, 25, 2, 5), 92);
  CHECK(m.valve(0).earlyStopsAtBoot == 2);
  CHECK(m.valve(0).cmdRejectedAtBoot == 5);
  CHECK((m.valve(0).health & (kHealthEarlyStop | kHealthCmdRejected)) == 0);
  m.applyValveEx(ex(0, 25, 3, 1), 93);
  CHECK(m.valve(0).earlyStopsAtBoot == 3);
  CHECK(m.valve(0).cmdRejectedAtBoot == 1);

  // One counter at its baseline while the other rises: no re-baseline.
  m.applyValveEx(ex(0, 25, 3, 2), 95);
  CHECK(m.valve(0).cmdRejectedAtBoot == 1);
  CHECK((m.valve(0).health & kHealthCmdRejected) != 0);
  m.applyValveEx(ex(0, 25, 4, 1), 96);
  CHECK(m.valve(0).earlyStopsAtBoot == 3);
  CHECK(m.valve(0).cmdRejectedAtBoot == 1);
  CHECK((m.valve(0).health & kHealthEarlyStop) != 0);

  // Target 100 is a valid read-back.
  m.applyValveEx(ex(0, 100, 4, 1), 97);
  CHECK(m.valve(0).stmTarget == 100);

  // Bad replies are ignored entirely.
  const uint32_t rev = m.valve(0).revision;
  m.applyValveEx(ex(12, 25), 94);
  m.applyValveEx(ex(0, 101, 9, 9), 94);
  CHECK(m.valve(0).revision == rev);
  CHECK(m.valve(0).earlyStops == 4);
}

TEST_CASE("applyValveStates only fills unknown valves") {
  ValveModel m;
  m.setActiveMask(0x0FFF);
  m.applyValveData(data(0, 1), 0);
  ValveStates s;
  for (uint8_t i = 0; i < kValveCount; ++i) s.status[i] = static_cast<uint8_t>(i < 10 ? i : 0x89);
  m.applyValveStates(s, 50);
  CHECK(m.valve(0).status == 1);  // known from gvlvd: untouched
  CHECK(m.valve(0).revision == 1);
  for (uint8_t i = 1; i < 10; ++i) {
    CHECK(m.valve(i).known);
    CHECK(m.valve(i).status == i);
    CHECK(m.valve(i).lastSeenMs == 0);  // not proof of fresh data
  }
  CHECK(m.valve(10).status == 9);  // 0x89 masked to 7 bits
  CHECK(m.valve(11).status == 9);
  CHECK((m.valve(10).health & kHealthBlocked) != 0);
  s.status[1] = 4;
  m.applyValveStates(s, 60);
  CHECK(m.valve(1).status == 1);
  ValveModel fresh;
  s.status[0] = 8;
  fresh.applyValveStates(s, 0);
  CHECK(fresh.valve(0).known);
  CHECK(fresh.valve(0).status == 8);
  CHECK(fresh.valve(0).revision == 1);
}

TEST_CASE("applyValveSensors resolves ids to config slots") {
  OneWireId slots[kTempSlotCount];
  slots[4] = idWithCrc(1);
  slots[9] = idWithCrc(2);
  const OneWireId garbage = idWithCrc(3);  // valid CRC, not configured

  ValveModel m;
  m.setActiveMask(0x0FFF);
  ValveSensors single;
  single.isList = false;
  single.valve = 2;
  single.ids[2][0] = slots[4];
  single.ids[2][1] = garbage;
  single.ids[3][0] = slots[9];  // ignored in the single form
  m.applyValveSensors(single, slots, kTempSlotCount);
  CHECK(m.valve(2).sensorId[0] == slots[4]);
  CHECK(m.valve(2).sensorId[1] == garbage);
  CHECK(m.valve(2).sensorSlot[0] == 5);
  CHECK(m.valve(2).sensorSlot[1] == 0);
  CHECK(m.valve(2).revision == 1);
  CHECK(isZero(m.valve(3).sensorId[0]));
  CHECK(m.valve(3).revision == 0);

  // Out-of-range single form: ignored.
  single.valve = 12;
  m.applyValveSensors(single, slots, kTempSlotCount);

  ValveSensors list;
  list.isList = true;
  list.valve = 200;  // ignored in the list form
  for (uint8_t i = 0; i < kValveCount; ++i) list.ids[i][0] = (i % 2) ? slots[9] : OneWireId{};
  list.ids[0][0] = slots[4];
  list.ids[11][1] = slots[4];
  m.applyValveSensors(list, slots, kTempSlotCount);
  for (uint8_t i = 0; i < kValveCount; ++i) {
    CHECK(m.valve(i).sensorId[0] == list.ids[i][0]);
    CHECK(m.valve(i).sensorSlot[0] == (i == 0 ? 5 : (i % 2) ? 10 : 0));
    CHECK(m.valve(i).sensorId[1] == list.ids[i][1]);
  }
  CHECK(m.valve(11).sensorSlot[1] == 5);
  CHECK(m.valve(10).sensorSlot[1] == 0);
  // A one-slot table resolves its only slot.
  ValveSensors one;
  one.valve = 7;
  one.ids[7][1] = slots[4];
  OneWireId table[1];
  table[0] = slots[4];
  m.applyValveSensors(one, table, 1);
  CHECK(m.valve(7).sensorSlot[1] == 1);
  // No slot table: slots stay 0 even for configured ids.
  list.ids[1][0] = slots[4];
  m.applyValveSensors(list, nullptr, kTempSlotCount);
  CHECK(m.valve(1).sensorSlot[0] == 0);
  m.applyValveSensors(list, slots, 0);
  CHECK(m.valve(1).sensorSlot[0] == 0);
}

TEST_CASE("applySensorTemps: v2 valve temperatures from gvlon + goned") {
  SensorModel s;
  OneWireList l;
  l.count = 3;
  l.hasList = true;
  l.ids[0] = idWithCrc(1);
  l.ids[1] = idWithCrc(2);
  l.ids[2] = idWithCrc(3);
  s.applyTempList(l, 0);
  auto read = [&](uint8_t bus, int16_t raw, uint32_t now) {
    TempData td;
    td.valid = true;
    td.id = l.ids[bus];
    td.value = raw;
    s.applyTempData(bus, td, now);
  };

  ValveModel m;
  m.setActiveMask(0x00F);
  ValveSensors vs;
  vs.isList = true;
  vs.ids[0][0] = idWithCrc(1);
  vs.ids[0][1] = idWithCrc(2);
  vs.ids[1][0] = idWithCrc(3);
  vs.ids[2][0] = idWithCrc(7);  // not on the bus
  m.applyValveSensors(vs, nullptr, 0);

  // Nothing read yet: nothing to publish.
  const uint32_t rev0 = m.valve(0).revision;
  m.applySensorTemps(s, 1000, 60000);
  CHECK(m.valve(0).temp1 == kTempUnassigned);
  CHECK(m.valve(0).revision == rev0);

  read(0, 215, 1000);
  read(1, -1270, 1000);  // the STM reports a read error for this sensor twice
  read(1, -1270, 1000);
  read(2, 199, 1000);
  m.applySensorTemps(s, 2000, 60000);
  CHECK(m.valve(0).temp1 == 215);
  CHECK(m.valve(0).temp2 == -1270);
  CHECK(m.valve(0).revision == rev0 + 1);
  CHECK((m.valve(0).health & kHealthTempFailed) != 0);
  CHECK(m.valve(1).temp1 == 199);
  CHECK(m.valve(1).temp2 == kTempUnassigned);  // zero id
  CHECK(m.valve(2).temp1 == kTempUnassigned);  // not on the bus, not settled
  CHECK(m.valve(3).temp1 == kTempUnassigned);
  // Unchanged readings: no revision bump.
  m.applySensorTemps(s, 3000, 60000);
  CHECK(m.valve(0).revision == rev0 + 1);
  CHECK((diffValve(ValveState{}, m.valve(1)) & kChangeTemp1) != 0);

  // Readings older than maxAge are read errors; fresh ones come back.
  read(0, 216, 50000);
  m.applySensorTemps(s, 61001, 60000);
  CHECK(m.valve(0).temp1 == 216);
  CHECK(m.valve(0).temp2 == kTempReadError);
  CHECK(m.valve(1).temp1 == kTempReadError);
  m.applySensorTemps(s, 61000, 60000);
  CHECK(m.valve(1).temp1 == 199);  // exactly maxAge old: still fresh

  // A sensor that left the bus (list re-read without it).
  l.count = 1;
  s.applyTempList(l, 62000);
  m.applySensorTemps(s, 62000, 60000);
  CHECK(m.valve(0).temp1 == 216);
  CHECK(m.valve(1).temp1 == kTempUnassigned);  // gone, not settled
  m.applySensorTemps(s, 62000, 60000, true);
  CHECK(m.valve(1).temp1 == kTempReadError);   // gone, settled
  CHECK(m.valve(2).temp1 == kTempReadError);   // never on the bus, settled
  CHECK(m.valve(0).temp1 == 216);
  CHECK(m.valve(3).temp1 == kTempUnassigned);  // nothing assigned stays unassigned
}

TEST_CASE("applySensorTemps: a sensor on the bus that was never read is a read error once settled") {
  SensorModel s;
  OneWireList l;
  l.count = 2;
  l.hasList = true;
  l.ids[0] = idWithCrc(1);
  l.ids[1] = idWithCrc(2);
  s.applyTempList(l, 0);
  TempData td;
  td.valid = true;
  td.id = l.ids[1];
  td.value = 199;
  s.applyTempData(1, td, 1000);
  ValveModel m;
  m.setActiveMask(0x001);
  ValveSensors vs;
  vs.isList = true;
  vs.ids[0][0] = idWithCrc(1);  // on the bus, never read
  vs.ids[0][1] = idWithCrc(2);  // on the bus, fresh
  m.applyValveSensors(vs, nullptr, 0);
  m.applySensorTemps(s, 2000, 60000, false);
  CHECK(m.valve(0).temp1 == kTempUnassigned);
  CHECK(m.valve(0).temp2 == 199);
  m.applySensorTemps(s, 2000, 60000, true);
  CHECK(m.valve(0).temp1 == kTempReadError);
  CHECK(m.valve(0).temp2 == 199);
  CHECK((m.valve(0).health & kHealthTempFailed) != 0);
}

TEST_CASE("applySensorTemps: the combined table per sensor") {
  SensorModel s;
  OneWireList l;
  l.count = 2;
  l.hasList = true;
  l.ids[0] = idWithCrc(1);
  l.ids[1] = idWithCrc(2);
  s.applyTempList(l, 0);
  auto read = [&](uint8_t bus, int16_t raw, uint32_t now) {
    TempData td;
    td.valid = true;
    td.id = l.ids[bus];
    td.value = raw;
    s.applyTempData(bus, td, now);
  };
  ValveModel m;
  m.setActiveMask(0x007);
  ValveSensors vs;
  vs.isList = true;
  vs.ids[0][0] = idWithCrc(1);
  vs.ids[0][1] = idWithCrc(2);
  OneWireId badCrc = idWithCrc(1);
  badCrc.b[7] ^= 1;
  vs.ids[1][0] = badCrc;
  m.applyValveSensors(vs, nullptr, 0);

  read(0, 215, 1000);
  read(1, 199, 1000);
  m.applySensorTemps(s, 1000, 60000, true);
  CHECK(m.valve(0).temp1 == 215);
  CHECK(m.valve(0).temp2 == 199);
  CHECK(m.valve(1).temp1 == kTempUnassigned);  // bad CRC: nothing assigned, even settled
  CHECK(m.valve(1).temp2 == kTempUnassigned);  // zero id
  // One failed read after a good one: the previous value (debounce).
  read(0, kTempReadError, 2000);
  m.applySensorTemps(s, 2000, 60000, true);
  CHECK(m.valve(0).temp1 == 215);
  CHECK(m.valve(0).temp2 == 199);  // sensor 2 independent of sensor 1
  read(0, kTempReadError, 3000);
  m.applySensorTemps(s, 3000, 60000, true);
  CHECK(m.valve(0).temp1 == kTempReadError);
  CHECK(m.valve(0).temp2 == 199);
  // The STM's temperature cycle is older than 200 s: every reading is stale.
  read(0, 220, 4000);
  s.setStmTempAge(201, 4000);
  m.applySensorTemps(s, 4000, 60000, true);
  CHECK(m.valve(0).temp1 == kTempReadError);
  CHECK(m.valve(0).temp2 == kTempReadError);
  s.setStmTempAge(0, 5000);
  m.applySensorTemps(s, 5000, 60000, true);
  CHECK(m.valve(0).temp1 == 220);
}

TEST_CASE("health flags per condition and activity") {
  ValveModel m;
  m.setActiveMask(0x001);
  m.applyValveData(data(0, 9), 0);
  CHECK(m.valve(0).health == kHealthBlocked);
  m.applyValveData(data(0, 4), 0);
  CHECK(m.valve(0).health == kHealthFailed);
  m.applyValveData(data(0, 6), 0);
  CHECK(m.valve(0).health == kHealthNoValve);
  m.applyValveData(data(0, 1), 0);
  CHECK(m.valve(0).health == 0);
  ValveData d = data(0, 1);
  d.calibRetries = 1;
  m.applyValveData(d, 0);
  CHECK(m.valve(0).health == kHealthCalibRetries);
  for (int16_t bad : {kTempReadError, kTempPowerOn, static_cast<int16_t>(1251),
                      static_cast<int16_t>(-551)}) {
    d = data(0, 1);
    d.temp1 = bad;
    m.applyValveData(d, 0);
    CHECK(m.valve(0).health == kHealthTempFailed);
    d.temp1 = kTempUnassigned;
    d.temp2 = bad;
    m.applyValveData(d, 0);
    CHECK(m.valve(0).health == kHealthTempFailed);
  }
  d = data(0, 1);
  d.temp1 = kTempUnassigned;
  d.temp2 = kTempUnassigned;
  m.applyValveData(d, 0);
  CHECK(m.valve(0).health == 0);
  d.temp1 = 1250;
  d.temp2 = -550;
  m.applyValveData(d, 0);
  CHECK(m.valve(0).health == 0);

  // Inactive: only Blocked / Failed.
  m.setActiveMask(0);
  d = data(0, 6);
  d.calibRetries = 2;
  d.temp1 = kTempReadError;
  m.applyValveData(d, 0);
  CHECK(m.valve(0).health == 0);
  m.applyValveData(data(0, 9), 0);
  CHECK(m.valve(0).health == kHealthBlocked);
  m.applyValveData(data(0, 4), 0);
  CHECK(m.valve(0).health == kHealthFailed);
  // Activation recomputes immediately.
  m.applyValveData(data(0, 6), 0);
  const uint32_t rev = m.valve(0).revision;
  m.setActiveMask(1);
  CHECK(m.valve(0).health == kHealthNoValve);
  CHECK(m.valve(0).revision == rev + 1);
  m.setActiveMask(1);  // no change, no bump
  CHECK(m.valve(0).revision == rev + 1);
}

TEST_CASE("staleness: measured from data or activation, latched, cleared by data") {
  ValveModelParams params;
  ValveModel m(params);
  m.setActiveMask(0x003);
  // Never-seen active valve: measured from the first tick.
  m.tick(1000);
  CHECK(m.valve(1).health == 0);
  m.tick(1000 + params.staleMs - 1);
  CHECK(m.valve(1).health == 0);
  m.tick(1000 + params.staleMs);
  CHECK(m.valve(1).health == kHealthStale);
  CHECK(m.valve(0).health == kHealthStale);

  m.applyValveData(data(0), 100000);
  CHECK(m.valve(0).health == 0);
  m.tick(100000 + params.staleMs - 1);
  CHECK(m.valve(0).health == 0);
  m.tick(100000 + params.staleMs);
  CHECK(m.valve(0).health == kHealthStale);
  // gvlst does not refresh.
  ValveStates s;
  m.applyValveStates(s, 200000);
  CHECK(m.valve(0).health == kHealthStale);
  // gvlvx does.
  m.applyValveEx(ex(0, 1), 300000);
  CHECK((m.valve(0).health & kHealthStale) == 0);

  // Latched across a millis() wrap.
  ValveModel w;
  w.setActiveMask(1);
  const uint32_t t0 = 0xFFFF0000u;
  w.applyValveData(data(0), t0);
  w.tick(t0 + params.staleMs);
  CHECK((w.valve(0).health & kHealthStale) != 0);
  w.tick(t0 + 0x10000u + 5);  // wrapped: elapsed looks small again
  CHECK((w.valve(0).health & kHealthStale) != 0);

  // Deactivating clears; re-activating measures from the next tick.
  w.setActiveMask(0);
  CHECK((w.valve(0).health & kHealthStale) == 0);
  w.tick(50);
  CHECK((w.valve(0).health & kHealthStale) == 0);
  w.setActiveMask(1);
  w.tick(100);
  CHECK((w.valve(0).health & kHealthStale) == 0);
  w.tick(100 + params.staleMs);
  CHECK((w.valve(0).health & kHealthStale) != 0);

  // Custom threshold.
  ValveModelParams shortParams;
  shortParams.staleMs = 10;
  ValveModel q(shortParams);
  q.setActiveMask(1);
  q.tick(0);
  q.tick(9);
  CHECK(q.valve(0).health == 0);
  q.tick(10);
  CHECK(q.valve(0).health == kHealthStale);
}

TEST_CASE("isBusy branches") {
  ValveModel m;
  m.setActiveMask(0x001);
  m.applyValveData(data(0, 1), 0);
  CHECK_FALSE(m.isBusy(0));
  m.applyValveData(data(0, 2), 0);
  CHECK(m.isBusy(0));
  m.applyValveData(data(0, 3), 0);
  CHECK(m.isBusy(0));
  m.applyValveData(data(0, 7), 0);
  CHECK_FALSE(m.isBusy(0));
  ValveData d = data(0, 1);
  d.calibrating = true;
  m.applyValveData(d, 0);
  CHECK(m.isBusy(0));
  m.applyValveData(data(0, 1), 0);
  m.applyTarget(target(0, 5), 0);
  CHECK_FALSE(m.isBusy(0));
}

TEST_CASE("revision bumps on every meaningful change only") {
  ValveModel m = syncedModel();
  const uint32_t r = m.valve(0).revision;
  m.tick(2000);  // not stale yet, nothing changes
  CHECK(m.valve(0).revision == r);
  ValveData d = data(0);
  d.position = 41;
  m.applyValveData(d, 2000);
  CHECK(m.valve(0).revision == r + 1);
  m.setActiveMask(0x001);
  CHECK(m.valve(0).revision == r + 1);
}

// ---------------------------------------------------------------- SensorModel

TEST_CASE("tempRawValid and vadValid boundaries") {
  CHECK(tempRawValid(0));
  CHECK(tempRawValid(215));
  CHECK(tempRawValid(-550));
  CHECK_FALSE(tempRawValid(-551));
  CHECK(tempRawValid(1250));
  CHECK_FALSE(tempRawValid(1251));
  CHECK_FALSE(tempRawValid(kTempUnassigned));
  CHECK_FALSE(tempRawValid(kTempReadError));
  CHECK_FALSE(tempRawValid(kTempPowerOn));
  CHECK(tempRawValid(849));
  CHECK(tempRawValid(851));
  CHECK(tempRawValid(-499));
  CHECK(tempRawValid(-501));
  CHECK_FALSE(tempRawValid(INT16_MIN));
  CHECK_FALSE(tempRawValid(INT16_MAX));
  CHECK_FALSE(vadValid(kVadFailed));
  CHECK_FALSE(vadValid(-5000));
  CHECK(vadValid(-999));
  CHECK(vadValid(0));
  CHECK(vadValid(INT32_MAX));
}

TEST_CASE("SensorModel temperature list and data") {
  SensorModel s;
  CHECK(s.tempCount() == 0);
  CHECK(s.findTemp(idWithCrc(1)) == -1);
  OneWireList l;
  l.count = 3;
  l.hasList = false;
  CHECK(s.applyTempList(l, 0));
  CHECK(s.tempCount() == 3);
  CHECK_FALSE(s.applyTempList(l, 0));  // same count
  l.hasList = true;
  l.ids[0] = idWithCrc(1);
  l.ids[1] = idWithCrc(2);
  l.ids[2] = idWithCrc(3);
  CHECK_FALSE(s.applyTempList(l, 0));
  CHECK(s.temp(0).id == idWithCrc(1));
  CHECK(s.temp(1).id == idWithCrc(2));
  CHECK(isZero(s.temp(3).id));
  CHECK(s.findTemp(idWithCrc(1)) == 0);
  CHECK_FALSE(s.temp(1).seen);
  CHECK(s.findTemp(idWithCrc(3)) == 2);
  CHECK(s.findTemp(OneWireId{}) == -1);

  TempData td;
  td.valid = true;
  td.id = idWithCrc(2);
  td.value = 205;
  s.applyTempData(1, td, 500);
  CHECK(s.temp(1).seen);
  CHECK(s.temp(1).raw == 205);
  CHECK(s.temp(1).lastSeenMs == 500);
  CHECK(s.tempFresh(1, 500, 0));
  CHECK(s.tempFresh(1, 60500, 60000));
  CHECK_FALSE(s.tempFresh(1, 60501, 60000));
  CHECK_FALSE(s.tempFresh(0, 500, 60000));  // never seen
  CHECK_FALSE(s.tempFresh(34, 500, 60000));

  // Same list again keeps the reading; a different id at an index resets it.
  CHECK_FALSE(s.applyTempList(l, 600));
  CHECK(s.temp(1).raw == 205);
  l.ids[1] = idWithCrc(9);
  s.applyTempList(l, 700);
  CHECK(s.temp(1).id == idWithCrc(9));
  CHECK_FALSE(s.temp(1).seen);
  CHECK(s.temp(1).raw == kTempUnassigned);

  // goned carries the authoritative id.
  td.id = idWithCrc(4);
  s.applyTempData(1, td, 800);
  CHECK(s.temp(1).id == idWithCrc(4));
  CHECK(s.findTemp(idWithCrc(4)) == 1);

  // Invalid form marks not seen (the second one in a row).
  TempData inv;
  s.applyTempData(1, inv, 900);
  CHECK(s.temp(1).seen);
  s.applyTempData(1, inv, 900);
  CHECK_FALSE(s.temp(1).seen);
  CHECK(s.temp(1).raw == kTempUnassigned);
  CHECK_FALSE(s.tempFresh(1, 900, 60000));

  // Out-of-range bus index ignored; temp() of it returns the empty reading.
  s.applyTempData(34, td, 1000);
  CHECK_FALSE(s.temp(34).seen);
  CHECK(&s.temp(34) == &s.temp(200));
  CHECK(isZero(s.volt(0).id));  // nothing written past the temperature table
  CHECK_FALSE(s.volt(0).seen);
  CHECK(s.voltCount() == 0);

  // Shrinking the count clears readings beyond it.
  s.applyTempData(2, td, 1000);
  l.count = 2;
  l.hasList = false;
  CHECK(s.applyTempList(l, 1100));
  CHECK(s.tempCount() == 2);
  CHECK_FALSE(s.temp(2).seen);
  CHECK(isZero(s.temp(2).id));
  CHECK(s.findTemp(idWithCrc(3)) == -1);

  // Ids beyond the count are ignored.
  l.count = 3;
  l.hasList = true;
  l.ids[3] = idWithCrc(11);
  s.applyTempList(l, 1200);
  CHECK(isZero(s.temp(3).id));
  CHECK(s.findTemp(idWithCrc(11)) == -1);

  // Count is clamped to the bus maximum.
  l.count = 200;
  l.hasList = false;
  CHECK(s.applyTempList(l, 0));
  CHECK(s.tempCount() == kTempSlotCount);
  // A full table is searched to its end and no further.
  OneWireList vl;
  vl.count = 1;
  vl.hasList = true;
  vl.ids[0] = idWithCrc(12);
  s.applyVoltList(vl, 0);
  CHECK(s.findTemp(idWithCrc(12)) == -1);
  TempData last;
  last.valid = true;
  last.id = idWithCrc(13);
  last.value = 1;
  s.applyTempData(33, last, 0);
  CHECK(s.findTemp(idWithCrc(13)) == 33);

  s.clear();
  CHECK(s.tempCount() == 0);
  CHECK(s.voltCount() == 0);
  CHECK(isZero(s.temp(0).id));
}

TEST_CASE("SensorModel volt list and data") {
  SensorModel s;
  OneWireList l;
  l.count = 2;
  l.hasList = true;
  l.ids[0] = idWithCrc(5);
  l.ids[1] = idWithCrc(6);
  CHECK(s.applyVoltList(l, 0));
  CHECK(s.voltCount() == 2);
  CHECK_FALSE(s.applyVoltList(l, 0));
  CHECK(s.findVolt(idWithCrc(6)) == 1);
  CHECK(s.findVolt(idWithCrc(5)) == 0);
  CHECK(s.volt(0).id == idWithCrc(5));
  CHECK(isZero(s.volt(2).id));
  CHECK(s.findVolt(OneWireId{}) == -1);
  CHECK(s.findVolt(idWithCrc(7)) == -1);

  VoltData vd;
  vd.valid = true;
  vd.id = idWithCrc(6);
  vd.vad = 1234;
  s.applyVoltData(1, vd, 10);
  CHECK(s.volt(1).seen);
  CHECK(s.volt(1).vad == 1234);
  CHECK(s.volt(1).lastSeenMs == 10);
  VoltData inv;
  s.applyVoltData(1, inv, 20);
  CHECK(s.volt(1).seen);
  s.applyVoltData(1, inv, 20);
  CHECK_FALSE(s.volt(1).seen);
  CHECK(s.volt(1).vad == kVadFailed);
  s.applyVoltData(8, vd, 30);
  CHECK_FALSE(s.volt(8).seen);
  CHECK(&s.volt(8) == &s.volt(255));
  CHECK(s.voltCount() == 2);  // nothing written past the volt table
  CHECK(s.tempCount() == 0);
  // A full volt table is searched to its end and no further.
  OneWireList full;
  full.count = kVoltSlotCount;
  full.hasList = true;
  for (uint8_t i = 0; i < kVoltSlotCount; ++i) full.ids[i] = idWithCrc(static_cast<uint8_t>(40 + i));
  s.applyVoltList(full, 0);
  CHECK(s.findVolt(idWithCrc(47)) == 7);
  CHECK(s.findVolt(idWithCrc(48)) == -1);

  l.ids[1] = idWithCrc(8);
  s.applyVoltData(1, vd, 40);
  s.applyVoltList(l, 50);
  CHECK(s.volt(1).id == idWithCrc(8));
  CHECK_FALSE(s.volt(1).seen);

  l.count = 20;
  l.hasList = false;
  CHECK(s.applyVoltList(l, 0));
  CHECK(s.voltCount() == kVoltSlotCount);
  l.count = 1;
  CHECK(s.applyVoltList(l, 0));
  CHECK(isZero(s.volt(1).id));
  CHECK(s.temp(0).raw == kTempUnassigned);
}

TEST_CASE("applySensorTemps: only the second sensor changes") {
  SensorModel s;
  OneWireList l;
  l.count = 2;
  l.hasList = true;
  l.ids[0] = idWithCrc(1);
  l.ids[1] = idWithCrc(2);
  s.applyTempList(l, 0);
  auto read = [&](uint8_t bus, int16_t raw, uint32_t now) {
    TempData td;
    td.valid = true;
    td.id = l.ids[bus];
    td.value = raw;
    s.applyTempData(bus, td, now);
  };
  ValveModel m;
  m.setActiveMask(0x001);
  ValveSensors vs;
  vs.isList = true;
  vs.ids[0][0] = idWithCrc(1);
  vs.ids[0][1] = idWithCrc(2);
  m.applyValveSensors(vs, nullptr, 0);
  const uint32_t rev0 = m.valve(0).revision;
  // Sensor 1 not read yet (stays unassigned, equal to the old temp2).
  read(1, 215, 1000);
  m.applySensorTemps(s, 1000, 60000);
  CHECK(m.valve(0).temp1 == kTempUnassigned);
  CHECK(m.valve(0).temp2 == 215);
  CHECK(m.valve(0).revision == rev0 + 1);
  // temp1 unchanged, temp2 changes again.
  read(1, 220, 2000);
  m.applySensorTemps(s, 2000, 60000);
  CHECK(m.valve(0).temp1 == kTempUnassigned);
  CHECK(m.valve(0).temp2 == 220);
  CHECK(m.valve(0).revision == rev0 + 2);
  // temp1 changes to the old temp2 value, temp2 unchanged.
  read(0, 220, 3000);
  m.applySensorTemps(s, 3000, 60000);
  CHECK(m.valve(0).temp1 == 220);
  CHECK(m.valve(0).temp2 == 220);
  CHECK(m.valve(0).revision == rev0 + 3);
}
