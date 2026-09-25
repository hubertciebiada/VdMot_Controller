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


#include "eeprom.h"
#include "hardware.h"
#include "I2C_eeprom.h"		// freat library from https://github.com/RobTillaart/I2C_EEPROM
#include "i2c_bus.h"
#include "app.h"
#include "vdm/config_store.h"
#include "vdm/eeprom_layout.h"
#include "vdm/replies_v2.h"
#include "vdm/store_scheduler.h"

//#define EEPROM_DEBUG(...)
//#define EEPROM_DEBUG 	Serial3.print
#define EEPROM_DEBUG 	Serial6.print


#define EE24LC64MAXBYTES 		64*1024/8
#define DEVICEADDRESS 			0x50		// A0, A1, A2 = GND

I2C_eeprom eeprom(DEVICEADDRESS, EE24LC64MAXBYTES);

struct eeprom_layout eep_content;

// failed block transfers allowed in one read of the configuration, whichever blocks they hit:
// each failure blocks for up to ~0.2 s, so a read on a marginal bus ends in well under a second
#define EEP_READ_FAILURES_MAX	3
#define EEP_RETRY_FIRST_S		30		// a failed read or write is retried after 30 s, 60 s, ... up to 1 h
#define EEP_RETRY_MAX_S			3600
#define EEP_ALL_SLOTS			0x00FFFFFFu		// owsensors1[0..11], owsensors2[0..11]
#define EEP_ALL_CALIB			0x0FFFu			// calibration records of the 12 valves

// when the configuration is written and read again (eepromloop() runs once per second)
static vdm::StoreScheduler eep_store(EEP_RETRY_FIRST_S, EEP_RETRY_MAX_S);
// the sensor slots and calibration records changed in RAM and not written yet (a re-read after a
// failed read keeps them, see vdm::mergeChanges)
static uint32_t eep_changed_slots = 0;
static uint16_t eep_changed_calib = 0;
// set once the current read has used up its failures (EEP_READ_FAILURES_MAX)
static bool eep_read_error = false;
// failed block transfers of the current read
static uint8_t eep_read_failures = 0;
static uint8_t eep_cfg_flags = 0;			// vdm::kCfg* of the last load
static uint32_t eep_cfg_events = 0;			// loads that repaired or defaulted a block
static uint32_t eep_write_steps = 0;		// successful write steps
static uint8_t eep_lease_source = vdm::kLeaseSourceDefault;

// static: the blocks and the resolved configuration are kept off the main loop stack
static vdm::RawImages eep_raw;
static vdm::LoadResult eep_loaded;

static_assert(EE_GENERALDATA_ADR == vdm::kLegacyLayoutAddress, "one address of the 1.x layout");


// call from setup function in main
int16_t eepromsetup () {
	eep_content.status = EEP_INIT;
	return 0;
}


// the RAM mirror follows the storage state: EEP_CHANGED while a write waits
static void eeprom_sync_status () {
	eep_content.status = eep_store.eepState() == vdm::kEepStatePending ? EEP_CHANGED : EEP_VALID;
}


// every I2C transfer on a disturbed bus costs up to ~0.2 s: stop at the first error
static int16_t eeprom_write_failed () {
	EEPROM_DEBUG("write error, aborted\r\n");
	return -1;
}


//----------------------------------------------------------------------------
//
// writes the blocks holding `fields` (EEP_CHANGED_*), in the order C(v) (valves in `calib`), B,
// 1.x layout, A: a write cut off by a reset leaves every block either old or new and block A (the
// layout CRC) last, see vdm::resolveConfig
//
//	returns 0 on success, -1 if an I2C write failed (the blocks may be partly written)
static int16_t eeprom_write_blocks (const struct eeprom_layout &lay, uint16_t fields, uint16_t calib) {
	static uint8_t image[vdm::kLegacyImageSize];		// static: keeps it off the main loop stack
	uint8_t block[vdm::kSafetyBlockSize];
	const uint8_t blocks = vdm::blocksFor(fields);

	EEPROM_DEBUG("write eeprom layout to eeprom...\r\n");

	if (blocks & vdm::kBlockCalib) {
		for (uint8_t v = 0; v < ACTUATOR_COUNT; v++) {
			if (!(calib & (1u << v))) continue;
			uint8_t record[vdm::kCalibBlockSize];
			const size_t n = vdm::encodeCalib(lay.calib[v], v, record);
			if (eeprom.writeBlock(vdm::kCalibBlockAddress + v * vdm::kCalibBlockSize, record, n) != 0) return eeprom_write_failed();
		}
	}

	if (blocks & vdm::kBlockSafety) {
		vdm::SafetyBlock safety;
		memcpy(safety.failsafePct, lay.failsafePct, sizeof(safety.failsafePct));
		safety.shadow.lowFac = lay.currentbound_low_fac;
		safety.shadow.highFac = lay.currentbound_high_fac;
		safety.shadow.movements = lay.numberOfMovements;
		safety.shadow.startOnPower = lay.startOnPower;
		safety.shadow.minCounts = lay.noOfMinCounts;
		safety.shadow.maxRetries = lay.maxCalibRetries;
		safety.leaseTimeoutMin = lay.leaseTimeoutMin;
		safety.leaseValid = true;
		const size_t n = vdm::encodeSafety(safety, block);
		if (eeprom.writeBlock(vdm::kSafetyBlockAddress, block, n) != 0) return eeprom_write_failed();
	}

	vdm::encodeLegacyLayout(lay, image);
	if (blocks & vdm::kBlockLayout) {
		if (eeprom.writeBlock(vdm::kLegacyLayoutAddress, image, sizeof(image)) != 0) return eeprom_write_failed();
	}

	if (blocks & vdm::kBlockSettings) {
		vdm::StoredExtension ext;
		uint8_t extbuf[vdm::kExtensionBlockSize];
		ext.escalation = lay.escalation;
		ext.learnTimeS = lay.learnTimeS;
		ext.leaseTimeoutMin = lay.leaseTimeoutMin;
		ext.layoutCrc = vdm::crc16Ccitt(image, sizeof(image));
		const size_t n = vdm::encodeExtension(ext, extbuf);
		if (eeprom.writeBlock(vdm::kExtensionAddress, extbuf, n) != 0) return eeprom_write_failed();
	}

	EEPROM_DEBUG("finished\r\n");

	return 0;
}


//----------------------------------------------------------------------------
//
// writes the 1.x layout with blocks B and A (not the calibration records)
//
//	returns 0 on success, -1 if an I2C write failed (the layout may be partly written)
int16_t eeprom_write_layout (struct eeprom_layout* lay) {
	return eeprom_write_blocks(*lay, EEP_CHANGED_ALL, 0);
}


// one write step of eepromloop(): the fields changed since the last successful write
static void eeprom_write_step () {
	const bool ok = eeprom_write_blocks(eep_content, eep_store.dirty(), eep_changed_calib) == 0;

	eep_store.writeResult(ok);
	if (ok) {
		eep_changed_slots = 0;
		eep_changed_calib = 0;
		eep_write_steps++;
	}
}


// reads a block, retried while the read has failures left; if it cannot be read the buffer is
// filled with 0xFF like an erased EEPROM, so the load falls back to the defaults instead of
// using stack garbage. The failures are counted over the whole read, not per block: after
// EEP_READ_FAILURES_MAX of them the bus is not used again in this read, so an intermittent bus
// (each failed transfer blocks for up to ~0.2 s) cannot stretch the read past the watchdog.
static void eeprom_read_block (uint16_t address, uint8_t *buf, uint16_t length) {
	while (!eep_read_error) {
		if (eeprom.readBlock(address, buf, length) == length) return;
		if (++eep_read_failures >= EEP_READ_FAILURES_MAX) {
			EEPROM_DEBUG("read error, using defaults\r\n");
			eep_read_error = true;
		}
	}
	memset(buf, 0xFF, length);
}


// reads the 1.x layout and the blocks A, B, C0..C11 and resolves them into eep_loaded
static void eeprom_load () {
	EEPROM_DEBUG("Read eeprom layout from eeprom...");

	eep_read_error = false;
	eep_read_failures = 0;
	eeprom_read_block(vdm::kLegacyLayoutAddress, eep_raw.layout, sizeof(eep_raw.layout));
	eeprom_read_block(vdm::kExtensionAddress, eep_raw.settings, sizeof(eep_raw.settings));
	eeprom_read_block(vdm::kSafetyBlockAddress, eep_raw.safety, sizeof(eep_raw.safety));
	eeprom_read_block(vdm::kCalibBlockAddress, &eep_raw.calib[0][0], sizeof(eep_raw.calib));
	eep_raw.readFailed = eep_read_error;

	vdm::resolveConfig(eep_raw, eep_loaded);
	eep_cfg_flags = eep_loaded.cfgFlags;
	if (vdm::repairsConfig(eep_loaded.cfgFlags)) eep_cfg_events++;
	eep_store.readResult(!eep_raw.readFailed);

	EEPROM_DEBUG("finished, cfgFlags ");
	EEPROM_DEBUG(eep_loaded.cfgFlags, HEX);
	EEPROM_DEBUG("\r\n");
}


//----------------------------------------------------------------------------
//
// reads the configuration at start-up; damaged or missing blocks are repaired (written back by
// eepromloop()). If a block cannot be read, writing is disabled and the read is retried by
// eepromloop().
//
//	returns 0 on success, -1 if a block could not be read
int16_t eeprom_read_layout (struct eeprom_layout* lay) {
	eeprom_load();
	*static_cast<vdm::ConfigImage *>(lay) = eep_loaded.image;
	eep_lease_source = eep_loaded.leaseSource;
	if (eep_loaded.rewrite != 0) eeprom_changed(eep_loaded.rewrite);
	lay->status = eep_loaded.rewrite != 0 ? EEP_CHANGED : EEP_VALID;
	return eep_raw.readFailed ? -1 : 0;
}


// retry of a failed read: the stored configuration is taken over, except the fields changed
// since (they are newer and get written), and writing is enabled again
static void eeprom_reread () {
	eeprom_load();
	if (eep_raw.readFailed) return;

	EEPROM_DEBUG("eeprom read after failure\r\n");
	const vdm::ChangeSet changes = {eep_store.dirty(), eep_changed_slots, eep_changed_calib};
	vdm::mergeChanges(eep_loaded.image, eep_content, changes);
	*static_cast<vdm::ConfigImage *>(&eep_content) = eep_loaded.image;
	if (eep_loaded.rewrite != 0) eeprom_changed(eep_loaded.rewrite);
	app_load_config();
}


int16_t eepromloop() {
	const vdm::StoreScheduler::Step step = eep_store.tick();

	if (step != vdm::StoreScheduler::Step::None) {
		// a retry may find the bus stuck the way the failed transfer left it
		if (eep_store.retrying()) i2c_bus_restart();
		if (step == vdm::StoreScheduler::Step::Reread) eeprom_reread();
		else eeprom_write_step();
	}
	eeprom_sync_status();
	return 0;
}


// call everytime some eeprom content was changed
void eeprom_changed (uint16_t fields) {
	if (fields & EEP_CHANGED_SENSORS) eep_changed_slots = EEP_ALL_SLOTS;
	if (fields & EEP_CHANGED_CALIB) eep_changed_calib = EEP_ALL_CALIB;
	eep_store.changed(fields);
	eeprom_sync_status();
}


// one sensor slot changed: only this slot is taken over at a re-read after a failed read
void eeprom_changed_slot (uint8_t slot) {
	eep_changed_slots |= 1ul << slot;
	eep_store.changed(EEP_CHANGED_SENSORS);
	eeprom_sync_status();
}


// calibration record of a valve: stored and written only when it differs
void eeprom_store_calib (uint8_t valve, const vdm::CalibRecord &rec) {
	if (valve >= ACTUATOR_COUNT) return;
	vdm::CalibRecord &stored = eep_content.calib[valve];

	if (stored.openingCount == rec.openingCount && stored.closingCount == rec.closingCount
		&& stored.meanCurrent == rec.meanCurrent && stored.flags == rec.flags) return;
	stored = rec;
	eep_changed_calib |= (uint16_t) (1u << valve);
	eep_store.changed(EEP_CHANGED_CALIB);
	eeprom_sync_status();
}


uint8_t eeprom_cfg_flags () {
	return eep_cfg_flags;
}


uint32_t eeprom_cfg_events () {
	return eep_cfg_events;
}


uint32_t eeprom_writes () {
	return eep_write_steps;
}


uint8_t eeprom_lease_source () {
	return eep_lease_source;
}


// health of the configuration storage for gstat
uint8_t eeprom_state () {
	return eep_store.eepState();
}


// true if no write is waiting: a reset may happen now
bool eeprom_free () {
	return eep_store.free();
}
