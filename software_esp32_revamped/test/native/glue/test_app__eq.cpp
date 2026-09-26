// Tests of src/app.cpp for a call the target cannot take: FreeRTOS reads the name given to
// xTaskGetHandle(), so the resource sample must never ask without one (the fake counts a null
// name as an RTOS violation).
#include "app.h"
#include "glue_test.h"

namespace {

TaskFunction_t appTaskFn() {
  for (const fakes::TaskRecord& t : fakes::rtos().tasks) {
    if (t.name == "app") return t.fn;
  }
  return nullptr;
}

void runAppTask(long passes) {
  fakes::rtos().stopAfterYields(passes);
  CHECK_THROWS_AS(appTaskFn()(nullptr), fakes::YieldLimit);
}

}  // namespace

TEST_CASE("app task: the resource sample asks FreeRTOS only for named library tasks") {
  glue::begin();
  app::setup();
  runAppTask(101);  // one sample after 10 s; the library tasks do not exist yet
  CHECK(fakes::rtos().violations == 0);
  static vdm::HealthSnapshot h;
  app::readHealth(h);
  CHECK(h.taskCount == 3);
}
