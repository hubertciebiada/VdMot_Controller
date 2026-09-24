#include "vdm/end_stop_detector.h"

namespace vdm {

namespace {

int32_t absolute(int32_t v) { return v < 0 ? -v : v; }

// far outside what the ADC can deliver; keeps the filter arithmetic in range
constexpr int32_t kRawLimit = 100000;

}  // namespace

void EndStopDetector::arm(int32_t low, int32_t high) {
  low_ = low;
  high_ = high;
  peak_ = 0;
  tripCurrent_ = 0;
  trip_ = Trip::None;
}

EndStopDetector::Trip EndStopDetector::sample(int32_t raw) {
  if (raw > kRawLimit) raw = kRawLimit;
  if (raw < -kRawLimit) raw = -kRawLimit;
  if (debounce_ < 255) debounce_++;

  // first order IIR, alpha = 0.02, same integer arithmetic as firmware 1.x
  const bool settled = debounce_ > kInrushSamples;
  if (settled) current_ = (current_ * 9800 + raw * 200) / 10000;

  const int32_t magnitude = absolute(current_);
  if (magnitude > peak_) peak_ = magnitude;

  // consecutive samples: a short spike in a long move no longer adds up
  if (magnitude > kSafetyLimit) {
    if (overCount_ < 255) overCount_++;
  } else {
    overCount_ = 0;
  }

  Trip t = Trip::None;
  if (magnitude > kHardLimit) {
    t = Trip::Hard;
  } else if (overCount_ > kSafetyConsecutive) {
    t = Trip::Safety;
  } else if (settled && (current_ > high_ || current_ < low_)) {
    t = Trip::Bound;
  }

  if (t != Trip::None && trip_ == Trip::None) {
    trip_ = t;
    tripCurrent_ = current_;
  }
  return t;
}

void EndStopDetector::idle() {
  current_ = 0;
  debounce_ = 0;
  overCount_ = 0;
}

}  // namespace vdm
