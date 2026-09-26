// Large long-lived working objects (snapshot copies, Config copies, views)
// are allocated once from the heap during static initialisation and never
// freed. Static DRAM is the scarcer resource on the ESP32 (the linker
// region is ~124 KB next to the network stack), and globals of these types
// would also carry their initial image in flash (.data) because of their
// non-zero default member initialisers.
#pragma once

#include <new>
#include <stddef.h>
#include <stdlib.h>

#include <sdkconfig.h>
#ifdef CONFIG_BT_ENABLED
#include <esp_bt.h>
#endif

// The static constructors run before initArduino() hands the DRAM reserved
// for the Bluetooth controller (~56 KB from 0x3FFB0000) to the heap, and the
// objects together need about 100 KB: without that DRAM the last one failed
// (2.1.0-revamped-rc1: abort() in do_global_ctors, a reboot every 0.5 s). So
// the first bootAlloc() releases it, whatever the constructor order. It is
// initArduino()'s own call; every later one (initArduino()'s included)
// returns ESP_ERR_INVALID_STATE, already released, and changes nothing. The
// firmware never starts Bluetooth.
inline void releaseBtDramForBoot() {
#ifdef CONFIG_BT_ENABLED
  esp_bt_controller_mem_release(ESP_BT_MODE_BTDM);
#endif
}

template <typename T>
T& bootAlloc() {
  releaseBtDramForBoot();
  T* p = new (std::nothrow) T();
  if (p == nullptr) abort();  // out of memory at boot: nothing sensible to do
  return *p;
}

// Fixed-size array wrapper for bootAlloc().
template <typename T, size_t N>
struct ObjArray {
  T items[N];
  T& operator[](size_t i) { return items[i]; }
  const T& operator[](size_t i) const { return items[i]; }
  T* data() { return items; }
};
