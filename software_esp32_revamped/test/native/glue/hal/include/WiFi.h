// Fake Arduino-ESP32 2.0.7 WiFi.h (WiFiGeneric + WiFiSTA): calls recorded in fakes::net(), system
// events delivered by fakes::net().fire(event, info) to every registered callback.
#pragma once

#include <stdint.h>

#include "IPAddress.h"
#include "WString.h"
#include "WiFiClient.h"
#include "WiFiType.h"
#include "WiFiUdp.h"

class WiFiClass {
 public:
  wifi_event_id_t onEvent(WiFiEventCb cbEvent, arduino_event_id_t event = ARDUINO_EVENT_MAX);
  wifi_event_id_t onEvent(WiFiEventFuncCb cbEvent, arduino_event_id_t event = ARDUINO_EVENT_MAX);
  wifi_event_id_t onEvent(WiFiEventSysCb cbEvent, arduino_event_id_t event = ARDUINO_EVENT_MAX);
  void removeEvent(wifi_event_id_t id);

  void persistent(bool persistent);
  bool mode(wifi_mode_t m);
  wifi_mode_t getMode();
  bool setHostname(const char* hostname);
  const char* getHostname();

  wl_status_t begin(const char* ssid, const char* passphrase = nullptr, int32_t channel = 0,
                    const uint8_t* bssid = nullptr, bool connect = true);
  wl_status_t begin(char* ssid, char* passphrase = nullptr, int32_t channel = 0,
                    const uint8_t* bssid = nullptr, bool connect = true);
  wl_status_t begin();
  bool config(IPAddress local_ip, IPAddress gateway, IPAddress subnet,
              IPAddress dns1 = (uint32_t)0x00000000, IPAddress dns2 = (uint32_t)0x00000000);
  bool reconnect();
  bool disconnect(bool wifioff = false, bool eraseap = false);
  bool isConnected();
  bool setAutoReconnect(bool autoReconnect);
  bool getAutoReconnect();
  wl_status_t status();

  IPAddress localIP();
  IPAddress subnetMask();
  IPAddress gatewayIP();
  IPAddress dnsIP(uint8_t dns_no = 0);
  String macAddress();
  uint8_t* macAddress(uint8_t* mac);
  int8_t RSSI();
  String SSID() const;
};

extern WiFiClass WiFi;
