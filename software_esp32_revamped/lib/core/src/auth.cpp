#include "vdm/auth.h"

#include <string.h>

#include "vdm/common.h"

namespace vdm {

namespace {

constexpr size_t kMaxDecoded = 130;
constexpr size_t kMaxEncoded = (kMaxDecoded + 2) / 3 * 4;  // 176

// 0..63 for a base64 alphabet character, -1 otherwise.
int base64Value(char c) {
  if (c >= 'A' && c <= 'Z') return c - 'A';
  if (c >= 'a' && c <= 'z') return c - 'a' + 26;
  if (c >= '0' && c <= '9') return c - '0' + 52;
  if (c == '+') return 62;
  if (c == '/') return 63;
  return -1;
}

// Strict RFC 4648 decode: length a multiple of 4, '=' only as the last one or
// two characters, unused bits of the last group zero. False on any violation
// or when the result does not fit `cap`.
bool base64Decode(const char* in, size_t len, uint8_t* out, size_t cap, size_t& outLen) {
  outLen = 0;
  if (len == 0 || len % 4 != 0) return false;
  size_t pad = 0;
  if (in[len - 1] == '=') pad = (in[len - 2] == '=') ? 2 : 1;
  if (len / 4 * 3 - pad > cap) return false;
  for (size_t i = 0; i < len; i += 4) {
    const bool last = (i + 4 == len);
    uint32_t group = 0;
    for (size_t k = 0; k < 4; ++k) {
      const int v = (last && k >= 4 - pad) ? 0 : base64Value(in[i + k]);
      if (v < 0) return false;
      group = (group << 6) | static_cast<uint32_t>(v);
    }
    if (last && pad == 2 && (group & 0xFFFFu) != 0) return false;
    if (last && pad == 1 && (group & 0xFFu) != 0) return false;
    const size_t bytes = last ? 3 - pad : 3;
    for (size_t k = 0; k < bytes; ++k) out[outLen++] = static_cast<uint8_t>(group >> (16 - 8 * k));
  }
  return true;
}

// Compares `la` bytes of `a` with the NUL-terminated secret `b` in time that
// depends only on strlen(b).
bool secretEquals(const uint8_t* a, size_t la, const char* b) {
  const size_t lb = strlen(b);
  uint8_t diff = (la != lb) ? 1 : 0;
  for (size_t i = 0; i < lb; ++i) {
    const uint8_t ca = (i < la) ? a[i] : 0;
    diff |= static_cast<uint8_t>(ca ^ static_cast<uint8_t>(b[i]));
  }
  return diff == 0;
}

}  // namespace

bool constantTimeEquals(const char* a, const char* b) {
  if (a == nullptr || b == nullptr) return false;
  return secretEquals(reinterpret_cast<const uint8_t*>(a), strlen(a), b);
}

bool checkBasicAuth(const char* header, size_t len, const char* user, const char* password) {
  if (header == nullptr || user == nullptr || password == nullptr || user[0] == '\0') return false;
  static const char kScheme[] = "basic ";
  constexpr size_t kSchemeLen = sizeof kScheme - 1;
  if (len <= kSchemeLen || len - kSchemeLen > kMaxEncoded) return false;
  for (size_t i = 0; i < kSchemeLen; ++i) {
    char c = header[i];
    if (c >= 'A' && c <= 'Z') c = static_cast<char>(c - 'A' + 'a');
    if (c != kScheme[i]) return false;
  }
  uint8_t decoded[kMaxDecoded];
  size_t n = 0;
  if (!base64Decode(header + kSchemeLen, len - kSchemeLen, decoded, sizeof decoded, n)) return false;
  size_t colon = n;
  for (size_t i = 0; i < n; ++i) {
    if (decoded[i] == '\0') return false;
    if (colon == n && decoded[i] == ':') colon = i;
  }
  if (colon == n) return false;
  // Both comparisons always run, so the timing does not tell which one failed.
  const bool userOk = secretEquals(decoded, colon, user);
  const bool pwdOk = secretEquals(decoded + colon + 1, n - colon - 1, password);
  return userOk & pwdOk;
}

AuthLimiter::AuthLimiter(uint8_t maxFailures, uint32_t windowMs, uint32_t lockoutMs)
    : maxFailures_(maxFailures), windowMs_(windowMs), lockoutMs_(lockoutMs) {}

void AuthLimiter::expire(uint32_t nowMs) const {
  if (lockedOut_ && elapsedMs(nowMs, lockStartMs_) >= lockoutMs_) {
    lockedOut_ = false;
    failures_ = 0;
  }
  if (!lockedOut_ && failures_ > 0 && elapsedMs(nowMs, windowStartMs_) >= windowMs_) failures_ = 0;
}

bool AuthLimiter::locked(uint32_t nowMs) const {
  expire(nowMs);
  return lockedOut_;
}

void AuthLimiter::onResult(bool success, uint32_t nowMs) {
  expire(nowMs);
  if (lockedOut_) return;
  if (success) {
    failures_ = 0;
    return;
  }
  if (maxFailures_ == 0) return;  // limiter disabled
  if (failures_ == 0) windowStartMs_ = nowMs;
  ++failures_;
  if (failures_ >= maxFailures_) {
    lockedOut_ = true;
    lockStartMs_ = nowMs;
  }
}

}  // namespace vdm
