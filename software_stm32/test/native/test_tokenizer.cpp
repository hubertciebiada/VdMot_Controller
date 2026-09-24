#include <string.h>

#include <string>

#include "doctest.h"
#include "vdm/tokenizer.h"

using vdm::Tokenizer;

TEST_CASE("Tokenizer: command with trailing space (protocol v1 style)") {
  char line[] = "stgtp 3 50 ";
  Tokenizer t;
  REQUIRE(t.parse(line, 5));
  CHECK(std::string(t.command()) == "stgtp");
  CHECK(t.is("stgtp"));
  CHECK(t.argc() == 2);
  CHECK(std::string(t.arg(0)) == "3");
  CHECK(std::string(t.arg(1)) == "50");
  CHECK_FALSE(t.tooManyArgs());
}

TEST_CASE("Tokenizer: last token without trailing separator") {
  char line[] = "gvlst";
  Tokenizer t;
  REQUIRE(t.parse(line));
  CHECK(t.is("gvlst"));
  CHECK(t.argc() == 0);

  char line2[] = "gvlvd 11";
  REQUIRE(t.parse(line2));
  CHECK(t.argc() == 1);
  CHECK(std::string(t.arg(0)) == "11");
}

TEST_CASE("Tokenizer: separator runs, tabs and leading blanks") {
  char line[] = " \t stvls\t\t1   a  b \t";
  Tokenizer t;
  REQUIRE(t.parse(line));
  CHECK(t.is("stvls"));
  REQUIRE(t.argc() == 3);
  CHECK(std::string(t.arg(0)) == "1");
  CHECK(std::string(t.arg(1)) == "a");
  CHECK(std::string(t.arg(2)) == "b");
}

TEST_CASE("Tokenizer: empty and blank lines") {
  Tokenizer t;
  char empty[] = "";
  CHECK_FALSE(t.parse(empty));
  CHECK(std::string(t.command()).empty());
  CHECK(t.argc() == 0);
  char blank[] = " \t  ";
  CHECK_FALSE(t.parse(blank));
  CHECK(std::string(t.command()).empty());
  CHECK_FALSE(t.parse(nullptr));
  CHECK_FALSE(t.tooManyArgs());
  CHECK_FALSE(t.is(""));
}

TEST_CASE("Tokenizer: argument limit") {
  Tokenizer t;
  char exact[] = "smotc 1 2 3 4 5 ";
  REQUIRE(t.parse(exact, 5));
  CHECK(t.argc() == 5);
  CHECK(std::string(t.arg(4)) == "5");

  char over[] = "smotc 1 2 3 4 5 6";
  CHECK_FALSE(t.parse(over, 5));
  CHECK(t.tooManyArgs());
  CHECK(t.is("smotc"));
  CHECK(t.argc() == 5);
  CHECK(std::string(t.arg(4)) == "5");

  char none[] = "gvers x";
  CHECK_FALSE(t.parse(none, 0));
  CHECK(t.tooManyArgs());
  CHECK(t.argc() == 0);
  char noneOk[] = "gvers ";
  CHECK(t.parse(noneOk, 0));
  CHECK_FALSE(t.tooManyArgs());
}

TEST_CASE("Tokenizer: maxArgs is capped at kMaxArgs") {
  Tokenizer t;
  char line[] = "c 1 2 3 4 5 6 7 8 9";
  CHECK_FALSE(t.parse(line, 200));
  CHECK(t.tooManyArgs());
  CHECK(t.argc() == Tokenizer::kMaxArgs);
  CHECK(std::string(t.arg(Tokenizer::kMaxArgs - 1)) == "8");

  char fits[] = "c 1 2 3 4 5 6 7 8";
  CHECK(t.parse(fits, 255));
  CHECK(t.argc() == Tokenizer::kMaxArgs);
}

TEST_CASE("Tokenizer: missing arguments read as empty strings") {
  Tokenizer t;
  char line[] = "gvlvd 1";
  REQUIRE(t.parse(line));
  CHECK(std::string(t.arg(1)).empty());
  CHECK(std::string(t.arg(255)).empty());
}

TEST_CASE("Tokenizer: exact command match") {
  Tokenizer t;
  char line[] = "stgtpX 1 2";
  REQUIRE(t.parse(line));
  CHECK_FALSE(t.is("stgtp"));
  CHECK(t.is("stgtpX"));
  char shortCmd[] = "stg";
  REQUIRE(t.parse(shortCmd));
  CHECK_FALSE(t.is("stgtp"));
  CHECK_FALSE(t.is(nullptr));
  CHECK_FALSE(t.is("STG"));
}

TEST_CASE("Tokenizer: state is reset between lines") {
  Tokenizer t;
  char a[] = "a 1 2 3";
  CHECK_FALSE(t.parse(a, 2));
  CHECK(t.tooManyArgs());
  char b[] = "b";
  REQUIRE(t.parse(b, 2));
  CHECK(t.is("b"));
  CHECK(t.argc() == 0);
  CHECK_FALSE(t.tooManyArgs());
}

TEST_CASE("Tokenizer: long tokens are kept whole and terminated") {
  std::string longArg(200, 'z');
  std::string text = "cmd " + longArg + " end";
  char line[256];
  memcpy(line, text.c_str(), text.size() + 1);
  Tokenizer t;
  REQUIRE(t.parse(line));
  CHECK(std::string(t.arg(0)) == longArg);
  CHECK(std::string(t.arg(1)) == "end");
}

TEST_CASE("Tokenizer: checked integer arguments") {
  Tokenizer t;
  char line[] = "stgtp 11 100 -5 x 65536 300";
  REQUIRE(t.parse(line));
  uint32_t u = 7;
  CHECK(t.argU32(0, 0, 11, u));
  CHECK(u == 11);
  CHECK_FALSE(t.argU32(0, 0, 10, u));
  CHECK(u == 11);
  CHECK(t.argU32(1, 0, 100, u));
  CHECK(u == 100);
  CHECK_FALSE(t.argU32(2, 0, 100, u));
  CHECK_FALSE(t.argU32(3, 0, 100, u));
  CHECK_FALSE(t.argU32(6, 0, 100, u));  // missing

  int32_t i = 0;
  CHECK(t.argI32(2, -10, 10, i));
  CHECK(i == -5);
  CHECK_FALSE(t.argI32(2, 0, 10, i));
  CHECK_FALSE(t.argI32(9, -10, 10, i));
  CHECK(i == -5);

  uint16_t w = 1;
  CHECK_FALSE(t.argU16(4, 0, 65535, w));
  CHECK(w == 1);
  CHECK(t.argU16(1, 0, 65535, w));
  CHECK(w == 100);
  CHECK_FALSE(t.argU16(3, 0, 65535, w));

  uint8_t b = 9;
  CHECK_FALSE(t.argU8(5, 0, 255, b));
  CHECK(b == 9);
  CHECK(t.argU8(0, 0, 255, b));
  CHECK(b == 11);
  CHECK_FALSE(t.argU8(1, 0, 99, b));
  CHECK(b == 11);
}

TEST_CASE("Tokenizer: default state before any parse") {
  const Tokenizer t;
  CHECK(t.argc() == 0);
  CHECK_FALSE(t.tooManyArgs());
  CHECK(std::string(t.command()).empty());
  CHECK(std::string(t.arg(0)).empty());
}

TEST_CASE("Tokenizer: arguments of a previous line are not visible") {
  char first[] = "c 1 2 -3";
  char second[] = "c 1";
  Tokenizer t;
  REQUIRE(t.parse(first, 5));
  REQUIRE(t.argc() == 3);
  REQUIRE(t.parse(second, 5));
  REQUIRE(t.argc() == 1);
  CHECK(std::string(t.arg(1)).empty());
  uint32_t u = 99;
  CHECK_FALSE(t.argU32(1, 0, 10, u));
  CHECK(u == 99);
  int32_t s = 99;
  CHECK_FALSE(t.argI32(1, -10, 10, s));
  CHECK(s == 99);
}
