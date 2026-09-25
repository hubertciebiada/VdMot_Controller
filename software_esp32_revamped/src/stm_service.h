// App-task side of the STM link: the scheduled calibration (DESIGN.md
// "Calibration schedule"). Runs in the app task; talks to the STM task only
// through app::submit() and the app.h accessors.
#pragma once

#include <stdint.h>

namespace stm_service {

// app::setup, after the config was loaded: restores the scheduled-calibration
// state (last booked slot, last calibration time).
void begin();

// App task, every second: scheduled calibration (evaluated every 10 s).
void service(uint32_t nowMs);

}  // namespace stm_service
