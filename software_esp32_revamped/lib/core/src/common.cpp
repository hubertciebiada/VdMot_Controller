#include "vdm/common.h"

#include <stdio.h>
#include <string.h>

namespace vdm {

uint32_t elapsedMs(uint32_t now, uint32_t since) { return now - since; }

bool timeReached(uint32_t now, uint32_t deadline) {
  return static_cast<int32_t>(now - deadline) >= 0;
}

Backoff::Backoff(uint32_t minMs, uint32_t maxMs)
    : min_(minMs > 0 ? minMs : 1), max_(maxMs > min_ ? maxMs : min_), delay_(min_) {}

bool Backoff::due(uint32_t nowMs) const {
  return armed_ || elapsedMs(nowMs, lastMs_) >= waitMs_;
}

void Backoff::onFailure(uint32_t nowMs) {
  armed_ = false;
  lastMs_ = nowMs;
  waitMs_ = delay_;
  delay_ = delay_ >= max_ / 2 ? max_ : delay_ * 2;
}

void Backoff::reset() {
  armed_ = true;
  delay_ = min_;
}

bool copyString(char* dst, size_t cap, const char* src) {
  if (cap == 0) return false;
  if (src == nullptr) {
    dst[0] = '\0';
    return false;
  }
  size_t n = 0;
  while (n + 1 < cap && src[n] != '\0') {
    dst[n] = src[n];
    ++n;
  }
  dst[n] = '\0';
  return src[n] == '\0';
}

size_t boundedLength(const char* s, size_t max) {
  if (s == nullptr) return 0;
  size_t n = 0;
  while (n < max && s[n] != '\0') ++n;
  return n;
}

size_t utf8SequenceLength(const char* s, size_t avail) {
  if (s == nullptr || avail == 0) return 0;
  const uint8_t b0 = static_cast<uint8_t>(s[0]);
  size_t n;
  uint32_t cp;
  if (b0 >= 0xC2 && b0 <= 0xDF) {
    n = 2;
    cp = b0 & 0x1Fu;
  } else if (b0 >= 0xE0 && b0 <= 0xEF) {
    n = 3;
    cp = b0 & 0x0Fu;
  } else if (b0 >= 0xF0 && b0 <= 0xF4) {
    n = 4;
    cp = b0 & 0x07u;
  } else {
    return 0;  // ASCII, continuation byte, C0/C1 overlong lead, or > U+13FFFF
  }
  if (avail < n) return 0;
  for (size_t i = 1; i < n; ++i) {
    const uint8_t b = static_cast<uint8_t>(s[i]);
    if ((b & 0xC0u) != 0x80u) return 0;
    cp = (cp << 6) | (b & 0x3Fu);
  }
  if ((n == 3 && cp < 0x800) || (n == 4 && cp < 0x10000)) return 0;  // overlong
  if (cp >= 0xD800 && cp <= 0xDFFF) return 0;                         // surrogate
  if (cp > 0x10FFFF) return 0;
  if (cp <= 0x9F) return 0;  // C1 control
  return n;
}

bool isPrintableText(const char* s, size_t len) {
  if (s == nullptr) return len == 0;
  for (size_t i = 0; i < len; ++i) {
    const uint8_t c = static_cast<uint8_t>(s[i]);
    if (c >= 0x80) {
      const size_t n = utf8SequenceLength(s + i, len - i);
      if (n == 0) return false;
      i += n - 1;
    } else if (c < 0x20 || c == 0x7F) {
      return false;
    }
  }
  return true;
}

bool isSafeName(const char* s, size_t maxLen, bool allowEmpty) {
  if (s == nullptr) return false;
  const size_t len = boundedLength(s, maxLen + 1);
  if (len > maxLen) return false;
  if (len == 0) return allowEmpty;
  for (size_t i = 0; i < len; ++i) {
    const char c = s[i];
    if (c == '+' || c == '#' || c == '/' || c == '"' || c == '\\') return false;
  }
  return isPrintableText(s, len);
}

size_t buildHostname(const char* station, char* out, size_t cap) {
  static const char kDefault[] = "VdMot";
  if (out == nullptr || cap == 0) return 0;
  if (cap < sizeof kDefault) {
    out[0] = '\0';
    return 0;
  }
  size_t n = 0;
  bool dash = false;  // a run of replaced bytes is pending
  for (const char* p = station; p != nullptr && *p != '\0' && n + 1 < cap; ++p) {
    const char c = *p;
    const bool keep = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') ||
                      c == '-' || c == '_';
    if (!keep) {
      dash = n > 0;  // never a leading '-'
      continue;
    }
    if (dash && c != '-' && out[n - 1] != '-') {
      out[n++] = '-';
      if (n + 1 >= cap) break;
    }
    dash = false;
    out[n++] = c;
  }
  while (n > 0 && out[n - 1] == '-') --n;
  size_t first = 0;
  while (first < n && out[first] == '-') ++first;
  if (first > 0) {
    memmove(out, out + first, n - first);
    n -= first;
  }
  if (n == 0) {
    memcpy(out, kDefault, sizeof kDefault);
    return sizeof kDefault - 1;
  }
  out[n] = '\0';
  return n;
}

bool isHostName(const char* s, size_t maxLen) {
  if (s == nullptr) return false;
  const size_t len = boundedLength(s, maxLen + 1);
  if (len == 0 || len > maxLen) return false;
  if (s[0] == '-' || s[0] == '.' || s[len - 1] == '-' || s[len - 1] == '.') return false;
  for (size_t i = 0; i < len; ++i) {
    const char c = s[i];
    const bool ok = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') ||
                    c == '-' || c == '.';
    if (!ok) return false;
  }
  return true;
}

namespace {

// U+00C0..U+017F -> ASCII base letter; '\0' = two-letter form (kTwoLetter),
// '_' = not a letter (U+00D7, U+00F7).
const char kLatinBase[192 + 1] =
    "AAAAAA\0CEEEEIIII"   // U+00C0
    "DNOOOOO_OUUUUY\0\0"  // U+00D0
    "aaaaaa\0ceeeeiiii"   // U+00E0
    "dnooooo_ouuuuy\0y"   // U+00F0
    "AaAaAaCcCcCcCcDd"    // U+0100
    "DdEeEeEeEeEeGgGg"    // U+0110
    "GgGgHhHhIiIiIiIi"    // U+0120
    "Ii\0\0JjKkkLlLlLlL"  // U+0130
    "lLlNnNnNnnNnOoOo"    // U+0140
    "Oo\0\0RrRrRrSsSsSs"  // U+0150
    "SsTtTtTtUuUuUuUu"    // U+0160
    "UuUuWwYyYZzZzZzs";   // U+0170
static_assert(sizeof kLatinBase == 193, "one byte per code point");
const uint16_t kTwoLetterCp[] = {0xC6, 0xDE, 0xDF, 0xE6, 0xFE, 0x132, 0x133, 0x152, 0x153};
const char kTwoLetter[] = "AETHssaethIJijOEoe";

bool isIdChar(uint8_t c) {
  return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '_' ||
         c == '-';
}

}  // namespace

size_t buildHaId(const char* in, size_t len, char* out, size_t cap) {
  if (out == nullptr || cap == 0) return 0;
  len = boundedLength(in, len);
  size_t n = 0;
  for (size_t i = 0; i < len;) {
    const uint8_t c = static_cast<uint8_t>(in[i]);
    char id[3] = {'_', '\0', '\0'};
    size_t step = 1;
    if (c < 0x80) {
      if (isIdChar(c)) id[0] = static_cast<char>(c);
    } else {
      const size_t seq = utf8SequenceLength(in + i, len - i);
      if (seq != 0) step = seq;  // an invalid byte is one '_' of its own
      const uint32_t cp =
          seq == 2 ? (c & 0x1Fu) << 6 | (static_cast<uint8_t>(in[i + 1]) & 0x3Fu) : 0;
      if (cp >= 0xC0 && cp <= 0x17F) {
        id[0] = kLatinBase[cp - 0xC0];
        for (size_t k = 0; k < sizeof kTwoLetterCp / sizeof *kTwoLetterCp; ++k) {
          if (kTwoLetterCp[k] != cp) continue;
          id[0] = kTwoLetter[2 * k];
          id[1] = kTwoLetter[2 * k + 1];
        }
      }
    }
    for (const char* p = id; *p != '\0'; ++p) {
      if (n + 1 >= cap) {
        out[0] = '\0';
        return 0;
      }
      out[n++] = *p;
    }
    i += step;
  }
  out[n] = '\0';
  return n;
}

namespace {

// Parses 1..10 decimal digits into a 64-bit accumulator.
bool parseDigits(const char* s, size_t len, uint64_t& out) {
  if (s == nullptr || len == 0 || len > 10) return false;
  uint64_t v = 0;
  for (size_t i = 0; i < len; ++i) {
    if (s[i] < '0' || s[i] > '9') return false;
    v = v * 10 + static_cast<uint64_t>(s[i] - '0');
  }
  out = v;
  return true;
}

}  // namespace

bool parseUint(const char* s, size_t len, uint32_t max, uint32_t& out) {
  uint64_t v = 0;
  if (!parseDigits(s, len, v) || v > max) return false;
  out = static_cast<uint32_t>(v);
  return true;
}

bool parseInt(const char* s, size_t len, int32_t min, int32_t max, int32_t& out) {
  if (s == nullptr || len == 0) return false;
  const bool neg = s[0] == '-';
  uint64_t mag = 0;
  if (!parseDigits(neg ? s + 1 : s, neg ? len - 1 : len, mag)) return false;
  const int64_t v = neg ? -static_cast<int64_t>(mag) : static_cast<int64_t>(mag);
  if (v < min || v > max) return false;
  out = static_cast<int32_t>(v);
  return true;
}

bool parseIpv4(const char* s, size_t len, uint32_t& out) {
  if (s == nullptr) return false;
  uint32_t ip = 0;
  size_t pos = 0;
  for (int octet = 0; octet < 4; ++octet) {
    size_t start = pos;
    while (pos < len && s[pos] != '.') ++pos;
    uint32_t v = 0;
    if (pos - start > 3 || !parseUint(s + start, pos - start, 255, v)) return false;
    ip |= v << (8 * octet);
    if (octet < 3) {
      if (pos >= len) return false;
      ++pos;  // skip '.'
    }
  }
  if (pos != len) return false;
  out = ip;
  return true;
}

size_t formatIpv4(uint32_t ip, char* out, size_t cap) {
  if (cap == 0) return 0;
  const int n = snprintf(out, cap, "%u.%u.%u.%u", static_cast<unsigned>(ip & 0xFF),
                         static_cast<unsigned>((ip >> 8) & 0xFF),
                         static_cast<unsigned>((ip >> 16) & 0xFF),
                         static_cast<unsigned>((ip >> 24) & 0xFF));
  if (n < 0 || static_cast<size_t>(n) >= cap) {
    out[0] = '\0';
    return 0;
  }
  return static_cast<size_t>(n);
}

bool roundTargetPercent(double v, uint8_t& out) {
  if (!(v >= 0.0 && v <= 100.0)) return false;  // also NaN
  out = static_cast<uint8_t>(v + 0.5);
  return true;
}

bool operator==(const OneWireId& a, const OneWireId& b) { return memcmp(a.b, b.b, 8) == 0; }
bool operator!=(const OneWireId& a, const OneWireId& b) { return !(a == b); }

bool isZero(const OneWireId& id) {
  for (uint8_t v : id.b) {
    if (v != 0) return false;
  }
  return true;
}

bool crcValid(const OneWireId& id) {
  uint8_t crc = 0;
  for (int i = 0; i < 7; ++i) {
    uint8_t in = id.b[i];
    for (int bit = 0; bit < 8; ++bit) {
      const uint8_t mix = (crc ^ in) & 0x01;
      crc >>= 1;
      if (mix) crc ^= 0x8C;
      in >>= 1;
    }
  }
  return crc == id.b[7];
}

namespace {

int hexValue(char c) {
  if (c >= '0' && c <= '9') return c - '0';
  if (c >= 'a' && c <= 'f') return c - 'a' + 10;
  if (c >= 'A' && c <= 'F') return c - 'A' + 10;
  return -1;
}

}  // namespace

bool parseOneWireId(const char* s, size_t len, OneWireId& out) {
  if (s == nullptr || len != kOneWireIdTextLen) return false;
  OneWireId id;
  for (size_t i = 0; i < 8; ++i) {
    const size_t p = i * 3;
    const int hi = hexValue(s[p]);
    const int lo = hexValue(s[p + 1]);
    if (hi < 0 || lo < 0) return false;
    if (i < 7 && s[p + 2] != '-') return false;
    id.b[i] = static_cast<uint8_t>(hi * 16 + lo);
  }
  out = id;
  return true;
}

size_t formatOneWireId(const OneWireId& id, char* out, size_t cap) {
  if (cap == 0) return 0;
  if (cap < kOneWireIdTextLen + 1) {
    out[0] = '\0';
    return 0;
  }
  static const char kHex[] = "0123456789abcdef";
  size_t n = 0;
  for (size_t i = 0; i < 8; ++i) {
    out[n++] = kHex[id.b[i] >> 4];
    out[n++] = kHex[id.b[i] & 0x0F];
    if (i < 7) out[n++] = '-';
  }
  out[n] = '\0';
  return n;
}

}  // namespace vdm
