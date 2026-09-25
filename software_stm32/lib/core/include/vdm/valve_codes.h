// Valve status codes, the gvlvy flag and fault codes and the gstax system flags
// (see PROTOCOL_V2.md). The ESP mirrors the values in vdm/stm_codec.h, so they
// never change. Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

// Valve status (gvlvd field 3, gvlst, gvlvx/gvlvy field 2); bit 7 of the status
// byte is the calibration flag (kStatusCalibrationBit in vdm/replies_v2.h).
enum : uint8_t {
  kStIdle = 1,         // at its target
  kStOpening = 2,
  kStClosing = 3,
  kStFailed = 4,       // a move or calibration stroke timed out, the presence test measured a short, the inrush limit tripped
  kStUnknown = 5,      // not tested yet: the presence test runs next
  kStOpenCircuit = 6,  // the presence test measured no current
  kStFullOpen = 7,     // staop: opens to the end stop
  kStPresent = 8,      // found by the presence test, calibration pending
  kStBlocked = 9,      // the calibration strokes stayed too short
};

// gvlvy field 20 `flags`
constexpr uint16_t kVlvFlagFsLease = 0x0001;       // drive = failsafe position because the lease expired
constexpr uint16_t kVlvFlagFsBlocked = 0x0002;     // drive = failsafe position because the valve is blocked
constexpr uint16_t kVlvFlagUncalibrated = 0x0004;  // no valid calibration counts
constexpr uint16_t kVlvFlagNeedsRef = 0x0008;      // position not referenced: the next move runs to an end stop first
constexpr uint16_t kVlvFlagRecal = 0x0010;         // full calibration required once the valve is present
constexpr uint16_t kVlvFlagCalRestored = 0x0020;   // counts restored from the EEPROM, no calibration since start-up
constexpr uint16_t kVlvFlagRetry = 0x0040;         // automatic calibration retry scheduled
constexpr uint16_t kVlvFlagEarlyPending = 0x0080;  // one early partial stop at the current drive target
constexpr uint16_t kVlvFlagAssembly = 0x0100;      // staop hold: the lease failsafe skips the valve
constexpr uint16_t kVlvFlagSvcHold = 0x0200;       // left where a service move or sstop ended

// gvlvy field 21 `fault`, cleared by a successful calibration and by a presence
// test that finds the valve present or absent
enum class ValveFault : uint8_t {
  None = 0,
  MoveTimeout = 1,      // status 4: a move ran 120 s without an end stop
  StrokeTimeout = 2,    // status 4: a calibration stroke timed out
  Short = 3,            // status 4: the presence test measured a short
  StrokesTooShort = 4,  // status 9: the calibration strokes stayed too short
  InrushTrip = 5,       // status 4: the inrush limit tripped at a motor start (never blocks)
};

// gstax field 23 `sysFlags`
constexpr uint8_t kSysFlagProtectSuspended = 0x01;  // short and inrush limits off until the next start

}  // namespace vdm
