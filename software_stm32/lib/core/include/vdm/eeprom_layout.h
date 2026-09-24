// EEPROM layout versioning. Hardware-free.
//
// Firmware 1.x writes the configuration from 0x0007 to 0x013B and never
// touches the bytes behind it (0xFF on a new chip). Fields added since are
// stored in an extension block appended at kExtensionAddress:
//
//   [0] layout version (kLayoutVersion)   [1] payload length n
//   [2 .. 2+n-1] payload                  [2+n] CRC-8 (Dallas) over [0 .. 2+n-1]
//
// A newer layout only appends payload bytes, so an older reader uses the
// prefix it knows. An image without a valid block (1.x image, erased chip,
// torn write) loads with defaults.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/calibration.h"

namespace vdm {

constexpr uint16_t kExtensionAddress = 0x013C;
constexpr uint8_t kLayoutVersion = 2;
constexpr size_t kExtensionBlockSize = 16;
constexpr size_t kExtensionMaxPayload = kExtensionBlockSize - 3;

// payload of layout version 2
struct StoredExtension {
  EscalationConfig escalation;
};
constexpr size_t kExtensionPayloadV2 = 3;

constexpr StoredExtension kStoredExtensionDefault{kEscalationDefault};

enum class ExtensionState : uint8_t {
  Legacy,   // no block: image written by firmware 1.x or erased chip
  Valid,    // block read (out-of-range fields replaced by defaults)
  Corrupt,  // block present but damaged
};

// `out` is always written: defaults unless the state is Valid.
ExtensionState decodeExtension(const uint8_t (&raw)[kExtensionBlockSize], StoredExtension& out);

// Encodes the current layout version; unused bytes are 0xFF. Returns the
// number of bytes to write.
size_t encodeExtension(const StoredExtension& ext, uint8_t (&out)[kExtensionBlockSize]);

// learn-after-movements as stored by 1.x: 50..65534, otherwise the default.
constexpr uint16_t kLearnMovementsDefault = 2000;
uint16_t sanitizeLearnMovements(uint16_t stored);

}  // namespace vdm
