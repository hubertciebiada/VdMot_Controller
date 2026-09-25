// Rate limit and aggregation of events published to MQTT. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/event_log.h"

namespace vdm {

// Rate limit for events published to MQTT (<root>/events) by priority class:
//   Critical: no hourly budget, one event per (code, valve) per criticalPerKeyMs;
//   Error:    own token bucket of errorPerHour, one per (code, valve) per perKeyMs;
//   Normal (everything else that reaches MQTT): bucket of maxPerHour, per key
//   perKeyMs.
// A bucket holds its per-hour count and refills one token per
// 3600000/perHour ms. Events for which eventReachesMqtt() is false are
// refused without counting them as suppressed. Fixed table of kKeys
// (valve, code) entries; when full the least recently used entry is recycled.
class EventRateLimiter {
 public:
  static constexpr size_t kKeys = 64;
  explicit EventRateLimiter(uint32_t perKeyMs = 600000, uint16_t maxPerHour = 30,
                            uint16_t errorPerHour = 30, uint32_t criticalPerKeyMs = 60000);
  // True when the event may be published now (and books it). Key entries
  // older than their per-key time are freed on every call, so a millis() wrap
  // cannot resurrect them.
  bool allow(const Event& e, uint32_t nowMs);
  uint32_t suppressed() const { return suppressed_; }

 private:
  struct Bucket {
    uint16_t perHour;
    uint32_t tokensMilli;      // tokens x 1000
    uint32_t lastRefillMs;
    uint32_t remainder;        // sub-milli-token refill carried over (< 3600000)
    void refill(uint32_t nowMs);
  };
  struct Key {
    uint16_t code;
    uint8_t valve;
    bool used;
    uint32_t lastMs;
    uint32_t holdMs;           // per-key time of the class that booked it
  };
  uint32_t perKeyMs_;
  uint32_t criticalPerKeyMs_;
  Bucket normal_;
  Bucket error_;
  Key keys_[kKeys];
  uint32_t suppressed_ = 0;
};

// One MQTT event: a single event, or one code on several valves.
struct PublishEvent {
  Event event;              // valve = kAllValves for >= 2 bits in valveMask
  uint16_t valveMask = 0;   // bit v = valve v; 0 for events that are not about a valve
};

// Merges valve events of the same code within kWindowMs into one MQTT event.
// A flushed slot with one valve bit is the original event; with two or more
// bits valve = kAllValves, the mask, the first event's seq/time/args/text and
// the highest severity of the merged events.
class EventAggregator {
 public:
  static constexpr uint32_t kWindowMs = 2000;
  static constexpr size_t kSlots = 4;
  // Valve events only (e.valve < kValveCount; others return false and are
  // not taken). Joins the slot of its code or opens one; when all slots are
  // busy the oldest is flushed into `flushed` (returns true). A valve already
  // in its slot is counted in duplicates() and dropped.
  bool offer(const Event& e, uint32_t nowMs, PublishEvent& flushed);
  // The oldest slot whose window passed (any slot with flushAll): true and
  // the event in `out`.
  bool poll(uint32_t nowMs, PublishEvent& out, bool flushAll = false);
  void reset();  // on disconnect: pending slots are dropped
  uint32_t duplicates() const { return duplicates_; }

 private:
  struct Slot {
    bool used = false;
    uint32_t openedMs = 0;
    Event first;
    Severity severity = Severity::Debug;
    uint16_t mask = 0;
  };
  static void take(Slot& s, PublishEvent& out);
  Slot slots_[kSlots];
  uint32_t duplicates_ = 0;
};

}  // namespace vdm
