// systemState (legacy common/state).
#include "doctest.h"
#include "vdm/mqtt_values.h"

using namespace vdm;

TEST_CASE("systemState") {
  CHECK(systemState(LinkState::Up, nullptr, 0, 0) == 0);
  CHECK(systemState(LinkState::Unknown, nullptr, 0, 0) == 1);
  CHECK(systemState(LinkState::Degraded, nullptr, 0, 0) == 1);
  CHECK(systemState(LinkState::Booting, nullptr, 0, 0) == 1);
  CHECK(systemState(LinkState::Suspended, nullptr, 0, 0) == 1);
  CHECK(systemState(LinkState::Down, nullptr, 0, 0) == 2);

  ValveState v[kValveCount];
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF) == 0);
  v[4].health = kHealthBlocked;
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF) == 2);
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FEF) == 0);  // inactive
  CHECK(systemState(LinkState::Up, v, 4, 0x0FFF) == 0);            // beyond count
  CHECK(systemState(LinkState::Up, v, 5, 0x0FFF) == 2);
  v[4].health = kHealthFailed;
  CHECK(systemState(LinkState::Degraded, v, kValveCount, 0x0010) == 2);
  const uint16_t others[] = {kHealthNoValve,    kHealthCalibRetries,      kHealthEarlyStop,
                             kHealthCmdRejected, kHealthStale,            kHealthTargetUnconfirmed,
                             kHealthTempFailed,  kHealthFailsafe,         kHealthStrokeShort};
  for (uint16_t f : others) {
    v[4].health = f;
    CHECK(systemState(LinkState::Up, v, kValveCount, 0x0010) == 1);
    CHECK(systemState(LinkState::Up, v, kValveCount, 0x0000) == 0);
    CHECK(systemState(LinkState::Down, v, kValveCount, 0x0010) == 2);
  }
  // Blocked on a later valve wins over an info flag on an earlier one.
  v[0].health = kHealthStale;
  v[11].health = kHealthBlocked;
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF) == 2);
  // The first valve counts as well.
  v[11].health = 0;
  v[0].health = kHealthBlocked;
  CHECK(systemState(LinkState::Up, v, 1, 0x0001) == 2);
  // count above 12 is clamped (no read past the array).
  v[11].health = 0;
  v[0].health = 0;
  v[4].health = 0;
  CHECK(systemState(LinkState::Up, v, 255, 0xFFFF) == 0);
}

TEST_CASE("systemState: safe mode and failsafe") {
  ValveState v[kValveCount];
  SystemFlags safe;
  safe.safeMode = true;
  SystemFlags fs;
  fs.failsafe = true;
  SystemFlags both;
  both.safeMode = true;
  both.failsafe = true;
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF, SystemFlags{}) == 0);
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF, safe) == 2);
  CHECK(systemState(LinkState::Up, nullptr, 0, 0, safe) == 2);
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF, fs) == 1);
  CHECK(systemState(LinkState::Up, nullptr, 0, 0, fs) == 1);
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF, both) == 2);
  CHECK(systemState(LinkState::Degraded, v, kValveCount, 0x0FFF, safe) == 2);
  CHECK(systemState(LinkState::Down, v, kValveCount, 0x0FFF, fs) == 2);
  // An error of a valve outranks the failsafe.
  v[2].health = kHealthFailed;
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF, fs) == 2);
  v[2].health = kHealthFailsafe;
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF, fs) == 1);
  CHECK(systemState(LinkState::Up, v, kValveCount, 0x0FFF) == 1);
}
