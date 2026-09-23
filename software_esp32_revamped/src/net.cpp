#include "net.h"

#include <Arduino.h>
#include <ESPmDNS.h>
#include <ETH.h>
#include <WiFi.h>
#include <esp_sntp.h>
#include <freertos/FreeRTOS.h>
#include <sys/time.h>
#include <time.h>

#include <vdm/event_log.h>
#include <vdm/health_monitor.h>

#include "board.h"
#include "logger.h"
#include "ota.h"

namespace net {

namespace {

portMUX_TYPE gMux = portMUX_INITIALIZER_UNLOCKED;
// Written from the WiFi/ETH event task, read everywhere (guarded by gMux).
volatile bool gEthUp = false;
volatile bool gWifiUp = false;
Info gInfo;

vdm::Config gCfg;  // only touched by begin()/reconfigure()/service() (app task)
vdm::NetWatchdog gWatchdog;
volatile uint32_t gLastSyncEpoch = 0;
bool gMdnsStarted = false;
uint32_t gEthDownSinceMs = 0;
uint32_t gWifiRetryAtMs = 0;
uint32_t gWifiBackoffMs = 5000;

void onTimeSync(struct timeval* tv) {
  gLastSyncEpoch = static_cast<uint32_t>(tv->tv_sec);
}

void applyStaticIp(bool eth) {
  if (gCfg.net.dhcp) return;
  const IPAddress ip(gCfg.net.ip), gw(gCfg.net.gateway), mask(gCfg.net.mask), dns(gCfg.net.dns);
  if (eth) {
    ETH.config(ip, gw, mask, dns);
  } else {
    WiFi.config(ip, gw, mask, dns);
  }
}

bool wifiWanted() {
  return gCfg.net.iface != vdm::NetInterface::Ethernet && gCfg.net.ssid[0] != '\0';
}

void startWifi() {
  WiFi.mode(WIFI_STA);
  WiFi.setHostname(gCfg.station);
  applyStaticIp(false);
  WiFi.begin(gCfg.net.ssid, gCfg.net.wifiPassword);
}

void onEvent(arduino_event_id_t event, arduino_event_info_t) {
  switch (event) {
    case ARDUINO_EVENT_ETH_START:
      ETH.setHostname(gCfg.station);
      break;
    case ARDUINO_EVENT_ETH_GOT_IP:
      portENTER_CRITICAL(&gMux);
      gEthUp = true;
      portEXIT_CRITICAL(&gMux);
      break;
    case ARDUINO_EVENT_ETH_DISCONNECTED:
    case ARDUINO_EVENT_ETH_STOP:
      portENTER_CRITICAL(&gMux);
      gEthUp = false;
      portEXIT_CRITICAL(&gMux);
      break;
    case ARDUINO_EVENT_WIFI_STA_GOT_IP:
      portENTER_CRITICAL(&gMux);
      gWifiUp = true;
      portEXIT_CRITICAL(&gMux);
      break;
    case ARDUINO_EVENT_WIFI_STA_DISCONNECTED:
    case ARDUINO_EVENT_WIFI_STA_LOST_IP:
      portENTER_CRITICAL(&gMux);
      gWifiUp = false;
      portEXIT_CRITICAL(&gMux);
      break;
    default:
      break;
  }
}

void refreshInfo(uint32_t nowMs) {
  Info i;
  const bool eth = gEthUp;
  const bool wifi = gWifiUp;
  if (eth) {
    i.state = vdm::NetState::Ethernet;
    i.ip = ETH.localIP();
    i.mask = ETH.subnetMask();
    i.gateway = ETH.gatewayIP();
    i.dns = ETH.dnsIP();
    snprintf(i.mac, sizeof i.mac, "%s", ETH.macAddress().c_str());
  } else if (wifi) {
    i.state = vdm::NetState::Wifi;
    i.ip = WiFi.localIP();
    i.mask = WiFi.subnetMask();
    i.gateway = WiFi.gatewayIP();
    i.dns = WiFi.dnsIP();
    i.rssi = static_cast<int8_t>(WiFi.RSSI());
    snprintf(i.mac, sizeof i.mac, "%s", WiFi.macAddress().c_str());
  }
  portENTER_CRITICAL(&gMux);
  const bool changed = i.state != gInfo.state || i.ip != gInfo.ip;
  i.upSinceMs = changed ? nowMs : gInfo.upSinceMs;
  i.reconnects = gInfo.reconnects + ((changed && i.state != vdm::NetState::Down) ? 1 : 0);
  gInfo = i;
  portEXIT_CRITICAL(&gMux);
  if (changed) {
    char ip[16];
    vdm::formatIpv4(i.ip, ip, sizeof ip);
    if (i.state == vdm::NetState::Down) {
      logger::log(vdm::EventCode::NetDown);
    } else {
      logger::log(vdm::EventCode::NetUp, vdm::kNoValve, static_cast<int32_t>(i.state), 0, ip);
    }
  }
}

}  // namespace

void begin(const vdm::Config& cfg) {
  gCfg = cfg;
  gWatchdog.configure(cfg.net.reconnectTimeoutMin);
  WiFi.onEvent(onEvent);
  if (cfg.net.iface != vdm::NetInterface::Wifi) {
    ETH.begin(board::kEthPhyAddr, board::kEthPhyPower, board::kEthMdc, board::kEthMdio,
              ETH_PHY_LAN8720, ETH_CLOCK_GPIO0_IN);
    applyStaticIp(true);
  }
  if (wifiWanted()) startWifi();
  sntp_set_time_sync_notification_cb(onTimeSync);
  if (cfg.time.ntpServer[0] != '\0') {
    configTzTime(cfg.time.tzPosix, cfg.time.ntpServer);
  } else {
    setenv("TZ", cfg.time.tzPosix, 1);
    tzset();
  }
}

void service(uint32_t nowMs) {
  refreshInfo(nowMs);
  const bool up = isUp();

  // Ethernet preferred: stop WiFi while ETH has an IP (Auto mode).
  if (gCfg.net.iface == vdm::NetInterface::Auto && gEthUp && WiFi.getMode() != WIFI_OFF) {
    WiFi.disconnect(true);
    WiFi.mode(WIFI_OFF);
  }
  if (!gEthUp) {
    if (gEthDownSinceMs == 0) gEthDownSinceMs = nowMs ? nowMs : 1;
  } else {
    gEthDownSinceMs = 0;
  }
  // WiFi reconnect with back-off when it is the active or fallback path.
  const bool wifiNeeded =
      wifiWanted() && !gEthUp &&
      (gCfg.net.iface == vdm::NetInterface::Wifi ||
       (gEthDownSinceMs != 0 && vdm::elapsedMs(nowMs, gEthDownSinceMs) >= 30000));
  if (wifiNeeded && !gWifiUp && vdm::timeReached(nowMs, gWifiRetryAtMs)) {
    startWifi();
    gWifiRetryAtMs = nowMs + gWifiBackoffMs;
    gWifiBackoffMs = gWifiBackoffMs >= 30000 ? 60000 : gWifiBackoffMs * 2;
  } else if (gWifiUp) {
    gWifiBackoffMs = 5000;
  }

  if (up && !gMdnsStarted) {
    gMdnsStarted = MDNS.begin(gCfg.station);
    if (gMdnsStarted) MDNS.addService("http", "tcp", 80);
  }
  if (gWatchdog.update(up, nowMs)) ota::requestRestart(2, 1000);
}

bool isUp() { return gEthUp || gWifiUp; }

Info info() {
  portENTER_CRITICAL(&gMux);
  const Info i = gInfo;
  portEXIT_CRITICAL(&gMux);
  return i;
}

vdm::LocalTime localTime() {
  vdm::LocalTime t;
  struct timeval tv;
  gettimeofday(&tv, nullptr);
  if (tv.tv_sec < 1577836800) return t;  // before 2020: not synced
  struct tm tm;
  localtime_r(&tv.tv_sec, &tm);
  t.valid = true;
  t.year = static_cast<uint16_t>(tm.tm_year + 1900);
  t.month = static_cast<uint8_t>(tm.tm_mon + 1);
  t.mday = static_cast<uint8_t>(tm.tm_mday);
  t.wday = static_cast<uint8_t>(tm.tm_wday);
  t.hour = static_cast<uint8_t>(tm.tm_hour);
  t.minute = static_cast<uint8_t>(tm.tm_min);
  t.second = static_cast<uint8_t>(tm.tm_sec);
  t.epoch = tv.tv_sec;
  return t;
}

bool timeValid() { return localTime().valid; }

uint32_t lastSyncEpoch() { return gLastSyncEpoch; }

void reconfigure(const vdm::Config& cfg) {
  const bool netChanged = cfg.net.iface != gCfg.net.iface || cfg.net.dhcp != gCfg.net.dhcp ||
                          cfg.net.ip != gCfg.net.ip || cfg.net.mask != gCfg.net.mask ||
                          cfg.net.gateway != gCfg.net.gateway || cfg.net.dns != gCfg.net.dns ||
                          strcmp(cfg.net.ssid, gCfg.net.ssid) != 0 ||
                          strcmp(cfg.net.wifiPassword, gCfg.net.wifiPassword) != 0 ||
                          strcmp(cfg.station, gCfg.station) != 0;
  gCfg = cfg;
  gWatchdog.configure(cfg.net.reconnectTimeoutMin);
  if (cfg.time.ntpServer[0] != '\0') {
    configTzTime(cfg.time.tzPosix, cfg.time.ntpServer);
  } else {
    sntp_stop();
    setenv("TZ", cfg.time.tzPosix, 1);
    tzset();
  }
  // Interface changes need a clean restart of the network stack; the device
  // restarts (the STM keeps running, R6).
  if (netChanged) ota::requestRestart(0, 1500);
}

}  // namespace net
