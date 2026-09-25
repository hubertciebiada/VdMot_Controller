// version: gvers grammar (all documented examples, every rejection),
// comparison, revamped detection, canonical formatting, build constants,
// fuzz.
#include <stdint.h>
#include <string.h>

#include <random>
#include <string>

#include "doctest.h"
#include "vdm/version.h"

using namespace vdm;

namespace {

bool parse(const std::string& s, Version& v) { return parseVersion(s.data(), s.size(), v); }

Version ver(const char* s) {
  Version v;
  REQUIRE(parseVersion(s, strlen(s), v));
  return v;
}

std::string fmt(const Version& v) {
  char out[40];
  const size_t n = formatVersion(v, out, sizeof out);
  CHECK(n == strlen(out));
  return out;
}

}  // namespace

TEST_CASE("version: documented examples") {
  struct Case {
    const char* text;
    uint16_t major, minor, patch;
    const char* suffix;
    const char* hw;
  };
  const Case cases[] = {
      {"1.4.9_Dev_C2", 1, 4, 9, "_Dev", "C2"},
      {"1.4.9_C1", 1, 4, 9, "", "C1"},
      {"2.0.0-revamped_C2", 2, 0, 0, "-revamped", "C2"},
      {"2.0.0-revamped-dev", 2, 0, 0, "-revamped-dev", ""},
      {"1.4.12+hc2", 1, 4, 12, "+hc2", ""},
      {"1.4.10", 1, 4, 10, "", ""},
      {"0.0.0", 0, 0, 0, "", ""},
      {"65535.65535.65535", 65535, 65535, 65535, "", ""},
      {"00001.02.3", 1, 2, 3, "", ""},
      {"1.4.9_C99", 1, 4, 9, "", "C99"},
      {"1.4.9_C0", 1, 4, 9, "", "C0"},
      {"1.4.9_C123", 1, 4, 9, "_C123", ""},
      {"1.4.9_C", 1, 4, 9, "_C", ""},
      {"1.4.9_c2", 1, 4, 9, "_c2", ""},
      {"1.4.9_C2_Dev", 1, 4, 9, "_C2_Dev", ""},
      {"1.4.9_C2x", 1, 4, 9, "_C2x", ""},
      {"1.4.9-", 1, 4, 9, "-", ""},
      {"1.4.9_", 1, 4, 9, "_", ""},
      {"1.4.9+", 1, 4, 9, "+", ""},
      {"1.4.9-a.b_c+d-e", 1, 4, 9, "-a.b_c+d-e", ""},
      {"1.4.9__C2", 1, 4, 9, "_", "C2"},
  };
  for (const Case& k : cases) {
    CAPTURE(k.text);
    Version v;
    v.major = 77;
    REQUIRE(parse(k.text, v));
    CHECK(v.valid);
    CHECK(v.major == k.major);
    CHECK(v.minor == k.minor);
    CHECK(v.patch == k.patch);
    CHECK(std::string(v.suffix) == k.suffix);
    CHECK(std::string(v.hw) == k.hw);
  }
}

TEST_CASE("version: longest strings") {
  // 31 chars: the longest accepted; suffix of 26 chars fits.
  const std::string s31 = "1.2.3-" + std::string(25, 'x');
  REQUIRE(s31.size() == 31);
  Version v;
  CHECK(parse(s31, v));
  CHECK(std::string(v.suffix) == "-" + std::string(25, 'x'));
  CHECK(fmt(v) == s31);
  const std::string hw31 = "1.2.3-" + std::string(21, 'x') + "_C12";
  REQUIRE(hw31.size() == 31);
  CHECK(parse(hw31, v));
  CHECK(std::string(v.hw) == "C12");
  CHECK(fmt(v) == hw31);
  CHECK_FALSE(parse(s31 + "x", v));
  CHECK_FALSE(v.valid);
}

TEST_CASE("version: every rejection resets the output") {
  const char* bad[] = {"",           "1",          "1.4",        "1.4.",       "1..4.9",
                       ".1.4.9",     "a.b.c",      "1.4.9 C2",   "1.4.9#",     "1.4.9x",
                       "1.4.9.1",    "1.4.9a",     "65536.0.0",  "0.65536.0",  "0.0.65536",
                       "123456.0.0", "-1.4.9",     "+1.4.9",     "1.4.-9",     "1,4,9",
                       "1.4.9_Dev!", "1.4.9-\x80", "1.4.9\n",    " 1.4.9",     "1.4.9 "};
  for (const char* b : bad) {
    CAPTURE(b);
    Version v = ver("2.0.0-revamped_C2");
    CHECK_FALSE(parse(b, v));
    CHECK_FALSE(v.valid);
    CHECK(v.major == 0);
    CHECK(v.suffix[0] == '\0');
    CHECK(v.hw[0] == '\0');
  }
  Version v = ver("1.2.3");
  CHECK_FALSE(parseVersion(nullptr, 5, v));
  CHECK_FALSE(v.valid);
  // Only `len` bytes count; a NUL inside is a bad char.
  CHECK(parseVersion("1.4.9_C2junk", 8, v));
  CHECK(std::string(v.hw) == "C2");
  CHECK_FALSE(parseVersion("1.4.9\0x", 7, v));
}

TEST_CASE("version: comparison uses the numeric part only") {
  CHECK(compareVersion(ver("1.4.9"), ver("1.4.9")) == 0);
  CHECK(compareVersion(ver("2.0.0-revamped"), ver("2.0.0")) == 0);
  CHECK(compareVersion(ver("1.4.9_C1"), ver("1.4.9_Dev_C2")) == 0);
  CHECK(compareVersion(ver("1.4.10"), ver("1.4.9")) > 0);
  CHECK(compareVersion(ver("1.4.9"), ver("1.4.10")) < 0);
  CHECK(compareVersion(ver("1.5.0"), ver("1.4.99")) > 0);
  CHECK(compareVersion(ver("1.4.99"), ver("1.5.0")) < 0);
  CHECK(compareVersion(ver("2.0.0"), ver("1.99.99")) > 0);
  CHECK(compareVersion(ver("1.99.99"), ver("2.0.0")) < 0);
  CHECK(compareVersion(ver("2.0.0-revamped"), ver("1.4.0")) > 0);
  CHECK(compareVersion(ver("1.3.9"), ver("1.4.0")) < 0);
  const Version invalid;
  CHECK(compareVersion(invalid, ver("0.0.0")) < 0);
  CHECK(compareVersion(ver("0.0.0"), invalid) > 0);
  CHECK(compareVersion(invalid, invalid) == 0);
  CHECK(compareVersion(ver("1.0.0"), ver("0.0.1")) == 1);
  CHECK(compareVersion(ver("0.0.1"), ver("1.0.0")) == -1);
  CHECK(compareVersion(invalid, ver("1.0.0")) == -1);
  CHECK(compareVersion(ver("1.0.0"), invalid) == 1);
}

TEST_CASE("version: character classes and length edges") {
  Version v;
  CHECK(parse("1.4.9-azAZ09._+-", v));
  CHECK(std::string(v.suffix) == "-azAZ09._+-");
  const char* badChars[] = {"1.4.9-`", "1.4.9-{", "1.4.9-@", "1.4.9-[", "1.4.9-/",
                            "1.4.9-:", "1.4.9-~", "1.4.9-!"};
  for (const char* b : badChars) {
    CAPTURE(b);
    CHECK_FALSE(parse(b, v));
  }
  // Components: at most 5 digits even when the value is small.
  CHECK(parse("00000.00000.00000", v));
  CHECK_FALSE(parse("000001.2.3", v));
  CHECK_FALSE(parse("1.000001.3", v));
  CHECK_FALSE(parse("1.2.000001", v));
  // Only `len` bytes are read (buffers need not be terminated).
  CHECK(parseVersion("1.4.95", 5, v));
  CHECK(v.patch == 9);
  CHECK(parseVersion("1.4.9_C2_", 8, v));
  CHECK(std::string(v.hw) == "C2");
  CHECK(std::string(v.suffix).empty());
  CHECK(parseVersion("1.4.9_C99", 9, v));
  CHECK(std::string(v.hw) == "C99");
  CHECK_FALSE(parseVersion("1.4.9", 0, v));
  CHECK_FALSE(parseVersion("1", 1, v));
  CHECK_FALSE(parseVersion("1.", 2, v));
  CHECK_FALSE(parseVersion("1.4", 3, v));
  CHECK_FALSE(parseVersion("1.4.", 4, v));
  // "_C" followed by a non-digit is part of the suffix.
  CHECK(parse("1.4.9_Cx", v));
  CHECK(std::string(v.suffix) == "_Cx");
  CHECK(std::string(v.hw).empty());
  CHECK(parse("1.4.9_C1x", v));
  CHECK(std::string(v.suffix) == "_C1x");
  CHECK(parse("1.4.9_D12", v));
  CHECK(std::string(v.suffix) == "_D12");
}

TEST_CASE("version: comparison returns exactly -1, 0 or 1") {
  CHECK(compareVersion(ver("1.2.0"), ver("1.1.9")) == 1);
  CHECK(compareVersion(ver("1.1.9"), ver("1.2.0")) == -1);
  CHECK(compareVersion(ver("1.1.2"), ver("1.1.1")) == 1);
  CHECK(compareVersion(ver("1.1.1"), ver("1.1.2")) == -1);
  CHECK(compareVersion(ver("3.0.0"), ver("1.9.9")) == 1);
  CHECK(compareVersion(ver("1.9.9"), ver("3.0.0")) == -1);
}

TEST_CASE("version: revamped detection") {
  CHECK(isRevamped(ver("2.0.0-revamped")));
  CHECK(isRevamped(ver("2.0.0-revamped_C2")));
  CHECK(isRevamped(ver("2.0.0-revamped-dev")));
  CHECK(isRevamped(ver("2.0.0_xrevampedx")));
  CHECK_FALSE(isRevamped(ver("2.0.0-Revamped")));
  CHECK_FALSE(isRevamped(ver("1.4.9_Dev_C2")));
  CHECK_FALSE(isRevamped(ver("2.0.0")));
  Version fake;
  strcpy(fake.suffix, "-revamped");  // not valid -> never revamped
  CHECK_FALSE(isRevamped(fake));
}

TEST_CASE("version: canonical formatting") {
  const char* texts[] = {"1.4.9_Dev_C2", "1.4.9_C1", "2.0.0-revamped_C2", "2.0.0-revamped-dev",
                         "1.4.12+hc2", "65535.65535.65535"};
  for (const char* t : texts) {
    CAPTURE(t);
    CHECK(fmt(ver(t)) == t);
  }
  CHECK(fmt(ver("00001.02.3")) == "1.2.3");
  char out[16];
  const Version v = ver("1.4.9_C1");  // 8 chars
  memset(out, 'z', sizeof out);
  CHECK(formatVersion(v, out, 9) == 8);
  CHECK(std::string(out) == "1.4.9_C1");
  memset(out, 'z', sizeof out);
  CHECK(formatVersion(v, out, 8) == 0);
  CHECK(out[0] == '\0');
  CHECK(out[8] == 'z');  // never past cap
  out[0] = 'q';
  CHECK(formatVersion(v, out, 0) == 0);
  CHECK(out[0] == 'q');
  const Version invalid;
  memset(out, 'z', sizeof out);
  CHECK(formatVersion(invalid, out, sizeof out) == 0);
  CHECK(out[0] == '\0');
}

TEST_CASE("version: build constants") {
  CHECK(std::string(firmwareVersion()) == "0.0.0-native");
  CHECK(std::string(minStmVersion()) == "1.4.0");
  Version fw, min;
  CHECK(parseVersion(firmwareVersion(), strlen(firmwareVersion()), fw));
  CHECK(parseVersion(minStmVersion(), strlen(minStmVersion()), min));
  // The revamped STM passes the gate the old ESP applies.
  CHECK(compareVersion(ver("2.0.0-revamped_C2"), min) >= 0);
}

TEST_CASE("version: stmSupport against the 1.4.0 minimum (W13.1)") {
  CHECK(stmSupport(ver("1.3.7_C2")) == StmSupport::TooOld);
  CHECK(stmSupport(ver("1.3.99")) == StmSupport::TooOld);
  CHECK(stmSupport(ver("0.9.9")) == StmSupport::TooOld);
  CHECK(stmSupport(ver("1.4.0_C1")) == StmSupport::Supported);
  CHECK(stmSupport(ver("1.4.0")) == StmSupport::Supported);
  CHECK(stmSupport(ver("1.4.9_Dev_C2")) == StmSupport::Supported);
  CHECK(stmSupport(ver("2.1.0-revamped_C2")) == StmSupport::Supported);
  CHECK(stmSupport(Version{}) == StmSupport::Unknown);
  Version invalid;
  CHECK_FALSE(parse("1.4", invalid));
  CHECK(stmSupport(invalid) == StmSupport::Unknown);

  CHECK(std::string(stmSupportName(StmSupport::Unknown)) == "unknown");
  CHECK(std::string(stmSupportName(StmSupport::Supported)) == "ok");
  CHECK(std::string(stmSupportName(StmSupport::TooOld)) == "too_old");
  CHECK(std::string(stmSupportName(static_cast<StmSupport>(3))) == "unknown");
}

TEST_CASE("version: fuzz") {
  std::mt19937 rng(777);
  const char alphabet[] = "0123456789._-+Cc revampedDv\x01\xff";
  for (int iter = 0; iter < 20000; ++iter) {
    std::string s;
    const size_t n = rng() % 40;
    for (size_t i = 0; i < n; ++i) s += alphabet[rng() % (sizeof alphabet - 1)];
    Version v;
    if (parse(s, v)) {
      CHECK(v.valid);
      CHECK(s.size() <= 31);
      CHECK(strlen(v.suffix) < sizeof v.suffix);
      CHECK(strlen(v.hw) < sizeof v.hw);
      // Canonical form parses back to the same fields.
      char out[40];
      REQUIRE(formatVersion(v, out, sizeof out) > 0);
      Version w;
      REQUIRE(parseVersion(out, strlen(out), w));
      CHECK(compareVersion(v, w) == 0);
      CHECK(std::string(v.suffix) == w.suffix);
      CHECK(std::string(v.hw) == w.hw);
    } else {
      CHECK_FALSE(v.valid);
    }
  }
}
