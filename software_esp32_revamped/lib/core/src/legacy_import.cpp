// Stub: contract in vdm/legacy_import.h; implemented by the core implementer.
#include "vdm/legacy_import.h"

namespace vdm {

ImportReport importLegacyConfig(LegacyNvsReader&, Config& out) {
  setDefaults(out);
  return ImportReport{};
}

}  // namespace vdm
