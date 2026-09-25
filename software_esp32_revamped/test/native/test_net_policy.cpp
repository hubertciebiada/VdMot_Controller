// NetReachability, NetWatchdog, network evidence names.
#include <string>

#include "doctest.h"
#include "vdm/net_policy.h"

using namespace vdm;

using A = NetWatchdog::Action;
using C = NetReachability::Change;

namespace {
constexpr uint32_t kGw = 0x0101A8C0;  // 192.168.1.1
}

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

TEST_CASE("NetReachability: IP down, then up; probe, ping reply, staleness, Lost, Regained") {
  NetReachability r;
  CHECK_FALSE(r.ipUp());
  r.update(false, kGw, 0);
  CHECK_FALSE(r.reachable(0));
  CHECK_FALSE(r.probeDue(0));
  CHECK_FALSE(r.proven());
  CHECK(r.change(0) == C::None);
  r.update(true, kGw, 1000);
  CHECK(r.ipUp());
  CHECK(r.reachable(1000));
  CHECK_FALSE(r.armed());
  CHECK_FALSE(r.proven());
  CHECK(r.lastEvidence() == NetEvidence::None);
  CHECK(r.evidenceAgeMs(1000) == UINT32_MAX);
  CHECK(r.probeDue(1000));
  CHECK(r.change(1000) == C::None);
  r.onProbeSent(1000);
  CHECK_FALSE(r.probeDue(1000));
  CHECK_FALSE(r.probeDue(60999));
  CHECK(r.probeDue(61000));
  r.onEvidence(NetEvidence::GatewayPing, 1200);
  CHECK(r.armed());
  CHECK(r.proven());
  CHECK(r.lastEvidence() == NetEvidence::GatewayPing);
  CHECK(r.evidenceAgeMs(1300) == 100);
  CHECK(r.reachable(1200 + 149999));
  CHECK(r.change(1200 + 149999) == C::None);
  CHECK_FALSE(r.reachable(1200 + 150000));
  CHECK(r.change(1200 + 150000) == C::Lost);
  CHECK(r.change(1200 + 151000) == C::None);
  CHECK(r.lostForMs(1200 + 151000) == 1000);
  CHECK(r.proven());  // evidence since the IP came up, only stale
  r.onEvidence(NetEvidence::Mqtt, 200000);
  CHECK(r.lastEvidence() == NetEvidence::Mqtt);
  CHECK(r.armed());
  CHECK(r.change(200000) == C::Regained);
  CHECK(r.lostForMs(200000) == 200000 - 151200);
  CHECK(r.change(201000) == C::None);
}

TEST_CASE("NetReachability: never armed, the IP alone is reachable for ever") {
  NetReachability r;
  r.update(true, kGw, 0);
  r.onEvidence(NetEvidence::Mqtt, 10);
  CHECK_FALSE(r.armed());
  CHECK(r.proven());
  CHECK(r.reachable(4000000000u));
  CHECK(r.change(4000000000u) == C::None);
  CHECK(r.lostForMs(4000000000u) == 0);
}

TEST_CASE("NetReachability: a gateway change disarms and makes the probe due") {
  NetReachability r;
  r.update(true, kGw, 0);
  r.onProbeSent(0);
  r.onEvidence(NetEvidence::GatewayPing, 100);
  CHECK(r.armed());
  CHECK_FALSE(r.reachable(150100));
  r.update(true, kGw + 1, 150100);
  CHECK_FALSE(r.armed());
  CHECK(r.reachable(150100));
  CHECK(r.probeDue(150100));
  CHECK(r.proven());  // the evidence stays
  r.onProbeSent(150100);
  r.update(true, 0, 150200);  // to no gateway
  CHECK_FALSE(r.probeDue(150200));
  CHECK_FALSE(r.probeDue(400000));
  CHECK(r.reachable(400000));
}

TEST_CASE("NetReachability: an IP loss clears the evidence and cancels a Lost") {
  NetReachability r;
  r.update(true, kGw, 0);
  r.onEvidence(NetEvidence::GatewayPing, 0);
  CHECK(r.change(0) == C::None);
  CHECK(r.change(150000) == C::Lost);
  r.update(false, 0, 160000);
  CHECK_FALSE(r.proven());
  CHECK(r.lastEvidence() == NetEvidence::None);
  CHECK(r.evidenceAgeMs(160000) == UINT32_MAX);
  CHECK(r.lostForMs(160000) == 0);
  CHECK(r.change(160000) == C::None);  // the IP loss is NetDown's business
  CHECK(r.armed());                    // same gateway: stays armed
  r.onEvidence(NetEvidence::Mqtt, 161000);  // ignored while down
  CHECK(r.lastEvidence() == NetEvidence::None);
  r.update(true, kGw, 170000);
  CHECK(r.armed());
  CHECK(r.probeDue(170000));
  CHECK(r.reachable(170000));  // the IP coming up starts the clock
  CHECK(r.change(170000) == C::None);  // no Regained: the IP loss cancelled the Lost
  CHECK(r.reachable(170000 + 149999));
  CHECK_FALSE(r.reachable(170000 + 150000));
  CHECK(r.change(170000 + 150000) == C::Lost);
}

TEST_CASE("NetReachability: None evidence is ignored") {
  NetReachability r;
  r.update(true, kGw, 0);
  r.onEvidence(NetEvidence::None, 5);
  CHECK_FALSE(r.proven());
  CHECK(r.evidenceAgeMs(5) == UINT32_MAX);
  r.onEvidence(NetEvidence::DhcpLease, 6);
  CHECK(r.proven());
  CHECK_FALSE(r.armed());
  CHECK(r.evidenceAgeMs(10) == 4);
}

TEST_CASE("NetWatchdog: interface restart after minutes, ESP restart after another wait") {
  NetWatchdog w;
  w.configure(5);
  CHECK(w.update(false, 1000) == A::None);  // boot counts as the start of the outage
  CHECK(w.outageMs(1000) == 0);
  CHECK(w.update(false, 1000 + 299999) == A::None);
  CHECK(w.outageMs(1000 + 299999) == 299999);
  CHECK(w.update(false, 1000 + 300000) == A::RestartInterface);
  CHECK(w.interfaceRestarts() == 1);
  CHECK(w.update(false, 1000 + 300001) == A::None);  // once
  CHECK(w.update(false, 1000 + 599999) == A::None);
  CHECK(w.restartsInOutage() == 0);
  CHECK(w.update(false, 1000 + 600000) == A::RestartEsp);
  CHECK(w.restartsInOutage() == 1);
  CHECK(w.update(false, 1000 + 600001) == A::None);
  CHECK(w.update(false, 4000000000u) == A::None);
  CHECK(w.interfaceRestarts() == 1);
  CHECK(w.update(true, 4000001000u) == A::None);
  CHECK(w.restartsInOutage() == 0);
  CHECK(w.outageMs(4000001000u) == 0);
  // Re-armed: a new outage runs both stages again.
  CHECK(w.update(false, 4000002000u) == A::None);
  CHECK(w.update(false, 4000002000u + 300000) == A::RestartInterface);
  CHECK(w.interfaceRestarts() == 2);
  CHECK(w.update(false, 4000002000u + 600000) == A::RestartEsp);
}

TEST_CASE("NetWatchdog: one earlier restart in the outage: interface at 5 min, ESP at 25 min") {
  NetWatchdog w;
  w.configure(5);
  w.setRestartsInOutage(1);
  CHECK(w.update(false, 0) == A::None);
  CHECK(w.update(false, 299999) == A::None);
  CHECK(w.update(false, 300000) == A::RestartInterface);
  CHECK(w.update(false, 1499999) == A::None);
  CHECK(w.update(false, 1500000) == A::RestartEsp);
  CHECK(w.restartsInOutage() == 2);
}

TEST_CASE("NetWatchdog: 0 disables both actions") {
  NetWatchdog off;
  CHECK(off.update(false, 0) == A::None);
  CHECK(off.update(false, 4000000000u) == A::None);
  CHECK(off.interfaceRestarts() == 0);
  CHECK(off.outageMs(4000000000u) == 4000000000u);
}

TEST_CASE("NetWatchdog: configure re-arms after an action; enabled later counts the outage") {
  NetWatchdog late;
  CHECK(late.update(false, 0) == A::None);
  CHECK(late.update(false, 200000) == A::None);
  late.configure(2);
  CHECK(late.update(false, 200001) == A::RestartInterface);
  CHECK(late.update(false, 239999) == A::None);
  CHECK(late.update(false, 240000) == A::RestartEsp);
  late.configure(2);  // re-armed, the grown wait (2 min * 4) applies to the ESP stage
  CHECK(late.update(false, 240001) == A::RestartInterface);
  CHECK(late.update(false, 599999) == A::None);
  CHECK(late.update(false, 600000) == A::RestartEsp);
  late.configure(0);
  CHECK(late.update(false, 999999999) == A::None);
}

TEST_CASE("NetWatchdog: up at boot, lost later; up again resets the timer") {
  NetWatchdog u;
  u.configure(1);
  CHECK(u.update(true, 0) == A::None);
  CHECK(u.update(false, 10000) == A::None);
  CHECK(u.update(true, 69000) == A::None);
  CHECK(u.update(false, 70000) == A::None);
  CHECK(u.update(false, 129999) == A::None);
  CHECK(u.update(false, 130000) == A::RestartInterface);
  CHECK(u.update(false, 189999) == A::None);
  CHECK(u.update(false, 190000) == A::RestartEsp);
}

TEST_CASE("NetWatchdog: maximum setting") {
  NetWatchdog max;
  max.configure(255);
  CHECK(max.update(false, 0) == A::None);
  CHECK(max.update(false, 255u * 60000u - 1) == A::None);
  CHECK(max.update(false, 255u * 60000u) == A::RestartInterface);
  CHECK(max.update(false, 510u * 60000u - 1) == A::None);
  CHECK(max.update(false, 510u * 60000u) == A::RestartEsp);
}

TEST_CASE("NetWatchdog: waitMs grows with every restart of one outage") {
  // Router off for hours: ESP restarts 5, 20, 80, 320, 1280 min after the
  // interface restart, then every 24 h (each one also resets the STM).
  const uint32_t expectMin[] = {5, 20, 80, 320, 1280, 1440, 1440};
  uint8_t kept = 0;  // what the glue keeps in RTC memory across restarts
  for (uint32_t m : expectMin) {
    CAPTURE(m);
    NetWatchdog w;  // a fresh boot
    w.configure(5);
    w.setRestartsInOutage(kept);
    CHECK(w.waitMs() == m * 60000u);
    CHECK(w.update(false, 700) == A::None);
    CHECK(w.update(false, 700 + 300000) == A::RestartInterface);
    CHECK(w.update(false, 700 + 300000 + m * 60000u - 1) == A::None);
    CHECK(w.update(false, 700 + 300000 + m * 60000u) == A::RestartEsp);
    kept = w.restartsInOutage();
  }
  CHECK(kept == 7);

  NetWatchdog big;
  big.configure(240);
  big.setRestartsInOutage(255);
  CHECK(big.waitMs() == 1440u * 60000u);
  CHECK(big.update(false, 0) == A::None);
  CHECK(big.update(false, 1680u * 60000u) == A::RestartEsp);
  CHECK(big.restartsInOutage() == 255);
  NetWatchdog one;
  one.configure(1);
  one.setRestartsInOutage(1);
  CHECK(one.waitMs() == 4u * 60000u);
  one.configure(0);
  CHECK(one.waitMs() == 0);
}

TEST_CASE("NetWatchdog: interface restarts saturate") {
  NetWatchdog w;
  w.configure(1);
  uint32_t t = 0;
  for (uint32_t i = 0; i < 65536; ++i) {
    w.update(false, t);
    w.update(false, t + 60000);
    w.update(true, t + 61000);
    t += 62000;
  }
  CHECK(w.interfaceRestarts() == 65535);
}
