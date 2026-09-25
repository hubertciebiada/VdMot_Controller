#include "vdm/mqtt_values.h"

namespace vdm {

uint8_t systemState(LinkState link, const ValveState* valves, uint8_t count, uint16_t activeMask,
                    const SystemFlags& f) {
  bool info = link != LinkState::Up || f.failsafe;
  if (link == LinkState::Down || f.safeMode) return 2;
  if (valves != nullptr) {
    const uint8_t n = count < kValveCount ? count : kValveCount;
    for (uint8_t i = 0; i < n; ++i) {
      if (((activeMask >> i) & 1u) == 0) continue;
      const uint16_t h = valves[i].health;
      if (h & (kHealthBlocked | kHealthFailed)) return 2;
      if (h != 0) info = true;
    }
  }
  return info ? 1 : 0;
}

}  // namespace vdm
