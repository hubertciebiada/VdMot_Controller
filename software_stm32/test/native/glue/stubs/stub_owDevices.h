// Link-seam stub of src/owDevices.cpp (include/owDevices.h): logs every call, returns the knobs of
// stub::owDevices; the globals of owDevices.cpp with their real types (sensors reads the fake 1-Wire
// bus, fake::oneWireBus).
#pragma once

#include <string>

#include "hardware.h"
#include "owDevices.h"
#include "stub_log.h"

namespace stub {

struct OwDevices {
  bool locked = false;
  uint32_t scanAgeS = 0;
  std::string sensorData = "{\"cnt\":0}\r\n";  // what print_sensordata() prints
};
extern OwDevices owDevices;

}  // namespace stub
