// Stub: contract in vdm/stm_flasher.h; implemented by the core implementer.
#include "vdm/stm_flasher.h"

namespace vdm {

const char* flashPhaseName(FlashPhase) { return ""; }
uint8_t legacyFlashStatus(FlashPhase) { return 0; }
const char* flashErrorName(FlashError) { return ""; }

FlashError validateImage(FlashImage&, uint16_t, bool, ImageInfo& out) {
  out = ImageInfo{};
  return FlashError::ImageEmpty;
}

uint8_t sectorsForImage(uint32_t) { return 0; }

StmFlasher::StmFlasher(FlashTransport& transport) : t_(transport), rx_() {}

bool StmFlasher::begin(FlashImage&, const FlashOptions&, uint32_t) { return false; }
FlashPhase StmFlasher::step(uint32_t) { return st_.phase; }
void StmFlasher::abort() { abortRequested_ = true; }
bool StmFlasher::active() const { return false; }

}  // namespace vdm
