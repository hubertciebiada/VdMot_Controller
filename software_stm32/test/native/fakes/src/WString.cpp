// String of the fake core (fixed buffer, see WString.h).
#include "WString.h"

#include <string.h>

namespace {

// digits of value in base (2..36), like the core's utoa/ultoa
void toText(char* out, size_t size, unsigned long value, unsigned char base) {
  char tmp[65];
  size_t n = 0;
  if (base < 2 || base > 36) base = 10;
  do {
    const unsigned d = static_cast<unsigned>(value % base);
    tmp[n++] = static_cast<char>(d < 10 ? '0' + d : 'a' + d - 10);
    value /= base;
  } while (value != 0 && n < sizeof tmp);
  size_t i = 0;
  while (n > 0 && i + 1 < size) out[i++] = tmp[--n];
  out[i] = '\0';
}

}  // namespace

String::String(const char* text) : length_(0) {
  buffer_[0] = '\0';
  append(text);
}

String::String(char c) : length_(0) {
  const char text[2] = {c, '\0'};
  buffer_[0] = '\0';
  append(text);
}

String::String(unsigned char value, unsigned char base) : String(static_cast<unsigned long>(value), base) {}

String::String(int value, unsigned char base) : String(static_cast<long>(value), base) {}

String::String(unsigned int value, unsigned char base) : String(static_cast<unsigned long>(value), base) {}

// base 10 is signed, like the core's ltoa
String::String(long value, unsigned char base) : length_(0) {
  const int32_t v = static_cast<int32_t>(value);
  char text[70];
  if (base == 10 && v < 0) {
    text[0] = '-';
    toText(text + 1, sizeof text - 1, 0ul - static_cast<uint32_t>(v), base);
  } else {
    toText(text, sizeof text, static_cast<uint32_t>(v), base);
  }
  buffer_[0] = '\0';
  append(text);
}

String::String(unsigned long value, unsigned char base) : length_(0) {
  char text[70];
  toText(text, sizeof text, static_cast<uint32_t>(value), base);
  buffer_[0] = '\0';
  append(text);
}

String& String::append(const char* text) {
  if (text == nullptr) return *this;
  const size_t room = sizeof buffer_ - 1 - length_;
  const size_t n = strnlen(text, room);
  memcpy(buffer_ + length_, text, n);
  length_ += static_cast<unsigned int>(n);
  buffer_[length_] = '\0';
  return *this;
}

bool String::operator==(const char* text) const { return text != nullptr && strcmp(buffer_, text) == 0; }

String operator+(const String& a, const String& b) {
  String out(a);
  out += b;
  return out;
}

String operator+(const String& a, const char* b) {
  String out(a);
  out += b;
  return out;
}

String operator+(const char* a, const String& b) {
  String out(a);
  out += b;
  return out;
}
