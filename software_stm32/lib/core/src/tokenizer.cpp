#include "vdm/tokenizer.h"

#include <string.h>

#include "vdm/arg_parser.h"

namespace vdm {

namespace {

bool isSeparator(char c) { return c == ' ' || c == '\t'; }

char* skipSeparators(char* p) {
  while (isSeparator(*p)) ++p;
  return p;
}

// Terminates the token starting at p and returns the start of the rest.
char* endToken(char* p) {
  while (*p != '\0' && !isSeparator(*p)) ++p;
  if (*p != '\0') *p++ = '\0';
  return p;
}

}  // namespace

Tokenizer::Tokenizer() : command_(""), args_(), argc_(0), tooMany_(false) {}

bool Tokenizer::parse(char* line, uint8_t maxArgs) {
  command_ = "";
  argc_ = 0;
  tooMany_ = false;
  if (maxArgs > kMaxArgs) maxArgs = kMaxArgs;
  if (line == nullptr) return false;

  char* p = skipSeparators(line);
  if (*p == '\0') return false;
  command_ = p;
  p = endToken(p);

  for (;;) {
    p = skipSeparators(p);
    if (*p == '\0') return true;
    if (argc_ == maxArgs) {
      tooMany_ = true;
      return false;
    }
    args_[argc_++] = p;
    p = endToken(p);
  }
}

bool Tokenizer::is(const char* name) const {
  return name != nullptr && command_[0] != '\0' && strcmp(command_, name) == 0;
}

const char* Tokenizer::arg(uint8_t i) const { return i < argc_ ? args_[i] : ""; }

bool Tokenizer::argU32(uint8_t i, uint32_t lo, uint32_t hi, uint32_t& out) const {
  return i < argc_ && parseU32(args_[i], lo, hi, out);
}

bool Tokenizer::argI32(uint8_t i, int32_t lo, int32_t hi, int32_t& out) const {
  return i < argc_ && parseI32(args_[i], lo, hi, out);
}

bool Tokenizer::argU16(uint8_t i, uint16_t lo, uint16_t hi, uint16_t& out) const {
  uint32_t value = 0;
  if (!argU32(i, lo, hi, value)) return false;
  out = static_cast<uint16_t>(value);
  return true;
}

bool Tokenizer::argU8(uint8_t i, uint8_t lo, uint8_t hi, uint8_t& out) const {
  uint32_t value = 0;
  if (!argU32(i, lo, hi, value)) return false;
  out = static_cast<uint8_t>(value);
  return true;
}

}  // namespace vdm
