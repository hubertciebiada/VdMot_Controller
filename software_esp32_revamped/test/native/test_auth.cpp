// Tests for vdm/auth.h: constant-time compare, Basic-auth header check and
// the per-address brute-force limiter.
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "doctest.h"
#include "vdm/auth.h"

using namespace vdm;

namespace {

bool check(const char* header, const char* user, const char* pwd) {
  return checkBasicAuth(header, strlen(header), user, pwd);
}

// Minimal encoder for building test headers.
void b64(const char* in, size_t n, char* out) {
  static const char a[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  size_t o = 0;
  for (size_t i = 0; i < n; i += 3) {
    uint32_t v = static_cast<uint32_t>(static_cast<uint8_t>(in[i])) << 16;
    if (i + 1 < n) v |= static_cast<uint32_t>(static_cast<uint8_t>(in[i + 1])) << 8;
    if (i + 2 < n) v |= static_cast<uint8_t>(in[i + 2]);
    out[o++] = a[(v >> 18) & 63];
    out[o++] = a[(v >> 12) & 63];
    out[o++] = (i + 1 < n) ? a[(v >> 6) & 63] : '=';
    out[o++] = (i + 2 < n) ? a[v & 63] : '=';
  }
  out[o] = '\0';
}

}  // namespace

TEST_CASE("constantTimeEquals") {
  CHECK(constantTimeEquals("", ""));
  CHECK(constantTimeEquals("secret", "secret"));
  CHECK_FALSE(constantTimeEquals("secret", "secreT"));
  CHECK_FALSE(constantTimeEquals("secre", "secret"));
  CHECK_FALSE(constantTimeEquals("secrets", "secret"));
  CHECK_FALSE(constantTimeEquals("", "x"));
  CHECK_FALSE(constantTimeEquals("x", ""));
  CHECK_FALSE(constantTimeEquals(nullptr, "x"));
  CHECK_FALSE(constantTimeEquals("x", nullptr));
  CHECK_FALSE(constantTimeEquals(nullptr, nullptr));
  // A prefix followed by the secret's terminator must not match.
  CHECK_FALSE(constantTimeEquals("ab\0c", "abc"));
}

TEST_CASE("checkBasicAuth accepts exact credentials") {
  // "admin:pw" -> YWRtaW46cHc=
  CHECK(check("Basic YWRtaW46cHc=", "admin", "pw"));
  CHECK(check("basic YWRtaW46cHc=", "admin", "pw"));
  CHECK(check("BASIC YWRtaW46cHc=", "admin", "pw"));
  CHECK(check("bAsIc YWRtaW46cHc=", "admin", "pw"));
  CHECK_FALSE(check("Basic YWRtaW46cHc=", "admin", "pW"));
  CHECK_FALSE(check("Basic YWRtaW46cHc=", "Admin", "pw"));
  CHECK_FALSE(check("Basic YWRtaW46cHc=", "admin", "pw2"));
  CHECK_FALSE(check("Basic YWRtaW46cHc=", "admi", "pw"));
  // Password may contain ':' (split at the first one).
  char enc[256];
  char hdr[300];
  b64("u:a:b", 5, enc);
  snprintf(hdr, sizeof hdr, "Basic %s", enc);
  CHECK(check(hdr, "u", "a:b"));
  CHECK_FALSE(check(hdr, "u:a", "b"));
  // Empty password is an exact match only against an empty password.
  b64("u:", 2, enc);
  snprintf(hdr, sizeof hdr, "Basic %s", enc);
  CHECK(check(hdr, "u", ""));
  CHECK_FALSE(check(hdr, "u", "x"));
  // Empty configured user never matches.
  b64(":", 1, enc);
  snprintf(hdr, sizeof hdr, "Basic %s", enc);
  CHECK_FALSE(check(hdr, "", ""));
}

TEST_CASE("checkBasicAuth honours the length argument") {
  const char* h = "Basic YWRtaW46cHc=garbage";
  CHECK(checkBasicAuth(h, 18, "admin", "pw"));
  CHECK_FALSE(checkBasicAuth(h, 17, "admin", "pw"));
  CHECK_FALSE(checkBasicAuth(h, strlen(h), "admin", "pw"));
  CHECK_FALSE(checkBasicAuth(h, 0, "admin", "pw"));
  CHECK_FALSE(checkBasicAuth(h, 6, "admin", "pw"));
}

TEST_CASE("checkBasicAuth rejects malformed headers") {
  CHECK_FALSE(checkBasicAuth(nullptr, 5, "a", "b"));
  CHECK_FALSE(check("Basic YWRtaW46cHc=", nullptr, "pw"));
  CHECK_FALSE(check("Basic YWRtaW46cHc=", "admin", nullptr));
  CHECK_FALSE(check("", "admin", "pw"));
  CHECK_FALSE(check("Basic", "admin", "pw"));
  CHECK_FALSE(check("Basic ", "admin", "pw"));
  CHECK_FALSE(check("Basic  YWRtaW46cHc=", "admin", "pw"));  // two spaces
  CHECK_FALSE(check("Basic\tYWRtaW46cHc=", "admin", "pw"));
  CHECK_FALSE(check("Bearer YWRtaW46cHc=", "admin", "pw"));
  CHECK_FALSE(check("Basix YWRtaW46cHc=", "admin", "pw"));
  CHECK_FALSE(check("Basic YWRtaW46cHc", "admin", "pw"));      // missing padding
  CHECK_FALSE(check("Basic YWRtaW46cHc=\r", "admin", "pw"));   // trailing byte
  CHECK_FALSE(check("Basic YWRtaW46cHc= ", "admin", "pw"));
  CHECK_FALSE(check("Basic YWRta W46cHc=", "admin", "pw"));
  CHECK_FALSE(check("Basic YWRtaW46c=c=", "admin", "pw"));     // '=' inside
  CHECK_FALSE(check("Basic ====", "admin", "pw"));
  CHECK_FALSE(check("Basic A===", "admin", "pw"));
  CHECK_FALSE(check("Basic YWRtaW46cHd=", "admin", "pw"));     // non-zero pad bits
  CHECK_FALSE(check("Basic YQ==", "a", ""));                   // "a": no colon
  CHECK_FALSE(check("Basic YR==", "a", ""));                   // non-zero pad bits
  CHECK_FALSE(check("Basic YWRtaW4-cHc=", "admin", "pw"));     // url-safe alphabet
  // Embedded NUL in the decoded credentials: "a\0:b"
  CHECK_FALSE(check("Basic YQA6Yg==", "a", "b"));
}

TEST_CASE("checkBasicAuth decoded length limit is 130") {
  char cred[160];
  char enc[256];
  char hdr[300];
  // 130 decoded bytes: user "u", password of 128 chars.
  char pwd[140];
  memset(pwd, 'p', 128);
  pwd[128] = '\0';
  snprintf(cred, sizeof cred, "u:%s", pwd);
  REQUIRE(strlen(cred) == 130);
  b64(cred, 130, enc);
  snprintf(hdr, sizeof hdr, "Basic %s", enc);
  CHECK(check(hdr, "u", pwd));
  // 131 bytes are refused even when they would match.
  pwd[128] = 'p';
  pwd[129] = '\0';
  snprintf(cred, sizeof cred, "u:%s", pwd);
  b64(cred, 131, enc);
  snprintf(hdr, sizeof hdr, "Basic %s", enc);
  CHECK_FALSE(check(hdr, "u", pwd));
  // 132 bytes: encoded length 176 passes the text limit, decoded size does not.
  pwd[129] = 'p';
  pwd[130] = '\0';
  snprintf(cred, sizeof cred, "u:%s", pwd);
  b64(cred, 132, enc);
  REQUIRE(strlen(enc) == 176);
  snprintf(hdr, sizeof hdr, "Basic %s", enc);
  CHECK_FALSE(check(hdr, "u", pwd));
}

TEST_CASE("checkBasicAuth survives random input" * doctest::test_suite("fuzz")) {
  uint32_t seed = 0x5eed1234u;
  char buf[200];
  for (int round = 0; round < 20000; ++round) {
    seed = seed * 1103515245u + 12345u;
    const size_t n = 6 + (seed >> 16) % 190;
    memcpy(buf, "Basic ", 6);
    for (size_t i = 6; i < n; ++i) {
      seed = seed * 1103515245u + 12345u;
      buf[i] = static_cast<char>(seed >> 24);
    }
    CHECK_FALSE(checkBasicAuth(buf, n, "admin", "a-password-nobody-guesses"));
  }
}

// ---- mutation-driven cases -------------------------------------------------

TEST_CASE("checkBasicAuth decodes the whole base64 alphabet exactly") {
  // '+' (62) and '/' (63) round-trip.
  CHECK(check("Basic dTo+Pj4/Pz8=", "u", ">>>???"));
  CHECK_FALSE(check("Basic dTo+Pj4/Pz8=", "u", ">>>?" "?>"));
  // Deterministic random credentials: every alphabet character appears; each
  // must decode to its exact value (a wrong value changes the password).
  static const char kAlpha[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
  bool seen[64] = {};
  uint32_t seed = 0xC0FFEEu;
  char pwd[80];
  char cred[90];
  char enc[140];
  char hdr[160];
  for (int round = 0; round < 400; ++round) {
    seed = seed * 1103515245u + 12345u;
    const size_t len = 1 + (seed >> 16) % 70;
    for (size_t i = 0; i < len; ++i) {
      seed = seed * 1103515245u + 12345u;
      char c = static_cast<char>(1 + (seed >> 16) % 255);
      if (c == ':') c = ';';
      pwd[i] = c;
    }
    pwd[len] = '\0';
    snprintf(cred, sizeof cred, "u:%s", pwd);
    b64(cred, strlen(cred), enc);
    for (const char* p = enc; *p; ++p) {
      const char* hit = strchr(kAlpha, *p);
      if (hit != nullptr && *p != '\0') seen[hit - kAlpha] = true;
    }
    snprintf(hdr, sizeof hdr, "Basic %s", enc);
    CHECK(check(hdr, "u", pwd));
    // Replacing any non-padding character by another alphabet character
    // changes the decoded bytes or sets padding bits: never accepted.
    const size_t el = strlen(enc);
    const size_t pos = (seed >> 8) % el;
    if (hdr[6 + pos] != '=') {
      const char* at = strchr(kAlpha, hdr[6 + pos]);
      hdr[6 + pos] = kAlpha[(at - kAlpha + 1 + (seed >> 20) % 63) % 64];
      CHECK_FALSE(check(hdr, "u", pwd));
    }
  }
  for (int i = 0; i < 64; ++i) CHECK_MESSAGE(seen[i], "alphabet char not covered: ", kAlpha[i]);
}

TEST_CASE("checkBasicAuth rejects invalid characters instead of decoding them as zero") {
  // An invalid character must fail the decode, not act as value 0 ('A').
  char enc[32];
  char hdr[48];
  b64("AAA:pw", 6, enc);  // QUFBOnB3
  snprintf(hdr, sizeof hdr, "Basic %s", enc);
  CHECK(check(hdr, "AAA", "pw"));
  // Replace each character in turn by an invalid one: never accepted.
  for (size_t i = 6; i < strlen(hdr); ++i) {
    const char saved = hdr[i];
    static const char kBad[] = {'*', '.', '-', '_', ' ', '\x80', '@', '[', '`', '{'};
    for (const char bad : kBad) {
      hdr[i] = bad;
      CHECK_FALSE(check(hdr, "AAA", "pw"));
    }
    hdr[i] = saved;
  }
  // An 'A' (value 0) replaced by an invalid char must not decode as 'A'.
  b64("u:p\x01" "ab", 6, enc);  // dTpwAWFi
  snprintf(hdr, sizeof hdr, "Basic %s", enc);
  REQUIRE(strchr(hdr + 6, 'A') != nullptr);
  CHECK(check(hdr, "u", "p\x01" "ab"));
  *strchr(hdr + 6, 'A') = '!';
  CHECK_FALSE(check(hdr, "u", "p\x01" "ab"));
}

TEST_CASE("checkBasicAuth uses only `len` bytes: a non multiple of 4 is refused") {
  // "admin:pwX" -> YWRtaW46cHdY (no padding). Only 11 of its 12 characters
  // are inside `len`: must not decode the 12th.
  const char* h = "Basic YWRtaW46cHdY";
  CHECK(checkBasicAuth(h, 18, "admin", "pwX"));
  CHECK_FALSE(checkBasicAuth(h, 17, "admin", "pwX"));
  CHECK_FALSE(checkBasicAuth(h, 17, "admin", "pw"));
}

TEST_CASE("checkBasicAuth: an invalid group after valid ones rejects all of it") {
  CHECK_FALSE(check("Basic YWRtaW46cHdY****", "admin", "pwX"));
  CHECK_FALSE(check("Basic YWRtaW46cHdYcH*=", "admin", "pwX"));
}

TEST_CASE("checkBasicAuth: non-zero padding bits reject the whole credential") {
  // "u:pw" -> dTpwdw== ; "u:pwd" -> dTpwd2Q= ; +1 on the last data char sets
  // a padding bit.
  CHECK(check("Basic dTpwdw==", "u", "pw"));
  CHECK_FALSE(check("Basic dTpwdx==", "u", "pw"));
  CHECK_FALSE(check("Basic dTpwdx==", "u", "p"));
  CHECK(check("Basic dTpwd2Q=", "u", "pwd"));
  CHECK_FALSE(check("Basic dTpwd2R=", "u", "pwd"));
  CHECK_FALSE(check("Basic dTpwd2R=", "u", "p"));
  // No padding at all: the last group is complete.
  CHECK(check("Basic YWRtaW46cHdY", "admin", "pwX"));
}

TEST_CASE("checkBasicAuth checks every scheme character") {
  CHECK_FALSE(check("Xasic YWRtaW46cHc=", "admin", "pw"));
  CHECK_FALSE(check("Bxsic YWRtaW46cHc=", "admin", "pw"));
  CHECK_FALSE(check("Baxic YWRtaW46cHc=", "admin", "pw"));
  CHECK_FALSE(check("Basxc YWRtaW46cHc=", "admin", "pw"));
  CHECK_FALSE(check("BasicXYWRtaW46cHc=", "admin", "pw"));
  CHECK_FALSE(check("Zasic YWRtaW46cHc=", "admin", "pw"));
}

TEST_CASE("checkBasicAuth splits at a colon in the first byte") {
  // ":a:b" -> OmE6Yg== : user is empty (never matches), password "a:b".
  CHECK_FALSE(check("Basic OmE6Yg==", ":a", "b"));
  CHECK_FALSE(check("Basic OmE6Yg==", "a", "b"));
  // A NUL as the very first decoded byte is refused: "\0u:pw" -> AHU6cHc=
  CHECK_FALSE(check("Basic AHU6cHc=", "u", "pw"));
}

TEST_CASE("checkBasicAuth: 130-byte credentials against a longer password") {
  // The comparison must never read past the decoded bytes (ASan-checked).
  char pwd[140];
  memset(pwd, 'p', 129);
  pwd[129] = '\0';
  char cred[160];
  snprintf(cred, sizeof cred, "u:%.128s", pwd);
  REQUIRE(strlen(cred) == 130);
  char enc[256];
  char hdr[300];
  b64(cred, 130, enc);
  snprintf(hdr, sizeof hdr, "Basic %s", enc);
  CHECK_FALSE(check(hdr, "u", pwd));
  pwd[128] = '\0';
  CHECK(check(hdr, "u", pwd));
}

// ---- per-address limiter ---------------------------------------------------

namespace {

constexpr uint32_t kA = 0x0201A8C0;  // 192.168.1.2
constexpr uint32_t kB = 0x0301A8C0;  // 192.168.1.3

// n failures of `ip` at `t`; returns how many of them started a lockout.
int fail(AuthLimiter& l, uint32_t ip, int n, uint32_t t) {
  int starts = 0;
  for (int i = 0; i < n; ++i) starts += l.onResult(ip, false, t) ? 1 : 0;
  return starts;
}

}  // namespace

TEST_CASE("AuthLimiter: 10 failures of one address lock it for 60 s, not the others") {
  AuthLimiter l;
  CHECK(fail(l, kA, 9, 1000) == 0);
  CHECK(l.failuresInWindow(kA) == 9);
  CHECK_FALSE(l.locked(kA, 1000));
  CHECK(l.onResult(kA, false, 1000));
  CHECK(l.failuresInWindow(kA) == 10);
  CHECK(l.lockLevel(kA) == 1);
  uint32_t retry = 0;
  CHECK(l.locked(kA, 1000, &retry));
  CHECK(retry == 60);
  CHECK_FALSE(l.locked(kB, 1000, &retry));
  CHECK(retry == 60);  // untouched for an unlocked address
  CHECK(l.locked(kA, 1001, &retry));
  CHECK(retry == 60);  // 59.999 s rounds up
  CHECK(l.locked(kA, 60001, &retry));
  CHECK(retry == 1);
  CHECK(l.locked(kA, 60999, &retry));
  CHECK(retry == 1);
  CHECK_FALSE(l.locked(kA, 61000));
  CHECK(l.failuresInWindow(kA) == 0);
  CHECK(l.lockLevel(kA) == 1);
  CHECK(l.failuresInWindow(kB) == 0);
  CHECK(l.lockLevel(kB) == 0);
}

TEST_CASE("AuthLimiter: lockouts escalate 60 s, 300 s, 900 s, 900 s") {
  AuthLimiter l;
  const uint32_t expected[] = {60, 300, 900, 900};
  uint32_t t = 0;
  for (uint8_t i = 0; i < 4; ++i) {
    CHECK(fail(l, kA, 10, t) == 1);
    uint32_t retry = 0;
    CHECK(l.locked(kA, t, &retry));
    CHECK(retry == expected[i]);
    CHECK(l.lockLevel(kA) == i + 1);
    CHECK(l.locked(kA, t + expected[i] * 1000 - 1));
    t += expected[i] * 1000;
    CHECK_FALSE(l.locked(kA, t));
  }
}

TEST_CASE("AuthLimiter: failures while locked are ignored") {
  AuthLimiter l;
  fail(l, kA, 10, 0);
  CHECK_FALSE(l.onResult(kA, false, 10));
  CHECK(l.failuresInWindow(kA) == 10);
  CHECK(l.lockLevel(kA) == 1);
  uint32_t retry = 0;
  CHECK(l.locked(kA, 10, &retry));
  CHECK(retry == 60);  // the lock still counts from its start
  // the first failure after the lock starts a new window
  CHECK_FALSE(l.onResult(kA, false, 60000));
  CHECK(l.failuresInWindow(kA) == 1);
}

TEST_CASE("AuthLimiter: a success forgets its own address only") {
  AuthLimiter l;
  fail(l, kA, 10, 0);
  fail(l, kB, 9, 0);
  CHECK_FALSE(l.onResult(kB, true, 5));
  CHECK(l.failuresInWindow(kB) == 0);
  CHECK(l.locked(kA, 5));
  CHECK(l.lockLevel(kA) == 1);
  CHECK_FALSE(l.onResult(kA, true, 60000));
  CHECK(l.lockLevel(kA) == 0);
  CHECK(l.failuresInWindow(kA) == 0);
  // level forgotten: the next lockout is 60 s again
  fail(l, kA, 10, 70000);
  uint32_t retry = 0;
  CHECK(l.locked(kA, 70000, &retry));
  CHECK(retry == 60);
  CHECK_FALSE(l.onResult(0x0909090Au, true, 70000));  // unknown address: nothing to forget
}

TEST_CASE("AuthLimiter: failures older than 60 s restart the window") {
  AuthLimiter l;
  fail(l, kA, 9, 0);
  CHECK(l.onResult(kA, false, 59999));  // 10th inside the window starting at 0
  AuthLimiter m;
  fail(m, kA, 9, 0);
  CHECK_FALSE(m.onResult(kA, false, 60000));  // window over: a new one starts here
  CHECK(m.failuresInWindow(kA) == 1);
  CHECK(fail(m, kA, 8, 119999) == 0);
  CHECK(m.failuresInWindow(kA) == 9);
  CHECK(m.onResult(kA, false, 119999));
}

TEST_CASE("AuthLimiter: the 10th failure at 59999 ms after the first locks") {
  AuthLimiter l;
  fail(l, kA, 9, 100);
  CHECK(l.onResult(kA, false, 60099));
  CHECK(l.locked(kA, 60099));
}

TEST_CASE("AuthLimiter: a 9th address replaces the least recently used unlocked entry") {
  AuthLimiter l;
  for (uint32_t i = 1; i <= 8; ++i) l.onResult(i, false, i);
  l.onResult(1, false, 20);  // 1 used again: 2 is now the least recent
  l.onResult(9, false, 30);
  CHECK(l.failuresInWindow(9) == 1);
  CHECK(l.failuresInWindow(2) == 0);
  CHECK(l.failuresInWindow(1) == 2);
  for (uint32_t i = 3; i <= 8; ++i) CHECK(l.failuresInWindow(i) == 1);
}

TEST_CASE("AuthLimiter: locked entries are kept while an unlocked one exists") {
  AuthLimiter l;
  fail(l, 1, 10, 0);  // oldest, but locked
  for (uint32_t i = 2; i <= 8; ++i) l.onResult(i, false, i);
  l.onResult(9, false, 100);
  CHECK(l.locked(1, 100));
  CHECK(l.failuresInWindow(2) == 0);  // the least recent unlocked one went
  CHECK(l.failuresInWindow(9) == 1);
}

TEST_CASE("AuthLimiter: an expired lock counts as unlocked when a slot is needed") {
  AuthLimiter l;
  for (uint32_t i = 1; i <= 8; ++i) fail(l, i, 10, i * 1000);
  // at 61500 the lock of 1 (from 1000) is over, the others still run
  l.onResult(9, false, 61500);
  CHECK(l.failuresInWindow(9) == 1);
  CHECK(l.lockLevel(1) == 0);
  for (uint32_t i = 2; i <= 8; ++i) CHECK(l.locked(i, 61500));
}

TEST_CASE("AuthLimiter: 8 locked addresses, a 9th replaces the lock that ends first") {
  AuthLimiter l;
  // 3 gets the longest lock: 60 s + 300 s levels
  fail(l, 3, 10, 0);
  CHECK_FALSE(l.locked(3, 60000));
  for (uint32_t i = 1; i <= 8; ++i) fail(l, i, 10, 60000 + i);
  CHECK(l.lockLevel(3) == 2);
  // lock ends: i at 120000 + i, 3 at 360003; the first to end is 1
  l.onResult(9, false, 70000);
  CHECK(l.failuresInWindow(9) == 1);
  CHECK(l.lockLevel(1) == 0);
  CHECK_FALSE(l.locked(1, 70000));
  for (uint32_t i = 2; i <= 8; ++i) CHECK(l.locked(i, 70000));
  // next: 2 ends first
  fail(l, 9, 9, 70000);
  CHECK(l.locked(9, 70000));
  l.onResult(10, false, 70001);
  CHECK(l.lockLevel(2) == 0);
  CHECK(l.locked(3, 70001));
  CHECK(l.locked(9, 70001));
}

TEST_CASE("AuthLimiter: millis wrap during the window and the lock") {
  AuthLimiter l;
  const uint32_t t0 = 0xFFFFFF00u;
  fail(l, kA, 9, t0);
  CHECK(l.onResult(kA, false, t0 + 59999u));  // wraps past 2^32, still within 60 s
  uint32_t retry = 0;
  CHECK(l.locked(kA, t0 + 59999u + 59000u, &retry));
  CHECK(retry == 1);
  CHECK(l.locked(kA, t0 + 59999u + 59999u));
  CHECK_FALSE(l.locked(kA, t0 + 59999u + 60000u));
}

TEST_CASE("AuthLimiter: constants") {
  CHECK(AuthLimiter::kSlots == 8);
  CHECK(AuthLimiter::kMaxFailures == 10);
  CHECK(AuthLimiter::kWindowMs == 60000);
  CHECK(AuthLimiter::kLockMs[0] == 60000);
  CHECK(AuthLimiter::kLockMs[1] == 300000);
  CHECK(AuthLimiter::kLockMs[2] == 900000);
}
