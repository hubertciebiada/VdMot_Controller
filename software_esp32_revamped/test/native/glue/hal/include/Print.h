// Fake Arduino-ESP32 2.0.7 Print.h / Stream.h base classes (the members the glue and the fake
// libraries use).
#pragma once

#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "WString.h"

#define DEC 10
#define HEX 16
#define OCT 8
#define BIN 2

class Print {
 public:
  virtual ~Print() = default;
  virtual size_t write(uint8_t) = 0;
  virtual size_t write(const uint8_t* buffer, size_t size);
  size_t write(const char* str) {
    return str == nullptr ? 0 : write(reinterpret_cast<const uint8_t*>(str), strlen(str));
  }
  size_t write(const char* buffer, size_t size) {
    return write(reinterpret_cast<const uint8_t*>(buffer), size);
  }
  virtual int availableForWrite() { return 0; }
  virtual void flush() {}

  size_t printf(const char* format, ...) __attribute__((format(printf, 2, 3)));
  size_t print(const String& s) { return write(s.c_str(), s.length()); }
  size_t print(const char* s) { return write(s); }
  size_t print(char c) { return write(static_cast<uint8_t>(c)); }
  size_t print(int n, int base = DEC) { return print(String(n, static_cast<unsigned char>(base))); }
  size_t print(unsigned int n, int base = DEC) {
    return print(String(n, static_cast<unsigned char>(base)));
  }
  size_t print(long n, int base = DEC) {
    return print(String(n, static_cast<unsigned char>(base)));
  }
  size_t print(unsigned long n, int base = DEC) {
    return print(String(n, static_cast<unsigned char>(base)));
  }
  size_t print(double n, int digits = 2) { return print(String(n, static_cast<unsigned>(digits))); }
  size_t println() { return write("\r\n"); }
  template <typename T>
  size_t println(const T& value) {
    const size_t n = print(value);
    return n + println();
  }
  template <typename T>
  size_t println(const T& value, int format) {
    const size_t n = print(value, format);
    return n + println();
  }
};

class Stream : public Print {
 public:
  virtual int available() = 0;
  virtual int read() = 0;
  virtual int peek() = 0;
  void setTimeout(unsigned long timeoutMs) { timeoutMs_ = timeoutMs; }
  unsigned long getTimeout() const { return timeoutMs_; }
  size_t readBytes(char* buffer, size_t length);
  size_t readBytes(uint8_t* buffer, size_t length) {
    return readBytes(reinterpret_cast<char*>(buffer), length);
  }

 protected:
  unsigned long timeoutMs_ = 1000;
};
