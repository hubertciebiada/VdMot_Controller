// Lease of the valve targets: while a client renews it, the valves follow their
// targets; once it expires they go to their failsafe positions (vdm/failsafe.h).
// The timeout is set by slcfg and stored in the EEPROM. Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

constexpr uint16_t kLeaseTimeoutOff = 0;  // the targets never expire
constexpr uint16_t kLeaseTimeoutMinMin = 5;
constexpr uint16_t kLeaseTimeoutMaxMin = 1440;
// timeout of an STM that no ESP 2.1 configured (or whose stored value is lost):
// the polls of a legacy or 2.0.0 ESP renew it, a dead ESP lets it expire
constexpr uint16_t kLeaseTimeoutDefaultMin = 60;
// a lease client is present while it sent a lease command within this time
constexpr uint32_t kLeaseClientIdleS = 300;

enum class LeaseState : uint8_t {
  Off = 0,      // timeout 0
  Running = 1,
  Expired = 2,  // failsafe active
};

// 0 (off) or kLeaseTimeoutMinMin..kLeaseTimeoutMaxMin minutes
bool leaseTimeoutValid(uint32_t minutes);

// Stored value at start-up: anything slcfg accepts is kept, anything else
// loads kLeaseTimeoutDefaultMin.
uint16_t sanitizeLeaseTimeout(uint16_t minutes);

// The lease itself. Time comes in as elapsed seconds (advance); all counters
// saturate, so a lease never wraps back to Running.
class Lease {
 public:
  // both counters, saturating at UINT32_MAX
  void advance(uint32_t elapsedS);
  // slcfg (validated by the caller); no renewal, except that switching the
  // lease on (0 -> non-zero) starts a fresh lease
  void setTimeout(uint16_t minutes);
  // slhbt/slcfg/sfspo/glcfg: a lease client is present
  void leaseCommand();
  // slhbt: a lease command; alive renews
  void heartbeat(bool alive);
  // gvlvd/gvlvx request: renews only while no lease client is present
  // (a legacy or 2.0.0 ESP keeps the lease alive with its polls)
  void valvePoll();

  // Off with timeout 0; Expired once timeout * 60 s passed without a renewal
  LeaseState state() const;
  // seconds to the expiry while Running, else 0
  uint32_t remainingS() const;
  // a lease command arrived within kLeaseClientIdleS
  bool clientPresent() const { return clientSeenWithin(kLeaseClientIdleS); }
  // a lease command arrived within the last s seconds
  bool clientSeenWithin(uint32_t s) const;
  uint16_t timeout() const { return timeoutMin_; }

  struct Snapshot {
    uint32_t sinceRenewalS;
    uint32_t sinceClientS;
    bool client;
  };
  Snapshot snapshot() const;
  // warm reset; the timeout is set separately
  void restore(const Snapshot& s);

 private:
  uint16_t timeoutMin_ = 0;
  uint32_t sinceRenewalS_ = 0;  // a fresh lease at start-up
  uint32_t sinceClientS_ = 0;
  bool client_ = false;
};

}  // namespace vdm
