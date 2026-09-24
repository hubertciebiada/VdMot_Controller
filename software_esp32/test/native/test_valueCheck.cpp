#include "doctest.h"
#include "valueCheck.h"

#include <climits>
#include <cmath>
#include <limits>

TEST_CASE("parseLong accepts integers as before")
{
  long v = -1;
  CHECK(parseLong("50", &v));
  CHECK(v == 50);
  CHECK(parseLong(" 7 ", &v));
  CHECK(v == 7);
  CHECK(parseLong("-3", &v));
  CHECK(v == -3);
  CHECK(parseLong("+4", &v));
  CHECK(v == 4);
  CHECK(parseLong("0", &v));
  CHECK(v == 0);
}

TEST_CASE("parseLong truncates decimal strings like ArduinoJson's as<int>() did")
{
  // the web UI posts number input values as strings
  long v = -1;
  CHECK(parseLong("50.5", &v));
  CHECK(v == 50);
  CHECK(parseLong("50.0", &v));
  CHECK(v == 50);
  CHECK(parseLong("5e1", &v));
  CHECK(v == 50);
  CHECK(parseLong("99.99", &v));
  CHECK(v == 99);
  CHECK(parseLong(".5", &v));
  CHECK(v == 0);
  CHECK(parseLong("-0.5", &v));
  CHECK(v == 0);
  CHECK(parseLong("100.9 ", &v));
  CHECK(v == 100);
}

TEST_CASE("parseLong rejects junk and leaves the value untouched")
{
  const char* bad[] = {"", " ", "abc", "50abc", "50 1", "5.5.5", "0x10", "inf", "-inf", "nan",
                       "1e999", "e5", ".", "-", "+", "5e", "1,5", "\t50x"};
  for (const char* s : bad) {
    long v = 1234;
    CAPTURE(s);
    CHECK_FALSE(parseLong(s, &v));
    CHECK(v == 1234);
  }
  long v = 1234;
  CHECK_FALSE(parseLong(nullptr, &v));
  CHECK(v == 1234);
}

TEST_CASE("parseLong rejects values out of range for long")
{
  long v = 1234;
  CHECK_FALSE(parseLong("99999999999999999999999", &v));
  CHECK_FALSE(parseLong("-99999999999999999999999", &v));
  CHECK_FALSE(parseLong("1e30", &v));
  CHECK(v == 1234);
}

TEST_CASE("parseDouble")
{
  double d = 0;
  CHECK(parseDouble("21.5", &d));
  CHECK(d == doctest::Approx(21.5));
  CHECK(parseDouble(" -1.25 ", &d));
  CHECK(d == doctest::Approx(-1.25));
  CHECK(parseDouble("1e2", &d));
  CHECK(d == doctest::Approx(100));
  d = 7;
  CHECK_FALSE(parseDouble("nan", &d));
  CHECK_FALSE(parseDouble("INF", &d));
  CHECK_FALSE(parseDouble("1e400", &d));
  CHECK_FALSE(parseDouble("0x1p3", &d));
  CHECK_FALSE(parseDouble("", &d));
  CHECK_FALSE(parseDouble(nullptr, &d));
  CHECK(d == 7);
}

TEST_CASE("doubleToLong")
{
  long v = 1234;
  CHECK(doubleToLong(50.9, &v));
  CHECK(v == 50);
  CHECK(doubleToLong(-50.9, &v));
  CHECK(v == -50);
  v = 1234;
  CHECK_FALSE(doubleToLong(std::numeric_limits<double>::quiet_NaN(), &v));
  CHECK_FALSE(doubleToLong(std::numeric_limits<double>::infinity(), &v));
  CHECK_FALSE(doubleToLong(-std::numeric_limits<double>::infinity(), &v));
  CHECK_FALSE(doubleToLong(1e30, &v));
  CHECK_FALSE(doubleToLong(-1e30, &v));
  CHECK(v == 1234);
}

TEST_CASE("tempOffsetToTenths rounds and clamps to +/- 10.0 degrees")
{
  int t = 1234;
  CHECK(tempOffsetToTenths(0.0, &t));
  CHECK(t == 0);
  CHECK(tempOffsetToTenths(1.5, &t));
  CHECK(t == 15);
  CHECK(tempOffsetToTenths(-0.3, &t));
  CHECK(t == -3);
  CHECK(tempOffsetToTenths(0.26, &t));
  CHECK(t == 3);
  CHECK(tempOffsetToTenths(10.0, &t));
  CHECK(t == 100);
  CHECK(tempOffsetToTenths(-60.0, &t));
  CHECK(t == -TEMP_OFFSET_MAX);
  CHECK(tempOffsetToTenths(3300.0, &t));
  CHECK(t == TEMP_OFFSET_MAX);
  CHECK(tempOffsetToTenths(1e300, &t));
  CHECK(t == TEMP_OFFSET_MAX);
  t = 1234;
  CHECK_FALSE(tempOffsetToTenths(std::numeric_limits<double>::quiet_NaN(), &t));
  CHECK_FALSE(tempOffsetToTenths(std::numeric_limits<double>::infinity(), &t));
  CHECK(t == 1234);
}

TEST_CASE("addTempOffset never turns a reading into a sentinel and never wraps")
{
  CHECK(addTempOffset(215, 15) == 230);
  CHECK(addTempOffset(215, -15) == 200);
  // an offset stored by an older firmware is clamped: -60.0 degrees -> -10.0
  CHECK(addTempOffset(215, -600) == 115);
  CHECK(addTempOffset(215, -800) == 115);
  CHECK(addTempOffset(215, 33000) == 315);
  CHECK(addTempOffset(215, INT_MIN) == 115);
  CHECK(addTempOffset(215, INT_MAX) == 315);
  // valid readings near the sentinel range stay valid
  CHECK(addTempOffset(-450, -100) == TEMP_SENTINEL_MAX + 1);
  CHECK(addTempOffset(-499, -1) == TEMP_SENTINEL_MAX + 1);
  // no int16 overflow
  CHECK(addTempOffset(INT16_MAX, 100) == INT16_MAX);
  CHECK(addTempOffset(INT16_MAX - 50, 100) == INT16_MAX);
  // STM sentinels pass unchanged
  CHECK(addTempOffset(-500, 100) == -500);
  CHECK(addTempOffset(-1270, 50) == -1270);
  CHECK(addTempOffset(INT16_MIN, 100) == INT16_MIN);
}
