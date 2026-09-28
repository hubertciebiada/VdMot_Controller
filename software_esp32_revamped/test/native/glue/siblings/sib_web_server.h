// Scripted results and recorded calls of the sibling fake of src/web_server.cpp
// (siblings/fake_web_server.cpp); both files go with src/web_server.cpp. See siblings.h.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "web_server.h"

namespace sib {

struct Web {
  bool started = false;  // set by begin()
  int begins = 0;
};
Web& web();

}  // namespace sib
