// Fork-per-case doctest runner of the native glue suites (software_stm32 and
// software_esp32_revamped test/native/glue). fork_main.cpp is linked into every glue test
// executable instead of a doctest main:
//   - every TEST_CASE runs in its own child process (doctest --dt-order-by=file --dt-first=i
//     --dt-last=i), so the glue's file-static state starts fresh in each case like after a boot;
//   - an alarm of VDM_CASE_TIMEOUT_S seconds (default 5) per boot stops a case that hangs;
//   - a case can span several simulated boots: reboot() saves the persistent stores through
//     the project's hooks and the runner starts the case again with boot() + 1.
//
// Runner options (command line or environment); every other argument goes to doctest:
//   --fail-fast   VDM_FAIL_FAST=1     stop at the first failing case (abort-after=1 inside it)
//   --no-fork     VDM_GLUE_NOFORK=1   run every case in this process (debugger); no reboots
//                 VDM_CASE_TIMEOUT_S  seconds per boot of a case, 0 = no limit (default 5)
//                 VDM_MAX_BOOTS       boots per case (default 8)
//
// Exit code of the runner (fork mode), the first that applies; tools/mutation/mutate.py tells a
// kill from a timeout and from a broken environment by it:
//   1    a case failed: an assertion, an invariant, a crash, too many boots
//   125  the runner itself failed: fork, waitpid, the hand-off directory, the hand-off file of the
//        stores (Hooks::save or Hooks::load returned false) or the reset kind file
//   124  a case ran out of VDM_CASE_TIMEOUT_S (a timeout, not a kill)
//   0    every case passed
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace testkit {

enum class Reset : uint8_t { PowerOn, Pin, Software, Watchdog };

// Boot index of the running case, 0 for its first boot.
unsigned boot();

// Reset that started this boot; PowerOn at boot 0.
Reset lastReset();

// Ends this boot: saves the stores (Hooks::save) to the hand-off file and exits with 75; the
// runner starts the case again with boot() + 1 and lastReset() == kind, loads the stores
// (Hooks::load) and, for kind == PowerOn, calls Hooks::powerOn. A boot that already has a failed
// assertion or a broken invariant (Hooks::checkInvariants) does not reboot: the case ends as
// failed.
[[noreturn]] void reboot(Reset kind);

struct Hooks {
  // Boot 0 and every PowerOn reboot: power-on content of the warm RAM (.noinit, RTC: 0xA5),
  // volatile fakes reset. Persistent stores (EEPROM, NVS, flash files) are not touched.
  void (*powerOn)();
  // Persistent stores and warm RAM to / from the hand-off file. false means that the file could
  // not be written or read, a failure of the environment and not of the code under test: the
  // runner exits with 125. A store state that must fail the case is a checkInvariants message.
  bool (*save)(const char* path);
  bool (*load)(const char* path);
  // After every case and before every reboot(); a message (not nullptr) fails the case even
  // without a failed assertion.
  const char* (*checkInvariants)();
};

// One set of hooks per project (glue/runner_hooks.cpp), usually from a static initializer.
void setHooks(const Hooks& hooks);

// Helpers for Hooks::save and Hooks::load: fixed memory regions to and from a hand-off file.
// loadRegions() fails when the file does not hold exactly these region sizes.
struct Region {
  void* data;
  size_t size;
};
bool saveRegions(const char* path, const Region* regions, size_t count);
bool loadRegions(const char* path, const Region* regions, size_t count);

// Address and size of a section of the running executable (e.g. ".noinit"), read from its ELF
// section table: no linker script needed, so it also works with mold. GCC's ASan puts no
// redzones into user sections, so Hooks may fill or copy the whole section. False when the
// executable has no such section.
bool sectionBounds(const char* name, uint8_t*& start, size_t& size);

}  // namespace testkit
