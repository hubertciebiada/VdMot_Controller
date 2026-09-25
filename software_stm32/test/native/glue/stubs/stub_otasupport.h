// Link-seam stub of src/otasupport.cpp (include/otasupport.h): logs every call; BootLoop() ends
// the boot window (bootstate = 1) at its call number stub::otasupport.windowCalls.
#pragma once

#include <Arduino.h>

#include "otasupport.h"
#include "stub_log.h"

namespace stub {

struct Otasupport {
  unsigned windowCalls = 1;
};
extern Otasupport otasupport;

}  // namespace stub
