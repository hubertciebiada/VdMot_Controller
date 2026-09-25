#include <stdint.h>
#include <string.h>

#include "doctest.h"
#include "vdm/config_blocks.h"
#include "vdm/onewire_check.h"

using vdm::BlockState;
using vdm::CalibRecord;
using vdm::SafetyBlock;

namespace {

typedef uint8_t SafetyRaw[vdm::kSafetyBlockSize];
typedef uint8_t CalibRaw[vdm::kCalibBlockSize];

SafetyBlock sampleSafety() {
  SafetyBlock b;
  memset(&b, 0, sizeof(b));
  const uint8_t fs[12] = {0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 255};
  memcpy(b.failsafePct, fs, sizeof(fs));
  b.shadow.lowFac = 12;
  b.shadow.highFac = 34;
  b.shadow.movements = 0x0102;
  b.shadow.startOnPower = 56;
  b.shadow.minCounts = 0x0304;
  b.shadow.maxRetries = 1;
  b.leaseTimeoutMin = 1440;  // 0x05A0
  return b;
}

bool isSafetyDefault(const SafetyBlock& b) {
  for (uint8_t pct : b.failsafePct) {
    if (pct != 50) return false;
  }
  return b.shadow.lowFac == 17 && b.shadow.highFac == 17 && b.shadow.movements == 2000 &&
         b.shadow.startOnPower == 30 && b.shadow.minCounts == 3000 && b.shadow.maxRetries == 2 &&
         b.leaseTimeoutMin == 60 && !b.leaseValid;
}

// rewrites version and payload length of an encoded block and seals it again
void reseal(uint8_t* raw, uint8_t version, uint8_t length) {
  raw[0] = version;
  raw[1] = length;
  raw[2 + length] = vdm::crc8(raw, 2u + length);
}

CalibRecord sampleCalib() {
  CalibRecord r;
  r.openingCount = 3567;  // 0x0DEF
  r.closingCount = 3610;  // 0x0E1A
  r.meanCurrent = 17;
  r.flags = vdm::kCalibValid;
  return r;
}

bool isNoRecord(const CalibRecord& r) {
  return r.openingCount == 0 && r.closingCount == 0 && r.meanCurrent == 0 && r.flags == 0;
}

BlockState decodeCalibWith(uint16_t opening, uint16_t closing, uint16_t mean, uint8_t flags, CalibRecord& out) {
  CalibRecord in;
  in.openingCount = opening;
  in.closingCount = closing;
  in.meanCurrent = mean;
  in.flags = flags;
  CalibRaw raw;
  vdm::encodeCalib(in, 5, raw);
  return vdm::decodeCalib(raw, 5, out);
}

}  // namespace

TEST_CASE("block B: version 1, payload length 22") {
  CHECK(vdm::kSafetyVersion == 1);
  CHECK(vdm::kSafetyPayload == 22);
}

TEST_CASE("encodeSafety: byte layout") {
  SafetyRaw raw;
  const size_t n = vdm::encodeSafety(sampleSafety(), raw);
  CHECK(n == 25);
  CHECK(raw[0] == 1);
  CHECK(raw[1] == 22);
  const uint8_t fs[12] = {0, 10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 255};
  CHECK(memcmp(raw + 2, fs, sizeof(fs)) == 0);
  CHECK(raw[14] == 12);
  CHECK(raw[15] == 34);
  CHECK(raw[16] == 0x02);
  CHECK(raw[17] == 0x01);
  CHECK(raw[18] == 56);
  CHECK(raw[19] == 0x04);
  CHECK(raw[20] == 0x03);
  CHECK(raw[21] == 1);
  CHECK(raw[22] == 0xA0);
  CHECK(raw[23] == 0x05);
  CHECK(raw[24] == vdm::crc8(raw, 24));
  for (size_t i = n; i < sizeof(raw); ++i) CHECK(raw[i] == 0xFF);
}

TEST_CASE("decodeSafety(encodeSafety(x)) == x") {
  SafetyRaw raw;
  vdm::encodeSafety(sampleSafety(), raw);
  SafetyBlock b;
  memset(&b, 0xAA, sizeof(b));
  REQUIRE(vdm::decodeSafety(raw, b) == BlockState::Valid);
  const SafetyBlock in = sampleSafety();
  CHECK(memcmp(b.failsafePct, in.failsafePct, sizeof(in.failsafePct)) == 0);
  CHECK(b.shadow.lowFac == 12);
  CHECK(b.shadow.highFac == 34);
  CHECK(b.shadow.movements == 0x0102);
  CHECK(b.shadow.startOnPower == 56);
  CHECK(b.shadow.minCounts == 0x0304);
  CHECK(b.shadow.maxRetries == 1);
  CHECK(b.leaseTimeoutMin == 1440);
  CHECK(b.leaseValid);
}

TEST_CASE("decodeSafety: never written is absent") {
  SafetyRaw raw;
  memset(raw, 0xFF, sizeof(raw));
  SafetyBlock b = sampleSafety();
  CHECK(vdm::decodeSafety(raw, b) == BlockState::Absent);
  CHECK(isSafetyDefault(b));
  // version byte erased, the rest written: absent as well
  vdm::encodeSafety(sampleSafety(), raw);
  raw[0] = 0xFF;
  b = sampleSafety();
  CHECK(vdm::decodeSafety(raw, b) == BlockState::Absent);
  CHECK(isSafetyDefault(b));
}

TEST_CASE("decodeSafety: every single-bit flip of bytes 0..24 is corrupt") {
  SafetyRaw good;
  vdm::encodeSafety(sampleSafety(), good);
  for (size_t byte = 0; byte < 25; ++byte) {
    for (int bit = 0; bit < 8; ++bit) {
      SafetyRaw raw;
      memcpy(raw, good, sizeof(raw));
      raw[byte] = static_cast<uint8_t>(raw[byte] ^ (1u << bit));
      SafetyBlock b = sampleSafety();
      INFO("byte ", byte, " bit ", bit);
      CHECK(vdm::decodeSafety(raw, b) == BlockState::Corrupt);
      CHECK(isSafetyDefault(b));
    }
  }
}

TEST_CASE("decodeSafety: version 0 and payload lengths outside 22..29 are corrupt") {
  SafetyRaw raw;
  SafetyBlock b;
  vdm::encodeSafety(sampleSafety(), raw);
  reseal(raw, 0, 22);
  CHECK(vdm::decodeSafety(raw, b) == BlockState::Corrupt);
  vdm::encodeSafety(sampleSafety(), raw);
  reseal(raw, 1, 21);
  CHECK(vdm::decodeSafety(raw, b) == BlockState::Corrupt);
  vdm::encodeSafety(sampleSafety(), raw);
  raw[1] = 30;
  CHECK(vdm::decodeSafety(raw, b) == BlockState::Corrupt);
  raw[1] = 0xFF;
  CHECK(vdm::decodeSafety(raw, b) == BlockState::Corrupt);
  CHECK(isSafetyDefault(b));
}

TEST_CASE("decodeSafety: a later version with a longer payload is read by its prefix") {
  SafetyRaw raw;
  vdm::encodeSafety(sampleSafety(), raw);
  for (size_t i = 24; i < 31; ++i) raw[i] = 0x5A;
  reseal(raw, 2, 29);
  SafetyBlock b;
  REQUIRE(vdm::decodeSafety(raw, b) == BlockState::Valid);
  CHECK(b.failsafePct[11] == 255);
  CHECK(b.shadow.maxRetries == 1);
  CHECK(b.leaseTimeoutMin == 1440);
  CHECK(b.leaseValid);
}

TEST_CASE("decodeSafety: invalid failsafe positions load 50, 0..100 and 255 are kept") {
  SafetyBlock in = sampleSafety();
  in.failsafePct[0] = 101;
  in.failsafePct[1] = 254;
  in.failsafePct[2] = 0;
  in.failsafePct[3] = 100;
  in.failsafePct[4] = 255;
  SafetyRaw raw;
  vdm::encodeSafety(in, raw);
  SafetyBlock b;
  REQUIRE(vdm::decodeSafety(raw, b) == BlockState::Valid);
  CHECK(b.failsafePct[0] == 50);
  CHECK(b.failsafePct[1] == 50);
  CHECK(b.failsafePct[2] == 0);
  CHECK(b.failsafePct[3] == 100);
  CHECK(b.failsafePct[4] == 255);
  CHECK(b.failsafePct[5] == 50);
  CHECK(b.failsafePct[10] == 100);
}

TEST_CASE("decodeSafety: an invalid lease copy counts as missing") {
  SafetyBlock in = sampleSafety();
  SafetyRaw raw;
  SafetyBlock b;
  in.leaseTimeoutMin = 4;
  vdm::encodeSafety(in, raw);
  REQUIRE(vdm::decodeSafety(raw, b) == BlockState::Valid);
  CHECK_FALSE(b.leaseValid);
  CHECK(b.leaseTimeoutMin == 60);
  CHECK(b.failsafePct[1] == 10);  // the block itself is used
  in.leaseTimeoutMin = 0xFFFF;
  vdm::encodeSafety(in, raw);
  REQUIRE(vdm::decodeSafety(raw, b) == BlockState::Valid);
  CHECK_FALSE(b.leaseValid);
  // 0 is the stored "off"
  in.leaseTimeoutMin = 0;
  vdm::encodeSafety(in, raw);
  REQUIRE(vdm::decodeSafety(raw, b) == BlockState::Valid);
  CHECK(b.leaseValid);
  CHECK(b.leaseTimeoutMin == 0);
}

TEST_CASE("block C: version 1, payload length 9, flags") {
  CHECK(vdm::kCalibVersion == 1);
  CHECK(vdm::kCalibPayload == 9);
  CHECK(vdm::kCalibValid == 0x01);
  CHECK(vdm::kCalibFailed == 0x02);
}

TEST_CASE("encodeCalib: byte layout") {
  CalibRaw raw;
  const size_t n = vdm::encodeCalib(sampleCalib(), 11, raw);
  CHECK(n == 12);
  CHECK(raw[0] == 1);
  CHECK(raw[1] == 9);
  CHECK(raw[2] == 0xEF);
  CHECK(raw[3] == 0x0D);
  CHECK(raw[4] == 0x1A);
  CHECK(raw[5] == 0x0E);
  CHECK(raw[6] == 17);
  CHECK(raw[7] == 0);
  CHECK(raw[8] == 0x01);
  CHECK(raw[9] == 11);
  CHECK(raw[10] == 0);
  CHECK(raw[11] == vdm::crc8(raw, 11));
  for (size_t i = n; i < sizeof(raw); ++i) CHECK(raw[i] == 0xFF);
}

TEST_CASE("decodeCalib(encodeCalib(x)) == x for valves 0 and 11") {
  const uint8_t valves[] = {0, 11};
  for (uint8_t v : valves) {
    CalibRecord in = sampleCalib();
    in.flags = static_cast<uint8_t>(vdm::kCalibValid | vdm::kCalibFailed);
    in.meanCurrent = static_cast<uint16_t>(300 + v);
    CalibRaw raw;
    vdm::encodeCalib(in, v, raw);
    CalibRecord out;
    memset(&out, 0xAA, sizeof(out));
    INFO("valve ", static_cast<int>(v));
    REQUIRE(vdm::decodeCalib(raw, v, out) == BlockState::Valid);
    CHECK(out.openingCount == 3567);
    CHECK(out.closingCount == 3610);
    CHECK(out.meanCurrent == 300 + v);
    CHECK(out.flags == 0x03);
  }
}

TEST_CASE("decodeCalib: never written is absent, no record") {
  CalibRaw raw;
  memset(raw, 0xFF, sizeof(raw));
  CalibRecord out = sampleCalib();
  CHECK(vdm::decodeCalib(raw, 0, out) == BlockState::Absent);
  CHECK(isNoRecord(out));
}

TEST_CASE("decodeCalib: a record of another valve is corrupt") {
  CalibRaw raw;
  vdm::encodeCalib(sampleCalib(), 3, raw);
  CalibRecord out = sampleCalib();
  CHECK(vdm::decodeCalib(raw, 4, out) == BlockState::Corrupt);
  CHECK(isNoRecord(out));
  vdm::encodeCalib(sampleCalib(), 0, raw);
  CHECK(vdm::decodeCalib(raw, 11, out) == BlockState::Corrupt);
  vdm::encodeCalib(sampleCalib(), 11, raw);
  CHECK(vdm::decodeCalib(raw, 0, out) == BlockState::Corrupt);
}

TEST_CASE("decodeCalib: every single-bit flip of bytes 0..11 is corrupt") {
  CalibRaw good;
  vdm::encodeCalib(sampleCalib(), 7, good);
  for (size_t byte = 0; byte < 12; ++byte) {
    for (int bit = 0; bit < 8; ++bit) {
      CalibRaw raw;
      memcpy(raw, good, sizeof(raw));
      raw[byte] = static_cast<uint8_t>(raw[byte] ^ (1u << bit));
      CalibRecord out = sampleCalib();
      INFO("byte ", byte, " bit ", bit);
      CHECK(vdm::decodeCalib(raw, 7, out) == BlockState::Corrupt);
      CHECK(isNoRecord(out));
    }
  }
}

TEST_CASE("decodeCalib: version 0 and payload lengths outside 9..13 are corrupt") {
  CalibRaw raw;
  CalibRecord out;
  vdm::encodeCalib(sampleCalib(), 2, raw);
  reseal(raw, 0, 9);
  CHECK(vdm::decodeCalib(raw, 2, out) == BlockState::Corrupt);
  vdm::encodeCalib(sampleCalib(), 2, raw);
  reseal(raw, 1, 8);
  CHECK(vdm::decodeCalib(raw, 2, out) == BlockState::Corrupt);
  vdm::encodeCalib(sampleCalib(), 2, raw);
  raw[1] = 14;
  CHECK(vdm::decodeCalib(raw, 2, out) == BlockState::Corrupt);
  // a later version with a longer payload: the known prefix is read
  vdm::encodeCalib(sampleCalib(), 2, raw);
  raw[11] = 0x5A;
  raw[12] = 0x5A;
  reseal(raw, 2, 13);
  REQUIRE(vdm::decodeCalib(raw, 2, out) == BlockState::Valid);
  CHECK(out.openingCount == 3567);
  CHECK(out.flags == vdm::kCalibValid);
}

TEST_CASE("decodeCalib: counts marked valid must be a real stroke") {
  CalibRecord out;
  CHECK(decodeCalibWith(99, 3000, 17, vdm::kCalibValid, out) == BlockState::Corrupt);
  CHECK(isNoRecord(out));
  CHECK(decodeCalibWith(3000, 99, 17, vdm::kCalibValid, out) == BlockState::Corrupt);
  CHECK(decodeCalibWith(100, 100, 17, vdm::kCalibValid, out) == BlockState::Valid);
  CHECK(out.openingCount == 100);
  CHECK(out.closingCount == 100);
  // a valve that never calibrated successfully has no counts, only the failure
  CHECK(decodeCalibWith(0, 0, 20, vdm::kCalibFailed, out) == BlockState::Valid);
  CHECK(out.flags == vdm::kCalibFailed);
  CHECK(out.openingCount == 0);
}

TEST_CASE("decodeCalib: a mean current of 0 or above 1000 mA loads 20") {
  CalibRecord out;
  REQUIRE(decodeCalibWith(3000, 3000, 0, vdm::kCalibValid, out) == BlockState::Valid);
  CHECK(out.meanCurrent == 20);
  REQUIRE(decodeCalibWith(3000, 3000, 1, vdm::kCalibValid, out) == BlockState::Valid);
  CHECK(out.meanCurrent == 1);
  REQUIRE(decodeCalibWith(3000, 3000, 1000, vdm::kCalibValid, out) == BlockState::Valid);
  CHECK(out.meanCurrent == 1000);
  REQUIRE(decodeCalibWith(3000, 3000, 1001, vdm::kCalibValid, out) == BlockState::Valid);
  CHECK(out.meanCurrent == 20);
  REQUIRE(decodeCalibWith(3000, 3000, 0xFFFF, vdm::kCalibValid, out) == BlockState::Valid);
  CHECK(out.meanCurrent == 20);
}
