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

#ifndef _MOTOR_H
	#define _MOTOR_H

#include "app.h"
#include "vdm/calibration.h"
#include "vdm/motor_params.h"
#include "vdm/move_classifier.h"
#include "vdm/profile_recorder.h"

#define CMD_A_OPEN      'o'
#define CMD_A_OPEN_END  'p'
#define CMD_A_CLOSE     'c'
#define CMD_A_CLOSE_END 'v'
#define CMD_A_LEARN     'l'
#define CMD_A_TARGET    't'
#define CMD_A_TEST      'x'
#define CMD_A_SERVICE   's'       // service move (svmov), parameters in appsetservice()

#define VLV_STATE_IDLE      (byte) 0x01       // nothing to do
#define VLV_STATE_OPENING   (byte) 0x02       // opens
#define VLV_STATE_CLOSING   (byte) 0x03       // closes
#define VLV_STATE_FAILED    (byte) 0x04       // failed (e.g jump away)
#define VLV_STATE_UNKNOWN   (byte) 0x05       // initial state
#define VLV_STATE_OPENCIR   (byte) 0x06       // open circuit detected
#define VLV_STATE_FULLOPEN  (byte) 0x07       // go directly full open
#define VLV_STATE_PRESENT   (byte) 0x08       // connected
#define VLV_STATE_BLOCKS    (byte) 0x09       // valve is blocked

enum CALIBSTATE {calibIdle,calibStarted,calibInProgress};

struct valvemotor {
//typedef struct valves {
  unsigned int closing_count;  
  unsigned int opening_count;
  int deadzone_count;              // closing_count - opening_count, may be negative
  unsigned int scaler;
  unsigned int meancurrent;
  // unsigned int sensorindex1;
  // unsigned int sensorindex2;
  // unsigned int learn_time;
  // unsigned int learn_movements;
  byte target_position;
  byte actual_position;
  byte status;
  uint8_t calibration;
  uint8_t calibTime;
  CALIBSTATE calibState;
  uint8_t connected;
  uint8_t calibRetries;
  uint8_t calibActive;             // a calibration of this valve is running (valve state machine)
};

// shared between the valve state machine (TIM2 interrupt) and the main loop
extern volatile valvemotor myvalvemots[ACTUATOR_COUNT];

enum ASTATE {
A_INIT, A_IDLE, A_CLOSE, A_OPEN1, A_OPEN2, A_LEARN1, 
A_LEARN2, A_LEARN3, A_LEARN4, A_SET, A_SET1, A_SET2, A_CLOSE1, A_CLOSE2, A_TEST,
A_SVC1, A_SVC2 };

extern volatile enum ASTATE valvestate;
extern volatile uint32_t valve_loop_ticks;     // incremented on every valve_loop run (watchdog heartbeat)
extern volatile bool valve_loop_stalled;       // valve state machine stuck in one busy state (watchdog must starve)




void valve_loop ();
//byte valve_setup (struct valve *valvedata);
//byte valve_setup (struct valvemotor *valvedata);
byte valve_setup ();
void valve_pins_safe ();

enum ASTATE valve_getstate ();
int16_t appsetaction(char cmd, unsigned int valveindex, byte pos, bool force=false);
// service move: dir vdm::kDirOpen/kDirClose, counts 1..10000, end-stop threshold maxmA 5..60
int16_t appsetservice(unsigned int valveindex, uint8_t dir, uint16_t counts, uint8_t maxmA);

#define SVMOV_COUNTS_MIN    1
#define SVMOV_COUNTS_MAX    10000
#define SVMOV_MAXMA_MIN     5
#define SVMOV_MAXMA_MAX     vdm::kSafetyLimit_mA

// diagnostics of one valve, written by the valve state machine
struct valve_diag {
  vdm::MoveResult last;         // last move (normal, calibration stroke or service move)
  uint16_t earlyStops;          // early end stops of normal moves since start-up
  bool earlyWarn;               // early end stop since the last successful calibration
  bool lastCalFailed;           // the last calibration did not succeed
};

// the fields gvlvx reports, copied together: the valve state machine changes several of
// them in one step (a calibration pass, the end of a move)
struct valve_snapshot {
  struct valve_diag diag;
  unsigned int opening_count;
  unsigned int closing_count;
  int deadzone_count;
  unsigned int meancurrent;
  unsigned int movements;
  byte status;
  byte actual_position;
  byte target_position;
  uint8_t calibration;
  uint8_t calibRetries;
  uint8_t calibActive;
};

// consistent copies, safe to call from the main loop
void valve_get_snapshot (unsigned int valveindex, struct valve_snapshot &out);
void valve_get_profile (unsigned int valveindex, vdm::ProfileRecorder &out);

// motor parameters (smotc/gmotc): RAM values and their EEPROM mirror
vdm::MotorParams motor_get_params ();
void motor_set_params (const vdm::MotorParams &params);

// breakaway escalation of calibration repetitions (scalx/gcalx)
vdm::EscalationConfig motor_get_escalation ();
void motor_set_escalation (const vdm::EscalationConfig &config);

extern uint8_t currentbound_low_fac;      // lower current limit factor for detection of end stop
extern uint8_t currentbound_high_fac;     // upper current limit factor for detection of end stop
extern uint8_t startOnPower;              // valve % on power start 
extern uint16_t noOfMinCounts;
extern uint8_t maxCalibRetries;

#endif     //_MOTOR_H