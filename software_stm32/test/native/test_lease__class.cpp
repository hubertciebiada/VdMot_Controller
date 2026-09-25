// vdm::Lease: expiry, renewal sources, client window, saturation, warm restore.
#include <stdint.h>

#include "doctest.h"
#include "vdm/lease.h"

using vdm::Lease;
using vdm::LeaseState;

TEST_CASE("Lease: timeout 0 is Off whatever happens") {
  Lease l;
  CHECK(l.timeout() == 0);
  CHECK(l.state() == LeaseState::Off);
  l.advance(100000);
  CHECK(l.state() == LeaseState::Off);
  CHECK(l.remainingS() == 0);
  l.heartbeat(true);
  l.valvePoll();
  CHECK(l.state() == LeaseState::Off);
  CHECK(l.remainingS() == 0);
}

TEST_CASE("Lease: runs down to the second, expires and is renewed by slhbt 1 only") {
  Lease l;
  l.setTimeout(5);
  CHECK(l.timeout() == 5);
  CHECK(l.state() == LeaseState::Running);
  CHECK(l.remainingS() == 300);
  l.advance(299);
  CHECK(l.state() == LeaseState::Running);
  CHECK(l.remainingS() == 1);
  l.advance(1);
  CHECK(l.state() == LeaseState::Expired);
  CHECK(l.remainingS() == 0);
  l.heartbeat(false);
  CHECK(l.state() == LeaseState::Expired);
  CHECK(l.clientPresent());
  l.heartbeat(true);
  CHECK(l.state() == LeaseState::Running);
  CHECK(l.remainingS() == 300);
}

TEST_CASE("Lease: a valve poll renews only while no lease client is present") {
  Lease l;
  l.setTimeout(5);
  l.advance(200);
  CHECK_FALSE(l.clientPresent());
  l.valvePoll();
  CHECK(l.remainingS() == 300);
  l.leaseCommand();
  CHECK(l.clientPresent());
  l.advance(200);
  l.valvePoll();
  CHECK(l.remainingS() == 100);
  l.advance(99);
  CHECK(l.clientPresent());  // 299 s since the lease command
  l.valvePoll();
  CHECK(l.remainingS() == 1);
  l.advance(1);
  CHECK_FALSE(l.clientPresent());  // 300 s
  CHECK(l.state() == LeaseState::Expired);
  l.valvePoll();
  CHECK(l.state() == LeaseState::Running);
  CHECK(l.remainingS() == 300);
}

TEST_CASE("Lease: setTimeout renews only when the lease is switched on") {
  Lease off;
  off.advance(7200);
  off.setTimeout(60);
  CHECK(off.state() == LeaseState::Running);
  CHECK(off.remainingS() == 3600);

  Lease on;
  on.setTimeout(30);
  on.advance(1000);
  on.setTimeout(60);
  CHECK(on.remainingS() == 3600 - 1000);
  on.setTimeout(5);
  CHECK(on.state() == LeaseState::Expired);
  on.setTimeout(0);
  CHECK(on.state() == LeaseState::Off);
  on.setTimeout(0);
  CHECK(on.state() == LeaseState::Off);
  on.setTimeout(5);
  CHECK(on.remainingS() == 300);
}

TEST_CASE("Lease: expiry is exactly timeout * 60 s for the largest timeout") {
  Lease l;
  l.setTimeout(1440);
  l.advance(86399);
  CHECK(l.remainingS() == 1);
  l.advance(1);
  CHECK(l.state() == LeaseState::Expired);
}

TEST_CASE("Lease: the client window and clientSeenWithin") {
  Lease l;
  CHECK_FALSE(l.clientSeenWithin(86400));
  CHECK_FALSE(l.clientPresent());
  l.advance(10);
  l.leaseCommand();
  l.advance(86399);
  CHECK(l.clientSeenWithin(86400));
  CHECK_FALSE(l.clientSeenWithin(86399));
  l.advance(1);
  CHECK_FALSE(l.clientSeenWithin(86400));
  l.leaseCommand();
  CHECK(l.clientSeenWithin(1));
}

TEST_CASE("Lease: the counters saturate instead of wrapping") {
  Lease l;
  l.setTimeout(1440);
  l.leaseCommand();
  l.advance(UINT32_MAX - 5);
  CHECK(l.snapshot().sinceRenewalS == UINT32_MAX - 5);
  l.advance(5);
  CHECK(l.snapshot().sinceRenewalS == UINT32_MAX);
  l.advance(10);
  CHECK(l.state() == LeaseState::Expired);
  CHECK(l.snapshot().sinceRenewalS == UINT32_MAX);
  CHECK(l.snapshot().sinceClientS == UINT32_MAX);
  l.advance(UINT32_MAX);
  CHECK(l.snapshot().sinceRenewalS == UINT32_MAX);
  CHECK_FALSE(l.clientPresent());
}

TEST_CASE("Lease: snapshot and restore round trip") {
  Lease a;
  a.setTimeout(5);
  a.advance(120);
  a.heartbeat(false);
  a.advance(7);
  const Lease::Snapshot s = a.snapshot();
  CHECK(s.sinceRenewalS == 127);
  CHECK(s.sinceClientS == 7);
  CHECK(s.client);
  Lease b;
  b.setTimeout(5);
  b.restore(s);
  CHECK(b.remainingS() == 173);
  CHECK(b.clientPresent());
  b.restore(Lease::Snapshot{400, 300, false});
  CHECK(b.state() == LeaseState::Expired);
  CHECK_FALSE(b.clientPresent());
  CHECK(b.snapshot().sinceClientS == 300);
  CHECK_FALSE(b.snapshot().client);
}
