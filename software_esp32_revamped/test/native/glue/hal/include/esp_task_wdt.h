// Fake ESP-IDF 4.4 esp_task_wdt.h: calls recorded in fakes::esp() (wdtInits, wdtAdds, wdtResets).
#pragma once

#include <stdbool.h>
#include <stdint.h>

#include "esp_err.h"
#include "freertos/FreeRTOS.h"

esp_err_t esp_task_wdt_init(uint32_t timeout, bool panic);
esp_err_t esp_task_wdt_add(TaskHandle_t handle);
esp_err_t esp_task_wdt_delete(TaskHandle_t handle);
esp_err_t esp_task_wdt_reset(void);
