#include "vdm/version.h"

#include <stdio.h>
#include <string.h>

#include "vdm/common.h"

#ifndef VDM_VERSION
#define VDM_VERSION "0.0.0-native"
#endif
#ifndef VDM_MIN_STM_VERSION
#define VDM_MIN_STM_VERSION "1.4.0"
#endif

namespace vdm {

namespace {

constexpr size_t kVersionMaxLen = 31;
// The suffix is what follows the shortest numeric part "0.0.0", so it always
// fits and parseVersion needs no runtime length check for it.
static_assert(sizeof(Version::suffix) >= kVersionMaxLen - 5 + 1, "suffix");  // NOMUTATE

bool isSuffixChar(char c) {
  return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '.' ||
         c == '_' || c == '+' || c == '-';
}

// Reads "<digits>" at s[pos..], stops at the first non-digit.
bool readComponent(const char* s, size_t len, size_t& pos, uint16_t& out) {
  const size_t start = pos;
  while (pos < len && s[pos] >= '0' && s[pos] <= '9') ++pos;
  uint32_t v;
  if (pos - start > 5 || !parseUint(s + start, pos - start, 65535, v)) return false;
  out = static_cast<uint16_t>(v);
  return true;
}

// "_C<1..2 digits>" exactly.
bool isHwPart(const char* s, size_t len) {
  if (len < 3 || len > 4 || s[0] != '_' || s[1] != 'C') return false;
  for (size_t i = 2; i < len; ++i) {
    if (s[i] < '0' || s[i] > '9') return false;
  }
  return true;
}

}  // namespace

bool parseVersion(const char* s, size_t len, Version& out) {
  out = Version{};
  if (s == nullptr || len > kVersionMaxLen) return false;  // len 0: no major
  Version v;
  size_t pos = 0;
  if (!readComponent(s, len, pos, v.major) || pos >= len || s[pos++] != '.') return false;
  if (!readComponent(s, len, pos, v.minor) || pos >= len || s[pos++] != '.') return false;
  if (!readComponent(s, len, pos, v.patch)) return false;

  size_t rest = len - pos;
  const char* tail = s + pos;
  // The suffix starts with a separator; the rest are suffix characters.
  for (size_t i = 0; i < rest; ++i) {
    const char c = tail[i];
    const bool ok = i == 0 ? (c == '-' || c == '_' || c == '+') : isSuffixChar(c);
    if (!ok) return false;
  }

  // hw = last '_' part when it is "_C<n>".
  const char* lastUnderscore = nullptr;
  for (size_t i = 0; i < rest; ++i) {
    if (tail[i] == '_') lastUnderscore = tail + i;
  }
  if (lastUnderscore != nullptr) {
    const size_t hwLen = static_cast<size_t>(tail + rest - lastUnderscore);
    if (isHwPart(lastUnderscore, hwLen)) {
      // "C<digits>"; v.hw is zero-initialised, so the NUL is already there.
      for (size_t i = 1; i < hwLen; ++i) v.hw[i - 1] = lastUnderscore[i];
      rest -= hwLen;
    }
  }
  memcpy(v.suffix, tail, rest);
  v.suffix[rest] = '\0';
  v.valid = true;
  out = v;
  return true;
}

int compareVersion(const Version& a, const Version& b) {
  if (!a.valid || !b.valid) return (a.valid ? 1 : 0) - (b.valid ? 1 : 0);
  if (a.major != b.major) return a.major < b.major ? -1 : 1;
  if (a.minor != b.minor) return a.minor < b.minor ? -1 : 1;
  if (a.patch != b.patch) return a.patch < b.patch ? -1 : 1;
  return 0;
}

bool isRevamped(const Version& v) { return v.valid && strstr(v.suffix, "revamped") != nullptr; }

size_t formatVersion(const Version& v, char* out, size_t cap) {
  if (cap == 0) return 0;
  out[0] = '\0';
  if (!v.valid) return 0;
  const int n = snprintf(out, cap, "%u.%u.%u%s%s%s", static_cast<unsigned>(v.major),
                         static_cast<unsigned>(v.minor), static_cast<unsigned>(v.patch), v.suffix,
                         v.hw[0] ? "_" : "", v.hw);
  if (static_cast<size_t>(n) >= cap) {  // does not fit (snprintf cannot fail here)
    out[0] = '\0';
    return 0;
  }
  return static_cast<size_t>(n);
}

const char* firmwareVersion() { return VDM_VERSION; }

const char* minStmVersion() { return VDM_MIN_STM_VERSION; }

const char* stmSupportName(StmSupport s) {
  switch (s) {
    case StmSupport::Unknown: return "unknown";
    case StmSupport::Supported: return "ok";
    case StmSupport::TooOld: return "too_old";
  }
  return "unknown";
}

StmSupport stmSupport(const Version& v) {
  if (!v.valid) return StmSupport::Unknown;
  Version min;
  parseVersion(minStmVersion(), strlen(minStmVersion()), min);
  return compareVersion(v, min) < 0 ? StmSupport::TooOld : StmSupport::Supported;
}

}  // namespace vdm
