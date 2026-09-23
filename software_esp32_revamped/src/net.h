// Network: Ethernet (LAN8720) first, WiFi STA when configured, DHCP or
// static, hostname = station name, SNTP with the POSIX TZ string, mDNS.
// Event-driven (WiFi/ETH events update the state); service() runs the
// reconnect policy from the app task.
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
  int8_t rssi = 0;
  uint32_t upSinceMs = 0;
  uint32_t reconnects = 0;
};

// Starts the interfaces per config (iface Auto: ETH and, if an SSID is set,
// WiFi; the first to get an IP wins, WiFi is stopped when ETH gets an IP and
// restarted when ETH loses its link for > 30 s). Also starts SNTP when
// ntpServer != "" and sets TZ.
void begin(const vdm::Config& cfg);

// App task, every second: state/IP refresh (NetUp/NetDown events), WiFi as
// fallback after 30 s without Ethernet IP (Auto) with reconnect back-off
// 5 s .. 60 s, WiFi off again once Ethernet has an IP, mDNS announce,
// NetWatchdog (legacy netConnTO) -> ota::requestRestart(2).
void service(uint32_t nowMs);

bool isUp();
Info info();

// Local time; valid only after SNTP sync (year >= 2020).
vdm::LocalTime localTime();
bool timeValid();
uint32_t lastSyncEpoch();

// Applies changed network settings. TZ/NTP apply live; interface, address,
// WiFi credential or station-name changes schedule an ESP restart (the STM
// keeps running, architecture R6) because the Arduino ETH driver cannot be
// re-initialised at run time.
void reconfigure(const vdm::Config& cfg);

}  // namespace net
