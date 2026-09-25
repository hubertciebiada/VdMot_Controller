// Fake ESP-IDF 4.4 esp_sntp.h: SNTP on/off and the sync callback (fakes::net()); a test syncs
// the clock with fakes::net().syncTime(epoch), which calls the callback like lwIP.
#pragma once

#include <stdbool.h>
#include <stdint.h>
#include <sys/time.h>

typedef void (*sntp_sync_time_cb_t)(struct timeval* tv);

#define SNTP_OPMODE_POLL 0

bool sntp_enabled(void);
void sntp_stop(void);
void sntp_init(void);
void sntp_setoperatingmode(uint8_t operating_mode);
void sntp_setservername(uint8_t idx, const char* server);
void sntp_set_time_sync_notification_cb(sntp_sync_time_cb_t callback);
