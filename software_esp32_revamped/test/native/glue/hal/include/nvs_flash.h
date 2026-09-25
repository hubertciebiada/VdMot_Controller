// Fake ESP-IDF 4.4 nvs_flash.h.
#pragma once

#include "esp_err.h"
#include "nvs.h"

esp_err_t nvs_flash_init(void);
esp_err_t nvs_flash_erase(void);
