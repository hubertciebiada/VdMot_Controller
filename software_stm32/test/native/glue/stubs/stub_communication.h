// Link-seam stub of src/communication.cpp (include/communication.h): logs every call, returns the
// knobs of stub::communication.
#pragma once

#include <string>

#include "communication.h"
#include "stub_log.h"

namespace stub {

struct Communication {
  int16_t loop = -1;
  int16_t setSensors = 0;
  int16_t setSensorIndex = 0;
  // what comm_print_valve_sensor_ids() prints (the delimiter goes between the two addresses)
  std::string firstSensor = "00-00-00-00-00-00-00-00";
  std::string secondSensor = "00-00-00-00-00-00-00-00";
};
extern Communication communication;

}  // namespace stub
