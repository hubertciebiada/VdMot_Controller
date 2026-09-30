// Fake ESP-IDF 4.4 esp_task.h: the stack sizes of the IDF tasks the glue watches, with the
// formulas of the real header (CONFIG_NEWLIB_NANO_FORMAT and CONFIG_LWIP_TCPIP_CORE_LOCKING are
// off in framework-arduinoespressif32 3.20007.0, so every task gets the 512 B extra).
#pragma once

#include "sdkconfig.h"

#define TASK_EXTRA_STACK_SIZE (512)
#define ESP_TASKD_EVENT_STACK (CONFIG_ESP_SYSTEM_EVENT_TASK_STACK_SIZE + TASK_EXTRA_STACK_SIZE)
#define ESP_TASK_TCPIP_STACK (CONFIG_LWIP_TCPIP_TASK_STACK_SIZE + TASK_EXTRA_STACK_SIZE)
