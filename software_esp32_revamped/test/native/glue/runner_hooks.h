// Runner hooks of the ESP glue suite (tools/native/testkit): what a restart of the ESP keeps.
//   NVS, the LittleFS tree        kept by every reset
//   OTA image states and the      kept by every reset; the next boot runs the bootloader's
//   boot selection                choice (fakes::Ota::bootloader())
//   RTC_NOINIT_ATTR section       kept by software, pin and watchdog resets; 0xA5 after power-on
//   esp_reset_reason()            ESP_RST_POWERON, _EXT (pin), _SW, _TASK_WDT (watchdog)
// Invariants after every case and before every reboot: no RTOS violation, no critical section
// left entered, every HTTP request answered exactly once. The hooks register themselves; every
// glue executable links runner_hooks.cpp.
#pragma once

#include <stdint.h>

#include <vector>

namespace glue {

// Copy of the RTC_NOINIT_ATTR section of this executable (empty when nothing is placed there).
std::vector<uint8_t> rtcSnapshot();

}  // namespace glue
