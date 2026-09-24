#include "vdm/link_policy.h"

#include <string.h>

namespace vdm {

namespace {

bool sameLine(const RequestLine& a, const RequestLine& b) {
  return a.len == b.len && memcmp(a.text, b.text, a.len) == 0;
}

// A matching reply in its error form completes the request as Rejected.
bool isRejection(const Reply& r) {
  switch (r.cmd) {
    case Cmd::Smotc:
    case Cmd::Scalx:
      return r.ack.error;
    case Cmd::Svmov:
      return !r.serviceMove.ok;
    case Cmd::Goned:
      return !r.tempData.valid;
    case Cmd::Gowvd:
      return !r.voltData.valid;
    case Cmd::Gvlon:
      return r.gvlonError;
    default:
      return false;
  }
}

}  // namespace

const char* linkStateName(LinkState s) {
  switch (s) {
    case LinkState::Unknown: return "unknown";
    case LinkState::Up: return "up";
    case LinkState::Degraded: return "degraded";
    case LinkState::Down: return "down";
    case LinkState::Booting: return "booting";
    case LinkState::Suspended: return "suspended";
  }
  return "unknown";
}

LinkPolicy::LinkPolicy(const LinkParams& params) : params_(params) {}

// ---------------------------------------------------------------- queue

size_t LinkPolicy::insertPos(Priority p, bool atHead) const {
  size_t i = 0;
  while (i < count_ && (atHead ? queue_[i].priority < p : queue_[i].priority <= p)) ++i;
  return i;
}

void LinkPolicy::insertAt(size_t pos, const Entry& e) {
  for (size_t i = count_; i > pos; --i) queue_[i] = queue_[i - 1];
  queue_[pos] = e;
  ++count_;
}

void LinkPolicy::removeAt(size_t pos) {
  for (size_t i = pos + 1; i < count_; ++i) queue_[i - 1] = queue_[i];
  --count_;
}

bool LinkPolicy::makeRoom() {
  if (count_ < kQueueCapacity) return true;
  // Sorted by priority: the newest Poll entry, if any, is the last one.
  if (queue_[count_ - 1].priority != Priority::Poll) return false;
  --count_;
  ++stats_.evictions;
  return true;
}

EnqueueResult LinkPolicy::enqueue(const RequestLine& request, Priority priority, uint16_t tag) {
  if (request.len == 0 || request.len > kRequestMaxLen || request.cmd == Cmd::None ||
      static_cast<uint8_t>(request.cmd) >= kCmdCount || priority > Priority::Poll) {
    return EnqueueResult::Invalid;
  }

  for (size_t i = 0; i < count_; ++i) {
    Entry& e = queue_[i];
    const bool sameTarget = request.cmd == Cmd::Stgtp && e.line.cmd == Cmd::Stgtp &&
                            e.line.valve == request.valve;
    if (!sameTarget && !sameLine(e.line, request)) continue;
    if (sameTarget) {
      e.line = request;  // latest target wins, as a new request with its own retries
      e.tag = tag;
      e.attempts = 0;
    } else if (tag != 0) {
      e.tag = tag;
    }
    if (priority < e.priority) {
      Entry moved = e;
      moved.priority = priority;
      removeAt(i);
      insertAt(insertPos(priority, false), moved);
    }
    return EnqueueResult::Coalesced;
  }

  if (count_ == kQueueCapacity && (priority == Priority::Poll || !makeRoom())) {
    ++stats_.queueFull;
    return EnqueueResult::Full;
  }
  Entry e;
  e.line = request;
  e.priority = priority;
  e.tag = tag;
  insertAt(insertPos(priority, false), e);
  return EnqueueResult::Queued;
}

size_t LinkPolicy::queued(Priority p) const {
  size_t n = 0;
  for (size_t i = 0; i < count_; ++i) n += queue_[i].priority == p ? 1 : 0;
  return n;
}

// ---------------------------------------------------------------- sending

uint16_t LinkPolicy::timeoutFor(const RequestLine& r) const {
  switch (r.cmd) {
    case Cmd::Gonec:
    case Cmd::Gowvc:
      return r.arg == kAllValves ? params_.longTimeoutMs : params_.timeoutMs;
    case Cmd::Gvlon:
      return r.valve == kAllValves ? params_.longTimeoutMs : params_.timeoutMs;
    case Cmd::Gprof:
      return params_.longTimeoutMs;
    case Cmd::Stons:
    case Cmd::Masns:
    case Cmd::Stdet:
    case Cmd::Reset:
    case Cmd::Smotc:
    case Cmd::Stvls:
      return params_.slowTimeoutMs;
    default:
      return params_.timeoutMs;
  }
}

bool LinkPolicy::bootHoldActive(uint32_t nowMs) const {
  return booting_ && elapsedMs(nowMs, bootAtMs_) < params_.bootHoldoffMs;
}

const RequestLine* LinkPolicy::nextToSend(uint32_t nowMs) {
  if (suspended_ || outstanding_ || count_ == 0) return nullptr;
  if (bootHoldActive(nowMs)) return nullptr;
  booting_ = false;
  if (quietActive_ && elapsedMs(nowMs, quietSinceMs_) < params_.interRequestGapMs) return nullptr;

  current_ = queue_[0];
  removeAt(0);
  if (current_.attempts < 0xFF) ++current_.attempts;
  outstanding_ = true;
  sentAtMs_ = nowMs;  // onSent() refines it; also covers a caller that forgets
  return &current_.line;
}

void LinkPolicy::onSent(uint32_t nowMs) {
  if (!outstanding_) return;
  sentAtMs_ = nowMs;
  ++stats_.sent;
}

void LinkPolicy::complete(Outcome outcome, uint32_t nowMs, Completion& out) {
  out.request = current_.line;
  out.priority = current_.priority;
  out.tag = current_.tag;
  out.outcome = outcome;
  out.attempts = current_.attempts;
  outstanding_ = false;
  quietActive_ = true;
  quietSinceMs_ = nowMs;
}

// ---------------------------------------------------------------- receiving

bool LinkPolicy::onReply(const Reply& reply, uint32_t nowMs, Completion& out) {
  if (!outstanding_ || !replyMatches(current_.line, reply)) {
    ++stats_.strayLines;
    return false;
  }
  complete(isRejection(reply) ? Outcome::Rejected : Outcome::Ok, nowMs, out);
  ++stats_.answered;
  stats_.consecutiveTimeouts = 0;
  stats_.lastReplyMs = nowMs;
  haveHeard_ = true;
  return true;
}

void LinkPolicy::onParseError(uint32_t) { ++stats_.parseErrors; }

bool LinkPolicy::poll(uint32_t nowMs, Completion& out) {
  if (booting_ && !bootHoldActive(nowMs)) booting_ = false;
  // Retire the reset rate limit while it is still measurable (elapsedMs
  // wraps after 49 days).
  if (policyResetDone_ && elapsedMs(nowMs, lastPolicyResetMs_) >= params_.resetMinIntervalMs) {
    policyResetDone_ = false;
  }
  if (!outstanding_ || elapsedMs(nowMs, sentAtMs_) < timeoutFor(current_.line)) return false;

  ++stats_.timeouts;
  if (current_.line.cmd != Cmd::Gproto) {
    if (stats_.consecutiveTimeouts == 0) firstTimeoutMs_ = nowMs;
    if (stats_.consecutiveTimeouts < 0xFF) ++stats_.consecutiveTimeouts;
  }

  if (cmdIsIdempotent(current_.line.cmd) && current_.attempts <= params_.retries && makeRoom()) {
    outstanding_ = false;
    quietActive_ = true;
    quietSinceMs_ = nowMs;
    insertAt(insertPos(current_.priority, true), current_);
    return false;
  }
  complete(Outcome::Timeout, nowMs, out);
  ++stats_.failedRequests;
  return true;
}

// ---------------------------------------------------------------- resets

bool LinkPolicy::shouldResetStm(uint32_t nowMs) const {
  // Booting needs no check: entering it clears consecutiveTimeouts and
  // nothing is sent until it ends.
  const uint8_t n = stats_.consecutiveTimeouts;
  if (suspended_ || n == 0 || n < params_.resetMinTimeouts) return false;
  if (elapsedMs(nowMs, firstTimeoutMs_) < params_.resetMinSpanMs) return false;
  return !policyResetDone_ || elapsedMs(nowMs, lastPolicyResetMs_) >= params_.resetMinIntervalMs;
}

void LinkPolicy::startHold(uint32_t nowMs) {
  outstanding_ = false;
  booting_ = true;
  bootAtMs_ = nowMs;
  quietActive_ = false;
  haveHeard_ = false;
  stats_.consecutiveTimeouts = 0;
}

void LinkPolicy::onStmReset(uint32_t nowMs, bool byPolicy) {
  size_t kept = 0;
  for (size_t i = 0; i < count_; ++i) {
    if (queue_[i].priority != Priority::Poll) queue_[kept++] = queue_[i];
  }
  count_ = static_cast<uint8_t>(kept);
  if (byPolicy) {
    ++stats_.policyResets;
    policyResetDone_ = true;
    lastPolicyResetMs_ = nowMs;
  } else {
    ++stats_.userResets;
  }
  startHold(nowMs);
}

size_t LinkPolicy::suspend() {
  const size_t dropped = count_ + (outstanding_ ? 1u : 0u);
  count_ = 0;
  outstanding_ = false;
  suspended_ = true;
  return dropped;
}

void LinkPolicy::resume(uint32_t nowMs) {
  suspended_ = false;
  startHold(nowMs);
}

LinkState LinkPolicy::state(uint32_t nowMs) const {
  if (suspended_) return LinkState::Suspended;
  if (bootHoldActive(nowMs)) return LinkState::Booting;
  const uint8_t n = stats_.consecutiveTimeouts;
  if (n > 0 && n >= params_.downAfter) return LinkState::Down;
  if (n > 0) return LinkState::Degraded;
  return haveHeard_ ? LinkState::Up : LinkState::Unknown;
}

// ---------------------------------------------------------------- reboot

bool RebootDetector::onStatus(const StmStatus& s) {
  const bool rebooted = haveStatus_ && (s.uptimeS < last_.uptimeS || s.resets != last_.resets);
  haveStatus_ = true;
  last_ = s;
  return rebooted;
}

bool RebootDetector::onValveData(const ValveData& d) {
  if (d.valve >= kValveCount) return false;
  if (d.openCount > 0 || d.closeCount > 0) {
    calibrated_[d.valve] = true;
    return false;
  }
  const uint8_t st = d.status;
  const bool bootState = st == static_cast<uint8_t>(ValveStatus::Unknown) ||
                         st == static_cast<uint8_t>(ValveStatus::NoValve) ||
                         st == static_cast<uint8_t>(ValveStatus::Connected);
  if (!calibrated_[d.valve] || d.moves != 0 || !bootState) return false;
  // Every valve lost its counts in the reboot: forget all of them so the
  // other valves' first post-reboot replies do not report it again.
  for (bool& c : calibrated_) c = false;
  return true;
}

bool RebootDetector::onLinkState(LinkState prev, LinkState cur) {
  return prev == LinkState::Down && cur == LinkState::Up;
}

void RebootDetector::reset() {
  haveStatus_ = false;
  last_ = StmStatus{};
  for (bool& c : calibrated_) c = false;
}

}  // namespace vdm
