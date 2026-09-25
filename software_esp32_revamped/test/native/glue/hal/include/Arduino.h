// Fake Arduino-ESP32 2.0.7 (framework-arduinoespressif32 3.20007.0) Arduino.h for the ESP glue
// tests: GPIO, time and the headers the real Arduino.h pulls in (FreeRTOS, esp_system, String,
// IPAddress, HardwareSerial, Esp). State and knobs are in fakes/fakes.h.
#pragma once

#include <inttypes.h>
#include <math.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <algorithm>
#include <cmath>

#include "Esp.h"
#include "HardwareSerial.h"
#include "IPAddress.h"
#include "Print.h"
#include "WString.h"
#include "esp_system.h"
#include "freertos/FreeRTOS.h"

#define LOW 0x0
#define HIGH 0x1

#define INPUT 0x01
#define OUTPUT 0x03
#define PULLUP 0x04
#define INPUT_PULLUP 0x05
#define PULLDOWN 0x08
#define INPUT_PULLDOWN 0x09
#define OPEN_DRAIN 0x10
#define OUTPUT_OPEN_DRAIN 0x12

#define PROGMEM
#define PGM_P const char*
#define PSTR(s) (s)
#define F(s) (reinterpret_cast<const __FlashStringHelper*>(s))
#define FPSTR(p) (reinterpret_cast<const __FlashStringHelper*>(p))
#define pgm_read_byte(addr) (*reinterpret_cast<const uint8_t*>(addr))
#define memcpy_P memcpy
#define strlen_P strlen

typedef bool boolean;
typedef uint8_t byte;
typedef unsigned int word;

using std::abs;
using std::max;
using std::min;

// GPIO: levels and modes in fakes::gpio(); every write is journaled ("gpio <pin>=<level>").
void pinMode(uint8_t pin, uint8_t mode);
void digitalWrite(uint8_t pin, uint8_t val);
int digitalRead(uint8_t pin);

// Time: the fake clock (fakes::nowMs()). millis() is its low 32 bits, like the target's 32-bit
// unsigned long. delay() advances it like vTaskDelay() and is journaled ("delay <ms>").
unsigned long millis();
unsigned long micros();
void delay(uint32_t ms);
void delayMicroseconds(uint32_t us);
void yield();

// esp32-hal-time.c: SNTP with the servers and the POSIX TZ string (fakes::net().tzConfigs).
void configTzTime(const char* tz, const char* server1, const char* server2 = nullptr,
                  const char* server3 = nullptr);
void configTime(long gmtOffset_sec, int daylightOffset_sec, const char* server1,
                const char* server2 = nullptr, const char* server3 = nullptr);
bool getLocalTime(struct tm* info, uint32_t ms = 5000);

// Arduino-ESP32 hooks the firmware may define (src/main.cpp).
extern "C" bool verifyRollbackLater();
extern "C" void initVariant();
void setup();
void loop();
