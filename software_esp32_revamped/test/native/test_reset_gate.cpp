// ResetGate: waiting for the STM EEPROM before a reset or restart.
#include "doctest.h"
#include "vdm/reset_gate.h"

using namespace vdm;

namespace {
using State = ResetGate::State;
}  // namespace

TEST_CASE("gate: constants") {
  CHECK(ResetGate::kPollMs == 500);
  CHECK(ResetGate::kMaxWaitMs == 10000);
  ResetGate g;
  CHECK(g.state() == State::Idle);
  CHECK(g.update(100000) == State::Idle);
  CHECK_FALSE(g.pollDue(0, false));
  CHECK(g.waitedMs(5000) == 0);
}

TEST_CASE("gate: an STM that does not answer is Ready at once") {
  ResetGate g;
  g.begin(1000, false);
  CHECK(g.state() == State::Ready);
  CHECK_FALSE(g.pollDue(1000, false));
  CHECK(g.update(20000) == State::Ready);
}

TEST_CASE("gate: polls at once, waits for queued requests and the reply, then every 500 ms") {
  ResetGate g;
  g.begin(1000, true);
  CHECK(g.state() == State::Waiting);
  CHECK(g.pollDue(1000, false));
  CHECK_FALSE(g.pollDue(1000, true));  // User/Config requests first
  g.onPollSent(1000);
  CHECK_FALSE(g.pollDue(5000, false));  // one eepst at a time
  g.onEepst(true, false, 1100);         // "eepst 0": a write is pending
  CHECK(g.state() == State::Waiting);
  CHECK_FALSE(g.pollDue(1100 + 499, false));
  CHECK(g.pollDue(1100 + 500, false));
  CHECK_FALSE(g.pollDue(1100 + 500, true));
  g.onPollSent(1600);
  g.onEepst(false, false, 2000);  // timed out: the next poll 500 ms later
  CHECK_FALSE(g.pollDue(2499, false));
  CHECK(g.pollDue(2500, false));
  g.onPollSent(2500);
  g.onEepst(false, true, 2600);  // no answer is never idle
  CHECK(g.state() == State::Waiting);
  g.onPollSent(3100);
  g.onEepst(true, true, 3200);
  CHECK(g.state() == State::Ready);
  CHECK_FALSE(g.pollDue(10000, false));
  CHECK(g.update(1000 + 10000) == State::Ready);
  CHECK(g.waitedMs(3200) == 2200);
}

TEST_CASE("gate: times out 10 s after begin") {
  ResetGate g;
  g.begin(5000, true);
  CHECK(g.update(5000 + 9999) == State::Waiting);
  CHECK(g.update(5000 + 10000) == State::TimedOut);
  CHECK(g.waitedMs(5000 + 10000) == 10000);
  CHECK_FALSE(g.pollDue(20000, false));
  g.onEepst(true, true, 20000);  // a late reply does not change the outcome
  CHECK(g.state() == State::TimedOut);
  g.reset();
  CHECK(g.state() == State::Idle);
  CHECK(g.waitedMs(30000) == 0);
}

TEST_CASE("gate: begin again restarts the wait and the first poll") {
  ResetGate g;
  g.begin(0, true);
  g.onPollSent(0);
  g.begin(20000, true);
  CHECK(g.pollDue(20000, false));
  CHECK(g.update(29999) == State::Waiting);
  g.reset();
  g.begin(40000, true);
  g.onPollSent(40000);
  g.reset();
  g.begin(50000, true);
  CHECK(g.pollDue(50000, false));  // reset forgot the poll in flight
}
