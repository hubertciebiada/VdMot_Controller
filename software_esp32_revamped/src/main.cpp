// VdMot Revamped - ESP32 (WT32-ETH01) firmware entry point.
// All work happens in the FreeRTOS tasks created by app::setup(); the Arduino
// loop task deletes itself. See DESIGN.md "Task model".
#include <Arduino.h>

#include "app.h"
#include "stm_link.h"

// Arduino-ESP32 hook: keep the new image in PENDING_VERIFY after an OTA so
// the bootloader rolls back unless ota::service() confirms it healthy
// (architecture §3, vdm::OtaValidator).
extern "C" bool verifyRollbackLater() { return true; }

// Arduino-ESP32 hook, runs in initArduino() right before setup(): release
// the STM from reset as early as possible (architecture R6; the IO15 strap
// pull-up holds it in reset while the ESP boots, specs/06 §5.2).
extern "C" void initVariant() { stm_link::releaseReset(); }

void setup() { app::setup(); }

void loop() { vTaskDelete(nullptr); }
