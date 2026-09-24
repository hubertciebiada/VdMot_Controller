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
#include "terminal.h"
#include "motor.h"
#include "owDevices.h"
#include "communication.h"
#include "eeprom.h"
#include "app.h"
#include "vdm/line_assembler.h"
#include "vdm/tokenizer.h"


//extern HardwareSerial Serial3;
extern void callback_app(char cmd, byte valveindex, byte pos);

void WriteEEPROMBaselayout(void);

//HardwareSerial Serial3(USART3);
HardwareSerial Serial6(USART6);


//#define COMM_DBG				Serial3		// serial port for debugging
//#define COMM_DBG				Serial6		// serial port for debugging


int testmode = 0;							// flag for testmode

//#define COMM_DBG				Serial3		// serial port for debugging
//#define COMM_DBG				Serial6		// serial port for debugging

#define TERM_ARG_CNT			 3			// number of allowed command arguments
#define TERM_LINE_SIZE			128			// max length of one terminal line
#define TERM_MAX_READ			256			// bytes taken from the terminal per call

static int16_t Terminal_Execute (const vdm::Tokenizer &req);

int16_t Terminal_Init (void) {

	Serial6.setRx(PA12);			//STM32F401 blackpill USART6 RX PA12
	Serial6.setTx(PA11);			//STM32F401 blackpill USART6 TX PA11
	
	COMM_DBG.begin(115200);
	while(!COMM_DBG);
	COMM_DBG.print("VdMot Controller "); 
	COMM_DBG.print(FIRMWARE_VERSION);

	#ifdef HARDWARE_REVISION_C1
		COMM_DBG.println("_C1");
	#elif HARDWARE_REVISION_C2
		COMM_DBG.println("_C2");
	#else
		error "no hardware revision defined"
	#endif

	COMM_DBG.flush();

	testmode = 0;

	return 0;
}


/**
  * @brief  Handler
  * @param  None
  * @retval command code, -1 if no complete line was received
  */
int16_t Terminal_Serve (void) {
	static vdm::StaticLineAssembler<TERM_LINE_SIZE> termLine;
	uint16_t budget = TERM_MAX_READ;

	while (!termLine.hasLine() && budget > 0 && COMM_DBG.available() > 0) {
		termLine.push((char) COMM_DBG.read());
		budget--;
	}

	char *line = termLine.takeLine();
	if (line == NULL) return -1;

	vdm::Tokenizer req;
	const bool parsed = req.parse(line, TERM_ARG_CNT);
	int16_t result = -1;

	if (parsed) result = Terminal_Execute(req);
	else if (req.tooManyArgs()) COMM_DBG.println("too many arguments");

	termLine.release();
	return result;
}


static int16_t Terminal_Execute (const vdm::Tokenizer &req) {
	uint16_t		x = 0;
	uint32_t		xu32 = 0;
	uint16_t		y = 0;

	// most commands take up to two numeric arguments
	const bool hasX = req.argc() >= 1 && req.argU16(0, 0, 65535, x);
	const bool hasY = req.argc() >= 2 && req.argU16(1, 0, 65535, y);

	// help
	if(req.is("help")) {
		COMM_DBG.println("Help:");
		COMM_DBG.println("*********************");
		
		return CMD_HELP;
	}

	// learn
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is("learn")) {
		if(req.argc() == 1 && hasX) {								
			if (appsetaction(CMD_A_LEARN,x,0)!=0) COMM_DBG.println("valve machine command not accepted");				
		}
		else COMM_DBG.println("to few arguments");

		return CMD_LEARN;
	}

	// open to value
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is("open")) {
		if(req.argc() == 2 && hasX && hasY && y <= 100) {								
			if (appsetaction(CMD_A_OPEN,x,(byte)y)!=0) COMM_DBG.println("valve machine not idle");
		}
		else COMM_DBG.println("to few arguments");

		return CMD_OPEN;
	}

	// close to value
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is("close")) {
		if(req.argc() == 2 && hasX && hasY && y <= 100) {								
			if (appsetaction(CMD_A_CLOSE,x,(byte)y)!=0) COMM_DBG.println("valve machine not idle");
		}
		else COMM_DBG.println("to few arguments");

		return CMD_CLOSE;
	}

	// set target value
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is("settar")) {
		if(req.argc() == 2 && hasX && hasY)
		{								
			if(x < ACTUATOR_COUNT && y<=100)
			{
				myvalvemots[x].target_position = y;
				COMM_DBG.print("set valve "); COMM_DBG.print(x, 10); COMM_DBG.print(" to "); COMM_DBG.println(y);
			}
		}
		else COMM_DBG.println("to few arguments");

		return CMD_CLOSE;
	}


	// set muxer
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is("smux")) {
		if(req.argc() == 1 && hasX) {
			if(x==0) MUX_OFF();
			else MUX_ON();
		}
	}

	// set direction
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is("sdir")) {
		if(req.argc() == 1 && hasX) {
			if(x==0) DIR_OFF();
			else DIR_ON();
		}
	}

	// set enable
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is("sena")) {
		if(req.argc() == 2 && hasX && hasY) {
			if(x==0) {
				if(y==0) ENA0_OFF();
				else ENA0_ON();
			}
			else if (x==1) {
				if(y==0) ENA1_OFF();
				else ENA1_ON();
			}
			else if (x==2) {
				if(y==0) ENA2_OFF();
				else ENA2_ON();
			}
			else if (x==3) {
				if(y==0) ENA3_OFF();
				else ENA3_ON();
			}
			else if (x==4) {
				if(y==0) ENA4_OFF();
				else ENA4_ON();
			}
			else if (x==5) {
				if(y==0) ENA5_OFF();
				else ENA5_ON();
			}
		}
	}

	// set test mode
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is("stm")) {
		if(req.argc() == 1 && hasX) {
			if(x==0) { testmode = 0; COMM_DBG.println("testmode off"); }
			else {testmode = 1; COMM_DBG.println("testmode on"); }
		}
	}

	// get onewire sensor data - sensor count and data and adress of all connected sensors
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is("getone")) {
		print_sensordata(COMM_DBG);
	}

	// set eeprom layout
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is("seteep")) {
		WriteEEPROMBaselayout();
		COMM_DBG.println("set eeprom layout");
	}

	// save eeprom layout
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is("saveep")) {
		//eeprom_write_layout(&eep_content);
		eeprom_changed(EEP_CHANGED_ALL);
		COMM_DBG.println("saved eeprom layout");
	}

	// set sensor index first / second sensor
	// x - valve index
	// y - temp sensor index
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SET1STSENSORINDEX) || req.is(APP_PRE_SET2NDSENSORINDEX)) {
		const bool first = req.is(APP_PRE_SET1STSENSORINDEX);

		if(req.argc() == 2 && hasX && hasY) {				
			COMM_DBG.println(first ? "comm: set 1st sensor index" : "comm: set 2nd sensor index");
			if (comm_set_valve_sensor_index(x, first ? 1 : 2, y) != 0) COMM_DBG.println("invalid valve or sensor index");
		}
		else COMM_DBG.println("to few arguments");
	}


	// set valve sensors
	// arg0 - valve index
	// arg1 - 8 byte hex address of 1st 1-wire sensor
	// arg2 - 8 byte hex address of 2nd 1-wire sensor
	// if hex address == 00-00-00-00-00-00-00-00 the sensor slot is cleared
	// example: 'stvls 1 00-00-00-00-00-00-00-00 00-00-00-00-00-00-00-00 '
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETVLVSENSOR)) {
		COMM_DBG.println("set valve sensors by address");

		if(req.argc() == 3 && hasX) {				
			if (comm_set_valve_sensors(x, req.arg(1), req.arg(2)) == 0) {
				// match sensor address to valve struct at runtime, otherwise restart needed
				app_match_sensors();
			}
			else COMM_DBG.println("invalid valve index");
		}
		else COMM_DBG.println("to few arguments");
	}


	// get 1st and 2nd onewire sensor address for valve x
	// example answer: gvlon 1 28-84-37-94-97-FF-03-23 00-00-00-00-00-00-00-00
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETONEWIRESETT)) {
		COMM_DBG.print("cmd: get 1st and 2nd onewire sensor addresses");

		if(req.argc() == 1 && hasX && x < ACTUATOR_COUNT) {
			// answer begin
			COMM_DBG.print(" - ");
			COMM_DBG.print(APP_PRE_GETONEWIRESETT);
			COMM_DBG.print(" ");

			// valve index
			COMM_DBG.print(x, DEC);
			COMM_DBG.print(" ");
			comm_print_valve_sensor_ids(COMM_DBG, x, ' ');

			// finish answer
			COMM_DBG.println(" ");
		}
		else {
			COMM_DBG.println(" - error");		
		}			
	}


	// get version request
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is("gvers")) {			
		COMM_DBG.print("Version: ");			
		COMM_DBG.println(FIRMWARE_VERSION);
	}


	// start new onewire sensor search
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETONEWIRESEARCH)) {
		COMM_DBG.print("start new 1-wire search");
		temp_command(TEMP_CMD_NEWSEARCH);
	}


	// set valve learning time
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETLEARNTIME)) {
		COMM_DBG.print("set valve learning time to ");

		if(req.argc() == 1 && req.argU32(0, 0, UINT32_MAX, xu32)) {
			if( app_set_learntime(xu32) == 0) COMM_DBG.println(xu32, DEC);
			else COMM_DBG.println("- error");
		}
		else {
			COMM_DBG.println("- error");
		}
	}


	// open all valves request
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETALLVLVOPEN)) {
		COMM_DBG.print("got open valve request for ");

		if(req.argc() == 1 && hasX) {
			if( app_set_valveopen(x) == 0) COMM_DBG.println(x, DEC);
			else COMM_DBG.println("- error");
		}
		else {
			COMM_DBG.println("- error");
		}
	}


	// learn valve x request
	// if x is 255 all valves will be learned
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETVLLEARN)) {
		COMM_DBG.print("start learning for valve ");

		if(req.argc() == 1 && hasX) {
			if( app_set_valvelearning(x) == 0) COMM_DBG.println(x, DEC);
			else COMM_DBG.println("- error");
		}
		else {
			COMM_DBG.println("- error");
		}
	}


	// set motor characteristics
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETMOTCHARS)) {
		COMM_DBG.print("got set motor characteristics request ");

		if(req.argc() == 2 && hasX && hasY) {
			vdm::MotorParams params = motor_get_params();
			const uint32_t values[5] = {x, y, params.startOnPower, 0, 0};

			if (vdm::applyMotorParamsRequest(params, 3, values) == vdm::ParamsRequest::Applied) {
				motor_set_params(params);
				eeprom_changed(EEP_CHANGED_MOTOR);
				COMM_DBG.println("- valid");
			}
			else COMM_DBG.println("- values out of bounds");
		}
		else {
			COMM_DBG.println("- error");
		}
	}


	// get motor characteristics
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETMOTCHARS)) {
		COMM_DBG.print("got get motor characteristics request - low: ");			
		COMM_DBG.print(currentbound_low_fac, DEC);
		COMM_DBG.println(" high: ");		
		COMM_DBG.println(currentbound_high_fac, DEC);			
	} 


	// detect valve status
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETDETECTVLV)) {
		#ifdef commDebug 
			COMM_DBG.print("got detect valve status request");
		#endif

		if(req.argc() == 1 && hasX) {
			// reset status of all valves so they will be detected again
			if( x == 255) {
				app_scan_valves();
				COMM_DBG.println(" - reset all valves");
			}
			// not supported at the moment, app rework needed
			// reset status of valve x so it will be detected again
			// else if (x < ACTUATOR_COUNT) {
			// 	myvalvemots[x].status = VLV_STATE_UNKNOWN;
			// 	COMM_DBG.print(" - reset valve ");
			// 	COMM_DBG.println(x, DEC);
			// }
			else COMM_DBG.println(" - error");
			
			COMM_SER.print(APP_PRE_SETDETECTVLV);
			COMM_SER.println(" ");
		}
		else COMM_DBG.println(" - error");		
	} 

	// unknown command
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else {
		//sprintf(&PrtBuf[0],"unknown command: %s\r",&cmdbuf[0]);
		//USART1_PutStr(&PrtBuf[0]);
		COMM_DBG.println("unknown command");
		return CMD_NONE;
	}

	return 0;
}


/**
  * @brief  Write EEPROM Baselayout
  * @param  None
  * @retval None
  */
void WriteEEPROMBaselayout(void) {

	COMM_DBG.println("write EEPROM layout...");

	eeprom_fill ();		// write eeprom mark

	eep_content.b_slave = 0;
	strncpy(eep_content.descr, SYSTEM_NAME, sizeof(eep_content.descr));
	eep_content.OneWireCfg[0] = 0;
	eep_content.OneWireCfg[1] = 0;
	eep_content.OneWireCfg[2] = 0;

//	eep.sensors[0].romcode[0] = 1;
//	eep.sensors[0].romcode[1] = 2;
//	eep.sensors[0].romcode[2] = 3;
//	eep.sensors[0].romcode[3] = 4;
//	eep.sensors[0].romcode[4] = 5;
//	eep.sensors[0].romcode[5] = 6;
//	eep.sensors[0].cfg = 0x55;
//	//eep.sensors[0].errors = 0x2FF2;
//	eep.sensors[0].crc = 0x11;
//	eep.sensors[0].cfg = 0x2222;
//	strcpy(eep.sensors[0].descr, "Sensor 1");
//	eep.sensors[0].modbusreg = 0x1111;

	eeprom_write_layout (&eep_content);
}
