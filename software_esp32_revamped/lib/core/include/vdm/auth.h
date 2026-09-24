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

// Brute-force limiter: after maxFailures failed attempts within windowMs,
// every attempt is refused for lockoutMs (HTTP 429), regardless of the
// credentials. One global counter (the device has one account).
// maxFailures 0 disables the limiter. A success clears the failure count.
// Expired windows and lockouts are cleared lazily by locked()/onResult().
class AuthLimiter {
 public:
  explicit AuthLimiter(uint8_t maxFailures = 10, uint32_t windowMs = 60000,
                       uint32_t lockoutMs = 60000);
  bool locked(uint32_t nowMs) const;
  // Record the result of a checked attempt (not called while locked).
  void onResult(bool success, uint32_t nowMs);
  uint32_t failuresInWindow() const { return failures_; }

 private:
  uint8_t maxFailures_;
  uint32_t windowMs_;
  uint32_t lockoutMs_;
  void expire(uint32_t nowMs) const;

  mutable uint32_t failures_ = 0;
  uint32_t windowStartMs_ = 0;
  mutable bool lockedOut_ = false;
  uint32_t lockStartMs_ = 0;
};

}  // namespace vdm
