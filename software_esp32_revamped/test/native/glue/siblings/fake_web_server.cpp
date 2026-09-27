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

void service(uint32_t nowMs) {
  sib::web().services.push_back(nowMs);
  fakes::note("web.service " + std::to_string(nowMs));
}

}  // namespace web
