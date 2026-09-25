// Network policy: end-to-end reachability of the network and the watchdog
// that restarts the interface and then the ESP when it is lost.
// Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

// Traffic that proves the network works end to end: a gateway ping reply,
// the MQTT session, an SNTP sync, an HTTP request from a LAN peer, a DHCP
// lease (events, /api/health).
enum class NetEvidence : uint8_t { None = 0, GatewayPing = 1, Mqtt = 2, TimeSync = 3,
                                   InboundHttp = 4, DhcpLease = 5 };
const char* netEvidenceName(NetEvidence e);  // "none","ping","mqtt","ntp","http","dhcp"; "unknown"

// End-to-end reachability. Evidence = traffic that crossed the network. The
// staleness check is armed only by a gateway ping reply, so a gateway that
// never answers ICMP (or a broker outage) leaves the IP-only behaviour.
class NetReachability {
 public:
  static constexpr uint32_t kStaleMs = 150000;  // 2.5 probe intervals
  static constexpr uint32_t kProbeIntervalMs = 60000;
  enum class Change : uint8_t { None, Lost, Regained };
  // Every second. An IP transition (up <-> down) clears the evidence (not
  // proven, age none); the probe is due at once when the IP comes up. A
  // gateway change (incl. to 0) disarms the check and makes the probe due at
  // once. The armed flag survives an IP loss with the same gateway.
  void update(bool ipUp, uint32_t gateway, uint32_t nowMs);
  // Ignored while the IP is down and for None. GatewayPing arms the check.
  void onEvidence(NetEvidence e, uint32_t nowMs);
  // ipUp, gateway != 0, and no probe since the IP came up / the gateway
  // changed, or >= kProbeIntervalMs since the last one.
  bool probeDue(uint32_t nowMs) const;
  void onProbeSent(uint32_t nowMs);
  bool armed() const { return armed_; }
  bool ipUp() const { return ipUp_; }
  // ipUp and evidence since the IP came up (the OTA network check).
  bool proven() const { return ipUp_ && evidence_ != NetEvidence::None; }
  // ipUp and (not armed, or evidence younger than kStaleMs; the IP coming up
  // starts the clock when there is no evidence yet).
  bool reachable(uint32_t nowMs) const;
  NetEvidence lastEvidence() const { return evidence_; }  // None since the IP came up
  uint32_t evidenceAgeMs(uint32_t nowMs) const;  // UINT32_MAX when none since the IP came up
  // Transitions while the IP stays up: Lost when reachable() turns false with
  // the IP up, Regained when it turns true again after a Lost (an IP loss in
  // between cancels it; NetDown/NetUp report that). Once per second after
  // update()/onEvidence().
  Change change(uint32_t nowMs);
  uint32_t lostForMs(uint32_t nowMs) const;  // since the last Lost, 0 when none

 private:
  bool ipUp_ = false;
  bool started_ = false;
  uint32_t gateway_ = 0;
  uint32_t upMs_ = 0;
  bool armed_ = false;
  NetEvidence evidence_ = NetEvidence::None;
  uint32_t evidenceMs_ = 0;
  bool probeSent_ = false;
  uint32_t probeMs_ = 0;
  bool wasReachable_ = false;
  bool lost_ = false;
  bool lostKnown_ = false;
  uint32_t lostMs_ = 0;
};

// Network watchdog (legacy netConnTO). `reachable` is
// NetReachability::reachable(). After `minutes` without reachability the
// network interface is restarted (once per outage); after another waitMs()
// the ESP restarts (once per outage). 0 disables both. waitMs() grows by
// kGrowth with every ESP restart of the same outage, up to kMaxWaitMin: each
// ESP restart also resets the STM when jumper X20 is fitted, stopping motors
// and recalibrating every valve, and restarting again does not fix a switch
// that is off. The glue keeps restartsInOutage() across software restarts
// (RTC memory). The first minutes after boot count as well.
class NetWatchdog {
 public:
  static constexpr uint32_t kGrowth = 4;
  static constexpr uint32_t kMaxWaitMin = 24 * 60;
  enum class Action : uint8_t { None, RestartInterface, RestartEsp };

  void configure(uint8_t minutes);  // re-arms both actions
  // Watchdog restarts earlier in the outage that is still going on at boot
  // (0 after power-on or once the network was up).
  void setRestartsInOutage(uint8_t n) { restarts_ = n; }
  uint8_t restartsInOutage() const { return restarts_; }
  // Wait between the interface restart and the ESP restart:
  // minutes * kGrowth^restarts, capped.
  uint32_t waitMs() const;
  // Once per second.
  //  reachable              -> outage over: restarts 0, both actions re-armed, None
  //  first unreachable call -> outage starts (boot counts as a start)
  //  minutes == 0           -> None
  //  outage >= minutes*60000 + waitMs() -> RestartEsp once (restarts + 1, saturating)
  //  outage >= minutes*60000            -> RestartInterface once
  Action update(bool reachable, uint32_t nowMs);
  uint32_t outageMs(uint32_t nowMs) const;  // 0 while reachable
  uint16_t interfaceRestarts() const { return ifaceRestarts_; }  // since boot, saturating

 private:
  uint8_t minutes_ = 0;
  uint8_t restarts_ = 0;
  bool down_ = true;
  uint32_t downSinceMs_ = 0;
  bool ifaceFired_ = false;
  bool espFired_ = false;
  bool started_ = false;
  uint16_t ifaceRestarts_ = 0;
};

}  // namespace vdm
