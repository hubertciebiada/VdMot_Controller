// Smoke tests of src/net.cpp: interfaces, state events, WiFi fallback, time, watchdog restarts.
#include <ETH.h>
#include <IPAddress.h>
#include <WiFi.h>
#include <time.h>

#include "glue_test.h"
#include "net.h"

namespace {

vdm::Config config() {
  vdm::Config c;
  vdm::setDefaults(c);
  vdm::copyString(c.station, sizeof c.station, "Heating Floor");
  return c;
}

void ethernetUp(uint32_t ip) {
  fakes::net().ethIp = ip;
  fakes::net().ethMask = IPAddress(255, 255, 255, 0);
  fakes::net().fire(ARDUINO_EVENT_ETH_CONNECTED);
  fakes::net().fire(ARDUINO_EVENT_ETH_GOT_IP);
}

// Network down from the current time; true when a restart was requested by `untilMs`.
bool restartBy(uint32_t fromMs, uint32_t untilMs) {
  for (uint32_t t = fromMs; t <= untilMs; t += 1000) {
    net::service(t, false);
    if (!sib::ota().restartRequests.empty()) return true;
  }
  return false;
}

}  // namespace

TEST_CASE("net begin: Ethernet with the WT32-ETH01 PHY wiring") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  REQUIRE(fakes::net().ethBegins.size() == 1);
  const fakes::EthBegin& b = fakes::net().ethBegins[0];
  CHECK(b.phyAddr == 1);
  CHECK(b.power == 16);
  CHECK(b.mdc == 23);
  CHECK(b.mdio == 18);
  CHECK(b.type == ETH_PHY_LAN8720);
  CHECK(b.clk == ETH_CLOCK_GPIO0_IN);
  CHECK(fakes::net().ethConfigs.empty());  // DHCP
  CHECK(fakes::net().wifiBegins == 0);
  char host[64];
  vdm::buildHostname("Heating Floor", host, sizeof host);
  CHECK(std::string(net::hostname()) == host);
}

TEST_CASE("net begin: a static address is configured on Ethernet") {
  glue::begin();
  vdm::Config c = config();
  c.net.dhcp = false;
  c.net.ip = IPAddress(192, 168, 1, 20);
  c.net.gateway = IPAddress(192, 168, 1, 1);
  c.net.mask = IPAddress(255, 255, 255, 0);
  c.net.dns = IPAddress(192, 168, 1, 2);
  net::begin(c);
  REQUIRE(fakes::net().ethConfigs.size() == 1);
  const fakes::IpConfig& ip = fakes::net().ethConfigs[0];
  CHECK(ip.ip == static_cast<uint32_t>(IPAddress(192, 168, 1, 20)));
  CHECK(ip.gateway == static_cast<uint32_t>(IPAddress(192, 168, 1, 1)));
  CHECK(ip.mask == static_cast<uint32_t>(IPAddress(255, 255, 255, 0)));
  CHECK(ip.dns1 == static_cast<uint32_t>(IPAddress(192, 168, 1, 2)));
}

TEST_CASE("net begin: an Ethernet driver that does not start is logged") {
  glue::begin();
  fakes::net().ethBeginResult = false;
  vdm::Config c = config();
  net::begin(c);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::NetDown).at(0);
  CHECK(e.arg1 == 1);
  CHECK(std::string(e.text) == "eth init failed");
}

TEST_CASE("net begin: SNTP with the configured server and the POSIX TZ string") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  REQUIRE(fakes::net().tzConfigs.size() == 1);
  CHECK(fakes::net().tzConfigs[0].first == "CET-1CEST,M3.5.0,M10.5.0/3");
  CHECK(fakes::net().tzConfigs[0].second == "pool.ntp.org");
}

TEST_CASE("net begin: without an NTP server SNTP is stopped and only TZ is set") {
  glue::begin();
  fakes::net().sntpEnabled = true;
  vdm::Config c = config();
  c.time.ntpServer[0] = '\0';
  net::begin(c);
  CHECK(fakes::net().tzConfigs.empty());
  CHECK(fakes::net().sntpStops == 1);
  CHECK(std::string(getenv("TZ")) == "CET-1CEST,M3.5.0,M10.5.0/3");
}

TEST_CASE("net: ETH_START sets the host name of the Ethernet interface") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  fakes::net().fire(ARDUINO_EVENT_ETH_START);
  CHECK(fakes::net().ethHostname == net::hostname());
}

TEST_CASE("net service: Ethernet with an address is up, NetUp names the address") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  net::service(1000, false);
  CHECK_FALSE(net::isUp());
  ethernetUp(IPAddress(192, 168, 1, 7));
  net::service(2000, false);
  CHECK(net::isUp());
  const net::Info i = net::info();
  CHECK(i.state == vdm::NetState::Ethernet);
  CHECK(i.ip == static_cast<uint32_t>(IPAddress(192, 168, 1, 7)));
  CHECK(i.upSinceMs == 2000);
  CHECK(i.reconnects == 1);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::NetUp).at(0);
  CHECK(e.arg1 == static_cast<int32_t>(vdm::NetState::Ethernet));
  CHECK(std::string(e.text) == "192.168.1.7");
  REQUIRE(fakes::net().mdnsBegins.size() == 1);
  CHECK(fakes::net().mdnsBegins[0] == net::hostname());
  CHECK(fakes::net().mdnsServices == std::vector<std::string>{"http tcp 80"});
  fakes::net().fire(ARDUINO_EVENT_ETH_DISCONNECTED);
  net::service(3000, false);
  CHECK_FALSE(net::isUp());
  CHECK(sib::logger().withCode(vdm::EventCode::NetDown).at(0).arg1 ==
        static_cast<int32_t>(vdm::NetState::Ethernet));
}

TEST_CASE("net service: WiFi as fallback after 30 s without Ethernet") {
  glue::begin();
  vdm::Config c = config();
  vdm::copyString(c.net.ssid, sizeof c.net.ssid, "home");
  vdm::copyString(c.net.wifiPassword, sizeof c.net.wifiPassword, "secret-pass");
  net::begin(c);
  net::service(0, false);
  net::service(29999, false);
  CHECK(fakes::net().wifiBegins == 0);
  net::service(30000, false);
  CHECK(fakes::net().wifiBegins == 1);
  CHECK(fakes::net().ssid == "home");
  CHECK(fakes::net().pass == "secret-pass");
  CHECK_FALSE(fakes::net().wifiPersistent);
  CHECK(fakes::net().wifiMode == WIFI_STA);
  CHECK(fakes::net().autoReconnect);
}

TEST_CASE("net reconfigure: station changes restart the ESP, a TZ change applies live") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  vdm::Config tz = c;
  vdm::copyString(tz.time.tzPosix, sizeof tz.time.tzPosix, "UTC0");
  net::reconfigure(tz);
  CHECK(sib::ota().restartRequests.empty());
  REQUIRE(fakes::net().tzConfigs.size() == 2);
  CHECK(fakes::net().tzConfigs[1].first == "UTC0");
  vdm::Config st = tz;
  vdm::copyString(st.station, sizeof st.station, "Other");
  net::reconfigure(st);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(sib::ota().restartRequests[0].reason == 0);
  CHECK(sib::ota().restartRequests[0].delayMs == 1500);
}

TEST_CASE("net time: valid only after 2020, local time follows TZ") {
  glue::begin();
  vdm::Config c = config();
  vdm::copyString(c.time.tzPosix, sizeof c.time.tzPosix, "UTC0");
  net::begin(c);
  CHECK_FALSE(net::timeValid());
  CHECK_FALSE(net::localTime().valid);
  fakes::net().syncTime(1790136000);  // 2026-09-23 04:00:00 UTC, a Wednesday
  CHECK(net::timeValid());
  CHECK(net::lastSyncEpoch() == 1790136000);
  const vdm::LocalTime t = net::localTime();
  CHECK(t.valid);
  CHECK(t.year == 2026);
  CHECK(t.month == 9);
  CHECK(t.mday == 23);
  CHECK(t.wday == 3);
  CHECK(t.hour == 4);
  CHECK(t.epoch == 1790136000);
}

TEST_CASE("net time: the first sync is Info, later ones Debug, with the clock step") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  net::service(1000, false);
  fakes::advanceMs(1000);
  fakes::net().syncTime(1790136000);
  net::service(2000, false);
  std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::TimeSynced);
  REQUIRE(ev.size() == 1);
  CHECK(ev[0].severity == vdm::Severity::Info);
  CHECK(ev[0].arg1 == 1790136000 - 1);  // expected: 1 s after the reference 0
  fakes::net().syncTime(1790136010);
  net::service(3000, false);
  ev = sib::logger().withCode(vdm::EventCode::TimeSynced);
  REQUIRE(ev.size() == 2);
  CHECK(ev[1].severity == vdm::Severity::Debug);
  CHECK(ev[1].arg1 == 9);
}

TEST_CASE("net watchdog: the restart count of an outage survives esp_restart") {
  glue::begin();
  vdm::Config c = config();
  c.net.reconnectTimeoutMin = 5;
  if (testkit::boot() == 0) {
    net::begin(c);
    CHECK_FALSE(restartBy(0, 299000));
    CHECK(restartBy(300000, 300000));
    CHECK(sib::ota().restartRequests[0].reason == 2);
    CHECK(sib::ota().restartRequests[0].delayMs == 1000);
    testkit::reboot(testkit::Reset::Software);
  }
  net::begin(c);  // boot 1: one restart in this outage, wait 4 x 5 min
  CHECK_FALSE(restartBy(0, 1199000));
  CHECK(restartBy(1200000, 1200000));
}
