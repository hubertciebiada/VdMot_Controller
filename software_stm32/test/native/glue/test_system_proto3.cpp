// Protocol 3 over the UART of the whole STM glue with the valve sim (glue_system): the failsafe with
// slhbt and legacy polls, the assembly hold, a blocked calibration, a service move and its stop, safe
// mode after watchdog resets. Lease time is time jumps.
#include "eeprom.h"
#include "system_uart.h"
#include "vdm/valve_codes.h"

using namespace sysuart;

// ---------------------------------------------------------------- lease and failsafe

TEST_CASE("system K1-4: slhbt 0 does not renew, the failsafe drives, slhbt 1 returns to the targets") {
  sim::Rig rig;
  bootController(rig, 0x0FF0);
  runMain(40000);
  calibratedIdle(4);
  for (unsigned v = 0; v < 4; v++) CHECK(exchange("stgtp " + std::to_string(v) + " 60\n") == "stgtp\r\n");
  REQUIRE(runMainUntil([] { return idleAt(0, 60) && idleAt(1, 60) && idleAt(2, 60) && idleAt(3, 60); }, 20000));
  CHECK(exchange("sfspo 255 20\n") == "sfspo 255 ok\r\n");
  CHECK(exchange("sfspo 3 255\n") == "sfspo 3 ok\r\n");
  CHECK(exchange("slhbt 1\n").rfind("slhbt 1 ", 0) == 0);
  CHECK(exchange("slcfg 5\n") == "slcfg ok\r\n");
  for (int i = 0; i < 4; i++) {
    jumpS(60);
    CHECK(exchange("slhbt 0\n").rfind("slhbt 1 ", 0) == 0);
  }
  const uint32_t enables3 = rig.valve[3].enables;
  jumpS(60);
  CHECK(exchange("slhbt 0\n") == "slhbt 2 0\r\n");
  REQUIRE(runMainUntil([] { return idleAt(0, 20) && idleAt(1, 20) && idleAt(2, 20); }, 20000));
  CHECK(+myvalvemots[3].actual_position == 60);
  CHECK(rig.valve[3].enables == enables3);
  CHECK(exchange("gtgtp 0\n") == "gtgtp 0 60 \r\n");
  CHECK(gstax(kLease) == 2);
  CHECK(gstax(kFailsafeMask) == 0x0007);
  CHECK(gvlvy(0, kFlags) == vdm::kVlvFlagFsLease);
  CHECK(gvlvy(0, kDrive) == 20);
  CHECK(gvlvy(3, kFlags) == 0);
  // a new target during the failsafe is stored, the valve stays
  const uint32_t enables0 = rig.valve[0].enables;
  CHECK(exchange("stgtp 0 30\n") == "stgtp\r\n");
  runMain(5000);
  CHECK(exchange("gtgtp 0\n") == "gtgtp 0 30 \r\n");
  CHECK(+myvalvemots[0].actual_position == 20);
  CHECK(rig.valve[0].enables == enables0);
  // slhbt 1: every valve back to its stored target
  CHECK(exchange("slhbt 1\n") == "slhbt 1 300\r\n");
  REQUIRE(runMainUntil([] { return idleAt(0, 30) && idleAt(1, 60) && idleAt(2, 60); }, 20000));
  CHECK(gstax(kLease) == 1);
  CHECK(gstax(kFailsafeMask) == 0);
  CHECK(rig.conflicts == 0);
}

TEST_CASE("system K1-5: gvlvx polls of an ESP 2.0.0 keep a stored lease, gvlvy polls do not") {
  sim::Rig rig;
  bootController(rig, 0x0FFF);
  if (testkit::boot() == 0) {
    runMain(5000);
    CHECK(exchange("slcfg 5\n") == "slcfg ok\r\n");
    runMain(5000);
    testkit::reboot(testkit::Reset::PowerOn);
  }
  // no lease command in this run: the legacy polls renew
  CHECK(gstax(kLeaseTimeout) == 5);
  for (int i = 0; i < 20; i++) {
    jumpS(60);
    exchange("gvlvx 0\n");
    CHECK(gstax(kLease) == 1);
  }
  for (int i = 0; i < 4; i++) {
    jumpS(60);
    exchange("gvlvy 0\n");
    CHECK(gstax(kLease) == 1);
  }
  jumpS(60);
  exchange("gvlvy 0\n");
  CHECK(gstax(kLease) == 2);
}

TEST_CASE("system K1-7: staop during the failsafe opens fully and holds 100 until the next stgtp") {
  sim::Rig rig;
  bootController(rig, 0x0FF0);
  runMain(40000);
  calibratedIdle(4);
  CHECK(exchange("sfspo 255 255\n") == "sfspo 255 ok\r\n");
  CHECK(exchange("sfspo 2 20\n") == "sfspo 2 ok\r\n");
  CHECK(exchange("slcfg 5\n") == "slcfg ok\r\n");
  jumpS(300);
  REQUIRE(gstax(kLease) == 2);
  REQUIRE(runMainUntil([] { return idleAt(2, 20); }, 20000));
  CHECK(exchange("staop 2\n") == "staop \r\n");
  REQUIRE(runMainUntil([] { return myvalvemots[2].actual_position == 100 && valve_idle(); }, 20000));
  runMain(20000);
  CHECK(+myvalvemots[2].actual_position == 100);
  CHECK(gvlvy(2, kFlags) == vdm::kVlvFlagAssembly);
  CHECK(gvlvy(2, kDrive) == 100);
  CHECK(gstax(kFailsafeMask) == 0);
  CHECK(exchange("stgtp 2 70\n") == "stgtp\r\n");
  REQUIRE(runMainUntil([] { return idleAt(2, 20); }, 20000));
  CHECK(gvlvy(2, kFlags) == vdm::kVlvFlagFsLease);
  CHECK(gstax(kFailsafeMask) == 0x0004);
  CHECK(rig.conflicts == 0);
}

// ---------------------------------------------------------------- blocked valve

TEST_CASE("system K2-3: a calibration with every stroke too short ends blocked at the failsafe position") {
  sim::Rig rig;
  bootController(rig, 0x0FFF & ~(1u << 3));
  // a stroke of 100 % at the default scaler (89 counts per %): the failsafe move ends at 50 %
  rig.valve[3].stroke = 8900;
  runMain(40000);
  REQUIRE(+myvalvemots[3].status == VLV_STATE_PRESENT);
  CHECK(exchange("slcfg 0\n") == "slcfg ok\r\n");
  CHECK(exchange("smotc 17 17 30 60000 0\n") == "smotc\r\n");
  CHECK(exchange("staln 3\n") == "staln\r\n");
  REQUIRE(runMainUntil([] { return myvalvemots[3].calibActive != 0; }, 10000));
  REQUIRE(runMainUntil([] { return myvalvemots[3].status == VLV_STATE_BLOCKS && valve_idle(); }, 60000));
  REQUIRE(runMainUntil([] { return myvalvemots[3].actual_position == 50 && valve_idle(); }, 20000));
  runMain(2000);
  CHECK(gvlvx(3, kStatus) == VLV_STATE_BLOCKS);
  CHECK(gvlvx(3, kPos) == 50);
  CHECK((gvlvx(3, kCalState) & 8) != 0);
  // FS_BLOCKED | RETRY, and UNCALIBRATED: no calibration of this valve ever succeeded
  CHECK(gvlvy(3, kFlags) == (vdm::kVlvFlagFsBlocked | vdm::kVlvFlagRetry | vdm::kVlvFlagUncalibrated));
  CHECK(gvlvy(3, kFault) == (long) vdm::ValveFault::StrokesTooShort);
  const long retryS = gvlvy(3, kRetryS);
  CHECK(retryS > 3500);
  CHECK(retryS <= 3600);
  const long rejected = gvlvx(3, kCmdRejected);
  const uint32_t enables = rig.valve[3].enables;
  CHECK(exchange("stgtp 3 80\n") == "stgtp\r\n");
  runMain(5000);
  CHECK(gvlvx(3, kCmdRejected) == rejected + 1);
  CHECK(gvlvx(3, kPos) == 50);
  CHECK(rig.valve[3].enables == enables);
  // failsafe hold: the valve stays where the calibration left it
  CHECK(exchange("sfspo 3 255\n") == "sfspo 3 ok\r\n");
  CHECK(exchange("staln 3\n") == "staln\r\n");
  REQUIRE(runMainUntil([] { return myvalvemots[3].calibActive != 0; }, 10000));
  REQUIRE(runMainUntil([] { return myvalvemots[3].calibActive == 0 && valve_idle(); }, 60000));
  runMain(5000);
  CHECK(gvlvx(3, kStatus) == VLV_STATE_BLOCKS);
  CHECK(gvlvx(3, kPos) == 0);
  CHECK(rig.conflicts == 0);
}

// ---------------------------------------------------------------- service move and stop

TEST_CASE("system S8-2: sstop ends a service move where it is, the valve is not moved back") {
  sim::Rig rig;
  bootController(rig, 0x0FF0);
  runMain(40000);
  calibratedIdle(4);
  rig.valve[2].pulsesPerMs = 0.5f;
  const long pos0 = gvlvx(2, kPos);
  CHECK(exchange("svmov 2 0 10000 40\n") == "svmov 2 ok\r\n");
  runMain(2500);
  CHECK(exchange("sstop 2\n") == "sstop 2 ok\r\n");
  REQUIRE(runMainUntil([] { return valve_idle(); }, 5000));
  const long lastCnt = gvlvx(2, kLastCnt);
  CHECK(gvlvx(2, kLastStop) == (long) vdm::StopReason::Aborted);
  CHECK(lastCnt > 360);
  CHECK(lastCnt < 10000);
  // the position follows the counted pulses (scaler 36 counts per %)
  const long pos = gvlvx(2, kPos);
  CHECK(pos >= pos0 + lastCnt / 36 - 1);
  CHECK(pos <= pos0 + lastCnt / 36 + 1);
  const uint32_t enables = rig.valve[2].enables;
  jumpS(150);
  jumpS(150);
  runMain(5000);
  CHECK(rig.valve[2].enables == enables);
  CHECK(gvlvx(2, kPos) == pos);
}

// ---------------------------------------------------------------- safe mode

TEST_CASE("system S9-2: after three watchdog resets no valve moves until ssafe 0") {
  sim::Rig rig;
  bootController(rig, 0x0FF0);
  if (testkit::boot() < 3) {
    runMain(2000);
    testkit::reboot(testkit::Reset::Watchdog);
  }
  CHECK(gstax(kSafeMode) == 1);
  runMain(40000);
  for (unsigned v = 0; v < 4; v++) {
    CAPTURE(v);
    CHECK(rig.valve[v].enables == 0);
    CHECK(+myvalvemots[v].status == VLV_STATE_UNKNOWN);
  }
  CHECK(exchange("ssafe 1\n") == "ssafe err\r\n");
  CHECK(exchange("ssafe 0\n") == "ssafe ok\r\n");
  CHECK(gstax(kSafeMode) == 0);
  runMain(40000);
  for (unsigned v = 0; v < 4; v++) {
    CAPTURE(v);
    CHECK(rig.valve[v].enables > 0);
    CHECK(+myvalvemots[v].status == VLV_STATE_PRESENT);
  }
}
