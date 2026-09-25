#include "stub_log.h"

#include <stdarg.h>
#include <stdio.h>

namespace stub {

Calls calls;

namespace {

std::vector<void (*)()>& resets() {
  static std::vector<void (*)()> list;  // filled at static initialization
  return list;
}

}  // namespace

void log(const char* format, ...) {
  char line[256];
  va_list args;
  va_start(args, format);
  vsnprintf(line, sizeof line, format, args);
  va_end(args);
  calls.push_back(line);
}

Calls callsOf(const std::string& name) {
  Calls out;
  const std::string prefix = name + "(";
  for (const std::string& c : calls) {
    if (c.compare(0, prefix.size(), prefix) == 0) out.push_back(c);
  }
  return out;
}

void reset() {
  for (void (*fn)() : resets()) fn();
  calls.clear();
}

Registrar::Registrar(void (*resetFn)()) { resets().push_back(resetFn); }

}  // namespace stub
