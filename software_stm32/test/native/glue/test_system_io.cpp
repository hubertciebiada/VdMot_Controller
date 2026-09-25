// End-to-end tests of the IO side of the STM glue (glue_system): the EEPROM is written only for a
// changed value (S13), the stored blocks load without flags at the next start (W11).
#include <sstream>
#include <vector>

#include "glue_test.h"
#include "hardware.h"
#include "otasupport.h"
#include "valve_sim.h"

void setup();
void setup_system();
void loop_system();

namespace {

void bootController(sim::Rig& rig) {
  glue::begin();
  setup();
  while (bootstate == 0) BootLoop();
  setup_system();
  rig.install();
}

void runMain(uint32_t ms) {
  for (uint32_t i = 0; i < ms; i++) {
    loop_system();
    fake::advanceMs(1);
  }
}

std::string exchange(const std::string& line) {
  fake::takeTx(Serial1);
  fake::inject(Serial1, line);
  runMain(50);
  return fake::takeTx(Serial1);
}

// value n (1-based) of the gstax reply
unsigned long gstaxField(unsigned n) {
  std::istringstream in(exchange("gstax\n"));
  std::string word;
  in >> word;
  REQUIRE(word == "gstax");
  unsigned long v = 0;
  for (unsigned i = 0; i < n; i++) in >> v;
  return v;
}

}  // namespace

TEST_CASE("system: repeated configuration requests write the EEPROM only for a new value") {
  sim::Rig rig;
  bootController(rig);
  runMain(5000);  // the first start's configuration write
  CHECK(exchange("eepst\n") == "eepst 1 \r\n");
  CHECK(exchange("stlnm 2000\n") == "stlnm\r\n");
  runMain(5000);
  const unsigned long writes = gstaxField(20);
  for (int i = 0; i < 10; i++) CHECK(exchange("stlnm 2000\n") == "stlnm\r\n");
  runMain(5000);
  CHECK(gstaxField(20) == writes);
  CHECK(exchange("eepst\n") == "eepst 1 \r\n");
  CHECK(exchange("stlnt 3600\n") == "stlnt\r\n");
  CHECK(exchange("stlnt 3600\n") == "stlnt\r\n");
  CHECK(exchange("eepst\n") == "eepst 0 \r\n");
  runMain(5000);
  CHECK(gstaxField(20) == writes + 1);
  CHECK(exchange("gtlnt\n") == "gtlnt 3600\r\n");
  CHECK(exchange("sfspo 255 40\n") == "sfspo 255 ok\r\n");
  CHECK(exchange("slcfg 60\n") == "slcfg ok\r\n");
  runMain(5000);
  CHECK(gstaxField(20) == writes + 2);
  CHECK(gstaxField(18) == 64);  // cfgFlags of the start-up load: unverified (a new chip)
}

TEST_CASE("system: after the first start and changes of every block the next start loads without flags") {
  sim::Rig rig;
  bootController(rig);
  if (testkit::boot() == 0) {
    runMain(5000);
    exchange("stlnt 3600\n");
    exchange("slcfg 90\n");
    exchange("sfspo 3 20\n");
    runMain(5000);
    fake::inject(Serial1, "reset\n");
    glue::run([] {
      runMain(5000);
      FAIL("the controller did not reset");
    });
  }
  CHECK(testkit::boot() == 1);
  CHECK(gstaxField(18) == 0);
  CHECK(gstaxField(19) == 0);
  CHECK(exchange("eepst\n") == "eepst 1 \r\n");
}
