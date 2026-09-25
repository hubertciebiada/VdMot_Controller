// StmSession details: sensor slot edges and counts, the config, the queue
// limits, command arguments and events, UART bytes, service moves, flash and
// reset edge cases.
#include <stdint.h>
#include <string.h>

#include <map>
#include <string>
#include <vector>

#include "doctest.h"
#include "support/session_rig.h"

namespace {

const char kIdA[] = "28-84-37-94-97-ff-03-23";
const char kIdB[] = "28-aa-bb-cc-dd-ee-01-67";
const char kIdC[] = "28-11-22-33-44-55-66-77";
const char kIdV[] = "26-01-02-03-04-05-06-07";
const char kIdW[] = "26-0a-0b-0c-0d-0e-0f-10";

OneWireId oid(const char* s) {
  OneWireId v;
  REQUIRE(parseOneWireId(s, strlen(s), v));
  return v;
}

// A sensor bus the STM reports: ids and readings per bus index.
struct Bus {
  std::vector<std::string> temps;
  std::map<std::string, int> raw;  // id -> raw, absent: "goned 0"
  std::vector<std::string> volts;
  std::map<std::string, int> vad;

  static std::string list(const char* cmd, const std::vector<std::string>& ids, const std::string& l) {
    std::string out = std::string(cmd) + " " + std::to_string(ids.size()) + " ";
    if (l == cmd) return out;
    for (size_t i = 0; i < ids.size(); ++i) out += (i ? "," : "") + ids[i];
    return out + " ";
  }
  static size_t index(const std::string& l) { return std::stoul(l.substr(6)); }

  void attach(Rig& r) {
    r.stm.answers["gonec"] = [this](const std::string& l) { return list("gonec", temps, l); };
    r.stm.answers["gowvc"] = [this](const std::string& l) { return list("gowvc", volts, l); };
    r.stm.answers["goned"] = [this](const std::string& l) {
      const size_t i = index(l);
      if (i >= temps.size() || raw.count(temps[i]) == 0) return std::string("goned 0");
      return "goned " + temps[i] + " " + std::to_string(raw[temps[i]]) + " ";
    };
    r.stm.answers["gowvd"] = [this](const std::string& l) {
      const size_t i = index(l);
      if (i >= volts.size() || vad.count(volts[i]) == 0) return std::string("gowvd 0");
      return "gowvd " + volts[i] + " " + std::to_string(vad[volts[i]]) + " ";
    };
  }
};

std::vector<Event> sensorEvents(const TestPort& p) {
  std::vector<Event> out;
  for (const Event& e : p.events) {
    if (e.code == EventCode::TempSensorFailed || e.code == EventCode::TempSensorRecovered ||
        e.code == EventCode::VoltSensorFailed || e.code == EventCode::SensorCountChanged) {
      out.push_back(e);
    }
  }
  return out;
}

// Temps A (slot 1), B (slot 2) and C (slot 3, inactive); volts V (slot 1), W (slot 2).
struct SensorRig {
  Rig r{3, 0x001};
  Bus bus;
  SensorRig() {
    r.cfg.temps[0].active = true;
    r.cfg.temps[0].id = oid(kIdA);
    r.cfg.temps[1].active = true;
    r.cfg.temps[1].id = oid(kIdB);
    r.cfg.temps[2].active = false;
    r.cfg.temps[2].id = oid(kIdC);
    r.cfg.temps[3].active = true;  // active without an id
    r.cfg.volts[0].active = true;
    r.cfg.volts[0].id = oid(kIdV);
    r.cfg.volts[1].active = true;
    r.cfg.volts[1].id = oid(kIdW);
    bus.temps = {kIdA, kIdB, kIdC};
    bus.raw = {{kIdA, 215}, {kIdB, 198}, {kIdC, 170}};
    bus.volts = {kIdV, kIdW};
    bus.vad = {{kIdV, 1200}, {kIdW, 1300}};
    bus.attach(r);
    r.start();
  }
};

StmCommand serviceMove(uint8_t v, uint16_t counts) {
  StmCommand c = cmd(StmCommandType::ServiceMove, v);
  c.dir = MoveDir::Open;
  c.counts = counts;
  c.maxmA = 40;
  return c;
}

}  // namespace

// ================================================================ sensors

TEST_CASE("session mut: sensor slots: settled after the grace, readings in the snapshot") {
  SensorRig t;
  Rig& r = t.r;
  r.run(20000);
  CHECK_FALSE(r.port.last.sensorsSettled);
  r.run(20000);
  CHECK(r.port.last.sensorsSettled);
  CHECK(r.port.last.tempCount == 3);
  CHECK(r.port.last.temps[0].raw == 215);
  CHECK(r.port.last.temps[1].raw == 198);
  CHECK(r.port.last.temps[2].raw == 170);
  CHECK(r.port.last.voltCount == 2);
  CHECK(r.port.last.volts[0].vad == 1200);
  CHECK(r.port.last.volts[1].vad == 1300);
  CHECK(sensorEvents(r.port).empty());
  // A bus scan starts the grace again.
  r.command(cmd(StmCommandType::ScanSensors));
  r.run(1500);
  CHECK_FALSE(r.port.last.sensorsSettled);
  r.run(40000);
  CHECK(r.port.last.sensorsSettled);
}

TEST_CASE("session mut: temp sensor failure and recovery per slot, the id in the text") {
  SensorRig t;
  Rig& r = t.r;
  r.run(45000);
  REQUIRE(sensorEvents(r.port).empty());
  t.bus.raw[kIdA] = kTempReadError;
  t.bus.raw[kIdC] = kTempReadError;  // inactive slot: silent
  r.run(40000);
  std::vector<Event> ev = sensorEvents(r.port);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].code == EventCode::TempSensorFailed);
  CHECK(ev[0].arg1 == 1);
  CHECK(ev[0].arg2 == kTempReadError);
  CHECK(std::string(ev[0].text) == kIdA);
  t.bus.raw[kIdA] = 216;
  r.run(30000);
  ev = sensorEvents(r.port);
  REQUIRE(ev.size() == 2);
  CHECK(ev[1].code == EventCode::TempSensorRecovered);
  CHECK(ev[1].arg1 == 1);
  CHECK(std::string(ev[1].text).empty());
  // Slot 2 (bus index 1) fails.
  t.bus.raw[kIdB] = kTempPowerOn;
  r.run(40000);
  ev = sensorEvents(r.port);
  REQUIRE(ev.size() == 3);
  CHECK(ev[2].code == EventCode::TempSensorFailed);
  CHECK(ev[2].arg1 == 2);
  CHECK(ev[2].arg2 == kTempPowerOn);
  CHECK(std::string(ev[2].text) == kIdB);
}

TEST_CASE("session mut: a temp sensor that stops answering fails after 60 s") {
  SensorRig t;
  Rig& r = t.r;
  r.run(45000);
  t.r.stm.answers["goned"] = [](const std::string&) { return std::string(); };
  r.run(55000);
  r.run(30000);
  const std::vector<Event> ev = sensorEvents(r.port);
  REQUIRE(ev.size() == 2);
  CHECK(ev[0].code == EventCode::TempSensorFailed);
  CHECK(ev[1].code == EventCode::TempSensorFailed);
  CHECK(ev[0].arg1 + ev[1].arg1 == 3);
}

TEST_CASE("session mut: volt sensor failures per slot") {
  SensorRig t;
  Rig& r = t.r;
  r.run(45000);
  t.bus.vad[kIdV] = kVadFailed;
  r.run(40000);
  std::vector<Event> ev = sensorEvents(r.port);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].code == EventCode::VoltSensorFailed);
  CHECK(ev[0].arg1 == 1);
  CHECK(ev[0].arg2 == kVadFailed);
  t.bus.vad[kIdV] = 1100;
  r.run(30000);
  CHECK(sensorEvents(r.port).size() == 1);  // no recovery event for volts
  t.bus.vad[kIdW] = kVadFailed - 5;
  r.run(40000);
  ev = sensorEvents(r.port);
  REQUIRE(ev.size() == 2);
  CHECK(ev[1].arg1 == 2);
  CHECK(ev[1].arg2 == kVadFailed - 5);
}

TEST_CASE("session mut: a volt sensor that stops answering fails after 60 s") {
  SensorRig t;
  Rig& r = t.r;
  r.run(45000);
  t.r.stm.answers["gowvd"] = [](const std::string&) { return std::string(); };
  r.run(55000);
  r.run(30000);
  const std::vector<Event> ev = sensorEvents(r.port);
  REQUIRE(ev.size() == 2);
  CHECK(ev[0].code == EventCode::VoltSensorFailed);
  CHECK(ev[1].code == EventCode::VoltSensorFailed);
}

TEST_CASE("session mut: sensor count changes are logged per bus once known") {
  SensorRig t;
  Rig& r = t.r;
  r.run(45000);
  REQUIRE(sensorEvents(r.port).empty());
  t.bus.temps = {kIdA, kIdB};
  r.run(35000);
  std::vector<Event> ev;
  for (const Event& e : r.port.events) {
    if (e.code == EventCode::SensorCountChanged) ev.push_back(e);
  }
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 2);
  CHECK(ev[0].arg2 == 0);
  t.bus.volts = {kIdV};
  r.run(35000);
  ev.clear();
  for (const Event& e : r.port.events) {
    if (e.code == EventCode::SensorCountChanged) ev.push_back(e);
  }
  REQUIRE(ev.size() == 2);
  CHECK(ev[1].arg1 == 1);
  CHECK(ev[1].arg2 == 1);
}

TEST_CASE("session mut: changed slot ids re-read the valve sensors, unchanged ones do not") {
  SensorRig t;
  Rig& r = t.r;
  r.run(45000);
  const size_t n = r.count("gvlon");
  r.s.applyConfig(r.cfg, true);
  r.run(3000);
  CHECK(r.count("gvlon") == n);
  r.cfg.temps[1].id = oid(kIdC);
  r.s.applyConfig(r.cfg, true);
  r.run(3000);
  CHECK(r.count("gvlon") == n + 1);
}

// ================================================================ config, targets, publishing

TEST_CASE("session mut: only configured valves are active") {
  Rig r(3, 0x002);
  r.start();
  r.run(40000);
  CHECK(r.count("gvlvy 1") >= 3);
  CHECK(r.count("gvlvy 0") < r.count("gvlvy 1") / 2);  // inactive: every 30 s
  CHECK(r.port.last.valves[1].known);
  r.command(r.target(0, 20));
  r.command(r.target(1, 20));
  r.run(3000);
  CHECK(r.stm.linesOf("stgtp") == std::vector<std::string>{"stgtp 1 20"});
}

TEST_CASE("session mut: config, a second begin and rejected targets publish without a model change") {
  Rig r(3, 0x001);
  r.s.applyConfig(r.cfg, true);
  r.s.begin(r.now, PersistedTargets{}, RestoreSource::None, nullptr);
  r.s.publishIfDue(r.now);
  int p = r.port.publishes;
  r.now += 200;
  r.s.publishIfDue(r.now);
  CHECK(r.port.publishes == p);
  r.s.applyConfig(r.cfg, true);
  r.now += 200;
  r.s.publishIfDue(r.now);
  CHECK(r.port.publishes == ++p);
  r.s.begin(r.now, PersistedTargets{}, RestoreSource::None, nullptr);
  r.now += 200;
  r.s.publishIfDue(r.now);
  CHECK(r.port.publishes == ++p);
  r.command(r.target(1, 30));  // inactive valve
  r.now += 200;
  r.s.publishIfDue(r.now);
  CHECK(r.port.publishes == ++p);
  r.command(r.target(kValveCount, 30));
  r.command(r.target(kValveCount - 1, 30));
  r.command(r.target(0, 101));
  const std::vector<Event> ev = r.port.withCode(EventCode::MqttCommandRejected);
  REQUIRE(ev.size() == 4);
  CHECK(ev[0].arg1 == 2);
  CHECK(ev[1].arg1 == 0);
  CHECK(ev[2].arg1 == 12);
  CHECK(ev[3].arg1 == 1);
  for (const Event& e : ev) {
    CHECK(e.arg2 == 0);
    CHECK(e.valve == kNoValve);
    CHECK(std::string(e.text) == "rejected by model");
  }
}

// ================================================================ queue limits

namespace {

// Fills the User part of the queue with distinct service moves of valve 0.
void fillQueue(Rig& r) {
  uint16_t counts = 100;
  while (r.s.link().queued(Priority::User) + r.s.link().queued(Priority::Config) <
         LinkPolicy::kQueueCapacity) {
    r.command(serviceMove(0, counts++));
    REQUIRE(counts < 200);
  }
}

}  // namespace

TEST_CASE("session mut: a full queue refuses commands and logs the command") {
  Rig r(3, 0x0FF);
  r.start();
  r.run(10000);
  r.stm.silent = true;
  fillQueue(r);
  REQUIRE_FALSE(r.port.has(EventCode::StmQueueFull));
  StmCommand c = cmd(StmCommandType::Calibrate, 3);
  c.scheduled = true;
  c.attempt = 5;
  r.command(c);
  REQUIRE(r.port.calibs.size() == 1);
  CHECK(r.port.calibs[0].attempt == 5);
  CHECK_FALSE(r.port.calibs[0].ok);
  CHECK(r.port.calibs[0].reason == CalibFailure::NotSent);
  std::vector<Event> ev = r.port.withCode(EventCode::StmQueueFull);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == static_cast<int32_t>(Cmd::Staln));
  r.command(cmd(StmCommandType::Assembly, 4));
  r.s.publishIfDue(r.now + 200);
  CHECK(r.port.last.valves[4].source != TargetSource::Assembly);
  ev = r.port.withCode(EventCode::StmQueueFull);
  REQUIRE(ev.size() == 2);
  CHECK(ev[1].arg1 == static_cast<int32_t>(Cmd::Staop));
  // Motor settings: all or nothing.
  StmCommand m = cmd(StmCommandType::SetMotorSettings);
  m.hasMotor = true;
  r.command(m);
  ev = r.port.withCode(EventCode::StmQueueFull);
  REQUIRE(ev.size() == 3);
  CHECK(ev[2].arg1 == static_cast<int32_t>(Cmd::Smotc));
}

TEST_CASE("session mut: a target push that finds the queue full is retried later") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.stm.silent = true;
  fillQueue(r);
  r.command(r.target(0, 30));
  r.run(100);
  CHECK(r.count("stgtp") == 0);
  r.stm.silent = false;
  r.run(15000);
  CHECK(r.stm.target[0] == 30);
}

TEST_CASE("session mut: motor settings need room for every line") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.stm.silent = true;
  r.command(serviceMove(0, 99));  // outstanding
  r.run(4);
  r.command(r.target(0, 30));     // a queued Config line (stgtp)
  r.run(4);
  REQUIRE(r.s.link().queued(Priority::Config) >= 1);
  uint16_t counts = 100;
  while (r.s.link().queued(Priority::User) + r.s.link().queued(Priority::Config) <
         LinkPolicy::kQueueCapacity - 2) {
    r.command(serviceMove(0, counts++));
  }
  StmCommand m = cmd(StmCommandType::SetMotorSettings);
  m.hasMotor = true;
  m.hasLearnMovements = true;
  m.learnMovements = 100;
  m.hasBreakaway = true;
  r.command(m);  // three lines, room for two
  CHECK(r.port.withCode(EventCode::StmQueueFull).size() == 1);
  m.hasBreakaway = false;
  r.command(m);  // two lines fit
  CHECK(r.port.withCode(EventCode::StmQueueFull).size() == 1);
  CHECK(r.s.link().queued(Priority::User) + r.s.link().queued(Priority::Config) ==
        LinkPolicy::kQueueCapacity);
}

TEST_CASE("session mut: motor settings: each part alone, breakaway only on protocol 2+") {
  for (int part = 0; part < 3; ++part) {
    CAPTURE(part);
    Rig r(2, 0x001);
    r.start();
    r.run(10000);
    const size_t gmotc = r.count("gmotc");
    StmCommand m = cmd(StmCommandType::SetMotorSettings);
    m.hasMotor = part == 0;
    m.hasLearnMovements = part == 1;
    m.learnMovements = 120;
    m.hasBreakaway = part == 2;
    r.command(m);
    r.run(2000);
    CHECK((r.count("smotc") > 0) == (part == 0));
    CHECK((r.count("stlnm") > 0) == (part == 1));
    CHECK((r.count("scalx") > 0) == (part == 2));
    CHECK(r.count("gmotc") > gmotc);  // read back
  }
  Rig old(1, 0x001);
  old.start();
  old.run(15000);
  const size_t gmotc = old.count("gmotc");
  StmCommand m = cmd(StmCommandType::SetMotorSettings);
  m.hasBreakaway = true;
  old.command(m);
  old.run(2000);
  CHECK(old.count("scalx") == 0);
  CHECK(old.count("gmotc") == gmotc);  // nothing sent: nothing to read back
}

// ================================================================ commands

TEST_CASE("session mut: assembly, detect, valve sensors and profiles reach the STM") {
  Rig r(3, 0x007);
  r.start();
  r.run(10000);
  r.command(cmd(StmCommandType::Detect));
  StmCommand vs = cmd(StmCommandType::SetValveSensors, 1);
  vs.ids[0] = oid(kIdA);
  vs.ids[1] = oid(kIdB);
  r.command(vs);
  r.command(cmd(StmCommandType::RequestProfile, 2));
  r.run(15000);
  CHECK(r.count("stdet") == 1);
  CHECK(r.stm.linesOf("stvls").size() >= 1);
  CHECK(r.count("masns") == 1);
  CHECK(r.count("gprof 2") >= 1);
}

TEST_CASE("session mut: service moves on protocol 2+: done event with the counted move") {
  Rig r(3, 0x003);
  r.start();
  r.run(10000);
  r.command(serviceMove(1, 300));
  r.run(3000);
  REQUIRE(r.stm.linesOf("svmov").size() == 1);
  CHECK(r.stm.linesOf("svmov")[0].compare(0, 8, "svmov 1 ") == 0);
  // The STM reports a new move sequence for valve 1 (counts 412, stop 3).
  Rig old(1, 0x003);
  old.start();
  old.run(15000);
  old.command(serviceMove(1, 300));
  old.run(2000);
  CHECK(old.count("svmov") == 0);
}

TEST_CASE("session mut: rejected and unanswered service moves are logged") {
  Rig r(3, 0x003);
  r.start();
  r.run(10000);
  r.stm.answers["svmov"] = [](const std::string& l) {
    return l.compare(0, 8, "svmov 1 ") == 0 ? std::string("svmov 1 err 3") : std::string();
  };
  r.command(serviceMove(1, 300));
  r.run(2000);
  std::vector<Event> ev = r.port.withCode(EventCode::ServiceMoveDone);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].valve == 1);
  CHECK(ev[0].severity == Severity::Warning);
  CHECK(ev[0].arg1 == -1);
  CHECK(ev[0].arg2 == 3);
  CHECK(std::string(ev[0].text) == "rejected");
  r.command(serviceMove(0, 300));
  r.run(3000);
  ev = r.port.withCode(EventCode::ServiceMoveDone);
  REQUIRE(ev.size() == 2);
  CHECK(ev[1].valve == 0);
  CHECK(ev[1].arg1 == -1);
  CHECK(ev[1].arg2 == -1);
  CHECK(std::string(ev[1].text) == "no reply");
}

TEST_CASE("session mut: a rejected target of an unsupported STM still reads the targets back") {
  Rig r(1, 0x006);
  r.stm.tooOld = true;
  r.start();
  r.run(10000);
  CHECK(r.count("gtgtp 1") >= 1);
  CHECK(r.count("gtgtp 2") >= 1);
  CHECK(r.count("gtgtp 0") == 0);
  r.command(cmd(StmCommandType::ScanSensors));
  r.command(cmd(StmCommandType::StopValve, 1));
  r.run(1000);
  CHECK(r.count("stons") == 0);
}

// ================================================================ UART bytes

TEST_CASE("session mut: replies as UART bytes: CR LF lines, several per read") {
  Rig r(3, 0x001);
  r.viaRx = true;
  r.start();
  r.run(10000);
  CHECK(r.port.last.link == LinkState::Up);
  CHECK(r.port.last.proto == 3);
  CHECK(r.port.last.valves[0].known);
  CHECK(r.s.link().stats().parseErrors == 0);
  // Two stray replies in one read, the second split over two reads.
  const std::string two = "gtgtp 0 61 \r\ngtgtp 0 62 \r\ngtgt";
  r.s.onRx(two.data(), two.size(), r.now);
  r.s.onRx("p 0 63 \r\n", 9, r.now);
  r.s.publishIfDue(r.now + 200);
  CHECK(r.port.last.valves[0].stmTarget == 63);
  CHECK(r.s.link().stats().strayLines >= 3);
  r.s.onRx("junk\r\n", 6, r.now);
  CHECK(r.s.link().stats().parseErrors == 1);
}

// ================================================================ commands on older STMs

TEST_CASE("session mut: an unsupported STM: a manual calibration reports nothing") {
  Rig r(1, 0x001);
  r.stm.tooOld = true;
  r.start();
  r.run(10000);
  REQUIRE(r.port.last.support == StmSupport::TooOld);
  r.command(cmd(StmCommandType::Calibrate, 0));
  r.run(1000);
  CHECK(r.port.calibs.empty());
  CHECK(r.count("staln") == 0);
}

TEST_CASE("session mut: service moves need protocol 2, the valve sensors line carries both ids") {
  Rig r(2, 0x003);
  r.start();
  r.run(10000);
  r.command(serviceMove(1, 300));
  StmCommand vs = cmd(StmCommandType::SetValveSensors, 1);
  vs.ids[0] = oid(kIdA);
  vs.ids[1] = oid(kIdB);
  r.command(vs);
  r.run(2000);
  CHECK(r.count("svmov") >= 1);
  const std::vector<std::string> l = r.stm.linesOf("stvls");
  REQUIRE_FALSE(l.empty());
  CHECK(l[0] == std::string("stvls 1 ") + kIdA + " " + kIdB);
}

TEST_CASE("session mut: STM version exactly the minimum is compatible") {
  Rig r(3, 0x001);
  r.stm.version = "1.4.0_C2 1712345678 ";
  r.start();
  r.run(10000);
  CHECK(r.port.last.compatible);
  CHECK(r.port.last.support == StmSupport::Supported);
  CHECK_FALSE(r.port.has(EventCode::StmIncompatible));
  const std::vector<Event> v = r.port.withCode(EventCode::StmVersion);
  REQUIRE(v.size() == 1);
  CHECK(std::string(v[0].text) == "1.4.0_C2");
}

// ================================================================ resets and flashing

TEST_CASE("session mut: a user reset counts as a user reset, not by policy") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.command(cmd(StmCommandType::ResetStm));
  r.run(1000);
  REQUIRE(r.port.pulses.size() == 1);
  CHECK(r.s.link().stats().userResets == 1);
  CHECK(r.s.link().stats().policyResets == 0);
}

namespace {

void put32(std::vector<uint8_t>& v, size_t off, uint32_t x) {
  for (int i = 0; i < 4; ++i) v[off + i] = static_cast<uint8_t>(x >> (8 * i));
}

// An image the flasher accepts for board `hw` (0x80 body, vectors, handshake,
// version and board tag).
std::vector<uint8_t> image(const char* hw) {
  std::vector<uint8_t> v(8192, 0x80);
  put32(v, 0, 0x20020000u);
  put32(v, 4, 0x080001C5u);
  const std::string s =
      std::string("\x01" "DEADBEEF\0\x01" "BEEFIT\0\x01" "2.1.0-revamped\0\x01", 35) + "VDM-HW:" + hw;
  memcpy(v.data() + 6000, s.data(), s.size());
  v[6000 + s.size()] = 0;
  return v;
}

StmCommand flashCmd(const char* name, bool blank) {
  StmCommand c = cmd(StmCommandType::StartFlash);
  copyString(c.image, sizeof c.image, name);
  c.blank = blank;
  return c;
}

}  // namespace

TEST_CASE("session mut: a blank flash runs to the end: events, last good copy, re-sync") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.port.image.data = image("C2");
  r.sim.bootPinResets = 1;  // blank: the STM starts in the ROM bootloader
  r.command(flashCmd("new.bin", true));
  REQUIRE(r.s.flashing());
  std::vector<Event> ev = r.port.withCode(EventCode::StmFlashStarted);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 8192);
  CHECK(ev[0].arg2 == 0);
  CHECK(std::string(ev[0].text) == "new.bin");
  // Nothing else while flashing.
  r.command(flashCmd("other.bin", true));
  ev = r.port.withCode(EventCode::StmFlashFailed);
  REQUIRE(ev.size() == 1);
  CHECK(std::string(ev[0].text) == "busy");
  CHECK(ev[0].arg1 == 0);
  CHECK(ev[0].arg2 == 0);
  r.command(cmd(StmCommandType::ResetStm));
  ev = r.port.withCode(EventCode::StmFlashFailed);
  REQUIRE(ev.size() == 2);
  CHECK(std::string(ev[1].text) == "reset refused");
  CHECK(ev[1].arg1 == 0);
  CHECK(ev[1].arg2 == 0);
  StmCommand c = cmd(StmCommandType::Calibrate, 0);
  c.scheduled = true;
  c.attempt = 3;
  r.command(c);
  REQUIRE(r.port.calibs.size() == 1);
  CHECK(r.port.calibs[0].attempt == 3);
  CHECK_FALSE(r.port.calibs[0].ok);
  CHECK(r.port.calibs[0].reason == CalibFailure::NotSent);
  CHECK(r.runUntil([&] { return !r.s.flashing(); }, 120000));
  ev = r.port.withCode(EventCode::StmFlashDone);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 > 0);
  CHECK(ev[0].arg2 == 0);
  CHECK(r.port.lastGood == std::vector<std::string>{"new.bin"});
  CHECK(r.port.closed == 1);
  CHECK(r.sim.configs.back() == std::make_pair(uint32_t{115200}, false));
  r.run(10000);
  CHECK(r.port.last.link == LinkState::Up);
  CHECK(r.port.last.flash.phase == FlashPhase::Done);
}

TEST_CASE("session mut: flash refusals carry no arguments; a pending flash is published") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.stm.eep = 0;
  r.port.restart = true;
  r.command(flashCmd("a.bin", false));
  std::vector<Event> ev = r.port.withCode(EventCode::StmFlashFailed);
  REQUIRE(ev.size() == 1);
  CHECK(std::string(ev[0].text) == "restart pending");
  CHECK(ev[0].arg1 == 0);
  CHECK(ev[0].arg2 == 0);
  r.port.restart = false;
  r.s.publishIfDue(r.now + 200);
  r.command(flashCmd("a.bin", false));
  r.s.publishIfDue(r.now + 400);
  CHECK(r.port.last.flashPending);
  r.command(cmd(StmCommandType::AbortFlash));
  r.s.publishIfDue(r.now + 600);
  CHECK_FALSE(r.port.last.flashPending);
  // An unreadable image: the name in the text, no address.
  r.port.imageOk = false;
  r.command(flashCmd("gone.bin", true));
  ev = r.port.withCode(EventCode::StmFlashFailed);
  REQUIRE(ev.size() == 2);
  CHECK(ev[1].arg1 == static_cast<int32_t>(FlashError::ImageRead));
  CHECK(ev[1].arg2 == 0);
  CHECK(std::string(ev[1].text) == "gone.bin");
  CHECK_FALSE(r.s.flashing());
  r.run(3000);
  CHECK(r.port.last.link == LinkState::Up);
}

TEST_CASE("session mut: without a running STM the chosen board is used") {
  Rig r(3, 0x001);
  r.stm.silent = true;
  r.start();
  r.run(1000);
  r.port.image.data = image("C1");
  r.sim.bootPinResets = 1;
  StmCommand c = flashCmd("c1.bin", true);
  copyString(c.board, sizeof c.board, "C1");
  r.command(c);
  REQUIRE(r.s.flashing());
  r.runUntil([&] { return !r.s.flashing(); }, 120000);
  CHECK(r.port.withCode(EventCode::StmFlashDone).size() == 1);
  // An invalid board choice leaves the board open: the tagged image is refused.
  Rig q(3, 0x001);
  q.stm.silent = true;
  q.start();
  q.run(1000);
  q.port.image.data = image("C1");
  q.sim.bootPinResets = 1;
  c = flashCmd("c1.bin", true);
  copyString(c.board, sizeof c.board, "X1");
  q.command(c);
  q.runUntil([&] { return !q.s.flashing(); }, 120000);
  CHECK(q.port.withCode(EventCode::StmFlashDone).empty());
}

TEST_CASE("session mut: flash progress is published while flashing") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.port.image.data = image("C2");
  r.sim.bootPinResets = 1;
  r.command(flashCmd("new.bin", true));
  std::vector<FlashPhase> seen;
  r.runUntil([&] {
    if (seen.empty() || seen.back() != r.port.last.flash.phase) seen.push_back(r.port.last.flash.phase);
    return !r.s.flashing();
  }, 120000);
  CHECK(seen.size() >= 4);
}

// ================================================================ replies

namespace {

// gvlvy reply of the LineStm layout with a chosen status and last move.
std::string gvlvy(int v, int status, int target, int counted, int stop) {
  const std::string t = std::to_string(target);
  return "gvlvy " + std::to_string(v) + " " + std::to_string(status) + " 40 " + t +
         " 21 3120 3350 -230 0 57 0 0 0 1 3000 " + std::to_string(counted) + " " +
         std::to_string(stop) + " 412 8123 0 0 50 " + t + " 0 0";
}

}  // namespace

TEST_CASE("session mut: stray list and reading replies") {
  SensorRig t;
  Rig& r = t.r;
  r.run(45000);
  REQUIRE(r.port.last.tempCount == 3);
  r.s.onLine("gonec 1 ", 8, r.now);
  const size_t vlists0 = r.count("gowvc");
  r.s.onLine("gowvc 0 ", 8, r.now);
  r.s.publishIfDue(r.now + 200);
  CHECK(r.port.last.tempCount == 3);
  CHECK(r.port.last.voltCount == 2);
  r.run(1000);
  CHECK(r.count("gowvc") == vlists0);  // a stray count asks for nothing
  // A stray reading of a known id lands on its index; an unknown id asks for the list.
  const size_t lists = r.count("gonec");
  const std::string known = std::string("goned ") + kIdB + " 205 ";
  r.s.onLine(known.c_str(), known.size(), r.now);
  r.s.publishIfDue(r.now + 400);
  CHECK(r.port.last.temps[1].raw == 205);
  r.run(1000);
  CHECK(r.count("gonec") == lists);
  const std::string unknown = std::string("goned 28-01-01-01-01-01-01-01 205 ");
  r.s.onLine(unknown.c_str(), unknown.size(), r.now);
  r.run(1000);
  CHECK(r.count("gonec") == lists + 1);
  r.s.onLine("goned 0", 7, r.now);
  r.run(1000);
  CHECK(r.count("gonec") == lists + 1);
  const size_t vlists = r.count("gowvc");
  const std::string vknown = std::string("gowvd ") + kIdW + " 1250 ";
  r.s.onLine(vknown.c_str(), vknown.size(), r.now);
  r.s.publishIfDue(r.now + 200);
  CHECK(r.port.last.volts[1].vad == 1250);
  r.run(1000);
  CHECK(r.count("gowvc") == vlists);
  const std::string vunknown = std::string("gowvd 26-01-01-01-01-01-01-01 1250 ");
  r.s.onLine(vunknown.c_str(), vunknown.size(), r.now);
  r.run(1000);
  CHECK(r.count("gowvc") == vlists + 1);
  r.s.onLine("gowvd 0", 7, r.now);
  r.run(1000);
  CHECK(r.count("gowvc") == vlists + 1);
}

TEST_CASE("session mut: stray valve states are ignored, motor and breakaway are published") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  CHECK(r.port.last.haveMotor);
  CHECK(r.port.last.motor.minCounts == 3000);
  CHECK(r.port.last.haveBreakaway);
  CHECK(r.port.last.learnMovements == 2000);
  const uint8_t status = r.port.last.valves[0].status;
  REQUIRE(status == 1);
  r.s.onLine("gvlst 12 9,9,9,9,9,9,9,9,9,9,9,9, ", 34, r.now);
  r.s.publishIfDue(r.now + 200);
  CHECK(r.port.last.valves[0].status == status);
}

TEST_CASE("session mut: a v1 valve that lost its counts is an STM reboot") {
  Rig r(1, 0x001);
  r.start();
  r.run(15000);
  REQUIRE_FALSE(r.port.has(EventCode::StmRebootDetected));
  r.stm.answers["gvlvd"] = [](const std::string& l) {
    return "gvlvd " + l.substr(6) + " 0 0 5 -500 -500 0 0 0 0 0 ";
  };
  r.run(5000);
  const std::vector<Event> ev = r.port.withCode(EventCode::StmRebootDetected);
  REQUIRE_FALSE(ev.empty());
  CHECK(ev[0].arg1 == 3);
}

TEST_CASE("session mut: no status after a link recovery is an STM reboot") {
  Rig r(3, 0x001);
  r.start();
  r.run(15000);
  r.stm.silent = true;
  r.run(8000);
  r.stm.answers["gstax"] = [](const std::string&) { return std::string(); };
  r.stm.silent = false;
  r.run(8000);
  const std::vector<Event> ev = r.port.withCode(EventCode::StmRebootDetected);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 4);
}

TEST_CASE("session mut: STM counters: baseline first, then increases per side") {
  Rig r(3, 0x001);
  uint32_t ovf = 5, perr = 3;
  r.stm.answers["gstax"] = [&](const std::string&) {
    return "gstax " + std::to_string(100 + r.now / 1000) + " 3 2 " + std::to_string(ovf) + " " +
           std::to_string(perr) + " 1 1 3540 1 60 0 0 0 0 0 0 0 0 0 0 0 0 0";
  };
  r.start();
  r.run(15000);
  CHECK_FALSE(r.port.has(EventCode::StmRxOverflow));
  CHECK_FALSE(r.port.has(EventCode::StmParseErrors));
  ovf = 7;
  perr = 4;
  r.run(12000);
  std::vector<Event> o = r.port.withCode(EventCode::StmRxOverflow);
  REQUIRE(o.size() == 1);
  CHECK(o[0].arg1 == 7);
  CHECK(o[0].arg2 == 1);
  std::vector<Event> p = r.port.withCode(EventCode::StmParseErrors);
  REQUIRE(p.size() == 1);
  CHECK(p[0].arg1 == 4);
  CHECK(p[0].arg2 == 1);
  // ESP side: a malformed line and a parse error.
  r.s.onRx("\x01\x02\r\n", 4, r.now);
  r.s.onRx("zz\r\n", 4, r.now);
  r.run(1100);
  p = r.port.withCode(EventCode::StmParseErrors);
  REQUIRE(p.size() == 2);
  CHECK(p[1].arg1 == 2);
  CHECK(p[1].arg2 == 0);
}

TEST_CASE("session mut: a valve without data turns stale") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  REQUIRE((r.port.last.valves[0].health & kHealthStale) == 0);
  r.stm.answers["gvlvy"] = [](const std::string&) { return std::string(); };
  r.run(70000);
  CHECK((r.port.last.valves[0].health & kHealthStale) != 0);
}

TEST_CASE("session mut: protocol 2: targets during a calibration and assembly by staop") {
  Rig r(2, 0x003);
  r.stm.answers["gvlvx"] = [&](const std::string& l) {
    const int v = std::stoi(l.substr(6));
    const std::string t = std::to_string(r.stm.target[v]);
    return "gvlvx " + std::to_string(v) + " " + (v == 0 ? "129" : "1") + " 40 " + t +
           " 21 3120 3350 -230 0 57 0 0 0 1 3000 1450 3 412 8123";
  };
  r.start();
  r.run(15000);
  REQUIRE(r.port.last.valves[0].calibrating);
  r.command(r.target(0, 20));
  r.run(3000);
  CHECK(r.stm.linesOf("stgtp") == std::vector<std::string>{"stgtp 0 20"});
  r.command(cmd(StmCommandType::Assembly, 1));
  r.run(3000);
  CHECK(r.count("staop 1") == 1);
  CHECK(r.stm.linesOf("stgtp").size() == 1);
}

TEST_CASE("session mut: stop read-back: one valve or every active valve, only after an ok") {
  Rig r(3, 0x007);
  r.start();
  r.run(10000);
  r.replyDelayMs = 20;
  r.command(cmd(StmCommandType::StopValve, 1));
  r.runUntil([&] { return r.count("sstop") == 1; }, 100);
  const size_t v1 = r.count("gvlvy 1"), v0 = r.count("gvlvy 0");
  r.run(150);
  CHECK(r.count("gvlvy 1") == v1 + 1);
  CHECK(r.count("gvlvy 0") == v0);
  r.command(cmd(StmCommandType::StopValve, kAllValves));
  r.runUntil([&] { return r.count("sstop") == 2; }, 100);
  const size_t a0 = r.count("gvlvy 0"), a1 = r.count("gvlvy 1"), a2 = r.count("gvlvy 2"),
               a3 = r.count("gvlvy 3");
  r.run(250);
  CHECK(r.count("gvlvy 0") == a0 + 1);
  CHECK(r.count("gvlvy 1") == a1 + 1);
  CHECK(r.count("gvlvy 2") == a2 + 1);
  CHECK(r.count("gvlvy 3") == a3);
  r.stm.answers["sstop"] = [](const std::string&) { return std::string("sstop 1 err 2"); };
  r.command(cmd(StmCommandType::StopValve, 1));
  r.runUntil([&] { return r.count("sstop") == 3; }, 100);
  const size_t e1 = r.count("gvlvy 1");
  r.run(150);
  CHECK(r.count("gvlvy 1") == e1);
}

TEST_CASE("session mut: masns re-reads the valve sensors only after an ok; v2 scans without masns") {
  Rig r(1, 0x001);
  r.start();
  r.run(15000);
  r.command(cmd(StmCommandType::ScanSensors));
  r.runUntil([&] { return r.count("masns") == 1; }, 8000);
  const size_t n = r.count("gvlon 255");
  r.run(1000);
  CHECK(r.count("gvlon 255") == n + 1);
  r.stm.answers["masns"] = [](const std::string&) { return std::string(); };
  r.command(cmd(StmCommandType::ScanSensors));
  r.runUntil([&] { return r.count("masns") == 2; }, 8000);
  const size_t m = r.count("gvlon 255");
  r.run(12000);
  CHECK(r.count("gvlon 255") == m);
  Rig v2(2, 0x001);
  v2.start();
  v2.run(10000);
  v2.command(cmd(StmCommandType::ScanSensors));
  v2.run(8000);
  CHECK(v2.count("stons") == 1);
  CHECK(v2.count("masns") == 0);
}

TEST_CASE("session mut: a policy reset reports the timeouts and the silent time") {
  Rig r(3, 0x001);
  r.start();
  r.run(15000);
  const uint32_t lastReply = r.s.link().stats().lastReplyMs;
  r.stm.silent = true;
  REQUIRE(r.runUntil([&] { return !r.port.pulses.empty(); }, 180000));
  const std::vector<Event> ev = r.port.withCode(EventCode::StmResetByPolicy);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 >= 5);
  CHECK(ev[0].arg2 == static_cast<int32_t>((r.port.pulses[0] - lastReply) / 1000));
  CHECK(r.s.link().stats().policyResets == 1);
  CHECK(r.s.link().stats().userResets == 0);
}

TEST_CASE("session mut: the silent time of a policy reset is in whole seconds, rounded down") {
  // Reply delays that put the silent time just below / just above a full second.
  const uint32_t delays[] = {183, 207};
  for (uint32_t d : delays) {
    CAPTURE(d);
    Rig r(3, 0);
    r.replyDelayMs = d;
    r.start();
    r.run(15000);
    r.stm.silent = true;
    const uint32_t lastReply = r.s.link().stats().lastReplyMs;
    REQUIRE(r.runUntil([&] { return !r.port.pulses.empty(); }, 180000));
    const uint32_t silentMs = r.port.pulses[0] - lastReply;
    CHECK((silentMs % 1000 >= 945 || silentMs % 1000 < 55));
    const std::vector<Event> ev = r.port.withCode(EventCode::StmResetByPolicy);
    REQUIRE(ev.size() == 1);
    CHECK(ev[0].arg2 == static_cast<int32_t>(silentMs / 1000));
  }
}

TEST_CASE("session mut: lease status changes publish, an unchanged status does not") {
  Rig r(3, 0x001);
  r.start();
  // No STM traffic: after the start-up hold nothing changes but the lease view.
  auto second = [&] {
    r.now += 1000;
    r.s.everySecond(r.now, r.reg, StmSaveState::Idle);
    r.s.publishIfDue(r.now);
  };
  for (int i = 0; i < 8; ++i) second();
  REQUIRE(r.port.last.link == LinkState::Unknown);
  int p = r.port.publishes;
  second();
  second();
  CHECK(r.port.publishes == p);
  r.reg.mode = MqttMode::Mqtt;  // broker not connected: the regulator is gone
  second();
  CHECK(r.port.publishes == ++p);
  second();
  CHECK(r.port.publishes == p);
  // Config trust and timeout alone (applyConfig publishes by itself first).
  r.s.applyConfig(r.cfg, false);
  r.s.publishIfDue(r.now + 500);
  p = r.port.publishes;
  second();
  CHECK(r.port.last.lease.configTrusted == false);
  CHECK(r.port.publishes == p + 1);
  r.cfg.failsafe.timeoutMin = 90;
  r.s.applyConfig(r.cfg, false);
  r.s.publishIfDue(r.now + 500);
  p = r.port.publishes;
  second();
  CHECK(r.port.last.lease.timeoutMin == 90);
  CHECK(r.port.publishes == p + 1);
  CHECK_FALSE(r.port.has(EventCode::TargetsRestored));
}

TEST_CASE("session mut: service move results: the next move of the valve, 5 min at most") {
  Rig r(3, 0x003);
  int counted = 1450, stop = 3;
  r.stm.answers["gvlvy"] = [&](const std::string& l) {
    const int v = std::stoi(l.substr(6));
    return gvlvy(v, 1, r.stm.target[v], v == 1 ? counted : 1450, v == 1 ? stop : 3);
  };
  r.start();
  r.run(10000);
  r.command(serviceMove(1, 300));
  r.run(2000);
  CHECK_FALSE(r.port.has(EventCode::ServiceMoveDone));
  counted = 1234;
  stop = 2;
  r.run(3000);
  std::vector<Event> ev = r.port.withCode(EventCode::ServiceMoveDone);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].valve == 1);
  CHECK(ev[0].arg1 == 1234);
  CHECK(ev[0].arg2 == 2);
  counted = 1235;
  r.run(3000);
  CHECK(r.port.withCode(EventCode::ServiceMoveDone).size() == 1);
  // No new move within 5 min: forgotten.
  r.command(serviceMove(1, 310));
  r.run(StmSession::kServiceMoveWaitMs + 2000);
  counted = 1300;
  r.run(3000);
  CHECK(r.port.withCode(EventCode::ServiceMoveDone).size() == 1);
}

TEST_CASE("session mut: CalibStarted of a scheduled calibration carries 1, a manual one 0") {
  Rig r(3, 0x003);
  bool cal[2] = {false, false};
  r.stm.answers["gvlvy"] = [&](const std::string& l) {
    const int v = std::stoi(l.substr(6));
    return gvlvy(v, v < 2 && cal[v] ? 129 : 1, r.stm.target[v], 1450, 3);
  };
  r.start();
  r.run(10000);
  StmCommand c = cmd(StmCommandType::Calibrate, kAllValves);
  c.scheduled = true;
  c.attempt = 1;
  r.command(c);
  r.run(1000);
  cal[1] = true;
  r.run(3000);
  std::vector<Event> ev = r.port.withCode(EventCode::CalibStarted);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].valve == 1);
  CHECK(ev[0].arg1 == 1);
  cal[1] = false;
  r.run(3000);
  r.command(cmd(StmCommandType::Calibrate, 0));
  r.run(1000);
  cal[0] = true;
  r.run(3000);
  ev = r.port.withCode(EventCode::CalibStarted);
  REQUIRE(ev.size() == 2);
  CHECK(ev[1].valve == 0);
  CHECK(ev[1].arg1 == 0);
}

TEST_CASE("session mut: the scheduled flag ends with the 4 h window") {
  Rig r(3, 0x001);
  bool cal = false;
  r.stm.answers["gvlvy"] = [&](const std::string& l) {
    const int v = std::stoi(l.substr(6));
    return gvlvy(v, v == 0 && cal ? 129 : 1, r.stm.target[v], 1450, 3);
  };
  r.start();
  r.run(10000);
  StmCommand c = cmd(StmCommandType::Calibrate, kAllValves);
  c.scheduled = true;
  r.command(c);
  r.run(2000);
  r.now += StmSession::kScheduledCalibWindowMs;
  r.lastSecond = r.now;
  r.s.everySecond(r.now, r.reg, StmSaveState::Idle);
  r.run(15000);
  cal = true;
  r.run(3000);
  const std::vector<Event> ev = r.port.withCode(EventCode::CalibStarted);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 0);
}

// ================================================================ status, snapshots

TEST_CASE("session mut: a repaired STM config is reported once per STM start") {
  Rig r(3, 0x001);
  uint32_t bootMs = r.now;
  r.stm.answers["gstax"] = [&](const std::string&) {
    return "gstax " + std::to_string((r.now - bootMs) / 1000) +
           " 3 2 0 0 1 1 3540 1 60 0 0 0 0 0 0 0 0 1 0 0 0 0";
  };
  r.start();
  r.run(30000);
  CHECK(r.port.last.haveStatus);
  CHECK(r.port.withCode(EventCode::StmConfigRepaired).size() == 1);
  // The STM restarts (uptime back to 0): the repair is reported again.
  bootMs = r.now;
  r.stm.reset(r.now);
  r.run(30000);
  CHECK(r.port.withCode(EventCode::StmRebootDetected).size() == 1);
  CHECK(r.port.withCode(EventCode::StmConfigRepaired).size() == 2);
}

TEST_CASE("session mut: a version reply alone is published") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.s.publishIfDue(r.now + 200);
  const std::string v = "gvers 2.1.7-revamped_C2 1712345678 ";
  r.s.onLine(v.c_str(), v.size(), r.now + 200);
  r.s.publishIfDue(r.now + 400);
  CHECK(r.port.last.version.patch == 7);
}

TEST_CASE("session mut: timeouts on a down link are published") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.stm.silent = true;
  r.run(10000);
  REQUIRE(r.port.last.link == LinkState::Down);
  const uint32_t failed = r.port.last.linkStats.failedRequests;
  r.run(5000);
  CHECK(r.port.last.linkStats.failedRequests > failed);
}

TEST_CASE("session mut: snapshots at most every 100 ms, exactly then when busy") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.port.publishTimes.clear();
  for (uint16_t i = 0; i < 20; ++i) r.command(serviceMove(0, static_cast<uint16_t>(100 + i)));
  r.run(3000);
  REQUIRE(r.port.publishTimes.size() >= 2);
  uint32_t minGap = UINT32_MAX;
  for (size_t i = 1; i < r.port.publishTimes.size(); ++i) {
    minGap = std::min(minGap, r.port.publishTimes[i] - r.port.publishTimes[i - 1]);
  }
  CHECK(minGap == 100);
  CHECK(r.port.publishTimes.size() <= 31);
}

TEST_CASE("session mut: protocol 2 valve temperatures come from the assigned sensors") {
  SensorRig t;
  Rig& r = t.r;
  r.stm.protocol = 2;
  r.stm.answers["gvlon"] = [](const std::string& l) {
    const std::string z = "00-00-00-00-00-00-00-00";
    if (l == "gvlon 255") {
      std::string ids = std::string(kIdA) + "," + kIdB;
      for (int k = 2; k < 24; ++k) ids += "," + z;
      return "gvlon 12 " + ids;
    }
    return "gvlon " + l.substr(6) + " " + (l.substr(6) == "0" ? std::string(kIdA) + " " + kIdB : z + " " + z) + " ";
  };
  r.run(45000);
  CHECK(r.port.last.proto == 2);
  CHECK(r.port.last.valves[0].temp1 == 215);
  CHECK(r.port.last.valves[0].temp2 == 198);
  // A stray assignment changes nothing.
  const std::string stray = std::string("gvlon 0 ") + kIdB + " " + kIdA + " ";
  r.s.onLine(stray.c_str(), stray.size(), r.now);
  r.run(1500);
  CHECK(r.port.last.valves[0].temp1 == 215);
}

TEST_CASE("session mut: protocol 1 valve temperatures come from gvlvd") {
  SensorRig t;
  Rig& r = t.r;
  r.stm.protocol = 1;
  t.bus.raw[kIdA] = 230;
  r.stm.answers["gvlon"] = [](const std::string& l) {
    const std::string z = "00-00-00-00-00-00-00-00";
    if (l == "gvlon 255") {
      std::string ids = std::string(kIdA) + "," + kIdB;
      for (int k = 2; k < 24; ++k) ids += "," + z;
      return "gvlon 12 " + ids;
    }
    return "gvlon " + l.substr(6) + " " + z + " " + z + " ";
  };
  r.run(45000);
  CHECK(r.port.last.proto == 1);
  CHECK(r.port.last.valves[0].temp1 == 215);
}

TEST_CASE("session mut: profiles are stored per valve; stray valve states are ignored") {
  Rig r(3, 0x001);
  r.start();
  r.s.onLine("gvlst 12 9,9,9,9,9,9,9,9,9,9,9,9, ", 34, r.now);
  r.s.publishIfDue(r.now);
  CHECK_FALSE(r.port.last.valves[3].known);
  r.run(10000);
  r.stm.answers["gprof"] = [](const std::string&) { return std::string("gprof 2 2 10:5 20:6"); };
  r.command(cmd(StmCommandType::RequestProfile, 2));
  r.run(2000);
  CHECK(r.port.last.profiles[2].count == 2);
  CHECK(r.port.last.profiles[2].samples[1].count == 20);
}

TEST_CASE("session mut: an incompatible STM is logged once without arguments") {
  Rig r(1, 0x001);
  r.stm.tooOld = true;
  r.start();
  r.run(10000);
  const std::vector<Event> ev = r.port.withCode(EventCode::StmIncompatible);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 0);
  CHECK(ev[0].arg2 == 0);
  CHECK(std::string(ev[0].text) == "1.3.5_C2");
}

TEST_CASE("session mut: sensors settle exactly 30 s after the start and it is published") {
  Rig r(3, 0);
  r.start();
  r.run(30000 - 2);
  CHECK_FALSE(r.port.last.sensorsSettled);
  r.run(100);
  CHECK(r.port.last.sensorsSettled);
}

TEST_CASE("session mut: an inactive volt slot with an id and an active one without are silent") {
  Rig r(3, 0x001);
  Bus bus;
  r.cfg.volts[0].active = false;
  r.cfg.volts[0].id = oid(kIdV);
  r.cfg.volts[1].active = true;  // no id
  bus.volts = {kIdV};
  bus.vad = {{kIdV, 1200}};
  bus.attach(r);
  r.start();
  r.run(45000);
  bus.vad[kIdV] = kVadFailed;
  r.run(40000);
  CHECK_FALSE(r.port.has(EventCode::VoltSensorFailed));
}

TEST_CASE("session mut: a flash that gave up waiting for the EEPROM publishes the refused start") {
  Rig r(3, 0);
  r.start();
  r.run(10000);
  r.stm.eep = 0;
  r.port.imageOk = false;
  r.command(flashCmd("x.bin", false));
  r.run(1000);
  REQUIRE(r.port.last.flashPending);
  REQUIRE(r.runUntil([&] { return r.port.has(EventCode::StmEepromWaitTimeout); }, 12000));
  CHECK(r.port.has(EventCode::StmFlashFailed));
  CHECK_FALSE(r.port.last.flashPending);
}

TEST_CASE("session mut: v1: no target push while calibrating; assembly re-pushed by stgtp") {
  Rig r(1, 0x003);
  r.stm.answers["gvlvd"] = [](const std::string& l) {
    const std::string v = l.substr(6);
    return "gvlvd " + v + " 42 18 " + (v == "0" ? "129" : "1") + " 215 -500 57 3120 3350 230 0 ";
  };
  r.start();
  r.run(15000);
  REQUIRE(r.port.last.valves[0].calibrating);
  r.command(r.target(0, 20));
  r.run(3000);
  CHECK(r.count("stgtp 0") == 0);
  r.command(cmd(StmCommandType::Assembly, 1));
  r.run(2000);
  REQUIRE(r.count("staop 1") == 1);
  r.stm.silent = true;  // a link interruption is a reboot on v1
  r.run(8000);
  r.stm.reset(r.now);
  r.stm.silent = false;
  r.run(20000);
  REQUIRE(r.port.has(EventCode::StmRebootDetected));
  CHECK(r.count("staop 1") == 1);
  CHECK(r.stm.target[1] == 100);
}

// ================================================================ EEPROM gate order

TEST_CASE("session mut: the EEPROM poll waits for queued config lines") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.replyDelayMs = 300;
  r.command(cmd(StmCommandType::Detect));
  r.run(4);
  r.command(r.target(0, 30));  // stgtp queued behind the outstanding stdet
  r.run(4);
  r.command(cmd(StmCommandType::ResetStm));
  r.run(2000);
  const std::vector<std::string>& l = r.stm.lines;
  const auto stgtp = std::find(l.begin(), l.end(), "stgtp 0 30");
  const auto eep = std::find(l.begin(), l.end(), "eepst");
  REQUIRE(stgtp != l.end());
  REQUIRE(eep != l.end());
  CHECK(stgtp < eep);
}

TEST_CASE("session mut: the EEPROM poll waits for queued user and config lines") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.replyDelayMs = 300;
  r.command(cmd(StmCommandType::Detect));
  r.run(4);
  r.command(cmd(StmCommandType::Calibrate, 0));
  r.command(r.target(0, 30));
  r.run(4);
  r.command(cmd(StmCommandType::ResetStm));
  r.run(3000);
  const std::vector<std::string>& l = r.stm.lines;
  const auto stgtp = std::find(l.begin(), l.end(), "stgtp 0 30");
  const auto eep = std::find(l.begin(), l.end(), "eepst");
  REQUIRE(stgtp != l.end());
  REQUIRE(eep != l.end());
  CHECK(stgtp < eep);
}

TEST_CASE("session mut: the EEPROM poll waits for the retries of an outstanding config line") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.stm.answers["stgtp"] = [](const std::string&) { return std::string(); };
  r.command(r.target(0, 30));
  r.runUntil([&] { return r.count("stgtp") == 1; }, 100);
  r.command(cmd(StmCommandType::ResetStm));
  r.run(3000);
  const std::vector<std::string>& l = r.stm.lines;
  const auto eep = std::find(l.begin(), l.end(), "eepst");
  REQUIRE(eep != l.end());
  CHECK(std::count(l.begin(), eep, std::string("stgtp 0 30")) == 3);
}

TEST_CASE("session mut: the EEPROM poll is sent every 500 ms, not more often") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.stm.eep = 0;
  r.command(cmd(StmCommandType::ResetStm));
  r.run(2000);
  CHECK(r.count("eepst") >= 3);
  CHECK(r.count("eepst") <= 5);
}

// ================================================================ exact time limits

TEST_CASE("session mut: a service move is forgotten exactly 5 min after its answer") {
  Rig r(3, 0x003);
  int counted = 1450;
  r.stm.answers["gvlvy"] = [&](const std::string& l) {
    const int v = std::stoi(l.substr(6));
    return gvlvy(v, 1, r.stm.target[v], v == 1 ? counted : 1450, 3);
  };
  r.start();
  r.run(10000);
  r.replyDelayMs = 2;
  r.command(serviceMove(1, 300));
  REQUIRE(r.runUntil([&] { return r.count("svmov") == 1; }, 200));
  const uint32_t answered = r.now + 2;  // the reply is applied in the next step
  r.run(4);
  // Drive the rest by hand: no polls, exact seconds.
  r.now = answered + StmSession::kServiceMoveWaitMs;
  r.s.everySecond(r.now, r.reg, StmSaveState::Idle);
  counted = 1234;
  const std::string y = gvlvy(1, 1, 50, 1234, 2);
  r.s.onLine(y.c_str(), y.size(), r.now);
  r.now += 1000;
  r.s.everySecond(r.now, r.reg, StmSaveState::Idle);
  CHECK_FALSE(r.port.has(EventCode::ServiceMoveDone));
}

TEST_CASE("session mut: a volt reading is valid for exactly 60 s") {
  SensorRig t;
  Rig& r = t.r;
  r.run(45000);
  const uint32_t seen = r.now;
  const std::string v = std::string("gowvd ") + kIdV + " 1200 ";
  r.s.onLine(v.c_str(), v.size(), seen);
  auto slot1Failed = [&] {
    for (const Event& e : r.port.withCode(EventCode::VoltSensorFailed)) {
      if (e.arg1 == 1) return true;
    }
    return false;
  };
  // Driven by hand: no further readings.
  r.s.everySecond(seen + StmSession::kSensorStaleMs, r.reg, StmSaveState::Idle);
  CHECK_FALSE(slot1Failed());
  r.s.everySecond(seen + StmSession::kSensorStaleMs + 1000, r.reg, StmSaveState::Idle);
  CHECK(slot1Failed());
}

TEST_CASE("session mut: the scheduled flag ends exactly 4 h after the command") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  StmCommand c = cmd(StmCommandType::Calibrate, kAllValves);
  c.scheduled = true;
  const uint32_t at = r.now;
  r.command(c);
  r.run(2000);
  r.now = at + StmSession::kScheduledCalibWindowMs;
  const std::string y = gvlvy(0, 129, 50, 1450, 3);
  r.s.onLine(y.c_str(), y.size(), r.now);
  r.s.everySecond(r.now, r.reg, StmSaveState::Idle);
  r.s.publishIfDue(r.now);
  const std::vector<Event> ev = r.port.withCode(EventCode::CalibStarted);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 0);
}

TEST_CASE("session mut: moving valves are polled every 500 ms, idle ones every 2 s") {
  Rig r(3, 0x007);
  r.stm.answers["gvlvy"] = [&](const std::string& l) {
    const int v = std::stoi(l.substr(6));
    return gvlvy(v, v < 2 ? 2 : 1, r.stm.target[v], 1450, 3);
  };
  r.start();
  r.run(15000);
  const size_t a = r.count("gvlvy 0"), b = r.count("gvlvy 1"), c = r.count("gvlvy 2");
  r.run(10000);
  CHECK(r.count("gvlvy 0") - a >= 15);
  CHECK(r.count("gvlvy 1") - b >= 15);
  CHECK(r.count("gvlvy 2") - c <= 6);
}

TEST_CASE("session mut: an assembly hold is pushed once after an STM reboot") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  r.command(cmd(StmCommandType::Assembly, 0));
  r.run(2000);
  REQUIRE(r.count("staop 0") == 1);
  r.stm.reset(r.now);
  r.run(20000);
  const size_t n = r.count("staop 0");
  CHECK(n >= 2);
  r.run(20000);
  CHECK(r.count("staop 0") == n);  // acknowledged: not pushed again
}

TEST_CASE("session mut: an STM reset forgets pending service moves") {
  Rig r(3, 0x003);
  int counted = 1450;
  r.stm.answers["gvlvy"] = [&](const std::string& l) {
    const int v = std::stoi(l.substr(6));
    return gvlvy(v, 1, r.stm.target[v], v == 1 ? counted : 1450, 3);
  };
  r.start();
  r.run(10000);
  r.command(serviceMove(1, 300));
  r.run(1000);
  r.command(cmd(StmCommandType::ResetStm));
  r.run(1000);
  REQUIRE(r.port.pulses.size() == 1);
  r.run(8000);
  counted = 1111;
  r.run(3000);
  CHECK_FALSE(r.port.has(EventCode::ServiceMoveDone));
}
