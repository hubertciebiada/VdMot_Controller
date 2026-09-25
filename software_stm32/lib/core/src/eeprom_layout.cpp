#include "vdm/eeprom_layout.h"

#include <string.h>

#include "vdm/onewire_check.h"

namespace vdm {

namespace {

uint16_t getU16(const uint8_t* p) { return static_cast<uint16_t>(p[0] | (p[1] << 8)); }

uint32_t getU32(const uint8_t* p) {
  return static_cast<uint32_t>(getU16(p)) | (static_cast<uint32_t>(getU16(p + 2)) << 16);
}

void putU16(uint8_t* p, uint16_t v) {
  p[0] = static_cast<uint8_t>(v);
  p[1] = static_cast<uint8_t>(v >> 8);
}

void putU32(uint8_t* p, uint32_t v) {
  putU16(p, static_cast<uint16_t>(v));
  putU16(p + 2, static_cast<uint16_t>(v >> 16));
}

}  // namespace

ExtensionState decodeExtension(const uint8_t (&raw)[kExtensionBlockSize], StoredExtension& out) {
  out = kStoredExtensionDefault;

  const uint8_t version = raw[0];
  // 0xFF: never written (1.x image or new chip); 0x00..0x01 were never used as a version
  if (version < kLayoutVersionV2 || version == 0xFF) return ExtensionState::Legacy;

  const uint8_t length = raw[1];
  if (length < kExtensionPayloadV2 || length > kExtensionMaxPayload) return ExtensionState::Corrupt;
  if (crc8(raw, 2u + length) != raw[2 + length]) return ExtensionState::Corrupt;

  const uint8_t* payload = raw + 2;
  EscalationConfig esc;
  esc.enable = payload[0];
  esc.stepPct = payload[1];
  esc.maxmA = payload[2];
  out.escalation = sanitizeEscalation(esc);

  if (version >= kLayoutVersion && length >= kExtensionPayloadV3) {
    out.learnTimeS = getU32(payload + 3);
    out.leaseTimeoutMin = sanitizeLeaseTimeout(getU16(payload + 7));
    out.layoutCrc = getU16(payload + 9);
    out.hasV3 = true;
  }
  return ExtensionState::Valid;
}

size_t encodeExtension(const StoredExtension& ext, uint8_t (&out)[kExtensionBlockSize]) {
  memset(out, 0xFF, sizeof(out));
  out[0] = kLayoutVersion;
  out[1] = kExtensionPayloadV3;
  uint8_t* payload = out + 2;
  payload[0] = ext.escalation.enable;
  payload[1] = ext.escalation.stepPct;
  payload[2] = ext.escalation.maxmA;
  putU32(payload + 3, ext.learnTimeS);
  putU16(payload + 7, ext.leaseTimeoutMin);
  putU16(payload + 9, ext.layoutCrc);
  out[2 + kExtensionPayloadV3] = crc8(out, 2 + kExtensionPayloadV3);
  return 3 + kExtensionPayloadV3;
}

}  // namespace vdm
