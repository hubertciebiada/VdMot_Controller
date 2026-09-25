#include <stdint.h>

#include <random>

#include "doctest.h"
#include "vdm/onewire_check.h"

namespace {

// Bitwise reference from Maxim application note 27.
uint8_t referenceCrc8(const uint8_t* data, size_t length) {
  uint8_t crc = 0;
  for (size_t i = 0; i < length; ++i) {
    for (int bit = 0; bit < 8; ++bit) {
      const uint8_t in = static_cast<uint8_t>((data[i] >> bit) & 1);
      const uint8_t fb = static_cast<uint8_t>((crc ^ in) & 1);
      crc = static_cast<uint8_t>(crc >> 1);
      if (fb) crc = static_cast<uint8_t>(crc ^ 0x8C);
    }
  }
  return crc;
}

}  // namespace

TEST_CASE("crc8: Maxim AN27 ROM example") {
  const uint8_t rom[] = {0x02, 0x1C, 0xB8, 0x01, 0x00, 0x00, 0x00};
  CHECK(vdm::crc8(rom, sizeof(rom)) == 0xA2);
}

TEST_CASE("crc8: empty input and single bytes") {
  CHECK(vdm::crc8(nullptr, 0) == 0);
  const uint8_t one = 0x01;
  CHECK(vdm::crc8(&one, 1) == 0x5E);
  const uint8_t ff = 0xFF;
  CHECK(vdm::crc8(&ff, 1) == 0x35);
}

TEST_CASE("crc8: matches the reference on random data" * doctest::test_suite("fuzz")) {
  std::mt19937 rng(20260924);
  uint8_t buf[16];
  for (int round = 0; round < 2000; ++round) {
    const size_t len = rng() % (sizeof(buf) + 1);
    for (size_t i = 0; i < len; ++i) buf[i] = static_cast<uint8_t>(rng());
    REQUIRE(vdm::crc8(buf, len) == referenceCrc8(buf, len));
  }
}

TEST_CASE("isValidScratchpad: accepts a page with a matching CRC") {
  uint8_t page[9] = {0x0F, 0x80, 0x19, 0xF4, 0x01, 0x00, 0x00, 0x00, 0x00};
  page[8] = vdm::crc8(page, 8);
  CHECK(vdm::isValidScratchpad(page));
}

TEST_CASE("isValidScratchpad: rejects a CRC mismatch in data or CRC byte") {
  uint8_t page[9] = {0x0F, 0x80, 0x19, 0xF4, 0x01, 0x00, 0x00, 0x00, 0x00};
  page[8] = vdm::crc8(page, 8);
  for (size_t i = 0; i < 9; ++i) {
    uint8_t bad[9];
    for (size_t k = 0; k < 9; ++k) bad[k] = page[k];
    bad[i] ^= 0x01;
    CHECK_FALSE(vdm::isValidScratchpad(bad));
  }
}

TEST_CASE("isValidScratchpad: rejects the all-zero read of a bus held low") {
  const uint8_t zeros[9] = {0, 0, 0, 0, 0, 0, 0, 0, 0};
  REQUIRE(vdm::crc8(zeros, 8) == 0);  // the CRC alone would accept it
  CHECK_FALSE(vdm::isValidScratchpad(zeros));
}

TEST_CASE("isValidScratchpad: a zero CRC byte is fine when the data is not all zero") {
  // find data whose CRC is 0 but which is not all zero
  uint8_t page[9] = {0, 0, 0, 0, 0, 0, 0, 0, 0};
  bool found = false;
  for (int v = 1; v < 65536 && !found; ++v) {
    page[0] = static_cast<uint8_t>(v);
    page[1] = static_cast<uint8_t>(v >> 8);
    found = vdm::crc8(page, 8) == 0;
  }
  REQUIRE(found);
  CHECK(vdm::isValidScratchpad(page));
}

TEST_CASE("isValidScratchpad: an all-0xFF read (no device) fails the CRC") {
  const uint8_t ones[9] = {0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF};
  CHECK_FALSE(vdm::isValidScratchpad(ones));
}
