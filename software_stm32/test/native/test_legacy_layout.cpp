#include <stdint.h>
#include <string.h>

#include "doctest.h"
#include "vdm/legacy_layout.h"

using vdm::LegacyLayout;
using vdm::SensorSlot;

namespace {

typedef uint8_t Image[vdm::kLegacyImageSize];

void fillSlot(SensorSlot& s, uint8_t family, uint8_t n) {
  s.familycode = family;
  for (uint8_t k = 0; k < 6; k++) s.romcode[k] = static_cast<uint8_t>(n * 11 + k * 37 + 1);
  s.crc = static_cast<uint8_t>(0xC0 ^ n);
}

// A different value in every field and in every byte of a slot, so any change
// of the byte order shows in the golden image.
LegacyLayout sampleLayout() {
  LegacyLayout l;
  memset(&l, 0, sizeof(l));
  l.b_slave = 0x5A;
  for (uint8_t i = 0; i < 25; i++) l.descr[i] = static_cast<char>('A' + i);
  l.OneWireCfg[0] = 0x11;
  l.OneWireCfg[1] = 0x22;
  l.OneWireCfg[2] = 0x33;
  l.currentbound_low_fac = 17;
  l.currentbound_high_fac = 23;
  l.numberOfMovements = 0x1234;
  for (uint8_t s = 0; s < 12; s++) fillSlot(l.owsensors1[s], 0x28, s);
  for (uint8_t s = 0; s < 12; s++) fillSlot(l.owsensors2[s], 0x10, static_cast<uint8_t>(12 + s));
  for (uint8_t s = 0; s < 10; s++) fillSlot(l.owsensors[s], 0x22, static_cast<uint8_t>(24 + s));
  l.startOnPower = 42;
  l.noOfMinCounts = 0xABCD;
  l.maxCalibRetries = 2;
  return l;
}

// sampleLayout() as the 58632d6 eeprom_write_layout() wrote it at 0x0007
// (generated once with that writer against a fake I2C_eeprom)
const Image kGolden = {
    0x5A, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A, 0x4B,
    0x4C, 0x4D, 0x4E, 0x4F, 0x50, 0x51, 0x52, 0x53, 0x54, 0x55, 0x56, 0x57,
    0x58, 0x59, 0x11, 0x22, 0x33, 0x11, 0x17, 0x34, 0x12, 0x28, 0xBA, 0x95,
    0x70, 0x4B, 0x26, 0x01, 0xC0, 0x28, 0xC5, 0xA0, 0x7B, 0x56, 0x31, 0x0C,
    0xC1, 0x28, 0xD0, 0xAB, 0x86, 0x61, 0x3C, 0x17, 0xC2, 0x28, 0xDB, 0xB6,
    0x91, 0x6C, 0x47, 0x22, 0xC3, 0x28, 0xE6, 0xC1, 0x9C, 0x77, 0x52, 0x2D,
    0xC4, 0x28, 0xF1, 0xCC, 0xA7, 0x82, 0x5D, 0x38, 0xC5, 0x28, 0xFC, 0xD7,
    0xB2, 0x8D, 0x68, 0x43, 0xC6, 0x28, 0x07, 0xE2, 0xBD, 0x98, 0x73, 0x4E,
    0xC7, 0x28, 0x12, 0xED, 0xC8, 0xA3, 0x7E, 0x59, 0xC8, 0x28, 0x1D, 0xF8,
    0xD3, 0xAE, 0x89, 0x64, 0xC9, 0x28, 0x28, 0x03, 0xDE, 0xB9, 0x94, 0x6F,
    0xCA, 0x28, 0x33, 0x0E, 0xE9, 0xC4, 0x9F, 0x7A, 0xCB, 0x10, 0x3E, 0x19,
    0xF4, 0xCF, 0xAA, 0x85, 0xCC, 0x10, 0x49, 0x24, 0xFF, 0xDA, 0xB5, 0x90,
    0xCD, 0x10, 0x54, 0x2F, 0x0A, 0xE5, 0xC0, 0x9B, 0xCE, 0x10, 0x5F, 0x3A,
    0x15, 0xF0, 0xCB, 0xA6, 0xCF, 0x10, 0x6A, 0x45, 0x20, 0xFB, 0xD6, 0xB1,
    0xD0, 0x10, 0x75, 0x50, 0x2B, 0x06, 0xE1, 0xBC, 0xD1, 0x10, 0x80, 0x5B,
    0x36, 0x11, 0xEC, 0xC7, 0xD2, 0x10, 0x8B, 0x66, 0x41, 0x1C, 0xF7, 0xD2,
    0xD3, 0x10, 0x96, 0x71, 0x4C, 0x27, 0x02, 0xDD, 0xD4, 0x10, 0xA1, 0x7C,
    0x57, 0x32, 0x0D, 0xE8, 0xD5, 0x10, 0xAC, 0x87, 0x62, 0x3D, 0x18, 0xF3,
    0xD6, 0x10, 0xB7, 0x92, 0x6D, 0x48, 0x23, 0xFE, 0xD7, 0x22, 0xC2, 0x9D,
    0x78, 0x53, 0x2E, 0x09, 0xD8, 0x22, 0xCD, 0xA8, 0x83, 0x5E, 0x39, 0x14,
    0xD9, 0x22, 0xD8, 0xB3, 0x8E, 0x69, 0x44, 0x1F, 0xDA, 0x22, 0xE3, 0xBE,
    0x99, 0x74, 0x4F, 0x2A, 0xDB, 0x22, 0xEE, 0xC9, 0xA4, 0x7F, 0x5A, 0x35,
    0xDC, 0x22, 0xF9, 0xD4, 0xAF, 0x8A, 0x65, 0x40, 0xDD, 0x22, 0x04, 0xDF,
    0xBA, 0x95, 0x70, 0x4B, 0xDE, 0x22, 0x0F, 0xEA, 0xC5, 0xA0, 0x7B, 0x56,
    0xDF, 0x22, 0x1A, 0xF5, 0xD0, 0xAB, 0x86, 0x61, 0xE0, 0x22, 0x25, 0x00,
    0xDB, 0xB6, 0x91, 0x6C, 0xE1, 0x2A, 0xCD, 0xAB, 0x02,
};

bool sameSlot(const SensorSlot& a, const SensorSlot& b) {
  return a.familycode == b.familycode && memcmp(a.romcode, b.romcode, sizeof(a.romcode)) == 0 && a.crc == b.crc;
}

bool sameLayout(const LegacyLayout& a, const LegacyLayout& b) {
  if (a.b_slave != b.b_slave || memcmp(a.descr, b.descr, sizeof(a.descr)) != 0 ||
      memcmp(a.OneWireCfg, b.OneWireCfg, sizeof(a.OneWireCfg)) != 0 ||
      a.currentbound_low_fac != b.currentbound_low_fac || a.currentbound_high_fac != b.currentbound_high_fac ||
      a.numberOfMovements != b.numberOfMovements || a.startOnPower != b.startOnPower ||
      a.noOfMinCounts != b.noOfMinCounts || a.maxCalibRetries != b.maxCalibRetries) {
    return false;
  }
  for (uint8_t s = 0; s < 12; s++) {
    if (!sameSlot(a.owsensors1[s], b.owsensors1[s]) || !sameSlot(a.owsensors2[s], b.owsensors2[s])) return false;
  }
  for (uint8_t s = 0; s < 10; s++) {
    if (!sameSlot(a.owsensors[s], b.owsensors[s])) return false;
  }
  return true;
}

// a DS18B20 address with its CRC (28 FF 4C 7A 61 16 04 1A)
SensorSlot realSlot() {
  SensorSlot s;
  s.familycode = 0x28;
  const uint8_t rom[6] = {0xFF, 0x4C, 0x7A, 0x61, 0x16, 0x04};
  memcpy(s.romcode, rom, sizeof(rom));
  s.crc = 0x1A;
  return s;
}

}  // namespace

TEST_CASE("legacy layout: sizes of the 1.x image") {
  CHECK(vdm::kLegacyImageSize == 309);
  CHECK(vdm::kValveCount == 12);
  CHECK(vdm::kExtraSensorSlots == 10);
  CHECK(sizeof(SensorSlot) == 8);
}

TEST_CASE("encodeLegacyLayout: byte order of the 58632d6 writer") {
  Image out;
  memset(out, 0xEE, sizeof(out));
  vdm::encodeLegacyLayout(sampleLayout(), out);
  for (size_t i = 0; i < sizeof(out); ++i) {
    INFO("byte ", i);
    CHECK(out[i] == kGolden[i]);
  }
}

TEST_CASE("decodeLegacyLayout: the golden image gives back every field") {
  LegacyLayout l;
  memset(&l, 0xAA, sizeof(l));
  vdm::decodeLegacyLayout(kGolden, l);
  CHECK(sameLayout(l, sampleLayout()));
  // spot checks of the byte order: rom code reversed, 16-bit values little endian
  CHECK(l.owsensors1[0].romcode[5] == 0xBA);
  CHECK(l.owsensors1[0].romcode[0] == 0x01);
  CHECK(l.numberOfMovements == 0x1234);
  CHECK(l.noOfMinCounts == 0xABCD);
  CHECK(l.maxCalibRetries == 2);
}

TEST_CASE("decodeLegacyLayout(encodeLegacyLayout(x)) == x") {
  LegacyLayout in = sampleLayout();
  in.b_slave = 0;
  in.descr[24] = '\0';
  in.numberOfMovements = 0xFFFE;
  in.noOfMinCounts = 0x0100;
  in.startOnPower = 100;
  in.maxCalibRetries = 0;
  fillSlot(in.owsensors2[11], 0x28, 200);
  fillSlot(in.owsensors[9], 0x26, 201);
  Image raw;
  vdm::encodeLegacyLayout(in, raw);
  LegacyLayout out;
  memset(&out, 0x55, sizeof(out));
  vdm::decodeLegacyLayout(raw, out);
  CHECK(sameLayout(in, out));

  // and back: the bytes survive a decode/encode round trip, an erased chip included
  Image again;
  vdm::encodeLegacyLayout(out, again);
  CHECK(memcmp(raw, again, sizeof(raw)) == 0);
  Image erased;
  memset(erased, 0xFF, sizeof(erased));
  vdm::decodeLegacyLayout(erased, out);
  CHECK(out.numberOfMovements == 0xFFFF);
  CHECK(out.noOfMinCounts == 0xFFFF);
  CHECK(out.maxCalibRetries == 0xFF);
  vdm::encodeLegacyLayout(out, again);
  CHECK(memcmp(erased, again, sizeof(erased)) == 0);
}

TEST_CASE("crc16Ccitt: CRC-16/CCITT-FALSE") {
  const uint8_t check[] = {'1', '2', '3', '4', '5', '6', '7', '8', '9'};
  CHECK(vdm::crc16Ccitt(check, sizeof(check)) == 0x29B1);
  CHECK(vdm::crc16Ccitt(check, 0) == 0xFFFF);
  const uint8_t zero[] = {0x00};
  CHECK(vdm::crc16Ccitt(zero, sizeof(zero)) == 0xE1F0);
  Image erased;
  memset(erased, 0xFF, sizeof(erased));
  CHECK(vdm::crc16Ccitt(erased, sizeof(erased)) == 0xA238);
}

TEST_CASE("crc16Ccitt: every single-bit change of the layout changes the CRC") {
  const uint16_t good = vdm::crc16Ccitt(kGolden, sizeof(kGolden));
  for (size_t i = 0; i < sizeof(kGolden); ++i) {
    for (uint8_t bit = 0; bit < 8; ++bit) {
      Image raw;
      memcpy(raw, kGolden, sizeof(raw));
      raw[i] = static_cast<uint8_t>(raw[i] ^ (1u << bit));
      REQUIRE(vdm::crc16Ccitt(raw, sizeof(raw)) != good);
    }
  }
}

TEST_CASE("sensorSlotValid: never assigned, cleared and real addresses") {
  SensorSlot s;
  memset(&s, 0xFF, sizeof(s));
  CHECK(vdm::sensorSlotValid(s));  // erased: never assigned
  memset(&s, 0x00, sizeof(s));
  CHECK(vdm::sensorSlotValid(s));  // cleared assignment: CRC 0 of zeros
  CHECK(vdm::sensorSlotValid(realSlot()));
}

TEST_CASE("sensorSlotValid: one flipped bit anywhere is invalid") {
  const SensorSlot good = realSlot();
  for (size_t i = 0; i < sizeof(SensorSlot); ++i) {
    for (uint8_t bit = 0; bit < 8; ++bit) {
      SensorSlot s = good;
      uint8_t* bytes = reinterpret_cast<uint8_t*>(&s);
      bytes[i] = static_cast<uint8_t>(bytes[i] ^ (1u << bit));
      INFO("byte ", i, " bit ", static_cast<int>(bit));
      CHECK_FALSE(vdm::sensorSlotValid(s));
    }
  }
  // and in a slot that is all 0xFF but one byte
  for (size_t i = 0; i < sizeof(SensorSlot); ++i) {
    SensorSlot s;
    memset(&s, 0xFF, sizeof(s));
    reinterpret_cast<uint8_t*>(&s)[i] = 0xFE;
    INFO("byte ", i);
    CHECK_FALSE(vdm::sensorSlotValid(s));
  }
}
