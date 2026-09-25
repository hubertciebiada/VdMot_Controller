// EEPROM layout of the configuration, and block A. Hardware-free.
//
// 24LC64 (8192 bytes), multi-byte fields little endian:
//   0x0007-0x013B  1.x layout (vdm/legacy_layout.h), byte-identical to firmware 1.x
//   0x013C-0x014B  block A "settings" (below)
//   0x014C-0x015F  reserved
//   0x0160-0x017F  block B "safety" (vdm/config_blocks.h)
//   0x0180-0x023F  blocks C0..C11 "calibration", valve v at 0x0180 + 16 v (vdm/config_blocks.h)
// Firmware 1.x writes only the 1.x layout and never touches the bytes behind it
// (0xFF on a new chip).
//
// Block A:
//   [0] layout version (kLayoutVersion)   [1] payload length n
//   [2 .. 2+n-1] payload                  [2+n] CRC-8 (Dallas) over [0 .. 2+n-1]
// Payload of version 2 (firmware 2.0.0): escalation enable, stepPct, maxmA.
// Version 3 appends learnTimeS (4 bytes), leaseTimeoutMin (2) and layoutCrc
// (2, the CRC-16 of the 1.x layout as last written). A newer layout only
// appends payload bytes, so an older reader uses the prefix it knows: 2.0.0
// takes the escalation of a version 3 block. A block that is missing (1.x
// image, erased chip) or damaged (torn write) loads with defaults.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/calibration.h"
#include "vdm/lease.h"
#include "vdm/legacy_layout.h"
#include "vdm/settings.h"

namespace vdm {

constexpr uint16_t kLegacyLayoutAddress = 0x0007;
constexpr size_t kLegacyLayoutSize = kLegacyImageSize;
constexpr uint16_t kExtensionAddress = 0x013C;
constexpr size_t kExtensionBlockSize = 16;
constexpr uint16_t kSafetyBlockAddress = 0x0160;
constexpr size_t kSafetyBlockSize = 32;
constexpr uint16_t kCalibBlockAddress = 0x0180;
constexpr size_t kCalibBlockSize = 16;
constexpr uint16_t kConfigEnd = 0x0240;
constexpr size_t kEepromSize = 8192;

static_assert(kLegacyLayoutAddress + kLegacyLayoutSize == kExtensionAddress, "block A follows the 1.x layout");
static_assert(kExtensionAddress + kExtensionBlockSize <= kSafetyBlockAddress, "block A ends before block B");
static_assert(kSafetyBlockAddress + kSafetyBlockSize == kCalibBlockAddress, "blocks C follow block B");
static_assert(kCalibBlockAddress + kValveCount * kCalibBlockSize == kConfigEnd, "one block C per valve");
static_assert(kConfigEnd <= kEepromSize, "the configuration fits the 24LC64");

constexpr uint8_t kLayoutVersionV2 = 2;  // first version with block A (firmware 2.0.0)
constexpr uint8_t kLayoutVersion = 3;    // written by this firmware
constexpr size_t kExtensionMaxPayload = kExtensionBlockSize - 3;

// payload of layout version 2, then of version 3
constexpr size_t kExtensionPayloadV2 = 3;
constexpr size_t kExtensionPayloadV3 = 11;

struct StoredExtension {
  EscalationConfig escalation = kEscalationDefault;
  uint32_t learnTimeS = kLearnTimeDefaultS;            // 0 = time trigger off
  uint16_t leaseTimeoutMin = kLeaseTimeoutDefaultMin;  // 0 = off, 5..1440
  uint16_t layoutCrc = 0;                              // crc16Ccitt() of the 1.x layout
  bool hasV3 = false;                                  // the three fields above were stored (version 3)
};

constexpr StoredExtension kStoredExtensionDefault{};

enum class ExtensionState : uint8_t {
  Legacy,   // no block: image written by firmware 1.x or erased chip
  Valid,    // block read (out-of-range fields replaced by defaults); version 2 has no hasV3 fields
  Corrupt,  // block present but damaged
};

// `out` is always written: defaults unless the state is Valid, the defaults of
// the version 3 fields unless out.hasV3.
ExtensionState decodeExtension(const uint8_t (&raw)[kExtensionBlockSize], StoredExtension& out);

// Encodes the current layout version; unused bytes are 0xFF. Returns the
// number of bytes to write.
size_t encodeExtension(const StoredExtension& ext, uint8_t (&out)[kExtensionBlockSize]);

}  // namespace vdm
