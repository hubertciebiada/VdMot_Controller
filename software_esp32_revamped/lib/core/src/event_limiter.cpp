#include "vdm/event_limiter.h"

namespace vdm {

namespace {

constexpr uint32_t kHourMs = 3600000;

}  // namespace

// ---------------------------------------------------------------- EventRateLimiter

void EventRateLimiter::Bucket::refill(uint32_t nowMs) {
  // The bucket starts full, so the first call only moves lastRefillMs.
  const uint32_t capacity = static_cast<uint32_t>(perHour) * 1000u;
  const uint64_t num =
      static_cast<uint64_t>(elapsedMs(nowMs, lastRefillMs)) * perHour * 1000u + remainder;
  lastRefillMs = nowMs;
  const uint64_t add = num / kHourMs;
  remainder = static_cast<uint32_t>(num % kHourMs);
  if (tokensMilli + add >= capacity) {
    tokensMilli = capacity;
    // Remainders are multiples of 1000 (so is an hour in ms): a remainder
    // off by < 1000 never changes a refill.
    remainder = 0;  // NOMUTATE: any value below 1000 is equivalent
  } else {
    tokensMilli += static_cast<uint32_t>(add);
  }
}

EventRateLimiter::EventRateLimiter(uint32_t perKeyMs, uint16_t maxPerHour, uint16_t errorPerHour,
                                   uint32_t criticalPerKeyMs)
    : perKeyMs_(perKeyMs),
      criticalPerKeyMs_(criticalPerKeyMs),
      // The buckets start full: the first refill only moves lastRefillMs.
      normal_{maxPerHour, static_cast<uint32_t>(maxPerHour) * 1000u, 0, 0},  // NOMUTATE: start time irrelevant (full bucket)
      error_{errorPerHour, static_cast<uint32_t>(errorPerHour) * 1000u, 0, 0},  // NOMUTATE: start time irrelevant (full bucket)
      keys_() {}

bool EventRateLimiter::allow(const Event& e, uint32_t nowMs) {
  normal_.refill(nowMs);
  error_.refill(nowMs);
  for (Key& k : keys_) {
    if (k.used && elapsedMs(nowMs, k.lastMs) >= k.holdMs) k.used = false;
  }
  if (!eventReachesMqtt(e)) return false;

  const bool critical = e.severity >= Severity::Critical;
  Bucket* bucket = critical ? nullptr : e.severity == Severity::Error ? &error_ : &normal_;
  const uint16_t code = static_cast<uint16_t>(e.code);
  for (Key& k : keys_) {
    if (k.used && k.code == code && k.valve == e.valve) {
      // Still within its per-key time (older entries were freed above).
      ++suppressed_;
      return false;
    }
  }
  if (bucket != nullptr && bucket->tokensMilli < 1000u) {
    ++suppressed_;
    return false;
  }
  Key* slot = nullptr;
  for (Key& k : keys_) {
    if (!k.used) {
      slot = &k;
      break;
    }
    if (slot == nullptr || elapsedMs(nowMs, k.lastMs) > elapsedMs(nowMs, slot->lastMs)) slot = &k;
  }
  if (bucket != nullptr) bucket->tokensMilli -= 1000u;
  slot->code = code;
  slot->valve = e.valve;
  slot->used = true;
  slot->lastMs = nowMs;
  slot->holdMs = critical ? criticalPerKeyMs_ : perKeyMs_;
  return true;
}

// ---------------------------------------------------------------- EventAggregator

void EventAggregator::take(Slot& s, PublishEvent& out) {
  out.event = s.first;
  out.valveMask = s.mask;
  if ((s.mask & (s.mask - 1u)) != 0) {  // two or more valves
    out.event.valve = kAllValves;
    out.event.severity = s.severity;
  }
  s.used = false;
}

bool EventAggregator::offer(const Event& e, uint32_t nowMs, PublishEvent& flushed) {
  if (e.valve >= kValveCount) return false;
  const uint16_t bit = static_cast<uint16_t>(1u << e.valve);
  Slot* free = nullptr;
  Slot* oldest = nullptr;
  for (Slot& s : slots_) {
    if (!s.used) {
      if (free == nullptr) free = &s;
      continue;
    }
    if (s.first.code == e.code) {
      if (s.mask & bit) {
        ++duplicates_;
      } else {
        s.mask = static_cast<uint16_t>(s.mask | bit);
        if (e.severity > s.severity) s.severity = e.severity;
      }
      return false;
    }
    if (oldest == nullptr || elapsedMs(nowMs, s.openedMs) > elapsedMs(nowMs, oldest->openedMs)) {
      oldest = &s;
    }
  }
  bool out = false;
  if (free == nullptr) {
    take(*oldest, flushed);
    free = oldest;
    out = true;
  }
  free->used = true;
  free->openedMs = nowMs;
  free->first = e;
  free->severity = e.severity;
  free->mask = bit;
  return out;
}

bool EventAggregator::poll(uint32_t nowMs, PublishEvent& out, bool flushAll) {
  Slot* oldest = nullptr;
  for (Slot& s : slots_) {
    if (!s.used || (!flushAll && elapsedMs(nowMs, s.openedMs) < kWindowMs)) continue;
    if (oldest == nullptr || elapsedMs(nowMs, s.openedMs) > elapsedMs(nowMs, oldest->openedMs)) {
      oldest = &s;
    }
  }
  if (oldest == nullptr) return false;
  take(*oldest, out);
  return true;
}

void EventAggregator::reset() {
  for (Slot& s : slots_) s.used = false;
}

}  // namespace vdm
