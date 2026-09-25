#include "vdm/end_stop_detector.h"

namespace vdm {

namespace {

int32_t absolute(int32_t v) { return v < 0 ? -v : v; }

// far outside what the ADC can deliver; keeps the filter arithmetic in range
constexpr int32_t kRawLimit = 100000;

}  // namespace

void EndStopDetector::arm(int32_t low, int32_t high, InrushMode inrush) {
  low_ = low;
  high_ = high;
  peak_ = 0;
  tripCurrent_ = 0;
  trip_ = Trip::None;
  inrush_ = inrush;
  inrushSeen_ = false;
  inrushTrip_ = false;
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

  // inrush limit on the raw samples (the filter is still held at 0)
  bool inrushOver = false;
  if (!settled && inrush_ != InrushMode::Off) {
    if (absolute(raw) > kInrushLimit) {
      if (inrushOver_ < 255) inrushOver_++;
    } else {
      inrushOver_ = 0;
    }
    inrushOver = inrushOver_ > kInrushConsecutive;
    if (inrushOver) inrushSeen_ = true;
  }

  Trip t = Trip::None;
  if (inrushOver && inrush_ == InrushMode::Enforce) {
    t = Trip::Hard;
    if (absolute(raw) > peak_) peak_ = absolute(raw);
    if (trip_ == Trip::None) {
      trip_ = t;
      tripCurrent_ = raw;
      inrushTrip_ = true;
    }
    return t;
  }
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
  inrushOver_ = 0;
}

}  // namespace vdm
