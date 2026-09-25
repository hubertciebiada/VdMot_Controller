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
