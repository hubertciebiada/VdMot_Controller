// Link-seam stub of src/terminal.cpp (include/terminal.h): logs every call, returns the knobs of
// stub::terminal. Defines Serial6 (the debug UART), which the firmware defines in terminal.cpp: every
// glue test executable except glue_terminal links this stub.
#pragma once

#include <Arduino.h>

#include "stub_log.h"
#include "terminal.h"

namespace stub {

struct Terminal {
  int16_t init = 0;
  int16_t serve = -1;
  bool manualActive = false;
};
extern Terminal terminal;

}  // namespace stub
