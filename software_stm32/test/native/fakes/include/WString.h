// Fake of the core's String: what the STM glue uses (numbers to text, concatenation, c_str) on a
// fixed buffer of 127 characters (longer results are cut). Glue-facing: no STL.
#pragma once

#include <stddef.h>
#include <stdint.h>

class __FlashStringHelper;
#define F(string_literal) (reinterpret_cast<const __FlashStringHelper*>(string_literal))

class String {
 public:
  String(const char* text = "");
  explicit String(char c);
  explicit String(unsigned char value, unsigned char base = 10);
  explicit String(int value, unsigned char base = 10);
  explicit String(unsigned int value, unsigned char base = 10);
  explicit String(long value, unsigned char base = 10);
  explicit String(unsigned long value, unsigned char base = 10);

  const char* c_str() const { return buffer_; }
  unsigned int length() const { return length_; }
  String& operator+=(const String& other) { return append(other.buffer_); }
  String& operator+=(const char* text) { return append(text); }
  bool operator==(const char* text) const;

 private:
  String& append(const char* text);
  char buffer_[128];
  unsigned int length_;
};

String operator+(const String& a, const String& b);
String operator+(const String& a, const char* b);
String operator+(const char* a, const String& b);
