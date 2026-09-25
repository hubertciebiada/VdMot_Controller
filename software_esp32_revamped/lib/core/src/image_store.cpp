#include "vdm/image_store.h"

#include <stdio.h>
#include <string.h>

namespace vdm {

namespace {

bool validNameChar(char c) {
  return (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '.' ||
         c == '_' || c == '-';
}

}  // namespace

bool normalizeImageName(const char* in, size_t len, char* out, size_t cap) {
  if (out == nullptr || cap == 0) return false;
  out[0] = '\0';
  if (in == nullptr) return false;
  if (len >= 4 && memcmp(in + len - 4, ".bin", 4) == 0) len -= 4;
  if (len == 0 || len > kImageNameMax || len >= cap || in[0] == '.') return false;
  for (size_t i = 0; i < len; ++i) {
    if (!validNameChar(in[i])) return false;
  }
  memcpy(out, in, len);
  out[len] = '\0';
  return true;
}

bool imagePath(const char* name, bool part, char* out, size_t cap) {
  const int n = snprintf(out, cap, "/stm/%s.bin%s", name, part ? ".part" : "");
  // An encoding error (n < 0) converts to a huge size and fails the same check.
  return static_cast<size_t>(n) < cap;
}

}  // namespace vdm
