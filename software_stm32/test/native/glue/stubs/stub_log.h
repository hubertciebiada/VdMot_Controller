// Call log and reset registry of the link-seam stubs (glue/stubs/stub_<module>.cpp). A glue test
// executable links the real src/<stem>.cpp and a stub for every other glue module it calls; each
// stubbed function appends "name(arg, arg)" to stub::calls (integers in decimal, masks in hex) and
// returns a knob of stub_<module>.h. Tests compare whole logs:
//   CHECK(stub::calls == stub::Calls{"app_set_learnmovements(2000)", "eeprom_changed(0x0002)"});
#pragma once

#include <string>
#include <vector>

namespace stub {

using Calls = std::vector<std::string>;
extern Calls calls;

void log(const char* format, ...) __attribute__((format(printf, 1, 2)));
// The entries of one function ("name(" prefix).
Calls callsOf(const std::string& name);
// Every knob and module global of the linked stubs back to its default, the log cleared.
void reset();

// Thrown by a stub whose knob asks it to end an endless loop of the glue (e.g. loop()).
struct Stop {};

// One per stub file: registers the stub's reset function for stub::reset().
struct Registrar {
  explicit Registrar(void (*resetFn)());
};

}  // namespace stub
