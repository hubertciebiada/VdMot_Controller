// Scripted results and recorded calls of the sibling fake of src/stm_service.cpp
// (siblings/fake_stm_service.cpp); both files go with src/stm_service.cpp. See siblings.h.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include <vector>

#include "stm_service.h"

namespace sib {

struct StmService {
  int begins = 0;
  std::vector<uint32_t> services;
  int restartFlushes = 0;
};
StmService& stmService();

}  // namespace sib
