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
         e.escalation.maxmA == vdm::kEscalationDefault.maxmA;
}

StoredExtension sample() {
  StoredExtension e;
  e.escalation = vdm::EscalationConfig{1, 30, 55};
  return e;
}

}  // namespace

TEST_CASE("extension block follows the 1.x layout") {
  // 1.x: last field maxCalibRetries at 0x013B
  CHECK(vdm::kExtensionAddress == 0x013C);
  CHECK(vdm::kLayoutVersion == 2);
  // must stay inside the 24LC64
  CHECK(vdm::kExtensionAddress + vdm::kExtensionBlockSize <= 8192);
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
  CHECK(vdm::decodeExtension(raw, e) == ExtensionState::Legacy);
}

TEST_CASE("encode/decode round trip") {
  Block raw;
  const size_t n = vdm::encodeExtension(sample(), raw);
  CHECK(n == 6);
  CHECK(raw[0] == 2);
  CHECK(raw[1] == 3);
  CHECK(raw[2] == 1);
  CHECK(raw[3] == 30);
  CHECK(raw[4] == 55);
  CHECK(raw[5] == vdm::crc8(raw, 5));
  for (size_t i = n; i < sizeof(raw); ++i) CHECK(raw[i] == 0xFF);

  StoredExtension e;
  REQUIRE(vdm::decodeExtension(raw, e) == ExtensionState::Valid);
  CHECK(e.escalation.enable == 1);
  CHECK(e.escalation.stepPct == 30);
  CHECK(e.escalation.maxmA == 55);
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

TEST_CASE("decodeExtension: a newer layout with a longer payload is read by its prefix") {
  Block raw;
  memset(raw, 0xFF, sizeof(raw));
  raw[0] = 3;
  raw[1] = static_cast<uint8_t>(vdm::kExtensionMaxPayload);
  raw[2] = 1;
  raw[3] = 10;
  raw[4] = 20;
  for (size_t i = 5; i < 2 + vdm::kExtensionMaxPayload; ++i) raw[i] = 0x5A;
  raw[2 + vdm::kExtensionMaxPayload] = vdm::crc8(raw, 2 + vdm::kExtensionMaxPayload);
  StoredExtension e;
  REQUIRE(vdm::decodeExtension(raw, e) == ExtensionState::Valid);
  CHECK(e.escalation.enable == 1);
  CHECK(e.escalation.stepPct == 10);
  CHECK(e.escalation.maxmA == 20);
}

TEST_CASE("decodeExtension: out-of-range values in a valid block fall back to defaults") {
  StoredExtension bad;
  bad.escalation = vdm::EscalationConfig{1, 25, 61};
  Block raw;
  vdm::encodeExtension(bad, raw);
  StoredExtension e;
  CHECK(vdm::decodeExtension(raw, e) == ExtensionState::Valid);
  CHECK(isDefault(e));
}
