// STM link task: sole owner of Serial2 and the NRST pin. Runs LinkPolicy,
// PollPlanner, ValveModel, SensorModel, RebootDetector and the StmFlasher;
// executes app::Command; publishes app::StmSnapshot. Nothing else in the
// firmware touches the UART (specs/04 §3.1: one owner task, message passing).
#pragma once

#include <stdint.h>

namespace stm_link {

// Drives BOOT0 LOW, then NRST released (IO15 LOW). The IO15 strap pull-up
// holds the STM in reset from ESP reset until this runs, so it is called
// from the earliest Arduino hook (initVariant), before setup().
void releaseReset();

// Configures the pins (NRST driven LOW = STM keeps running: architecture R6,
// the ESP never resets the STM on its own boot), opens Serial2 8N1.
void begin();

// Task entry (app::kStmTask). Loop, every 2 ms:
//  1. drain app::receive() commands (validate, enqueue at Priority::User or
//     start the flasher),
//  2. read UART bytes into the LineAssembler, parse each line, feed
//     LinkPolicy/models,
//  3. LinkPolicy::poll() timeouts; shouldResetStm() -> NRST pulse,
//  4. ValveModel target pushes/verifies (Priority::Config), PollPlanner
//     (Priority::Poll) when the queue has room,
//  5. write LinkPolicy::nextToSend() to the UART,
//  6. health events, snapshot publish (at most every 100 ms or on change),
//  7. feed the task watchdog.
// While flashing, steps 2-5 are replaced by StmFlasher::step().
void task(void* arg);

// Pulses NRST for 100 ms (task-internal; exposed for tests of the glue).
void pulseReset();

}  // namespace stm_link
