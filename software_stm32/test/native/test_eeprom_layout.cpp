#include <stdint.h>
#include <string.h>

#include "doctest.h"
#include "vdm/eeprom_layout.h"
#include "vdm/onewire_check.h"

using vdm::ExtensionState;
using vdm::StoredExtension;

namespace {

typedef uint8_t Block[vdm::kExtensionBlockSize];

bool isDefault(const StoredExtension& e) {
  return e.escalation.enable == vdm::kEscalationDefault.enable &&
         e.escalation.stepPct == vdm::kEscalationDefault.stepPct &&
         e.escalation.maxmA == vdm::kEscalationDefault.maxmA && e.learnTimeS == 604800 &&
         e.leaseTimeoutMin == 60 && e.layoutCrc == 0 && !e.hasV3;
}

StoredExtension sample() {
  StoredExtension e;
  e.escalation = vdm::EscalationConfig{1, 30, 55};
  e.learnTimeS = 0x12345678;
  e.leaseTimeoutMin = 1440;  // 0x05A0
  e.layoutCrc = 0xBEEF;
  e.hasV3 = true;
  return e;
}

// a block as firmware 2.0.0 wrote it: version 2, the escalation only
void writeV2(Block& raw, uint8_t version, uint8_t length) {
  memset(raw, 0xFF, sizeof(raw));
  raw[0] = version;
  raw[1] = length;
  raw[2] = 1;
  raw[3] = 10;
  raw[4] = 20;
  for (size_t i = 5; i < 2u + length; ++i) raw[i] = static_cast<uint8_t>(0x30 + i);
  raw[2 + length] = vdm::crc8(raw, 2u + length);
}

// The decode rule of firmware 2.0.0 (58632d6 lib/core/src/eeprom_layout.cpp), to
// show that a downgrade keeps the escalation of a block written by this firmware.
ExtensionState decode200(const Block& raw, vdm::EscalationConfig& esc) {
  esc = vdm::kEscalationDefault;
  const uint8_t version = raw[0];
  if (version < 2 || version == 0xFF) return ExtensionState::Legacy;
  const uint8_t length = raw[1];
  if (length < 3 || length > 13) return ExtensionState::Corrupt;
  if (vdm::crc8(raw, 2u + length) != raw[2 + length]) return ExtensionState::Corrupt;
  vdm::EscalationConfig c;
  c.enable = raw[2];
  c.stepPct = raw[3];
  c.maxmA = raw[4];
  esc = vdm::sanitizeEscalation(c);
  return ExtensionState::Valid;
}

}  // namespace

TEST_CASE("EEPROM address map: 1.x layout, blocks A, B and C") {
  CHECK(vdm::kLegacyLayoutAddress == 0x0007);
  CHECK(vdm::kLegacyLayoutSize == 309);
  // 1.x: last field maxCalibRetries at 0x013B
  CHECK(vdm::kExtensionAddress == 0x013C);
  CHECK(vdm::kExtensionBlockSize == 16);
  CHECK(vdm::kSafetyBlockAddress == 0x0160);
  CHECK(vdm::kSafetyBlockSize == 32);
  CHECK(vdm::kCalibBlockAddress == 0x0180);
  CHECK(vdm::kCalibBlockSize == 16);
  CHECK(vdm::kConfigEnd == 0x0240);
  CHECK(vdm::kConfigEnd <= 8192);
  CHECK(vdm::kLayoutVersion == 3);
  CHECK(vdm::kExtensionPayloadV2 == 3);
  CHECK(vdm::kExtensionPayloadV3 == 11);
}

TEST_CASE("decodeExtension: 1.x image (erased bytes) loads defaults") {
  Block raw;
  memset(raw, 0xFF, sizeof(raw));
  StoredExtension e = sample();
  CHECK(vdm::decodeExtension(raw, e) == ExtensionState::Legacy);
  CHECK(isDefault(e));
}

TEST_CASE("decodeExtension: all-zero block and versions below 2 are legacy") {
  Block raw;
  memset(raw, 0x00, sizeof(raw));
  StoredExtension e = sample();
  CHECK(vdm::decodeExtension(raw, e) == ExtensionState::Legacy);
  CHECK(isDefault(e));
  raw[0] = 1;
  e = sample();
  CHECK(vdm::decodeExtension(raw, e) == ExtensionState::Legacy);
  CHECK(isDefault(e));
}

TEST_CASE("encodeExtension: version 3 block") {
  Block raw;
  const size_t n = vdm::encodeExtension(sample(), raw);
  CHECK(n == 14);
  CHECK(raw[0] == 3);
  CHECK(raw[1] == 11);
  CHECK(raw[2] == 1);
  CHECK(raw[3] == 30);
  CHECK(raw[4] == 55);
  // learnTimeS, leaseTimeoutMin and layoutCrc, little endian
  CHECK(raw[5] == 0x78);
  CHECK(raw[6] == 0x56);
  CHECK(raw[7] == 0x34);
  CHECK(raw[8] == 0x12);
  CHECK(raw[9] == 0xA0);
  CHECK(raw[10] == 0x05);
  CHECK(raw[11] == 0xEF);
  CHECK(raw[12] == 0xBE);
  CHECK(raw[13] == vdm::crc8(raw, 13));
  for (size_t i = n; i < sizeof(raw); ++i) CHECK(raw[i] == 0xFF);
}

TEST_CASE("encode/decode round trip") {
  Block raw;
  vdm::encodeExtension(sample(), raw);
  StoredExtension e;
  REQUIRE(vdm::decodeExtension(raw, e) == ExtensionState::Valid);
  CHECK(e.escalation.enable == 1);
  CHECK(e.escalation.stepPct == 30);
  CHECK(e.escalation.maxmA == 55);
  CHECK(e.learnTimeS == 0x12345678);
  CHECK(e.leaseTimeoutMin == 1440);
  CHECK(e.layoutCrc == 0xBEEF);
  CHECK(e.hasV3);

  // time trigger and lease off are stored values, not defaults
  StoredExtension off = sample();
  off.learnTimeS = 0;
  off.leaseTimeoutMin = 0;
  off.layoutCrc = 0;
  vdm::encodeExtension(off, raw);
  REQUIRE(vdm::decodeExtension(raw, e) == ExtensionState::Valid);
  CHECK(e.learnTimeS == 0);
  CHECK(e.leaseTimeoutMin == 0);
  CHECK(e.layoutCrc == 0);
  CHECK(e.hasV3);
}

TEST_CASE("decodeExtension: any flipped bit is detected") {
  Block good;
  const size_t n = vdm::encodeExtension(sample(), good);
  for (size_t byte = 0; byte < n; ++byte) {
    for (int bit = 0; bit < 8; ++bit) {
      Block raw;
      memcpy(raw, good, sizeof(raw));
      raw[byte] = static_cast<uint8_t>(raw[byte] ^ (1u << bit));
      StoredExtension e = sample();
      const ExtensionState s = vdm::decodeExtension(raw, e);
      INFO("byte ", byte, " bit ", bit);
      CHECK(s != ExtensionState::Valid);
      CHECK(isDefault(e));
    }
  }
}

TEST_CASE("decodeExtension: bad payload length is corrupt") {
  Block raw;
  vdm::encodeExtension(sample(), raw);
  StoredExtension e;
  raw[1] = 2;
  raw[4] = vdm::crc8(raw, 4);
  CHECK(vdm::decodeExtension(raw, e) == ExtensionState::Corrupt);
  raw[1] = static_cast<uint8_t>(vdm::kExtensionMaxPayload + 1);
  CHECK(vdm::decodeExtension(raw, e) == ExtensionState::Corrupt);
  raw[1] = 0xFF;
  CHECK(vdm::decodeExtension(raw, e) == ExtensionState::Corrupt);
  CHECK(isDefault(e));
}

TEST_CASE("decodeExtension: a 2.0.0 block keeps the escalation, the version 3 fields are defaults") {
  Block raw;
  writeV2(raw, 2, 3);
  StoredExtension e = sample();
  REQUIRE(vdm::decodeExtension(raw, e) == ExtensionState::Valid);
  CHECK(e.escalation.enable == 1);
  CHECK(e.escalation.stepPct == 10);
  CHECK(e.escalation.maxmA == 20);
  CHECK_FALSE(e.hasV3);
  CHECK(e.learnTimeS == 604800);
  CHECK(e.leaseTimeoutMin == 60);
  CHECK(e.layoutCrc == 0);
}

TEST_CASE("decodeExtension: the version, not the length, selects the version 3 fields") {
  Block raw;
  StoredExtension e;
  // version 3 with the payload of version 2: read like version 2
  writeV2(raw, 3, 3);
  REQUIRE(vdm::decodeExtension(raw, e) == ExtensionState::Valid);
  CHECK(e.escalation.stepPct == 10);
  CHECK_FALSE(e.hasV3);
  CHECK(e.learnTimeS == 604800);
  // version 3 one byte short of the version 3 fields
  writeV2(raw, 3, 10);
  REQUIRE(vdm::decodeExtension(raw, e) == ExtensionState::Valid);
  CHECK_FALSE(e.hasV3);
  // version 2 with a payload as long as version 3: the extra bytes are not version 3 fields
  writeV2(raw, 2, 11);
  REQUIRE(vdm::decodeExtension(raw, e) == ExtensionState::Valid);
  CHECK(e.escalation.maxmA == 20);
  CHECK_FALSE(e.hasV3);
  CHECK(e.learnTimeS == 604800);
  CHECK(e.leaseTimeoutMin == 60);
}

TEST_CASE("decodeExtension: a newer layout with a longer payload is read by its prefix") {
  Block raw;
  vdm::encodeExtension(sample(), raw);
  raw[0] = 4;
  raw[1] = static_cast<uint8_t>(vdm::kExtensionMaxPayload);
  raw[13] = 0x5A;
  raw[14] = 0x5A;
  raw[2 + vdm::kExtensionMaxPayload] = vdm::crc8(raw, 2 + vdm::kExtensionMaxPayload);
  StoredExtension e;
  REQUIRE(vdm::decodeExtension(raw, e) == ExtensionState::Valid);
  CHECK(e.escalation.enable == 1);
  CHECK(e.escalation.stepPct == 30);
  CHECK(e.escalation.maxmA == 55);
  CHECK(e.learnTimeS == 0x12345678);
  CHECK(e.leaseTimeoutMin == 1440);
  CHECK(e.layoutCrc == 0xBEEF);
  CHECK(e.hasV3);
}

TEST_CASE("decodeExtension: out-of-range values in a valid block fall back to defaults") {
  StoredExtension bad = sample();
  bad.escalation = vdm::EscalationConfig{1, 25, 61};
  bad.leaseTimeoutMin = 4;
  Block raw;
  vdm::encodeExtension(bad, raw);
  StoredExtension e;
  CHECK(vdm::decodeExtension(raw, e) == ExtensionState::Valid);
  CHECK(e.escalation.enable == vdm::kEscalationDefault.enable);
  CHECK(e.escalation.stepPct == vdm::kEscalationDefault.stepPct);
  CHECK(e.escalation.maxmA == vdm::kEscalationDefault.maxmA);
  CHECK(e.leaseTimeoutMin == 60);
  // the other fields of the block are still taken
  CHECK(e.learnTimeS == 0x12345678);
  CHECK(e.layoutCrc == 0xBEEF);
  CHECK(e.hasV3);
}

TEST_CASE("a version 3 block keeps the escalation after a downgrade to 2.0.0") {
  Block raw;
  vdm::encodeExtension(sample(), raw);
  vdm::EscalationConfig esc;
  REQUIRE(decode200(raw, esc) == ExtensionState::Valid);
  CHECK(esc.enable == 1);
  CHECK(esc.stepPct == 30);
  CHECK(esc.maxmA == 55);
}
