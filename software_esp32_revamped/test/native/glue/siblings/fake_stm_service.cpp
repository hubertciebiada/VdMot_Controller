// Sibling fake of src/stm_service.cpp (see siblings.h).
#include "fakes/fakes.h"
#include "siblings.h"
#include "stm_service.h"

namespace stm_service {

void begin() {
  ++sib::stmService().begins;
  fakes::note("stm_service.begin");
}

void service(uint32_t nowMs) {
  sib::stmService().services.push_back(nowMs);
  fakes::note("stm_service.service " + std::to_string(nowMs));
}

void flushForRestart() {
  ++sib::stmService().restartFlushes;
  fakes::note("stm_service.flushForRestart");
}

}  // namespace stm_service
