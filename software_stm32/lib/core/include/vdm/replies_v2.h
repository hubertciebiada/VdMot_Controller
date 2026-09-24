// Reply formatters of the protocol v2 commands (see PROTOCOL_V2.md).
// All replies are one line of space separated integers without a trailing
// space; the caller appends CR LF. Each formatter writes all or nothing.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/buf_writer.h"
#include "vdm/calibration.h"
#include "vdm/move_classifier.h"
#include "vdm/profile_recorder.h"

namespace vdm {

constexpr uint8_t kProtocolVersion = 2;

// gvlvx calState bits
constexpr uint8_t kCalStateMask = 0x03;       // 0 idle, 1 requested, 2 running
constexpr uint8_t kCalStateIdle = 0;
constexpr uint8_t kCalStateRequested = 1;
constexpr uint8_t kCalStateRunning = 2;
constexpr uint8_t kCalFlagEarlyStop = 0x04;   // early end stop since the last good calibration
constexpr uint8_t kCalFlagLastFailed = 0x08;  // the last calibration did not succeed

// running wins over requested
uint8_t composeCalState(bool running, bool requested, bool earlyWarn, bool lastFailed);

// gvlvx status in the gvlvd encoding: valve status, bit 7 (0x80) set while a
// calibration is requested or running (the calibration flag of the valve)
constexpr uint8_t kStatusCalibrationBit = 0x80;
uint8_t encodeValveStatus(uint8_t status, bool calibration);

struct ValveExtReply {
  uint8_t index;
  uint8_t status;
  uint8_t position;
  uint8_t target;
  uint16_t meanCurrent;  // mA
  uint32_t openingCount;
  uint32_t closingCount;
  int32_t deadzoneCount;
  uint8_t calibRetries;
  uint32_t movements;
  uint8_t calState;
  uint16_t earlyStops;
  uint16_t cmdRejected;
  MoveResult last;
};

// "gvlvx" + 20 numbers of up to 11 characters, separated by spaces.
constexpr size_t kValveExtReplyMaxLen = 5 + 20 * 12;
bool formatValveExt(BufWriter& out, const ValveExtReply& r);

// "gprof idx n c1:m1 ... cn:mn": up to 32 pairs of 5-digit numbers.
constexpr size_t kProfileReplyMaxLen = 5 + 4 + 3 + kProfileSamples * 12;
bool formatProfile(BufWriter& out, uint8_t index, const ProfileRecorder& profile);

struct StatReply {
  uint32_t uptimeSeconds;
  uint32_t resets;
  uint8_t bootReason;
  uint32_t rxOverflow;
  uint32_t parseErrors;
  uint8_t eepromState;
};

// gstat eepState values
constexpr uint8_t kEepStateOk = 0;
constexpr uint8_t kEepStatePending = 1;
constexpr uint8_t kEepStateWriteFailed = 2;
constexpr uint8_t kEepStateReadFailed = 3;

// v1 "eepst x": 1 only when the configuration is stored. While a write is
// pending, and while writing fails or is disabled after a failed read, it is 0:
// the legacy ESP then does not take the configuration as saved (it waits up to
// 60 s and restarts, which resets the STM), a v2 ESP reads the cause from gstat.
constexpr uint8_t eepstSaved(uint8_t eepState) { return eepState == kEepStateOk ? 1 : 0; }

constexpr size_t kStatReplyMaxLen = 5 + 6 * 12;
bool formatStat(BufWriter& out, const StatReply& r);

// "gcalx enable stepPct maxmA"
bool formatEscalation(BufWriter& out, const EscalationConfig& c);

// "gmotx lowMin lowMax highMin highMax sopMin sopMax minCntMin minCntMax retrMin retrMax"
constexpr size_t kMotorLimitsReplyMaxLen = 5 + 10 * 6;
bool formatMotorLimits(BufWriter& out);

// "gproto 2"
bool formatProtocolVersion(BufWriter& out);

// "<cmd> ok" / "<cmd> err" and "<cmd> <index> ok" / "<cmd> <index> err <code>"
// (index -1 when the request had no valid index)
bool formatResult(BufWriter& out, const char* cmd, bool ok);
bool formatIndexedResult(BufWriter& out, const char* cmd, int32_t index, uint8_t errorCode);

}  // namespace vdm
