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
#include "vdm/stall_detector.h"



//#define COMM_DBG				Serial3		// serial port for debugging
#define COMM_DBG				Serial6		// serial port for debugging


#define DIR_CLOSE      (int)  1
#define DIR_OPEN       (int)  0 

#define TIMER0_INTERVAL_MS        1

#define TIMEOUT_NORMALCURRENT    120*100      // 120 seconds timeout with 10 ms cycle time
#define TIMEOUT_OVERCURRENT      1            // cycles of "byte motorcycle (byte valvenr, byte cmd)"
#define TIMEOUT_UNDERCURRENT     4*50           // cycles of "byte motorcycle (byte valvenr, byte cmd)"
#define TIMEOUT_UNDERCURRENTTEST 4*20           // cycles of "byte motorcycle (byte valvenr, byte cmd)"
#define THRESHOLD_UNDERCURRENT   20           // threshold for detecting undercurrent in 1/10 mA
#define TIMEOUT_TURNON           10           // cycles of motorcycle() to wait for TimerHandler0 (1 ms) to enable the motor
#define TIMEOUT_VALVESTATE       5*60*100     // 5 minutes with 10 ms cycle time, more than the longest valve state (one move <= ~123 s)


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
    const byte actual = myvalvemots[v].actual_position;
    expected = dir == DIR_OPEN ? (byte) (100 - (actual > 100 ? 100 : actual)) : actual;
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
  if (c.early && move_kind == MOVE_NORMAL) {
    if (rec.diag.earlyStops < 0xFFFF) rec.diag.earlyStops++;
    rec.diag.earlyWarn = true;
  }
}


// records a move the motor state machine did not start
static void record_refused_move (int v) {
  struct valve_record &rec = valve_records[v];
  rec.profile.reset();
  rec.diag.last = vdm::makeMoveResult(move_req, vdm::StopReason::Aborted, 0, 0, 0);
}


// every way out of a calibration: clears the request so stgtp is accepted again
static void learn_end (int v, bool success) {
  myvalvemots[v].calibration = false;
  myvalvemots[v].calibState = calibIdle;
  myvalvemots[v].calibActive = 0;
  valve_records[v].diag.lastCalFailed = !success;
  if (success) valve_records[v].diag.earlyWarn = false;
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


void valve_loop () {
  byte temp = 0;
  
  static byte pos_change = 0;

  static unsigned int closing_count = 0;  
  static unsigned int opening_count = 0;
  static uint16_t open_mean_mA = 0;
  static uint16_t open_mean_samples = 0;

  static int valveindex = 0;

  static int waittimer = 0;
  static int psuofftimer = 0;
  static int locktimer = 0;

  static uint8_t svc_move_dir = 0;
  static uint16_t svc_move_counts = 0;
  static uint8_t svc_move_maxmA = 0;
  static byte svc_prev_status = 0;

  valve_loop_ticks++;

  if(waittimer) waittimer--;

  switch (valvestate) {
    case A_INIT:  
                  if ( motorcycle(0, CMD_M_NOTHING) == M_RES_IDLE ) {                    
                     #ifdef motDebug
                        COMM_DBG.println("A: init ready");
                     #endif
                     valvestate = A_IDLE;
                     
                     //valveindex = 0;                     
                     //myvalves[valveindex].scaler = 130;
                     //myvalves[valveindex].target_position = 0;    

                     valveindex = 255;
                    
                  }
                  break;

    case A_IDLE:  
                  valveindex = valvenr;
                  if (command == CMD_A_OPEN) {
                    #ifdef motDebug                    
                      COMM_DBG.print("A: cmd open for valve ");                  
                      COMM_DBG.println(valveindex, 10);                    
                    #endif
                    valvestate = A_OPEN1;
                    PSU_ON();
                    waittimer = WAIT_TIMER50;
                    psuofftimer = 0;
                    pos_change = poschangecmd;
                  }
                  else if (command == CMD_A_CLOSE) {
                    #ifdef motDebug
                      COMM_DBG.print("A: cmd close for valve ");                  
                      COMM_DBG.println(valveindex, 10);                                        
                    #endif
                    valvestate = A_CLOSE1;  
                    PSU_ON();
                    waittimer = WAIT_TIMER50;
                    psuofftimer = 0;
                    pos_change = poschangecmd;
                  }
                  else if (command == CMD_A_OPEN_END) {  
                    #ifdef motDebug                  
                      COMM_DBG.print("A: cmd open to endstop for valve ");                  
                      COMM_DBG.println(valveindex, 10);                    
                    #endif
                    valvestate = A_OPEN1;
                    PSU_ON();
                    waittimer = WAIT_TIMER50;
                    psuofftimer = 0;
                    pos_change = 255;
                  }
                  else if (command == CMD_A_CLOSE_END) {   
                    #ifdef motDebug                 
                      COMM_DBG.print("A: cmd close to endstop for valve ");                  
                      COMM_DBG.println(valveindex, 10);                    
                    #endif
                    valvestate = A_CLOSE1;
                    PSU_ON();
                    waittimer = WAIT_TIMER50;
                    psuofftimer = 0;
                    pos_change = 255;
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
                  if (!waittimer) {
                    prepare_normal_move(valveindex, DIR_OPEN, pos_change);
                    
                    if (motorcycle (valveindex, CMD_M_OPEN) == M_RES_OPENS) {
                      #ifdef motDebug
                        COMM_DBG.print("A: begin opening by ");                  
                        COMM_DBG.println(pos_change);
                      #endif
                      myvalvemots[valveindex].status = VLV_STATE_OPENING;                                                            
                      valvestate = A_OPEN2;
                      isr_counter=0;
                    }
                    else {
                      #ifdef motDebug
                        COMM_DBG.print("A: cant open valve");                  
                      #endif
                      record_refused_move(valveindex);
                      valvestate = A_IDLE;
                    }
                  }
                  break;
  
    case A_OPEN2:  // wait for finish opening
                  temp = motorcycle (valveindex, 0);
                  if (temp == M_RES_STOP) {
                    #ifdef motDebug
                      COMM_DBG.println("A: opened valve"); 
                    #endif                 
                    finish_move(valveindex);
                    valvestate = A_IDLE;
                    myvalvemots[valveindex].actual_position = position_add(myvalvemots[valveindex].actual_position, pos_change);
                    myvalvemots[valveindex].status = VLV_STATE_IDLE; 
                    #ifdef motDebug                   
                      COMM_DBG.print("A: new position "); COMM_DBG.println(myvalvemots[valveindex].actual_position);
                    #endif
                  }
                  else if (temp == M_RES_NOCURRENT) {
                    #ifdef motDebug
                      COMM_DBG.println("A: (A_OPEN2) undercurrent");
                    #endif
                    finish_move(valveindex);
                    myvalvemots[valveindex].status = VLV_STATE_OPENCIR;
                    myvalvemots[valveindex].actual_position = myvalvemots[valveindex].target_position;
                    valvestate = A_IDLE;
                    isr_counter=0;
                  } 
                  else if (temp == M_RES_ENDSTOP) {
                    #ifdef motDebug
                      COMM_DBG.println("A: opened valve to end stop");
                    #endif
                    finish_move(valveindex);
                    valvestate = A_IDLE;
                    myvalvemots[valveindex].status = VLV_STATE_IDLE;
                    myvalvemots[valveindex].actual_position = 100;
                    #ifdef motDebug
                      COMM_DBG.print("A: new position "); COMM_DBG.println(myvalvemots[valveindex].actual_position);
                    #endif
                  }
                  else if (temp == M_RES_ERROR) {
                    #ifdef motDebug
                      COMM_DBG.println("A: opening valve failed, timeout");
                    #endif
                    // the position is unknown: it stays as it was, app_loop reports the target as rejected (S04)
                    finish_move(valveindex);
                    valvestate = A_IDLE;
                    myvalvemots[valveindex].status = VLV_STATE_FAILED;
                  }                                     
                  break;

    case A_CLOSE1:  // start valve closing
                  if (!waittimer) {
                    prepare_normal_move(valveindex, DIR_CLOSE, pos_change);
                    
                    if (motorcycle (valveindex, CMD_M_CLOSE) == M_RES_CLOSES) {
                      #ifdef motDebug
                        COMM_DBG.print("A: begin closing by ");                  
                        COMM_DBG.println(pos_change);       
                      #endif
                      myvalvemots[valveindex].status = VLV_STATE_CLOSING;                                                     
                      valvestate = A_CLOSE2;
                      isr_counter=0;
                    }   
                    else {
                      #ifdef motDebug
                        COMM_DBG.print("A: cant close valve");                  
                      #endif
                      record_refused_move(valveindex);
                      valvestate = A_IDLE;
                    }
                  }
                  break;

    case A_CLOSE2:  // wait for finish closing
                  temp = motorcycle (valveindex, 0);
                  if (temp == M_RES_STOP) {
                    #ifdef motDebug
                      COMM_DBG.println("A: closed valve");
                    #endif
                    finish_move(valveindex);
                    valvestate = A_IDLE;
                    myvalvemots[valveindex].status = VLV_STATE_IDLE;
                    myvalvemots[valveindex].actual_position = position_sub(myvalvemots[valveindex].actual_position, pos_change);
                    #ifdef motDebug
                      COMM_DBG.print("A: new position "); COMM_DBG.println(myvalvemots[valveindex].actual_position);                   
                    #endif
                  }
                  else if (temp == M_RES_NOCURRENT) {
                    #ifdef motDebug
                      COMM_DBG.println("A: (A_CLOSE2) undercurrent");
                    #endif
                    finish_move(valveindex);
                    myvalvemots[valveindex].status = VLV_STATE_OPENCIR;
                    myvalvemots[valveindex].actual_position = myvalvemots[valveindex].target_position;
                    valvestate = A_IDLE;
                    isr_counter=0;
                  } 
                  else if (temp == M_RES_ENDSTOP) {
                    #ifdef motDebug
                      COMM_DBG.println("A: closed valve to end stop");
                    #endif
                    finish_move(valveindex);
                    valvestate = A_IDLE;
                    myvalvemots[valveindex].status = VLV_STATE_IDLE;
                    myvalvemots[valveindex].actual_position = 0;
                    #ifdef motDebug
                      COMM_DBG.print("A: new position "); COMM_DBG.println(myvalvemots[valveindex].actual_position);
                    #endif
                  }            
                  else if (temp == M_RES_ERROR) {
                    #ifdef motDebug
                      COMM_DBG.println("A: closing valve failed, timeout");
                    #endif
                    // the position is unknown: it stays as it was, app_loop reports the target as rejected (S04)
                    finish_move(valveindex);
                    valvestate = A_IDLE;
                    myvalvemots[valveindex].status = VLV_STATE_FAILED;
                  }                   
                  break;                  
    
    case A_LEARN1:  // prepare learning  
                  #ifdef motDebug        
                    COMM_DBG.println("A: try learning valve");                  
                  #endif
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
                  if (!waittimer) {
                    // waiting for closed valve as start position
                    temp = motorcycle (valveindex, 0);
                    if (temp == M_RES_ENDSTOP) {
                      #ifdef motDebug
                        COMM_DBG.println("A: closed valve before learning, now opening");                  
                      #endif
                      finish_move(valveindex);
                      // second: opening completely and count rotations
                      isr_counter=0;
                      myvalvemots[valveindex].status = VLV_STATE_OPENING;
                      prepare_learn_stroke(valveindex, DIR_OPEN, true);
                      motorcycle (valveindex, CMD_M_OPEN);
                      valvestate = A_LEARN3;
                      waittimer = WAIT_TIMER20;
                    }
                    else if (temp == M_RES_NOCURRENT) {
                      #ifdef motDebug
                        COMM_DBG.println("A: (A_LEARN2) undercurrent");
                      #endif
                      finish_move(valveindex);
                      myvalvemots[valveindex].status = VLV_STATE_OPENCIR;
                      myvalvemots[valveindex].target_position = myvalvemots[valveindex].actual_position;
                      learn_end(valveindex, false);
                      valvestate = A_IDLE;
                      isr_counter=0;
                    }
                    // stop: the counter ran out (isr_target 65535) without an end stop, idle: motor machine not running
                    else if (temp == M_RES_ERROR || temp == M_RES_STOP || temp == M_RES_IDLE) {
                      #ifdef motDebug
                        COMM_DBG.println("A: closing valve failed, timeout");
                      #endif
                      if (temp != M_RES_IDLE) finish_move(valveindex);
                      learn_end(valveindex, false);
                      valvestate = A_IDLE;
                      myvalvemots[valveindex].status = VLV_STATE_FAILED;
                    }           
                  }               
                  break;
                  
    case A_LEARN3:  // learning opening way
                  if (!waittimer) {
                    // waiting for opened valve
                    temp = motorcycle (valveindex, 0);
                    if (temp == M_RES_ENDSTOP) {
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
                      isr_counter=0;
                      myvalvemots[valveindex].status = VLV_STATE_CLOSING;
                      prepare_learn_stroke(valveindex, DIR_CLOSE, true);
                      motorcycle (valveindex, CMD_M_CLOSE);
                      valvestate = A_LEARN4;
                      waittimer = WAIT_TIMER20;
                    }          
                    else if (temp == M_RES_NOCURRENT) {
                      #ifdef motDebug
                        COMM_DBG.println("A: (A_LEARN3) undercurrent");
                      #endif
                      finish_move(valveindex);
                      myvalvemots[valveindex].status = VLV_STATE_OPENCIR;
                      myvalvemots[valveindex].target_position = myvalvemots[valveindex].actual_position;
                      learn_end(valveindex, false);
                      valvestate = A_IDLE;
                      isr_counter=0;
                    }
                    // stop: the counter ran out (isr_target 65535) without an end stop, idle: motor machine not running
                    else if (temp == M_RES_ERROR || temp == M_RES_STOP || temp == M_RES_IDLE) {
                      #ifdef motDebug
                        COMM_DBG.println("A: opening valve failed, timeout");
                      #endif
                      if (temp != M_RES_IDLE) finish_move(valveindex);
                      learn_end(valveindex, false);
                      valvestate = A_IDLE;
                      myvalvemots[valveindex].status = VLV_STATE_FAILED;
                    }                
                  }              
                  break;
                  
    case A_LEARN4:  // learning closing way
                  if (!waittimer) {
                    // waiting for closed valve again
                    temp = motorcycle (valveindex, 0);
                    if (temp == M_RES_ENDSTOP) { 
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
                        valve_records[valveindex].diag.lastCalFailed = false;
                        valve_records[valveindex].diag.earlyWarn = false;
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
                        COMM_DBG.println("A: (A_LEARN4) undercurrent");
                      #endif
                      finish_move(valveindex);
                      myvalvemots[valveindex].status = VLV_STATE_OPENCIR;
                      myvalvemots[valveindex].target_position = myvalvemots[valveindex].actual_position;
                      learn_end(valveindex, false);
                                          
                      valveindex = 255;
                      valvestate = A_IDLE;
                      isr_counter=0;
                    }     
                    // stop: the counter ran out (isr_target 65535) without an end stop, idle: motor machine not running
                    else if (temp == M_RES_ERROR || temp == M_RES_STOP || temp == M_RES_IDLE) {
                      #ifdef motDebug
                        COMM_DBG.println("A: closing valve failed, timeout");
                      #endif
                      if (temp != M_RES_IDLE) finish_move(valveindex);
                      learn_end(valveindex, false);
                      valvestate = A_IDLE;
                      myvalvemots[valveindex].status = VLV_STATE_FAILED;
                    }        
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
                  if (!waittimer) {
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
  
    case A_SET2:  // wait for finish opening
                  temp = motorcycle (valveindex, 0);
                  if (temp == M_RES_STOP) {
                    #ifdef motDebug
                      COMM_DBG.println("A_SET2: opened valve"); 
                    #endif                 
                    finish_move(valveindex);
                    myvalvemots[valveindex].actual_position = position_add(myvalvemots[valveindex].actual_position, pos_change);
                    #ifdef motDebug                   
                      COMM_DBG.print("A_SET2: new position "); COMM_DBG.println(myvalvemots[valveindex].actual_position);
                    #endif
                    myvalvemots[valveindex].status = VLV_STATE_IDLE; 
                    learn_end(valveindex, true);
                    valveindex = 255;                                                                                  
                    valvestate = A_IDLE;
                  }
                  else if (temp == M_RES_NOCURRENT) {
                    #ifdef motDebug
                      COMM_DBG.println("A_SET2: (A_OPEN2) undercurrent");
                    #endif
                    finish_move(valveindex);
                    myvalvemots[valveindex].status = VLV_STATE_OPENCIR;
                    myvalvemots[valveindex].actual_position = myvalvemots[valveindex].target_position;
                    learn_end(valveindex, true);
                    valvestate = A_IDLE;
                    isr_counter=0;
                  } 
                  else if (temp == M_RES_ENDSTOP) {
                    #ifdef motDebug
                      COMM_DBG.println("A_SET2: opened valve to end stop");
                    #endif
                    finish_move(valveindex);
                    valvestate = A_IDLE;
                    myvalvemots[valveindex].status = VLV_STATE_IDLE;
                    myvalvemots[valveindex].actual_position = 100;
                    learn_end(valveindex, true);
                    #ifdef motDebug
                      COMM_DBG.print("A_SET2: new position "); COMM_DBG.println(myvalvemots[valveindex].actual_position);
                    #endif
                  }
                  else if (temp == M_RES_ERROR) {
                    #ifdef motDebug
                      COMM_DBG.println("A_SET2: opening valve failed, timeout");
                    #endif
                    finish_move(valveindex);
                    valvestate = A_IDLE;
                    myvalvemots[valveindex].status = VLV_STATE_FAILED;
                    learn_end(valveindex, true);
                  }                                     
                  break;

     case A_TEST:  // test if a valve is connected                  
                  if (!waittimer) {
                    
                    temp = motorcycle (valveindex, CMD_M_TEST);
                    if (temp != M_RES_TEST) {  
                      #ifdef motDebug
                        COMM_DBG.print("A: (A_TEST) test valve ");
                        COMM_DBG.print(valveindex, DEC);                  
                      #endif
                      if(temp == M_RES_NOCURRENT) {
                        #ifdef motDebug
                          COMM_DBG.println(" --> undercurrent, no valve");
                        #endif
                        myvalvemots[valveindex].status = VLV_STATE_OPENCIR;
                      }
                      else {
                        #ifdef motDebug
                          COMM_DBG.println(" --> valve present");
                        #endif
                        myvalvemots[valveindex].status = VLV_STATE_PRESENT;
                      }

                      valvestate = A_IDLE;                    
                    }
                  }
                  break;

    case A_SVC1:  // start service move (F03)
                  if (!waittimer) {
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
                      valvestate = A_IDLE;
                    }
                  }
                  break;

    case A_SVC2:  // wait for the end of the service move
                  temp = motorcycle (valveindex, 0);
                  if (temp == M_RES_STOP || temp == M_RES_ENDSTOP || temp == M_RES_NOCURRENT || temp == M_RES_ERROR) {
                    finish_move(valveindex);
                    // the pulses really turned: the position follows them, also after a threshold stop
                    service_move_position(valveindex, isr_counter);
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
  static int testcnt = 0;
 
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
                    endstop.arm(move_bound_low, move_bound_high);
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
                    if(testcnt<255) testcnt++;

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
                
                    cyclecnt = 0;
                    undercurrcnt = 0;
                    normalcurrcnt = 0;
                    overcurrcnt=0;
                    debouncecnt = 0;

                    analog_current_old = 0;
                    analog_current = 0;

                    motorstate = M_TEST;
                    result = M_RES_TEST;                    

                    break;

      case M_TEST:
                    #ifdef motDebug
                      COMM_DBG.print("M: testing"); 
                    #endif
                    result = M_RES_TEST;
                    // calc current in 1/10 mA
                    #ifdef motDebug
                      COMM_DBG.print(" - current: ");
                      COMM_DBG.println (analog_current,DEC);
                    #endif
                    if(debouncecnt<255) debouncecnt++;

                    if(debouncecnt>7) {

                      // under current detection
                      if(analog_current < THRESHOLD_UNDERCURRENT && analog_current > -THRESHOLD_UNDERCURRENT) 
                      {
                        undercurrcnt++;
                        if (undercurrcnt > TIMEOUT_UNDERCURRENTTEST)
                        {
                          #ifdef motDebug
                            COMM_DBG.println("test: (M_TEST) undercurrent!");
                          #endif
                          undercurrcnt = 0;
                          motorstate = M_IDLE;
                          result = M_RES_NOCURRENT;
                          ena_motor(0, 0);
                        }                        
                      }

                      // overcurrent detection
                      else if(isr_overcurrentevent)                              
                      {
                        isr_overcurrentevent = 0;
                        #ifdef motDebug
                          COMM_DBG.println("test: overcurrent!");
                        #endif
                        motorstate = M_IDLE;
                        result = M_RES_ENDSTOP;
                        ena_motor(0, 0);
                      }

                      // normal turning
                      else  {
                        normalcurrcnt++;
                        if(normalcurrcnt > 5) {
                          #ifdef motDebug          
                            COMM_DBG.println("test: normal turning!");            
                          #endif
                          motorstate = M_IDLE;
                          result = M_RES_OPENS;
                          ena_motor(0, 0);      
                        }                
                      }
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


int16_t appsetaction(char cmd, unsigned int valveindex, byte posdelta, bool force) {
  bool accepted = false;

  if (valveindex < ACTUATOR_COUNT) {
    // valve_loop (TIM2) must see valve, position and command as one consistent set
    const uint32_t primask = __get_PRIMASK();
    __disable_irq();
    // a command accepted earlier but not yet taken by valve_loop must not be overwritten
    if ((valvestate == A_IDLE && command == '\0') || force) {
      valvenr = (int) valveindex;
      poschangecmd = posdelta;
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
    command = CMD_A_SERVICE;      // last: valve_loop acts on the command
    accepted = true;
  }
  __set_PRIMASK(primask);

  return accepted ? 0 : -2;
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
