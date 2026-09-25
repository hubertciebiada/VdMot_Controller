// The decision part of app_loop: which valve the valve state machine works on
// next (presence test, move, calibration). One decision per call while the
// valve state machine is idle; the glue builds the views and applies the
// decision. Hardware-free.
//
// Order of one call (spec-stm 6.10 and 13 C-1..C-3):
//  0. safe mode or a temperature hold -> nothing.
//  1. presence test of an unknown valve (test index 0 2 4 .. 10 1 3 .. 11 0 ..).
//  2. plain moves (targets before calibrations), round robin:
//     a. full open requested (staop) -> to the open end stop;
//     b. open circuit and drive != actual -> presence test (retest);
//     c. blocked at its failsafe position, no request -> move there, status kept;
//     d. idle, calibrated, no request -> reference move to an end stop when the
//        position is not referenced (and the valve is due: drive != actual,
//        touched by a target request or driven by the lease failsafe), else a
//        move to the drive target.
//  3. calibrations and requests, round robin:
//     a. present and (a target change happened, an explicit request, due) -> calibrate;
//     b. idle and calibration pending (never calibrated, recal) and due -> calibrate;
//     c. a request of a valve that is not unknown or present -> mark it present.
//  After 12 step-2 decisions in a row step 3 goes first once (fairness).
//
// End-stop latch per valve: a move that ends at an end stop (EndStop,
// EarlyEndStop, SafetyOvercurrent) is not repeated in the same direction while
// the drive value and the status stay; after an early end stop of a normal move
// exactly one retry is allowed. A move of a failed or blocked valve gets no
// retry. The latch clears when the drive or the status changes, after a move in
// the other direction, a completed reference move, and when the valve gets a
// test or a calibration (and clearLatch() after a successful calibration).
#pragma once

#include <stdint.h>

#include "vdm/legacy_layout.h"
#include "vdm/move_classifier.h"
#include "vdm/valve_codes.h"

namespace vdm {

struct ValveView {
  uint8_t status;
  uint8_t actual;
  uint8_t target;
  uint8_t drive;           // vdm::driveTarget()
  bool blockedFailsafe;    // drive source BlockedFailsafe
  bool leaseForced;        // drive source LeaseFailsafe
  bool calibFlag;          // movement trigger / staln flag (calibration && calibStarted)
  bool forcedLearn;        // staln
  bool timedLearn;         // time trigger
  bool retryLearn;         // automatic retry of a failed or blocked valve
  bool earlyLearn;         // second early partial stop in a row
  bool calibrated;         // counts of a successful calibration (learned or restored)
  bool recal;              // a full calibration is required
  bool needsReference;     // position not referenced
  bool svcHold;            // left where a service move or sstop ended
  bool touched;            // accepted stgtp since the last reference move or calibration
};

enum class ActionKind : uint8_t { None, Test, OpenEnd, CloseEnd, Open, Close, Learn, MarkPresent };

struct Decision {
  ActionKind kind = ActionKind::None;
  uint8_t valve = 0;
  uint8_t delta = 0;        // Open / Close: % to move
  bool keepStatus = false;  // the move keeps the status of the valve (blocked valve to its failsafe)
  bool reference = false;   // reference move of a valve whose position is not referenced
};

struct SchedulerInputs {
  bool safeMode;
  bool holdForTemperature;
};

class ValveScheduler {
 public:
  static constexpr uint8_t kFairnessRun = 12;

  Decision next(const ValveView (&v)[kValveCount], const SchedulerInputs& in);
  // a counted rejected target of a failed or blocked valve is a target change too
  void noteChange() { firstChange_ = true; }
  bool firstChange() const { return firstChange_; }
  // The move the last decision for the valve started has ended; status is the
  // status after the move. early: the classification of the move.
  void moveEnded(uint8_t valve, StopReason reason, bool early, uint8_t status);
  // a successful calibration of the valve
  void clearLatch(uint8_t valve);
  bool latched(uint8_t valve) const { return valve < kValveCount && latch_[valve].valid; }

 private:
  struct Latch {
    bool valid;
    uint8_t dir;
    uint8_t drive;
    uint8_t status;
    bool retry;  // one retry in dir left
  };
  struct Pending {
    bool valid;
    uint8_t dir;
    uint8_t drive;
    bool keepStatus;
    bool reference;
  };

  bool step2(const ValveView (&v)[kValveCount], Decision& d);
  bool step3(const ValveView (&v)[kValveCount], Decision& d);
  bool moveAllowed(uint8_t i, const ValveView& v, uint8_t dir);
  void setMove(Decision& d, uint8_t i, const ValveView& v, bool keepStatus);

  Latch latch_[kValveCount] = {};
  Pending pending_[kValveCount] = {};
  uint8_t testIndex_ = 0;
  uint8_t rr_ = 0;
  uint8_t step2Run_ = 0;
  bool firstChange_ = false;
};

}  // namespace vdm
