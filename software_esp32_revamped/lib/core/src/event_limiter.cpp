#include "vdm/event_limiter.h"

namespace vdm {

namespace {

constexpr uint32_t kHourMs = 3600000;

}  // namespace

EventRateLimiter::EventRateLimiter(uint32_t perKeyMs, uint16_t maxPerHour)
    : perKeyMs_(perKeyMs),
      maxPerHour_(maxPerHour),
      tokensMilli_(static_cast<uint32_t>(maxPerHour) * 1000u),
      lastRefillMs_(0),  // NOMUTATE: the bucket starts full, the first refill only resets it
      keys_() {}

void EventRateLimiter::refill(uint32_t nowMs) {
  // The bucket starts full, so the first call only moves lastRefillMs_.
  const uint32_t capacity = static_cast<uint32_t>(maxPerHour_) * 1000u;
  const uint64_t num = static_cast<uint64_t>(elapsedMs(nowMs, lastRefillMs_)) * maxPerHour_ * 1000u +
                       refillRemainder_;
  lastRefillMs_ = nowMs;
  const uint64_t add = num / kHourMs;
  refillRemainder_ = static_cast<uint32_t>(num % kHourMs);
  if (tokensMilli_ + add >= capacity) {
    tokensMilli_ = capacity;
    // NOMUTATE on the next line: remainders are multiples of 1000 (so is an
    // hour in ms), a remainder off by < 1000 never changes a refill.
    refillRemainder_ = 0;  // NOMUTATE
  } else {
    tokensMilli_ += static_cast<uint32_t>(add);
  }
}

bool EventRateLimiter::allow(const Event& e, uint32_t nowMs) {
  refill(nowMs);
  for (Key& k : keys_) {
    if (k.used && elapsedMs(nowMs, k.lastMs) >= perKeyMs_) k.used = false;
  }
  if (e.severity < Severity::Warning && !eventIsCalibrationOutcome(e.code)) return false;

  const uint16_t code = static_cast<uint16_t>(e.code);
  Key* slot = nullptr;
  for (Key& k : keys_) {
    if (k.used && k.code == code && k.valve == e.valve) {
      // Still within perKeyMs (older entries were freed above).
      ++suppressed_;
      return false;
    }
  }
  if (tokensMilli_ < 1000u) {
    ++suppressed_;
    return false;
  }
  for (Key& k : keys_) {
    if (!k.used) {
      slot = &k;
      break;
    }
    if (slot == nullptr || elapsedMs(nowMs, k.lastMs) > elapsedMs(nowMs, slot->lastMs)) slot = &k;
  }
  tokensMilli_ -= 1000u;
  slot->code = code;
  slot->valve = e.valve;
  slot->used = true;
  slot->lastMs = nowMs;
  return true;
}

}  // namespace vdm
