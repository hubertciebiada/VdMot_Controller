#include "vdm/health_monitor.h"

namespace vdm {

namespace {

constexpr uint8_t kStatusIdle = static_cast<uint8_t>(ValveStatus::Idle);
constexpr uint8_t kStatusFailed = static_cast<uint8_t>(ValveStatus::Failed);
constexpr uint8_t kStatusNoValve = static_cast<uint8_t>(ValveStatus::NoValve);
constexpr uint8_t kStatusBlocked = static_cast<uint8_t>(ValveStatus::Blocked);
constexpr uint32_t kCounterEventIntervalMs = 600000;
constexpr uint16_t kRecoverableFlags = kHealthStale | kHealthTargetUnconfirmed;
// arg2 of ValveBlocked/CalibFailed (failsafe position) and ValveFailed
// (fault) when the snapshot has neither: the message leaves it out.
constexpr int32_t kArgNone = -1;
constexpr uint32_t kCfgEventMaxUptimeS = 600;  // an older repair is not re-reported after a restart

// Position the STM drives a blocked valve to (protocol 3), else kArgNone.
int32_t blockedPosition(const ValveState& v) {
  if (!v.hasV3 || (v.stmFlags & kStmFlagFsBlocked) == 0 || v.fsPct == kFailsafeHold) return kArgNone;
  return v.fsPct;
}

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

// The event of a bad status (badStatus() != 0).
void addBad(Sink& sink, uint8_t valve, uint8_t status, const ValveState& v) {
  switch (status) {
    case kStatusBlocked:
      sink.add(EventCode::ValveBlocked, valve, v.calibRetries, blockedPosition(v));
      break;
    case kStatusFailed:
      sink.add(EventCode::ValveFailed, valve, v.calibRetries, v.hasV3 ? v.fault : kArgNone);
      break;
    default: sink.add(EventCode::ValveNoValve, valve, v.calibRetries); break;
  }
}

bool rose(const ValveState& before, const ValveState& after, uint16_t flag) {
  return (after.health & flag) != 0 && (before.health & flag) == 0;
}

void addTargetSet(Sink& sink, uint8_t valve, const ValveState& before, const ValveState& after) {
  if (!after.desiredValid) return;
  if (after.source != TargetSource::Web && after.source != TargetSource::Mqtt &&
      after.source != TargetSource::Assembly) {
    return;
  }
  if (before.desiredValid && before.desired == after.desired) return;
  sink.add(EventCode::TargetSet, valve, after.desired, static_cast<int32_t>(after.source));
}

// Events of a known valve between two snapshots, except TargetSet and
// ValveStateChanged. Returns true when a status-related event covered the
// status change.
bool addTransitions(Sink& sink, uint8_t valve, const ValveState& before, const ValveState& after,
                    bool active, uint16_t minCounts) {
  const uint8_t prevBad = badStatus(before, active);
  const uint8_t curBad = badStatus(after, active);

  // Calibration outcome first: it replaces the status event it implies.
  bool calibOk = false;
  bool calibFailed = false;
  if (!before.calibrating && after.calibrating) {
    sink.add(EventCode::CalibStarted, valve, after.autoRetry ? 2 : 0);
  } else if (before.calibrating && !after.calibrating) {
    // v2 reports the outcome itself (calState bit 3); 1.x only by the status.
    const bool failed = after.status == kStatusBlocked ||
                        (after.hasExtended && (after.calFlags & kCalFlagLastFailed) != 0);
    if (failed) {
      sink.add(EventCode::CalibFailed, valve, after.calibRetries, blockedPosition(after));
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
        addBad(sink, valve, curBad, after);
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
  if (rose(before, after, kHealthStrokeShort)) {
    const uint32_t stroke = after.openCount < after.closeCount ? after.openCount : after.closeCount;
    sink.add(EventCode::CalibStrokeShort, valve, static_cast<int32_t>(stroke), minCounts);
  }
  const uint16_t cleared = static_cast<uint16_t>(before.health & ~after.health & kRecoverableFlags);
  if (cleared != 0) sink.add(EventCode::ValveRecovered, valve, 0, cleared);
  return covered;
}

}  // namespace

// ---------------------------------------------------------------- HealthMonitor

size_t HealthMonitor::onValve(uint8_t valve, const ValveState& before, const ValveState& after,
                              bool active, Event* out, size_t maxOut) {
  if (valve >= kValveCount) return 0;
  Sink sink(out, maxOut);
  bool statusChanged = false;
  if (!before.known && after.known) {
    const uint8_t bad = badStatus(after, active);
    if (bad != 0) addBad(sink, valve, bad, after);
  } else if (after.known) {
    statusChanged = !addTransitions(sink, valve, before, after, active, minCounts_) &&
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

size_t HealthMonitor::onStmStatus(const StmStatus* before, const StmStatus& after, uint32_t nowMs,
                                  Event* out, size_t maxOut) {
  if (!after.v3) return 0;
  Sink sink(out, maxOut);
  const StmStatus* prev = before != nullptr && before->v3 ? before : nullptr;
  if (after.safeMode && (prev == nullptr || !prev->safeMode)) {
    sink.add(EventCode::StmSafeMode, kNoValve, after.wdgResets);
  } else if (!after.safeMode && prev != nullptr && prev->safeMode) {
    sink.add(EventCode::StmSafeModeEnded, kNoValve);
  }
  const bool repaired = prev == nullptr
                            ? after.cfgEvents > 0 && after.uptimeS < kCfgEventMaxUptimeS
                            : after.cfgEvents > prev->cfgEvents;
  if (repaired) {
    sink.add(EventCode::StmConfigRepaired, kNoValve, after.cfgFlags,
             static_cast<int32_t>(after.cfgEvents));
  }
  const uint32_t lineErrors = after.uartOre + after.uartFe + after.uartNe;
  if (prev == nullptr) uart_.baselined = false;
  if (counterIncreased(uart_, lineErrors + after.rxDropped, nowMs)) {
    sink.add(EventCode::StmUartErrors, kNoValve, static_cast<int32_t>(lineErrors),
             static_cast<int32_t>(after.rxDropped));
  }
  const bool suspended = (after.sysFlags & kStmSysProtectSuspended) != 0;
  if (suspended && (prev == nullptr || (prev->sysFlags & kStmSysProtectSuspended) == 0)) {
    sink.add(EventCode::StmProtectionSuspended, kNoValve);
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

}  // namespace vdm
