#include "vdm/auth.h"

#include <string.h>

#include "vdm/common.h"

namespace vdm {

namespace {

constexpr size_t kMaxDecoded = 130;

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
// or when the result does not fit `cap` (checked before any group is decoded,
// so an overlong header costs nothing). len > 0.
bool base64Decode(const char* in, size_t len, uint8_t* out, size_t cap, size_t& outLen) {
  outLen = 0;
  if (len % 4 != 0) return false;
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
  if (len <= kSchemeLen) return false;
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

constexpr uint32_t AuthLimiter::kLockMs[3];

AuthLimiter::Entry* AuthLimiter::find(uint32_t ip) {
  for (Entry& e : slots_) {
    if (e.used && e.ip == ip) return &e;
  }
  return nullptr;
}

const AuthLimiter::Entry* AuthLimiter::find(uint32_t ip) const {
  for (const Entry& e : slots_) {
    if (e.used && e.ip == ip) return &e;
  }
  return nullptr;
}

void AuthLimiter::expire(Entry& e, uint32_t nowMs) {
  if (e.locked && elapsedMs(nowMs, e.lockStartMs) >= e.lockMs) {
    e.locked = false;
    e.failures = 0;
  }
}

// A free entry, else the least recently used unlocked one, else the locked
// one whose lock ends first.
AuthLimiter::Entry& AuthLimiter::claim(uint32_t ip, uint32_t nowMs) {
  Entry* pick = nullptr;
  for (Entry& e : slots_) {
    if (!e.used) {
      pick = &e;
      break;
    }
    expire(e, nowMs);
    if (e.locked) continue;
    if (pick == nullptr || e.lastUse < pick->lastUse) pick = &e;
  }
  if (pick == nullptr) {
    uint32_t best = UINT32_MAX;
    for (Entry& e : slots_) {
      const uint32_t left = e.lockMs - elapsedMs(nowMs, e.lockStartMs);
      if (left < best) {
        best = left;
        pick = &e;
      }
    }
  }
  *pick = Entry{};
  pick->used = true;
  pick->ip = ip;
  return *pick;
}

bool AuthLimiter::locked(uint32_t ip, uint32_t nowMs, uint32_t* retryAfterS) {
  Entry* e = find(ip);
  if (e == nullptr) return false;
  expire(*e, nowMs);
  if (!e->locked) return false;
  if (retryAfterS != nullptr) {
    const uint32_t left = e->lockMs - elapsedMs(nowMs, e->lockStartMs);
    *retryAfterS = (left + 999) / 1000;
  }
  return true;
}

bool AuthLimiter::onResult(uint32_t ip, bool success, uint32_t nowMs) {
  Entry* e = find(ip);
  if (success) {
    if (e != nullptr) *e = Entry{};
    return false;
  }
  if (e == nullptr) e = &claim(ip, nowMs);
  expire(*e, nowMs);
  e->lastUse = ++useCounter_;
  if (e->locked) return false;
  if (e->failures == 0 || elapsedMs(nowMs, e->windowStartMs) >= kWindowMs) {
    e->failures = 0;
    e->windowStartMs = nowMs;
  }
  if (++e->failures < kMaxFailures) return false;
  e->locked = true;
  e->lockStartMs = nowMs;
  e->lockMs = kLockMs[e->level < 2 ? e->level : 2];
  if (e->level < 255) ++e->level;
  return true;
}

uint32_t AuthLimiter::failuresInWindow(uint32_t ip) const {
  const Entry* e = find(ip);
  return e != nullptr ? e->failures : 0;
}

uint8_t AuthLimiter::lockLevel(uint32_t ip) const {
  const Entry* e = find(ip);
  return e != nullptr ? e->level : 0;
}

}  // namespace vdm
