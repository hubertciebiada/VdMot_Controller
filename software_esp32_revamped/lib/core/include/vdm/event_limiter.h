// Rate limit for events published to MQTT. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/event_log.h"

namespace vdm {

// Rate limit for events published to MQTT (<root>/events), architecture
// §3.4: at most one event per (valve, code) per perKeyMs, and at most
// maxPerHour events in any rolling hour (token bucket: capacity maxPerHour,
// one token per 3600000/maxPerHour ms). Events below Warning are rejected
// unless eventIsCalibrationOutcome(). Fixed table of kKeys (valve, code)
// entries; when full the least recently used entry is recycled.
class EventRateLimiter {
 public:
  static constexpr size_t kKeys = 64;
  explicit EventRateLimiter(uint32_t perKeyMs = 600000, uint16_t maxPerHour = 30);
  // True when the event may be published now (and books it). Events below
  // Warning that are not calibration outcomes are refused without counting
  // them as suppressed. Key entries older than perKeyMs are freed on every
  // call, so a millis() wrap cannot resurrect them.
  bool allow(const Event& e, uint32_t nowMs);
  uint32_t suppressed() const { return suppressed_; }

 private:
  struct Key {
    uint16_t code;
    uint8_t valve;
    bool used;
    uint32_t lastMs;
  };
  void refill(uint32_t nowMs);
  uint32_t perKeyMs_;
  uint16_t maxPerHour_;
  uint32_t tokensMilli_;   // tokens x 1000
  uint32_t lastRefillMs_;
  uint32_t refillRemainder_ = 0;  // sub-milli-token refill carried over (< 3600000)
  Key keys_[kKeys];
  uint32_t suppressed_ = 0;
};

}  // namespace vdm
