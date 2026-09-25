// Partial moves that end at an end stop (W9): early check, position, run of early stops.
#include <stdint.h>

#include "doctest.h"
#include "vdm/move_classifier.h"

using vdm::MotorStop;
using vdm::MoveRequest;
using vdm::StopReason;

namespace {

MoveRequest partialChecked(uint16_t counts) {
  MoveRequest r{vdm::kDirOpen, counts, 0, 4000};
  r.partialEarlyCheck = true;
  return r;
}

}  // namespace

TEST_CASE("classifyMove: a checked partial move is early below 80 % of the requested pulses") {
  CHECK(vdm::kPartialEarlyPct == 80);
  const MoveRequest r = partialChecked(1000);
  vdm::MoveClassification c = vdm::classifyMove(r, MotorStop::EndStop, 799);
  CHECK(c.reason == StopReason::EarlyEndStop);
  CHECK(c.early);
  c = vdm::classifyMove(r, MotorStop::EndStop, 800);
  CHECK(c.reason == StopReason::EndStop);
  CHECK_FALSE(c.early);
  c = vdm::classifyMove(r, MotorStop::SafetyOvercurrent, 500);
  CHECK(c.reason == StopReason::SafetyOvercurrent);
  CHECK(c.early);
  c = vdm::classifyMove(r, MotorStop::SafetyOvercurrent, 800);
  CHECK_FALSE(c.early);
  c = vdm::classifyMove(r, MotorStop::CountReached, 10);
  CHECK(c.reason == StopReason::Target);
  CHECK_FALSE(c.early);
  c = vdm::classifyMove(r, MotorStop::Undercurrent, 10);
  CHECK(c.reason == StopReason::Undercurrent);
  CHECK_FALSE(c.early);
  // 79 % of 40 x 89
  const MoveRequest s = partialChecked(3560);
  CHECK(vdm::classifyMove(s, MotorStop::EndStop, 2812).early);
  CHECK_FALSE(vdm::classifyMove(s, MotorStop::EndStop, 2848).early);
}

TEST_CASE("classifyMove: no partial check for service moves and calibration strokes") {
  MoveRequest r{vdm::kDirOpen, 1000, 0, 4000};
  const vdm::MoveClassification c = vdm::classifyMove(r, MotorStop::EndStop, 10);
  CHECK(c.reason == StopReason::EndStop);
  CHECK_FALSE(c.early);
  CHECK_FALSE(r.partialEarlyCheck);
}

TEST_CASE("classifyMove: the check does not overflow for the largest request") {
  const MoveRequest r = partialChecked(0xFFFE);
  // 80 % of 65534 = 52427.2
  CHECK(vdm::classifyMove(r, MotorStop::EndStop, 52427).early);
  CHECK_FALSE(vdm::classifyMove(r, MotorStop::EndStop, 52428).early);
  CHECK_FALSE(vdm::classifyMove(r, MotorStop::EndStop, 0xFFFFFFFFu).early);
}

TEST_CASE("classifyMove: a move to the end stop keeps its rule with partialEarlyCheck") {
  MoveRequest r{vdm::kDirClose, vdm::kRunToEndStop, 100, 4000};
  r.partialEarlyCheck = true;
  CHECK(vdm::classifyMove(r, MotorStop::EndStop, 1999).early);
  CHECK_FALSE(vdm::classifyMove(r, MotorStop::EndStop, 2000).early);
}

TEST_CASE("positionAfterEndStop: from the counted pulses for a partial move") {
  CHECK(vdm::positionAfterEndStop(20, vdm::kDirOpen, 40 * 89, 5 * 89, 89) == 25);
  CHECK(vdm::positionAfterEndStop(20, vdm::kDirOpen, 40 * 89, 5 * 89 + 88, 89) == 25);
  CHECK(vdm::positionAfterEndStop(10, vdm::kDirClose, 40 * 89, 1001 * 89, 89) == 0);
  CHECK(vdm::positionAfterEndStop(10, vdm::kDirClose, 40 * 89, 10 * 89, 89) == 0);
  CHECK(vdm::positionAfterEndStop(10, vdm::kDirClose, 40 * 89, 9 * 89, 89) == 1);
  CHECK(vdm::positionAfterEndStop(95, vdm::kDirOpen, 40 * 89, 20 * 89, 89) == 100);
  CHECK(vdm::positionAfterEndStop(95, vdm::kDirOpen, 40 * 89, 5 * 89, 89) == 100);
  CHECK(vdm::positionAfterEndStop(95, vdm::kDirOpen, 40 * 89, 4 * 89, 89) == 99);
  CHECK(vdm::positionAfterEndStop(0, vdm::kDirOpen, 100, 0xFFFFFFFFu, 1) == 100);
  CHECK(vdm::positionAfterEndStop(33, vdm::kDirOpen, 40 * 89, 5 * 89, 0) == 33);
  CHECK(vdm::positionAfterEndStop(33, vdm::kDirClose, 40 * 89, 5 * 89, 0) == 33);
  CHECK(vdm::positionAfterEndStop(33, vdm::kDirOpen, vdm::kRunToEndStop, 5, 89) == 100);
  CHECK(vdm::positionAfterEndStop(33, vdm::kDirClose, vdm::kRunToEndStop, 5, 89) == 0);
}

TEST_CASE("EarlyStopRun: the second early partial stop in a row requests a calibration") {
  vdm::EarlyStopRun run;
  CHECK(run.count() == 0);
  CHECK_FALSE(run.onMove(true));
  CHECK(run.count() == 1);
  CHECK(run.onMove(true));
  CHECK(run.count() == 0);
  CHECK_FALSE(run.onMove(true));
  CHECK_FALSE(run.onMove(false));
  CHECK(run.count() == 0);
  CHECK_FALSE(run.onMove(true));
  CHECK(run.count() == 1);
  run.reset();
  CHECK(run.count() == 0);
  CHECK_FALSE(run.onMove(true));
  CHECK(run.onMove(true));
}
