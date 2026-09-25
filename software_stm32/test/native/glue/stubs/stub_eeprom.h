// Link-seam stub of src/eeprom.cpp (include/eeprom.h): logs every call (field masks in hex),
// returns the knobs of stub::eeprom; eep_content with its real type.
#pragma once

#include "eeprom.h"
#include "hardware.h"
#include "stub_log.h"
#include "vdm/replies_v2.h"

namespace stub {

struct Eeprom {
  int16_t setup = 0;
  int16_t loop = 0;
  int16_t writeLayout = 0;
  int16_t readLayout = 0;
  uint8_t cfgFlags = 0;
  uint32_t cfgEvents = 0;
  uint32_t writes = 0;
  uint8_t leaseSource = vdm::kLeaseSourceDefault;
  bool free = true;
  vdm::CalibRecord lastCalib = {};  // record of the last eeprom_store_calib()
  uint8_t state = vdm::kEepStateOk;
};
extern Eeprom eeprom;

}  // namespace stub
