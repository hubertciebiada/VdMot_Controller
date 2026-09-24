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

template <typename T>
T& bootAlloc() {
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
