// Common part of the STM glue tests (harness-stm 6.1, contracts 17.5): every TEST_CASE runs in its
// own process (tools/native/testkit) and starts with glue::begin(); no SUBCASE, fake time only,
// whole outputs and whole call logs.
#pragma once

#include <string>

#include "doctest.h"
#include "fake_board.h"
#include "runner_hooks.h"
#include "stub_log.h"
#include "testkit.h"

namespace glue {

// First statement of every glue test case (and of every boot of a multi-boot case): the volatile
// fakes and every stub knob and global back to their defaults, the call log empty. The persistent
// stores (EEPROM, .noinit, RCC->CSR) stay as the boot found them.
inline void begin() {
  fake::reset();
  stub::reset();
}

// Runs fn like the device runs code that may reset: HAL_NVIC_SystemReset() ends the boot with a
// software reset, an expired watchdog with a watchdog reset (testkit::reboot: the case goes on with
// the next boot). A test that expects the reset itself uses CHECK_THROWS_AS instead.
template <class F>
void run(F&& fn) {
  try {
    fn();
  } catch (const fake::SystemReset&) {
    testkit::reboot(testkit::Reset::Software);
  } catch (const fake::WatchdogReset&) {
    testkit::reboot(testkit::Reset::Watchdog);
  }
}

}  // namespace glue
