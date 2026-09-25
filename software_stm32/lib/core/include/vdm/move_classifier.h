// Classification of a finished motor move (stop reason, early end stop) and
// the per-move record reported by gvlvx. Hardware-free.
#pragma once

#include <stdint.h>

namespace vdm {

// Wire values of gvlvx lastStop.
enum class StopReason : uint8_t {
  None = 0,               // no move since start-up
  Target = 1,             // requested pulse count reached
  EndStop = 2,            // current above the end-stop threshold
  EarlyEndStop = 3,       // end stop after less than half of the expected travel
  Timeout = 4,            // motor ran too long without an end stop
  Undercurrent = 5,       // no motor current (open circuit)
  SafetyOvercurrent = 6,  // safety or hard current limit
  Aborted = 7,            // move could not be started / was cancelled
};

// What the motor layer saw when the move ended.
enum class MotorStop : uint8_t {
  None = 0,
  CountReached,
  EndStop,
  SafetyOvercurrent,
  Undercurrent,
  Timeout,
  Aborted,
};

enum MoveDirection : uint8_t { kDirOpen = 0, kDirClose = 1 };

// Pulse count meaning "run until an end stop".
constexpr uint16_t kRunToEndStop = 0xFFFF;

// A move to an end stop is checked for an early end stop only when it was
// expected to cover at least this much of the full stroke. On a shorter move
// half of the expected travel is within the error of the believed position
// (scaler rounding, drift over many partial moves) and of the 250 ms inrush
// time in which no end stop is detected, so a correct stop would count as early.
constexpr uint8_t kEarlyCheckMinTravelPct = 50;

struct MoveRequest {
  uint8_t dir;               // MoveDirection
  uint16_t requestedCounts;  // kRunToEndStop for a move to an end stop
  // For a move to an end stop: expected travel in % of the full stroke
  // (100 = full travel); below kEarlyCheckMinTravelPct (0 included) the
  // early end stop check is off.
  uint8_t expectedTravelPct;
  // Full stroke learned by the last successful calibration, 0 = unknown.
  uint32_t learnedTravel;
  // A partial move (requestedCounts != kRunToEndStop) is checked for an
  // early end stop: true only for normal partial moves (not for service moves,
  // calibration strokes and the moves of a failed or blocked valve).
  bool partialEarlyCheck = false;
};

// A partial move that ends at an end stop before this share of the requested
// pulses stopped early.
constexpr uint8_t kPartialEarlyPct = 80;

struct MoveResult {
  uint8_t dir;
  uint16_t requestedCounts;
  uint16_t countedCounts;
  uint8_t stopReason;   // StopReason
  uint16_t peakCurrent;  // 0.1 mA, largest filtered |current|
  uint32_t durationMs;
};

struct MoveClassification {
  StopReason reason;
  bool early;  // end stop (threshold or safety) before half of the expected travel
};

// Early: a move to an end stop with expectedTravelPct >= kEarlyCheckMinTravelPct
// that ended at an end stop (threshold or safety limit) after less than 50 % of
// learnedTravel * expectedTravelPct / 100; a partial move with
// partialEarlyCheck that ended at an end stop before kPartialEarlyPct % of the
// requested pulses. A safety stop keeps reason SafetyOvercurrent but may still
// be early.
MoveClassification classifyMove(const MoveRequest& req, MotorStop stop, uint32_t counted);

MoveResult makeMoveResult(const MoveRequest& req, StopReason reason, uint32_t counted,
                          int32_t peak, uint32_t durationMs);

// Position after a move that ended at an end stop: 100 / 0 for a move to the
// end stop (kRunToEndStop); for a partial move the start moved by the counted
// pulses (counted / scaler %, the start when the scaler is 0), clamped to 0..100.
uint8_t positionAfterEndStop(uint8_t start, uint8_t dir, uint16_t requestedCounts, uint32_t counted,
                             uint32_t scaler);

// Early partial stops in a row of one valve: the second one requests a calibration.
class EarlyStopRun {
 public:
  // partialEarly: this move stopped early; true when it is the second in a row
  // (the run starts again); any other move ends the run
  bool onMove(bool partialEarly);
  void reset() { run_ = 0; }
  uint8_t count() const { return run_; }

 private:
  uint8_t run_ = 0;
};

}  // namespace vdm
