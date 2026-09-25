#include "vdm/valve_scheduler.h"

namespace vdm {

namespace {

bool faulted(const ValveView& v) { return v.status == kStFailed || v.status == kStBlocked; }

// a failed or blocked valve is calibrated only on staln and by its automatic retry
bool explicitReq(const ValveView& v) {
  if (faulted(v)) return v.forcedLearn || v.retryLearn;
  return v.forcedLearn || v.retryLearn || v.earlyLearn || v.calibFlag;
}

bool anyReq(const ValveView& v) { return explicitReq(v) || (v.timedLearn && !faulted(v)); }

bool pendingCal(const ValveView& v) { return !v.calibrated || v.recal; }

// the valve has something to do: its drive target differs, a target request
// touched it, or the lease failsafe drives it
bool due(const ValveView& v) { return v.drive != v.actual || v.touched || v.leaseForced; }

uint8_t moveDir(const ValveView& v) { return v.drive > v.actual ? kDirOpen : kDirClose; }

}  // namespace

Decision ValveScheduler::next(const ValveView (&v)[kValveCount], const SchedulerInputs& in) {
  Decision d;
  if (in.safeMode || in.holdForTemperature) return d;

  // even and odd valves alternate (fewer MUX relay switches on the C1 board)
  const uint8_t t = testIndex_;
  testIndex_ = static_cast<uint8_t>(testIndex_ + 2);
  if (testIndex_ == kValveCount) {
    testIndex_ = 1;
  } else if (testIndex_ > kValveCount) {
    testIndex_ = 0;
  }
  if (v[t].status == kStUnknown) {
    d.kind = ActionKind::Test;
    d.valve = t;
    latch_[t].valid = false;
    return d;
  }

  if (step2Run_ >= kFairnessRun && step3(v, d)) {
    step2Run_ = 0;
    return d;
  }
  if (step2(v, d)) {
    if (step2Run_ < kFairnessRun) step2Run_++;
    return d;
  }
  if (step3(v, d)) step2Run_ = 0;
  return d;
}

bool ValveScheduler::moveAllowed(uint8_t i, const ValveView& v, uint8_t dir) {
  Latch& l = latch_[i];
  if (!l.valid) return true;
  if (l.drive != v.drive || l.status != v.status) {
    l.valid = false;
    return true;
  }
  return dir != l.dir || l.retry;
}

void ValveScheduler::setMove(Decision& d, uint8_t i, const ValveView& v, bool keepStatus) {
  const uint8_t dir = moveDir(v);
  d.valve = i;
  d.keepStatus = keepStatus;
  if (v.drive == 100) {
    d.kind = ActionKind::OpenEnd;
  } else if (v.drive == 0) {
    d.kind = ActionKind::CloseEnd;
  } else if (dir == kDirOpen) {
    d.kind = ActionKind::Open;
    d.delta = static_cast<uint8_t>(v.drive - v.actual);
  } else {
    d.kind = ActionKind::Close;
    d.delta = static_cast<uint8_t>(v.actual - v.drive);
  }
  if (latch_[i].valid && latch_[i].dir == dir) latch_[i].retry = false;
  pending_[i] = Pending{true, dir, v.drive, keepStatus, false};
}

bool ValveScheduler::step2(const ValveView (&v)[kValveCount], Decision& d) {
  for (uint8_t n = 0; n < kValveCount; n++) {
    const uint8_t i = static_cast<uint8_t>((rr_ + n) % kValveCount);
    const ValveView& x = v[i];
    bool found = true;
    if (x.status == kStFullOpen) {
      d.kind = ActionKind::OpenEnd;
      d.valve = i;
      pending_[i] = Pending{true, kDirOpen, x.drive, false, false};
    } else if (x.status == kStOpenCircuit && x.drive != x.actual) {
      d.kind = ActionKind::Test;
      d.valve = i;
      latch_[i].valid = false;
    } else if (x.status == kStBlocked && x.blockedFailsafe && !x.svcHold && !anyReq(x) && x.drive != x.actual &&
               moveAllowed(i, x, moveDir(x))) {
      setMove(d, i, x, true);
    } else if (x.status == kStIdle && !x.svcHold && !pendingCal(x) && !anyReq(x) && x.needsReference && due(x)) {
      // the start is not known: to the end stop nearer to the drive target first
      const uint8_t dir = x.drive >= 50 ? kDirOpen : kDirClose;
      d.kind = dir == kDirOpen ? ActionKind::OpenEnd : ActionKind::CloseEnd;
      d.valve = i;
      d.reference = true;
      latch_[i].valid = false;
      pending_[i] = Pending{true, dir, x.drive, false, true};
      firstChange_ = true;
    } else if (x.status == kStIdle && !x.svcHold && !pendingCal(x) && !anyReq(x) && !x.needsReference &&
               x.drive != x.actual && moveAllowed(i, x, moveDir(x))) {
      setMove(d, i, x, false);
      firstChange_ = true;
    } else {
      found = false;
    }
    if (found) {
      rr_ = static_cast<uint8_t>((i + 1) % kValveCount);
      return true;
    }
  }
  return false;
}

bool ValveScheduler::step3(const ValveView (&v)[kValveCount], Decision& d) {
  for (uint8_t n = 0; n < kValveCount; n++) {
    const uint8_t i = static_cast<uint8_t>((rr_ + n) % kValveCount);
    const ValveView& x = v[i];
    if (x.status == kStPresent && (firstChange_ || explicitReq(x) || due(x))) {
      d.kind = ActionKind::Learn;
      if (x.drive != x.actual) firstChange_ = true;
    } else if (x.status == kStIdle && pendingCal(x) && !x.svcHold && due(x)) {
      d.kind = ActionKind::Learn;
      firstChange_ = true;
    } else if (anyReq(x) && x.status != kStUnknown && x.status != kStPresent) {
      d.kind = ActionKind::MarkPresent;
    } else {
      continue;
    }
    d.valve = i;
    latch_[i].valid = false;
    pending_[i].valid = false;
    rr_ = static_cast<uint8_t>((i + 1) % kValveCount);
    return true;
  }
  return false;
}

void ValveScheduler::moveEnded(uint8_t valve, StopReason reason, bool early, uint8_t status) {
  if (valve >= kValveCount) return;
  const Pending p = pending_[valve];
  pending_[valve].valid = false;
  if (!p.valid) return;
  Latch& l = latch_[valve];
  const bool endStop =
      reason == StopReason::EndStop || reason == StopReason::EarlyEndStop || reason == StopReason::SafetyOvercurrent;
  if (p.reference || !endStop) {
    l.valid = false;
    return;
  }
  if (l.valid && l.dir == p.dir && l.drive == p.drive) {
    // the retry ended at the end stop again: no further move this way
    l.status = status;
    return;
  }
  l = Latch{true, p.dir, p.drive, status, early && !p.keepStatus};
}

void ValveScheduler::clearLatch(uint8_t valve) {
  if (valve < kValveCount) latch_[valve].valid = false;
}

}  // namespace vdm
