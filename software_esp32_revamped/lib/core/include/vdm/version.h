// Firmware version strings: this ESP build, and parsing of STM `gvers`
// versions. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace vdm {

// A parsed version such as "1.4.9_Dev_C2", "2.0.0-revamped_C2",
// "2.0.0-revamped-dev" or "1.4.12+hc2".
//
// Grammar (the whole string, 1..31 chars, printable ASCII without spaces):
//   version := major '.' minor '.' patch [suffix] [hw]
//   major/minor/patch := 1..5 decimal digits, value 0..65535
//   hw     := '_' 'C' 1..2 digits          (only when it is the LAST '_' part)
//   suffix := one of '-', '_', '+' followed by [A-Za-z0-9._+-]*  (may be "")
// Examples:
//   "1.4.9_Dev_C2"       -> 1.4.9 suffix "_Dev"      hw "C2"
//   "1.4.9_C1"           -> 1.4.9 suffix ""          hw "C1"
//   "2.0.0-revamped_C2"  -> 2.0.0 suffix "-revamped" hw "C2"
//   "2.0.0-revamped-dev" -> 2.0.0 suffix "-revamped-dev" hw ""
//   "1.4"                -> invalid (patch missing)
struct Version {
  bool valid = false;
  uint16_t major = 0;
  uint16_t minor = 0;
  uint16_t patch = 0;
  // Verbatim including its leading separator. 31 chars minus the shortest
  // numeric part "0.0.0" leaves at most 26 chars, so every grammatical
  // version fits.
  char suffix[27] = {0};
  char hw[4] = {0};       // "C1".."C99" or ""
};

// Parses `len` bytes of `s`. On any grammar violation returns false and sets
// out = Version{} (valid == false).
bool parseVersion(const char* s, size_t len, Version& out);

// Compares the numeric part only: <0, 0, >0 like strcmp. Suffix and hw are
// ignored ("2.0.0-revamped" == "2.0.0"). Invalid versions compare lowest.
int compareVersion(const Version& a, const Version& b);

// True when the suffix contains "revamped" (case-sensitive), i.e. the STM runs
// the revamped firmware. Protocol v2 is still detected with `gproto`, never
// from the version string.
bool isRevamped(const Version& v);

// Writes "M.m.p<suffix>[_<hw>]" (the canonical form of the input). Returns the
// number of chars written, 0 if the version is invalid or does not fit.
size_t formatVersion(const Version& v, char* out, size_t cap);

// Version of this ESP build: the VDM_VERSION build flag ("2.0.0-revamped" or
// "2.0.0-revamped-dev"). Native builds without the flag report
// "0.0.0-native". Never null.
const char* firmwareVersion();

// Minimum STM version the ESP accepts without the "incompatible" banner:
// the VDM_MIN_STM_VERSION build flag, default "1.4.0".
const char* minStmVersion();

}  // namespace vdm
