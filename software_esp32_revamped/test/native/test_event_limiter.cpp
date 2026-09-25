// EventRateLimiter.
#include "doctest.h"
#include "vdm/event_limiter.h"

using namespace vdm;

namespace {

Event event(EventCode c, uint8_t valve = 0, Severity sev = Severity::Warning) {
  return makeEvent(c, sev, valve, 0, 0, "");
}

}  // namespace

TEST_CASE("EventRateLimiter: severity gate and per-key limit") {
  EventRateLimiter rl;
  CHECK_FALSE(rl.allow(event(EventCode::TargetSet, 0, Severity::Info), 0));
  CHECK_FALSE(rl.allow(event(EventCode::ValveStateChanged, 0, Severity::Debug), 0));
  CHECK(rl.suppressed() == 0);
  CHECK(rl.allow(event(EventCode::CalibOk, 0, Severity::Info), 0));
  CHECK(rl.allow(event(EventCode::EarlyStop, 0), 0));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 0), 1));
  CHECK(rl.suppressed() == 1);
  CHECK(rl.allow(event(EventCode::EarlyStop, 1), 1));             // other valve
  CHECK(rl.allow(event(EventCode::CmdRejected, 0), 1));           // other code
  CHECK(rl.allow(event(EventCode::LinkDown, kNoValve, Severity::Error), 1));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 0), 599999));
  CHECK(rl.suppressed() == 2);
  CHECK(rl.allow(event(EventCode::EarlyStop, 0), 600000));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 0), 600001));
  CHECK(rl.allow(event(EventCode::StmFlashFailed, kNoValve, Severity::Critical), 600001));
}

TEST_CASE("EventRateLimiter: hourly token bucket") {
  EventRateLimiter rl(600000, 30);
  for (uint8_t i = 0; i < 30; ++i) {
    CHECK(rl.allow(event(EventCode::EarlyStop, i), 1000));
  }
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 100), 1000));
  CHECK(rl.suppressed() == 1);
  // One token per 120 s.
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 101), 1000 + 119999));
  CHECK(rl.allow(event(EventCode::EarlyStop, 102), 1000 + 120000));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 103), 1000 + 120000));
  // Frequent calls must not lose the fractional refill.
  EventRateLimiter f(600000, 30);
  for (uint8_t i = 0; i < 30; ++i) REQUIRE(f.allow(event(EventCode::EarlyStop, i), 0));
  uint32_t t = 0;
  while (t < 119980) {
    t += 20;
    CHECK_FALSE(f.allow(event(EventCode::TargetSet, 0, Severity::Info), t));
  }
  CHECK_FALSE(f.allow(event(EventCode::EarlyStop, 200), 119999));
  CHECK(f.allow(event(EventCode::EarlyStop, 201), 120000));
  // The bucket never exceeds its capacity.
  EventRateLimiter c(1, 2);
  CHECK(c.allow(event(EventCode::EarlyStop, 0), 0));
  CHECK(c.allow(event(EventCode::EarlyStop, 1), 0));
  CHECK_FALSE(c.allow(event(EventCode::EarlyStop, 2), 0));
  CHECK(c.allow(event(EventCode::EarlyStop, 3), 36000000));
  CHECK(c.allow(event(EventCode::EarlyStop, 4), 36000000));
  CHECK_FALSE(c.allow(event(EventCode::EarlyStop, 5), 36000000));
  // Refill precision at one token per hour, and the remainder is dropped
  // when the bucket is full.
  EventRateLimiter h(1, 1);
  CHECK(h.allow(event(EventCode::EarlyStop, 0), 0));
  CHECK_FALSE(h.allow(event(EventCode::EarlyStop, 1), 3599999));
  CHECK(h.allow(event(EventCode::EarlyStop, 2), 3600000));
  EventRateLimiter q(1, 1);
  CHECK(q.allow(event(EventCode::EarlyStop, 0), 0));
  CHECK(q.allow(event(EventCode::EarlyStop, 1), 3601800));
  CHECK_FALSE(q.allow(event(EventCode::EarlyStop, 2), 7200000));
  CHECK(q.allow(event(EventCode::EarlyStop, 3), 7201800));
  // No budget at all.
  EventRateLimiter z(600000, 0);
  CHECK_FALSE(z.allow(event(EventCode::EarlyStop, 0), 0));
  CHECK_FALSE(z.allow(event(EventCode::EarlyStop, 0), 3600000));
  CHECK(z.suppressed() == 2);
}

TEST_CASE("EventRateLimiter: key table recycles the least recently used entry") {
  EventRateLimiter rl(1000000000u, 1000);
  for (uint8_t i = 0; i < EventRateLimiter::kKeys; ++i) {
    REQUIRE(rl.allow(event(EventCode::EarlyStop, i), 10u + i));
  }
  CHECK(rl.allow(event(EventCode::EarlyStop, 200), 100));   // evicts valve 0
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 5), 101));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 200), 101));
  CHECK(rl.allow(event(EventCode::EarlyStop, 0), 102));     // evicts valve 1
  CHECK(rl.allow(event(EventCode::EarlyStop, 1), 103));     // evicts valve 2
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 3), 104));
  CHECK(rl.allow(event(EventCode::EarlyStop, 2), 105));
}

TEST_CASE("EventRateLimiter: of equally old entries the first one is recycled") {
  EventRateLimiter rl(1000000000u, 1000);
  for (uint8_t i = 0; i < EventRateLimiter::kKeys; ++i) {
    REQUIRE(rl.allow(event(EventCode::EarlyStop, i), 10));
  }
  CHECK(rl.allow(event(EventCode::EarlyStop, 200), 20));  // evicts valve 0
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, EventRateLimiter::kKeys - 1), 21));
  CHECK(rl.allow(event(EventCode::EarlyStop, 0), 22));
}

TEST_CASE("EventRateLimiter: entries expire across a millis() wrap") {
  EventRateLimiter rl(600000, 30);
  const uint32_t t0 = 0xFFFFFF00u;
  CHECK(rl.allow(event(EventCode::EarlyStop, 0), t0));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 0), t0 + 599999u));
  CHECK(rl.allow(event(EventCode::EarlyStop, 0), t0 + 600000u));
}

TEST_CASE("EventRateLimiter: Error events have their own bucket") {
  EventRateLimiter rl;
  for (uint8_t i = 0; i < 30; ++i) REQUIRE(rl.allow(event(EventCode::EarlyStop, i), 0));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 30), 0));  // Normal bucket empty
  CHECK(rl.allow(event(EventCode::LinkDown, kNoValve, Severity::Error), 0));
  CHECK(rl.suppressed() == 1);
  EventRateLimiter e;
  for (uint8_t i = 0; i < 30; ++i) {
    REQUIRE(e.allow(event(EventCode::CalibFailed, i, Severity::Error), 0));
  }
  CHECK_FALSE(e.allow(event(EventCode::CalibFailed, 30, Severity::Error), 0));  // Error bucket empty
  CHECK(e.allow(event(EventCode::EarlyStop, 0), 0));                             // Warning still passes
  CHECK(e.suppressed() == 1);
  // The Error bucket refills on its own: one token per 3600000 / 30 ms.
  CHECK_FALSE(e.allow(event(EventCode::CalibFailed, 31, Severity::Error), 119999));
  CHECK(e.allow(event(EventCode::CalibFailed, 32, Severity::Error), 120000));
  // Its size is the third parameter.
  EventRateLimiter s(600000, 30, 1);
  CHECK(s.allow(event(EventCode::LinkDown, 0, Severity::Error), 0));
  CHECK_FALSE(s.allow(event(EventCode::LinkDown, 1, Severity::Error), 0));
  CHECK(s.allow(event(EventCode::EarlyStop, 1), 0));
  // An Error key is held perKeyMs (not the Critical minute).
  EventRateLimiter k(600000, 30, 30, 1000);
  CHECK(k.allow(event(EventCode::LinkDown, 0, Severity::Error), 0));
  CHECK_FALSE(k.allow(event(EventCode::LinkDown, 0, Severity::Error), 599999));
  CHECK(k.allow(event(EventCode::LinkDown, 0, Severity::Error), 600000));
}

TEST_CASE("EventRateLimiter: Critical events bypass the buckets, one per key per minute") {
  EventRateLimiter rl(600000, 30, 30);
  for (uint8_t i = 0; i < 30; ++i) {
    REQUIRE(rl.allow(event(EventCode::EarlyStop, i), 0));
    REQUIRE(rl.allow(event(EventCode::CalibFailed, i, Severity::Error), 0));
  }
  const Event crit = event(EventCode::StmFlashFailed, kNoValve, Severity::Critical);
  CHECK(rl.allow(crit, 0));
  CHECK_FALSE(rl.allow(crit, 59999));
  CHECK(rl.suppressed() == 1);
  CHECK(rl.allow(crit, 60000));
  CHECK_FALSE(rl.allow(crit, 119999));
  CHECK(rl.allow(crit, 120000));
  // Critical takes no tokens: both buckets refilled exactly one each by now.
  CHECK(rl.allow(event(EventCode::EarlyStop, 100), 120000));
  CHECK_FALSE(rl.allow(event(EventCode::EarlyStop, 101), 120000));
  CHECK(rl.allow(event(EventCode::CalibFailed, 100, Severity::Error), 120000));
  CHECK_FALSE(rl.allow(event(EventCode::CalibFailed, 101, Severity::Error), 120000));
  // Another per-key time for Critical.
  EventRateLimiter c(600000, 30, 30, 1000);
  CHECK(c.allow(crit, 0));
  CHECK_FALSE(c.allow(crit, 999));
  CHECK(c.allow(crit, 1000));
}

TEST_CASE("EventRateLimiter: events that do not reach MQTT are refused without counting") {
  EventRateLimiter rl;
  CHECK_FALSE(rl.allow(event(EventCode::FilesRemoved, kNoValve, Severity::Warning), 0));  // column No
  CHECK_FALSE(rl.allow(event(EventCode::LinkDown, kNoValve, Severity::Info), 0));  // below Warning
  CHECK(rl.suppressed() == 0);
  CHECK(rl.allow(event(EventCode::CalibOk, 0, Severity::Info), 0));  // Always
}

namespace {

Event at(EventCode c, uint8_t valve, uint32_t seq, Severity sev = Severity::Warning) {
  Event e = makeEvent(c, sev, valve, 60, 0, "");
  e.seq = seq;
  return e;
}

}  // namespace

TEST_CASE("EventAggregator: one event for the same code on several valves") {
  EventAggregator a;
  PublishEvent f;
  for (uint8_t v = 0; v < kValveCount; ++v) {
    CHECK_FALSE(a.offer(at(EventCode::ValveStale, v, 10u + v), v, f));
  }
  PublishEvent out;
  CHECK_FALSE(a.poll(1999, out));
  REQUIRE(a.poll(2000, out));
  CHECK(out.valveMask == 0x0FFF);
  CHECK(out.event.valve == kAllValves);
  CHECK(out.event.seq == 10);  // the first event's
  CHECK(out.event.arg1 == 60);
  CHECK(out.event.code == EventCode::ValveStale);
  CHECK_FALSE(a.poll(100000, out));
  CHECK(a.duplicates() == 0);
}

TEST_CASE("EventAggregator: a single valve is the original event") {
  EventAggregator a;
  PublishEvent f;
  CHECK_FALSE(a.offer(at(EventCode::EarlyStop, 3, 7, Severity::Info), 100, f));
  PublishEvent out;
  CHECK_FALSE(a.poll(2099, out));
  REQUIRE(a.poll(2100, out));
  CHECK(out.valveMask == 0x0008);
  CHECK(out.event.valve == 3);
  CHECK(out.event.seq == 7);
  CHECK(out.event.severity == Severity::Info);
}

TEST_CASE("EventAggregator: highest severity, duplicates, non-valve events, flushAll, reset") {
  EventAggregator a;
  PublishEvent f;
  CHECK_FALSE(a.offer(at(EventCode::EarlyStop, 0, 1, Severity::Warning), 0, f));
  CHECK_FALSE(a.offer(at(EventCode::EarlyStop, 1, 2, Severity::Critical), 1, f));
  CHECK_FALSE(a.offer(at(EventCode::EarlyStop, 2, 3, Severity::Info), 2, f));
  CHECK_FALSE(a.offer(at(EventCode::EarlyStop, 1, 4, Severity::Error), 3, f));  // duplicate valve
  CHECK(a.duplicates() == 1);
  CHECK_FALSE(a.offer(at(EventCode::EarlyStop, kNoValve, 5), 4, f));  // not taken
  CHECK_FALSE(a.offer(at(EventCode::EarlyStop, kAllValves, 6), 4, f));
  CHECK_FALSE(a.offer(at(EventCode::EarlyStop, kValveCount, 6), 4, f));
  PublishEvent out;
  CHECK_FALSE(a.poll(5, out));
  REQUIRE(a.poll(5, out, true));
  CHECK(out.valveMask == 0x0007);
  CHECK(out.event.severity == Severity::Critical);
  CHECK(out.event.seq == 1);
  CHECK_FALSE(a.poll(5, out, true));
  // Two valves at the first event's severity when nothing is higher.
  CHECK_FALSE(a.offer(at(EventCode::EarlyStop, 4, 8, Severity::Error), 10, f));
  CHECK_FALSE(a.offer(at(EventCode::EarlyStop, 5, 9, Severity::Warning), 10, f));
  REQUIRE(a.poll(5000, out));
  CHECK(out.event.severity == Severity::Error);
  CHECK(out.valveMask == 0x0030);
  CHECK_FALSE(a.offer(at(EventCode::EarlyStop, 0, 7), 10, f));
  a.reset();
  CHECK_FALSE(a.poll(100000, out, true));
  CHECK(a.duplicates() == 1);  // reset keeps the counter
}

TEST_CASE("EventAggregator: a fifth code flushes the oldest slot") {
  EventAggregator a;
  PublishEvent f;
  const EventCode codes[] = {EventCode::EarlyStop, EventCode::ValveStale, EventCode::CalibFailed,
                             EventCode::TargetNotConfirmed, EventCode::CmdRejected};
  CHECK_FALSE(a.offer(at(codes[0], 0, 1), 10, f));
  CHECK_FALSE(a.offer(at(codes[1], 1, 2), 5, f));  // opened earlier: the oldest
  CHECK_FALSE(a.offer(at(codes[2], 2, 3), 20, f));
  CHECK_FALSE(a.offer(at(codes[3], 3, 4), 30, f));
  REQUIRE(a.offer(at(codes[4], 4, 5), 40, f));
  CHECK(f.event.code == EventCode::ValveStale);
  CHECK(f.event.valve == 1);
  CHECK(f.valveMask == 0x0002);
  // Oldest first when several windows passed.
  PublishEvent out;
  REQUIRE(a.poll(100000, out));
  CHECK(out.event.code == EventCode::EarlyStop);
  REQUIRE(a.poll(100000, out));
  CHECK(out.event.code == EventCode::CalibFailed);
  REQUIRE(a.poll(100000, out));
  CHECK(out.event.code == EventCode::TargetNotConfirmed);
  REQUIRE(a.poll(100000, out));
  CHECK(out.event.code == EventCode::CmdRejected);
  CHECK_FALSE(a.poll(100000, out));
  // Windows across a millis() wrap.
  EventAggregator w;
  CHECK_FALSE(w.offer(at(codes[0], 0, 1), 0xFFFFFF00u, f));
  CHECK_FALSE(w.poll(0xFFFFFF00u + 1999u, out));
  CHECK(w.poll(0xFFFFFF00u + 2000u, out));
}
