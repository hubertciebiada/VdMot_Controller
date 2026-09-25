// Valve control of the whole STM glue with the valve sim (glue_system): a blocked or jammed valve at
// its failsafe position and its automatic retry (K2, C-1), the failsafe after a cold boot with a
// silent ESP (C-2), targets before calibrations (S8), the stop of a calibration series (S8), warm
// resets with an expired lease and in a hand-over (K1-8, W2, C-8). The 3 h of C-1 are time jumps.
#include <functional>
#include <string>

#include "eeprom.h"
#include "glue_test.h"
#include "hardware.h"
#include "motor.h"
#include "otasupport.h"
#include "valve_sim.h"
#include "vdm/valve_codes.h"

void setup();
void setup_system();
void loop_system();

namespace {

// from reset to the main loop; the valves in `absent` are not connected, every failsafe position
// in the EEPROM mirror is `failsafe`
void bootController(sim::Rig& rig, uint16_t absent = 0, uint8_t failsafe = 255) {
  glue::begin();
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    rig.valve[v].connected = (absent & (1u << v)) == 0;
    rig.valve[v].pulsesPerMs = 2.0f;
    eep_content.failsafePct[v] = failsafe;
  }
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

// s seconds pass without a valve_loop tick (nothing runs in them), then 1.1 s of main loop
void jumpS(uint32_t s) {
  fake::board.nowUs += static_cast<uint64_t>(s) * 1000000u;
  IWatchdog.lastReloadUs = fake::board.nowUs;
  runMain(1100);
}

bool runMainUntil(const std::function<bool()>& done, uint32_t maxMs) {
  for (uint32_t i = 0; i < maxMs; i++) {
    if (done()) return true;
    runMain(1);
  }
  return done();
}

std::string exchange(const std::string& line) {
  fake::takeTx(Serial1);
  fake::inject(Serial1, line);
  runMain(50);
  return fake::takeTx(Serial1);
}

// first ms of a status change of the valve after `from`, 0 if none
uint32_t firstStatus(const sim::Rig& rig, uint8_t valve, uint8_t status, uint32_t from) {
  for (const sim::Rig::Step& s : rig.transitions) {
    if (s.ms >= from && s.valve == valve && s.status == status) return s.ms;
  }
  return 0;
}

}  // namespace

TEST_CASE("system K2/C-1: a blocked valve makes one failsafe move and waits for its retry") {
  sim::Rig rig;
  bootController(rig, 0x0FFF & ~(1u << 3));
  rig.valve[3].stroke = 2500;
  rig.valve[3].position = 1200;
  runMain(40000);
  REQUIRE(+myvalvemots[3].status == VLV_STATE_PRESENT);
  app_set_failsafe(3, 50);
  CHECK(exchange("stgtp 3 30\n") == "stgtp\r\n");
  REQUIRE(runMainUntil([] { return myvalvemots[3].status == VLV_STATE_BLOCKS && valve_idle(); }, 60000));
  REQUIRE(runMainUntil([] { return myvalvemots[3].actual_position != 0 && valve_idle(); }, 20000));
  CHECK(+myvalvemots[3].status == VLV_STATE_BLOCKS);
  CHECK(+myvalvemots[3].actual_position == 2500 / 89);
  CHECK(+myvalvemots[3].faultReason == (uint8_t) vdm::ValveFault::StrokesTooShort);
  const uint32_t enables = rig.valve[3].enables;
  runMain(60000);
  CHECK(rig.valve[3].enables == enables);
  valve_v3_info info;
  app_get_valve_v3(3, info);
  CHECK(info.flags == (vdm::kVlvFlagFsBlocked | vdm::kVlvFlagUncalibrated | vdm::kVlvFlagRetry));
  CHECK(info.retries == 0);
  CHECK(info.retryS > 3500);
  CHECK(info.retryS <= 3600);
  // a new target is rejected and counted, the valve stays
  CHECK(exchange("stgtp 3 80\n") == "stgtp\r\n");
  runMain(5000);
  CHECK(+myvalves[3].cmdRejected == 1);
  CHECK(rig.valve[3].enables == enables);
  // the retry: one calibration series, blocked again, one failsafe move, the next retry in 6 h
  jumpS(info.retryS);
  REQUIRE(runMainUntil([] { return myvalvemots[3].calibActive != 0; }, 20000));
  REQUIRE(runMainUntil([] { return myvalvemots[3].status == VLV_STATE_BLOCKS && valve_idle(); }, 60000));
  REQUIRE(runMainUntil([] { return myvalvemots[3].actual_position != 0 && valve_idle(); }, 20000));
  const uint32_t after = rig.valve[3].enables;
  runMain(30000);
  CHECK(rig.valve[3].enables == after);
  app_get_valve_v3(3, info);
  CHECK(info.retries == 1);
  CHECK(info.retryS > 21500);
  CHECK(rig.conflicts == 0);
}

TEST_CASE("system C-1: a valve jammed at 25 % makes one calibration series and one failsafe move per retry") {
  sim::Rig rig;
  bootController(rig, 0x0FFF & ~(1u << 5));
  rig.valve[5].position = 600;
  rig.valve[5].jamFrom = 900;
  rig.valve[5].jamTo = 3600;
  runMain(40000);
  REQUIRE(+myvalvemots[5].status == VLV_STATE_PRESENT);
  app_set_failsafe(5, 50);
  CHECK(exchange("stgtp 5 30\n") == "stgtp\r\n");
  REQUIRE(runMainUntil([] { return myvalvemots[5].status == VLV_STATE_BLOCKS && valve_idle(); }, 60000));
  CHECK(+myvalvemots[5].calibSeq == 1);
  const uint32_t blocked = rig.valve[5].enables;
  runMain(60000);
  // at most one failsafe move, then the valve stays
  CHECK(rig.valve[5].enables <= blocked + 1);
  const uint32_t enables = rig.valve[5].enables;
  CHECK(+myvalvemots[5].status == VLV_STATE_BLOCKS);
  valve_v3_info info;
  app_get_valve_v3(5, info);
  CHECK(info.retries == 0);
  REQUIRE(info.retryS > 60);
  CHECK(info.retryS <= 3600);
  // nothing until retryS reaches 0
  jumpS(info.retryS - 60);
  CHECK(rig.valve[5].enables == enables);
  CHECK(+myvalvemots[5].calibSeq == 1);
  jumpS(60);
  REQUIRE(runMainUntil([] { return myvalvemots[5].calibActive != 0; }, 20000));
  REQUIRE(runMainUntil([] { return myvalvemots[5].status == VLV_STATE_BLOCKS && valve_idle(); }, 60000));
  CHECK(+myvalvemots[5].calibSeq == 2);
  const uint32_t again = rig.valve[5].enables;
  runMain(60000);
  CHECK(rig.valve[5].enables <= again + 1);
  app_get_valve_v3(5, info);
  CHECK(info.retries == 1);
  CHECK(info.retryS > 21000);
  CHECK(rig.conflicts == 0);
}

namespace {

// C-2: boot 0 stores startOnPower = failsafe = pct, the lease timeout 5 min and the calibration of
// valve 0; after a power cycle the ESP stays silent and the lease expires
void coldBootFailsafe(uint8_t pct) {
  sim::Rig rig;
  bootController(rig, 0x0FF0);
  if (testkit::boot() == 0) {
    runMain(40000);
    const vdm::MotorParams p = motor_get_params();
    const std::string sop = std::to_string(pct);
    CHECK(exchange("smotc " + std::to_string(p.lowFac) + " " + std::to_string(p.highFac) + " " + sop + "\n") ==
          "smotc\r\n");
    exchange("sfspo 255 " + sop + "\n");
    CHECK(exchange("slcfg 5\n") == "slcfg ok\r\n");
    CHECK(exchange("staln 0\n") == "staln\r\n");
    REQUIRE(runMainUntil([] { return myvalvemots[0].calibSeq != 0 && myvalvemots[0].calibActive == 0 && valve_idle(); },
                         60000));
    REQUIRE(+myvalvemots[0].calibrated == 1);
    runMain(10000);
    testkit::reboot(testkit::Reset::PowerOn);
  }
  CHECK(app_lease_timeout() == 5);
  runMain(40000);
  uint32_t enables[4];
  bool calibrated[4];
  for (unsigned v = 0; v < 4; v++) {
    CAPTURE(v);
    CHECK(app_failsafe_pct(v) == pct);
    CHECK(+myvalvemots[v].actual_position == pct);
    // every present valve is unreferenced after a cold boot
    if (myvalvemots[v].calibrated) {
      CHECK(+myvalvemots[v].needsReference == 1);
    } else {
      CHECK(+myvalvemots[v].status == VLV_STATE_PRESENT);
    }
    enables[v] = rig.valve[v].enables;
    calibrated[v] = myvalvemots[v].calibrated != 0;
  }
  CHECK(calibrated[0]);
  // nothing moves while the lease runs
  jumpS(200);
  CHECK(app_lease_state() != 2);
  for (unsigned v = 0; v < 4; v++) CHECK(rig.valve[v].enables == enables[v]);
  jumpS(60);
  REQUIRE(app_lease_state() == 2);
  REQUIRE(runMainUntil(
    [] {
      for (unsigned v = 0; v < 4; v++) {
        if (myvalvemots[v].status != VLV_STATE_IDLE || myvalvemots[v].needsReference) return false;
      }
      return valve_idle();
    },
    300000));
  for (unsigned v = 0; v < 4; v++) {
    CAPTURE(v);
    CHECK(rig.valve[v].enables > enables[v]);
    // a reference move (needsReference ends at an end stop) or a calibration
    CHECK((calibrated[v] ? myvalvemots[v].calibSeq == 0 : myvalvemots[v].calibSeq >= 1));
    CHECK(+myvalvemots[v].calibrated == 1);
    CHECK(+myvalvemots[v].actual_position == pct);
  }
  CHECK(rig.conflicts == 0);
}

}  // namespace

TEST_CASE("system C-2: failsafe 50 after a power cycle with a silent ESP, reference or calibration") {
  coldBootFailsafe(50);
}

TEST_CASE("system C-2: failsafe 30 after a power cycle with a silent ESP, reference or calibration") {
  coldBootFailsafe(30);
}

TEST_CASE("system S8-4: a target change of another valve goes between two calibrations") {
  sim::Rig rig;
  bootController(rig, 0x0FF0);
  runMain(40000);
  for (unsigned v = 0; v < 4; v++) {
    REQUIRE(+myvalvemots[v].status == VLV_STATE_PRESENT);
    myvalvemots[v].status = VLV_STATE_IDLE;
    myvalvemots[v].calibrated = 1;
    myvalvemots[v].scaler = 36;
  }
  CHECK(exchange("staln 0\n") == "staln\r\n");
  CHECK(exchange("staln 1\n") == "staln\r\n");
  REQUIRE(runMainUntil([] { return myvalvemots[0].calibActive != 0; }, 10000));
  const uint32_t from = rig.ms;
  CHECK(exchange("stgtp 3 20\n") == "stgtp\r\n");
  REQUIRE(runMainUntil([] { return myvalvemots[1].calibActive != 0; }, 60000));
  runMain(100);
  const uint32_t move3 = firstStatus(rig, 3, VLV_STATE_CLOSING, from);
  const uint32_t learn1 = firstStatus(rig, 1, VLV_STATE_CLOSING, from);
  const uint32_t end0 = firstStatus(rig, 0, VLV_STATE_IDLE, from);
  CHECK(end0 != 0);
  CHECK(move3 > end0);
  CHECK(move3 < learn1);
  REQUIRE(runMainUntil([] { return myvalvemots[1].calibActive == 0 && valve_idle(); }, 60000));
  CHECK(+myvalvemots[3].actual_position == 20);
}

TEST_CASE("system S8-3: sstop 255 ends the running calibration and the requested ones") {
  sim::Rig rig;
  bootController(rig, 0x0FF0);
  runMain(40000);
  for (unsigned v = 0; v < 4; v++) {
    myvalvemots[v].status = VLV_STATE_IDLE;
    myvalvemots[v].calibrated = 1;
  }
  CHECK(exchange("staln 255\n") == "staln\r\n");
  REQUIRE(runMainUntil([] { return myvalvemots[0].calibActive != 0 || myvalvemots[1].calibActive != 0; }, 10000));
  CHECK(app_stop(255) == 0);
  runMain(20000);
  for (unsigned v = 0; v < 4; v++) {
    CAPTURE(v);
    CHECK(+myvalvemots[v].calibActive == 0);
    CHECK_FALSE(app_learn_pending(v, myvalvemots[v].status, myvalvemots[v].calibration != 0));
    CHECK(+myvalvemots[v].status == VLV_STATE_IDLE);
  }
  CHECK(app_stop(20) == -1);
}

TEST_CASE("system K1-8/W2: a warm reset keeps the expired lease and the valve states, no presence test") {
  sim::Rig rig;
  bootController(rig, 0x0F00);
  if (testkit::boot() == 0) {
    runMain(40000);
    REQUIRE(+myvalvemots[0].status == VLV_STATE_PRESENT);
    REQUIRE(+myvalvemots[8].status == VLV_STATE_OPENCIR);
    // stored like the ESP does it, the EEPROM values win over the defaults after the reset; the
    // failsafe holds every valve where it is
    CHECK(exchange("sfspo 255 255\n") == "sfspo 255 ok\r\n");
    CHECK(exchange("slcfg 5\n") == "slcfg ok\r\n");
    jumpS(300);
    REQUIRE(app_lease_state() == 2);
    runMain(1000);
    fake::inject(Serial1, "reset\n");
    glue::run([] {
      runMain(2000);
      FAIL("the controller did not reset");
    });
  }
  CHECK(testkit::boot() == 1);
  CHECK(app_lease_state() == 2);
  CHECK(app_lease_timeout() == 5);
  runMain(10000);
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    CAPTURE(v);
    CHECK(rig.valve[v].enables == 0);
    CHECK(+myvalvemots[v].status == (v < 8 ? VLV_STATE_PRESENT : VLV_STATE_OPENCIR));
  }
  CHECK(app_lease_state() == 2);
}

TEST_CASE("system C-8: a reset right after a command hand-over tests that valve again") {
  sim::Rig rig;
  bootController(rig);
  if (testkit::boot() == 0) {
    runMain(40000);
    runMain(100);
    REQUIRE(valve_idle());
    REQUIRE(appsetaction(CMD_A_OPEN, 4, 10) == 0);
    testkit::reboot(testkit::Reset::Pin);
  }
  runMain(10000);
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    CAPTURE(v);
    CHECK(rig.valve[v].enables == (v == 4 ? 1u : 0u));
    CHECK(+myvalvemots[v].status == VLV_STATE_PRESENT);
  }
}
