#include "vdm/move_classifier.h"

namespace vdm {

namespace {

bool endedEarly(const MoveRequest& req, uint32_t counted) {
  if (req.requestedCounts != kRunToEndStop || req.learnedTravel == 0) return false;
  const uint32_t pct = req.expectedTravelPct > 100 ? 100 : req.expectedTravelPct;
  const uint64_t expected = static_cast<uint64_t>(req.learnedTravel) * pct / 100;
  return static_cast<uint64_t>(counted) * 2 < expected;
}

uint16_t saturate16(uint64_t v) { return v > 0xFFFF ? 0xFFFF : static_cast<uint16_t>(v); }

}  // namespace

MoveClassification classifyMove(const MoveRequest& req, MotorStop stop, uint32_t counted) {
  MoveClassification c{StopReason::None, false};
  switch (stop) {
    case MotorStop::CountReached:
      c.reason = StopReason::Target;
      break;
    case MotorStop::EndStop:
      c.early = endedEarly(req, counted);
      c.reason = c.early ? StopReason::EarlyEndStop : StopReason::EndStop;
      break;
    case MotorStop::SafetyOvercurrent:
      c.early = endedEarly(req, counted);
      c.reason = StopReason::SafetyOvercurrent;
      break;
    case MotorStop::Undercurrent:
      c.reason = StopReason::Undercurrent;
      break;
    case MotorStop::Timeout:
      c.reason = StopReason::Timeout;
      break;
    case MotorStop::Aborted:
    case MotorStop::None:
    default:
      c.reason = StopReason::Aborted;
      break;
  }
  return c;
}

MoveResult makeMoveResult(const MoveRequest& req, StopReason reason, uint32_t counted,
                          int32_t peak, uint32_t durationMs) {
  MoveResult r;
  r.dir = req.dir == kDirClose ? kDirClose : kDirOpen;
  r.requestedCounts = req.requestedCounts;
  r.countedCounts = saturate16(counted);
  r.stopReason = static_cast<uint8_t>(reason);
  r.peakCurrent = peak <= 0 ? 0 : saturate16(static_cast<uint64_t>(peak));
  r.durationMs = durationMs;
  return r;
}

}  // namespace vdm
