// End-stop / overcurrent detection of a running valve motor. Hardware-free.
// Fed once per millisecond (TIM1 interrupt) with the raw motor current; all
// currents are in 0.1 mA and signed (the sign depends on the direction).
#pragma once

#include <stdint.h>

namespace vdm {

class EndStopDetector {
 public:
  // Samples ignored after the motor start (inrush); the filter is held at 0.
  // All limits below compare the filtered current, so none of them can trip
  // during the inrush time, and after it the filter (alpha 0.02) needs some
  // tens of samples to reach them (firmware 1.x behaves the same).
  static constexpr uint16_t kInrushSamples = 250;
  // Safety limit: more than kSafetyConsecutive consecutive filtered samples
  // above it trip.
  static constexpr int32_t kSafetyLimit = 600;
  static constexpr uint8_t kSafetyConsecutive = 10;
  // Hard limit: the first filtered sample above it trips.
  static constexpr int32_t kHardLimit = 1000;

  enum class Trip : uint8_t {
    None = 0,
    Bound,   // filtered current left [low, high] after the inrush time (end stop)
    Safety,  // consecutive samples above the safety limit
    Hard,    // hard limit exceeded
  };

  // Sets the end-stop bounds for the next move and clears the per-move
  // statistics (peak, trip). Call before the motor is switched on.
  void arm(int32_t low, int32_t high);

  // One sample while the motor runs. Returns the trip condition of this
  // sample (Hard before Safety before Bound); the first trip of a move is
  // latched in trip() / tripCurrent().
  Trip sample(int32_t raw);

  // Motor not running: resets filter, inrush and safety counters (the
  // per-move statistics stay readable until the next arm()).
  void idle();

  // Filtered current (0 during the inrush time).
  int32_t current() const { return current_; }
  // Largest |filtered current| since arm().
  int32_t peak() const { return peak_; }
  Trip trip() const { return trip_; }
  // Filtered current at the first trip since arm().
  int32_t tripCurrent() const { return tripCurrent_; }
  uint8_t overCount() const { return overCount_; }

 private:
  int32_t low_ = 0;
  int32_t high_ = 0;
  int32_t current_ = 0;
  int32_t peak_ = 0;
  int32_t tripCurrent_ = 0;
  uint16_t debounce_ = 0;
  uint8_t overCount_ = 0;
  Trip trip_ = Trip::None;
};

}  // namespace vdm
