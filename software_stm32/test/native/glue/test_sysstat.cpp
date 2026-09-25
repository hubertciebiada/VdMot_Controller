// Smoke tests of src/sysstat.cpp (glue_sysstat): boot reason from RCC->CSR, reset counter in
// .noinit across boots, uptime across the millis() wrap, the safe-mode functions of wave 0.
#include "glue_test.h"
#include "sysstat.h"

using vdm::BootReason;

namespace {

BootReason captureWith(uint32_t flags) {
  RCC->CSR = flags;
  sysstat_capture_reset();
  return sysstat_boot_reason();
}

bool allA5(const std::vector<uint8_t>& bytes) {
  for (uint8_t b : bytes) {
    if (b != 0xA5) return false;
  }
  return !bytes.empty();
}

}  // namespace

TEST_CASE("sysstat: before the capture the reason is unknown and nothing is counted") {
  glue::begin();
  CHECK(sysstat_boot_reason() == BootReason::Unknown);
  CHECK(sysstat_resets() == 0);
  CHECK(sysstat_uptime_s() == 0);
}

TEST_CASE("sysstat: the first boot is a power-on with no resets; RMVF clears the flags") {
  glue::begin();
  sysstat_capture_reset();
  CHECK(sysstat_boot_reason() == BootReason::PowerOn);
  CHECK(sysstat_resets() == 0);
  CHECK(RCC->CSR.clears == 1);
  CHECK(static_cast<uint32_t>(RCC->CSR) == 0);
}

TEST_CASE("sysstat: each reset flag of RCC_CSR alone gives its boot reason") {
  glue::begin();
  CHECK(captureWith(RCC_CSR_LPWRRSTF) == BootReason::LowPower);
  CHECK(captureWith(RCC_CSR_WWDGRSTF) == BootReason::WindowWatchdog);
  CHECK(captureWith(RCC_CSR_IWDGRSTF) == BootReason::IndependentWatchdog);
  CHECK(captureWith(RCC_CSR_SFTRSTF) == BootReason::Software);
  CHECK(captureWith(RCC_CSR_PORRSTF) == BootReason::PowerOn);
  CHECK(captureWith(RCC_CSR_PINRSTF) == BootReason::Pin);
  CHECK(captureWith(RCC_CSR_BORRSTF) == BootReason::BrownOut);
  CHECK(captureWith(0) == BootReason::Unknown);
  CHECK(RCC->CSR.clears == 8);
}

TEST_CASE("sysstat: the flags of a watchdog, software and pin reset together give the most specific reason") {
  glue::begin();
  CHECK(captureWith(RCC_CSR_IWDGRSTF | RCC_CSR_PINRSTF) == BootReason::IndependentWatchdog);
  CHECK(captureWith(RCC_CSR_SFTRSTF | RCC_CSR_PINRSTF) == BootReason::Software);
  CHECK(captureWith(RCC_CSR_PORRSTF | RCC_CSR_PINRSTF | RCC_CSR_BORRSTF) == BootReason::PowerOn);
  CHECK(captureWith(RCC_CSR_BORRSTF | RCC_CSR_PINRSTF) == BootReason::BrownOut);
  CHECK(captureWith(RCC_CSR_LPWRRSTF | RCC_CSR_PINRSTF) == BootReason::LowPower);
  CHECK(captureWith(RCC_CSR_WWDGRSTF | RCC_CSR_PINRSTF) == BootReason::WindowWatchdog);
}

TEST_CASE("sysstat: the reset counter counts every warm reset and starts at 0 after a power-on") {
  glue::begin();
  // boot: 0 power-on, 1 and 2 pin, 3 software, 4 watchdog, 5 power-on
  static const BootReason kReason[] = {BootReason::PowerOn,  BootReason::Pin,
                                       BootReason::Pin,      BootReason::Software,
                                       BootReason::IndependentWatchdog, BootReason::PowerOn};
  static const uint32_t kResets[] = {0, 1, 2, 3, 4, 0};
  static const testkit::Reset kNext[] = {testkit::Reset::Pin, testkit::Reset::Pin, testkit::Reset::Software,
                                         testkit::Reset::Watchdog, testkit::Reset::PowerOn};
  const unsigned boot = testkit::boot();
  REQUIRE(boot < 6);
  sysstat_capture_reset();
  CHECK(sysstat_boot_reason() == kReason[boot]);
  CHECK(sysstat_resets() == kResets[boot]);
  if (boot < 5) testkit::reboot(kNext[boot]);
}

TEST_CASE("sysstat: the counter lives in .noinit: 0xA5 at power-on, written by the capture, kept by a pin reset") {
  glue::begin();
  if (testkit::boot() == 0) {
    CHECK(allA5(glue::noinitSnapshot()));
    sysstat_capture_reset();
    CHECK_FALSE(allA5(glue::noinitSnapshot()));
    testkit::reboot(testkit::Reset::Pin);
  }
  CHECK_FALSE(allA5(glue::noinitSnapshot()));
  sysstat_capture_reset();
  CHECK(sysstat_resets() == 1);
}

TEST_CASE("sysstat: a cell with random content counts from 0 after a warm reset") {
  glue::begin();
  if (testkit::boot() == 0) {
    // flags cleared but no capture: the next boot finds the power-on pattern in .noinit
    RCC->CSR |= RCC_CSR_RMVF;
    testkit::reboot(testkit::Reset::Pin);
  }
  sysstat_capture_reset();
  CHECK(sysstat_boot_reason() == BootReason::Pin);
  CHECK(sysstat_resets() == 0);
}

TEST_CASE("sysstat: the uptime counts seconds across the wrap of millis()") {
  glue::begin();
  sysstat_loop();
  fake::advanceMs(999);
  sysstat_loop();
  CHECK(sysstat_uptime_s() == 0);
  fake::advanceMs(1);
  sysstat_loop();
  CHECK(sysstat_uptime_s() == 1);
  fake::board.nowUs = (1ull << 31) * 1000;
  sysstat_loop();
  CHECK(sysstat_uptime_s() == 2147483);
  fake::board.nowUs = ((1ull << 32) - 500) * 1000;
  sysstat_loop();
  fake::advanceMs(1500);
  sysstat_loop();
  CHECK(millis() == 1000);
  CHECK(sysstat_uptime_s() == 4294968);
}

TEST_CASE("sysstat: safe mode is not active in wave 0 and ssafe 0 changes nothing") {
  glue::begin();
  sysstat_capture_reset();
  CHECK_FALSE(sysstat_safe_mode());
  CHECK(sysstat_wdg_resets() == 0);
  sysstat_leave_safe_mode();
  CHECK_FALSE(sysstat_safe_mode());
  CHECK(sysstat_resets() == 0);
  CHECK(sysstat_boot_reason() == BootReason::PowerOn);
}
