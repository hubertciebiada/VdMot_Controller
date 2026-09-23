// Strict parsers for protocol arguments. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace vdm {

// Decimal digits only (no sign, no whitespace, no trailing characters),
// overflow-checked, value within [lo, hi]. `out` is written only on success.
bool parseU32(const char* s, uint32_t lo, uint32_t hi, uint32_t& out);

// Optional leading '+' or '-', then decimal digits; otherwise as parseU32.
bool parseI32(const char* s, int32_t lo, int32_t hi, int32_t& out);

// Length of the text form "28-84-37-94-97-ff-03-23" of a 1-Wire ROM address.
constexpr size_t kOneWireAddressTextLen = 23;

// Parses exactly two hex digits (either case) per byte separated by '-'.
// `out` is written only on success.
bool parseOneWireAddress(const char* s, uint8_t (&out)[8]);

// True if all 8 bytes are zero (the protocol's "no sensor" address).
bool isZeroAddress(const uint8_t (&addr)[8]);

}  // namespace vdm
