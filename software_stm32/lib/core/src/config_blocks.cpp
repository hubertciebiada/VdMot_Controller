#include "vdm/config_blocks.h"

#include <string.h>

#include "vdm/calibration.h"
#include "vdm/failsafe.h"
#include "vdm/lease.h"
#include "vdm/motor_params.h"
#include "vdm/onewire_check.h"
#include "vdm/settings.h"

namespace vdm {

namespace {

uint16_t getU16(const uint8_t* p) { return static_cast<uint16_t>(p[0] | (p[1] << 8)); }

void putU16(uint8_t* p, uint16_t v) {
  p[0] = static_cast<uint8_t>(v);
  p[1] = static_cast<uint8_t>(v >> 8);
}

// Valid if the block holds at least `payload` bytes (any version from 1 on)
// and its CRC matches
BlockState checkBlock(const uint8_t* raw, size_t size, size_t payload) {
  if (raw[0] == 0xFF) return BlockState::Absent;
  const uint8_t length = raw[1];
  if (raw[0] == 0 || length < payload || length > size - 3) return BlockState::Corrupt;
  if (crc8(raw, 2u + length) != raw[2 + length]) return BlockState::Corrupt;
  return BlockState::Valid;
}

void safetyDefaults(SafetyBlock& out) {
  memset(out.failsafePct, kFailsafeDefaultPct, sizeof(out.failsafePct));
  out.shadow.lowFac = kMotorParamsDefault.lowFac;
  out.shadow.highFac = kMotorParamsDefault.highFac;
  out.shadow.movements = kLearnMovementsDefault;
  out.shadow.startOnPower = kMotorParamsDefault.startOnPower;
  out.shadow.minCounts = kMotorParamsDefault.minCounts;
  out.shadow.maxRetries = kMotorParamsDefault.maxRetries;
  out.leaseTimeoutMin = kLeaseTimeoutDefaultMin;
  out.leaseValid = false;
}

}  // namespace

BlockState decodeSafety(const uint8_t (&raw)[kSafetyBlockSize], SafetyBlock& out) {
  safetyDefaults(out);
  const BlockState state = checkBlock(raw, sizeof(raw), kSafetyPayload);
  if (state != BlockState::Valid) return state;

  const uint8_t* stored = raw + 2;
  for (uint8_t& pct : out.failsafePct) pct = sanitizeFailsafePct(*stored++);
  out.shadow.lowFac = raw[14];
  out.shadow.highFac = raw[15];
  out.shadow.movements = getU16(raw + 16);
  out.shadow.startOnPower = raw[18];
  out.shadow.minCounts = getU16(raw + 19);
  out.shadow.maxRetries = raw[21];
  const uint16_t lease = getU16(raw + 22);
  if (leaseTimeoutValid(lease)) {
    out.leaseTimeoutMin = lease;
    out.leaseValid = true;
  }
  return BlockState::Valid;
}

size_t encodeSafety(const SafetyBlock& in, uint8_t (&out)[kSafetyBlockSize]) {
  memset(out, 0xFF, sizeof(out));
  out[0] = kSafetyVersion;
  out[1] = kSafetyPayload;
  memcpy(out + 2, in.failsafePct, sizeof(in.failsafePct));
  out[14] = in.shadow.lowFac;
  out[15] = in.shadow.highFac;
  putU16(out + 16, in.shadow.movements);
  out[18] = in.shadow.startOnPower;
  putU16(out + 19, in.shadow.minCounts);
  out[21] = in.shadow.maxRetries;
  putU16(out + 22, in.leaseTimeoutMin);
  out[2 + kSafetyPayload] = crc8(out, 2 + kSafetyPayload);
  return 3 + kSafetyPayload;
}

BlockState decodeCalib(const uint8_t (&raw)[kCalibBlockSize], uint8_t valve, CalibRecord& out) {
  memset(&out, 0, sizeof(out));
  const BlockState state = checkBlock(raw, sizeof(raw), kCalibPayload);
  if (state != BlockState::Valid) return state;
  if (raw[9] != valve) return BlockState::Corrupt;

  CalibRecord r;
  r.openingCount = getU16(raw + 2);
  r.closingCount = getU16(raw + 4);
  r.meanCurrent = getU16(raw + 6);
  r.flags = raw[8];
  if ((r.flags & kCalibValid) && (r.openingCount < kMinTravelCounts || r.closingCount < kMinTravelCounts)) {
    return BlockState::Corrupt;
  }
  if (r.meanCurrent == 0 || r.meanCurrent > 1000) r.meanCurrent = kMeanCurrentDefault_mA;
  out = r;
  return BlockState::Valid;
}

size_t encodeCalib(const CalibRecord& in, uint8_t valve, uint8_t (&out)[kCalibBlockSize]) {
  memset(out, 0xFF, sizeof(out));
  out[0] = kCalibVersion;
  out[1] = kCalibPayload;
  putU16(out + 2, in.openingCount);
  putU16(out + 4, in.closingCount);
  putU16(out + 6, in.meanCurrent);
  out[8] = in.flags;
  out[9] = valve;
  out[10] = 0;
  out[2 + kCalibPayload] = crc8(out, 2 + kCalibPayload);
  return 3 + kCalibPayload;
}

}  // namespace vdm
