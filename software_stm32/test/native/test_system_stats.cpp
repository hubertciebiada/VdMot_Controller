#include <stdint.h>

#include "doctest.h"
#include "vdm/system_stats.h"

using vdm::BootReason;
using vdm::ResetCounterCell;
using vdm::ResetFlags;

TEST_CASE("UptimeCounter: whole seconds, remainder carried") {
  vdm::UptimeCounter u;
  CHECK(u.seconds() == 0);
  u.update(999);
  CHECK(u.seconds() == 0);
  u.update(1000);
  CHECK(u.seconds() == 1);
  u.update(2500);
  CHECK(u.seconds() == 2);
  u.update(2999);
  CHECK(u.seconds() == 2);
  u.update(3000);
  CHECK(u.seconds() == 3);
  u.update(3000);
  CHECK(u.seconds() == 3);
}

TEST_CASE("UptimeCounter: continues across the 49.7 day wrap of millis()") {
  vdm::UptimeCounter u;
  uint32_t now = 0;
  // walk to just before the wrap in steps below the wrap period
  for (int i = 0; i < 4; ++i) {
    now += 0x3FFFFFFFu;
    u.update(now);
  }
  const uint32_t before = u.seconds();
  CHECK(before == 4294967u);  // 4 * 0x3FFFFFFF ms = 4294967.292 s
  now += 10000;               // wraps
  u.update(now);
  CHECK(u.seconds() == before + 10);
}

TEST_CASE("UptimeCounter: many small steps do not lose milliseconds") {
  vdm::UptimeCounter u;
  uint32_t now = 0;
  for (int i = 0; i < 100000; ++i) {
    now += 11;
    u.update(now);
  }
  CHECK(u.seconds() == 1100);
}

TEST_CASE("classifyReset: most specific flag wins") {
  ResetFlags f{};
  CHECK(vdm::classifyReset(f) == BootReason::Unknown);

  f = ResetFlags{};
  f.pin = true;
  CHECK(vdm::classifyReset(f) == BootReason::Pin);

  f.powerOn = true;
  f.brownOut = true;  // set together with power-on
  CHECK(vdm::classifyReset(f) == BootReason::PowerOn);

  f = ResetFlags{};
  f.brownOut = true;
  f.pin = true;
  CHECK(vdm::classifyReset(f) == BootReason::BrownOut);

  f = ResetFlags{};
  f.software = true;
  f.pin = true;
  CHECK(vdm::classifyReset(f) == BootReason::Software);

  f.independentWatchdog = true;
  CHECK(vdm::classifyReset(f) == BootReason::IndependentWatchdog);

  f = ResetFlags{};
  f.windowWatchdog = true;
  f.pin = true;
  CHECK(vdm::classifyReset(f) == BootReason::WindowWatchdog);

  f = ResetFlags{};
  f.lowPower = true;
  f.software = true;
  CHECK(vdm::classifyReset(f) == BootReason::LowPower);

  CHECK(static_cast<int>(BootReason::PowerOn) == 1);
  CHECK(static_cast<int>(BootReason::BrownOut) == 7);
}

TEST_CASE("countReset: counts warm resets, restarts after power-on") {
  ResetCounterCell cell{0xDEADBEEF, 1234, 0};  // random RAM after power-up
  CHECK(vdm::countReset(cell, BootReason::Pin) == 0);
  CHECK(vdm::countReset(cell, BootReason::Software) == 1);
  CHECK(vdm::countReset(cell, BootReason::IndependentWatchdog) == 2);
  CHECK(vdm::countReset(cell, BootReason::Pin) == 3);
  CHECK(vdm::countReset(cell, BootReason::PowerOn) == 0);
  CHECK(vdm::countReset(cell, BootReason::Unknown) == 1);
  CHECK(vdm::countReset(cell, BootReason::BrownOut) == 0);
}

TEST_CASE("countReset: damaged cell starts again") {
  ResetCounterCell cell{vdm::kResetCounterMagic, 5, ~5u};
  CHECK(vdm::countReset(cell, BootReason::Pin) == 6);
  cell.count = 100;  // check word no longer matches
  CHECK(vdm::countReset(cell, BootReason::Pin) == 0);
  CHECK(cell.magic == vdm::kResetCounterMagic);
  CHECK(cell.check == ~cell.count);
}

TEST_CASE("countReset: saturates") {
  ResetCounterCell cell{vdm::kResetCounterMagic, 0xFFFFFFFFu, 0};
  CHECK(vdm::countReset(cell, BootReason::Pin) == 0xFFFFFFFFu);
  CHECK(vdm::countReset(cell, BootReason::Pin) == 0xFFFFFFFFu);
}
