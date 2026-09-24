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
#include "app.h"
#include "motor.h"
#include "communication.h"
#include "owDevices.h"
#include "eeprom.h"
#include "DallasTemperature.h"
#include "vdm/arg_parser.h"
#include "vdm/buf_writer.h"
#include "vdm/line_assembler.h"
#include "vdm/replies.h"
#include "vdm/settings.h"
#include "vdm/tokenizer.h"
#include <string.h>

// DEBUG
#ifdef commDebug
	#define commdbg_print(format, ...) COMM_DBG.print(format, ##__VA_ARGS__)
	#define commdbg_println(format, ...) COMM_DBG.println(format, ##__VA_ARGS__)
#else
	#define commdbg_print(format, ...)	(void)0
	#define commdbg_println(format, ...)	(void)0
#endif

#define COMM_LINE_SIZE			128			// longest v1 request (stvls) has less than 64 characters
#define COMM_MAX_LINES			4			// requests handled per communication_loop call
#define COMM_MAX_READ			512			// bytes taken from the UART per communication_loop call
#define NO_SENSOR_ADDRESS		"00-00-00-00-00-00-00-00"

static vdm::StaticLineAssembler<COMM_LINE_SIZE> commLine;


static void storeSensorAddress (struct ds1820_eeprom_layout &slot, const uint8_t *address)
{
	slot.familycode = address[0];
	for (uint8_t i = 0; i < 6; i++) slot.romcode[i] = address[1 + i];
	slot.crc = address[7];
}


// parses one "xx-xx-xx-xx-xx-xx-xx-xx" address into an EEPROM sensor slot
// valid addresses are stored, the all-zero address clears the slot, anything else is ignored
static void setValveIDSensor (const char *text, struct ds1820_eeprom_layout &slot)
{
	DeviceAddress address;

	if (!vdm::parseOneWireAddress(text, address)) return;
	if (vdm::isZeroAddress(address) || sensors.validAddress(address)) {
		storeSensorAddress(slot, address);
		eeprom_changed();
	}
}


int16_t comm_set_valve_sensors (uint16_t valve, const char *first, const char *second)
{
	if (valve >= ACTUATOR_COUNT) return -1;
	setValveIDSensor(first, eep_content.owsensors1[valve]);
	setValveIDSensor(second, eep_content.owsensors2[valve]);
	return 0;
}


int16_t comm_set_valve_sensor_index (uint16_t valve, uint8_t slot, uint16_t sensor)
{
	if (valve >= ACTUATOR_COUNT || sensor >= noOfDS18Devices || sensor >= MAXONEWIRECNT) return -1;

	if (slot == 1) {
		storeSensorAddress(eep_content.owsensors1[valve], tempsensors[sensor].address);
		myvalves[valve].sensorindex1 = sensor;
	}
	else if (slot == 2) {
		storeSensorAddress(eep_content.owsensors2[valve], tempsensors[sensor].address);
		myvalves[valve].sensorindex2 = sensor;
	}
	else return -1;

	eeprom_changed();
	return 0;
}


static void printSensorAddress (Print &out, const DeviceAddress &address)
{
	vdm::StaticBufWriter<vdm::kOneWireAddressTextLen + 1> text;

	if (text.appendOneWireAddress(address)) out.print(text.c_str());
	else out.print(NO_SENSOR_ADDRESS);
}


static void printValveSensorAddress (Print &out, unsigned int sensorindex)
{
	if (sensorindex < MAXONEWIRECNT) printSensorAddress(out, tempsensors[sensorindex].address);
	else out.print(NO_SENSOR_ADDRESS);
}


void comm_print_valve_sensor_ids (Print &out, uint16_t valve, char delimiter)
{
	if (valve >= ACTUATOR_COUNT) return;
	printValveSensorAddress(out, myvalves[valve].sensorindex1);
	out.print(delimiter);
	printValveSensorAddress(out, myvalves[valve].sensorindex2);
}


static int32_t valveTemperature (unsigned int sensorindex)
{
	if (sensorindex < MAXONEWIRECNT) return tempsensors[sensorindex].temperature;
	return -500;
}


void communication_setup (void) {
	
	// UART to ESP32
	COMM_SER.setRx(PA10);			//STM32F401 blackpill USART1 RX PA10
	COMM_SER.setTx(PA9);			//STM32F401 blackpill USART1 TX PA9
	COMM_SER.begin(115200, SERIAL_8N1);
	while(!COMM_SER);
	// drop bytes received with the wrong framing during the 8E1 boot window
	while (COMM_SER.available() > 0) COMM_SER.read();
	commLine.reset();
	//COMM_SER.println("alive");COMM_SER.flush();
	#ifdef commDebug
		COMM_DBG.print("SERIAL_BUFFER_SIZE TX=");
		COMM_DBG.print(SERIAL_TX_BUFFER_SIZE);
		COMM_DBG.print(" RX=");
		COMM_DBG.println(SERIAL_RX_BUFFER_SIZE);
	#endif
}


static void communication_dispatch (const vdm::Tokenizer &req)
{
	uint16_t	x = 0;
	uint32_t	xu32 = 0;
	uint16_t	y = 0;
	uint8_t		pos = 0;

	// set target position
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	if(req.is(APP_PRE_SETTARGETPOS)) {
		commdbg_println("set target pos");

		if (req.argc() == 2 && req.argU16(0, 0, ACTUATOR_COUNT - 1, x) && req.argU8(1, 0, 100, pos)) {
			if (!myvalvemots[x].calibration)  // wdu ???
				myvalvemots[x].target_position = pos;
			COMM_SER.println(APP_PRE_SETTARGETPOS);
		}
		else commdbg_println("invalid arguments");
	}


	// get target position
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETTARGETPOS)) {
		commdbg_println("get target pos");

		if (req.argc() == 1 && req.argU16(0, 0, ACTUATOR_COUNT - 1, x)) {
			COMM_SER.print(APP_PRE_GETTARGETPOS);
			COMM_SER.print(" ");
			COMM_SER.print(x, DEC);
			COMM_SER.print(" ");
			COMM_SER.print(myvalvemots[x].target_position, DEC);
			COMM_SER.println(" ");
		}
		else commdbg_println("invalid arguments");
	}


	// get valve data (actual position, meancurrent, status, temperatures, movements, counts)
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETVLVDATA)) {
		if (req.argc() == 1 && req.argU16(0, 0, ACTUATOR_COUNT - 1, x)) {
			vdm::ValveDataReply data;
			vdm::StaticBufWriter<vdm::kValveDataReplyMaxLen + 1> sendbuffer;
			uint8_t status = myvalvemots[x].status;

			if (myvalvemots[x].calibration) status |= 0x80;

			data.index = x;
			data.actualPosition = myvalvemots[x].actual_position;
			data.meanCurrent = (int32_t) myvalvemots[x].meancurrent;
			data.status = status;
			data.temperature1 = valveTemperature(myvalves[x].sensorindex1);
			data.temperature2 = valveTemperature(myvalves[x].sensorindex2);
			data.movements = (int32_t) myvalves[x].movements;
			data.openingCount = (int32_t) myvalvemots[x].opening_count;
			data.closingCount = (int32_t) myvalvemots[x].closing_count;
			data.deadzoneCount = myvalvemots[x].deadzone_count;
			data.calibRetries = myvalvemots[x].calibRetries;

			if (vdm::formatValveData(sendbuffer, APP_PRE_GETVLVDATA, data)) COMM_SER.println(sendbuffer.c_str());
		}
		else commdbg_println("gvlvd: invalid arguments");
	}

	// get valve status of all valves
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETVLSTATUS)) {
		uint8_t status[ACTUATOR_COUNT];
		vdm::StaticBufWriter<64> sendbuffer;

		commdbg_println("cmd: get valve status");
		for (uint8_t xx = 0; xx < ACTUATOR_COUNT; xx++) status[xx] = myvalvemots[xx].status;
		if (vdm::formatStatusList(sendbuffer, APP_PRE_GETVLSTATUS, status, ACTUATOR_COUNT))
			COMM_SER.println(sendbuffer.c_str());
	}

	// get temp onewire sensor count
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETONEWIRECNT)) {
		const uint8_t count = noOfDS18Devices < MAXONEWIRECNT ? noOfDS18Devices : MAXONEWIRECNT;

		if (req.argc() == 0) {
			COMM_SER.print(APP_PRE_GETONEWIRECNT);
			COMM_SER.print(" ");			
			COMM_SER.print(count, DEC);
			COMM_SER.println(" ");
		}	
		else if (req.argc() == 1 && req.argU16(0, 0, 65535, x) && x == 255) {
			// get all onewire detected sensor
			COMM_SER.print(APP_PRE_GETONEWIRECNT);
			COMM_SER.print(" ");		
			COMM_SER.print(count, DEC);
			if (count > 0) COMM_SER.print(" ");
			for (uint8_t i = 0; i < count; i++) {
				printSensorAddress(COMM_SER, tempsensors[i].address);
				if (i < count - 1) COMM_SER.print(",");
			}	
			COMM_SER.println(" ");
		}
	}

	// get temp onewire sensor data of sensor x (address and temperature)
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETONEWIREDATA)) {
		COMM_SER.print(APP_PRE_GETONEWIREDATA);
		COMM_SER.print(" ");
		if (req.argc() == 1 && req.argU16(0, 0, MAXONEWIRECNT - 1, x)) {
			printSensorAddress(COMM_SER, tempsensors[x].address);
			COMM_SER.print(" ");
			COMM_SER.print(tempsensors[x].temperature, DEC);
		}
		else COMM_SER.print("0");
		COMM_SER.println(" ");			
	}


	// get 1st and 2nd onewire sensor address for valve x
	// example answer: gvlon 1 28-84-37-94-97-FF-03-23 00-00-00-00-00-00-00-00
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETONEWIRESETT)) {
		commdbg_print("cmd: get 1st and 2nd onewire sensor addresses");

		if (req.argc() == 1 && req.argU16(0, 0, ACTUATOR_COUNT - 1, x)) {
			COMM_SER.print(APP_PRE_GETONEWIRESETT);
			COMM_SER.print(" ");
			// valve index
			COMM_SER.print(x, DEC);
			COMM_SER.print(" ");
			comm_print_valve_sensor_ids(COMM_SER, x, ' ');
			COMM_SER.println(" ");
		}
		else if (req.argc() == 1 && req.argU16(0, 255, 255, x)) {
			COMM_SER.print(APP_PRE_GETONEWIRESETT);
			COMM_SER.print(" ");
			COMM_SER.print(ACTUATOR_COUNT, DEC);
			COMM_SER.print(" ");
			for (uint8_t i = 0; i < ACTUATOR_COUNT; i++) {
				comm_print_valve_sensor_ids(COMM_SER, i, ',');
				if (i < ACTUATOR_COUNT - 1) COMM_SER.print(",");
			}
			COMM_SER.println(" ");
		}
		else {
			commdbg_println(" - error");
			// v1 reports this error with the goned prefix, the ESP relies on it
			COMM_SER.print(APP_PRE_GETONEWIREDATA);
			COMM_SER.println(" error ");
		}
	}

	// get volt onewire sensor count
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETOWVOLTCNT)) {
		const uint8_t count = noOfDS2438Devices < MAXDS2438CNT ? noOfDS2438Devices : MAXDS2438CNT;

		if (req.argc() == 0) {
			COMM_SER.print(APP_PRE_GETOWVOLTCNT);
			COMM_SER.print(" ");			
			COMM_SER.print(count, DEC);
			COMM_SER.println(" ");
		}	
		else if (req.argc() == 1 && req.argU16(0, 0, 65535, x) && x == 255) {
			// get all onewire detected sensor
			COMM_SER.print(APP_PRE_GETOWVOLTCNT);
			COMM_SER.print(" ");		
			COMM_SER.print(count, DEC);
			if (count > 0) COMM_SER.print(" ");
			for (uint8_t i = 0; i < count; i++) {
				printSensorAddress(COMM_SER, voltsensors[i].address);
				if (i < count - 1) COMM_SER.print(",");
			}	
			COMM_SER.println(" ");
		}
	}

	// get volt onewire sensor data of sensor x (address and voltage)
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETOWVOLTDATA)) {
		COMM_SER.print(APP_PRE_GETOWVOLTDATA);
		COMM_SER.print(" ");
		if (req.argc() == 1 && req.argU16(0, 0, MAXDS2438CNT - 1, x)) {
			printSensorAddress(COMM_SER, voltsensors[x].address);
			COMM_SER.print(" ");
			COMM_SER.print(voltsensors[x].vad, DEC);
		}
		else COMM_SER.print("0");
		COMM_SER.println(" ");			
	}


	// start new onewire sensor search
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETONEWIRESEARCH)) {
		commdbg_println("start new 1-wire search");
		temp_command(TEMP_CMD_NEWSEARCH);
		COMM_SER.println(APP_PRE_SETONEWIRESEARCH);
	}
	

	// set valve learning time
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETLEARNTIME)) {
		commdbg_print("set valve learning time to ");

		if (req.argc() == 1 && req.argU32(0, 0, UINT32_MAX, xu32) && app_set_learntime(xu32) == 0) {
			COMM_SER.println(APP_PRE_SETLEARNTIME);
			commdbg_println(xu32, DEC);
		}
		else commdbg_println("- error");
	}


	// set valve learning movements
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETLEARNMOVEM)) {
		commdbg_print("set valve learning movements to ");

		// the ESP sends a uint32; v1 always replied, so cap instead of rejecting large counts
		const bool valid = req.argc() == 1 && req.argU32(0, 0, UINT32_MAX, xu32);
		if (valid) x = vdm::capLearnMovements(xu32);

		if (valid && app_set_learnmovements(x) == 0) {
			commdbg_println(x, DEC);
			eep_content.numberOfMovements=x;
			eeprom_changed();
			COMM_SER.println(APP_PRE_SETLEARNMOVEM);
		}
		else commdbg_println("- error");
	}

	// get valve learning movements
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETLEARNMOVEM)) {
		commdbg_print("get valve learning movements");
		COMM_SER.print(APP_PRE_GETLEARNMOVEM);
		COMM_SER.print(" ");			
		COMM_SER.print(learning_movements, DEC);
		COMM_SER.println(" ");			
	}


	// ESPalive
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is("ESPalive")) {
		commdbg_println("received ESPalive");
	}


	// set first / second sensor index of valve
	// x - valve index
	// y - temp sensor index (position in the gonec list)
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SET1STSENSORINDEX) || req.is(APP_PRE_SET2NDSENSORINDEX)) {
		const bool first = req.is(APP_PRE_SET1STSENSORINDEX);

		commdbg_println(first ? "comm: set 1st sensor index" : "comm: set 2nd sensor index");
		if (req.argc() == 2 && req.argU16(0, 0, ACTUATOR_COUNT - 1, x) && req.argU16(1, 0, MAXONEWIRECNT - 1, y)
			&& comm_set_valve_sensor_index(x, first ? 1 : 2, y) == 0) {
			COMM_SER.println(first ? APP_PRE_SET1STSENSORINDEX : APP_PRE_SET2NDSENSORINDEX);
		}
		else commdbg_println("invalid arguments");
	}


	// set valve sensors
	// x - valve index
	// arg1 - 8 byte hex address of 1st 1-wire sensor
	// arg2 - 8 byte hex address of 2nd 1-wire sensor
	// if hex address == 00-00-00-00-00-00-00-00 this sensor will be ignored
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETVLVSENSOR)) {
		if (req.argc() == 3 && req.argU16(0, 0, ACTUATOR_COUNT - 1, x)) {
			commdbg_println("comm: set valve sensors");
			comm_set_valve_sensors(x, req.arg(1), req.arg(2));
			COMM_SER.print(APP_PRE_SETVLVSENSOR);
			COMM_SER.print(" ");
			COMM_SER.println(x, DEC);
		}
		else commdbg_println("invalid arguments");
	}


	// open valve request (255 = all valves)
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETALLVLVOPEN)) {
		commdbg_print("got open valve request for ");

		if (req.argc() == 1 && req.argU16(0, 0, 65535, x) && app_set_valveopen(x) == 0) {
			COMM_SER.print(APP_PRE_SETALLVLVOPEN);
			COMM_SER.println(" ");
			commdbg_println(x, DEC);
		}
		else commdbg_println("- error");
	}


	// learn valve x request
	// if x is 255 all valves will be learned
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETVLLEARN)) {
		commdbg_print("start learning for valve ");

		if (req.argc() == 1 && req.argU16(0, 0, 65535, x) && app_set_valvelearning(x) == 0) {
			COMM_SER.println(APP_PRE_SETVLLEARN);
			commdbg_println(x, DEC);
		}
		else commdbg_println("- error");
	}

	// set motor characteristics
	// low high startOnPower [noOfMinCounts [maxCalibRetries]]
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETMOTCHARS)) {
		uint8_t low = 0, high = 0, onPower = 0, retries = 0;
		uint16_t minCounts = 0;

		commdbg_print("got set motor characteristics request ");

		if (req.argc() >= 3
			&& req.argU8(0, 5, 50, low)
			&& req.argU8(1, 5, 50, high)
			&& req.argU8(2, 0, 255, onPower)
			&& (req.argc() < 4 || req.argU16(3, 0, 65535, minCounts))
			&& (req.argc() < 5 || req.argU8(4, 0, 255, retries))) {

			currentbound_low_fac = low;
			currentbound_high_fac = high;
			startOnPower = onPower;

			eep_content.currentbound_low_fac = currentbound_low_fac;
			eep_content.currentbound_high_fac = currentbound_high_fac;
			eep_content.startOnPower = startOnPower;
			if (req.argc() >= 4) {
				noOfMinCounts = minCounts;
				eep_content.noOfMinCounts = noOfMinCounts;	
			}
			if (req.argc() == 5) {
				maxCalibRetries = retries;
				eep_content.maxCalibRetries = maxCalibRetries;	
			}
			eeprom_changed();
			COMM_SER.println(APP_PRE_SETMOTCHARS);
			commdbg_println("- valid");
		}
		else commdbg_println("- invalid arguments");
	}


	// get motor characteristics
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETMOTCHARS)) {
		commdbg_println("got get motor characteristics request ");
		COMM_SER.print(APP_PRE_GETMOTCHARS);
		COMM_SER.print(" ");			
		COMM_SER.print(currentbound_low_fac, DEC);
		COMM_SER.print(" ");		
		COMM_SER.print(currentbound_high_fac, DEC);
		COMM_SER.print(" ");		
		COMM_SER.print(startOnPower, DEC);
		COMM_SER.print(" ");		
		COMM_SER.print(noOfMinCounts, DEC);
		COMM_SER.print(" ");		
		COMM_SER.print(maxCalibRetries, DEC);
		COMM_SER.println(" ");			
	} 


	// detect valve status
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETDETECTVLV)) {
		commdbg_print("got detect valve status request");

		if (req.argc() == 1 && req.argU16(0, 0, 65535, x)) {
			// reset status of all valves so they will be detected again
			if (x == 255) {
				app_scan_valves();
				commdbg_println(" - reset all valves");
			}
			else commdbg_println(" - error");
			
			COMM_SER.print(APP_PRE_SETDETECTVLV);
			COMM_SER.println(" ");
		}
		else commdbg_println(" - error");		
	} 


	// get version request
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETVERSION)) {
		commdbg_println("got version request");

		COMM_SER.print(APP_PRE_GETVERSION);
		COMM_SER.print(" ");			

		COMM_SER.print(FIRMWARE_VERSION);
		#ifdef HARDWARE_VERSION
			COMM_SER.print("_");
			COMM_SER.print(HARDWARE_VERSION);
		#endif

		#ifdef FIRMWARE_BUILD
			COMM_SER.print(" ");
			COMM_SER.print(FIRMWARE_BUILD);
		#endif

		COMM_SER.println(" ");	
	}

	// get hw Info request
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETHWINFO)) {
		commdbg_println("got hw info request");
		COMM_SER.print(APP_PRE_GETHWINFO);
		COMM_SER.print(" ");			
		// read ID
		COMM_SER.print(HAL_GetDEVID());
		COMM_SER.println(" ");	
	}


	// match sensors request
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_MATCHSENS)) {
		commdbg_println("got match sensors request");

		app_match_sensors();

		COMM_SER.print(APP_PRE_MATCHSENS);
		COMM_SER.println(" ");
	}


	// software reset request
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SOFTRESET)) {
		commdbg_println("got software reset request");

		COMM_SER.print(APP_PRE_SOFTRESET);
		COMM_SER.println(" ");

		delay(200);

		reset_STM32();
	}

	// get eeprom state
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_EEPSTATE)) {
		commdbg_println("got get eeprom status request ");
		COMM_SER.print(APP_PRE_EEPSTATE);
		COMM_SER.print(" ");			
		COMM_SER.print(eeprom_free(), DEC);
		COMM_SER.println(" ");			
	} 

	// unknown commands are ignored without reply (protocol v1)
}


/**
  * @brief  Handler: reads bytes from the ESP32 and executes complete requests
  * @param  None
  * @retval 0 if at least one request was executed, otherwise -1
  */
int16_t communication_loop (void) {
	int16_t result = -1;
	uint16_t budget = COMM_MAX_READ;

	for (uint8_t n = 0; n < COMM_MAX_LINES; n++) {
		// bytes following a complete line stay in the UART buffer for the next request
		while (!commLine.hasLine() && budget > 0 && COMM_SER.available() > 0) {
			commLine.push((char) COMM_SER.read());
			budget--;
		}

		char *line = commLine.takeLine();
		if (line == NULL) break;

		vdm::Tokenizer req;
		if (req.parse(line, NO_OF_ARGS)) {
			communication_dispatch(req);
			result = 0;
		}
		else if (req.tooManyArgs()) commdbg_println("comm: too many arguments");
		commLine.release();
	}

	return result;
}
