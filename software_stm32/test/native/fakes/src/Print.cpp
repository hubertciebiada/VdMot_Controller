// Print of the fake core: the formatting of framework-arduinoststm32 Print.cpp on a 32-bit target.
#include "Print.h"

#include <math.h>

size_t Print::write(const uint8_t* buffer, size_t size) {
  size_t n = 0;
  while (size--) {
    if (write(*buffer++)) {
      n++;
    } else {
      break;
    }
  }
  return n;
}

size_t Print::print(const __FlashStringHelper* text) { return print(reinterpret_cast<const char*>(text)); }

size_t Print::print(const String& s) { return write(s.c_str(), s.length()); }

size_t Print::print(const char text[]) { return write(text); }

size_t Print::print(char c) { return write(static_cast<uint8_t>(c)); }

size_t Print::print(unsigned char b, int base) { return print(static_cast<unsigned long>(b), base); }

size_t Print::print(int n, int base) { return print(static_cast<long>(n), base); }

size_t Print::print(unsigned int n, int base) { return print(static_cast<unsigned long>(n), base); }

// long has 32 bits on the target
size_t Print::print(long n, int base) {
  const int32_t v = static_cast<int32_t>(n);
  if (base == 0) return write(static_cast<uint8_t>(v));
  if (base == 10) {
    if (v < 0) {
      const size_t t = print('-');
      return printNumber(static_cast<uint32_t>(0u - static_cast<uint32_t>(v)), 10) + t;
    }
    return printNumber(static_cast<uint32_t>(v), 10);
  }
  return printNumber(static_cast<uint32_t>(v), static_cast<uint8_t>(base));
}

size_t Print::print(unsigned long n, int base) {
  const uint32_t v = static_cast<uint32_t>(n);
  if (base == 0) return write(static_cast<uint8_t>(v));
  return printNumber(v, static_cast<uint8_t>(base));
}

size_t Print::print(long long n, int base) {
  if (base == 0) return write(static_cast<uint8_t>(n));
  if (base == 10) {
    if (n < 0) {
      const size_t t = print('-');
      return printNumber(0ull - static_cast<unsigned long long>(n), 10) + t;
    }
    return printNumber(static_cast<uint64_t>(n), 10);
  }
  return printNumber(static_cast<uint64_t>(n), static_cast<uint8_t>(base));
}

size_t Print::print(unsigned long long n, int base) {
  if (base == 0) return write(static_cast<uint8_t>(n));
  return printNumber(n, static_cast<uint8_t>(base));
}

size_t Print::print(float n, int digits) { return printFloat(n, static_cast<uint8_t>(digits)); }

size_t Print::print(double n, int digits) { return printFloat(n, static_cast<uint8_t>(digits)); }

size_t Print::println(const __FlashStringHelper* text) { return print(text) + println(); }
size_t Print::println(const String& s) { return print(s) + println(); }
size_t Print::println(const char text[]) { return print(text) + println(); }
size_t Print::println(char c) { return print(c) + println(); }
size_t Print::println(unsigned char b, int base) { return print(b, base) + println(); }
size_t Print::println(int n, int base) { return print(n, base) + println(); }
size_t Print::println(unsigned int n, int base) { return print(n, base) + println(); }
size_t Print::println(long n, int base) { return print(n, base) + println(); }
size_t Print::println(unsigned long n, int base) { return print(n, base) + println(); }
size_t Print::println(long long n, int base) { return print(n, base) + println(); }
size_t Print::println(unsigned long long n, int base) { return print(n, base) + println(); }
size_t Print::println(float n, int digits) { return print(n, digits) + println(); }
size_t Print::println(double n, int digits) { return print(n, digits) + println(); }
size_t Print::println(void) { return write("\r\n"); }

size_t Print::printNumber(uint64_t n, uint8_t base) {
  char buf[65];
  char* str = &buf[sizeof buf - 1];
  *str = '\0';
  // the core's guard against base 1
  if (base < 2) base = 10;
  do {
    const char c = static_cast<char>(n % base);
    n /= base;
    *--str = c < 10 ? static_cast<char>(c + '0') : static_cast<char>(c + 'A' - 10);
  } while (n);
  return write(str);
}

template <class T>
size_t Print::printFloat(T number, uint8_t digits) {
  size_t n = 0;
  if (isnan(number)) return print("nan");
  if (isinf(number)) return print("inf");
  if (number > 4294967040.0) return print("ovf");
  if (number < -4294967040.0) return print("ovf");
  if (number < 0.0) {
    n += print('-');
    number = -number;
  }
  T rounding = 0.5;
  for (uint8_t i = 0; i < digits; ++i) rounding /= 10.0;
  number += rounding;
  const unsigned long intPart = static_cast<unsigned long>(number);
  T remainder = number - static_cast<T>(intPart);
  n += print(intPart);
  if (digits > 0) n += print('.');
  while (digits-- > 0) {
    remainder *= 10.0;
    const unsigned int toPrint = static_cast<unsigned int>(remainder);
    n += print(toPrint);
    remainder -= toPrint;
  }
  return n;
}

template size_t Print::printFloat<float>(float, uint8_t);
template size_t Print::printFloat<double>(double, uint8_t);
