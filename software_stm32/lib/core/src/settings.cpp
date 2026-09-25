#include "vdm/settings.h"

namespace vdm {

uint16_t learnMovementsFromRequest(uint32_t movements) {
  return movements == 0 ? 0
         : movements < kMinLearnMovements ? kMinLearnMovements  // NOMUTATE: `<=` is equivalent, 50 raised to 50 stays 50
         : movements > kMaxLearnMovements ? kMaxLearnMovements  // NOMUTATE: `>=` is equivalent, 65534 capped at 65534 stays 65534
                                          : static_cast<uint16_t>(movements);
}

uint16_t sanitizeLearnMovements(uint16_t stored) {
  return (stored == 0 || (stored >= kMinLearnMovements && stored <= kMaxLearnMovements))
             ? stored
             : kLearnMovementsDefault;
}

}  // namespace vdm
