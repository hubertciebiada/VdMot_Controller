// Fake heap: the nothrow forms of operator new replaced by forms with scripted failures
// (fakes::heap()). The memory comes from the throwing forms, so delete and ASan see an ordinary
// new.
#include <new>

#include "fakes/fakes.h"
#include "hal_internal.h"

namespace fakes {

// Constructed on first use: static initialisers of the firmware allocate before this file's
// globals would be initialised.
Heap& heap() {
  static Heap h;
  return h;
}

void resetHeapVolatile() { heap() = Heap{}; }

}  // namespace fakes

namespace {

bool allowed() {
  fakes::Heap& h = fakes::heap();
  if (h.next.empty()) return !h.failAll;
  const bool ok = h.next.front();
  h.next.pop_front();
  return ok;
}

}  // namespace

void* operator new(std::size_t size, const std::nothrow_t&) noexcept {
  if (!allowed()) return nullptr;
  try {
    void* p = ::operator new(size);
    fakes::heap().allocated.push_back(size);
    return p;
  } catch (...) {
    return nullptr;
  }
}

void* operator new[](std::size_t size, const std::nothrow_t&) noexcept {
  if (!allowed()) return nullptr;
  try {
    void* p = ::operator new[](size);
    fakes::heap().allocated.push_back(size);
    return p;
  } catch (...) {
    return nullptr;
  }
}
