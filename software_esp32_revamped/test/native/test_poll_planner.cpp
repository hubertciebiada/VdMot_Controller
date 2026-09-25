// PollPlanner: re-sync sequence (v1/v2), one-shots, periodic cadence,
// priorities, retries and lost requests.
#include <stdint.h>
#include <string.h>

#include <algorithm>
#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/poll_planner.h"
#include "vdm/version.h"

using namespace vdm;

namespace {

std::string text(const RequestLine& r) {
  std::string s(r.text, r.len);
  // Drop the " \r\n" tail for readable comparisons.
  if (s.size() >= 3) s.resize(s.size() - 3);
  return s;
}

std::string next(PollPlanner& p, uint32_t now) {
  RequestLine r;
  if (!p.next(now, r)) {
    CHECK(r.len == 0);
    return "-";
  }
  CHECK(r.len > 0);
  return text(r);
}

// Cadence where every period is the same, for tie-order tests.
PollCadence flat(uint16_t ms) {
  PollCadence c;
  c.valveBusyMs = ms;
  c.valveActiveMs = ms;
  c.valveInactiveMs = ms;
  c.tempDataMs = ms;
  c.voltDataMs = ms;
  c.sensorCountMs = ms;
  c.statusMs = ms;
  c.versionMs = ms;
  return c;
}

// Takes everything that is due at `now` (the initial burst of valve reads).
std::vector<std::string> drain(PollPlanner& p, uint32_t now) {
  std::vector<std::string> out;
  for (int guard = 0; guard < 200; ++guard) {
    RequestLine r;
    if (!p.next(now, r)) return out;
    out.push_back(text(r));
    if (p.lastWasResync()) p.onResult(r, true, now);
  }
  FAIL("planner never ran dry");
  return out;
}

// Runs the re-sync to completion answering every step with `ok` (the
// planner's own gproto handling aside); returns only the step lines.
std::vector<std::string> runResync(PollPlanner& p, uint32_t& now, uint8_t protoReply) {
  std::vector<std::string> steps;
  for (int guard = 0; guard < 500 && p.resyncActive(); ++guard) {
    RequestLine r;
    if (!p.next(now, r)) {
      now += 100;
      continue;
    }
    if (!p.lastWasResync()) continue;  // periodic item: ignored
    steps.push_back(text(r));
    if (r.cmd == Cmd::Gproto) {
      if (protoReply) p.setProtocol(protoReply);
      p.onResult(r, protoReply != 0, now);
    } else {
      p.onResult(r, true, now);
    }
  }
  REQUIRE_FALSE(p.resyncActive());
  return steps;
}

std::vector<std::string> v1Steps() {
  std::vector<std::string> s = {"gproto", "gvers",     "ghwin",     "gmotc",     "gtlnm",
                                "gonec 255", "gowvc 255", "gvlon 255", "gvlst"};
  for (int v = 0; v < 12; ++v) s.push_back("gtgtp " + std::to_string(v));
  return s;
}

Version ver(const char* s) {
  Version v;
  parseVersion(s, strlen(s), v);
  return v;
}

std::vector<std::string> v3Steps() {
  std::vector<std::string> s = {"gproto", "gvers", "ghwin", "gmotc", "gtlnm", "gcalx",
                                "gonec 255", "gowvc 255", "gvlon 255"};
  for (int v = 0; v < 12; ++v) s.push_back("gvlvy " + std::to_string(v));
  return s;
}

std::vector<std::string> v2Steps() {
  std::vector<std::string> s = {"gproto", "gvers", "ghwin", "gmotc", "gtlnm", "gcalx",
                                "gonec 255", "gowvc 255", "gvlon 255"};
  for (int v = 0; v < 12; ++v) s.push_back("gvlvx " + std::to_string(v));
  return s;
}

}  // namespace

TEST_CASE("planner: cadence defaults match the binding table") {
  PollCadence c;
  CHECK(c.valveBusyMs == 500);
  CHECK(c.valveActiveMs == 2000);
  CHECK(c.valveInactiveMs == 30000);
  CHECK(c.tempDataMs == 10000);
  CHECK(c.voltDataMs == 10000);
  CHECK(c.sensorCountMs == 30000);
  CHECK(c.statusMs == 10000);
  CHECK(c.versionMs == 300000);
  PollPlanner p;
  CHECK(p.protocol() == 0);
  CHECK_FALSE(p.resyncActive());
  CHECK(p.resyncStep() == ResyncStep::Done);
  CHECK_FALSE(p.lastWasResync());
}

TEST_CASE("planner: fresh planner reads every valve once, then waits") {
  PollPlanner p;
  std::vector<std::string> expect;
  for (int v = 0; v < 12; ++v) expect.push_back("gvlvd " + std::to_string(v));
  CHECK(drain(p, 1000) == expect);
  CHECK(next(p, 1000 + 29999) == "-");
  // Sensor counts are first due one period after start; on a tie they
  // precede inactive valves.
  CHECK(next(p, 1000 + 30000) == "gonec");
  CHECK(next(p, 1000 + 30000) == "gowvc");
  CHECK(next(p, 1000 + 30000) == "gvlvd 0");
  CHECK(next(p, 1000 + 30000) == "gvlvd 1");
}

TEST_CASE("planner: valve classes and periods") {
  PollPlanner p;
  p.setActiveMask(0x0002);     // valve 1 active
  p.setValveBusy(2, true);     // valve 2 busy (inactive)
  p.setValveBusy(1, false);
  const uint32_t t0 = 5000;
  std::vector<std::string> first = drain(p, t0);
  REQUIRE(first.size() == 12);
  // Most overdue first: primed as due for 30 s, so the busy (500 ms) and the
  // active (2 s) valve lead.
  CHECK(first[0] == "gvlvd 2");
  CHECK(first[1] == "gvlvd 1");
  CHECK(first[2] == "gvlvd 0");
  CHECK(next(p, t0 + 499) == "-");
  CHECK(next(p, t0 + 500) == "gvlvd 2");
  CHECK(next(p, t0 + 999) == "-");
  CHECK(next(p, t0 + 1000) == "gvlvd 2");
  CHECK(next(p, t0 + 1500) == "gvlvd 2");
  CHECK(next(p, t0 + 1999) == "-");
  // Both exactly due: busy before active.
  CHECK(next(p, t0 + 2000) == "gvlvd 2");
  CHECK(next(p, t0 + 2000) == "gvlvd 1");
  CHECK(next(p, t0 + 2000) == "-");
}

TEST_CASE("planner: busy wins a tie with active, and goes back to its class when idle") {
  PollPlanner p;
  p.setActiveMask(0x0003);
  p.setValveBusy(1, true);
  drain(p, 0);
  // At 2000 both valves are exactly due by class (valve 1 busy is 1500 overdue).
  CHECK(next(p, 2000) == "gvlvd 1");
  CHECK(next(p, 2000) == "gvlvd 0");
  p.setValveBusy(1, false);
  CHECK(next(p, 3999) == "-");
  CHECK(next(p, 4000) == "gvlvd 0");
  CHECK(next(p, 4000) == "gvlvd 1");
  // Invalid valve indices are ignored.
  p.setValveBusy(12, true);
  p.setValveBusy(255, true);
  CHECK(next(p, 4499) == "-");
}

TEST_CASE("planner: active mask ignores bits above valve 11") {
  PollPlanner p;
  p.setActiveMask(0xF000);
  drain(p, 0);
  CHECK(next(p, 2000) == "-");
  CHECK(next(p, 29999) == "-");
}

TEST_CASE("planner: v2 uses gvlvx and polls gstat") {
  PollPlanner p;
  p.setProtocol(2);
  CHECK(p.protocol() == 2);
  std::vector<std::string> first = drain(p, 0);
  REQUIRE(first.size() == 13);
  CHECK(first[0] == "gstat");  // tie with inactive valves: gstat first
  CHECK(first[1] == "gvlvx 0");
  CHECK(first[12] == "gvlvx 11");
  CHECK(next(p, 9999) == "-");
  CHECK(next(p, 10000) == "gstat");
  p.setProtocol(7);
  CHECK(p.protocol() == 3);
  p.setProtocol(1);
  CHECK(p.protocol() == 1);
  CHECK(next(p, 20000) == "-");  // no gstat on v1
  CHECK(next(p, 30000) == "gonec");
  CHECK(next(p, 30000) == "gowvc");
  CHECK(next(p, 30000) == "gvlvd 0");
}

TEST_CASE("planner: tie order busy, active, gstat, goned, gowvd, counts, inactive, gvers") {
  PollPlanner p(flat(1000));
  p.setProtocol(2);
  p.setSensorCounts(1, 1);
  p.setActiveMask(1u << 1);
  p.setValveBusy(2, true);
  const uint32_t t0 = 100;
  std::vector<std::string> expect0 = {"gvlvx 2", "gvlvx 1", "gstat", "goned 0", "gowvd 0",
                                      "gvlvx 0"};
  for (int v = 3; v < 12; ++v) expect0.push_back("gvlvx " + std::to_string(v));
  CHECK(drain(p, t0) == expect0);
  std::vector<std::string> expect1 = {"gvlvx 2", "gvlvx 1", "gstat", "goned 0", "gowvd 0",
                                      "gonec",   "gowvc",   "gvlvx 0"};
  for (int v = 3; v < 12; ++v) expect1.push_back("gvlvx " + std::to_string(v));
  expect1.push_back("gvers");
  CHECK(drain(p, t0 + 1000) == expect1);
}

TEST_CASE("planner: the most overdue item wins over the tie order") {
  PollPlanner p(flat(1000));
  drain(p, 0);     // 12 valves
  drain(p, 1000);  // counts, 12 valves, gvers: everything handed out at 1000
  p.setActiveMask(1u << 5);
  CHECK(next(p, 2600) == "gvlvd 5");  // all 600 overdue: active first
  CHECK(next(p, 2700) == "gonec");    // 700 overdue, counts before inactive
  CHECK(next(p, 2700) == "gowvc");
  // Valve 5 (active) is 100 ms overdue, valve 0 (inactive) 1700 ms.
  CHECK(next(p, 3700) == "gvlvd 0");
  CHECK(next(p, 3700) == "gvlvd 1");
}

TEST_CASE("planner: DS18 and DS2438 round robin spread over the period") {
  PollPlanner p;
  p.setSensorCounts(3, 2);
  drain(p, 0);  // valves + goned 0 + gowvd 0
  // goned every 10000/3 = 3333 ms, gowvd every 5000 ms.
  CHECK(next(p, 3332) == "-");
  CHECK(next(p, 3333) == "goned 1");
  CHECK(next(p, 4999) == "-");
  CHECK(next(p, 5000) == "gowvd 1");
  CHECK(next(p, 6666) == "goned 2");
  CHECK(next(p, 9999) == "goned 0");
  CHECK(next(p, 10000) == "gowvd 0");
  // Shrinking the count wraps the index.
  p.setSensorCounts(1, 1);
  CHECK(next(p, 19999) == "goned 0");
  CHECK(next(p, 20000) == "gowvd 0");
  p.setSensorCounts(0, 0);
  CHECK(next(p, 29999) == "-");
}

TEST_CASE("planner: sensor counts are clamped") {
  PollPlanner p;
  p.setSensorCounts(200, 200);
  std::vector<std::string> first = drain(p, 0);
  // Primed one full period overdue: far more than their short spacing.
  CHECK(first[0] == "goned 0");
  CHECK(first[1] == "gowvd 0");
  CHECK(first.size() == 14);
  // 34 DS18 -> one goned every 294 ms, 8 DS2438 -> every 1250 ms.
  CHECK(next(p, 293) == "-");
  CHECK(next(p, 294) == "goned 1");
  uint32_t t = 294;
  for (int i = 2; i < 34; ++i) {
    t += 294;
    std::string s = next(p, t);
    if (s.rfind("gowvd", 0) == 0) s = next(p, t);
    CHECK(s == "goned " + std::to_string(i));
  }
  t += 294;
  std::string s = next(p, t);
  if (s.rfind("gowvd", 0) == 0) s = next(p, t);
  CHECK(s == "goned 0");  // wrapped after index 33
}

TEST_CASE("planner: gvers every 5 minutes") {
  PollPlanner p;
  drain(p, 0);
  bool seen = false;
  for (uint32_t t = 1000; t < 300000; t += 1000) {
    for (const std::string& s : drain(p, t)) CHECK(s != "gvers");
  }
  for (const std::string& s : drain(p, 299999)) CHECK(s != "gvers");
  for (const std::string& s : drain(p, 300000)) seen = seen || s == "gvers";
  CHECK(seen);
  for (const std::string& s : drain(p, 599999)) CHECK(s != "gvers");
  seen = false;
  for (const std::string& s : drain(p, 600000)) seen = seen || s == "gvers";
  CHECK(seen);
}

// ================================================================ resync

TEST_CASE("planner: v1 resync sequence") {
  PollPlanner p;
  p.requestResync();
  CHECK(p.resyncActive());
  CHECK(p.resyncStep() == ResyncStep::Proto);
  uint32_t now = 0;
  CHECK(runResync(p, now, 0) == v1Steps());
  CHECK(p.protocol() == 1);
  CHECK(p.resyncStep() == ResyncStep::Done);
}

TEST_CASE("planner: v2 resync sequence") {
  PollPlanner p;
  p.requestResync();
  uint32_t now = 0;
  CHECK(runResync(p, now, 2) == v2Steps());
  CHECK(p.protocol() == 2);
}

TEST_CASE("planner: gproto answered without a usable protocol keeps v1 commands") {
  PollPlanner p;
  p.requestResync();
  uint32_t now = 0;
  RequestLine r;
  REQUIRE(p.next(now, r));
  CHECK(r.cmd == Cmd::Gproto);
  p.onResult(r, true, now);  // caller did not call setProtocol
  CHECK(p.protocol() == 0);
  std::vector<std::string> rest = runResync(p, now, 0);
  std::vector<std::string> expect = v1Steps();
  expect.erase(expect.begin());
  CHECK(rest == expect);
}

TEST_CASE("planner: a gproto timeout does not downgrade a known protocol") {
  PollPlanner p;
  p.requestResync();
  RequestLine r;
  REQUIRE(p.next(0, r));
  p.setProtocol(2);  // e.g. a stray gproto reply was applied
  p.onResult(r, false, 0);
  CHECK(p.protocol() == 2);
  CHECK(p.resyncStep() == ResyncStep::Version);
}

TEST_CASE("planner: resync steps alternate with due items") {
  PollPlanner p;
  p.setActiveMask(0x0FFF);
  p.requestResync();
  std::vector<std::string> seq;
  std::vector<bool> isStep;
  for (int i = 0; i < 8; ++i) {
    RequestLine r;
    REQUIRE(p.next(0, r));
    seq.push_back(text(r));
    isStep.push_back(p.lastWasResync());
    if (p.lastWasResync()) {
      if (r.cmd == Cmd::Gproto) p.setProtocol(2);
      p.onResult(r, true, 0);
    }
  }
  // gproto and gvers go out alone, then steps alternate with valve polls.
  CHECK(seq == std::vector<std::string>{"gproto", "gvers", "gvlvx 0", "ghwin", "gvlvx 1",
                                        "gmotc", "gvlvx 2", "gtlnm"});
  CHECK(isStep == std::vector<bool>{true, true, false, true, false, true, false, true});
}

TEST_CASE("planner: steps go back to back when nothing else is due") {
  PollPlanner p;
  drain(p, 0);
  p.requestResync();
  RequestLine r;
  REQUIRE(p.next(1, r));
  CHECK(text(r) == "gproto");
  p.onResult(r, false, 1);
  REQUIRE(p.next(1, r));
  CHECK(text(r) == "gvers");
  CHECK(p.lastWasResync());
}

TEST_CASE("planner: a handed-out step is not repeated until its result") {
  PollPlanner p;
  drain(p, 0);
  p.requestResync();
  RequestLine r;
  REQUIRE(p.next(1, r));
  CHECK(text(r) == "gproto");
  CHECK(next(p, 2) == "-");
  CHECK(next(p, 1 + PollPlanner::kLostRequestMs - 1) == "-");
  // Lost (dropped by an STM reset or evicted): handed out again.
  CHECK(next(p, 1 + PollPlanner::kLostRequestMs) == "gproto");
  CHECK(p.lastWasResync());
}

TEST_CASE("planner: a failed step is retried after valveActiveMs") {
  PollPlanner p;
  drain(p, 0);
  p.requestResync();
  RequestLine r;
  REQUIRE(p.next(1, r));
  p.onResult(r, false, 1);  // gproto timeout -> v1, advance
  REQUIRE(p.next(1, r));
  CHECK(text(r) == "gvers");
  p.onResult(r, false, 10);
  CHECK(p.resyncStep() == ResyncStep::Version);
  CHECK(next(p, 2009) == "-");
  CHECK(next(p, 2010) == "gvers");
  p.onResult(r, true, 2010);
  CHECK(p.resyncStep() == ResyncStep::HwId);
}

TEST_CASE("planner: a failed Targets step retries the same valve") {
  PollPlanner p;
  uint32_t now = 0;
  drain(p, now);
  p.requestResync();
  RequestLine r;
  // Answer until the Targets step reaches valve 5.
  for (int guard = 0; guard < 50; ++guard) {
    REQUIRE(p.next(now, r));
    if (r.cmd == Cmd::Gtgtp && r.valve == 5) break;
    p.onResult(r, r.cmd != Cmd::Gproto, now);
  }
  REQUIRE(r.cmd == Cmd::Gtgtp);
  p.onResult(r, false, now);
  CHECK(p.resyncStep() == ResyncStep::Targets);
  CHECK(next(p, now + 1999) == "-");
  CHECK(next(p, now + 2000) == "gtgtp 5");
}

TEST_CASE("planner: results for other requests do not advance the resync") {
  PollPlanner p;
  drain(p, 0);
  p.requestResync();
  RequestLine r;
  REQUIRE(p.next(1, r));  // gproto
  RequestLine other;
  REQUIRE(buildGetVersion(other));
  p.onResult(other, true, 1);
  CHECK(p.resyncStep() == ResyncStep::Proto);
  REQUIRE(buildCalibrate(3, other));
  p.onResult(other, true, 1);
  CHECK(p.resyncStep() == ResyncStep::Proto);
  p.onResult(r, true, 1);
  CHECK(p.resyncStep() == ResyncStep::Version);
  // A duplicate result for the finished step changes nothing.
  p.onResult(r, true, 1);
  CHECK(p.resyncStep() == ResyncStep::Version);
}

TEST_CASE("planner: requestResync restarts the sequence and resets the protocol") {
  PollPlanner p;
  p.requestResync();
  uint32_t now = 0;
  runResync(p, now, 2);
  CHECK(p.protocol() == 2);
  p.requestProfile(3);
  p.requestTempList();
  p.requestTarget(4);
  p.requestResync();
  CHECK(p.protocol() == 0);
  CHECK(p.resyncStep() == ResyncStep::Proto);
  // Pending covered one-shots and v2-only one-shots were dropped.
  std::vector<std::string> steps = runResync(p, now, 0);
  CHECK(steps == v1Steps());
  for (int i = 0; i < 40; ++i) {
    const std::string s = next(p, now);
    CHECK(s != "gprof 3");
    CHECK(s != "gonec 255");
    CHECK(s != "gtgtp 4");
  }
}

TEST_CASE("planner: requestResync while a step is in flight") {
  PollPlanner p;
  drain(p, 0);
  p.requestResync();
  RequestLine r;
  REQUIRE(p.next(1, r));
  p.onResult(r, false, 1);
  REQUIRE(p.next(1, r));
  CHECK(text(r) == "gvers");
  p.requestResync();
  RequestLine r2;
  REQUIRE(p.next(2, r2));
  CHECK(text(r2) == "gproto");
  p.onResult(r, true, 3);  // the old gvers result is not the current step
  CHECK(p.resyncStep() == ResyncStep::Proto);
}

// ================================================================ one-shots

TEST_CASE("planner: one-shots are coalesced and wait for their result") {
  PollPlanner p;
  drain(p, 0);
  p.requestTempList();
  p.requestTempList();
  RequestLine r;
  REQUIRE(p.next(1, r));
  CHECK(text(r) == "gonec 255");
  CHECK_FALSE(p.lastWasResync());
  p.requestTempList();  // already in flight: nothing new
  CHECK(next(p, 2) == "-");
  p.onResult(r, true, 3);
  CHECK(next(p, 4) == "-");
  // After success a new request is served again.
  p.requestTempList();
  CHECK(next(p, 5) == "gonec 255");
}

TEST_CASE("planner: failed one-shot retried after valveActiveMs, lost after kLostRequestMs") {
  PollPlanner p;
  drain(p, 0);
  p.requestVoltList();
  RequestLine r;
  REQUIRE(p.next(100, r));
  CHECK(text(r) == "gowvc 255");
  p.onResult(r, false, 200);
  CHECK(next(p, 2199) == "-");
  CHECK(next(p, 2200) == "gowvc 255");
  CHECK(next(p, 2200 + PollPlanner::kLostRequestMs - 1).rfind("gowvc 255", 0) != 0);
  // Never answered: handed out again.
  PollPlanner q;
  drain(q, 0);
  q.requestValveSensors();
  CHECK(next(q, 100) == "gvlon 255");
  CHECK(next(q, 100 + PollPlanner::kLostRequestMs - 1) == "-");
  CHECK(next(q, 100 + PollPlanner::kLostRequestMs) == "gvlon 255");
  REQUIRE(q.next(100 + PollPlanner::kLostRequestMs, r) == false);
  RequestLine g;
  REQUIRE(buildValveSensors(kAllValves, g));
  q.onResult(g, true, 15000);
  CHECK(next(q, 15000 + PollPlanner::kLostRequestMs) == "-");
}

TEST_CASE("planner: one-shot priority: targets, profiles, lists, motor params") {
  PollPlanner p;
  p.setProtocol(2);
  drain(p, 0);
  p.requestMotorParams();
  p.requestValveSensors();
  p.requestVoltList();
  p.requestTempList();
  p.requestProfile(7);
  p.requestProfile(2);
  p.requestTarget(9);
  p.requestTarget(1);
  std::vector<std::string> got;
  for (int i = 0; i < 12; ++i) got.push_back(next(p, 1));
  CHECK(got == std::vector<std::string>{"gvlvx 1", "gvlvx 9", "gprof 2", "gprof 7", "gonec 255",
                                        "gowvc 255", "gvlon 255", "gmotc", "gtlnm", "gcalx", "-",
                                        "-"});
}

TEST_CASE("planner: one-shots precede periodic items") {
  PollPlanner p;
  p.setActiveMask(0x0FFF);
  p.requestTarget(3);
  CHECK(next(p, 0) == "gtgtp 3");
  CHECK(next(p, 0) == "gvlvd 0");
}

TEST_CASE("planner: v1 motor params and target read-backs") {
  PollPlanner p;
  p.setProtocol(1);
  drain(p, 0);
  p.requestMotorParams();
  p.requestTarget(11);
  p.requestTarget(12);   // ignored
  p.requestTarget(255);  // ignored
  CHECK(next(p, 1) == "gtgtp 11");
  CHECK(next(p, 1) == "gmotc");
  CHECK(next(p, 1) == "gtlnm");
  CHECK(next(p, 1) == "-");
}

TEST_CASE("planner: profiles only on v2") {
  PollPlanner p;
  drain(p, 0);
  p.requestProfile(3);  // proto unknown
  CHECK(next(p, 1) == "-");
  p.setProtocol(1);
  p.requestProfile(3);
  CHECK(next(p, 1) == "-");
  p.setProtocol(2);
  p.requestProfile(12);  // invalid
  p.requestProfile(3);
  p.requestProfile(4);
  CHECK(next(p, 1) == "gprof 3");
  // Protocol downgrade drops pending and in-flight v2 one-shots.
  p.requestMotorParams();
  p.setProtocol(1);
  CHECK(next(p, 1) == "gmotc");
  CHECK(next(p, 1) == "gtlnm");
  CHECK(next(p, 1 + PollPlanner::kLostRequestMs) == "gmotc");
  CHECK(next(p, 1 + PollPlanner::kLostRequestMs) == "gtlnm");
  CHECK(next(p, 1 + PollPlanner::kLostRequestMs) == "-");
}

TEST_CASE("planner: target read-back uses gvlvx on v2; a periodic gvlvx result also satisfies it") {
  PollPlanner p;
  p.setProtocol(2);
  drain(p, 0);
  p.requestTarget(4);
  RequestLine r;
  REQUIRE(p.next(1, r));
  CHECK(text(r) == "gvlvx 4");
  CHECK(r.cmd == Cmd::Gvlvx);
  RequestLine same;
  REQUIRE(buildValveEx(4, same));
  p.onResult(same, true, 2);
  for (const std::string& s : drain(p, 2 + PollPlanner::kLostRequestMs)) CHECK(s != "gvlvx 4");
}

TEST_CASE("planner: an unrelated failed result does not disturb one-shots") {
  PollPlanner p;
  drain(p, 0);
  p.requestTempList();
  RequestLine r;
  REQUIRE(p.next(1, r));
  RequestLine count;
  REQUIRE(buildTempCount(count));  // same cmd, different arg
  p.onResult(count, true, 2);
  CHECK(next(p, 3) == "-");  // still in flight ...
  CHECK(next(p, 1 + PollPlanner::kLostRequestMs) == "gonec 255");  // ... and still wanted
  p.onResult(r, true, 3);
  CHECK(next(p, 3 + 2 * PollPlanner::kLostRequestMs) == "-");
}

TEST_CASE("planner: out and lastWasResync on an idle call") {
  PollPlanner p;
  drain(p, 0);
  p.requestResync();
  RequestLine r;
  REQUIRE(p.next(1, r));
  CHECK(p.lastWasResync());
  RequestLine idle;
  REQUIRE(buildGetStatus(idle));
  CHECK_FALSE(p.next(2, idle));
  CHECK(idle.len == 0);
  CHECK(idle.cmd == Cmd::None);
  CHECK(idle.text[0] == '\0');
}

TEST_CASE("planner: works across the millis wrap") {
  PollPlanner p;
  p.setActiveMask(1);
  const uint32_t t0 = 0xFFFFFC00u;
  std::vector<std::string> first = drain(p, t0);
  CHECK(first.size() == 12);
  CHECK(next(p, t0 + 1999) == "-");
  CHECK(next(p, t0 + 2000) == "gvlvd 0");  // t0 + 2000 wrapped past 0
}

// ================================================================ edge cases

TEST_CASE("planner: setProtocol(2) again keeps pending v2 one-shots") {
  PollPlanner p;
  p.setProtocol(2);
  drain(p, 0);
  p.requestProfile(0);
  p.setProtocol(2);
  CHECK(next(p, 1) == "gprof 0");
}

TEST_CASE("planner: valve 0 as busy valve and as target/profile one-shot") {
  PollPlanner p;
  p.setValveBusy(0, true);
  drain(p, 0);
  CHECK(next(p, 499) == "-");
  CHECK(next(p, 500) == "gvlvd 0");
  p.setProtocol(1);
  p.requestTarget(0);
  RequestLine r;
  REQUIRE(p.next(501, r));
  CHECK(text(r) == "gtgtp 0");
  p.onResult(r, true, 502);
  CHECK(next(p, 502 + PollPlanner::kLostRequestMs) != "gtgtp 0");
}

TEST_CASE("planner: a result for a step that was never handed out is ignored") {
  PollPlanner p;
  drain(p, 0);
  p.requestResync();
  RequestLine g;
  REQUIRE(buildGetProto(g));
  p.onResult(g, false, 1);  // stale timeout from before the restart
  CHECK(p.protocol() == 0);
  CHECK(p.resyncStep() == ResyncStep::Proto);
  // After a step completes, a result for the next step arrives early.
  RequestLine r;
  REQUIRE(p.next(2, r));
  p.onResult(r, false, 3);
  CHECK(p.resyncStep() == ResyncStep::Version);
  RequestLine v;
  REQUIRE(buildGetVersion(v));
  p.onResult(v, true, 4);
  CHECK(p.resyncStep() == ResyncStep::Version);
}

TEST_CASE("planner: a late success for a failed step waits for the retry") {
  PollPlanner p;
  drain(p, 0);
  p.requestResync();
  RequestLine r;
  REQUIRE(p.next(1, r));
  p.onResult(r, false, 1);  // -> Version
  REQUIRE(p.next(2, r));
  CHECK(text(r) == "gvers");
  p.onResult(r, false, 3);  // failed, retry held
  p.onResult(r, true, 4);   // duplicate late reply: not in flight any more
  CHECK(p.resyncStep() == ResyncStep::Version);
  CHECK(next(p, 2003) == "gvers");
}

TEST_CASE("planner: an in-flight one-shot does not block later ones") {
  PollPlanner p;
  p.setProtocol(1);
  drain(p, 0);
  p.requestTarget(0);
  CHECK(next(p, 1) == "gtgtp 0");  // now held while in flight
  p.requestTarget(2);
  CHECK(next(p, 2) == "gtgtp 2");
  p.requestTarget(1);
  CHECK(next(p, 3) == "gtgtp 1");
  CHECK(next(p, 4) == "-");
}

TEST_CASE("planner: a shrinking sensor count restarts the round robin at index 0") {
  PollPlanner p(flat(60000));
  p.setSensorCounts(5, 5);
  // flat(): every item one period overdue at the first call; goned 0 first.
  std::vector<std::string> first = drain(p, 0);
  REQUIRE(std::count(first.begin(), first.end(), "goned 0") == 1);
  // Walk both round robins to index 3 with an ample period per index.
  uint32_t t = 0;
  for (int i = 1; i <= 3; ++i) {
    t += 12000;
    std::vector<std::string> due = drain(p, t);
    CHECK(std::count(due.begin(), due.end(), "goned " + std::to_string(i)) == 1);
    CHECK(std::count(due.begin(), due.end(), "gowvd " + std::to_string(i)) == 1);
  }
  // Shrink to exactly the next index: it is out of range now, so restart at 0.
  p.setSensorCounts(4, 4);
  t += 15000;
  std::vector<std::string> due = drain(p, t);
  CHECK(std::count(due.begin(), due.end(), "goned 0") == 1);
  CHECK(std::count(due.begin(), due.end(), "gowvd 0") == 1);
  CHECK(std::count(due.begin(), due.end(), "goned 4") == 0);
  CHECK(std::count(due.begin(), due.end(), "gowvd 4") == 0);
  // A count that still covers the index keeps it.
  p.setSensorCounts(3, 3);
  t += 20000;
  due = drain(p, t);
  CHECK(std::count(due.begin(), due.end(), "goned 1") == 1);
  CHECK(std::count(due.begin(), due.end(), "gowvd 1") == 1);
}

TEST_CASE("planner: protocol 3 polls gvlvy and gstax, reads back with gvlvy, re-syncs with gvlvy") {
  PollPlanner p;
  p.setProtocol(4);
  CHECK(p.protocol() == 3);
  p.setProtocol(3);
  std::vector<std::string> first = drain(p, 0);
  REQUIRE(first.size() == 13);
  CHECK(first[0] == "gstax");
  CHECK(first[1] == "gvlvy 0");
  CHECK(first[12] == "gvlvy 11");
  CHECK(next(p, 9999) == "-");
  CHECK(next(p, 10000) == "gstax");
  p.requestTarget(5);
  CHECK(next(p, 10000) == "gvlvy 5");
  p.requestResync();
  uint32_t now = 20000;
  CHECK(runResync(p, now, 3) == v3Steps());
  CHECK(p.protocol() == 3);
}

TEST_CASE("planner: gproto and gvers go out alone; valve polls wait for the version") {
  PollPlanner p;
  p.setActiveMask(0x0FFF);
  p.requestResync();
  RequestLine r;
  REQUIRE(p.next(0, r));
  CHECK(text(r) == "gproto");
  CHECK(p.lastWasResync());
  CHECK(next(p, 0) == "-");  // in flight: nothing else goes out
  CHECK(next(p, 9999) == "-");
  p.onResult(r, false, 100);  // probe timed out: protocol 1
  REQUIRE(p.next(100, r));
  CHECK(text(r) == "gvers");
  CHECK(next(p, 100) == "-");
  p.onResult(r, false, 200);  // failed: retried after valveActiveMs, nothing in between
  CHECK(next(p, 2199) == "-");
  REQUIRE(p.next(2200, r));
  CHECK(text(r) == "gvers");
  p.onResult(r, true, 2300);
  CHECK(next(p, 2300) == "gvlvd 0");
  CHECK(p.resyncStep() == ResyncStep::HwId);
}

TEST_CASE("planner: an STM below 1.4.0 ends the re-sync and gets gvers every 30 s") {
  PollPlanner p;
  p.setActiveMask(0x0FFF);
  CHECK(p.support() == StmSupport::Unknown);
  CHECK(PollCadence{}.unsupportedVersionMs == 30000);
  p.requestResync();
  RequestLine r;
  REQUIRE(p.next(0, r));
  p.onResult(r, false, 0);  // gproto: silent
  REQUIRE(p.next(0, r));
  CHECK(text(r) == "gvers");
  p.requestTempList();
  p.requestMotorParams();
  p.onVersion(ver("1.3.5_C2"));
  CHECK(p.support() == StmSupport::TooOld);
  CHECK_FALSE(p.resyncActive());
  p.onResult(r, true, 10);  // the gvers result of the finished step changes nothing
  CHECK_FALSE(p.resyncActive());
  CHECK(next(p, 10) == "-");  // one-shots dropped, no valve polls
  CHECK(next(p, 29999) == "-");
  CHECK(next(p, 30000) == "gvers");
  CHECK(next(p, 30000) == "-");
  CHECK(next(p, 59999) == "-");
  CHECK(next(p, 60000) == "gvers");
  // Only target read-backs are still taken.
  p.requestTempList();
  p.requestVoltList();
  p.requestValveSensors();
  p.requestMotorParams();
  p.requestProfile(1);
  p.requestStatus();
  p.requestMatchSensors(60000);
  CHECK(next(p, 70000) == "-");
  p.requestTarget(0);
  CHECK(next(p, 70000) == "gtgtp 0");
  // The same too old version again changes nothing.
  p.onVersion(ver("1.3.5_C2"));
  CHECK_FALSE(p.resyncActive());
  CHECK(p.support() == StmSupport::TooOld);
  // A supported version (after an update) restarts the re-sync.
  p.onVersion(ver("1.4.9_C2"));
  CHECK(p.resyncActive());
  CHECK(p.resyncStep() == ResyncStep::Proto);
  CHECK(p.support() == StmSupport::Unknown);
  CHECK(p.protocol() == 0);
}

TEST_CASE("planner: a supported version marks support and keeps the re-sync going") {
  PollPlanner p;
  p.requestResync();
  RequestLine r;
  REQUIRE(p.next(0, r));
  p.setProtocol(2);
  p.onResult(r, true, 0);
  REQUIRE(p.next(0, r));
  CHECK(text(r) == "gvers");
  p.onVersion(ver("2.0.0-revamped_C2"));
  CHECK(p.support() == StmSupport::Supported);
  CHECK(p.resyncStep() == ResyncStep::Version);
  p.onResult(r, true, 0);
  CHECK(p.resyncStep() == ResyncStep::HwId);
  p.onVersion(ver("garbage"));
  CHECK(p.support() == StmSupport::Unknown);
}

TEST_CASE("planner: a different version outside a re-sync restarts it, the same one does not") {
  PollPlanner p;
  p.onVersion(ver("2.0.0-revamped_C2"));  // first version ever: no re-sync
  CHECK_FALSE(p.resyncActive());
  p.onVersion(ver("2.0.0-revamped_C2"));
  CHECK_FALSE(p.resyncActive());
  p.onVersion(ver("garbage"));  // an unparsable version keeps the last text
  CHECK_FALSE(p.resyncActive());
  p.onVersion(ver("2.0.0-revamped_C2"));
  CHECK_FALSE(p.resyncActive());
  p.onVersion(ver("2.1.0-revamped_C2"));
  CHECK(p.resyncActive());
  // Inside the re-sync another change does not restart it again.
  RequestLine r;
  REQUIRE(p.next(0, r));
  p.onResult(r, false, 0);
  REQUIRE(p.next(0, r));
  CHECK(text(r) == "gvers");
  p.onVersion(ver("2.1.1-revamped_C2"));
  CHECK(p.resyncStep() == ResyncStep::Version);
  p.onResult(r, true, 0);
  CHECK(p.resyncStep() == ResyncStep::HwId);
}

TEST_CASE("planner: a revamped gvers on protocol 1 arms a gproto probe; success re-syncs") {
  PollPlanner p;
  p.requestResync();
  uint32_t now = 0;
  CHECK(runResync(p, now, 0) == v1Steps());
  REQUIRE(p.protocol() == 1);
  drain(p, now);
  p.onVersion(ver("2.1.0-revamped_C2"));
  CHECK_FALSE(p.resyncActive());
  RequestLine r;
  REQUIRE(p.next(now, r));
  CHECK(text(r) == "gproto");
  CHECK(r.probe);
  CHECK_FALSE(p.lastWasResync());
  p.setProtocol(3);
  p.onResult(r, true, now);
  CHECK(p.resyncActive());
  CHECK(p.resyncStep() == ResyncStep::Proto);
  CHECK(p.protocol() == 0);
}

TEST_CASE("planner: a timed-out probe leaves protocol 1 and the next gvers arms it again") {
  PollPlanner p;
  p.requestResync();
  uint32_t now = 0;
  runResync(p, now, 0);
  drain(p, now);
  p.onVersion(ver("2.1.0-revamped_C2"));
  RequestLine r;
  REQUIRE(p.next(now, r));
  REQUIRE(text(r) == "gproto");
  p.onResult(r, false, now);
  CHECK(p.protocol() == 1);
  CHECK_FALSE(p.resyncActive());
  // A timeout drops the probe until the next gvers.
  for (const std::string& s : drain(p, now + 100000)) CHECK(s != "gproto");
  now += 100000;
  p.onVersion(ver("2.1.0-revamped_C2"));
  REQUIRE(p.next(now, r));
  CHECK(text(r) == "gproto");
  p.onResult(r, true, now);  // answered, but protocol 1 is still set: no re-sync
  CHECK_FALSE(p.resyncActive());
  CHECK(p.protocol() == 1);
  drain(p, now);
  p.onVersion(ver("2.1.0-revamped_C2"));
  CHECK(next(p, now) == "gproto");
}

TEST_CASE("planner: no probe for a legacy version or on protocol 0, 2 and 3") {
  for (uint8_t proto : {0, 2, 3}) {
    CAPTURE(int(proto));
    PollPlanner p;
    p.setProtocol(proto);
    drain(p, 0);
    p.onVersion(ver("2.1.0-revamped_C2"));
    for (const std::string& s : drain(p, 100000)) CHECK(s != "gproto");
  }
  PollPlanner p;
  p.requestResync();
  uint32_t now = 0;
  runResync(p, now, 0);
  drain(p, now);
  p.onVersion(ver("1.4.9_C2"));
  for (const std::string& s : drain(p, now + 100000)) CHECK(s != "gproto");
}

TEST_CASE("planner: requestStatus asks gstax on 3, gstat on 2, nothing on 0/1") {
  const char* want[] = {"-", "-", "gstat", "gstax"};
  for (uint8_t proto = 0; proto <= 3; ++proto) {
    CAPTURE(int(proto));
    PollPlanner p;
    p.setProtocol(proto);
    drain(p, 0);
    p.requestStatus();
    CHECK(next(p, 1) == want[proto]);
  }
}

TEST_CASE("planner: a status one-shot is dropped when the protocol falls below 2") {
  PollPlanner p;
  p.setProtocol(3);
  drain(p, 0);
  p.requestStatus();
  p.setProtocol(1);
  CHECK(next(p, 1) == "-");
}

TEST_CASE("planner: masns waits 5 s after the request, coalesces and retries after a failure") {
  PollPlanner p;
  drain(p, 0);
  p.requestMatchSensors(1000);
  CHECK(next(p, 5999) == "-");
  p.requestMatchSensors(3000);  // coalesced: the first delay stays
  RequestLine r;
  REQUIRE(p.next(6000, r));
  CHECK(text(r) == "masns");
  CHECK(next(p, 6000) == "-");
  p.onResult(r, false, 6000);
  CHECK(next(p, 7999) == "-");
  REQUIRE(p.next(8000, r));
  CHECK(text(r) == "masns");
  p.onResult(r, true, 8000);
  CHECK(next(p, 8000 + PollPlanner::kLostRequestMs) != "masns");
}

TEST_CASE("planner: requestResync drops a pending masns") {
  PollPlanner p;
  drain(p, 0);
  p.requestMatchSensors(0);
  p.requestResync();
  uint32_t now = 10000;
  runResync(p, now, 0);
  for (const std::string& s : drain(p, now + 100000)) CHECK(s != "masns");
}

TEST_CASE("planner: one-shot priority ends with probe, status, masns") {
  PollPlanner p;
  p.setProtocol(2);
  drain(p, 0);
  p.requestMatchSensors(0);
  p.requestStatus();
  p.requestMotorParams();
  CHECK(next(p, 5000) == "gmotc");
  CHECK(next(p, 5000) == "gtlnm");
  CHECK(next(p, 5000) == "gcalx");
  CHECK(next(p, 5000) == "gstat");
  CHECK(next(p, 5000) == "masns");
}
