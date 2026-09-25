// LeaseClient: attempt counters, protocol changes, exact durations in events and status, restored
// records and the RTC record checks behind a valid CRC.
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

std::string glcfg(const LeaseConfig& c) {
  std::string s = "glcfg " + std::to_string(c.timeoutMin);
  for (uint8_t p : c.failsafePct) s += " " + std::to_string(p);
  return s;
}

struct Rig {
  LeaseClient lc;
  RequestLine last;
  std::string next(uint32_t now) {
    RequestLine r;
    if (!lc.next(now, r)) return "-";
    last = r;
    return text(r);
  }
  void ok(const std::string& line, uint32_t now) {
    const Reply r = reply(line);
    lc.onCompletion(last, Outcome::Ok, &r, now);
  }
  void timeout(uint32_t now) { lc.onCompletion(last, Outcome::Timeout, nullptr, now); }
  std::vector<Event> tick(uint32_t now) {
    Event e[8];
    const size_t n = lc.tick(now, e, 8);
    return std::vector<Event>(e, e + n);
  }
};

void startV3(Rig& r, uint32_t now) {
  r.lc.setProtocol(3, now);
  r.lc.setRegulator(RegulatorCause::Alive, 0, now);
  REQUIRE(r.next(now) == "slhbt 1");
  r.ok("slhbt 1 3600", now);
}

// a failed glcfg attempt at `now`: the heartbeat first when it is due
void failGlcfg(Rig& r, uint32_t now) {
  std::string n = r.next(now);
  if (n == "slhbt 1") {
    r.ok("slhbt 1 3600", now);
    n = r.next(now);
  }
  REQUIRE(n == "glcfg");
  r.timeout(now);
}

bool has(const std::vector<Event>& ev, EventCode code) {
  for (const Event& e : ev) {
    if (e.code == code) return true;
  }
  return false;
}

const Event* find(const std::vector<Event>& ev, EventCode code) {
  for (const Event& e : ev) {
    if (e.code == code) return &e;
  }
  return nullptr;
}

}  // namespace

TEST_CASE("lease: a glcfg that differs only in valve 0 is pushed") {
  Rig r;
  LeaseConfig want = allPct(60, 50);
  want.failsafePct[0] = 30;
  r.lc.setConfig(want);
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  r.ok(glcfg(allPct(60, 50)), 10);
  CHECK_FALSE(r.lc.status(10).configSynced);
  CHECK(r.next(10) == "sfspo 0 30");
}

TEST_CASE("lease: a config change starts the attempt count again") {
  Rig r;
  r.lc.setConfig(allPct(60, 50));
  startV3(r, 0);
  failGlcfg(r, 10);
  r.lc.setConfig(allPct(60, 40));
  failGlcfg(r, 20);
  failGlcfg(r, 20 + LeaseClient::kConfigRetryMs);
  CHECK_FALSE(r.lc.status(70000).configFailed);
  CHECK_FALSE(has(r.tick(70000), EventCode::LeaseConfigFailed));
  failGlcfg(r, 20 + 2 * LeaseClient::kConfigRetryMs);
  CHECK(r.lc.status(130000).configFailed);
}

TEST_CASE("lease: a success starts the attempt count again") {
  Rig r;
  r.lc.setConfig(allPct(60, 50));
  startV3(r, 0);
  failGlcfg(r, 10);
  failGlcfg(r, 10 + LeaseClient::kConfigRetryMs);
  REQUIRE(r.next(10 + 2 * LeaseClient::kConfigRetryMs) == "slhbt 1");
  r.ok("slhbt 1 3600", 10 + 2 * LeaseClient::kConfigRetryMs);
  REQUIRE(r.next(10 + 2 * LeaseClient::kConfigRetryMs) == "glcfg");
  r.ok(glcfg(allPct(60, 50)), 10 + 2 * LeaseClient::kConfigRetryMs);
  REQUIRE(r.lc.status(130000).configSynced);
  StmStatus s;  // gstax: the timeout drifted, a re-read without a config change
  s.v3 = true;
  s.leaseTimeoutMin = 0;
  r.lc.onStatus(s, 130000);
  failGlcfg(r, 130000);
  failGlcfg(r, 130000 + LeaseClient::kConfigRetryMs);
  CHECK_FALSE(r.lc.status(200000).configFailed);
}

TEST_CASE("lease: a protocol above 3 counts as 3") {
  Rig r;
  r.lc.setProtocol(4, 0);
  CHECK(r.lc.status(0).mode == LeaseMode::Stm);
  r.lc.setRegulator(RegulatorCause::Alive, 0, 0);
  CHECK(r.next(0) == "slhbt 1");
}

TEST_CASE("lease: only the return to protocol 3 starts a new session") {
  Rig r;
  r.lc.setConfig(allPct(60, 50));
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  r.ok(glcfg(allPct(60, 50)), 0);
  REQUIRE(r.lc.status(0).configSynced);
  r.lc.setProtocol(1, 1000);
  CHECK(r.lc.status(1000).configSynced);
  CHECK(r.next(1000) == "-");
  r.lc.setProtocol(3, 2000);
  CHECK(r.next(2000) == "slhbt 1");
  r.ok("slhbt 1 3600", 2000);
  CHECK(r.next(2000) == "glcfg");
}

TEST_CASE("lease: the cfgEvents of a rebooted STM are a new baseline") {
  Rig r;
  r.lc.setConfig(allPct(60, 50));
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  r.ok(glcfg(allPct(60, 50)), 0);
  StmStatus s;
  s.v3 = true;
  s.leaseTimeoutMin = 60;
  s.cfgEvents = 5;
  r.lc.onStatus(s, 1000);
  REQUIRE(r.lc.status(1000).configSynced);
  r.lc.onStmReboot();
  REQUIRE(r.next(2000) == "slhbt 1");
  r.ok("slhbt 1 3600", 2000);
  REQUIRE(r.next(2000) == "glcfg");
  r.ok(glcfg(allPct(60, 50)), 2000);
  s.cfgEvents = 0;  // counted again since the reboot
  r.lc.onStatus(s, 3000);
  CHECK(r.lc.status(3000).configSynced);
  CHECK(r.next(3000) == "-");
}

TEST_CASE("lease: exact seconds of the lost regulator in status, RegulatorBack and the snapshot") {
  Rig r;
  startV3(r, 0);
  r.lc.setRegulator(RegulatorCause::BrokerDown, 0, 0);
  CHECK(r.tick(60000).size() == 1);
  CHECK(r.lc.status(999000).regulatorLostS == 999);
  CHECK(r.lc.status(1998000).regulatorLostS == 1998);
  r.lc.setRegulator(RegulatorCause::Alive, 0, 999000);
  CHECK(r.lc.status(999000).regulatorLostS == 0);
  CHECK_FALSE(r.lc.snapshot(999000).lost);
  const std::vector<Event> ev = r.tick(999000);
  const Event* back = find(ev, EventCode::RegulatorBack);
  REQUIRE(back != nullptr);
  CHECK(back->arg1 == 999);
}

TEST_CASE("lease: a timeout of 1 min emulates") {
  LeaseClient lc;
  lc.setConfig(allPct(1, 50));
  lc.setProtocol(2, 0);
  CHECK(lc.status(0).mode == LeaseMode::Emulated);
  lc.setRegulator(RegulatorCause::BrokerDown, 0, 0);
  CHECK(lc.status(60000).state == LeaseState::Expired);
}

TEST_CASE("lease: a restored active record without a lost regulator drives its own mask") {
  LeaseClient::Snapshot s;
  s.active = true;
  s.mask = 0x0005;
  LeaseClient lc;
  lc.setConfig(allPct(60, 50));
  lc.restore(s, 100);
  CHECK(lc.status(100).state == LeaseState::Expired);
  CHECK(lc.emulatedMask(100) == 0x0005);
  CHECK(lc.status(100).failsafeMask == 0x0005);
  lc.setRegulator(RegulatorCause::Alive, 0, 100);
  CHECK(lc.emulatedMask(200) == 0x0005);
}

TEST_CASE("lease: exactly two differing valves with one wanted value are one sfspo 255") {
  Rig r;
  r.lc.setConfig(allPct(60, 40));
  startV3(r, 0);
  REQUIRE(r.next(0) == "glcfg");
  LeaseConfig stm = allPct(60, 40);
  stm.failsafePct[3] = 50;
  stm.failsafePct[9] = 20;
  r.ok(glcfg(stm), 10);
  CHECK(r.next(10) == "sfspo 255 40");
}

TEST_CASE("lease: a completion of another request of the same length is ignored") {
  Rig r;
  r.lc.setProtocol(3, 0);
  r.lc.setRegulator(RegulatorCause::Alive, 0, 0);
  REQUIRE(r.next(0) == "slhbt 1");
  RequestLine other;
  REQUIRE(buildHeartbeat(false, other));
  REQUIRE(other.len == r.last.len);
  const Reply rep = reply("slhbt 1 3600");
  r.lc.onCompletion(other, Outcome::Ok, &rep, 10);
  CHECK(r.next(20) == "-");
  r.ok("slhbt 1 3600", 30);
  CHECK(r.next(30) == "glcfg");
}

TEST_CASE("lease: FailsafeEnded once with the exact seconds, STM and emulation") {
  Rig r;
  startV3(r, 0);
  StmStatus s;
  s.v3 = true;
  s.lease = LeaseState::Expired;
  r.lc.onStatus(s, 1000);
  REQUIRE(r.tick(1000).size() == 1);
  s.lease = LeaseState::Running;
  r.lc.onStatus(s, 1000 + 999000);
  std::vector<Event> ev = r.tick(1000 + 999000);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].code == EventCode::FailsafeEnded);
  CHECK(ev[0].arg1 == 999);
  CHECK(r.tick(1000 + 999500).empty());

  Rig e;
  e.lc.setConfig(allPct(1, 50));
  e.lc.setProtocol(2, 0);
  e.lc.setRegulator(RegulatorCause::BrokerDown, 0, 0);
  ev = e.tick(60000);
  REQUIRE(has(ev, EventCode::FailsafeActive));
  e.lc.setRegulator(RegulatorCause::Alive, 1, 60000 + 999000);
  ev = e.tick(60000 + 999000);
  const Event* ended = find(ev, EventCode::FailsafeEnded);
  REQUIRE(ended != nullptr);
  CHECK(ended->arg1 == 999);
  CHECK(e.tick(60000 + 999500).empty());
}

TEST_CASE("lease: a restored loss of exactly 60 s was reported already") {
  LeaseClient::Snapshot s;
  s.lost = true;
  s.lostElapsedMs = LeaseClient::kRegulatorEventMs;
  LeaseClient lc;
  lc.restore(s, 100000);
  Event e[8];
  CHECK(lc.tick(100000, e, 8) == 0);
  s.lostElapsedMs = LeaseClient::kRegulatorEventMs - 1;
  LeaseClient lc2;
  lc2.restore(s, 100000);
  CHECK(lc2.tick(100001, e, 8) == 1);
}

TEST_CASE("lease: the RTC record checks every magic byte and each flag on its own") {
  LeaseClient::Snapshot s;
  s.lost = true;
  s.mask = 0x0003;
  s.lostElapsedMs = 1234;
  uint8_t b[kLeaseRecordSize];
  encodeLeaseRecord(s, b);
  for (size_t i = 0; i < 4; ++i) {
    uint8_t c[kLeaseRecordSize];
    memcpy(c, b, sizeof c);
    c[i] = 'X';
    const uint32_t crc = crc32(c, 12);
    c[12] = static_cast<uint8_t>(crc);
    c[13] = static_cast<uint8_t>(crc >> 8);
    LeaseClient::Snapshot out;
    CAPTURE(i);
    CHECK_FALSE(decodeLeaseRecord(c, sizeof c, out));
  }
  LeaseClient::Snapshot back;
  REQUIRE(decodeLeaseRecord(b, sizeof b, back));
  CHECK(back.lost);
  CHECK_FALSE(back.active);
  s.lost = false;
  s.active = true;
  encodeLeaseRecord(s, b);
  REQUIRE(decodeLeaseRecord(b, sizeof b, back));
  CHECK_FALSE(back.lost);
  CHECK(back.active);
}
