#include "stub_sysstat.h"

namespace stub {

Sysstat sysstat;

namespace {

void resetSysstat() { sysstat = Sysstat(); }

Registrar g_registrar(resetSysstat);

}  // namespace

}  // namespace stub

using stub::log;
using stub::sysstat;

void sysstat_capture_reset(void) { log("sysstat_capture_reset()"); }

void sysstat_loop(void) { log("sysstat_loop()"); }

uint32_t sysstat_uptime_s(void) {
  log("sysstat_uptime_s()");
  return sysstat.uptime;
}

uint32_t sysstat_resets(void) {
  log("sysstat_resets()");
  return sysstat.resets;
}

vdm::BootReason sysstat_boot_reason(void) {
  log("sysstat_boot_reason()");
  return sysstat.reason;
}

bool sysstat_safe_mode(void) {
  log("sysstat_safe_mode()");
  return sysstat.safeMode;
}

uint8_t sysstat_wdg_resets(void) {
  log("sysstat_wdg_resets()");
  return sysstat.wdgResets;
}

void sysstat_leave_safe_mode(void) { log("sysstat_leave_safe_mode()"); }
