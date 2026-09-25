// Blocks B "safety" and C "calibration" of the configuration EEPROM (address
// map in vdm/eeprom_layout.h). Hardware-free. Multi-byte fields little endian,
// unused bytes 0xFF, CRC-8 (Dallas) over the bytes before it.
//
// Block B (0x0160), version 1, payload length 22:
//   [0] version  [1] payload length n  [2..13] failsafePct[0..11]
//   [14..21] shadow of the 1.x motor fields: lowFac, highFac, movements (2),
//            startOnPower, minCounts (2), maxRetries
//   [22..23] leaseTimeoutMin (copy of block A)   [2+n] CRC-8 over [0 .. 1+n]
// The shadow replaces the 1.x fields when the 1.x layout fails its CRC.
//
// Block C of valve v (0x0180 + 16 v), version 1, payload length 9:
//   [0] version  [1] payload length n  [2..3] openingCount  [4..5] closingCount
//   [6..7] meanCurrent (mA)  [8] flags  [9] v  [10] 0  [2+n] CRC-8 over [0 .. 1+n]
//
// A later version only appends payload bytes; the known prefix is read.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/eeprom_layout.h"
#include "vdm/legacy_layout.h"

namespace vdm {

enum class BlockState : uint8_t {
  Absent,   // never written (0xFF)
  Valid,
  Corrupt,  // damaged: wrong version, length, CRC or content
};

struct MotorShadow {
  uint8_t lowFac;
  uint8_t highFac;
  uint16_t movements;
  uint8_t startOnPower;
  uint16_t minCounts;
  uint8_t maxRetries;
};

struct SafetyBlock {
  uint8_t failsafePct[kValveCount];  // 0..100, kFailsafeHold
  MotorShadow shadow;
  uint16_t leaseTimeoutMin;  // copy of the timeout of block A
  bool leaseValid;           // decode: the copy is a valid timeout (encode ignores it)
};

constexpr uint8_t kSafetyVersion = 1;
constexpr size_t kSafetyPayload = 22;

// `out` is always written: failsafe positions 50, the default motor fields and
// no lease copy unless the state is Valid. An invalid failsafe position loads
// 50, an invalid lease copy counts as missing.
BlockState decodeSafety(const uint8_t (&raw)[kSafetyBlockSize], SafetyBlock& out);
// Returns the number of bytes to write (25).
size_t encodeSafety(const SafetyBlock& in, uint8_t (&out)[kSafetyBlockSize]);

constexpr uint8_t kCalibValid = 0x01;   // counts of a successful calibration
constexpr uint8_t kCalibFailed = 0x02;  // the last calibration of the valve ended blocked

struct CalibRecord {
  uint16_t openingCount;
  uint16_t closingCount;
  uint16_t meanCurrent;  // mA
  uint8_t flags;         // kCalibValid, kCalibFailed; 0 = no record
};

constexpr uint8_t kCalibVersion = 1;
constexpr size_t kCalibPayload = 9;

// `out` is always written: all zero (no record) unless the state is Valid. A
// record whose counts are marked valid but shorter than kMinTravelCounts is
// Corrupt; a mean current of 0 or above 1000 mA loads kMeanCurrentDefault_mA.
BlockState decodeCalib(const uint8_t (&raw)[kCalibBlockSize], uint8_t valve, CalibRecord& out);
// Returns the number of bytes to write (12).
size_t encodeCalib(const CalibRecord& in, uint8_t valve, uint8_t (&out)[kCalibBlockSize]);

}  // namespace vdm
