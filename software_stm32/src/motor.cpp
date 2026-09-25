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
#include <Wire.h>
#include "motor.h"
#include "hardware.h"
#include "STM32TimerInterrupt.h"      // library https://github.com/khoih-prog/STM32_TimerInterrupt
#include "terminal.h"
#include "app.h"
#include "eeprom.h"
#include "owDevices.h"
#include "vdm/end_stop_detector.h"
#include "vdm/presence_test.h"
#include "vdm/protection_guard.h"
#include "vdm/stall_detector.h"
#include "vdm/valve_codes.h"



//#define COMM_DBG				Serial3		// serial port for debugging
#define COMM_DBG				Serial6		// serial port for debugging


#define DIR_CLOSE      (int)  1
#define DIR_OPEN       (int)  0 

#define TIMER0_INTERVAL_MS        1

#define TIMEOUT_NORMALCURRENT    120*100      // 120 seconds timeout with 10 ms cycle time
#define TIMEOUT_OVERCURRENT      1            // cycles of "byte motorcycle (byte valvenr, byte cmd)"
#define TIMEOUT_UNDERCURRENT     4*50           // cycles of "byte motorcycle (byte valvenr, byte cmd)"
#define THRESHOLD_UNDERCURRENT   20           // threshold for detecting undercurrent in 1/10 mA
#define TIMEOUT_TURNON           10           // cycles of motorcycle() to wait for TimerHandler0 (1 ms) to enable the motor
#define TIMEOUT_VALVESTATE       5*60*100     // 5 minutes with 10 ms cycle time, more than the longest valve state (one move <= ~123 s)
#define TIMEOUT_TEMPGAP          300          // longest pause between two calibration strokes for a temperature cycle (3 s)


// commands motor statemachine
#define CMD_M_OPEN      'o'
#define CMD_M_CLOSE     'c'
#define CMD_M_STOP      's'
#define CMD_M_NOTHING   'n'
#define CMD_M_TEST      't'


// return values motor cycle
#define M_RES_INIT        1
#define M_RES_IDLE        2
#define M_RES_OPENS       3
#define M_RES_CLOSES      4
#define M_RES_TURNING     5
#define M_RES_ENDSTOP     6
#define M_RES_STOP        7
#define M_RES_NOCURRENT   9
#define M_RES_TEST        10
#define M_RES_ERROR       11

#define WAIT_TIMER100     2*100
#define WAIT_TIMER50      2*50
#define WAIT_TIMER20      2*20

void set_motor (int smvalvenr, int dir);
void ena_motor (int emvalvenr, int state);
void isr_count ();
void callback_motorstop ();
byte motorcycle (int mvalvenr, byte cmd);
byte appcycle(byte cmd, byte valvenr);

void TimerHandler0();
static void motor_halt ();

#define MUX_SETTLE_TICKS  ((WAIT_MUX + 9) / 10)     // motorcycle() runs every 10 ms

// position arithmetic clamped to 0..100 %
static byte position_add (byte position, byte delta) {
  unsigned int sum = (unsigned int) position + delta;
  return sum > 100 ? 100 : (byte) sum;
}

static byte position_sub (byte position, byte delta) {
  return delta >= position ? 0 : (byte) (position - delta);
}

volatile enum ASTATE valvestate;
volatile uint32_t valve_loop_ticks = 0;
volatile bool valve_loop_stalled = false;
static vdm::StallDetector valve_stall(TIMEOUT_VALVESTATE);

volatile bool temp_refresh_request = false;
volatile bool protect_suspended = false;
volatile bool protect_enforce = vdm::kProtectEnforce;
// sstop of the valve in work; cleared when the valve state machine is back in A_IDLE
static volatile bool stop_request = false;

// Init STM32 timer TIM1
STM32Timer ITimer0(TIM1);

//int counter;
//unsigned long time;
volatile int current_mA = 0;                     // current valve motor in 1/10 mA for normal mode

volatile int analog_current = 0;                 // current valve motor in 1/10 mA for test mode
volatile int analog_current_old = 0;             // filter memory



// unsigned int idlecurrent = 2048;        // idle current adc value (digits)
// unsigned int idlecurrent_old = 2048;    // filter

volatile unsigned int isr_counter = 0;      // ISR var for counting revolutions
volatile byte         isr_turning = 0;      // ISR var for state of motor
volatile unsigned int isr_target = 0;       // ISR var for valve target count
volatile int          isr_valvenr = 0;      // ISR var for valve nr
volatile byte         isr_timer_go = 0;     // ISR var for timer pwm start
volatile byte         isr_timer_fin = 0;    // ISR var for timer pwm finished

volatile byte         isr_overcurrentevent = 0;
volatile byte         isr_stop_request = 0;     // set by EXTI when the target count is reached

volatile valvemotor myvalvemots[ACTUATOR_COUNT];


// handoff main loop -> valve_loop (TIM2), written by appsetaction() with interrupts disabled
volatile char command = '\0';
volatile int valvenr = 0;
volatile byte poschangecmd = 0;
volatile uint8_t moveflagscmd = 0;       // MOVE_KEEP_STATUS, MOVE_REFERENCE
volatile uint8_t svc_dir = 0;             // service move parameters, see appsetservice()
volatile uint16_t svc_counts = 0;
volatile uint8_t svc_maxmA = 0;

uint8_t currentbound_low_fac = 17;        // lower current limit factor for detection of end stop
uint8_t currentbound_high_fac = 17;       // upper current limit factor for detection of end stop

uint8_t startOnPower = 50;
uint16_t noOfMinCounts = NO_OF_MIN_COUNTS;
uint8_t maxCalibRetries = 0;
uint8_t calibRetries = 0;

// written by the main loop with interrupts disabled, read by valve_loop (TIM2)
static vdm::EscalationConfig calib_escalation = vdm::kEscalationDefault;

//volatile uint32_t revcounter;

static int undercurrcnt = 0;
static int overcurrcnt = 0;

// end-stop detection, fed by TimerHandler0 (TIM1); armed and read by the motor state machine
// (TIM2). Both interrupts have the same priority and cannot preempt each other.
static vdm::EndStopDetector endstop;

// the move in progress (TIM2 context)
#define MOVE_NORMAL   0           // position change requested by app_loop (counts for early stops)
#define MOVE_LEARN    1           // calibration stroke
#define MOVE_SERVICE  2           // service move (svmov)

static vdm::MoveRequest move_req;
static uint8_t move_kind = MOVE_NORMAL;
static int32_t move_bound_low = 0;        // end-stop bounds for the next motor start, 1/10 mA
static int32_t move_bound_high = 0;
static uint32_t move_start_ms = 0;
static vdm::MotorStop motor_stop_cause = vdm::MotorStop::None;
static int last_turning_current = 0;      // current_mA at the last M_TURNING tick
static uint16_t stroke_mean_mA = 0;       // mean current of the last stroke that ended at an end stop
static uint16_t stroke_mean_samples = 0;
static vdm::ProfileRecorder move_profile;
static uint8_t move_flags = 0;            // flags of the normal move in progress
static byte keep_status = 0;              // status a MOVE_KEEP_STATUS move restores
static byte pos_change = 0;               // requested change of the normal move in %, 255: to the end stop
static int waittimer = 0;
static vdm::PresenceTest test_presence;   // M_TEST

#ifdef inrushDebug
// first 50 ms of every motor start (TimerHandler0): peak raw current and samples above the inrush limit
static volatile uint16_t inrush_samples = 0;
static volatile int32_t inrush_peak = 0;
static volatile uint16_t inrush_over = 0;
#endif

// per valve diagnostics; written by valve_loop (TIM2), read via valve_get_snapshot()/valve_get_profile()
struct valve_record {
  struct valve_diag diag;
  vdm::ProfileRecorder profile;
};
static struct valve_record valve_records[ACTUATOR_COUNT];

// drives the valve PSU and motor enable outputs to their inactive level;
// called right after reset, before the 3 s boot window
void valve_pins_safe () {
  // preset the output latch first: switching to open drain with the reset value LOW
  // would enable the valve PSU for a moment
  set_GPIO_Port_Clock(STM_PORT(digitalPinToPinName(POWER_ENA)));
  PSU_OFF();
  pinMode(POWER_ENA, OUTPUT_OPEN_DRAIN);
  PSU_OFF();

  // L293 enable inputs; PA15/PB3 are JTAG pins with pull resistors after reset
  pinMode(CTRL_ENA0, OUTPUT);
  pinMode(CTRL_ENA1, OUTPUT);
  pinMode(CTRL_ENA2, OUTPUT);
  pinMode(CTRL_ENA3, OUTPUT);
  pinMode(CTRL_ENA4, OUTPUT);
  pinMode(CTRL_ENA5, OUTPUT);
  ena_motor(0, 0);
}


// call from setup function in main, after app_setup() loaded the motor parameters
byte valve_setup () {

  // valve MUX relay
  pinMode(CTRL_MUX, OUTPUT);

  // L293 enable pins
  pinMode(CTRL_ENA0, OUTPUT);
  pinMode(CTRL_ENA1, OUTPUT);
  pinMode(CTRL_ENA2, OUTPUT);
  pinMode(CTRL_ENA3, OUTPUT);
  pinMode(CTRL_ENA4, OUTPUT);
  pinMode(CTRL_ENA5, OUTPUT);

  // L293 direction pin
  pinMode(CTRL_DIRECTION, OUTPUT); 

  // input for counting revs
  pinMode(REVINPIN, INPUT);

  // external interrupt for counting revs
  //attachInterrupt(digitalPinToInterrupt(REVINPIN),isr_counter,RISING);

  #ifdef motDebug
    COMM_DBG.print("Current end stop factor low: ");
    COMM_DBG.println((float) (currentbound_low_fac)/10,DEC);
    COMM_DBG.print("Current end stop factor high: ");
    COMM_DBG.println((float) (currentbound_high_fac)/10,DEC);
  #endif

// init valve data
  for (unsigned int x = 0;x<ACTUATOR_COUNT;x++) {
    myvalvemots[x].actual_position = startOnPower;
    myvalvemots[x].target_position = startOnPower;
    myvalvemots[x].meancurrent = vdm::kMeanCurrentDefault_mA;
    myvalvemots[x].scaler = 89;
    myvalvemots[x].calibRetries = 0;
    myvalvemots[x].calibActive = 0;
    myvalvemots[x].calibrated = 0;
    myvalvemots[x].recal = 0;
    myvalvemots[x].needsReference = 0;
    myvalvemots[x].calibSeq = 0;
    myvalvemots[x].calibFailed = 0;
    myvalvemots[x].earlyLearnDue = 0;
    myvalvemots[x].faultReason = (uint8_t) vdm::ValveFault::None;
    myvalvemots[x].moveSeq = 0;
    myvalvemots[x].tripSeq = 0;
  }

  valvestate = A_INIT;

  // Interval in microsecs
  if (ITimer0.attachInterruptInterval(TIMER0_INTERVAL_MS * 1000, TimerHandler0))
  {
    #ifdef motDebug 
        COMM_DBG.print(F("Starting ITimer0 OK, millis() = ")); COMM_DBG.println(millis());
    #endif
  }
  else {
    #ifdef motDebug
      COMM_DBG.println(F("Can't set ITimer0. Select another freq. or timer"));
    #endif
  }

 

  return 0;
}


vdm::MotorParams motor_get_params () {
  vdm::MotorParams p;
  p.lowFac = currentbound_low_fac;
  p.highFac = currentbound_high_fac;
  p.startOnPower = startOnPower;
  p.minCounts = noOfMinCounts;
  p.maxRetries = maxCalibRetries;
  return p;
}


// takes validated parameters into RAM and the EEPROM mirror (the caller decides about writing)
void motor_set_params (const vdm::MotorParams &params) {
  currentbound_low_fac = params.lowFac;
  currentbound_high_fac = params.highFac;
  startOnPower = params.startOnPower;
  noOfMinCounts = params.minCounts;
  maxCalibRetries = params.maxRetries;

  eep_content.currentbound_low_fac = params.lowFac;
  eep_content.currentbound_high_fac = params.highFac;
  eep_content.startOnPower = params.startOnPower;
  eep_content.noOfMinCounts = params.minCounts;
  eep_content.maxCalibRetries = params.maxRetries;
}


vdm::EscalationConfig motor_get_escalation () {
  return calib_escalation;
}


// takes a validated configuration into RAM and the EEPROM mirror
void motor_set_escalation (const vdm::EscalationConfig &config) {
  const uint32_t primask = __get_PRIMASK();
  __disable_irq();
  calib_escalation = config;
  __set_PRIMASK(primask);
  eep_content.escalation = config;
}


void valve_get_snapshot (unsigned int valveindex, struct valve_snapshot &out) {
  if (valveindex >= ACTUATOR_COUNT) {
    out = valve_snapshot();
    return;
  }
  volatile valvemotor &mot = myvalvemots[valveindex];
  const uint32_t primask = __get_PRIMASK();
  __disable_irq();
  out.diag = valve_records[valveindex].diag;
  out.opening_count = mot.opening_count;
  out.closing_count = mot.closing_count;
  out.deadzone_count = mot.deadzone_count;
  out.meancurrent = mot.meancurrent;
  out.movements = myvalves[valveindex].movements;
  out.status = mot.status;
  out.actual_position = mot.actual_position;
  out.target_position = mot.target_position;
  out.calibration = mot.calibration;
  out.calibRetries = mot.calibRetries;
  out.calibActive = mot.calibActive;
  __set_PRIMASK(primask);
}


void valve_get_profile (unsigned int valveindex, vdm::ProfileRecorder &out) {
  if (valveindex >= ACTUATOR_COUNT) {
    out.reset();
    return;
  }
  const uint32_t primask = __get_PRIMASK();
  __disable_irq();
  out = valve_records[valveindex].profile;
  __set_PRIMASK(primask);
}


// full stroke of the last successful calibration in the direction of the move, 0 if unknown
static uint32_t learned_travel (int v, uint8_t dir) {
  const uint32_t travel = dir == DIR_OPEN ? myvalvemots[v].opening_count : myvalvemots[v].closing_count;
  return (myvalvemots[v].scaler > 0 && travel >= vdm::kMinTravelCounts) ? travel : 0;
}


// end-stop bounds from the learned mean current (S01: floor of at least 15 mA), optionally escalated
static void set_move_bounds (int v, uint16_t floor_mA, uint8_t repetition) {
  const uint16_t mean = (uint16_t) (myvalvemots[v].meancurrent > 0xFFFF ? 0xFFFF : myvalvemots[v].meancurrent);
  move_bound_high = vdm::escalatedBound(vdm::endStopBound(mean, currentbound_high_fac, floor_mA), repetition, calib_escalation);
  move_bound_low = -vdm::escalatedBound(vdm::endStopBound(mean, currentbound_low_fac, floor_mA), repetition, calib_escalation);
}


// prepares a position change by `change` % (255: to the end stop)
static void prepare_normal_move (int v, uint8_t dir, byte change) {
  uint16_t requested = vdm::kRunToEndStop;
  uint8_t expected = 0;

  if (change == 255) {
    // no early check when the start is not known (reference move) or the valve is failed or blocked
    if ((move_flags & (MOVE_REFERENCE | MOVE_KEEP_STATUS)) == 0) {
      const byte actual = myvalvemots[v].actual_position;
      expected = dir == DIR_OPEN ? (byte) (100 - (actual > 100 ? 100 : actual)) : actual;
    }
  }
  else {
    const uint32_t counts = (uint32_t) myvalvemots[v].scaler * change;
    requested = (uint16_t) (counts < vdm::kRunToEndStop ? counts : vdm::kRunToEndStop - 1);
  }

  isr_target = requested;
  move_req.dir = dir;
  move_req.requestedCounts = requested;
  move_req.expectedTravelPct = expected;
  move_req.learnedTravel = learned_travel(v, dir);
  move_req.partialEarlyCheck = change != 255 && (move_flags & MOVE_KEEP_STATUS) == 0;
  move_kind = MOVE_NORMAL;
  set_move_bounds(v, vdm::kMeanCurrentFloor_mA, 0);
}


// prepares a calibration stroke to the end stop; `full` if it starts at the opposite end stop
static void prepare_learn_stroke (int v, uint8_t dir, bool full) {
  isr_target = vdm::kRunToEndStop;       // max value to disable stopping
  move_req.dir = dir;
  move_req.requestedCounts = vdm::kRunToEndStop;
  move_req.expectedTravelPct = full ? 100 : 0;
  move_req.learnedTravel = learned_travel(v, dir);
  move_req.partialEarlyCheck = false;
  move_kind = MOVE_LEARN;
  set_move_bounds(v, vdm::calibrationFloor(dir), calibRetries);
}


// prepares a service move: exact pulse count, fixed threshold of maxmA
static void prepare_service_move (int v, uint8_t dir, uint16_t counts, uint8_t maxmA) {
  isr_target = counts - 1;               // the motor stops on pulse isr_target + 1
  move_req.dir = dir;
  move_req.requestedCounts = counts;
  move_req.expectedTravelPct = 0;
  move_req.learnedTravel = learned_travel(v, dir);
  move_req.partialEarlyCheck = false;
  move_kind = MOVE_SERVICE;
  move_bound_high = (int32_t) maxmA * 10;
  move_bound_low = -move_bound_high;
}


// records the move that just ended (S05)
static void finish_move (int v) {
  const uint32_t counted = isr_counter;
  const vdm::MoveClassification c = vdm::classifyMove(move_req, motor_stop_cause, counted);
  struct valve_record &rec = valve_records[v];

  move_profile.finish(counted, endstop.trip() != vdm::EndStopDetector::Trip::None ? endstop.tripCurrent() : last_turning_current);
  rec.profile = move_profile;
  rec.diag.last = vdm::makeMoveResult(move_req, c.reason, counted, endstop.peak(), millis() - move_start_ms);
  rec.diag.lastEarly = c.early;
  if (c.early && move_kind == MOVE_NORMAL) {
    if (rec.diag.earlyStops < 0xFFFF) rec.diag.earlyStops++;
    rec.diag.earlyWarn = true;
  }
  if (move_kind == MOVE_NORMAL) {
    // the second early partial stop in a row requests a calibration; the moves of a failed or
    // blocked valve never do (its calibrations come from staln and the automatic retry only)
    if ((move_flags & MOVE_KEEP_STATUS) == 0 && rec.diag.earlyRun.onMove(c.early && move_req.partialEarlyCheck))
      myvalvemots[v].earlyLearnDue = 1;
    myvalvemots[v].moveSeq++;
  }
  // the inrush limit was exceeded (reported, or the move was stopped by it)
  if (endstop.inrushSeen()) {
    myvalvemots[v].faultReason = (uint8_t) vdm::ValveFault::InrushTrip;
    myvalvemots[v].tripSeq++;
  }
  #ifdef inrushDebug
    COMM_DBG.print("inrush: valve "); COMM_DBG.print(v); COMM_DBG.print(" dir "); COMM_DBG.print(move_req.dir);
    COMM_DBG.print(" peak "); COMM_DBG.print(inrush_peak); COMM_DBG.print(" over "); COMM_DBG.println(inrush_over);
  #endif
}


// records a move the motor state machine did not start
static void record_refused_move (int v) {
  struct valve_record &rec = valve_records[v];
  rec.profile.reset();
  rec.diag.last = vdm::makeMoveResult(move_req, vdm::StopReason::Aborted, 0, 0, 0);
}


// every way out of a calibration: clears the request (calibration flag, gvlvd bit 7) and its state
static void learn_end (int v, bool success) {
  myvalvemots[v].calibration = false;
  myvalvemots[v].calibState = calibIdle;
  myvalvemots[v].calibActive = 0;
  valve_records[v].diag.lastCalFailed = !success;
  if (success) valve_records[v].diag.earlyWarn = false;
}


// a calibration stopped by sstop: not a failed calibration; the position is lost
static void learn_abort (int v) {
  myvalvemots[v].calibration = false;
  myvalvemots[v].calibState = calibIdle;
  myvalvemots[v].calibActive = 0;
  myvalvemots[v].status = VLV_STATE_IDLE;
  myvalvemots[v].needsReference = 1;
}


// the valve lost its contact: no current in a move, a calibration stroke or a service move
static void lost_contact (int v) {
  myvalvemots[v].status = VLV_STATE_OPENCIR;
  myvalvemots[v].recal = 1;
}


// a failed calibration stroke (timeout, no end stop, or the inrush limit stopped it)
static void learn_failed (int v, vdm::ValveFault fault) {
  learn_end(v, false);
  myvalvemots[v].status = VLV_STATE_FAILED;
  myvalvemots[v].faultReason = (uint8_t) fault;
}


// status after a normal move that did not fail
static byte move_status () {
  return (move_flags & MOVE_KEEP_STATUS) ? keep_status : VLV_STATE_IDLE;
}


// position after a service move: counted pulses converted with the learned scaler
static void service_move_position (int v, uint32_t counted) {
  const unsigned int scaler = myvalvemots[v].scaler;
  if (scaler == 0) return;
  const uint32_t pct = counted / scaler;
  const byte delta = (byte) (pct > 100 ? 100 : pct);
  if (move_req.dir == DIR_OPEN) myvalvemots[v].actual_position = position_add(myvalvemots[v].actual_position, delta);
  else myvalvemots[v].actual_position = position_sub(myvalvemots[v].actual_position, delta);
}


// end of a normal move (also to an end stop, the failsafe move of a blocked valve and a reference
// move); false while the motor runs
static bool end_normal_move (int v, byte result) {
  if (result == M_RES_STOP) {
    finish_move(v);
    // sstop: the position follows the pulses that really turned
    if (motor_stop_cause == vdm::MotorStop::Aborted) service_move_position(v, isr_counter);
    else if (move_req.dir == DIR_OPEN) myvalvemots[v].actual_position = position_add(myvalvemots[v].actual_position, pos_change);
    else myvalvemots[v].actual_position = position_sub(myvalvemots[v].actual_position, pos_change);
    myvalvemots[v].status = move_status();
  }
  else if (result == M_RES_NOCURRENT) {
    #ifdef motDebug
      COMM_DBG.println("A: undercurrent");
    #endif
    finish_move(v);
    lost_contact(v);
    myvalvemots[v].actual_position = myvalvemots[v].target_position;
    isr_counter = 0;
  }
  else if (result == M_RES_ENDSTOP) {
    finish_move(v);
    // the inrush limit stopped the motor at its start: the position did not change
    if (endstop.inrushTrip()) myvalvemots[v].status = VLV_STATE_FAILED;
    else {
      // a partial move takes the position from the counted pulses, a move to the end stop 0 / 100 %
      myvalvemots[v].actual_position = vdm::positionAfterEndStop(myvalvemots[v].actual_position, move_req.dir,
        move_req.requestedCounts, isr_counter, myvalvemots[v].scaler);
      if (pos_change == 255) myvalvemots[v].needsReference = 0;
      myvalvemots[v].status = move_status();
    }
  }
  else if (result == M_RES_ERROR) {
    #ifdef motDebug
      COMM_DBG.println("A: move failed, timeout");
    #endif
    // the position is unknown: it stays as it was, app_loop reports the target as rejected (S04)
    finish_move(v);
    myvalvemots[v].status = VLV_STATE_FAILED;
    myvalvemots[v].faultReason = (uint8_t) vdm::ValveFault::MoveTimeout;
  }
  else return false;
  #ifdef motDebug
    COMM_DBG.print("A: new position "); COMM_DBG.println(myvalvemots[v].actual_position);
  #endif
  return true;
}


// starts a full calibration stroke (the valve stands at the opposite end stop)
static void start_learn_stroke (int v, uint8_t dir) {
  isr_counter = 0;
  myvalvemots[v].status = dir == DIR_OPEN ? VLV_STATE_OPENING : VLV_STATE_CLOSING;
  prepare_learn_stroke(v, dir, true);
  motorcycle(v, dir == DIR_OPEN ? CMD_M_OPEN : CMD_M_CLOSE);
  waittimer = WAIT_TIMER20;
}


// the valve state machine went back to A_IDLE without starting the motor (sstop)
static void refuse_move (int v) {
  record_refused_move(v);
  valvestate = A_IDLE;
}


void valve_loop () {
  byte temp = 0;

  static unsigned int closing_count = 0;
  static unsigned int opening_count = 0;
  static uint16_t open_mean_mA = 0;
  static uint16_t open_mean_samples = 0;

  static int valveindex = 0;

  static int psuofftimer = 0;
  static int locktimer = 0;
  static int gaptimer = 0;
  static uint8_t gap_dir = DIR_OPEN;          // stroke after the temperature gap
  static enum ASTATE gap_next = A_LEARN3;

  static uint8_t svc_move_dir = 0;
  static uint16_t svc_move_counts = 0;
  static uint8_t svc_move_maxmA = 0;
  static byte svc_prev_status = 0;

  valve_loop_ticks++;

  if(waittimer) waittimer--;

  // the motor state machine gets a stop command while an operator stop (sstop) is pending
  const byte runcmd = stop_request ? CMD_M_STOP : 0;

  switch (valvestate) {
    case A_INIT:
                  if ( motorcycle(0, CMD_M_NOTHING) == M_RES_IDLE ) {
                     #ifdef motDebug
                        COMM_DBG.println("A: init ready");
                     #endif
                     valvestate = A_IDLE;
                     valveindex = 255;
                  }
                  break;

    case A_IDLE:
                  stop_request = false;
                  valveindex = valvenr;
                  move_flags = 0;
                  if (command == CMD_A_OPEN || command == CMD_A_CLOSE || command == CMD_A_OPEN_END || command == CMD_A_CLOSE_END) {
                    #ifdef motDebug
                      COMM_DBG.print("A: cmd "); COMM_DBG.print(command);
                      COMM_DBG.print(" for valve "); COMM_DBG.println(valveindex, 10);
                    #endif
                    valvestate = (command == CMD_A_OPEN || command == CMD_A_OPEN_END) ? A_OPEN1 : A_CLOSE1;
                    PSU_ON();
                    waittimer = WAIT_TIMER50;
                    psuofftimer = 0;
                    pos_change = (command == CMD_A_OPEN || command == CMD_A_CLOSE) ? poschangecmd : 255;
                    move_flags = moveflagscmd;
                    keep_status = myvalvemots[valveindex].status;
                  }
                  else if (command == CMD_A_LEARN) {
                    #ifdef motDebug
                      COMM_DBG.print("A: cmd learn for valve ");
                      COMM_DBG.println(valveindex, 10);
                    #endif
                    valvestate = A_LEARN1;
                    PSU_ON();
                    psuofftimer = 0;
                    waittimer = WAIT_TIMER50;
                    calibRetries = 0;
                    myvalvemots[valveindex].calibRetries=0;
                    myvalvemots[valveindex].calibActive = 1;
                  }
                  else if (command == CMD_A_TEST) {
                    #ifdef motDebug
                      COMM_DBG.print("A: cmd test valve ");
                      COMM_DBG.println(valveindex, 10);
                    #endif
                    valvestate = A_TEST;
                    PSU_ON();
                    waittimer = WAIT_TIMER100;
                    psuofftimer = 0;
                  }
                  else if (command == CMD_A_SERVICE) {
                    #ifdef motDebug
                      COMM_DBG.print("A: cmd service move for valve ");
                      COMM_DBG.println(valveindex, 10);
                    #endif
                    valvestate = A_SVC1;
                    PSU_ON();
                    waittimer = WAIT_TIMER50;
                    psuofftimer = 0;
                    svc_move_dir = svc_dir;
                    svc_move_counts = svc_counts;
                    svc_move_maxmA = svc_maxmA;
                  }
                  else {
                    valvestate = A_IDLE;

                    // switch off PSU after some inactive (idle) time
                    psuofftimer++;
                    if(psuofftimer > 1500) {      // 15 s
                      psuofftimer = 0;
                      PSU_OFF();
                      MUX_OFF();
                    }
                  }

                  // decrement learning counter
                  if (!myvalvemots[valveindex].calibration) {
                    if(valvestate == A_OPEN1 || valvestate == A_CLOSE1) {
                      if(myvalves[valveindex].learn_movements) {
                        myvalves[valveindex].learn_movements--;
                      }
                      myvalves[valveindex].movements++;
                    }
                  }

                  // pause temperature measurement to avoid ADC interference
                  if(valvestate != A_IDLE) {
                    temp_command(TEMP_CMD_LOCK);
                    locktimer = 0;
                  }
                  else {
                    if (locktimer<50) locktimer++;
                    else temp_command(TEMP_CMD_UNLOCK);
                  }

                  command = '\0';       // clear command for next loop call, prevents reevaluating
                  break;

    case A_OPEN1:  // start valve opening
    case A_CLOSE1: // start valve closing
                  {
                    const bool open = valvestate == A_OPEN1;
                    if (stop_request) {
                      prepare_normal_move(valveindex, open ? DIR_OPEN : DIR_CLOSE, pos_change);
                      refuse_move(valveindex);
                    }
                    else if (!waittimer) {
                      prepare_normal_move(valveindex, open ? DIR_OPEN : DIR_CLOSE, pos_change);
                      if (motorcycle (valveindex, open ? CMD_M_OPEN : CMD_M_CLOSE) == (open ? M_RES_OPENS : M_RES_CLOSES)) {
                        #ifdef motDebug
                          COMM_DBG.print("A: begin move by ");
                          COMM_DBG.println(pos_change);
                        #endif
                        myvalvemots[valveindex].status = open ? VLV_STATE_OPENING : VLV_STATE_CLOSING;
                        valvestate = open ? A_OPEN2 : A_CLOSE2;
                        isr_counter=0;
                      }
                      else {
                        #ifdef motDebug
                          COMM_DBG.print("A: cant move valve");
                        #endif
                        refuse_move(valveindex);
                      }
                    }
                  }
                  break;

    case A_OPEN2:  // wait for the end of the move
    case A_CLOSE2:
                  if (end_normal_move(valveindex, motorcycle (valveindex, runcmd))) valvestate = A_IDLE;
                  break;

    case A_LEARN1:  // prepare learning
                  #ifdef motDebug
                    COMM_DBG.println("A: try learning valve");
                  #endif
                  if (stop_request) {
                    learn_abort(valveindex);
                    refuse_move(valveindex);
                    break;
                  }
                  // first: closing completely
                  undercurrcnt = 0;
                  overcurrcnt = 0;
                  prepare_learn_stroke(valveindex, DIR_CLOSE, false);
                  motorcycle (valveindex, CMD_M_CLOSE);
                  myvalvemots[valveindex].status = VLV_STATE_CLOSING;
                  valvestate = A_LEARN2;
                  waittimer = WAIT_TIMER20;
                  break;

    case A_LEARN2:  // goto start position
    case A_LEARN3:  // learning opening way
    case A_LEARN4:  // learning closing way
                  if (!waittimer) {
                    temp = motorcycle (valveindex, runcmd);
                    if (temp == M_RES_STOP && motor_stop_cause == vdm::MotorStop::Aborted && stop_request) {
                      // sstop: the calibration ends without a result, the position is lost
                      finish_move(valveindex);
                      learn_abort(valveindex);
                      valvestate = A_IDLE;
                    }
                    else if (temp == M_RES_ENDSTOP && endstop.inrushTrip()) {
                      // never calibration evidence: no counts, no BLOCKS
                      finish_move(valveindex);
                      learn_failed(valveindex, vdm::ValveFault::InrushTrip);
                      valvestate = A_IDLE;
                    }
                    else if (temp == M_RES_ENDSTOP && valvestate == A_LEARN2) {
                      #ifdef motDebug
                        COMM_DBG.println("A: closed valve before learning, now opening");
                      #endif
                      finish_move(valveindex);
                      // second: opening completely and count rotations
                      gap_dir = DIR_OPEN;
                      gap_next = A_LEARN3;
                    }
                    else if (temp == M_RES_ENDSTOP && valvestate == A_LEARN3) {
                      #ifdef motDebug
                        COMM_DBG.println("A: opened valve for learning, now closing again");
                      #endif
                      finish_move(valveindex);
                      opening_count = isr_counter;
                      // kept until the pass is accepted (S02)
                      open_mean_mA = stroke_mean_mA;
                      open_mean_samples = stroke_mean_samples;
                      // third: closing completely and count rotations, threshold from the learned mean current
                      // but not below the 1.x closing threshold (S01)
                      gap_dir = DIR_CLOSE;
                      gap_next = A_LEARN4;
                    }
                    else if (temp == M_RES_ENDSTOP) {
                      #ifdef motDebug
                        COMM_DBG.println("A: closed valve for learning");
                      #endif
                      finish_move(valveindex);
                      closing_count = isr_counter;
                      myvalvemots[valveindex].actual_position = 0;    // because valve was closed completely

                      const vdm::CalibrationVerdict verdict =
                        vdm::evaluateCalibration(opening_count, closing_count, noOfMinCounts, calibRetries, maxCalibRetries);
                      #ifdef motDebug
                        COMM_DBG.print("A: counts open/close/min = "); COMM_DBG.print(opening_count);
                        COMM_DBG.print(" "); COMM_DBG.print(closing_count);
                        COMM_DBG.print(" "); COMM_DBG.println(noOfMinCounts);
                      #endif

                      if (verdict == vdm::CalibrationVerdict::Accept) {
                        // only a successful pass changes the learned values (S02, S03)
                        myvalvemots[valveindex].meancurrent = vdm::learnMeanCurrent((uint16_t) myvalvemots[valveindex].meancurrent,
                          open_mean_mA, open_mean_samples, stroke_mean_mA, stroke_mean_samples);
                        myvalvemots[valveindex].closing_count = closing_count;
                        myvalvemots[valveindex].opening_count = opening_count;
                        myvalvemots[valveindex].deadzone_count = (int) closing_count - (int) opening_count;
                        myvalvemots[valveindex].scaler = opening_count / 100;
                        myvalvemots[valveindex].status = VLV_STATE_IDLE;
                        myvalvemots[valveindex].calibrated = 1;
                        myvalvemots[valveindex].recal = 0;
                        myvalvemots[valveindex].needsReference = 0;
                        myvalvemots[valveindex].calibFailed = 0;
                        myvalvemots[valveindex].faultReason = (uint8_t) vdm::ValveFault::None;
                        myvalvemots[valveindex].calibSeq++;
                        valve_records[valveindex].diag.lastCalFailed = false;
                        valve_records[valveindex].diag.earlyWarn = false;
                        valve_records[valveindex].diag.earlyRun.reset();
                        // the movement trigger counts from this calibration (S14)
                        myvalves[valveindex].movements = 0;
                        myvalves[valveindex].learn_movements = learning_movements;
                        #ifdef motDebug
                          COMM_DBG.print("A: learned scaler = "); COMM_DBG.println(myvalvemots[valveindex].scaler);
                          COMM_DBG.print("A: learned mean current = "); COMM_DBG.println(myvalvemots[valveindex].meancurrent);
                        #endif
                        valvestate = A_SET;
                      }
                      else {
                        calibRetries++;
                        if (myvalvemots[valveindex].calibRetries < 255) myvalvemots[valveindex].calibRetries++;

                        if (verdict == vdm::CalibrationVerdict::Retry) {
                          valvestate = A_LEARN1;
                          PSU_ON();
                          psuofftimer = 0;
                          waittimer = WAIT_TIMER50;
                          #ifdef motDebug
                            COMM_DBG.print("A: calibration retry  = "); COMM_DBG.println(calibRetries);
                          #endif
                        }
                        else {
                          // BLOCKS sticks: no positioning with the counts of a failed pass (S03)
                          myvalvemots[valveindex].status = VLV_STATE_BLOCKS;
                          myvalvemots[valveindex].faultReason = (uint8_t) vdm::ValveFault::StrokesTooShort;
                          myvalvemots[valveindex].calibFailed = 1;
                          myvalvemots[valveindex].calibSeq++;
                          learn_end(valveindex, false);
                          valveindex = 255;
                          valvestate = A_IDLE;
                          #ifdef motDebug
                            COMM_DBG.println("A: calibration failed, valve blocked");
                          #endif
                        }
                      }
                    }
                    else if (temp == M_RES_NOCURRENT) {
                      #ifdef motDebug
                        COMM_DBG.println("A: (A_LEARN) undercurrent");
                      #endif
                      finish_move(valveindex);
                      lost_contact(valveindex);
                      // the target stays: the valve goes there once it is found again (S2)
                      myvalvemots[valveindex].actual_position = myvalvemots[valveindex].target_position;
                      learn_end(valveindex, false);
                      valvestate = A_IDLE;
                      isr_counter=0;
                    }
                    // stop: the counter ran out (isr_target 65535) without an end stop, idle: motor machine not running
                    else if (temp == M_RES_ERROR || temp == M_RES_STOP || temp == M_RES_IDLE) {
                      #ifdef motDebug
                        COMM_DBG.println("A: calibration stroke failed, timeout");
                      #endif
                      if (temp != M_RES_IDLE) finish_move(valveindex);
                      learn_failed(valveindex, vdm::ValveFault::StrokeTimeout);
                      valvestate = A_IDLE;
                    }

                    // between two strokes (motor stopped at an end stop): a due temperature cycle first (S1)
                    if (valvestate == A_LEARN2 || valvestate == A_LEARN3) {
                      if (temp == M_RES_ENDSTOP) {
                        if (temp_refresh_request) {
                          temp_command(TEMP_CMD_UNLOCK);
                          gaptimer = 0;
                          valvestate = A_GAP;
                        }
                        else {
                          start_learn_stroke(valveindex, gap_dir);
                          valvestate = gap_next;
                        }
                      }
                    }
                  }
                  break;

    case A_GAP:   // pause between two calibration strokes for a temperature cycle
                  if (stop_request) {
                    temp_command(TEMP_CMD_LOCK);
                    learn_abort(valveindex);
                    valvestate = A_IDLE;
                  }
                  else if (!temp_refresh_request || ++gaptimer >= TIMEOUT_TEMPGAP) {
                    temp_command(TEMP_CMD_LOCK);
                    start_learn_stroke(valveindex, gap_dir);
                    valvestate = gap_next;
                  }
                  break;

     case A_SET:  // set to previous %
                  valveindex = valvenr;
                  #ifdef motDebug
                      COMM_DBG.print("A_SET: set position "); COMM_DBG.println(myvalvemots[valveindex].target_position);
                  #endif
                  myvalvemots[valveindex].actual_position=0;
                  if (myvalvemots[valveindex].target_position==0) {
                      learn_end(valveindex, true);
                      valveindex = 255;
                      valvestate = A_IDLE;
                  } else {
                    valvestate = A_SET1;
                    PSU_ON();
                    waittimer = WAIT_TIMER50;
                    psuofftimer = 0;
                    pos_change = myvalvemots[valveindex].target_position;
                  }
                  break;

     case A_SET1:  // start valve opening
                  if (stop_request) {
                    prepare_normal_move(valveindex, DIR_OPEN, pos_change);
                    learn_end(valveindex, true);
                    refuse_move(valveindex);
                  }
                  else if (!waittimer) {
                    prepare_normal_move(valveindex, DIR_OPEN, pos_change);

                    if (motorcycle (valveindex, CMD_M_OPEN) == M_RES_OPENS) {
                      #ifdef motDebug
                        COMM_DBG.print("A_SET1: begin opening by ");
                        COMM_DBG.println(pos_change);
                      #endif
                      myvalvemots[valveindex].status = VLV_STATE_OPENING;
                      valvestate = A_SET2;
                      isr_counter=0;
                    }
                    else {
                      #ifdef motDebug
                        COMM_DBG.print("A_SET1: cant open valve");
                      #endif
                      record_refused_move(valveindex);
                      learn_end(valveindex, true);
                      valvestate = A_IDLE;
                    }
                  }
                  break;

    case A_SET2:  // wait for finish opening: the calibration already succeeded
                  if (end_normal_move(valveindex, motorcycle (valveindex, runcmd))) {
                    learn_end(valveindex, true);
                    valvestate = A_IDLE;
                  }
                  break;

     case A_TEST:  // test if a valve is connected
                  if (!waittimer) {

                    temp = motorcycle (valveindex, CMD_M_TEST);
                    if (temp != M_RES_TEST) {
                      const vdm::PresenceTest::Result result = temp == M_RES_NOCURRENT ? vdm::PresenceTest::Result::Absent
                        : temp == M_RES_ERROR ? vdm::PresenceTest::Result::Short : vdm::PresenceTest::Result::Present;
                      const vdm::PresenceOutcome outcome = vdm::presenceOutcome(result, myvalvemots[valveindex].calibrated,
                                                                                myvalvemots[valveindex].recal);
                      #ifdef motDebug
                        COMM_DBG.print("A: (A_TEST) test valve ");
                        COMM_DBG.print(valveindex, DEC);
                        COMM_DBG.print(" --> status ");
                        COMM_DBG.println(outcome.status, DEC);
                      #endif
                      myvalvemots[valveindex].status = outcome.status;
                      myvalvemots[valveindex].faultReason = outcome.fault;
                      if (outcome.needsReference) myvalvemots[valveindex].needsReference = 1;
                      // no contact: the valve is tested again at its next target change (S2)
                      if (result == vdm::PresenceTest::Result::Absent)
                        myvalvemots[valveindex].actual_position = myvalvemots[valveindex].target_position;
                      // a short (enforced or only reported)
                      if (test_presence.shortSeen()) {
                        myvalvemots[valveindex].faultReason = (uint8_t) vdm::ValveFault::Short;
                        myvalvemots[valveindex].tripSeq++;
                      }
                      valvestate = A_IDLE;
                    }
                  }
                  break;

    case A_SVC1:  // start service move (F03)
                  if (stop_request) {
                    prepare_service_move(valveindex, svc_move_dir, svc_move_counts, svc_move_maxmA);
                    refuse_move(valveindex);
                  }
                  else if (!waittimer) {
                    prepare_service_move(valveindex, svc_move_dir, svc_move_counts, svc_move_maxmA);
                    svc_prev_status = myvalvemots[valveindex].status;

                    if (motorcycle (valveindex, svc_move_dir == DIR_OPEN ? CMD_M_OPEN : CMD_M_CLOSE) ==
                        (svc_move_dir == DIR_OPEN ? M_RES_OPENS : M_RES_CLOSES)) {
                      myvalvemots[valveindex].status = svc_move_dir == DIR_OPEN ? VLV_STATE_OPENING : VLV_STATE_CLOSING;
                      valvestate = A_SVC2;
                      isr_counter = 0;
                    }
                    else {
                      record_refused_move(valveindex);
                      // nothing moved: app_loop may correct the position again
                      myvalves[valveindex].svcHold = 0;
                      valvestate = A_IDLE;
                    }
                  }
                  break;

    case A_SVC2:  // wait for the end of the service move
                  temp = motorcycle (valveindex, runcmd);
                  if (temp == M_RES_STOP || temp == M_RES_ENDSTOP || temp == M_RES_NOCURRENT || temp == M_RES_ERROR) {
                    finish_move(valveindex);
                    // the pulses really turned: the position follows them, also after a threshold stop
                    service_move_position(valveindex, isr_counter);
                    if (temp == M_RES_NOCURRENT) myvalvemots[valveindex].recal = 1;
                    // a fault or a pending request stays until a calibration clears it
                    if (svc_prev_status != VLV_STATE_IDLE) myvalvemots[valveindex].status = svc_prev_status;
                    else if (temp == M_RES_NOCURRENT) myvalvemots[valveindex].status = VLV_STATE_OPENCIR;
                    else if (temp == M_RES_ERROR) myvalvemots[valveindex].status = VLV_STATE_FAILED;
                    else myvalvemots[valveindex].status = VLV_STATE_IDLE;
                    valvestate = A_IDLE;
                    isr_counter = 0;
                  }
                  break;


    default:      valvestate = A_IDLE;
                  break;
  }
  if (valveindex<12) myvalvemots[valveindex].connected= (myvalvemots[valveindex].status != VLV_STATE_OPENCIR);

  // a valve state that never ends stops feeding the watchdog (see loop_system)
  valve_stall.tick((uint8_t) valvestate, valvestate == A_IDLE);
  valve_loop_stalled = valve_stall.stalled();
}


byte motorcycle (int mvalvenr, byte cmd) {
  #define M_INIT      0
  #define M_IDLE      1
  #define M_OPEN      2
  #define M_CLOSE     3
  #define M_TURNING   4
  #define M_TURNON    8
  #define M_STOP      5
  #define M_UNDERCURR 7
  #define M_TESTPREP  9
  #define M_TEST      10
  #define M_SETTLE    11      // wait for the MUX relay after set_motor()
  #define M_START     12      // start motor after M_SETTLE (open/close)
  #define M_TESTSTART 13      // start motor after M_SETTLE (test)

  static byte motorstate = M_INIT;
  static byte settle_next = M_IDLE;
  static byte settle_result = M_RES_TURNING;
  static int settlecnt = 0;
  static int turnoncnt = 0;

  static int cyclecnt = 0;
  static int debouncecnt = 0;

  static int normalcurrcnt = 0;

  static uint16_t meancurrent_cnt = 0;          // counts meancurrent values
  static long meancurrent_mem = 0;              // memory for meancurrent values
  byte result = 0;

  // target count reached (flag set by the EXTI handler, which must not run this state machine itself)
  if (isr_stop_request) {
    isr_stop_request = 0;
    if (motorstate == M_TURNON || motorstate == M_TURNING) {
      motorstate = M_STOP;
      motor_stop_cause = vdm::MotorStop::CountReached;
    }
  }

    switch (motorstate) {
      case M_INIT:  MUX_OFF();         // relay MUX off
                    ena_motor(0, 0);
                    DIR_OFF();         // direction off
  
                    motorstate = M_IDLE;
                    isr_turning = 0;
                    isr_counter = 0;
                    isr_target = 0;
                    result = M_RES_INIT;
                    isr_timer_go = 0;
                    isr_timer_fin = 0;
  
                    break;
  
      case M_IDLE:
                    if (cmd == CMD_M_OPEN) {
                      motorstate = M_OPEN;  
                      result = M_RES_OPENS;
                    }
                    else if (cmd == CMD_M_CLOSE) {
                      motorstate = M_CLOSE;  
                      result = M_RES_CLOSES;
                    }
                    else if (cmd == CMD_M_STOP) {
                      motorstate = M_STOP;
                      result = M_RES_STOP;  
                    }
                    else if (cmd == CMD_M_TEST) {
                      motorstate = M_TESTPREP;
                      result = M_RES_TEST;  
                    }
                    else {
                      motorstate = M_IDLE;  
                      result = M_RES_IDLE;
                    }
  
                    // correct motor nr?
                    if (motorstate != M_IDLE && (mvalvenr < 0 || mvalvenr >= (int) ACTUATOR_COUNT)) {
                      motorstate = M_IDLE;
                      result = M_RES_IDLE;
                    }

                    undercurrcnt = 0;
                    overcurrcnt = 0;
                    
                    break;
      
      case M_OPEN:  
                    #ifdef motDebug
                      COMM_DBG.println("M: state open");                                  
                    #endif
                    set_motor(mvalvenr, DIR_OPEN);
                    settlecnt = MUX_SETTLE_TICKS;
                    settle_next = M_START;
                    settle_result = M_RES_TURNING;
                    motorstate = M_SETTLE;
                    result = M_RES_OPENS;
                    break;
                    
      case M_CLOSE:
                    #ifdef motDebug
                      COMM_DBG.println("M: state close");                                 
                    #endif
                    set_motor(mvalvenr, DIR_CLOSE);
                    settlecnt = MUX_SETTLE_TICKS;
                    settle_next = M_START;
                    settle_result = M_RES_TURNING;
                    motorstate = M_SETTLE;
                    result = M_RES_CLOSES;
                    break;

      case M_SETTLE:
                    if (settlecnt > 0) settlecnt--;
                    if (settlecnt == 0) motorstate = settle_next;
                    result = settle_result;
                    break;

      case M_START:
                    undercurrcnt = 0;
                    overcurrcnt = 0;
                    motorstate = M_TURNON;
                    isr_valvenr = mvalvenr;
                    isr_counter=0;
                    cyclecnt = 0;
                    debouncecnt = 0;
                    isr_stop_request = 0;
                    isr_overcurrentevent = 0;     // may be left over from the previous move
                    isr_timer_go = 0;             // soft start handshake with TimerHandler0 starts from scratch
                    isr_timer_fin = 0;
                    turnoncnt = 0;
                    // isr_turning is still 0: TimerHandler0 does not sample before the motor is switched on
                    endstop.arm(move_bound_low, move_bound_high, protect_suspended ? vdm::EndStopDetector::InrushMode::Off
                      : protect_enforce ? vdm::EndStopDetector::InrushMode::Enforce : vdm::EndStopDetector::InrushMode::Report);
                    #ifdef inrushDebug
                      inrush_samples = 0;
                      inrush_peak = 0;
                      inrush_over = 0;
                    #endif
                    move_profile.reset();
                    motor_stop_cause = vdm::MotorStop::None;
                    last_turning_current = 0;
                    stroke_mean_mA = 0;
                    stroke_mean_samples = 0;
                    move_start_ms = millis();
                    result = M_RES_TURNING;
                    attachInterrupt(digitalPinToInterrupt(REVINPIN), isr_count, RISING);
                    break;
                  
      case M_TURNON:
                    isr_turning = 1;
                    result = M_RES_TURNING;
                    isr_timer_go = 1;

                    meancurrent_mem = 0;
                    meancurrent_cnt = 0;

                    analog_current_old = 0;
                    undercurrcnt = 0;
                    overcurrcnt = 0;
                    normalcurrcnt = 0;

                    // check if pwm is finished
                    if(isr_timer_fin) {
                      motorstate = M_TURNING;
                      isr_timer_fin = 0;
                      isr_timer_go = 0;
                    }
                    // TimerHandler0 (TIM1) did not enable the motor
                    else if (++turnoncnt > TIMEOUT_TURNON) {
                      #ifdef motDebug
                        COMM_DBG.println("M: motor enable timeout");
                      #endif
                      motor_halt();
                      motor_stop_cause = vdm::MotorStop::Aborted;
                      motorstate = M_IDLE;
                      result = M_RES_ERROR;
                    }

                    break;

      case M_TURNING:
                    result = M_RES_TURNING;
    
                    if (cmd == CMD_M_STOP) {
                      motorstate = M_STOP;  
                    }
                    cyclecnt++;
                                
                    if(debouncecnt<255) debouncecnt++;

                    last_turning_current = current_mA;
                    move_profile.add(isr_counter, last_turning_current);

                    if (debouncecnt > 3) {                      
                      // overcurrent detection
                      if(isr_overcurrentevent)         
                      {                                         
                          motor_halt();
                          motorstate = M_IDLE;
                          result = M_RES_ENDSTOP;
                          isr_overcurrentevent = 0;
                          motor_stop_cause = endstop.trip() == vdm::EndStopDetector::Trip::Bound
                            ? vdm::MotorStop::EndStop : vdm::MotorStop::SafetyOvercurrent;

                          stroke_mean_mA = vdm::strokeMeanCurrent((int32_t) meancurrent_mem, meancurrent_cnt);
                          stroke_mean_samples = meancurrent_cnt;
                          #ifdef motDebug
                            COMM_DBG.print("M: Current: "); COMM_DBG.print(current_mA/10,10); COMM_DBG.println(" mA");
                            COMM_DBG.print("M: Cnt:     "); COMM_DBG.println(isr_counter, DEC);                                 
                            COMM_DBG.println("M: Autostop, reached end stop");
                          #endif
                      }

                       // under current detection
                      else if(current_mA < THRESHOLD_UNDERCURRENT && current_mA > -THRESHOLD_UNDERCURRENT) 
                      { 
                        undercurrcnt++;
                        #ifdef motDebug
                            COMM_DBG.print("M: (M_TURNING) under current detection: undercurrcnt="); 
                            COMM_DBG.print(undercurrcnt); 
                            COMM_DBG.print(" current=");
                            COMM_DBG.print(current_mA/10,10); COMM_DBG.println(" mA");
                        #endif
                    
                        if (undercurrcnt > TIMEOUT_UNDERCURRENT)
                        {
                          #ifdef motDebug
                            COMM_DBG.print("M: (M_TURNING) under current detected: undercurrcnt="); 
                            COMM_DBG.print(undercurrcnt); 
                            COMM_DBG.print(" current=");
                            COMM_DBG.print(current_mA/10,10); COMM_DBG.println(" mA. set motorstate to  M_UNDERCURR");
                          #endif
                          undercurrcnt = 0;
                          motorstate = M_UNDERCURR;
                        }                        
                      }

                      // normal turning
                      else  {
                        normalcurrcnt++;
                        if(normalcurrcnt > TIMEOUT_NORMALCURRENT) {
                          #ifdef motDebug          
                            COMM_DBG.println("M: normal turning timeout");            
                          #endif
                          motor_halt();
                          normalcurrcnt = 0;
                          motor_stop_cause = vdm::MotorStop::Timeout;
                          motorstate = M_IDLE;
                          result = M_RES_ERROR;
                        }                
                      }
                    
                      // output position and current
                        #ifdef motDebugPosAndCurrent
                          COMM_DBG.print("tm;"); 
                          COMM_DBG.print(current_mA,10); 
                          COMM_DBG.print(";"); 
                          COMM_DBG.println(isr_counter);
                        #endif
                    }

                    if (debouncecnt > 50 && cyclecnt > 50) {
                      cyclecnt=0;
                      #ifdef motDebug
                        COMM_DBG.print("M: Current: "); COMM_DBG.print(current_mA/10, 10); COMM_DBG.println(" mA");
                      #endif
                      meancurrent_mem += current_mA;
                      if (meancurrent_cnt < 0xFFFF) meancurrent_cnt++;
                    }
                    break;
                      
      case M_STOP:         
                    #ifdef motDebug
                      COMM_DBG.println("M: state stop");    
                    #endif                                  
                    motor_halt();
                    if (motor_stop_cause == vdm::MotorStop::None) motor_stop_cause = vdm::MotorStop::Aborted;
                    #ifdef motDebug
                      COMM_DBG.print("M: Cnt: ");  
                      COMM_DBG.println(isr_counter, DEC);                                 
                    #endif
                    motorstate = M_IDLE;
                    result = M_RES_STOP;                  
                    
                    break;

      case M_UNDERCURR:         
                    motor_halt();
                    motor_stop_cause = vdm::MotorStop::Undercurrent;
                    #ifdef motDebug
                      COMM_DBG.println("M: state undercurrent detected (M_UNDERCURR), stopped");    
                    #endif                                  
                    motorstate = M_IDLE;
                    result = M_RES_NOCURRENT;                  
                    
                    break;

      case M_TESTPREP:
                    #ifdef motDebug
                      COMM_DBG.println("M: test prep");                                  
                    #endif
                    set_motor(mvalvenr, DIR_OPEN); 
                    settlecnt = MUX_SETTLE_TICKS;
                    settle_next = M_TESTSTART;
                    settle_result = M_RES_TEST;
                    motorstate = M_SETTLE;
                    result = M_RES_TEST;
                    break;

      case M_TESTSTART:
                    isr_overcurrentevent = 0;     // may be left over from the previous move
                    ena_motor(mvalvenr, 1);                 
                
                    test_presence.start(!protect_suspended, protect_enforce);

                    analog_current_old = 0;
                    analog_current = 0;

                    motorstate = M_TEST;
                    result = M_RES_TEST;                    

                    break;

      case M_TEST:
                    // open circuit, motor present or short, from the filtered test current (vdm::PresenceTest)
                    result = M_RES_TEST;
                    switch (test_presence.sample(analog_current)) {
                      case vdm::PresenceTest::Result::Absent:
                        result = M_RES_NOCURRENT;
                        break;
                      case vdm::PresenceTest::Result::Present:
                        result = M_RES_OPENS;
                        break;
                      case vdm::PresenceTest::Result::Short:
                        result = M_RES_ERROR;
                        break;
                      default:
                        break;
                    }
                    if (result != M_RES_TEST) {
                      #ifdef motDebug
                        COMM_DBG.print("test: result "); COMM_DBG.println(result);
                      #endif
                      motorstate = M_IDLE;
                      ena_motor(0, 0);
                    }
                    break;

      default:      motorstate = M_IDLE;
                    result = M_RES_IDLE;
                    break;
    }  

  return result;
}


// sets direction and mux relay
// the relay needs WAIT_MUX ms to settle before the motor is enabled, motorcycle() waits in M_SETTLE
void set_motor (int smvalvenr, int dir) {

  if (smvalvenr % 2) { MUX_OFF(); }
  else { MUX_ON(); }
  
  if (dir == DIR_OPEN) { DIR_OFF(); } 
  else { DIR_ON(); }
}


// enables motor 
void ena_motor (int emvalvenr, int state) {

  if (state == 0) { 
    ENA0_OFF(); ENA1_OFF(); ENA2_OFF(); ENA3_OFF(); ENA4_OFF(); ENA5_OFF(); 
  }
  else {
    if (emvalvenr == 0 || emvalvenr == 1) { ENA0_ON(); }
    else if (emvalvenr == 2 || emvalvenr == 3) { ENA1_ON(); }
    else if (emvalvenr == 4 || emvalvenr == 5) { ENA2_ON(); }
    else if (emvalvenr == 6 || emvalvenr == 7) { ENA3_ON(); }
    else if (emvalvenr == 8 || emvalvenr == 9) { ENA4_ON(); }
    else if (emvalvenr == 10 || emvalvenr == 11) { ENA5_ON(); }
  }
}


void isr_count () {
  if (isr_turning) isr_counter++;
  if (isr_target > 0) isr_target--;
  else callback_motorstop();
}


// switches the motor off and cancels a soft start that TimerHandler0 has not done yet;
// called by the motor state machine (TIM2)
static void motor_halt () {
  detachInterrupt(digitalPinToInterrupt(REVINPIN));
  isr_turning = 0;
  isr_timer_go = 0;       // before fin: TimerHandler0 enables the motor on go && !fin
  isr_timer_fin = 0;
  ena_motor(0, 0);
}


// call from isr to stop motor immediately
// the state machine picks the stop up on its next run (TIM2), it is not reentrant
void callback_motorstop () {
  detachInterrupt(digitalPinToInterrupt(REVINPIN));
  isr_turning = 0;
  ena_motor(0, 0);  
  isr_stop_request = 1;
}


// returns state of application  state machine
enum ASTATE valve_getstate () {
  return valvestate;
}


// the valve state machine is idle and no command is pending: no valve moves until the main loop
// hands over the next command, so until then the main loop may change the state of any valve
bool valve_idle () {
  const uint32_t primask = __get_PRIMASK();
  __disable_irq();
  const bool idle = valvestate == A_IDLE && command == '\0';
  __set_PRIMASK(primask);
  return idle;
}


int16_t appsetaction(char cmd, unsigned int valveindex, byte posdelta, bool force, uint8_t flags) {
  bool accepted = false;

  if (valveindex < ACTUATOR_COUNT) {
    // valve_loop (TIM2) must see valve, position and command as one consistent set
    const uint32_t primask = __get_PRIMASK();
    __disable_irq();
    // a command accepted earlier but not yet taken by valve_loop must not be overwritten
    if ((valvestate == A_IDLE && command == '\0') || force) {
      valvenr = (int) valveindex;
      poschangecmd = posdelta;
      moveflagscmd = flags;
      // from here on the kept position of the valve is not valid (warm reset in the move)
      app_warm_moving(valveindex);
      command = cmd;              // last: valve_loop acts on the command
      accepted = true;
    }
    __set_PRIMASK(primask);
  }

  if (accepted) return 0;
  else {
    #ifdef motDebug          
      COMM_DBG.print("command rejected, state: ");
      COMM_DBG.println(valvestate, DEC);            
    #endif
    return -1;
  }
}


int16_t appsetservice(unsigned int valveindex, uint8_t dir, uint16_t counts, uint8_t maxmA) {
  bool accepted = false;

  if (valveindex >= ACTUATOR_COUNT || dir > DIR_CLOSE || counts < SVMOV_COUNTS_MIN || counts > SVMOV_COUNTS_MAX
      || maxmA < SVMOV_MAXMA_MIN || maxmA > SVMOV_MAXMA_MAX) return -1;

  const uint32_t primask = __get_PRIMASK();
  __disable_irq();
  if (valvestate == A_IDLE && command == '\0') {
    svc_dir = dir;
    svc_counts = counts;
    svc_maxmA = maxmA;
    valvenr = (int) valveindex;
    // the valve is left where the move puts it; set before the command, as a refused start clears it
    myvalves[valveindex].svcHold = SVMOV_HOLD_10S;
    app_warm_moving(valveindex);
    command = CMD_A_SERVICE;      // last: valve_loop acts on the command
    accepted = true;
  }
  __set_PRIMASK(primask);

  return accepted ? 0 : -2;
}


int16_t appstop(unsigned int valve) {
  int16_t stopped = -1;
  const bool any = valve == 255;
  const uint32_t primask = __get_PRIMASK();
  __disable_irq();
  if (command != '\0' && (any || valvenr == (int) valve)) {
    // handed over but not taken yet: the command is dropped
    command = '\0';
    stopped = (int16_t) valvenr;
  }
  else if (valvestate != A_IDLE && valvestate != A_INIT && (any || valvenr == (int) valve)) {
    // the waiting states stop the motor; the request ends when the machine is back in A_IDLE
    stop_request = true;
    stopped = (int16_t) valvenr;
  }
  __set_PRIMASK(primask);
  return stopped;
}


int valve_busy_index () {
  const uint32_t primask = __get_PRIMASK();
  __disable_irq();
  const int busy = (valvestate == A_IDLE || valvestate == A_INIT) && command == '\0' ? -1 : valvenr;
  __set_PRIMASK(primask);
  return busy;
}


void TimerHandler0()        // called every 1 ms
{
  int analog_value;                     // current valve motor in 1/10 mA read by analog pin

  // current measurement
  analog_value = (int) ((( (int32_t) analogRead(ANINCURRENT) - (int32_t) analogRead(ANINREFHALF)) * ANINCURRENTGAIN) / 100);

  // filter test mode
  analog_current = (int) (((int32_t) analog_current_old * 9800 + (int32_t) analog_value * 200) / 10000);
  analog_current_old = analog_current;

  if(isr_turning) {
    #ifdef inrushDebug
      if (inrush_samples < 50) {
        inrush_samples++;
        const int32_t magnitude = analog_value < 0 ? -analog_value : analog_value;
        if (magnitude > inrush_peak) inrush_peak = magnitude;
        if (magnitude > vdm::EndStopDetector::kInrushLimit) inrush_over++;
      }
    #endif
    // inrush time, filter, end-stop bounds, safety and hard limit (EndStopDetector)
    const bool trip = endstop.sample(analog_value) != vdm::EndStopDetector::Trip::None;
    current_mA = endstop.current();

    if (trip) {
      // stop motor immediately
      detachInterrupt(digitalPinToInterrupt(REVINPIN));                           
      ena_motor(0, 0);
      isr_turning = 0;
      isr_overcurrentevent = 1;
    }
  }
  else {
    endstop.idle();
    current_mA = 0;
  } 

  // ENA valve (soft start requested by M_TURNON and not cancelled since)
  if(isr_turning && isr_timer_go && !isr_timer_fin) {
    isr_timer_fin = 1;
    ena_motor(isr_valvenr, 1);
  }
}
