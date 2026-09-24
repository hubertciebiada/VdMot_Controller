#include "vdm/arg_parser.h"

namespace vdm {

namespace {

bool isDigit(char c) { return c >= '0' && c <= '9'; }

// Parses the digit-only magnitude; false on empty input, a non-digit or a
// value above `limit`.
bool parseMagnitude(const char* s, uint32_t limit, uint32_t& out) {
  if (*s == '\0') return false;
  uint32_t value = 0;
  for (; *s != '\0'; ++s) {
    if (!isDigit(*s)) return false;
    const uint32_t digit = static_cast<uint32_t>(*s - '0');
    if (value > limit / 10 || (value == limit / 10 && digit > limit % 10)) return false;
    value = value * 10 + digit;
  }
  out = value;
  return true;
}

int hexValue(char c) {
  if (c >= '0' && c <= '9') return c - '0';
  if (c >= 'a' && c <= 'f') return c - 'a' + 10;
  if (c >= 'A' && c <= 'F') return c - 'A' + 10;
  return -1;  // NOMUTATE: callers only test hexValue() < 0, so any negative sentinel (-1/-2) is indistinguishable
}

}  // namespace

bool parseU32(const char* s, uint32_t lo, uint32_t hi, uint32_t& out) {
  if (s == nullptr) return false;
  uint32_t value = 0;  // NOMUTATE: initial value is dead; parseMagnitude() overwrites it on success and it is unused on failure
  if (!parseMagnitude(s, hi, value) || value < lo) return false;
  out = value;
  return true;
}

bool parseI32(const char* s, int32_t lo, int32_t hi, int32_t& out) {
  if (s == nullptr) return false;
  bool negative = false;
  if (*s == '+' || *s == '-') {
    negative = (*s == '-');
    ++s;
  }
  // 2147483648 is representable only as a negative value.
  const uint32_t limit = negative ? 2147483648u : 2147483647u;
  uint32_t magnitude = 0;  // NOMUTATE: initial value is dead; parseMagnitude() overwrites it on success and it is unused on failure
  if (!parseMagnitude(s, limit, magnitude)) return false;
  int32_t value;
  if (negative) {
    value = magnitude == 2147483648u ? INT32_MIN : -static_cast<int32_t>(magnitude);
  } else {
    value = static_cast<int32_t>(magnitude);
  }
  if (value < lo || value > hi) return false;
  out = value;
  return true;
}

bool parseOneWireAddress(const char* s, uint8_t (&out)[8]) {
  if (s == nullptr) return false;
  uint8_t parsed[8];  // NOMUTATE: scratch array size; only indices 0..7 are written/read, a larger size is unobservable
  for (size_t i = 0; i < 8; ++i) {
    const char* p = s + i * 3;
    const int hi = hexValue(p[0]);
    if (hi < 0) return false;
    const int lo = hexValue(p[1]);
    if (lo < 0) return false;
    const char sep = p[2];
    if (i < 7 ? sep != '-' : sep != '\0') return false;
    parsed[i] = static_cast<uint8_t>(hi * 16 + lo);
  }
  for (size_t i = 0; i < 8; ++i) out[i] = parsed[i];
  return true;
}

bool isZeroAddress(const uint8_t (&addr)[8]) {
  for (uint8_t b : addr) {
    if (b != 0) return false;
  }
  return true;
}

}  // namespace vdm
