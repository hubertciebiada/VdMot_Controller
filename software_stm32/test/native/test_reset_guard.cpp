#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "doctest.h"
#include "vdm/legacy_layout.h"
#include "vdm/reset_guard.h"

using vdm::BootReason;
using vdm::ResetGuardCell;

namespace {

ResetGuardCell garbage() {
  ResetGuardCell c;
  memset(&c, 0xA5, sizeof c);
  return c;
}

// one boot with the reason, then alive with the uptime; returns safe after the boot
bool boot(ResetGuardCell& c, BootReason r, uint32_t uptime) {
  const bool safe = vdm::resetGuardOnBoot(c, r);
  vdm::resetGuardAlive(c, uptime);
  return safe;
}

bool sealed(const ResetGuardCell& c) {
  return c.magic == vdm::kResetGuardMagic &&
         c.crc == vdm::crc16Ccitt(reinterpret_cast<const uint8_t*>(&c), offsetof(ResetGuardCell, crc));
}

}  // namespace

TEST_CASE("reset guard: constants") {
  CHECK(vdm::kSafeModeResets == 3);
  CHECK(vdm::kSafeModeWindowS == 600);
  CHECK(vdm::kSafeModeExitS == 1800);
  CHECK(vdm::kResetGuardMagic == 0x56445247u);
  CHECK(offsetof(ResetGuardCell, crc) == 16);
}

TEST_CASE("reset guard: power-on, brown-out and unknown clear the cell") {
  const BootReason cold[] = {BootReason::PowerOn, BootReason::BrownOut, BootReason::Unknown};
  for (BootReason r : cold) {
    ResetGuardCell c = garbage();
    CHECK_FALSE(vdm::resetGuardOnBoot(c, r));
    CHECK(c.count == 0);
    CHECK(c.safe == 0);
    CHECK(c.windowS == 0);
    CHECK(c.lastUptimeS == 0);
    CHECK(c.pad == 0);
    CHECK(sealed(c));
  }
  ResetGuardCell c = garbage();
  boot(c, BootReason::PowerOn, 50);
  boot(c, BootReason::IndependentWatchdog, 50);
  boot(c, BootReason::IndependentWatchdog, 50);
  CHECK(c.count == 2);
  CHECK_FALSE(boot(c, BootReason::PowerOn, 50));
  CHECK(c.count == 0);
}

TEST_CASE("reset guard: three watchdog boots 100 s apart enter safe mode on the third") {
  ResetGuardCell c = garbage();
  CHECK_FALSE(boot(c, BootReason::PowerOn, 100));
  CHECK_FALSE(boot(c, BootReason::IndependentWatchdog, 100));
  CHECK(c.count == 1);
  CHECK(c.windowS == 0);
  CHECK_FALSE(boot(c, BootReason::IndependentWatchdog, 100));
  CHECK(c.count == 2);
  CHECK(c.windowS == 100);
  CHECK(boot(c, BootReason::WindowWatchdog, 100));
  CHECK(c.count == 3);
  CHECK(c.windowS == 200);
  CHECK(c.safe == 1);
  // stays safe across a pin reset
  CHECK(boot(c, BootReason::Pin, 100));
  CHECK(sealed(c));
}

TEST_CASE("reset guard: watchdog boots 400 s apart leave the window and count again") {
  ResetGuardCell c = garbage();
  boot(c, BootReason::PowerOn, 400);
  CHECK_FALSE(boot(c, BootReason::IndependentWatchdog, 400));
  CHECK_FALSE(boot(c, BootReason::IndependentWatchdog, 400));
  CHECK(c.count == 2);
  CHECK(c.windowS == 400);
  CHECK_FALSE(boot(c, BootReason::IndependentWatchdog, 400));
  CHECK(c.count == 1);
  CHECK(c.windowS == 0);
}

TEST_CASE("reset guard: the window may reach exactly 600 s") {
  ResetGuardCell c = garbage();
  boot(c, BootReason::PowerOn, 0);
  boot(c, BootReason::IndependentWatchdog, 300);
  boot(c, BootReason::IndependentWatchdog, 300);
  CHECK(boot(c, BootReason::IndependentWatchdog, 0));
  CHECK(c.windowS == 600);
  ResetGuardCell d = garbage();
  boot(d, BootReason::PowerOn, 0);
  boot(d, BootReason::IndependentWatchdog, 300);
  boot(d, BootReason::IndependentWatchdog, 301);
  CHECK_FALSE(boot(d, BootReason::IndependentWatchdog, 0));
  CHECK(d.count == 1);
}

TEST_CASE("reset guard: a pin reset between watchdog resets keeps the count and adds its uptime") {
  ResetGuardCell c = garbage();
  boot(c, BootReason::PowerOn, 0);
  boot(c, BootReason::IndependentWatchdog, 100);
  CHECK_FALSE(boot(c, BootReason::Pin, 150));
  CHECK(c.count == 1);
  CHECK(c.windowS == 100);
  CHECK_FALSE(boot(c, BootReason::Software, 50));
  CHECK(c.windowS == 250);
  CHECK_FALSE(boot(c, BootReason::IndependentWatchdog, 10));
  CHECK(c.count == 2);
  CHECK(c.windowS == 300);
}

TEST_CASE("reset guard: an invalid cell on a warm boot is cleared") {
  ResetGuardCell c = garbage();
  boot(c, BootReason::PowerOn, 0);
  boot(c, BootReason::IndependentWatchdog, 10);
  boot(c, BootReason::IndependentWatchdog, 10);
  c.count ^= 0x40;  // CRC no longer matches
  CHECK_FALSE(vdm::resetGuardOnBoot(c, BootReason::IndependentWatchdog));
  CHECK(c.count == 0);
  CHECK(sealed(c));
  ResetGuardCell m = garbage();
  boot(m, BootReason::PowerOn, 0);
  m.magic = 0x56445248u;
  m.crc = vdm::crc16Ccitt(reinterpret_cast<const uint8_t*>(&m), offsetof(ResetGuardCell, crc));
  m.count = 2;
  m.crc = vdm::crc16Ccitt(reinterpret_cast<const uint8_t*>(&m), offsetof(ResetGuardCell, crc));
  CHECK_FALSE(vdm::resetGuardOnBoot(m, BootReason::IndependentWatchdog));
  CHECK(m.count == 0);
}

TEST_CASE("reset guard: the window sum saturates") {
  ResetGuardCell c = garbage();
  boot(c, BootReason::PowerOn, 0);
  boot(c, BootReason::Pin, UINT32_MAX);
  CHECK(c.windowS == 0);
  vdm::resetGuardOnBoot(c, BootReason::Pin);
  CHECK(c.windowS == UINT32_MAX);
  vdm::resetGuardAlive(c, 5);
  vdm::resetGuardOnBoot(c, BootReason::Pin);
  CHECK(c.windowS == UINT32_MAX);
}

TEST_CASE("resetGuardAlive: records the uptime and leaves safe mode after 1800 s") {
  ResetGuardCell c = garbage();
  boot(c, BootReason::PowerOn, 0);
  boot(c, BootReason::IndependentWatchdog, 1);
  boot(c, BootReason::IndependentWatchdog, 1);
  CHECK(vdm::resetGuardOnBoot(c, BootReason::IndependentWatchdog));
  CHECK(vdm::resetGuardAlive(c, 1799));
  CHECK(c.lastUptimeS == 1799);
  CHECK(sealed(c));
  CHECK_FALSE(vdm::resetGuardAlive(c, 1800));
  CHECK(c.count == 0);
  CHECK(c.windowS == 0);
  CHECK(c.safe == 0);
  CHECK(c.lastUptimeS == 1800);
  // not safe: the uptime does not clear the count
  ResetGuardCell d = garbage();
  boot(d, BootReason::PowerOn, 0);
  vdm::resetGuardOnBoot(d, BootReason::IndependentWatchdog);
  CHECK_FALSE(vdm::resetGuardAlive(d, 5000));
  CHECK(d.count == 1);
}

TEST_CASE("resetGuardClear: ssafe 0 leaves safe mode and clears the window") {
  ResetGuardCell c = garbage();
  boot(c, BootReason::PowerOn, 0);
  boot(c, BootReason::IndependentWatchdog, 1);
  boot(c, BootReason::IndependentWatchdog, 1);
  CHECK(vdm::resetGuardOnBoot(c, BootReason::IndependentWatchdog));
  vdm::resetGuardAlive(c, 20);
  vdm::resetGuardClear(c);
  CHECK(c.safe == 0);
  CHECK(c.count == 0);
  CHECK(c.windowS == 0);
  CHECK(c.lastUptimeS == 20);
  CHECK(sealed(c));
  CHECK_FALSE(vdm::resetGuardOnBoot(c, BootReason::IndependentWatchdog));
  CHECK(c.count == 1);
}
