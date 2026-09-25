// Fake of the core's Print (framework-arduinoststm32 Print.cpp), byte-exact on the 32-bit target:
// long and unsigned long are formatted as 32-bit values (the host long has 64 bits), base 0 writes
// the raw byte, base 10 is signed, every other base prints the unsigned two's complement
// (print(-1, HEX) is "FFFFFFFF"), a base below 2 prints decimal, print(unsigned char) prints
// digits, println() ends with "\r\n". Glue-facing: no STL.
#pragma once

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
  virtual ~Print() {}
  virtual size_t write(uint8_t c) = 0;
  size_t write(const char* str) {
    if (str == nullptr) return 0;
    return write(reinterpret_cast<const uint8_t*>(str), strlen(str));
  }
  virtual size_t write(const uint8_t* buffer, size_t size);
  size_t write(const char* buffer, size_t size) {
    return write(reinterpret_cast<const uint8_t*>(buffer), size);
  }

  size_t print(const __FlashStringHelper* text);
  size_t print(const String& s);
  size_t print(const char text[]);
  size_t print(char c);
  size_t print(unsigned char b, int base = DEC);
  size_t print(int n, int base = DEC);
  size_t print(unsigned int n, int base = DEC);
  size_t print(long n, int base = DEC);
  size_t print(unsigned long n, int base = DEC);
  size_t print(long long n, int base = DEC);
  size_t print(unsigned long long n, int base = DEC);
  size_t print(float n, int digits = 2);
  size_t print(double n, int digits = 2);

  size_t println(const __FlashStringHelper* text);
  size_t println(const String& s);
  size_t println(const char text[]);
  size_t println(char c);
  size_t println(unsigned char b, int base = DEC);
  size_t println(int n, int base = DEC);
  size_t println(unsigned int n, int base = DEC);
  size_t println(long n, int base = DEC);
  size_t println(unsigned long n, int base = DEC);
  size_t println(long long n, int base = DEC);
  size_t println(unsigned long long n, int base = DEC);
  size_t println(float n, int digits = 2);
  size_t println(double n, int digits = 2);
  size_t println(void);

 private:
  size_t printNumber(uint64_t n, uint8_t base);
  // float and double round in their own precision, like the core's template
  template <class T>
  size_t printFloat(T number, uint8_t digits);
};
