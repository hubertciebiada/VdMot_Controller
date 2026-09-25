// Scripted results and recorded calls of the sibling fake of src/stm_link.cpp
// (siblings/fake_stm_link.cpp); both files go with src/stm_link.cpp. See siblings.h.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "stm_link.h"

namespace sib {

struct StmLink {
  int releaseResets = 0;
  int begins = 0;
  int pulseResets = 0;
  int tasks = 0;
};
StmLink& stmLink();

}  // namespace sib
