// Common part of the ESP glue tests: every TEST_CASE runs in its own process (tools/native/testkit)
// and starts with glue::begin(); no SUBCASE, fake time only, whole outputs and whole call logs,
// boundary pairs, one failure injection per error branch.
#pragma once

#include <string>

#include "doctest.h"
#include "fakes/fakes.h"
#include "fakes/http.h"
#include "runner_hooks.h"
#include "siblings.h"
#include "testkit.h"

namespace glue {

// First statement of every glue test case (and of every boot of a multi-boot case): the volatile
// fakes and every sibling fake back to their defaults, the journal empty. The persistent stores
// (NVS, LittleFS, RTC section, OTA states) stay as the boot found them.
inline void begin() {
  fakes::reset();
  sib::reset();
}

// Runs fn like the device runs code that may restart: esp_restart() ends the boot with a software
// reset (testkit::reboot: the case goes on with the next boot). A test that expects the restart
// itself uses CHECK_THROWS_AS(..., fakes::Restarted) instead.
template <class F>
void run(F&& fn) {
  try {
    fn();
  } catch (const fakes::Restarted&) {
    testkit::reboot(testkit::Reset::Software);
  }
}

}  // namespace glue
