#include "stub_communication.h"

namespace stub {

Communication communication;

namespace {

void resetCommunication() { communication = Communication(); }

Registrar g_registrar(resetCommunication);

}  // namespace

}  // namespace stub

using stub::communication;
using stub::log;

void communication_setup(void) { log("communication_setup()"); }

int16_t communication_loop(void) {
  log("communication_loop()");
  return communication.loop;
}

int16_t comm_set_valve_sensors(uint16_t valve, const char* first, const char* second) {
  log("comm_set_valve_sensors(%u, %s, %s)", valve, first, second);
  return communication.setSensors;
}

int16_t comm_set_valve_sensor_index(uint16_t valve, uint8_t slot, uint16_t sensor) {
  log("comm_set_valve_sensor_index(%u, %u, %u)", valve, slot, sensor);
  return communication.setSensorIndex;
}

void comm_print_valve_sensor_ids(Print& out, uint16_t valve, char delimiter) {
  log("comm_print_valve_sensor_ids(%u, '%c')", valve, delimiter);
  out.print(communication.firstSensor.c_str());
  out.print(delimiter);
  out.print(communication.secondSensor.c_str());
}
