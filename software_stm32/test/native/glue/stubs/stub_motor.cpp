#include "stub_motor.h"

volatile valvemotor myvalvemots[ACTUATOR_COUNT];
volatile enum ASTATE valvestate;
volatile uint32_t valve_loop_ticks = 0;
volatile bool valve_loop_stalled = false;
volatile bool temp_refresh_request = false;
volatile bool protect_suspended = false;
volatile bool protect_enforce = vdm::kProtectEnforce;
uint8_t currentbound_low_fac = 17;
uint8_t currentbound_high_fac = 17;
uint8_t startOnPower = 50;
uint16_t noOfMinCounts = NO_OF_MIN_COUNTS;
uint8_t maxCalibRetries = 0;

namespace stub {

Motor motor;

namespace {

void resetMotor() {
  motor = Motor();
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    myvalvemots[v].closing_count = 0;
    myvalvemots[v].opening_count = 0;
    myvalvemots[v].deadzone_count = 0;
    myvalvemots[v].scaler = 0;
    myvalvemots[v].meancurrent = 0;
    myvalvemots[v].target_position = 0;
    myvalvemots[v].actual_position = 0;
    myvalvemots[v].status = 0;
    myvalvemots[v].calibration = 0;
    myvalvemots[v].calibTime = 0;
    myvalvemots[v].calibState = calibIdle;
    myvalvemots[v].connected = 0;
    myvalvemots[v].calibRetries = 0;
    myvalvemots[v].calibActive = 0;
    myvalvemots[v].calibrated = 0;
    myvalvemots[v].recal = 0;
    myvalvemots[v].needsReference = 0;
    myvalvemots[v].calibSeq = 0;
    myvalvemots[v].calibFailed = 0;
    myvalvemots[v].earlyLearnDue = 0;
    myvalvemots[v].faultReason = 0;
    myvalvemots[v].moveSeq = 0;
    myvalvemots[v].tripSeq = 0;
  }
  temp_refresh_request = false;
  protect_suspended = false;
  protect_enforce = vdm::kProtectEnforce;
  valvestate = A_INIT;
  valve_loop_ticks = 0;
  valve_loop_stalled = false;
  currentbound_low_fac = 17;
  currentbound_high_fac = 17;
  startOnPower = 50;
  noOfMinCounts = NO_OF_MIN_COUNTS;
  maxCalibRetries = 0;
}

Registrar g_registrar(resetMotor);

}  // namespace

}  // namespace stub

using stub::log;
using stub::motor;

void valve_loop() { log("valve_loop()"); }

byte valve_setup() {
  log("valve_setup()");
  return motor.setup;
}

void valve_pins_safe() { log("valve_pins_safe()"); }

enum ASTATE valve_getstate() {
  log("valve_getstate()");
  return motor.state;
}

bool valve_idle() {
  log("valve_idle()");
  return motor.idle;
}

int16_t appsetaction(char cmd, unsigned int valveindex, byte pos, bool force, uint8_t flags) {
  if (flags != 0) log("appsetaction(%c, %u, %u, %d, 0x%02x)", cmd, valveindex, pos, force ? 1 : 0, flags);
  else log("appsetaction(%c, %u, %u, %d)", cmd, valveindex, pos, force ? 1 : 0);
  return motor.action;
}

int16_t appstop(unsigned int valve) {
  log("appstop(%u)", valve);
  return motor.stop;
}

int valve_busy_index() {
  log("valve_busy_index()");
  return motor.busy;
}

int16_t appsetservice(unsigned int valveindex, uint8_t dir, uint16_t counts, uint8_t maxmA) {
  log("appsetservice(%u, %u, %u, %u)", valveindex, dir, counts, maxmA);
  return motor.service;
}

void valve_get_snapshot(unsigned int valveindex, struct valve_snapshot& out) {
  log("valve_get_snapshot(%u)", valveindex);
  out = valveindex < ACTUATOR_COUNT ? motor.snapshot[valveindex] : valve_snapshot();
}

void valve_get_profile(unsigned int valveindex, vdm::ProfileRecorder& out) {
  log("valve_get_profile(%u)", valveindex);
  out = motor.profile;
}

vdm::MotorParams motor_get_params() {
  log("motor_get_params()");
  return motor.params;
}

void motor_set_params(const vdm::MotorParams& params) {
  log("motor_set_params(%u, %u, %u, %u, %u)", params.lowFac, params.highFac, params.startOnPower,
      params.minCounts, params.maxRetries);
  motor.params = params;
}

vdm::EscalationConfig motor_get_escalation() {
  log("motor_get_escalation()");
  return motor.escalation;
}

void motor_set_escalation(const vdm::EscalationConfig& config) {
  log("motor_set_escalation(%u, %u, %u)", config.enable, config.stepPct, config.maxmA);
  motor.escalation = config;
}
