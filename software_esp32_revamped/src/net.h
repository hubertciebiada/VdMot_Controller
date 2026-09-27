// Network: Ethernet (LAN8720) only, DHCP or static, hostname = station
// name, SNTP with the POSIX TZ string. Event-driven (the ETH events update
// the state); service() runs the reconnect policy from the app task.
#pragma once

#include <stdint.h>

#include <vdm/common.h>
#include <vdm/config.h>
#include <vdm/json_api.h>

namespace net {

struct Info {
  vdm::NetState state = vdm::NetState::Down;
  uint32_t ip = 0, mask = 0, gateway = 0, dns = 0;
  char mac[18] = {0};
  uint32_t upSinceMs = 0;
  uint32_t reconnects = 0;
};

// Starts Ethernet with the configured addresses. Also starts SNTP when
// ntpServer != "" and sets TZ. May put back the previous network settings
// into `cfg` (an interrupted network trial).
void begin(vdm::Config& cfg);

// App task, every second: state/IP refresh (NetUp/NetDown events),
// end-to-end reachability (gateway ping every 60 s, evidence,
// NetUnreachable / NetReachable), the network trial, and the NetWatchdog
// (legacy netConnTO): Ethernet restart after reconnectTimeoutMin, then
// ota::requestRestart(2) with a wait growing per restart of one outage.
// mqttConnected: the MQTT session is up (proof that the network works end
// to end).
void service(uint32_t nowMs, bool mqttConnected);

bool isUp();
Info info();
// Reachability and trial state for /api/health.
vdm::NetHealthInfo health(uint32_t nowMs);
// The network check of the OTA validator: reachable and proven end to end.
bool otaNetOk();
// Host name in use (from the station name).
const char* hostname();
// A request from `remoteIp` reached the web server (evidence of a working
// network; loopback and the own address are ignored).
void noteInboundHttp(uint32_t remoteIp);

// Network trial: true when a trial runs (acted on in the next service()).
bool requestTrialConfirm();
bool requestTrialRevert();
struct TrialInfo {
  bool active = false;
  uint32_t remainS = 0;
};
TrialInfo trialInfo();

// Local time; valid only after SNTP sync (year >= 2020).
vdm::LocalTime localTime();
bool timeValid();
uint32_t lastSyncEpoch();

// Applies changed network settings. TZ/NTP apply live; changes of
// vdm::configRestartReasons() (DHCP, static address, host name) schedule an
// ESP restart because the Arduino ETH driver cannot be re-initialised at run
// time; with jumper X20 fitted the ESP restart also resets the STM
// (INSTALL.md). A change of the trial fields runs on trial.
void reconfigure(const vdm::Config& cfg);

}  // namespace net
