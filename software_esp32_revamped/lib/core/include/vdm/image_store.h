// Names and paths of the STM image store on LittleFS (/stm/<name>.bin).
// Hardware-free; storage glue keeps the index and does the file I/O.
#pragma once

#include <stddef.h>

namespace vdm {

constexpr size_t kImageNameMax = 31;

// Accepts "x" or "x.bin" (`len` bytes) and writes the bare name: 1..31 of
// [A-Za-z0-9._-] without a leading '.' (the {name} rule of the HTTP routes).
bool normalizeImageName(const char* in, size_t len, char* out, size_t cap);
// "/stm/<name>.bin" (+ ".part"); false when it does not fit.
bool imagePath(const char* name, bool part, char* out, size_t cap);

}  // namespace vdm
