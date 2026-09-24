// STM link request scheduler: fixed-capacity priority queue, exactly one
// outstanding request, per-request timeout and retry, consecutive-failure
// tracking, STM reset decision (architecture R6) and STM reboot detection.
// Hardware-free: the stm_link glue task feeds bytes/lines and time in and
// writes the returned request lines to the UART.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/stm_codec.h"

namespace vdm {

enum class Priority : uint8_t { User = 0, Config = 1, Poll = 2 };

enum class LinkState : uint8_t {
  Unknown,    // nothing received since ESP boot
  Up,         // last request answered
  Degraded,   // >= 1 consecutive timed-out attempt
  Down,       // >= downAfter consecutive timed-out attempts
  Booting,    // STM was reset by us; requests held for bootHoldoffMs
  Suspended,  // UART owned by the flasher; nothing is sent
};
const char* linkStateName(LinkState s);  // "unknown","up",... never null

// Binding numbers (DESIGN.md "STM link policy"). Timeouts are measured from
// the moment the request was handed to the UART (onSent()).
struct LinkParams {
  uint16_t timeoutMs = 400;          // normal requests
  uint16_t longTimeoutMs = 1500;     // list replies: gonec 255, gowvc 255, gvlon 255, gprof
  uint16_t slowTimeoutMs = 3000;     // stons, masns, stdet, reset, smotc, stvls (EEPROM/1-Wire work)
  uint8_t retries = 2;               // extra attempts for idempotent commands
  uint16_t interRequestGapMs = 5;    // quiet time after a reply/timeout before the next send
  uint16_t bootHoldoffMs = 5000;     // after STM reset: STM boot window (~3.6 s) + 1-Wire
  uint8_t downAfter = 5;             // consecutive timed-out attempts -> Down
  uint8_t resetMinTimeouts = 5;      // R6: >= 5 consecutive timeouts ...
  uint32_t resetMinSpanMs = 60000;   // ... spanning >= 60 s ...
  uint32_t resetMinIntervalMs = 600000;  // ... and at most one policy reset per 10 min
};

enum class Outcome : uint8_t {
  Ok,        // matching reply received (for acks: success form)
  Rejected,  // matching reply in its error form ("smotc err", "svmov v err n", "goned 0", gvlon error)
  Timeout,   // no matching reply after all attempts
};

// Result of one request, reported exactly once per dequeued request.
struct Completion {
  RequestLine request;
  Priority priority = Priority::Poll;
  uint16_t tag = 0;       // caller's correlation id (0 = none)
  Outcome outcome = Outcome::Timeout;
  uint8_t attempts = 0;   // 1 + retries used
};

enum class EnqueueResult : uint8_t {
  Queued,     // appended
  Coalesced,  // merged into an equal queued request (see enqueue())
  Full,       // no room even after evicting Poll entries
  Invalid,    // request.len == 0 or cmd None
};

struct LinkStats {
  uint32_t sent = 0;              // attempts written to the UART
  uint32_t answered = 0;          // completions with Ok/Rejected
  uint32_t timeouts = 0;          // timed-out attempts (not requests)
  uint32_t failedRequests = 0;    // completions with Timeout
  uint32_t strayLines = 0;        // valid replies that matched no outstanding request
  uint32_t parseErrors = 0;       // lines rejected by parseReply
  uint32_t queueFull = 0;         // enqueue() == Full
  uint32_t evictions = 0;         // Poll requests dropped to make room
  uint32_t policyResets = 0;      // STM resets decided by shouldResetStm()
  uint32_t userResets = 0;        // STM resets requested by the user
  uint8_t consecutiveTimeouts = 0;
  uint32_t lastReplyMs = 0;       // nowMs of the last matching reply
};

class LinkPolicy {
 public:
  static constexpr uint8_t kQueueCapacity = 24;

  explicit LinkPolicy(const LinkParams& params = LinkParams());

  // Queues a request. Ordering: by priority (User before Config before Poll),
  // FIFO within a priority. Coalescing, so repeated clicks/polls cannot fill
  // the queue:
  //  - stgtp for a valve already queued: the queued line is replaced by the
  //    new one (latest target wins), position kept, tag updated, retry count
  //    restarted (a queued retry becomes a fresh request) -> Coalesced;
  //  - any other byte-identical line already queued -> Coalesced (the higher
  //    priority of the two is kept; a non-zero tag replaces the queued one).
  // A raised entry moves behind the entries already queued at its new
  // priority. The outstanding request is never coalesced with.
  // When the queue is full, the newest Poll entry is evicted to make room for
  // a User/Config request; a Poll request never evicts anything -> Full.
  // Allowed in every state; while Booting/Suspended requests wait.
  EnqueueResult enqueue(const RequestLine& request, Priority priority, uint16_t tag = 0);

  // Returns the next line to write to the UART, or nullptr when a request is
  // outstanding, the queue is empty, the inter-request gap has not passed,
  // or the link is Booting/Suspended. The returned pointer stays valid until
  // the request completes. The caller must call onSent() right after the
  // write (the timeout starts there).
  const RequestLine* nextToSend(uint32_t nowMs);
  void onSent(uint32_t nowMs);

  // Feeds a parsed line. Returns true and fills `out` when it completes the
  // outstanding request (replyMatches). A valid reply that matches nothing is
  // counted as stray and returns false (the caller may still apply
  // self-identifying data: gvlvd, gvlvx, gtgtp, goned, gowvd, gprof). Any
  // matching reply resets the consecutive-timeout counter and makes the link
  // Up.
  bool onReply(const Reply& reply, uint32_t nowMs, Completion& out);
  // A line that parseReply rejected: counted only; it neither completes nor
  // proves the link (noise on the UART must not look healthy).
  void onParseError(uint32_t nowMs);

  // Timeout handling; call every loop iteration. When the outstanding attempt
  // expired: an idempotent request with attempts left is re-sent (it goes
  // back to the head of its priority; on a full queue the newest Poll entry
  // is evicted for it) and false is returned; otherwise (or when nothing can
  // be evicted) the request completes with Outcome::Timeout -> true, `out`
  // filled.
  // A gproto probe (v2 feature detection) never counts toward
  // consecutiveTimeouts because a v1 STM stays silent by design.
  bool poll(uint32_t nowMs, Completion& out);

  // R6: true when consecutiveTimeouts >= resetMinTimeouts AND the first of
  // them is >= resetMinSpanMs old AND no policy reset happened in the last
  // resetMinIntervalMs AND not Booting/Suspended.
  bool shouldResetStm(uint32_t nowMs) const;
  // Must be called when the glue has pulsed NRST (by policy or by the user).
  // Drops the outstanding request (no completion), removes all Poll entries,
  // keeps User/Config entries, clears the failure counters and enters
  // Booting for bootHoldoffMs; afterwards the state is Unknown until the
  // first matching reply.
  void onStmReset(uint32_t nowMs, bool byPolicy);

  // ESP boot: the IO15 strap pull-up holds NRST while the ESP boots
  // (specs/06 §5.2), so the STM is most likely starting up as well. Enters
  // Booting for bootHoldoffMs like onStmReset(), but counts no reset and
  // keeps the queue. Without it the first gproto probe lands in the STM's
  // start-up window, times out and selects protocol v1 for a v2 STM.
  void holdAfterEspBoot(uint32_t nowMs);

  // Flasher takes/returns the UART. suspend() drops the outstanding request
  // and all queued entries (they are stale after a re-flash) and returns how
  // many were dropped; resume() enters Booting (the flasher reset the STM).
  size_t suspend();
  void resume(uint32_t nowMs);

  LinkState state(uint32_t nowMs) const;
  const LinkStats& stats() const { return stats_; }
  size_t queued() const { return count_; }
  size_t queued(Priority p) const;
  bool busy() const { return outstanding_; }

 private:
  // queue_[0..count_) is kept sorted by priority, FIFO within a priority.
  struct Entry {
    RequestLine line;
    Priority priority = Priority::Poll;
    uint16_t tag = 0;
    uint8_t attempts = 0;  // attempts already made (retries re-queue with > 0)
  };
  uint16_t timeoutFor(const RequestLine& r) const;
  bool bootHoldActive(uint32_t nowMs) const;
  void insertAt(size_t pos, const Entry& e);
  void removeAt(size_t pos);
  // Index where an entry of priority p goes: after the last entry with the
  // same or higher priority (atHead=false) or before the first entry of the
  // same or lower priority (atHead=true, used for retries).
  size_t insertPos(Priority p, bool atHead) const;
  // Frees one slot when the queue is full by evicting the newest Poll entry.
  bool makeRoom();
  void startHold(uint32_t nowMs);
  void complete(Outcome outcome, uint32_t nowMs, Completion& out);

  LinkParams params_;
  Entry queue_[kQueueCapacity];
  uint8_t count_ = 0;
  Entry current_{};
  bool outstanding_ = false;
  uint32_t sentAtMs_ = 0;
  bool quietActive_ = false;
  uint32_t quietSinceMs_ = 0;
  bool haveHeard_ = false;
  bool suspended_ = false;
  bool booting_ = false;
  uint32_t bootAtMs_ = 0;
  uint32_t firstTimeoutMs_ = 0;
  bool policyResetDone_ = false;
  uint32_t lastPolicyResetMs_ = 0;
  LinkStats stats_{};
};

// Detects that the STM rebooted underneath us (power glitch, watchdog, reset
// by the user at the board) so the ESP can re-sync: re-read parameters and
// re-push the desired targets.
class RebootDetector {
 public:
  // v2: gstat. True when uptime decreased or the reset counter changed
  // compared with the previous gstat (the first gstat only primes).
  bool onStatus(const StmStatus& s);
  // v1 heuristic (spec 01 §10): true when a valve that previously reported
  // openCount>0 or closeCount>0 now reports openCount==0 && closeCount==0 &&
  // moves==0 with status Unknown(5), Connected(8) or NoValve(6).
  bool onValveData(const ValveData& d);
  // True on the transition Down -> Up (the STM may have been power cycled
  // while silent; re-sync is cheap and always safe).
  bool onLinkState(LinkState prev, LinkState cur);
  // Forget history (after an ESP-initiated reset or a re-flash, where the
  // reboot is known).
  void reset();

 private:
  bool haveStatus_ = false;
  StmStatus last_{};
  bool calibrated_[kValveCount] = {false};
};

}  // namespace vdm
