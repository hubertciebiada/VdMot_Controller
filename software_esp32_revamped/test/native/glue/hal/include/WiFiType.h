// Fake Arduino-ESP32 2.0.7 WiFiType.h / WiFiGeneric.h types: modes, status, the system events and
// the event callback types.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <functional>

#include "esp_eth.h"

typedef enum {
  WIFI_MODE_NULL = 0,
  WIFI_MODE_STA,
  WIFI_MODE_AP,
  WIFI_MODE_APSTA,
  WIFI_MODE_MAX
} wifi_mode_t;

#define WIFI_OFF WIFI_MODE_NULL
#define WIFI_STA WIFI_MODE_STA
#define WIFI_AP WIFI_MODE_AP
#define WIFI_AP_STA WIFI_MODE_APSTA

typedef enum {
  WL_NO_SHIELD = 255,
  WL_IDLE_STATUS = 0,
  WL_NO_SSID_AVAIL = 1,
  WL_SCAN_COMPLETED = 2,
  WL_CONNECTED = 3,
  WL_CONNECT_FAILED = 4,
  WL_CONNECTION_LOST = 5,
  WL_DISCONNECTED = 6
} wl_status_t;

typedef enum {
  ARDUINO_EVENT_WIFI_READY = 0,
  ARDUINO_EVENT_WIFI_SCAN_DONE,
  ARDUINO_EVENT_WIFI_STA_START,
  ARDUINO_EVENT_WIFI_STA_STOP,
  ARDUINO_EVENT_WIFI_STA_CONNECTED,
  ARDUINO_EVENT_WIFI_STA_DISCONNECTED,
  ARDUINO_EVENT_WIFI_STA_AUTHMODE_CHANGE,
  ARDUINO_EVENT_WIFI_STA_GOT_IP,
  ARDUINO_EVENT_WIFI_STA_GOT_IP6,
  ARDUINO_EVENT_WIFI_STA_LOST_IP,
  ARDUINO_EVENT_WIFI_AP_START,
  ARDUINO_EVENT_WIFI_AP_STOP,
  ARDUINO_EVENT_WIFI_AP_STACONNECTED,
  ARDUINO_EVENT_WIFI_AP_STADISCONNECTED,
  ARDUINO_EVENT_WIFI_AP_STAIPASSIGNED,
  ARDUINO_EVENT_WIFI_AP_PROBEREQRECVED,
  ARDUINO_EVENT_WIFI_AP_GOT_IP6,
  ARDUINO_EVENT_WIFI_FTM_REPORT,
  ARDUINO_EVENT_ETH_START,
  ARDUINO_EVENT_ETH_STOP,
  ARDUINO_EVENT_ETH_CONNECTED,
  ARDUINO_EVENT_ETH_DISCONNECTED,
  ARDUINO_EVENT_ETH_GOT_IP,
  ARDUINO_EVENT_ETH_GOT_IP6,
  ARDUINO_EVENT_WPS_ER_SUCCESS,
  ARDUINO_EVENT_WPS_ER_FAILED,
  ARDUINO_EVENT_WPS_ER_TIMEOUT,
  ARDUINO_EVENT_WPS_ER_PIN,
  ARDUINO_EVENT_WPS_ER_PBC_OVERLAP,
  ARDUINO_EVENT_SC_SCAN_DONE,
  ARDUINO_EVENT_SC_FOUND_CHANNEL,
  ARDUINO_EVENT_SC_GOT_SSID_PSWD,
  ARDUINO_EVENT_SC_SEND_ACK_DONE,
  ARDUINO_EVENT_PROV_INIT,
  ARDUINO_EVENT_PROV_DEINIT,
  ARDUINO_EVENT_PROV_START,
  ARDUINO_EVENT_PROV_END,
  ARDUINO_EVENT_PROV_CRED_RECV,
  ARDUINO_EVENT_PROV_CRED_FAIL,
  ARDUINO_EVENT_PROV_CRED_SUCCESS,
  ARDUINO_EVENT_MAX
} arduino_event_id_t;

typedef struct {
  uint8_t ssid[32];
  uint8_t ssid_len;
  uint8_t bssid[6];
  uint8_t reason;
} wifi_event_sta_disconnected_t;

typedef struct {
  uint32_t addr;
} esp_ip4_addr_t;

typedef struct {
  esp_ip4_addr_t ip;
  esp_ip4_addr_t netmask;
  esp_ip4_addr_t gw;
} esp_netif_ip_info_t;

typedef struct {
  int if_index;
  void* esp_netif;
  esp_netif_ip_info_t ip_info;
  bool ip_changed;
} ip_event_got_ip_t;

typedef union {
  wifi_event_sta_disconnected_t wifi_sta_disconnected;
  ip_event_got_ip_t got_ip;
  esp_eth_handle_t eth_connected;
} arduino_event_info_t;

typedef struct {
  arduino_event_id_t event_id;
  arduino_event_info_t event_info;
} arduino_event_t;

typedef void (*WiFiEventCb)(arduino_event_id_t event);
typedef std::function<void(arduino_event_id_t event, arduino_event_info_t info)> WiFiEventFuncCb;
typedef void (*WiFiEventSysCb)(arduino_event_t* event);
typedef size_t wifi_event_id_t;
