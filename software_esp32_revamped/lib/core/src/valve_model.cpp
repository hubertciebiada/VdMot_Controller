#include "vdm/valve_model.h"

namespace vdm {

namespace {

const char* const kStatusText[] = {"",        "idle",     "opens",     "closes",    "failed",
                                   "unknown", "no valve", "full open", "connected", "blocked"};
const char* const kStatusKey[] = {"nodata",  "idle",    "opening",  "closing",   "failed",
                                  "unknown", "novalve", "fullopen", "connected", "blocked"};
constexpr uint8_t kStatusNames = sizeof kStatusText / sizeof *kStatusText;

constexpr uint8_t kStatusOpening = static_cast<uint8_t>(ValveStatus::Opening);
constexpr uint8_t kStatusClosing = static_cast<uint8_t>(ValveStatus::Closing);
constexpr uint8_t kStatusFailed = static_cast<uint8_t>(ValveStatus::Failed);
constexpr uint8_t kStatusNoValve = static_cast<uint8_t>(ValveStatus::NoValve);
constexpr uint8_t kStatusBlocked = static_cast<uint8_t>(ValveStatus::Blocked);
constexpr uint16_t kValveMaskAll = (1u << kValveCount) - 1u;

bool sameMove(const MoveResult& a, const MoveResult& b) {
  return a.dir == b.dir && a.requestedCounts == b.requestedCounts &&
         a.countedCounts == b.countedCounts && a.stop == b.stop &&
         a.peakCurrent == b.peakCurrent && a.durationMs == b.durationMs;
}

// Every field that counts for ValveState::revision; the timestamps
// lastSeenMs/lastPushMs and the revision itself are excluded.
bool sameState(const ValveState& a, const ValveState& b) {
  return diffValve(a, b) == 0 && a.source == b.source && a.pushAttempts == b.pushAttempts &&
         a.hasExtended == b.hasExtended && a.earlyStopsAtBoot == b.earlyStopsAtBoot &&
         a.cmdRejectedAtBoot == b.cmdRejectedAtBoot && sameMove(a.lastMove, b.lastMove);
}

bool tempFailed(int16_t raw) { return raw != kTempUnassigned && !tempRawValid(raw); }

bool addressed(uint8_t valveOrAll, uint8_t i) { return valveOrAll == kAllValves || valveOrAll == i; }

// gvlvd/gvlvx carry no protocol 3 fields; fsPct stays (ESP config on 1/2).
void clearV3(ValveState& v) {
  v.hasV3 = false;
  v.stmFlags = 0;
  v.fault = 0;
  v.drive = 0;
  v.retryS = 0;
  v.retries = 0;
  v.autoRetry = false;
}

}  // namespace

const char* valveStatusText(uint8_t status) {
  return status < kStatusNames ? kStatusText[status] : "";
}

const char* valveStatusKey(uint8_t status) {
  return status < kStatusNames ? kStatusKey[status] : "invalid";
}

const char* targetSourceName(TargetSource s) {
  switch (s) {
    case TargetSource::None: return "none";
    case TargetSource::Stm: return "stm";
    case TargetSource::Web: return "web";
    case TargetSource::Mqtt: return "mqtt";
    case TargetSource::Restored: return "restored";
    case TargetSource::Assembly: return "assembly";
  }
  return "unknown";
}

FailsafeKind failsafeKind(const ValveState& v) {
  if (v.hasV3 && (v.stmFlags & kStmFlagFsBlocked) != 0) return FailsafeKind::Blocked;
  if (v.fsOverride || (v.hasV3 && (v.stmFlags & kStmFlagFsLease) != 0)) return FailsafeKind::Lease;
  return FailsafeKind::None;
}

bool valveAtFailsafe(const ValveState& v) { return failsafeKind(v) == FailsafeKind::Lease; }

bool strokeNearMinimum(uint32_t openCount, uint32_t closeCount, uint16_t minCounts) {
  if (minCounts == 0 || openCount == 0 || closeCount == 0) return false;
  const uint64_t stroke = openCount < closeCount ? openCount : closeCount;
  return stroke * 5 < uint64_t{minCounts} * 6;
}

const char* targetSyncName(TargetSync s) {
  switch (s) {
    case TargetSync::Unknown: return "unknown";
    case TargetSync::Synced: return "synced";
    case TargetSync::Pending: return "pending";
    case TargetSync::AwaitAck: return "await_ack";
    case TargetSync::AwaitVerify: return "await_verify";
    case TargetSync::Failed: return "failed";
  }
  return "invalid";
}

uint32_t diffValve(const ValveState& a, const ValveState& b) {
  uint32_t m = 0;
  if (a.status != b.status || a.calibrating != b.calibrating) m |= kChangeStatus;
  if (a.position != b.position) m |= kChangePosition;
  if (a.desiredValid != b.desiredValid || a.desired != b.desired) m |= kChangeTarget;
  if (a.meanCurrent != b.meanCurrent) m |= kChangeMeanCurrent;
  if (a.temp1 != b.temp1) m |= kChangeTemp1;
  if (a.temp2 != b.temp2) m |= kChangeTemp2;
  if (a.moves != b.moves || a.openCount != b.openCount || a.closeCount != b.closeCount ||
      a.deadZone != b.deadZone) {
    m |= kChangeCounters;
  }
  if (a.calibRetries != b.calibRetries) m |= kChangeCalibRetries;
  if (a.calState != b.calState || a.calFlags != b.calFlags || a.earlyStops != b.earlyStops ||
      a.cmdRejected != b.cmdRejected) {
    m |= kChangeExtended;
  }
  if (a.moveSeq != b.moveSeq) m |= kChangeLastMove;
  if (a.sync != b.sync || a.stmTargetKnown != b.stmTargetKnown || a.stmTarget != b.stmTarget) {
    m |= kChangeSync;
  }
  if (a.sensorId[0] != b.sensorId[0] || a.sensorId[1] != b.sensorId[1] ||
      a.sensorSlot[0] != b.sensorSlot[0] || a.sensorSlot[1] != b.sensorSlot[1]) {
    m |= kChangeSensors;
  }
  if (a.health != b.health) m |= kChangeHealth;
  if (a.known != b.known) m |= kChangeKnown;
  if (a.hasV3 != b.hasV3 || a.stmFlags != b.stmFlags || a.fault != b.fault ||
      a.fsPct != b.fsPct || a.drive != b.drive || a.retries != b.retries ||
      a.autoRetry != b.autoRetry || a.fsOverride != b.fsOverride || a.fsTarget != b.fsTarget) {
    m |= kChangeFailsafe;
  }
  return m;
}

// ---------------------------------------------------------------- ValveModel

ValveModel::ValveModel(const ValveModelParams& params) : params_(params) {}

bool ValveModel::isActive(uint8_t i) const { return ((active_ >> i) & 1u) != 0; }

void ValveModel::commit(uint8_t i, const ValveState& before) {
  updateHealth(i);
  const ValveState& v = v_[i];
  if (before.desiredValid != v.desiredValid || before.desired != v.desired ||
      before.source != v.source) {
    ++desiredRev_;
  }
  if (!sameState(before, v)) ++v_[i].revision;
}

void ValveModel::setActiveMask(uint16_t mask) {
  mask &= kValveMaskAll;
  for (uint8_t i = 0; i < kValveCount; ++i) {
    const bool now = ((mask >> i) & 1u) != 0;
    if (isActive(i) == now) continue;
    const ValveState before = v_[i];
    active_ = static_cast<uint16_t>(now ? (active_ | (1u << i)) : (active_ & ~(1u << i)));
    // Staleness of a newly activated valve is measured from the next tick.
    staleRefValid_[i] = false;
    stale_[i] = false;
    commit(i, before);
  }
}

bool ValveModel::setDesiredTarget(uint8_t valve, uint8_t pos, TargetSource src, uint32_t nowMs) {
  (void)nowMs;
  if (valve >= kValveCount || pos > 100 || !isActive(valve)) return false;
  ValveState& v = v_[valve];
  const ValveState before = v;
  // The same value from a web/MQTT command ends an assembly: the stgtp ends
  // the STM's assembly hold.
  const bool endsAssembly = v.source == TargetSource::Assembly && src != TargetSource::Assembly;
  if (v.desiredValid && v.desired == pos && !endsAssembly) {
    // Same value: only a Failed delivery is re-armed (explicit retry).
    if (v.sync == TargetSync::Failed) {
      v.source = src;
      v.sync = TargetSync::Pending;
      v.pushAttempts = 0;
      keepUnconfirmed_[valve] = false;
    }
  } else {
    const uint8_t pushedBefore = pushTarget(valve);
    const bool inFlight = v.sync == TargetSync::AwaitAck || v.sync == TargetSync::AwaitVerify;
    v.desiredValid = true;
    v.desired = pos;
    v.source = src;
    assemblyPending_[valve] = false;
    readBackFirst_[valve] = false;
    if (endsAssembly) v.forcePush = true;
    // Under the failsafe override the STM keeps getting fsTarget: the new
    // desired value waits without touching the delivery.
    if (!(v.fsOverride && before.desiredValid && pushTarget(valve) == pushedBefore)) {
      v.pushAttempts = 0;
      keepUnconfirmed_[valve] = false;
      v.sync = (!inFlight && !v.forcePush && v.stmTargetKnown && v.stmTarget == pushTarget(valve))
                   ? TargetSync::Synced
                   : TargetSync::Pending;
    }
  }
  commit(valve, before);
  return true;
}

void ValveModel::markSeen(uint8_t i, uint32_t nowMs) {
  v_[i].known = true;
  v_[i].lastSeenMs = nowMs;
  staleRefMs_[i] = nowMs;
  staleRefValid_[i] = true;
  stale_[i] = false;
}

void ValveModel::applyValveData(const ValveData& d, uint32_t nowMs) {
  if (d.valve >= kValveCount) return;
  ValveState& v = v_[d.valve];
  const ValveState before = v;
  markSeen(d.valve, nowMs);
  v.status = d.status;
  v.calibrating = d.calibrating;
  v.position = d.position;
  v.meanCurrent = d.meanCurrent;
  v.temp1 = d.temp1;
  v.temp2 = d.temp2;
  v.moves = d.moves;
  v.openCount = d.openCount;
  v.closeCount = d.closeCount;
  v.deadZone = d.deadZone;
  v.calibRetries = d.calibRetries;
  clearV3(v);
  commit(d.valve, before);
}

void ValveModel::applyValveEx(const ValveEx& d, uint32_t nowMs) {
  if (d.valve >= kValveCount || d.target > 100) return;
  const uint8_t i = d.valve;
  ValveState& v = v_[i];
  const ValveState before = v;
  markSeen(i, nowMs);
  v.status = d.status;
  v.calibrating = d.calibrating;
  v.position = d.position;
  v.meanCurrent = d.meanCurrent;
  v.openCount = d.openCount;
  v.closeCount = d.closeCount;
  v.deadZone = d.deadZone;
  v.calibRetries = d.calibRetries;
  v.moves = d.moves;
  v.hasExtended = true;
  v.calState = d.calState;
  v.calFlags = d.calFlags;
  v.earlyStops = d.earlyStops;
  v.cmdRejected = d.cmdRejected;
  // A counter below its baseline means an STM reboot went unnoticed.
  if (!baselined_[i] || d.earlyStops < v.earlyStopsAtBoot || d.cmdRejected < v.cmdRejectedAtBoot) {
    v.earlyStopsAtBoot = d.earlyStops;
    v.cmdRejectedAtBoot = d.cmdRejected;
    baselined_[i] = true;
  }
  if (!sameMove(v.lastMove, d.lastMove)) {
    v.lastMove = d.lastMove;
    ++v.moveSeq;
  }
  if (d.v3) {
    v.hasV3 = true;
    v.stmFlags = d.flags;
    v.fault = d.fault;
    v.fsPct = d.fsPct;
    v.drive = d.drive;
    v.retryS = d.retryS;
    // More automatic retries than before: the next calibration start is one.
    if (d.retries > v.retries) v.autoRetry = true;
    v.retries = d.retries;
    if (d.retries == 0 || (before.calibrating && !d.calibrating)) v.autoRetry = false;
  } else {
    clearV3(v);
  }
  const bool holdOk = !(d.v3 && v.source == TargetSource::Assembly &&
                        (d.flags & kStmFlagAssembly) == 0);
  applyReadBack(i, d.target, holdOk);
  commit(i, before);
}

void ValveModel::applyValveStates(const ValveStates& s, uint32_t nowMs) {
  (void)nowMs;
  for (uint8_t i = 0; i < kValveCount; ++i) {
    ValveState& v = v_[i];
    if (v.known) continue;
    const ValveState before = v;
    v.known = true;
    v.status = static_cast<uint8_t>(s.status[i] & 0x7F);
    commit(i, before);
  }
}

void ValveModel::applyTarget(const TargetReply& t, uint32_t nowMs) {
  (void)nowMs;
  if (t.valve >= kValveCount || t.target > 100) return;
  const ValveState before = v_[t.valve];
  applyReadBack(t.valve, t.target);
  commit(t.valve, before);
}

void ValveModel::applyReadBack(uint8_t i, uint8_t target, bool holdOk) {
  ValveState& v = v_[i];
  v.stmTargetKnown = true;
  v.stmTarget = target;
  readBackFirst_[i] = false;
  if (!v.desiredValid) {
    v.desiredValid = true;
    v.desired = target;
    v.source = TargetSource::Stm;
    markSynced(i);
    return;
  }
  // A forced push (after an STM reboot) goes out although the value matches.
  const bool equal = target == pushTarget(i) && holdOk && !v.forcePush;
  switch (v.sync) {
    case TargetSync::AwaitAck:
      // The read-back may predate the stgtp in flight; the ack decides.
      break;
    case TargetSync::Failed:
      if (equal) markSynced(i);
      break;
    case TargetSync::Unknown:
    case TargetSync::Synced:
    case TargetSync::Pending:
    case TargetSync::AwaitVerify:
      if (equal) {
        markSynced(i);
      } else {
        v.sync = TargetSync::Pending;
      }
      break;
  }
}

void ValveModel::markSynced(uint8_t i) {
  v_[i].sync = TargetSync::Synced;
  v_[i].pushAttempts = 0;
  keepUnconfirmed_[i] = false;
}

void ValveModel::applyValveSensors(const ValveSensors& s, const OneWireId* slotIds,
                                   uint8_t slotCount) {
  if (!s.isList && s.valve >= kValveCount) return;
  const uint8_t first = s.isList ? 0 : s.valve;
  const uint8_t end = s.isList ? kValveCount : static_cast<uint8_t>(s.valve + 1);
  for (uint8_t i = first; i < end; ++i) {
    ValveState& v = v_[i];
    const ValveState before = v;
    for (uint8_t k = 0; k < 2; ++k) {
      v.sensorId[k] = s.ids[i][k];
      v.sensorSlot[k] = slotIds != nullptr ? resolveTempSlot(s.ids[i][k], slotIds, slotCount) : 0;
    }
    commit(i, before);
  }
}

void ValveModel::applySensorTemps(const SensorModel& sensors, uint32_t nowMs, uint32_t maxAgeMs,
                                  bool settled) {
  for (uint8_t i = 0; i < kValveCount; ++i) {
    ValveState& v = v_[i];
    int16_t raw[2];
    for (uint8_t k = 0; k < 2; ++k) {
      const OneWireId& id = v.sensorId[k];
      const int bus = sensors.findTemp(id);  // -1 for a zero id
      if (isZero(id) || !crcValid(id)) {
        raw[k] = kTempUnassigned;
      } else if (bus >= 0 && sensors.tempFresh(static_cast<uint8_t>(bus), nowMs, maxAgeMs)) {
        raw[k] = sensors.temp(static_cast<uint8_t>(bus)).raw;
      } else if (bus >= 0 && sensors.temp(static_cast<uint8_t>(bus)).seen) {
        raw[k] = kTempReadError;  // read before, too old now
      } else {
        raw[k] = settled ? kTempReadError : kTempUnassigned;
      }
    }
    if (raw[0] == v.temp1 && raw[1] == v.temp2) continue;
    const ValveState before = v;
    v.temp1 = raw[0];
    v.temp2 = raw[1];
    commit(i, before);
  }
}

bool ValveModel::nextDelivery(uint32_t nowMs, bool assembly, uint8_t& valve) {
  for (uint8_t n = 0; n < kValveCount; ++n) {
    const uint8_t i = static_cast<uint8_t>((pushCursor_ + n) % kValveCount);
    ValveState& v = v_[i];
    if (!isActive(i) || !(v.known || v.stmTargetKnown) || !v.desiredValid) continue;
    if (readBackFirst_[i]) continue;
    const bool viaStaop = assemblyViaStaop_ && v.source == TargetSource::Assembly;
    if (viaStaop != assembly) continue;
    if (v.calibrating && holdWhileCalibrating_) continue;
    const uint32_t sincePush = elapsedMs(nowMs, v.lastPushMs);
    const bool rearm = v.sync == TargetSync::Failed;
    if (rearm) {
      if (sincePush < params_.failedRetryMs) continue;
    } else if (v.sync != TargetSync::Pending) {
      continue;
    }
    if (pushedOnce_[i] && sincePush < params_.pushRetryMs) continue;
    const ValveState before = v;
    if (rearm) {
      v.pushAttempts = 0;
      keepUnconfirmed_[i] = true;
    }
    v.sync = TargetSync::AwaitAck;
    if (v.pushAttempts < UINT8_MAX) ++v.pushAttempts;
    v.lastPushMs = nowMs;
    v.stmTargetKnown = false;  // uncertain until the read-back
    v.forcePush = false;
    assemblyPending_[i] = assembly;
    pushedOnce_[i] = true;
    pushCursor_ = static_cast<uint8_t>((i + 1) % kValveCount);
    commit(i, before);
    valve = i;
    return true;
  }
  return false;
}

bool ValveModel::nextTargetPush(uint32_t nowMs, uint8_t& valve, uint8_t& pos) {
  if (!nextDelivery(nowMs, false, valve)) return false;
  pos = pushTarget(valve);
  return true;
}

bool ValveModel::nextAssemblyPush(uint32_t nowMs, uint8_t& valve) {
  return nextDelivery(nowMs, true, valve);
}

void ValveModel::onTargetPushDropped(uint8_t valve, uint32_t nowMs) {
  (void)nowMs;
  if (valve >= kValveCount || v_[valve].sync != TargetSync::AwaitAck) return;
  ValveState& v = v_[valve];
  const ValveState before = v;
  v.sync = TargetSync::Pending;
  assemblyPending_[valve] = false;
  if (v.pushAttempts > 0) --v.pushAttempts;
  commit(valve, before);
}

void ValveModel::onTargetAck(uint8_t valve, uint32_t nowMs) {
  (void)nowMs;
  if (valve >= kValveCount || v_[valve].sync != TargetSync::AwaitAck) return;
  if (assemblyPending_[valve]) return;  // the staop result decides
  const ValveState before = v_[valve];
  v_[valve].sync = TargetSync::AwaitVerify;
  commit(valve, before);
}

void ValveModel::onTargetTimeout(uint8_t valve, uint32_t nowMs) {
  (void)nowMs;
  if (valve >= kValveCount || v_[valve].sync != TargetSync::AwaitAck) return;
  if (assemblyPending_[valve]) return;
  ValveState& v = v_[valve];
  const ValveState before = v;
  v.sync = v.pushAttempts >= params_.maxPushAttempts ? TargetSync::Failed : TargetSync::Pending;
  commit(valve, before);
}

void ValveModel::setAssembly(uint8_t valveOrAll, uint32_t nowMs) {
  (void)nowMs;
  for (uint8_t i = 0; i < kValveCount; ++i) {
    if (!addressed(valveOrAll, i) || !isActive(i)) continue;
    ValveState& v = v_[i];
    const ValveState before = v;
    v.desiredValid = true;
    v.desired = 100;
    v.source = TargetSource::Assembly;
    v.sync = TargetSync::AwaitAck;
    v.pushAttempts = 0;
    v.stmTargetKnown = false;
    v.forcePush = false;
    v.fsOverride = false;
    v.fsTarget = 0;
    keepUnconfirmed_[i] = false;
    readBackFirst_[i] = false;
    assemblyPending_[i] = true;
    commit(i, before);
  }
}

void ValveModel::assemblyResult(uint8_t valveOrAll, bool ok) {
  for (uint8_t i = 0; i < kValveCount; ++i) {
    if (!addressed(valveOrAll, i) || !assemblyPending_[i]) continue;
    ValveState& v = v_[i];
    const ValveState before = v;
    assemblyPending_[i] = false;
    if (ok) {
      v.sync = TargetSync::AwaitVerify;
    } else {
      v.sync =
          v.pushAttempts >= params_.maxPushAttempts ? TargetSync::Failed : TargetSync::Pending;
    }
    commit(i, before);
  }
}

void ValveModel::onAssemblyAck(uint8_t valveOrAll, uint32_t nowMs) {
  (void)nowMs;
  assemblyResult(valveOrAll, true);
}

void ValveModel::onAssemblyFailed(uint8_t valveOrAll, uint32_t nowMs) {
  (void)nowMs;
  assemblyResult(valveOrAll, false);
}

bool ValveModel::restoreDesired(uint8_t valve, uint8_t pos, TargetSource src) {
  if (valve >= kValveCount || pos > 100 || !isActive(valve)) return false;
  ValveState& v = v_[valve];
  const ValveState before = v;
  v.desiredValid = true;
  v.desired = pos;
  v.source = src == TargetSource::Assembly ? TargetSource::Assembly : TargetSource::Restored;
  v.sync = TargetSync::Pending;
  v.pushAttempts = 0;
  keepUnconfirmed_[valve] = false;
  readBackFirst_[valve] = true;
  commit(valve, before);
  return true;
}

uint8_t ValveModel::pushTarget(uint8_t valve) const {
  if (valve >= kValveCount) return 0;
  const ValveState& v = v_[valve];
  return v.fsOverride ? v.fsTarget : v.desired;
}

void ValveModel::setFailsafeDrive(uint16_t mask, const uint8_t (&pct)[kValveCount]) {
  for (uint8_t i = 0; i < kValveCount; ++i) {
    ValveState& v = v_[i];
    const ValveState before = v;
    const bool on = ((mask >> i) & 1u) != 0 && isActive(i) && v.desiredValid &&
                    v.source != TargetSource::Assembly && pct[i] <= 100;
    v.fsOverride = on;
    v.fsTarget = on ? pct[i] : 0;
    if (!v.hasV3) v.fsPct = pct[i];
    const uint8_t pushed = pushTarget(i);
    const uint8_t pushedBefore = before.fsOverride ? before.fsTarget : before.desired;
    if (v.desiredValid && pushed != pushedBefore) {
      v.pushAttempts = 0;
      keepUnconfirmed_[i] = false;
      v.sync = v.stmTargetKnown && v.stmTarget == pushed ? TargetSync::Synced : TargetSync::Pending;
    }
    commit(i, before);
  }
}

void ValveModel::setMinCounts(uint16_t minCounts) {
  minCounts_ = minCounts;
  for (uint8_t i = 0; i < kValveCount; ++i) {
    const ValveState before = v_[i];
    commit(i, before);
  }
}

void ValveModel::forgetStmData() {
  for (uint8_t i = 0; i < kValveCount; ++i) {
    ValveState& v = v_[i];
    const ValveState before = v;
    ValveState fresh;
    fresh.desiredValid = v.desiredValid;
    fresh.desired = v.desired;
    fresh.source = v.source;
    fresh.sync = v.desiredValid ? TargetSync::Pending : TargetSync::Unknown;
    fresh.fsOverride = v.fsOverride;
    fresh.fsTarget = v.fsTarget;
    fresh.fsPct = v.fsPct;
    fresh.moveSeq = v.moveSeq;
    fresh.revision = v.revision;
    v = fresh;
    stale_[i] = false;
    staleRefValid_[i] = false;
    baselined_[i] = false;
    assemblyPending_[i] = false;
    keepUnconfirmed_[i] = false;
    commit(i, before);
  }
}

bool ValveModel::nextVerify(uint8_t& valve) const {
  for (uint8_t i = 0; i < kValveCount; ++i) {
    if (v_[i].sync == TargetSync::AwaitVerify) {
      valve = i;
      return true;
    }
  }
  return false;
}

void ValveModel::onStmRebooted(uint32_t nowMs) {
  (void)nowMs;
  for (uint8_t i = 0; i < kValveCount; ++i) {
    ValveState& v = v_[i];
    const ValveState before = v;
    v.stmTargetKnown = false;
    v.stmTarget = 0;
    v.pushAttempts = 0;
    v.sync = v.desiredValid ? TargetSync::Pending : TargetSync::Unknown;
    v.forcePush = v.desiredValid;
    keepUnconfirmed_[i] = false;
    assemblyPending_[i] = false;
    baselined_[i] = false;
    commit(i, before);
  }
}

void ValveModel::tick(uint32_t nowMs) {
  for (uint8_t i = 0; i < kValveCount; ++i) {
    if (!isActive(i)) continue;
    const ValveState before = v_[i];
    if (!staleRefValid_[i]) {
      staleRefValid_[i] = true;
      staleRefMs_[i] = nowMs;
    }
    // Latched: a counter wrap after 49 days of silence cannot clear it.
    if (elapsedMs(nowMs, staleRefMs_[i]) >= params_.staleMs) stale_[i] = true;
    commit(i, before);
  }
}

const ValveState& ValveModel::valve(uint8_t i) const {
  static const ValveState kEmpty{};
  return i < kValveCount ? v_[i] : kEmpty;
}

bool ValveModel::anyCalibrating() const {
  for (const ValveState& v : v_) {
    if (v.calibrating) return true;
  }
  return false;
}

bool ValveModel::isBusy(uint8_t i) const {
  const ValveState& v = valve(i);  // out of range: the empty state, never busy
  if (v.calibrating || v.status == kStatusOpening || v.status == kStatusClosing) return true;
  return v.sync == TargetSync::Pending || v.sync == TargetSync::AwaitAck ||
         v.sync == TargetSync::AwaitVerify;
}

void ValveModel::updateHealth(uint8_t i) {
  const ValveState& v = v_[i];
  uint16_t h = 0;
  if (v.status == kStatusBlocked) h |= kHealthBlocked;
  if (v.status == kStatusFailed) h |= kHealthFailed;
  if (isActive(i)) {
    if (v.status == kStatusNoValve) h |= kHealthNoValve;
    if (v.calibRetries > 0) h |= kHealthCalibRetries;
    if (v.hasExtended && v.earlyStops > v.earlyStopsAtBoot) h |= kHealthEarlyStop;
    if (v.hasExtended && v.cmdRejected > v.cmdRejectedAtBoot) h |= kHealthCmdRejected;
    if (stale_[i]) h |= kHealthStale;
    if (v.sync == TargetSync::Failed || (keepUnconfirmed_[i] && v.sync != TargetSync::Synced)) {
      h |= kHealthTargetUnconfirmed;
    }
    if (tempFailed(v.temp1) || tempFailed(v.temp2)) h |= kHealthTempFailed;
    if (valveAtFailsafe(v)) h |= kHealthFailsafe;
    if (strokeNearMinimum(v.openCount, v.closeCount, minCounts_)) h |= kHealthStrokeShort;
  }
  v_[i].health = h;
}

// ---------------------------------------------------------------- sensors

bool tempRawValid(int16_t raw) {
  if (raw == kTempUnassigned || raw == kTempReadError || raw == kTempPowerOn) return false;
  return raw >= -550 && raw <= 1250;
}

bool vadValid(int32_t vad) { return vad > kVadFailed; }

namespace {

// Shared list logic of applyTempList/applyVoltList.
template <typename Reading, size_t N>
bool applyList(Reading (&readings)[N], uint8_t& count, const OneWireList& l) {
  const uint8_t n = l.count < N ? l.count : static_cast<uint8_t>(N);
  const bool changed = n != count;
  for (size_t i = n; i < N; ++i) readings[i] = Reading{};
  count = n;
  if (l.hasList) {
    for (uint8_t i = 0; i < n; ++i) {
      if (readings[i].id != l.ids[i]) {
        readings[i] = Reading{};  // another sensor now sits at this bus index
        readings[i].id = l.ids[i];
      }
    }
  }
  return changed;
}

}  // namespace

bool SensorModel::applyTempList(const OneWireList& l, uint32_t nowMs) {
  (void)nowMs;
  return applyList(temps_, tempCount_, l);
}

bool SensorModel::applyVoltList(const OneWireList& l, uint32_t nowMs) {
  (void)nowMs;
  return applyList(volts_, voltCount_, l);
}

void SensorModel::applyTempData(uint8_t busIndex, const TempData& d, uint32_t nowMs) {
  if (busIndex >= kTempSlotCount) return;
  TempReading& r = temps_[busIndex];
  if (d.valid && tempRawValid(d.value)) {
    r.failStreak = 0;
  } else {
    if (r.failStreak < UINT8_MAX) ++r.failStreak;
    if (r.failStreak < kSensorFailDebounce) return;  // one failure is held
    if (!d.valid) {
      r.seen = false;
      r.raw = kTempUnassigned;
      return;
    }
  }
  r.id = d.id;
  r.raw = d.value;
  r.seen = true;
  r.lastSeenMs = nowMs;
}

void SensorModel::applyVoltData(uint8_t busIndex, const VoltData& d, uint32_t nowMs) {
  if (busIndex >= kVoltSlotCount) return;
  VoltReading& r = volts_[busIndex];
  if (d.valid && vadValid(d.vad)) {
    r.failStreak = 0;
  } else {
    if (r.failStreak < UINT8_MAX) ++r.failStreak;
    if (r.failStreak < kSensorFailDebounce) return;  // one failure is held
    if (!d.valid) {
      r.seen = false;
      r.vad = kVadFailed;
      return;
    }
  }
  r.id = d.id;
  r.vad = d.vad;
  r.seen = true;
  r.lastSeenMs = nowMs;
}

bool SensorModel::applyStrayTempData(const TempData& d, uint32_t nowMs) {
  const int bus = d.valid ? findTemp(d.id) : -1;
  if (bus < 0) return false;
  applyTempData(static_cast<uint8_t>(bus), d, nowMs);
  return true;
}

bool SensorModel::applyStrayVoltData(const VoltData& d, uint32_t nowMs) {
  const int bus = d.valid ? findVolt(d.id) : -1;
  if (bus < 0) return false;
  applyVoltData(static_cast<uint8_t>(bus), d, nowMs);
  return true;
}

void SensorModel::setStmTempAge(uint32_t ageS, uint32_t nowMs) {
  haveStmAge_ = true;
  stmAgeS_ = ageS;
  stmAgeAtMs_ = nowMs;
}

void SensorModel::clear() {
  for (TempReading& r : temps_) r = TempReading{};
  for (VoltReading& r : volts_) r = VoltReading{};
  tempCount_ = 0;
  voltCount_ = 0;
}

const TempReading& SensorModel::temp(uint8_t busIndex) const {
  static const TempReading kEmpty{};
  return busIndex < kTempSlotCount ? temps_[busIndex] : kEmpty;
}

const VoltReading& SensorModel::volt(uint8_t busIndex) const {
  static const VoltReading kEmpty{};
  return busIndex < kVoltSlotCount ? volts_[busIndex] : kEmpty;
}

int SensorModel::findTemp(const OneWireId& id) const {
  if (isZero(id)) return -1;
  for (uint8_t i = 0; i < tempCount_; ++i) {
    if (temps_[i].id == id) return i;
  }
  return -1;
}

int SensorModel::findVolt(const OneWireId& id) const {
  if (isZero(id)) return -1;
  for (uint8_t i = 0; i < voltCount_; ++i) {
    if (volts_[i].id == id) return i;
  }
  return -1;
}

bool SensorModel::tempFresh(uint8_t busIndex, uint32_t nowMs, uint32_t maxAgeMs) const {
  if (busIndex >= kTempSlotCount) return false;
  const TempReading& r = temps_[busIndex];
  if (!r.seen || elapsedMs(nowMs, r.lastSeenMs) > maxAgeMs) return false;
  return !haveStmAge_ ||
         uint64_t{stmAgeS_} + elapsedMs(nowMs, stmAgeAtMs_) / 1000 <= kStmTempMaxAgeS;
}

void expectSensor(RequestLine& r, const SensorModel& s) {
  if (r.cmd == Cmd::Goned) {
    r.expect = s.temp(static_cast<uint8_t>(r.arg)).id;
  } else if (r.cmd == Cmd::Gowvd) {
    r.expect = s.volt(static_cast<uint8_t>(r.arg)).id;
  }
}

}  // namespace vdm
