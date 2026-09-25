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

}  // namespace vdm
