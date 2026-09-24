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
#include "app.h"
#include "hardware.h"
#include "motor.h"
#include "owDevices.h"
#include "eeprom.h"


volatile struct valve myvalves[ACTUATOR_COUNT];

//unsigned char target_position_mirror[ACTUATOR_COUNT];
unsigned int learning_time = LEARN_AFTER_TIME_DEFAULT;
unsigned int learning_movements = LEARN_AFTER_MOVEMENTS_DEFAULT;


unsigned int reset_request = 0;


// hands a calibration to the valve state machine
static void app_start_learn (unsigned int valve) {
  if (appsetaction(CMD_A_LEARN, valve, 0) != 0) return;
  myvalvemots[valve].calibState = calibInProgress;
  myvalves[valve].forcedLearn = 0;
  myvalves[valve].timedLearn = 0;
  myvalves[valve].svcHold = 0;
  myvalves[valve].rejectedTarget = VALVE_NO_TARGET;
}


// a calibration of the valve is requested and has not ended yet (staln, time or movement
// trigger, a found valve after the start); status and calibration as read by the caller
bool app_learn_pending (uint16_t valve, byte status, bool calibration) {
  if (valve >= ACTUATOR_COUNT) return false;
  return calibration || myvalves[valve].forcedLearn || myvalves[valve].timedLearn
    || status == VLV_STATE_PRESENT;
}


// a target request (stgtp) ends the hold of a service move, also when the target did not change
void app_target_changed (uint16_t valve) {
  if (valve < ACTUATOR_COUNT) myvalves[valve].svcHold = 0;
}


// svmov: 0 accepted, -1 invalid arguments, -2 valve state machine busy, -3 calibration pending
// (the calibration would start right after the move and undo it)
int16_t app_service_move (uint16_t valve, uint8_t dir, uint16_t counts, uint8_t maxmA) {
  if (valve >= ACTUATOR_COUNT) return -1;
  if (app_learn_pending(valve, myvalvemots[valve].status, myvalvemots[valve].calibration)) return -3;
  const int16_t result = appsetservice(valve, dir, counts, maxmA);
  if (result == 0) myvalves[valve].svcHold = SVMOV_HOLD_10S;
  return result;
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
      // distribute learn timing equaly over valve slots
      myvalves[x].learn_time = (unsigned int) (((long)LEARN_AFTER_TIME_DEFAULT * ((long)x+1)) / (long)ACTUATOR_COUNT);  
  }

  // match sensor address from eeprom with found sensors and set index/slot to valve struct
  app_match_sensors();

  if ((eep_content.numberOfMovements>=50) && (eep_content.numberOfMovements<65535))
    learning_movements=eep_content.numberOfMovements;
    app_set_learnmovements(learning_movements);
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
  return 0;
}

int16_t app_loop (void) {
  static byte firstchange = 0;
  static unsigned int lastvalve = 0;
  static unsigned int testvlvindex = 0;

  reset_check();

    // if valve machine is idle search for new tasks
    if(valve_getstate() == A_IDLE) 
    {
        // find unknown states and try to find out whats on with the valve        
        if(myvalvemots[testvlvindex].status == VLV_STATE_UNKNOWN) 
        {
          #ifdef appDebug
            COMM_DBG.print("App: valve "); COMM_DBG.print(testvlvindex, 10);
            COMM_DBG.println(" unknown, try to find out...");
          #endif
          appsetaction(CMD_A_TEST,testvlvindex,0);    
        }
        
        else
        {        
          // fully open valves if needed
          if(myvalvemots[lastvalve].status == VLV_STATE_FULLOPEN) {
            appsetaction(CMD_A_OPEN_END,lastvalve,(byte)0);        
          }

          // learn all present valves if any target change happened before
          // this keeps controller calm right after startup, otherwise controller would be busy for up to 12 valve learning times (10 min ?!)
          // an explicit learn request (staln) does not wait for a target change (S08)
          else if((firstchange > 0 || myvalves[lastvalve].forcedLearn) && myvalvemots[lastvalve].status == VLV_STATE_PRESENT)  {
            #ifdef appDebug
              COMM_DBG.print("App 1: learning started for valve "); 
              COMM_DBG.println(lastvalve, 10);
            #endif
            app_start_learn(lastvalve);
          }

          // a learn request that a running move of the valve overwrote (the move ended with its own
          // status) is renewed; without it the time trigger would wait another learning_time and a
          // movement trigger would keep the valve's targets locked (calibration flag) until then
          else if ((myvalves[lastvalve].forcedLearn || myvalves[lastvalve].timedLearn
                    || (myvalvemots[lastvalve].calibration && myvalvemots[lastvalve].calibState == calibStarted))
                   && myvalvemots[lastvalve].status != VLV_STATE_UNKNOWN
                   && myvalvemots[lastvalve].status != VLV_STATE_PRESENT) {
            myvalvemots[lastvalve].status = VLV_STATE_PRESENT;
          }

          // handle first found difference then break
          // (a valve moved by svmov is left where it is until the next target request or the hold time is over)
          else if (myvalves[lastvalve].svcHold == 0 && myvalvemots[lastvalve].actual_position != myvalvemots[lastvalve].target_position)
          {
              #ifdef appDebug
                COMM_DBG.print("App: target pos changed for valve "); 
                COMM_DBG.print(lastvalve, 10);
                COMM_DBG.print(" target = ");
                COMM_DBG.println(myvalvemots[lastvalve].target_position, 10);
              #endif

              firstchange = 1;
                    
              // check if valve was learned before              
              if(myvalvemots[lastvalve].status == VLV_STATE_PRESENT)             
              {
                #ifdef appDebug
                  COMM_DBG.print("App 2: learning started for valve "); 
                  COMM_DBG.println(lastvalve, 10);
                #endif
                app_start_learn(lastvalve);
              }
              else if ((myvalvemots[lastvalve].status != VLV_STATE_FAILED) && (myvalvemots[lastvalve].status != VLV_STATE_BLOCKS))
              {
                myvalves[lastvalve].rejectedTarget = VALVE_NO_TARGET;
                // should valve be opened
                if(myvalvemots[lastvalve].target_position > myvalvemots[lastvalve].actual_position) {                  
                  if(myvalvemots[lastvalve].target_position == 100) appsetaction(CMD_A_OPEN_END,lastvalve,(byte)0);
                  else appsetaction(CMD_A_OPEN,lastvalve,myvalvemots[lastvalve].target_position-myvalvemots[lastvalve].actual_position);
                }
                // valve should be closed
                else {
                  if(myvalvemots[lastvalve].target_position == 0) appsetaction(CMD_A_CLOSE_END,lastvalve,(byte)0);
                  else appsetaction(CMD_A_CLOSE,lastvalve,myvalvemots[lastvalve].actual_position-myvalvemots[lastvalve].target_position);
                }
              }
              else {
                // the motor is not driven: the position stays as it is and the target is reported as
                // rejected, once per target value (S04); a calibration (time trigger, staln) clears the fault
                const byte target = myvalvemots[lastvalve].target_position;
                if (myvalves[lastvalve].rejectedTarget != target) {
                  myvalves[lastvalve].rejectedTarget = target;
                  if (myvalves[lastvalve].cmdRejected < 0xFFFF) myvalves[lastvalve].cmdRejected++;
                }
              }
          }

          lastvalve++;
          if (lastvalve>=ACTUATOR_COUNT) lastvalve = 0;
        }

        testvlvindex += 2;
        // vary startindexes to always get the even and the odd valves in one flow
        // helps reducing relay rattle (only C1 revision)
        if (testvlvindex == ACTUATOR_COUNT) testvlvindex = 1;
        else if (testvlvindex >= ACTUATOR_COUNT + 1) testvlvindex = 0;

    }

return 0;
}





byte app_10s_loop () {

  unsigned int x = 0;

  for (x=0; x< ACTUATOR_COUNT; x++) {
    if (myvalves[x].svcHold) myvalves[x].svcHold--;
  }

  // walk through valves and evaluate learning values

  // learning times
  if (learning_time > 0) {
    for (x=0; x< ACTUATOR_COUNT; x++) { 
      if(myvalves[x].learn_time <= 10) {
        myvalves[x].learn_time = learning_time;
        // a calibration of the valve that runs now satisfies the trigger; otherwise the request is
        // also kept in timedLearn, because a move of the valve that is running now ends with its
        // own status and overwrites PRESENT (app_loop renews it)
        if (myvalvemots[x].calibState == calibInProgress) continue;
        myvalves[x].timedLearn = 1;
        myvalvemots[x].status = VLV_STATE_PRESENT;     // next set target req will do a learning cycle
        #ifdef appDebug
          COMM_DBG.print("App: Valve "); 
          COMM_DBG.print(x, 10); 
          COMM_DBG.println(" will be learned soon");
        #endif
      }
      else myvalves[x].learn_time -= 10;    
    }
  }

  // learning movements
  if (learning_movements > 0) { 
    for (x=0; x< ACTUATOR_COUNT; x++) {
      // timeout for calibration 
      if (myvalvemots[x].connected) {
        if (myvalvemots[x].calibState==calibInProgress) {
          if (myvalvemots[x].calibTime>0) 
            myvalvemots[x].calibTime--;
          else  {
            myvalvemots[x].calibration=false;
            myvalvemots[x].calibState=calibIdle;
          }
        }
        if((myvalves[x].learn_movements == 0) && (myvalvemots[x].calibState == calibIdle)) {
          myvalvemots[x].calibration=true;
          myvalvemots[x].calibTime=10;
          myvalvemots[x].calibState = calibStarted;
          //myvalvemots[x].actual_position=0;
          myvalves[x].movements = 0;
          myvalves[x].learn_movements = learning_movements;
          myvalvemots[x].status = VLV_STATE_PRESENT;     // mark state as present, net set target req will do a learning cycle
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


// sets learning time
// after time seconds a learning cycle will be executed
int16_t app_set_learntime(uint32_t time) {
 
  // update reload value
  learning_time = time;
    
  // update all valve memories 
  for (unsigned int x = 0;x<ACTUATOR_COUNT;x++) {          
    // distribute learn timing equaly over valve slots
    // 64 bit product, learning_time * (x+1) can exceed 32 bit
    myvalves[x].learn_time = (unsigned int) (((uint64_t)learning_time * (x+1)) / ACTUATOR_COUNT);
  }

  return 0;

}


// sets learning of valve 
// a learning cycle for valve will be executed
// if valve = 255, all valves will be learned
int16_t app_set_valvelearning(uint16_t valve) {

  if(valve < ACTUATOR_COUNT) {
   // myvalvemots[valve].actual_position = 0;     // fake some position deviation
    myvalves[valve].forcedLearn = 1;
    myvalves[valve].svcHold = 0;
    myvalvemots[valve].status = VLV_STATE_PRESENT; //VLV_STATE_UNKNOWN;
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
        //myvalvemots[xx].actual_position = 0;      // fake some position deviation
        myvalves[xx].forcedLearn = 1;
        myvalves[xx].svcHold = 0;
        myvalvemots[xx].status = VLV_STATE_PRESENT; //VLV_STATE_UNKNOWN;
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


// scan valves 
// a learning cycle for valve will be executed
void app_scan_valves() 
{
    // scan all valves
    for(unsigned int xx=0;xx<ACTUATOR_COUNT;xx++){
      myvalvemots[xx].actual_position = 0;      // fake some position deviation
      myvalvemots[xx].status = VLV_STATE_UNKNOWN;
      myvalves[xx].svcHold = 0;
    }
}


// sets valve full open
// valve will be opened fully
// if valve = 255, all valves will be opened fully
int16_t app_set_valveopen(uint16_t valve) {

  if(valve < ACTUATOR_COUNT) {
    myvalvemots[valve].target_position = 100;
		myvalvemots[valve].status = VLV_STATE_FULLOPEN;
    myvalves[valve].svcHold = 0;
    return 0;
  }
  else if (valve == 255) {
    // update all valves
    for(unsigned int xx=0;xx<ACTUATOR_COUNT;xx++){
      myvalvemots[xx].target_position = 100;
			myvalvemots[xx].status = VLV_STATE_FULLOPEN;
      myvalves[xx].svcHold = 0;
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