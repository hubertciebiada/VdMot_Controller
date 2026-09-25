#include "stub_app.h"

volatile struct valve myvalves[ACTUATOR_COUNT];
unsigned int learning_movements = LEARN_AFTER_MOVEMENTS_DEFAULT;

namespace stub {

App app;

namespace {

unsigned g_loopCalls = 0;

void resetApp() {
  app = App();
  g_loopCalls = 0;
  for (unsigned v = 0; v < ACTUATOR_COUNT; v++) {
    myvalves[v].sensorindex1 = 0;
    myvalves[v].sensorindex2 = 0;
    myvalves[v].learn_time = 0;
    myvalves[v].learn_movements = 0;
    myvalves[v].movements = 0;
    myvalves[v].statusm = 0;
    myvalves[v].cmdRejected = 0;
    myvalves[v].rejectedTarget = 0;
    myvalves[v].forcedLearn = 0;
    myvalves[v].timedLearn = 0;
    myvalves[v].svcHold = 0;
    myvalves[v].retestRequest = 0;
    myvalves[v].openRequest = 0;
  }
  learning_movements = LEARN_AFTER_MOVEMENTS_DEFAULT;
}

Registrar g_registrar(resetApp);

}  // namespace

}  // namespace stub

using stub::app;
using stub::log;

int16_t app_setup(void) {
  log("app_setup()");
  return app.setup;
}

void app_load_config(void) { log("app_load_config()"); }

int16_t app_loop(void) {
  log("app_loop()");
  if (app.loopStopAfter != 0 && ++stub::g_loopCalls >= app.loopStopAfter) throw stub::Stop{};
  return app.loop;
}

byte app_10s_loop(uint32_t elapsedS) {
  log("app_10s_loop(%u)", static_cast<unsigned>(elapsedS));
  return app.tenSecondLoop;
}

int16_t app_set_learnmovements(uint16_t cycles) {
  log("app_set_learnmovements(%u)", cycles);
  return app.setLearnMovements;
}

int16_t app_set_learntime(uint32_t time) {
  log("app_set_learntime(%u)", static_cast<unsigned>(time));
  return app.setLearnTime;
}

int16_t app_set_valvelearning(uint16_t valve) {
  log("app_set_valvelearning(%u)", valve);
  return app.setValveLearning;
}

void app_scan_valves() { log("app_scan_valves()"); }

int16_t app_set_valveopen(uint16_t valve) {
  log("app_set_valveopen(%u)", valve);
  return app.setValveOpen;
}

void app_target_changed(uint16_t valve) { log("app_target_changed(%u)", valve); }

bool app_learn_pending(uint16_t valve, byte status, bool calibration) {
  log("app_learn_pending(%u, %u, %d)", valve, status, calibration ? 1 : 0);
  return app.learnPending;
}

int16_t app_service_move(uint16_t valve, uint8_t dir, uint16_t counts, uint8_t maxmA) {
  log("app_service_move(%u, %u, %u, %u)", valve, dir, counts, maxmA);
  return app.serviceMove;
}

int16_t app_match_sensors() {
  log("app_match_sensors()");
  return app.matchSensors;
}

void reset_check() { log("reset_check()"); }

void reset_STM32() { log("reset_STM32()"); }

void app_1s_tick(uint32_t elapsedS) { log("app_1s_tick(%u)", static_cast<unsigned>(elapsedS)); }

void app_restore(void) { log("app_restore()"); }

void app_warm_save(void) { log("app_warm_save()"); }

void app_lease_poll(void) { log("app_lease_poll()"); }

void app_lease_command(void) { log("app_lease_command()"); }

void app_lease_heartbeat(bool alive) { log("app_lease_heartbeat(%d)", alive ? 1 : 0); }

void app_lease_configure(uint16_t minutes) { log("app_lease_configure(%u)", minutes); }

uint8_t app_lease_state(void) {
  log("app_lease_state()");
  return app.leaseState;
}

uint32_t app_lease_remaining_s(void) {
  log("app_lease_remaining_s()");
  return app.leaseRemainingS;
}

bool app_lease_client(void) {
  log("app_lease_client()");
  return app.leaseClient;
}

uint16_t app_lease_timeout(void) {
  log("app_lease_timeout()");
  return app.leaseTimeout;
}

uint16_t app_failsafe_mask(void) {
  log("app_failsafe_mask()");
  return app.failsafeMask;
}

void app_set_failsafe(uint16_t valve, uint8_t pct) { log("app_set_failsafe(%u, %u)", valve, pct); }

uint8_t app_failsafe_pct(uint16_t valve) {
  log("app_failsafe_pct(%u)", valve);
  return app.failsafePct;
}

int16_t app_stop(uint16_t valve) {
  log("app_stop(%u)", valve);
  return app.stop;
}

uint32_t app_get_learntime(void) {
  log("app_get_learntime()");
  return app.learnTime;
}

void app_temp_cycle_done(void) { log("app_temp_cycle_done()"); }

uint32_t app_temp_age_s(void) {
  log("app_temp_age_s()");
  return app.tempAgeS;
}

bool app_protect_suspended(void) {
  log("app_protect_suspended()");
  return app.protectSuspended;
}

void app_get_valve_v3(uint16_t valve, struct valve_v3_info& out) {
  log("app_get_valve_v3(%u)", valve);
  out = app.valveV3;
}
