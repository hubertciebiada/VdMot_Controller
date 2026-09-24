// Current profile of one motor move: at most kProfileSamples points
// (pulse count, |current| in 0.1 mA) at equal count spacing. Hardware-free,
// fixed size, no allocation; safe to copy by assignment.
#pragma once

#include <stdint.h>

namespace vdm {

constexpr uint8_t kProfileSamples = 32;

struct ProfileSample {
  uint16_t count;
  uint16_t current;  // 0.1 mA, absolute value
};

// Samples are taken at the first poll at or after each multiple of the
// spacing, which starts at one pulse. When the buffer is full the spacing
// doubles and only the first sample of each new grid cell is kept, so the
// samples stay on an equally spaced grid whatever the length of the move.
class ProfileRecorder {
 public:
  void reset();

  // Offers the state at one poll; recorded if the count reached the next
  // sampling point. Counts must not decrease within a move (a smaller count
  // is ignored).
  void add(uint32_t count, int32_t current);

  // Records the stop point of the move (unless it is already the last
  // sample), so the profile always ends with the current at the stop.
  void finish(uint32_t count, int32_t current);

  uint8_t size() const { return size_; }
  uint16_t spacing() const { return spacing_; }
  // i < size(); out-of-range indexes return {0, 0}.
  ProfileSample at(uint8_t i) const;

 private:
  void append(uint16_t count, uint16_t current);
  void compact();
  uint32_t nextGridPoint(uint16_t count) const;

  ProfileSample samples_[kProfileSamples] = {};
  uint8_t size_ = 0;
  uint16_t spacing_ = 1;
  uint32_t next_ = 0;
};

}  // namespace vdm
