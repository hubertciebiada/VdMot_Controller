// LeaseClient: heartbeat, config sync with the STM, ESP emulation, status,
// events and the RTC record.
#include <string.h>

#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/lease_client.h"

using namespace vdm;

namespace {

Reply reply(const std::string& s) {
  Reply r;
  REQUIRE(parseReply(s.c_str(), s.size(), r) == ParseStatus::Ok);
  return r;
}

std::string text(const RequestLine& r) {
  std::string s(r.text, r.len);
  if (s.size() >= 3) s.resize(s.size() - 3);
  return s;
}

LeaseConfig allPct(uint16_t timeout, uint8_t pct) {
  LeaseConfig c;
  c.timeoutMin = timeout;
  for (uint8_t& p : c.failsafePct) p = pct;
  return c;
}

// glcfg reply text for a config.
std::string glcfg(const LeaseConfig& c) {
  std::string s = "glcfg " + std::to_string(c.timeoutMin);
  for (uint8_t p : c.failsafePct) s += " " + std::to_string(p);
  return s;
}

// Drives a client like the STM task: next() and scripted completions.
struct Rig {
  LeaseClient lc;
  RequestLine last;
  std::string next(uint32_t now) {
    RequestLine r;
    if (!lc.next(now, r)) {
      CHECK(r.len == 0);
      return "-";
    }
    last = r;
    return text(r);
  }
  // The periodic heartbeat, answered.
  void heartbeat(uint32_t now) {
    REQUIRE(next(now) == "slhbt 1");
    ok("slhbt 1 3600", now);
  }
  void ok(const std::string& line, uint32_t now) {
    const Reply r = reply(line);
    lc.onCompletion(last, Outcome::Ok, &r, now);
  }
  void rejected(const std::string& line, uint32_t now) {
    const Reply r = reply(line);
    lc.onCompletion(last, Outcome::Rejected, &r, now);
  }
  void timeout(uint32_t now) { lc.onCompletion(last, Outcome::Timeout, nullptr, now); }
  std::vector<Event> tick(uint32_t now) {
    Event e[8];
    const size_t n = lc.tick(now, e, 8);
    return std::vector<Event>(e, e + n);
  }
};

// Protocol 3 session with a first heartbeat answered at `now`.
void startV3(Rig& r, uint32_t now, RegulatorCause c = RegulatorCause::Alive) {
  r.lc.setProtocol(3, now);
  r.lc.setRegulator(c, 0, now);
  REQUIRE(r.next(now) == (c == RegulatorCause::Alive ? "slhbt 1" : "slhbt 0"));
  r.ok("slhbt 1 3600", now);
}

void checkEvent(const Event& e, EventCode code, int32_t a1, int32_t a2 = 0) {
  CHECK(e.code == code);
  CHECK(e.arg1 == a1);
  CHECK(e.arg2 == a2);
  CHECK(e.valve == kNoValve);
  CHECK(e.severity == eventDefaultSeverity(code));
}

}  // namespace

TEST_CASE("lease: constants") {
  CHECK(LeaseClient::kHeartbeatMs == 60000);
  CHECK(LeaseClient::kConfigCheckMs == 600000);
  CHECK(LeaseClient::kConfigRetryMs == 60000);
  CHECK(LeaseClient::kConfigMaxAttempts == 3);
  CHECK(LeaseClient::kRegulatorEventMs == 60000);
  CHECK(LeaseClient::kLostRequestMs == 10000);
  CHECK(LeaseClient::kRenewHoldMs == 120000);
  LeaseConfig c;
  CHECK(c.timeoutMin == 60);
  for (uint8_t p : c.failsafePct) CHECK(p == 50);
}

TEST_CASE("lease: effectiveLeaseConfig holds inactive valves; operator== per field") {
  Config cfg;
  LeaseConfig lc;
  effectiveLeaseConfig(cfg, lc);
  CHECK(lc.timeoutMin == 60);
  for (uint8_t p : lc.failsafePct) CHECK(p == kFailsafeHold);
  cfg.valves[3].active = true;
  cfg.valves[3].failsafePct = 40;
  cfg.valves[4].failsafePct = 10;  // inactive
  cfg.failsafe.timeoutMin = 5;
  effectiveLeaseConfig(cfg, lc);
  CHECK(lc.timeoutMin == 5);
  CHECK(lc.failsafePct[3] == 40);
  CHECK(lc.failsafePct[4] == kFailsafeHold);
  CHECK(lc.failsafePct[2] == kFailsafeHold);
  const LeaseConfig a = allPct(60, 50);
  LeaseConfig b = a;
  CHECK(a == b);
  CHECK_FALSE(a != b);
  b.timeoutMin = 61;
  CHECK_FALSE(a == b);
  for (uint8_t v = 0; v < kValveCount; ++v) {
    b = a;
    b.failsafePct[v] = 51;
    CAPTURE(int(v));
    CHECK(a != b);
  }
}

// ================================================================ heartbeat

TEST_CASE("lease: heartbeat every 60 s after the last one completed, at once on an alive change") {
  Rig r;
  r.lc.setProtocol(3, 0);
  r.lc.setRegulator(RegulatorCause::BrokerDown, 0, 0);
  CHECK(r.next(0) == "slhbt 0");
  CHECK(r.next(50) == "-");  // in flight
  r.ok("slhbt 1 3600", 100);
  CHECK(r.next(101) == "glcfg");  // config after the first answered heartbeat
  r.ok(glcfg(LeaseConfig{}), 150);
  CHECK(r.next(60099) == "-");
  CHECK(r.next(60100) == "slhbt 0");
  r.ok("slhbt 1 3500", 60200);
  r.lc.setRegulator(RegulatorCause::Alive, 0, 70000);
  CHECK(r.next(70000) == "slhbt 1");
  r.timeout(70400);  // no answer: the next one a period later
  CHECK(r.next(130399) == "-");
  CHECK(r.next(130400) == "slhbt 1");
  r.rejected("slhbt err", 130500);  // error form: also a period later
  CHECK(r.next(190499) == "-");
  CHECK(r.next(190500) == "slhbt 1");
}

TEST_CASE("lease: nothing on protocols 0, 1 and 2") {
  for (uint8_t p : {0, 1, 2}) {
    Rig r;
    r.lc.setProtocol(p, 0);
    r.lc.setRegulator(RegulatorCause::Alive, 0, 0);
    CHECK(r.next(0) == "-");
    CHECK(r.next(1000000) == "-");
  }
  Rig r;
  r.lc.setProtocol(7, 0);  // counts as 3
  CHECK(r.next(0) == "slhbt 0");
}

TEST_CASE("lease: a lost request counts as a timeout after 10 s") {
  Rig r;
  r.lc.setProtocol(3, 0);
  CHECK(r.next(0) == "slhbt 0");
  CHECK(r.next(9999) == "-");
  CHECK(r.next(10000) == "-");  // timed out now: the next heartbeat 60 s later
  CHECK(r.next(69999) == "-");
  CHECK(r.next(70000) == "slhbt 0");
}

TEST_CASE("lease: completions of other requests are ignored") {
  Rig r;
  r.lc.setProtocol(3, 0);
  CHECK(r.next(0) == "slhbt 0");
  RequestLine other;
  REQUIRE(buildHeartbeat(true, other));
  const Reply rep = reply("slhbt 2 0");
  r.lc.onCompletion(other, Outcome::Ok, &rep, 10);
  CHECK(r.next(20) == "-");  // still in flight
  r.lc.onCompletion(r.last, Outcome::Ok, &rep, 30);
  CHECK(r.lc.status(30).state == LeaseState::Expired);
}

TEST_CASE("lease: the heartbeat reply updates the STM lease state") {
  Rig r;
  r.lc.setProtocol(3, 0);
  r.lc.setRegulator(RegulatorCause::Alive, 0, 0);
  REQUIRE(r.next(0) == "slhbt 1");
  r.ok("slhbt 1 3540", 1000);
  LeaseStatus s = r.lc.status(1000);
  CHECK(s.mode == LeaseMode::Stm);
  CHECK(s.state == LeaseState::Running);
  CHECK(s.remainS == 3540);
  s = r.lc.status(1000 + 9999);
  CHECK(s.remainS == 3531);
  s = r.lc.status(1000 + 4000000);
  CHECK(s.remainS == 0);
}

// ================================================================ config sync

TEST_CASE("lease: an equal glcfg is synced; the next compare 10 min later") {
  Rig r;
  r.lc.setConfig(allPct(60, 50));
  startV3(r, 0);
  CHECK_FALSE(r.lc.status(0).configSynced);
  REQUIRE(r.next(0) == "glcfg");
  r.ok(glcfg(allPct(60, 50)), 100);
  CHECK(r.lc.status(100).configSynced);
  CHECK(r.next(200) == "-");
  // heartbeats keep going
  CHECK(r.next(60000) == "slhbt 1");
  r.ok("slhbt 1 3600", 60000);
  CHECK(r.next(600099) == "slhbt 1");
  r.ok("slhbt 1 3600", 600099);
  CHECK(r.next(600099) == "-");
  CHECK(r.next(600100) == "glcfg");
}

TEST_CASE("lease: push order slcfg, sfspo per valve, then verify") {
  Rig r;
  LeaseConfig want = allPct(60, 50);
  want.failsafePct[7] = 30;
  r.lc.setConfig(want);
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  LeaseConfig stm = allPct(0, 50);
  stm.failsafePct[2] = 20;
  r.ok(glcfg(stm), 10);
  CHECK(r.next(10) == "slcfg 60");
  r.ok("slcfg ok", 20);
  CHECK(r.next(20) == "sfspo 2 50");
  r.ok("sfspo 2 ok", 30);
  CHECK(r.next(30) == "sfspo 7 30");
  r.ok("sfspo 7 ok", 40);
  CHECK(r.next(40) == "glcfg");
  CHECK_FALSE(r.lc.status(40).configSynced);
  r.ok(glcfg(want), 50);
  CHECK(r.lc.status(50).configSynced);
}

TEST_CASE("lease: one sfspo 255 when all valves want one value and several differ") {
  Rig r;
  r.lc.setConfig(allPct(60, 40));
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  r.ok(glcfg(allPct(60, 50)), 10);
  CHECK(r.next(10) == "sfspo 255 40");
  r.ok("sfspo 255 ok", 20);
  CHECK(r.next(20) == "glcfg");
}

TEST_CASE("lease: one differing valve is a single sfspo even when all are equal") {
  Rig r;
  r.lc.setConfig(allPct(60, 40));
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  LeaseConfig stm = allPct(60, 40);
  stm.failsafePct[5] = 50;
  r.ok(glcfg(stm), 10);
  CHECK(r.next(10) == "sfspo 5 40");
  r.ok("sfspo 5 ok", 20);
  CHECK(r.next(20) == "glcfg");
}

TEST_CASE("lease: two differing valves with unequal targets are pushed one by one") {
  Rig r;
  LeaseConfig want = allPct(60, 40);
  want.failsafePct[11] = kFailsafeHold;
  r.lc.setConfig(want);
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  LeaseConfig stm = allPct(60, 50);
  stm.failsafePct[11] = kFailsafeHold;
  stm.failsafePct[1] = 40;
  r.ok(glcfg(stm), 10);
  std::vector<std::string> sent;
  for (int i = 0; i < 12; ++i) {
    const std::string s = r.next(10 + i);
    if (s == "glcfg") break;
    sent.push_back(s);
    r.ok("sfspo " + s.substr(6, s.find(' ', 6) - 6) + " ok", 10 + i);
  }
  CHECK(sent.size() == 10);  // valves 0, 2..10
  CHECK(sent.front() == "sfspo 0 40");
  CHECK(sent.back() == "sfspo 10 40");
}

TEST_CASE("lease: a failed attempt waits 60 s; the third one reports once, then every 10 min") {
  Rig r;
  r.lc.setConfig(allPct(60, 50));
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  r.ok(glcfg(allPct(0, 50)), 0);
  REQUIRE(r.next(0) == "slcfg 60");
  r.timeout(1000);  // attempt 1: no reply
  CHECK(r.tick(1000).empty());
  r.heartbeat(60000);
  CHECK(r.next(60999) == "-");
  REQUIRE(r.next(61000) == "glcfg");
  r.ok(glcfg(allPct(0, 50)), 61000);
  REQUIRE(r.next(61000) == "slcfg 60");
  r.rejected("slcfg err", 62000);  // attempt 2: rejected
  r.heartbeat(120000);
  CHECK(r.next(121999) == "-");
  REQUIRE(r.next(122000) == "glcfg");
  r.ok(glcfg(allPct(0, 50)), 122000);
  REQUIRE(r.next(122000) == "slcfg 60");
  r.ok("slcfg ok", 122000);
  REQUIRE(r.next(122000) == "glcfg");
  r.ok(glcfg(allPct(0, 50)), 123000);  // attempt 3: the read-back still differs
  CHECK(r.lc.status(123000).configFailed);
  std::vector<Event> ev = r.tick(123000);
  REQUIRE(ev.size() == 1);
  checkEvent(ev[0], EventCode::LeaseConfigFailed, 3, 3);
  CHECK(r.tick(124000).empty());
  // Later attempts every 10 min, no further event.
  const uint32_t t = 123000;
  for (uint32_t hb = 180000; hb < 723000; hb += 60000) r.heartbeat(hb);
  CHECK(r.next(722999) == "-");
  CHECK(r.next(723000) == "glcfg");
  r.ok(glcfg(allPct(0, 50)), t + 600000);
  REQUIRE(r.next(t + 600000) == "slcfg 60");
  r.timeout(t + 600000);
  CHECK(r.tick(t + 600000).empty());
  // A successful verify clears configFailed.
  r.lc.setConfig(allPct(0, 50));
  REQUIRE(r.next(t + 600001) == "glcfg");
  r.ok(glcfg(allPct(0, 50)), t + 600001);
  CHECK(r.lc.status(t + 600001).configSynced);
  CHECK_FALSE(r.lc.status(t + 600001).configFailed);
}

TEST_CASE("lease: a rejected sfspo ends the attempt with reason 2") {
  Rig r;
  r.lc.setConfig(allPct(60, 40));
  startV3(r, 0);
  for (int attempt = 0; attempt < 3; ++attempt) {
    const uint32_t t = static_cast<uint32_t>(attempt) * 60000;
    if (attempt > 0) {
      REQUIRE(r.next(t) == "slhbt 1");
      r.ok("slhbt 1 3600", t);
    }
    REQUIRE(r.next(t) == "glcfg");
    r.ok(glcfg(allPct(60, 50)), t);
    REQUIRE(r.next(t) == "sfspo 255 40");
    r.rejected("sfspo -1 err 1", t);
  }
  std::vector<Event> ev = r.tick(200000);
  REQUIRE(ev.size() == 1);
  checkEvent(ev[0], EventCode::LeaseConfigFailed, 2, 3);
}

TEST_CASE("lease: a glcfg timeout is a failed attempt with reason 1") {
  Rig r;
  r.lc.setConfig(allPct(60, 40));
  startV3(r, 0);
  for (int attempt = 0; attempt < 3; ++attempt) {
    const uint32_t t = static_cast<uint32_t>(attempt) * 60000;
    if (attempt > 0) {
      REQUIRE(r.next(t) == "slhbt 1");
      r.ok("slhbt 1 3600", t);
    }
    REQUIRE(r.next(t) == "glcfg");
    r.timeout(t);
  }
  std::vector<Event> ev = r.tick(200000);
  REQUIRE(ev.size() == 1);
  checkEvent(ev[0], EventCode::LeaseConfigFailed, 1, 3);
}

TEST_CASE("lease: re-read triggers") {
  Rig r;
  r.lc.setConfig(allPct(60, 50));
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  r.ok(glcfg(allPct(60, 50)), 0);
  REQUIRE(r.lc.status(0).configSynced);
  CHECK(r.next(1000) == "-");
  r.lc.setConfig(allPct(60, 50));  // the same value: nothing
  CHECK(r.next(1000) == "-");
  r.lc.setConfig(allPct(60, 40));
  CHECK_FALSE(r.lc.status(1000).configSynced);
  CHECK(r.next(1000) == "glcfg");
  r.ok(glcfg(allPct(60, 40)), 1000);
  // gstax: the timeout drifted.
  StmStatus s;
  s.v3 = true;
  s.leaseTimeoutMin = 60;
  r.lc.onStatus(s, 2000);
  CHECK(r.next(2000) == "-");
  s.leaseTimeoutMin = 0;
  r.lc.onStatus(s, 3000);
  CHECK(r.next(3000) == "glcfg");
  r.ok(glcfg(allPct(60, 40)), 3000);
  // gstax: cfgEvents changed (0 was the baseline above).
  s.leaseTimeoutMin = 60;
  r.lc.onStatus(s, 4000);
  CHECK(r.next(4000) == "-");
  s.cfgEvents = 1;
  r.lc.onStatus(s, 5000);
  CHECK(r.next(5000) == "glcfg");
  r.ok(glcfg(allPct(60, 40)), 5000);
  // A status without protocol 3 data is ignored.
  StmStatus old;
  old.leaseTimeoutMin = 0;
  r.lc.onStatus(old, 6000);
  CHECK(r.next(6000) == "-");
  // STM reboot: heartbeat first, then glcfg.
  r.lc.onStmReboot();
  CHECK(r.next(7000) == "slhbt 1");
  CHECK(r.next(7000) == "-");
  r.ok("slhbt 1 3600", 7000);
  CHECK(r.next(7000) == "glcfg");
}

TEST_CASE("lease: a drifting timeout is not re-read while unsynced (no retry storm)") {
  Rig r;
  r.lc.setConfig(allPct(60, 50));
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  r.ok(glcfg(allPct(0, 50)), 0);
  REQUIRE(r.next(0) == "slcfg 60");
  r.rejected("slcfg err", 0);
  StmStatus s;
  s.v3 = true;
  s.leaseTimeoutMin = 0;
  r.lc.onStatus(s, 10000);
  CHECK(r.next(10000) == "-");
}

TEST_CASE("lease: a config change drops the result of the glcfg in flight") {
  Rig r;
  r.lc.setConfig(allPct(60, 50));
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  const RequestLine old = r.last;
  r.lc.setConfig(allPct(60, 40));
  const Reply rep = reply(glcfg(allPct(60, 50)));
  r.lc.onCompletion(old, Outcome::Ok, &rep, 10);
  CHECK_FALSE(r.lc.status(10).configSynced);
  CHECK(r.next(10) == "glcfg");
}

TEST_CASE("lease: an untrusted config is compared but never pushed") {
  Rig r;
  r.lc.setConfig(allPct(60, 50));
  r.lc.setConfigTrusted(false);
  CHECK_FALSE(r.lc.status(0).configTrusted);
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  r.ok(glcfg(allPct(0, 50)), 0);
  CHECK(r.next(0) == "-");
  CHECK_FALSE(r.lc.status(0).configSynced);
  r.lc.setConfigTrusted(false);  // unchanged
  CHECK(r.next(0) == "-");
  r.lc.setConfigTrusted(true);
  CHECK(r.lc.status(0).configTrusted);
  REQUIRE(r.next(0) == "glcfg");
  r.ok(glcfg(allPct(0, 50)), 0);
  CHECK(r.next(0) == "slcfg 60");
}

TEST_CASE("lease: an untrusted config compares again 10 min later") {
  Rig r;
  r.lc.setConfigTrusted(false);
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  r.ok(glcfg(allPct(0, 50)), 0);
  for (uint32_t hb = 60000; hb <= 600000; hb += 60000) {
    REQUIRE(r.next(hb) == "slhbt 1");
    r.ok("slhbt 1 3600", hb);
    if (hb < 600000) CHECK(r.next(hb) == "-");
  }
  CHECK(r.next(600000) == "glcfg");
}

// ================================================================ renewal hold

TEST_CASE("lease: during an expired STM lease a regulator blip does not renew") {
  Rig r;
  startV3(r, 0, RegulatorCause::BrokerDown);
  StmStatus s;
  s.v3 = true;
  s.lease = LeaseState::Expired;
  s.failsafeMask = 0x00F;
  s.leaseTimeoutMin = 60;
  r.lc.onStatus(s, 1000);
  REQUIRE(r.next(1000) == "glcfg");
  r.ok(glcfg(LeaseConfig{}), 1000);
  r.lc.setRegulator(RegulatorCause::Alive, 0, 10000);
  CHECK(r.next(10000) == "-");
  r.lc.setRegulator(RegulatorCause::BrokerDown, 0, 40000);  // 30 s blip
  CHECK(r.next(40000) == "-");
  r.lc.setRegulator(RegulatorCause::Alive, 0, 50000);
  r.lc.setRegulator(RegulatorCause::Alive, 0, 50000 + 119999);
  CHECK(r.next(50000 + 119999) == "slhbt 0");  // periodic, still dead
  r.ok("slhbt 2 0", 50000 + 119999);
  r.lc.setRegulator(RegulatorCause::Alive, 0, 50000 + 120000);
  CHECK(r.next(50000 + 120000) == "slhbt 1");
  CHECK(r.lc.status(50000 + 120000).regulator == RegulatorCause::Alive);
}

TEST_CASE("lease: an MQTT command renews at once during the failsafe") {
  Rig r;
  startV3(r, 0, RegulatorCause::BrokerDown);
  StmStatus s;
  s.v3 = true;
  s.lease = LeaseState::Expired;
  r.lc.onStatus(s, 1000);
  REQUIRE(r.next(1000) == "glcfg");
  r.ok(glcfg(LeaseConfig{}), 1000);
  r.lc.setRegulator(RegulatorCause::Alive, 0, 2000);
  CHECK(r.next(2000) == "-");
  r.lc.setRegulator(RegulatorCause::Alive, 1, 3000);
  CHECK(r.next(3000) == "slhbt 1");
}

TEST_CASE("lease: without a failsafe the renewal is immediate") {
  Rig r;
  startV3(r, 0, RegulatorCause::HaOffline);
  REQUIRE(r.next(0) == "glcfg");
  r.ok(glcfg(LeaseConfig{}), 0);
  r.lc.setRegulator(RegulatorCause::Alive, 0, 5000);
  CHECK(r.next(5000) == "slhbt 1");
}

// ================================================================ emulation

TEST_CASE("lease: emulation after timeoutMin without the regulator, valves with a position") {
  LeaseClient lc;
  LeaseConfig c = allPct(60, kFailsafeHold);
  c.failsafePct[0] = 50;
  c.failsafePct[3] = 20;
  lc.setConfig(c);
  lc.setProtocol(1, 0);
  CHECK(lc.status(0).mode == LeaseMode::Emulated);
  lc.setRegulator(RegulatorCause::BrokerDown, 0, 1000);
  CHECK(lc.emulatedMask(3601000 - 1) == 0);
  CHECK(lc.emulatedMask(3601000) == 0x009);
  lc.setRegulator(RegulatorCause::BrokerDown, 0, 3602000);  // still dead: the timer keeps running
  CHECK(lc.emulatedMask(3602000) == 0x009);
  lc.setProtocol(0, 3603000);  // re-sync: the mode stays
  CHECK(lc.status(3603000).mode == LeaseMode::Emulated);
  CHECK(lc.emulatedMask(3603000) == 0x009);
  lc.setProtocol(2, 3603000);
  lc.setRegulator(RegulatorCause::Alive, 1, 3604000);  // a command renews at once
  CHECK(lc.emulatedMask(3604000) == 0);
  lc.setProtocol(3, 3604000);
  lc.setRegulator(RegulatorCause::BrokerDown, 1, 3605000);
  CHECK(lc.emulatedMask(3605000 + 3600000) == 0);
  CHECK(lc.status(3605000).mode == LeaseMode::Stm);
}

TEST_CASE("lease: timeout 0 never emulates; no protocol ever known is mode None") {
  LeaseClient lc;
  lc.setConfig(allPct(0, 50));
  lc.setProtocol(1, 0);
  lc.setRegulator(RegulatorCause::BrokerDown, 0, 0);
  CHECK(lc.emulatedMask(100000000) == 0);
  CHECK(lc.status(0).mode == LeaseMode::None);
  CHECK(lc.status(0).state == LeaseState::Off);
  LeaseClient fresh;
  fresh.setRegulator(RegulatorCause::BrokerDown, 0, 0);
  CHECK(fresh.status(0).mode == LeaseMode::None);
  CHECK(fresh.emulatedMask(100000000) == 0);
}

TEST_CASE("lease: the lost timer starts at the first dead call; alive first starts none") {
  LeaseClient lc;
  lc.setConfig(allPct(5, 50));
  lc.setProtocol(2, 0);
  lc.setRegulator(RegulatorCause::Alive, 0, 0);
  CHECK(lc.emulatedMask(10000000) == 0);
  lc.setRegulator(RegulatorCause::HaOffline, 0, 1000);
  lc.setRegulator(RegulatorCause::BrokerDown, 0, 2000);  // another dead cause keeps the timer
  CHECK(lc.emulatedMask(1000 + 299999) == 0);
  CHECK(lc.emulatedMask(1000 + 300000) == 0x0FFF);
  CHECK(lc.status(1000 + 300000).regulator == RegulatorCause::BrokerDown);
}

TEST_CASE("lease: emulation renews only after 120 s of life or a command") {
  LeaseClient lc;
  lc.setConfig(allPct(5, 50));
  lc.setProtocol(1, 0);
  lc.setRegulator(RegulatorCause::BrokerDown, 0, 0);
  REQUIRE(lc.emulatedMask(300000) != 0);
  lc.setRegulator(RegulatorCause::Alive, 0, 300000);
  CHECK(lc.emulatedMask(300000) != 0);
  lc.setRegulator(RegulatorCause::Alive, 0, 300000 + 119999);
  CHECK(lc.emulatedMask(300000 + 119999) != 0);
  lc.setRegulator(RegulatorCause::Alive, 0, 300000 + 120000);
  CHECK(lc.emulatedMask(300000 + 120000) == 0);
}

TEST_CASE("lease: status of the emulation") {
  LeaseClient lc;
  lc.setConfig(allPct(5, 50));
  lc.setProtocol(1, 0);
  lc.setRegulator(RegulatorCause::Alive, 0, 0);
  LeaseStatus s = lc.status(0);
  CHECK(s.state == LeaseState::Running);
  CHECK(s.remainS == 300);
  CHECK(s.regulatorLostS == 0);
  CHECK(s.timeoutMin == 5);
  CHECK(s.regulator == RegulatorCause::Alive);
  lc.setRegulator(RegulatorCause::HaOffline, 0, 1000);
  s = lc.status(1000 + 120500);
  CHECK(s.regulatorLostS == 120);
  CHECK(s.remainS == 180);
  CHECK(s.regulator == RegulatorCause::HaOffline);
  CHECK(s.failsafeMask == 0);
  s = lc.status(1000 + 300000);
  CHECK(s.state == LeaseState::Expired);
  CHECK(s.remainS == 0);
  CHECK(s.failsafeMask == 0x0FFF);
}

TEST_CASE("lease: status of the STM lease from gstax") {
  Rig r;
  startV3(r, 0);
  StmStatus s;
  s.v3 = true;
  s.lease = LeaseState::Running;
  s.leaseRemainS = 100;
  s.failsafeMask = 0x003;
  r.lc.onStatus(s, 1000);
  LeaseStatus st = r.lc.status(1000 + 30999);
  CHECK(st.state == LeaseState::Running);
  CHECK(st.remainS == 70);
  CHECK(st.failsafeMask == 0x003);
  st = r.lc.status(1000 + 100000);
  CHECK(st.remainS == 0);
  s.lease = LeaseState::Expired;
  r.lc.onStatus(s, 2000);
  st = r.lc.status(2000);
  CHECK(st.state == LeaseState::Expired);
  CHECK(st.remainS == 0);
}

// ================================================================ events

TEST_CASE("lease: RegulatorLost at 60 s once, RegulatorBack after it") {
  Rig r;
  r.lc.setRegulator(RegulatorCause::Alive, 0, 0);
  r.lc.setRegulator(RegulatorCause::HaOffline, 0, 1000);
  CHECK(r.tick(1000 + 59999).empty());
  std::vector<Event> ev = r.tick(1000 + 60000);
  REQUIRE(ev.size() == 1);
  checkEvent(ev[0], EventCode::RegulatorLost, 2);
  CHECK(r.tick(1000 + 70000).empty());
  r.lc.setRegulator(RegulatorCause::Alive, 0, 1000 + 90500);
  ev = r.tick(1000 + 91000);
  REQUIRE(ev.size() == 1);
  checkEvent(ev[0], EventCode::RegulatorBack, 90);
  CHECK(r.tick(1000 + 92000).empty());
  // A short loss is silent both ways.
  r.lc.setRegulator(RegulatorCause::BrokerDown, 0, 200000);
  CHECK(r.tick(250000).empty());
  r.lc.setRegulator(RegulatorCause::Alive, 0, 255000);
  CHECK(r.tick(400000).empty());
}

TEST_CASE("lease: FailsafeActive/Ended of the emulation, also with an all-hold config") {
  Rig r;
  r.lc.setConfig(allPct(5, kFailsafeHold));
  r.lc.setProtocol(1, 0);
  r.lc.setRegulator(RegulatorCause::BrokerDown, 0, 0);
  CHECK(r.tick(299999).size() == 1);  // RegulatorLost only
  std::vector<Event> ev = r.tick(300000);
  REQUIRE(ev.size() == 1);
  checkEvent(ev[0], EventCode::FailsafeActive, 0, 2);
  CHECK(r.tick(301000).empty());
  r.lc.setRegulator(RegulatorCause::Alive, 1, 360000);
  ev = r.tick(360500);
  REQUIRE(ev.size() == 2);
  checkEvent(ev[0], EventCode::RegulatorBack, 360);
  checkEvent(ev[1], EventCode::FailsafeEnded, 60, 2);
}

TEST_CASE("lease: FailsafeActive/Ended of the STM lease") {
  Rig r;
  startV3(r, 0);
  StmStatus s;
  s.v3 = true;
  s.lease = LeaseState::Expired;
  s.failsafeMask = 0x00F;
  r.lc.onStatus(s, 1000);
  std::vector<Event> ev = r.tick(1000);
  REQUIRE(ev.size() == 1);
  checkEvent(ev[0], EventCode::FailsafeActive, 0x00F, 1);
  CHECK(r.tick(2000).empty());
  s.lease = LeaseState::Running;
  r.lc.onStatus(s, 61000);
  ev = r.tick(61500);
  REQUIRE(ev.size() == 1);
  checkEvent(ev[0], EventCode::FailsafeEnded, 60, 1);
}

TEST_CASE("lease: tick respects the output bounds") {
  LeaseClient lc;
  lc.setRegulator(RegulatorCause::BrokerDown, 0, 0);
  CHECK(lc.tick(60000, nullptr, 8) == 0);
  Event e[1];
  LeaseClient lc2;
  lc2.setRegulator(RegulatorCause::BrokerDown, 0, 0);
  CHECK(lc2.tick(60000, e, 0) == 0);
}

// ================================================================ restarts

TEST_CASE("lease: snapshot and restore continue the lost timer") {
  LeaseClient before;
  before.setConfig(allPct(60, 50));
  before.setProtocol(1, 0);
  before.setRegulator(RegulatorCause::BrokerDown, 0, 0);
  const LeaseClient::Snapshot s = before.snapshot(50 * 60000);
  CHECK(s.lost);
  CHECK_FALSE(s.active);
  CHECK(s.mask == 0);
  CHECK(s.lostElapsedMs == 50u * 60000u);
  LeaseClient after;
  after.setConfig(allPct(60, 50));
  after.restore(s, 1000);
  after.setRegulator(RegulatorCause::BrokerDown, 0, 1000);
  after.setProtocol(1, 2000);
  CHECK(after.emulatedMask(1000 + 10 * 60000 - 1) == 0);
  CHECK(after.emulatedMask(1000 + 10 * 60000) == 0x0FFF);
  Event e[8];
  CHECK(after.tick(1000 + 10 * 60000, e, 8) == 1);  // FailsafeActive; the loss was reported before
  CHECK(e[0].code == EventCode::FailsafeActive);
}

TEST_CASE("lease: an active record drives its mask at once without a second event") {
  LeaseClient before;
  before.setConfig(allPct(5, 50));
  before.setProtocol(2, 0);
  before.setRegulator(RegulatorCause::BrokerDown, 0, 0);
  const LeaseClient::Snapshot s = before.snapshot(400000);
  CHECK(s.active);
  CHECK(s.mask == 0x0FFF);
  LeaseClient after;
  after.setConfig(allPct(5, 50));
  after.restore(s, 100);
  CHECK(after.emulatedMask(100) == 0x0FFF);
  CHECK(after.status(100).mode == LeaseMode::Emulated);
  after.setRegulator(RegulatorCause::BrokerDown, 0, 100);
  Event e[8];
  CHECK(after.tick(200, e, 8) == 0);
  after.setProtocol(2, 300);
  CHECK(after.emulatedMask(300) == 0x0FFF);
  CHECK(after.tick(400, e, 8) == 0);
  LeaseClient alive;
  alive.setRegulator(RegulatorCause::Alive, 0, 0);
  const LeaseClient::Snapshot a = alive.snapshot(1000);
  CHECK_FALSE(a.lost);
  CHECK(a.lostElapsedMs == 0);
}

TEST_CASE("lease: the RTC record round trip and its rejections") {
  LeaseClient::Snapshot s;
  s.lost = true;
  s.active = true;
  s.mask = 0x0A05;
  s.lostElapsedMs = 0x01020304;
  uint8_t b[kLeaseRecordSize];
  CHECK(encodeLeaseRecord(s, b) == kLeaseRecordSize);
  CHECK(memcmp(b, "VDLE\x01\x01\x05\x0A\x04\x03\x02\x01", 12) == 0);
  const uint32_t crc = crc32(b, 12);
  CHECK(b[12] == static_cast<uint8_t>(crc));
  CHECK(b[13] == static_cast<uint8_t>(crc >> 8));
  LeaseClient::Snapshot back;
  REQUIRE(decodeLeaseRecord(b, sizeof b, back));
  CHECK(back.lost);
  CHECK(back.active);
  CHECK(back.mask == 0x0A05);
  CHECK(back.lostElapsedMs == 0x01020304u);
  for (size_t i = 0; i < kLeaseRecordSize; ++i) {
    for (int bit = 0; bit < 8; ++bit) {
      uint8_t c[kLeaseRecordSize];
      memcpy(c, b, sizeof c);
      c[i] = static_cast<uint8_t>(c[i] ^ (1u << bit));
      LeaseClient::Snapshot out = s;
      CAPTURE(i);
      CAPTURE(bit);
      CHECK_FALSE(decodeLeaseRecord(c, sizeof c, out));
      CHECK_FALSE(out.lost);
      CHECK(out.mask == 0);
    }
  }
  CHECK_FALSE(decodeLeaseRecord(b, 13, back));
  CHECK_FALSE(decodeLeaseRecord(nullptr, 14, back));
  uint8_t d[kLeaseRecordSize];
  memcpy(d, b, sizeof d);
  d[4] = 2;  // flag byte above 1 with a matching CRC
  const uint32_t c2 = crc32(d, 12);
  d[12] = static_cast<uint8_t>(c2);
  d[13] = static_cast<uint8_t>(c2 >> 8);
  CHECK_FALSE(decodeLeaseRecord(d, sizeof d, back));
  d[4] = 1;
  d[5] = 2;
  const uint32_t c3 = crc32(d, 12);
  d[12] = static_cast<uint8_t>(c3);
  d[13] = static_cast<uint8_t>(c3 >> 8);
  CHECK_FALSE(decodeLeaseRecord(d, sizeof d, back));
  s = LeaseClient::Snapshot{};
  encodeLeaseRecord(s, b);
  REQUIRE(decodeLeaseRecord(b, sizeof b, back));
  CHECK_FALSE(back.lost);
  CHECK_FALSE(back.active);
}
