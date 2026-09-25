#include "vdm/presence_test.h"

#include "vdm/valve_codes.h"

namespace vdm {

void PresenceTest::start(bool shortCheck, bool enforce) {
  settle_ = 0;
  low_ = 0;
  normal_ = 0;
  over_ = 0;
  shortCheck_ = shortCheck;
  enforce_ = enforce;
  shortSeen_ = false;
}

PresenceTest::Result PresenceTest::sample(int32_t filteredCurrent) {
  if (settle_ <= kSettleTicks) settle_++;
  if (settle_ <= kSettleTicks) return Result::Pending;

  const int32_t magnitude = filteredCurrent < 0 ? -filteredCurrent : filteredCurrent;
  if (shortCheck_ && magnitude > kShortLimit) {
    if (++over_ >= kShortTicks) {
      shortSeen_ = true;
      if (enforce_) return Result::Short;
    }
  } else {
    over_ = 0;
  }

  if (magnitude < kNoCurrent) {
    if (++low_ > kAbsentTicks) return Result::Absent;
  } else if (++normal_ > kPresentTicks) {
    return Result::Present;
  }
  return Result::Pending;
}

PresenceOutcome presenceOutcome(PresenceTest::Result r, bool calibrated, bool recal) {
  switch (r) {
    case PresenceTest::Result::Absent:
      return PresenceOutcome{kStOpenCircuit, false, static_cast<uint8_t>(ValveFault::None)};
    case PresenceTest::Result::Short:
      return PresenceOutcome{kStFailed, false, static_cast<uint8_t>(ValveFault::Short)};
    case PresenceTest::Result::Present:
      if (calibrated && !recal) return PresenceOutcome{kStIdle, true, static_cast<uint8_t>(ValveFault::None)};
      return PresenceOutcome{kStPresent, false, static_cast<uint8_t>(ValveFault::None)};
    case PresenceTest::Result::Pending:
    default:
      return PresenceOutcome{kStUnknown, false, static_cast<uint8_t>(ValveFault::None)};
  }
}

}  // namespace vdm
