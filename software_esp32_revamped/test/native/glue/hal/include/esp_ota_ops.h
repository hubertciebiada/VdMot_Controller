// Fake ESP-IDF 4.4 esp_ota_ops.h on fakes::ota(): two OTA app partitions (app0, app1 of the
// board's default.csv, 0x140000 bytes each), their image states and the boot selection. The
// states and the selection are persistent stores (handed to the next boot of a case); a boot
// with a NEW image runs it as PENDING_VERIFY, a boot of an image still PENDING_VERIFY rolls back
// (ABORTED) like the bootloader with CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "esp_err.h"
#include "esp_partition.h"

#define ESP_ERR_OTA_BASE 0x1500
#define ESP_ERR_OTA_PARTITION_CONFLICT (ESP_ERR_OTA_BASE + 0x01)
#define ESP_ERR_OTA_SELECT_INFO_INVALID (ESP_ERR_OTA_BASE + 0x02)
#define ESP_ERR_OTA_VALIDATE_FAILED (ESP_ERR_OTA_BASE + 0x03)
#define ESP_ERR_OTA_SMALL_SEC_VER (ESP_ERR_OTA_BASE + 0x04)
#define ESP_ERR_OTA_ROLLBACK_FAILED (ESP_ERR_OTA_BASE + 0x05)
#define ESP_ERR_OTA_ROLLBACK_INVALID_STATE (ESP_ERR_OTA_BASE + 0x06)

typedef enum {
  ESP_OTA_IMG_NEW = 0x0U,
  ESP_OTA_IMG_PENDING_VERIFY = 0x1U,
  ESP_OTA_IMG_VALID = 0x2U,
  ESP_OTA_IMG_INVALID = 0x3U,
  ESP_OTA_IMG_ABORTED = 0x4U,
  ESP_OTA_IMG_UNDEFINED = 0xFFFFFFFFU,
} esp_ota_img_states_t;

typedef uint32_t esp_ota_handle_t;

// nullptr when fakes::ota().hasRunning is false.
const esp_partition_t* esp_ota_get_running_partition(void);
const esp_partition_t* esp_ota_get_boot_partition(void);
// The other app partition; nullptr when fakes::ota().hasNext is false.
const esp_partition_t* esp_ota_get_next_update_partition(const esp_partition_t* start_from);
// fakes::ota().stateResult (default ESP_OK) and the partition's state.
esp_err_t esp_ota_get_state_partition(const esp_partition_t* partition,
                                      esp_ota_img_states_t* ota_state);
// The running image VALID; fakes::ota().markValidResult.
esp_err_t esp_ota_mark_app_valid_cancel_rollback(void);
// The running image INVALID and the other one selected: with a VALID other image this reboots
// (throws fakes::Restarted after journaling "esp_ota_mark_app_invalid_rollback_and_reboot"),
// otherwise ESP_FAIL is returned (nothing to roll back to).
esp_err_t esp_ota_mark_app_invalid_rollback_and_reboot(void);
esp_err_t esp_ota_set_boot_partition(const esp_partition_t* partition);
