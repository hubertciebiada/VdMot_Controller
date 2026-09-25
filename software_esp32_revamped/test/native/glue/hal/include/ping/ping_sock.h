// Fake ESP-IDF 4.4 ping/ping_sock.h (esp_ping): sessions are recorded in fakes::net().ping; the
// ping task does not run by itself: fakes::net().pingStep() delivers the next scripted answer
// (reply or timeout) to every started session through its callbacks, and on_ping_end after
// `count` answers (count 0 = infinite).
#pragma once

#include <stdint.h>

#include "esp_err.h"

#define IPADDR_TYPE_V4 0U
#define IPADDR_TYPE_V6 6U

typedef struct {
  uint32_t addr;
} ip4_addr_t;

typedef struct {
  union {
    ip4_addr_t ip4;
  } u_addr;
  uint8_t type;
} ip_addr_t;

// lwIP layout: first octet in the low byte, like IPAddress.
#define IP_ADDR4(ipaddr, a, b, c, d)                                                           \
  do {                                                                                         \
    (ipaddr)->u_addr.ip4.addr = ((uint32_t)(a)) | ((uint32_t)(b) << 8) | ((uint32_t)(c) << 16) | \
                                ((uint32_t)(d) << 24);                                         \
    (ipaddr)->type = IPADDR_TYPE_V4;                                                           \
  } while (0)

typedef void* esp_ping_handle_t;

typedef struct {
  void* cb_args;
  void (*on_ping_success)(esp_ping_handle_t hdl, void* args);
  void (*on_ping_timeout)(esp_ping_handle_t hdl, void* args);
  void (*on_ping_end)(esp_ping_handle_t hdl, void* args);
} esp_ping_callbacks_t;

typedef struct {
  uint32_t count;
  uint32_t interval_ms;
  uint32_t timeout_ms;
  uint32_t data_size;
  int tos;
  int ttl;
  ip_addr_t target_addr;
  uint32_t task_stack_size;
  uint32_t task_prio;
  uint32_t interface;
} esp_ping_config_t;

#define ESP_PING_COUNT_INFINITE (0)
#define ESP_PING_DEFAULT_CONFIG() {5, 1000, 1000, 64, 0, 255, {{{0}}, IPADDR_TYPE_V4}, 2048, 2, 0}

typedef enum {
  ESP_PING_PROF_SEQNO,
  ESP_PING_PROF_TOS,
  ESP_PING_PROF_TTL,
  ESP_PING_PROF_REQUEST,
  ESP_PING_PROF_REPLY,
  ESP_PING_PROF_IPADDR,
  ESP_PING_PROF_SIZE,
  ESP_PING_PROF_TIMEGAP,
  ESP_PING_PROF_DURATION
} esp_ping_profile_t;

esp_err_t esp_ping_new_session(const esp_ping_config_t* config, const esp_ping_callbacks_t* cbs,
                               esp_ping_handle_t* hdl_out);
esp_err_t esp_ping_delete_session(esp_ping_handle_t hdl);
esp_err_t esp_ping_start(esp_ping_handle_t hdl);
esp_err_t esp_ping_stop(esp_ping_handle_t hdl);
esp_err_t esp_ping_get_profile(esp_ping_handle_t hdl, esp_ping_profile_t profile, void* data,
                               uint32_t size);
