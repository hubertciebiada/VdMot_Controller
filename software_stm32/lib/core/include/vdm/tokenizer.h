// Command line tokenizer for the text protocols. Hardware-free.
#pragma once

#include <stddef.h>
#include <stdint.h>

namespace vdm {

// Splits a command line in place into a command and its arguments.
// Separators are runs of spaces and tabs, so a trailing separator is optional
// and empty tokens never occur. Every token is NUL-terminated inside the line.
class Tokenizer {
 public:
  static constexpr uint8_t kMaxArgs = 8;

  Tokenizer();

  // Tokenizes `line` (modified in place). Returns false for a null/empty line
  // or when there are more than min(maxArgs, kMaxArgs) arguments; in the
  // latter case command() and the first arguments stay available and
  // tooManyArgs() is true.
  bool parse(char* line, uint8_t maxArgs = kMaxArgs);

  const char* command() const { return command_; }
  // Exact, case-sensitive match of the command token; false without one.
  bool is(const char* name) const;
  uint8_t argc() const { return argc_; }
  bool tooManyArgs() const { return tooMany_; }
  // Argument i, or "" if it does not exist.
  const char* arg(uint8_t i) const;

  // Checked conversions of argument i (see arg_parser.h); false if the
  // argument is missing, malformed or outside [lo, hi]. `out` is only
  // written on success.
  bool argU32(uint8_t i, uint32_t lo, uint32_t hi, uint32_t& out) const;
  bool argI32(uint8_t i, int32_t lo, int32_t hi, int32_t& out) const;
  bool argU16(uint8_t i, uint16_t lo, uint16_t hi, uint16_t& out) const;
  bool argU8(uint8_t i, uint8_t lo, uint8_t hi, uint8_t& out) const;

 private:
  const char* command_;
  const char* args_[kMaxArgs];
  uint8_t argc_;
  bool tooMany_;
};

}  // namespace vdm
