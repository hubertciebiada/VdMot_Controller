// NetWatchdog, network evidence names.
#include <string>

#include "doctest.h"
#include "vdm/net_policy.h"

using namespace vdm;

TEST_CASE("netEvidenceName") {
  CHECK(std::string(netEvidenceName(NetEvidence::None)) == "none");
  CHECK(std::string(netEvidenceName(NetEvidence::GatewayPing)) == "ping");
  CHECK(std::string(netEvidenceName(NetEvidence::Mqtt)) == "mqtt");
  CHECK(std::string(netEvidenceName(NetEvidence::TimeSync)) == "ntp");
  CHECK(std::string(netEvidenceName(NetEvidence::InboundHttp)) == "http");
  CHECK(std::string(netEvidenceName(NetEvidence::DhcpLease)) == "dhcp");
  CHECK(std::string(netEvidenceName(static_cast<NetEvidence>(6))) == "unknown");
  CHECK(static_cast<uint8_t>(NetEvidence::DhcpLease) == 5);
}

TEST_CASE("NetWatchdog") {
  NetWatchdog off;
  CHECK_FALSE(off.update(false, 0));
  CHECK_FALSE(off.update(false, 100000000));

  NetWatchdog w;
  w.configure(5);
  CHECK_FALSE(w.update(false, 1000));  // boot counts as the start of the outage
  CHECK_FALSE(w.update(false, 1000 + 299999));
  CHECK(w.update(false, 1000 + 300000));
  CHECK_FALSE(w.update(false, 1000 + 300001));  // once
  CHECK_FALSE(w.update(true, 400000));
  CHECK_FALSE(w.update(false, 500000));
  CHECK_FALSE(w.update(false, 799999));
  CHECK(w.update(false, 800000));

  // Up at boot, lost later; up again resets the timer.
  NetWatchdog u;
  u.configure(1);
  CHECK_FALSE(u.update(true, 0));
  CHECK_FALSE(u.update(false, 10000));
  CHECK_FALSE(u.update(true, 69000));
  CHECK_FALSE(u.update(false, 70000));
  CHECK_FALSE(u.update(false, 129999));
  CHECK(u.update(false, 130000));

  // Enabled later: the outage is measured from when it started.
  NetWatchdog late;
  CHECK_FALSE(late.update(false, 0));
  CHECK_FALSE(late.update(false, 200000));
  late.configure(2);
  CHECK(late.update(false, 200001));
  // configure() re-arms after firing, with the grown wait (2 min * 4).
  late.configure(2);
  CHECK_FALSE(late.update(false, 200002));
  CHECK_FALSE(late.update(false, 479999));
  CHECK(late.update(false, 480000));
  late.configure(0);
  CHECK_FALSE(late.update(false, 999999999));

  // Maximum setting.
  NetWatchdog max;
  max.configure(255);
  CHECK_FALSE(max.update(false, 0));
  CHECK_FALSE(max.update(false, 255u * 60000u - 1));
  CHECK(max.update(false, 255u * 60000u));
}

TEST_CASE("NetWatchdog: the wait grows with every restart of one outage") {
  // Router off for hours: restarts after 5, 20, 80, 320, 1280 min, then
  // every 24 h, instead of every 5 min (each one also resets the STM).
  const uint32_t expectMin[] = {5, 20, 80, 320, 1280, 1440, 1440};
  uint8_t kept = 0;  // what the glue keeps in RTC memory across restarts
  for (uint32_t m : expectMin) {
    CAPTURE(m);
    NetWatchdog w;  // a fresh boot
    w.configure(5);
    w.setRestartsInOutage(kept);
    CHECK(w.waitMs() == m * 60000u);
    CHECK_FALSE(w.update(false, 700));
    CHECK_FALSE(w.update(false, 700 + m * 60000u - 1));
    CHECK(w.update(false, 700 + m * 60000u));
    kept = w.restartsInOutage();
  }
  CHECK(kept == 7);

  // The network came up once: the next outage starts over at 5 min.
  NetWatchdog w;
  w.configure(5);
  w.setRestartsInOutage(kept);
  CHECK_FALSE(w.update(true, 0));
  CHECK(w.restartsInOutage() == 0);
  CHECK(w.waitMs() == 300000u);
  CHECK_FALSE(w.update(false, 1000));
  CHECK(w.update(false, 301000));
  CHECK(w.restartsInOutage() == 1);

  // Saturation and the cap with the largest setting.
  NetWatchdog big;
  big.configure(240);
  big.setRestartsInOutage(255);
  CHECK(big.waitMs() == 1440u * 60000u);
  CHECK_FALSE(big.update(false, 0));
  CHECK(big.update(false, 1440u * 60000u));
  CHECK(big.restartsInOutage() == 255);
  NetWatchdog one;
  one.configure(1);
  one.setRestartsInOutage(1);
  CHECK(one.waitMs() == 4u * 60000u);
  one.configure(0);
  CHECK(one.waitMs() == 0);
  CHECK_FALSE(one.update(false, 0));
  CHECK_FALSE(one.update(false, 0xFFFFFFFFu));
}

TEST_CASE("NetWatchdog: fires once per outage even when the grown wait passes too") {
  NetWatchdog w;
  w.configure(1);
  CHECK_FALSE(w.update(false, 0));
  CHECK_FALSE(w.update(false, 59999));
  CHECK(w.update(false, 60000));
  CHECK(w.restartsInOutage() == 1);
  CHECK(w.waitMs() == 4u * 60000u);
  // Same outage, no restart happened: the grown wait (4 min) passes as well.
  CHECK_FALSE(w.update(false, 240000));
  CHECK_FALSE(w.update(false, 3600000));
  CHECK(w.restartsInOutage() == 1);
  // Network back and lost again: a new outage fires again after 1 min.
  CHECK_FALSE(w.update(true, 3600001));
  CHECK_FALSE(w.update(false, 3600002));
  CHECK(w.update(false, 3660002));
}
