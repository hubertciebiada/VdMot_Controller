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

#ifndef _APP_H
	#define _APP_H

#include <Arduino.h>
#include "hardware.h"
#include "motor.h"



#define LEARN_AFTER_MOVEMENTS_DEFAULT     2000          // after x movements a learning cycle will be executed
#define LEARN_AFTER_TIME_DEFAULT          7*24*3600   // after x seconds a learning cycle will be executed
#define NO_OF_MIN_COUNTS                  3000

#define VALVE_INIT_TEMPERATURE            -2000   // init value for struct value of temperature
#define VALVE_SENSOR_UNKNOWN              65535   // marks that no sensor slot is selected
#define VALVE_NO_TARGET                   255     // rejectedTarget: no target known
#define SVMOV_HOLD_10S                    30      // app_10s_loop calls (~11 s) a service moved valve is left alone
#define CALIB_START_TICKS                 2       // app_10s_loop calls a handed over calibration may take to start
#define RETEST_TEST                       1       // retestRequest: presence test (automatic retry after a short)
#define RETEST_DETECT                     2       // retestRequest: stdet, a valve found present calibrates fully


int16_t app_setup (void);
void app_load_config (void);
int16_t app_loop (void);
byte app_10s_loop (uint32_t elapsedS);    // elapsedS: real seconds since the last call
int16_t app_set_learnmovements(uint16_t cycles);
int16_t app_set_learntime(uint32_t time);
int16_t app_set_valvelearning(uint16_t valve);
void app_scan_valves();
int16_t app_set_valveopen(uint16_t valve);
void app_target_changed(uint16_t valve);
bool app_learn_pending(uint16_t valve, byte status, bool calibration);   // calState "requested"
int16_t app_service_move(uint16_t valve, uint8_t dir, uint16_t counts, uint8_t maxmA);
int16_t app_match_sensors();
void reset_check();
void reset_STM32();

// protocol 3: lease of the targets, failsafe positions, stop, learn time, warm restart, diagnostics
void app_1s_tick(uint32_t elapsedS);                 // main loop, every second: lease, retries, learn time rule
void app_restore(void);                              // start-up, after valve_setup() and before the valve timer
void app_warm_save(void);                            // main loop, 10 ms branch after app_loop()
void app_lease_poll(void);                           // gvlvd/gvlvx request: renews the lease only without a lease client
void app_lease_command(void);                        // slhbt/slcfg/sfspo/glcfg: a lease client is present
void app_lease_heartbeat(bool alive);                // slhbt
void app_lease_configure(uint16_t minutes);          // slcfg, validated and persisted by the caller
uint8_t app_lease_state(void);                       // 0 off, 1 running, 2 expired
uint32_t app_lease_remaining_s(void);
bool app_lease_client(void);
uint16_t app_lease_timeout(void);
uint16_t app_failsafe_mask(void);                    // bit v: valve v is at its failsafe position (lease expired)
void app_set_failsafe(uint16_t valve, uint8_t pct);  // valve 0..11 or 255; runtime only
uint8_t app_failsafe_pct(uint16_t valve);
int16_t app_stop(uint16_t valve);                    // sstop: valve 0..11 or 255; -1 invalid
uint32_t app_get_learntime(void);                    // stored learn time (gtlnt)
void app_temp_cycle_done(void);                      // owDevices: a temperature cycle completed
uint32_t app_temp_age_s(void);                       // seconds since the last complete temperature cycle
bool app_protect_suspended(void);                    // short and inrush limits suspended until the next start
struct valve_v3_info { uint16_t flags; uint8_t fault; uint8_t fsPct; uint8_t drive; uint32_t retryS; uint8_t retries; };
void app_get_valve_v3(uint16_t valve, struct valve_v3_info &out);   // gvlvy values 20..25
void app_warm_moving(unsigned int valve);            // appsetaction/appsetservice (interrupts disabled): the kept position is not valid

// struct valvemotor {
// //typedef struct valves {
//   unsigned int closing_count;  
//   unsigned int opening_count;
//   unsigned int deadzone_count;  
//   unsigned int scaler;
//   unsigned int meancurrent;
//   // unsigned int sensorindex1;
//   // unsigned int sensorindex2;
//   // unsigned int learn_time;
//   // unsigned int learn_movements;
//   byte target_position;
//   byte actual_position;
//   byte status;
// };



struct valve {
  unsigned int sensorindex1;
  unsigned int sensorindex2;
  unsigned int learn_time;
  unsigned int learn_movements;
  unsigned int movements;
//   byte target_position;
//   byte actual_position;
  byte statusm;
  uint16_t cmdRejected;       // target changes not executed because the valve is FAILED or BLOCKS
  byte rejectedTarget;        // last target that is not a new request (see vdm::rejectTarget), VALVE_NO_TARGET if none
  uint8_t forcedLearn;        // staln: learn without waiting for a target change
  uint8_t timedLearn;         // time trigger: learn at the next target change (after firstchange)
  uint8_t svcHold;            // after svmov the position is left alone (app_10s_loop calls); set by appsetservice, cleared if the start is refused
  uint8_t retestRequest;      // RETEST_*: test the valve again (applied by app_loop while no valve moves)
  uint8_t openRequest;        // staop: open fully (applied by app_loop while no valve moves)
  uint8_t assemblyHold;       // staop: the lease failsafe does not apply until the next stgtp
  uint8_t retryLearn;         // automatic retry of a failed or blocked valve: calibrate without waiting for a target change
  uint8_t earlyLearn;         // second early partial stop in a row: calibrate
  uint8_t calibRestored;      // counts restored from the EEPROM, no calibration since start-up
  uint8_t storedSeq;          // myvalvemots[].calibSeq of the calibration record last handed to the EEPROM
  uint8_t touched;            // stgtp since the last reference move or calibration
  //struct valvemotor valvemot;
};

extern volatile struct valve myvalves[ACTUATOR_COUNT];
extern unsigned int learning_movements;

#endif //_APP_H


