#include <string.h>

#include <string>
#include <vector>

#include "doctest.h"
#include "vdm/line_assembler.h"

using vdm::LineAssembler;
using vdm::StaticLineAssembler;

namespace {

// Feeds the whole string, collecting every completed line.
std::vector<std::string> feedAll(LineAssembler& la, const std::string& input) {
  std::vector<std::string> lines;
  size_t pos = 0;
  while (pos < input.size()) {
    pos += la.feed(input.data() + pos, input.size() - pos);
    if (la.hasLine()) {
      lines.emplace_back(la.line());
      la.release();
    }
  }
  return lines;
}

}  // namespace

TEST_CASE("LineAssembler: LF, CR and CRLF terminate lines") {
  StaticLineAssembler<32> la;
  CHECK(feedAll(la, "gvlst \n") == std::vector<std::string>{"gvlst "});
  CHECK(feedAll(la, "gvers \r") == std::vector<std::string>{"gvers "});
  CHECK(feedAll(la, "stgtp 1 50 \r\n") == std::vector<std::string>{"stgtp 1 50 "});
  CHECK(feedAll(la, "a\n\rb\r\n\r\nc\n") == std::vector<std::string>{"a", "b", "c"});
  CHECK(la.overflowCount() == 0);
  CHECK(la.malformedCount() == 0);
}

TEST_CASE("LineAssembler: empty lines and bare terminators are ignored") {
  StaticLineAssembler<8> la;
  CHECK(feedAll(la, "\n\r\r\n\n").empty());
  CHECK_FALSE(la.hasLine());
  CHECK(la.length() == 0);
  CHECK(std::string(la.line()).empty());
}

TEST_CASE("LineAssembler: a line is not delivered before its terminator") {
  StaticLineAssembler<16> la;
  CHECK(la.feed("gver", 4) == 4);
  CHECK_FALSE(la.hasLine());
  CHECK(std::string(la.line()) == "gver");
  CHECK(la.length() == 4);
  CHECK(la.takeLine() == nullptr);
  CHECK(la.feed("s\n", 2) == 2);
  REQUIRE(la.hasLine());
  CHECK(std::string(la.line()) == "gvers");
  REQUIRE(la.takeLine() != nullptr);
  CHECK(std::string(la.takeLine()) == "gvers");
}

TEST_CASE("LineAssembler: bytes after the terminator stay with the caller") {
  StaticLineAssembler<32> la;
  const char input[] = "gvlst \ngvers \nrest";
  const size_t len = strlen(input);
  size_t used = la.feed(input, len);
  CHECK(used == 7);
  REQUIRE(la.hasLine());
  CHECK(std::string(la.line()) == "gvlst ");

  // While a line is pending nothing more is consumed.
  CHECK(la.feed(input + used, len - used) == 0);
  CHECK_FALSE(la.push('x'));
  CHECK(std::string(la.line()) == "gvlst ");

  la.release();
  used += la.feed(input + used, len - used);
  CHECK(used == 14);
  REQUIRE(la.hasLine());
  CHECK(std::string(la.line()) == "gvers ");
  la.release();
  used += la.feed(input + used, len - used);
  CHECK(used == len);
  CHECK_FALSE(la.hasLine());
  CHECK(std::string(la.line()) == "rest");
}

TEST_CASE("LineAssembler: push reports consumption") {
  StaticLineAssembler<8> la;
  CHECK(la.push('a'));
  CHECK(la.push('\n'));
  CHECK(la.hasLine());
  CHECK_FALSE(la.push('b'));
  la.release();
  CHECK(la.push('b'));
  CHECK(std::string(la.line()) == "b");
}

TEST_CASE("LineAssembler: capacity boundary") {
  StaticLineAssembler<6> la;  // at most 5 characters
  CHECK(feedAll(la, "12345\n") == std::vector<std::string>{"12345"});
  CHECK(la.overflowCount() == 0);

  CHECK(feedAll(la, "123456\n").empty());
  CHECK(la.overflowCount() == 1);

  // The oversize line is dropped up to its terminator, the next is intact.
  CHECK(feedAll(la, "1234567890abc\nok\n") == std::vector<std::string>{"ok"});
  CHECK(la.overflowCount() == 2);

  // CRLF after an oversize line does not create an extra line.
  CHECK(feedAll(la, "abcdefgh\r\nxy\r\n") == std::vector<std::string>{"xy"});
  CHECK(la.overflowCount() == 3);
}

TEST_CASE("LineAssembler: an endless line is counted once and keeps memory bounded") {
  StaticLineAssembler<4> la;
  std::string noise(10000, 'x');
  CHECK(feedAll(la, noise).empty());
  CHECK(la.overflowCount() == 1);
  CHECK(la.length() == 0);
  CHECK(feedAll(la, "\nab\n") == std::vector<std::string>{"ab"});
  CHECK(la.overflowCount() == 1);
}

TEST_CASE("LineAssembler: lines with control or non-ASCII bytes are dropped") {
  StaticLineAssembler<32> la;
  CHECK(feedAll(la, std::string("gv\0ers\n", 7)).empty());
  CHECK(la.malformedCount() == 1);
  CHECK(feedAll(la, "\x7fgvers\n").empty());
  CHECK(feedAll(la, "gvers\x80\n").empty());
  CHECK(feedAll(la, "\x1b[A\n").empty());
  CHECK(la.malformedCount() == 4);
  CHECK(la.overflowCount() == 0);
  // TAB and every printable character are accepted.
  CHECK(feedAll(la, "a\tb ~\n") == std::vector<std::string>{"a\tb ~"});
  CHECK(feedAll(la, " !/09:@AZ[`az{~\n") == std::vector<std::string>{" !/09:@AZ[`az{~"});
  CHECK(la.malformedCount() == 4);
}

TEST_CASE("LineAssembler: malformed then oversize in one line counts once") {
  StaticLineAssembler<4> la;
  CHECK(feedAll(la, "\x01xxxxxxxx\n").empty());
  CHECK(la.malformedCount() == 1);
  CHECK(la.overflowCount() == 0);
  CHECK(feedAll(la, "xxxx\x01\n").empty());
  CHECK(la.malformedCount() == 1);
  CHECK(la.overflowCount() == 1);
}

TEST_CASE("LineAssembler: reset drops partial line and discard state") {
  StaticLineAssembler<4> la;
  CHECK(la.feed("abcdef", 6) == 6);  // now discarding
  la.reset();
  CHECK(feedAll(la, "ok\n") == std::vector<std::string>{"ok"});
  CHECK(la.feed("par", 3) == 3);
  la.reset();
  CHECK(la.length() == 0);
  CHECK(feedAll(la, "z\n") == std::vector<std::string>{"z"});
  CHECK(la.feed("x\n", 2) == 2);
  la.reset();
  CHECK_FALSE(la.hasLine());
}

TEST_CASE("LineAssembler: degenerate storage drops every line as overflow") {
  char one[1];
  LineAssembler tiny(one, sizeof(one));
  CHECK(feedAll(tiny, "a\n\nb\n").empty());
  CHECK(tiny.overflowCount() == 2);
  CHECK(std::string(tiny.line()).empty());

  LineAssembler none(nullptr, 100);
  CHECK(feedAll(none, "abc\n").empty());
  CHECK(none.overflowCount() == 1);
  CHECK(std::string(none.line()).empty());
  CHECK(none.feed(nullptr, 5) == 0);

  char two[2];
  LineAssembler smallest(two, sizeof(two));
  CHECK(feedAll(smallest, "a\nbc\nd\n") == std::vector<std::string>{"a", "d"});
  CHECK(smallest.overflowCount() == 1);
}

TEST_CASE("LineAssembler: every dropped line is counted") {
  StaticLineAssembler<2> la;
  for (int i = 0; i < 1000; ++i) {
    la.feed("xx\n", 3);
    la.feed("\x01\n", 2);
  }
  CHECK(la.overflowCount() == 1000);
  CHECK(la.malformedCount() == 1000);
}
