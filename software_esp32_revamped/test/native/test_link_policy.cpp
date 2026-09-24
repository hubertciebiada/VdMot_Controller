// LinkPolicy (queue, priorities, timeouts, retries, R6 reset policy, states)
// and RebootDetector.
#include <stdint.h>
#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/link_policy.h"

using namespace vdm;

namespace {

RequestLine valveData(uint8_t v) {
  RequestLine r;
  REQUIRE(buildValveData(v, r));
  return r;
}

RequestLine setTarget(uint8_t v, uint8_t pos) {
  RequestLine r;
  REQUIRE(buildSetTarget(v, pos, r));
  return r;
}

RequestLine calibrate(uint8_t v) {
  RequestLine r;
  REQUIRE(buildCalibrate(v, r));
  return r;
}

RequestLine proto() {
  RequestLine r;
  REQUIRE(buildGetProto(r));
  return r;
}

Reply reply(const char* s) {
  Reply r;
  REQUIRE(parseReply(s, strlen(s), r) == ParseStatus::Ok);
  return r;
}

std::string gvlvd(uint8_t v) {
  return "gvlvd " + std::to_string(v) + " 42 18 1 215 -500 57 3120 3350 230 0";
}

std::string text(const RequestLine* r) { return r ? std::string(r->text, r->len) : "<null>"; }

// Sends the next line at `now`, returns its text.
std::string send(LinkPolicy& lp, uint32_t now) {
  const RequestLine* r = lp.nextToSend(now);
  if (r) lp.onSent(now);
  return text(r);
}

// Lets the outstanding request time out completely; returns the completion.
Completion expire(LinkPolicy& lp, uint32_t& now) {
  Completion c;
  for (int guard = 0; guard < 100; ++guard) {
    now += 5000;
    if (lp.poll(now, c)) return c;
    now += 10;
    REQUIRE(lp.nextToSend(now) != nullptr);
    lp.onSent(now);
  }
  FAIL("request never completed");
  return c;
}

}  // namespace

TEST_CASE("link: state names") {
  CHECK(std::string(linkStateName(LinkState::Unknown)) == "unknown");
  CHECK(std::string(linkStateName(LinkState::Up)) == "up");
  CHECK(std::string(linkStateName(LinkState::Degraded)) == "degraded");
  CHECK(std::string(linkStateName(LinkState::Down)) == "down");
  CHECK(std::string(linkStateName(LinkState::Booting)) == "booting");
  CHECK(std::string(linkStateName(LinkState::Suspended)) == "suspended");
  CHECK(std::string(linkStateName(static_cast<LinkState>(42))) == "unknown");
}

TEST_CASE("link: defaults match the binding table") {
  LinkParams p;
  CHECK(p.timeoutMs == 400);
  CHECK(p.longTimeoutMs == 1500);
  CHECK(p.slowTimeoutMs == 3000);
  CHECK(p.retries == 2);
  CHECK(p.interRequestGapMs == 5);
  CHECK(p.bootHoldoffMs == 5000);
  CHECK(p.downAfter == 5);
  CHECK(p.resetMinTimeouts == 5);
  CHECK(p.resetMinSpanMs == 60000);
  CHECK(p.resetMinIntervalMs == 600000);
  CHECK(LinkPolicy::kQueueCapacity == 24);
  LinkPolicy lp;
  CHECK(lp.state(0) == LinkState::Unknown);
  CHECK(lp.queued() == 0);
  CHECK_FALSE(lp.busy());
  CHECK(lp.nextToSend(0) == nullptr);
  Completion c;
  CHECK_FALSE(lp.poll(0, c));
  CHECK_FALSE(lp.shouldResetStm(0));
}

TEST_CASE("link: enqueue rejects invalid requests") {
  LinkPolicy lp;
  RequestLine empty;
  CHECK(lp.enqueue(empty, Priority::User) == EnqueueResult::Invalid);
  RequestLine noCmd = valveData(1);
  noCmd.cmd = Cmd::None;
  CHECK(lp.enqueue(noCmd, Priority::User) == EnqueueResult::Invalid);
  RequestLine badCmd = valveData(1);
  badCmd.cmd = static_cast<Cmd>(kCmdCount);
  CHECK(lp.enqueue(badCmd, Priority::User) == EnqueueResult::Invalid);
  RequestLine tooLong = valveData(1);
  tooLong.len = kRequestMaxLen + 1;
  CHECK(lp.enqueue(tooLong, Priority::User) == EnqueueResult::Invalid);
  CHECK(lp.enqueue(valveData(1), static_cast<Priority>(3)) == EnqueueResult::Invalid);
  CHECK(lp.queued() == 0);
  RequestLine maxLen = valveData(1);
  maxLen.len = kRequestMaxLen;
  CHECK(lp.enqueue(maxLen, Priority::User) == EnqueueResult::Queued);
}

TEST_CASE("link: priority order, FIFO within a priority") {
  LinkPolicy lp;
  CHECK(lp.enqueue(valveData(0), Priority::Poll) == EnqueueResult::Queued);
  CHECK(lp.enqueue(valveData(1), Priority::Config) == EnqueueResult::Queued);
  CHECK(lp.enqueue(valveData(2), Priority::User) == EnqueueResult::Queued);
  CHECK(lp.enqueue(valveData(3), Priority::Poll) == EnqueueResult::Queued);
  CHECK(lp.enqueue(valveData(4), Priority::User) == EnqueueResult::Queued);
  CHECK(lp.enqueue(valveData(5), Priority::Config) == EnqueueResult::Queued);
  CHECK(lp.queued() == 6);
  CHECK(lp.queued(Priority::User) == 2);
  CHECK(lp.queued(Priority::Config) == 2);
  CHECK(lp.queued(Priority::Poll) == 2);
  const int order[] = {2, 4, 1, 5, 0, 3};
  uint32_t now = 1000;
  for (int v : order) {
    CHECK(send(lp, now) == "gvlvd " + std::to_string(v) + " \r\n");
    CHECK(lp.busy());
    Completion c;
    REQUIRE(lp.onReply(reply(gvlvd(static_cast<uint8_t>(v)).c_str()), now + 10, c));
    CHECK(c.outcome == Outcome::Ok);
    now += 100;
  }
  CHECK(lp.queued() == 0);
}

TEST_CASE("link: identical lines coalesce, priority raised, tag rules") {
  LinkPolicy lp;
  CHECK(lp.enqueue(valveData(0), Priority::Poll, 1) == EnqueueResult::Queued);
  CHECK(lp.enqueue(valveData(1), Priority::Poll, 2) == EnqueueResult::Queued);
  CHECK(lp.enqueue(valveData(2), Priority::Config, 3) == EnqueueResult::Queued);
  // Same priority: position and tag kept when the new tag is 0.
  CHECK(lp.enqueue(valveData(0), Priority::Poll) == EnqueueResult::Coalesced);
  CHECK(lp.queued() == 3);
  // Lower priority request does not demote.
  CHECK(lp.enqueue(valveData(2), Priority::Poll, 0) == EnqueueResult::Coalesced);
  CHECK(lp.queued(Priority::Config) == 1);
  // Raising to User moves it to the tail of the User group, new tag taken.
  CHECK(lp.enqueue(valveData(1), Priority::User, 9) == EnqueueResult::Coalesced);
  CHECK(lp.queued(Priority::User) == 1);
  CHECK(lp.queued(Priority::Poll) == 1);
  CHECK(lp.queued() == 3);

  uint32_t now = 0;
  Completion c;
  CHECK(send(lp, now) == "gvlvd 1 \r\n");
  REQUIRE(lp.onReply(reply(gvlvd(1).c_str()), now, c));
  CHECK(c.tag == 9);
  CHECK(c.priority == Priority::User);
  now += 10;
  CHECK(send(lp, now) == "gvlvd 2 \r\n");
  REQUIRE(lp.onReply(reply(gvlvd(2).c_str()), now, c));
  CHECK(c.tag == 3);
  CHECK(c.priority == Priority::Config);
  now += 10;
  CHECK(send(lp, now) == "gvlvd 0 \r\n");
  REQUIRE(lp.onReply(reply(gvlvd(0).c_str()), now, c));
  CHECK(c.tag == 1);
  CHECK(c.priority == Priority::Poll);
}

TEST_CASE("link: raised entry goes behind existing entries of its new priority") {
  LinkPolicy lp;
  lp.enqueue(valveData(0), Priority::Config);
  lp.enqueue(valveData(1), Priority::Poll);
  lp.enqueue(valveData(2), Priority::Config);
  CHECK(lp.enqueue(valveData(1), Priority::Config) == EnqueueResult::Coalesced);
  uint32_t now = 0;
  Completion c;
  for (int v : {0, 2, 1}) {
    CHECK(send(lp, now) == "gvlvd " + std::to_string(v) + " \r\n");
    REQUIRE(lp.onReply(reply(gvlvd(static_cast<uint8_t>(v)).c_str()), now, c));
    now += 10;
  }
}

TEST_CASE("link: stgtp for the same valve is replaced in place (latest wins)") {
  LinkPolicy lp;
  lp.enqueue(setTarget(3, 20), Priority::Config, 1);
  lp.enqueue(setTarget(4, 20), Priority::Config, 2);
  CHECK(lp.enqueue(setTarget(3, 70), Priority::Config, 5) == EnqueueResult::Coalesced);
  CHECK(lp.queued() == 2);
  // A different stgtp payload for another valve is not merged.
  CHECK(lp.enqueue(setTarget(5, 70), Priority::Config) == EnqueueResult::Queued);
  // Replacing with the tag 0 still takes the new tag (latest request owns it).
  CHECK(lp.enqueue(setTarget(4, 21), Priority::Config, 0) == EnqueueResult::Coalesced);
  uint32_t now = 0;
  Completion c;
  CHECK(send(lp, now) == "stgtp 3 70 \r\n");
  REQUIRE(lp.onReply(reply("stgtp"), now, c));
  CHECK(c.tag == 5);
  CHECK(c.request.arg == 70);
  now += 10;
  CHECK(send(lp, now) == "stgtp 4 21 \r\n");
  REQUIRE(lp.onReply(reply("stgtp"), now, c));
  CHECK(c.tag == 0);
  now += 10;
  CHECK(send(lp, now) == "stgtp 5 70 \r\n");
}

TEST_CASE("link: stgtp replacement can raise priority") {
  LinkPolicy lp;
  lp.enqueue(valveData(0), Priority::User);
  lp.enqueue(setTarget(3, 20), Priority::Poll);
  lp.enqueue(valveData(1), Priority::Config);
  CHECK(lp.enqueue(setTarget(3, 30), Priority::User) == EnqueueResult::Coalesced);
  uint32_t now = 0;
  Completion c;
  CHECK(send(lp, now) == "gvlvd 0 \r\n");
  REQUIRE(lp.onReply(reply(gvlvd(0).c_str()), now, c));
  now += 10;
  CHECK(send(lp, now) == "stgtp 3 30 \r\n");
}

TEST_CASE("link: an outstanding line is not coalesced with a new one") {
  LinkPolicy lp;
  lp.enqueue(valveData(0), Priority::Poll);
  CHECK(send(lp, 0) == "gvlvd 0 \r\n");
  CHECK(lp.enqueue(valveData(0), Priority::Poll) == EnqueueResult::Queued);
  CHECK(lp.queued() == 1);
}

TEST_CASE("link: full queue, eviction of the newest Poll entry") {
  LinkPolicy lp;
  for (uint8_t i = 0; i < 24; ++i) {
    RequestLine r;
    REQUIRE(buildTempData(i, r));
    CHECK(lp.enqueue(r, Priority::Poll) == EnqueueResult::Queued);
  }
  CHECK(lp.queued() == 24);
  RequestLine extra;
  REQUIRE(buildTempData(30, extra));
  CHECK(lp.enqueue(extra, Priority::Poll) == EnqueueResult::Full);
  CHECK(lp.stats().queueFull == 1);
  CHECK(lp.stats().evictions == 0);
  // Coalescing still works on a full queue.
  RequestLine dup;
  REQUIRE(buildTempData(5, dup));
  CHECK(lp.enqueue(dup, Priority::Poll) == EnqueueResult::Coalesced);
  CHECK(lp.stats().queueFull == 1);

  CHECK(lp.enqueue(calibrate(1), Priority::User) == EnqueueResult::Queued);
  CHECK(lp.stats().evictions == 1);
  CHECK(lp.queued() == 24);
  CHECK(lp.queued(Priority::Poll) == 23);
  CHECK(lp.enqueue(setTarget(1, 1), Priority::Config) == EnqueueResult::Queued);
  CHECK(lp.stats().evictions == 2);

  // The evicted ones were the newest: goned 23 and 22.
  uint32_t now = 0;
  CHECK(send(lp, now) == "staln 1 \r\n");
  Completion c;
  REQUIRE(lp.onReply(reply("staln"), now, c));
  now += 10;
  CHECK(send(lp, now) == "stgtp 1 1 \r\n");
  REQUIRE(lp.onReply(reply("stgtp"), now, c));
  std::string last;
  for (int i = 0; i < 22; ++i) {
    now += 10;
    last = send(lp, now);
    REQUIRE(lp.onReply(reply("goned 0"), now, c));
  }
  CHECK(last == "goned 21 \r\n");
  CHECK(lp.queued() == 0);
}

TEST_CASE("link: queue of only User/Config entries is Full for everyone") {
  LinkPolicy lp;
  for (uint8_t i = 0; i < 12; ++i) {
    CHECK(lp.enqueue(calibrate(i), Priority::User) == EnqueueResult::Queued);
    CHECK(lp.enqueue(setTarget(i, 1), Priority::Config) == EnqueueResult::Queued);
  }
  CHECK(lp.enqueue(calibrate(kAllValves), Priority::User) == EnqueueResult::Full);
  CHECK(lp.enqueue(valveData(0), Priority::Config) == EnqueueResult::Full);
  CHECK(lp.enqueue(valveData(0), Priority::Poll) == EnqueueResult::Full);
  CHECK(lp.stats().queueFull == 3);
  CHECK(lp.stats().evictions == 0);
}

TEST_CASE("link: send path, onSent accounting and inter-request gap") {
  LinkPolicy lp;
  lp.enqueue(valveData(0), Priority::Poll);
  lp.enqueue(valveData(1), Priority::Poll);
  const RequestLine* r = lp.nextToSend(100);
  REQUIRE(r != nullptr);
  CHECK(text(r) == "gvlvd 0 \r\n");
  CHECK(lp.nextToSend(100) == nullptr);  // one outstanding
  CHECK(lp.stats().sent == 0);
  lp.onSent(100);
  CHECK(lp.stats().sent == 1);
  Completion c;
  REQUIRE(lp.onReply(reply(gvlvd(0).c_str()), 150, c));
  CHECK(c.attempts == 1);
  CHECK(lp.nextToSend(154) == nullptr);  // 4 ms < 5 ms gap
  CHECK(lp.nextToSend(155) != nullptr);
  lp.onSent(155);
  CHECK(lp.stats().sent == 2);
  lp.onSent(156);  // spurious extra call while outstanding counts; never crashes
  REQUIRE(lp.onReply(reply(gvlvd(1).c_str()), 160, c));
  lp.onSent(170);  // nothing outstanding: ignored
  CHECK(lp.stats().sent == 3);
}

TEST_CASE("link: the first request after construction is not delayed") {
  LinkPolicy lp;
  lp.enqueue(valveData(0), Priority::Poll);
  CHECK(lp.nextToSend(0) != nullptr);
}

TEST_CASE("link: zero gap sends immediately after a reply") {
  LinkParams p;
  p.interRequestGapMs = 0;
  LinkPolicy lp(p);
  lp.enqueue(valveData(0), Priority::Poll);
  lp.enqueue(valveData(1), Priority::Poll);
  Completion c;
  CHECK(send(lp, 7) == "gvlvd 0 \r\n");
  REQUIRE(lp.onReply(reply(gvlvd(0).c_str()), 9, c));
  CHECK(send(lp, 9) == "gvlvd 1 \r\n");
}

TEST_CASE("link: matching reply completes Ok and brings the link Up") {
  LinkPolicy lp;
  lp.enqueue(valveData(3), Priority::Poll, 77);
  CHECK(lp.state(0) == LinkState::Unknown);
  send(lp, 10);
  Completion c;
  CHECK_FALSE(lp.onReply(reply(gvlvd(4).c_str()), 20, c));  // other valve: stray
  CHECK(lp.stats().strayLines == 1);
  CHECK(lp.busy());
  REQUIRE(lp.onReply(reply(gvlvd(3).c_str()), 30, c));
  CHECK(c.outcome == Outcome::Ok);
  CHECK(c.tag == 77);
  CHECK(c.priority == Priority::Poll);
  CHECK(c.attempts == 1);
  CHECK(std::string(c.request.text) == "gvlvd 3 \r\n");
  CHECK(lp.stats().answered == 1);
  CHECK(lp.stats().lastReplyMs == 30);
  CHECK(lp.state(30) == LinkState::Up);
  CHECK_FALSE(lp.busy());
  // A reply with nothing outstanding is stray.
  CHECK_FALSE(lp.onReply(reply(gvlvd(3).c_str()), 40, c));
  CHECK(lp.stats().strayLines == 2);
  CHECK(lp.stats().answered == 1);
  CHECK(lp.stats().lastReplyMs == 30);
}

TEST_CASE("link: stray replies do not prove the link") {
  LinkPolicy lp;
  Completion c;
  CHECK_FALSE(lp.onReply(reply("stgtp"), 1, c));
  CHECK(lp.state(1) == LinkState::Unknown);
  CHECK(lp.stats().lastReplyMs == 0);
}

TEST_CASE("link: error forms complete as Rejected") {
  struct Case {
    RequestLine req;
    const char* rep;
    Outcome expect;
  };
  RequestLine smotc, scalx, svmov, goned, gowvd, gvlon, gvlonAll;
  MotorChars m;
  Breakaway b;
  REQUIRE(buildSetMotorChars(m, smotc));
  REQUIRE(buildSetBreakaway(b, scalx));
  REQUIRE(buildServiceMove(2, MoveDir::Open, 100, 30, svmov));
  REQUIRE(buildTempData(3, goned));
  REQUIRE(buildVoltData(3, gowvd));
  REQUIRE(buildValveSensors(2, gvlon));
  REQUIRE(buildValveSensors(kAllValves, gvlonAll));
  const Case cases[] = {
      {smotc, "smotc err", Outcome::Rejected},
      {smotc, "smotc", Outcome::Ok},
      {scalx, "scalx err", Outcome::Rejected},
      {scalx, "scalx ok", Outcome::Ok},
      {svmov, "svmov 2 err 3", Outcome::Rejected},
      {svmov, "svmov 2 ok", Outcome::Ok},
      {goned, "goned 0", Outcome::Rejected},
      {goned, "goned 28-84-37-94-97-ff-03-23 215", Outcome::Ok},
      {gowvd, "gowvd 0", Outcome::Rejected},
      {gowvd, "gowvd 26-11-22-33-44-55-66-29 12", Outcome::Ok},
      {gvlon, "goned error", Outcome::Rejected},
      {gvlonAll, "goned error", Outcome::Rejected},
      {gvlon, "gvlon 2 00-00-00-00-00-00-00-00 00-00-00-00-00-00-00-00", Outcome::Ok},
  };
  for (const Case& k : cases) {
    CAPTURE(k.rep);
    LinkPolicy lp;
    REQUIRE(lp.enqueue(k.req, Priority::User) == EnqueueResult::Queued);
    send(lp, 0);
    Completion c;
    REQUIRE(lp.onReply(reply(k.rep), 5, c));
    CHECK(c.outcome == k.expect);
    CHECK(lp.state(5) == LinkState::Up);
    CHECK(lp.stats().answered == 1);
  }
}

TEST_CASE("link: parse errors are counted only") {
  LinkPolicy lp;
  lp.enqueue(valveData(0), Priority::Poll);
  send(lp, 0);
  lp.onParseError(1);
  lp.onParseError(2);
  CHECK(lp.stats().parseErrors == 2);
  CHECK(lp.busy());
  CHECK(lp.state(2) == LinkState::Unknown);
}

namespace {

void checkTimeout(const RequestLine& req, uint16_t expectMs) {
  CAPTURE(std::string(req.text));
  LinkParams p;
  p.retries = 0;
  LinkPolicy lp(p);
  REQUIRE(lp.enqueue(req, Priority::User) == EnqueueResult::Queued);
  const uint32_t t0 = 0xFFFFFF00u;  // across the millis() wrap
  REQUIRE(lp.nextToSend(t0) != nullptr);
  lp.onSent(t0);
  Completion c;
  CHECK_FALSE(lp.poll(t0 + expectMs - 1, c));
  CHECK(lp.poll(t0 + expectMs, c));
  CHECK(c.outcome == Outcome::Timeout);
  CHECK(c.attempts == 1);
}

}  // namespace

TEST_CASE("link: per-command timeouts") {
  RequestLine r;
  REQUIRE(buildValveData(1, r));
  checkTimeout(r, 400);
  REQUIRE(buildTempCount(r));
  checkTimeout(r, 400);
  REQUIRE(buildVoltCount(r));
  checkTimeout(r, 400);
  REQUIRE(buildValveSensors(3, r));
  checkTimeout(r, 400);
  REQUIRE(buildGetProto(r));
  checkTimeout(r, 400);
  REQUIRE(buildTempList(r));
  checkTimeout(r, 1500);
  REQUIRE(buildVoltList(r));
  checkTimeout(r, 1500);
  REQUIRE(buildValveSensors(kAllValves, r));
  checkTimeout(r, 1500);
  REQUIRE(buildProfile(0, r));
  checkTimeout(r, 1500);
  REQUIRE(buildScanOneWire(r));
  checkTimeout(r, 3000);
  REQUIRE(buildMatchSensors(r));
  checkTimeout(r, 3000);
  REQUIRE(buildDetect(r));
  checkTimeout(r, 3000);
  REQUIRE(buildSoftReset(r));
  checkTimeout(r, 3000);
  REQUIRE(buildSetMotorChars(MotorChars{}, r));
  checkTimeout(r, 3000);
  REQUIRE(buildSetValveSensors(1, OneWireId{}, OneWireId{}, r));
  checkTimeout(r, 3000);
  REQUIRE(buildCalibrate(1, r));
  checkTimeout(r, 400);
}

TEST_CASE("link: timeout starts at onSent, not at nextToSend") {
  LinkParams p;
  p.retries = 0;
  LinkPolicy lp(p);
  lp.enqueue(valveData(0), Priority::Poll);
  REQUIRE(lp.nextToSend(0) != nullptr);
  lp.onSent(100);
  Completion c;
  CHECK_FALSE(lp.poll(499, c));
  CHECK(lp.poll(500, c));
}

TEST_CASE("link: idempotent requests are retried at the head of their priority") {
  LinkPolicy lp;
  lp.enqueue(valveData(0), Priority::Poll, 5);
  lp.enqueue(valveData(1), Priority::Poll);
  lp.enqueue(valveData(2), Priority::Config);
  uint32_t now = 0;
  CHECK(send(lp, now) == "gvlvd 2 \r\n");
  Completion c;
  REQUIRE(lp.onReply(reply(gvlvd(2).c_str()), now, c));
  now += 10;
  CHECK(send(lp, now) == "gvlvd 0 \r\n");
  now += 400;
  CHECK_FALSE(lp.poll(now, c));  // retry queued, no completion
  CHECK(lp.stats().timeouts == 1);
  CHECK(lp.stats().consecutiveTimeouts == 1);
  CHECK(lp.queued() == 2);
  // A User request queued meanwhile still goes first.
  lp.enqueue(calibrate(4), Priority::User);
  CHECK(lp.nextToSend(now + 4) == nullptr);  // gap after a timeout too
  now += 5;
  CHECK(send(lp, now) == "staln 4 \r\n");
  REQUIRE(lp.onReply(reply("staln"), now, c));
  CHECK(lp.stats().consecutiveTimeouts == 0);
  now += 10;
  CHECK(send(lp, now) == "gvlvd 0 \r\n");  // retry before gvlvd 1
  now += 400;
  CHECK_FALSE(lp.poll(now, c));
  now += 10;
  CHECK(send(lp, now) == "gvlvd 0 \r\n");
  now += 399;
  CHECK_FALSE(lp.poll(now, c));
  now += 1;
  REQUIRE(lp.poll(now, c));
  CHECK(c.outcome == Outcome::Timeout);
  CHECK(c.attempts == 3);
  CHECK(c.tag == 5);
  CHECK(c.priority == Priority::Poll);
  CHECK(lp.stats().timeouts == 3);
  CHECK(lp.stats().failedRequests == 1);
  CHECK(lp.stats().consecutiveTimeouts == 2);
  now += 10;
  CHECK(send(lp, now) == "gvlvd 1 \r\n");
}

TEST_CASE("link: a retry answered completes with its attempt count") {
  LinkPolicy lp;
  lp.enqueue(valveData(0), Priority::Poll);
  send(lp, 0);
  Completion c;
  CHECK_FALSE(lp.poll(400, c));
  send(lp, 405);
  REQUIRE(lp.onReply(reply(gvlvd(0).c_str()), 450, c));
  CHECK(c.attempts == 2);
  CHECK(c.outcome == Outcome::Ok);
}

TEST_CASE("link: retries parameter bounds the attempts") {
  for (uint8_t retries : {0, 1, 3}) {
    CAPTURE(retries);
    LinkParams p;
    p.retries = retries;
    LinkPolicy lp(p);
    lp.enqueue(valveData(0), Priority::Poll);
    uint32_t now = 0;
    send(lp, now);
    const Completion c = expire(lp, now);
    CHECK(c.attempts == retries + 1);
    CHECK(lp.stats().timeouts == retries + 1u);
    CHECK(lp.stats().sent == retries + 1u);
  }
}

TEST_CASE("link: actions are never retried") {
  const Cmd actions[] = {Cmd::Staln, Cmd::Staop, Cmd::Stdet, Cmd::Stons, Cmd::Masns, Cmd::Reset,
                         Cmd::Svmov};
  for (Cmd a : actions) {
    CAPTURE(cmdName(a));
    RequestLine r;
    switch (a) {
      case Cmd::Staln: REQUIRE(buildCalibrate(1, r)); break;
      case Cmd::Staop: REQUIRE(buildAssembly(1, r)); break;
      case Cmd::Stdet: REQUIRE(buildDetect(r)); break;
      case Cmd::Stons: REQUIRE(buildScanOneWire(r)); break;
      case Cmd::Masns: REQUIRE(buildMatchSensors(r)); break;
      case Cmd::Reset: REQUIRE(buildSoftReset(r)); break;
      default: REQUIRE(buildServiceMove(1, MoveDir::Open, 10, 10, r)); break;
    }
    LinkPolicy lp;
    lp.enqueue(r, Priority::User);
    send(lp, 0);
    Completion c;
    REQUIRE(lp.poll(3000, c));
    CHECK(c.attempts == 1);
    CHECK(c.outcome == Outcome::Timeout);
    CHECK(lp.queued() == 0);
  }
}

TEST_CASE("link: retry into a full queue evicts the newest Poll entry") {
  LinkPolicy lp;
  lp.enqueue(setTarget(0, 1), Priority::Config);
  send(lp, 0);
  for (uint8_t i = 0; i < 24; ++i) {
    RequestLine r;
    REQUIRE(buildTempData(i, r));
    lp.enqueue(r, Priority::Poll);
  }
  Completion c;
  CHECK_FALSE(lp.poll(400, c));
  CHECK(lp.stats().evictions == 1);
  CHECK(lp.queued() == 24);
  CHECK(send(lp, 405) == "stgtp 0 1 \r\n");
}

TEST_CASE("link: retry with no room completes as Timeout") {
  LinkPolicy lp;
  lp.enqueue(setTarget(0, 1), Priority::Config);
  send(lp, 0);
  for (uint8_t i = 0; i < 12; ++i) {
    lp.enqueue(calibrate(i), Priority::User);
    lp.enqueue(setTarget(i, 2), Priority::Config);
  }
  REQUIRE(lp.queued() == 24);
  Completion c;
  REQUIRE(lp.poll(400, c));
  CHECK(c.outcome == Outcome::Timeout);
  CHECK(c.attempts == 1);
  CHECK(lp.stats().failedRequests == 1);
  CHECK(lp.stats().evictions == 0);
}

TEST_CASE("link: Degraded after one timeout, Down after five, Up after a reply") {
  LinkParams p;
  p.retries = 0;
  LinkPolicy lp(p);
  uint32_t now = 0;
  Completion c;
  for (int i = 1; i <= 6; ++i) {
    lp.enqueue(valveData(0), Priority::Poll);
    now += 10;
    send(lp, now);
    now += 400;
    REQUIRE(lp.poll(now, c));
    CHECK(lp.stats().consecutiveTimeouts == i);
    CHECK(lp.state(now) == (i >= 5 ? LinkState::Down : LinkState::Degraded));
  }
  lp.enqueue(valveData(0), Priority::Poll);
  now += 10;
  send(lp, now);
  REQUIRE(lp.onReply(reply(gvlvd(0).c_str()), now, c));
  CHECK(lp.state(now) == LinkState::Up);
  CHECK(lp.stats().consecutiveTimeouts == 0);
}

TEST_CASE("link: a rejected reply also proves the link") {
  LinkParams p;
  p.retries = 0;
  LinkPolicy lp(p);
  RequestLine g;
  REQUIRE(buildTempData(0, g));
  lp.enqueue(g, Priority::Poll);
  send(lp, 0);
  Completion c;
  REQUIRE(lp.poll(400, c));
  lp.enqueue(g, Priority::Poll);
  send(lp, 500);
  REQUIRE(lp.onReply(reply("goned 0"), 510, c));
  CHECK(c.outcome == Outcome::Rejected);
  CHECK(lp.state(510) == LinkState::Up);
}

TEST_CASE("link: downAfter parameter") {
  LinkParams p;
  p.retries = 0;
  p.downAfter = 2;
  LinkPolicy lp(p);
  Completion c;
  lp.enqueue(valveData(0), Priority::Poll);
  send(lp, 0);
  REQUIRE(lp.poll(400, c));
  CHECK(lp.state(400) == LinkState::Degraded);
  lp.enqueue(valveData(0), Priority::Poll);
  send(lp, 500);
  REQUIRE(lp.poll(900, c));
  CHECK(lp.state(900) == LinkState::Down);
}

TEST_CASE("link: gproto timeouts never count toward the failure counter") {
  LinkPolicy lp;
  lp.enqueue(proto(), Priority::Config);
  uint32_t now = 0;
  send(lp, now);
  const Completion c = expire(lp, now);
  CHECK(c.outcome == Outcome::Timeout);
  CHECK(c.attempts == 3);  // still retried: a v2 STM may have lost it
  CHECK(lp.stats().timeouts == 3);
  CHECK(lp.stats().consecutiveTimeouts == 0);
  CHECK(lp.state(now) == LinkState::Unknown);
  CHECK_FALSE(lp.shouldResetStm(now + 100000));
}

TEST_CASE("link: consecutive timeouts saturate at 255") {
  LinkParams p;
  p.retries = 0;
  LinkPolicy lp(p);
  uint32_t now = 0;
  Completion c;
  for (int i = 0; i < 300; ++i) {
    lp.enqueue(valveData(0), Priority::Poll);
    now += 10;
    send(lp, now);
    now += 400;
    REQUIRE(lp.poll(now, c));
  }
  CHECK(lp.stats().consecutiveTimeouts == 255);
  CHECK(lp.stats().timeouts == 300);
  CHECK(lp.state(now) == LinkState::Down);
}

namespace {

// Drives `n` single-attempt timeouts spaced `stepMs` apart starting at now.
void failRequests(LinkPolicy& lp, uint32_t& now, int n, uint32_t stepMs) {
  Completion c;
  for (int i = 0; i < n; ++i) {
    lp.enqueue(valveData(0), Priority::Poll);
    REQUIRE(lp.nextToSend(now) != nullptr);
    lp.onSent(now);
    now += 400;
    REQUIRE(lp.poll(now, c));
    now += stepMs;
  }
}

LinkParams noRetry() {
  LinkParams p;
  p.retries = 0;
  return p;
}

}  // namespace

TEST_CASE("link: R6 reset needs >= 5 consecutive timeouts") {
  LinkPolicy lp(noRetry());
  uint32_t now = 1000;
  const uint32_t first = now + 400;
  failRequests(lp, now, 4, 30000);
  CHECK(lp.stats().consecutiveTimeouts == 4);
  CHECK_FALSE(lp.shouldResetStm(first + 200000));
  failRequests(lp, now, 1, 0);
  CHECK(lp.shouldResetStm(first + 200000));
}

TEST_CASE("link: R6 reset needs the first timeout >= 60 s ago") {
  LinkPolicy lp(noRetry());
  uint32_t now = 1000;
  const uint32_t first = now + 400;  // poll() time of the first timeout
  failRequests(lp, now, 5, 10);
  CHECK_FALSE(lp.shouldResetStm(first + 59999));
  CHECK(lp.shouldResetStm(first + 60000));
  // A reply clears it.
  lp.enqueue(valveData(0), Priority::Poll);
  REQUIRE(lp.nextToSend(first + 60000) != nullptr);
  Completion c;
  REQUIRE(lp.onReply(reply(gvlvd(0).c_str()), first + 60001, c));
  CHECK_FALSE(lp.shouldResetStm(first + 60002));
}

TEST_CASE("link: R6 reset at most once per 10 minutes") {
  LinkPolicy lp(noRetry());
  uint32_t now = 0;
  failRequests(lp, now, 5, 20000);
  REQUIRE(lp.shouldResetStm(now));
  const uint32_t resetAt = now;
  lp.onStmReset(resetAt, true);
  CHECK(lp.stats().policyResets == 1);
  CHECK(lp.stats().userResets == 0);
  CHECK(lp.stats().consecutiveTimeouts == 0);
  CHECK_FALSE(lp.shouldResetStm(resetAt));
  // STM stays dead after the hold-off.
  now = resetAt + 5000;
  failRequests(lp, now, 10, 20000);
  CHECK(lp.stats().consecutiveTimeouts == 10);
  CHECK_FALSE(lp.shouldResetStm(resetAt + 599999));
  CHECK(lp.shouldResetStm(resetAt + 600000));
}

TEST_CASE("link: a user reset does not rate-limit a policy reset") {
  LinkPolicy lp(noRetry());
  uint32_t now = 0;
  lp.onStmReset(now, false);
  CHECK(lp.stats().userResets == 1);
  CHECK(lp.stats().policyResets == 0);
  now = 5000;
  failRequests(lp, now, 5, 20000);
  CHECK(lp.shouldResetStm(now));
}

TEST_CASE("link: the reset rate limit survives the millis wrap via poll()") {
  LinkPolicy lp(noRetry());
  lp.onStmReset(1000, true);
  Completion c;
  lp.poll(1000 + 600000, c);  // retires the limit
  // Exactly 2^32 ms after the reset: elapsedMs() alone would say ~100 s.
  uint32_t now = 1000;
  failRequests(lp, now, 5, 20000);
  CHECK(lp.shouldResetStm(now));
}

TEST_CASE("link: no R6 reset while Booting or Suspended") {
  LinkPolicy lp(noRetry());
  uint32_t now = 0;
  failRequests(lp, now, 5, 20000);
  REQUIRE(lp.shouldResetStm(now));
  lp.suspend();
  CHECK_FALSE(lp.shouldResetStm(now));
  CHECK(lp.state(now) == LinkState::Suspended);
}

TEST_CASE("link: STM reset drops the outstanding request and Poll entries") {
  LinkPolicy lp;
  lp.enqueue(valveData(0), Priority::Poll);
  lp.enqueue(valveData(1), Priority::Poll);
  lp.enqueue(setTarget(1, 50), Priority::Config);
  lp.enqueue(calibrate(2), Priority::User);
  lp.enqueue(valveData(3), Priority::Poll);
  CHECK(send(lp, 0) == "staln 2 \r\n");
  lp.onStmReset(100, false);
  CHECK_FALSE(lp.busy());
  CHECK(lp.queued() == 1);
  CHECK(lp.queued(Priority::Config) == 1);
  CHECK(lp.state(100) == LinkState::Booting);
  CHECK(lp.state(5099) == LinkState::Booting);
  CHECK(lp.nextToSend(5099) == nullptr);
  CHECK(lp.state(5100) == LinkState::Unknown);
  Completion c;
  CHECK_FALSE(lp.onReply(reply("staln"), 200, c));  // late reply is stray
  CHECK_FALSE(lp.poll(4000, c));                   // no completion for the dropped one
  CHECK(send(lp, 5100) == "stgtp 1 50 \r\n");
}

TEST_CASE("link: Booting forgets Up; the next reply brings it back") {
  LinkPolicy lp;
  lp.enqueue(valveData(0), Priority::Poll);
  send(lp, 0);
  Completion c;
  REQUIRE(lp.onReply(reply(gvlvd(0).c_str()), 1, c));
  CHECK(lp.state(1) == LinkState::Up);
  lp.onStmReset(10, true);
  CHECK(lp.state(5010) == LinkState::Unknown);
  lp.enqueue(valveData(0), Priority::Poll);
  send(lp, 5010);
  REQUIRE(lp.onReply(reply(gvlvd(0).c_str()), 5011, c));
  CHECK(lp.state(5011) == LinkState::Up);
}

TEST_CASE("link: Booting hold is released by poll() and survives time wrap") {
  LinkPolicy lp;
  const uint32_t t0 = 0xFFFFF000u;
  lp.onStmReset(t0, false);
  lp.enqueue(valveData(0), Priority::Config);
  CHECK(lp.nextToSend(t0 + 4999) == nullptr);
  Completion c;
  lp.poll(t0 + 5000, c);
  CHECK(lp.state(t0 + 5000) == LinkState::Unknown);
  // Far in the future (elapsed wraps): not Booting again.
  CHECK(lp.state(t0 + 5000 + 0xFFFFF000u) == LinkState::Unknown);
  CHECK(lp.nextToSend(t0 + 0xFFFFF000u) != nullptr);
}

TEST_CASE("link: suspend drops everything, resume enters Booting") {
  LinkPolicy lp;
  lp.enqueue(valveData(0), Priority::Poll);
  lp.enqueue(setTarget(1, 50), Priority::Config);
  lp.enqueue(calibrate(2), Priority::User);
  send(lp, 0);
  CHECK(lp.suspend() == 3);
  CHECK(lp.queued() == 0);
  CHECK_FALSE(lp.busy());
  CHECK(lp.state(0) == LinkState::Suspended);
  // Requests may still be queued, but wait.
  CHECK(lp.enqueue(valveData(0), Priority::Poll) == EnqueueResult::Queued);
  CHECK(lp.nextToSend(100000) == nullptr);
  CHECK(lp.state(100000) == LinkState::Suspended);
  CHECK(lp.suspend() == 1);
  lp.enqueue(valveData(0), Priority::Poll);
  lp.resume(200000);
  CHECK(lp.state(200000) == LinkState::Booting);
  CHECK(lp.nextToSend(204999) == nullptr);
  CHECK(lp.state(205000) == LinkState::Unknown);
  CHECK(send(lp, 205000) == "gvlvd 0 \r\n");
  CHECK(lp.suspend() == 1);  // only the outstanding one
}

TEST_CASE("link: resume clears the failure counter") {
  LinkPolicy lp(noRetry());
  uint32_t now = 0;
  failRequests(lp, now, 5, 20000);
  lp.suspend();
  lp.resume(now);
  CHECK(lp.stats().consecutiveTimeouts == 0);
  CHECK_FALSE(lp.shouldResetStm(now + 1000000));
}

// ================================================================ reboot detector

TEST_CASE("reboot: gstat uptime decrease or reset counter change") {
  RebootDetector d;
  StmStatus s;
  s.uptimeS = 100;
  s.resets = 3;
  CHECK_FALSE(d.onStatus(s));  // primes
  s.uptimeS = 110;
  CHECK_FALSE(d.onStatus(s));
  CHECK_FALSE(d.onStatus(s));  // equal uptime is not a reboot
  s.uptimeS = 109;
  CHECK(d.onStatus(s));
  s.uptimeS = 200;
  CHECK_FALSE(d.onStatus(s));
  s.resets = 4;
  CHECK(d.onStatus(s));
  s.resets = 2;
  s.uptimeS = 300;
  CHECK(d.onStatus(s));
  CHECK_FALSE(d.onStatus(s));
  d.reset();
  s.uptimeS = 1;
  CHECK_FALSE(d.onStatus(s));  // primes again
}

namespace {
ValveData vd(uint8_t valve, uint8_t status, uint32_t oc, uint32_t cc, uint32_t moves) {
  ValveData d;
  d.valve = valve;
  d.status = status;
  d.openCount = oc;
  d.closeCount = cc;
  d.moves = moves;
  return d;
}
}  // namespace

TEST_CASE("reboot: v1 heuristic on gvlvd") {
  RebootDetector d;
  // Never calibrated valve: zeros are normal.
  CHECK_FALSE(d.onValveData(vd(0, 5, 0, 0, 0)));
  CHECK_FALSE(d.onValveData(vd(0, 8, 0, 0, 0)));
  // Calibrated, then zeros in each boot status.
  for (uint8_t st : {5, 6, 8}) {
    CAPTURE(st);
    CHECK_FALSE(d.onValveData(vd(1, 1, 3000, 0, 5)));
    CHECK(d.onValveData(vd(1, st, 0, 0, 0)));
    CHECK_FALSE(d.onValveData(vd(1, st, 0, 0, 0)));  // reported once
  }
  CHECK_FALSE(d.onValveData(vd(1, 1, 0, 3000, 5)));  // cc alone marks calibrated
  for (uint8_t st : {0, 1, 2, 3, 4, 7, 9, 10}) {
    CAPTURE(st);
    CHECK_FALSE(d.onValveData(vd(1, st, 0, 0, 0)));
  }
  CHECK_FALSE(d.onValveData(vd(1, 5, 0, 0, 1)));  // moves != 0
  CHECK_FALSE(d.onValveData(vd(1, 5, 1, 0, 0)));  // still has counts
  CHECK_FALSE(d.onValveData(vd(1, 5, 0, 1, 0)));
  CHECK(d.onValveData(vd(1, 5, 0, 0, 0)));
  CHECK_FALSE(d.onValveData(vd(12, 5, 0, 0, 0)));  // out of range ignored
  CHECK_FALSE(d.onValveData(vd(12, 1, 10, 10, 0)));
}

TEST_CASE("reboot: one detection forgets every valve") {
  RebootDetector d;
  for (uint8_t v = 0; v < kValveCount; ++v) CHECK_FALSE(d.onValveData(vd(v, 1, 100, 100, 0)));
  CHECK(d.onValveData(vd(3, 8, 0, 0, 0)));
  for (uint8_t v = 0; v < kValveCount; ++v) CHECK_FALSE(d.onValveData(vd(v, 8, 0, 0, 0)));
}

TEST_CASE("reboot: reset forgets calibration history") {
  RebootDetector d;
  CHECK_FALSE(d.onValveData(vd(11, 1, 100, 100, 0)));
  d.reset();
  CHECK_FALSE(d.onValveData(vd(11, 5, 0, 0, 0)));
}

TEST_CASE("reboot: link Down -> Up only") {
  RebootDetector d;
  const LinkState all[] = {LinkState::Unknown, LinkState::Up,      LinkState::Degraded,
                           LinkState::Down,    LinkState::Booting, LinkState::Suspended};
  for (LinkState a : all) {
    for (LinkState b : all) {
      CAPTURE(linkStateName(a));
      CAPTURE(linkStateName(b));
      CHECK(d.onLinkState(a, b) == (a == LinkState::Down && b == LinkState::Up));
    }
  }
}

// ================================================================ edge cases

TEST_CASE("link: a request with a command but no text is invalid") {
  LinkPolicy lp;
  RequestLine r = valveData(1);
  r.len = 0;
  CHECK(lp.enqueue(r, Priority::User) == EnqueueResult::Invalid);
  r.len = 1;
  CHECK(lp.enqueue(r, Priority::User) == EnqueueResult::Queued);
}

TEST_CASE("link: eviction looks at the newest entry only") {
  LinkPolicy lp;
  for (uint8_t i = 0; i < 12; ++i) lp.enqueue(calibrate(i), Priority::User);
  for (uint8_t i = 0; i < 11; ++i) lp.enqueue(setTarget(i, 1), Priority::Config);
  lp.enqueue(valveData(0), Priority::Poll);
  REQUIRE(lp.queued() == 24);
  CHECK(lp.enqueue(setTarget(11, 1), Priority::Config) == EnqueueResult::Queued);
  CHECK(lp.stats().evictions == 1);
  CHECK(lp.queued(Priority::Poll) == 0);
}

TEST_CASE("link: poll() during the hold-off keeps Booting") {
  LinkPolicy lp;
  lp.onStmReset(100, true);
  lp.enqueue(valveData(0), Priority::Config);
  Completion c;
  CHECK_FALSE(lp.poll(200, c));
  CHECK(lp.state(200) == LinkState::Booting);
  CHECK(lp.nextToSend(300) == nullptr);
  CHECK(lp.nextToSend(5100) != nullptr);
}

TEST_CASE("link: resetMinTimeouts 0 still needs a timeout") {
  LinkParams p = noRetry();
  p.resetMinTimeouts = 0;
  p.resetMinSpanMs = 0;
  LinkPolicy lp(p);
  CHECK_FALSE(lp.shouldResetStm(1000000));
  uint32_t now = 0;
  failRequests(lp, now, 1, 0);
  CHECK(lp.shouldResetStm(now));
}

TEST_CASE("link: resetMinTimeouts 1 resets after the first timeout and span") {
  LinkParams p = noRetry();
  p.resetMinTimeouts = 1;
  LinkPolicy lp(p);
  uint32_t now = 0;
  failRequests(lp, now, 1, 0);
  CHECK_FALSE(lp.shouldResetStm(400 + 59999));
  CHECK(lp.shouldResetStm(400 + 60000));
}

TEST_CASE("link: STM reset clears the inter-request gap") {
  LinkParams p;
  p.bootHoldoffMs = 0;
  p.interRequestGapMs = 100;
  LinkPolicy lp(p);
  lp.enqueue(valveData(0), Priority::Poll);
  send(lp, 0);
  Completion c;
  REQUIRE(lp.onReply(reply(gvlvd(0).c_str()), 10, c));
  lp.onStmReset(11, false);
  lp.enqueue(valveData(1), Priority::Config);
  CHECK(lp.nextToSend(12) != nullptr);
}

TEST_CASE("link: STM reset keeps every User/Config entry and nothing stale") {
  LinkPolicy lp;
  lp.enqueue(calibrate(0), Priority::User);
  lp.enqueue(calibrate(1), Priority::User);
  lp.enqueue(setTarget(2, 5), Priority::Config);
  lp.enqueue(valveData(3), Priority::Poll);
  CHECK(send(lp, 0) == "staln 0 \r\n");  // leaves a stale copy past the end
  lp.onStmReset(1, false);
  CHECK(lp.queued() == 2);
  CHECK(lp.queued(Priority::User) == 1);
  CHECK(lp.queued(Priority::Config) == 1);
  CHECK(lp.queued(Priority::Poll) == 0);
  CHECK(send(lp, 5001) == "staln 1 \r\n");
  Completion c;
  REQUIRE(lp.onReply(reply("staln"), 5002, c));
  CHECK(send(lp, 5010) == "stgtp 2 5 \r\n");
  REQUIRE(lp.onReply(reply("stgtp"), 5011, c));
  CHECK(lp.nextToSend(5020) == nullptr);

  // A stale non-Poll slot just past the end is never resurrected.
  LinkPolicy lq;
  lq.enqueue(calibrate(0), Priority::User);
  lq.enqueue(calibrate(1), Priority::User);
  send(lq, 0);
  lq.onStmReset(1, false);
  CHECK(lq.queued() == 1);
}

TEST_CASE("link: downAfter 1 goes Down on the first timeout") {
  LinkParams p = noRetry();
  p.downAfter = 1;
  LinkPolicy lp(p);
  uint32_t now = 0;
  failRequests(lp, now, 1, 0);
  CHECK(lp.state(now) == LinkState::Down);
}

TEST_CASE("link: a replaced stgtp retry gets a fresh retry budget") {
  LinkPolicy lp;
  lp.enqueue(setTarget(3, 20), Priority::Config, 1);
  uint32_t now = 0;
  CHECK(send(lp, now) == "stgtp 3 20 \r\n");
  now += 400;
  Completion c;
  CHECK_FALSE(lp.poll(now, c));  // re-queued as retry (1 attempt made)
  REQUIRE(lp.queued() == 1);
  CHECK(lp.enqueue(setTarget(3, 60), Priority::Config, 2) == EnqueueResult::Coalesced);
  CHECK(lp.queued() == 1);
  // The new target gets 1 + retries attempts of its own.
  for (int attempt = 1; attempt <= 2; ++attempt) {
    now += 10;
    CHECK(send(lp, now) == "stgtp 3 60 \r\n");
    now += 400;
    CHECK_FALSE(lp.poll(now, c));
  }
  now += 10;
  CHECK(send(lp, now) == "stgtp 3 60 \r\n");
  now += 400;
  REQUIRE(lp.poll(now, c));
  CHECK(c.outcome == Outcome::Timeout);
  CHECK(c.attempts == 3);
  CHECK(c.tag == 2);
  CHECK(lp.stats().timeouts == 4);
}

TEST_CASE("link: an identical non-stgtp line keeps its retry count") {
  LinkPolicy lp;
  lp.enqueue(valveData(1), Priority::Poll);
  uint32_t now = 0;
  CHECK(send(lp, now) == "gvlvd 1 \r\n");
  now += 400;
  Completion c;
  CHECK_FALSE(lp.poll(now, c));
  CHECK(lp.enqueue(valveData(1), Priority::Poll) == EnqueueResult::Coalesced);
  now += 10;
  CHECK(send(lp, now) == "gvlvd 1 \r\n");
  now += 400;
  CHECK_FALSE(lp.poll(now, c));
  now += 10;
  CHECK(send(lp, now) == "gvlvd 1 \r\n");
  now += 400;
  REQUIRE(lp.poll(now, c));
  CHECK(c.attempts == 3);
}
