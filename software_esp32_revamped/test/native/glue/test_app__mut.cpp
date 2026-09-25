// Edge cases of src/app.cpp: the exact second and resource periods, the queue never blocks,
// the snapshot accessors before the first snapshot.
#include <Arduino.h>

#include "app.h"
#include "glue_test.h"

namespace {

TaskFunction_t appTask() {
  for (const fakes::TaskRecord& t : fakes::rtos().tasks) {
    if (t.name == "app") return t.fn;
  }
  return nullptr;
}

void runPasses(long passes) {
  fakes::rtos().stopAfterYields(passes);
  CHECK_THROWS_AS(appTask()(nullptr), fakes::YieldLimit);
}

// setup(), then the clock moved so that the first app task pass runs at `ms`.
void setupFirstPassAt(uint64_t ms) {
  glue::begin();
  app::setup();
  REQUIRE(fakes::nowMs() <= ms);
  fakes::advanceMs(ms - fakes::nowMs());
}

}  // namespace

TEST_CASE("app snapshot: protocol and revision are 0 before the first snapshot") {
  glue::begin();
  app::setup();
  CHECK(app::stmProtocol() == 0);
  CHECK(app::stmSnapshotRevision() == 0);
}

TEST_CASE("app submit/receive: a full or an empty queue never waits") {
  glue::begin();
  app::setup();
  app::Command c;
  for (uint8_t i = 0; i < 16; ++i) REQUIRE(app::submit(c));
  const uint64_t t = fakes::nowMs();
  CHECK_FALSE(app::submit(c));
  CHECK(fakes::nowMs() == t);
  app::Command out;
  for (uint8_t i = 0; i < 16; ++i) REQUIRE(app::receive(out));
  CHECK_FALSE(app::receive(out));
  CHECK(fakes::nowMs() == t);
}

TEST_CASE("app task: the once-a-second work runs at 1000 ms of uptime, not before") {
  SUBCASE("1000 ms") {
    setupFirstPassAt(1000);
    runPasses(1);
    CHECK(sib::net().services.size() == 1);
  }
  SUBCASE("999 ms") {
    setupFirstPassAt(999);
    runPasses(1);
    CHECK(sib::net().services.empty());
  }
}

TEST_CASE("app task: resources are sampled 10000 ms after the start, not 1 ms earlier") {
  glue::begin();
  app::setup();
  fakes::esp().freeHeap = 29000;
  fakes::esp().minFreeHeap = 20000;
  fakes::esp().maxAllocHeap = 4000;
  // every pass 101 ms apart: the 100th pass runs 9999 ms after the start
  fakes::rtos().onDelay = [](uint32_t) { fakes::advanceMs(1); };
  runPasses(100);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::LowHeap));
}
