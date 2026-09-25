// Presence test of a valve motor (stdet, start-up): the motor output is
// switched on for a short time and the filtered current tells an open
// circuit, a present motor and a short circuit apart. Hardware-free; one
// sample per 10 ms tick of the valve state machine.
#pragma once

#include <stdint.h>

namespace vdm {

class PresenceTest {
 public:
  static constexpr uint8_t kSettleTicks = 7;    // samples before the current is evaluated
  static constexpr int32_t kNoCurrent = 20;     // 0.1 mA: below is no current
  static constexpr uint8_t kAbsentTicks = 80;   // absent after more than 80 samples without current
  static constexpr uint8_t kPresentTicks = 5;   // present after more than 5 samples with current
  static constexpr int32_t kShortLimit = 2000;  // 0.1 mA filtered (hardware tuning value)
  static constexpr uint8_t kShortTicks = 3;     // consecutive samples above kShortLimit

  enum class Result : uint8_t { Pending, Present, Absent, Short };

  // shortCheck false: the short limit is off (protection guard suspended).
  // enforce false (report only, vdm::kProtectEnforce): a short is only
  // recorded in shortSeen() and the test goes on with the 1.x logic.
  void start(bool shortCheck, bool enforce);
  // The short check first, then the 1.x logic unchanged (currents of either sign).
  Result sample(int32_t filteredCurrent);
  // kShortTicks consecutive samples above kShortLimit since start()
  bool shortSeen() const { return shortSeen_; }

 private:
  uint8_t settle_ = 0;
  uint8_t low_ = 0;
  uint8_t normal_ = 0;
  uint8_t over_ = 0;
  bool shortCheck_ = true;
  bool enforce_ = true;
  bool shortSeen_ = false;
};

struct PresenceOutcome {
  uint8_t status;
  bool needsReference;
  uint8_t fault;  // ValveFault
};

// Absent -> 6; Short -> 4, fault Short; Present -> 1 with needsReference for a
// calibrated valve without recal (its counts are kept, the position is not
// known), else 8 (calibration pending). Pending -> 5.
PresenceOutcome presenceOutcome(PresenceTest::Result r, bool calibrated, bool recal);

}  // namespace vdm
