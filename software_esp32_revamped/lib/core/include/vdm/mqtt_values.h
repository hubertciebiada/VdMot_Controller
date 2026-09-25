// Values published over MQTT that are derived from the STM snapshot.
// Hardware-free.
#pragma once

#include <stdint.h>

#include "vdm/link_policy.h"
#include "vdm/valve_model.h"

namespace vdm {

// Legacy MQTT common/state value (0 ok, 1 info, 2 error), derived:
//  2 when the link is Down, or an ACTIVE valve has kHealthBlocked or
//    kHealthFailed;
//  1 when the link is Unknown/Degraded/Booting, or an active valve has any
//    other health flag;
//  0 otherwise (Suspended during flashing counts as 1).
uint8_t systemState(LinkState link, const ValveState* valves, uint8_t count, uint16_t activeMask);

}  // namespace vdm
