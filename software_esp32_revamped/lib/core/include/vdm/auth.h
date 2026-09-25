// HTTP authentication decisions: Basic-auth header check with constant-time
// comparison and a brute-force limiter. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace vdm {

// Compares two strings in time that depends only on the length of `b`
// (the secret); lengths must also match. Null pointers compare unequal.
bool constantTimeEquals(const char* a, const char* b);

// Checks an "Authorization" header value "Basic <base64(user:password)>"
// (scheme case-insensitive, one space). Decoded length <= 130. Returns true
// only for an exact user and password match.
bool checkBasicAuth(const char* header, size_t len, const char* user, const char* password);

// Brute-force limiter per client address (IPv4, legacy uint32 layout): 10
// failed attempts of one address within 60 s of its first failure lock that
// address for 1 min, its second lockout 5 min, the third and every later one
// 15 min; other addresses are not affected. A success forgets the address
// (failures and level). Up to kSlots addresses are tracked: a new address
// replaces the least recently used unlocked entry, or, when all are locked,
// the entry whose lock ends first. Recency is a use counter, durations use
// elapsedMs(), so the 49-day millis() wrap changes nothing.
class AuthLimiter {
 public:
  static constexpr size_t kSlots = 8;
  static constexpr uint8_t kMaxFailures = 10;
  static constexpr uint32_t kWindowMs = 60000;
  static constexpr uint32_t kLockMs[3] = {60000, 300000, 900000};
  // True while `ip` is locked; retryAfterS = whole seconds to the end (>= 1).
  bool locked(uint32_t ip, uint32_t nowMs, uint32_t* retryAfterS = nullptr);
  // Result of a checked attempt (not called while locked). Returns true when
  // this failure started a lockout.
  bool onResult(uint32_t ip, bool success, uint32_t nowMs);
  uint32_t failuresInWindow(uint32_t ip) const;  // 0 for unknown addresses
  uint8_t lockLevel(uint32_t ip) const;          // lockouts of the address so far

 private:
  struct Entry {
    bool used = false;
    uint32_t ip = 0;
    uint8_t failures = 0;
    uint32_t windowStartMs = 0;
    bool locked = false;
    uint32_t lockStartMs = 0;
    uint32_t lockMs = 0;
    uint8_t level = 0;
    uint32_t lastUse = 0;
  };
  Entry* find(uint32_t ip);
  const Entry* find(uint32_t ip) const;
  Entry& claim(uint32_t ip, uint32_t nowMs);
  static void expire(Entry& e, uint32_t nowMs);

  Entry slots_[kSlots];
  uint32_t useCounter_ = 0;
};

}  // namespace vdm
