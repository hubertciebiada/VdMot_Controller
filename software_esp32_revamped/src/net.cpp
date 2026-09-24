#include "net.h"

#include <Arduino.h>
#include <ESPmDNS.h>
#include <ETH.h>
#include <WiFi.h>
#include <esp_attr.h>
#include <esp_sntp.h>
#include <freertos/FreeRTOS.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>

#include <vdm/event_log.h>
#include <vdm/health_monitor.h>

#include "board.h"
#include "boot_alloc.h"
#include "logger.h"
#include "ota.h"

namespace net {

namespace {

constexpr int64_t kMinValidEpoch = 1577836800;  // 2020-01-01: before that SNTP has not run
constexpr uint32_t kWifiFallbackMs = 30000;     // Auto: WiFi after 30 s without Ethernet
constexpr uint32_t kWifiBackoffMinMs = 5000;
constexpr uint32_t kWifiBackoffMaxMs = 60000;

// NetWatchdog restarts of the outage in progress, kept across software
// restarts (RTC slow memory is not cleared by esp_restart() or a panic;
// after power-on it holds garbage, hence the check word).
constexpr uint32_t kRtcMagic = 0x564E5744;  // "VNWD"
RTC_NOINIT_ATTR uint32_t gRtcWdMagic;
RTC_NOINIT_ATTR uint32_t gRtcWdRestarts;

uint8_t loadOutageRestarts() {
  if (gRtcWdMagic != kRtcMagic || gRtcWdRestarts > UINT8_MAX) return 0;
  return static_cast<uint8_t>(gRtcWdRestarts);
}

void storeOutageRestarts(uint8_t n) {
  gRtcWdRestarts = n;
  gRtcWdMagic = kRtcMagic;
}

portMUX_TYPE gMux = portMUX_INITIALIZER_UNLOCKED;
// Written from the system event task (flags only), read by the app task.
volatile bool gEthLink = false;
volatile bool gEthIp = false;
volatile bool gWifiUp = false;
volatile bool gStaticIp = false;
volatile uint32_t gSyncCount = 0;
volatile uint32_t gLastSyncEpoch = 0;
Info gInfo;  // guarded by gMux

// App task only.
vdm::Config& gCfg = bootAlloc<vdm::Config>();
vdm::NetWatchdog gWatchdog;
bool gMdnsStarted = false;
bool gEthStarted = false;
bool gWifiStarted = false;
uint32_t gEthDownSinceMs = 0;
bool gEthDownKnown = false;
vdm::Backoff gWifiBackoff(kWifiBackoffMinMs, kWifiBackoffMaxMs);
uint32_t gSyncSeen = 0;
bool gTimeSyncedOnce = false;
int64_t gClockRefEpoch = 0;  // wall clock at the previous service() call
uint32_t gClockRefMs = 0;
// Hostname buffer: the ETH driver keeps the pointer until DHCP runs.
char gHostname[vdm::kStationNameMax + 1] = "VdMot";

void onTimeSync(struct timeval* tv) {
  gLastSyncEpoch = tv != nullptr && tv->tv_sec > 0 ? static_cast<uint32_t>(tv->tv_sec) : 0;
  gSyncCount = gSyncCount + 1;
}

bool ethUp() { return gEthLink && (gEthIp || gStaticIp); }

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
  if (!gWifiStarted) {
    WiFi.persistent(false);  // credentials come from vdmrev, never from the WiFi NVS
    WiFi.mode(WIFI_STA);
    WiFi.setHostname(gHostname);
    WiFi.setAutoReconnect(true);
    applyStaticIp(false);
    gWifiStarted = true;
  }
  WiFi.begin(gCfg.net.ssid, gCfg.net.wifiPassword);
}

void stopWifi() {
  if (!gWifiStarted) return;
  WiFi.disconnect(true);
  WiFi.mode(WIFI_OFF);
  gWifiStarted = false;
  gWifiUp = false;
}

void onEvent(arduino_event_id_t event, arduino_event_info_t) {
  switch (event) {
    case ARDUINO_EVENT_ETH_START:
      ETH.setHostname(gHostname);
      break;
    case ARDUINO_EVENT_ETH_CONNECTED:
      gEthLink = true;
      break;
    case ARDUINO_EVENT_ETH_GOT_IP:
      gEthIp = true;
      break;
    case ARDUINO_EVENT_ETH_DISCONNECTED:
      gEthLink = false;
      gEthIp = false;  // DHCP renews after the link returns
      break;
    case ARDUINO_EVENT_ETH_STOP:
      gEthLink = false;
      gEthIp = false;
      break;
    case ARDUINO_EVENT_WIFI_STA_GOT_IP:
      gWifiUp = true;
      break;
    case ARDUINO_EVENT_WIFI_STA_DISCONNECTED:
    case ARDUINO_EVENT_WIFI_STA_LOST_IP:
    case ARDUINO_EVENT_WIFI_STA_STOP:
      gWifiUp = false;
      break;
    default:
      break;
  }
}

void refreshInfo(uint32_t nowMs) {
  Info i;
  if (ethUp()) {
    i.state = vdm::NetState::Ethernet;
    i.ip = ETH.localIP();
    i.mask = ETH.subnetMask();
    i.gateway = ETH.gatewayIP();
    i.dns = ETH.dnsIP();
    snprintf(i.mac, sizeof i.mac, "%s", ETH.macAddress().c_str());
  } else if (gWifiUp) {
    i.state = vdm::NetState::Wifi;
    i.ip = WiFi.localIP();
    i.mask = WiFi.subnetMask();
    i.gateway = WiFi.gatewayIP();
    i.dns = WiFi.dnsIP();
    const int rssi = WiFi.RSSI();
    i.rssi = static_cast<int8_t>(rssi < -128 ? -128 : (rssi > 0 ? 0 : rssi));
    snprintf(i.mac, sizeof i.mac, "%s", WiFi.macAddress().c_str());
  }
  if (i.state != vdm::NetState::Down && i.ip == 0) i.state = vdm::NetState::Down;
  vdm::NetState before;
  portENTER_CRITICAL(&gMux);
  before = gInfo.state;
  const bool changed = i.state != gInfo.state || i.ip != gInfo.ip;
  i.upSinceMs = changed ? nowMs : gInfo.upSinceMs;
  i.reconnects = gInfo.reconnects + ((changed && i.state != vdm::NetState::Down) ? 1 : 0);
  gInfo = i;
  portEXIT_CRITICAL(&gMux);
  if (!changed) return;
  if (i.state == vdm::NetState::Down) {
    logger::log(vdm::EventCode::NetDown, vdm::kNoValve, static_cast<int32_t>(before));
  } else {
    char ip[16];
    vdm::formatIpv4(i.ip, ip, sizeof ip);
    logger::log(vdm::EventCode::NetUp, vdm::kNoValve, static_cast<int32_t>(i.state), 0, ip);
  }
}

void applyTime(const vdm::Config& cfg) {
  if (cfg.time.ntpServer[0] != '\0') {
    // configTzTime copies neither string; the config copy (gCfg) outlives it.
    configTzTime(cfg.time.tzPosix, cfg.time.ntpServer);
  } else {
    if (sntp_enabled()) sntp_stop();
    setenv("TZ", cfg.time.tzPosix, 1);
    tzset();
  }
}

// TimeSynced: step = wall clock after the sync minus the clock expected from
// the previous observation (Info the first time, Debug afterwards).
void checkTimeSync(uint32_t nowMs) {
  struct timeval tv;
  gettimeofday(&tv, nullptr);
  const int64_t nowEpoch = tv.tv_sec;
  const uint32_t count = gSyncCount;
  if (count != gSyncSeen) {
    gSyncSeen = count;
    const int64_t expected = gClockRefEpoch + vdm::elapsedMs(nowMs, gClockRefMs) / 1000;
    int64_t step = nowEpoch - expected;
    if (step > INT32_MAX) step = INT32_MAX;
    if (step < INT32_MIN) step = INT32_MIN;
    logger::logSev(vdm::EventCode::TimeSynced,
                   gTimeSyncedOnce ? vdm::Severity::Debug : vdm::Severity::Info, vdm::kNoValve,
                   static_cast<int32_t>(step));
    gTimeSyncedOnce = true;
  }
  gClockRefEpoch = nowEpoch;
  gClockRefMs = nowMs;
}

}  // namespace

void begin(const vdm::Config& cfg) {
  gCfg = cfg;
  // DHCP/mDNS need a host name; the station name may hold spaces and UTF-8.
  vdm::buildHostname(cfg.station, gHostname, sizeof gHostname);
  gStaticIp = !cfg.net.dhcp;
  gWatchdog.configure(cfg.net.reconnectTimeoutMin);
  gWatchdog.setRestartsInOutage(loadOutageRestarts());
  WiFi.onEvent(onEvent);
  if (cfg.net.iface != vdm::NetInterface::Wifi) {
    gEthStarted = ETH.begin(board::kEthPhyAddr, board::kEthPhyPower, board::kEthMdc,
                            board::kEthMdio, ETH_PHY_LAN8720, ETH_CLOCK_GPIO0_IN);
    if (gEthStarted) {
      applyStaticIp(true);
    } else {
      logger::log(vdm::EventCode::NetDown, vdm::kNoValve, 1, 0, "eth init failed");
    }
  }
  if (cfg.net.iface == vdm::NetInterface::Wifi && wifiWanted()) startWifi();
  sntp_set_time_sync_notification_cb(onTimeSync);
  applyTime(gCfg);
  gClockRefEpoch = time(nullptr);
  gClockRefMs = millis();
}

void service(uint32_t nowMs) {
  refreshInfo(nowMs);
  checkTimeSync(nowMs);
  const bool eth = ethUp();

  // Ethernet preferred: WiFi off while Ethernet has an IP (Auto mode).
  if (gCfg.net.iface == vdm::NetInterface::Auto && eth && gWifiStarted) stopWifi();

  if (eth || !gEthStarted) {
    gEthDownKnown = false;
  } else if (!gEthDownKnown) {
    gEthDownKnown = true;
    gEthDownSinceMs = nowMs;
  }
  // WiFi when it is the configured interface, or the Auto fallback after
  // kWifiFallbackMs without Ethernet (or when Ethernet failed to start).
  const bool wifiNeeded =
      wifiWanted() && !eth &&
      (gCfg.net.iface == vdm::NetInterface::Wifi || !gEthStarted ||
       (gEthDownKnown && vdm::elapsedMs(nowMs, gEthDownSinceMs) >= kWifiFallbackMs));
  if (gWifiUp || !wifiNeeded) {
    gWifiBackoff.reset();
  } else if (gWifiBackoff.due(nowMs)) {
    startWifi();
    gWifiBackoff.onFailure(nowMs);  // counts as failed until gWifiUp
  }

  const bool up = isUp();
  if (up && !gMdnsStarted) {
    gMdnsStarted = MDNS.begin(gHostname);
    if (gMdnsStarted) MDNS.addService("http", "tcp", 80);
  }
  const bool restart = gWatchdog.update(up, nowMs);
  if (gWatchdog.restartsInOutage() != loadOutageRestarts()) {
    storeOutageRestarts(gWatchdog.restartsInOutage());
  }
  if (restart) ota::requestRestart(2, 1000);
}

bool isUp() {
  portENTER_CRITICAL(&gMux);
  const bool up = gInfo.state != vdm::NetState::Down;
  portEXIT_CRITICAL(&gMux);
  return up;
}

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
  if (tv.tv_sec < kMinValidEpoch) return t;  // not synced yet
  struct tm tm;
  if (localtime_r(&tv.tv_sec, &tm) == nullptr) return t;
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

bool timeValid() { return time(nullptr) >= kMinValidEpoch; }

uint32_t lastSyncEpoch() { return gLastSyncEpoch; }

void reconfigure(const vdm::Config& cfg) {
  const bool netChanged = cfg.net.iface != gCfg.net.iface || cfg.net.dhcp != gCfg.net.dhcp ||
                          cfg.net.ip != gCfg.net.ip || cfg.net.mask != gCfg.net.mask ||
                          cfg.net.gateway != gCfg.net.gateway || cfg.net.dns != gCfg.net.dns ||
                          strcmp(cfg.net.ssid, gCfg.net.ssid) != 0 ||
                          strcmp(cfg.net.wifiPassword, gCfg.net.wifiPassword) != 0 ||
                          strcmp(cfg.station, gCfg.station) != 0;
  const bool timeChanged = strcmp(cfg.time.ntpServer, gCfg.time.ntpServer) != 0 ||
                           strcmp(cfg.time.tzPosix, gCfg.time.tzPosix) != 0;
  gCfg = cfg;
  gWatchdog.configure(cfg.net.reconnectTimeoutMin);
  if (timeChanged) applyTime(gCfg);
  // Interface/address/hostname changes need a clean start of the network
  // stack (the Arduino ETH driver cannot be re-initialised): restart the
  // ESP after the HTTP response went out. The STM keeps running (R6).
  if (netChanged) ota::requestRestart(0, 1500);
}

}  // namespace net
