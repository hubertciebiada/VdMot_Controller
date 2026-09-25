// Smoke tests of src/main.cpp (glue_main): start-up order, setup_system(), the three branches of
// loop_system() and the watchdog feed, loop() through the boot window.
#include "glue_test.h"
#include "hardware.h"
#include "stub_app.h"
#include "stub_motor.h"
#include "stub_otasupport.h"
#include "stub_sysstat.h"

void setup();
void loop();
void setup_system();
void loop_system();
extern STM32Timer ITimer1;

using fake::Ev;

namespace {

// loop_system() at fake time ms
void loopAt(uint32_t ms) {
  fake::board.nowUs = static_cast<uint64_t>(ms) * 1000;
  loop_system();
}

// the stub calls of the branches, without the sysstat_loop() of every run
stub::Calls branchCalls() {
  stub::Calls out;
  for (const std::string& c : stub::calls) {
    if (c != "sysstat_loop()") out.push_back(c);
  }
  return out;
}

const stub::Calls k10msBranch = {"app_loop()", "app_warm_save()", "communication_loop()", "temperature_loop()",
                                 "terminal_supervise()"};

}  // namespace

TEST_CASE("setup: reset cause first, then the valve outputs safe, then the boot window") {
  glue::begin();
  setup();
  CHECK(stub::calls == stub::Calls{"sysstat_capture_reset()", "valve_pins_safe()", "BootSetup()"});
}

TEST_CASE("setup_system: watchdog, I2C, pins, modules and the valve timer in this order") {
  glue::begin();
  setup_system();
  CHECK(stub::calls == stub::Calls{"sysstat_boot_reason()", "i2c_bus_recover()", "Terminal_Init()",
                                   "sysstat_safe_mode()", "communication_setup()", "eepromsetup()", "eeprom_read_layout(&eep_content)",
                                   "temperature_setup()", "app_setup()", "valve_setup()", "app_restore()"});
  CHECK(IWatchdog.timeoutUs == 8000000);
  CHECK(IWatchdog.reloads == 3);
  const std::vector<fake::Event> begin = fake::eventsOf(Ev::WatchdogBegin);
  const std::vector<fake::Event> wire = fake::eventsOf(Ev::WireBegin);
  REQUIRE(begin.size() == 1);
  REQUIRE(wire.size() == 1);
  CHECK(begin[0].seq < wire[0].seq);
  CHECK(wire[0].pin == I2C_SDA_PIN);
  CHECK(wire[0].value == I2C_SCL_PIN);
  // the valve PSU stays off: open drain, latch high
  CHECK(fake::eventsOf(Ev::Mode, POWER_ENA) == std::vector<fake::Event>{{0, Ev::Mode, POWER_ENA, OUTPUT_OPEN_DRAIN, 0}});
  CHECK(fake::eventsOf(Ev::Write, POWER_ENA) == std::vector<fake::Event>{{0, Ev::Write, POWER_ENA, HIGH, 0}});
  CHECK(fake::board.mode[LED] == OUTPUT);
  CHECK(fake::board.mode[BUTTON] == INPUT_PULLUP);
  CHECK(fake::board.mode[ANINCURRENT] == INPUT_ANALOG);
  CHECK(fake::board.mode[ANINREFHALF] == INPUT_ANALOG);
  CHECK(fake::board.analogBits == 12);
  CHECK(fake::eventsOf(Ev::Delay) == std::vector<fake::Event>{{0, Ev::Delay, 0, 500, 0}});
  CHECK(ITimer1.intervalUs == 10000);
  CHECK(ITimer1.callback == valve_loop);
  CHECK(fake::takeTx(Serial6).empty());
}

TEST_CASE("setup_system: a watchdog reset is reported on the terminal") {
  glue::begin();
  stub::sysstat.reason = vdm::BootReason::IndependentWatchdog;
  setup_system();
  CHECK(fake::takeTx(Serial6) == "reset by watchdog\r\n");
}

TEST_CASE("setup_system: safe mode is reported on the terminal") {
  glue::begin();
  stub::sysstat.safeMode = true;
  setup_system();
  CHECK(fake::takeTx(Serial6) == "safe mode\r\n");
}

TEST_CASE("loop_system: the 10 ms, 100 ms and 1 s branches run when more than their period has passed") {
  glue::begin();
  loopAt(0);
  loopAt(10);
  CHECK(branchCalls().empty());
  loopAt(11);
  CHECK(branchCalls() == k10msBranch);
  stub::calls.clear();
  loopAt(21);
  CHECK(branchCalls().empty());
  loopAt(22);
  CHECK(branchCalls() == k10msBranch);
  stub::calls.clear();
  loopAt(100);
  CHECK(branchCalls() == k10msBranch);
  stub::calls.clear();
  loopAt(101);
  CHECK(branchCalls() == stub::Calls{"Terminal_Serve()"});
  stub::calls.clear();
  loopAt(1000);
  stub::Calls both = {"Terminal_Serve()"};
  both.insert(both.end(), k10msBranch.begin(), k10msBranch.end());
  CHECK(branchCalls() == both);
  stub::calls.clear();
  stub::sysstat.uptime = 1;
  loopAt(1001);
  CHECK(branchCalls() == stub::Calls{"sysstat_uptime_s()", "app_1s_tick(1)", "eepromloop()"});
}

TEST_CASE("loop_system: app_10s_loop gets the real seconds at every 10th second branch") {
  glue::begin();
  uint32_t ms = 0;
  for (uint32_t s = 1; s <= 20; s++) {
    ms += 1001;
    stub::sysstat.uptime = s >= 10 ? s + 2 : s;  // the 10th branch ran late
    stub::calls.clear();
    loopAt(ms);
    CAPTURE(s);
    CHECK(stub::callsOf("app_10s_loop") ==
          (s == 10 ? stub::Calls{"app_10s_loop(12)"} : s == 20 ? stub::Calls{"app_10s_loop(10)"} : stub::Calls{}));
    CHECK(stub::callsOf("app_1s_tick") == stub::Calls{s == 10 ? "app_1s_tick(3)" : "app_1s_tick(1)"});
  }
}

TEST_CASE("loop_system: the LED goes off at the 30th and on at the 31st 100 ms tick") {
  glue::begin();
  uint32_t ms = 0;
  for (int tick = 1; tick <= 31; tick++) {
    ms += 101;
    loopAt(ms);
    if (tick < 30) CHECK(fake::eventsOf(Ev::Write, LED).empty());
  }
  CHECK(fake::eventsOf(Ev::Write, LED) ==
        std::vector<fake::Event>{{0, Ev::Write, LED, LOW, 0}, {0, Ev::Write, LED, HIGH, 0}});
}

TEST_CASE("loop_system: the button is reported every other 100 ms tick while it reads high") {
  glue::begin();
  fake::board.in[BUTTON] = 1;
  loopAt(101);
  loopAt(202);
  loopAt(303);
  CHECK(fake::takeTx(Serial6) == "Button pressed\r\nButton pressed\r\n");
}

TEST_CASE("loop_system: the watchdog is fed only while the valve timer makes progress") {
  glue::begin();
  IWatchdog.begin(8000000);
  loopAt(11);
  CHECK(IWatchdog.reloads == 0);
  valve_loop_ticks = 1;
  loopAt(22);
  CHECK(IWatchdog.reloads == 1);
  loopAt(33);
  CHECK(IWatchdog.reloads == 1);
  valve_loop_ticks = 2;
  valve_loop_stalled = true;
  loopAt(44);
  CHECK(IWatchdog.reloads == 1);
  valve_loop_stalled = false;
  loopAt(55);
  CHECK(IWatchdog.reloads == 2);
}

TEST_CASE("loop: the boot window first, then setup_system and the main loop for ever") {
  glue::begin();
  stub::otasupport.windowCalls = 3;
  loop();
  loop();
  CHECK(stub::calls == stub::Calls{"BootLoop()", "BootLoop()"});
  stub::app.loopStopAfter = 2;
  fake::board.autoAdvanceUs = 100;
  CHECK_THROWS_AS(loop(), stub::Stop);
  CHECK(stub::callsOf("app_loop").size() == 2);
  CHECK(stub::callsOf("app_setup").size() == 1);
}
