// Tests for vdm/common.h helpers: time, Backoff, bounded strings, UTF-8,
// names, host names, HA ids, strict number parsers, IPv4, target rounding
// and 1-Wire ids. Every boundary is checked from both sides (written against
// mutation testing).
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include <limits>
#include <string>

#include "doctest.h"
#include "vdm/common.h"

using namespace vdm;

namespace {

// Reference Dallas/Maxim CRC-8 of the first 7 ROM bytes (bitwise, reflected
// 0x31), independent of crcValid().
uint8_t refCrc(const OneWireId& x) {
  uint8_t crc = 0;
  for (int i = 0; i < 7; ++i) {
    for (int bit = 0; bit < 8; ++bit) {
      const bool mix = ((crc ^ (x.b[i] >> bit)) & 1) != 0;
      crc = static_cast<uint8_t>(crc >> 1);
      if (mix) crc ^= 0x8C;
    }
  }
  return crc;
}

}  // namespace

TEST_CASE("common: elapsedMs and timeReached") {
  CHECK(elapsedMs(100, 40) == 60);
  CHECK(elapsedMs(40, 100) == 0xFFFFFFC4u);
  CHECK(elapsedMs(7, 7) == 0);
  CHECK(timeReached(100, 100));
  CHECK_FALSE(timeReached(99, 100));
  CHECK(timeReached(101, 100));
  CHECK(timeReached(100, 50));
  CHECK_FALSE(timeReached(50, 100));
  CHECK_FALSE(timeReached(0, 1));
  CHECK(timeReached(0x7FFFFFFFu, 0));
  CHECK_FALSE(timeReached(0x80000000u, 0));
  CHECK(timeReached(3, 0xFFFFFFFEu));
  CHECK_FALSE(timeReached(0xFFFFFFFEu, 3));
}

TEST_CASE("common: Backoff exact doubling and cap") {
  Backoff b(10, 21);
  CHECK(b.delayMs() == 10);
  b.onFailure(0);
  CHECK(b.delayMs() == 21);  // 10 >= 21/2: straight to the cap
  CHECK_FALSE(b.due(9));
  CHECK(b.due(10));
  b.onFailure(10);
  CHECK(b.delayMs() == 21);
  CHECK_FALSE(b.due(30));
  CHECK(b.due(31));

  Backoff c(3, 14);  // 3 -> 6 -> 12 -> 14
  c.onFailure(0);
  CHECK(c.delayMs() == 6);
  c.onFailure(0);
  CHECK(c.delayMs() == 12);
  c.onFailure(0);
  CHECK(c.delayMs() == 14);

  Backoff one(1, 1);
  CHECK(one.delayMs() == 1);
  one.onFailure(5);
  CHECK(one.delayMs() == 1);
  CHECK_FALSE(one.due(5));
  CHECK(one.due(6));

  Backoff inv(50, 10);  // max below min is raised to min
  inv.onFailure(0);
  CHECK(inv.delayMs() == 50);
  CHECK_FALSE(inv.due(49));
  CHECK(inv.due(50));
}

TEST_CASE("common: Backoff reset re-arms at once") {
  Backoff b(1000, 8000);
  b.onFailure(0);
  b.onFailure(1000);
  CHECK(b.delayMs() == 4000);
  CHECK_FALSE(b.due(1001));
  b.reset();
  CHECK(b.due(1001));
  CHECK(b.delayMs() == 1000);
  b.onFailure(2000);
  CHECK_FALSE(b.due(2999));
  CHECK(b.due(3000));
}

TEST_CASE("common: copyString") {
  char buf[8];
  memset(buf, 'x', sizeof buf);
  CHECK_FALSE(copyString(buf, 0, "a"));
  CHECK(buf[0] == 'x');  // cap 0 writes nothing

  CHECK(copyString(buf, 1, ""));
  CHECK(buf[0] == '\0');
  memset(buf, 'x', sizeof buf);
  CHECK_FALSE(copyString(buf, 1, "a"));
  CHECK(buf[0] == '\0');

  memset(buf, 'x', sizeof buf);
  CHECK_FALSE(copyString(buf, 4, nullptr));
  CHECK(buf[0] == '\0');
  CHECK(buf[1] == 'x');

  memset(buf, 'x', sizeof buf);
  CHECK(copyString(buf, 4, "abc"));
  CHECK(std::string(buf) == "abc");
  memset(buf, 'x', sizeof buf);
  CHECK_FALSE(copyString(buf, 3, "abc"));
  CHECK(std::string(buf) == "ab");
  CHECK(buf[3] == 'x');
  CHECK(copyString(buf, 8, "ab"));
  CHECK(std::string(buf) == "ab");
}

TEST_CASE("common: boundedLength") {
  CHECK(boundedLength(nullptr, 5) == 0);
  CHECK(boundedLength("", 5) == 0);
  CHECK(boundedLength("abc", 5) == 3);
  CHECK(boundedLength("abc", 3) == 3);
  CHECK(boundedLength("abc", 2) == 2);
  CHECK(boundedLength("abc", 0) == 0);
}

TEST_CASE("common: utf8SequenceLength lead byte ranges") {
  // Every lead byte with valid-looking continuation bytes.
  for (int b0 = 0; b0 < 256; ++b0) {
    char s[4] = {static_cast<char>(b0), '\xa0', '\x90', '\x90'};
    size_t expect = 0;
    if (b0 >= 0xC2 && b0 <= 0xDF) expect = 2;
    else if (b0 == 0xE0) expect = 3;             // cp 0x0810: not overlong
    else if (b0 == 0xED) expect = 0;             // 0xD810: surrogate
    else if (b0 >= 0xE1 && b0 <= 0xEF) expect = 3;
    else if (b0 >= 0xF0 && b0 <= 0xF3) expect = 4;
    else if (b0 == 0xF4) expect = 0;             // 0x120410 > 0x10FFFF
    CAPTURE(b0);
    CHECK(utf8SequenceLength(s, 4) == expect);
  }
  // Leads outside the 4-byte range must not be decoded as 4-byte sequences.
  CHECK(utf8SequenceLength("\xc1\x80\x80\x80", 4) == 0);
  CHECK(utf8SequenceLength("\xc0\x80\x80\x80", 4) == 0);
  CHECK(utf8SequenceLength("\xfb\x80\x80\x80", 4) == 0);
  CHECK(utf8SequenceLength("\xf9\x80\x80\x80", 4) == 0);
  CHECK(utf8SequenceLength("\x81\x80\x80\x80", 4) == 0);
  CHECK(utf8SequenceLength("\xf1\x80\x80\x80", 4) == 4);
  CHECK(utf8SequenceLength("\xf4\x80\x80\x80", 4) == 4);
  CHECK(utf8SequenceLength("\xef\x80\x80", 3) == 3);
  CHECK(utf8SequenceLength("\xe1\x80\x80", 3) == 3);
}

TEST_CASE("common: utf8SequenceLength avail and continuation bytes") {
  CHECK(utf8SequenceLength("\xc3\xa4", 2) == 2);
  CHECK(utf8SequenceLength("\xc3\xa4", 1) == 0);
  CHECK(utf8SequenceLength("\xe2\x82\xac", 3) == 3);
  CHECK(utf8SequenceLength("\xe2\x82\xac", 2) == 0);
  CHECK(utf8SequenceLength("\xe2\x82\xac", 4) == 3);
  CHECK(utf8SequenceLength("\xf0\x9f\x98\x80", 4) == 4);
  CHECK(utf8SequenceLength("\xf0\x9f\x98\x80", 3) == 0);
  CHECK(utf8SequenceLength("\xf0\x9f\x98\x80", 5) == 4);
  // Continuation must be 10xxxxxx in every position.
  CHECK(utf8SequenceLength("\xc3\xc0", 2) == 0);
  CHECK(utf8SequenceLength("\xc3\x7f", 2) == 0);
  CHECK(utf8SequenceLength("\xc3\xbf", 2) == 2);
  CHECK(utf8SequenceLength("\xc3\x80", 2) == 2);
  CHECK(utf8SequenceLength("\xe2\x82\xc0", 3) == 0);
  CHECK(utf8SequenceLength("\xe2\x02\xac", 3) == 0);
  CHECK(utf8SequenceLength("\xf0\x9f\x98\x40", 4) == 0);
  CHECK(utf8SequenceLength("\xf0\x9f\xd8\x80", 4) == 0);
  // Exact code point values at every limit.
  CHECK(utf8SequenceLength("\xc2\x9f", 2) == 0);          // U+009F
  CHECK(utf8SequenceLength("\xc2\xa0", 2) == 2);          // U+00A0
  CHECK(utf8SequenceLength("\xe0\x9f\xbf", 3) == 0);      // overlong U+07FF
  CHECK(utf8SequenceLength("\xe0\xa0\x80", 3) == 3);      // U+0800
  CHECK(utf8SequenceLength("\xed\x9f\xbf", 3) == 3);      // U+D7FF
  CHECK(utf8SequenceLength("\xed\xa0\x80", 3) == 0);      // U+D800
  CHECK(utf8SequenceLength("\xed\xbf\xbf", 3) == 0);      // U+DFFF
  CHECK(utf8SequenceLength("\xee\x80\x80", 3) == 3);      // U+E000
  CHECK(utf8SequenceLength("\xf0\x8f\xbf\xbf", 4) == 0);  // overlong U+FFFF
  CHECK(utf8SequenceLength("\xf0\x90\x80\x80", 4) == 4);  // U+10000
  CHECK(utf8SequenceLength("\xf4\x8f\xbf\xbf", 4) == 4);  // U+10FFFF
  CHECK(utf8SequenceLength("\xf4\x90\x80\x80", 4) == 0);  // U+110000
  CHECK(utf8SequenceLength("\xdf\xbf", 2) == 2);          // U+07FF
  CHECK(utf8SequenceLength("\xef\xbf\xbf", 3) == 3);      // U+FFFF
  CHECK(utf8SequenceLength(nullptr, 0) == 0);
  CHECK(utf8SequenceLength("\xc3\xa4", 0) == 0);
}

TEST_CASE("common: isPrintableText") {
  CHECK(isPrintableText("", 0));
  CHECK(isPrintableText(nullptr, 0));
  CHECK_FALSE(isPrintableText(nullptr, 1));
  CHECK(isPrintableText(" ", 1));
  CHECK(isPrintableText("~", 1));
  CHECK_FALSE(isPrintableText("\x1f", 1));
  CHECK_FALSE(isPrintableText("\x7f", 1));
  CHECK_FALSE(isPrintableText("\x80", 1));
  CHECK_FALSE(isPrintableText("\x01", 1));
  CHECK_FALSE(isPrintableText("a\x00" "b", 3));
  // A multibyte sequence is skipped as a whole, then ASCII continues.
  CHECK(isPrintableText("\xc3\xa4" "a", 3));
  CHECK_FALSE(isPrintableText("\xc3\xa4\x01", 3));
  CHECK(isPrintableText("\xe2\x82\xac" "b\xc3\xa4", 6));
  CHECK_FALSE(isPrintableText("\xe2\x82\xac\x1f", 4));
  CHECK(isPrintableText("\xf0\x9f\x98\x80z", 5));
  CHECK_FALSE(isPrintableText("\xf0\x9f\x98\x80\x7f", 5));
  // The sequence is bounded by len.
  CHECK_FALSE(isPrintableText("a\xe2\x82\xac", 3));
  CHECK(isPrintableText("a\xe2\x82\xac", 4));
}

TEST_CASE("common: isSafeName") {
  CHECK(isSafeName("abc", 3, false));
  CHECK_FALSE(isSafeName("abcd", 3, false));
  CHECK(isSafeName("abcd", 4, false));
  CHECK(isSafeName("", 0, true));
  CHECK_FALSE(isSafeName("", 0, false));
  CHECK_FALSE(isSafeName("a", 0, true));
  for (const char* bad : {"+", "#", "/", "\"", "\\", "x+", "x#", "x/", "x\"", "x\\", "\x01"}) {
    CAPTURE(bad);
    CHECK_FALSE(isSafeName(bad, 5, false));
    CHECK_FALSE(isSafeName(bad, 5, true));
  }
  for (const char* ok : {"-", ".", "a b", "*", "$", "\xc3\xa4"}) {
    CAPTURE(ok);
    CHECK(isSafeName(ok, 5, false));
  }
}

TEST_CASE("common: buildHostname details") {
  char out[21];
  const char* const cases[][2] = {
      {"a", "a"},           {"z", "z"},          {"A", "A"},         {"Z", "Z"},
      {"0", "0"},           {"9", "9"},          {"-", "VdMot"},     {"_", "_"},
      {"a`b", "a-b"},       {"a{b", "a-b"},      {"a@b", "a-b"},     {"a[b", "a-b"},
      {"a/b", "a-b"},       {"a:b", "a-b"},      {"a-b", "a-b"},     {"a --b", "a--b"},
      {"a- b", "a-b"},      {"a -b", "a-b"},     {"a  b", "a-b"},    {"a_ b", "a_-b"},
      {" a", "a"},          {"a ", "a"},         {"--a", "a"},       {"a--", "a"},
      {"-_-", "_"},         {"---abc", "abc"},   {"_a_", "_a_"},     {"a b c", "a-b-c"},
  };
  for (const auto& c : cases) {
    CAPTURE(c[0]);
    memset(out, 'x', sizeof out);
    CHECK(buildHostname(c[0], out, sizeof out) == strlen(c[1]));
    CHECK(std::string(out) == c[1]);
  }
  // Capacity exactly the default name + NUL.
  char six[6];
  CHECK(buildHostname("", six, sizeof six) == 5);
  CHECK(std::string(six) == "VdMot");
  CHECK(buildHostname("abcdefg", six, sizeof six) == 5);
  CHECK(std::string(six) == "abcde");
  // A separator that would be the last byte is dropped, not left dangling.
  CHECK(buildHostname("abcd e", six, sizeof six) == 4);
  CHECK(std::string(six) == "abcd");
  CHECK(buildHostname("abc de", six, sizeof six) == 5);
  CHECK(std::string(six) == "abc-d");
  CHECK(buildHostname("ab cde", six, sizeof six) == 5);
  CHECK(std::string(six) == "ab-cd");
  // Leading '-' kept from the input are removed by moving exactly the rest
  // (ASan: an exact-size buffer must not be read past its end).
  CHECK(buildHostname("--abc", six, sizeof six) == 3);
  CHECK(std::string(six) == "abc");
  char five[5] = "yyyy";
  CHECK(buildHostname("ab", five, sizeof five) == 0);
  CHECK(five[0] == '\0');
  CHECK(five[1] == 'y');
  // Full-length result is NUL terminated exactly at n.
  char big[21];
  memset(big, 'x', sizeof big);
  CHECK(buildHostname("abcdefghijklmnopqrstuvwxyz", big, sizeof big) == 20);
  CHECK(std::string(big) == "abcdefghijklmnopqrst");
  memset(big, 'x', sizeof big);
  CHECK(buildHostname("--ab", big, sizeof big) == 2);
  CHECK(big[2] == '\0');
  CHECK(std::string(big) == "ab");
}

TEST_CASE("common: isHostName") {
  CHECK(isHostName("a", 5));
  CHECK(isHostName("a.b-c", 5));
  CHECK_FALSE(isHostName("a.b-cd", 5));
  CHECK(isHostName("aZ09z", 5));
  CHECK(isHostName("A-9", 5));
  CHECK_FALSE(isHostName("", 5));
  CHECK_FALSE(isHostName(nullptr, 5));
  CHECK_FALSE(isHostName("-a", 5));
  CHECK_FALSE(isHostName(".a", 5));
  CHECK_FALSE(isHostName("a-", 5));
  CHECK_FALSE(isHostName("a.", 5));
  for (const char* bad : {"_a", "@a", "[a", "`a", "{a", "/a", ":a", " a", "\xc3\xa4" "a"}) {
    CAPTURE(bad);
    CHECK_FALSE(isHostName(bad, 9));
  }
  for (const char* bad : {"a_b", "a b", "a/b", "a@b", "a[b", "a`b", "a{b", "a:b", "a\xc3\xa4"}) {
    CAPTURE(bad);
    CHECK_FALSE(isHostName(bad, 9));
  }
}

TEST_CASE("common: parseUint") {
  uint32_t v = 7;
  CHECK(parseUint("0", 1, 0, v));
  CHECK(v == 0);
  CHECK(parseUint("9", 1, 9, v));
  CHECK(v == 9);
  CHECK_FALSE(parseUint("10", 2, 9, v));
  CHECK(v == 9);
  CHECK(parseUint("4294967295", 10, 0xFFFFFFFFu, v));
  CHECK(v == 0xFFFFFFFFu);
  CHECK_FALSE(parseUint("4294967296", 10, 0xFFFFFFFFu, v));
  CHECK_FALSE(parseUint("9999999999", 10, 0xFFFFFFFFu, v));
  CHECK_FALSE(parseUint("00000000001", 11, 100, v));  // 11 digits
  CHECK(parseUint("0000000001", 10, 100, v));
  CHECK(v == 1);
  CHECK(parseUint("12345", 5, 99999, v));
  CHECK(v == 12345);
  CHECK(parseUint("123", 2, 99999, v));  // only len bytes
  CHECK(v == 12);
  CHECK_FALSE(parseUint("/", 1, 100, v));
  CHECK_FALSE(parseUint(":", 1, 100, v));
  CHECK_FALSE(parseUint("1a", 2, 100, v));
  CHECK_FALSE(parseUint("a1", 2, 100, v));
  CHECK_FALSE(parseUint(nullptr, 1, 100, v));
  CHECK_FALSE(parseUint("1", 0, 100, v));
  CHECK_FALSE(parseUint("-1", 2, 100, v));
}

TEST_CASE("common: parseInt") {
  int32_t v = 0;
  CHECK(parseInt("-5", 2, -5, 5, v));
  CHECK(v == -5);
  CHECK_FALSE(parseInt("-6", 2, -5, 5, v));
  CHECK(parseInt("5", 1, -5, 5, v));
  CHECK(v == 5);
  CHECK_FALSE(parseInt("6", 1, -5, 5, v));
  CHECK(parseInt("0", 1, 0, 0, v));
  CHECK(v == 0);
  CHECK(parseInt("-0", 2, 0, 0, v));
  CHECK(parseInt("-2147483648", 11, INT32_MIN, INT32_MAX, v));
  CHECK(v == INT32_MIN);
  CHECK(parseInt("2147483647", 10, INT32_MIN, INT32_MAX, v));
  CHECK(v == INT32_MAX);
  CHECK_FALSE(parseInt("2147483648", 10, INT32_MIN, INT32_MAX, v));
  CHECK_FALSE(parseInt("-2147483649", 11, INT32_MIN, INT32_MAX, v));
  CHECK(parseInt("-123", 4, -1000, 1000, v));
  CHECK(v == -123);
  CHECK(parseInt("123", 3, -1000, 1000, v));
  CHECK(v == 123);
  CHECK_FALSE(parseInt("", 0, -5, 5, v));
  CHECK_FALSE(parseInt(nullptr, 1, -5, 5, v));
  CHECK_FALSE(parseInt("-", 1, -5, 5, v));
  CHECK_FALSE(parseInt("+1", 2, -5, 5, v));
  CHECK_FALSE(parseInt("1-", 2, -5, 5, v));
  CHECK_FALSE(parseInt("--1", 3, -5, 5, v));
}

TEST_CASE("common: parseIpv4 and formatIpv4") {
  uint32_t ip = 0;
  CHECK(parseIpv4("1.2.3.4", 7, ip));
  CHECK(ip == 0x04030201u);
  CHECK(parseIpv4("0.0.0.0", 7, ip));
  CHECK(ip == 0);
  CHECK(parseIpv4("255.255.255.255", 15, ip));
  CHECK(ip == 0xFFFFFFFFu);
  CHECK(parseIpv4("10.0.0.1x", 8, ip));  // only len bytes
  CHECK(ip == 0x0100000Au);
  ip = 42;
  CHECK_FALSE(parseIpv4("10.0.0.1x", 9, ip));
  CHECK(ip == 42);
  CHECK_FALSE(parseIpv4("1.2.3.4.", 8, ip));
  CHECK_FALSE(parseIpv4("1.2.3.4.5", 9, ip));
  CHECK_FALSE(parseIpv4("1.2.3", 5, ip));
  CHECK_FALSE(parseIpv4("1.2.3.", 6, ip));
  CHECK_FALSE(parseIpv4("1..3.4", 6, ip));
  CHECK_FALSE(parseIpv4(".1.2.3", 6, ip));
  CHECK_FALSE(parseIpv4("0001.2.3.4", 10, ip));  // 4 digits
  CHECK(parseIpv4("001.002.003.004", 15, ip));
  CHECK(ip == 0x04030201u);
  CHECK_FALSE(parseIpv4("256.1.1.1", 9, ip));
  CHECK_FALSE(parseIpv4("1.1.1.256", 9, ip));
  CHECK_FALSE(parseIpv4(nullptr, 7, ip));
  CHECK_FALSE(parseIpv4("", 0, ip));
  CHECK_FALSE(parseIpv4("1.2.3.4", 6, ip));

  char buf[16];
  CHECK(formatIpv4(0x04030201u, buf, sizeof buf) == 7);
  CHECK(std::string(buf) == "1.2.3.4");
  CHECK(formatIpv4(0xFFFFFFFFu, buf, sizeof buf) == 15);
  CHECK(std::string(buf) == "255.255.255.255");
  CHECK(formatIpv4(0xFFFFFFFFu, buf, 16) == 15);
  memset(buf, 'x', sizeof buf);
  CHECK(formatIpv4(0xFFFFFFFFu, buf, 15) == 0);
  CHECK(buf[0] == '\0');
  CHECK(formatIpv4(0x04030201u, buf, 8) == 7);
  memset(buf, 'x', sizeof buf);
  CHECK(formatIpv4(0x04030201u, buf, 7) == 0);
  CHECK(buf[0] == '\0');
  buf[0] = 'x';
  CHECK(formatIpv4(0x04030201u, buf, 0) == 0);
  CHECK(buf[0] == 'x');
}

TEST_CASE("common: OneWireId compare, zero and CRC") {
  OneWireId a;
  OneWireId b;
  CHECK(isZero(a));
  CHECK(a == b);
  CHECK_FALSE(a != b);
  for (int i = 0; i < 8; ++i) {
    b = a;
    b.b[i] = 1;
    CAPTURE(i);
    CHECK_FALSE(isZero(b));
    CHECK_FALSE(a == b);
    CHECK(a != b);
  }
  b = a;
  b.b[7] = 0x80;
  CHECK_FALSE(isZero(b));

  OneWireId id;
  REQUIRE(parseOneWireId("28-84-37-94-97-ff-03-23", 23, id));
  CHECK(crcValid(id) == (refCrc(id) == id.b[7]));
  OneWireId zero;
  CHECK(crcValid(zero));
  // Maxim application note 27 example ROM: 02 1C B8 01 00 00 00, CRC A2.
  OneWireId maxim;
  const uint8_t rom[8] = {0x02, 0x1C, 0xB8, 0x01, 0x00, 0x00, 0x00, 0xA2};
  memcpy(maxim.b, rom, 8);
  CHECK(crcValid(maxim));
  maxim.b[7] = 0xA3;
  CHECK_FALSE(crcValid(maxim));
}

TEST_CASE("common: OneWireId CRC of fixed-seed random ids" * doctest::test_suite("fuzz")) {
  uint32_t seed = 12345;
  int valid = 0;
  for (int round = 0; round < 2000; ++round) {
    OneWireId r;
    for (int i = 0; i < 7; ++i) {
      seed = seed * 1103515245u + 12345u;
      r.b[i] = static_cast<uint8_t>(seed >> 16);
    }
    r.b[7] = refCrc(r);
    CHECK(crcValid(r));
    valid += crcValid(r) ? 1 : 0;
    r.b[7] = static_cast<uint8_t>(r.b[7] ^ (1u << (round % 8)));
    CHECK_FALSE(crcValid(r));
    r.b[7] = static_cast<uint8_t>(r.b[7] ^ (1u << (round % 8)));
    r.b[round % 7] = static_cast<uint8_t>(r.b[round % 7] ^ 0x01);
    CHECK_FALSE(crcValid(r));
  }
  CHECK(valid == 2000);
}

TEST_CASE("common: parseOneWireId and formatOneWireId") {
  OneWireId id;
  REQUIRE(parseOneWireId("00-19-9a-AF-f0-0F-a9-Ff", 23, id));
  const uint8_t expect[8] = {0x00, 0x19, 0x9A, 0xAF, 0xF0, 0x0F, 0xA9, 0xFF};
  CHECK(memcmp(id.b, expect, 8) == 0);
  REQUIRE(parseOneWireId("10-2b-3c-4d-5e-6f-7a-8b", 23, id));
  const uint8_t expect2[8] = {0x10, 0x2B, 0x3C, 0x4D, 0x5E, 0x6F, 0x7A, 0x8B};
  CHECK(memcmp(id.b, expect2, 8) == 0);
  OneWireId keep = id;
  CHECK_FALSE(parseOneWireId("10-2b-3c-4d-5e-6f-7a-8b", 22, id));
  CHECK_FALSE(parseOneWireId("10-2b-3c-4d-5e-6f-7a-8b-", 24, id));
  CHECK_FALSE(parseOneWireId(nullptr, 23, id));
  for (const char* bad : {"g0-2b-3c-4d-5e-6f-7a-8b", "1g-2b-3c-4d-5e-6f-7a-8b", "10-2b-3c-4d-5e-6f-7a-8G",
                          "10:2b-3c-4d-5e-6f-7a-8b", "10-2b-3c-4d-5e-6f-7a:8b", "10-2b-3c-4d-5e-6f-7a-/b",
                          "10-2b-3c-4d-5e-6f-7a-:b", "10-2b-3c-4d-5e-6f-7a-@b", "10-2b-3c-4d-5e-6f-7a-`b",
                          "10-2b-3c-4d-5e-6f-7a-8g", "10-2b-3c-4d-5e-6f-7a-8G", " 0-2b-3c-4d-5e-6f-7a-8b"}) {
    CAPTURE(bad);
    CHECK_FALSE(parseOneWireId(bad, 23, id));
  }
  CHECK(id == keep);  // unchanged on failure
  // Last byte may be followed by anything outside len.
  CHECK(parseOneWireId("10-2b-3c-4d-5e-6f-7a-8bX", 23, id));
  // Every hex digit value in both nibble positions.
  const char* hex = "0123456789abcdefABCDEF";
  for (int i = 0; i < 22; ++i) {
    char t[24];
    snprintf(t, sizeof t, "%c%c-00-00-00-00-00-00-%c%c", hex[i], hex[21 - i], hex[21 - i], hex[i]);
    CAPTURE(t);
    REQUIRE(parseOneWireId(t, 23, id));
    auto val = [](char c) { return c <= '9' ? c - '0' : (c | 0x20) - 'a' + 10; };
    CHECK(id.b[0] == val(hex[i]) * 16 + val(hex[21 - i]));
    CHECK(id.b[7] == val(hex[21 - i]) * 16 + val(hex[i]));
  }

  char out[24];
  memset(out, 'x', sizeof out);
  CHECK(formatOneWireId(id, out, 0) == 0);
  CHECK(out[0] == 'x');
  CHECK(formatOneWireId(id, out, 23) == 0);
  CHECK(out[0] == '\0');
  REQUIRE(parseOneWireId("00-19-9a-af-f0-0f-a9-ff", 23, id));
  CHECK(formatOneWireId(id, out, 24) == 23);
  CHECK(std::string(out) == "00-19-9a-af-f0-0f-a9-ff");
}

namespace {

std::string haId(const char* in) {
  char out[64];
  memset(out, 'x', sizeof out);
  const size_t n = buildHaId(in, strlen(in), out, sizeof out);
  CHECK(n == strlen(out));
  return out;
}

}  // namespace

TEST_CASE("common: buildHaId maps names to HA ids") {
  CHECK(haId("VdMot") == "VdMot");
  CHECK(haId("Dom 1") == "Dom_1");
  CHECK(haId("\xC5\x81" "azienka") == "Lazienka");      // Łazienka
  CHECK(haId("K\xC3\xBC" "che") == "Kuche");            // Küche
  CHECK(haId("Stra\xC3\x9F" "e") == "Strasse");         // Straße
  CHECK(haId("\xC3\x86" "ble") == "AEble");             // Æble
  CHECK(haId("\xC5\xBC\xC3\xB3\xC5\x82w") == "zolw");   // żółw
  CHECK(haId("\xC4\x8C" "e\xC5\xA1ky") == "Cesky");     // Česky
  CHECK(haId("Bad.1") == "Bad_1");
  CHECK(haId("a-b_c") == "a-b_c");
  // Every other sequence is one '_': symbols, other scripts, 3 and 4 bytes.
  CHECK(haId("\xE2\x82\xAC") == "_");      // €
  CHECK(haId("\xC3\x97") == "_");          // ×
  CHECK(haId("\xC3\xB7") == "_");          // ÷
  CHECK(haId("\xC2\xBF") == "_");          // ¿ U+00BF, below the table
  CHECK(haId("\xC6\x80") == "_");          // ƀ U+0180, above the table
  CHECK(haId("\xCE\xA9") == "_");          // Ω
  CHECK(haId("\xE4\xB8\xAD") == "_");      // 中 (its first two bytes would read as U+0138)
  CHECK(haId("\xF0\x9F\x98\x80" "a") == "_a");
  // Invalid UTF-8: one '_' per byte.
  CHECK(haId("\x80") == "_");
  CHECK(haId("\xC5" "a") == "_a");         // lead byte without continuation
  CHECK(haId("a\xC5") == "a_");            // cut at the end
  CHECK(haId("\xC0\xB0") == "__");         // overlong
  CHECK(haId("\xC2\x80") == "__");         // C1 control
  CHECK(haId("\xED\xA0\x80") == "___");    // surrogate
  CHECK(haId("\xFF\xFE") == "__");
  // Every ASCII byte: letters, digits, '_' and '-' are kept, the rest is '_'.
  for (int c = 1; c < 0x80; ++c) {
    const char in[2] = {static_cast<char>(c), '\0'};
    const bool keep = (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
                      (c >= '0' && c <= '9') || c == '_' || c == '-';
    CAPTURE(c);
    CHECK(haId(in) == (keep ? std::string(in) : std::string("_")));
  }
}

TEST_CASE("common: buildHaId maps U+00C0..U+017F to base letters") {
  const char* const kExpect[192] = {
      "A", "A", "A", "A", "A", "A", "AE", "C", "E", "E", "E", "E", "I", "I", "I", "I",  // C0
      "D", "N", "O", "O", "O", "O", "O", "_", "O", "U", "U", "U", "U", "Y", "TH", "ss",  // D0
      "a", "a", "a", "a", "a", "a", "ae", "c", "e", "e", "e", "e", "i", "i", "i", "i",  // E0
      "d", "n", "o", "o", "o", "o", "o", "_", "o", "u", "u", "u", "u", "y", "th", "y",  // F0
      "A", "a", "A", "a", "A", "a", "C", "c", "C", "c", "C", "c", "C", "c", "D", "d",   // 100
      "D", "d", "E", "e", "E", "e", "E", "e", "E", "e", "E", "e", "G", "g", "G", "g",   // 110
      "G", "g", "G", "g", "H", "h", "H", "h", "I", "i", "I", "i", "I", "i", "I", "i",   // 120
      "I", "i", "IJ", "ij", "J", "j", "K", "k", "k", "L", "l", "L", "l", "L", "l", "L", // 130
      "l", "L", "l", "N", "n", "N", "n", "N", "n", "n", "N", "n", "O", "o", "O", "o",   // 140
      "O", "o", "OE", "oe", "R", "r", "R", "r", "R", "r", "S", "s", "S", "s", "S", "s", // 150
      "S", "s", "T", "t", "T", "t", "T", "t", "U", "u", "U", "u", "U", "u", "U", "u",   // 160
      "U", "u", "U", "u", "W", "w", "Y", "y", "Y", "Z", "z", "Z", "z", "Z", "z", "s",   // 170
  };
  for (uint32_t cp = 0xC0; cp <= 0x17F; ++cp) {
    const char in[3] = {static_cast<char>(0xC0 | (cp >> 6)), static_cast<char>(0x80 | (cp & 0x3F)),
                        '\0'};
    CAPTURE(cp);
    CHECK(haId(in) == kExpect[cp - 0xC0]);
  }
  // Never longer than the input: every 2-byte sequence gives 1 or 2 chars.
  for (uint32_t cp = 0x80; cp < 0x800; ++cp) {
    const char in[2] = {static_cast<char>(0xC0 | (cp >> 6)), static_cast<char>(0x80 | (cp & 0x3F))};
    char out[3];
    CAPTURE(cp);
    const size_t n = buildHaId(in, sizeof in, out, sizeof out);
    CHECK(n >= 1);
    CHECK(n <= 2);
  }
}

TEST_CASE("common: buildHaId capacity and input bounds") {
  char out[8];
  memset(out, 'x', sizeof out);
  CHECK(buildHaId("VdMot", 5, out, 6) == 5);  // capacity len + 1
  CHECK(std::string(out) == "VdMot");
  memset(out, 'x', sizeof out);
  CHECK(buildHaId("VdMot", 5, out, 5) == 0);
  CHECK(out[0] == '\0');
  // A two-letter form fits as a whole or not at all.
  memset(out, 'x', sizeof out);
  CHECK(buildHaId("a\xC3\x9F", 3, out, 4) == 3);
  CHECK(std::string(out) == "ass");
  memset(out, 'x', sizeof out);
  CHECK(buildHaId("a\xC3\x9F", 3, out, 3) == 0);
  CHECK(out[0] == '\0');
  // Empty, null, no room.
  out[0] = 'x';
  CHECK(buildHaId("", 0, out, sizeof out) == 0);
  CHECK(out[0] == '\0');
  out[0] = 'x';
  CHECK(buildHaId(nullptr, 5, out, sizeof out) == 0);
  CHECK(out[0] == '\0');
  out[0] = 'x';
  CHECK(buildHaId("ab", 2, out, 0) == 0);
  CHECK(out[0] == 'x');
  CHECK(buildHaId("ab", 2, nullptr, sizeof out) == 0);
  // At most len bytes; a NUL ends the input earlier.
  CHECK(buildHaId("abc", 2, out, sizeof out) == 2);
  CHECK(std::string(out) == "ab");
  CHECK(buildHaId("a\0b", 3, out, sizeof out) == 1);
  CHECK(std::string(out) == "a");
  // A sequence cut by len is an invalid byte; nothing past len is read
  // (exact-size buffer for ASan).
  CHECK(buildHaId("\xC3\xBC", 1, out, sizeof out) == 1);
  CHECK(std::string(out) == "_");
  const char cut[2] = {'a', '\xC5'};
  CHECK(buildHaId(cut, sizeof cut, out, sizeof out) == 2);
  CHECK(std::string(out) == "a_");
}

TEST_CASE("common: roundTargetPercent") {
  struct Row {
    double v;
    bool ok;
    uint8_t out;
  };
  const double inf = std::numeric_limits<double>::infinity();
  const Row rows[] = {
      {43.7, true, 44},     {43.5, true, 44},    {43.49999, true, 43}, {0.49, true, 0},
      {0.5, true, 1},       {99.5, true, 100},   {99.49, true, 99},    {100.0, true, 100},
      {0.0, true, 0},       {-0.0, true, 0},     {100.4, false, 0},    {100.0000001, false, 0},
      {-0.4, false, 0},     {-1e-300, false, 0}, {inf, false, 0},      {-inf, false, 0},
      {std::numeric_limits<double>::quiet_NaN(), false, 0},
  };
  for (const Row& r : rows) {
    CAPTURE(r.v);
    uint8_t out = 77;
    CHECK(roundTargetPercent(r.v, out) == r.ok);
    CHECK(out == (r.ok ? r.out : 77));
  }
}
