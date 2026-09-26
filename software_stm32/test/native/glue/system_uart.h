// Helpers of the glue_system tests that talk to the controller over its UART: boot, main loop,
// time jumps, requests and the fields of their replies.
#pragma once

#include <functional>
#include <sstream>
#include <string>
#include <vector>

#include "glue_test.h"
#include "hardware.h"
#include "motor.h"
#include "otasupport.h"
#include "valve_sim.h"

void setup();
void setup_system();
void loop_system();

namespace sysuart {

// from reset to the main loop; the valves in `absent` are not connected, `before` runs after
// glue::begin() (for the fakes of this boot)
inline void bootController(sim::Rig& rig, uint16_t absent = 0, const std::function<void()>& before = {}) {
  glue::begin();
  if (before) before();
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    rig.valve[v].connected = (absent & (1u << v)) == 0;
    rig.valve[v].pulsesPerMs = 2.0f;
  }
  setup();
  while (bootstate == 0) BootLoop();
  setup_system();
  rig.install();
}

inline void runMain(uint32_t ms) {
  for (uint32_t i = 0; i < ms; i++) {
    loop_system();
    fake::advanceMs(1);
  }
}

// s seconds pass without a valve_loop tick (nothing runs in them), then 1.1 s of main loop
inline void jumpS(uint32_t s) {
  fake::board.nowUs += static_cast<uint64_t>(s) * 1000000u;
  IWatchdog.lastReloadUs = fake::board.nowUs;
  runMain(1100);
}

inline bool runMainUntil(const std::function<bool()>& done, uint32_t maxMs) {
  for (uint32_t i = 0; i < maxMs; i++) {
    if (done()) return true;
    runMain(1);
  }
  return done();
}

inline std::string exchange(const std::string& line) {
  fake::takeTx(Serial1);
  fake::inject(Serial1, line);
  runMain(50);
  return fake::takeTx(Serial1);
}

// value n (1-based, after the command word) of a reply
inline long field(const std::string& reply, unsigned n) {
  std::istringstream in(reply);
  std::string word;
  in >> word;
  long v = -1;
  for (unsigned i = 0; i < n; i++) {
    if (!(in >> v)) return -1;
  }
  return v;
}

inline long gstax(unsigned n) { return field(exchange("gstax\n"), n); }
inline long gvlvx(unsigned valve, unsigned n) { return field(exchange("gvlvx " + std::to_string(valve) + "\n"), n); }
inline long gvlvy(unsigned valve, unsigned n) { return field(exchange("gvlvy " + std::to_string(valve) + "\n"), n); }

// gvlvx/gvlvy fields
constexpr unsigned kStatus = 2, kPos = 3, kTarget = 4, kMeanCur = 5, kOc = 6, kCc = 7, kCalState = 11,
                   kCmdRejected = 13, kLastReq = 15, kLastCnt = 16, kLastStop = 17, kLastMs = 19, kFlags = 20,
                   kFault = 21, kDrive = 23, kRetryS = 24;
// gstax fields
constexpr unsigned kLease = 7, kLeaseTimeout = 10, kFailsafeMask = 11, kSafeMode = 12, kCfgFlags = 18;

// the reset command: it answers, waits for the EEPROM and restarts the controller (software reset)
inline void resetController() {
  fake::inject(Serial1, "reset\n");
  glue::run([] {
    runMain(5000);
    FAIL("the controller did not reset");
  });
}

// valves 0..count-1 idle and calibrated where the presence test left them (startOnPower 30 %)
inline void calibratedIdle(unsigned count) {
  for (unsigned v = 0; v < count; v++) {
    REQUIRE(+myvalvemots[v].status == VLV_STATE_PRESENT);
    myvalvemots[v].status = VLV_STATE_IDLE;
    myvalvemots[v].calibrated = 1;
    myvalvemots[v].scaler = 36;
  }
}

inline bool idleAt(unsigned valve, uint8_t pct) {
  return myvalvemots[valve].actual_position == pct && myvalvemots[valve].status == VLV_STATE_IDLE && valve_idle();
}

inline std::string glcfg(uint16_t timeout, const std::vector<uint8_t>& fs) {
  std::string s = "glcfg " + std::to_string(timeout);
  for (uint8_t p : fs) s += " " + std::to_string(p);
  return s + "\r\n";
}

}  // namespace sysuart
