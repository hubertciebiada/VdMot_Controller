// Network policy: when a lost network justifies an ESP restart.
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

// Network watchdog (legacy netConnTO): the ESP restarts after `minutes`
// consecutive minutes without an IP address. 0 disables it. The first
// minutes after boot count as well.
// Unlike the legacy firmware (a restart every N minutes for as long as the
// outage lasts), the wait grows by kGrowth with every watchdog restart of the
// same outage, up to kMaxWaitMin: each ESP restart also resets the STM
// (IO15 strap, specs/06 §5.2), stopping motors and recalibrating every valve,
// and restarting again does not fix a switch that is off. The glue keeps
// restartsInOutage() across software restarts (RTC memory).
class NetWatchdog {
 public:
  static constexpr uint32_t kGrowth = 4;
  static constexpr uint32_t kMaxWaitMin = 24 * 60;

  void configure(uint8_t minutes);
  // Watchdog restarts earlier in the outage that is still going on at boot
  // (0 after power-on or once the network was up).
  void setRestartsInOutage(uint8_t n) { restarts_ = n; }
  uint8_t restartsInOutage() const { return restarts_; }
  // Wait before the next restart: minutes * kGrowth^restarts, capped.
  uint32_t waitMs() const;
  // Call once per second. True = restart the ESP now (returned once per
  // outage and configure(); counts the restart).
  bool update(bool netUp, uint32_t nowMs);

 private:
  uint8_t minutes_ = 0;
  uint8_t restarts_ = 0;
  bool down_ = true;
  uint32_t downSinceMs_ = 0;
  bool fired_ = false;
  bool started_ = false;
};

}  // namespace vdm
