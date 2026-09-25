#include <stdint.h>

#include "doctest.h"
#include "vdm/protection_guard.h"

using vdm::ProtectionGuard;

TEST_CASE("ProtectionGuard: constants; the limits report only until measured on the hardware") {
  CHECK(vdm::kProtectTripValves == 3);
  CHECK(vdm::kProtectWindowS == 600);
  CHECK_FALSE(vdm::kProtectEnforce);
}

TEST_CASE("ProtectionGuard: trips of 2 valves within 600 s do not suspend") {
  ProtectionGuard g;
  CHECK_FALSE(g.suspended());
  CHECK_FALSE(g.onTrip(0, 100));
  CHECK_FALSE(g.onTrip(1, 200));
  CHECK_FALSE(g.onTrip(1, 300));
  CHECK_FALSE(g.suspended());
}

TEST_CASE("ProtectionGuard: trips of 3 valves within 600 s suspend until the next start") {
  ProtectionGuard g;
  CHECK_FALSE(g.onTrip(3, 1000));
  CHECK_FALSE(g.onTrip(7, 1300));
  CHECK(g.onTrip(11, 1599));
  CHECK(g.suspended());
  CHECK(g.onTrip(3, 100000));
}

TEST_CASE("ProtectionGuard: 3 trips of one valve do not suspend") {
  ProtectionGuard g;
  CHECK_FALSE(g.onTrip(5, 10));
  CHECK_FALSE(g.onTrip(5, 20));
  CHECK_FALSE(g.onTrip(5, 30));
  CHECK_FALSE(g.suspended());
}

TEST_CASE("ProtectionGuard: the window boundary is 599 s in, 600 s out") {
  ProtectionGuard in;
  in.onTrip(0, 0);
  in.onTrip(1, 0);
  CHECK(in.onTrip(2, 599));
  ProtectionGuard out;
  out.onTrip(0, 0);
  out.onTrip(1, 0);
  CHECK_FALSE(out.onTrip(2, 600));
  // the older trips left the window, a later trip of them counts again
  CHECK_FALSE(out.onTrip(3, 1300));
  CHECK_FALSE(out.onTrip(0, 1300));
  CHECK(out.onTrip(4, 1301));
}

TEST_CASE("ProtectionGuard: a valve that never tripped does not count with uptime 0") {
  ProtectionGuard g;
  CHECK_FALSE(g.onTrip(0, 0));
  CHECK_FALSE(g.onTrip(1, 0));
  CHECK_FALSE(g.suspended());
  CHECK_FALSE(g.onTrip(12, 0));  // no such valve
  CHECK(g.onTrip(11, 0));
}
