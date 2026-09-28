// Tests of src/net.cpp, second part: boundaries and state transitions of Ethernet, the clock, the
// second-granular event arguments and the RTC restart count.
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
  c.net.dhcp = false;
  c.net.ip = ip;
  c.net.gateway = kGw;
  c.net.mask = kMask;
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

TEST_CASE("net reachability: a probe due while the previous one runs waits for its report") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  ethernetUp(kIp, kGw);
  fakes::setMs(1000);
  net::service(1000, false);  // the probe is sent, the ping task has not answered yet
  REQUIRE(fakes::net().pings.size() == 1);
  const uint32_t gw2 = IPAddress(192, 168, 1, 254);
  fakes::net().ethGateway = gw2;
  fakes::net().fire(ARDUINO_EVENT_ETH_GOT_IP);  // the new gateway makes the next probe due
  fakes::setMs(2000);
  net::service(2000, false);
  CHECK(fakes::net().pings.size() == 1);  // no second session while the first one runs
  CHECK_FALSE(fakes::net().pings[0]->deleted);
  fakes::net().pingStep();  // the reply of the first probe
  tick(3000);
  REQUIRE(fakes::net().pings.size() == 2);
  CHECK(fakes::net().pings[0]->deleted);
  CHECK(fakes::net().pings[1]->config.target_addr.u_addr.ip4.addr == gw2);
  CHECK(fakes::find("ping.delete") < fakes::find("ping.new 192.168.1.254"));
  CHECK(net::health(3000).evidence == vdm::NetEvidence::GatewayPing);  // the old reply counts
}

TEST_CASE("net reachability: a probe that times out goes in the next pass, like one with a reply") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  ethernetUp(kIp, kGw);
  fakes::net().pingAnswers = {false};
  tick(1000);  // the probe times out
  REQUIRE(fakes::net().pings.size() == 1);
  CHECK_FALSE(fakes::net().pings[0]->deleted);
  tick(2000);
  CHECK(fakes::net().pings[0]->deleted);
  CHECK(net::health(2000).evidence == vdm::NetEvidence::DhcpLease);  // a timeout is no evidence
}

TEST_CASE("net reachability: a session without a report is dropped after 10 s") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  ethernetUp(kIp, kGw);
  for (uint32_t t = 1000; t <= 10000; t += 1000) {  // the ping task never reports
    fakes::setMs(t);
    net::service(t, false);
  }
  fakes::setMs(10999);
  net::service(10999, false);
  REQUIRE(fakes::net().pings.size() == 1);
  CHECK_FALSE(fakes::net().pings[0]->deleted);
  fakes::setMs(11000);
  net::service(11000, false);
  CHECK(fakes::net().pings[0]->deleted);
  tick(60000);
  CHECK(fakes::net().pings.size() == 1);  // the next probe keeps its cadence
  tick(61000);
  CHECK(fakes::net().pings.size() == 2);
}

TEST_CASE("net reachability: a session that does not start goes at once, the next probe tries again") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  ethernetUp(kIp, kGw);
  fakes::net().pingStartResult = ESP_FAIL;
  tick(1000);
  REQUIRE(fakes::net().pings.size() == 1);
  CHECK(fakes::net().pings[0]->deleted);
  CHECK(fakes::journalOf("ping.") == std::vector<std::string>{"ping.new 192.168.1.1", "ping.delete"});
  tick(2000);
  CHECK(fakes::net().pings.size() == 1);  // the failed probe counts as sent
  fakes::net().pingStartResult = ESP_OK;
  tick(61000);
  REQUIRE(fakes::net().pings.size() == 2);
  tick(62000);
  CHECK(net::health(62000).evidence == vdm::NetEvidence::GatewayPing);
  CHECK(fakes::net().pings[1]->deleted);
}

TEST_CASE("net reachability: a session that cannot be created is tried again at the next probe") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  ethernetUp(kIp, kGw);
  fakes::net().pingNewResult = ESP_ERR_NO_MEM;
  tick(1000);
  tick(2000);
  CHECK(fakes::net().pings.empty());
  fakes::net().pingNewResult = ESP_OK;
  tick(60000);
  CHECK(fakes::net().pings.empty());
  tick(61000);
  REQUIRE(fakes::net().pings.size() == 1);
  tick(62000);
  CHECK(net::health(62000).evidence == vdm::NetEvidence::GatewayPing);
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
