// Edge cases of src/main.cpp (glue_main): the exact LED ticks over two cycles, a released button.
#include "glue_test.h"
#include "hardware.h"
#include "stub_sysstat.h"

void loop_system();

using fake::Ev;

namespace {

void loopAt(uint32_t ms) {
  fake::board.nowUs = static_cast<uint64_t>(ms) * 1000;
  loop_system();
}

}  // namespace

TEST_CASE("loop_system: the LED goes off exactly at the 30th tick and on at the 31st, every 31 ticks") {
  glue::begin();
  uint32_t ms = 0;
  for (int tick = 1; tick <= 62; tick++) {
    ms += 101;
    loopAt(ms);
    CAPTURE(tick);
    const std::vector<fake::Event> w = fake::eventsOf(Ev::Write, LED);
    if (tick < 30) {
      CHECK(w.empty());
    } else if (tick == 30) {
      CHECK(w == std::vector<fake::Event>{{0, Ev::Write, LED, LOW, 0}});
    } else if (tick < 61) {
      CHECK(w.size() == 2);
    } else if (tick == 61) {
      CHECK(w.size() == 3);
      CHECK(w.back().value == LOW);
    } else {
      CHECK(w.size() == 4);
      CHECK(w.back().value == HIGH);
    }
  }
}

TEST_CASE("loop_system: a released button is never reported") {
  glue::begin();
  fake::board.in[BUTTON] = 0;
  loopAt(101);
  loopAt(202);
  CHECK(fake::takeTx(Serial6).empty());
}
