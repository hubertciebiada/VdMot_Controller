#include <stdint.h>

#include "doctest.h"
#include "vdm/lease.h"

TEST_CASE("lease: timeout bounds, default and client idle time") {
  CHECK(vdm::kLeaseTimeoutOff == 0);
  CHECK(vdm::kLeaseTimeoutMinMin == 5);
  CHECK(vdm::kLeaseTimeoutMaxMin == 1440);
  CHECK(vdm::kLeaseTimeoutDefaultMin == 60);
  CHECK(vdm::kLeaseClientIdleS == 300);
  // gstax field 7
  CHECK(static_cast<uint8_t>(vdm::LeaseState::Off) == 0);
  CHECK(static_cast<uint8_t>(vdm::LeaseState::Running) == 1);
  CHECK(static_cast<uint8_t>(vdm::LeaseState::Expired) == 2);
}

TEST_CASE("leaseTimeoutValid: 0 (off) and 5..1440 minutes") {
  CHECK(vdm::leaseTimeoutValid(0));
  CHECK_FALSE(vdm::leaseTimeoutValid(1));
  CHECK_FALSE(vdm::leaseTimeoutValid(4));
  CHECK(vdm::leaseTimeoutValid(5));
  CHECK(vdm::leaseTimeoutValid(6));
  CHECK(vdm::leaseTimeoutValid(60));
  CHECK(vdm::leaseTimeoutValid(1439));
  CHECK(vdm::leaseTimeoutValid(1440));
  CHECK_FALSE(vdm::leaseTimeoutValid(1441));
  CHECK_FALSE(vdm::leaseTimeoutValid(0xFFFF));
  // the request value is 32 bit: no truncation to 16 bit
  CHECK_FALSE(vdm::leaseTimeoutValid(0x10000));
  CHECK_FALSE(vdm::leaseTimeoutValid(0x10000 + 60));
  CHECK_FALSE(vdm::leaseTimeoutValid(UINT32_MAX));
}

TEST_CASE("sanitizeLeaseTimeout: what slcfg accepts is kept, anything else loads 60") {
  CHECK(vdm::sanitizeLeaseTimeout(0) == 0);
  CHECK(vdm::sanitizeLeaseTimeout(1) == 60);
  CHECK(vdm::sanitizeLeaseTimeout(3) == 60);
  CHECK(vdm::sanitizeLeaseTimeout(4) == 60);
  CHECK(vdm::sanitizeLeaseTimeout(5) == 5);
  CHECK(vdm::sanitizeLeaseTimeout(30) == 30);
  CHECK(vdm::sanitizeLeaseTimeout(1440) == 1440);
  CHECK(vdm::sanitizeLeaseTimeout(1441) == 60);
  CHECK(vdm::sanitizeLeaseTimeout(0xFFFF) == 60);  // erased EEPROM
}
