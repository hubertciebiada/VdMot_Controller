#include "net.h"

#include <Arduino.h>
#include <ESPmDNS.h>
#include <ETH.h>
#include <WiFi.h>
#include <esp_attr.h>
#include <esp_eth.h>
#include <esp_sntp.h>
#include <freertos/FreeRTOS.h>
#include <freertos/task.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>

#include "ping/ping_sock.h"

#include <vdm/event_log.h>
#include <vdm/net_policy.h>
#include <vdm/net_trial.h>

#include "board.h"
#include "boot_alloc.h"
#include "logger.h"
#include "ota.h"
#include "storage.h"

namespace net {

namespace {

constexpr int64_t kMinValidEpoch = 1577836800;  // 2020-01-01: before that SNTP has not run
constexpr uint32_t kWifiFallbackMs = 30000;     // Auto: WiFi after 30 s without Ethernet
constexpr uint32_t kWifiBackoffMinMs = 5000;
constexpr uint32_t kWifiBackoffMaxMs = 60000;
constexpr uint32_t kLoopbackNet = 127;          // 127.x.x.x (first octet in the low byte)

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
// Written from the system event task, the ping task or a web handler
// (flags and counters only), read by the app task.
volatile bool gEthLink = false;
volatile bool gEthIp = false;
volatile bool gWifiUp = false;
volatile bool gStaticIp = false;
volatile uint32_t gSyncCount = 0;
volatile uint32_t gLastSyncEpoch = 0;
volatile uint32_t gGotIpCount = 0;
volatile uint32_t gPingReplies = 0;
volatile uint32_t gInboundCount = 0;
volatile bool gConfirmRequest = false;
volatile bool gRevertRequest = false;
esp_eth_handle_t gEthHandle = nullptr;  // guarded by gMux
Info gInfo;                             // guarded by gMux
vdm::NetHealthInfo gHealth;             // guarded by gMux
bool gTrialActive = false;              // guarded by gMux
uint32_t gTrialRemainS = 0;             // guarded by gMux

// App task only.
vdm::Config& gCfg = bootAlloc<vdm::Config>();
vdm::NetWatchdog gWatchdog;
vdm::NetReachability gReach;
vdm::NetTrial gTrial;
vdm::NetConfig gTrialPrev;     // settings before the running trial
vdm::NetConfig gArmedPrev;     // previous settings of the record armed during this boot
bool gArmedThisBoot = false;
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
uint32_t gGotIpSeen = 0;
uint32_t gPingSeen = 0;
uint32_t gInboundSeen = 0;
esp_ping_handle_t gPing = nullptr;
uint32_t gPingTarget = 0;
// Hostname buffer: the ETH driver keeps the pointer until DHCP runs.
char gHostname[vdm::kStationNameMax + 1] = "VdMot";

void onTimeSync(struct timeval* tv) {
  gLastSyncEpoch = tv != nullptr && tv->tv_sec > 0 ? static_cast<uint32_t>(tv->tv_sec) : 0;
  gSyncCount = gSyncCount + 1;
}

void onPingSuccess(esp_ping_handle_t, void*) { gPingReplies = gPingReplies + 1; }

bool ethUp() { return gEthLink && (gEthIp || gStaticIp); }

void applyStaticIp(bool eth) {
  if (gCfg.net.dhcp) return;
  // Without a DNS server the gateway resolves (NTP, broker names).
  const IPAddress ip(gCfg.net.ip), gw(gCfg.net.gateway), mask(gCfg.net.mask),
      dns(vdm::effectiveDns(gCfg.net));
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

void onEvent(arduino_event_id_t event, arduino_event_info_t info) {
  switch (event) {
    case ARDUINO_EVENT_ETH_START:
      ETH.setHostname(gHostname);
      break;
    case ARDUINO_EVENT_ETH_CONNECTED:
      // ETHClass keeps its driver handle private; the event carries it
      // (interface restart of the watchdog).
      portENTER_CRITICAL(&gMux);
      gEthHandle = info.eth_connected;
      portEXIT_CRITICAL(&gMux);
      gEthLink = true;
      break;
    case ARDUINO_EVENT_ETH_GOT_IP:
      gEthIp = true;
      gGotIpCount = gGotIpCount + 1;
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
      gGotIpCount = gGotIpCount + 1;
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
// the previous observation (Info the first time, Debug afterwards). True
// when a sync happened since the last call.
bool checkTimeSync(uint32_t nowMs) {
  struct timeval tv;
  gettimeofday(&tv, nullptr);
  const int64_t nowEpoch = tv.tv_sec;
  const uint32_t count = gSyncCount;
  const bool synced = count != gSyncSeen;
  if (synced) {
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
  return synced;
}

// One ICMP echo to the gateway (esp_ping, own task); the reply counts as
// evidence in a later service() call. A session that cannot start is tried
// again at the next probe.
void probeGateway(uint32_t gateway) {
  if (gPing != nullptr && gPingTarget != gateway) {
    esp_ping_delete_session(gPing);
    gPing = nullptr;
  }
  if (gPing == nullptr) {
    esp_ping_config_t cfg = ESP_PING_DEFAULT_CONFIG();
    cfg.count = 1;
    cfg.timeout_ms = 1000;
    cfg.interval_ms = 1000;
    cfg.data_size = 32;
    cfg.target_addr.u_addr.ip4.addr = gateway;  // same byte order as the legacy uint32
    cfg.target_addr.type = IPADDR_TYPE_V4;
    esp_ping_callbacks_t cbs = {};
    cbs.on_ping_success = onPingSuccess;
    if (esp_ping_new_session(&cfg, &cbs, &gPing) != ESP_OK) {
      gPing = nullptr;
      return;
    }
    gPingTarget = gateway;
  }
  esp_ping_start(gPing);
}

// Watchdog stage 1: restart the interfaces that run. Returns 1 Ethernet,
// 2 WiFi, 3 both, 0 none.
int32_t restartInterface() {
  int32_t mask = 0;
  portENTER_CRITICAL(&gMux);
  const esp_eth_handle_t eth = gEthHandle;
  portEXIT_CRITICAL(&gMux);
  if (eth != nullptr) {
    esp_eth_stop(eth);
    vTaskDelay(pdMS_TO_TICKS(200));
    esp_eth_start(eth);
    mask |= 1;
  }
  if (gWifiStarted) {
    WiFi.reconnect();
    mask |= 2;
  }
  return mask;
}

void noteEvidence(uint32_t nowMs, bool mqttConnected, bool synced) {
  const uint32_t pings = gPingReplies;
  if (pings != gPingSeen) {
    gPingSeen = pings;
    gReach.onEvidence(vdm::NetEvidence::GatewayPing, nowMs);
  }
  if (mqttConnected) gReach.onEvidence(vdm::NetEvidence::Mqtt, nowMs);
  if (synced) gReach.onEvidence(vdm::NetEvidence::TimeSync, nowMs);
  const uint32_t inbound = gInboundCount;
  if (inbound != gInboundSeen) {
    gInboundSeen = inbound;
    gReach.onEvidence(vdm::NetEvidence::InboundHttp, nowMs);
  }
  // A static configuration raises GOT_IP as well: only a DHCP lease counts.
  const uint32_t gotIp = gGotIpCount;
  if (gotIp != gGotIpSeen) {
    gGotIpSeen = gotIp;
    if (gCfg.net.dhcp) gReach.onEvidence(vdm::NetEvidence::DhcpLease, nowMs);
  }
}

void reportReachability(uint32_t nowMs) {
  switch (gReach.change(nowMs)) {
    case vdm::NetReachability::Change::Lost: {
      const uint32_t age = gReach.evidenceAgeMs(nowMs);
      logger::log(vdm::EventCode::NetUnreachable, vdm::kNoValve,
                  age == UINT32_MAX ? -1 : static_cast<int32_t>(age / 1000),
                  static_cast<int32_t>(gReach.lastEvidence()));
      break;
    }
    case vdm::NetReachability::Change::Regained:
      logger::log(vdm::EventCode::NetReachable, vdm::kNoValve,
                  static_cast<int32_t>(gReach.lostForMs(nowMs) / 1000));
      break;
    case vdm::NetReachability::Change::None:
      break;
  }
}

// The trial settings failed (or the user asked): the previous fields go back
// into the stored config and the ESP restarts. A failed persist leaves the
// record Running, so the next boot reverts before the interfaces start.
void revertTrial(vdm::NetTrialRevert reason) {
  vdm::applyNetTrialFields(gCfg.net, gTrialPrev);
  char addr[16];
  vdm::formatNetAddress(gTrialPrev, addr, sizeof addr);
  char path[32];
  const bool ok = storage::applyConfig(gCfg, path, sizeof path);
  if (ok) storage::clearNetTrial();
  logger::log(vdm::EventCode::NetTrialReverted, vdm::kNoValve, static_cast<int32_t>(reason),
              ok ? 0 : -1, addr);
  ota::requestRestart(5, 1000);
}

void serviceTrial(uint32_t nowMs, bool up) {
  if (gConfirmRequest) {
    gConfirmRequest = false;
    const uint32_t upFor = gTrial.upForMs(nowMs);
    if (gTrial.confirm()) {
      storage::clearNetTrial();
      logger::log(vdm::EventCode::NetTrialConfirmed, vdm::kNoValve,
                  static_cast<int32_t>(upFor / 1000), 0);
    }
  }
  if (gRevertRequest) {
    gRevertRequest = false;
    if (gTrial.confirm()) revertTrial(vdm::NetTrialRevert::User);
  }
  if (gTrial.update(up, nowMs) == vdm::NetTrial::Decision::Revert) revertTrial(gTrial.reason());
}

bool saveTrialRecord(const vdm::NetTrialRecord& r) {
  uint8_t blob[vdm::kNetTrialBlobMax];
  const size_t n = vdm::encodeNetTrial(r, blob, sizeof blob);
  return storage::saveNetTrialBlob(blob, n);
}

// Boot: a record Armed by the previous boot starts the trial; a Running one
// (that boot ended during the trial) is reverted at once, before the
// interfaces start.
void beginTrial(vdm::Config& cfg, uint32_t nowMs) {
  uint8_t blob[vdm::kNetTrialBlobMax];
  const size_t n = storage::loadNetTrialBlob(blob, sizeof blob);
  vdm::NetTrialRecord rec;
  const bool have = n != 0 && vdm::decodeNetTrial(blob, n, rec);
  if (n != 0 && !have) storage::clearNetTrial();
  switch (vdm::netTrialAtBoot(have ? &rec : nullptr, cfg.net)) {
    case vdm::NetTrialBoot::None:
      break;
    case vdm::NetTrialBoot::Stale:
      storage::clearNetTrial();
      break;
    case vdm::NetTrialBoot::Start:
      rec.state = vdm::NetTrialState::Running;
      saveTrialRecord(rec);
      gTrialPrev = rec.previous;
      gTrial.start(nowMs);
      break;
    case vdm::NetTrialBoot::RevertNow: {
      vdm::applyNetTrialFields(cfg.net, rec.previous);
      char addr[16];
      vdm::formatNetAddress(rec.previous, addr, sizeof addr);
      char path[32];
      const bool ok = storage::applyConfig(cfg, path, sizeof path);
      if (ok) storage::clearNetTrial();
      logger::log(vdm::EventCode::NetTrialReverted, vdm::kNoValve,
                  static_cast<int32_t>(vdm::NetTrialRevert::Interrupted), ok ? 0 : -1, addr);
      break;
    }
  }
}

void publishHealth(uint32_t nowMs) {
  vdm::NetHealthInfo h;
  h.ipUp = gReach.ipUp();
  h.reachable = gReach.reachable(nowMs);
  h.proven = gReach.proven();
  h.pingArmed = gReach.armed();
  h.evidence = gReach.lastEvidence();
  const uint32_t age = gReach.evidenceAgeMs(nowMs);
  h.evidenceAgeS = age == UINT32_MAX ? UINT32_MAX : age / 1000;
  h.ifaceRestarts = gWatchdog.interfaceRestarts();
  h.trialActive = gTrial.active();
  h.trialRemainingS = gTrial.remainingMs(nowMs) / 1000;
  portENTER_CRITICAL(&gMux);
  gHealth = h;
  gTrialActive = h.trialActive;
  gTrialRemainS = h.trialRemainingS;
  portEXIT_CRITICAL(&gMux);
}

}  // namespace

void begin(vdm::Config& cfg) {
  beginTrial(cfg, millis());
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
  publishHealth(millis());
}

void service(uint32_t nowMs, bool mqttConnected) {
  refreshInfo(nowMs);
  const bool synced = checkTimeSync(nowMs);
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

  const Info i = info();
  const bool up = i.state != vdm::NetState::Down;
  if (up && !gMdnsStarted) {
    gMdnsStarted = MDNS.begin(gHostname);
    if (gMdnsStarted) MDNS.addService("http", "tcp", 80);
  }

  // End-to-end reachability: evidence, gateway probe, Lost/Regained.
  gReach.update(up, i.gateway, nowMs);
  noteEvidence(nowMs, mqttConnected, synced);
  if (gReach.probeDue(nowMs)) {
    probeGateway(i.gateway);
    gReach.onProbeSent(nowMs);
  }
  reportReachability(nowMs);

  serviceTrial(nowMs, up);

  switch (gWatchdog.update(gReach.reachable(nowMs), nowMs)) {
    case vdm::NetWatchdog::Action::RestartInterface: {
      const int32_t mask = restartInterface();
      if (mask != 0) {
        logger::log(vdm::EventCode::NetInterfaceRestart, vdm::kNoValve,
                    static_cast<int32_t>(gWatchdog.outageMs(nowMs) / 1000), mask);
      }
      break;
    }
    case vdm::NetWatchdog::Action::RestartEsp:
      ota::requestRestart(2, 1000, static_cast<int32_t>(gWatchdog.outageMs(nowMs) / 60000));
      break;
    case vdm::NetWatchdog::Action::None:
      break;
  }
  if (gWatchdog.restartsInOutage() != loadOutageRestarts()) {
    storeOutageRestarts(gWatchdog.restartsInOutage());
  }
  publishHealth(nowMs);
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

vdm::NetHealthInfo health(uint32_t) {
  portENTER_CRITICAL(&gMux);
  const vdm::NetHealthInfo h = gHealth;
  portEXIT_CRITICAL(&gMux);
  return h;
}

bool otaNetOk() {
  portENTER_CRITICAL(&gMux);
  const bool ok = gHealth.reachable && gHealth.proven;
  portEXIT_CRITICAL(&gMux);
  return ok;
}

const char* hostname() { return gHostname; }

void noteInboundHttp(uint32_t remoteIp) {
  if ((remoteIp & 0xFF) == kLoopbackNet) return;
  portENTER_CRITICAL(&gMux);
  const uint32_t own = gInfo.ip;
  portEXIT_CRITICAL(&gMux);
  if (remoteIp == own) return;
  gInboundCount = gInboundCount + 1;
}

bool requestTrialConfirm() {
  portENTER_CRITICAL(&gMux);
  const bool active = gTrialActive;
  portEXIT_CRITICAL(&gMux);
  if (active) gConfirmRequest = true;
  return active;
}

bool requestTrialRevert() {
  portENTER_CRITICAL(&gMux);
  const bool active = gTrialActive;
  portEXIT_CRITICAL(&gMux);
  if (active) gRevertRequest = true;
  return active;
}

TrialInfo trialInfo() {
  TrialInfo t;
  portENTER_CRITICAL(&gMux);
  t.active = gTrialActive;
  t.remainS = gTrialRemainS;
  portEXIT_CRITICAL(&gMux);
  return t;
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
  const bool restart = vdm::configRestartReasons(gCfg, cfg) != 0;
  if (vdm::netTrialRequired(gCfg.net, cfg.net)) {
    vdm::NetTrialRecord r;
    r.trialCrc = vdm::netTrialFieldsCrc(cfg.net);
    if (!gArmedThisBoot) {
      // A change saved during a running trial came over the trial network:
      // it confirms that trial and the running settings become the previous.
      const uint32_t upFor = gTrial.upForMs(millis());
      if (gTrial.confirm()) {
        logger::log(vdm::EventCode::NetTrialConfirmed, vdm::kNoValve,
                    static_cast<int32_t>(upFor / 1000), 1);
      }
      gArmedPrev = gCfg.net;
      gArmedThisBoot = true;
    }
    // Two changes before the restart: the first one's previous settings are
    // the proven ones.
    r.previous = gArmedPrev;
    saveTrialRecord(r);
    char addr[16];
    vdm::formatNetAddress(cfg.net, addr, sizeof addr);
    logger::log(vdm::EventCode::NetTrialStarted, vdm::kNoValve,
                static_cast<int32_t>(vdm::kNetTrialWindowMs / 1000), 0, addr);
  }
  const bool timeChanged = strcmp(cfg.time.ntpServer, gCfg.time.ntpServer) != 0 ||
                           strcmp(cfg.time.tzPosix, gCfg.time.tzPosix) != 0;
  gCfg = cfg;
  gWatchdog.configure(cfg.net.reconnectTimeoutMin);
  if (timeChanged) applyTime(gCfg);
  // Interface/address/hostname changes need a clean start of the network
  // stack (the Arduino ETH driver cannot be re-initialised): restart the
  // ESP after the HTTP response went out.
  if (restart) ota::requestRestart(0, 1500);
}

}  // namespace net
