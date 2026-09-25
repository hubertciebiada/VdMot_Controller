// RestartGate: an expired guard stays final.
#include "doctest.h"
#include "vdm/restart_gate.h"

using namespace vdm;

using St = RestartGate::Step;

TEST_CASE("RestartGate: after the guard expired it proceeds even when the clock reads the start again") {
  RestartGate g;
  CHECK(g.update(StmSaveState::Idle, 5000) == St::RequestStmSave);
  CHECK(g.update(StmSaveState::Waiting, 5000 + RestartGate::kGuardMs) == St::Proceed);
  CHECK(g.guardExpired());
  // 2^32 ms later (millis() wrapped): the elapsed time is below the guard again
  CHECK(g.update(StmSaveState::Waiting, 5000) == St::Proceed);
  CHECK(g.update(StmSaveState::Idle, 5001) == St::Proceed);
}
