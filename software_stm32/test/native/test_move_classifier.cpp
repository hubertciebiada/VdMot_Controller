#include <stdint.h>

#include "doctest.h"
#include "vdm/move_classifier.h"

using vdm::MotorStop;
using vdm::MoveRequest;
using vdm::StopReason;

namespace {

MoveRequest toEnd(uint8_t pct, uint32_t travel) {
  return MoveRequest{vdm::kDirClose, vdm::kRunToEndStop, pct, travel};
}

MoveRequest partial(uint16_t counts) { return MoveRequest{vdm::kDirOpen, counts, 0, 4000}; }

}  // namespace

TEST_CASE("classifyMove: stop reasons map to their wire values") {
  CHECK(static_cast<int>(StopReason::None) == 0);
  CHECK(static_cast<int>(StopReason::Target) == 1);
  CHECK(static_cast<int>(StopReason::EndStop) == 2);
  CHECK(static_cast<int>(StopReason::EarlyEndStop) == 3);
  CHECK(static_cast<int>(StopReason::Timeout) == 4);
  CHECK(static_cast<int>(StopReason::Undercurrent) == 5);
  CHECK(static_cast<int>(StopReason::SafetyOvercurrent) == 6);
  CHECK(static_cast<int>(StopReason::Aborted) == 7);

  const MoveRequest r = partial(1200);
  CHECK(vdm::classifyMove(r, MotorStop::CountReached, 1201).reason == StopReason::Target);
  CHECK(vdm::classifyMove(r, MotorStop::EndStop, 300).reason == StopReason::EndStop);
  CHECK(vdm::classifyMove(r, MotorStop::SafetyOvercurrent, 30).reason == StopReason::SafetyOvercurrent);
  CHECK(vdm::classifyMove(r, MotorStop::Undercurrent, 0).reason == StopReason::Undercurrent);
  CHECK(vdm::classifyMove(r, MotorStop::Timeout, 9).reason == StopReason::Timeout);
  CHECK(vdm::classifyMove(r, MotorStop::Aborted, 0).reason == StopReason::Aborted);
  CHECK(vdm::classifyMove(r, MotorStop::None, 0).reason == StopReason::Aborted);
  CHECK(vdm::classifyMove(r, static_cast<MotorStop>(200), 0).reason == StopReason::Aborted);
}

TEST_CASE("classifyMove: early end stop only for moves to an end stop (S05)") {
  // partial moves never count as early
  const auto c = vdm::classifyMove(partial(3000), MotorStop::EndStop, 10);
  CHECK_FALSE(c.early);
  CHECK(c.reason == StopReason::EndStop);

  // full travel: less than half of the learned stroke
  CHECK(vdm::classifyMove(toEnd(100, 4000), MotorStop::EndStop, 1999).early);
  CHECK(vdm::classifyMove(toEnd(100, 4000), MotorStop::EndStop, 1999).reason == StopReason::EarlyEndStop);
  CHECK_FALSE(vdm::classifyMove(toEnd(100, 4000), MotorStop::EndStop, 2000).early);
  CHECK(vdm::classifyMove(toEnd(100, 4000), MotorStop::EndStop, 2000).reason == StopReason::EndStop);
  CHECK_FALSE(vdm::classifyMove(toEnd(100, 4001), MotorStop::EndStop, 2001).early);
  CHECK(vdm::classifyMove(toEnd(100, 4001), MotorStop::EndStop, 2000).early);
}

TEST_CASE("classifyMove: expected travel scales with the distance to the end") {
  // from 40 % to the closed end: 40 % of 4000 = 1600 expected, early below 800
  CHECK(vdm::classifyMove(toEnd(40, 4000), MotorStop::EndStop, 799).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(40, 4000), MotorStop::EndStop, 800).early);
  // more than 100 % is treated as 100 %
  CHECK(vdm::classifyMove(toEnd(250, 4000), MotorStop::EndStop, 1999).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(250, 4000), MotorStop::EndStop, 2000).early);
  // 0 % or an unknown stroke disables the check
  CHECK_FALSE(vdm::classifyMove(toEnd(0, 4000), MotorStop::EndStop, 0).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(100, 0), MotorStop::EndStop, 0).early);
}

TEST_CASE("classifyMove: early safety stop keeps its reason") {
  const auto c = vdm::classifyMove(toEnd(100, 4000), MotorStop::SafetyOvercurrent, 100);
  CHECK(c.early);
  CHECK(c.reason == StopReason::SafetyOvercurrent);
  CHECK_FALSE(vdm::classifyMove(toEnd(100, 4000), MotorStop::SafetyOvercurrent, 3000).early);
  // other causes are never early
  CHECK_FALSE(vdm::classifyMove(toEnd(100, 4000), MotorStop::Undercurrent, 0).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(100, 4000), MotorStop::Timeout, 0).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(100, 4000), MotorStop::CountReached, 0).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(100, 4000), MotorStop::Aborted, 0).early);
}

TEST_CASE("classifyMove: large values do not overflow") {
  CHECK(vdm::classifyMove(toEnd(100, 0xFFFFFFFFu), MotorStop::EndStop, 0x7FFFFFFFu).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(100, 0xFFFFFFFFu), MotorStop::EndStop, 0x80000000u).early);
}

TEST_CASE("makeMoveResult: fields and saturation") {
  const MoveRequest r{vdm::kDirClose, 1234, 0, 4000};
  const vdm::MoveResult m = vdm::makeMoveResult(r, StopReason::Target, 1235, 187, 5230);
  CHECK(m.dir == 1);
  CHECK(m.requestedCounts == 1234);
  CHECK(m.countedCounts == 1235);
  CHECK(m.stopReason == 1);
  CHECK(m.peakCurrent == 187);
  CHECK(m.durationMs == 5230);

  const MoveRequest o{7, vdm::kRunToEndStop, 100, 0};
  const vdm::MoveResult s = vdm::makeMoveResult(o, StopReason::EndStop, 70000, 70000, 0xFFFFFFFFu);
  CHECK(s.dir == 0);
  CHECK(s.requestedCounts == 65535);
  CHECK(s.countedCounts == 65535);
  CHECK(s.peakCurrent == 65535);
  CHECK(s.durationMs == 0xFFFFFFFFu);

  CHECK(vdm::makeMoveResult(o, StopReason::Aborted, 0, -5, 0).peakCurrent == 0);
}
