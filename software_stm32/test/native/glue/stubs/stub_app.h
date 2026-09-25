// Link-seam stub of src/app.cpp (include/app.h): logs every call, returns the knobs of stub::app;
// the globals of app.cpp (myvalves, learning_movements) with their real types.
#pragma once

#include "app.h"
#include "stub_log.h"

namespace stub {

struct App {
  int16_t setup = 0;
  int16_t loop = 0;
  unsigned loopStopAfter = 0;  // app_loop() throws stub::Stop at this call (0: never)
  byte tenSecondLoop = 0;
  int16_t setLearnMovements = 0;
  int16_t setLearnTime = 0;
  int16_t setValveLearning = 0;
  int16_t setValveOpen = 0;
  bool learnPending = false;
  int16_t serviceMove = 0;
  int16_t matchSensors = 0;
  uint8_t leaseState = 0;
  uint32_t leaseRemainingS = 0;
  bool leaseClient = false;
  uint16_t leaseTimeout = 0;
  uint16_t failsafeMask = 0;
  uint8_t failsafePct = 255;
  int16_t stop = 0;
  uint32_t learnTime = 0;
  uint32_t tempAgeS = 0;
  bool protectSuspended = false;
  valve_v3_info valveV3 = {0, 0, 255, 0, 0, 0};
};
extern App app;

}  // namespace stub
