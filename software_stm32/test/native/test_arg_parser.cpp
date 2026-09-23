#include <stdint.h>

#include "doctest.h"
#include "vdm/arg_parser.h"

using vdm::isZeroAddress;
using vdm::parseI32;
using vdm::parseOneWireAddress;
using vdm::parseU32;

TEST_CASE("parseU32: valid values and range boundaries") {
  uint32_t v = 99;
  CHECK(parseU32("0", 0, 10, v));
  CHECK(v == 0);
  CHECK(parseU32("10", 0, 10, v));
  CHECK(v == 10);
  CHECK(parseU32("007", 0, 10, v));
  CHECK(v == 7);
  CHECK(parseU32("5", 5, 5, v));
  CHECK(v == 5);
  CHECK(parseU32("4294967295", 0, UINT32_MAX, v));
  CHECK(v == UINT32_MAX);
  CHECK(parseU32("65535", 0, 65535, v));
  CHECK(v == 65535);
}

TEST_CASE("parseU32: out of range leaves the output untouched") {
  uint32_t v = 42;
  CHECK_FALSE(parseU32("11", 0, 10, v));
  CHECK_FALSE(parseU32("4", 5, 10, v));
  CHECK_FALSE(parseU32("65536", 0, 65535, v));
  CHECK_FALSE(parseU32("4294967296", 0, UINT32_MAX, v));
  CHECK_FALSE(parseU32("99999999999999999999", 0, UINT32_MAX, v));
  CHECK_FALSE(parseU32("1", 10, 0, v));  // inverted range
  CHECK(v == 42);
}

TEST_CASE("parseU32: overflow check per digit against small limits") {
  uint32_t v = 0;
  // limit 5: "7" must fail even though 7 > limit - digit would underflow.
  CHECK_FALSE(parseU32("7", 0, 5, v));
  CHECK(parseU32("5", 0, 5, v));
  CHECK_FALSE(parseU32("6", 0, 5, v));
  CHECK_FALSE(parseU32("10", 0, 9, v));
  CHECK(parseU32("255", 0, 255, v));
  CHECK_FALSE(parseU32("256", 0, 255, v));
  CHECK_FALSE(parseU32("260", 0, 255, v));
  CHECK_FALSE(parseU32("1000", 0, 255, v));
  CHECK(parseU32("0000000000000255", 0, 255, v));
  CHECK(v == 255);
}

TEST_CASE("parseU32: malformed input") {
  uint32_t v = 3;
  CHECK_FALSE(parseU32("", 0, 10, v));
  CHECK_FALSE(parseU32(nullptr, 0, 10, v));
  CHECK_FALSE(parseU32("-1", 0, UINT32_MAX, v));
  CHECK_FALSE(parseU32("+1", 0, 10, v));
  CHECK_FALSE(parseU32(" 1", 0, 10, v));
  CHECK_FALSE(parseU32("1 ", 0, 10, v));
  CHECK_FALSE(parseU32("1a", 0, 100, v));
  CHECK_FALSE(parseU32("a1", 0, 100, v));
  CHECK_FALSE(parseU32("0x10", 0, 100, v));
  CHECK_FALSE(parseU32("1.5", 0, 100, v));
  CHECK_FALSE(parseU32("/", 0, 100, v));
  CHECK_FALSE(parseU32(":", 0, 100, v));
  CHECK(v == 3);
}

TEST_CASE("parseI32: valid values and boundaries") {
  int32_t v = 1;
  CHECK(parseI32("0", -5, 5, v));
  CHECK(v == 0);
  CHECK(parseI32("-0", -5, 5, v));
  CHECK(v == 0);
  CHECK(parseI32("+5", -5, 5, v));
  CHECK(v == 5);
  CHECK(parseI32("-5", -5, 5, v));
  CHECK(v == -5);
  CHECK(parseI32("2147483647", INT32_MIN, INT32_MAX, v));
  CHECK(v == INT32_MAX);
  CHECK(parseI32("-2147483648", INT32_MIN, INT32_MAX, v));
  CHECK(v == INT32_MIN);
  CHECK(parseI32("-2147483647", INT32_MIN, INT32_MAX, v));
  CHECK(v == -2147483647);
}

TEST_CASE("parseI32: out of range and malformed") {
  int32_t v = 17;
  CHECK_FALSE(parseI32("6", -5, 5, v));
  CHECK_FALSE(parseI32("-6", -5, 5, v));
  CHECK_FALSE(parseI32("2147483648", INT32_MIN, INT32_MAX, v));
  CHECK_FALSE(parseI32("-2147483649", INT32_MIN, INT32_MAX, v));
  CHECK_FALSE(parseI32("", -5, 5, v));
  CHECK_FALSE(parseI32("-", -5, 5, v));
  CHECK_FALSE(parseI32("+", -5, 5, v));
  CHECK_FALSE(parseI32("--1", -5, 5, v));
  CHECK_FALSE(parseI32("+-1", -5, 5, v));
  CHECK_FALSE(parseI32("1-", -5, 5, v));
  CHECK_FALSE(parseI32(" 1", -5, 5, v));
  CHECK_FALSE(parseI32(nullptr, -5, 5, v));
  CHECK_FALSE(parseI32("1", 5, -5, v));  // inverted range
  CHECK(v == 17);
}

TEST_CASE("parseOneWireAddress: valid addresses in both cases") {
  uint8_t a[8] = {};
  REQUIRE(parseOneWireAddress("28-84-37-94-97-ff-03-23", a));
  const uint8_t expected[8] = {0x28, 0x84, 0x37, 0x94, 0x97, 0xFF, 0x03, 0x23};
  for (int i = 0; i < 8; ++i) CHECK(a[i] == expected[i]);

  REQUIRE(parseOneWireAddress("0A-bC-De-F0-9a-00-10-Ff", a));
  const uint8_t mixed[8] = {0x0A, 0xBC, 0xDE, 0xF0, 0x9A, 0x00, 0x10, 0xFF};
  for (int i = 0; i < 8; ++i) CHECK(a[i] == mixed[i]);

  REQUIRE(parseOneWireAddress("00-00-00-00-00-00-00-00", a));
  CHECK(isZeroAddress(a));
}

TEST_CASE("parseOneWireAddress: malformed input leaves the output untouched") {
  uint8_t a[8] = {1, 2, 3, 4, 5, 6, 7, 8};
  const char* bad[] = {
      "",
      "28",
      "28-84-37-94-97-ff-03",      // 7 bytes
      "28-84-37-94-97-ff-03-2",    // short last byte
      "28-84-37-94-97-ff-03-23-",  // trailing separator
      "28-84-37-94-97-ff-03-234",  // too long
      "28:84:37:94:97:ff:03:23",   // wrong separator
      "28-84-37-94-97-ff-03 23",
      "2g-84-37-94-97-ff-03-23",  // non-hex
      "g2-84-37-94-97-ff-03-23",
      "28-84-37-94-97-ff-03-2G",
      "28--4-37-94-97-ff-03-23",
      "-28-84-37-94-97-ff-03-2",
      " 28-84-37-94-97-ff-03-2",
      "28-84-37-94-97-ff-0323",
  };
  for (const char* s : bad) {
    CAPTURE(s);
    CHECK_FALSE(parseOneWireAddress(s, a));
  }
  CHECK_FALSE(parseOneWireAddress(nullptr, a));
  for (int i = 0; i < 8; ++i) CHECK(a[i] == i + 1);
}

TEST_CASE("parseOneWireAddress: hex digit boundaries") {
  uint8_t a[8] = {};
  REQUIRE(parseOneWireAddress("09-0a-0f-90-a0-f0-AF-FA", a));
  const uint8_t expected[8] = {0x09, 0x0A, 0x0F, 0x90, 0xA0, 0xF0, 0xAF, 0xFA};
  for (int i = 0; i < 8; ++i) CHECK(a[i] == expected[i]);
  // Characters adjacent to the valid ranges.
  const char* bad[] = {"/0-00-00-00-00-00-00-00", ":0-00-00-00-00-00-00-00",
                       "`0-00-00-00-00-00-00-00", "g0-00-00-00-00-00-00-00",
                       "@0-00-00-00-00-00-00-00", "G0-00-00-00-00-00-00-00",
                       "0/-00-00-00-00-00-00-00", "0:-00-00-00-00-00-00-00",
                       "0`-00-00-00-00-00-00-00", "0g-00-00-00-00-00-00-00",
                       "0@-00-00-00-00-00-00-00", "0G-00-00-00-00-00-00-00"};
  for (const char* s : bad) {
    CAPTURE(s);
    CHECK_FALSE(parseOneWireAddress(s, a));
  }
}

TEST_CASE("isZeroAddress") {
  uint8_t zero[8] = {};
  CHECK(isZeroAddress(zero));
  for (int i = 0; i < 8; ++i) {
    uint8_t a[8] = {};
    a[i] = 1;
    CHECK_FALSE(isZeroAddress(a));
  }
}
