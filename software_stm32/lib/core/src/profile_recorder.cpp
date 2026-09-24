#include "vdm/profile_recorder.h"

namespace vdm {

namespace {

uint16_t saturate16(uint32_t v) { return v > 0xFFFF ? 0xFFFF : static_cast<uint16_t>(v); }

uint16_t magnitude16(int32_t v) {
  const uint32_t m = v < 0 ? static_cast<uint32_t>(-static_cast<int64_t>(v)) : static_cast<uint32_t>(v);
  return saturate16(m);
}

constexpr uint16_t kMaxSpacing = 0x8000;

}  // namespace

void ProfileRecorder::reset() {
  size_ = 0;
  spacing_ = 1;
  next_ = 0;
}

void ProfileRecorder::append(uint16_t count, uint16_t current) {
  samples_[size_].count = count;
  samples_[size_].current = current;
  size_++;
  next_ = nextGridPoint(count);
}

uint32_t ProfileRecorder::nextGridPoint(uint16_t count) const {
  return (static_cast<uint32_t>(count) / spacing_ + 1) * spacing_;
}

void ProfileRecorder::compact() {
  // Every sample is the first poll at or after a grid point. With a doubled
  // spacing the first sample in each new grid cell is exactly what sampling
  // with that spacing from the start would have recorded.
  while (size_ == kProfileSamples && spacing_ < kMaxSpacing) {
    spacing_ = static_cast<uint16_t>(spacing_ * 2);
    uint8_t kept = 0;
    for (uint8_t i = 0; i < size_; ++i) {
      if (kept == 0 || samples_[i].count / spacing_ != samples_[kept - 1].count / spacing_) {
        samples_[kept++] = samples_[i];
      }
    }
    size_ = kept;
  }
  // with 16-bit counts the largest spacing leaves at most two cells, so size_ < kProfileSamples here
  next_ = size_ ? nextGridPoint(samples_[size_ - 1].count) : 0;
}

void ProfileRecorder::add(uint32_t count, int32_t current) {
  const uint16_t c = saturate16(count);
  if (c < next_) return;
  if (size_ == kProfileSamples) {
    compact();
    if (c < next_) return;
  }
  append(c, magnitude16(current));
}

void ProfileRecorder::finish(uint32_t count, int32_t current) {
  const uint16_t c = saturate16(count);
  if (size_ > 0) {
    ProfileSample& last = samples_[size_ - 1];
    if (c < last.count) return;
    if (c == last.count) {
      last.current = magnitude16(current);
      return;
    }
  }
  if (size_ == kProfileSamples) compact();
  append(c, magnitude16(current));
}

ProfileSample ProfileRecorder::at(uint8_t i) const {
  if (i >= size_) return ProfileSample{0, 0};
  return samples_[i];
}

}  // namespace vdm
