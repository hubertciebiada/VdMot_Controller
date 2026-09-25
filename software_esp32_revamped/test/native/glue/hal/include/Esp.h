// Fake Arduino-ESP32 2.0.7 Esp.h: heap and sketch figures from fakes::esp().
#pragma once

#include <stdint.h>

class EspClass {
 public:
  uint32_t getHeapSize();
  uint32_t getFreeHeap();
  uint32_t getMinFreeHeap();
  uint32_t getMaxAllocHeap();
  uint32_t getSketchSize();
  uint32_t getFreeSketchSpace();
  uint64_t getEfuseMac();
  const char* getSdkVersion();
  [[noreturn]] void restart();
};

extern EspClass ESP;
