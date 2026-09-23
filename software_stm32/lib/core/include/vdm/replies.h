// Formatters for UART replies whose length depends on runtime values.
// Output is byte-identical to protocol v1; the caller appends CR LF.
#pragma once

#include <stddef.h>
#include <stdint.h>

#include "vdm/buf_writer.h"

namespace vdm {

// Fields of the `gvlvd` reply, in wire order after the valve index.
struct ValveDataReply {
  uint32_t index;
  int32_t actualPosition;
  int32_t meanCurrent;
  int32_t status;  // status | 0x80 while calibrating
  int32_t temperature1;
  int32_t temperature2;
  int32_t movements;
  int32_t openingCount;
  int32_t closingCount;
  int32_t deadzoneCount;
  int32_t calibRetries;
};

// Worst case: 5-char prefix, 11 numbers of up to 11 characters, 12 spaces.
constexpr size_t kValveDataReplyMaxLen = 5 + 11 * 11 + 12;

// "<prefix> idx actual mean status t1 t2 movements open close dead retries "
// Writes nothing and returns false if `out` has too little room.
bool formatValveData(BufWriter& out, const char* prefix, const ValveDataReply& r);

// "<prefix> n s0,s1,...,s(n-1) " (for n == 0: "<prefix> 0  ").
// Writes nothing and returns false if `out` has too little room.
bool formatStatusList(BufWriter& out, const char* prefix, const uint8_t* status, size_t n);

}  // namespace vdm
