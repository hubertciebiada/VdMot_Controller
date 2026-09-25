// StmSession end to end against a scripted STM (line level) and a fake port:
// start-up per protocol, unsupported STMs, lease and failsafe emulation,
// assembly, restored targets, scheduled calibration results, EEPROM-safe
// resets and restarts, link recovery, flashing, protocol 3 commands.
#include <stdint.h>
#include <string.h>

#include <algorithm>
#include <deque>
#include <string>
#include <utility>
#include <vector>

#include "doctest.h"
#include "support/line_stm.h"
#include "support/sim_stm.h"
#include "vdm/stm_session.h"

using namespace vdm;
using vdm_test::LineStm;
using vdm_test::SimStm;

namespace {

class MemImage : public FlashImage {
 public:
  std::vector<uint8_t> data;
  uint32_t size() const override { return static_cast<uint32_t>(data.size()); }
  bool read(uint32_t offset, uint8_t* out, size_t len) override {
    if (offset > data.size() || len > data.size() - offset) return false;
    memcpy(out, data.data() + offset, len);
    return true;
  }
};

struct TestPort : StmSessionPort {
  LineStm* stm = nullptr;
  std::vector<Event> events;
  StmSnapshot last;
  int publishes = 0;
  std::vector<uint32_t> pulses;
  std::vector<std::string> lastGood;
  bool restart = false;
  int flashMarks = 0;
  std::vector<PersistedTargets> targets;
  struct Calib {
    uint16_t attempt;
    bool ok;
    CalibFailure reason;
  };
  std::vector<Calib> calibs;
  std::vector<StmSaveState> saves;
  LeaseClient::Snapshot lease;
  int leaseRecords = 0;
  MemImage image;
  bool imageOk = true;
  std::vector<std::string> opened;
  int closed = 0;

  void logEvent(const Event& e) override { events.push_back(e); }
  uint32_t pulseReset(uint32_t nowMs) override {
    pulses.push_back(nowMs);
    if (stm) stm->reset(nowMs + 100);
    return nowMs + 100;
  }
  void publish(const StmSnapshot& s) override {
    last = s;
    ++publishes;
  }
  void requestLastGoodCopy(const char* image) override { lastGood.push_back(image); }
  bool restartPending() override { return restart; }
  void markFlashActive() override { ++flashMarks; }
  void storeDesiredTargets(const PersistedTargets& t) override { targets.push_back(t); }
  void postScheduledCalibResult(uint16_t attempt, bool ok, CalibFailure reason) override {
    calibs.push_back({attempt, ok, reason});
  }
  void setStmSaveState(StmSaveState s) override { saves.push_back(s); }
  void storeLeaseRecord(const LeaseClient::Snapshot& s) override {
    lease = s;
    ++leaseRecords;
  }
  FlashImage* openImage(const char* name) override {
    opened.push_back(name);
    return imageOk ? &image : nullptr;
  }
  void closeImage() override { ++closed; }

  std::vector<Event> withCode(EventCode c) const {
    std::vector<Event> out;
    for (const Event& e : events) {
      if (e.code == c) out.push_back(e);
    }
    return out;
  }
  bool has(EventCode c) const { return !withCode(c).empty(); }
};

struct Rig {
  uint32_t now = 1000;
  SimStm sim{now};
  TestPort port;
  LineStm stm;
  StmSession s{port, sim};
  Config cfg;
  bool trusted = true;
  RegulatorInput reg;
  StmSaveState save = StmSaveState::Idle;
  std::deque<std::pair<uint32_t, std::string>> replies;
  uint32_t lastSecond = 0;
  uint32_t replyDelayMs = 3;

  explicit Rig(uint8_t protocol = 3, uint16_t active = 0x00F) {
    port.stm = &stm;
    stm.protocol = protocol;
    setDefaults(cfg);
    for (uint8_t v = 0; v < kValveCount; ++v) cfg.valves[v].active = ((active >> v) & 1u) != 0;
  }
  void start(const PersistedTargets& t = PersistedTargets{},
             RestoreSource src = RestoreSource::None, const LeaseClient::Snapshot* lease = nullptr) {
    s.applyConfig(cfg, trusted);
    s.begin(now, t, src, lease);
    lastSecond = now;
  }
  void command(const StmCommand& c) { s.handleCommand(c, now); }
  void step() {
    now += 2;
    while (!replies.empty() && replies.front().first <= now) {
      const std::string r = replies.front().second;
      replies.pop_front();
      s.onLine(r.c_str(), r.size(), now);
    }
    if (s.flashing()) {
      s.flashStep(now);
    } else {
      s.poll(now);
      if (const RequestLine* line = s.nextToSend(now)) {
        std::string text(line->text, line->len);
        while (!text.empty() && (text.back() == '\n' || text.back() == '\r' || text.back() == ' ')) {
          text.pop_back();
        }
        s.onSent(now);
        const std::string r = stm.answer(text, now);
        if (!r.empty()) replies.emplace_back(now + replyDelayMs, r);
      }
    }
    if (now - lastSecond >= 1000) {
      lastSecond = now;
      s.everySecond(now, reg, save);
    }
    s.publishIfDue(now);
  }
  void run(uint32_t ms) {
    const uint32_t end = now + ms;
    while (static_cast<int32_t>(end - now) > 0) step();
  }
  // Runs until `cond` or `ms` passed; returns cond().
  template <typename F>
  bool runUntil(F cond, uint32_t ms) {
    const uint32_t end = now + ms;
    while (!cond() && static_cast<int32_t>(end - now) > 0) step();
    return cond();
  }
  size_t count(const std::string& cmd) const { return stm.linesOf(cmd).size(); }
  StmCommand target(uint8_t v, uint8_t pos, TargetSource src = TargetSource::Web) const {
    StmCommand c;
    c.type = StmCommandType::SetTarget;
    c.valve = v;
    c.pos = pos;
    c.source = src;
    return c;
  }
};

StmCommand cmd(StmCommandType t, uint8_t valve = kNoValve) {
  StmCommand c;
  c.type = t;
  c.valve = valve;
  return c;
}

}  // namespace

// ================================================================ start-up

TEST_CASE("session: protocol 3 start-up: gproto and gvers first, then polls, lease and learn time") {
  Rig r(3, 0);
  r.cfg.calib.dayMask = 0;
  r.start();
  r.run(10000);
  REQUIRE(r.stm.lines.size() >= 3);
  CHECK(r.stm.lines[0] == "gproto");
  CHECK(r.stm.lines[1] == "gvers");
  CHECK(r.port.last.link == LinkState::Up);
  CHECK(r.port.last.proto == 3);
  CHECK(r.port.last.support == StmSupport::Supported);
  CHECK(r.count("gvlvy 0") >= 1);
  CHECK(r.count("gstax") >= 1);
  CHECK(r.count("slhbt 1") == 1);  // MQTT off: the regulator is alive
  CHECK(r.count("glcfg") == 2);    // read, push, verify
  CHECK(r.stm.linesOf("sfspo") == std::vector<std::string>{"sfspo 255 255"});  // no valve active
  CHECK(r.port.last.lease.mode == LeaseMode::Stm);
  CHECK(r.port.last.lease.configSynced);
  CHECK(r.count("gtlnt") == 1);  // schedule off, the STM holds the default already
  CHECK(r.port.last.haveLearnTime);
  CHECK(r.port.last.learnTimeS == 604800);
  CHECK(r.count("stgtp") == 0);
  CHECK_FALSE(r.port.has(EventCode::StmRebootDetected));
  CHECK(r.port.leaseRecords >= 9);
  CHECK_FALSE(r.port.lease.lost);
}

TEST_CASE("session: protocol 1 start-up polls gvlvd and gtgtp, no protocol 3 command") {
  Rig r(1);
  r.start();
  r.run(15000);
  CHECK(r.port.last.link == LinkState::Up);
  CHECK(r.port.last.proto == 1);
  CHECK(r.count("gvlvd 0") >= 1);
  CHECK(r.count("gtgtp 0") >= 1);
  CHECK(r.count("slhbt") == 0);
  CHECK(r.count("glcfg") == 0);
  CHECK(r.count("gtlnt") == 0);
  CHECK(r.count("gstat") == 0);
  CHECK(r.port.last.lease.mode == LeaseMode::Emulated);
}

TEST_CASE("session: the first snapshot waits for a change and 100 ms between snapshots") {
  Rig r(3);
  r.start();
  r.run(4);
  CHECK(r.port.publishes == 1);
  const uint32_t rev = r.port.last.revision;
  r.run(50);
  CHECK(r.port.publishes == 1);  // within 100 ms
  r.run(20000);
  CHECK(r.port.last.revision > rev);
  CHECK(r.port.last.takenMs <= r.now);
}

// ================================================================ unsupported STM (W13)

TEST_CASE("session: an STM below 1.4.0 gets gvers every 30 s and targets only") {
  Rig r(1);
  r.stm.tooOld = true;
  r.start();
  r.run(10000);
  CHECK(r.port.last.support == StmSupport::TooOld);
  CHECK(r.count("gproto") == 3);  // one probe, three attempts
  const size_t gvers = r.count("gvers");
  CHECK(r.count("gvlvd") == 0);
  CHECK(r.count("gowvc") == 0);
  CHECK(r.count("ghwin") == 0);
  r.command(r.target(1, 33));
  r.run(2000);
  CHECK(r.stm.linesOf("stgtp") == std::vector<std::string>{"stgtp 1 33"});
  CHECK(r.count("gtgtp 1") >= 1);
  CHECK(r.port.last.valves[1].sync == TargetSync::Synced);
  // Other STM commands are dropped; a scheduled calibration reports it.
  StmCommand c = cmd(StmCommandType::Calibrate, kAllValves);
  c.scheduled = true;
  c.attempt = 7;
  r.command(c);
  r.command(cmd(StmCommandType::Detect));
  r.command(cmd(StmCommandType::Assembly, 2));
  r.run(1000);
  CHECK(r.count("staln") == 0);
  CHECK(r.count("stdet") == 0);
  CHECK(r.count("staop") == 0);
  REQUIRE(r.port.calibs.size() == 1);
  CHECK(r.port.calibs[0].attempt == 7);
  CHECK_FALSE(r.port.calibs[0].ok);
  CHECK(r.port.calibs[0].reason == CalibFailure::Unsupported);
  r.run(10 * 60000);
  CHECK(r.count("gvers") >= gvers + 20);
  CHECK_FALSE(r.port.has(EventCode::LinkDegraded));
  CHECK(r.port.withCode(EventCode::StmIncompatible).size() == 1);
  CHECK_FALSE(r.port.last.valves[0].known);
}

TEST_CASE("session: flashing a supported firmware after an unsupported one re-syncs") {
  Rig r(3);
  r.stm.tooOld = true;
  r.start();
  r.run(8000);
  REQUIRE(r.port.last.support == StmSupport::TooOld);
  r.stm.tooOld = false;
  r.run(31000);
  CHECK(r.port.last.support == StmSupport::Supported);
  CHECK(r.port.last.proto == 3);
  CHECK(r.count("gproto") == 4);  // three unanswered attempts, then the answered one
}

// ================================================================ lease and failsafe (K1)

TEST_CASE("session: protocol 3 heartbeat every 60 s and slhbt 0 within a second of HA offline") {
  Rig r(3);
  r.reg.mode = MqttMode::MqttHa;
  r.reg.brokerConnected = true;
  r.start();
  r.run(130000);
  CHECK(r.count("slhbt 1") == 3);
  const std::vector<std::string> hb = r.stm.linesOf("slhbt");
  r.reg.ha = HaStatus::Offline;
  const size_t before = r.count("slhbt 0");
  r.run(1100);
  CHECK(r.count("slhbt 0") == before + 1);
  CHECK(hb.size() == 3);
  r.reg.ha = HaStatus::Online;
  r.run(1100);
  CHECK(r.count("slhbt 1") == 4);
}

TEST_CASE("session: an old STM gets the failsafe positions from the ESP after the timeout") {
  Rig r(1, 0x003);
  r.cfg.failsafe.timeoutMin = 5;
  r.cfg.valves[1].failsafePct = 20;
  r.reg.mode = MqttMode::Mqtt;
  r.reg.brokerConnected = true;
  r.start();
  r.run(15000);
  r.command(r.target(0, 33, TargetSource::Mqtt));
  r.command(r.target(1, 44, TargetSource::Mqtt));
  r.run(5000);
  REQUIRE(r.stm.target[0] == 33);
  REQUIRE(r.stm.target[1] == 44);
  r.reg.brokerConnected = false;  // the broker goes away
  r.run(5 * 60000 - 2000);
  CHECK(r.stm.target[0] == 33);
  r.run(4000);
  CHECK(r.stm.target[0] == 50);
  CHECK(r.stm.target[1] == 20);
  CHECK(r.port.last.valves[0].desired == 33);  // the MQTT target is unchanged
  CHECK(r.port.last.valves[0].fsOverride);
  CHECK(r.port.last.lease.state == LeaseState::Expired);
  const std::vector<Event> active = r.port.withCode(EventCode::FailsafeActive);
  REQUIRE(active.size() == 1);
  CHECK(active[0].arg1 == 0x003);
  CHECK(active[0].arg2 == 2);
  CHECK(r.port.lease.active);
  // Broker back: renewed after 120 s of life (no command), the targets return.
  r.reg.brokerConnected = true;
  r.run(119000);
  CHECK(r.stm.target[0] == 50);
  r.run(4000);
  CHECK(r.stm.target[0] == 33);
  CHECK(r.stm.target[1] == 44);
  CHECK(r.port.has(EventCode::FailsafeEnded));
}

TEST_CASE("session: after an ESP restart an active lease record drives the failsafe at once") {
  Rig r(2, 0x001);
  r.cfg.failsafe.timeoutMin = 5;
  LeaseClient::Snapshot rec;
  rec.lost = true;
  rec.active = true;
  rec.mask = 0x001;
  rec.lostElapsedMs = 400000;
  PersistedTargets t;
  t.valid[0] = true;
  t.pos[0] = 30;
  t.source[0] = TargetSource::Mqtt;
  r.reg.mode = MqttMode::Mqtt;
  r.stm.target[0] = 30;
  r.start(t, RestoreSource::Rtc, &rec);
  r.run(20000);
  const std::vector<std::string> pushes = r.stm.linesOf("stgtp 0");
  REQUIRE_FALSE(pushes.empty());
  CHECK(pushes.front() == "stgtp 0 50");
  CHECK_FALSE(r.port.has(EventCode::FailsafeActive));
  CHECK(r.port.withCode(EventCode::TargetsRestored).size() == 1);
}

TEST_CASE("session: an assembly valve is never overridden and gets staop again after a reboot") {
  Rig r(2, 0x001);
  r.cfg.failsafe.timeoutMin = 5;
  r.reg.mode = MqttMode::Mqtt;  // broker never up: the regulator is dead
  r.start();
  r.run(15000);
  r.command(cmd(StmCommandType::Assembly, 0));
  r.run(2000);
  REQUIRE(r.count("staop 0") == 1);
  REQUIRE(r.port.last.valves[0].source == TargetSource::Assembly);
  // STM cold reboot (uptime back to 0).
  r.stm.reset(r.now);
  r.run(20000);
  CHECK(r.count("staop 0") >= 2);
  r.run(6 * 60000);
  CHECK(r.count("stgtp") == 0);
  CHECK_FALSE(r.port.last.valves[0].fsOverride);
  CHECK(r.stm.target[0] == 100);
}

// ================================================================ assembly (W1)

TEST_CASE("session: assembly sends staop and no stgtp; a web target afterwards is pushed") {
  Rig r(3, 0x004);
  r.start();
  r.run(10000);
  r.command(r.target(2, 30));
  r.run(3000);
  REQUIRE(r.stm.target[2] == 30);
  r.command(cmd(StmCommandType::Assembly, 2));
  r.run(60000);
  CHECK(r.count("staop 2") == 1);
  CHECK(r.stm.linesOf("stgtp") == std::vector<std::string>{"stgtp 2 30"});
  const ValveState& v = r.port.last.valves[2];
  CHECK(v.desired == 100);
  CHECK(v.source == TargetSource::Assembly);
  CHECK(v.sync == TargetSync::Synced);
  const std::vector<Event> set = r.port.withCode(EventCode::TargetSet);
  REQUIRE_FALSE(set.empty());
  CHECK(set.back().arg1 == 100);
  CHECK(set.back().arg2 == 5);
  r.command(r.target(2, 40));
  r.run(3000);
  CHECK(r.stm.linesOf("stgtp").back() == "stgtp 2 40");
}

// ================================================================ restored targets (W2)

TEST_CASE("session: restored targets: no stgtp when the STM holds them, pushed when it does not") {
  Rig r(3, 0x003);
  PersistedTargets t;
  t.valid[0] = true;
  t.pos[0] = 33;
  t.source[0] = TargetSource::Mqtt;
  t.valid[1] = true;
  t.pos[1] = 77;
  t.source[1] = TargetSource::Web;
  r.stm.target[0] = 33;
  r.start(t, RestoreSource::Rtc);
  r.run(15000);
  CHECK(r.stm.linesOf("stgtp") == std::vector<std::string>{"stgtp 1 77"});
  CHECK(r.port.last.valves[0].source == TargetSource::Restored);
  CHECK(r.port.last.valves[0].sync == TargetSync::Synced);
  const std::vector<Event> ev = r.port.withCode(EventCode::TargetsRestored);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 2);
  CHECK(ev[0].arg2 == 1);
  CHECK_FALSE(r.port.has(EventCode::TargetSet));
}

TEST_CASE("session: desired target changes go to the port once per change") {
  Rig r(3, 0x001);
  r.start();
  r.run(10000);
  const size_t n = r.port.targets.size();
  CHECK(n >= 1);  // the adopted STM target
  r.command(r.target(0, 20));
  r.run(3000);
  REQUIRE(r.port.targets.size() == n + 1);
  CHECK(r.port.targets.back().valid[0]);
  CHECK(r.port.targets.back().pos[0] == 20);
  CHECK(r.port.targets.back().source[0] == TargetSource::Web);
  r.run(10000);
  CHECK(r.port.targets.size() == n + 1);
}

// ================================================================ scheduled calibration (E1)

TEST_CASE("session: a scheduled staln reports its STM result with the attempt") {
  Rig r(3);
  r.start();
  r.run(10000);
  StmCommand c = cmd(StmCommandType::Calibrate, kAllValves);
  c.scheduled = true;
  c.attempt = 4;
  r.command(c);
  r.run(1000);
  CHECK(r.count("staln 255") == 1);
  REQUIRE(r.port.calibs.size() == 1);
  CHECK(r.port.calibs[0].attempt == 4);
  CHECK(r.port.calibs[0].ok);
  CHECK(r.port.calibs[0].reason == CalibFailure::None);
  // A manual one reports nothing.
  r.command(cmd(StmCommandType::Calibrate, 1));
  r.run(1000);
  CHECK(r.port.calibs.size() == 1);
}

TEST_CASE("session: a scheduled staln without an answer reports no_reply") {
  Rig r(3);
  r.start();
  r.run(10000);
  r.stm.answers["staln"] = [](const std::string&) { return std::string(); };
  StmCommand c = cmd(StmCommandType::Calibrate, kAllValves);
  c.scheduled = true;
  c.attempt = 9;
  r.command(c);
  r.run(5000);
  REQUIRE(r.port.calibs.size() == 1);
  CHECK(r.port.calibs[0].attempt == 9);
  CHECK_FALSE(r.port.calibs[0].ok);
  CHECK(r.port.calibs[0].reason == CalibFailure::NoReply);
}

// ================================================================ EEPROM-safe resets (E4)

TEST_CASE("session: a user reset waits for the EEPROM: motor settings, eepst until 1, then NRST") {
  Rig r(3);
  r.start();
  r.run(10000);
  r.stm.eep = 0;
  StmCommand m = cmd(StmCommandType::SetMotorSettings);
  m.hasMotor = true;
  m.motor.minCounts = 3100;
  r.command(m);
  r.command(cmd(StmCommandType::ResetStm));
  r.run(1600);
  CHECK(r.port.pulses.empty());
  const std::vector<std::string>& l = r.stm.lines;
  const auto smotc = std::find_if(l.begin(), l.end(), [](const std::string& s) {
    return s.compare(0, 5, "smotc") == 0;
  });
  const auto eep = std::find(l.begin(), l.end(), "eepst");
  REQUIRE(smotc != l.end());
  REQUIRE(eep != l.end());
  CHECK(smotc < eep);
  CHECK(r.count("eepst") >= 3);  // at once, then every 500 ms
  r.stm.eep = 1;
  r.run(600);
  CHECK(r.port.pulses.size() == 1);
  CHECK(r.port.has(EventCode::StmResetByUser));
  CHECK_FALSE(r.port.has(EventCode::StmEepromWaitTimeout));
  r.command(cmd(StmCommandType::ResetStm));
  r.command(cmd(StmCommandType::ResetStm));  // a second one while the gate runs is ignored
  r.run(12000);
  CHECK(r.port.pulses.size() == 2);
}

TEST_CASE("session: a reset gives up waiting after 10 s and reports it") {
  Rig r(3);
  r.start();
  r.run(10000);
  r.stm.eep = 0;
  const uint32_t t0 = r.now;
  r.command(cmd(StmCommandType::ResetStm));
  r.run(9990);
  CHECK(r.port.pulses.empty());
  r.run(20);
  REQUIRE(r.port.pulses.size() == 1);
  CHECK(r.port.pulses[0] - t0 == 10000);
  const std::vector<Event> ev = r.port.withCode(EventCode::StmEepromWaitTimeout);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 10000);
  CHECK(ev[0].arg2 == 1);
}

TEST_CASE("session: a silent STM is reset at once by the user") {
  Rig r(3);
  r.start();
  r.stm.silent = true;
  r.run(30000);
  REQUIRE(r.port.last.link == LinkState::Down);
  r.command(cmd(StmCommandType::ResetStm));
  r.run(4);
  CHECK(r.port.pulses.size() == 1);
  CHECK(r.count("eepst") == 0);
}

TEST_CASE("session: the STM save before an ESP restart: eepst until 1 -> Saved") {
  Rig r(3);
  r.start();
  r.run(10000);
  r.stm.eep = 0;
  r.save = StmSaveState::Waiting;
  r.run(1100);
  const size_t first = r.count("eepst");
  CHECK(first >= 1);
  r.run(1000);
  CHECK(r.count("eepst") >= first + 2);
  CHECK(r.port.saves.empty());
  r.stm.eep = 1;
  r.run(600);
  CHECK(r.port.saves == std::vector<StmSaveState>{StmSaveState::Saved});
}

TEST_CASE("session: the STM save: no link -> Unavailable at once; no answer -> TimedOut at 10 s") {
  Rig down(3);
  down.start();
  down.run(3000);  // still in the start-up hold: the STM does not answer yet
  down.save = StmSaveState::Waiting;
  down.run(1000);
  CHECK(down.port.saves == std::vector<StmSaveState>{StmSaveState::Unavailable});
  CHECK(down.count("eepst") == 0);

  Rig r(3);
  r.start();
  r.run(10000);
  r.stm.eep = 0;
  r.save = StmSaveState::Waiting;
  r.runUntil([&] { return r.count("eepst") > 0; }, 2000);
  const uint32_t t0 = r.now;
  r.runUntil([&] { return !r.port.saves.empty(); }, 12000);
  REQUIRE(r.port.saves == std::vector<StmSaveState>{StmSaveState::TimedOut});
  CHECK(r.now - t0 >= 9000);
  const std::vector<Event> ev = r.port.withCode(EventCode::StmEepromWaitTimeout);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 >= 10000);
  CHECK(ev[0].arg2 == 3);
}

// ================================================================ link recovery (E8)

TEST_CASE("session: a link interruption on protocol 3 checks gstax and is no reboot") {
  Rig r(3);
  r.start();
  r.run(15000);
  const size_t gproto = r.count("gproto");
  r.stm.silent = true;
  r.run(8000);
  REQUIRE(r.port.has(EventCode::LinkDown));
  const size_t gstax = r.count("gstax");
  r.stm.silent = false;
  r.run(3000);
  CHECK(r.port.has(EventCode::LinkUp));
  CHECK(r.count("gstax") >= gstax + 1);
  CHECK_FALSE(r.port.has(EventCode::StmRebootDetected));
  CHECK(r.count("gproto") == gproto);
}

TEST_CASE("session: an STM reset while the link was down is detected by the uptime") {
  Rig r(3);
  r.start();
  r.run(15000);
  r.stm.silent = true;
  r.run(8000);
  r.stm.reset(r.now);
  r.stm.silent = false;
  r.run(5000);
  const std::vector<Event> ev = r.port.withCode(EventCode::StmRebootDetected);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 1);
  CHECK(r.count("gproto") == 2);
}

TEST_CASE("session: a link interruption on protocol 1 is a reboot") {
  Rig r(1);
  r.start();
  r.run(15000);
  r.stm.silent = true;
  r.run(8000);
  r.stm.silent = false;
  r.run(5000);
  const std::vector<Event> ev = r.port.withCode(EventCode::StmRebootDetected);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == 4);
}

// ================================================================ sensors (E5, E9)

TEST_CASE("session: a sensor scan on a v1 STM matches the sensors 5 s later") {
  Rig r(1);
  r.start();
  r.run(15000);
  r.command(cmd(StmCommandType::ScanSensors));
  r.run(4000);
  CHECK(r.count("stons") == 1);
  CHECK(r.count("masns") == 0);
  r.run(2000);
  CHECK(r.count("masns") == 1);
  r.run(1000);
  CHECK(r.stm.linesOf("gvlon 255").size() >= 2);
}

TEST_CASE("session: no masns after a scan on protocol 2/3") {
  Rig r(3);
  r.start();
  r.run(10000);
  r.command(cmd(StmCommandType::ScanSensors));
  r.run(8000);
  CHECK(r.count("stons") == 1);
  CHECK(r.count("masns") == 0);
}

TEST_CASE("session: a goned reply with another id does not answer the request and lands by id") {
  Rig r(1);
  const std::string idA = "28-84-37-94-97-ff-03-23";
  const std::string idB = "28-aa-bb-cc-dd-ee-01-67";
  r.stm.answers["gonec"] = [&](const std::string& l) {
    return l == "gonec" ? std::string("gonec 2 ") : "gonec 2 " + idA + "," + idB + " ";
  };
  int calls = 0;
  r.stm.answers["goned"] = [&](const std::string& l) {
    ++calls;
    // bus 0 answers with sensor B's reading (a late reply for the other index)
    if (l == "goned 0") return "goned " + idB + " 201 ";
    return "goned " + idB + " 201 ";
  };
  r.start();
  r.run(40000);
  CHECK(calls > 0);
  CHECK(r.s.sensors().temp(1).raw == 201);
  CHECK(r.s.sensors().temp(1).seen);
  CHECK_FALSE(r.s.sensors().temp(0).seen);  // never answered with its own id
  CHECK(r.s.link().stats().strayLines > 0);
}

// ================================================================ protocol 3 commands (S3, S8, S9)

TEST_CASE("session: stop and safe-mode exit on protocol 3, dropped below") {
  Rig r(3, 0x007);
  r.start();
  r.run(10000);
  r.command(cmd(StmCommandType::StopValve, 2));
  r.run(2);
  CHECK(r.count("sstop 2") == 1);
  r.run(200);
  CHECK(r.count("gvlvy 2") >= 2);  // read back at once
  r.command(cmd(StmCommandType::StopValve, kAllValves));
  r.command(cmd(StmCommandType::LeaveSafeMode));
  r.run(500);
  CHECK(r.count("sstop 255") == 1);
  CHECK(r.count("ssafe 0") == 1);
  Rig old(2);
  old.start();
  old.run(10000);
  old.command(cmd(StmCommandType::StopValve, 2));
  old.command(cmd(StmCommandType::LeaveSafeMode));
  old.run(1000);
  CHECK(old.count("sstop") == 0);
  CHECK(old.count("ssafe") == 0);
}

TEST_CASE("session: the ESP schedule switches the STM time trigger off") {
  Rig r(3);
  r.cfg.calib.dayMask = 0x7F;
  r.cfg.calib.hour = 3;
  r.start();
  r.run(10000);
  CHECK(r.stm.linesOf("stlnt") == std::vector<std::string>{"stlnt 0"});
  CHECK(r.stm.learnTime == 0);
  CHECK(r.port.last.learnTimeS == 0);
  r.cfg.calib.dayMask = 0;
  r.s.applyConfig(r.cfg, true);
  r.run(2000);
  CHECK(r.stm.learnTime == 604800);
}

// ================================================================ flashing (W8, E4)

TEST_CASE("session: a flash of an image for another board fails in Validating, the STM untouched") {
  Rig r(3);
  r.start();
  r.run(10000);
  REQUIRE(std::string(r.port.last.version.hw) == "C2");
  std::vector<uint8_t> img(4096, 0x80);
  const uint32_t sp = 0x20020000u, pc = 0x080001C5u;
  memcpy(img.data(), &sp, 4);
  memcpy(img.data() + 4, &pc, 4);
  const char strs[] = "\x01" "DEADBEEF\0\x01" "BEEFIT\0\x01" "VDM-HW:C1";
  memcpy(img.data() + 2000, strs, sizeof strs);
  r.port.image.data = img;
  StmCommand c = cmd(StmCommandType::StartFlash);
  memcpy(c.image, "vdm.bin", 8);
  r.command(c);
  CHECK(r.port.flashMarks == 1);
  CHECK(r.s.flashPending());
  r.run(3000);
  CHECK_FALSE(r.s.flashing());
  CHECK_FALSE(r.s.flashPending());
  CHECK(r.count("eepst") >= 1);
  CHECK(r.port.opened == std::vector<std::string>{"vdm.bin"});
  CHECK(r.port.closed == 1);
  const std::vector<Event> ev = r.port.withCode(EventCode::StmFlashFailed);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].arg1 == static_cast<int32_t>(FlashError::BoardMismatch));
  CHECK(r.sim.resets.empty());
  CHECK(r.port.pulses.empty());
  CHECK(r.port.last.flash.error == FlashError::BoardMismatch);
}

TEST_CASE("session: flash requests: busy, restart pending, unreadable image, abort while waiting") {
  Rig r(3);
  r.start();
  r.run(10000);
  r.stm.eep = 0;
  StmCommand c = cmd(StmCommandType::StartFlash);
  memcpy(c.image, "a.bin", 6);
  r.command(c);
  REQUIRE(r.s.flashPending());
  r.command(c);  // a second one while waiting
  CHECK(std::string(r.port.withCode(EventCode::StmFlashFailed).back().text) == "busy");
  r.command(cmd(StmCommandType::AbortFlash));
  CHECK_FALSE(r.s.flashPending());
  r.run(12000);
  CHECK(r.port.opened.empty());
  r.port.restart = true;
  r.command(c);
  CHECK(std::string(r.port.withCode(EventCode::StmFlashFailed).back().text) == "restart pending");
  CHECK_FALSE(r.s.flashPending());
  r.port.restart = false;
  r.port.imageOk = false;
  c.blank = true;  // blank mode: no EEPROM wait
  r.command(c);
  CHECK(r.port.opened.size() == 1);
  CHECK(r.port.withCode(EventCode::StmFlashFailed).back().arg1 ==
        static_cast<int32_t>(FlashError::ImageRead));
  CHECK_FALSE(r.s.flashing());
}
