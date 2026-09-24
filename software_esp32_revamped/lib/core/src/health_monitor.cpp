#include "vdm/health_monitor.h"

namespace vdm {

namespace {

constexpr uint8_t kStatusIdle = static_cast<uint8_t>(ValveStatus::Idle);
constexpr uint8_t kStatusFailed = static_cast<uint8_t>(ValveStatus::Failed);
constexpr uint8_t kStatusNoValve = static_cast<uint8_t>(ValveStatus::NoValve);
constexpr uint8_t kStatusBlocked = static_cast<uint8_t>(ValveStatus::Blocked);
constexpr uint32_t kCounterEventIntervalMs = 600000;
constexpr uint32_t kHourMs = 3600000;
constexpr uint16_t kRecoverableFlags = kHealthStale | kHealthTargetUnconfirmed;

// Bounded event sink for one call.
class Sink {
 public:
  Sink(Event* out, size_t maxOut)
      : out_(out), max_(out == nullptr ? 0 : (maxOut < kMaxEventsPerUpdate ? maxOut
                                                                           : kMaxEventsPerUpdate)) {}
  void add(EventCode code, uint8_t valve, int32_t a1 = 0, int32_t a2 = 0) {
    if (n_ < max_) out_[n_++] = makeEvent(code, eventDefaultSeverity(code), valve, a1, a2, "");
  }
  size_t count() const { return n_; }

 private:
  Event* out_;
  size_t max_;
  size_t n_ = 0;
};

// Status value of a "bad" condition (Blocked/Failed/NoValve), 0 otherwise.
// Only called for known snapshots.
uint8_t badStatus(const ValveState& v, bool active) {
  if (v.status == kStatusBlocked || v.status == kStatusFailed) return v.status;
  if (v.status == kStatusNoValve && active) return v.status;
  return 0;
}

EventCode badEvent(uint8_t status) {
  switch (status) {
    case kStatusBlocked: return EventCode::ValveBlocked;
    case kStatusFailed: return EventCode::ValveFailed;
    default: return EventCode::ValveNoValve;
  }
}

bool rose(const ValveState& before, const ValveState& after, uint16_t flag) {
  return (after.health & flag) != 0 && (before.health & flag) == 0;
}

void addTargetSet(Sink& sink, uint8_t valve, const ValveState& before, const ValveState& after) {
  if (!after.desiredValid) return;
  if (after.source != TargetSource::Web && after.source != TargetSource::Mqtt) return;
  if (before.desiredValid && before.desired == after.desired) return;
  sink.add(EventCode::TargetSet, valve, after.desired, static_cast<int32_t>(after.source));
}

// Events of a known valve between two snapshots, except TargetSet and
// ValveStateChanged. Returns true when a status-related event covered the
// status change.
bool addTransitions(Sink& sink, uint8_t valve, const ValveState& before, const ValveState& after,
                    bool active) {
  const uint8_t prevBad = badStatus(before, active);
  const uint8_t curBad = badStatus(after, active);

  // Calibration outcome first: it replaces the status event it implies.
  bool calibOk = false;
  bool calibFailed = false;
  if (!before.calibrating && after.calibrating) {
    sink.add(EventCode::CalibStarted, valve, 0);
  } else if (before.calibrating && !after.calibrating) {
    // v2 reports the outcome itself (calState bit 3); 1.x only by the status.
    const bool failed = after.status == kStatusBlocked ||
                        (after.hasExtended && (after.calFlags & kCalFlagLastFailed) != 0);
    if (failed) {
      sink.add(EventCode::CalibFailed, valve, after.calibRetries);
      calibFailed = true;
    } else if (after.status == kStatusIdle) {
      sink.add(EventCode::CalibOk, valve, static_cast<int32_t>(after.openCount),
               static_cast<int32_t>(after.closeCount));
      calibOk = true;
    }
  }
  if ((before.calibrating || after.calibrating) && !calibFailed &&
      after.calibRetries > before.calibRetries) {
    sink.add(EventCode::CalibRetry, valve, after.calibRetries);
  }

  bool covered = calibOk || calibFailed;
  if (curBad != prevBad) {
    if (curBad != 0) {
      if (!(calibFailed && curBad == kStatusBlocked)) {
        sink.add(badEvent(curBad), valve, after.calibRetries);
      }
    } else if (!covered) {
      sink.add(EventCode::ValveRecovered, valve, prevBad, 0);
    }
    covered = true;
  }

  if (before.hasExtended && after.hasExtended) {
    if (after.earlyStops > before.earlyStops) {
      sink.add(EventCode::EarlyStop, valve, static_cast<int32_t>(after.earlyStops),
               static_cast<int32_t>(after.lastMove.stop));
    }
    if (after.cmdRejected > before.cmdRejected) {
      sink.add(EventCode::CmdRejected, valve, static_cast<int32_t>(after.cmdRejected));
    }
  }
  if (rose(before, after, kHealthTargetUnconfirmed)) {
    sink.add(EventCode::TargetNotConfirmed, valve, after.desired, after.pushAttempts);
  }
  if (rose(before, after, kHealthStale)) {
    sink.add(EventCode::ValveStale, valve, static_cast<int32_t>(ValveModelParams().staleMs / 1000u));
  }
  const uint16_t cleared = static_cast<uint16_t>(before.health & ~after.health & kRecoverableFlags);
  if (cleared != 0) sink.add(EventCode::ValveRecovered, valve, 0, cleared);
  return covered;
}

}  // namespace

uint8_t systemState(LinkState link, const ValveState* valves, uint8_t count, uint16_t activeMask) {
  bool info = link != LinkState::Up;
  if (link == LinkState::Down) return 2;
  if (valves != nullptr) {
    const uint8_t n = count < kValveCount ? count : kValveCount;
    for (uint8_t i = 0; i < n; ++i) {
      if (((activeMask >> i) & 1u) == 0) continue;
      const uint16_t h = valves[i].health;
      if (h & (kHealthBlocked | kHealthFailed)) return 2;
      if (h != 0) info = true;
    }
  }
  return info ? 1 : 0;
}

// ---------------------------------------------------------------- HealthMonitor

size_t HealthMonitor::onValve(uint8_t valve, const ValveState& before, const ValveState& after,
                              bool active, Event* out, size_t maxOut) {
  if (valve >= kValveCount) return 0;
  Sink sink(out, maxOut);
  bool statusChanged = false;
  if (!before.known && after.known) {
    const uint8_t bad = badStatus(after, active);
    if (bad != 0) sink.add(badEvent(bad), valve, after.calibRetries);
  } else if (after.known) {
    statusChanged = !addTransitions(sink, valve, before, after, active) &&
                    before.status != after.status;
  }
  addTargetSet(sink, valve, before, after);
  if (statusChanged) sink.add(EventCode::ValveStateChanged, valve, before.status, after.status);
  return sink.count();
}

size_t HealthMonitor::onLink(LinkState before, LinkState after, uint8_t consecutiveTimeouts,
                             Event* out, size_t maxOut) {
  if (before == after) return 0;
  Sink sink(out, maxOut);
  switch (after) {
    case LinkState::Up:
      sink.add(EventCode::LinkUp, kNoValve);
      break;
    case LinkState::Degraded:
      sink.add(EventCode::LinkDegraded, kNoValve, consecutiveTimeouts);
      break;
    case LinkState::Down:
      sink.add(EventCode::LinkDown, kNoValve, consecutiveTimeouts);
      break;
    case LinkState::Unknown:
    case LinkState::Booting:
    case LinkState::Suspended:
      break;
  }
  return sink.count();
}

bool HealthMonitor::counterIncreased(CounterTrack& t, uint32_t total, uint32_t nowMs) {
  if (!t.baselined || total < t.total) {
    t.baselined = true;
    t.total = total;
    return false;
  }
  if (total == t.total) return false;
  if (t.reported && elapsedMs(nowMs, t.lastEventMs) < kCounterEventIntervalMs) return false;
  t.total = total;
  t.reported = true;
  t.lastEventMs = nowMs;
  return true;
}

size_t HealthMonitor::onStmCounters(uint32_t rxOverflowTotal, uint32_t parseErrTotal,
                                    uint8_t side, uint32_t nowMs, Event* out, size_t maxOut) {
  if (side > 1) return 0;
  Sink sink(out, maxOut);
  if (side == 0) {
    // The ESP counters start at 0 with the ESP: that is their baseline.
    rxOverflow_[0].baselined = true;
    parseErr_[0].baselined = true;
  }
  if (counterIncreased(rxOverflow_[side], rxOverflowTotal, nowMs)) {
    sink.add(EventCode::StmRxOverflow, kNoValve, static_cast<int32_t>(rxOverflowTotal), side);
  }
  if (counterIncreased(parseErr_[side], parseErrTotal, nowMs)) {
    sink.add(EventCode::StmParseErrors, kNoValve, static_cast<int32_t>(parseErrTotal), side);
  }
  return sink.count();
}

size_t HealthMonitor::onTempSensor(uint8_t slot, bool wasKnown, bool wasValid, bool isValid,
                                   int16_t raw, Event* out, size_t maxOut) {
  if (slot == 0 || slot > kTempSlotCount || !wasKnown || wasValid == isValid) return 0;
  Sink sink(out, maxOut);
  if (isValid) {
    sink.add(EventCode::TempSensorRecovered, kNoValve, slot);
  } else {
    sink.add(EventCode::TempSensorFailed, kNoValve, slot, raw);
  }
  return sink.count();
}

// ---------------------------------------------------------------- EventRateLimiter

EventRateLimiter::EventRateLimiter(uint32_t perKeyMs, uint16_t maxPerHour)
    : perKeyMs_(perKeyMs),
      maxPerHour_(maxPerHour),
      tokensMilli_(static_cast<uint32_t>(maxPerHour) * 1000u),
      lastRefillMs_(0),
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
    refillRemainder_ = 0;
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

// ---------------------------------------------------------------- OtaValidator

OtaValidator::OtaValidator(uint32_t confirmMs, uint32_t networkOnlyMs, uint32_t giveUpMs)
    : confirmMs_(confirmMs), networkOnlyMs_(networkOnlyMs), giveUpMs_(giveUpMs) {}

void OtaValidator::begin(bool pendingVerify, uint32_t nowMs) {
  pending_ = pendingVerify;
  startMs_ = nowMs;
  healthy_ = false;
  healthySinceMs_ = nowMs;
}

OtaValidator::Decision OtaValidator::update(bool netUp, bool linkUp, uint32_t nowMs) {
  if (!pending_) return Decision::NotPending;
  const bool healthy = netUp && linkUp;
  if (healthy && !healthy_) healthySinceMs_ = nowMs;
  healthy_ = healthy;
  const uint32_t uptime = elapsedMs(nowMs, startMs_);
  if ((healthy_ && elapsedMs(nowMs, healthySinceMs_) >= confirmMs_) ||
      (netUp && uptime >= networkOnlyMs_)) {
    pending_ = false;
    return Decision::MarkValid;
  }
  if (uptime >= giveUpMs_) {
    pending_ = false;
    return Decision::Rollback;
  }
  return Decision::Wait;
}

bool OtaValidator::confirmBeforeRestart(bool userRequested, bool netUp) {
  if (!pending_ || !userRequested || !netUp) return false;
  pending_ = false;
  return true;
}

// ---------------------------------------------------------------- NetWatchdog

void NetWatchdog::configure(uint8_t minutes) {
  minutes_ = minutes;
  fired_ = false;
}

uint32_t NetWatchdog::waitMs() const {
  uint32_t min = minutes_;
  for (uint8_t i = 0; i < restarts_ && min < kMaxWaitMin; ++i) min *= kGrowth;
  return (min < kMaxWaitMin ? min : kMaxWaitMin) * 60000u;
}

bool NetWatchdog::update(bool netUp, uint32_t nowMs) {
  if (netUp) {
    down_ = false;
    fired_ = false;
    restarts_ = 0;
    return false;
  }
  if (!started_ || !down_) {
    // Boot (the first call) or the moment the network was lost.
    started_ = true;
    down_ = true;
    downSinceMs_ = nowMs;
  }
  if (minutes_ == 0 || fired_) return false;
  if (elapsedMs(nowMs, downSinceMs_) < waitMs()) return false;
  fired_ = true;
  if (restarts_ < UINT8_MAX) ++restarts_;
  return true;
}

}  // namespace vdm
