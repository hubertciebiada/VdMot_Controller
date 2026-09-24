#include "vdm/eeprom_layout.h"

#include <string.h>

#include "vdm/onewire_check.h"

namespace vdm {

ExtensionState decodeExtension(const uint8_t (&raw)[kExtensionBlockSize], StoredExtension& out) {
  out = kStoredExtensionDefault;

  const uint8_t version = raw[0];
  // 0xFF: never written (1.x image or new chip); 0x00..0x01 were never used as a version
  if (version < kLayoutVersion || version == 0xFF) return ExtensionState::Legacy;

  const uint8_t length = raw[1];
  if (length < kExtensionPayloadV2 || length > kExtensionMaxPayload) return ExtensionState::Corrupt;
  if (crc8(raw, 2u + length) != raw[2 + length]) return ExtensionState::Corrupt;

  const uint8_t* payload = raw + 2;
  EscalationConfig esc;
  esc.enable = payload[0];
  esc.stepPct = payload[1];
  esc.maxmA = payload[2];
  out.escalation = sanitizeEscalation(esc);
  return ExtensionState::Valid;
}

size_t encodeExtension(const StoredExtension& ext, uint8_t (&out)[kExtensionBlockSize]) {
  memset(out, 0xFF, sizeof(out));
  out[0] = kLayoutVersion;
  out[1] = kExtensionPayloadV2;
  out[2] = ext.escalation.enable;
  out[3] = ext.escalation.stepPct;
  out[4] = ext.escalation.maxmA;
  out[2 + kExtensionPayloadV2] = crc8(out, 2 + kExtensionPayloadV2);
  return 3 + kExtensionPayloadV2;
}

uint16_t sanitizeLearnMovements(uint16_t stored) {
  return (stored >= 50 && stored < 0xFFFF) ? stored : kLearnMovementsDefault;
}

}  // namespace vdm
