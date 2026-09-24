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
  // from 60 % to the closed end: 60 % of 4000 = 2400 expected, early below 1200
  CHECK(vdm::classifyMove(toEnd(60, 4000), MotorStop::EndStop, 1199).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(60, 4000), MotorStop::EndStop, 1200).early);
  // the shortest checked move: 50 % of 4000 = 2000 expected, early below 1000
  CHECK(vdm::classifyMove(toEnd(vdm::kEarlyCheckMinTravelPct, 4000), MotorStop::EndStop, 999).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(vdm::kEarlyCheckMinTravelPct, 4000), MotorStop::EndStop, 1000).early);
  // more than 100 % is treated as 100 %
  CHECK(vdm::classifyMove(toEnd(250, 4000), MotorStop::EndStop, 1999).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(250, 4000), MotorStop::EndStop, 2000).early);
  // 0 % or an unknown stroke disables the check
  CHECK_FALSE(vdm::classifyMove(toEnd(0, 4000), MotorStop::EndStop, 0).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(100, 0), MotorStop::EndStop, 0).early);
}

TEST_CASE("classifyMove: short moves to an end stop are not checked") {
  // review finding: believed 3 % of a 3600 pulse stroke, the valve really is at
  // about 1.4 % and reaches the end stop after 50 pulses; this is not early
  CHECK(vdm::kEarlyCheckMinTravelPct == 50);
  const auto c = vdm::classifyMove(toEnd(3, 3600), MotorStop::EndStop, 50);
  CHECK_FALSE(c.early);
  CHECK(c.reason == StopReason::EndStop);
  CHECK_FALSE(vdm::classifyMove(toEnd(3, 3600), MotorStop::SafetyOvercurrent, 0).early);
  // just below the limit: not checked even for a stop right after the start
  CHECK_FALSE(vdm::classifyMove(toEnd(vdm::kEarlyCheckMinTravelPct - 1, 4000), MotorStop::EndStop, 0).early);
  CHECK(vdm::classifyMove(toEnd(vdm::kEarlyCheckMinTravelPct, 4000), MotorStop::EndStop, 0).early);
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

TEST_CASE("classifyMove: the early check only applies to a run to the end stop") {
  // a counted move with a learned travel is never early, whatever it counted
  const MoveRequest r{vdm::kDirClose, 500, 100, 4000};
  const auto c = vdm::classifyMove(r, MotorStop::EndStop, 0);
  CHECK_FALSE(c.early);
  CHECK(c.reason == StopReason::EndStop);
}

TEST_CASE("classifyMove: a learned travel of 1 count is a learned travel") {
  // expected 1 * 100 / 100 = 1 count, 0 counted -> 0 < 1: early
  const auto c = vdm::classifyMove(toEnd(100, 1), MotorStop::EndStop, 0);
  CHECK(c.early);
  CHECK(c.reason == StopReason::EarlyEndStop);
  // no learned travel: never early
  CHECK_FALSE(vdm::classifyMove(toEnd(100, 0), MotorStop::EndStop, 0).early);
}

TEST_CASE("classifyMove: the expected travel is capped at exactly 100 %") {
  // pct 200 counts as 100: expected 1000, early below 500 counted
  // pct 101 is above 100 as well: expected 1000, not 1010
  CHECK(vdm::classifyMove(toEnd(101, 1000), MotorStop::EndStop, 499).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(101, 1000), MotorStop::EndStop, 500).early);
  CHECK(vdm::classifyMove(toEnd(200, 1000), MotorStop::EndStop, 499).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(200, 1000), MotorStop::EndStop, 500).early);
  CHECK(vdm::classifyMove(toEnd(100, 1000), MotorStop::EndStop, 499).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(100, 1000), MotorStop::EndStop, 500).early);
  CHECK(vdm::classifyMove(toEnd(99, 1000), MotorStop::EndStop, 494).early);
  CHECK_FALSE(vdm::classifyMove(toEnd(99, 1000), MotorStop::EndStop, 495).early);
}

TEST_CASE("makeMoveResult: a peak of 1 is kept, only <= 0 reads 0") {
  CHECK(vdm::makeMoveResult(partial(10), StopReason::Target, 10, 1, 0).peakCurrent == 1);
  CHECK(vdm::makeMoveResult(partial(10), StopReason::Target, 10, 0, 0).peakCurrent == 0);
  CHECK(vdm::makeMoveResult(partial(10), StopReason::Target, 10, -1, 0).peakCurrent == 0);
}
