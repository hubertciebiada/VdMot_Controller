// STM link task: sole owner of Serial2 and the NRST pin. Runs vdm::StmSession
// (link policy, planner, models, lease, reset gate, flasher) and implements
// its port; executes app::Command; publishes app::StmSnapshot. Nothing else
// in the firmware touches the UART (one owner task, message passing).
#pragma once

#include <stdint.h>

namespace stm_link {

// Drives BOOT0 LOW, then NRST released (IO15 LOW). The IO15 strap pull-up
// holds the STM in reset from ESP reset until this runs, so it is called
// from the earliest Arduino hook (initVariant), before setup().
void releaseReset();

// Configures the pins (NRST driven LOW = STM keeps running: architecture R6,
// the firmware never drives an STM reset on its own boot; the IO15 strap pull-up can still reset it with jumper X20 fitted), opens Serial2 8N1.
void begin();

// Task entry (app::kStmTask). At start: config, boot targets and lease
// record from stm_service, StmSession::begin(). Loop, every 2 ms:
//  1. feed the task watchdog, re-read a changed config,
//  2. drain app::receive() commands into the session,
//  3. read at most 512 UART bytes into the session, StmSession::poll(),
//     write StmSession::nextToSend() to the UART,
//  4. once per second StmSession::everySecond() with the MQTT regulator
//     state and the STM save state of a pending ESP restart,
//  5. StmSession::publishIfDue().
// While flashing, step 3 is replaced by StmSession::flashStep().
void task(void* arg);

// Pulses NRST for 100 ms (task-internal; exposed for tests of the glue).
void pulseReset();

}  // namespace stm_link
