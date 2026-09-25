// LineAssembler: CR/LF framing, bounds, overflow/malformed accounting.
#include <stdint.h>
#include <string.h>

#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/line_assembler.h"

using namespace vdm;

namespace {

// Feeds everything, collecting completed lines (the way the glue does it).
std::vector<std::string> feedAll(LineAssembler& la, const char* data, size_t len) {
  std::vector<std::string> lines;
  size_t off = 0;
  while (off < len) {
    off += la.feed(data + off, len - off);
    if (la.hasLine()) {
      lines.emplace_back(la.line(), la.length());
      la.release();
    }
  }
  return lines;
}

std::vector<std::string> feedAll(LineAssembler& la, const std::string& s) {
  return feedAll(la, s.data(), s.size());
}

}  // namespace

TEST_CASE("line assembler: initial state") {
  StaticLineAssembler<8> la;
  CHECK_FALSE(la.hasLine());
  CHECK(la.length() == 0);
  CHECK(std::string(la.line()) == "");
  CHECK(la.overflowCount() == 0);
  CHECK(la.malformedCount() == 0);
}

TEST_CASE("line assembler: CR, LF and CRLF each end exactly one line") {
  StaticLineAssembler<64> la;
  CHECK(feedAll(la, "abc\r") == std::vector<std::string>{"abc"});
  CHECK(feedAll(la, "def\n") == std::vector<std::string>{"def"});
  CHECK(feedAll(la, "ghi\r\n") == std::vector<std::string>{"ghi"});
  CHECK(feedAll(la, "a\r\nb\rc\nd\r\n") == std::vector<std::string>{"a", "b", "c", "d"});
  // LF CR is two terminators around nothing: still one line.
  CHECK(feedAll(la, "e\n\r") == std::vector<std::string>{"e"});
  CHECK(la.overflowCount() == 0);
  CHECK(la.malformedCount() == 0);
}

TEST_CASE("line assembler: CRLF split across feed calls counts once") {
  StaticLineAssembler<64> la;
  CHECK(feedAll(la, "gvlvd 1\r") == std::vector<std::string>{"gvlvd 1"});
  CHECK(feedAll(la, "\ngvlvd 2\r") == std::vector<std::string>{"gvlvd 2"});
  CHECK(feedAll(la, "\n").empty());
  CHECK_FALSE(la.hasLine());
}

TEST_CASE("line assembler: empty lines are ignored") {
  StaticLineAssembler<16> la;
  CHECK(feedAll(la, "\r\n\r\n\n\n\r\r").empty());
  CHECK(feedAll(la, "\r\nx\r\n\r\n") == std::vector<std::string>{"x"});
}

TEST_CASE("line assembler: push semantics and hold of the pending line") {
  StaticLineAssembler<16> la;
  CHECK(la.push('a'));
  CHECK(la.length() == 1);
  CHECK(std::string(la.line()) == "a");  // partial line visible
  CHECK_FALSE(la.hasLine());
  CHECK(la.push('\r'));
  CHECK(la.hasLine());
  // Pending line: nothing more is consumed, the line stays intact.
  CHECK_FALSE(la.push('b'));
  CHECK_FALSE(la.push('\n'));
  CHECK(std::string(la.line()) == "a");
  CHECK(la.length() == 1);
  la.release();
  CHECK_FALSE(la.hasLine());
  CHECK(la.length() == 0);
  CHECK(std::string(la.line()) == "");
  CHECK(la.push('b'));
  CHECK(la.push('\n'));
  CHECK(std::string(la.line()) == "b");
}

TEST_CASE("line assembler: feed stops after a complete line and keeps the rest") {
  StaticLineAssembler<32> la;
  const char data[] = "one\r\ntwo\r\n";
  size_t used = la.feed(data, strlen(data));
  CHECK(used == 4);  // "one\r"
  CHECK(la.hasLine());
  CHECK(std::string(la.line()) == "one");
  CHECK(la.feed(data + used, strlen(data) - used) == 0);  // held
  la.release();
  size_t used2 = la.feed(data + used, strlen(data) - used);
  CHECK(used2 == 5);  // "\ntwo\r"
  CHECK(std::string(la.line()) == "two");
  la.release();
  CHECK(la.feed(data + used + used2, 1) == 1);  // final "\n" swallowed
  CHECK_FALSE(la.hasLine());
}

TEST_CASE("line assembler: feed edge cases") {
  StaticLineAssembler<8> la;
  CHECK(la.feed(nullptr, 5) == 0);
  CHECK(la.feed("abc", 0) == 0);
  CHECK(la.length() == 0);
  CHECK(la.feed("abc", 3) == 3);
  CHECK_FALSE(la.hasLine());
  CHECK(la.length() == 3);
}

TEST_CASE("line assembler: capacity boundary") {
  StaticLineAssembler<5> la;  // 4 chars max
  CHECK(feedAll(la, "abcd\r") == std::vector<std::string>{"abcd"});
  CHECK(la.overflowCount() == 0);
  CHECK(feedAll(la, "abcde\r").empty());
  CHECK(la.overflowCount() == 1);
  // Discarding continues up to the terminator, the next line is clean.
  CHECK(feedAll(la, "abcdefghij\r\nxy\r\n") == std::vector<std::string>{"xy"});
  CHECK(la.overflowCount() == 2);
  CHECK(la.malformedCount() == 0);
}

TEST_CASE("line assembler: overflow counted once per line, even without terminator") {
  StaticLineAssembler<4> la;
  std::string longLine(1000, 'x');
  CHECK(feedAll(la, longLine).empty());
  CHECK(la.overflowCount() == 1);
  CHECK(la.length() == 0);
  CHECK(std::string(la.line()) == "");
  CHECK(feedAll(la, "\rok\r") == std::vector<std::string>{"ok"});
  CHECK(la.overflowCount() == 1);
}

TEST_CASE("line assembler: kStmMaxLineLen line fits, one more does not") {
  StaticLineAssembler<kStmMaxLineLen + 1> la;
  std::string exact(kStmMaxLineLen, 'a');
  CHECK(feedAll(la, exact + "\r\n") == std::vector<std::string>{exact});
  CHECK(la.overflowCount() == 0);
  CHECK(feedAll(la, exact + "b\r\n").empty());
  CHECK(la.overflowCount() == 1);
}

TEST_CASE("line assembler: non-printable bytes drop the line as malformed") {
  StaticLineAssembler<32> la;
  CHECK(feedAll(la, std::string("ab\x01" "cd\r\nok\r\n")) == std::vector<std::string>{"ok"});
  CHECK(la.malformedCount() == 1);
  CHECK(feedAll(la, std::string("a\x7f\r")).empty());
  CHECK(la.malformedCount() == 2);
  CHECK(feedAll(la, std::string("\x80\xff\xfe\r")).empty());
  CHECK(la.malformedCount() == 3);  // once per line
  CHECK(feedAll(la, std::string("\x1f\r")).empty());
  CHECK(la.malformedCount() == 4);
  std::string nul("a\0b\r", 4);
  CHECK(feedAll(la, nul).empty());
  CHECK(la.malformedCount() == 5);
  CHECK(la.overflowCount() == 0);
}

TEST_CASE("line assembler: printable range and TAB are accepted") {
  StaticLineAssembler<128> la;
  std::string all;
  for (int c = 0x20; c <= 0x7E; ++c) all.push_back(static_cast<char>(c));
  all.push_back('\t');
  CHECK(feedAll(la, all + "\n") == std::vector<std::string>{all});
  CHECK(la.malformedCount() == 0);
}

TEST_CASE("line assembler: malformed then overflow in the same line counts once") {
  StaticLineAssembler<4> la;
  CHECK(feedAll(la, std::string("\x01xxxxxxxx\r")).empty());
  CHECK(la.malformedCount() == 1);
  CHECK(la.overflowCount() == 0);
  CHECK(feedAll(la, std::string("xxxxx\x01\r")).empty());
  CHECK(la.malformedCount() == 1);
  CHECK(la.overflowCount() == 1);
}

TEST_CASE("line assembler: reset drops partial line, pending line and discard state") {
  StaticLineAssembler<4> la;
  CHECK(la.feed("abcdef", 6) == 6);  // overflowing -> discarding
  CHECK(la.overflowCount() == 1);
  la.reset();
  CHECK(feedAll(la, "xy\r") == std::vector<std::string>{"xy"});  // not discarded

  CHECK(la.feed("ab", 2) == 2);
  la.reset();
  CHECK(la.length() == 0);
  CHECK(feedAll(la, "c\r") == std::vector<std::string>{"c"});

  CHECK(la.feed("d\r", 2) == 2);
  CHECK(la.hasLine());
  la.reset();
  CHECK_FALSE(la.hasLine());
  CHECK(la.length() == 0);
  CHECK(std::string(la.line()) == "");
  // Counters survive reset.
  CHECK(la.overflowCount() == 1);
}

TEST_CASE("line assembler: degenerate capacities") {
  SUBCASE("null storage") {
    LineAssembler la(nullptr, 100);
    CHECK(std::string(la.line()) == "");
    CHECK(feedAll(la, "abc\r\n").empty());
    CHECK(la.overflowCount() == 1);
    CHECK(la.length() == 0);
    la.release();
    la.reset();
    CHECK(std::string(la.line()) == "");
  }
  SUBCASE("capacity 0") {
    char buf[1] = {'z'};
    LineAssembler la(buf, 0);
    CHECK(feedAll(la, "a\r").empty());
    CHECK(la.overflowCount() == 1);
    CHECK(buf[0] == 'z');  // never written
    CHECK(std::string(la.line()) == "");
  }
  SUBCASE("capacity 1") {
    char buf[1] = {'z'};
    LineAssembler la(buf, 1);
    CHECK(buf[0] == '\0');
    CHECK(feedAll(la, "a\rb\r").empty());
    CHECK(la.overflowCount() == 2);
    CHECK(feedAll(la, "\r\n").empty());
    CHECK(la.overflowCount() == 2);  // empty lines are not overflows
  }
  SUBCASE("capacity 2") {
    char buf[2];
    LineAssembler la(buf, 2);
    CHECK(feedAll(la, "a\r") == std::vector<std::string>{"a"});
    CHECK(feedAll(la, "ab\r").empty());
    CHECK(la.overflowCount() == 1);
  }
}

TEST_CASE("line assembler: buffer is always NUL-terminated and never overrun") {
  char buf[10];
  memset(buf, 'Q', sizeof buf);
  char guard = 'G';
  LineAssembler la(buf, 8);
  CHECK(la.feed("12345678901234", 14) == 14);
  CHECK(buf[8] == 'Q');
  CHECK(buf[9] == 'Q');
  CHECK(guard == 'G');
  la.reset();
  CHECK(la.feed("1234567\r", 8) == 8);
  CHECK(strlen(la.line()) == 7);
  CHECK(buf[7] == '\0');
}

TEST_CASE("line assembler: fixed-seed fuzz keeps invariants" * doctest::test_suite("fuzz")) {
  uint32_t seed = 0x1234567u;
  auto rnd = [&seed]() {
    seed = seed * 1664525u + 1013904223u;
    return seed >> 8;
  };
  StaticLineAssembler<17> la;
  size_t lines = 0;
  for (int iter = 0; iter < 20000; ++iter) {
    char chunk[64];
    const size_t n = rnd() % sizeof chunk;
    for (size_t i = 0; i < n; ++i) {
      const uint32_t r = rnd() % 100;
      // Mostly printable, some terminators, a few junk bytes.
      chunk[i] = r < 80 ? static_cast<char>(0x20 + rnd() % 95)
                 : r < 95 ? (r & 1 ? '\r' : '\n')
                          : static_cast<char>(rnd() & 0xFF);
    }
    size_t off = 0;
    while (off < n) {
      const size_t used = la.feed(chunk + off, n - off);
      off += used;
      REQUIRE(la.length() <= 16);
      REQUIRE(strlen(la.line()) == la.length());
      if (la.hasLine()) {
        REQUIRE(la.length() > 0);
        for (size_t i = 0; i < la.length(); ++i) {
          const unsigned char c = static_cast<unsigned char>(la.line()[i]);
          REQUIRE((c == '\t' || (c >= 0x20 && c < 0x7F)));
        }
        ++lines;
        la.release();
      } else {
        REQUIRE(off == n);
      }
    }
  }
  CHECK(lines > 1000);
  CHECK(la.overflowCount() > 0);
  CHECK(la.malformedCount() > 0);
}
