// Sibling fake of src/web_server.cpp (see siblings.h).
#include "fakes/fakes.h"
#include "siblings.h"
#include "web_server.h"

namespace web {

void begin() {
  ++sib::web().begins;
  sib::web().started = true;
  fakes::note("web.begin");
}

bool started() { return sib::web().started; }

}  // namespace web
