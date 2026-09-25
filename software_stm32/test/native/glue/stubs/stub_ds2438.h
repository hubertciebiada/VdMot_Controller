// Link-seam stub of the vendored src/DS2438.cpp (not part of the glue suite; its CRC and
// scratchpad checks are vdm::crc8 and vdm::isValidScratchpad of lib/core): the members owDevices.cpp
// uses. readVAD() returns the voltage scripted for the selected address, -10 (the value of a failed
// read) for an address without one.
#pragma once

#include <stdint.h>

#include <map>

#include "DS2438.h"
#include "stub_log.h"

namespace stub {

struct Ds2438 {
  std::map<uint64_t, float> vad;  // key: stub::ds2438Key(address)
  bool begin = true;
};
extern Ds2438 ds2438;

uint64_t ds2438Key(const uint8_t* address);

}  // namespace stub
