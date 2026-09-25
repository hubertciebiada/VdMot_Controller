// Fake network layer: ETH, WiFi, system events, SNTP/TZ, UDP, mDNS, esp_ping, TCP clients.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include <string>

#include "ESPmDNS.h"
#include "ETH.h"
#include "WiFi.h"
#include "WiFiClient.h"
#include "WiFiUdp.h"
#include "esp_eth.h"
#include "esp_sntp.h"
#include "fakes/fakes.h"
#include "hal_internal.h"
#include "ping/ping_sock.h"

namespace fakes {

namespace {

Net g_net;

std::string ipText(uint32_t ip) {
  char buf[16];
  snprintf(buf, sizeof buf, "%u.%u.%u.%u", ip & 0xFF, (ip >> 8) & 0xFF, (ip >> 16) & 0xFF,
           (ip >> 24) & 0xFF);
  return buf;
}

wifi_event_id_t addHandler(WiFiEventFuncCb cb, arduino_event_id_t event) {
  g_net.handlers.push_back({std::move(cb), event, false});
  return g_net.handlers.size();
}

PingSession* sessionOf(esp_ping_handle_t hdl) {
  for (auto& s : g_net.pings) {
    if (s.get() == hdl) return s.get();
  }
  return nullptr;
}

}  // namespace

Net& net() { return g_net; }

void resetNetVolatile() { g_net = Net{}; }

void Net::fire(arduino_event_id_t event, arduino_event_info_t info) {
  note("net.event " + std::to_string(static_cast<int>(event)));
  // the Ethernet driver passes its handle with CONNECTED (esp_eth_stop/start need it)
  if (event == ARDUINO_EVENT_ETH_CONNECTED && info.eth_connected == nullptr) {
    info.eth_connected = ethHandle;
  }
  // copies: a callback may register another one
  const std::vector<Handler> current = handlers;
  for (const Handler& h : current) {
    if (!h.removed && (h.event == ARDUINO_EVENT_MAX || h.event == event)) h.cb(event, info);
  }
}

void Net::syncTime(int64_t epoch) {
  setWallClock(epoch);
  note("sntp.sync " + std::to_string(epoch));
  if (sntpCb != nullptr) {
    struct timeval tv;
    tv.tv_sec = static_cast<time_t>(epoch);
    tv.tv_usec = 0;
    sntpCb(&tv);
  }
}

void Net::pingStep() {
  for (auto& s : pings) {
    if (!s->started || s->deleted || s->ended) continue;
    bool answer = pingDefault;
    if (!pingAnswers.empty()) {
      answer = pingAnswers.front();
      pingAnswers.pop_front();
    }
    ++s->seqno;
    ++s->requests;
    if (answer) {
      ++s->replies;
      if (s->cbs.on_ping_success != nullptr) s->cbs.on_ping_success(s.get(), s->cbs.cb_args);
    } else if (s->cbs.on_ping_timeout != nullptr) {
      s->cbs.on_ping_timeout(s.get(), s->cbs.cb_args);
    }
    if (s->config.count != 0 && s->requests >= s->config.count) {
      s->ended = true;
      s->started = false;
      if (s->cbs.on_ping_end != nullptr) s->cbs.on_ping_end(s.get(), s->cbs.cb_args);
    }
  }
}

}  // namespace fakes

// ---------------------------------------------------------------- ETH

ETHClass ETH;

bool ETHClass::begin(uint8_t phy_addr, int power, int mdc, int mdio, eth_phy_type_t type,
                     eth_clock_mode_t clk_mode, bool) {
  fakes::Net& n = fakes::net();
  n.ethBegins.push_back({phy_addr, power, mdc, mdio, type, clk_mode});
  char buf[80];
  snprintf(buf, sizeof buf, "eth.begin %u %d %d %d %d %d", phy_addr, power, mdc, mdio,
           static_cast<int>(type), static_cast<int>(clk_mode));
  fakes::note(buf);
  return n.ethBeginResult;
}

bool ETHClass::config(IPAddress local_ip, IPAddress gateway, IPAddress subnet, IPAddress dns1,
                      IPAddress dns2) {
  fakes::net().ethConfigs.push_back({local_ip, gateway, subnet, dns1, dns2});
  fakes::note("eth.config " + fakes::ipText(local_ip));
  return true;
}

const char* ETHClass::getHostname() { return fakes::net().ethHostname.c_str(); }

bool ETHClass::setHostname(const char* hostname) {
  fakes::net().ethHostname = hostname != nullptr ? hostname : "";
  fakes::note("eth.setHostname " + fakes::net().ethHostname);
  return true;
}

bool ETHClass::fullDuplex() { return true; }
bool ETHClass::linkUp() { return fakes::net().ethLink; }
uint8_t ETHClass::linkSpeed() { return 100; }
IPAddress ETHClass::localIP() { return IPAddress(fakes::net().ethIp); }
IPAddress ETHClass::subnetMask() { return IPAddress(fakes::net().ethMask); }
IPAddress ETHClass::gatewayIP() { return IPAddress(fakes::net().ethGateway); }
IPAddress ETHClass::dnsIP(uint8_t) { return IPAddress(fakes::net().ethDns); }
String ETHClass::macAddress() { return String(fakes::net().ethMac.c_str()); }

uint8_t* ETHClass::macAddress(uint8_t* mac) {
  unsigned v[6] = {0, 0, 0, 0, 0, 0};
  sscanf(fakes::net().ethMac.c_str(), "%x:%x:%x:%x:%x:%x", &v[0], &v[1], &v[2], &v[3], &v[4],
         &v[5]);
  for (int i = 0; i < 6; ++i) mac[i] = static_cast<uint8_t>(v[i]);
  return mac;
}

esp_err_t esp_eth_start(esp_eth_handle_t) {
  ++fakes::net().ethStarts;
  fakes::note("esp_eth_start");
  return fakes::net().ethStartResult;
}

esp_err_t esp_eth_stop(esp_eth_handle_t) {
  ++fakes::net().ethStops;
  fakes::note("esp_eth_stop");
  return fakes::net().ethStopResult;
}

// ---------------------------------------------------------------- WiFi

WiFiClass WiFi;

wifi_event_id_t WiFiClass::onEvent(WiFiEventCb cbEvent, arduino_event_id_t event) {
  return fakes::addHandler([cbEvent](arduino_event_id_t e, arduino_event_info_t) { cbEvent(e); },
                           event);
}

wifi_event_id_t WiFiClass::onEvent(WiFiEventFuncCb cbEvent, arduino_event_id_t event) {
  return fakes::addHandler(std::move(cbEvent), event);
}

wifi_event_id_t WiFiClass::onEvent(WiFiEventSysCb cbEvent, arduino_event_id_t event) {
  return fakes::addHandler(
      [cbEvent](arduino_event_id_t e, arduino_event_info_t info) {
        arduino_event_t ev;
        ev.event_id = e;
        ev.event_info = info;
        cbEvent(&ev);
      },
      event);
}

void WiFiClass::removeEvent(wifi_event_id_t id) {
  if (id >= 1 && id <= fakes::net().handlers.size()) fakes::net().handlers[id - 1].removed = true;
}

void WiFiClass::persistent(bool persistent) {
  fakes::net().wifiPersistent = persistent;
  fakes::note(std::string("wifi.persistent ") + (persistent ? "1" : "0"));
}

bool WiFiClass::mode(wifi_mode_t m) {
  fakes::net().wifiMode = m;
  fakes::net().wifiModes.push_back(m);
  fakes::note("wifi.mode " + std::to_string(static_cast<int>(m)));
  return true;
}

wifi_mode_t WiFiClass::getMode() { return fakes::net().wifiMode; }

bool WiFiClass::setHostname(const char* hostname) {
  fakes::net().wifiHostname = hostname != nullptr ? hostname : "";
  fakes::note("wifi.setHostname " + fakes::net().wifiHostname);
  return true;
}

const char* WiFiClass::getHostname() { return fakes::net().wifiHostname.c_str(); }

wl_status_t WiFiClass::begin(const char* ssid, const char* passphrase, int32_t, const uint8_t*,
                             bool) {
  fakes::Net& n = fakes::net();
  ++n.wifiBegins;
  n.ssid = ssid != nullptr ? ssid : "";
  n.pass = passphrase != nullptr ? passphrase : "";
  fakes::note("wifi.begin " + n.ssid);
  return n.wifiStatus;
}

wl_status_t WiFiClass::begin(char* ssid, char* passphrase, int32_t channel, const uint8_t* bssid,
                             bool connect) {
  return begin(static_cast<const char*>(ssid), static_cast<const char*>(passphrase), channel, bssid,
               connect);
}

wl_status_t WiFiClass::begin() {
  ++fakes::net().wifiBegins;
  fakes::note("wifi.begin");
  return fakes::net().wifiStatus;
}

bool WiFiClass::config(IPAddress local_ip, IPAddress gateway, IPAddress subnet, IPAddress dns1,
                       IPAddress dns2) {
  fakes::net().wifiConfigs.push_back({local_ip, gateway, subnet, dns1, dns2});
  fakes::note("wifi.config " + fakes::ipText(local_ip));
  return true;
}

bool WiFiClass::reconnect() {
  ++fakes::net().wifiReconnects;
  fakes::note("wifi.reconnect");
  return true;
}

bool WiFiClass::disconnect(bool wifioff, bool) {
  fakes::Net& n = fakes::net();
  ++n.wifiDisconnects;
  n.lastDisconnectWifiOff = wifioff;
  fakes::note(std::string("wifi.disconnect ") + (wifioff ? "1" : "0"));
  return true;
}

bool WiFiClass::isConnected() { return fakes::net().wifiStatus == WL_CONNECTED; }

bool WiFiClass::setAutoReconnect(bool autoReconnect) {
  fakes::net().autoReconnect = autoReconnect;
  fakes::note(std::string("wifi.setAutoReconnect ") + (autoReconnect ? "1" : "0"));
  return true;
}

bool WiFiClass::getAutoReconnect() { return fakes::net().autoReconnect; }
wl_status_t WiFiClass::status() { return fakes::net().wifiStatus; }
IPAddress WiFiClass::localIP() { return IPAddress(fakes::net().wifiIp); }
IPAddress WiFiClass::subnetMask() { return IPAddress(fakes::net().wifiMask); }
IPAddress WiFiClass::gatewayIP() { return IPAddress(fakes::net().wifiGateway); }
IPAddress WiFiClass::dnsIP(uint8_t) { return IPAddress(fakes::net().wifiDns); }
String WiFiClass::macAddress() { return String(fakes::net().wifiMac.c_str()); }

uint8_t* WiFiClass::macAddress(uint8_t* mac) {
  unsigned v[6] = {0, 0, 0, 0, 0, 0};
  sscanf(fakes::net().wifiMac.c_str(), "%x:%x:%x:%x:%x:%x", &v[0], &v[1], &v[2], &v[3], &v[4],
         &v[5]);
  for (int i = 0; i < 6; ++i) mac[i] = static_cast<uint8_t>(v[i]);
  return mac;
}

int8_t WiFiClass::RSSI() { return fakes::net().rssi; }
String WiFiClass::SSID() const { return String(fakes::net().ssid.c_str()); }

// ---------------------------------------------------------------- SNTP and TZ

void configTzTime(const char* tz, const char* server1, const char* server2, const char* server3) {
  (void)server2;
  (void)server3;
  fakes::Net& n = fakes::net();
  n.tzConfigs.emplace_back(tz != nullptr ? tz : "", server1 != nullptr ? server1 : "");
  n.sntpEnabled = true;
  fakes::note(std::string("configTzTime ") + (tz != nullptr ? tz : "") + " " +
              (server1 != nullptr ? server1 : ""));
  setenv("TZ", tz != nullptr ? tz : "", 1);
  tzset();
}

void configTime(long gmtOffset_sec, int daylightOffset_sec, const char* server1,
                const char* server2, const char* server3) {
  (void)server2;
  (void)server3;
  fakes::Net& n = fakes::net();
  n.tzConfigs.emplace_back("", server1 != nullptr ? server1 : "");
  n.sntpEnabled = true;
  char tz[32];
  const long offset = gmtOffset_sec + daylightOffset_sec;  // POSIX: west of UTC is positive
  snprintf(tz, sizeof tz, "UTC%+ld", -offset / 3600);
  setenv("TZ", tz, 1);
  tzset();
}

bool getLocalTime(struct tm* info, uint32_t) {
  const time_t now = time(nullptr);
  if (now < 1451606400) return false;  // 2016: not synced
  return localtime_r(&now, info) != nullptr;
}

bool sntp_enabled(void) { return fakes::net().sntpEnabled; }

void sntp_stop(void) {
  fakes::net().sntpEnabled = false;
  ++fakes::net().sntpStops;
  fakes::note("sntp_stop");
}

void sntp_init(void) { fakes::net().sntpEnabled = true; }
void sntp_setoperatingmode(uint8_t) {}
void sntp_setservername(uint8_t, const char*) {}

void sntp_set_time_sync_notification_cb(sntp_sync_time_cb_t callback) {
  fakes::net().sntpCb = callback;
}

// ---------------------------------------------------------------- UDP

uint8_t WiFiUDP::begin(uint16_t) { return 1; }
void WiFiUDP::stop() { open_ = false; }
int WiFiUDP::beginPacket() { return 0; }

int WiFiUDP::beginPacket(IPAddress ip, uint16_t port) {
  if (fakes::net().udpBeginResult == 0) return 0;
  open_ = true;
  ip_ = ip;
  port_ = port;
  data_.clear();
  return 1;
}

int WiFiUDP::beginPacket(const char*, uint16_t) { return 0; }

int WiFiUDP::endPacket() {
  if (!open_) return 0;
  open_ = false;
  fakes::net().udpSent.push_back({ip_, port_, data_});
  fakes::note("udp " + fakes::ipText(ip_) + ":" + std::to_string(port_));
  return fakes::net().udpEndResult;
}

size_t WiFiUDP::write(uint8_t c) { return write(&c, 1); }

size_t WiFiUDP::write(const uint8_t* buffer, size_t size) {
  if (!open_) return 0;
  data_.append(reinterpret_cast<const char*>(buffer), size);
  return size;
}

int WiFiUDP::parsePacket() { return 0; }
int WiFiUDP::available() { return 0; }
int WiFiUDP::read() { return -1; }
int WiFiUDP::read(unsigned char*, size_t) { return 0; }
int WiFiUDP::peek() { return -1; }
void WiFiUDP::flush() {}

// ---------------------------------------------------------------- mDNS

MDNSResponder MDNS;

bool MDNSResponder::begin(const char* hostName) {
  fakes::net().mdnsBegins.push_back(hostName != nullptr ? hostName : "");
  fakes::note(std::string("mdns.begin ") + (hostName != nullptr ? hostName : ""));
  return fakes::net().mdnsResult;
}

void MDNSResponder::end() {}

bool MDNSResponder::addService(char* service, char* proto, uint16_t port) {
  const std::string s = std::string(service) + " " + proto + " " + std::to_string(port);
  fakes::net().mdnsServices.push_back(s);
  fakes::note("mdns.addService " + s);
  return true;
}

// ---------------------------------------------------------------- esp_ping

esp_err_t esp_ping_new_session(const esp_ping_config_t* config, const esp_ping_callbacks_t* cbs,
                               esp_ping_handle_t* hdl_out) {
  fakes::Net& n = fakes::net();
  if (config == nullptr || hdl_out == nullptr) return ESP_ERR_INVALID_ARG;
  if (n.pingNewResult != ESP_OK) return n.pingNewResult;
  auto s = std::unique_ptr<fakes::PingSession>(new fakes::PingSession());
  s->config = *config;
  if (cbs != nullptr) s->cbs = *cbs;
  *hdl_out = s.get();
  fakes::note("ping.new " + fakes::ipText(config->target_addr.u_addr.ip4.addr));
  n.pings.push_back(std::move(s));
  return ESP_OK;
}

esp_err_t esp_ping_delete_session(esp_ping_handle_t hdl) {
  fakes::PingSession* s = fakes::sessionOf(hdl);
  if (s == nullptr || s->deleted) return ESP_ERR_INVALID_ARG;
  s->deleted = true;
  s->started = false;
  fakes::note("ping.delete");
  return ESP_OK;
}

esp_err_t esp_ping_start(esp_ping_handle_t hdl) {
  fakes::PingSession* s = fakes::sessionOf(hdl);
  if (s == nullptr || s->deleted) return ESP_ERR_INVALID_ARG;
  s->started = true;
  s->stopped = false;
  s->ended = false;
  s->requests = 0;
  s->replies = 0;
  fakes::note("ping.start");
  return ESP_OK;
}

esp_err_t esp_ping_stop(esp_ping_handle_t hdl) {
  fakes::PingSession* s = fakes::sessionOf(hdl);
  if (s == nullptr || s->deleted) return ESP_ERR_INVALID_ARG;
  s->started = false;
  s->stopped = true;
  fakes::note("ping.stop");
  return ESP_OK;
}

esp_err_t esp_ping_get_profile(esp_ping_handle_t hdl, esp_ping_profile_t profile, void* data,
                               uint32_t size) {
  fakes::PingSession* s = fakes::sessionOf(hdl);
  if (s == nullptr || data == nullptr) return ESP_ERR_INVALID_ARG;
  uint32_t v = 0;
  switch (profile) {
    case ESP_PING_PROF_SEQNO: v = s->seqno; break;
    case ESP_PING_PROF_TOS: v = static_cast<uint32_t>(s->config.tos); break;
    case ESP_PING_PROF_TTL: v = static_cast<uint32_t>(s->config.ttl); break;
    case ESP_PING_PROF_REQUEST: v = s->requests; break;
    case ESP_PING_PROF_REPLY: v = s->replies; break;
    case ESP_PING_PROF_IPADDR:
      if (size < sizeof(ip_addr_t)) return ESP_ERR_INVALID_SIZE;
      memcpy(data, &s->config.target_addr, sizeof(ip_addr_t));
      return ESP_OK;
    case ESP_PING_PROF_SIZE: v = s->config.data_size; break;
    case ESP_PING_PROF_TIMEGAP: v = fakes::net().pingTimeMs; break;
    case ESP_PING_PROF_DURATION: v = s->requests * s->config.interval_ms; break;
  }
  if (size < sizeof v) return ESP_ERR_INVALID_SIZE;
  memcpy(data, &v, sizeof v);
  return ESP_OK;
}

// ---------------------------------------------------------------- TCP clients

int WiFiClient::connect(IPAddress ip, uint16_t port) { return connect(ip, port, 3000); }

int WiFiClient::connect(IPAddress ip, uint16_t port, int32_t timeout_ms) {
  fakes::Net& n = fakes::net();
  n.tcpConnects.push_back({ip, "", port, timeout_ms});
  fakes::note("tcp.connect " + fakes::ipText(ip) + ":" + std::to_string(port));
  open_ = n.tcpConnectResult != 0;
  answered_ = false;
  ip_ = ip;
  port_ = port;
  request_.clear();
  reply_.clear();
  replyPos_ = 0;
  return n.tcpConnectResult;
}

int WiFiClient::connect(const char* host, uint16_t port) { return connect(host, port, 3000); }

int WiFiClient::connect(const char* host, uint16_t port, int32_t timeout_ms) {
  fakes::Net& n = fakes::net();
  n.tcpConnects.push_back({0, host != nullptr ? host : "", port, timeout_ms});
  fakes::note(std::string("tcp.connect ") + (host != nullptr ? host : "") + ":" +
              std::to_string(port));
  open_ = n.tcpConnectResult != 0;
  answered_ = false;
  ip_ = 0;
  port_ = port;
  request_.clear();
  reply_.clear();
  replyPos_ = 0;
  return n.tcpConnectResult;
}

size_t WiFiClient::write(uint8_t c) { return write(&c, 1); }

size_t WiFiClient::write(const uint8_t* buf, size_t size) {
  if (!open_ || answered_) return 0;
  request_.append(reinterpret_cast<const char*>(buf), size);
  return size;
}

void WiFiClient::respond() {
  if (!open_ || answered_) return;
  answered_ = true;
  fakes::Net& n = fakes::net();
  if (n.tcpResponder) {
    const fakes::TcpConnect to{ip_, "", port_, 0};
    reply_ = n.tcpResponder(to, request_);
  }
}

int WiFiClient::available() {
  respond();
  return open_ && replyPos_ < reply_.size() ? static_cast<int>(reply_.size() - replyPos_) : 0;
}

int WiFiClient::read() {
  uint8_t c = 0;
  return read(&c, 1) == 1 ? c : -1;
}

int WiFiClient::read(uint8_t* buf, size_t size) {
  respond();
  if (!open_ || replyPos_ >= reply_.size()) return -1;
  size_t n = reply_.size() - replyPos_;
  if (n > size) n = size;
  memcpy(buf, reply_.data() + replyPos_, n);
  replyPos_ += n;
  return static_cast<int>(n);
}

int WiFiClient::peek() {
  respond();
  return open_ && replyPos_ < reply_.size() ? static_cast<uint8_t>(reply_[replyPos_]) : -1;
}

void WiFiClient::flush() {}

void WiFiClient::stop() {
  if (fakes::isMqttSocket(this)) {
    fakes::mqttSocketStopped();
    return;
  }
  if (open_) fakes::note("tcp.stop");
  open_ = false;
}

uint8_t WiFiClient::connected() {
  if (fakes::isMqttSocket(this)) return fakes::mqttSocketConnected() ? 1 : 0;
  if (!open_) return 0;
  return !answered_ || replyPos_ < reply_.size() ? 1 : 0;
}

IPAddress WiFiClient::remoteIP() const { return IPAddress(ip_); }
uint16_t WiFiClient::remotePort() const { return port_; }
int WiFiClient::setTimeout(uint32_t) { return 0; }
