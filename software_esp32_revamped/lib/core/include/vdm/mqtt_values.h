// Values published over MQTT that are derived from the STM snapshot.
// Hardware-free.
#pragma once

#include <stdint.h>

#include "vdm/link_policy.h"
#include "vdm/valve_model.h"

namespace vdm {

// System-wide conditions besides the link and the valves.
struct SystemFlags {
  bool safeMode = false;  // the STM runs in safe mode
  bool failsafe = false;  // valves are at their failsafe positions (lease or ESP emulation)
};

// Legacy MQTT common/state value (0 ok, 1 info, 2 error), derived:
//  2 when the link is Down, the STM is in safe mode, or an ACTIVE valve has
//    kHealthBlocked or kHealthFailed;
//  1 when the link is Unknown/Degraded/Booting, the failsafe is active, or
//    an active valve has any other health flag;
//  0 otherwise (Suspended during flashing counts as 1).
uint8_t systemState(LinkState link, const ValveState* valves, uint8_t count, uint16_t activeMask,
                    const SystemFlags& f = {});

}  // namespace vdm
