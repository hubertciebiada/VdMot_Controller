// Link-seam stub of src/motor.cpp (include/motor.h): logs every call, returns the knobs of
// stub::motor; the globals of motor.cpp with their real types and start values. The setters store
// into the knobs, so a get after a set returns the new value.
#pragma once

#include "motor.h"
#include "vdm/protection_guard.h"
#include "stub_log.h"

namespace stub {

struct Motor {
  byte setup = 0;
  enum ASTATE state = A_IDLE;
  bool idle = true;
  int16_t action = 0;
  int16_t service = 0;
  int16_t stop = -1;            // appstop()
  int busy = -1;                // valve_busy_index()
  valve_snapshot snapshot[ACTUATOR_COUNT] = {};  // valve_get_snapshot() of valve v
  vdm::ProfileRecorder profile;                 // valve_get_profile() of every valve
  vdm::MotorParams params = {17, 17, 50, NO_OF_MIN_COUNTS, 0};
  vdm::EscalationConfig escalation = vdm::kEscalationDefault;
};
extern Motor motor;

}  // namespace stub
