// Fake ESP-IDF 4.4 esp_timer.h: microseconds since boot from the fake clock.
#pragma once

#include <stdint.h>

int64_t esp_timer_get_time(void);
