#include "vdm/poll_planner.h"

#include <algorithm>

namespace vdm {

namespace {

bool sameRequest(const RequestLine& a, const RequestLine& b) {
  return a.cmd == b.cmd && a.valve == b.valve && a.arg == b.arg;
}

// Index of the lowest set bit of a non-zero mask.
uint8_t lowestBit(uint32_t mask) { return static_cast<uint8_t>(__builtin_ctz(mask)); }

}  // namespace

PollPlanner::PollPlanner(const PollCadence& cadence) : cadence_(cadence) {}

// ---------------------------------------------------------------- inputs

void PollPlanner::setProtocol(uint8_t proto) {
  proto_ = std::min<uint8_t>(proto, 2);
  if (proto_ < 2) {
    pending_ &= ~kV2Items;
    inflight_ &= ~kV2Items;
  }
}

void PollPlanner::setActiveMask(uint16_t mask) {
  activeMask_ = static_cast<uint16_t>(mask & ((1u << kValveCount) - 1u));
}

void PollPlanner::setValveBusy(uint8_t valve, bool busy) {
  if (valve >= kValveCount) return;
  const uint16_t bit = static_cast<uint16_t>(1u << valve);
  busyMask_ = static_cast<uint16_t>(busy ? busyMask_ | bit : busyMask_ & ~bit);
}

void PollPlanner::setSensorCounts(uint8_t temps, uint8_t volts) {
  tempCount_ = std::min(temps, kTempSlotCount);
  voltCount_ = std::min(volts, kVoltSlotCount);
  if (tempIndex_ >= tempCount_) tempIndex_ = 0;
  if (voltIndex_ >= voltCount_) voltIndex_ = 0;
}

void PollPlanner::requestResync() {
  step_ = ResyncStep::Proto;  // advanceStep() zeroes stepValve_ before Targets
  stepInflight_ = false;
  stepHold_ = Hold{};
  lastWasResync_ = false;
  pending_ &= ~kResyncCovered;
  inflight_ &= ~kResyncCovered;
  setProtocol(0);
}

void PollPlanner::setPending(uint8_t item) {
  const uint32_t bit = 1u << item;
  if (pending_ & bit) return;  // coalesced
  pending_ |= bit;
  inflight_ &= ~bit;
  itemHold_[item] = Hold{};
}

void PollPlanner::requestTempList() { setPending(kItemTempList); }
void PollPlanner::requestVoltList() { setPending(kItemVoltList); }
void PollPlanner::requestValveSensors() { setPending(kItemValveSensors); }

void PollPlanner::requestMotorParams() {
  setPending(kItemMotorChars);
  setPending(kItemLearnMovements);
  if (proto_ >= 2) setPending(kItemBreakaway);
}

void PollPlanner::requestTarget(uint8_t valve) {
  if (valve < kValveCount) setPending(static_cast<uint8_t>(kItemTarget + valve));
}

void PollPlanner::requestProfile(uint8_t valve) {
  if (valve < kValveCount && proto_ >= 2) setPending(static_cast<uint8_t>(kItemProfile + valve));
}

// ---------------------------------------------------------------- helpers

bool PollPlanner::holdExpired(const Hold& h, uint32_t nowMs) {
  return elapsedMs(nowMs, h.heldAt) >= h.holdFor;
}

void PollPlanner::hold(Hold& h, uint32_t nowMs, uint16_t ms) {
  h.heldAt = nowMs;
  h.holdFor = ms;
}

bool PollPlanner::valveRequest(uint8_t valve, RequestLine& out) const {
  return proto_ >= 2 ? buildValveEx(valve, out) : buildValveData(valve, out);
}

bool PollPlanner::buildItem(uint8_t item, RequestLine& out) const {
  if (item < kItemProfile) {  // kItemTarget == 0: the item is the valve
    return proto_ >= 2 ? buildValveEx(item, out) : buildGetTarget(item, out);
  }
  if (item < kItemTempList) return buildProfile(static_cast<uint8_t>(item - kItemProfile), out);
  switch (item) {
    case kItemTempList: return buildTempList(out);
    case kItemVoltList: return buildVoltList(out);
    case kItemValveSensors: return buildValveSensors(kAllValves, out);
    case kItemMotorChars: return buildGetMotorChars(out);
    case kItemLearnMovements: return buildGetLearnMovements(out);
    default: return buildGetBreakaway(out);  // kItemBreakaway
  }
}

bool PollPlanner::buildStep(RequestLine& out) const {
  switch (step_) {
    case ResyncStep::Proto: return buildGetProto(out);
    case ResyncStep::Version: return buildGetVersion(out);
    case ResyncStep::HwId: return buildGetHwId(out);
    case ResyncStep::MotorChars: return buildGetMotorChars(out);
    case ResyncStep::LearnMovements: return buildGetLearnMovements(out);
    case ResyncStep::Breakaway: return buildGetBreakaway(out);
    case ResyncStep::TempList: return buildTempList(out);
    case ResyncStep::VoltList: return buildVoltList(out);
    case ResyncStep::ValveSensors: return buildValveSensors(kAllValves, out);
    case ResyncStep::ValveStates: return buildValveStates(out);
    default:  // Targets (never called for Done)
      return proto_ >= 2 ? buildValveEx(stepValve_, out) : buildGetTarget(stepValve_, out);
  }
}

void PollPlanner::advanceStep() {
  stepInflight_ = false;
  stepHold_ = Hold{};
  if (step_ == ResyncStep::Done) return;
  if (step_ == ResyncStep::Targets && ++stepValve_ < kValveCount) return;
  step_ = static_cast<ResyncStep>(static_cast<uint8_t>(step_) + 1);
  stepValve_ = 0;
}

void PollPlanner::skipStepsForProtocol() {
  while ((step_ == ResyncStep::Breakaway && proto_ < 2) ||
         (step_ == ResyncStep::ValveStates && proto_ >= 2)) {
    advanceStep();
  }
}

void PollPlanner::prime(uint32_t nowMs) {
  primed_ = true;
  // Valves, gstat and sensor data are due at once; the re-sync sequence
  // already reads the counts/lists and the version.
  for (uint32_t& t : valveLastMs_) t = nowMs - cadence_.valveInactiveMs;
  statusLastMs_ = nowMs - cadence_.statusMs;
  tempLastMs_ = nowMs - cadence_.tempDataMs;
  voltLastMs_ = nowMs - cadence_.voltDataMs;
  tempCountLastMs_ = nowMs;
  voltCountLastMs_ = nowMs;
  versionLastMs_ = nowMs;
}

// ---------------------------------------------------------------- next

bool PollPlanner::nextOneShot(uint32_t nowMs, RequestLine& out) {
  // Lowest item index first: that is the one-shot priority order.
  for (uint32_t m = pending_; m != 0; m &= m - 1) {
    const uint8_t i = lowestBit(m);
    if (!holdExpired(itemHold_[i], nowMs)) continue;
    buildItem(i, out);  // cannot fail for an item index
    inflight_ |= 1u << i;
    hold(itemHold_[i], nowMs, kLostRequestMs);
    return true;
  }
  return false;
}

bool PollPlanner::nextPeriodic(uint32_t nowMs, RequestLine& out) {
  enum : uint8_t { kStatus = kValveCount, kTemp, kVolt, kTempCount, kVoltCount, kVersion, kNone };
  uint8_t best = kNone;
  int64_t bestOverdue = INT64_MIN;
  // elapsedMs() wraps after 49 days; for an item that was ineligible that
  // long (no sensors, v1 STM) this costs at most one period, never a stall.
  auto consider = [&](uint8_t id, uint32_t last, uint32_t period) {
    const uint32_t e = elapsedMs(nowMs, last);
    if (e >= period && int64_t{e - period} > bestOverdue) {  // strict: ties keep the earlier
      best = id;
      bestOverdue = e - period;
    }
  };
  // 0 busy, 1 active, 2 inactive; busy wins over the active flag.
  auto valveClass = [this](uint8_t v) -> uint8_t {
    if ((busyMask_ >> v) & 1u) return 0;
    return ((activeMask_ >> v) & 1u) ? 1 : 2;
  };
  auto considerValves = [&](uint8_t cls, uint32_t period) {
    for (uint8_t v = 0; v < kValveCount; ++v) {
      if (valveClass(v) == cls) consider(v, valveLastMs_[v], period);
    }
  };

  // Candidates in tie-break order.
  considerValves(0, cadence_.valveBusyMs);
  considerValves(1, cadence_.valveActiveMs);
  if (proto_ >= 2) consider(kStatus, statusLastMs_, cadence_.statusMs);
  if (tempCount_ > 0) consider(kTemp, tempLastMs_, cadence_.tempDataMs / tempCount_);
  if (voltCount_ > 0) consider(kVolt, voltLastMs_, cadence_.voltDataMs / voltCount_);
  consider(kTempCount, tempCountLastMs_, cadence_.sensorCountMs);
  consider(kVoltCount, voltCountLastMs_, cadence_.sensorCountMs);
  considerValves(2, cadence_.valveInactiveMs);
  consider(kVersion, versionLastMs_, cadence_.versionMs);

  switch (best) {
    case kNone:
      return false;
    case kStatus:
      statusLastMs_ = nowMs;
      return buildGetStatus(out);
    case kTemp:
      tempLastMs_ = nowMs;
      buildTempData(tempIndex_, out);
      tempIndex_ = static_cast<uint8_t>((tempIndex_ + 1) % tempCount_);
      return true;
    case kVolt:
      voltLastMs_ = nowMs;
      buildVoltData(voltIndex_, out);
      voltIndex_ = static_cast<uint8_t>((voltIndex_ + 1) % voltCount_);
      return true;
    case kTempCount:
      tempCountLastMs_ = nowMs;
      return buildTempCount(out);
    case kVoltCount:
      voltCountLastMs_ = nowMs;
      return buildVoltCount(out);
    case kVersion:
      versionLastMs_ = nowMs;
      return buildGetVersion(out);
    default:
      valveLastMs_[best] = nowMs;
      return valveRequest(best, out);
  }
}

bool PollPlanner::next(uint32_t nowMs, RequestLine& out) {
  out = RequestLine{};
  if (!primed_) prime(nowMs);

  skipStepsForProtocol();
  const bool stepReady = step_ != ResyncStep::Done && holdExpired(stepHold_, nowMs);
  if (!(stepReady && !lastWasResync_)) {
    if (nextOneShot(nowMs, out) || nextPeriodic(nowMs, out)) {
      lastWasResync_ = false;
      return true;
    }
    if (!stepReady) return false;
  }
  buildStep(out);  // cannot fail while a step is active
  stepInflight_ = true;
  hold(stepHold_, nowMs, kLostRequestMs);
  lastWasResync_ = true;
  return true;
}

// ---------------------------------------------------------------- results


void PollPlanner::onResult(const RequestLine& request, bool ok, uint32_t nowMs) {
  RequestLine expected;
  if (stepInflight_ && buildStep(expected) && sameRequest(expected, request)) {
    if (ok) {
      advanceStep();
    } else if (step_ == ResyncStep::Proto) {
      if (proto_ == 0) proto_ = 1;  // silent STM: protocol v1
      advanceStep();
    } else {
      stepInflight_ = false;
      hold(stepHold_, nowMs, cadence_.valveActiveMs);
    }
  }

  for (uint32_t m = inflight_; m != 0; m &= m - 1) {
    const uint8_t i = lowestBit(m);
    buildItem(i, expected);  // cannot fail for an item index
    if (!sameRequest(expected, request)) continue;
    const uint32_t bit = 1u << i;
    inflight_ &= ~bit;
    if (ok) {
      pending_ &= ~bit;
    } else {
      hold(itemHold_[i], nowMs, cadence_.valveActiveMs);
    }
  }
}

}  // namespace vdm
