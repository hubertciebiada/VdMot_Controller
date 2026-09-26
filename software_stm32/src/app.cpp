/**HEADER*******************************************************************
  project : VdMot Controller
  author : Lenti84
  Comments:
  Version :
  Modifcations :
***************************************************************************
*
* THIS SOFTWARE IS PROVIDED "AS IS" AND ANY EXPRESSED OR
* IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED WARRANTIES
* OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE DISCLAIMED.
* IN NO EVENT SHALL THE DEVELOPER OR ANY CONTRIBUTORS BE LIABLE FOR ANY DIRECT,
* INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
* (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
* SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
* HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT,
* STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING
* IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF
* THE POSSIBILITY OF SUCH DAMAGE.
*
**************************************************************************
  This program is free software: you can redistribute it and/or modify
  it under the terms of the GNU General Public License as published by
  the Free Software Foundation, either version 3 of the License.
  See the GNU General Public License for more details.
  You should have received a copy of the GNU General Public License
  along with this program.  If not, see <http://www.gnu.org/licenses/>.
  Copyright (C) 2021 Lenti84  https://github.com/Lenti84/VdMot_Controller
*END************************************************************************/

#include <Arduino.h>
#include <string.h>
#include "app.h"
#include "hardware.h"
#include "motor.h"
#include "owDevices.h"
#include "eeprom.h"
#include "sysstat.h"
#include "terminal.h"
#include "vdm/failsafe.h"
#include "vdm/fault_retry.h"
#include "vdm/lease.h"
#include "vdm/protection_guard.h"
#include "vdm/settings.h"
#include "vdm/target_rejection.h"
#include "vdm/temp_refresh.h"
#include "vdm/valve_codes.h"
#include "vdm/valve_scheduler.h"
#include "vdm/warm_state.h"

static_assert(VALVE_NO_TARGET == vdm::kNoRejectedTarget, "one marker for no rejected target");
static_assert(LEARN_AFTER_MOVEMENTS_DEFAULT == vdm::kLearnMovementsDefault, "one learn movements default");
static_assert(LEARN_AFTER_TIME_DEFAULT == vdm::kLearnTimeDefaultS, "one learn time default");
// valve status codes of the protocol
static_assert(VLV_STATE_IDLE == vdm::kStIdle, "idle");
static_assert(VLV_STATE_OPENING == vdm::kStOpening, "opening");
static_assert(VLV_STATE_CLOSING == vdm::kStClosing, "closing");
static_assert(VLV_STATE_FAILED == vdm::kStFailed, "failed");
static_assert(VLV_STATE_UNKNOWN == vdm::kStUnknown, "unknown");
static_assert(VLV_STATE_OPENCIR == vdm::kStOpenCircuit, "open circuit");
static_assert(VLV_STATE_FULLOPEN == vdm::kStFullOpen, "full open");
static_assert(VLV_STATE_PRESENT == vdm::kStPresent, "present");
static_assert(VLV_STATE_BLOCKS == vdm::kStBlocked, "blocked");


volatile struct valve myvalves[ACTUATOR_COUNT];

//unsigned char target_position_mirror[ACTUATOR_COUNT];
unsigned int learning_time = LEARN_AFTER_TIME_DEFAULT;       // stored learn time (stlnt, gtlnt)
unsigned int learning_movements = LEARN_AFTER_MOVEMENTS_DEFAULT;
static uint32_t learning_time_active = LEARN_AFTER_TIME_DEFAULT;   // the countdowns run with it (vdm::effectiveLearnTime)


unsigned int reset_request = 0;

// objects with member functions: only the main loop uses them (not members of the volatile structs)
static vdm::Lease app_lease;
static vdm::FaultRetry app_retry[ACTUATOR_COUNT];
static vdm::ValveScheduler app_sched;
static vdm::TempRefresh app_temp;
static vdm::ProtectionGuard app_guard;
static uint8_t app_failsafe[ACTUATOR_COUNT];      // active failsafe positions (sfspo, EEPROM block B)
static uint8_t move_seen[ACTUATOR_COUNT];         // myvalvemots[].moveSeq handed to the scheduler
static uint8_t trip_seen[ACTUATOR_COUNT];         // myvalvemots[].tripSeq handed to the protection guard

// valve positions, lease and retry schedule across a warm reset: not cleared by the start-up code
static vdm::WarmState warm_state __attribute__((noinit));
// lease timeout and failsafe positions of the run before a warm reset (app_restore): they stand in
// for what the EEPROM cannot supply, also at a later re-read of the EEPROM (app_load_config)
static bool app_warm_copies = false;
static uint16_t app_warm_lease;
static uint8_t app_warm_failsafe[ACTUATOR_COUNT];


static bool app_faulted (unsigned int valve) {
  const byte status = myvalvemots[valve].status;
  return status == VLV_STATE_FAILED || status == VLV_STATE_BLOCKS;
}


static vdm::Drive app_drive (unsigned int valve) {
  return vdm::driveTarget(myvalvemots[valve].target_position, app_failsafe[valve], myvalvemots[valve].status,
                          app_lease.state() == vdm::LeaseState::Expired, myvalves[valve].assemblyHold != 0);
}


// hands a calibration to the valve state machine
static void app_start_learn (unsigned int valve) {
  if (appsetaction(CMD_A_LEARN, valve, 0) != 0) return;
  myvalvemots[valve].calibState = calibInProgress;
  myvalvemots[valve].calibTime = CALIB_START_TICKS;
  myvalves[valve].forcedLearn = 0;
  myvalves[valve].timedLearn = 0;
  myvalves[valve].retryLearn = 0;
  myvalves[valve].earlyLearn = 0;
  myvalves[valve].svcHold = 0;
  myvalves[valve].touched = 0;
  // the target the calibration positions to; a later different one is new (S04)
  myvalves[valve].rejectedTarget = myvalvemots[valve].target_position;
}


// a failed or blocked valve is not driven to its target: its position stays (a blocked valve goes to
// its failsafe position) and each new target is reported as rejected (S04); a calibration (staln, the
// automatic retry) clears the fault. Returns true for a target change that was counted.
static bool app_reject_target (unsigned int valve) {
  uint8_t rejected = myvalves[valve].rejectedTarget;
  uint16_t count = myvalves[valve].cmdRejected;
  const bool counted = vdm::rejectTarget(rejected, count, myvalvemots[valve].target_position,
                                         myvalvemots[valve].actual_position);
  myvalves[valve].rejectedTarget = rejected;
  myvalves[valve].cmdRejected = count;
  return counted;
}


// a calibration of the valve is requested and has not ended yet (staln, time or movement trigger,
// automatic retry, early stops, a found valve, a valve without valid counts); status and calibration
// as read by the caller
bool app_learn_pending (uint16_t valve, byte status, bool calibration) {
  if (valve >= ACTUATOR_COUNT) return false;
  const bool pendingCal = !myvalvemots[valve].calibrated || myvalvemots[valve].recal;
  return calibration || myvalves[valve].forcedLearn || myvalves[valve].timedLearn || myvalves[valve].retryLearn
    || myvalves[valve].earlyLearn || status == VLV_STATE_PRESENT
    || ((status == VLV_STATE_IDLE || status == VLV_STATE_FULLOPEN) && pendingCal);
}


// a target request (stgtp) ends the hold of a service move and the assembly hold, also when the
// target did not change, and makes a valve without a referenced position or calibration due
void app_target_changed (uint16_t valve) {
  if (valve >= ACTUATOR_COUNT) return;
  myvalves[valve].svcHold = 0;
  myvalves[valve].assemblyHold = 0;
  myvalves[valve].touched = 1;
}


// svmov: 0 accepted, -1 invalid arguments, -2 valve state machine busy, -3 calibration pending
// (the calibration would start right after the move and undo it); safe mode moves no valve (S9.2)
int16_t app_service_move (uint16_t valve, uint8_t dir, uint16_t counts, uint8_t maxmA) {
  if (valve >= ACTUATOR_COUNT) return -1;
  if (sysstat_safe_mode()) return -2;
  if (app_learn_pending(valve, myvalvemots[valve].status, myvalvemots[valve].calibration)) return -3;
  // an accepted move also starts the hold (svcHold), a start the valve state machine refuses ends it
  return appsetservice(valve, dir, counts, maxmA);
}

int16_t app_setup (void) {

  // init valve data
  for (unsigned int x = 0;x<ACTUATOR_COUNT;x++) {
      myvalvemots[x].target_position = 50;
      myvalvemots[x].actual_position = 50;
      myvalvemots[x].status = VLV_STATE_UNKNOWN;
      myvalvemots[x].calibration = false;
      myvalvemots[x].calibState=calibIdle;
      myvalves[x].sensorindex1 = VALVE_SENSOR_UNKNOWN;        // marks that no slot is selected
      myvalves[x].sensorindex2 = VALVE_SENSOR_UNKNOWN;        // marks that no slot is selected
      myvalves[x].learn_movements = LEARN_AFTER_MOVEMENTS_DEFAULT;
      myvalves[x].movements = 0;
      myvalves[x].cmdRejected = 0;
      myvalves[x].rejectedTarget = VALVE_NO_TARGET;
      myvalves[x].forcedLearn = 0;
      myvalves[x].timedLearn = 0;
      myvalves[x].svcHold = 0;
      myvalves[x].retestRequest = 0;
      myvalves[x].openRequest = 0;
      myvalves[x].assemblyHold = 0;
      myvalves[x].retryLearn = 0;
      myvalves[x].earlyLearn = 0;
      myvalves[x].calibRestored = 0;
      myvalves[x].storedSeq = 0;
      myvalves[x].touched = 0;
      // distribute learn timing equaly over valve slots
      myvalves[x].learn_time = (unsigned int) (((long)LEARN_AFTER_TIME_DEFAULT * ((long)x+1)) / (long)ACTUATOR_COUNT);
  }

  app_load_config();
  return 0;
}


// takes the configuration from the EEPROM mirror (at start-up, and after the EEPROM could be read
// again, see eeprom.cpp): sensor assignment, learn movements and time, motor parameters, escalation,
// lease timeout and failsafe positions. A stored value out of range loads its default, and the
// mirror is corrected.
void app_load_config (void) {
  // match sensor address from eeprom with found sensors and set index/slot to valve struct
  app_match_sensors();

  // the range of stlnm (0 = off, 50..65534), so a value set at runtime survives a restart
  const uint16_t movements = vdm::sanitizeLearnMovements(eep_content.numberOfMovements);
  eep_content.numberOfMovements = movements;
  if (movements != learning_movements) app_set_learnmovements(movements);
  #ifdef appDebug
    COMM_DBG.print("learning_movements: ");
    COMM_DBG.println(learning_movements, DEC);
  #endif

  // the range table of smotc also applies to the stored values (S07): each field that is out of
  // range loads its default, and the EEPROM mirror is corrected so the next write stores valid values
  vdm::MotorParams stored;
  stored.lowFac = eep_content.currentbound_low_fac;
  stored.highFac = eep_content.currentbound_high_fac;
  stored.startOnPower = eep_content.startOnPower;
  stored.minCounts = eep_content.noOfMinCounts;
  stored.maxRetries = eep_content.maxCalibRetries;
  motor_set_params(vdm::sanitizeMotorParams(stored));
  motor_set_escalation(vdm::sanitizeEscalation(eep_content.escalation));

  // learn time (stlnt): the countdowns start again only when it changed
  if (eep_content.learnTimeS != learning_time) app_set_learntime(eep_content.learnTimeS);

  // lease timeout: block A or its copy in block B, else the copy of a warm reset, else the default;
  // failsafe positions: block B, else the copies of a warm reset (app_restore takes them at start-up).
  // The mirror of both is corrected by eeprom.cpp, not here.
  uint16_t lease = vdm::kLeaseTimeoutDefaultMin;
  if (eeprom_lease_source() != vdm::kLeaseSourceDefault) lease = vdm::sanitizeLeaseTimeout(eep_content.leaseTimeoutMin);
  else if (app_warm_copies) lease = app_warm_lease;
  app_lease.setTimeout(lease);
  const bool warmFailsafe = app_warm_copies && (eeprom_cfg_flags() & (vdm::kCfgSafetyCorrupt | vdm::kCfgReadFailed));
  for (unsigned int x = 0; x < ACTUATOR_COUNT; x++) {
    app_failsafe[x] = warmFailsafe ? app_warm_failsafe[x] : vdm::sanitizeFailsafePct(eep_content.failsafePct[x]);
  }
}


// requests of stdet, staop and the automatic retry change the status (and position) of a valve;
// they are applied here, while no valve moves, because the end of a move writes status and position
// of its valve
static void app_apply_requests (void) {
  for (unsigned int x = 0; x < ACTUATOR_COUNT; x++) {
    // a valve with a short is not calibrated: each calibration request tests it again (W10)
    if (myvalvemots[x].status == VLV_STATE_FAILED && myvalvemots[x].faultReason == (uint8_t) vdm::ValveFault::Short
        && (myvalves[x].forcedLearn || myvalves[x].timedLearn || myvalves[x].retryLearn || myvalves[x].earlyLearn
            || myvalvemots[x].calibration)) {
      myvalves[x].forcedLearn = 0;
      myvalves[x].timedLearn = 0;
      myvalves[x].retryLearn = 0;
      myvalves[x].earlyLearn = 0;
      myvalvemots[x].calibration = false;
      myvalvemots[x].calibState = calibIdle;
      myvalves[x].retestRequest = RETEST_TEST;
    }
    if (myvalves[x].retestRequest) {
      // stdet: a replaced valve head must not use the old counts (W2)
      if (myvalves[x].retestRequest == RETEST_DETECT) myvalvemots[x].recal = 1;
      myvalves[x].retestRequest = 0;
      myvalves[x].rejectedTarget = VALVE_NO_TARGET;
      myvalvemots[x].actual_position = 0;      // fake some position deviation
      myvalvemots[x].status = VLV_STATE_UNKNOWN;
    }
    if (myvalves[x].openRequest) {
      myvalves[x].openRequest = 0;
      // a failed or blocked valve is not moved until a calibration clears the fault;
      // its new target 100 is counted as rejected (S04)
      if (!app_faulted(x)) myvalvemots[x].status = VLV_STATE_FULLOPEN;
    }
  }
}


// results of the valve state machine: the end of a move for the end-stop latch, the calibration
// record for the EEPROM, the early-stop request, the trips for the protection guard
static void app_track_valves (void) {
  for (unsigned int x = 0; x < ACTUATOR_COUNT; x++) {
    volatile valvemotor &mot = myvalvemots[x];
    if (mot.moveSeq != move_seen[x]) {
      move_seen[x] = mot.moveSeq;
      struct valve_snapshot snap;
      valve_get_snapshot(x, snap);
      app_sched.moveEnded(x, (vdm::StopReason) snap.diag.last.stopReason, snap.diag.lastEarly, snap.status);
    }
    if (mot.calibSeq != myvalves[x].storedSeq) {
      myvalves[x].storedSeq = mot.calibSeq;
      vdm::CalibRecord rec;
      rec.openingCount = (uint16_t) mot.opening_count;
      rec.closingCount = (uint16_t) mot.closing_count;
      rec.meanCurrent = (uint16_t) mot.meancurrent;
      rec.flags = (uint8_t) ((mot.calibrated ? vdm::kCalibValid : 0) | (mot.calibFailed ? vdm::kCalibFailed : 0));
      eeprom_store_calib(x, rec);
      myvalves[x].calibRestored = 0;
      if (!mot.calibFailed) app_sched.clearLatch(x);
    }
    if (mot.earlyLearnDue) {
      mot.earlyLearnDue = 0;
      if (!app_faulted(x)) myvalves[x].earlyLearn = 1;
    }
    if (mot.tripSeq != trip_seen[x]) {
      trip_seen[x] = mot.tripSeq;
      app_guard.onTrip(x, sysstat_uptime_s());
    }
  }

  // short or inrush trips on several valves: the limits are off until the next start, the valves
  // they failed are tested again
  if (app_guard.suspended() && !protect_suspended) {
    protect_suspended = true;
    for (unsigned int x = 0; x < ACTUATOR_COUNT; x++) {
      const uint8_t fault = myvalvemots[x].faultReason;
      if (myvalvemots[x].status == VLV_STATE_FAILED
          && (fault == (uint8_t) vdm::ValveFault::Short || fault == (uint8_t) vdm::ValveFault::InrushTrip)) {
        myvalvemots[x].status = VLV_STATE_UNKNOWN;
        myvalves[x].rejectedTarget = VALVE_NO_TARGET;
      }
    }
  }
}


static vdm::ValveView app_view (unsigned int x) {
  vdm::ValveView v;
  const vdm::Drive drive = app_drive(x);
  v.status = myvalvemots[x].status;
  v.actual = myvalvemots[x].actual_position;
  v.target = myvalvemots[x].target_position;
  v.drive = drive.position;
  v.blockedFailsafe = drive.source == vdm::DriveSource::BlockedFailsafe;
  v.leaseForced = drive.source == vdm::DriveSource::LeaseFailsafe;
  v.calibFlag = myvalvemots[x].calibration && myvalvemots[x].calibState == calibStarted;
  v.forcedLearn = myvalves[x].forcedLearn != 0;
  v.timedLearn = myvalves[x].timedLearn != 0;
  v.retryLearn = myvalves[x].retryLearn != 0;
  v.earlyLearn = myvalves[x].earlyLearn != 0;
  v.calibrated = myvalvemots[x].calibrated != 0;
  v.recal = myvalvemots[x].recal != 0;
  v.needsReference = myvalvemots[x].needsReference != 0;
  v.svcHold = myvalves[x].svcHold != 0;
  v.touched = myvalves[x].touched != 0;
  return v;
}


// hands the decision of the scheduler to the valve state machine
static void app_apply (const vdm::Decision &d) {
  const unsigned int x = d.valve;
  char cmd = CMD_A_OPEN;
  switch (d.kind) {
    case vdm::ActionKind::Test:
      #ifdef appDebug
        COMM_DBG.print("App: valve "); COMM_DBG.print(x, 10);
        COMM_DBG.println(" unknown, try to find out...");
      #endif
      if (appsetaction(CMD_A_TEST, x, 0) == 0) myvalves[x].rejectedTarget = myvalvemots[x].target_position;
      return;
    case vdm::ActionKind::Learn:
      #ifdef appDebug
        COMM_DBG.print("App: learning started for valve ");
        COMM_DBG.println(x, 10);
      #endif
      app_start_learn(x);
      return;
    case vdm::ActionKind::MarkPresent:
      // a learn request (staln, time or movement trigger, retry, early stops) marks its valve PRESENT
      // here, while no valve moves; the next pass starts the calibration
      myvalvemots[x].status = VLV_STATE_PRESENT;
      return;
    case vdm::ActionKind::OpenEnd:
      cmd = CMD_A_OPEN_END;
      break;
    case vdm::ActionKind::CloseEnd:
      cmd = CMD_A_CLOSE_END;
      break;
    case vdm::ActionKind::Close:
      cmd = CMD_A_CLOSE;
      break;
    case vdm::ActionKind::Open:
      break;
    default:
      return;
  }
  const uint8_t flags = (uint8_t) ((d.keepStatus ? MOVE_KEEP_STATUS : 0) | (d.reference ? MOVE_REFERENCE : 0));
  if (appsetaction(cmd, x, d.delta, false, flags) != 0) return;
  // the target of this move (a failed or blocked end counts later changes, S04)
  myvalves[x].rejectedTarget = myvalvemots[x].target_position;
  myvalves[x].touched = 0;
}


int16_t app_loop (void) {
  reset_check();

  // a motor output switched on from the debug terminal: no command for the valve state machine
  if (terminal_manual_active()) return 0;

  // a due temperature cycle pauses a calibration series between two strokes (S1)
  if (temp_gap_timeout) {
    temp_gap_timeout = false;
    app_temp.holdTimedOut(millis());
  }
  temp_refresh_request = app_temp.due(millis());

  // if valve machine is idle search for new tasks; no valve moves until the next command,
  // so the status and position of every valve may be changed here
  if (!valve_idle()) return 0;

  app_track_valves();
  app_apply_requests();

  vdm::ValveView views[ACTUATOR_COUNT];
  for (unsigned int x = 0; x < ACTUATOR_COUNT; x++) {
    // a counted rejected target is a target change for the calibrations of the found valves
    if (app_faulted(x) && app_reject_target(x)) app_sched.noteChange();
    views[x] = app_view(x);
  }

  vdm::SchedulerInputs in;
  in.safeMode = sysstat_safe_mode();
  in.holdForTemperature = app_temp.holdCommands(millis());
  app_apply(app_sched.next(views, in));
  return 0;
}


byte app_10s_loop (uint32_t elapsedS) {

  unsigned int x = 0;

  for (x=0; x< ACTUATOR_COUNT; x++) {
    // the valve state machine clears the hold of a refused service move (interrupt)
    const uint32_t primask = __get_PRIMASK();
    __disable_irq();
    if (myvalves[x].svcHold) myvalves[x].svcHold--;
    __set_PRIMASK(primask);

    // backstop for a calibration handed to the valve state machine: its end (learn_end) clears the
    // state; a calibration that did not start, or ended without it, is cleared after CALIB_START_TICKS
    if (myvalvemots[x].calibState == calibInProgress) {
      if (myvalvemots[x].calibActive) myvalvemots[x].calibTime = CALIB_START_TICKS;
      else if (myvalvemots[x].calibTime > 0) myvalvemots[x].calibTime--;
      else {
        myvalvemots[x].calibration = false;
        myvalvemots[x].calibState = calibIdle;
      }
    }
  }

  // learning times, counted in real elapsed seconds (S3)
  if (learning_time_active > 0) {
    for (x=0; x< ACTUATOR_COUNT; x++) {
      uint32_t rest = myvalves[x].learn_time;
      const bool due = vdm::countdown(rest, elapsedS);
      myvalves[x].learn_time = due ? learning_time_active : rest;
      if (!due) continue;
      // a calibration of the valve that runs now (or was just handed over) satisfies the trigger;
      // a failed or blocked valve is calibrated by staln and its automatic retry only
      if (myvalvemots[x].calibActive || myvalvemots[x].calibState == calibInProgress || app_faulted(x)) continue;
      // app_loop marks the valve PRESENT while no valve moves, the next target change starts the calibration
      myvalves[x].timedLearn = 1;
      #ifdef appDebug
        COMM_DBG.print("App: Valve ");
        COMM_DBG.print(x, 10);
        COMM_DBG.println(" will be learned soon");
      #endif
    }
  }

  // learning movements
  if (learning_movements > 0) {
    for (x=0; x< ACTUATOR_COUNT; x++) {
      if (myvalvemots[x].connected && !app_faulted(x)) {
        if((myvalves[x].learn_movements == 0) && (myvalvemots[x].calibState == calibIdle)) {
          myvalvemots[x].calibration=true;
          myvalvemots[x].calibTime=10;
          myvalvemots[x].calibState = calibStarted;
          myvalves[x].movements = 0;
          myvalves[x].learn_movements = learning_movements;
          // app_loop marks the valve PRESENT while no valve moves and starts the calibration
          #ifdef appDebug
            COMM_DBG.print("App: Valve ");
            COMM_DBG.print(x, 10);
            COMM_DBG.println(" will be learned soon");
          #endif
        }
      }
    }
  }

return 0;
}


// sets learning movements
// after number of movements a learning cycle will be executed
int16_t app_set_learnmovements(uint16_t movements) {

  // update reload value
  learning_movements = movements;

  // update all valve memories
  for (unsigned int x = 0;x<ACTUATOR_COUNT;x++) {
    myvalves[x].learn_movements = learning_movements;
    myvalves[x].movements = 0;
  }

  return 0;

}


// the countdowns start again with the learn time, spread equally over the valve slots
static void app_learn_reload (uint32_t time) {
  learning_time_active = time;
  for (unsigned int x = 0;x<ACTUATOR_COUNT;x++) {
    // 64 bit product, time * (x+1) can exceed 32 bit
    myvalves[x].learn_time = (unsigned int) (((uint64_t)time * (x+1)) / ACTUATOR_COUNT);
  }
}


// sets learning time (stlnt, EEPROM)
// after time seconds a learning cycle will be executed; 0 is the ESP's own schedule, honoured while
// a lease client is present (vdm::effectiveLearnTime)
int16_t app_set_learntime(uint32_t time) {
  learning_time = time;
  app_learn_reload(vdm::effectiveLearnTime(time, app_lease.clientSeenWithin(vdm::kLearnTimeClientWindowS)));
  return 0;
}


// sets learning of valve
// a learning cycle for valve will be executed
// if valve = 255, all valves will be learned
int16_t app_set_valvelearning(uint16_t valve) {

  if(valve < ACTUATOR_COUNT) {
    // app_loop marks the valve PRESENT while no valve moves and starts the calibration
    myvalves[valve].forcedLearn = 1;
    myvalves[valve].svcHold = 0;
    myvalvemots[valve].calibration = true;
    myvalvemots[valve].calibState=calibStarted;
    myvalvemots[valve].calibTime=10;
    myvalves[valve].movements = 0;
    myvalves[valve].learn_movements = learning_movements;
    return 0;
  }
  else if (valve == 255) {
    // update all valves
    for(uint8_t xx=0;xx<ACTUATOR_COUNT;xx++){
      if (myvalvemots[xx].connected) {
        myvalves[xx].forcedLearn = 1;
        myvalves[xx].svcHold = 0;
        myvalvemots[xx].calibration = true;
        myvalvemots[xx].calibState=calibStarted;
        myvalvemots[xx].calibTime=10;
        myvalves[xx].movements = 0;
        myvalves[xx].learn_movements = learning_movements;
      }
    }
    return 0;
  }

  return -1;
}


// scan valves (stdet 255)
// every valve is tested again; a valve found present calibrates fully (app_apply_requests)
void app_scan_valves()
{
    for(unsigned int xx=0;xx<ACTUATOR_COUNT;xx++){
      myvalves[xx].retestRequest = RETEST_DETECT;
      myvalves[xx].svcHold = 0;
    }
}


// sets valve full open
// valve will be opened fully (app_loop sets FULLOPEN while no valve moves; a failed or blocked
// valve only gets the target and stays where it is until a calibration); the assembly hold keeps
// the lease failsafe away from the valve until its next stgtp
// if valve = 255, all valves will be opened fully
int16_t app_set_valveopen(uint16_t valve) {

  if(valve < ACTUATOR_COUNT) {
    myvalvemots[valve].target_position = 100;
    myvalves[valve].openRequest = 1;
    myvalves[valve].svcHold = 0;
    myvalves[valve].assemblyHold = 1;
    return 0;
  }
  else if (valve == 255) {
    // update all valves
    for(unsigned int xx=0;xx<ACTUATOR_COUNT;xx++){
      myvalvemots[xx].target_position = 100;
      myvalves[xx].openRequest = 1;
      myvalves[xx].svcHold = 0;
      myvalves[xx].assemblyHold = 1;
    }
    return 0;
  }

  return -1;
}


// true if the 1-Wire address matches the EEPROM sensor slot (rom code and crc)
static bool app_sensor_matches(const struct ds1820_eeprom_layout &slot, const uint8_t *address) {
  if (slot.crc != address[7]) return false;
  for (unsigned int z = 0; z < 6; z++) {
    if (slot.romcode[z] != address[1+z]) return false;
  }
  return true;
}


// match sensor address from eeprom with found sensors and set index/slot to valve struct
// the stored index is the position in tempsensors[] (DS18 sensors only), the same index
// space that gvlvd/gvlon and the ESP's sensor list use
int16_t app_match_sensors() {
  const uint8_t count = noOfDS18Devices < MAXONEWIRECNT ? noOfDS18Devices : MAXONEWIRECNT;

  for (uint8_t i=0;i<ACTUATOR_COUNT;i++) {
       myvalves[i].sensorindex1 = VALVE_SENSOR_UNKNOWN;
       myvalves[i].sensorindex2 = VALVE_SENSOR_UNKNOWN;
  }

  #ifdef appDebug
    COMM_DBG.println("Read 1-wire sensor addresses from eeprom");
  #endif

  for (unsigned int owsensorindex=0; owsensorindex<count; owsensorindex++)
  {
      const uint8_t *currAddress = tempsensors[owsensorindex].address;
      bool found = false;
      #ifdef appDebug
        printAddress((uint8_t *) currAddress);
      #endif

      for (unsigned int valveindex = 0;valveindex<ACTUATOR_COUNT;valveindex++) {
        // first sensor of valve
        if (app_sensor_matches(eep_content.owsensors1[valveindex], currAddress)) {
            #ifdef appDebug
              COMM_DBG.print(" found as 1st sensor at valve: ");
              COMM_DBG.println(String(valveindex)+":"+String(owsensorindex));
            #endif
            myvalves[valveindex].sensorindex1 = owsensorindex;
            found = true;
        }
        // second sensor of valve
        if (app_sensor_matches(eep_content.owsensors2[valveindex], currAddress)) {
            #ifdef appDebug
              COMM_DBG.print(" found as 2nd sensor at valve: ");
              COMM_DBG.println(valveindex, DEC);
            #endif
            myvalves[valveindex].sensorindex2 = owsensorindex;
            found = true;
        }
      }

      if(!found) {
        #ifdef appDebug
          COMM_DBG.println(" not found");
        #endif
      }
  }
  return 0;
}


// set soft reset request
void reset_STM32 () {
  reset_request = 1;
  #ifdef appDebug
    COMM_DBG.println("prepare for soft reset");
  #endif
}


// soft reset STM32
// waits until eeprom is written completely
void reset_check () {
  if(reset_request && eeprom_free()) {
    #ifdef appDebug
      COMM_DBG.println("soft reset now");
    #endif
    HAL_NVIC_SystemReset();
   // #define AIRCR_VECTKEY_MASK    (0x05FA0000)
   //   SCB->AIRCR = AIRCR_VECTKEY_MASK | 0x04;
    while(1);
  }
}


// every second: the lease, the learn time without a lease client (C-7), the automatic retries (K2)
void app_1s_tick (uint32_t elapsedS) {
  app_lease.advance(elapsedS);

  const uint32_t active = vdm::effectiveLearnTime(learning_time, app_lease.clientSeenWithin(vdm::kLearnTimeClientWindowS));
  if (active != learning_time_active) app_learn_reload(active);

  for (unsigned int x = 0; x < ACTUATOR_COUNT; x++) {
    // a calibration or presence test of the valve is requested or runs: no retry is counted down
    const bool busy = myvalves[x].retestRequest || myvalves[x].forcedLearn || myvalves[x].retryLearn
      || myvalvemots[x].calibration || myvalvemots[x].calibActive || myvalvemots[x].calibState == calibInProgress
      || myvalvemots[x].status == VLV_STATE_UNKNOWN || myvalvemots[x].status == VLV_STATE_PRESENT;
    if (app_retry[x].update(app_faulted(x), busy, elapsedS)) {
      // a short is tested again, anything else calibrates (without waiting for a target change)
      if (myvalvemots[x].faultReason == (uint8_t) vdm::ValveFault::Short) myvalves[x].retestRequest = RETEST_TEST;
      else myvalves[x].retryLearn = 1;
    }
  }
}


// start-up, after valve_setup() and before the valve timer runs: the calibration records of the
// EEPROM, and after a warm reset the positions, the lease and the retry schedule (W2)
void app_restore (void) {
  vdm::WarmState kept;
  memcpy(&kept, (const void *) &warm_state, sizeof kept);
  const bool warm = vdm::isWarmBoot(sysstat_boot_reason()) && vdm::warmStateValid(kept);

  for (unsigned int x = 0; x < ACTUATOR_COUNT; x++) {
    const vdm::CalibRecord &rec = eep_content.calib[x];
    if (rec.flags & vdm::kCalibValid) {
      myvalvemots[x].opening_count = rec.openingCount;
      myvalvemots[x].closing_count = rec.closingCount;
      myvalvemots[x].deadzone_count = (int) rec.closingCount - (int) rec.openingCount;
      myvalvemots[x].scaler = rec.openingCount / 100;
      myvalvemots[x].meancurrent = rec.meanCurrent;
      myvalvemots[x].calibrated = 1;
      myvalves[x].calibRestored = 1;
    }
    // the last calibration ended blocked: the counts are not trusted, a full calibration follows
    if (rec.flags & vdm::kCalibFailed) myvalvemots[x].recal = 1;
    myvalves[x].storedSeq = myvalvemots[x].calibSeq;

    if (!warm) continue;
    const vdm::WarmValve &w = kept.valves[x];
    const vdm::RestoredValve r = vdm::restoreValve(w, myvalvemots[x].calibrated != 0);
    // a record that does not pass its checks: this valve starts cold (presence test)
    if (!r.valid) continue;
    myvalvemots[x].status = r.status;
    myvalvemots[x].actual_position = r.actual;
    myvalvemots[x].target_position = r.target;
    myvalvemots[x].needsReference = r.needsReference;
    if (r.recal) myvalvemots[x].recal = 1;
    myvalvemots[x].connected = r.status != VLV_STATE_UNKNOWN && r.status != VLV_STATE_OPENCIR;
    myvalves[x].assemblyHold = r.assemblyHold;
    myvalves[x].rejectedTarget = r.target;
    vdm::FaultRetry::Snapshot retry;
    retry.attempts = w.retryAttempts;
    retry.scheduled = w.retryScheduled != 0;
    retry.remainingS = w.retryRemainingS;
    app_retry[x].restore(retry);
  }

  if (!warm) return;
  // lease timeout and failsafe positions the EEPROM could not supply: the copies of the last run
  app_warm_copies = true;
  app_warm_lease = vdm::sanitizeLeaseTimeout(kept.leaseTimeoutMin);
  for (unsigned int x = 0; x < ACTUATOR_COUNT; x++) app_warm_failsafe[x] = vdm::sanitizeFailsafePct(kept.failsafePct[x]);
  if (eeprom_lease_source() == vdm::kLeaseSourceDefault) app_lease.setTimeout(app_warm_lease);
  if (eeprom_cfg_flags() & (vdm::kCfgSafetyCorrupt | vdm::kCfgReadFailed)) {
    memcpy(app_failsafe, app_warm_failsafe, sizeof app_failsafe);
  }
  vdm::Lease::Snapshot lease;
  lease.sinceRenewalS = kept.leaseSinceRenewalS;
  lease.sinceClientS = kept.leaseSinceClientS;
  lease.client = kept.leaseClient != 0;
  app_lease.restore(lease);
}


// main loop, 10 ms branch: the record app_restore() reads after a warm reset, written in every pass
void app_warm_save (void) {
  vdm::WarmState ws;
  memset(&ws, 0, sizeof ws);
  // a move or calibration in work: the position of its valve is not valid
  const int busy = valve_busy_index();
  for (unsigned int x = 0; x < ACTUATOR_COUNT; x++) {
    vdm::WarmValve &w = ws.valves[x];
    const uint32_t primask = __get_PRIMASK();
    __disable_irq();
    w.actual = myvalvemots[x].actual_position;
    w.target = myvalvemots[x].target_position;
    w.status = myvalvemots[x].status;
    w.flags = (uint8_t) ((busy == (int) x ? 0 : vdm::kWarmPosValid)
      | (myvalves[x].assemblyHold ? vdm::kWarmAssemblyHold : 0)
      | (myvalvemots[x].needsReference ? vdm::kWarmNeedsReference : 0)
      | (myvalvemots[x].recal ? vdm::kWarmRecal : 0));
    __set_PRIMASK(primask);
    const vdm::FaultRetry::Snapshot retry = app_retry[x].snapshot();
    w.retryAttempts = retry.attempts;
    w.retryScheduled = retry.scheduled ? 1 : 0;
    w.retryRemainingS = retry.remainingS;
    ws.failsafePct[x] = app_failsafe[x];
  }
  const vdm::Lease::Snapshot lease = app_lease.snapshot();
  ws.leaseSinceRenewalS = lease.sinceRenewalS;
  ws.leaseSinceClientS = lease.sinceClientS;
  ws.leaseClient = lease.client ? 1 : 0;
  ws.leaseTimeoutMin = app_lease.timeout();
  vdm::warmStateSeal(ws);
  memcpy((void *) &warm_state, &ws, sizeof ws);
}


// called by appsetaction()/appsetservice() with interrupts disabled, in the step that hands the
// command over: a reset from here on must not trust the kept position of the valve
void app_warm_moving (unsigned int valve) {
  if (valve >= ACTUATOR_COUNT || !vdm::warmStateValid(warm_state)) return;
  warm_state.valves[valve].flags &= (uint8_t) ~vdm::kWarmPosValid;
  vdm::warmStateSeal(warm_state);
}


void app_lease_poll (void) {
  app_lease.valvePoll();
}


void app_lease_command (void) {
  app_lease.leaseCommand();
}


void app_lease_heartbeat (bool alive) {
  app_lease.heartbeat(alive);
}


void app_lease_configure (uint16_t minutes) {
  app_lease.setTimeout(minutes);
}


uint8_t app_lease_state (void) {
  return (uint8_t) app_lease.state();
}


uint32_t app_lease_remaining_s (void) {
  return app_lease.remainingS();
}


bool app_lease_client (void) {
  return app_lease.clientPresent();
}


uint16_t app_lease_timeout (void) {
  return app_lease.timeout();
}


uint16_t app_failsafe_mask (void) {
  uint16_t mask = 0;
  for (unsigned int x = 0; x < ACTUATOR_COUNT; x++) {
    if (app_drive(x).source == vdm::DriveSource::LeaseFailsafe) mask = (uint16_t) (mask | (1u << x));
  }
  return mask;
}


void app_set_failsafe (uint16_t valve, uint8_t pct) {
  for (unsigned int x = 0; x < ACTUATOR_COUNT; x++) {
    if (valve == 255 || valve == x) app_failsafe[x] = pct;
  }
}


uint8_t app_failsafe_pct (uint16_t valve) {
  return valve < ACTUATOR_COUNT ? app_failsafe[valve] : vdm::kFailsafeHold;
}


// sstop: stops what runs for the valve (255: whatever runs), cancels the requested calibrations and
// leaves the stopped valve where it is (service hold)
int16_t app_stop (uint16_t valve) {
  if (valve >= ACTUATOR_COUNT && valve != 255) return -1;
  const int16_t stopped = appstop(valve);
  for (unsigned int x = 0; x < ACTUATOR_COUNT; x++) {
    if (valve != 255 && valve != x) continue;
    const uint32_t primask = __get_PRIMASK();
    __disable_irq();
    myvalves[x].forcedLearn = 0;
    myvalves[x].timedLearn = 0;
    myvalves[x].retryLearn = 0;
    myvalves[x].earlyLearn = 0;
    // a running calibration ends through the stop (learn_abort)
    if (!myvalvemots[x].calibActive) {
      myvalvemots[x].calibration = false;
      myvalvemots[x].calibState = calibIdle;
    }
    // PRESENT from such a request; a valve that needs its first calibration stays pending
    if (myvalvemots[x].status == VLV_STATE_PRESENT && myvalvemots[x].calibrated && !myvalvemots[x].recal)
      myvalvemots[x].status = VLV_STATE_IDLE;
    __set_PRIMASK(primask);
  }
  if (valve != 255) myvalves[valve].svcHold = SVMOV_HOLD_10S;
  else if (stopped >= 0) myvalves[stopped].svcHold = SVMOV_HOLD_10S;
  return 0;
}


uint32_t app_get_learntime (void) {
  return learning_time;
}


void app_temp_cycle_done (void) {
  app_temp.cycleDone(millis());
}


uint32_t app_temp_age_s (void) {
  return app_temp.ageS(millis());
}


bool app_protect_suspended (void) {
  return app_guard.suspended();
}


void app_get_valve_v3 (uint16_t valve, struct valve_v3_info &out) {
  if (valve >= ACTUATOR_COUNT) {
    out = valve_v3_info();
    out.fsPct = vdm::kFailsafeHold;
    return;
  }
  struct valve_snapshot snap;
  valve_get_snapshot(valve, snap);
  const vdm::Drive drive = app_drive(valve);
  uint16_t flags = 0;
  if (drive.source == vdm::DriveSource::LeaseFailsafe) flags |= vdm::kVlvFlagFsLease;
  if (drive.source == vdm::DriveSource::BlockedFailsafe) flags |= vdm::kVlvFlagFsBlocked;
  if (!myvalvemots[valve].calibrated) flags |= vdm::kVlvFlagUncalibrated;
  if (myvalvemots[valve].needsReference) flags |= vdm::kVlvFlagNeedsRef;
  if (myvalvemots[valve].recal) flags |= vdm::kVlvFlagRecal;
  if (myvalves[valve].calibRestored) flags |= vdm::kVlvFlagCalRestored;
  if (app_retry[valve].scheduled()) flags |= vdm::kVlvFlagRetry;
  if (snap.diag.earlyRun.count() == 1) flags |= vdm::kVlvFlagEarlyPending;
  if (myvalves[valve].assemblyHold) flags |= vdm::kVlvFlagAssembly;
  if (myvalves[valve].svcHold) flags |= vdm::kVlvFlagSvcHold;
  out.flags = flags;
  out.fault = myvalvemots[valve].faultReason;
  out.fsPct = app_failsafe[valve];
  out.drive = drive.position;
  out.retryS = app_retry[valve].remainingS();
  out.retries = app_retry[valve].attempts();
}
