#include "stub_owDevices.h"

#include <string.h>

tempsensor tempsensors[MAXONEWIRECNT];
voltsensor voltsensors[MAXDS2438CNT];
uint8_t noOfDevices = 0;
uint8_t noOfDS18Devices = 0;
uint8_t noOfDS2438Devices = 0;
OneWire oneWire(ONEW_PIN);
DallasTemperature sensors(&oneWire);

namespace stub {

OwDevices owDevices;

namespace {

void resetOwDevices() {
  owDevices = OwDevices();
  memset(tempsensors, 0, sizeof tempsensors);
  memset(voltsensors, 0, sizeof voltsensors);
  noOfDevices = 0;
  noOfDS18Devices = 0;
  noOfDS2438Devices = 0;
  oneWire = OneWire(ONEW_PIN);
  sensors = DallasTemperature(&oneWire);
}

Registrar g_registrar(resetOwDevices);

}  // namespace

}  // namespace stub

using stub::log;
using stub::owDevices;

void temperature_setup() { log("temperature_setup()"); }

void setDeviceAddress() { log("setDeviceAddress()"); }

void temperature_loop() { log("temperature_loop()"); }

void print_sensordata(Print& out) {
  log("print_sensordata()");
  out.print(owDevices.sensorData.c_str());
}

void temp_command(int command) { log("temp_command(%d)", command); }

bool temp_locked(void) {
  log("temp_locked()");
  return owDevices.locked;
}

uint32_t ow_scan_age_s(void) {
  log("ow_scan_age_s()");
  return owDevices.scanAgeS;
}

void printAddress(DeviceAddress deviceAddress) {
  log("printAddress(%02x-%02x-%02x-%02x-%02x-%02x-%02x-%02x)", deviceAddress[0], deviceAddress[1],
      deviceAddress[2], deviceAddress[3], deviceAddress[4], deviceAddress[5], deviceAddress[6],
      deviceAddress[7]);
}
