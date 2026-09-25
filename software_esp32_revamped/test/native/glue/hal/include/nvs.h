// Fake ESP-IDF 4.4 nvs.h on the typed store of fakes::nvs() (shared with Preferences): a getter
// of another type does not find the key, a length query passes out == nullptr, a too small
// buffer is ESP_ERR_NVS_INVALID_LENGTH, a read-only open of a missing namespace fails.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "esp_err.h"

#define ESP_ERR_NVS_BASE 0x1100
#define ESP_ERR_NVS_NOT_INITIALIZED (ESP_ERR_NVS_BASE + 0x01)
#define ESP_ERR_NVS_NOT_FOUND (ESP_ERR_NVS_BASE + 0x02)
#define ESP_ERR_NVS_TYPE_MISMATCH (ESP_ERR_NVS_BASE + 0x03)
#define ESP_ERR_NVS_READ_ONLY (ESP_ERR_NVS_BASE + 0x04)
#define ESP_ERR_NVS_NOT_ENOUGH_SPACE (ESP_ERR_NVS_BASE + 0x05)
#define ESP_ERR_NVS_INVALID_NAME (ESP_ERR_NVS_BASE + 0x06)
#define ESP_ERR_NVS_INVALID_HANDLE (ESP_ERR_NVS_BASE + 0x07)
#define ESP_ERR_NVS_REMOVE_FAILED (ESP_ERR_NVS_BASE + 0x08)
#define ESP_ERR_NVS_KEY_TOO_LONG (ESP_ERR_NVS_BASE + 0x09)
#define ESP_ERR_NVS_PAGE_FULL (ESP_ERR_NVS_BASE + 0x0a)
#define ESP_ERR_NVS_INVALID_STATE (ESP_ERR_NVS_BASE + 0x0b)
#define ESP_ERR_NVS_INVALID_LENGTH (ESP_ERR_NVS_BASE + 0x0c)
#define ESP_ERR_NVS_NO_FREE_PAGES (ESP_ERR_NVS_BASE + 0x0d)
#define ESP_ERR_NVS_VALUE_TOO_LONG (ESP_ERR_NVS_BASE + 0x0e)

typedef uint32_t nvs_handle_t;
typedef nvs_handle_t nvs_handle;

typedef enum {
  NVS_READONLY,
  NVS_READWRITE,
} nvs_open_mode_t;
typedef nvs_open_mode_t nvs_open_mode;

typedef enum {
  NVS_TYPE_U8 = 0x01,
  NVS_TYPE_I8 = 0x11,
  NVS_TYPE_U16 = 0x02,
  NVS_TYPE_I16 = 0x12,
  NVS_TYPE_U32 = 0x04,
  NVS_TYPE_I32 = 0x14,
  NVS_TYPE_U64 = 0x08,
  NVS_TYPE_I64 = 0x18,
  NVS_TYPE_STR = 0x21,
  NVS_TYPE_BLOB = 0x42,
  NVS_TYPE_ANY = 0xff
} nvs_type_t;

esp_err_t nvs_open(const char* name, nvs_open_mode_t open_mode, nvs_handle_t* out_handle);
void nvs_close(nvs_handle_t handle);
esp_err_t nvs_commit(nvs_handle_t handle);
esp_err_t nvs_erase_key(nvs_handle_t handle, const char* key);
esp_err_t nvs_erase_all(nvs_handle_t handle);

esp_err_t nvs_set_i8(nvs_handle_t handle, const char* key, int8_t value);
esp_err_t nvs_set_u8(nvs_handle_t handle, const char* key, uint8_t value);
esp_err_t nvs_set_i16(nvs_handle_t handle, const char* key, int16_t value);
esp_err_t nvs_set_u16(nvs_handle_t handle, const char* key, uint16_t value);
esp_err_t nvs_set_i32(nvs_handle_t handle, const char* key, int32_t value);
esp_err_t nvs_set_u32(nvs_handle_t handle, const char* key, uint32_t value);
esp_err_t nvs_set_i64(nvs_handle_t handle, const char* key, int64_t value);
esp_err_t nvs_set_u64(nvs_handle_t handle, const char* key, uint64_t value);
esp_err_t nvs_set_str(nvs_handle_t handle, const char* key, const char* value);
esp_err_t nvs_set_blob(nvs_handle_t handle, const char* key, const void* value, size_t length);

esp_err_t nvs_get_i8(nvs_handle_t handle, const char* key, int8_t* out_value);
esp_err_t nvs_get_u8(nvs_handle_t handle, const char* key, uint8_t* out_value);
esp_err_t nvs_get_i16(nvs_handle_t handle, const char* key, int16_t* out_value);
esp_err_t nvs_get_u16(nvs_handle_t handle, const char* key, uint16_t* out_value);
esp_err_t nvs_get_i32(nvs_handle_t handle, const char* key, int32_t* out_value);
esp_err_t nvs_get_u32(nvs_handle_t handle, const char* key, uint32_t* out_value);
esp_err_t nvs_get_i64(nvs_handle_t handle, const char* key, int64_t* out_value);
esp_err_t nvs_get_u64(nvs_handle_t handle, const char* key, uint64_t* out_value);
esp_err_t nvs_get_str(nvs_handle_t handle, const char* key, char* out_value, size_t* length);
esp_err_t nvs_get_blob(nvs_handle_t handle, const char* key, void* out_value, size_t* length);
