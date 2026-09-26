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
#include "sysstat.h"
#include "DallasTemperature.h"
#include "vdm/arg_parser.h"
#include "vdm/buf_writer.h"
#include "vdm/failsafe.h"
#include "vdm/lease.h"
#include "vdm/line_assembler.h"
#include "vdm/replies.h"
#include "vdm/replies_v2.h"
#include "vdm/replies_v3.h"
#include "vdm/settings.h"
#include "vdm/tokenizer.h"
#include "vdm/uart_errors.h"
#include "vdm/valve_codes.h"
#include <stddef.h>
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
#define COMM_LINE_TIMEOUT_MS	100			// an unterminated line is dropped after this idle time (a request takes ~10 ms)
#define NO_SENSOR_ADDRESS		"00-00-00-00-00-00-00-00"
#define ALL_VALVES				255			// sfspo, sstop: every valve

// The board revision this image was built for. The ESP flasher looks for this string in a firmware
// file before it flashes it (one marker per image), gvers reports the tag from the same array.
extern const char kHardwareMarker[] __attribute__((used)) = HARDWARE_MARKER_PREFIX HARDWARE_REVISION_TAG;

static vdm::StaticLineAssembler<COMM_LINE_SIZE> commLine;
static uint32_t commLastByteMs = 0;			// when commLine consumed its last byte
static uint32_t commTooManyArgs = 0;		// requests dropped for too many arguments (gstat parseErr)
static vdm::UartErrorCounters commUartErrors;	// written by the USART1 RX interrupt

// longest reply (gprof), static to keep it off the main loop stack
static vdm::StaticBufWriter<vdm::kProfileReplyMaxLen + 1> replyLine;
static_assert(vdm::kValveExtV3ReplyMaxLen <= vdm::kProfileReplyMaxLen, "gvlvy fits replyLine");
static_assert(vdm::kStatV3ReplyMaxLen <= vdm::kProfileReplyMaxLen, "gstax fits replyLine");

static_assert(vdm::kUartErrorParity == HAL_UART_ERROR_PE, "HAL parity error bit");
static_assert(vdm::kUartErrorNoise == HAL_UART_ERROR_NE, "HAL noise error bit");
static_assert(vdm::kUartErrorFraming == HAL_UART_ERROR_FE, "HAL framing error bit");
static_assert(vdm::kUartErrorOverrun == HAL_UART_ERROR_ORE, "HAL overrun error bit");


// sends the reply formatted into replyLine (nothing if it did not fit)
static void sendReply (bool formatted)
{
	if (formatted) COMM_SER.println(replyLine.c_str());
	replyLine.clear();
}


// RX interrupt of the ESP UART: the HAL sets the error code of a byte before it hands the byte
// over. The errors and the bytes that find the ring full are counted, then the core's handler
// stores the byte.
static void comm_rx_irq (serial_t *obj)
{
	const bool ringFull = (obj->rx_head + 1) % SERIAL_RX_BUFFER_SIZE == obj->rx_tail;
	vdm::countUartErrors(commUartErrors, obj->handle.ErrorCode, ringFull);
	HardwareSerial::_rx_complete_irq(obj);
}


// stores a sensor address in the EEPROM sensor slot `stored` (slot number s for the EEPROM, see
// eeprom_changed_slot); the EEPROM is marked only when the slot changes
static void storeSensorAddress (struct ds1820_eeprom_layout &stored, uint8_t s, const uint8_t *address)
{
	struct ds1820_eeprom_layout slot;

	slot.familycode = address[0];
	memcpy(slot.romcode, address + 1, sizeof slot.romcode);
	slot.crc = address[7];
	if (memcmp(&stored, &slot, sizeof(slot)) == 0) return;
	stored = slot;
	eeprom_changed_slot(s);
}


// parses one "xx-xx-xx-xx-xx-xx-xx-xx" address into an EEPROM sensor slot
// valid addresses are stored, the all-zero address clears the slot, anything else is ignored
static void setValveIDSensor (const char *text, struct ds1820_eeprom_layout &slot, uint8_t s)
{
	DeviceAddress address;

	if (!vdm::parseOneWireAddress(text, address)) return;
	if (vdm::isZeroAddress(address) || sensors.validAddress(address)) storeSensorAddress(slot, s, address);
}


int16_t comm_set_valve_sensors (uint16_t valve, const char *first, const char *second)
{
	if (valve >= ACTUATOR_COUNT) return -1;
	setValveIDSensor(first, eep_content.owsensors1[valve], valve);
	setValveIDSensor(second, eep_content.owsensors2[valve], ACTUATOR_COUNT + valve);
	return 0;
}


int16_t comm_set_valve_sensor_index (uint16_t valve, uint8_t slot, uint16_t sensor)
{
	if (valve >= ACTUATOR_COUNT || sensor >= noOfDS18Devices || sensor >= MAXONEWIRECNT) return -1;

	if (slot == 1) {
		storeSensorAddress(eep_content.owsensors1[valve], valve, tempsensors[sensor].address);
		myvalves[valve].sensorindex1 = sensor;
	}
	else if (slot == 2) {
		storeSensorAddress(eep_content.owsensors2[valve], ACTUATOR_COUNT + valve, tempsensors[sensor].address);
		myvalves[valve].sensorindex2 = sensor;
	}
	else return -1;

	return 0;
}


int16_t comm_set_learntime (uint32_t seconds)
{
	if (app_set_learntime(seconds) != 0) return -1;
	if (eep_content.learnTimeS != seconds) {
		eep_content.learnTimeS = seconds;
		eeprom_changed(EEP_CHANGED_LEARNTIME);
	}
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


// argument i is a valve index or 255 (every valve)
static bool argValveOrAll (const vdm::Tokenizer &req, uint8_t i, uint16_t &x)
{
	return req.argU16(i, 0, ACTUATOR_COUNT - 1, x) || req.argU16(i, ALL_VALVES, ALL_VALVES, x);
}


// stores the failsafe position of one valve or of all (255) and applies it; while the EEPROM read
// has failed the mirror holds the default 50, not the stored positions, so every position is
// marked: the re-read takes over the marked fields only
static void setFailsafe (uint16_t valve, uint8_t pct)
{
	bool changed = false;

	for (uint8_t v = 0; v < ACTUATOR_COUNT; v++) {
		if ((valve == v || valve == ALL_VALVES) && eep_content.failsafePct[v] != pct) {
			eep_content.failsafePct[v] = pct;
			changed = true;
		}
	}
	if (changed || eeprom_state() == vdm::kEepStateReadFailed) eeprom_changed(EEP_CHANGED_FAILSAFE);
	app_set_failsafe(valve, pct);
}


// the gvlvx values of valve x (also the first ones of gvlvy)
static void fillValveExt (uint16_t x, vdm::ValveExtReply &data)
{
	struct valve_snapshot snap;

	// one copy: a calibration pass or the end of a move changes several fields at once
	valve_get_snapshot(x, snap);
	const bool requested = app_learn_pending(x, snap.status, snap.calibration != 0);

	data.index = (uint8_t) x;
	data.status = vdm::encodeValveStatus(snap.status, snap.calibration != 0);
	data.position = snap.actual_position;
	data.target = snap.target_position;
	data.meanCurrent = (uint16_t) (snap.meancurrent > 0xFFFF ? 0xFFFF : snap.meancurrent);
	data.openingCount = snap.opening_count;
	data.closingCount = snap.closing_count;
	data.deadzoneCount = snap.deadzone_count;
	data.calibRetries = snap.calibRetries;
	data.movements = snap.movements;
	data.calState = vdm::composeCalState(snap.calibActive != 0, requested, snap.diag.earlyWarn, snap.diag.lastCalFailed);
	data.earlyStops = snap.diag.earlyStops;
	data.cmdRejected = myvalves[x].cmdRejected;
	data.last = snap.diag.last;
}


// the gstat values (also the first ones of gstax)
static void fillStat (vdm::StatReply &stat)
{
	stat.uptimeSeconds = sysstat_uptime_s();
	stat.resets = sysstat_resets();
	stat.bootReason = (uint8_t) sysstat_boot_reason();
	stat.rxOverflow = commLine.overflowCount();
	stat.parseErrors = commLine.malformedCount() + commLine.expiredCount() + commTooManyArgs;
	stat.eepromState = eeprom_state();
}


void communication_setup (void) {
	
	// UART to ESP32
	COMM_SER.setRx(PA10);			//STM32F401 blackpill USART1 RX PA10
	COMM_SER.setTx(PA9);			//STM32F401 blackpill USART1 TX PA9
	COMM_SER.begin(115200, SERIAL_8N1);
	while(!COMM_SER);
	// the core has no hook for receive errors: its RX callback is wrapped. serial_t is found from
	// the UART handle like the core's get_serial_obj() (uart.c) does.
	serial_t *serial = (serial_t *) ((char *) COMM_SER.getHandle() - offsetof(serial_t, handle));
	const uint32_t primask = __get_PRIMASK();
	__disable_irq();
	serial->rx_callback = comm_rx_irq;
	__set_PRIMASK(primask);
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

	// the valve polls of an ESP that sends no lease commands (2.0.0, legacy) keep the lease alive
	if (req.is(APP_PRE_GETVLVDATA) || req.is(APP_PRE_GETVLVEXT)) app_lease_poll();

	// set target position
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	if(req.is(APP_PRE_SETTARGETPOS)) {
		commdbg_println("set target pos");

		if (req.argc() == 2 && req.argU16(0, 0, ACTUATOR_COUNT - 1, x) && req.argU8(1, 0, 100, pos)) {
			// also taken while a calibration is requested or running: its final positioning (A_SET)
			// reads the target when the calibration ends, and app_loop starts a requested
			// calibration before it moves the valve to a new target
			myvalvemots[x].target_position = pos;
			app_target_changed(x);
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
		else if (req.argc() == 1 && req.argU16(0, 255, 255, x)) {
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
		else if (req.argc() == 1 && req.argU16(0, 255, 255, x)) {
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
	

	// set valve learning time (stored in the EEPROM)
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETLEARNTIME)) {
		commdbg_print("set valve learning time to ");

		if (req.argc() == 1 && req.argU32(0, 0, UINT32_MAX, xu32) && comm_set_learntime(xu32) == 0) {
			COMM_SER.println(APP_PRE_SETLEARNTIME);
			commdbg_println(xu32, DEC);
		}
		else commdbg_println("- error");
	}


	// set valve learning movements
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETLEARNMOVEM)) {
		commdbg_print("set valve learning movements to ");

		// the ESP sends a uint32; v1 always replied, so a value outside 0, 50..65534 is moved to the
		// nearest bound instead of being rejected (the same range is loaded at start-up)
		const bool valid = req.argc() == 1 && req.argU32(0, 0, UINT32_MAX, xu32);
		if (valid) x = vdm::learnMovementsFromRequest(xu32);

		// every request restarts the movement counters of all valves (v1), the EEPROM is written
		// only for a new value
		if (valid && app_set_learnmovements(x) == 0) {
			commdbg_println(x, DEC);
			if (eep_content.numberOfMovements != x) {
				eep_content.numberOfMovements = x;
				eeprom_changed(EEP_CHANGED_MOVEMENTS);
			}
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
	// every value is checked against the range table (gmotx): the values in range are applied, a value
	// out of range leaves its field unchanged and the reply is "smotc err" (an end-stop factor of 41..50 is
	// applied as 40, see vdm::kFacRequestMax); a request with fewer than
	// 3 values or a value that is not a number changes nothing ("smotc err")
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETMOTCHARS)) {
		uint32_t values[5] = {0, 0, 0, 0, 0};
		const vdm::MotorParams current = motor_get_params();
		vdm::MotorParams params = current;
		vdm::ParamsRequest result = vdm::ParamsRequest::Rejected;
		bool numbers = req.argc() >= 3 && req.argc() <= 5;

		commdbg_print("got set motor characteristics request ");

		for (uint8_t i = 0; numbers && i < req.argc(); i++) numbers = req.argU32(i, 0, UINT32_MAX, values[i]);
		if (numbers) result = vdm::applyMotorParamsRequest(params, req.argc(), values);

		if (result != vdm::ParamsRequest::Rejected && !vdm::sameMotorParams(params, current)) {
			motor_set_params(params);
			eeprom_changed(EEP_CHANGED_MOTOR);
		}

		if (result == vdm::ParamsRequest::Applied) {
			COMM_SER.println(APP_PRE_SETMOTCHARS);
			commdbg_println("- valid");
		}
		else {
			sendReply(vdm::formatResult(replyLine, APP_PRE_SETMOTCHARS, false));
			commdbg_println("- invalid arguments");
		}
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


	// detect valve status: stdet 255 tests every valve again; another number is answered with
	// "stdet err" (v1 confirmed it without doing anything)
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETDETECTVLV)) {
		commdbg_print("got detect valve status request");

		if (req.argc() == 1 && req.argU16(0, 0, 65535, x)) {
			// reset status of all valves so they will be detected again
			if (x == 255) {
				app_scan_valves();
				commdbg_println(" - reset all valves");
				COMM_SER.print(APP_PRE_SETDETECTVLV);
				COMM_SER.println(" ");
			}
			else {
				commdbg_println(" - error");
				sendReply(vdm::formatResult(replyLine, APP_PRE_SETDETECTVLV, false));
			}
		}
		else commdbg_println(" - error");		
	} 


	// get version request: "gvers <version>_<board revision> <build> "
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETVERSION)) {
		commdbg_println("got version request");

		COMM_SER.print(APP_PRE_GETVERSION);
		COMM_SER.print(" ");			
		COMM_SER.print(FIRMWARE_VERSION);
		COMM_SER.print("_");
		COMM_SER.print(kHardwareMarker + sizeof(HARDWARE_MARKER_PREFIX) - 1);

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
		COMM_SER.print(vdm::eepstSaved(eeprom_state()), DEC);
		COMM_SER.println(" ");			
	} 

	// protocol version (v2 feature detection)
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETPROTOCOL)) {
		sendReply(vdm::formatProtocolVersion(replyLine));
	}

	// extended valve data: gvlvx idx status pos target meanCur oc cc dc cr moves calState earlyStops
	//                      cmdRejected lastDir lastReq lastCnt lastStop lastPeak lastMs
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETVLVEXT)) {
		if (req.argc() == 1 && req.argU16(0, 0, ACTUATOR_COUNT - 1, x)) {
			vdm::ValveExtReply data;

			fillValveExt(x, data);
			sendReply(vdm::formatValveExt(replyLine, data));
		}
		else commdbg_println("gvlvx: invalid arguments");
	}

	// current profile of the last move: gprof idx n c1:m1 ... cn:mn
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETPROFILE)) {
		if (req.argc() == 1 && req.argU16(0, 0, ACTUATOR_COUNT - 1, x)) {
			static vdm::ProfileRecorder profile;		// static: keeps 136 bytes off the stack
			valve_get_profile(x, profile);
			sendReply(vdm::formatProfile(replyLine, (uint8_t) x, profile));
		}
		else commdbg_println("gprof: invalid arguments");
	}

	// service move: svmov idx dir counts maxmA -> "svmov idx ok" / "svmov idx err code"
	// code 1: invalid arguments, 2: valve state machine busy, 3: calibration of the valve pending
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SERVICEMOVE)) {
		uint8_t dir = 0, maxmA = 0;
		uint16_t counts = 0;
		const int32_t index = (req.argc() >= 1 && req.argU16(0, 0, ACTUATOR_COUNT - 1, x)) ? (int32_t) x : -1;
		uint8_t error = 1;

		if (index >= 0 && req.argc() == 4
			&& req.argU8(1, vdm::kDirOpen, vdm::kDirClose, dir)
			&& req.argU16(2, SVMOV_COUNTS_MIN, SVMOV_COUNTS_MAX, counts)
			&& req.argU8(3, SVMOV_MAXMA_MIN, SVMOV_MAXMA_MAX, maxmA)) {
			const int16_t result = app_service_move(x, dir, counts, maxmA);
			error = result == 0 ? 0 : (result == -3 ? 3 : 2);
		}
		sendReply(vdm::formatIndexedResult(replyLine, APP_PRE_SERVICEMOVE, index, error));
	}

	// breakaway escalation: scalx enable stepPct maxmA
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETCALESC)) {
		vdm::EscalationConfig config;
		const bool valid = req.argc() == 3
			&& req.argU8(0, 0, 1, config.enable)
			&& req.argU8(1, 0, vdm::kEscalationStepMax, config.stepPct)
			&& req.argU8(2, vdm::kEscalationMaxmAMin, vdm::kEscalationMaxmAMax, config.maxmA);

		if (valid) {
			const bool changed = memcmp(&eep_content.escalation, &config, sizeof(config)) != 0;
			motor_set_escalation(config);
			if (changed) eeprom_changed(EEP_CHANGED_ESCALATION);
		}
		sendReply(vdm::formatResult(replyLine, APP_PRE_SETCALESC, valid));
	}

	else if(req.is(APP_PRE_GETCALESC)) {
		sendReply(vdm::formatEscalation(replyLine, motor_get_escalation()));
	}

	// health: gstat uptime_s resets bootReason rxOverflow parseErr eepState
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETSTATUS)) {
		vdm::StatReply stat;

		fillStat(stat);
		sendReply(vdm::formatStat(replyLine, stat));
	}

	// ranges of the smotc values
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETMOTLIMITS)) {
		sendReply(vdm::formatMotorLimits(replyLine));
	}

	// protocol 3 ------------------------------------------------------------

	// lease heartbeat: slhbt alive (0/1) -> "slhbt lease remainS" / "slhbt err"
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_LEASEHEARTBEAT)) {
		if (req.argc() == 1 && req.argU8(0, 0, 1, pos)) {
			app_lease_heartbeat(pos != 0);
			const uint8_t lease = app_lease_state();
			sendReply(vdm::formatHeartbeat(replyLine, lease, app_lease_remaining_s()));
		}
		else sendReply(vdm::formatResult(replyLine, APP_PRE_LEASEHEARTBEAT, false));
	}

	// lease timeout: slcfg minutes (0 = off, 5..1440), stored in the EEPROM; no renewal
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETLEASE)) {
		const bool valid = req.argc() == 1 && req.argU32(0, 0, UINT32_MAX, xu32) && vdm::leaseTimeoutValid(xu32);

		if (valid) {
			// while the EEPROM read has failed the mirror holds the default 60, not the stored
			// timeout: marked also when equal, the re-read takes over the marked fields only
			if (eep_content.leaseTimeoutMin != xu32 || eeprom_state() == vdm::kEepStateReadFailed) {
				eep_content.leaseTimeoutMin = (uint16_t) xu32;
				eeprom_changed(EEP_CHANGED_LEASE);
			}
			app_lease_configure((uint16_t) xu32);
			app_lease_command();
		}
		sendReply(vdm::formatResult(replyLine, APP_PRE_SETLEASE, valid));
	}

	// failsafe position: sfspo idx|255 pct (0..100, 255 = hold), stored in the EEPROM
	// -> "sfspo idx ok" / "sfspo idx err 1" / "sfspo -1 err 1" (no valid index)
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SETFAILSAFE)) {
		const int32_t index = (req.argc() >= 1 && argValveOrAll(req, 0, x)) ? (int32_t) x : -1;
		uint8_t error = 1;

		if (index >= 0 && req.argc() == 2 && req.argU8(1, 0, 255, pos) && vdm::failsafePctValid(pos)) {
			setFailsafe(x, pos);
			app_lease_command();
			error = 0;
		}
		sendReply(vdm::formatIndexedResult(replyLine, APP_PRE_SETFAILSAFE, index, error));
	}

	// lease timeout and failsafe positions: "glcfg timeoutMin fs0 ... fs11"
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETLEASE)) {
		uint8_t fs[ACTUATOR_COUNT];

		app_lease_command();
		for (uint8_t v = 0; v < ACTUATOR_COUNT; v++) fs[v] = app_failsafe_pct(v);
		sendReply(vdm::formatLeaseConfig(replyLine, app_lease_timeout(), fs));
	}

	// gvlvx plus flags fault fsPct drive retryS retries
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETVLVEXT3)) {
		if (req.argc() == 1 && req.argU16(0, 0, ACTUATOR_COUNT - 1, x)) {
			vdm::ValveExtV3Reply data;
			struct valve_v3_info info;

			fillValveExt(x, data.base);
			app_get_valve_v3(x, info);
			data.flags = info.flags;
			data.fault = info.fault;
			data.failsafePct = info.fsPct;
			data.drive = info.drive;
			data.retryS = info.retryS;
			data.retries = info.retries;
			sendReply(vdm::formatValveExtV3(replyLine, data));
		}
		else commdbg_println("gvlvy: invalid arguments");
	}

	// gstat plus lease, safe mode, UART errors, EEPROM load and writes, temperature ages, sysFlags
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETSTATUS3)) {
		vdm::StatV3Reply stat;

		fillStat(stat.base);
		stat.lease = app_lease_state();
		stat.leaseRemainS = app_lease_remaining_s();
		stat.leaseClient = app_lease_client() ? 1 : 0;
		stat.leaseTimeoutMin = app_lease_timeout();
		stat.failsafeMask = app_failsafe_mask();
		stat.safeMode = sysstat_safe_mode() ? 1 : 0;
		stat.wdgResets = sysstat_wdg_resets();
		const uint32_t primask = __get_PRIMASK();
		__disable_irq();
		const vdm::UartErrorCounters uart = commUartErrors;
		__set_PRIMASK(primask);
		stat.uartOre = uart.overrun;
		stat.uartFe = uart.framing;
		stat.uartNe = uart.noise;
		stat.rxDropped = uart.dropped;
		stat.cfgFlags = eeprom_cfg_flags();
		stat.cfgEvents = eeprom_cfg_events();
		stat.eepWrites = eeprom_writes();
		stat.tempAgeS = app_temp_age_s();
		stat.owScanAgeS = ow_scan_age_s();
		stat.sysFlags = app_protect_suspended() ? vdm::kSysFlagProtectSuspended : 0;
		sendReply(vdm::formatStatV3(replyLine, stat));
	}

	// stop: sstop idx|255 -> "sstop idx ok" (also when nothing ran) / "sstop -1 err 1"
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_STOP)) {
		const bool valid = req.argc() == 1 && argValveOrAll(req, 0, x) && app_stop(x) == 0;

		sendReply(vdm::formatIndexedResult(replyLine, APP_PRE_STOP, valid ? (int32_t) x : -1, valid ? 0 : 1));
	}

	// stored learn time: "gtlnt seconds" (0 = time trigger off)
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_GETLEARNTIME)) {
		sendReply(vdm::formatLearnTime(replyLine, app_get_learntime()));
	}

	// leave safe mode: ssafe 0 -> "ssafe ok" / "ssafe err"
	// ++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
	else if(req.is(APP_PRE_SAFEMODE)) {
		const bool valid = req.argc() == 1 && req.argU8(0, 0, 0, pos);

		if (valid) sysstat_leave_safe_mode();
		sendReply(vdm::formatResult(replyLine, APP_PRE_SAFEMODE, valid));
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
			commLastByteMs = millis();
			budget--;
		}

		char *line = commLine.takeLine();
		if (line == NULL) break;

		vdm::Tokenizer req;
		if (req.parse(line, NO_OF_ARGS)) {
			communication_dispatch(req);
			result = 0;
		}
		else if (req.tooManyArgs()) {
			commTooManyArgs++;
			commdbg_println("comm: too many arguments");
		}
		commLine.release();
	}

	// the rest of a partial line is read first: the main loop may have been blocked while it arrived.
	// Bytes of a line that stay unterminated (ESP restarted in the middle of a request, noise) must
	// not be glued to the next request.
	if (COMM_SER.available() == 0 && commLine.expire(millis(), commLastByteMs, COMM_LINE_TIMEOUT_MS))
		commdbg_println("comm: incomplete line dropped");

	return result;
}
