// RestartGate: STM EEPROM save before an ESP restart.
#include "doctest.h"
#include "vdm/restart_gate.h"

using namespace vdm;

using St = RestartGate::Step;

TEST_CASE("RestartGate: request first, proceed on every final answer") {
  const StmSaveState finals[] = {StmSaveState::Saved, StmSaveState::Unavailable,
                                 StmSaveState::TimedOut};
  for (StmSaveState f : finals) {
    RestartGate g;
    CHECK(g.waitedMs(500) == 0);
    CHECK(g.update(f, 1000) == St::RequestStmSave);  // the answer of an earlier restart counts not
    CHECK(g.update(StmSaveState::Waiting, 1100) == St::Wait);
    CHECK(g.update(StmSaveState::Idle, 1200) == St::Wait);
    CHECK(g.waitedMs(1200) == 200);
    CHECK(g.update(f, 1300) == St::Proceed);
    CHECK_FALSE(g.guardExpired());
    CHECK(g.update(StmSaveState::Waiting, 1400) == St::Proceed);  // final
    CHECK_FALSE(g.guardExpired());
  }
}

TEST_CASE("RestartGate: the guard proceeds at 12000 ms, not 11999") {
  RestartGate g;
  CHECK(g.update(StmSaveState::Idle, 0xFFFFF000u) == St::RequestStmSave);
  CHECK(g.update(StmSaveState::Waiting, 0xFFFFF000u + 11999u) == St::Wait);
  CHECK_FALSE(g.guardExpired());
  CHECK(g.update(StmSaveState::Waiting, 0xFFFFF000u + 12000u) == St::Proceed);
  CHECK(g.guardExpired());
  CHECK(g.waitedMs(0xFFFFF000u + 12000u) == 12000);
  CHECK(g.update(StmSaveState::Waiting, 0xFFFFF000u + 12001u) == St::Proceed);
  CHECK(RestartGate::kGuardMs == 12000);
}

TEST_CASE("RestartGate: reset starts over") {
  RestartGate g;
  g.update(StmSaveState::Idle, 0);
  CHECK(g.update(StmSaveState::Waiting, 12000) == St::Proceed);
  g.reset();
  CHECK_FALSE(g.guardExpired());
  CHECK(g.waitedMs(20000) == 0);
  CHECK(g.update(StmSaveState::Saved, 20000) == St::RequestStmSave);
  CHECK(g.update(StmSaveState::Waiting, 20001) == St::Wait);
}
