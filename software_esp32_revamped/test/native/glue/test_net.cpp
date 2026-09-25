// Tests of src/net.cpp: interfaces, state events, WiFi fallback, time, end-to-end reachability,
// watchdog stages, network trial.
#include <ETH.h>
#include <IPAddress.h>
#include <WiFi.h>
#include <time.h>

#include <vdm/net_trial.h>

#include "glue_test.h"
#include "net.h"

namespace {

const uint32_t kIp = IPAddress(192, 168, 1, 20);
const uint32_t kNewIp = IPAddress(192, 168, 1, 50);
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

// Ethernet link with an address (DHCP: GOT_IP; static: the link is enough).
void ethernetUp(uint32_t ip, uint32_t gateway = 0) {
  fakes::net().ethIp = ip;
  fakes::net().ethMask = kMask;
  fakes::net().ethGateway = gateway;
  fakes::net().fire(ARDUINO_EVENT_ETH_CONNECTED);
  fakes::net().fire(ARDUINO_EVENT_ETH_GOT_IP);
}

// One second of the app task: service, then the ping task answers a started probe.
void tick(uint32_t t, bool mqtt = false) {
  fakes::setMs(t);
  net::service(t, mqtt);
  fakes::net().pingStep();
}

// Ticks every `step` ms in [from, to]; stops after the first restart request.
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

vdm::NetTrialRecord storedRecord() {
  vdm::NetTrialRecord r;
  const std::vector<uint8_t>& b = sib::storage().netTrial;
  REQUIRE(vdm::decodeNetTrial(b.data(), b.size(), r));
  return r;
}

// Boot with a trial record for `onTrial` whose previous settings are `prev`.
void bootWithRecord(vdm::NetTrialState st, const vdm::Config& prev, vdm::Config& onTrial) {
  vdm::NetTrialRecord r;
  r.state = st;
  r.previous = prev.net;
  r.trialCrc = vdm::netTrialFieldsCrc(onTrial.net);
  sib::storage().netTrial = blob(r);
  net::begin(onTrial);
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
  CHECK(sib::storage().netTrialClears == 0);
  CHECK_FALSE(net::trialInfo().active);
}

TEST_CASE("net begin: a static address is configured on Ethernet") {
  glue::begin();
  vdm::Config c = staticConfig(kIp);
  c.net.dns = IPAddress(192, 168, 1, 2);
  net::begin(c);
  REQUIRE(fakes::net().ethConfigs.size() == 1);
  const fakes::IpConfig& ip = fakes::net().ethConfigs[0];
  CHECK(ip.ip == kIp);
  CHECK(ip.gateway == kGw);
  CHECK(ip.mask == kMask);
  CHECK(ip.dns1 == static_cast<uint32_t>(IPAddress(192, 168, 1, 2)));
}

TEST_CASE("net begin: a static address without DNS uses the gateway as DNS") {
  glue::begin();
  vdm::Config c = staticConfig(kIp);
  net::begin(c);
  REQUIRE(fakes::net().ethConfigs.size() == 1);
  CHECK(fakes::net().ethConfigs[0].dns1 == kGw);
  CHECK(c.net.dns == 0);  // the stored value stays empty
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

TEST_CASE("net reachability: DHCP lease, MQTT, SNTP and LAN HTTP prove the network") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  ethernetUp(kIp, kGw);
  fakes::net().pingDefault = false;
  tick(1000);  // the GOT_IP of this DHCP configuration counts as evidence
  vdm::NetHealthInfo h = net::health(1000);
  CHECK(h.ipUp);
  CHECK(h.reachable);
  CHECK(h.proven);
  CHECK_FALSE(h.pingArmed);
  CHECK(h.evidence == vdm::NetEvidence::DhcpLease);
  CHECK(h.evidenceAgeS == 0);
  CHECK(net::otaNetOk());
  tick(5000, true);
  CHECK(net::health(5000).evidence == vdm::NetEvidence::Mqtt);
  fakes::net().syncTime(1790136000);
  tick(6000);
  CHECK(net::health(6000).evidence == vdm::NetEvidence::TimeSync);
  net::noteInboundHttp(IPAddress(127, 0, 0, 1));
  net::noteInboundHttp(kIp);
  tick(9000);
  h = net::health(9000);
  CHECK(h.evidence == vdm::NetEvidence::TimeSync);
  CHECK(h.evidenceAgeS == 3);
  net::noteInboundHttp(IPAddress(192, 168, 1, 99));
  tick(10000);
  CHECK(net::health(10000).evidence == vdm::NetEvidence::InboundHttp);
}

TEST_CASE("net reachability: a static address is not proven by its GOT_IP, the gateway ping is") {
  glue::begin();
  vdm::Config c = staticConfig(kIp);
  net::begin(c);
  fakes::net().pingDefault = false;
  ethernetUp(kIp, kGw);
  tick(1000);
  vdm::NetHealthInfo h = net::health(1000);
  CHECK(h.ipUp);
  CHECK(h.reachable);
  CHECK_FALSE(h.proven);
  CHECK(h.evidence == vdm::NetEvidence::None);
  CHECK(h.evidenceAgeS == UINT32_MAX);
  CHECK_FALSE(net::otaNetOk());
  REQUIRE(fakes::net().pings.size() == 1);
  const esp_ping_config_t& pc = fakes::net().pings[0]->config;
  CHECK(pc.target_addr.u_addr.ip4.addr == kGw);
  CHECK(pc.target_addr.type == IPADDR_TYPE_V4);
  CHECK(pc.count == 1);
  CHECK(pc.timeout_ms == 1000);
  CHECK(pc.interval_ms == 1000);
  CHECK(pc.data_size == 32);
  fakes::net().pingDefault = true;
  tick(61000);  // the next probe, answered
  tick(62000);
  h = net::health(62000);
  CHECK(h.proven);
  CHECK(h.pingArmed);
  CHECK(h.evidence == vdm::NetEvidence::GatewayPing);
  CHECK(net::otaNetOk());
  CHECK(fakes::net().pings.size() == 1);  // one session, restarted per probe
  CHECK(fakes::journalOf("ping.start").size() == 2);
}

TEST_CASE("net watchdog: gateway silent -> unreachable, interface restart, ESP restart") {
  glue::begin();
  vdm::Config c = staticConfig(kIp);
  net::begin(c);
  ethernetUp(kIp, kGw);
  tick(1000);  // probe answered
  tick(2000);  // the reply seen: armed, evidence at 2 s
  REQUIRE(net::health(2000).pingArmed);
  fakes::net().pingDefault = false;
  run(3000, 151000);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::NetUnreachable));
  tick(152000);
  const std::vector<vdm::Event> lost = sib::logger().withCode(vdm::EventCode::NetUnreachable);
  REQUIRE(lost.size() == 1);
  CHECK(lost[0].arg1 == 150);
  CHECK(lost[0].arg2 == static_cast<int32_t>(vdm::NetEvidence::GatewayPing));
  CHECK_FALSE(net::health(152000).reachable);
  CHECK_FALSE(net::otaNetOk());
  run(153000, 451000);
  CHECK(fakes::net().ethStops == 0);
  tick(452000);
  CHECK(fakes::net().ethStops == 1);
  CHECK(fakes::net().ethStarts == 1);
  CHECK(fakes::find("esp_eth_stop") < fakes::find("esp_eth_start"));
  const vdm::Event ir = sib::logger().withCode(vdm::EventCode::NetInterfaceRestart).at(0);
  CHECK(ir.arg1 == 300);
  CHECK(ir.arg2 == 1);
  CHECK(net::health(452000).ifaceRestarts == 1);
  run(453000, 751000);
  CHECK(sib::ota().restartRequests.empty());
  tick(752000);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(sib::ota().restartRequests[0].reason == 2);
  CHECK(sib::ota().restartRequests[0].delayMs == 1000);
  CHECK(sib::ota().restartRequests[0].detail == 10);
}

TEST_CASE("net watchdog: replies coming back end the outage without a restart") {
  glue::begin();
  vdm::Config c = staticConfig(kIp);
  net::begin(c);
  ethernetUp(kIp, kGw);
  tick(1000);
  tick(2000);
  fakes::net().pingDefault = false;
  run(3000, 200000);
  REQUIRE(sib::logger().has(vdm::EventCode::NetUnreachable));
  fakes::net().pingDefault = true;
  run(201000, 800000);
  const std::vector<vdm::Event> back = sib::logger().withCode(vdm::EventCode::NetReachable);
  REQUIRE(back.size() == 1);
  CHECK(back[0].arg1 == 90);  // lost at 152 s, the reply of the 241 s probe seen at 242 s
  CHECK(sib::ota().restartRequests.empty());
  CHECK(fakes::net().ethStops == 0);
}

TEST_CASE("net watchdog: a gateway that never answers leaves the IP-only rule for 24 h") {
  glue::begin();
  vdm::Config c = staticConfig(kIp);
  net::begin(c);
  fakes::net().pingDefault = false;
  ethernetUp(kIp, kGw);
  run(1000, 86400000, 10000);
  CHECK(sib::ota().restartRequests.empty());
  CHECK(fakes::net().ethStops == 0);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::NetUnreachable));
  CHECK_FALSE(net::health(86400000).pingArmed);
}

TEST_CASE("net watchdog: no IP from boot, interface restart only with a driver handle") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  fakes::net().fire(ARDUINO_EVENT_ETH_CONNECTED);  // link, no DHCP answer
  run(0, 299000);
  CHECK(fakes::net().ethStops == 0);
  tick(300000);
  CHECK(fakes::net().ethStops == 1);
  CHECK(sib::logger().withCode(vdm::EventCode::NetInterfaceRestart).at(0).arg1 == 300);
  run(301000, 600000);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(fakes::nowMs() == 600000);
  CHECK(sib::ota().restartRequests[0].detail == 10);
}

TEST_CASE("net watchdog: without a driver handle and WiFi nothing is restarted at stage 1") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  run(0, 300000);
  CHECK(fakes::net().ethStops == 0);
  CHECK(fakes::net().wifiReconnects == 0);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::NetInterfaceRestart));
  CHECK(net::health(300000).ifaceRestarts == 1);
  run(301000, 600000);
  CHECK(fakes::nowMs() == 600000);
  REQUIRE(sib::ota().restartRequests.size() == 1);
}

TEST_CASE("net watchdog: WiFi is reconnected at stage 1") {
  glue::begin();
  vdm::Config c = config();
  c.net.iface = vdm::NetInterface::Wifi;
  vdm::copyString(c.net.ssid, sizeof c.net.ssid, "home");
  net::begin(c);
  run(0, 300000);
  CHECK(fakes::net().wifiReconnects == 1);
  CHECK(sib::logger().withCode(vdm::EventCode::NetInterfaceRestart).at(0).arg2 == 2);
}

TEST_CASE("net watchdog: 0 minutes disables both stages") {
  glue::begin();
  vdm::Config c = config();
  c.net.reconnectTimeoutMin = 0;
  net::begin(c);
  fakes::net().fire(ARDUINO_EVENT_ETH_CONNECTED);
  run(0, 3600000, 10000);
  CHECK(fakes::net().ethStops == 0);
  CHECK(sib::ota().restartRequests.empty());
}

TEST_CASE("net watchdog: the restart count of an outage survives esp_restart") {
  glue::begin();
  vdm::Config c = config();
  c.net.reconnectTimeoutMin = 5;
  if (testkit::boot() == 0) {
    net::begin(c);
    run(0, 599000);
    CHECK(sib::ota().restartRequests.empty());
    run(600000, 600000);
    REQUIRE(sib::ota().restartRequests.size() == 1);
    CHECK(sib::ota().restartRequests[0].reason == 2);
    testkit::reboot(testkit::Reset::Software);
  }
  net::begin(c);  // boot 1: one restart in this outage, ESP stage 5 + 4 x 5 min
  run(0, 1499000);
  CHECK(sib::ota().restartRequests.empty());
  run(1500000, 1500000);
  CHECK(sib::ota().restartRequests.size() == 1);
}

TEST_CASE("net reconfigure: a TZ change applies live, a station change restarts without trial") {
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
  CHECK(sib::storage().netTrialSaves == 0);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::NetTrialStarted));
}

TEST_CASE("net reconfigure: unused static fields and reconnectTimeoutMin do not restart") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  vdm::Config d = c;
  d.net.ip = kNewIp;
  d.net.gateway = kGw;
  d.net.reconnectTimeoutMin = 9;
  vdm::copyString(d.time.ntpServer, sizeof d.time.ntpServer, "ntp.example");
  net::reconfigure(d);
  CHECK(sib::ota().restartRequests.empty());
  CHECK(sib::storage().netTrialSaves == 0);
  CHECK(fakes::net().tzConfigs.back().second == "ntp.example");
}

TEST_CASE("net trial: a new static address is armed with the old settings and restarts") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  vdm::Config n = staticConfig(kNewIp);
  net::reconfigure(n);
  const vdm::NetTrialRecord r = storedRecord();
  CHECK(r.state == vdm::NetTrialState::Armed);
  CHECK(r.previous.dhcp);
  CHECK(r.previous.iface == vdm::NetInterface::Auto);
  CHECK(r.trialCrc == vdm::netTrialFieldsCrc(n.net));
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::NetTrialStarted).at(0);
  CHECK(e.arg1 == 120);
  CHECK(std::string(e.text) == "192.168.1.50");
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(sib::ota().restartRequests[0].reason == 0);
}

TEST_CASE("net trial: two changes before the restart keep the first previous settings") {
  glue::begin();
  vdm::Config c = config();
  net::begin(c);
  vdm::Config a = staticConfig(kNewIp);
  net::reconfigure(a);
  vdm::Config b = staticConfig(IPAddress(192, 168, 1, 60));
  net::reconfigure(b);
  const vdm::NetTrialRecord r = storedRecord();
  CHECK(r.previous.dhcp);
  CHECK(r.trialCrc == vdm::netTrialFieldsCrc(b.net));
  CHECK(sib::storage().netTrialSaves == 2);
  CHECK_FALSE(sib::logger().has(vdm::EventCode::NetTrialConfirmed));
}

TEST_CASE("net trial: boot with an Armed record runs it; no confirm -> revert at 120 s after the IP") {
  glue::begin();
  vdm::Config old = config();
  vdm::Config n = staticConfig(kNewIp);
  bootWithRecord(vdm::NetTrialState::Armed, old, n);
  CHECK(storedRecord().state == vdm::NetTrialState::Running);
  CHECK(n.net.ip == kNewIp);  // not reverted
  CHECK(net::trialInfo().active);
  CHECK(net::trialInfo().remainS == 120);
  run(1000, 4000);
  ethernetUp(kNewIp, kGw);
  run(5000, 124000);
  CHECK(sib::ota().restartRequests.empty());
  CHECK(net::trialInfo().remainS == 1);
  CHECK(net::health(124000).trialActive);
  CHECK(net::health(124000).trialRemainingS == 1);
  tick(125000);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(sib::ota().restartRequests[0].reason == 5);
  CHECK(sib::ota().restartRequests[0].delayMs == 1000);
  REQUIRE_FALSE(sib::storage().applied.empty());
  const vdm::Config& stored = sib::storage().applied.back();
  CHECK(stored.net.dhcp);
  CHECK(stored.net.iface == vdm::NetInterface::Auto);
  CHECK(sib::storage().netTrial.empty());
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::NetTrialReverted).at(0);
  CHECK(e.arg1 == 1);
  CHECK(e.arg2 == 0);
  CHECK(std::string(e.text) == "dhcp");
  CHECK_FALSE(net::trialInfo().active);
  CHECK_FALSE(net::requestTrialConfirm());
}

TEST_CASE("net trial: no network within 120 s -> revert (no network)") {
  glue::begin();
  vdm::Config old = staticConfig(kIp);
  vdm::Config n = staticConfig(kNewIp);
  bootWithRecord(vdm::NetTrialState::Armed, old, n);
  run(1000, 120000);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(fakes::nowMs() == 120000);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::NetTrialReverted).at(0);
  CHECK(e.arg1 == 2);
  CHECK(std::string(e.text) == "192.168.1.20");
  CHECK(sib::storage().applied.back().net.ip == kIp);
}

TEST_CASE("net trial: a failed persist on revert keeps the record for the next boot") {
  glue::begin();
  vdm::Config old = staticConfig(kIp);
  vdm::Config n = staticConfig(kNewIp);
  bootWithRecord(vdm::NetTrialState::Armed, old, n);
  sib::storage().applyResult = false;
  run(1000, 120000);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(sib::ota().restartRequests[0].reason == 5);
  CHECK(storedRecord().state == vdm::NetTrialState::Running);
  CHECK(sib::logger().withCode(vdm::EventCode::NetTrialReverted).at(0).arg2 == -1);
}

TEST_CASE("net trial: confirm ends it, the record is erased, no revert") {
  glue::begin();
  vdm::Config old = config();
  vdm::Config n = staticConfig(kNewIp);
  bootWithRecord(vdm::NetTrialState::Armed, old, n);
  ethernetUp(kNewIp, kGw);
  run(1000, 59000);  // the IP counted from 1 s
  CHECK(net::requestTrialConfirm());
  CHECK(net::trialInfo().active);  // acted on in the next pass
  tick(60000);
  CHECK(sib::storage().netTrial.empty());
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::NetTrialConfirmed).at(0);
  CHECK(e.arg1 == 59);
  CHECK(e.arg2 == 0);
  run(61000, 200000);
  CHECK(sib::ota().restartRequests.empty());
  CHECK_FALSE(net::requestTrialConfirm());
  CHECK_FALSE(net::requestTrialRevert());
  CHECK_FALSE(net::trialInfo().active);
  CHECK(net::trialInfo().remainS == 0);
}

TEST_CASE("net trial: revert on request (reason 4) restarts") {
  glue::begin();
  vdm::Config old = config();
  vdm::Config n = staticConfig(kNewIp);
  bootWithRecord(vdm::NetTrialState::Armed, old, n);
  ethernetUp(kNewIp, kGw);
  run(1000, 59000);
  CHECK(net::requestTrialRevert());
  tick(60000);
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(sib::ota().restartRequests[0].reason == 5);
  CHECK(sib::logger().withCode(vdm::EventCode::NetTrialReverted).at(0).arg1 == 4);
  CHECK(sib::storage().applied.back().net.dhcp);
}

TEST_CASE("net trial: a network change during a running trial confirms it and arms a new one") {
  glue::begin();
  vdm::Config old = config();
  vdm::Config n = staticConfig(kNewIp);
  bootWithRecord(vdm::NetTrialState::Armed, old, n);
  ethernetUp(kNewIp, kGw);
  run(1000, 31000);
  vdm::Config next = staticConfig(IPAddress(192, 168, 1, 70));
  net::reconfigure(next);
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::NetTrialConfirmed).at(0);
  CHECK(e.arg1 == 30);
  CHECK(e.arg2 == 1);
  const vdm::NetTrialRecord r = storedRecord();
  CHECK(r.state == vdm::NetTrialState::Armed);
  CHECK(r.previous.ip == kNewIp);  // the running settings
  CHECK_FALSE(r.previous.dhcp);
  CHECK(r.trialCrc == vdm::netTrialFieldsCrc(next.net));
  REQUIRE(sib::ota().restartRequests.size() == 1);
  CHECK(sib::ota().restartRequests[0].reason == 0);
}

TEST_CASE("net trial: a boot that finds a Running record reverts before the interfaces start") {
  glue::begin();
  vdm::Config old = staticConfig(kIp);
  vdm::Config n = staticConfig(kNewIp);
  bootWithRecord(vdm::NetTrialState::Running, old, n);
  CHECK(n.net.ip == kIp);  // the caller's config is reverted
  REQUIRE(fakes::net().ethConfigs.size() == 1);
  CHECK(fakes::net().ethConfigs[0].ip == kIp);
  CHECK(sib::storage().applied.back().net.ip == kIp);
  CHECK(sib::storage().netTrial.empty());
  const vdm::Event e = sib::logger().withCode(vdm::EventCode::NetTrialReverted).at(0);
  CHECK(e.arg1 == 3);
  CHECK(e.arg2 == 0);
  CHECK(std::string(e.text) == "192.168.1.20");
  CHECK(sib::ota().restartRequests.empty());
  CHECK_FALSE(net::trialInfo().active);
  CHECK(fakes::find("storage.applyConfig") < fakes::find("eth.config 192.168.1.20"));
}

TEST_CASE("net trial: a Running record whose revert cannot be stored stays") {
  glue::begin();
  sib::storage().applyResult = false;
  vdm::Config old = staticConfig(kIp);
  vdm::Config n = staticConfig(kNewIp);
  bootWithRecord(vdm::NetTrialState::Running, old, n);
  CHECK(n.net.ip == kIp);
  CHECK_FALSE(sib::storage().netTrial.empty());
  CHECK(sib::logger().withCode(vdm::EventCode::NetTrialReverted).at(0).arg2 == -1);
}

TEST_CASE("net trial: a stale or undecodable record is erased without action") {
  glue::begin();
  vdm::Config old = staticConfig(kIp);
  vdm::Config n = staticConfig(kNewIp);
  vdm::Config other = staticConfig(IPAddress(10, 0, 0, 5));
  vdm::NetTrialRecord r;
  r.state = vdm::NetTrialState::Running;
  r.previous = old.net;
  r.trialCrc = vdm::netTrialFieldsCrc(other.net);
  sib::storage().netTrial = blob(r);
  net::begin(n);
  CHECK(n.net.ip == kNewIp);
  CHECK(sib::storage().netTrial.empty());
  CHECK(sib::storage().netTrialClears == 1);
  CHECK(sib::storage().applied.empty());
  CHECK_FALSE(sib::logger().has(vdm::EventCode::NetTrialReverted));
  glue::begin();
  sib::storage().netTrial = {1, 2, 3};
  vdm::Config m = staticConfig(kNewIp);
  net::begin(m);
  CHECK(sib::storage().netTrial.empty());
  CHECK(sib::storage().netTrialClears == 1);
  CHECK_FALSE(net::trialInfo().active);
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
