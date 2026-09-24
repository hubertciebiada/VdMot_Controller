// Integrity checks for data read from 1-Wire devices. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace vdm {

// Dallas/Maxim CRC-8 (polynomial x^8 + x^5 + x^4 + 1), as used by 1-Wire devices.
uint8_t crc8(const uint8_t* data, size_t length);

// A scratchpad page as read from a DS2438: 8 data bytes followed by their CRC.
// Rejects a CRC mismatch and an all-zero read, which has a valid CRC but is
// what a bus held low returns.
bool isValidScratchpad(const uint8_t (&page)[9]);

}  // namespace vdm
