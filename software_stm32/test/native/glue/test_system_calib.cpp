// Calibration records of the whole STM glue in the real eeprom.cpp (glue_system): counts, position and
// status after a reset, counts and a reference move after a power cycle, a blocked calibration after
// a power cycle.
#include <string.h>

#include "eeprom.h"
#include "system_uart.h"
#include "vdm/valve_codes.h"

using namespace sysuart;

namespace {

// boot 0: valve 0 (the only one connected) calibrated by staln 0, its record written
void calibrateValve0() {
  REQUIRE(+myvalvemots[0].status == VLV_STATE_PRESENT);
  CHECK(exchange("staln 0\n") == "staln\r\n");
  REQUIRE(runMainUntil([] { return myvalvemots[0].calibActive != 0; }, 10000));
  REQUIRE(runMainUntil([] { return myvalvemots[0].calibActive == 0 && valve_idle(); }, 60000));
  REQUIRE(+myvalvemots[0].calibrated == 1);
  runMain(5000);  // the record is written 3 s later
  REQUIRE(exchange("eepst\n") == "eepst 1 \r\n");
}

struct Kept {
  long oc, cc, meanCur, pos, target;
};

Kept keptOf0() { return Kept{gvlvx(0, kOc), gvlvx(0, kCc), gvlvx(0, kMeanCur), gvlvx(0, kPos), gvlvx(0, kTarget)}; }

}  // namespace

TEST_CASE("system W2-5: after a calibration and a reset the counts, position and status come back") {
  sim::Rig rig;
  bootController(rig, 0x0FFE);
  static Kept before;
  if (testkit::boot() == 0) {
    runMain(40000);
    calibrateValve0();
    before = keptOf0();
    // what the next boot compares with: written into the EEPROM's free area
    memcpy(fake::eeprom.bytes + 0x1000, &before, sizeof before);
    resetController();
  }
  memcpy(&before, fake::eeprom.bytes + 0x1000, sizeof before);
  CHECK(before.oc > 100);
  const Kept after = keptOf0();
  CHECK(after.oc == before.oc);
  CHECK(after.cc == before.cc);
  CHECK(after.meanCur == before.meanCur);
  CHECK(after.pos == before.pos);
  CHECK(after.target == before.target);
  CHECK(gvlvx(0, kStatus) == VLV_STATE_IDLE);
  CHECK(gvlvx(0, kLastStop) == 0);
  CHECK(gvlvx(0, kLastMs) == 0);
  const long flags = gvlvy(0, kFlags);
  CHECK((flags & vdm::kVlvFlagCalRestored) != 0);
  CHECK((flags & vdm::kVlvFlagNeedsRef) == 0);
  CHECK((flags & vdm::kVlvFlagUncalibrated) == 0);
}

TEST_CASE("system W2-6: after a power cycle the counts come back, a reference move instead of a calibration") {
  sim::Rig rig;
  bootController(rig, 0x0FFE);
  if (testkit::boot() == 0) {
    runMain(40000);
    calibrateValve0();
    const Kept k = keptOf0();
    memcpy(fake::eeprom.bytes + 0x1000, &k, sizeof k);
    testkit::reboot(testkit::Reset::PowerOn);
  }
  Kept before;
  memcpy(&before, fake::eeprom.bytes + 0x1000, sizeof before);
  runMain(40000);  // the presence test
  CHECK(gvlvx(0, kOc) == before.oc);
  CHECK(gvlvx(0, kCc) == before.cc);
  CHECK(gvlvx(0, kStatus) == VLV_STATE_IDLE);
  long flags = gvlvy(0, kFlags);
  CHECK((flags & vdm::kVlvFlagUncalibrated) == 0);
  CHECK((flags & vdm::kVlvFlagNeedsRef) != 0);
  const uint32_t enables = rig.valve[0].enables;
  CHECK(exchange("stgtp 0 60\n") == "stgtp\r\n");
  REQUIRE(runMainUntil([] { return idleAt(0, 60) && myvalvemots[0].needsReference == 0; }, 30000));
  runMain(2000);
  CHECK(rig.valve[0].enables == enables + 2);
  CHECK(gvlvx(0, kLastReq) == 40 * (before.oc / 100));
  CHECK((gvlvx(0, kCalState) & 3) == 0);
  CHECK(+myvalvemots[0].calibSeq == 0);
  flags = gvlvy(0, kFlags);
  CHECK((flags & vdm::kVlvFlagNeedsRef) == 0);
}

TEST_CASE("system W2-8: a calibration that ended blocked needs a full calibration after a power cycle") {
  sim::Rig rig;
  bootController(rig, 0x0FFE);
  if (testkit::boot() == 0) {
    runMain(40000);
    CHECK(exchange("smotc 17 17 30 60000 0\n") == "smotc\r\n");
    CHECK(exchange("staln 0\n") == "staln\r\n");
    REQUIRE(runMainUntil([] { return myvalvemots[0].calibActive != 0; }, 10000));
    REQUIRE(runMainUntil([] { return myvalvemots[0].status == VLV_STATE_BLOCKS && valve_idle(); }, 60000));
    CHECK(exchange("smotc 17 17 30 100 0\n") == "smotc\r\n");
    runMain(10000);
    REQUIRE(exchange("eepst\n") == "eepst 1 \r\n");
    testkit::reboot(testkit::Reset::PowerOn);
  }
  runMain(40000);  // the presence test
  CHECK(gvlvx(0, kStatus) == VLV_STATE_PRESENT);
  CHECK((gvlvy(0, kFlags) & vdm::kVlvFlagRecal) != 0);
  CHECK(rig.valve[0].enables == 1);
  CHECK(exchange("stgtp 0 60\n") == "stgtp\r\n");
  REQUIRE(runMainUntil([] { return myvalvemots[0].calibActive != 0; }, 10000));
  REQUIRE(runMainUntil([] { return myvalvemots[0].calibActive == 0 && valve_idle(); }, 60000));
  CHECK(+myvalvemots[0].calibSeq == 1);
  CHECK(gvlvx(0, kStatus) == VLV_STATE_IDLE);
  CHECK(gvlvx(0, kPos) == 60);
}
