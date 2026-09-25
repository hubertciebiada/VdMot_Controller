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

#ifndef _EEPROM_H
	#define _EEPROM_H

#include <Arduino.h>
#include "vdm/config_store.h"

int16_t eepromsetup();
int16_t eepromloop();
void eeprom_fill (void);
int16_t eeprom_write_layout (struct eeprom_layout* lay);
int16_t eeprom_read_layout (struct eeprom_layout* lay);
// fields of the configuration, for eeprom_changed() (= vdm::kChanged*): they select the EEPROM
// blocks to write, and a configuration that could not be read is merged with them
#define EEP_CHANGED_SENSORS			vdm::kChangedSensors		// owsensors1/2 (sensor assignment)
#define EEP_CHANGED_MOVEMENTS		vdm::kChangedMovements		// numberOfMovements
#define EEP_CHANGED_MOTOR			vdm::kChangedMotor			// factors, startOnPower, noOfMinCounts, maxCalibRetries
#define EEP_CHANGED_ESCALATION		vdm::kChangedEscalation		// escalation
#define EEP_CHANGED_LEARNTIME		vdm::kChangedLearnTime		// learnTimeS
#define EEP_CHANGED_LEASE			vdm::kChangedLease			// leaseTimeoutMin
#define EEP_CHANGED_FAILSAFE		vdm::kChangedFailsafe		// failsafePct
#define EEP_CHANGED_CALIB			vdm::kChangedCalib			// calib
#define EEP_CHANGED_ALL				vdm::kChangedAll

void eeprom_changed(uint16_t fields);
bool eeprom_free();
uint8_t eeprom_state();		// gstat eepState: vdm::kEepStateOk/Pending/WriteFailed/ReadFailed

extern struct eeprom_layout eep_content;


#endif