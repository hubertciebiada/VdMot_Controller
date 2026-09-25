// Runner hooks of the STM glue suite (tools/native/testkit): what a reset of the controller keeps.
//   .noinit RAM   kept by pin, software and watchdog resets; 0xA5 after a power-on
//   24LC64        kept by every reset (fake::eeprom.bytes)
//   RCC->CSR      every boot starts with the flags of its reset, added to the flags the firmware
//                 did not clear (RMVF); a power-on starts with PORRSTF | PINRSTF | BORRSTF
// The hooks register themselves; every glue executable links runner_hooks.cpp.
#pragma once

#include <stdint.h>

#include <vector>

namespace glue {

// Copy of the .noinit section of this executable (empty when nothing is placed there).
std::vector<uint8_t> noinitSnapshot();

}  // namespace glue
