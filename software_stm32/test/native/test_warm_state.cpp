#include <stdint.h>
#include <string.h>

#include "doctest.h"
#include "vdm/warm_state.h"

using vdm::BootReason;
using vdm::WarmState;
using vdm::WarmValve;

namespace {

WarmState sample() {
  WarmState s;
  memset(&s, 0, sizeof s);
  for (uint8_t v = 0; v < vdm::kValveCount; v++) {
    s.valves[v].actual = static_cast<uint8_t>(v * 8);
    s.valves[v].target = static_cast<uint8_t>(100 - v);
    s.valves[v].status = 1;
    s.valves[v].flags = vdm::kWarmPosValid;
    s.failsafePct[v] = 50;
  }
  s.leaseSinceRenewalS = 1234;
  s.leaseSinceClientS = 56;
  s.leaseClient = 1;
  s.leaseTimeoutMin = 60;
  vdm::warmStateSeal(s);
  return s;
}

WarmValve valve(uint8_t actual, uint8_t target, uint8_t status, uint8_t flags) {
  WarmValve w;
  memset(&w, 0, sizeof w);
  w.actual = actual;
  w.target = target;
  w.status = status;
  w.flags = flags;
  return w;
}

}  // namespace

TEST_CASE("warm state: constants of the layout") {
  CHECK(vdm::kWarmStateMagic == 0x56445753u);
  CHECK(vdm::kWarmStateVersion == 1);
  CHECK(vdm::kWarmPosValid == 1);
  CHECK(vdm::kWarmAssemblyHold == 2);
  CHECK(vdm::kWarmNeedsReference == 4);
  CHECK(vdm::kWarmRecal == 8);
  CHECK(sizeof(WarmState) == 180);
}

TEST_CASE("warmStateSeal: magic, version, count, reserved and a CRC that warmStateValid accepts") {
  WarmState s = sample();
  CHECK(s.magic == vdm::kWarmStateMagic);
  CHECK(s.version == 1);
  CHECK(s.count == 12);
  CHECK(s.reserved == 0);
  CHECK(s.crc == vdm::crc16Ccitt(reinterpret_cast<const uint8_t*>(&s), 176));
  CHECK(vdm::warmStateValid(s));
  s.reserved = 7;
  vdm::warmStateSeal(s);
  CHECK(s.reserved == 0);
  CHECK(vdm::warmStateValid(s));
}

TEST_CASE("warmStateValid: any changed byte before the CRC, and the CRC itself, invalidate the record") {
  const WarmState good = sample();
  for (size_t i = 0; i < 178; i++) {
    CAPTURE(i);
    WarmState s = good;
    reinterpret_cast<uint8_t*>(&s)[i] ^= 0x01;
    CHECK_FALSE(vdm::warmStateValid(s));
  }
}

TEST_CASE("warmStateValid: wrong magic, version or count with a matching CRC are invalid") {
  WarmState s = sample();
  s.magic = 0x56445752u;
  s.crc = vdm::crc16Ccitt(reinterpret_cast<const uint8_t*>(&s), 176);
  CHECK_FALSE(vdm::warmStateValid(s));
  s = sample();
  s.version = 2;
  s.crc = vdm::crc16Ccitt(reinterpret_cast<const uint8_t*>(&s), 176);
  CHECK_FALSE(vdm::warmStateValid(s));
  s = sample();
  s.count = 11;
  s.crc = vdm::crc16Ccitt(reinterpret_cast<const uint8_t*>(&s), 176);
  CHECK_FALSE(vdm::warmStateValid(s));
  WarmState erased;
  memset(&erased, 0xA5, sizeof erased);
  CHECK_FALSE(vdm::warmStateValid(erased));
}

TEST_CASE("isWarmBoot: pin, software, watchdogs and low power are warm") {
  CHECK_FALSE(vdm::isWarmBoot(BootReason::Unknown));
  CHECK_FALSE(vdm::isWarmBoot(BootReason::PowerOn));
  CHECK(vdm::isWarmBoot(BootReason::Pin));
  CHECK(vdm::isWarmBoot(BootReason::Software));
  CHECK(vdm::isWarmBoot(BootReason::IndependentWatchdog));
  CHECK(vdm::isWarmBoot(BootReason::WindowWatchdog));
  CHECK(vdm::isWarmBoot(BootReason::LowPower));
  CHECK_FALSE(vdm::isWarmBoot(BootReason::BrownOut));
}

TEST_CASE("restoreValve: statuses 1, 4..9 are kept with a valid position") {
  const uint8_t kept[] = {1, 4, 5, 6, 7, 8, 9};
  for (uint8_t st : kept) {
    CAPTURE(+st);
    const vdm::RestoredValve r = vdm::restoreValve(valve(30, 70, st, vdm::kWarmPosValid), true);
    CHECK(r.valid);
    CHECK(+r.status == +st);
    CHECK(+r.actual == 30);
    CHECK(+r.target == 70);
    CHECK_FALSE(r.needsReference);
    CHECK_FALSE(r.recal);
    CHECK_FALSE(r.assemblyHold);
  }
}

TEST_CASE("restoreValve: a moving valve or an invalid position is referenced again or tested") {
  const uint8_t moving[] = {2, 3};
  for (uint8_t st : moving) {
    CAPTURE(+st);
    vdm::RestoredValve r = vdm::restoreValve(valve(30, 70, st, vdm::kWarmPosValid), true);
    CHECK(r.valid);
    CHECK(+r.status == 1);
    CHECK(r.needsReference);
    r = vdm::restoreValve(valve(30, 70, st, vdm::kWarmPosValid), false);
    CHECK(+r.status == 5);
    CHECK_FALSE(r.needsReference);
  }
  vdm::RestoredValve r = vdm::restoreValve(valve(30, 70, 1, 0), true);
  CHECK(+r.status == 1);
  CHECK(r.needsReference);
  CHECK(+r.actual == 30);
  r = vdm::restoreValve(valve(30, 70, 9, 0), false);
  CHECK(+r.status == 5);
  CHECK_FALSE(r.needsReference);
  r = vdm::restoreValve(valve(30, 70, 9, vdm::kWarmNeedsReference), false);
  CHECK(+r.status == 5);
  CHECK(r.needsReference);
}

TEST_CASE("restoreValve: flags") {
  const vdm::RestoredValve r = vdm::restoreValve(
      valve(0, 100, 1, vdm::kWarmPosValid | vdm::kWarmAssemblyHold | vdm::kWarmNeedsReference | vdm::kWarmRecal), true);
  CHECK(r.valid);
  CHECK(r.needsReference);
  CHECK(r.recal);
  CHECK(r.assemblyHold);
  CHECK(+r.actual == 0);
  CHECK(+r.target == 100);
  const vdm::RestoredValve a = vdm::restoreValve(valve(0, 100, 1, vdm::kWarmPosValid | vdm::kWarmAssemblyHold), true);
  CHECK(a.assemblyHold);
  CHECK_FALSE(a.recal);
  CHECK_FALSE(a.needsReference);
  const vdm::RestoredValve b = vdm::restoreValve(valve(0, 100, 8, vdm::kWarmPosValid | vdm::kWarmRecal), false);
  CHECK(b.recal);
  CHECK_FALSE(b.assemblyHold);
}

TEST_CASE("restoreValve: every field out of range sends the valve down the cold path") {
  CHECK_FALSE(vdm::restoreValve(valve(101, 50, 1, 1), true).valid);
  CHECK_FALSE(vdm::restoreValve(valve(50, 101, 1, 1), true).valid);
  CHECK_FALSE(vdm::restoreValve(valve(50, 50, 0, 1), true).valid);
  CHECK_FALSE(vdm::restoreValve(valve(50, 50, 10, 1), true).valid);
  CHECK_FALSE(vdm::restoreValve(valve(50, 50, 1, 0x10), true).valid);
  CHECK_FALSE(vdm::restoreValve(valve(50, 50, 1, 0x80), true).valid);
  CHECK(vdm::restoreValve(valve(100, 100, 9, 0x0F), true).valid);
  WarmValve w = valve(50, 50, 1, 1);
  w.retryScheduled = 2;
  CHECK_FALSE(vdm::restoreValve(w, true).valid);
  w.retryScheduled = 1;
  w.retryRemainingS = 86401;
  CHECK_FALSE(vdm::restoreValve(w, true).valid);
  w.retryRemainingS = 86400;
  CHECK(vdm::restoreValve(w, true).valid);
  const vdm::RestoredValve bad = vdm::restoreValve(valve(101, 50, 1, 1), true);
  CHECK_FALSE(bad.needsReference);
  CHECK(+bad.status == 0);
}
