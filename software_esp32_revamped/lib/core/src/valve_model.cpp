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
  }
  return "unknown";
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
  if (a.calState != b.calState || a.earlyStops != b.earlyStops || a.cmdRejected != b.cmdRejected) {
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
  return m;
}

// ---------------------------------------------------------------- ValveModel

ValveModel::ValveModel(const ValveModelParams& params) : params_(params) {}

bool ValveModel::isActive(uint8_t i) const { return ((active_ >> i) & 1u) != 0; }

void ValveModel::commit(uint8_t i, const ValveState& before) {
  updateHealth(i);
  if (!sameState(before, v_[i])) ++v_[i].revision;
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
  if (v.desiredValid && v.desired == pos) {
    // Same value: only a Failed delivery is re-armed (explicit retry).
    if (v.sync == TargetSync::Failed) {
      v.source = src;
      v.sync = TargetSync::Pending;
      v.pushAttempts = 0;
      keepUnconfirmed_[valve] = false;
    }
  } else {
    const bool inFlight = v.sync == TargetSync::AwaitAck || v.sync == TargetSync::AwaitVerify;
    v.desiredValid = true;
    v.desired = pos;
    v.source = src;
    v.pushAttempts = 0;
    keepUnconfirmed_[valve] = false;
    v.sync = (!inFlight && v.stmTargetKnown && v.stmTarget == pos) ? TargetSync::Synced
                                                                   : TargetSync::Pending;
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
  applyReadBack(i, d.target);
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

void ValveModel::applyReadBack(uint8_t i, uint8_t target) {
  ValveState& v = v_[i];
  v.stmTargetKnown = true;
  v.stmTarget = target;
  if (!v.desiredValid) {
    v.desiredValid = true;
    v.desired = target;
    v.source = TargetSource::Stm;
    markSynced(i);
    return;
  }
  const bool equal = target == v.desired;
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

bool ValveModel::nextTargetPush(uint32_t nowMs, uint8_t& valve, uint8_t& pos) {
  for (uint8_t n = 0; n < kValveCount; ++n) {
    const uint8_t i = static_cast<uint8_t>((pushCursor_ + n) % kValveCount);
    ValveState& v = v_[i];
    if (!isActive(i) || !v.known || !v.desiredValid || v.calibrating) continue;
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
    pushedOnce_[i] = true;
    pushCursor_ = static_cast<uint8_t>((i + 1) % kValveCount);
    commit(i, before);
    valve = i;
    pos = v.desired;
    return true;
  }
  return false;
}

void ValveModel::onTargetAck(uint8_t valve, uint32_t nowMs) {
  (void)nowMs;
  if (valve >= kValveCount || v_[valve].sync != TargetSync::AwaitAck) return;
  const ValveState before = v_[valve];
  v_[valve].sync = TargetSync::AwaitVerify;
  commit(valve, before);
}

void ValveModel::onTargetTimeout(uint8_t valve, uint32_t nowMs) {
  (void)nowMs;
  if (valve >= kValveCount || v_[valve].sync != TargetSync::AwaitAck) return;
  ValveState& v = v_[valve];
  const ValveState before = v;
  v.sync = v.pushAttempts >= params_.maxPushAttempts ? TargetSync::Failed : TargetSync::Pending;
  commit(valve, before);
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
    keepUnconfirmed_[i] = false;
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
  if (!d.valid) {
    r.seen = false;
    r.raw = kTempUnassigned;
    return;
  }
  r.id = d.id;
  r.raw = d.value;
  r.seen = true;
  r.lastSeenMs = nowMs;
}

void SensorModel::applyVoltData(uint8_t busIndex, const VoltData& d, uint32_t nowMs) {
  if (busIndex >= kVoltSlotCount) return;
  VoltReading& r = volts_[busIndex];
  if (!d.valid) {
    r.seen = false;
    r.vad = kVadFailed;
    return;
  }
  r.id = d.id;
  r.vad = d.vad;
  r.seen = true;
  r.lastSeenMs = nowMs;
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
  return r.seen && elapsedMs(nowMs, r.lastSeenMs) <= maxAgeMs;
}

}  // namespace vdm
