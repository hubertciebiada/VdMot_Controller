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
#include "hardware.h"
#include <Wire.h>
#include "../lib/OneWire/OneWire.cpp"
#include <DallasTemperature.h>
#include <ArduinoJson.h>            // library https://github.com/bblanchon/ArduinoJson
#include "owDevices.h"
#include "DS2438.h"
#include "app.h"


tempsensor  tempsensors[MAXONEWIRECNT];
voltsensor  voltsensors[MAXDS2438CNT];
uint8_t     noOfDevices = 0;
uint8_t     noOfDS18Devices = 0;
uint8_t     noOfDS2438Devices = 0;

OneWire     oneWire(ONEW_PIN);

DallasTemperature sensors(&oneWire);
DS2438    bm(&oneWire);

enum t_state {T_INIT, T_IDLE, T_REQUEST, T_OUTPUT, T_READVAD, T_WAIT, T_SEARCH};

volatile int temp_cmd = 0;
volatile int lock = 0;

#define COMM_DBG				Serial6		// serial port for debugging

#define DS2438_FAMILY         0x26
#define MAX_ONEWIRE_SEARCH    64          // upper bound of search passes per enumeration
#define TEMP_UNKNOWN          -500        // temperature (1/10 degC) of a sensor that was not read yet


void printAddress(DeviceAddress deviceAddress)
{
  #if defined tempDebug || defined appDebug
    COMM_DBG.print("{");
    for (uint8_t i = 0; i < 8; i++)
    {  
      COMM_DBG.print(" ");
      if (deviceAddress[i] < 16) COMM_DBG.print("0");
      COMM_DBG.print(deviceAddress[i], HEX);
      if (i<7) COMM_DBG.print(", ");
      
    }
    COMM_DBG.print(" }");  
  #endif
}

// enumerates the bus once and fills tempsensors[] (DS18 family) and voltsensors[] (DS2438)
// in search order; devices beyond the array sizes are ignored
void setDeviceAddress() {
    DeviceAddress addr;
    uint8_t devices = 0;
    uint8_t ds18 = 0;
    uint8_t ds2438 = 0;

    oneWire.reset_search();
    for (uint8_t guard = 0; guard < MAX_ONEWIRE_SEARCH && oneWire.search(addr); guard++) {
        if (OneWire::crc8(addr, 7) != addr[7]) continue;
        devices++;
        if (addr[0] == DS2438_FAMILY) {
          if (ds2438 < MAXDS2438CNT) {
            memcpy(voltsensors[ds2438].address, addr, sizeof(DeviceAddress));
            #ifdef tempDebug
              printAddress(voltsensors[ds2438].address);       
              COMM_DBG.println();
            #endif
            ds2438++;
          }
        } 
        else if (sensors.validFamily(addr)) {
          if (ds18 < MAXONEWIRECNT) {
            // a different sensor at this index must not inherit the old temperature
            if (memcmp(tempsensors[ds18].address, addr, sizeof(DeviceAddress)) != 0) {
              memcpy(tempsensors[ds18].address, addr, sizeof(DeviceAddress));
              tempsensors[ds18].temperature = TEMP_UNKNOWN;
            }
            #ifdef tempDebug
              printAddress(tempsensors[ds18].address);       
              COMM_DBG.println();
            #endif
            ds18++;
          }
        }
    }

    // forget sensors that are no longer present
    for (uint8_t i = ds18; i < MAXONEWIRECNT; i++) {
      memset(tempsensors[i].address, 0, sizeof(DeviceAddress));
      tempsensors[i].temperature = TEMP_UNKNOWN;
    }
    for (uint8_t i = ds2438; i < MAXDS2438CNT; i++) {
      memset(voltsensors[i].address, 0, sizeof(DeviceAddress));
      voltsensors[i].vad = 0;
    }

    noOfDevices = devices;
    noOfDS18Devices = ds18;
    noOfDS2438Devices = ds2438;

    #ifdef tempDebug
      COMM_DBG.print("Found "); 
      COMM_DBG.print(noOfDS18Devices, 10); 
      COMM_DBG.print(" temp sensors,");
      COMM_DBG.print(noOfDS2438Devices, 10); 
      COMM_DBG.println(" voltage sensors:");
    #endif
}

void temperature_setup() {
  uint8_t thisSensorAddress [8];

    #ifdef tempDebug
      COMM_DBG.println("starting 1-wire setup"); 
    #endif
    for (uint8_t i=0; i<MAXONEWIRECNT; i++)
    {
      memset (tempsensors[i].address,0x0,8);
      tempsensors[i].temperature = TEMP_UNKNOWN;
    }

    sensors.begin();
  
    sensors.setWaitForConversion(false);
    #ifdef tempDebug
      COMM_DBG.println("search for devices"); 
    #endif
    setDeviceAddress();

    bm.begin();

}



void temperature_loop() {
    static enum t_state tempstate = T_INIT;
    static int substate = 0;
    static int devcnt = 0;
    static int devDS2438Cnt = 0;
    static unsigned int timer = 0;

    DeviceAddress currAddress;
    float temp = 0;
    float v = 0;

  switch (tempstate) {
    case T_INIT:  
              #ifdef tempDebug
                COMM_DBG.print("Found "); 
                COMM_DBG.print(noOfDevices, 10); 
                COMM_DBG.println(" temp sensors");
              #endif
              timer = CONV_INTERVALL / 10;                
              tempstate = T_IDLE;
              break;

    case T_IDLE:    

              if(temp_cmd == TEMP_CMD_NEWSEARCH) {
                substate = 0;
                tempstate = T_SEARCH;
              }
              else {
                if (lock == 0) {
                  tempstate = T_REQUEST;
                } 
              }

              break;

    case T_REQUEST:
              #ifdef tempDebug
                COMM_DBG.println("Requesting temperatures...");
              #endif
              sensors.requestTemperatures();

              timer = 20 + (sensors.millisToWaitForConversion(sensors.getResolution()) / 10);
              tempstate = T_WAIT;

              break;

    case T_WAIT:
              if(timer) timer--;
              else {
                devcnt=0;
                tempstate = T_OUTPUT;
              }
              break;
              
    case T_OUTPUT:  
              if(devcnt < noOfDS18Devices && devcnt < MAXONEWIRECNT) {
                // read by ROM address: bus indexes also count DS2438 devices
                temp = sensors.getTempC(tempsensors[devcnt].address);
                tempsensors[devcnt].temperature = round(temp*10);
                #ifdef tempDebug
                  COMM_DBG.print("Sensor temp ");
                  COMM_DBG.print(devcnt);
                  COMM_DBG.print(": ");
                  COMM_DBG.print(tempsensors[devcnt].temperature/10);
                  COMM_DBG.print(".");
                  COMM_DBG.print(tempsensors[devcnt].temperature%10);
                  COMM_DBG.println();  
                #endif
              }
              else {
                if (noOfDS2438Devices>0) {
                  tempstate = T_READVAD;
                  devDS2438Cnt = 0;
                }
                else tempstate = T_IDLE;
              }
              devcnt++;
              break;

    case T_READVAD:
              if (devDS2438Cnt<noOfDS2438Devices && devDS2438Cnt<MAXDS2438CNT) {
                bm.setAddress(voltsensors[devDS2438Cnt].address);
                  v=bm.readVAD();
                  voltsensors[devDS2438Cnt].vad=100*v; // 10mV resolution
                  #ifdef tempDebug
                    COMM_DBG.print("Sensor voltage vad ");
                    COMM_DBG.print(devDS2438Cnt);
                    COMM_DBG.print(": ");
                    COMM_DBG.print(voltsensors[devDS2438Cnt].vad/100);
                    COMM_DBG.print(".");
                    COMM_DBG.print(voltsensors[devDS2438Cnt].vad%100);
                    COMM_DBG.println();  
                  #endif
                devDS2438Cnt++;
              }
              else {
                tempstate = T_IDLE;
              }
              break;

    case T_SEARCH:
              // start new search
              if (substate == 0) {
                #ifdef tempDebug
                  COMM_DBG.println("New 1-wire search");
                #endif
                sensors.begin();
                substate = 1;
                noOfDS18Devices = 0;
                noOfDS2438Devices = 0;
              }
              // read device count
              else if (substate == 1) {
                noOfDevices = sensors.getDeviceCount();
                #ifdef tempDebug
                  COMM_DBG.print("Found "); 
                  COMM_DBG.print(noOfDevices, 10); 
                  COMM_DBG.println(" temp sensors:");
                #endif
                devcnt=0;
                substate = 2;
              }
              // get adress and values of all sensors
              // wdu todo ds2438
              else if (substate == 2) {
                setDeviceAddress();
                // sensor indexes of the valves refer to the new search order
                app_match_sensors();
                temp_cmd = TEMP_CMD_NONE;
                tempstate = T_IDLE;
              }
             
              break;

    default:  tempstate = T_IDLE;
              break;
  }

}


void get_sensordata (unsigned int index, char *buffer, int buflen) {

  DynamicJsonDocument doc(1024);      // buffer size should be enough for about 22 sensors
  String testjson;
  String AddressStr;
 
  const int count = noOfDS18Devices < MAXONEWIRECNT ? noOfDS18Devices : MAXONEWIRECNT;

  doc["cnt"] = count;

  for (int i=0; i<count; i++)
  {     
    doc["sns"][i]["temp"] = tempsensors[i].temperature;

    AddressStr = "";
    for (uint8_t x = 0; x < 8; x++)
    {
      if (tempsensors[i].address[x] < 16) AddressStr += '0';
      AddressStr += String(tempsensors[i].address[x], HEX);
      if (x<7) AddressStr += ' ';
    }
    doc["sns"][i]["add"] = AddressStr;
  }

  serializeJson(doc, buffer, buflen);
  doc.clear();
}

void temp_command(int command) {
  
  if (command == TEMP_CMD_LOCK) {
    lock = 1;
  }
  else if (command == TEMP_CMD_UNLOCK) {
    lock = 0;
  }
  else if (temp_cmd == TEMP_CMD_NONE) temp_cmd = command;

}
