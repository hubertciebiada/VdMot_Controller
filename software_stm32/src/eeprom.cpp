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
#include "motor.h"
#include "vdm/eeprom_layout.h"
#include "vdm/replies_v2.h"

//#define EEPROM_DEBUG(...)
//#define EEPROM_DEBUG 	Serial3.print
#define EEPROM_DEBUG 	Serial6.print


byte buffer[256];
//long address;

#define EE24LC64MAXBYTES 		64*1024/8
#define DEVICEADDRESS 			0x50		// A0, A1, A2 = GND

I2C_eeprom eeprom(DEVICEADDRESS, EE24LC64MAXBYTES);

// SPIEEPROM eep(EEPROM_TYPE_16BIT, EEP_CS_PIN); // parameter is type
//                     // type=0: 16-bits address
//                     // type=1: 24-bits address
//                     // type>1: defaults to type 0

struct eeprom_layout eep_content;

#define EEP_READ_ATTEMPTS		3		// per block at startup
#define EEP_WRITE_ATTEMPTS		3		// per change, one attempt per eepromloop() run

// the layout could not be read completely at startup: RAM holds 0xFF fallbacks for the missing
// fields, and writing the layout back would destroy the configuration still stored in the EEPROM
static bool eep_read_failed = false;
// the last change could not be written (all attempts failed); cleared by the next successful write
static bool eep_write_failed = false;

// size of the 1.x layout from EE_GENERALDATA_ADR: base block, sensor slots, tail
static_assert(EE_GENERALDATA_ADR + 33 + (2 * ACTUATOR_COUNT + ADDITIONAL_SENSOR_COUNT) * 8 + 4 == vdm::kExtensionAddress,
	"the extension block must follow the 1.x layout");



void fill_buffer()
{
  for (int i=0;i<256;i++)
  {
    buffer[i]=i;
  }
}


// call from setup function in main
int16_t eepromsetup () {

    // SPI.setMOSI(EEP_MOSI_PIN);
    // SPI.setMISO(EEP_MISO_PIN);
    // SPI.setSCLK(EEP_CLK_PIN);
    // eep.setup(); // setup eeprom


    // // test
    // fill_buffer();
	// address = 0;
	
	// EEPROM_DEBUG("Starting to write on EEPROM:");
	// EEPROM_DEBUG(millis());
	// EEPROM_DEBUG("\r\n");
	
	// //eep.write(address, buffer, (sizeof(buffer)/sizeof(byte)));
	// eeprom.writeBlock(address, buffer, (sizeof(buffer)/sizeof(byte)));
	
	// EEPROM_DEBUG("Finish to write:");
	// EEPROM_DEBUG(millis());
	// EEPROM_DEBUG("\r\n");

	eep_content.status = EEP_INIT;
	
  return 0;
}


int16_t eepromloop() {
	static int writedelaycnt = 0;
	static int writeattempts = 0;

    #define E_INIT      0
    #define E_IDLE      1
    #define E_READCFG   2
	#define E_WRITECFG  3

    static int eepromstate = E_INIT;

    //int x;

    switch (eepromstate) {
        case E_INIT:
                    // address = 0;
                    // EEPROM_DEBUG("Test read eeprom\r\n");
                    // for (x=0;x<30;x++) {
                    //     EEPROM_DEBUG("Address:|");
                    //     EEPROM_DEBUG(address);
                    //     EEPROM_DEBUG("| - Value:|");
                    //     //EEPROM_DEBUG(eep.readByte(address), DEC);
					// 	EEPROM_DEBUG(eeprom.readByte(address), DEC);
                    //     EEPROM_DEBUG("|\r\n");
                    //     address++;
                    //     delay(10);
                    // }
                    // if (address == 256)
                    //    address = 0;    
					
					writedelaycnt = 0;
					eep_content.status = EEP_INIT;
                    eepromstate = E_IDLE;
                    break;

        case E_IDLE:                  
                    if (eep_content.status == EEP_CHANGED) {
						writedelaycnt++;						
					}

					if (writedelaycnt > 2) {		// write to eeprom not earlier than after 5 s 
						writedelaycnt=0;
						eepromstate = E_WRITECFG;
					}
					
                    break;


        case E_READCFG:


                    break;


		case E_WRITECFG:
					
					if (eep_read_failed) {
						// changes stay in RAM until the next start reads the EEPROM successfully
						EEPROM_DEBUG("eeprom not written, layout was not read at startup\r\n");
						eep_content.status = EEP_VALID;
					}
					else if (eeprom_write_layout (&eep_content) == 0) {
						writeattempts = 0;
						eep_write_failed = false;
						eep_content.status = EEP_VALID;
					}
					else if (++writeattempts >= EEP_WRITE_ATTEMPTS) {
						// after the last failed attempt give up, a pending write must not block a reset for ever
						writeattempts = 0;
						eep_write_failed = true;
						eep_content.status = EEP_VALID;
					}
					// otherwise EEP_CHANGED remains and the write is retried after the write delay
					eepromstate = E_IDLE;

                    break;


        default:    eepromstate = E_IDLE;
                    break;
    }

        


  return 0;
}





//----------------------------------------------------------------------------
//
// places inital eeprom layout
void eeprom_fill (void) {
	//u16 a;
	unsigned char eef_buffer[4];

	// mark eeprom as written
	((*((uint32_t*)&eef_buffer[0]))) = 0x1F2F3F4F;
  	//eep.write(EEPROM_MARK_ADD, eef_buffer, 4);
	eeprom.writeBlock(EEPROM_MARK_ADD, eef_buffer, 4);

	// version
	((*((uint32_t*)&eef_buffer[0]))) = 11;
  	//eep.write(EEPROM_VERS1_ADD, eef_buffer, 1);
	eeprom.writeBlock(EEPROM_VERS1_ADD, eef_buffer, 4);

	// clear reserved area
	/*((*((u32*)&eef_buffer[0]))) = 0x00000000;
	for(a=EEPROM_VERS+1;a<EEPROM_IP;a++) {
    eep.write(EEPROM_VERS1_ADD+1, eef_buffer, 1);
	} */

}


//----------------------------------------------------------------------------
//
// checks if the eeprom is marked
//
//	returns 0 if mark was found
uint8_t eeprom_mark (void) {
	
	unsigned long eef_buffer;	   
  	//eep.readByteArray(EEPROM_MARK_ADD, (uint8_t *) (&eef_buffer), 4);
	eeprom.readBlock(EEPROM_MARK_ADD, (uint8_t *) (&eef_buffer), 4);

	EEPROM_DEBUG("read mark:");
  	EEPROM_DEBUG((unsigned int) eef_buffer, HEX);
  	EEPROM_DEBUG("\r\n");

	if ( eef_buffer == 0x1F2F3F4F) {
		EEPROM_DEBUG("found mark in eeprom...\r\n");
		return 0;
	}
	EEPROM_DEBUG("no mark found in eeprom...\r\n");
	return 1;
}




// every I2C transfer on a disturbed bus costs up to ~0.2 s: stop at the first error
static int16_t eeprom_write_failed () {
	EEPROM_DEBUG("write error, aborted\r\n");
	return -1;
}


//----------------------------------------------------------------------------
//
// writes eeprom layout to eeprom
//
//	returns 0 on success, -1 if an I2C write failed (the layout may be partly written)
int16_t eeprom_write_layout (struct eeprom_layout* lay) {

	uint8_t buf[100];
	uint16_t* pb;
	uint16_t x, y;
	uint16_t scnt;
	uint16_t address;

	EEPROM_DEBUG("write eeprom layout to eeprom...\r\n");

	x=0;
	address = EE_GENERALDATA_ADR;

	// first write base layout
	buf[x++] = lay->b_slave;
	for(y=0;y<sizeof(lay->descr);y++) {
		buf[x] = (uint8_t) (lay->descr[y]);
		x++;
	}
	for(y=0;y<sizeof(lay->OneWireCfg);y++) {
		buf[x] = (uint8_t) (lay->OneWireCfg[y]);
		x++;
	}

	// current bounds
	buf[x++] =  lay->currentbound_low_fac;
	buf[x++] =  lay->currentbound_high_fac;
	pb=(uint16_t*) &buf[x];
	*pb = lay->numberOfMovements;
	x+=2; 

  	//eep.write(address, buf, x);
	if (eeprom.writeBlock(address, buf, x) != 0) return eeprom_write_failed();

	// then write sensor data
	address = EE_GENERALDATA_ADR + x;
	
	// first sensors
	for(scnt=0;scnt<ACTUATOR_COUNT;scnt++) {
		x = 0;
		buf[x++] = lay->owsensors1[scnt].familycode;
		buf[x++] = lay->owsensors1[scnt].romcode[5];
		buf[x++] = lay->owsensors1[scnt].romcode[4];
		buf[x++] = lay->owsensors1[scnt].romcode[3];
		buf[x++] = lay->owsensors1[scnt].romcode[2];
		buf[x++] = lay->owsensors1[scnt].romcode[1];
		buf[x++] = lay->owsensors1[scnt].romcode[0];
		buf[x++] = lay->owsensors1[scnt].crc;

		if (eeprom.writeBlock(address, buf, x) != 0) return eeprom_write_failed();

		address += x;
	}

	// second sensors
	for(scnt=0;scnt<ACTUATOR_COUNT;scnt++) {
		x = 0;
		buf[x++] = lay->owsensors2[scnt].familycode;
		buf[x++] = lay->owsensors2[scnt].romcode[5];
		buf[x++] = lay->owsensors2[scnt].romcode[4];
		buf[x++] = lay->owsensors2[scnt].romcode[3];
		buf[x++] = lay->owsensors2[scnt].romcode[2];
		buf[x++] = lay->owsensors2[scnt].romcode[1];
		buf[x++] = lay->owsensors2[scnt].romcode[0];
		buf[x++] = lay->owsensors2[scnt].crc;

		if (eeprom.writeBlock(address, buf, x) != 0) return eeprom_write_failed();

		address += x;
	}

	// rest of sensors
	for(scnt=0;scnt<ADDITIONAL_SENSOR_COUNT;scnt++) {
		x = 0;
		buf[x++] = lay->owsensors[scnt].familycode;
		buf[x++] = lay->owsensors[scnt].romcode[5];
		buf[x++] = lay->owsensors[scnt].romcode[4];
		buf[x++] = lay->owsensors[scnt].romcode[3];
		buf[x++] = lay->owsensors[scnt].romcode[2];
		buf[x++] = lay->owsensors[scnt].romcode[1];
		buf[x++] = lay->owsensors[scnt].romcode[0];
		buf[x++] = lay->owsensors[scnt].crc;

		if (eeprom.writeBlock(address, buf, x) != 0) return eeprom_write_failed();

		address += x;
	}

// current bounds
	x=0;
	buf[x++] =  lay->startOnPower;
	pb=(uint16_t*) &buf[x];
	*pb = lay->noOfMinCounts;
	x+=2; 
	buf[x++] =  lay->maxCalibRetries;
	if (eeprom.writeBlock(address, buf, x) != 0) return eeprom_write_failed();

	// layout version 2 extension, behind the 1.x fields
	vdm::StoredExtension ext;
	uint8_t extbuf[vdm::kExtensionBlockSize];
	ext.escalation = lay->escalation;
	x = (uint16_t) vdm::encodeExtension(ext, extbuf);
	if (eeprom.writeBlock(vdm::kExtensionAddress, extbuf, x) != 0) return eeprom_write_failed();

	EEPROM_DEBUG("finished\r\n");

	return 0;
}


// reads a block with retries; if it cannot be read the buffer is filled with 0xFF like an
// erased EEPROM, so the range checks at startup fall back to the defaults instead of using
// stack garbage. After the first unreadable block the bus is not used again (each failed
// transfer blocks for up to ~0.2 s) and writing the layout back is disabled.
static void eeprom_read_block (uint16_t address, uint8_t *buf, uint16_t length) {
	for (uint8_t attempt = 0; !eep_read_failed && attempt < EEP_READ_ATTEMPTS; attempt++) {
		if (eeprom.readBlock(address, buf, length) == length) return;
	}
	if (!eep_read_failed) EEPROM_DEBUG("read error, using defaults\r\n");
	eep_read_failed = true;
	memset(buf, 0xFF, length);
}


//----------------------------------------------------------------------------
//
// reads eeprom layout from eeprom
//
//	returns 0 if mark was found
int16_t eeprom_read_layout (struct eeprom_layout* lay) {

	uint8_t buf[100];
	uint16_t* pb;
	uint16_t x, y;
	uint16_t scnt;
	uint16_t address;

	EEPROM_DEBUG("Read eeprom layout from eeprom...");

	address = EE_GENERALDATA_ADR;

	// first read base layout
	x = 1 + sizeof(lay->descr) + sizeof(lay->OneWireCfg) + sizeof(lay->currentbound_low_fac) + sizeof(lay->currentbound_high_fac)+ sizeof(lay->numberOfMovements);
	//eep.readByteArray(address, buf, x);
	eeprom_read_block(address, buf, x);

	x = 0;
	lay->b_slave = buf[x++];
	for(y=0;y<sizeof(lay->descr);y++) {
		lay->descr[y] = (char) (buf[x]);
		x++;
	}
	for(y=0;y<sizeof(lay->OneWireCfg);y++) {
		lay->OneWireCfg[y] = buf[x];
		x++;
	}
		// current bounds
	lay->currentbound_low_fac =  buf[x++];
	lay->currentbound_high_fac = buf[x++];
	pb=(uint16_t*) &buf[x];
	lay->numberOfMovements = *pb;
	x+=2; 
	address = EE_GENERALDATA_ADR + x;
	

	// first sensors
	for(scnt=0;scnt<ACTUATOR_COUNT;scnt++) {
		x = 8;

		eeprom_read_block(address, buf, x);

		lay->owsensors1[scnt].familycode = buf[0];
		lay->owsensors1[scnt].romcode[5] = buf[1];
		lay->owsensors1[scnt].romcode[4] = buf[2];
		lay->owsensors1[scnt].romcode[3] = buf[3];
		lay->owsensors1[scnt].romcode[2] = buf[4];
		lay->owsensors1[scnt].romcode[1] = buf[5];
		lay->owsensors1[scnt].romcode[0] = buf[6];
		lay->owsensors1[scnt].crc = buf[7];

		address += x;
	}

	// second sensors
	for(scnt=0;scnt<ACTUATOR_COUNT;scnt++) {
		x = 8;

		eeprom_read_block(address, buf, x);

		lay->owsensors2[scnt].familycode = buf[0];
		lay->owsensors2[scnt].romcode[5] = buf[1];
		lay->owsensors2[scnt].romcode[4] = buf[2];
		lay->owsensors2[scnt].romcode[3] = buf[3];
		lay->owsensors2[scnt].romcode[2] = buf[4];
		lay->owsensors2[scnt].romcode[1] = buf[5];
		lay->owsensors2[scnt].romcode[0] = buf[6];
		lay->owsensors2[scnt].crc = buf[7];

		address += x;
	}

	// rest of sensors
	for(scnt=0;scnt<ADDITIONAL_SENSOR_COUNT;scnt++) {
		x = 8;

		eeprom_read_block(address, buf, x);

		lay->owsensors[scnt].familycode = buf[0];
		lay->owsensors[scnt].romcode[5] = buf[1];
		lay->owsensors[scnt].romcode[4] = buf[2];
		lay->owsensors[scnt].romcode[3] = buf[3];
		lay->owsensors[scnt].romcode[2] = buf[4];
		lay->owsensors[scnt].romcode[1] = buf[5];
		lay->owsensors[scnt].romcode[0] = buf[6];
		lay->owsensors[scnt].crc = buf[7];

		address += x;
	}

	eeprom_read_block(address, buf, 1);
	lay->startOnPower = buf[0];
	address++;
	eeprom_read_block(address, buf, 2);
	pb=(uint16_t*) &buf[0];
	lay->noOfMinCounts = *pb;
	address+=2;
	eeprom_read_block(address, buf, 1);
	lay->maxCalibRetries = buf[0];
	address++;

	// layout version 2 extension; a 1.x image or a damaged block loads the defaults
	uint8_t extbuf[vdm::kExtensionBlockSize];
	vdm::StoredExtension ext;
	eeprom_read_block(vdm::kExtensionAddress, extbuf, sizeof(extbuf));
	const vdm::ExtensionState extstate = vdm::decodeExtension(extbuf, ext);
	lay->escalation = ext.escalation;
	if (extstate == vdm::ExtensionState::Legacy) EEPROM_DEBUG("layout 1.x, defaults for new fields...");
	else if (extstate == vdm::ExtensionState::Corrupt) EEPROM_DEBUG("extension damaged, defaults for new fields...");

	eep_content.status = EEP_VALID;
	EEPROM_DEBUG("finished\r\n");

	return 0;
}

// call everytime some eeprom content was changed
void eeprom_changed () {

	eep_content.status = EEP_CHANGED;

}


// health of the configuration storage for gstat
uint8_t eeprom_state () {
	if (eep_read_failed) return vdm::kEepStateReadFailed;
	if (eep_content.status == EEP_CHANGED) return vdm::kEepStatePending;
	if (eep_write_failed) return vdm::kEepStateWriteFailed;
	return vdm::kEepStateOk;
}


// return 1 if eeprom is not in change
bool eeprom_free () {
	return ((eep_content.status == EEP_VALID) || (eep_content.status == EEP_INIT));
	
}