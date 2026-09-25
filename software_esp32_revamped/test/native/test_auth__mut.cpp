// AuthLimiter: eviction ties between locked entries and the saturating lockout level.
#include <stdint.h>

#include "doctest.h"
#include "vdm/auth.h"

using vdm::AuthLimiter;

namespace {

// kMaxFailures failures of ip at t: the last one starts a lockout
void lockOut(AuthLimiter& l, uint32_t ip, uint32_t t) {
  for (uint8_t i = 1; i < AuthLimiter::kMaxFailures; i++) CHECK_FALSE(l.onResult(ip, false, t));
  CHECK(l.onResult(ip, false, t));
}

}  // namespace

TEST_CASE("AuthLimiter: all entries locked with the same end, a new address replaces the first") {
  AuthLimiter l;
  for (uint32_t ip = 1; ip <= AuthLimiter::kSlots; ip++) lockOut(l, ip, 1000);
  for (uint32_t ip = 1; ip <= AuthLimiter::kSlots; ip++) CHECK(l.locked(ip, 2000));
  CHECK_FALSE(l.onResult(100, false, 2000));
  CHECK(l.failuresInWindow(100) == 1);
  CHECK_FALSE(l.locked(1, 2000));
  CHECK(l.failuresInWindow(1) == 0);
  for (uint32_t ip = 2; ip <= AuthLimiter::kSlots; ip++) CHECK(l.locked(ip, 2000));
}

TEST_CASE("AuthLimiter: the lockout level saturates at 255 and stays at the longest lock") {
  AuthLimiter l;
  uint32_t t = 0;
  for (int n = 1; n <= 256; n++) {
    lockOut(l, 7, t);
    t += AuthLimiter::kLockMs[2];  // every lock has ended
  }
  CHECK(l.lockLevel(7) == 255);
  lockOut(l, 7, t);
  CHECK(l.lockLevel(7) == 255);
  CHECK(l.locked(7, t + AuthLimiter::kLockMs[2] - 1));
  CHECK_FALSE(l.locked(7, t + AuthLimiter::kLockMs[2]));
}
