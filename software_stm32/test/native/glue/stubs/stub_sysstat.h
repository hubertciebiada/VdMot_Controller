// Link-seam stub of src/sysstat.cpp (include/sysstat.h): logs every call, returns the knobs of
// stub::sysstat.
#pragma once

#include "stub_log.h"
#include "sysstat.h"

namespace stub {

struct Sysstat {
  uint32_t uptime = 0;
  uint32_t resets = 0;
  vdm::BootReason reason = vdm::BootReason::Unknown;
  bool safeMode = false;
  uint8_t wdgResets = 0;
};
extern Sysstat sysstat;

}  // namespace stub
