// Smoke tests of src/otasupport.cpp (glue_otasupport): the boot window after reset in which the ESP
// may start an STM update (DEADBEEF -> BEEFIT -> ROM bootloader).
#include "glue_test.h"
#include "hardware.h"
#include "otasupport.h"

using fake::Ev;

namespace {

// BootLoop() calls until the window ends or n calls
unsigned loopCalls(unsigned n) {
  unsigned calls = 0;
  while (calls < n && bootstate == 0) {
    BootLoop();
    calls++;
  }
  return calls;
}

}  // namespace

TEST_CASE("BootSetup: LED on, USART1 at 115200 8E1 on PA10/PA9, bytes of the first 10 ms dropped") {
  glue::begin();
  fake::board.onMs = [] { fake::inject(Serial1, "x"); };  // the ESP talks during the start
  BootSetup();
  CHECK(fake::board.mode[LED] == OUTPUT);
  CHECK(fake::board.out[LED] == LOW);
  CHECK(Serial1.rxPin == PA10);
  CHECK(Serial1.txPin == PA9);
  CHECK(Serial1.baud == 115200);
  CHECK(Serial1.config == SERIAL_8E1);
  CHECK(fake::eventsOf(Ev::Delay) == std::vector<fake::Event>{{0, Ev::Delay, 0, 10, 0}});
  CHECK(Serial1.readCalls == 10);
  CHECK(Serial1.available() == 0);
}

TEST_CASE("BootLoop: without DEADBEEF the window ends at call 3001, call 3002 sets bootstate") {
  glue::begin();
  BootSetup();
  const size_t delaysBefore = fake::eventsOf(Ev::Delay).size();
  CHECK(loopCalls(3001) == 3001);
  CHECK(bootstate == 0);
  // one delay(1) per call of the window
  CHECK(fake::eventsOf(Ev::Delay).size() - delaysBefore == 3001);
  BootLoop();
  CHECK(bootstate == 1);
  CHECK(fake::eventsOf(Ev::Delay).size() - delaysBefore == 3001);
  CHECK(fake::takeTx(Serial1).empty());
}

TEST_CASE("BootLoop: the LED toggles on every 102nd call of the window") {
  glue::begin();
  BootSetup();
  const size_t writesBefore = fake::eventsOf(Ev::Write, LED).size();
  loopCalls(101);
  CHECK(fake::eventsOf(Ev::Write, LED).size() == writesBefore);
  loopCalls(1);
  CHECK(fake::eventsOf(Ev::Write, LED).size() == writesBefore + 1);
  CHECK(fake::board.out[LED] == HIGH);
  loopCalls(101);
  CHECK(fake::eventsOf(Ev::Write, LED).size() == writesBefore + 1);
  loopCalls(1);
  CHECK(fake::board.out[LED] == LOW);
  CHECK(fake::eventsOf(Ev::Write, LED).size() == writesBefore + 2);
}

TEST_CASE("BootLoop: DEADBEEF answers BEEFIT and jumps into the bootloader") {
  glue::begin();
  BootSetup();
  loopCalls(5);
  fake::inject(Serial1, "DEADBEEF");
  BootLoop();
  CHECK(fake::board.out[LED] == LOW);
  CHECK(Serial1.available() == 0);
  const size_t delaysBefore = fake::eventsOf(Ev::Delay).size();
  CHECK_THROWS_AS(BootLoop(), fake::BootloaderJump);
  CHECK(fake::takeTx(Serial1) == "BEEFIT\r\n");
  CHECK(Serial1.flushes == 1);
  const std::vector<fake::Event> delays = fake::eventsOf(Ev::Delay);
  REQUIRE(delays.size() == delaysBefore + 2);
  CHECK(delays[delaysBefore].value == 10);
  CHECK(delays[delaysBefore + 1].value == 200);
  // LED off before the jump (HIGH = off on the blackpill)
  CHECK(fake::board.out[LED] == HIGH);
  CHECK(stub::calls == stub::Calls{"JumpToBootloader()"});
  CHECK(bootstate == 0);
}

TEST_CASE("BootLoop: fewer than 8 bytes wait, 8 other bytes are read and ignored") {
  glue::begin();
  BootSetup();
  fake::inject(Serial1, "DEADBEE");
  BootLoop();
  CHECK(Serial1.available() == 7);
  fake::inject(Serial1, "X");
  BootLoop();
  CHECK(Serial1.available() == 0);
  CHECK(loopCalls(2999) == 2999);
  CHECK(bootstate == 0);
  BootLoop();
  CHECK(bootstate == 1);
  CHECK(stub::calls.empty());
}

TEST_CASE("BootLoop: DEADBEEF in the last call of the window still starts the update") {
  glue::begin();
  BootSetup();
  loopCalls(3000);
  fake::inject(Serial1, "DEADBEEF");
  BootLoop();
  CHECK_THROWS_AS(BootLoop(), fake::BootloaderJump);
  CHECK(bootstate == 0);
}
