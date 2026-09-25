#include "stub_otasupport.h"

uint8_t bootstate = 0;

namespace stub {

Otasupport otasupport;

namespace {

unsigned g_loopCalls = 0;

void resetOtasupport() {
  otasupport = Otasupport();
  bootstate = 0;
  g_loopCalls = 0;
}

Registrar g_registrar(resetOtasupport);

}  // namespace

}  // namespace stub

using stub::log;

void BootSetup(void) { log("BootSetup()"); }

void BootLoop(void) {
  log("BootLoop()");
  if (++stub::g_loopCalls >= stub::otasupport.windowCalls) bootstate = 1;
}
