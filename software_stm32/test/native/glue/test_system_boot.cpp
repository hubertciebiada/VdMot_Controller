// End-to-end smoke tests of the whole STM glue with the valve sim (glue_system): the controller boots
// through the window, answers today's v1 requests byte-exact, and a soft reset reboots it.
#include "glue_test.h"
#include "hardware.h"
#include "otasupport.h"
#include "valve_sim.h"

void setup();
void setup_system();
void loop_system();

namespace {

// from reset to the main loop: setup(), the boot window without an update, setup_system(); the sim
// is installed once setup_system() attached valve_loop to its timer
void bootController(sim::Rig& rig) {
  glue::begin();
  setup();
  while (bootstate == 0) BootLoop();
  setup_system();
  rig.install();
}

// the main loop, one loop_system() per millisecond
void runMain(uint32_t ms) {
  for (uint32_t i = 0; i < ms; i++) {
    loop_system();
    fake::advanceMs(1);
  }
}

// a request of the ESP and what the controller sent back within 50 ms
std::string exchange(const std::string& line) {
  fake::takeTx(Serial1);
  fake::inject(Serial1, line);
  runMain(50);
  return fake::takeTx(Serial1);
}

}  // namespace

TEST_CASE("system: a new controller answers gvers, gproto, gtgtp, gvlvd, gstat and stgtp like 2.0.0") {
  sim::Rig rig;
  bootController(rig);
  CHECK(millis() == 3511);
  CHECK(exchange("gvers\n") == "gvers 2.0.0-revamped_C2 1 \r\n");
  CHECK(exchange("gproto\n") == "gproto 3\r\n");
  // erased EEPROM: startOnPower loads its default 30
  CHECK(exchange("gtgtp 0\n") == "gtgtp 0 30 \r\n");
  // valve 0 is still under its presence test: status 5 (unknown)
  CHECK(exchange("gvlvd 0\n") == "gvlvd 0 30 20 5 -500 -500 0 0 0 0 0 \r\n");
  // the first start stores the configuration with its CRC: eepState 1 until the write 3 s later
  CHECK(exchange("gstat\n") == "gstat 3 0 1 0 0 1\r\n");
  CHECK(exchange("stgtp 0 50\n") == "stgtp\r\n");
  CHECK(exchange("gtgtp 0\n") == "gtgtp 0 50 \r\n");
}

TEST_CASE("system: the presence test finds every connected valve, an open one is reported") {
  sim::Rig rig;
  bootController(rig);
  rig.valve[5].connected = false;
  runMain(40000);
  const std::string reply = exchange("gvlst\n");
  CHECK(reply == "gvlst 12 8,8,8,8,8,6,8,8,8,8,8,8 \r\n");
  CHECK(rig.conflicts == 0);
}

TEST_CASE("system: reset answers, waits for the EEPROM and restarts the controller (software reset)") {
  sim::Rig rig;
  bootController(rig);
  if (testkit::boot() == 0) {
    fake::inject(Serial1, "reset\n");
    glue::run([] {
      while (fake::tx(Serial1).empty()) runMain(1);
      CHECK(fake::takeTx(Serial1) == "reset \r\n");
      runMain(4000);  // the first start's configuration write comes first
      FAIL("the controller did not reset");
    });
  }
  CHECK(testkit::boot() == 1);
  CHECK(exchange("gstat\n") == "gstat 3 1 3 0 0 0\r\n");
}
