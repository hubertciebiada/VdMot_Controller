// Scheduled calibration confirmed by the STM (attempts, retries, no result,
// missed slots) and the STM learn-time sync.
#include <stdint.h>
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/calib_schedule.h"

using namespace vdm;

namespace {

// 2026-09-23 is a Wednesday (3), 2026-09-24 a Thursday.
LocalTime wed(int h, int mi, int d = 23) {
  LocalTime t;
  t.valid = true;
  t.year = 2026;
  t.month = 9;
  t.mday = static_cast<uint8_t>(d);
  t.wday = static_cast<uint8_t>(3 + d - 23);
  t.hour = static_cast<uint8_t>(h);
  t.minute = static_cast<uint8_t>(mi);
  return t;
}

CalibScheduleConfig daily(uint8_t hour) {
  CalibScheduleConfig c;
  c.dayMask = 0x7F;
  c.hour = hour;
  return c;
}

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

struct Sync {
  LearnTimeSync ls;
  RequestLine last;
  std::string next(uint32_t now) {
    RequestLine r;
    if (!ls.next(now, r)) {
      CHECK(r.len == 0);
      return "-";
    }
    last = r;
    return text(r);
  }
  void ok(const std::string& line, uint32_t now) {
    const Reply r = reply(line);
    ls.onCompletion(last, Outcome::Ok, &r, now);
  }
  void timeout(uint32_t now) { ls.onCompletion(last, Outcome::Timeout, nullptr, now); }
};

}  // namespace

TEST_CASE("calib E1: constants and failure names") {
  CHECK(CalibScheduler::kRetryMs == 600000);
  CHECK(CalibScheduler::kResultTimeoutMs == 60000);
  CHECK(strcmp(calibFailureName(CalibFailure::None), "none") == 0);
  CHECK(strcmp(calibFailureName(CalibFailure::NoReply), "no_reply") == 0);
  CHECK(strcmp(calibFailureName(CalibFailure::NotSent), "not_sent") == 0);
  CHECK(strcmp(calibFailureName(CalibFailure::NoResult), "no_result") == 0);
  CHECK(strcmp(calibFailureName(CalibFailure::Unsupported), "stm_unsupported") == 0);
  CHECK(strcmp(calibFailureName(static_cast<CalibFailure>(5)), "unknown") == 0);
}

TEST_CASE("calib E1: a Fire books nothing; the confirmation books the slot once") {
  CalibScheduler s;
  CHECK_FALSE(s.attemptPending());
  CHECK(s.attemptSlot() == 0);
  CHECK(s.attempts() == 0);
  CHECK_FALSE(s.onResult(true, 0));  // nothing pending
  CHECK(s.evaluate(daily(0), wed(0, 0), 1000) == CalibDecision::Fire);
  CHECK(s.lastSlot() == 0);
  CHECK(s.attemptPending());
  CHECK(s.attemptSlot() == 20260923u);
  CHECK(s.attempts() == 1);
  CHECK(s.evaluate(daily(0), wed(0, 1), 2000) == CalibDecision::None);  // waiting
  CHECK(s.onResult(true, 3000));
  CHECK(s.lastSlot() == 20260923u);
  CHECK_FALSE(s.attemptPending());
  CHECK_FALSE(s.onResult(true, 4000));
  CHECK(s.evaluate(daily(0), wed(1, 0), 700000) == CalibDecision::None);
  CHECK(s.evaluate(daily(0), wed(3, 0), 800000) == CalibDecision::None);  // window closed: booked
}

TEST_CASE("calib E1: a failed attempt fires again 10 min later while the window is open") {
  CalibScheduler s;
  const uint32_t u = 100000;
  REQUIRE(s.evaluate(daily(3), wed(3, 0), u - 5000) == CalibDecision::Fire);
  CHECK_FALSE(s.onResult(false, u));
  CHECK_FALSE(s.attemptPending());
  CHECK(s.lastSlot() == 0);
  CHECK(s.evaluate(daily(3), wed(3, 9), u + 599999) == CalibDecision::None);
  CHECK(s.evaluate(daily(3), wed(3, 10), u + 600000) == CalibDecision::Fire);
  CHECK(s.attempts() == 2);
  CHECK(s.lateMinutes() == 10);
  CHECK(s.onResult(true, u + 601000));
  CHECK(s.lastSlot() == 20260923u);
}

TEST_CASE("calib E1: no result within 60 s is a failed attempt") {
  CalibScheduler s;
  const uint32_t u = 50000;
  REQUIRE(s.evaluate(daily(3), wed(3, 0), u) == CalibDecision::Fire);
  CHECK(s.evaluate(daily(3), wed(3, 0), u + 59999) == CalibDecision::None);
  CHECK(s.evaluate(daily(3), wed(3, 1), u + 60000) == CalibDecision::NoResult);
  CHECK_FALSE(s.attemptPending());
  CHECK_FALSE(s.onResult(true, u + 60001));  // a late result is ignored
  CHECK(s.lastSlot() == 0);
  CHECK(s.evaluate(daily(3), wed(3, 10), u + 60000 + 599999) == CalibDecision::None);
  CHECK(s.evaluate(daily(3), wed(3, 11), u + 60000 + 600000) == CalibDecision::Fire);
  CHECK(s.attempts() == 2);
}

TEST_CASE("calib E1: the window closes after failed attempts: Missed once, then the next date") {
  CalibScheduler s;
  REQUIRE(s.evaluate(daily(3), wed(3, 0), 0) == CalibDecision::Fire);
  s.onResult(false, 1000);
  REQUIRE(s.evaluate(daily(3), wed(3, 20), 700000) == CalibDecision::Fire);
  s.onResult(false, 701000);
  CHECK(s.evaluate(daily(3), wed(4, 59), 1300999) == CalibDecision::None);  // open, retry hold
  CHECK(s.evaluate(daily(3), wed(5, 0), 2100000) == CalibDecision::Missed);
  CHECK(s.attemptSlot() == 20260923u);
  CHECK(s.attempts() == 2);
  CHECK(s.evaluate(daily(3), wed(5, 1), 2200000) == CalibDecision::None);
  CHECK(s.evaluate(daily(3), wed(3, 0, 24), 90000000) == CalibDecision::Fire);
  CHECK(s.attemptSlot() == 20260924u);
  CHECK(s.attempts() == 1);
}

TEST_CASE("calib E1: an attempt pending over the window end reports NoResult first, then Missed") {
  CalibScheduler s;
  REQUIRE(s.evaluate(daily(3), wed(4, 59), 0) == CalibDecision::Fire);
  CHECK(s.evaluate(daily(3), wed(5, 0), 30000) == CalibDecision::None);  // still waiting
  CHECK(s.evaluate(daily(3), wed(5, 1), 60000) == CalibDecision::NoResult);
  CHECK(s.evaluate(daily(3), wed(5, 1), 70000) == CalibDecision::Missed);
  CHECK(s.evaluate(daily(3), wed(5, 2), 80000) == CalibDecision::None);
}

TEST_CASE("calib E1: another date closes an unconfirmed slot too") {
  CalibScheduler s;
  REQUIRE(s.evaluate(daily(3), wed(3, 0), 0) == CalibDecision::Fire);
  s.onResult(false, 0);
  CHECK(s.evaluate(daily(3), wed(1, 0, 24), 80000000) == CalibDecision::Missed);
  CHECK(s.evaluate(daily(3), wed(3, 0, 24), 90000000) == CalibDecision::Fire);
}

TEST_CASE("calib E1: a reboot inside the window fires again (the slot was never booked)") {
  CalibScheduler a;
  a.restoreLastSlot(20260922);
  REQUIRE(a.evaluate(daily(3), wed(3, 0), 0) == CalibDecision::Fire);
  a.onResult(false, 0);
  CalibScheduler b;
  b.restoreLastSlot(20260922);
  CHECK(b.evaluate(daily(3), wed(3, 5), 5000) == CalibDecision::Fire);
}

TEST_CASE("calib E1: a confirmed slot is never reported as missed") {
  CalibScheduler s;
  REQUIRE(s.evaluate(daily(3), wed(3, 0), 0) == CalibDecision::Fire);
  REQUIRE(s.onResult(true, 0));
  CHECK(s.evaluate(daily(3), wed(6, 0), 100000) == CalibDecision::None);
  CHECK(s.evaluate(daily(3), wed(1, 0, 24), 200000) == CalibDecision::None);
}

// ================================================================ learn time

TEST_CASE("learn time: 0 while the ESP schedule is on, else one week") {
  CHECK(kStmLearnTimeDefaultS == 604800);
  CalibScheduleConfig c;
  c.dayMask = 0;
  CHECK(stmLearnTime(c) == 604800);
  c.dayMask = 9;
  CHECK(stmLearnTime(c) == 0);
  c.dayMask = 0x80;
  CHECK(stmLearnTime(c) == 604800);
  c.dayMask = 1;
  c.hour = 24;
  CHECK(stmLearnTime(c) == 604800);
  c.hour = 23;
  c.minute = 60;
  CHECK(stmLearnTime(c) == 604800);
  c.minute = 59;
  CHECK(stmLearnTime(c) == 0);
  CHECK(LearnTimeSync::kRetryMs == 60000);
  CHECK(LearnTimeSync::kLostRequestMs == 10000);
}

TEST_CASE("learn time: protocol 3 reads, writes a different value and verifies") {
  Sync s;
  s.ls.setProtocol(3);
  CHECK(s.next(0) == "-");  // no desired value yet
  s.ls.setDesired(0);
  CHECK(s.next(0) == "gtlnt");
  CHECK(s.next(1) == "-");
  s.ok("gtlnt 604800", 10);
  CHECK(s.ls.haveStmValue());
  CHECK(s.ls.stmValue() == 604800);
  CHECK(s.next(10) == "stlnt 0");
  s.ok("stlnt", 20);
  CHECK(s.next(20) == "gtlnt");
  s.ok("gtlnt 0", 30);
  CHECK(s.ls.stmValue() == 0);
  CHECK(s.next(40) == "-");
  CHECK(s.next(10000000) == "-");
  s.ls.setDesired(0);  // unchanged
  CHECK(s.next(10000000) == "-");
  s.ls.setDesired(604800);
  CHECK(s.next(10000000) == "gtlnt");
  s.ok("gtlnt 0", 10000000);
  CHECK(s.next(10000000) == "stlnt 604800");
  s.ls.onStmReboot();
  CHECK(s.next(10000001) == "gtlnt");
  s.ok("gtlnt 604800", 10000002);
  CHECK(s.next(10000003) == "-");
}

TEST_CASE("learn time: protocol 3 failures retry after 60 s") {
  Sync s;
  s.ls.setProtocol(3);
  s.ls.setDesired(0);
  REQUIRE(s.next(0) == "gtlnt");
  s.timeout(100);
  CHECK(s.next(60099) == "-");
  REQUIRE(s.next(60100) == "gtlnt");
  s.ok("gtlnt 5", 60100);
  REQUIRE(s.next(60100) == "stlnt 0");
  s.timeout(60200);
  CHECK(s.next(120199) == "-");
  CHECK(s.next(120200) == "gtlnt");
  s.ok("gtlnt 5", 120200);
  REQUIRE(s.next(120200) == "stlnt 0");
  s.ok("stlnt", 120300);
  REQUIRE(s.next(120300) == "gtlnt");
  s.ok("gtlnt 5", 120400);  // the STM kept its value: a failure
  CHECK(s.next(180399) == "-");
  CHECK(s.next(180400) == "gtlnt");
  CHECK(s.next(180400 + 9999) == "-");
  CHECK(s.next(180400 + 10000) == "-");  // lost: a failure, next try 60 s later
  CHECK(s.next(180400 + 10000 + 59999) == "-");
  CHECK(s.next(180400 + 10000 + 60000) == "gtlnt");
}

TEST_CASE("learn time: protocols 1/2 send stlnt 0 once per session, the default only after it") {
  Sync s;
  s.ls.setProtocol(1);
  s.ls.setDesired(604800);
  CHECK(s.next(0) == "-");  // nothing was switched off in this session
  s.ls.setDesired(0);
  CHECK(s.next(0) == "stlnt 0");
  s.ok("stlnt", 10);
  CHECK(s.next(20) == "-");
  CHECK(s.next(10000000) == "-");
  s.ls.setDesired(604800);
  CHECK(s.next(10000000) == "stlnt 604800");
  s.ok("stlnt", 10000010);
  CHECK(s.next(10000020) == "-");
  CHECK_FALSE(s.ls.haveStmValue());
  s.ls.setDesired(0);
  CHECK(s.next(10000030) == "stlnt 0");
  s.timeout(10000040);  // failed: again 60 s later
  CHECK(s.next(10060039) == "-");
  CHECK(s.next(10060040) == "stlnt 0");
  s.ok("stlnt", 10060050);
  s.ls.setProtocol(2);  // new session: once more
  CHECK(s.next(10060060) == "stlnt 0");
  s.ok("stlnt", 10060070);
  s.ls.onStmReboot();
  CHECK(s.next(10060080) == "stlnt 0");
  s.ok("stlnt", 10060090);
  s.ls.setProtocol(0);
  s.ls.setDesired(604800);
  CHECK(s.next(20000000) == "-");
}

TEST_CASE("learn time: completions of other requests are ignored") {
  Sync s;
  s.ls.setProtocol(3);
  s.ls.setDesired(0);
  REQUIRE(s.next(0) == "gtlnt");
  RequestLine other;
  REQUIRE(buildSetLearnTime(0, other));
  const Reply r = reply("gtlnt 0");
  s.ls.onCompletion(other, Outcome::Ok, &r, 10);
  CHECK_FALSE(s.ls.haveStmValue());
  CHECK(s.next(20) == "-");
}

TEST_CASE("calib E1: the slot epoch takes the UTC offset of the reference time") {
  const int64_t midnight = 1790121600;  // 2026-09-23 00:00 UTC
  LocalTime utc = wed(3, 5);
  utc.second = 30;
  utc.epoch = midnight + 3 * 3600 + 5 * 60 + 30;
  CHECK(calibSlotEpoch(20260930, 3, 0, utc) == midnight + 7 * 86400 + 3 * 3600);
  CHECK(calibSlotEpoch(20260923, 23, 59, utc) == midnight + 23 * 3600 + 59 * 60);
  CHECK(calibSlotEpoch(20261001, 0, 1, utc) == midnight + 8 * 86400 + 60);
  LocalTime cest = wed(5, 5);  // UTC+2: 05:05 local is 03:05 UTC
  cest.epoch = midnight + 3 * 3600 + 5 * 60;
  CHECK(calibSlotEpoch(20260930, 3, 0, cest) == midnight + 7 * 86400 + 3600);
  LocalTime west = wed(22, 5, 22);  // UTC-5: Tuesday 22:05 local is Wednesday 03:05 UTC
  west.wday = 2;
  west.epoch = midnight + 3 * 3600 + 5 * 60;
  CHECK(calibSlotEpoch(20260923, 3, 0, west) == midnight + 8 * 3600);
}

TEST_CASE("calib E1: no slot epoch without a slot or a valid reference time") {
  LocalTime ref = wed(3, 5);
  ref.epoch = 1790121600 + 3 * 3600 + 5 * 60;
  CHECK(calibSlotEpoch(0, 3, 0, ref) == 0);
  CHECK(calibSlotEpoch(20260923, 3, 5, ref) == ref.epoch);
  ref.valid = false;
  CHECK(calibSlotEpoch(20260930, 3, 0, ref) == 0);
}
