#include "vdm/onewire_check.h"

namespace vdm {

uint8_t crc8(const uint8_t* data, size_t length) {
  uint8_t crc = 0;
  for (size_t i = 0; i < length; ++i) {
    uint8_t byte = data[i];
    for (uint8_t bit = 0; bit < 8; ++bit) {
      const bool mix = ((crc ^ byte) & 0x01) != 0;
      crc >>= 1;
      if (mix) crc ^= 0x8C;
      byte >>= 1;
    }
  }
  return crc;
}

bool isValidScratchpad(const uint8_t (&page)[9]) {
  bool allZero = true;
  for (uint8_t b : page) {
    if (b != 0) allZero = false;
  }
  return !allZero && crc8(page, 8) == page[8];
}

}  // namespace vdm
