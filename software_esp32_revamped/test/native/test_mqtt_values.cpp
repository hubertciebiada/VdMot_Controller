// Values published over MQTT: systemState (legacy common/state), sensor
// slots, segments, targets, problem flag, names, calibration ends, the
// on-change key of a valve.
#include <string.h>

#include <functional>
#include <string>
#include <utility>
#include <vector>

#include "doctest.h"
#include "vdm/mqtt_values.h"

using namespace vdm;

namespace {

// A valve with a value in every field (none at its default).
ValveState fullValve() {
  ValveState v;
  v.known = true;
  v.lastSeenMs = 1000;
  v.status = 3;
  v.calibrating = true;
  v.position = 40;
  v.meanCurrent = 12;
  v.temp1 = 215;
  v.temp2 = 198;
  v.moves = 70;
  v.openCount = 30;
  v.closeCount = 29;
  v.deadZone = -4;
  v.calibRetries = 1;
  v.hasExtended = true;
  v.calState = 2;
  v.calFlags = 1;
  v.earlyStops = 5;
  v.cmdRejected = 6;
  v.earlyStopsAtBoot = 2;
  v.cmdRejectedAtBoot = 3;
  v.lastMove.countedCounts = 100;
  v.moveSeq = 8;
  v.desiredValid = true;
  v.desired = 55;
  v.source = TargetSource::Web;
  v.stmTargetKnown = true;
  v.stmTarget = 50;
  v.sync = TargetSync::Pending;
  v.pushAttempts = 2;
  v.lastPushMs = 900;
  v.sensorId[0].b[0] = 0x28;
  v.sensorId[1].b[7] = 0x11;
  v.sensorSlot[0] = 3;
  v.sensorSlot[1] = 4;
  v.health = 1;
  v.revision = 17;
  v.hasV3 = true;
  v.stmFlags = 2;
  v.fault = 1;
  v.fsPct = 30;
  v.drive = 45;
  v.retryS = 60;
  v.retries = 1;
  v.autoRetry = true;
  v.fsOverride = true;
  v.fsTarget = 20;
  v.forcePush = true;
  return v;
}

}  // namespace

TEST_CASE("valveCompatKey: exactly the fields of the compat groups change the key") {
  const ValveState base = fullValve();
  const uint32_t key = valveCompatKey(base);
  CHECK(valveCompatKey(fullValve()) == key);
  using Change = std::pair<const char*, std::function<void(ValveState&)>>;
  const std::vector<Change> changes = {
      {"known", [](ValveState& v) { v.known = false; }},
      {"lastSeenMs", [](ValveState& v) { ++v.lastSeenMs; }},
      {"status", [](ValveState& v) { ++v.status; }},
      {"calibrating", [](ValveState& v) { v.calibrating = false; }},
      {"position", [](ValveState& v) { ++v.position; }},
      {"meanCurrent", [](ValveState& v) { ++v.meanCurrent; }},
      {"temp1", [](ValveState& v) { ++v.temp1; }},
      {"temp2", [](ValveState& v) { ++v.temp2; }},
      {"moves", [](ValveState& v) { ++v.moves; }},
      {"openCount", [](ValveState& v) { ++v.openCount; }},
      {"closeCount", [](ValveState& v) { ++v.closeCount; }},
      {"deadZone", [](ValveState& v) { ++v.deadZone; }},
      {"calibRetries", [](ValveState& v) { ++v.calibRetries; }},
      {"hasExtended", [](ValveState& v) { v.hasExtended = false; }},
      {"calState", [](ValveState& v) { ++v.calState; }},
      {"calFlags", [](ValveState& v) { ++v.calFlags; }},
      {"earlyStops", [](ValveState& v) { ++v.earlyStops; }},
      {"cmdRejected", [](ValveState& v) { ++v.cmdRejected; }},
      {"earlyStopsAtBoot", [](ValveState& v) { ++v.earlyStopsAtBoot; }},
      {"cmdRejectedAtBoot", [](ValveState& v) { ++v.cmdRejectedAtBoot; }},
      {"lastMove", [](ValveState& v) { ++v.lastMove.countedCounts; }},
      {"moveSeq", [](ValveState& v) { ++v.moveSeq; }},
      {"desiredValid", [](ValveState& v) { v.desiredValid = false; }},
      {"desired", [](ValveState& v) { ++v.desired; }},
      {"source", [](ValveState& v) { v.source = TargetSource::Mqtt; }},
      {"stmTargetKnown", [](ValveState& v) { v.stmTargetKnown = false; }},
      {"stmTarget", [](ValveState& v) { ++v.stmTarget; }},
      {"sync", [](ValveState& v) { v.sync = TargetSync::Synced; }},
      {"pushAttempts", [](ValveState& v) { ++v.pushAttempts; }},
      {"lastPushMs", [](ValveState& v) { ++v.lastPushMs; }},
      {"sensorId[0] first byte", [](ValveState& v) { ++v.sensorId[0].b[0]; }},
      {"sensorId[0] last byte", [](ValveState& v) { ++v.sensorId[0].b[7]; }},
      {"sensorId[1] first byte", [](ValveState& v) { ++v.sensorId[1].b[0]; }},
      {"sensorId[1] last byte", [](ValveState& v) { ++v.sensorId[1].b[7]; }},
      {"sensorSlot[0]", [](ValveState& v) { ++v.sensorSlot[0]; }},
      {"sensorSlot[1]", [](ValveState& v) { ++v.sensorSlot[1]; }},
      {"health", [](ValveState& v) { v.health = 0x100; }},
      {"revision", [](ValveState& v) { ++v.revision; }},
      {"hasV3", [](ValveState& v) { v.hasV3 = false; }},
      {"stmFlags", [](ValveState& v) { v.stmFlags = 0x100; }},
      {"fault", [](ValveState& v) { ++v.fault; }},
      {"fsPct", [](ValveState& v) { ++v.fsPct; }},
      {"drive", [](ValveState& v) { ++v.drive; }},
      {"retryS", [](ValveState& v) { ++v.retryS; }},
      {"retries", [](ValveState& v) { ++v.retries; }},
      {"autoRetry", [](ValveState& v) { v.autoRetry = false; }},
      {"fsOverride", [](ValveState& v) { v.fsOverride = false; }},
      {"fsTarget", [](ValveState& v) { ++v.fsTarget; }},
      {"forcePush", [](ValveState& v) { v.forcePush = false; }},
  };
  size_t keyed = 0;
  for (const Change& c : changes) {
    CAPTURE(c.first);
    ValveState v = base;
    c.second(v);
    const bool compat = (diffValve(base, v) & kValveCompatMask) != 0;
    CHECK((valveCompatKey(v) != key) == compat);
    keyed += compat;
  }
  CHECK(keyed == 33);  // the fields of the compat groups (sensor ids at both ends)
}

TEST_CASE("valveCompatKey: the default valve and the padding") {
  ValveState a;
  ValveState b;
  memset(static_cast<void*>(&b), 0xa5, sizeof b);  // every byte, padding included
  b = ValveState{};                                  // the fields back at their defaults
  CHECK(valveCompatKey(a) == valveCompatKey(b));
  CHECK(valveCompatKey(a) != valveCompatKey(fullValve()));
}

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
                             kHealthTempFailed,  kHealthFailsafe,         kHealthStrokeShort};
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

TEST_CASE("systemState: safe mode and failsafe") {
  ValveState v[kValveCount];
  SystemFlags safe;
  safe.safeMode = true;
  SystemFlags fs;
  fs.failsafe = true;
  SystemFlags both;
  both.safeMode = true;
  both.failsafe = true;
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF, SystemFlags{}) == 0);
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF, safe) == 2);
  CHECK(systemState(LinkState::Up, nullptr, 0, 0, safe) == 2);
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF, fs) == 1);
  CHECK(systemState(LinkState::Up, nullptr, 0, 0, fs) == 1);
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF, both) == 2);
  CHECK(systemState(LinkState::Degraded, v, kValveCount, 0x0FFF, safe) == 2);
  CHECK(systemState(LinkState::Down, v, kValveCount, 0x0FFF, fs) == 2);
  // An error of a valve outranks the failsafe.
  v[2].health = kHealthFailed;
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF, fs) == 2);
  v[2].health = kHealthFailsafe;
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF, fs) == 1);
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF) == 1);
}

namespace {

OneWireId id(uint8_t last) {
  OneWireId o;
  o.b[0] = 0x28;
  o.b[7] = last;
  return o;
}

std::string seg(const Config& c, ItemKind k, uint8_t i, int bus, size_t cap = 16) {
  char out[16];
  memset(out, 'X', sizeof out);
  const size_t n = sensorTopicSegment(c, k, i, bus, out, cap);
  CHECK(n == strlen(out));
  return out;
}

}  // namespace

TEST_CASE("activeValveMask") {
  static Config c;
  c = Config{};
  CHECK(activeValveMask(c) == 0);
  c.valves[0].active = true;
  c.valves[11].active = true;
  c.valves[5].active = true;
  CHECK(activeValveMask(c) == 0x0821);
}

TEST_CASE("slot temperatures and volts from the bus readings") {
  static Config c;
  c = Config{};
  c.temps[2].id = id(3);
  c.temps[2].offset = -5;
  TempReading t[3];
  t[1].id = id(3);
  t[1].seen = true;
  t[1].raw = 215;
  t[1].lastSeenMs = 1000;
  int32_t tenths = 0;
  CHECK(slotTempTenths(c, t, 3, 3, 61000, 60000, tenths));
  CHECK(tenths == 210);
  CHECK_FALSE(slotTempTenths(c, t, 3, 3, 61001, 60000, tenths));  // stale
  CHECK_FALSE(slotTempTenths(c, t, 1, 3, 1000, 60000, tenths));   // beyond count
  CHECK_FALSE(slotTempTenths(c, t, 3, 0, 1000, 60000, tenths));
  CHECK_FALSE(slotTempTenths(c, t, 3, kTempSlotCount + 1, 1000, 60000, tenths));
  CHECK_FALSE(slotTempTenths(c, t, 3, 2, 1000, 60000, tenths));   // empty slot
  t[1].raw = kTempReadError;
  CHECK_FALSE(slotTempTenths(c, t, 3, 3, 1000, 60000, tenths));
  t[1].raw = 215;
  t[1].seen = false;
  CHECK_FALSE(slotTempTenths(c, t, 3, 3, 1000, 60000, tenths));
  CHECK_FALSE(slotTempTenths(c, nullptr, 3, 3, 1000, 60000, tenths));
  CHECK(tenths == 210);

  c.volts[1].id = id(9);
  c.volts[1].offset = 1.0f;
  c.volts[1].factor = 2.0f;
  VoltReading v[2];
  v[0].id = id(9);
  v[0].seen = true;
  v[0].vad = 1234;
  v[0].lastSeenMs = 0;
  double value = 0;
  CHECK(slotVoltValue(c, v, 2, 1, 60000, 60000, value));
  CHECK(value == doctest::Approx(26.68));
  CHECK_FALSE(slotVoltValue(c, v, 2, 1, 60001, 60000, value));
  CHECK_FALSE(slotVoltValue(c, v, 0, 1, 0, 60000, value));
  CHECK_FALSE(slotVoltValue(c, v, 2, 0, 0, 60000, value));
  CHECK_FALSE(slotVoltValue(c, v, 2, kVoltSlotCount, 0, 60000, value));
  v[0].vad = kVadFailed;
  CHECK_FALSE(slotVoltValue(c, v, 2, 1, 0, 60000, value));
  v[0].vad = 1234;
  v[0].seen = false;
  CHECK_FALSE(slotVoltValue(c, v, 2, 1, 0, 60000, value));
}

TEST_CASE("published temps and volts") {
  static Config c;
  c = Config{};
  ValveState valves[kValveCount];
  c.temps[4].active = true;
  CHECK_FALSE(tempPublished(c, valves, 4));  // no id
  c.temps[4].id = id(5);
  CHECK(tempPublished(c, valves, 4));
  valves[7].sensorSlot[1] = 5;  // slot 5 (1-based) = index 4
  CHECK(tempAssignedToValve(valves, 5));
  CHECK_FALSE(tempAssignedToValve(valves, 4));
  CHECK_FALSE(tempAssignedToValve(valves, 0));
  CHECK_FALSE(tempAssignedToValve(nullptr, 5));
  CHECK(tempPublished(c, valves, 4));  // allTemps
  c.mqtt.allTemps = false;
  CHECK_FALSE(tempPublished(c, valves, 4));
  valves[7].sensorSlot[1] = 0;
  valves[0].sensorSlot[0] = 5;
  CHECK_FALSE(tempPublished(c, valves, 4));
  CHECK(tempPublished(c, nullptr, 4));
  c.temps[4].active = false;
  CHECK_FALSE(tempPublished(c, nullptr, 4));
  CHECK_FALSE(tempPublished(c, nullptr, kTempSlotCount));
  // E23: every configured volt slot is published; discovery only the active ones.
  c.volts[3].id = id(1);
  CHECK(voltPublishedMqtt(c, 3));
  CHECK_FALSE(voltAnnounced(c, 3));
  c.volts[3].active = true;
  CHECK(voltAnnounced(c, 3));
  c.volts[3].id = OneWireId{};
  CHECK_FALSE(voltPublishedMqtt(c, 3));
  CHECK_FALSE(voltAnnounced(c, 3));
  CHECK_FALSE(voltPublishedMqtt(c, kVoltSlotCount));
  CHECK_FALSE(voltAnnounced(c, kVoltSlotCount));
}

TEST_CASE("bus index and sensor topic segments (E22)") {
  TempReading t[4];
  t[2].id = id(7);
  t[3].id = id(8);
  CHECK(findTempBus(t, 4, id(7)) == 2);
  CHECK(findTempBus(t, 4, id(8)) == 3);
  CHECK(findTempBus(t, 3, id(8)) == -1);
  CHECK(findTempBus(t, 4, OneWireId{}) == -1);
  CHECK(findTempBus(nullptr, 4, id(7)) == -1);
  CHECK(findTempBus(t, 255, id(7)) == 2);
  VoltReading v[2];
  v[1].id = id(9);
  CHECK(findVoltBus(v, 2, id(9)) == 1);
  CHECK(findVoltBus(v, 1, id(9)) == -1);
  CHECK(findVoltBus(v, 2, OneWireId{}) == -1);
  CHECK(findVoltBus(nullptr, 2, id(9)) == -1);
  static VoltReading many[kVoltSlotCount + 1];
  many[kVoltSlotCount].id = id(9);  // beyond the bus maximum
  CHECK(findVoltBus(many, 255, id(9)) == -1);

  static Config c;
  c = Config{};
  CHECK(seg(c, ItemKind::Temp, 0, 2) == "3");
  CHECK(seg(c, ItemKind::Temp, 0, 0) == "1");
  CHECK(seg(c, ItemKind::Temp, 0, -1) == "");
  copyString(c.temps[0].name, sizeof c.temps[0].name, "Wohn zi");
  CHECK(seg(c, ItemKind::Temp, 0, 2) == "Wohn_zi");
  CHECK(seg(c, ItemKind::Temp, 0, -1) == "Wohn_zi");
  copyString(c.temps[1].topic, sizeof c.temps[1].topic, "Bad/WC");
  CHECK(seg(c, ItemKind::Temp, 1, -1) == "Bad/WC");
  CHECK(seg(c, ItemKind::Volt, 7, 33) == "34");
  CHECK(seg(c, ItemKind::Volt, 7, 33, 2) == "");  // does not fit
  CHECK(seg(c, ItemKind::Volt, 7, 33, 3) == "34");
  copyString(c.volts[7].name, sizeof c.volts[7].name, "Batt");
  CHECK(seg(c, ItemKind::Volt, 7, -1) == "Batt");
  CHECK(seg(c, ItemKind::Volt, kVoltSlotCount, 1) == "");
  CHECK(seg(c, ItemKind::Temp, kTempSlotCount, 1) == "");
  CHECK(seg(c, ItemKind::Valve, 0, 1) == "");
  CHECK(sensorTopicSegment(c, ItemKind::Temp, 5, 1, nullptr, 4) == 0);
  char one[1] = {'X'};
  CHECK(sensorTopicSegment(c, ItemKind::Temp, 5, 1, one, 0) == 0);
  CHECK(one[0] == 'X');
}

TEST_CASE("published target (W5)") {
  ValveState v;
  uint8_t out = 200;
  // Not separate: the desired target.
  CHECK_FALSE(publishedTarget(v, false, out));
  v.desiredValid = true;
  v.desired = 60;
  v.stmTargetKnown = true;
  v.stmTarget = 30;
  CHECK(publishedTarget(v, false, out));
  CHECK(out == 60);
  // Separate: the read-back.
  CHECK(publishedTarget(v, true, out));
  CHECK(out == 30);
  v.stmTargetKnown = false;
  out = 200;
  CHECK_FALSE(publishedTarget(v, true, out));
  CHECK(out == 200);
  // ESP emulation: the desired target (the STM holds the failsafe position).
  v.fsOverride = true;
  CHECK(publishedTarget(v, true, out));
  CHECK(out == 60);
  v.desiredValid = false;
  CHECK_FALSE(publishedTarget(v, true, out));
  // A restored target is held until it was synced.
  v = ValveState{};
  v.desiredValid = true;
  v.desired = 50;
  v.source = TargetSource::Restored;
  v.sync = TargetSync::Pending;
  v.stmTargetKnown = true;
  v.stmTarget = 50;
  out = 200;
  CHECK_FALSE(publishedTarget(v, true, out));
  CHECK(out == 200);
  CHECK(publishedTarget(v, false, out));  // not separate: unchanged rule
  CHECK(out == 50);
  v.sync = TargetSync::Synced;
  out = 200;
  CHECK(publishedTarget(v, true, out));
  CHECK(out == 50);
  v.source = TargetSource::Web;
  v.sync = TargetSync::Pending;
  CHECK(publishedTarget(v, true, out));
}

TEST_CASE("valve problem and STM online") {
  ValveState v;
  CHECK_FALSE(valveProblem(v));
  const uint16_t problems[] = {kHealthBlocked, kHealthFailed, kHealthNoValve, kHealthStale,
                               kHealthTargetUnconfirmed, kHealthTempFailed};
  for (uint16_t f : problems) {
    v.health = f;
    CHECK(valveProblem(v));
  }
  for (uint16_t f : {kHealthCalibRetries, kHealthEarlyStop, kHealthCmdRejected, kHealthFailsafe,
                     kHealthStrokeShort}) {
    v.health = f;
    CHECK_FALSE(valveProblem(v));
  }
  CHECK(kProblemMask == 0x01C7);
  CHECK(stmOnline(LinkState::Up));
  CHECK(stmOnline(LinkState::Degraded));
  for (LinkState s : {LinkState::Unknown, LinkState::Down, LinkState::Booting, LinkState::Suspended}) {
    CHECK_FALSE(stmOnline(s));
  }
}

TEST_CASE("valve display names") {
  char out[16];
  CHECK(valveDisplayName("Bad 1", 0, out, sizeof out) == 5);
  CHECK(std::string(out) == "Bad 1");
  CHECK(valveDisplayName("", 0, out, sizeof out) == 7);
  CHECK(std::string(out) == "Valve 1");
  CHECK(valveDisplayName(nullptr, 11, out, sizeof out) == 8);
  CHECK(std::string(out) == "Valve 12");
  CHECK(valveDisplayName("\xc5\x81" "azienka", 3, out, sizeof out) == 9);
  char eight[8];
  memset(eight, 'X', sizeof eight);
  CHECK(valveDisplayName("", 11, eight, sizeof eight) == 0);
  CHECK(eight[0] == '\0');
  char nine[9];
  CHECK(valveDisplayName("", 11, nine, sizeof nine) == 8);
  CHECK(valveDisplayName("abcdefgh", 0, nine, sizeof nine) == 8);
  CHECK(valveDisplayName("abcdefghi", 0, nine, sizeof nine) == 0);
  CHECK(valveDisplayName("x", 0, nullptr, 4) == 0);
  CHECK(valveDisplayName("x", 0, out, 0) == 0);
}

TEST_CASE("calibration end tracker") {
  CalibEndTracker t;
  ValveState v[kValveCount];
  LocalTime a;
  a.valid = true;
  a.year = 2026;
  a.epoch = 100;
  LocalTime b = a;
  b.epoch = 200;
  v[3].calibrating = true;
  CHECK(t.observe(v, a) == 0);  // the first observation only learns the state
  CHECK_FALSE(t.ended(3));
  v[3].calibrating = false;
  v[5].calibrating = true;
  CHECK(t.observe(v, a) == 0x0008);
  CHECK(t.ended(3));
  CHECK(t.dirty(3));
  CHECK(t.end(3).epoch == 100);
  CHECK_FALSE(t.ended(5));
  t.clearDirty(3);
  CHECK_FALSE(t.dirty(3));
  CHECK(t.ended(3));
  v[5].calibrating = false;
  CHECK(t.observe(v, b) == 0x0020);
  CHECK(t.end(5).epoch == 200);
  CHECK(t.end(3).epoch == 100);
  CHECK(t.dirty(5));
  CHECK(t.observe(v, b) == 0);
  CHECK(t.observe(nullptr, b) == 0);
  CHECK_FALSE(t.ended(12));
  CHECK_FALSE(t.dirty(12));
  t.clearDirty(12);
  CHECK(t.dirty(5));
  CHECK(t.end(12).epoch == t.end(0).epoch);  // out of range reads entry 0
  // A calibration that starts at the first observation and ends later.
  CalibEndTracker u;
  v[0].calibrating = true;
  CHECK(u.observe(v, a) == 0);
  v[0].calibrating = false;
  v[11].calibrating = true;
  CHECK(u.observe(v, b) == 0x0001);
  v[11].calibrating = false;
  CHECK(u.observe(v, b) == 0x0800);
  CHECK(u.ended(11));
}
