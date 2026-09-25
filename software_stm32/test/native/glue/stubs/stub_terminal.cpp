#include "stub_terminal.h"

HardwareSerial Serial6(USART6);
int testmode = 0;

namespace stub {

Terminal terminal;

namespace {

void resetTerminal() {
  terminal = Terminal();
  testmode = 0;
}

Registrar g_registrar(resetTerminal);

}  // namespace

}  // namespace stub

using stub::log;
using stub::terminal;

int16_t Terminal_Init(void) {
  log("Terminal_Init()");
  return terminal.init;
}

int16_t Terminal_Serve(void) {
  log("Terminal_Serve()");
  return terminal.serve;
}

void terminal_supervise(void) { log("terminal_supervise()"); }

bool terminal_manual_active(void) {
  log("terminal_manual_active()");
  return terminal.manualActive;
}
