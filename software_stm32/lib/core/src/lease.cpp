#include "vdm/lease.h"

namespace vdm {

bool leaseTimeoutValid(uint32_t minutes) {
  return minutes == kLeaseTimeoutOff || (minutes >= kLeaseTimeoutMinMin && minutes <= kLeaseTimeoutMaxMin);
}

uint16_t sanitizeLeaseTimeout(uint16_t minutes) {
  return leaseTimeoutValid(minutes) ? minutes : kLeaseTimeoutDefaultMin;
}

namespace {

uint32_t addSaturating(uint32_t a, uint32_t b) { return a > UINT32_MAX - b ? UINT32_MAX : a + b; }

}  // namespace

void Lease::advance(uint32_t elapsedS) {
  sinceRenewalS_ = addSaturating(sinceRenewalS_, elapsedS);
  sinceClientS_ = addSaturating(sinceClientS_, elapsedS);
}

void Lease::setTimeout(uint16_t minutes) {
  if (timeoutMin_ == kLeaseTimeoutOff && minutes != kLeaseTimeoutOff) sinceRenewalS_ = 0;
  timeoutMin_ = minutes;
}

void Lease::leaseCommand() {
  client_ = true;
  sinceClientS_ = 0;
}

void Lease::heartbeat(bool alive) {
  leaseCommand();
  if (alive) sinceRenewalS_ = 0;
}

void Lease::valvePoll() {
  if (!clientPresent()) sinceRenewalS_ = 0;
}

LeaseState Lease::state() const {
  if (timeoutMin_ == kLeaseTimeoutOff) return LeaseState::Off;
  return sinceRenewalS_ >= static_cast<uint32_t>(timeoutMin_) * 60 ? LeaseState::Expired : LeaseState::Running;
}

uint32_t Lease::remainingS() const {
  if (state() != LeaseState::Running) return 0;
  return static_cast<uint32_t>(timeoutMin_) * 60 - sinceRenewalS_;
}

bool Lease::clientSeenWithin(uint32_t s) const { return client_ && sinceClientS_ < s; }

Lease::Snapshot Lease::snapshot() const { return Snapshot{sinceRenewalS_, sinceClientS_, client_}; }

void Lease::restore(const Snapshot& s) {
  sinceRenewalS_ = s.sinceRenewalS;
  sinceClientS_ = s.sinceClientS;
  client_ = s.client;
}

}  // namespace vdm
