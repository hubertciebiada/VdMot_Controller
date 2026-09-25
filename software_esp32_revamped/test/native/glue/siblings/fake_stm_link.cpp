// Sibling fake of src/stm_link.cpp (see siblings.h).
#include "fakes/fakes.h"
#include "siblings.h"
#include "stm_link.h"

namespace stm_link {

void releaseReset() {
  ++sib::stmLink().releaseResets;
  fakes::note("stm_link.releaseReset");
}

void begin() {
  ++sib::stmLink().begins;
  fakes::note("stm_link.begin");
}

void task(void*) {
  ++sib::stmLink().tasks;
  fakes::note("stm_link.task");
}

void pulseReset() {
  ++sib::stmLink().pulseResets;
  fakes::note("stm_link.pulseReset");
}

}  // namespace stm_link
