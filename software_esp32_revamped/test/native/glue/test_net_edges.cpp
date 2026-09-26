// Tests of src/net.cpp, second part: boundaries and state transitions of the interfaces, the WiFi
// back-off, the clock, the second-granular event arguments and the RTC restart count.
#include <ETH.h>
#include <IPAddress.h>
#include <WiFi.h>
#include <freertos/FreeRTOS.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>

#include <algorithm>

#include <vdm/net_trial.h>

#include "glue_test.h"
#include "net.h"

namespace {

const uint32_t kIp = IPAddress(192, 168, 1, 20);
const uint32_t kNewIp = IPAddress(192, 168, 1, 50);
const uint32_t kLongIp = IPAddress(192, 168, 100, 200);  // 15 chars, the longest text
const uint32_t kGw = IPAddress(192, 168, 1, 1);
const uint32_t kMask = IPAddress(255, 255, 255, 0);

vdm::Config config() {
  vdm::Config c;
  vdm::setDefaults(c);
  vdm::copyString(c.station, sizeof c.station, "Heating Floor");
  return c;
}

vdm::Config staticConfig(uint32_t ip) {
  vdm::Config c = config();
  c.net.iface = vdm::NetInterface::Ethernet;
  c.net.dhcp = false;
  c.net.ip = ip;
  c.net.gateway = kGw;
  c.net.mask = kMask;
  return c;
}

vdm::Config wifiConfig(const char* ssid) {
  vdm::Config c = config();
  c.net.iface = vdm::NetInterface::Wifi;
  vdm::copyString(c.net.ssid, sizeof c.net.ssid, ssid);
  return c;
}

void ethernetUp(uint32_t ip, uint32_t gateway = 0) {
  fakes::net().ethIp = ip;
  fakes::net().ethMask = kMask;
  fakes::net().ethGateway = gateway;
  fakes::net().fire(ARDUINO_EVENT_ETH_CONNECTED);
  fakes::net().fire(ARDUINO_EVENT_ETH_GOT_IP);
}

// One pass of the app task at t, then the ping task answers a started probe.
void tick(uint32_t t, bool mqtt = false) {
  fakes::setMs(t);
  net::service(t, mqtt);
  fakes::net().pingStep();
}

void run(uint32_t from, uint32_t to, uint32_t step = 1000) {
  for (uint32_t t = from; t <= to; t += step) {
    tick(t);
    if (!sib::ota().restartRequests.empty()) return;
  }
}

std::vector<uint8_t> blob(const vdm::NetTrialRecord& r) {
  std::vector<uint8_t> b(vdm::kNetTrialBlobMax);
  b.resize(vdm::encodeNetTrial(r, b.data(), b.size()));
  return b;
}

void bootWithRecord(vdm::NetTrialState st, const vdm::Config& prev, vdm::Config& onTrial) {
  vdm::NetTrialRecord r;
  r.state = st;
  r.previous = prev.net;
  r.trialCrc = vdm::netTrialFieldsCrc(onTrial.net);
  sib::storage().netTrial = blob(r);
  net::begin(onTrial);
}

// ---- the RTC_NOINIT_ATTR words of net.cpp (check word and restart count)

constexpr uint32_t kRtcMagic = 0x564E5744;  // "VNWD"

__attribute__((no_sanitize_address)) uint32_t rtcWord(const uint8_t* start, size_t off) {
  const volatile uint8_t* p = start + off;
  return static_cast<uint32_t>(p[0]) | static_cast<uint32_t>(p[1]) << 8 |
         static_cast<uint32_t>(p[2]) << 16 | static_cast<uint32_t>(p[3]) << 24;
}

__attribute__((no_sanitize_address)) void setRtcWord(uint8_t* start, size_t off, uint32_t v) {
  volatile uint8_t* p = start + off;
  for (int i = 0; i < 4; ++i) p[i] = static_cast<uint8_t>(v >> (8 * i));
}

// Offsets of the check word and of the word holding `count` (after a store of that count).
void findRtcWords(uint8_t*& start, size_t& magicOff, size_t& countOff, uint32_t count) {
  size_t size = 0;
  REQUIRE(testkit::sectionBounds("vdm_rtc_noinit", start, size));
  int magics = 0, counts = 0;
  for (size_t off = 0; off + 4 <= size; off += 4) {
    const uint32_t w = rtcWord(start, off);
    if (w == kRtcMagic) {
      magicOff = off;
      ++magics;
    } else if (w == count) {
      countOff = off;
      ++counts;
    }
  }
  REQUIRE(magics == 1);
  REQUIRE(counts == 1);
}

}  // namespace

// ---------------------------------------------------------------- interfaces

TEST_CASE("net: a static address needs the Ethernet link") {
  glue::begin();
  vdm::Config c = staticConfig(kIp);
  net::begin(c);
  fakes::net().ethIp = kIp;  // the driver has the address, the link is not there yet
  net::service(1000, false);
  CHECK_FALSE(net::isUp());
  fakes::net().fire(ARDUINO_EVENT_ETH_CONNECTED);
  net::service(2000, false);
  CHECK(net::isUp());
  fakes::net().fire(ARDUINO_EVENT_ETH_DISCONNECTED);
  net::service(3000, false);
  CHECK_FALSE(net::isUp());
  fakes::net().fire(ARDUINO_EVENT_ETH_CONNECTED);
  net::service(4000, false);
  CHECK(net::isUp());
  fakes::net().fire(ARDUINO_EVENT_ETH_STOP);
  net::service(5000, false);
  CHECK_FALSE(net::isUp());
}

TEST_CASE("net: with DHCP the link is up only with a lease, also after a link loss") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  fakes::net().ethIp = kIp;
  fakes::net().fire(ARDUINO_EVENT_ETH_CONNECTED);
  net::service(1000, false);
  CHECK_FALSE(net::isUp());
  fakes::net().fire(ARDUINO_EVENT_ETH_GOT_IP);
  net::service(2000, false);
  CHECK(net::isUp());
  fakes::net().fire(ARDUINO_EVENT_ETH_DISCONNECTED);
  fakes::net().fire(ARDUINO_EVENT_ETH_CONNECTED);
  net::service(3000, false);
  CHECK_FALSE(net::isUp());  // the old lease does not count
  fakes::net().fire(ARDUINO_EVENT_ETH_GOT_IP);
  net::service(4000, false);
  CHECK(net::isUp());
  fakes::net().fire(ARDUINO_EVENT_ETH_STOP);
  fakes::net().fire(ARDUINO_EVENT_ETH_CONNECTED);
  net::service(5000, false);
  CHECK_FALSE(net::isUp());
}

TEST_CASE("net: an interface without an address is down") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  ethernetUp(0);
  net::service(1000, false);
  CHECK_FALSE(net::isUp());
  CHECK(net::info().state == vdm::NetState::Down);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::NetUp));
}

TEST_CASE("net: a new address on the same interface is a new NetUp, down is no reconnect") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  ethernetUp(kIp);
  net::service(1000, false);
  net::service(2000, false);
  CHECK(net::info().reconnects == 1);
  CHECK(net::info().upSinceMs == 1000);
  fakes::net().ethIp = kLongIp;
  fakes::net().fire(ARDUINO_EVENT_ETH_GOT_IP);
  net::service(3000, false);
  const net::Info i = net::info();
  CHECK(i.ip == kLongIp);
  CHECK(i.reconnects == 2);
  CHECK(i.upSinceMs == 3000);
  const std::vector<vdm::Event> up = sib::logger().withCode(vdm::EventCode::NetUp);
  REQUIRE(up.size() == 2);
  CHECK(std::string(up[1].text) == "192.168.100.200");
  CHECK(up[1].arg2 == 0);
  fakes::net().fire(ARDUINO_EVENT_ETH_DISCONNECTED);
  net::service(4000, false);
  CHECK(net::info().state == vdm::NetState::Down);
  CHECK(net::info().reconnects == 2);
  CHECK(net::info().upSinceMs == 4000);
}

TEST_CASE("net: WiFi with its address, RSSI, MAC and a DHCP lease as evidence") {
  glue::begin();
  vdm::Config c = wifiConfig("home");
  net::begin(c);
  fakes::net().wifiIp = kLongIp;
  fakes::net().wifiMask = kMask;
  fakes::net().rssi = -60;
  fakes::net().fire(ARDUINO_EVENT_WIFI_STA_GOT_IP);
  tick(1000);
  net::Info i = net::info();
  CHECK(i.state == vdm::NetState::Wifi);
  CHECK(i.ip == kLongIp);
  CHECK(i.mask == kMask);
  CHECK(i.rssi == -60);
  CHECK(std::string(i.mac) == "24:0A:C4:12:34:56");
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::NetUp).at(0);
  CHECK(e.arg1 == static_cast<int32_t>(vdm::NetState::Wifi));
  CHECK(e.arg2 == 0);
  CHECK(std::string(e.text) == "192.168.100.200");
  CHECK(net::health(1000).evidence == vdm::NetEvidence::DhcpLease);
  fakes::net().rssi = -128;
  tick(2000);
  CHECK(net::info().rssi == -128);
  fakes::net().rssi = 1;  // no valid reading
  tick(3000);
  CHECK(net::info().rssi == 0);
  fakes::net().rssi = 5;
  tick(4000);
  CHECK(net::info().rssi == 0);
  const arduino_event_id_t downs[] = {ARDUINO_EVENT_WIFI_STA_DISCONNECTED,
                                      ARDUINO_EVENT_WIFI_STA_LOST_IP, ARDUINO_EVENT_WIFI_STA_STOP};
  uint32_t t = 5000;
  for (arduino_event_id_t ev : downs) {
    fakes::net().fire(ev);
    tick(t);
    CHECK(net::info().state == vdm::NetState::Down);
    fakes::net().fire(ARDUINO_EVENT_WIFI_STA_GOT_IP);
    tick(t + 1000);
    CHECK(net::info().state == vdm::NetState::Wifi);
    t += 2000;
  }
}

TEST_CASE("net: a one-character SSID is a WiFi network") {
  glue::begin();
  vdm::Config c = wifiConfig("a");
  net::begin(c);
  CHECK(fakes::net().wifiBegins == 1);
  CHECK(fakes::net().ssid == "a");
}

TEST_CASE("net: a static address on WiFi is configured on the WiFi interface") {
  glue::begin();
  vdm::Config c = wifiConfig("home");
  c.net.dhcp = false;
  c.net.ip = kIp;
  c.net.gateway = kGw;
  c.net.mask = kMask;
  net::begin(c);
  CHECK(fakes::net().ethBegins.empty());
  CHECK(fakes::net().ethConfigs.empty());
  REQUIRE(fakes::net().wifiConfigs.size() == 1);
  CHECK(fakes::net().wifiConfigs[0].ip == kIp);
  CHECK(fakes::net().wifiConfigs[0].gateway == kGw);
}

TEST_CASE("net: WiFi retries back off from 5 s, doubling up to 60 s") {
  glue::begin();
  vdm::Config c = wifiConfig("home");
  net::begin(c);
  CHECK(fakes::net().wifiBegins == 1);
  const std::pair<uint32_t, int> steps[] = {
      {0, 2},      {4999, 2},   {5000, 3},   {14999, 3},  {15000, 4},  {34999, 4},  {35000, 5},
      {74999, 5},  {75000, 6},  {134999, 6}, {135000, 7}, {194999, 7}, {195000, 8}};
  for (const auto& s : steps) {
    net::service(s.first, false);
    INFO("t = " << s.first);
    CHECK(fakes::net().wifiBegins == s.second);
  }
}

TEST_CASE("net: Auto fallback to WiFi and back to Ethernet") {
  glue::begin();
  vdm::Config c = config();
  vdm::copyString(c.net.ssid, sizeof c.net.ssid, "home");
  net::begin(c);
  net::service(0, false);
  net::service(30000, false);
  CHECK(fakes::net().wifiBegins == 1);
  net::service(31000, false);
  CHECK(fakes::net().wifiDisconnects == 0);  // WiFi stays while Ethernet is down
  fakes::net().wifiIp = kNewIp;
  fakes::net().fire(ARDUINO_EVENT_WIFI_STA_GOT_IP);
  net::service(32000, false);
  CHECK(net::info().state == vdm::NetState::Wifi);
  ethernetUp(kIp);
  net::service(40000, false);
  CHECK(net::info().state == vdm::NetState::Ethernet);
  CHECK(fakes::net().wifiDisconnects == 1);
  CHECK(fakes::net().lastDisconnectWifiOff);
  CHECK(fakes::net().wifiMode == WIFI_OFF);
  // Ethernet lost 60 s later: down (WiFi is off), WiFi again 30 s after the loss.
  fakes::net().fire(ARDUINO_EVENT_ETH_DISCONNECTED);
  net::service(100000, false);
  CHECK(net::info().state == vdm::NetState::Down);
  net::service(129999, false);
  CHECK(fakes::net().wifiBegins == 1);
  net::service(130000, false);
  CHECK(fakes::net().wifiBegins == 2);
  CHECK(fakes::net().wifiModes == std::vector<wifi_mode_t>{WIFI_STA, WIFI_OFF, WIFI_STA});
}

TEST_CASE("net: the Auto fallback counts from the first pass without Ethernet") {
  glue::begin();
  vdm::Config c = config();
  vdm::copyString(c.net.ssid, sizeof c.net.ssid, "home");
  net::begin(c);
  net::service(10000, false);
  net::service(39999, false);
  CHECK(fakes::net().wifiBegins == 0);
  net::service(40000, false);
  CHECK(fakes::net().wifiBegins == 1);
}

TEST_CASE("net: Auto with an Ethernet driver that failed starts WiFi at once") {
  glue::begin();
  fakes::net().ethBeginResult = false;
  vdm::Config c = config();
  vdm::copyString(c.net.ssid, sizeof c.net.ssid, "home");
  net::begin(c);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::NetDown).at(0);
  CHECK(e.arg1 == 1);
  CHECK(e.arg2 == 0);
  CHECK(fakes::net().wifiBegins == 0);
  net::service(0, false);
  CHECK(fakes::net().wifiBegins == 1);
}

TEST_CASE("net: a station name of the maximum length is the whole host name") {
  glue::begin();
  vdm::Config c = config();
  vdm::copyString(c.station, sizeof c.station, "ABCDEFGHIJKLMNOPQRST");
  REQUIRE(strlen(c.station) == vdm::kStationNameMax);
  net::begin(c);
  CHECK(std::string(net::hostname()) == "ABCDEFGHIJKLMNOPQRST");
}

// ---------------------------------------------------------------- time

TEST_CASE("net time: valid from 2020-01-01 00:00:00 UTC") {
  glue::begin();
  vdm::Config c = config();
  vdm::copyString(c.time.tzPosix, sizeof c.time.tzPosix, "UTC0");
  net::begin(c);
  fakes::setWallClock(1577836799);
  CHECK_FALSE(net::timeValid());
  CHECK_FALSE(net::localTime().valid);
  fakes::setWallClock(1577836800);
  CHECK(net::timeValid());
  const vdm::LocalTime t = net::localTime();
  CHECK(t.valid);
  CHECK(t.year == 2020);
  CHECK(t.epoch == 1577836800);
}

TEST_CASE("net time: the last sync epoch is 0 before a sync and for no positive time") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  CHECK(net::lastSyncEpoch() == 0);
  REQUIRE(fakes::net().sntpCb != nullptr);
  fakes::net().syncTime(1);
  CHECK(net::lastSyncEpoch() == 1);
  fakes::net().sntpCb(nullptr);
  CHECK(net::lastSyncEpoch() == 0);
  fakes::net().syncTime(1790136000);
  struct timeval tv;
  tv.tv_sec = -5;
  tv.tv_usec = 0;
  fakes::net().sntpCb(&tv);
  CHECK(net::lastSyncEpoch() == 0);
}

TEST_CASE("net time: without an NTP server the TZ string replaces the environment's") {
  glue::begin();
  setenv("TZ", "EST5", 1);
  vdm::Config c = config();
  c.time.ntpServer[0] = '\0';
  vdm::copyString(c.time.tzPosix, sizeof c.time.tzPosix, "UTC0");
  net::begin(c);
  CHECK(std::string(getenv("TZ")) == "UTC0");
}

TEST_CASE("net time: a clock that kept time for 999 s steps by 0") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  fakes::setMs(1000);
  fakes::net().syncTime(1790136000);
  net::service(1000, false);
  fakes::setMs(1000000);
  fakes::net().syncTime(1790136999);
  net::service(1000000, false);
  const std::vector<vdm::Event> ev = sib::logger().withCode(vdm::EventCode::TimeSynced);
  REQUIRE(ev.size() == 2);
  CHECK(ev[1].arg1 == 0);
}

TEST_CASE("net reconfigure: the same time settings are not applied again") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  REQUIRE(fakes::net().tzConfigs.size() == 1);
  net::reconfigure(c);
  CHECK(fakes::net().tzConfigs.size() == 1);
  CHECK(sib::ota().restartRequests.empty());
}

// ---------------------------------------------------------------- reachability

TEST_CASE("net reachability: a new gateway gets a new ping session") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  ethernetUp(kIp, kGw);
  tick(1000);
  REQUIRE(fakes::net().pings.size() == 1);
  const uint32_t gw2 = IPAddress(192, 168, 1, 254);
  fakes::net().ethGateway = gw2;
  fakes::net().fire(ARDUINO_EVENT_ETH_GOT_IP);  // lease renewed with another gateway
  tick(2000);
  REQUIRE(fakes::net().pings.size() == 2);
  CHECK(fakes::net().pings[0]->deleted);
  CHECK(fakes::net().pings[1]->config.target_addr.u_addr.ip4.addr == gw2);
  CHECK(fakes::find("ping.delete") < fakes::find("ping.new 192.168.1.254"));
}

TEST_CASE("net reachability: the evidence age in whole seconds") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  ethernetUp(kIp);
  tick(1000);  // the DHCP lease
  tick(2998);
  CHECK(net::health(2998).evidenceAgeS == 1);
}

TEST_CASE("net reachability: no lease evidence without a GOT_IP") {
  glue::begin();
  vdm::Config c = staticConfig(kIp);
  net::begin(c);
  fakes::net().ethIp = kIp;
  fakes::net().fire(ARDUINO_EVENT_ETH_CONNECTED);
  vdm::Config d = c;  // DHCP saved, the restart not yet done
  d.net.dhcp = true;
  net::reconfigure(d);
  tick(1000);
  CHECK(net::health(1000).ipUp);
  CHECK(net::health(1000).evidence == vdm::NetEvidence::None);
}

TEST_CASE("net reachability: Lost and Regained name whole seconds") {
  glue::begin();
  vdm::Config c = staticConfig(kIp);
  net::begin(c);
  ethernetUp(kIp, kGw);
  tick(1000);
  tick(2000);  // armed, evidence at 2 s
  fakes::net().pingDefault = false;
  tick(152850);
  const vdm::Event lost = sib::logger().withCode(vdm::EventCode::NetUnreachable).at(0);
  CHECK(lost.arg1 == 150);
  CHECK(lost.arg2 == static_cast<int32_t>(vdm::NetEvidence::GatewayPing));
  tick(243800, true);  // MQTT back 90.95 s later
  CHECK(sib::logger().withCode(vdm::EventCode::NetReachable).at(0).arg1 == 90);
}

TEST_CASE("net reachability: Lost without evidence since the IP came back has no age") {
  glue::begin();
  vdm::Config c = staticConfig(kIp);
  net::begin(c);
  ethernetUp(kIp, kGw);
  tick(1000);
  tick(2000);  // armed
  fakes::net().pingDefault = false;
  fakes::net().fire(ARDUINO_EVENT_ETH_DISCONNECTED);
  tick(3000);
  fakes::net().fire(ARDUINO_EVENT_ETH_CONNECTED);
  tick(4000);  // up again with the same gateway: still armed, no evidence
  tick(153000);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::NetUnreachable));
  tick(154000);
  const vdm::Event lost = sib::logger().withCode(vdm::EventCode::NetUnreachable).at(0);
  CHECK(lost.arg1 == -1);
  CHECK(lost.arg2 == static_cast<int32_t>(vdm::NetEvidence::None));
}

// ---------------------------------------------------------------- watchdog

TEST_CASE("net watchdog: the interface restart waits 200 ms and names whole seconds") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  fakes::net().fire(ARDUINO_EVENT_ETH_CONNECTED);  // link, no lease
  tick(0);
  tick(300700);
  CHECK(fakes::net().ethStops == 1);
  const std::vector<uint32_t>& d = fakes::rtos().delays;
  CHECK(std::count(d.begin(), d.end(), static_cast<uint32_t>(pdMS_TO_TICKS(200))) == 1);
  CHECK(sib::logger().withCode(vdm::EventCode::NetInterfaceRestart).at(0).arg1 == 300);
}

TEST_CASE("net watchdog: the ESP restart names whole minutes of the outage") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  tick(0);
  tick(659990);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(sib::ota().restartRequests[0].reason == 2);
  CHECK(sib::ota().restartRequests[0].detail == 10);
}

TEST_CASE("net watchdog: a restart count without the RTC check word is ignored") {
  glue::begin();
  vdm::Config c = config();
  c.net.reconnectTimeoutMin = 5;
  if (testkit::boot() == 0) {
    net::begin(c);
    run(0, 600000);
    REQUIRE(sib::ota().restartRequests.size() == 1);
    uint8_t* start = nullptr;
    size_t magicOff = 0, countOff = 0;
    findRtcWords(start, magicOff, countOff, 1);
    setRtcWord(start, magicOff, 0);  // count 1 stays, the check word is gone
    testkit::reboot(testkit::Reset::Software);
  }
  net::begin(c);
  run(0, 600000);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(fakes::nowMs() == 600000);  // 5 + 5 min: no earlier restart counted
}

TEST_CASE("net watchdog: 255 restarts in the RTC word are taken, the wait is capped") {
  glue::begin();
  vdm::Config c = config();
  c.net.reconnectTimeoutMin = 5;
  uint8_t* start = nullptr;
  size_t magicOff = 0, countOff = 0;
  if (testkit::boot() == 0) {
    net::begin(c);
    run(0, 600000);
    REQUIRE(sib::ota().restartRequests.size() == 1);
    findRtcWords(start, magicOff, countOff, 1);
    setRtcWord(start, countOff, 255);
    testkit::reboot(testkit::Reset::Software);
  }
  net::begin(c);
  run(0, 900000, 10000);
  CHECK(sib::ota().restartRequests.empty());  // 5 min + 24 h
  CHECK(net::health(900000).ifaceRestarts == 1);
  findRtcWords(start, magicOff, countOff, 255);
}

// ---------------------------------------------------------------- trial

TEST_CASE("net trial: the remaining time in whole seconds") {
  glue::begin();
  vdm::Config old = config();
  vdm::Config n = staticConfig(kNewIp);
  bootWithRecord(vdm::NetTrialState::Armed, old, n);
  tick(120);
  CHECK(net::health(120).trialRemainingS == 119);
  CHECK(net::trialInfo().remainS == 119);
}

TEST_CASE("net trial: confirm names the whole seconds with the network") {
  glue::begin();
  vdm::Config old = config();
  vdm::Config n = staticConfig(kNewIp);
  bootWithRecord(vdm::NetTrialState::Armed, old, n);
  ethernetUp(kNewIp, kGw);
  run(1000, 59000);  // the IP counted from 1 s
  CHECK(net::requestTrialConfirm());
  tick(60940);
  CHECK(sib::logger().withCode(vdm::EventCode::NetTrialConfirmed).at(0).arg1 == 59);
}

TEST_CASE("net trial: a change during the trial names the whole seconds it ran") {
  glue::begin();
  vdm::Config old = config();
  vdm::Config n = staticConfig(kNewIp);
  bootWithRecord(vdm::NetTrialState::Armed, old, n);
  ethernetUp(kNewIp, kGw);
  run(1000, 31000);
  fakes::setMs(31970);
  vdm::Config next = staticConfig(IPAddress(192, 168, 1, 70));
  net::reconfigure(next);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::NetTrialConfirmed).at(0);
  CHECK(e.arg1 == 30);
  CHECK(e.arg2 == 1);
}

TEST_CASE("net trial: the longest address in NetTrialStarted and NetTrialReverted") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  vdm::Config n = staticConfig(kLongIp);
  net::reconfigure(n);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::NetTrialStarted).at(0);
  CHECK(e.arg1 == 120);
  CHECK(e.arg2 == 0);
  CHECK(std::string(e.text) == "192.168.100.200");
}

TEST_CASE("net trial: a revert after the window names the longest previous address") {
  glue::begin();
  vdm::Config old = staticConfig(kLongIp);
  vdm::Config n = staticConfig(kNewIp);
  bootWithRecord(vdm::NetTrialState::Armed, old, n);
  run(1000, 120000);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(std::string(sib::logger().withCode(vdm::EventCode::NetTrialReverted).at(0).text) ==
        "192.168.100.200");
}

TEST_CASE("net trial: a revert at boot names the longest previous address") {
  glue::begin();
  vdm::Config old = staticConfig(kLongIp);
  vdm::Config n = staticConfig(kNewIp);
  bootWithRecord(vdm::NetTrialState::Running, old, n);
  CHECK(std::string(sib::logger().withCode(vdm::EventCode::NetTrialReverted).at(0).text) ==
        "192.168.100.200");
}
